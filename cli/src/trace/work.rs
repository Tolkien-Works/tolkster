use std::time::Duration;

use super::event::{Line, Parts, Show, pr};
use super::{Kind, Mark};
use crate::error::Error;
use crate::machine::{Outcome as Answer, Waiting};
use crate::plain::{Id, IssueKey};

// What carrying out a command came to on the way.
#[derive(Debug, PartialEq)]
pub enum Work {
    // It failed, or its order didn't read.
    Failed(Error),
    // A transient failure, tried again on the command's resend.
    Transient(Error),
    NoRepo { issue: IssueKey },
    HasWorktree { issue: IssueKey },
    NoWorktree { issue: IssueKey, purpose: Purpose },
    NoWorkerRecord { issue: IssueKey, pr: Option<u64> },
    // By its path.
    NotItsWorktree { path: String, issue: IssueKey },
    NoNoteThread { session_id: Id, issue: IssueKey },
    NoSession,
    NoHandle { issue: IssueKey, pr: u64 },
    // A message's worktree has no agent's terminal to type it to.
    NoAgent { issue: IssueKey },
    // Something short of the command that didn't happen, which the command goes on without.
    NotDone { what: Undone, error: Error },
    // The open turn wasn't handed the request, so it was closed and a new one opens.
    HandoverFailed { thread: Option<Id>, error: Error },
    Started { mode: Mode, rung: String, effort: String, worktree: String, host: Option<String>, problems: Vec<Problem> },
    InThread(Turn),
    // The conversations checkout couldn't be brought up to date, so the turn is told it may be behind.
    Behind { issue: IssueKey, error: Error },
    // A post or a reply a run before this one made.
    PostedAlready { key: String, error: Error },
    RepliedAlready { key: String, error: Error },
    UpdateRefused { repo: String, pr: u64 },
    MergeRefused { repo: String, pr: u64 },
    // A held job, tried again after `wait`.
    Retrying { issue: Option<IssueKey>, kind: Kind, job: String, wait: Duration },
    // A command's result, whose connection has gone: its resend gets one.
    ResultHeld { command: String },
    // Why a queued request waits, by the blockers' identifiers when it is on them.
    Queued { waiting: Waiting, blocked_by: Vec<String> },
    // A re-read of config.json that differs from the one in use, by the keys it changed.
    ConfigChanged { changes: Vec<String> },
    // A re-read that failed, so the last config stays in use.
    ConfigKept(Error),
}

// The most blockers a queued line names.
const NAMED: usize = 5;

#[derive(Debug, PartialEq, Eq)]
pub enum Purpose {
    Resume,
    Round(u64),
    Branch(u64),
    Message,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Undone {
    PickupWording,
    Pickup,
    OwnerTold,
    Rung,
    IssueStarted,
    Report,
    TurnClosed,
    CardStatus,
    Timing,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    Work,
    SubIssues,
    PlanFirst,
}

// What went wrong in a start short of failing it.
#[derive(Debug, PartialEq)]
pub enum Problem {
    NotNamed(Error),
    FirstThought(Error),
    Rung(RungNote),
}

// Why a worker's labels didn't pick its rung (config/ladder.rs). No label's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RungNote {
    // A `model:` or `effort:` label, by its key.
    Unmatched(&'static str),
    Agent,
    Apart,
}

// What a thread's turn came to (converse::InThread).
#[derive(Debug, PartialEq)]
pub enum Turn {
    Unanswered(&'static str),
    NoRepo { posted: bool },
    Answered,
    Opened { behind: Option<Error> },
}

// What a job came to, as its summing-up line.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Answered(Answer),
    // A line typed into a worker whose turn wasn't seen begin: `failed` to the machine, so it
    // is never typed again, though it likely arrived.
    Unconfirmed,
    Panicked,
    // An observation queued for the Durable Object.
    Reported,
    NothingToDo,
    Refreshed,
    Failed,
    // A command given back while its worker's host is made, woken or put to sleep.
    Awaits,
    Error(Error),
}

// A conversations checkout brought up to date, or not (converse::refresh_idle), by its repo's
// Orca ID.
#[derive(Debug, PartialEq)]
pub enum Refresh {
    Refreshed { repo_id: String },
    Failed { repo_id: String, error: Error },
    Unlisted { repo_id: String, error: Error },
}

// A typed line whose turn wasn't seen begin, as the log and the panel say it.
pub const UNCONFIRMED: &str = "typed, not confirmed";

const BEHIND: &str = "refreshing the conversations checkout failed, so the turn is told it may be behind";

impl Work {
    pub(super) fn line(&self) -> Line {
        match self {
            Work::Failed(error) => Line::error(Parts::of("failed").error(error)),
            Work::Transient(error) => Line::warning(Parts::of("failed, tried again on its resend").error(error)),
            Work::NoRepo { issue } => Line::warning(Parts::of(format!("{issue} maps to no repo"))),
            Work::HasWorktree { issue } => Line::routine(Parts::of(format!("{issue} has its worktree already"))),
            Work::NoWorktree { issue, purpose: Purpose::Resume } => Line::warning(Parts::of(format!("{issue} has no worktree to resume"))),
            Work::NoWorktree { issue, purpose: Purpose::Round(number) } => Line::warning(Parts::of(format!("{issue} has no worktree for {}", pr(*number)))),
            Work::NoWorktree { issue, purpose: Purpose::Branch(number) } => Line::warning(Parts::of(format!("{issue} has no worktree branch for {}", pr(*number)))),
            Work::NoWorktree { issue, purpose: Purpose::Message } => Line::warning(Parts::of(format!("{issue} has no worktree to deliver a message to"))),
            Work::NoWorkerRecord { issue, pr: number } => {
                let of = number.map(|n| format!(" for {}", pr(n))).unwrap_or_default();
                Line::warning(Parts::of(format!("{issue}'s worktree has no worker's record{of}")))
            }
            Work::NotItsWorktree { path, issue } => Line::warning(Parts::of(format!("{path} isn't a worktree of {issue}"))),
            Work::NoNoteThread { session_id, issue } => Line::warning(Parts::of("a session").opaque(session_id.as_str()).text(format!(" is no note's thread on {issue}, so nothing said"))),
            Work::NoSession => Line::warning(Parts::of("no session to report the round in")),
            Work::NoHandle { issue, pr: number } => Line::warning(Parts::of(format!("{issue}'s agent has no handle to type {}'s round to", pr(*number)))),
            Work::NoAgent { issue } => Line::routine(Parts::of(format!("{issue}'s worktree has no agent to deliver a message to"))),
            Work::NotDone { what, error } => Line::error(Parts::of(undone(what)).error(error)),
            Work::HandoverFailed { thread, error } => {
                let parts = match thread {
                    Some(thread) => Parts::of("a thread").opaque(thread.as_str()).text(": handing"),
                    None => Parts::of("handing"),
                };
                Line::warning(parts.text(" the request to the open turn failed, so it is closed for a new one").error(error))
            }
            Work::Started { mode, rung, effort, worktree, host, problems } => {
                let mode = match mode {
                    Mode::Work => "work",
                    Mode::SubIssues => "work, through its sub-issues",
                    Mode::PlanFirst => "work, planning first",
                };
                let parts = Parts::of(format!("started a worker ({mode}, {rung}, {effort} effort) in {worktree} on {}", host.as_deref().unwrap_or("this machine")));
                Line::routine(problems.iter().fold(parts, |parts, problem| parts.text("; ").then(self::problem(problem))))
            }
            Work::InThread(turn) => Line::routine(match turn {
                Turn::Unanswered(why) => Parts::of(format!("no turn: {why}")),
                Turn::NoRepo { posted: true } => Parts::of("no repo for its issue, which I said in the thread"),
                Turn::NoRepo { posted: false } => Parts::of("no repo for its issue, already said"),
                Turn::Answered => Parts::of("no reply to answer"),
                Turn::Opened { behind: None } => Parts::of("opened a turn in the thread"),
                Turn::Opened { behind: Some(error) } => Parts::of("opened a turn in the thread; ").text(BEHIND).error(error),
            }),
            Work::Behind { issue, error } => Line::routine(Parts::of(format!("{issue}: {BEHIND}")).error(error)),
            Work::PostedAlready { key, error } => Line::routine(Parts::of("posted already").opaque(key).error(error)),
            Work::RepliedAlready { key, error } => Line::routine(Parts::of("replied already").opaque(key).error(error)),
            Work::UpdateRefused { repo, pr: number } => Line::warning(Parts::of(format!("{} in {repo}: GitHub refused the branch's update", pr(*number)))),
            Work::MergeRefused { repo, pr: number } => Line::warning(Parts::of(format!("{} in {repo}: GitHub refused its merge", pr(*number)))),
            Work::Retrying { issue, kind, job, wait } => {
                let line = Line::routine(Parts::of(kind.label()).opaque(job).text(format!(": trying again in {} s", wait.as_secs())));
                match issue {
                    Some(issue) => line.issue(issue),
                    None => line,
                }
            }
            Work::ResultHeld { command } => Line::routine(Parts::of("a command").opaque(command).text("'s connection has gone; its result waits for the resend")),
            Work::ConfigChanged { changes } => Line::routine(Parts::of(format!("config.json changed: {}", changes.join("; ")))).area("config"),
            Work::ConfigKept(error) => Line::warning(Parts::of("config.json wasn't applied: it didn't read or failed its checks, so the last config stays in use").error(error)).area("config"),
            Work::Queued { waiting, blocked_by } => Line::routine(Parts::of(format!("queued: {}", queued_reason(*waiting, blocked_by)))),
        }
    }
}

// Why a queued request waits, as its line says it after `queued: ` and the panel's row whole.
pub fn queued_reason(waiting: Waiting, blocked_by: &[String]) -> String {
    match waiting {
        Waiting::Slot => "waiting for a slot".into(),
        Waiting::Held => "waiting for a held worker".into(),
        Waiting::Merged => "waiting for a reap".into(),
        Waiting::Blockers if blocked_by.is_empty() => "blocked".into(),
        Waiting::Blockers => {
            let more = blocked_by.len().saturating_sub(NAMED);
            let more = if more > 0 { format!(" and {more} more") } else { String::new() };
            format!("blocked by {}{more}", blocked_by.iter().take(NAMED).map(String::as_str).collect::<Vec<_>>().join(", "))
        }
    }
}

fn undone(what: &Undone) -> &'static str {
    match what {
        Undone::PickupWording => "the pickup's wording wasn't posted",
        Undone::Pickup => "the pickup wasn't posted",
        Undone::OwnerTold => "telling the owner failed",
        Undone::Rung => "its new rung wasn't recorded",
        Undone::IssueStarted => "the issue wasn't moved to started",
        Undone::Report => "reporting to Tolkster's state failed",
        Undone::TurnClosed => "closing the turn failed",
        Undone::CardStatus => "the worktree card's status wasn't set",
        Undone::Timing => "a line of the timing file wasn't written",
    }
}

fn problem(problem: &Problem) -> Parts {
    match problem {
        Problem::NotNamed(error) => Parts::of("naming it failed").error(error),
        Problem::FirstThought(error) => Parts::of("posting its first thought failed").error(error),
        Problem::Rung(RungNote::Unmatched(key)) => Parts::of(format!("a {key}: label matches no rung of the ladder, so it runs on the default rung")),
        Problem::Rung(RungNote::Agent) => Parts::of("an agent: label names a harness no rung runs on; ignored"),
        Problem::Rung(RungNote::Apart) => Parts::of("the agent:, model: and effort: labels match no rung together, so it runs on the default rung"),
    }
}

impl Outcome {
    pub(super) fn line(&self) -> Line {
        let (mark, parts) = match self {
            Outcome::Answered(answer) => match answer {
                Answer::Ok => (Mark::Quiet, Parts::of("ok")),
                Answer::Held => (Mark::Routine, Parts::of("held")),
                Answer::Gone => (Mark::Routine, Parts::of("gone")),
                Answer::Failed => (Mark::Error, Parts::of("failed")),
                Answer::Retryable => (Mark::Warning, Parts::of("retryable")),
            },
            Outcome::Unconfirmed => (Mark::Warning, Parts::of(UNCONFIRMED)),
            Outcome::Panicked => (Mark::Error, Parts::of("panicked")),
            Outcome::Reported => (Mark::Quiet, Parts::of("reported")),
            Outcome::NothingToDo => (Mark::Quiet, Parts::of("nothing to do")),
            Outcome::Refreshed => (Mark::Quiet, Parts::of("refreshed")),
            Outcome::Failed => (Mark::Error, Parts::of("failed")),
            Outcome::Awaits => (Mark::Routine, Parts::of("waits for a machine")),
            Outcome::Error(error) => (Mark::Error, Parts::of("error").error(error)),
        };
        Line::new(mark, parts).show(Show::Outcome)
    }
}

impl Refresh {
    pub(super) fn line(&self) -> Line {
        match self {
            Refresh::Refreshed { repo_id } => Line::routine(Parts::of("refreshed a repo's conversations checkout").opaque(repo_id)).show(Show::Verbose),
            Refresh::Failed { repo_id, error } => Line::error(Parts::of("refreshing a repo's conversations checkout").opaque(repo_id).text(" failed").error(error)),
            Refresh::Unlisted { repo_id, error } => Line::error(Parts::of("a repo's conversations checkout").opaque(repo_id).text(" wasn't refreshed: listing its terminals failed").error(error)),
        }
    }
}
