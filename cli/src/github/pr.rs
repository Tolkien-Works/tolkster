// A PR as GitHub has it: the PRs on a branch (`branch_prs`), and one PR by number
// (`read_pr`) with what the review resume reads of an open one (`read_open_pr`).

use std::collections::HashMap;

use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::json;

use super::app::Answer;
use super::cost::site;
use super::{Author, NameWithOwner, Newest, Next, Nodes, OnePr, Reader, Wrote, bot_author, connection, has_body, is_bot, login, published, read_older, repo_variables};
use crate::error::Error;
use crate::json::decode;
use crate::plain::Sha;
use crate::time::instant;
use crate::trace;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

// One PR of a branch, as `branch_prs` reads every one: its number, state and head commit.
// What reap decides on (reap::verdict).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchPr {
    pub number: u64,
    pub state: PrState,
    pub head: String,
}

// What the review resume (review.rs) reads of an open PR's reviews, review threads and
// conversation comments, and nothing more: authors, states, times and commits, and
// whether a review has a body, never the text. GraphQL throughout, so Copilot has one
// login. The first page of each comes with the PR, the rest with `read_open_pr`, so
// the usual PR costs one query.
const REVIEW_CONNECTIONS: [(&str, &str); 3] = [
    ("reviews", "author { __typename login } state submittedAt body commit { oid }"),
    ("reviewThreads", "isResolved isOutdated comments(last: 1) { nodes { author { login } createdAt publishedAt } }"),
    ("comments", "author { login } createdAt"),
];

// What the review resume reads of an open PR, in a query: the fields `parse_pr` reads and
// the first page of each review connection, as `read_pr` reads it.
fn open_pr_fields() -> String {
    let connections: Vec<String> = REVIEW_CONNECTIONS.iter().map(|(name, fields)| connection(name, fields, false)).collect();
    format!(
        "number headRefOid mergeStateStatus baseRefName author {{ __typename login }} repository {{ nameWithOwner }}
        autoMergeRequest {{ enabledAt }} timelineItems(itemTypes: [AUTO_MERGE_DISABLED_EVENT], last: 1) {{ nodes {{ __typename }} }}
        baseRefOid baseRef {{ target {{ ... on Commit {{ oid committedDate }} }} }}
        commits(last: 1) {{ nodes {{ commit {{ oid statusCheckRollup {{ state }} }} }} }}
        {}",
        connections.join("\n        ")
    )
}

// One light query for a branch: every PR on it, with no review field, which costs about one
// point. A list with more PRs than a page fails rather than be judged on some of them. A
// fork's PRs share the branch's name and are left out by `parse_branch_prs`.
pub(super) fn branch_prs_query() -> String {
    format!(
        "query($owner: String!, $name: String!, $branch: String!) {{
  repository(owner: $owner, name: $name) {{
    all: {}
  }}
}}",
        pull_requests("$branch", BRANCH_PR_FIELDS)
    )
}

const BRANCH_PR_FIELDS: &str = "number state headRefOid isCrossRepository";

// The PRs on the branch named by the variable `branch`, newest first, one page.
fn pull_requests(branch: &str, fields: &str) -> String {
    format!("pullRequests(headRefName: {branch}, first: 100, orderBy: {{ field: CREATED_AT, direction: DESC }}) {{ nodes {{ {fields} }} pageInfo {{ hasNextPage }} }}")
}

// `branch_prs_query`'s answer.
#[derive(Debug, Deserialize)]
struct BranchPrsRead {
    repository: Option<AllPrs>,
}

#[derive(Debug, Deserialize)]
struct AllPrs {
    all: Next<BranchPrNode>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BranchPrNode {
    number: u64,
    state: PrState,
    head_ref_oid: String,
    is_cross_repository: bool,
}

// One query for the PR `evaluatePr` names: what its kind is decided
// by (`Head`), and what the review resume reads of an open one (`open_pr_fields`), so an
// open PR is read once for both.
pub(super) fn pr_query() -> String {
    format!(
        "query($owner: String!, $name: String!, $number: Int!) {{
  repository(owner: $owner, name: $name) {{
    pullRequest(number: $number) {{
      state mergedAt closedAt updatedAt headRefName headRepository {{ nameWithOwner }}
      {}
    }}
  }}
}}",
        open_pr_fields()
    )
}

// Every PR for `branch` in `repo`, newest first, in one read with nothing of their reviews
// (`read_pr` reads those of one PR, by number). A PR whose head isn't in `repo` is left out:
// GitHub matches a fork's PR by branch name too, and a worker's branch is never the
// stranger's.
pub fn branch_prs(gh: &mut dyn Reader, repo: &str, branch: &str) -> Result<Vec<BranchPr>, Error> {
    let _site = site("branch_prs");
    let data = gh.graphql(repo, &branch_prs_query(), repo_variables(repo, json!({ "branch": branch }))?)?;
    parse_branch_prs(decode(data, "GitHub's answer to the branch's PRs query")?, repo, branch)
}

fn parse_branch_prs(data: BranchPrsRead, repo: &str, branch: &str) -> Result<Vec<BranchPr>, Error> {
    let Some(list) = data.repository.map(|r| r.all) else {
        return Err(Error::Failed(format!("GitHub had no pull requests for {repo}")));
    };
    if list.page_info.has_next_page {
        return Err(Error::Failed(format!("branch {branch} has over 100 pull requests in {repo}")));
    }
    Ok(list.nodes.into_iter().filter(|pr| !pr.is_cross_repository).map(|pr| BranchPr { number: pr.number, state: pr.state, head: pr.head_ref_oid }).collect())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Review {
    pub author: Option<String>,
    pub bot: bool,
    pub state: String,
    pub at: Option<i64>,
    pub has_body: bool,
    // The PR head it reviewed.
    pub commit: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Thread {
    pub resolved: bool,
    pub outdated: bool,
    // Its latest comment's author and time.
    pub author: Option<String>,
    pub at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    pub author: Option<String>,
    pub at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Pr {
    pub number: u64,
    // `owner/name`.
    pub repo: String,
    pub head: String,
    pub merge_state: String,
    pub base: String,
    // The base branch's head commit as read, or, its branch gone, the base GitHub last
    // recorded for the PR. Empty when GitHub gave neither.
    pub base_head: String,
    // When the base branch's head commit was made.
    pub base_at: Option<i64>,
    // The PR's author when it is a bot: the GitHub App's, which opened it. Its replies
    // are the worker's own, so they never count as feedback.
    pub bot_author: Option<String>,
    // GitHub rolls every check on the head up as `SUCCESS`, required or not: none failed, none
    // waits, none needs a person, so `review::failed_checks` has nothing to read.
    pub checks_green: bool,
    // Auto-merge is off now, and someone or something turned it off: it was on once.
    pub auto_merge_off: bool,
    // Auto-merge is on now: GitHub merges the PR once its required checks pass.
    pub auto_merge_on: bool,
    pub reviews: Vec<Review>,
    pub threads: Vec<Thread>,
    pub comments: Vec<Comment>,
}

// The PR as `pr_query` reads it: what its `Head` is read from, then the fields `parse_pr`
// reads and the first page of each of REVIEW_CONNECTIONS.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrNode {
    state: PrState,
    merged_at: Option<String>,
    closed_at: Option<String>,
    updated_at: String,
    head_ref_name: String,
    // Null once a fork is deleted.
    head_repository: Option<NameWithOwner>,
    number: u64,
    head_ref_oid: String,
    merge_state_status: String,
    base_ref_name: String,
    author: Option<Author>,
    repository: NameWithOwner,
    auto_merge_request: Option<IgnoredAny>,
    timeline_items: Nodes<IgnoredAny>,
    base_ref_oid: String,
    // Null once the base branch is gone.
    base_ref: Option<BaseRef>,
    commits: Nodes<HeadCommit>,
    reviews: Newest<ReviewNode>,
    review_threads: Newest<ThreadNode>,
    comments: Newest<CommentNode>,
}

#[derive(Debug, Deserialize)]
struct BaseRef {
    target: Option<BaseTarget>,
}

// The fragment `... on Commit`: none of its fields for a target of another type.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BaseTarget {
    #[serde(default)]
    oid: Option<String>,
    #[serde(default)]
    committed_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HeadCommit {
    commit: Rollup,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rollup {
    oid: String,
    // Null for a commit with no checks.
    status_check_rollup: Option<RollupState>,
}

#[derive(Debug, Deserialize)]
struct RollupState {
    state: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewNode {
    author: Option<Author>,
    state: String,
    submitted_at: Option<String>,
    body: String,
    commit: Option<Oid>,
}

#[derive(Debug, Deserialize)]
struct Oid {
    oid: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadNode {
    is_resolved: bool,
    is_outdated: bool,
    comments: Nodes<LastComment>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LastComment {
    author: Option<Author>,
    created_at: String,
    published_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommentNode {
    author: Option<Author>,
    created_at: String,
}

// One open PR's node, as `read_pr`, and then `read_open_pr`, read it.
fn parse_pr(pr: &PrNode) -> Pr {
    let base_target = pr.base_ref.as_ref().and_then(|b| b.target.as_ref());
    Pr {
        number: pr.number,
        repo: pr.repository.name_with_owner.clone(),
        head: pr.head_ref_oid.clone(),
        merge_state: pr.merge_state_status.clone(),
        base: pr.base_ref_name.clone(),
        base_head: base_target.and_then(|t| t.oid.clone()).unwrap_or_else(|| pr.base_ref_oid.clone()),
        base_at: base_target.and_then(|t| t.committed_date.as_deref()).and_then(instant),
        bot_author: bot_author(&pr.author).map(String::from),
        checks_green: rolled_up_green(pr),
        // The event's node, not `totalCount`, which counts every timeline item whatever the filter.
        auto_merge_off: pr.auto_merge_request.is_none() && !pr.timeline_items.nodes.is_empty(),
        auto_merge_on: pr.auto_merge_request.is_some(),
        reviews: pr
            .reviews
            .nodes
            .iter()
            .map(|r| Review {
                author: login(&r.author).map(String::from),
                bot: is_bot(&r.author),
                state: r.state.clone(),
                at: r.submitted_at.as_deref().and_then(instant),
                has_body: has_body(&r.body),
                commit: r.commit.as_ref().map(|c| c.oid.clone()),
            })
            .collect(),
        threads: pr
            .review_threads
            .nodes
            .iter()
            .map(|t| {
                let last = t.comments.nodes.last();
                Thread {
                    resolved: t.is_resolved,
                    outdated: t.is_outdated,
                    author: last.and_then(|c| login(&c.author)).map(String::from),
                    at: last.and_then(|c| published(c.published_at.as_deref(), &c.created_at)),
                }
            })
            .collect(),
        comments: pr.comments.nodes.iter().map(|c| Comment { author: login(&c.author).map(String::from), at: instant(&c.created_at) }).collect(),
    }
}

// Whether the rollup of the PR's head commit is `SUCCESS`. A last commit that isn't the head,
// which a push between GitHub's reads could give, says nothing.
fn rolled_up_green(pr: &PrNode) -> bool {
    let commit = pr.commits.nodes.first().map(|c| &c.commit).filter(|c| c.oid == pr.head_ref_oid);
    commit.and_then(|c| c.status_check_rollup.as_ref()).is_some_and(|r| r.state == "SUCCESS")
}

// An open PR's node as `read_pr` reads it: the first page of each review
// connection, which only `read_open_pr` reads on from.
#[derive(Debug)]
pub struct FirstPage(PrNode);

// The open PR of `first`, with every page of each connection: the rest of each the first
// page cut short is read, oldest first, so feedback beyond the newest page of anything is
// never silently missed.
pub fn read_open_pr(gh: &mut dyn Reader, repo: &str, first: FirstPage) -> Result<Pr, Error> {
    let _site = site("read_open_pr");
    let mut node = first.0;
    let [reviews, threads, comments] = REVIEW_CONNECTIONS;
    read_older(gh, repo, node.number, reviews, &mut node.reviews)?;
    read_older(gh, repo, node.number, threads, &mut node.review_threads)?;
    read_older(gh, repo, node.number, comments, &mut node.comments)?;
    Ok(parse_pr(&node))
}

// The PR of `first` as its first page has it, its review connections cut short: for a PR
// read only for its merge state, base and auto-merge.
pub fn first_page_pr(first: &FirstPage) -> Pr {
    parse_pr(&first.0)
}

// A branch's PRs, read once per repo and branch for a run. A read that failed isn't kept:
// the next ask reads again.
#[derive(Default)]
pub struct Branches {
    read: HashMap<(String, String), Vec<BranchPr>>,
}

impl Branches {
    pub fn new() -> Branches {
        Branches::default()
    }

    pub fn prs(&mut self, gh: &mut dyn Reader, repo: &str, branch: &str) -> Result<&[BranchPr], Error> {
        let key = (repo.to_ascii_lowercase(), branch.to_string());
        if !self.read.contains_key(&key) {
            let prs = branch_prs(gh, repo, branch)?;
            self.read.insert(key.clone(), prs);
        }
        Ok(&self.read[&key])
    }

}

// What GitHub says of the PR `evaluatePr` names (`read_pr`): its head branch, whether that
// branch is in the PR's own repo (a fork's never is a worker's), its state, its head
// commit.
#[derive(Debug, PartialEq)]
pub struct Head {
    pub number: u64,
    pub branch: String,
    pub same_repo: bool,
    pub state: PrState,
    pub sha: String,
    // When that state began on GitHub's clock: `mergedAt`, `closedAt` or, open, `updatedAt`.
    pub at: Option<i64>,
}

// PR `number` of `repo` (`owner/name`, `split_repo`), in one GraphQL query (`pr_query`):
// its head, and its first page for `read_open_pr`, as `branch_prs` reads an open one, so
// `evaluatePr` reads the PR once whatever its state. IDs, states and times only: the PR's text
// is never read from the response.
pub fn read_pr(gh: &mut dyn Reader, repo: &str, number: u64) -> Result<(Head, FirstPage), Error> {
    let _site = site("read_pr");
    let data = gh.graphql(repo, &pr_query(), repo_variables(repo, json!({ "number": number }))?)?;
    let read: OnePr<PrNode> = decode(data, "GitHub's answer to the PR query")?;
    let Some(pr) = read.pr() else {
        return Err(Error::Failed(format!("GitHub had no PR #{number} for {repo}")));
    };
    let at = match pr.state {
        PrState::Open => Some(pr.updated_at.as_str()),
        PrState::Merged => pr.merged_at.as_deref(),
        PrState::Closed => pr.closed_at.as_deref(),
    };
    let head = Head {
        number,
        branch: pr.head_ref_name.clone(),
        same_repo: pr.head_repository.as_ref().is_some_and(|r| r.name_with_owner.eq_ignore_ascii_case(repo)),
        state: pr.state,
        sha: pr.head_ref_oid.clone(),
        at: at.and_then(instant),
    };
    Ok((head, FirstPage(pr)))
}

// What a branch update found (`update_branch`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Updated {
    // GitHub merged the base into the branch.
    Done,
    // Not behind: nothing to bring up to date.
    Current,
    // The head isn't the one the machine read: a push it hasn't seen, never written over.
    Moved,
    // GitHub won't merge the base in at the head read, as for a conflict: a worker's to do.
    Refused,
}

const UPDATE_READ: &str = "query($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) { pullRequest(number: $number) { id state headRefName headRefOid mergeStateStatus headRepository { nameWithOwner } } } }";
// UPDATE_READ's, with whether the owner turned auto-merge off after it was on, as `read_pr`'s.
const MERGE_READ: &str = "query($owner: String!, $name: String!, $number: Int!) { repository(owner: $owner, name: $name) { pullRequest(number: $number) { id state headRefName headRefOid headRepository { nameWithOwner } autoMergeRequest { enabledAt } timelineItems(itemTypes: [AUTO_MERGE_DISABLED_EVENT], last: 1) { nodes { __typename } } } } }";
// A write a `Reader` makes (`Reader::update_branch`), fixed here so a caller passes IDs
// alone. Merged, not rebased, so a worker's own checkout of the branch stays an ancestor of it.
pub(super) const UPDATE: &str = "mutation($pr: ID!, $head: GitObjectID!) { updatePullRequestBranch(input: { pullRequestId: $pr, expectedHeadOid: $head, updateMethod: MERGE }) { pullRequest { headRefOid } } }";

// The PR as UPDATE_READ or MERGE_READ reads it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OwnPr {
    id: String,
    state: PrState,
    head_ref_name: String,
    head_ref_oid: String,
    head_repository: Option<NameWithOwner>,
    // UPDATE_READ's alone.
    #[serde(default)]
    merge_state_status: Option<String>,
    // MERGE_READ's alone.
    #[serde(default)]
    auto_merge_request: Option<IgnoredAny>,
    #[serde(default)]
    timeline_items: Option<Nodes<IgnoredAny>>,
}

// Brings PR `number` of `repo` up to date with its base as the app, only while it is open on
// `branch`, the issue's own, in `repo`, and its head is `head`: GitHub's `expectedHeadOid`
// refuses an update over a push made between this read and the write. A write GitHub turns
// down is told from a head that moved meanwhile by reading the head again.
pub fn update_branch(gh: &mut dyn Reader, repo: &str, number: u64, head: &Sha, branch: &str) -> Result<Updated, Error> {
    let _site = site("update_branch");
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Updating {
        update_pull_request_branch: Option<Wrote<IgnoredAny>>,
    }
    let read = |gh: &mut dyn Reader| -> Result<OwnPr, Error> {
        let pr = own_pr(gh, UPDATE_READ, repo, number, branch)?;
        if pr.state != PrState::Open {
            return Err(Error::Failed(format!("PR #{number} isn't open on {branch} in {repo}")));
        }
        Ok(pr)
    };
    let pr = read(gh)?;
    if pr.head_ref_oid != *head {
        return Ok(Updated::Moved);
    }
    match pr.merge_state_status.as_deref() {
        Some("DIRTY") => return Ok(Updated::Refused),
        // GitHub may not have worked it out yet: the update says.
        Some("BEHIND" | "UNKNOWN") => {}
        _ => return Ok(Updated::Current),
    }
    // Only GitHub's own `errors` are its refusal: a call that failed on the way, a token
    // included, is read again later rather than sent to a worker.
    let answer: Answer<Updating> = decode(gh.update_branch(repo, &pr.id, head)?, "GitHub's answer to the branch update")?;
    if answer.data.and_then(|d| d.update_pull_request_branch).is_some_and(|wrote| wrote.pull_request.is_some()) {
        return Ok(Updated::Done);
    }
    if !answer.errors.iter().any(|e| e.message().is_some()) {
        return Err(Error::Failed(format!("GitHub didn't say it updated PR #{number}'s branch")));
    }
    if read(gh)?.head_ref_oid != *head {
        return Ok(Updated::Moved);
    }
    // Not GitHub's errors: its text stays off this line.
    trace::event(trace::Work::UpdateRefused { repo: repo.into(), pr: number });
    Ok(Updated::Refused)
}

// PR `number` of `repo` as `query` reads it, only while it is on `branch`, the issue's own,
// in `repo`, so a command never acts on another issue's PR.
fn own_pr(gh: &mut dyn Reader, query: &str, repo: &str, number: u64, branch: &str) -> Result<OwnPr, Error> {
    let data = gh.graphql(repo, query, repo_variables(repo, json!({ "number": number }))?)?;
    let read: OnePr<OwnPr> = decode(data, "GitHub's answer to the PR's head query")?;
    let pr = read.pr().ok_or_else(|| Error::Failed(format!("GitHub had no PR #{number} for {repo}")))?;
    let own = pr.head_ref_name == branch && pr.head_repository.as_ref().is_some_and(|r| r.name_with_owner.eq_ignore_ascii_case(repo));
    if !own {
        return Err(Error::Failed(format!("PR #{number} isn't on {branch} in {repo}")));
    }
    Ok(pr)
}

// What a merge found (`merge_pr`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Merged {
    // GitHub merged it, squashed, or had already, as by its own auto-merge.
    Done,
    // The head isn't the one the machine read: a push it hasn't evaluated, never merged.
    Moved,
    // GitHub won't merge it at the head read, as when a requirement came back since.
    Refused,
    // The owner turned auto-merge off since the machine's read: theirs to merge.
    Held,
}

// `Reader::merge_pr`'s write, squashed as auto-merge would.
pub(super) const MERGE: &str = "mutation($pr: ID!, $head: GitObjectID!) { mergePullRequest(input: { pullRequestId: $pr, expectedHeadOid: $head, mergeMethod: SQUASH }) { pullRequest { state } } }";

// Merges PR `number` of `repo` as the app, only while it is on `branch`, the issue's own, and
// its head is `head`: GitHub's `expectedHeadOid` refuses a merge over a push made since the
// machine's read. Whether GitHub may merge it now is GitHub's own check. A merge GitHub turns
// down is read again, as its own auto-merge may have won the race, or a push landed. Auto-merge
// turned off since the machine's read holds it, as the read would have.
pub fn merge_pr(gh: &mut dyn Reader, repo: &str, number: u64, head: &Sha, branch: &str) -> Result<Merged, Error> {
    let _site = site("merge_pr");
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Merging {
        merge_pull_request: Option<Wrote<IgnoredAny>>,
    }
    let found = |pr: &OwnPr| {
        let held = pr.auto_merge_request.is_none() && pr.timeline_items.as_ref().is_some_and(|t| !t.nodes.is_empty());
        match (pr.state, pr.head_ref_oid == *head) {
            (PrState::Merged, _) => Ok(Some(Merged::Done)),
            (PrState::Open, true) if held => Ok(Some(Merged::Held)),
            (PrState::Open, true) => Ok(None),
            (PrState::Open, false) => Ok(Some(Merged::Moved)),
            (PrState::Closed, _) => Err(Error::Failed(format!("PR #{number} isn't open on {branch} in {repo}"))),
        }
    };
    let pr = own_pr(gh, MERGE_READ, repo, number, branch)?;
    if let Some(merged) = found(&pr)? {
        return Ok(merged);
    }
    let answer: Answer<Merging> = decode(gh.merge_pr(repo, &pr.id, head)?, "GitHub's answer to the merge")?;
    if answer.data.and_then(|d| d.merge_pull_request).is_some_and(|wrote| wrote.pull_request.is_some()) {
        return Ok(Merged::Done);
    }
    if answer.errors.is_empty() {
        return Err(Error::Failed(format!("GitHub didn't say it merged PR #{number}")));
    }
    if let Some(merged) = found(&own_pr(gh, MERGE_READ, repo, number, branch)?)? {
        return Ok(merged);
    }
    // Not GitHub's errors: its text stays off this line.
    trace::event(trace::Work::MergeRefused { repo: repo.into(), pr: number });
    Ok(Merged::Refused)
}

#[cfg(test)]
mod tests;
