// A review round the Durable Object's machine started (docs/review-feedback.md): the reads
// `evaluatePr` decides from (`failed_checks`, `newest_feedback_and_reviewers`), and the fixed
// lines a round types into the worker's terminal or starts a new session on
// (executor::round). GitHub is read as the app through a `Reader`, so the rules can be
// tested without it.
//
// Review and comment text is untrusted: only authors, states, times, commits and whether a
// review has a body are read, and the worker gets one fixed line telling it to read the PR
// itself with `tolkster pr feedback`.

use std::path::Path;

use crate::error::Error;
use crate::github::{self, CheckState, Pr, Reader, is_copilot};
use crate::machine::Notice;
use crate::plain::{Id, IssueKey, Line};

// What one read of a PR's head checks found, for the resume and for the notice that CI
// couldn't run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Checked {
    pub failed: bool,
    // A required check run never ran (Actions concluded it without starting it) or waits on a
    // person. Nothing of the code was tried, so the PR can't merge until CI runs, and no
    // worker can fix that.
    pub unran: bool,
    // A required check hasn't finished, or the head has moved since `pr` was read, so its
    // checks are still to come.
    pub pending: bool,
}

// What the checks required for PR `pr` came to at its head, each as `pr checks` classifies it
// (`CheckState::of`). `failed`: a check that failed after `marker` (a run Actions never
// started, for want of minutes or billing, or one waiting on a person, says nothing about
// the code: it is `unran`). A head that has moved since `pr` was read says nothing
// (`github::head_checks`): the push brings its own checks. GitHub gives check times to the
// second and the marker has milliseconds, so a check counts from the marker's second on: one
// that failed later in that second is never missed for good.
pub fn failed_checks(gh: &mut dyn Reader, pr: &Pr, marker: Option<i64>) -> Result<Checked, Error> {
    if pr.checks_green {
        return Ok(Checked::default());
    }
    let Some(checks) = github::head_checks(gh, pr)? else { return Ok(Checked { pending: true, ..Checked::default() }) };
    let required = || checks.iter().filter(|c| c.required);
    let failed = required().any(|c| c.state == CheckState::Fail && c.at.is_some_and(|at| marker.is_none_or(|m| at >= m.div_euclid(1000) * 1000)));
    let unran = required().any(|c| c.state == CheckState::Unavailable);
    let pending = required().any(|c| c.state == CheckState::Pending);
    Ok(Checked { failed, unran, pending })
}

fn counts(pr: &Pr, author: &Option<String>, blessed: &[String]) -> bool {
    github::blessed(author.as_deref(), pr.bot_author.as_deref(), blessed)
}

// The newest feedback from a blessed account that asks the worker for something:
// 1. an unresolved, current review thread whose latest comment is blessed;
// 2. a blessed person's review requesting changes, or commenting with a body. A bot's
//    review counts only through its threads, so Copilot's overview and its "unable to
//    review" notes never count;
// 3. a blessed account's comment on the PR conversation.
pub fn newest_feedback(pr: &Pr, blessed: &[String]) -> Option<i64> {
    feedback_from(pr, blessed, |_| true)
}

// The newest feedback, as `newest_feedback`, and the newest of it not Copilot's, which
// alone resumes a worker past the cap even when Copilot wrote after it.
pub fn newest_feedback_and_reviewers(pr: &Pr, blessed: &[String]) -> Option<(i64, Option<i64>)> {
    Some((newest_feedback(pr, blessed)?, newest_feedback_but_copilots(pr, blessed)))
}

// The newest feedback from anyone blessed but Copilot. After the cap on rounds, only this
// resumes the worker: Copilot's reviews go to the owner then.
fn newest_feedback_but_copilots(pr: &Pr, blessed: &[String]) -> Option<i64> {
    feedback_from(pr, blessed, |author| !is_copilot(author.as_deref()))
}

fn feedback_from(pr: &Pr, blessed: &[String], from: impl Fn(&Option<String>) -> bool) -> Option<i64> {
    let counts = |author: &Option<String>| counts(pr, author, blessed) && from(author);
    let threads = pr.threads.iter().filter(|t| !t.resolved && !t.outdated && counts(&t.author)).filter_map(|t| t.at);
    let reviews = pr
        .reviews
        .iter()
        .filter(|r| !r.bot && counts(&r.author))
        .filter(|r| r.state == "CHANGES_REQUESTED" || (r.state == "COMMENTED" && r.has_body))
        .filter_map(|r| r.at);
    let comments = pr.comments.iter().filter(|c| counts(&c.author)).filter_map(|c| c.at);
    threads.chain(reviews).chain(comments).max()
}

// Copilot's newest review is a comment on the PR's head, and nothing else there resumes a worker
// or lets the PR merge: no thread open, no approval of the head, and GitHub holds the merge
// (`BLOCKED`) with every required check passed. Copilot doesn't approve after commenting, so
// what holds it is an approval only a person gives.
pub fn copilot_wont_approve(pr: &Pr, checked: Checked) -> bool {
    let at_head = |commit: &Option<String>| commit.as_deref() == Some(pr.head.as_str());
    let newest = pr.reviews.iter().filter(|r| is_copilot(r.author.as_deref())).max_by_key(|r| r.at);
    pr.merge_state == "BLOCKED"
        && checked == Checked::default()
        && newest.is_some_and(|r| r.state == "COMMENTED" && at_head(&r.commit))
        && !pr.threads.iter().any(|t| !t.resolved && !t.outdated)
        && !pr.reviews.iter().any(|r| r.state == "APPROVED" && at_head(&r.commit))
}

const NOTE_PR: &str = " on PR #";

// What a round is for: review feedback, a failed check or a rebase, in the repo's last round
// or not, or a rebase alone, which has a brief of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Round {
    Review { last: bool },
    Rebase,
}

// What the durable action picked up: its parameter, after PICKED_UP.
pub fn picked_up(number: u64, short_head: Option<&str>, round: Round) -> String {
    let at = short_head.map_or(String::new(), |head| format!(" at {head}"));
    match round {
        Round::Review { last } => {
            let last = if last { ", its last round" } else { "" };
            format!("review feedback, failing checks or a rebase{NOTE_PR}{number}{at}{last}")
        }
        Round::Rebase => format!("a rebase{NOTE_PR}{number}{at}"),
    }
}

pub fn capped_number(number: u64, cap: u32) -> String {
    let rounds = if cap == 1 { "its review round".to_string() } else { format!("its {cap} review rounds") };
    format!("PR #{} has had {rounds}, so I've stopped picking up Copilot's reviews on it. Can you take a look?", number)
}

// Why a PR won't merge by itself, as a response, so the card shows it as the latest word.
// `notice_text` passes only these five notices.
pub fn told(number: u64, notice: Notice) -> String {
    match notice {
        Notice::PrClosed => format!("PR #{number} was closed without merging. I've left its branch as it is."),
        Notice::AutoMergeOff => format!("Auto-merge is off on PR #{number}, so it won't merge by itself."),
        Notice::CiCouldntRun => format!("CI couldn't run on PR #{number}, so it can't merge until it does."),
        Notice::ChecksPending => format!("PR #{number}'s checks have been pending for over an hour, so it can't merge until they finish."),
        Notice::CopilotWontApprove => format!("Copilot won't approve PR #{number} without a person looking. Can you take a look?"),
        _ => unreachable!("no merge notice"),
    }
}

// The one line the worker gets: no feedback text, and nothing `Line` refuses, since a resumed
// worker's line is typed into its terminal's shell.
pub fn line(number: u64, identifier: &IssueKey, session_id: &Id, round: Round) -> Result<Line, Error> {
    Line::check(&match round {
        Round::Review { last } => format!(
            "PR #{number} has new review feedback, failing checks or needs a rebase. Read the file {brief}.md beside your task brief in full, then address them as it says, {instructions}",
            brief = crate::brief::round_name(identifier),
            instructions = instructions(number, identifier, session_id, last),
        ),
        Round::Rebase => format!(
            "PR #{number} needs a rebase. Read the file {brief}.md beside your task brief in full, then rebase it as it says. Report in Linear session {session_id}.",
            brief = crate::brief::rebase_name(identifier),
        ),
    })
}

// What `line` and `fresh` share for a review round.
fn instructions(number: u64, identifier: &IssueKey, session_id: &Id, last: bool) -> String {
    let last = if last { format!(" This is the last review round: {}", last_round(identifier)) } else { String::new() };
    format!("reading them with tolkster pr feedback --pr {number} --session {session_id} and tolkster pr checks --pr {number}.{last} Report in Linear session {session_id}.")
}

pub fn brief_name(identifier: &IssueKey, round: Round) -> String {
    match round {
        Round::Review { .. } => crate::brief::round_name(identifier),
        Round::Rebase => crate::brief::rebase_name(identifier),
    }
}

// The line a new session starts an exited worker's round on, its task brief at `task` and
// the round's at `brief` (setup.rs).
pub fn fresh(number: u64, identifier: &IssueKey, task: &Path, brief: &Path, session_id: &Id, round: Round) -> String {
    let (what, ask) = match round {
        Round::Review { last } => ("a review round of", format!("PR #{number} has new review feedback, failing checks or needs a rebase: address them as the round brief says, {}", instructions(number, identifier, session_id, last))),
        Round::Rebase => ("rebasing", format!("PR #{number} needs a rebase: rebase it as that brief says. Report in Linear session {session_id}.")),
    };
    format!(
        "Read the file {brief} in full. It is your brief for {what} PR #{number}, started in a new session: the work is on the PR already. It builds on your task brief, the file {task}, which you read too, following only its While working and Finishing sections: the rest was done before the PR opened. {ask}",
        brief = brief.display(),
        task = task.display(),
    )
}

// What a last round does, as the line says it: after "This is the last review round: " on
// a resume.
pub fn last_round(identifier: &str) -> String {
    format!("fix only blocking findings and file the rest, as the file {}.md beside your task brief says.", crate::brief::last_round_name(identifier))
}

#[cfg(test)]
mod tests;
