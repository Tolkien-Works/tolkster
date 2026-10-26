// The PR orders: a PR read for the machine (`evaluatePr`), brought up to date or merged as the
// app, on the branch of the worktree Orca links to the issue.

use serde_json::{Value, json};

use crate::error::Error;
use crate::executor;
use crate::github::{self, Head, Pr, PrState};
use crate::plain::split_repo;
use crate::machine::{Command, Finished};
use crate::ports::Ports;
use crate::config::Config;
use crate::plain::Sha;
use crate::review::{self, Checked, failed_checks, newest_feedback_and_reviewers};
use crate::time::{iso, now_ms};
use crate::trace::{self, Purpose, Work};

pub(super) fn evaluate_pr(config: &Config, ports: &mut Ports, command: &Command, repo: &str, number: u64) -> Result<Finished, Error> {
    let gh = &mut *ports.github;
    let (head, first) = github::read_pr(gh, repo, number)?;
    let review = config.review_for(split_repo(repo).map(|(_, name)| name));
    if let (PrState::Merged, Some(at)) = (&head.state, head.at) {
        ports.shared.timing.merged(&command.issue_id, &command.identifier, Some(number), at, true);
    }
    // A PR no longer open is read for its merge or close alone: the machine decides nothing
    // else from it, so its feedback's pages, checks and label go unread, and no failure of
    // theirs can keep a lost merge or close from being applied.
    if head.state != PrState::Open {
        return Ok(Finished::ok(json!({ "pr": pr_read(&head, &github::first_page_pr(&first), None, review.max_rounds)? })));
    }
    let pr = github::read_open_pr(gh, repo, first)?;
    let checked = failed_checks(gh, &pr, None)?;
    let open = Open {
        checked,
        feedback: newest_feedback_and_reviewers(&pr, &review.reviewers),
        copilot_wont_approve: review::copilot_wont_approve(&pr, checked),
        manual_merge: ports.linear.manual_merge(&command.issue_id)?,
    };
    Ok(Finished::ok(json!({ "pr": pr_read(&head, &pr, Some(open), review.max_rounds)? })))
}

// A PR only behind its base brought up to date as the app, on the branch of the worktree Orca
// links to the issue, so it is never another issue's PR. `moved`: its head wasn't the one the
// machine read, and nothing was written. `refused` alone sends a worker: any other failure is
// read again later.
pub(super) fn update_branch(ports: &mut Ports, command: &Command, repo: &str, number: u64, head: &Sha) -> Result<Finished, Error> {
    let Some(branch) = executor::linked_worktree(ports, &command.identifier)?.and_then(|w| w.branch) else {
        trace::event(Work::NoWorktree { issue: command.identifier.clone(), purpose: Purpose::Branch(number) });
        return Ok(Finished::failed());
    };
    Ok(match github::update_branch(ports.github, repo, number, head, &branch)? {
        github::Updated::Moved => Finished::ok(json!({ "moved": true })),
        github::Updated::Done | github::Updated::Current => Finished::ok(json!({})),
        github::Updated::Refused => Finished::failed_with(json!({ "refused": true })),
    })
}

// A PR merged as the app, on the branch of the worktree Orca links to the issue, as a branch's
// update is, unless the owner held it since the machine's read: `manual-merge`, or auto-merge
// turned off. One merged already, as by GitHub's own auto-merge, is done. `refused`, by GitHub
// or the owner, is left to the next PR fact; any other failure is read again later.
pub(super) fn merge_pr(ports: &mut Ports, command: &Command, repo: &str, number: u64, head: &Sha) -> Result<Finished, Error> {
    if ports.linear.manual_merge(&command.issue_id)? {
        return Ok(Finished::failed_with(json!({ "refused": true })));
    }
    let Some(branch) = executor::linked_worktree(ports, &command.identifier)?.and_then(|w| w.branch) else {
        trace::event(Work::NoWorktree { issue: command.identifier.clone(), purpose: Purpose::Branch(number) });
        return Ok(Finished::failed());
    };
    Ok(match github::merge_pr(ports.github, repo, number, head, &branch)? {
        github::Merged::Done => {
            ports.shared.timing.merged(&command.issue_id, &command.identifier, Some(number), now_ms(), false);
            Finished::ok(json!({}))
        }
        github::Merged::Moved => Finished::ok(json!({ "moved": true })),
        github::Merged::Refused | github::Merged::Held => Finished::failed_with(json!({ "refused": true })),
    })
}

// What only an open PR is read for.
pub struct Open {
    pub checked: Checked,
    pub feedback: Option<(i64, Option<i64>)>,
    pub copilot_wont_approve: bool,
    pub manual_merge: bool,
}

// The machine's cap on `review.maxRounds` (validate.ts, parsePr): a repo's setting over it is read
// as it, rather than have every read refused.
const MAX_ROUNDS: i64 = 100;

// The PR as the machine's `PrRead` takes it (src/worker/machine/types.ts). Its times are
// GitHub's, to the second, written with `.000` milliseconds, the machine's `TIME`: an event's
// time must reach the machine in the same form, or a read and an event from the same second
// misorder. Err for a read the machine would refuse, so it is `failed` rather than invalid.
pub fn pr_read(head: &Head, pr: &Pr, open: Option<Open>, max_rounds: i64) -> Result<Value, Error> {
    let at = head.at.ok_or_else(|| Error::Failed(format!("GitHub's PR #{} had no time for its state", head.number)))?;
    let (Some(head_sha), Some(base)) = (Sha::parse(&head.sha), Sha::parse(&pr.base_head)) else {
        return Err(Error::Failed(format!("GitHub's PR #{} had no head or base commit", head.number)));
    };
    let status = match head.state {
        PrState::Open => "open",
        PrState::Closed => "closed",
        PrState::Merged => "merged",
    };
    let Open { checked, feedback, copilot_wont_approve, manual_merge } =
        open.unwrap_or(Open { checked: Checked { pending: true, ..Checked::default() }, feedback: None, copilot_wont_approve: false, manual_merge: false });
    // A failure is the worker's to fix, so it wins over a CI that couldn't run.
    let checks = match checked {
        Checked { failed: true, .. } => "failed",
        Checked { unran: true, .. } => "couldntRun",
        Checked { pending: true, .. } => "pending",
        _ => "passed",
    };
    Ok(json!({
        "number": head.number,
        "status": status,
        "at": iso(at),
        "head": head_sha,
        "base": base,
        "mergeState": merge_state(&pr.merge_state),
        "autoMergeOff": pr.auto_merge_off,
        "autoMerge": pr.auto_merge_on,
        // GitHub's own: nothing it requires is left, which `mergeState`'s `CLEAN` doesn't say.
        "mergeable": pr.merge_state == "CLEAN",
        "manualMerge": manual_merge,
        "checks": checks,
        "feedback": feedback.map(|(at, reviewer)| json!({ "at": iso(at), "reviewerAt": reviewer.map(iso) })),
        "copilotWontApprove": copilot_wont_approve,
        "maxRounds": max_rounds.min(MAX_ROUNDS),
    }))
}

// GitHub's merge state as the machine's four: only a conflict or a base the PR must catch
// up with asks for a rebase, so blocked, unstable, draft and hooked PRs are `CLEAN`.
fn merge_state(github: &str) -> &'static str {
    match github {
        "BEHIND" => "BEHIND",
        "DIRTY" => "DIRTY",
        "CLEAN" | "BLOCKED" | "UNSTABLE" | "HAS_HOOKS" | "DRAFT" => "CLEAN",
        _ => "UNKNOWN",
    }
}
