// A PR's checks, as `tolkster pr checks` and the review resume read them: GitHub's rollup of
// its head commit's check runs and statuses.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::cost::site;
use super::{MAX_PAGES, Next, Nodes, OnePr, PAGE, Pr, PrState, Reader, repo_variables};
use crate::error::Error;
use crate::json::decode;
use crate::time::instant;

// The checks on a PR's head commit: check runs and commit statuses, as GitHub rolls them up,
// each with whether it is required for the PR and when it finished. Every page is read, so
// a check beyond the first 100 is never missed. `headRefOid` and the rollup's own commit
// come apart while GitHub moves a head (`checks`), and `mergeable` is what GitHub makes of
// merging that head into its base (`Mergeable`). `$run` is what else a check run asks.
macro_rules! checks_query {
    ($run:literal) => {
        concat!(
            "query($owner: String!, $name: String!, $number: Int!, $after: String) { repository(owner: $owner, name: $name) { pullRequest(number: $number) {
  headRefOid headRefName state mergeable mergeStateStatus
  commits(last: 1) { nodes { commit { oid statusCheckRollup { contexts(first: 100, after: $after) { nodes {
    __typename
    ... on CheckRun { name status conclusion detailsUrl completedAt isRequired(pullRequestNumber: $number) checkSuite { app { slug } }",
            $run,
            " }
    ... on StatusContext { context state targetUrl createdAt isRequired(pullRequestNumber: $number) }
  } pageInfo { hasNextPage endCursor } } } } } }
} } }"
        )
    };
}

// With what `never_ran` reads. GitHub charges for each nested connection, so this costs
// about twice `CHECKS_LIGHT`, which has what settles a check and no more.
pub(super) const CHECKS_QUERY: &str = checks_query!(" steps(first: 1) { totalCount } annotations(first: 20) { nodes { message } }");
pub(super) const CHECKS_LIGHT: &str = checks_query!("");

// GitHub Actions' app slug, as `checkSuite.app.slug` names it.
const ACTIONS: &str = "github-actions";

// The PR as the checks query reads it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChecksPr {
    head_ref_oid: String,
    head_ref_name: String,
    state: PrState,
    mergeable: String,
    merge_state_status: String,
    commits: Nodes<RollupCommit>,
}

#[derive(Debug, Deserialize)]
struct RollupCommit {
    commit: Rollup,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rollup {
    oid: String,
    // Null for a commit with no checks.
    status_check_rollup: Option<RollupContexts>,
}

#[derive(Debug, Deserialize)]
struct RollupContexts {
    contexts: Next<Context>,
}

// One check context of a PR's head: a check run or a commit status, by `__typename`.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "__typename")]
enum Context {
    CheckRun(CheckRun),
    StatusContext(StatusContext),
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckRun {
    name: String,
    status: String,
    // Null until it completes.
    conclusion: Option<String>,
    details_url: Option<String>,
    completed_at: Option<String>,
    is_required: bool,
    check_suite: CheckSuite,
    // CHECKS_QUERY's alone.
    #[serde(default)]
    steps: Option<StepCount>,
    #[serde(default)]
    annotations: Option<Nodes<Annotation>>,
}

#[derive(Clone, Debug, Deserialize)]
struct CheckSuite {
    // Null once the app is gone.
    app: Option<Slug>,
}

#[derive(Clone, Debug, Deserialize)]
struct Slug {
    slug: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StepCount {
    total_count: u64,
}

#[derive(Clone, Debug, Deserialize)]
struct Annotation {
    message: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusContext {
    context: String,
    state: String,
    target_url: Option<String>,
    created_at: String,
    is_required: bool,
}

// What GitHub's own annotation says when Actions kept a job from starting: "The job was
// not started because recent account payments have failed or your spending limit needs
// to be increased." Matched without case, and never printed: the annotation is GitHub's
// text, and the state says enough. The phrases are GitHub's, not words a workflow's own
// step is likely to write in an annotation of its own.
const NOT_STARTED: [&str; 3] = ["was not started", "spending limit", "payments have failed"];

// Whether a check run never ran: GitHub Actions concluded it without a single step (the
// job never started, as when the org has no Actions minutes or its billing failed, or it
// was cancelled before it began), or its annotation says Actions didn't start it. Such a
// check says nothing about the code. Only Actions' runs count: another app's check run has
// no steps to count, and its annotations are its own. A startup failure is the workflow
// file's own fault, so it stays a failure. A check run whose steps weren't read counts as
// run.
fn never_ran(check: &CheckRun) -> bool {
    let startup = check.conclusion.as_deref() == Some("STARTUP_FAILURE");
    let no_steps = check.steps.as_ref().is_some_and(|steps| steps.total_count == 0);
    let not_started = check.annotations.iter().flat_map(|a| &a.nodes).any(|a| {
        let message = a.message.to_ascii_lowercase();
        NOT_STARTED.iter().any(|phrase| message.contains(phrase))
    });
    is_actions(check) && !startup && (no_steps || not_started)
}

fn is_actions(check: &CheckRun) -> bool {
    check.check_suite.app.as_ref().is_some_and(|app| app.slug.eq_ignore_ascii_case(ACTIONS))
}

// Whether a check, read without its steps and annotations, could be a run that never ran
// (`never_ran`): Actions' own, concluded neither passing nor as a startup failure.
fn may_not_have_run(context: &Context) -> bool {
    let Context::CheckRun(check) = context else { return false };
    is_actions(check) && check.status == "COMPLETED" && !matches!(check.conclusion.as_deref(), Some("SUCCESS" | "NEUTRAL" | "SKIPPED" | "STALE" | "STARTUP_FAILURE"))
}

// What a check, or all of a PR's, came to, worst last: a PR's state is the worst of its
// checks' (`read_checks`), so a check that couldn't run (`Unavailable`) counts only once
// nothing is pending, and a check that ran and failed counts over it. `None` when the PR
// has no checks at all. `Merged` and `Closed` are a PR's alone, never a check's: a PR no
// longer open has no checks to wait for, and so is `Conflicts`: a PR whose head can't merge
// into its base runs no `pull_request` workflow until it is rebased.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckState {
    Pass,
    // CI couldn't run: the check concluded without running (`never_ran`) or waits on a
    // person (`ACTION_REQUIRED`). Not a failure.
    Unavailable,
    Pending,
    Fail,
    None,
    Conflicts,
    Merged,
    Closed,
}

impl CheckState {
    // What one check context of a PR's head, a check run or a commit status, came to: the one
    // rule `pr checks` (`read_checks`) and `evaluatePr` (`head_checks`, `review::failed_checks`)
    // share. A check run is pending until it completes, then its conclusion: skipped and
    // neutral pass, as `gh pr checks` has them, and so does stale, which never blocks a
    // merge. One that concluded without running (`never_ran`) or needs a person's action
    // (`ACTION_REQUIRED`) is unavailable: nothing of the code was tried or nothing a worker
    // can do clears it. Any other conclusion, one GitHub adds later too, is a failure.
    fn of(context: &Context) -> CheckState {
        let check = match context {
            Context::StatusContext(status) => {
                return match status.state.as_str() {
                    "SUCCESS" => CheckState::Pass,
                    "PENDING" | "EXPECTED" => CheckState::Pending,
                    _ => CheckState::Fail,
                };
            }
            Context::CheckRun(check) => check,
        };
        match (check.status.as_str(), check.conclusion.as_deref().unwrap_or_default()) {
            ("COMPLETED", "SUCCESS" | "NEUTRAL" | "SKIPPED" | "STALE") => CheckState::Pass,
            ("COMPLETED", "ACTION_REQUIRED") => CheckState::Unavailable,
            ("COMPLETED", _) if never_ran(check) => CheckState::Unavailable,
            ("COMPLETED", _) => CheckState::Fail,
            _ => CheckState::Pending,
        }
    }

    // As `gh pr checks` exits: 0 passed, 1 failed, 8 still pending. No checks at all is
    // "nothing to do", 3. CI that couldn't run is 4: neither green nor red. A PR merged is
    // 5 and one closed unmerged 6: whatever was pushed after will never be its head. One that
    // conflicts with its base is 7.
    pub fn exit_code(self) -> u8 {
        match self {
            CheckState::Pass => 0,
            CheckState::Fail => 1,
            CheckState::Pending => 8,
            CheckState::None => 3,
            CheckState::Unavailable => 4,
            CheckState::Merged => 5,
            CheckState::Closed => 6,
            CheckState::Conflicts => 7,
        }
    }
}

// What `tolkster pr checks` prints.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Checks {
    pub pr: u64,
    pub head: String,
    pub state: CheckState,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub conflicts: bool,
    pub checks: Vec<Check>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Check {
    pub name: String,
    pub state: CheckState,
    pub url: Option<String>,
}

// The commit `pr checks` waits for: the one the worker pushed. From `--head`, it applies
// to the PR whatever its branch. From the checkout's upstream it names that branch, and
// applies only to the PR whose head branch it is: a checkout of another branch says
// nothing of this PR.
pub struct Pushed {
    pub branch: Option<String>,
    pub sha: String,
}

// Whether GitHub can merge a PR's head into its base. It works that out anew after each push,
// and says `UNKNOWN` until it has. `mergeable` saying `UNKNOWN` or `CONFLICTING` decides
// first, then `mergeStateStatus`. Behind its base alone isn't a conflict: the machine
// brings such a branch up to date through the app.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mergeable {
    Yes,
    Conflicts,
    Unknown,
}

fn mergeable(pr: &ChecksPr) -> Mergeable {
    match (pr.mergeable.as_str(), pr.merge_state_status.as_str()) {
        ("UNKNOWN", _) => Mergeable::Unknown,
        ("CONFLICTING", _) => Mergeable::Conflicts,
        (_, "UNKNOWN") => Mergeable::Unknown,
        (_, "DIRTY") => Mergeable::Conflicts,
        _ => Mergeable::Yes,
    }
}

// The commit whose rollup was read, as one page shows it.
fn rollup(pr: &ChecksPr) -> Option<&Rollup> {
    pr.commits.nodes.first().map(|c| &c.commit)
}

// Whether `pr`'s checks are those of the commit that counts: the rollup is of the PR's
// head, GitHub having moved it and its commits at different moments, and the head is
// `pushed`'s, when there is one for this PR's branch.
fn is_current(pr: &ChecksPr, pushed: Option<&Pushed>) -> bool {
    let head = pr.head_ref_oid.as_str();
    let wanted = pushed.filter(|p| p.branch.as_deref().is_none_or(|b| pr.head_ref_name == b));
    rollup(pr).is_some_and(|r| r.oid == head) && wanted.is_none_or(|p| head.eq_ignore_ascii_case(&p.sha))
}

// Every check context on PR `number`'s head, from one commit, and whether it merges, as the
// last page has it. Instead, with the commit read:
// `Pending` while the head, or the commit read, isn't `pushed` (`is_current`), and when a
// push between pages moved it, never a mix of two commits' checks; `Merged` or `Closed`,
// with the PR's head, once it isn't open, checked before the head is.
type Contexts = (String, Result<(Vec<Context>, Mergeable), CheckState>);

fn read_contexts(gh: &mut dyn Reader, query: &str, repo: &str, number: u64, pushed: Option<&Pushed>) -> Result<Contexts, Error> {
    let (mut first, mut contexts, mut after) = (None, Vec::new(), Value::Null);
    for _ in 0..MAX_PAGES {
        let data = gh.graphql(repo, query, repo_variables(repo, json!({ "number": number, "after": after }))?)?;
        let read: OnePr<ChecksPr> = decode(data, "GitHub's answer to the checks query")?;
        let mut page = read.pr().ok_or_else(|| Error::Failed(format!("{repo}#{number} is not a pull request")))?;
        let ended = match page.state {
            PrState::Merged => Some(CheckState::Merged),
            PrState::Closed => Some(CheckState::Closed),
            PrState::Open => None,
        };
        if let Some(ended) = ended {
            return Ok((page.head_ref_oid, Err(ended)));
        }
        let commit = rollup(&page).map(|r| r.oid.clone()).unwrap_or_default();
        if !is_current(&page, pushed) || first.as_ref().is_some_and(|first| *first != commit) {
            return Ok((commit, Err(CheckState::Pending)));
        }
        let merge = mergeable(&page);
        // A commit with no rollup has no checks yet.
        let Some(read) = page.commits.nodes.pop().and_then(|c| c.commit.status_check_rollup).map(|r| r.contexts) else {
            return Ok((commit, Ok((contexts, merge))));
        };
        contexts.extend(read.nodes);
        if !read.page_info.has_next_page {
            return Ok((commit, Ok((contexts, merge))));
        }
        after = read.page_info.end_cursor.map_or(Value::Null, Value::String);
        first.get_or_insert(commit);
    }
    Err(Error::Failed(format!("{repo}#{number} has over {} checks", MAX_PAGES * PAGE)))
}

// `pending` while the head, or the commit read, isn't `pushed`: GitHub moves a PR's head
// a moment after a push, and the checks of the head before it are all finished. A push
// between pages reads as pending too. Pending lists no checks, for they would be another
// commit's, and its `head` is the commit that was read. A PR merged or closed is that, at
// once, with its head and no checks, whatever was pushed.
//
// Open and current, a PR that conflicts with its base is that, over whatever its checks came
// to: those pending will never run, and a failure on a head that can't merge isn't the first
// thing to fix. No checks while GitHub hasn't worked out whether it merges is pending, so
// "no checks" is never read before a conflict could show.
//
// Read light (`CHECKS_LIGHT`), and read again in full only when a check run may not have
// run (`may_not_have_run`), which the light read can't tell from a failure.
pub fn checks(gh: &mut dyn Reader, repo: &str, number: u64, pushed: Option<&Pushed>) -> Result<Checks, Error> {
    let _site = site("checks");
    let light = read_contexts(gh, CHECKS_LIGHT, repo, number, pushed)?;
    let read = match &light.1 {
        Ok((contexts, Mergeable::Yes | Mergeable::Unknown)) if contexts.iter().any(may_not_have_run) => read_contexts(gh, CHECKS_QUERY, repo, number, pushed)?,
        _ => light,
    };
    Ok(match read {
        (head, Ok((contexts, merge))) => {
            let mut read = read_checks(head, &contexts, number);
            read.state = match (merge, read.state) {
                (Mergeable::Conflicts, _) => CheckState::Conflicts,
                (Mergeable::Unknown, CheckState::None) => CheckState::Pending,
                (_, state) => state,
            };
            read.conflicts = merge == Mergeable::Conflicts;
            read
        }
        (head, Err(state)) => Checks { pr: number, head, state, conflicts: false, checks: Vec::new() },
    })
}

// A check on a PR's head as the review resume weighs it (review::failed_checks): what it came
// to (`CheckState::of`), whether it is required for the PR, and when it finished. Never its
// name or text: an annotation is matched (`never_ran`), never kept.
pub struct HeadCheck {
    pub state: CheckState,
    pub required: bool,
    // When a check run completed, or a commit status was set.
    pub at: Option<i64>,
}

// Every check on `pr`'s head, as `pr` was read: None once the head has moved since, or the PR
// is no longer open. Read light, and in full only as `checks` does.
pub fn head_checks(gh: &mut dyn Reader, pr: &Pr) -> Result<Option<Vec<HeadCheck>>, Error> {
    let _site = site("head_checks");
    let pushed = Pushed { branch: None, sha: pr.head.clone() };
    let Ok((mut contexts, _)) = read_contexts(gh, CHECKS_LIGHT, &pr.repo, pr.number, Some(&pushed))?.1 else { return Ok(None) };
    if contexts.iter().any(may_not_have_run) {
        let Ok((full, _)) = read_contexts(gh, CHECKS_QUERY, &pr.repo, pr.number, Some(&pushed))?.1 else { return Ok(None) };
        contexts = full;
    }
    let checks = contexts.iter().map(|c| {
        let (required, at) = match c {
            Context::StatusContext(status) => (status.is_required, instant(&status.created_at)),
            Context::CheckRun(check) => (check.is_required, check.completed_at.as_deref().and_then(instant)),
        };
        HeadCheck { state: CheckState::of(c), required, at }
    });
    Ok(Some(checks.collect()))
}

fn read_checks(head: String, contexts: &[Context], number: u64) -> Checks {
    let checks: Vec<Check> = contexts
        .iter()
        .map(|c| {
            let (name, url) = match c {
                Context::StatusContext(status) => (&status.context, &status.target_url),
                Context::CheckRun(check) => (&check.name, &check.details_url),
            };
            Check { name: name.clone(), state: CheckState::of(c), url: url.clone() }
        })
        .collect();
    // The worst of them: any failure fails, else any pending is pending, else any check
    // that couldn't run makes the PR's CI unavailable.
    let state = checks.iter().map(|c| c.state).max().unwrap_or(CheckState::None);
    Checks { pr: number, head, state, conflicts: false, checks }
}

#[cfg(test)]
mod tests;
