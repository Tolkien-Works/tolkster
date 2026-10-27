use ureq::Agent;

use crate::cli::{ISSUE_ENV, WORKTREE_ENV, env};
use crate::config::files;
use crate::exec::{self, run_command};
use crate::harness::continued;
use crate::orca::{self, Status};
use crate::plain::{IssueKey, WorktreeId, inside};
use crate::state::{Records, State, WorkerRecord};

// A worker's worktree card in Orca, whose comment the owner reads for where the worker is
// and how to resume it, and whose column follows its issue's state (`pr create` moves it to in
// review, a review round back to in progress). The commands that mark a worker's
// checkpoints set the comment (a durable `action` and `pr create`), so no brief asks the
// worker to.

// The worker whose worktree holds `dir` on `host` (config::host), by the records of the
// workers Tolkster started, the innermost when worktrees nest, and its worktree's root. A
// directory that is no worker's, the primary checkout's included, has none, and nor has
// another host's worktree at the same path.
pub fn worker<'w>(workers: &'w [WorkerRecord], host: Option<&str>, dir: &str) -> Option<(&'w WorkerRecord, &'w str)> {
    workers
        .iter()
        .filter(|w| w.worktree_id.host() == host)
        .map(|w| (w, w.worktree_id.path()))
        .filter(|(w, root)| IssueKey::parse(&w.issue).is_some() && inside(dir, root))
        .max_by_key(|(_, root)| root.len())
}

// The worker a command runs for, as its launch put it in the agent's environment
// (`launch_env`): its worktree's ID and root and its issue as Tolkster's record holds them.
// The worker's knowledge of itself, never read back from the records: a conversation's turn
// or the owner's checkout has none.
#[derive(Clone, Debug, PartialEq)]
pub struct Launched {
    pub worktree_id: WorktreeId,
    pub root: String,
    pub issue: String,
}

// What `tolkster worker` puts in its agent's environment for `record`, the worker's.
pub fn launch_env(record: &WorkerRecord) -> [(String, String); 2] {
    [(WORKTREE_ENV.into(), record.worktree_id.to_string()), (ISSUE_ENV.into(), record.issue.clone())]
}

pub fn launched() -> Option<Launched> {
    launched_of(env(WORKTREE_ENV), env(ISSUE_ENV))
}

// None unless both are there and in shape.
pub fn launched_of(worktree_id: Option<String>, issue: Option<String>) -> Option<Launched> {
    let (worktree_id, issue) = (WorktreeId::parse(&worktree_id?)?, issue.filter(|issue| IssueKey::parse(issue).is_some())?);
    let root = worktree_id.path().to_string();
    Some(Launched { worktree_id, root, issue })
}

// Sets the card's comment of the worker this command runs for (`launched`) to an action's
// checkpoint (`action`), unless it holds a PR's (`pr`), so the card ends on the PR and how to
// resume, whatever the worker posts after. Outside a worker, nothing. A failure never fails
// the command it rides on, which has printed its answer already: it prints one fixed line.
pub fn checkpoint(worker: Option<&Launched>, action: &str) {
    if let Some(worker) = worker {
        set(worker, Kind::Action, action);
    }
}

// Moves the card to in review, and sets it to PR #`pr`'s checkpoint, from the worker's record,
// read for how it was launched; the comment nothing where that can't be read.
pub fn checkpoint_pr(agent: &Agent, pr: u64, what: &str) {
    let Some(worker) = launched() else { return };
    moved(&mut run_command, &worker, Status::InReview);
    let Ok(workers) = files().and_then(|files| State::open(agent, &files)?.workers()) else { return };
    if let Some(record) = workers.iter().find(|w| w.worktree_id == worker.worktree_id) {
        set(&worker, Kind::Pr, &self::pr(record, pr, what));
    }
}

// Moves the worker's card to `status`'s column. A failure never fails the command it rides on:
// it prints one fixed line.
fn moved(run: &mut exec::Run, worker: &Launched, status: Status) {
    // The card is this host's, so its Orca's own ID names it.
    let card = worker.worktree_id.own();
    if orca::set_status(run, &card, status).is_err() {
        eprintln!("tolkster: the worktree card's status wasn't set");
    }
}

fn set(worker: &Launched, kind: Kind, comment: &str) {
    // The card is this host's, so its Orca's own ID names it.
    let card = worker.worktree_id.own();
    let run = &mut run_command;
    let set = match kind {
        Kind::Action => orca::comment(run, &card).map(|now| !holds_pr(now.as_deref())),
        Kind::Pr => Ok(true),
    }
    .and_then(|replace| if replace { orca::set_comment(run, &card, comment) } else { Ok(()) });
    if set.is_err() {
        eprintln!("tolkster: the worktree card's comment wasn't set");
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Action,
    Pr,
}

// Whether the card's comment is a PR's checkpoint, as `pr` writes one.
pub fn holds_pr(comment: Option<&str>) -> bool {
    comment.is_some_and(|comment| comment.starts_with("PR #"))
}

// A milestone or a commit, as the worker posted it.
pub fn action(action: &str, parameter: &str) -> String {
    format!("{action}: {parameter}")
}

// The form the owner reads a worker's PR on its card in: `PR #<n>, <what>. Resume with:
// <resume>`, the resume launched as the worker was. A launch that can't go on a command
// line leaves the resume out.
pub fn pr(worker: &WorkerRecord, pr: u64, what: &str) -> String {
    match continued(Some(&worker.launch)) {
        Ok(resume) => format!("PR #{pr}, {what}. Resume with: {}", resume.as_str()),
        Err(_) => format!("PR #{pr}, {what}."),
    }
}

#[cfg(test)]
mod tests;
