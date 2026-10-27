use std::fmt;

use super::event::{Line, Parts, listed, pr};
use crate::error::Error;
use crate::plain::{IssueKey, WorktreeId};

// A triage start (command.rs, triage.rs).
#[derive(Debug, PartialEq)]
pub enum Triage {
    Unread(Error),
    UnreadOne,
    Refused { issue: IssueKey, why: Refusal },
    Started(Vec<TriageNote>),
}

// Why an issue asked isn't triaged (triage::may_triage).
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    NotInTriage,
    Archived,
    NotOurs,
}

#[derive(Debug, PartialEq)]
pub enum TriageNote {
    Missing { issue: IssueKey, missing: Missing },
    MovedIn { issue: IssueKey },
    // Its history couldn't be read: with Linear's budget spent the rest wait.
    History { issue: IssueKey, error: Error, rest_wait: bool },
    TriagedAlready(usize),
    Started(Vec<IssueKey>),
}

// What an issue's repo lacks to work in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Missing {
    Repo,
    Primary,
}

// A completed issue's context session (context.rs).
#[derive(Debug, PartialEq)]
pub enum Context {
    NotCompleted { issue: IssueKey },
    NoneOpen { issue: IssueKey },
    NotMine { issue: IssueKey },
    Missing { issue: IssueKey, missing: Missing },
    Already { issue: IssueKey },
    Started { issue: IssueKey, issues: Vec<IssueKey> },
}

// What a reap did with `issue`'s worktree (reap.rs): a line of `listen`'s, and of `tolkster
// reap`'s and `reap-here`'s output, which show it whole.
#[derive(Debug, PartialEq)]
pub struct Reap {
    pub issue: String,
    pub what: Reaping,
}

#[derive(Debug, PartialEq)]
pub enum Reaping {
    Removed { path: String, branch: String, why: Why, branch_kept: bool },
    RemovedOnHost { path: String, branch: String, host: String, why: Why },
    GoneFromHost { path: String, host: String, why: Why },
    Kept { path: String, rule: Rule, because: Keep },
    HostKept { path: String, host: String, why: Why },
    NoHostPrimary { path: String, host: String },
    WouldRemove { path: String, branch: String, rule: Rule, closing: bool },
    WouldAskHost { path: String, branch: String, host: String, why: Why },
    ClosedWaits { path: String, pr: u64 },
    NoWorktreeHere { branch: String },
    RuleNotBorne { branch: String },
    AgentRunning,
    GoneRecordStays { worktree_id: WorktreeId, error: Error },
    Failed(Error),
}

// The rule a worktree was reaped on, and what came of it on the way.
#[derive(Clone, Debug, PartialEq)]
pub struct Why {
    pub rule: Rule,
    pub closed_agent: bool,
    pub record_stays: Option<Error>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    Merged(u64),
    Landed,
    Cancelled,
    Completed,
}

// Why a worktree a rule would reap stays.
#[derive(Debug, PartialEq, Eq)]
pub enum Keep {
    AtOther { local: String },
    OffDefault { local: String },
    Dirty { first: String },
    BranchGone { branch: String },
    Unpushed(u64),
    Uncounted,
}

// As an issue's: `TW-1 is archived`.
impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Refusal::NotInTriage => "isn't in Triage",
            Refusal::Archived => "is archived",
            Refusal::NotOurs => "wasn't created by the owner or by me",
        })
    }
}

impl Triage {
    pub(super) fn line(&self) -> Line {
        Line::routine(Parts::of("triage: ").then(match self {
            Triage::Unread(error) => Parts::of("the issues asked couldn't be read").error(error),
            Triage::UnreadOne => Parts::of("an issue asked wasn't read"),
            Triage::Refused { issue, why } => Parts::of(format!("{issue} {why}, so no triage")),
            Triage::Started(notes) if notes.is_empty() => Parts::of("nothing to triage"),
            Triage::Started(notes) => notes.iter().enumerate().fold(Parts::default(), |parts, (n, note)| parts.text(if n == 0 { "" } else { "; " }).then(triage_note(note))),
        }))
    }
}

fn triage_note(note: &TriageNote) -> Parts {
    match note {
        TriageNote::Missing { issue, missing: why } => Parts::of(format!("{issue}: {}, so no triage", self::missing(why))),
        TriageNote::MovedIn { issue } => Parts::of(format!("{issue} was moved into Triage after it was created, so no triage")),
        TriageNote::History { issue, error, rest_wait } => Parts::of(format!("{issue}: its history couldn't be read")).error(error).text(if *rest_wait { ", so the rest wait" } else { ", so no triage" }),
        TriageNote::TriagedAlready(count) => Parts::of(format!("{count} triaged already")),
        TriageNote::Started(issues) => Parts::of(format!("started triage for {}", listed(issues))),
    }
}

fn missing(missing: &Missing) -> &'static str {
    match missing {
        Missing::Repo => "no repo for its project or team",
        Missing::Primary => "its repo has no local primary checkout",
    }
}

impl Context {
    pub(super) fn line(&self) -> Line {
        Line::routine(Parts::of(match self {
            Context::NotCompleted { issue } => format!("{issue}: not completed, so no context"),
            Context::NoneOpen { issue } => format!("{issue}: no open related or sibling issues, so no context"),
            Context::NotMine { issue } => format!("{issue}: not mine, so no context"),
            Context::Missing { issue, missing } => format!("{issue}: {}, so no context", self::missing(missing)),
            Context::Already { issue } => format!("{issue}: context started already"),
            Context::Started { issue, issues } => format!("{issue}: started context for {}", listed(issues)),
        }))
    }
}

impl Reap {
    pub(super) fn line(&self) -> Line {
        let parts = Parts::of(format!("{}: ", self.issue)).then(self.what.parts());
        // A worktree gone with its worker record left is a write half done.
        let half = match &self.what {
            Reaping::Removed { why, .. } | Reaping::RemovedOnHost { why, .. } | Reaping::GoneFromHost { why, .. } => why.record_stays.is_some(),
            what => matches!(what, Reaping::Failed(_) | Reaping::GoneRecordStays { .. }),
        };
        if half { Line::error(parts) } else { Line::routine(parts) }
    }
}

// Whole, as `tolkster reap` prints it and an error carries it.
impl fmt::Display for Reap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.issue, self.what.parts().render(true))
    }
}

impl fmt::Display for Why {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.parts().render(true))
    }
}

impl Why {
    pub fn of(rule: Rule) -> Why {
        Why { rule, closed_agent: false, record_stays: None }
    }

    fn parts(&self) -> Parts {
        let parts = Parts::of(rule(self.rule));
        let parts = if self.closed_agent { parts.text("; closed its idle agent") } else { parts };
        match &self.record_stays {
            Some(error) => parts.text("; its worker record stays").error(error),
            None => parts,
        }
    }
}

fn rule(rule: Rule) -> String {
    match rule {
        Rule::Merged(number) => format!("{} merged", pr(number)),
        Rule::Landed => "work landed on the default branch".into(),
        Rule::Cancelled => "issue cancelled".into(),
        Rule::Completed => "issue completed with no PR".into(),
    }
}

impl Reaping {
    fn parts(&self) -> Parts {
        let why = |why: &Why| Parts::of(" (").then(why.parts()).text(")");
        match self {
            Reaping::Removed { path, branch, why: w, branch_kept: false } => Parts::of(format!("removed {path} and branch {branch}")).then(why(w)),
            Reaping::Removed { path, branch, why: w, branch_kept: true } => Parts::of(format!("removed {path}")).then(why(w)).text(format!("; kept branch {branch}: no local primary checkout")),
            Reaping::RemovedOnHost { path, branch, host, why: w } => Parts::of(format!("removed {path} and branch {branch} on {host}")).then(why(w)),
            Reaping::GoneFromHost { path, host, why: w } => Parts::of(format!("{path} is gone from {host}")).then(why(w)),
            Reaping::Kept { path, rule: r, because } => {
                let pr_may_be = if *r == Rule::Cancelled { ", but its branch is on origin with work, so a PR may be open, and " } else { ", but " };
                Parts::of(format!("kept {path}: {}{pr_may_be}{}", rule(*r), keep(because)))
            }
            Reaping::HostKept { path, host, why: w } => Parts::of(format!("kept {path}: {host}'s checks kept it")).then(why(w)),
            Reaping::NoHostPrimary { path, host } => Parts::of(format!("kept {path}: {host} lists no primary checkout of its repo to reap it from")),
            Reaping::WouldRemove { path, branch, rule: r, closing } => {
                let closing = if *closing { "close its idle agent and " } else { "" };
                Parts::of(format!("would {closing}remove {path} and branch {branch} ({})", rule(*r)))
            }
            Reaping::WouldAskHost { path, branch, host, why: w } => Parts::of(format!("would ask {host} to check {path}, then remove it and branch {branch}")).then(why(w)),
            Reaping::ClosedWaits { path, pr: number } => Parts::of(format!("closed the idle agent in {path} ({} merged); its terminal is still listed, so the worktree waits", pr(*number))),
            Reaping::NoWorktreeHere { branch } => Parts::of(format!("no worktree of its on branch {branch} here")),
            Reaping::RuleNotBorne { branch } => Parts::of(format!("the PRs on branch {branch} don't bear the rule out, so the worktree stays")),
            Reaping::AgentRunning => Parts::of("its agent is running, so the worktree stays"),
            Reaping::GoneRecordStays { worktree_id, error } => Parts::of("its worktree").opaque(worktree_id).text(" is gone, but its worker record stays").error(error),
            Reaping::Failed(error) => Parts::of("error").error(error),
        }
    }
}

fn keep(keep: &Keep) -> String {
    match keep {
        Keep::AtOther { local } => format!("the checkout is at {local}, not the PR's head"),
        Keep::OffDefault { local } => format!("the checkout is at {local}, which origin's default branch lacks"),
        Keep::Dirty { first } => format!("the checkout has uncommitted or ignored files, e.g. {first}"),
        Keep::BranchGone { branch } => format!("its local branch {branch} is gone, so the commits on no remote weren't counted"),
        Keep::Unpushed(count) => format!("{count} commits on HEAD or the branch are on no remote"),
        Keep::Uncounted => "git didn't count the commits on no remote".into(),
    }
}
