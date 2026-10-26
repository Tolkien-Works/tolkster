// A worktree on another host is reaped there: git here can't read its checkout, and Orca
// holds no checkout's state. Its rule goes to that host's `tolkster reap-here` (`Step`), in a
// terminal opened in the host's primary checkout, where a stop, which closes every terminal in
// the worker's worktree, can't close it. That step runs the rule's guards as reap.rs lists
// them, on its own listing and git, removes the worktree and deletes the branch in its own
// primary checkout.
// Its exit code says kept, and only the listing read here after it says removed, since a
// shell that never ran it may exit 0 too. No agent of another host's is closed from here.
// Anyone on that host can run the step, so it takes only the worktree from its line: the rule
// must hold by the branch's PRs and Linear as that host reads them, a cancel a day old.

use crate::args::{Kind, Parsed};
use crate::error::Error;
use crate::github::PrState;
use crate::orca::{Orca, Worktree, candidates};
use crate::plain::{BranchName, CommandLine, IssueKey, Line, Sha, Time};
use crate::sessions::{IssueNow, Tolksters};
use crate::trace::{Reap, Reaping};

use super::{Prs, Rule, Verdict, joined, reap_by, verdict};

// The exit code of a `Step` that kept its worktree, or found none to reap: nothing to do.
pub const KEPT: i32 = 3;

// A rule sent to a worktree's own host, as the line `tolkster reap-here` runs there: the
// issue's worktree on `branch` is the one it reaps. IDs, a SHA, a time and flags only, so any
// host's shell reads the line as it was written, and no Linear or GitHub text.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub issue: IssueKey,
    pub branch: BranchName,
    pub rule: Rule,
}

pub const STEP_OPTIONS: [(&str, Kind); 9] = [
    ("issue", Kind::String),
    ("branch", Kind::String),
    ("merged", Kind::String),
    ("landed", Kind::Boolean),
    ("head", Kind::String),
    ("cancelled", Kind::Boolean),
    ("completed", Kind::Boolean),
    ("at", Kind::String),
    ("no-pr", Kind::Boolean),
];

impl Step {
    pub fn command(&self) -> Result<CommandLine, Error> {
        let mut words = vec![format!("tolkster reap-here --issue {} --branch {}", self.issue, self.branch)];
        let at = |at: &Option<String>| at.as_deref().map(|at| Time::check(at).map(|at| format!("--at {at}"))).transpose();
        match &self.rule {
            Rule::Merged { number, head } => words.push(format!("--merged {number} --head {}", Sha::check(head)?)),
            Rule::Landed => words.push("--landed".into()),
            Rule::Cancelled { at: when, no_pr } => {
                words.push("--cancelled".into());
                words.extend(at(when)?);
                words.extend(no_pr.then(|| "--no-pr".to_string()));
            }
            Rule::Completed { at: when } => {
                words.push("--completed".into());
                words.extend(at(when)?);
            }
        }
        Ok(CommandLine::new(&Line::check(&words.join(" "))?).exiting())
    }

    pub fn parse(parsed: &Parsed) -> Result<Step, Error> {
        let usage = |what: &str| Error::Usage(format!("reap-here: {what}"));
        let named = |option: &str| parsed.non_empty(option);
        let (Some(issue), Some(branch)) = (named("issue"), named("branch")) else { return Err(usage("needs --issue and --branch")) };
        let at = named("at").map(|at| Time::check(at).map(String::from)).transpose()?;
        let rule = match (named("merged"), parsed.flag("landed"), parsed.flag("cancelled"), parsed.flag("completed")) {
            (Some(number), false, false, false) => {
                let number = number.parse().map_err(|_| usage("--merged takes a PR's number"))?;
                let head = named("head").map(Sha::check).transpose()?.ok_or_else(|| usage("--merged needs --head <sha>"))?;
                Rule::Merged { number, head: head.into() }
            }
            (None, true, false, false) => Rule::Landed,
            (None, false, true, false) => Rule::Cancelled { at, no_pr: parsed.flag("no-pr") },
            (None, false, false, true) => Rule::Completed { at },
            _ => return Err(usage("pass one of --merged, --landed, --cancelled or --completed")),
        };
        Ok(Step { issue: IssueKey::check(issue)?, branch: BranchName::check(branch)?, rule })
    }
}

// Reaps on this host, on `step`'s rule, its issue's worktree on its branch, as `reap_by` does,
// forgetting no record: the host that sent it does, once it sees the worktree gone. The rule
// holds only as `prs`, the branch's on GitHub, say: the PR it names merged at its head, or for
// a cancel or a landing no PR open, for a completion none at all; and a cancel, completion or
// landing only as Linear has it now, a cancel a day old, a landing's issue completed. What it
// did, and whether it removed the worktree; Err where its turn failed.
pub fn reap_here(orca: &mut dyn Orca, step: &Step, prs: &mut Prs, mut tolksters: impl FnMut(&[IssueKey]) -> Result<Tolksters, Error>) -> Result<(Vec<Reap>, bool), Error> {
    let reap = |what| Reap { issue: step.issue.to_string(), what };
    let worktrees = orca.worktrees()?;
    let own = |w: &&Worktree| w.is_local && w.linked_issue.as_deref().is_some_and(|i| i.eq_ignore_ascii_case(&step.issue)) && w.branch.as_deref() == Some(step.branch.as_str());
    let Some(worktree) = candidates(&worktrees).into_iter().find(own) else {
        return Ok((vec![reap(Reaping::NoWorktreeHere { branch: step.branch.to_string() })], false));
    };
    let on_branch = prs(worktree, &step.branch)?;
    let open = on_branch.iter().any(|pr| pr.state == PrState::Open);
    let rule = match &step.rule {
        Rule::Merged { number, head } if verdict(&on_branch) == (Verdict::Merged { number: *number, head: head.clone() }) => step.rule.clone(),
        // Linear has the issue done, and its own git bears the rest out (`reap_landed`): a landed
        // worktree there waits, retried, until its issue is moved to Done.
        Rule::Landed if !open && matches!(tolksters(std::slice::from_ref(&step.issue))?.get(step.issue.as_str()), Some(IssueNow::Completed(_))) => Rule::Landed,
        Rule::Cancelled { no_pr, .. } if !open => Rule::Cancelled { at: None, no_pr: *no_pr },
        Rule::Completed { .. } if on_branch.is_empty() => Rule::Completed { at: None },
        _ => return Ok((vec![reap(Reaping::RuleNotBorne { branch: step.branch.to_string() })], false)),
    };
    let reaped = reap_by(orca, &mut |_| Ok(()), worktree, &worktrees, rule, tolksters)?;
    if reaped.failed {
        return Err(Error::Failed(joined(&reaped.lines)));
    }
    Ok((reaped.lines, reaped.removed))
}
