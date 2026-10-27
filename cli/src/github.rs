// The GitHub App's calls: writes as the app (a PR, replies to and resolves of its review
// threads, auto-merge, a PR's description, a branch's update), so the bot is their author, and the reads around
// them. The key, tokens and transport are in github/app.rs; each call asks it for a token
// narrowed to what the call needs (`Purpose`). A resolve's and an auto-merge's carry
// `contents: write`, which GitHub requires though its docs don't say so.
//
// Nothing here asks Copilot for a review: the ruleset decides that (docs/auto-merge.md).
// Reads use read-only tokens, one per repo. A branch's PRs are read once per run
// (`Branches`). A PR's own record and its branch's PRs are read in github/pr.rs, its
// checks in github/checks.rs, its feedback in github/feedback.rs.

mod app;
mod checks;
mod cost;
mod feedback;
pub mod manifest;
mod pr;

use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ureq::Agent;

pub use app::{App, AppReader, app, app_record, hold, push_token, revoke_held};
pub use checks::{CheckState, Pushed, checks, head_checks};
pub use feedback::feedback;
pub use pr::{BranchPr, Branches, Head, Merged, Pr, PrState, Updated, first_page_pr, merge_pr, read_open_pr, read_pr, update_branch};
#[cfg(test)]
pub use pr::{Comment, Review, Thread, branch_prs};
use cost::site;
use app::{API, Explained, Purpose, Said, as_app, get, graphql, graphql_read, patch, post, refused};
use crate::error::Error;
use crate::http::url_part;
use crate::plain::{Sha, split_repo};
use crate::time::instant;
use crate::json::decode;

// The shapes GitHub's GraphQL answers share. A connection as the query asks for it: `Nodes`
// with no page info, `Newest` read back from the end (`last:`), `Next` read on (`first:`).
#[derive(Clone, Debug, Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Newest<T> {
    nodes: Vec<T>,
    page_info: Before,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Before {
    has_previous_page: bool,
    // Asked only where a page before is read (`connection`).
    #[serde(default)]
    start_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Next<T> {
    nodes: Vec<T>,
    page_info: After,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct After {
    has_next_page: bool,
    // Asked only where a page after is read.
    #[serde(default)]
    end_cursor: Option<String>,
}

// `author { login }`, with `__typename` where the query asks for it. An author is null once
// their account is deleted.
#[derive(Debug, Deserialize)]
struct Author {
    login: String,
    #[serde(rename = "__typename", default)]
    typename: Option<String>,
}

// `repository { nameWithOwner }`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NameWithOwner {
    name_with_owner: String,
}

// `repository { pullRequest { … } }`, the shape of each query of one PR: either is null when
// GitHub has no such repo or PR that the app can read.
#[derive(Debug, Deserialize)]
struct OnePr<T> {
    repository: Option<PrOf<T>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrOf<T> {
    pull_request: Option<T>,
}

impl<T> OnePr<T> {
    fn pr(self) -> Option<T> {
        self.repository?.pull_request
    }
}

// What a mutation of a PR answers: the PR it wrote, null when it wrote none.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Wrote<T> {
    pull_request: Option<T>,
}

pub struct NewPr<'a> {
    pub repo: &'a str,
    pub head: &'a str,
    pub base: &'a str,
    pub title: &'a str,
    pub body: &'a str,
}

// What `tolkster pr create` prints. `existing` when the branch had an open PR already,
// which is used instead: a retry after a create whose answer was lost.
#[derive(Debug, PartialEq, Serialize)]
pub struct Opened {
    pub number: u64,
    pub url: String,
    pub existing: bool,
}

// `as_app` on `repo`, owner/name as --repo gives it, `act` given the token, the owner and
// the name.
fn as_app_on<T>(agent: &Agent, app: &App, repo: &str, purpose: Purpose, now_secs: u64, act: impl FnOnce(&str, &str, &str) -> Result<T, Error>) -> Result<T, Error> {
    let (owner, name) = split_repo(repo).ok_or_else(|| Error::Usage(format!("--repo must be owner/name: {repo}")))?;
    as_app(agent, app, owner, name, purpose, now_secs, |token| act(token, owner, name))
}

pub fn create_pr(agent: &Agent, app: &App, pr: &NewPr, now_secs: u64) -> Result<Opened, Error> {
    let _site = site("create_pr");
    as_app_on(agent, app, pr.repo, Purpose::Create, now_secs, |token, owner, name| open_pr(agent, token, owner, name, pr))
}

fn open_pr(agent: &Agent, token: &str, owner: &str, name: &str, pr: &NewPr) -> Result<Opened, Error> {
    let body = json!({ "title": pr.title, "head": pr.head, "base": pr.base, "body": pr.body });
    let (status, response) = post(agent, &format!("{API}/repos/{owner}/{name}/pulls"), token, body)?;
    if status == 422 && already_open(&response) {
        return existing_pr(agent, token, owner, name, pr.head);
    }
    if status != 201 {
        return Err(refused("the pull request", status, &response));
    }
    Ok(opened(decode(response, "GitHub's answer to POST /repos/{repo}/pulls")?, false))
}

// A PR as REST's create and list answer with it, of which only these are read.
#[derive(Debug, Deserialize)]
struct RestPr {
    number: u64,
    html_url: String,
}

fn already_open(response: &Value) -> bool {
    Said::of(response).errors.iter().filter_map(Explained::message).any(|m| m.starts_with("A pull request already exists"))
}

fn existing_pr(agent: &Agent, token: &str, owner: &str, name: &str, head: &str) -> Result<Opened, Error> {
    let head = if head.contains(':') { head.to_string() } else { format!("{owner}:{head}") };
    let url = format!("{API}/repos/{owner}/{name}/pulls?state=open&head={}", url_part(&head));
    let (status, response) = get(agent, &url, token)?;
    if status != 200 {
        return Err(refused("the branch's open pull request", status, &response));
    }
    let listed: Vec<RestPr> = decode(response, "GitHub's answer to GET /repos/{repo}/pulls")?;
    let open = listed.into_iter().max_by_key(|p| p.number);
    Ok(opened(open.ok_or_else(|| Error::Failed(format!("GitHub said {head} has a pull request, but lists none open")))?, true))
}

fn opened(pr: RestPr, existing: bool) -> Opened {
    Opened { number: pr.number, url: pr.html_url, existing }
}

pub struct PrRef<'a> {
    pub repo: &'a str,
    pub pr: u64,
}

// What `tolkster pr auto-merge` prints. `autoMerge` false: GitHub refused it while the PR is
// mergeable now (`clean`), which `pr auto-merge` fails on and `pr create` asks the machine to merge.
#[derive(Debug, PartialEq, Serialize)]
pub struct AutoMerge {
    pub pr: u64,
    #[serde(rename = "autoMerge")]
    pub auto_merge: bool,
}

// Copilot's review login, as GraphQL gives it.
pub const COPILOT: &str = "copilot-pull-request-reviewer";

pub fn is_copilot(login: Option<&str>) -> bool {
    login.is_some_and(|l| l.eq_ignore_ascii_case(COPILOT))
}

// Whether what `author` wrote on a PR counts as feedback: a blessed account (config.json's
// `review.reviewers`), and never `bot_author`, the bot of the app that opened the PR, even when
// it is listed: its replies are the worker's own.
pub fn blessed(author: Option<&str>, bot_author: Option<&str>, listed: &[String]) -> bool {
    let Some(author) = author.filter(|a| !a.is_empty()) else { return false };
    listed.iter().any(|b| b.eq_ignore_ascii_case(author)) && !bot_author.is_some_and(|b| b.eq_ignore_ascii_case(author))
}

const PR_QUERY: &str = "query($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) { pullRequest(number: $number) { id state autoMergeRequest { enabledAt } } } }";
const AUTO_MERGE_READ: &str = "query($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) { autoMergeAllowed pullRequest(number: $number) { id state mergeStateStatus autoMergeRequest { enabledAt } } } }";
const AUTO_MERGE: &str = "mutation($pr: ID!) { enablePullRequestAutoMerge(input: { pullRequestId: $pr, mergeMethod: SQUASH }) { pullRequest { autoMergeRequest { enabledAt } } } }";

// Turns on auto-merge, squash, for an open PR as the app. A PR with it on already is left
// as it is, so a retry does nothing twice. GitHub refuses it on a PR it could merge now, as
// auto-merge fires only when a requirement changes: that refusal is told from any other by
// the PR read again `CLEAN` in a repo that allows auto-merge, never by its error's text.
pub fn auto_merge(agent: &Agent, app: &App, pr: &PrRef, now_secs: u64) -> Result<AutoMerge, Error> {
    let _site = site("auto_merge");
    as_app_on(agent, app, pr.repo, Purpose::AutoMerge, now_secs, |token, owner, name| {
        let read = || -> Result<PrRead, Error> { decode(graphql_read(agent, token, AUTO_MERGE_READ, json!({ "owner": owner, "name": name, "number": pr.pr }))?, "GitHub's answer to the auto-merge query") };
        if let Some(id) = needs_auto_merge(&read()?, pr)? {
            let data = match graphql(agent, token, AUTO_MERGE, json!({ "pr": id })) {
                Err(Error::Failed(_)) if clean(&read()?, pr)? => return Ok(AutoMerge { pr: pr.pr, auto_merge: false }),
                data => data?,
            };
            if !turned_on(data)? {
                return Err(Error::Failed(format!("GitHub didn't turn on auto-merge for {}#{}", pr.repo, pr.pr)));
            }
        }
        Ok(AutoMerge { pr: pr.pr, auto_merge: true })
    })
}

// PR_QUERY's answer, and AUTO_MERGE_READ's, which adds whether the repo allows auto-merge
// and what GitHub makes of merging the PR now.
#[derive(Debug, Deserialize)]
struct PrRead {
    repository: Option<PrRepository>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrRepository {
    #[serde(default)]
    auto_merge_allowed: Option<bool>,
    pull_request: Option<Merging>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Merging {
    id: String,
    state: PrState,
    #[serde(default)]
    merge_state_status: Option<String>,
    // Null while auto-merge is off.
    auto_merge_request: Option<IgnoredAny>,
}

// Whether AUTO_MERGE's answer has auto-merge on.
fn turned_on(data: Value) -> Result<bool, Error> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Enabled {
        enable_pull_request_auto_merge: Option<Wrote<AutoMergeOf>>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct AutoMergeOf {
        auto_merge_request: Option<IgnoredAny>,
    }
    let enabled: Enabled = decode(data, "GitHub's answer to the auto-merge mutation")?;
    Ok(enabled.enable_pull_request_auto_merge.and_then(|wrote| wrote.pull_request).is_some_and(|p| p.auto_merge_request.is_some()))
}

// Whether an open PR without auto-merge is one GitHub could merge now, in a repo that allows
// auto-merge: one that doesn't refuses it for that, and its PRs are the owner's to merge.
fn clean(data: &PrRead, pr: &PrRef) -> Result<bool, Error> {
    let (_, on) = an_open_pr(data, pr)?;
    let repository = data.repository.as_ref();
    let allowed = repository.and_then(|r| r.auto_merge_allowed) == Some(true);
    let status = repository.and_then(|r| r.pull_request.as_ref()).and_then(|p| p.merge_state_status.as_deref());
    Ok(allowed && !on && status == Some("CLEAN"))
}

// The PR's node ID when auto-merge is still to be turned on, None when it is on already.
// Only an open PR can have it.
fn needs_auto_merge(data: &PrRead, pr: &PrRef) -> Result<Option<String>, Error> {
    let (id, on) = an_open_pr(data, pr)?;
    Ok((!on).then_some(id))
}

// An open PR's node ID and whether auto-merge is on, from PR_QUERY's answer. A PR that
// isn't open is refused: nothing here changes one.
fn an_open_pr(data: &PrRead, pr: &PrRef) -> Result<(String, bool), Error> {
    let Some(found) = data.repository.as_ref().and_then(|r| r.pull_request.as_ref()) else {
        return Err(Error::Failed(format!("{}#{} is not a pull request", pr.repo, pr.pr)));
    };
    let ended = match found.state {
        PrState::Open => None,
        PrState::Merged => Some("merged"),
        PrState::Closed => Some("closed"),
    };
    if let Some(ended) = ended {
        return Err(Error::Failed(format!("{}#{} is not open: {ended}", pr.repo, pr.pr)));
    }
    Ok((found.id.clone(), found.auto_merge_request.is_some()))
}

// What `tolkster pr describe` prints.
#[derive(Debug, PartialEq, Serialize)]
pub struct Described {
    pub pr: u64,
    pub described: bool,
}

// Replaces an open PR's description whole; nothing else of the PR changes. A PR that isn't
// open is refused: its description is history. The same body again changes nothing, so a
// retry is safe.
pub fn describe(agent: &Agent, app: &App, pr: &PrRef, body: &str, now_secs: u64) -> Result<Described, Error> {
    let _site = site("describe");
    as_app_on(agent, app, pr.repo, Purpose::Describe, now_secs, |token, owner, name| {
        let read: PrRead = decode(graphql(agent, token, PR_QUERY, json!({ "owner": owner, "name": name, "number": pr.pr }))?, "GitHub's answer to the PR query")?;
        an_open_pr(&read, pr)?;
        let (status, response) = patch(agent, &format!("{API}/repos/{owner}/{name}/pulls/{}", pr.pr), token, json!({ "body": body }))?;
        if status != 200 {
            return Err(refused("the pull request's description", status, &response));
        }
        Ok(Described { pr: pr.pr, described: true })
    })
}

// A PR's connections are read in pages of this many, newest first, and at most MAX_PAGES
// of each: a PR with more fails rather than hide any.
const PAGE: usize = 100;
const MAX_PAGES: usize = 50;

// One connection of a PR in a query: its newest PAGE nodes, or with `before`, the page
// before `$before`, each with what `read_older` pages by.
fn connection(name: &str, fields: &str, before: bool) -> String {
    let before = if before { ", before: $before" } else { "" };
    format!("{name}(last: {PAGE}{before}) {{ nodes {{ {fields} }} pageInfo {{ hasPreviousPage startCursor }} }}")
}

// Reads into `first`, connection `name` of PR `number` of `repo` as a query with
// `connection` returned it, the rest of it the first page cut short, oldest first, so
// nothing beyond the newest PAGE of anything is silently missed.
fn read_older<T: DeserializeOwned>(gh: &mut dyn Reader, repo: &str, number: u64, (name, fields): (&str, &str), first: &mut Newest<T>) -> Result<(), Error> {
    // The page before, under one name whatever the connection.
    #[derive(Deserialize)]
    struct Older<U> {
        page: Newest<U>,
    }
    let query = format!(
        "query($owner: String!, $name: String!, $number: Int!, $before: String!) {{ repository(owner: $owner, name: $name) {{ pullRequest(number: $number) {{ page: {} }} }} }}",
        connection(name, fields, true)
    );
    let mut pages = 1;
    while first.page_info.has_previous_page {
        if pages >= MAX_PAGES {
            return Err(Error::Failed(format!("{repo}#{number} has over {} {name}", MAX_PAGES * PAGE)));
        }
        pages += 1;
        let cursor = first.page_info.start_cursor.clone().unwrap_or_default();
        let read: OnePr<Older<T>> = decode(gh.graphql(repo, &query, repo_variables(repo, json!({ "number": number, "before": cursor }))?)?, "GitHub's answer to the older page query")?;
        let page = read.pr().ok_or_else(|| Error::Failed(format!("GitHub's response had no {name} for {repo}#{number}")))?.page;
        let newer = std::mem::replace(&mut first.nodes, page.nodes);
        first.nodes.extend(newer);
        first.page_info = page.page_info;
    }
    Ok(())
}

// A review thread on one PR, for `tolkster pr reply` and `tolkster pr resolve`.
pub struct ThreadRef<'a> {
    pub repo: &'a str,
    pub pr: u64,
    // The thread's GraphQL node ID, e.g. PRRT_kwDO….
    pub thread: &'a str,
}

// What `tolkster pr reply` prints: the new comment, as GitHub answers with it.
#[derive(Debug, PartialEq, Deserialize, Serialize)]
pub struct Replied {
    pub id: String,
    pub url: String,
}

// What `tolkster pr resolve` prints.
#[derive(Debug, PartialEq, Serialize)]
pub struct Resolved {
    pub thread: String,
    pub resolved: bool,
}

pub fn is_node_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 200 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '='))
}

const THREAD_QUERY: &str = "query($id: ID!) { node(id: $id) { ... on PullRequestReviewThread { isResolved pullRequest { number repository { nameWithOwner } } } } }";
const REPLY: &str = "mutation($thread: ID!, $body: String!) { addPullRequestReviewThreadReply(input: { pullRequestReviewThreadId: $thread, body: $body }) { comment { id url } } }";
const RESOLVE: &str = "mutation($thread: ID!) { resolveReviewThread(input: { threadId: $thread }) { thread { isResolved } } }";

// Runs `act` as the app, with a token for `purpose`, once the thread is known to be on
// that PR, and with whether it is resolved.
fn on_thread<T>(agent: &Agent, app: &App, thread: &ThreadRef, purpose: Purpose, now_secs: u64, act: impl FnOnce(&str, bool) -> Result<T, Error>) -> Result<T, Error> {
    as_app_on(agent, app, thread.repo, purpose, now_secs, |token, _, _| {
        let read: ThreadRead = decode(graphql(agent, token, THREAD_QUERY, json!({ "id": thread.thread }))?, "GitHub's answer to the thread query")?;
        let resolved = check_thread(read, thread)?;
        act(token, resolved)
    })
}

pub fn reply(agent: &Agent, app: &App, thread: &ThreadRef, body: &str, now_secs: u64) -> Result<Replied, Error> {
    let _site = site("reply");
    on_thread(agent, app, thread, Purpose::Reply, now_secs, |token, _| replied(graphql(agent, token, REPLY, json!({ "thread": thread.thread, "body": body }))?))
}

// Resolves a review thread as the app. A thread that is resolved already is left as it
// is, so a retry does nothing twice.
pub fn resolve(agent: &Agent, app: &App, thread: &ThreadRef, now_secs: u64) -> Result<Resolved, Error> {
    let _site = site("resolve");
    on_thread(agent, app, thread, Purpose::Resolve, now_secs, |token, resolved| {
        if !resolved && !now_resolved(graphql(agent, token, RESOLVE, json!({ "thread": thread.thread }))?)? {
            return Err(Error::Failed(format!("GitHub didn't resolve review thread {}", thread.thread)));
        }
        Ok(Resolved { thread: thread.thread.into(), resolved: true })
    })
}

// THREAD_QUERY's answer: the node, null when GitHub has none by that ID, and with none of
// the fragment's fields when it is a node of another type.
#[derive(Debug, Deserialize)]
struct ThreadRead {
    node: Option<ThreadNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadNode {
    #[serde(default)]
    is_resolved: Option<bool>,
    #[serde(default)]
    pull_request: Option<ThreadPr>,
}

#[derive(Debug, Deserialize)]
struct ThreadPr {
    number: u64,
    repository: NameWithOwner,
}

// Whether RESOLVE's answer has the thread resolved.
fn now_resolved(data: Value) -> Result<bool, Error> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Resolving {
        resolve_review_thread: Option<ResolvedThread>,
    }
    #[derive(Deserialize)]
    struct ResolvedThread {
        thread: Option<IsResolved>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct IsResolved {
        is_resolved: bool,
    }
    let data: Resolving = decode(data, "GitHub's answer to the resolve mutation")?;
    Ok(data.resolve_review_thread.and_then(|r| r.thread).is_some_and(|t| t.is_resolved))
}

// Whether the thread is resolved, once it is known to be on `thread.pr` in
// `thread.repo`. The token reaches only that repo; this keeps a write off another PR.
fn check_thread(data: ThreadRead, thread: &ThreadRef) -> Result<bool, Error> {
    let resolved = data.node.and_then(|n| {
        let pr = n.pull_request?;
        let resolved = n.is_resolved?;
        (pr.number == thread.pr && pr.repository.name_with_owner.eq_ignore_ascii_case(thread.repo)).then_some(resolved)
    });
    resolved.ok_or_else(|| Error::Failed(format!("{} is not a review thread on {}#{}", thread.thread, thread.repo, thread.pr)))
}

fn replied(data: Value) -> Result<Replied, Error> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Replying {
        add_pull_request_review_thread_reply: Option<Reply>,
    }
    #[derive(Deserialize)]
    struct Reply {
        comment: Option<Replied>,
    }
    let data: Replying = decode(data, "GitHub's answer to the reply mutation")?;
    data.add_pull_request_review_thread_reply.and_then(|r| r.comment).ok_or_else(|| Error::Failed("GitHub's reply response had no comment".into()))
}

// `owner/name` of a GitHub remote's URL, over HTTPS or SSH.
pub fn remote_repo(url: &str) -> Option<String> {
    let url = url.trim();
    let path = ["https://github.com/", "http://github.com/", "ssh://git@github.com/", "git@github.com:"].iter().find_map(|prefix| url.strip_prefix(prefix))?;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    (!owner.is_empty() && !name.is_empty() && !name.contains('/')).then(|| format!("{owner}/{name}"))
}

// Reads GitHub for one repo (`owner/name`) at a time. `AppReader` reads as the app; tests
// read from a fake.
pub trait Reader {
    // A GraphQL query's `data`.
    fn graphql(&mut self, repo: &str, query: &str, variables: Value) -> Result<Value, Error>;
    // A write: GitHub merges PR `pr`'s (its node ID) base into its branch, only while
    // its head is `head` (`pr::UPDATE`), with a token that can push. GitHub's whole answer,
    // its `data` or the `errors` it refused the update with; Err for a call that failed.
    fn update_branch(&mut self, repo: &str, pr: &str, head: &Sha) -> Result<Value, Error>;
    // The other write: GitHub squash-merges PR `pr` only while its head is `head` (`pr::MERGE`),
    // with a token that can merge. Its whole answer, as `update_branch`'s.
    fn merge_pr(&mut self, repo: &str, pr: &str, head: &Sha) -> Result<Value, Error>;
    // A REST GET under the repo, e.g. `pulls/7`.
    fn get(&mut self, repo: &str, path: &str) -> Result<Value, Error>;
    // The app's own record, `GET /app`, read with its JWT rather than a repo's token.
    fn app(&mut self) -> Result<AppRecord, Error>;
}

// The Worker's record of the GitHub App (src/worker/mint.ts, `appRecord`): the events it is
// subscribed to, its slug, its installations, and whether GITHUB_INSTALLATION_ID is one of
// them. A Worker deployed before it listed installations has neither.
#[derive(Debug, Deserialize)]
pub struct AppRecord {
    pub events: Vec<String>,
    pub slug: Option<String>,
    #[serde(default)]
    pub installations: Option<Vec<u64>>,
    #[serde(default)]
    pub installed: Option<bool>,
}

// The webhook events the Worker takes, as `GITHUB_EVENTS` in src/worker/github.ts lists
// them: a test holds the two the same.
pub const GITHUB_EVENTS: [&str; 8] = ["pull_request", "pull_request_review", "pull_request_review_comment", "pull_request_review_thread", "issue_comment", "check_suite", "push", "status"];

fn login(author: &Option<Author>) -> Option<&str> {
    author.as_ref().map(|a| a.login.as_str())
}

fn is_bot(author: &Option<Author>) -> bool {
    author.as_ref().is_some_and(|a| a.typename.as_deref() == Some("Bot"))
}

// The PR's author when it is a bot: never blessed (`blessed`).
fn bot_author(author: &Option<Author>) -> Option<&str> {
    login(author).filter(|_| is_bot(author))
}

fn has_body(body: &str) -> bool {
    !body.trim().is_empty()
}

// When a review thread's comment reached the PR's readers: `publishedAt`, the moment its
// review was submitted, not `createdAt`, when it was drafted, which a review held back
// until after another round would put before that round. Null while pending, so a comment
// without one counts from `createdAt`.
fn published(published_at: Option<&str>, created_at: &str) -> Option<i64> {
    published_at.and_then(instant).or_else(|| instant(created_at))
}

// An active rule on a branch, as `GET /repos/{repo}/rules/branches/{branch}` lists them: its
// type, and of its parameters, which are each type's own, Copilot's alone.
#[derive(Debug, Deserialize)]
struct Rule {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    parameters: Option<RuleParameters>,
}

#[derive(Debug, Deserialize)]
struct RuleParameters {
    #[serde(default)]
    review_on_push: Option<bool>,
}

const RULES: &str = "GitHub's answer to GET /repos/{repo}/rules/branches/{branch}";

// The rule type the ruleset's "Automatically request Copilot code review" adds.
pub const COPILOT_RULE: &str = "copilot_code_review";

// How a ruleset has Copilot review the PRs opened against a branch (`copilot_reviews`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopilotReview {
    // No Copilot rule: nothing looks at a PR but its worker's own review.
    Off,
    // The rule with its "review on push" off: Copilot reviews a PR as it opens, and
    // nothing looks at a push after that but the worker.
    OnOpening,
    // The rule with "review on push" on: Copilot reviews the opening and every push.
    OnEachPush,
}

impl CopilotReview {
    // Whether Copilot reviews a PR at all, on its opening.
    pub fn opening(self) -> bool {
        self != CopilotReview::Off
    }
}

// How the ruleset on `base` of `repo` (`owner/name`) has Copilot review its PRs: whether
// the active rules on the branch have one of type COPILOT_RULE, and whether its
// `parameters.review_on_push` is on. The endpoint needs Metadata: read alone, which every
// app has. Only the rules' types and that one parameter are read. A rule without the
// parameter counts as opening only, which errs towards the worker's own review of a push.
pub fn copilot_reviews(gh: &mut dyn Reader, repo: &str, base: &str) -> Result<CopilotReview, Error> {
    let _site = site("copilot_reviews");
    let rules: Vec<Rule> = decode(gh.get(repo, &format!("rules/branches/{base}"))?, RULES)?;
    let copilot: Vec<&Rule> = rules.iter().filter(|rule| rule.kind == COPILOT_RULE).collect();
    let on_push = |rule: &&Rule| rule.parameters.as_ref().and_then(|p| p.review_on_push) == Some(true);
    Ok(if copilot.is_empty() {
        CopilotReview::Off
    } else if copilot.iter().any(on_push) {
        CopilotReview::OnEachPush
    } else {
        CopilotReview::OnOpening
    })
}

// What `tolkster doctor` reads of a repo, as the app: its default branch (None for an
// empty repo), whether it allows auto-merge, whether that branch requires a status check
// (a ruleset's `required_status_checks` rule, or classic branch protection's), and
// whether it has an `AGENTS.md` or a `CLAUDE.md` at its root. Only names and flags are
// read, never a file's text. A branch's rules and protection are read apart from the rest
// (GitHub refuses rulesets on some plans), so `required_check` holds why it couldn't be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Readiness {
    pub default_branch: Option<String>,
    pub auto_merge: bool,
    pub required_check: Result<bool, String>,
    pub instructions: bool,
}

const READINESS: &str = "query($owner: String!, $name: String!) { repository(owner: $owner, name: $name) { autoMergeAllowed defaultBranchRef { name } agents: object(expression: \"HEAD:AGENTS.md\") { __typename } claude: object(expression: \"HEAD:CLAUDE.md\") { __typename } } }";

// The rule type a ruleset's "Require status checks to pass" adds.
const REQUIRED_CHECKS_RULE: &str = "required_status_checks";

// The repo `repo` (`owner/name`) as `tolkster doctor` checks it. The reader's first call
// mints its token, so an app that isn't installed on the repo fails here.
pub fn readiness(gh: &mut dyn Reader, repo: &str) -> Result<Readiness, Error> {
    let _site = site("readiness");
    #[derive(Deserialize)]
    struct Read {
        repository: Option<Repository>,
    }
    // `agents` and `claude` are the root's AGENTS.md and CLAUDE.md, null when it has none.
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Repository {
        auto_merge_allowed: bool,
        default_branch_ref: Option<BranchName>,
        agents: Option<IgnoredAny>,
        claude: Option<IgnoredAny>,
    }
    #[derive(Deserialize)]
    struct BranchName {
        name: String,
    }
    let read: Read = decode(gh.graphql(repo, READINESS, repo_variables(repo, json!({}))?)?, "GitHub's answer to the readiness query")?;
    let repository = read.repository.ok_or_else(|| Error::Failed(format!("GitHub has no repository {repo} that the app can read")))?;
    let default_branch = repository.default_branch_ref.map(|b| b.name);
    let required_check = match &default_branch {
        Some(branch) => required_check(gh, repo, branch).map_err(|error| error.to_string()),
        None => Ok(false),
    };
    Ok(Readiness { auto_merge: repository.auto_merge_allowed, instructions: repository.agents.is_some() || repository.claude.is_some(), required_check, default_branch })
}

// Whether `branch` requires a status check: an active rule on it of REQUIRED_CHECKS_RULE,
// or else classic protection with a required check whose enforcement isn't off. Rules
// that can't be read still leave the classic protection to read: Err only when that
// finds no check either.
fn required_check(gh: &mut dyn Reader, repo: &str, branch: &str) -> Result<bool, Error> {
    // `GET /repos/{repo}/branches/{branch}`'s classic protection, of which only its required
    // checks are read: null or missing when it has none.
    #[derive(Deserialize)]
    struct Branch {
        protection: Option<Protection>,
    }
    #[derive(Deserialize)]
    struct Protection {
        required_status_checks: Option<RequiredChecks>,
    }
    #[derive(Deserialize)]
    struct RequiredChecks {
        enforcement_level: Option<String>,
        #[serde(default)]
        contexts: Vec<IgnoredAny>,
        #[serde(default)]
        checks: Vec<IgnoredAny>,
    }
    let branch = url_part(branch);
    let rules = gh.get(repo, &format!("rules/branches/{branch}?per_page=100")).and_then(|rules| decode::<Vec<Rule>>(rules, RULES));
    if rules.as_ref().is_ok_and(|rules| rules.iter().any(|rule| rule.kind == REQUIRED_CHECKS_RULE)) {
        return Ok(true);
    }
    let classic: Branch = decode(gh.get(repo, &format!("branches/{branch}"))?, "GitHub's answer to GET /repos/{repo}/branches/{branch}")?;
    let required = classic.protection.and_then(|p| p.required_status_checks).is_some_and(|checks| {
        let enforced = checks.enforcement_level.as_deref() != Some("off");
        enforced && (!checks.contexts.is_empty() || !checks.checks.is_empty())
    });
    match rules {
        Err(error) if !required => Err(error),
        _ => Ok(required),
    }
}

fn repo_variables(repo: &str, extra: Value) -> Result<Value, Error> {
    let (owner, name) = split_repo(repo).ok_or_else(|| Error::Usage(format!("not a GitHub repo, owner/name: {repo}")))?;
    let mut variables = json!({ "owner": owner, "name": name });
    if let (Some(all), Value::Object(more)) = (variables.as_object_mut(), extra) {
        all.extend(more);
    }
    Ok(variables)
}

#[cfg(test)]
mod tests;
