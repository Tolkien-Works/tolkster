// `tolkster reap`: removes the Orca worktrees of Tolkster workers whose PR has merged, whose
// issue was cancelled a day ago, or whose issue was completed with no PR, and the machine's
// `reapWorktree` those whose work landed on the default branch. Orca is asked
// through its port (`orca::Orca`), `git` through the port's shell, and a branch's PRs come
// through `Prs`: read from GitHub as the app, once per branch (github::branch_prs). So the
// rules below can be tested with fakes, without Orca, GitHub or a network. Only `reapWorktree`, `tolkster reap` and
// `tolkster reap-here` use it.
// `tolkster reap` decides which rule applies to each worktree from GitHub and Linear. A
// `reapWorktree` command carries the rule the machine decided (`Rule`, `reap_by`), so it reads
// no PR and keeps no clock, and runs only that rule's guards below, each as listed.
//
// A merged worker's worktree is removed only when all of these hold:
// - it is linked to an issue that is Tolkster's (`tolksters` says which): delegated to
//   Tolkster, or with a Tolkster session the owner started, which is how a mention on
//   an undelegated issue shows. Worktrees linked to other issues are never touched;
// - a PR for its branch is MERGED and none is open. Closed-unmerged, a failed merge or
//   Linear's Done alone never count;
// - no agent is working in it: auto-merge can merge the PR while its worker is still
//   finishing, or while a resumed one works, and the worktree waits for it to exit. An
//   agent that has sat idle for `IDLE_FOR_MS`, as Orca has it from the agent's own hooks
//   (watch::idle_in: done with its turn, or waiting at a prompt), is closed first, once
//   the checks below have passed and Orca has been read again just before, so a worker
//   that never closed its terminal doesn't keep its worktree for ever. Its transcript
//   stays on disk, but the worker's context goes with the worktree, as for any reaped
//   one: a later reply on the completed issue is a conversation turn in the shared
//   checkout, not the old worker resumed;
// - its checkout is clean and at the merged PR's head, so nothing the PR didn't carry
//   is lost. `--force` deletes ignored files too, so an ignored file such as `.env`
//   keeps the worktree; ignored directories (`target/`, `node_modules/`) are build
//   output and don't.
//
// A landed worker's worktree, whose change its worker pushed to the default branch with no PR,
// is removed on the machine's rule alone, when all of these hold:
// - it is linked to an issue that is Tolkster's, as above;
// - no agent is running in it: the machine closes an idle or held one first, as for a merge;
// - origin fetched first, its checkout is clean, as for the merged rule, its HEAD is on
//   origin's default branch, and no commit at HEAD or on its branch is on no remote, as its
//   remote-tracking refs have it. It names no commit, so a landing pushed again, or after a
//   PR merged, is reaped all the same.
// No PR merged its branch, so Orca keeps it, and it goes with the worktree.
//
// A cancelled issue's worktree is removed when all of these hold:
// - it is linked to one of Tolkster's issues, as above, whose state is `canceled` or
//   `duplicate` as Linear says now: cancellation is never stored;
// - it was cancelled at least `CANCELLED_FOR_MS` ago, so reopening a mis-click still
//   finds the worker to resume;
// - no agent is running in it, and no PR for its branch is open: an open PR keeps it,
//   and Tolkster never closes one.
// Its checkout isn't checked: a cancelled worker was usually mid-task, and the owner
// cancelled that work. What it pushed stays on the remote branch. On the machine's rule,
// which reads no PR, the cancel Linear has now must be a day old too; and where its store
// holds no PR, a checkout whose branch is on origin with commits its default branch lacks,
// as a PR the store missed would be, stays unclean or with commits on no remote.
//
// A completed issue's worktree whose branch has no PR, such as a parent's worked through
// its sub-issues (docs/parent-issues.md), is removed when all of these hold:
// - it is linked to one of Tolkster's issues, as above, whose state is `completed` as
//   Linear says now, read again just before removal;
// - no PR for its branch exists, in any state: one that does is the merged rule's to
//   decide, or keeps the worktree;
// - no agent terminal is open in it, idle or not: nothing is closed for this rule;
// - its checkout is clean, as for the merged rule, and every commit at its HEAD and on its
//   branch, which is deleted with it, is on a remote (`git rev-list HEAD refs/heads/<branch>
//   --not --remotes` lists none), so no work goes with it.
//
// A worktree on another host is reaped there, by its own `tolkster reap-here` (reap/step.rs).

use std::time::Duration;

use crate::exec::Run;
use crate::watch::idle_in;
use crate::error::{Error, Waits};
use crate::git;
use crate::machine::ReapRule;
use crate::github::{BranchPr, Branches, PrState, Reader};
use crate::orca::{Orca, Worktree, agent_in, agent_terminal, candidates, primary_checkout};
use crate::plain::{BranchName, IssueKey, Terminal};
use crate::ports::Ports;
use crate::trace::{self, Keep, Reap, Reaping, Rule as Reason, Why};
use crate::sessions::{IssueNow, Tolksters};
use crate::time::{iso, now_ms};

// Forgets the worker record of a worktree, by its ID, once the worktree is removed (see
// state.rs, `WorkerRecord`).
pub type Forget<'a> = dyn FnMut(&str) -> Result<(), Error> + 'a;

// The PRs for a worktree's branch, as `verdict` reads them: every one, open or not.
pub type Prs<'a> = dyn FnMut(&Worktree, &str) -> Result<Vec<BranchPr>, Error> + 'a;

// `Prs` that reads each branch from GitHub, once per branch for the run (`Branches`).
pub fn read_prs<'a>(gh: &'a mut dyn Reader, branches: &'a mut Branches) -> impl FnMut(&Worktree, &str) -> Result<Vec<BranchPr>, Error> + 'a {
    move |worktree, branch| Ok(branches.prs(gh, worktree.origin()?, branch)?.to_vec())
}

// How long an issue stays cancelled before its worktree goes: a day.
pub const CANCELLED_FOR_MS: i64 = 86_400_000;

// How far the workstation's clock may sit behind the Durable Object's, which sends the
// machine's cancelled reap a day after the cancel it saw.
const SKEW_MS: i64 = 3_600_000;

// How long an agent must have sat idle before reap closes it in a merged PR's worktree, so a
// turn that has only just ended, or a line just sent to it, is left alone.
pub const IDLE_FOR_MS: i64 = 10 * 60 * 1000;

// How often, and how many times, `remove` asks git whether it still lists a removed worktree.
const UNLISTED_POLL: Duration = if cfg!(test) { Duration::ZERO } else { Duration::from_millis(250) };
const UNLISTED_POLLS: u32 = 20;

// What in `git status --porcelain --ignored` would be lost with the checkout: anything
// but an ignored directory or one of `harness::LEFTOVERS`, which only counts when ignored.
pub fn would_lose(status: &str) -> Vec<&str> {
    let kept = |line: &str| !line.starts_with("!! ") || !(line.ends_with('/') || crate::harness::LEFTOVERS.contains(&&line[3..]));
    status.lines().filter(|line| !line.trim().is_empty() && kept(line)).map(|line| line.get(3..).unwrap_or(line)).collect()
}

// The rule a worktree is reaped on, and what it names of the decision. `at`, on the other
// two, is when Linear said the issue was cancelled or completed at the first read, which the
// read just before removal must repeat; None where the machine decided, and that read need
// only find the state still so. `no_pr` is the machine's store holding no PR for the issue,
// which it may have missed; false where a PR was read or held.
#[derive(Clone, Debug, PartialEq)]
pub enum Rule {
    Merged { number: u64, head: String },
    Landed,
    Cancelled { at: Option<String>, no_pr: bool },
    Completed { at: Option<String> },
}

impl From<&ReapRule> for Rule {
    fn from(rule: &ReapRule) -> Rule {
        match rule {
            ReapRule::Merged { number, head } => Rule::Merged { number: *number, head: head.to_string() },
            ReapRule::Landed => Rule::Landed,
            ReapRule::Cancelled { no_pr } => Rule::Cancelled { at: None, no_pr: *no_pr },
            ReapRule::Completed => Rule::Completed { at: None },
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Verdict {
    // Merged, and nothing is open: the worktree may go once its checkout is checked.
    Merged { number: u64, head: String },
    // Not done yet, or never will be: leave it, quietly.
    Keep,
}

// What a branch's PRs, every one, come to for its worktree.
pub fn verdict(prs: &[BranchPr]) -> Verdict {
    // A branch reused for a second PR: the open one is still in review.
    if prs.is_empty() || prs.iter().any(|pr| pr.state == PrState::Open) {
        return Verdict::Keep;
    }
    prs.iter()
        .filter(|pr| pr.state == PrState::Merged)
        .max_by_key(|pr| pr.number)
        .map_or(Verdict::Keep, |pr| Verdict::Merged { number: pr.number, head: pr.head.clone() })
}

// The first thing in the checkout that removing it would lose, if any (see `would_lose`).
fn dirty(run: &mut Run, path: &str) -> Result<Option<String>, Error> {
    let status = git::status_porcelain(run, path)?;
    Ok(would_lose(&status).first().map(|first| first.to_string()))
}

// What `reap` did: a line per worktree removed, kept against a merged PR, or failed, and
// whether it tried to remove any, which leaves Orca's listing out of date. `tried` counts
// the worktrees it tried to remove (`Reaper::remove`). `failed` says one worktree's turn
// ended in an error, so what wanted it gone should look again, and `spent` that one found
// GitHub's budget spent, with the wait: it should look again after it.
pub struct Reaped {
    pub lines: Vec<Reap>,
    pub failed: bool,
    pub spent: Option<Duration>,
    // A failure that decided nothing (`Error::later`), such as a host not heard: tried again,
    // once what the first of them waited on clears, if it named it.
    pub later: bool,
    pub waits: Option<Waits>,
    pub removed: bool,
    tried: usize,
}

impl Reaped {
    fn none() -> Reaped {
        Reaped { lines: Vec::new(), failed: false, spent: None, later: false, waits: None, removed: false, tried: 0 }
    }
}

// Reaps from the worktrees Orca listed, each on the rule GitHub and Linear give it. Only
// asking Linear or listing the terminals can fail the whole run.
pub fn reap(orca: &mut dyn Orca, prs: &mut Prs, forget: &mut Forget, worktrees: &[Worktree], mut tolksters: impl FnMut(&[IssueKey]) -> Result<Tolksters, Error>, dry_run: bool) -> Result<Reaped, Error> {
    let linked = candidates(worktrees);
    if linked.is_empty() {
        return Ok(Reaped::none());
    }
    let mut identifiers: Vec<IssueKey> = linked.iter().filter_map(|w| IssueKey::parse(w.linked_issue.as_deref()?)).collect();
    identifiers.sort();
    identifiers.dedup();
    let theirs_now = tolksters(&identifiers)?;
    let theirs: Vec<&Worktree> = linked.into_iter().filter(|w| w.linked_issue.as_ref().is_some_and(|i| theirs_now.contains_key(i))).collect();
    if theirs.is_empty() {
        return Ok(Reaped::none());
    }
    let terminals = orca.terminals()?;
    let now = now_ms();
    // Cancelled at or before this, an issue's worktree may go.
    let cancelled_by = iso(now.saturating_sub(CANCELLED_FOR_MS));
    let mut reaper = Reaper { orca, forget, tolksters: &mut tolksters, all: worktrees, dry_run, idle_by: iso(now.saturating_sub(IDLE_FOR_MS)), reaped: Reaped::none() };

    for worktree in theirs {
        let issue = worktree.linked_issue.as_deref().unwrap_or_default();
        let now = theirs_now.get(issue).cloned().unwrap_or(IssueNow::Open);
        // Cancelled now, as Linear says, however recently and whichever rule applies.
        let cancelled_now = match &now {
            IssueNow::Cancelled(at) => Some(at.clone()),
            _ => None,
        };
        let cancelled = cancelled_now.clone().filter(|at| *at <= cancelled_by);
        let completed = match now {
            IssueNow::Completed(at) => Some(at),
            _ => None,
        };
        // An agent running there keeps the worktree, for the machine's `reapWorktree` after
        // its exit to look again, unless it has sat idle for IDLE_FOR_MS and the PR merged:
        // then `reap_merged` closes it. A cancelled issue's agent is never closed here, however
        // recent the cancel, and whichever of `reap` or `reapWorktree` looks: its halt
        // stops it (the machine's `stopWorker`), and a mis-click reopened within the day finds it.
        let mut idle = None;
        if agent_in(&terminals, &worktree.id) {
            if worktree.host().is_some() || cancelled_now.is_some() || !idle_in(worktree, &terminals).is_some_and(|since| since <= reaper.idle_by) {
                continue;
            }
            // One with no handle to close it by is left running, and its worktree with it.
            let Some(handle) = agent_terminal(&terminals, &worktree.id) else { continue };
            idle = Some(handle);
        }
        let Some(branch) = worktree.branch.as_deref().and_then(BranchName::parse) else { continue };
        let result = prs(worktree, &branch).and_then(|prs| match decide(&prs, cancelled, completed, idle.is_some()) {
            Some(rule) => reaper.apply(worktree, &branch, rule, idle.as_ref()),
            None => Ok(None),
        });
        reaper.record(issue, result);
    }
    Ok(reaper.reaped)
}

// Reaps `worktree`, a linked issue's, on the `rule` the machine decided: no PR is read, no
// clock kept and no agent closed, and the rule's own guards run as they do for `reap`. A
// merged or landed rule holds only for an issue that is Tolkster's, which the cancelled and
// completed rules' read just before removal also finds.
pub fn reap_by(orca: &mut dyn Orca, forget: &mut Forget, worktree: &Worktree, worktrees: &[Worktree], rule: Rule, mut tolksters: impl FnMut(&[IssueKey]) -> Result<Tolksters, Error>) -> Result<Reaped, Error> {
    // A branch whose name isn't plain never reaches git, so its worktree stays.
    let (Some(issue), Some(branch)) = (worktree.linked_issue.as_deref().and_then(IssueKey::parse), worktree.branch.as_deref().and_then(BranchName::parse)) else { return Ok(Reaped::none()) };
    if matches!(rule, Rule::Merged { .. } | Rule::Landed) && !tolksters(std::slice::from_ref(&issue))?.contains_key(issue.as_str()) {
        return Ok(Reaped::none());
    }
    let mut reaper = Reaper { orca, forget, tolksters: &mut tolksters, all: worktrees, dry_run: false, idle_by: String::new(), reaped: Reaped::none() };
    let result = reaper.apply(worktree, &branch, rule, None);
    reaper.record(&issue, result);
    Ok(reaper.reaped)
}

// A `reapWorktree` command's reap, on a worktree of `issue`'s only: whether the worktree is
// gone now, reaped by this or before it, or was kept, with its agent running, unclean or
// unpushed. Err for a worktree of another issue's, which is left as it is, and Error::Later
// for one no listing vouches for either way. The machine sends
// one command at a time per issue, so no resume starts an agent there between the check and
// the removal. The machine decided `rule`; this runs only its guards (`reap_by`).
pub fn reap_for(ports: &mut Ports, worktree_id: &str, issue: &str, rule: &ReapRule) -> Result<bool, Error> {
    let (lines, reaped) = reap_once(ports, worktree_id, issue, &rule.into())?;
    lines.into_iter().for_each(trace::event);
    Ok(reaped)
}

fn reap_once(ports: &mut Ports, worktree_id: &str, issue: &str, rule: &Rule) -> Result<(Vec<Reap>, bool), Error> {
    let reap = |what| Reap { issue: issue.into(), what };
    // Its host alone holds it, and its repo's primary checkout.
    let listing = ports.orca.listing_at(worktree_id)?;
    // Gone only by its host's complete listing (orca::complete_listing): one cut off, or a
    // silent host's, decides nothing, and the reap is sent again, not reported done.
    let Some(at) = listing.worktrees.iter().position(|w| w.id == worktree_id) else {
        if !listing.lacks(worktree_id) {
            return Err(Error::unheard(crate::orca::host_of(worktree_id)));
        }
        // Another host's step may have removed it after an earlier reap stopped waiting on it.
        if let Err(error) = ports.records.forget_worker(worktree_id) {
            return Ok((vec![reap(Reaping::GoneRecordStays { worktree_id: worktree_id.into(), error })], true));
        }
        return Ok((Vec::new(), true));
    };
    if !listing.worktrees[at].linked_issue.as_deref().is_some_and(|linked| linked.eq_ignore_ascii_case(issue)) {
        return Err(Error::Failed(format!("{issue}: {worktree_id} is another issue's worktree")));
    }
    if agent_in(&listing.terminals, worktree_id) {
        return Ok((vec![reap(Reaping::AgentRunning)], false));
    }
    let mut worktrees = listing.worktrees;
    ports.orca.origins(&mut worktrees);
    let (linear, records) = (ports.linear, ports.records);
    let reaped = reap_by(ports.orca, &mut |id| records.forget_worker(id), &worktrees[at], &worktrees, rule.clone(), |identifiers| linear.tolksters(identifiers))?;
    let lines = reaped.lines;
    let message = joined(&lines);
    match reaped.spent {
        Some(wait) => Err(Error::RateLimited { message, wait }),
        None if reaped.later => Err(match reaped.waits {
            Some(waits) => Error::Waits(waits, message),
            None => Error::Later(message),
        }),
        None if reaped.failed => Err(Error::Failed(message)),
        None => Ok((lines, reaped.removed)),
    }
}

// The rule `prs`, every PR on the branch, give a worktree whose issue Linear says was
// cancelled long enough ago (`cancelled`) or completed (`completed`), if any. A cancelled
// issue's worktree stays while a PR is open. A completed one's goes only when no PR exists,
// and never by closing an agent, `idle`: any other with a PR is the merged rule's.
fn decide(prs: &[BranchPr], cancelled: Option<String>, completed: Option<String>, idle: bool) -> Option<Rule> {
    if let Some(at) = cancelled {
        return (!prs.iter().any(|pr| pr.state == PrState::Open)).then_some(Rule::Cancelled { at: Some(at), no_pr: false });
    }
    if let Some(at) = completed.filter(|_| prs.is_empty()) {
        return (!idle).then_some(Rule::Completed { at: Some(at) });
    }
    match verdict(prs) {
        Verdict::Merged { number, head } => Some(Rule::Merged { number, head }),
        Verdict::Keep => None,
    }
}

// One run of reap: what its steps share, and what it has done so far (`reaped`), to which
// `remove` adds each worktree it tries to remove.
struct Reaper<'r, 'f> {
    orca: &'r mut (dyn Orca + 'f),
    forget: &'r mut Forget<'f>,
    tolksters: &'r mut dyn FnMut(&[IssueKey]) -> Result<Tolksters, Error>,
    // Orca's whole listing, for the repo's primary checkout.
    all: &'r [Worktree],
    dry_run: bool,
    // Idle since at or before this, an agent in a merged PR's worktree may be closed.
    idle_by: String,
    reaped: Reaped,
}

impl Reaper<'_, '_> {
    // Adds what `result` came to for `issue`'s worktree to what was done.
    fn record(&mut self, issue: &str, result: Result<Option<Reaping>, Error>) {
        let what = match result {
            Ok(Some(what)) => what,
            Ok(None) => return,
            Err(error) => {
                self.reaped.failed = true;
                self.reaped.spent = self.reaped.spent.max(error.rate_limited());
                self.reaped.later |= error.later();
                if let (None, Error::Waits(waits, _)) = (&self.reaped.waits, &error) {
                    self.reaped.waits = Some(waits.clone());
                }
                Reaping::Failed(error)
            }
        };
        self.reaped.lines.push(Reap { issue: issue.into(), what });
    }

    // Reaps `worktree` on `rule`. `idle` is the terminal of an agent that has sat idle there
    // since `idle_by` or before, to close once the merged rule's checks have passed, and only
    // if Orca, read again just before, still has it idle in that same terminal: the owner may
    // have reached it since the listing, and a line typed into it must not be cut off.
    fn apply(&mut self, worktree: &Worktree, branch: &BranchName, rule: Rule, idle: Option<&Terminal>) -> Result<Option<Reaping>, Error> {
        if let Some(host) = worktree.host() {
            return self.on_host(worktree, host, branch, rule);
        }
        match rule {
            Rule::Cancelled { at, no_pr } => self.reap_cancelled(worktree, branch, at, no_pr),
            Rule::Completed { at } => self.reap_completed(worktree, branch, at),
            Rule::Merged { number, head } => self.reap_merged(worktree, branch, number, &head, idle),
            Rule::Landed => self.reap_landed(worktree, branch),
        }
    }

    // Sends `rule` to `host`, the worktree's own, to reap it there (`Step`), and reads Orca's
    // listing after: only a worktree gone from a complete one was removed, and has its record
    // forgotten here, as the step reads no record.
    fn on_host(&mut self, worktree: &Worktree, host: &str, branch: &BranchName, rule: Rule) -> Result<Option<Reaping>, Error> {
        let (path, host) = (worktree.path.clone(), host.to_string());
        let Some(issue) = worktree.linked_issue.as_deref().and_then(IssueKey::parse) else { return Ok(None) };
        let Some(primary) = self.all.iter().find(|w| w.is_main && w.host() == Some(host.as_str()) && w.repo_id == worktree.repo_id) else {
            return Ok(Some(Reaping::NoHostPrimary { path, host }));
        };
        let mut why = Why::of(match &rule {
            Rule::Merged { number, .. } => Reason::Merged(*number),
            Rule::Landed => Reason::Landed,
            Rule::Cancelled { .. } => Reason::Cancelled,
            Rule::Completed { .. } => Reason::Completed,
        });
        let command = Step { issue, branch: branch.clone(), rule }.command()?;
        if self.dry_run {
            return Ok(Some(Reaping::WouldAskHost { path, branch: branch.to_string(), host, why }));
        }
        // An error here may follow a step that went on to remove it: `reap_once` forgets the
        // record of a worktree it finds gone.
        let code = self.orca.run_to_exit(&primary.id, &command)?;
        // A host that can't be read now decides nothing: the reap is sent again.
        let gone = match self.orca.listing_at(&worktree.id) {
            Ok(listing) => listing.lacks(&worktree.id),
            // It waits on the host only when the host is what didn't answer.
            Err(error) if code != Some(KEPT) => {
                let message = format!("its host's listing after its reap of {path} couldn't be read: {error}");
                return Err(match error {
                    Error::Waits(waits, _) => Error::Waits(waits, message),
                    _ => Error::Later(message),
                });
            }
            Err(_) => false,
        };
        if !gone {
            return match code {
                Some(KEPT) => Ok(Some(Reaping::HostKept { path, host, why })),
                code => Err(Error::Failed(format!("{host}'s reap of {path} ({why}) ended with {}, and the worktree is still listed", code.map_or("no exit code".into(), |code| format!("code {code}"))))),
            };
        }
        self.reaped.removed = true;
        self.reaped.tried += 1;
        why.record_stays = (self.forget)(&worktree.id).err();
        match code {
            Some(0) => Ok(Some(Reaping::RemovedOnHost { path, branch: branch.to_string(), host, why })),
            // Another run removed it first.
            Some(KEPT) => Ok(Some(Reaping::GoneFromHost { path, host, why })),
            _ => Err(Error::Failed(format!("removed {path} on {host} ({why}), but its reap there didn't end well, so branch {branch} may be left there"))),
        }
    }

    // Removes a merged PR's worktree once its checkout is at the PR's head and clean.
    fn reap_merged(&mut self, worktree: &Worktree, branch: &BranchName, number: u64, head: &str, idle: Option<&Terminal>) -> Result<Option<Reaping>, Error> {
        let path = worktree.path.as_str();
        let kept = |because| Ok(Some(Reaping::Kept { path: worktree.path.clone(), rule: Reason::Merged(number), because }));
        let local = git::head(self.orca.shell(), path)?;
        if local != head {
            return kept(Keep::AtOther { local: local.to_string() });
        }
        if let Some(first) = dirty(self.orca.shell(), path)? {
            return kept(Keep::Dirty { first });
        }
        if self.dry_run {
            return Ok(Some(Reaping::WouldRemove { path: worktree.path.clone(), branch: branch.to_string(), rule: Reason::Merged(number), closing: idle.is_some() }));
        }
        let mut why = Why::of(Reason::Merged(number));
        if let Some(handle) = idle {
            if !still_idle(self.orca, worktree, handle, &self.idle_by)? {
                return Ok(None);
            }
            self.orca.close(handle)?;
            why.closed_agent = true;
        }
        let tried_before = self.reaped.tried;
        let result = self.remove(worktree, branch, why, true);
        // Once the agent is closed, every line says so: `why` does, whatever came of a
        // removal, and this does for one never tried, because Orca still lists the terminal
        // just closed (then the machine's `reapWorktree` after its exit reaps the worktree) or
        // couldn't be asked.
        if idle.is_some() && self.reaped.tried == tried_before {
            return match result {
                Err(error) => Err(Error::Failed(format!("closed the idle agent in {} (PR #{number} merged), but {error}", worktree.path))),
                _ => Ok(Some(Reaping::ClosedWaits { path: worktree.path.clone(), pr: number })),
            };
        }
        result
    }

    // Removes a landed worker's worktree once, origin fetched, its checkout is clean, its HEAD
    // on origin's default branch, and nothing at HEAD or on its branch on no remote: the fetch
    // comes first, as the shared remote-tracking refs are any worker's to move. `reap` never
    // decides this rule, so no dry run reaches it.
    fn reap_landed(&mut self, worktree: &Worktree, branch: &BranchName) -> Result<Option<Reaping>, Error> {
        let path = worktree.path.as_str();
        let kept = |because| Ok(Some(Reaping::Kept { path: worktree.path.clone(), rule: Reason::Landed, because }));
        if let Some(first) = dirty(self.orca.shell(), path)? {
            return kept(Keep::Dirty { first });
        }
        git::fetch(self.orca.shell(), path)?;
        let local = git::head(self.orca.shell(), path)?;
        if !git::on_default(self.orca.shell(), path, &local)? {
            return kept(Keep::OffDefault { local: local.to_string() });
        }
        if let Some(because) = self.unpushed_keep(path, branch)? {
            return kept(because);
        }
        self.remove(worktree, branch, Why::of(Reason::Landed), false)
    }

    // Removes a cancelled issue's worktree, dirty or not, unless a PR for its branch is
    // open, or, where the machine's store holds no PR (`no_pr`), may be. The issue is read
    // again from Linear just before, so one reopened since the first read keeps its worktree.
    fn reap_cancelled(&mut self, worktree: &Worktree, branch: &BranchName, canceled_at: Option<String>, no_pr: bool) -> Result<Option<Reaping>, Error> {
        if self.dry_run {
            return Ok(Some(Reaping::WouldRemove { path: worktree.path.clone(), branch: branch.to_string(), rule: Reason::Cancelled, closing: false }));
        }
        // The machine's store holds no PR, and may have missed one opened, so the checkout
        // stays when it holds work that origin's copy, carrying a PR's worth of work,
        // doesn't. A PR it holds closed is no guard: that work was cancelled.
        if no_pr && let Some(because) = self.under_a_pr(worktree, branch)? {
            return Ok(Some(Reaping::Kept { path: worktree.path.clone(), rule: Reason::Cancelled, because }));
        }
        let Some(issue) = worktree.linked_issue.as_deref().and_then(IssueKey::parse) else { return Ok(None) };
        // The machine's rule names no cancel, and its command may have waited in the queue
        // through a reopen and a new cancel: that one must be a day old too, give or take
        // the clocks' skew.
        let old_enough = iso(now_ms().saturating_sub(CANCELLED_FOR_MS - SKEW_MS));
        let holds = match ((self.tolksters)(std::slice::from_ref(&issue))?.get(issue.as_str()), &canceled_at) {
            (Some(IssueNow::Cancelled(now)), Some(first)) => now == first,
            (Some(IssueNow::Cancelled(now)), None) => *now <= old_enough,
            _ => false,
        };
        if !holds {
            return Ok(None);
        }
        self.remove(worktree, branch, Why::of(Reason::Cancelled), false)
    }

    // What a PR on `branch` would lose with the checkout, when origin's copy of the branch
    // holds commits its default branch doesn't, as a PR needs: uncommitted or ignored
    // files, or commits on no remote. None when it would lose nothing, or no PR can be.
    fn under_a_pr(&mut self, worktree: &Worktree, branch: &BranchName) -> Result<Option<Keep>, Error> {
        let path = worktree.path.as_str();
        // A count that isn't a number may be a PR's: it keeps the worktree.
        if git::pushed_work(self.orca.shell(), path, branch)? == Some(0) {
            return Ok(None);
        }
        if let Some(first) = dirty(self.orca.shell(), path)? {
            return Ok(Some(Keep::Dirty { first }));
        }
        // With no local branch git can't count, and HEAD's own commits may be the PR's.
        if !git::branch_exists(self.orca.shell(), path, branch)? {
            return Ok(Some(Keep::BranchGone { branch: branch.to_string() }));
        }
        self.unpushed_keep(path, branch)
    }

    // What commits on no remote keep, none when git counts none.
    fn unpushed_keep(&mut self, path: &str, branch: &BranchName) -> Result<Option<Keep>, Error> {
        Ok(match git::unpushed(self.orca.shell(), path, branch)? {
            Some(0) => None,
            Some(count) => Some(Keep::Unpushed(count)),
            None => Some(Keep::Uncounted),
        })
    }

    // Removes a completed issue's worktree, whose branch has no PR, once its checkout is
    // clean and every commit at its HEAD and on its branch is on a remote: the checkout
    // may have moved off the branch since Orca listed it, and `remove` deletes the branch.
    // The issue is read again from Linear just before, so one reopened since the first read
    // keeps its worktree.
    fn reap_completed(&mut self, worktree: &Worktree, branch: &BranchName, completed_at: Option<String>) -> Result<Option<Reaping>, Error> {
        let path = worktree.path.as_str();
        let kept = |because| Ok(Some(Reaping::Kept { path: worktree.path.clone(), rule: Reason::Completed, because }));
        if let Some(first) = dirty(self.orca.shell(), path)? {
            return kept(Keep::Dirty { first });
        }
        // Anything but a count of none keeps the worktree, and a missing branch fails.
        if let Some(because) = self.unpushed_keep(path, branch)? {
            return kept(because);
        }
        if self.dry_run {
            return Ok(Some(Reaping::WouldRemove { path: worktree.path.clone(), branch: branch.to_string(), rule: Reason::Completed, closing: false }));
        }
        let Some(issue) = worktree.linked_issue.as_deref().and_then(IssueKey::parse) else { return Ok(None) };
        let holds = match ((self.tolksters)(std::slice::from_ref(&issue))?.get(issue.as_str()), &completed_at) {
            (Some(IssueNow::Completed(now)), Some(first)) => now == first,
            (Some(IssueNow::Completed(_)), None) => true,
            _ => false,
        };
        if !holds {
            return Ok(None);
        }
        self.remove(worktree, branch, Why::of(Reason::Completed), false)
    }

    // Removes the worktree, forgets its worker record, then deletes its local branch from
    // the primary checkout, unless `orca_deletes_branch`. `why` ends each line.
    // A record that can't be forgotten is only noted: its worktree is gone, so it matches
    // nothing. The removal is recorded before Orca is asked, as even a failed one may have
    // changed what it lists. None, with nothing tried or recorded, when an agent is running
    // there now: what the caller decided from may be older than an agent resumed since.
    // Orca deletes the branch of a merged PR's worktree itself, as it does of any worktree it
    // can prove merged; it keeps the others', which this deletes.
    fn remove(&mut self, worktree: &Worktree, branch: &BranchName, mut why: Why, orca_deletes_branch: bool) -> Result<Option<Reaping>, Error> {
        if agent_in(&self.orca.listing_at(&worktree.id)?.terminals, &worktree.id) {
            return Ok(None);
        }
        self.reaped.removed = true;
        self.reaped.tried += 1;
        self.orca.remove_worktree(&worktree.id).map_err(|error| Error::Failed(format!("couldn't remove {} ({why}): {error}", worktree.path)))?;
        why.record_stays = (self.forget)(&worktree.id).err();
        let primary = primary_checkout(self.all, &worktree.repo_id).map(|w| w.path.as_str());
        let removed = |branch_kept, why| Reaping::Removed { path: worktree.path.clone(), branch: branch.to_string(), why, branch_kept };
        let Some(primary) = primary.filter(|_| !orca_deletes_branch) else {
            return Ok(Some(removed(primary.is_none(), why)));
        };
        // The worktree is gone by now, so an error from here on still says so, and `why`.
        let failed = |what: &str, detail: String| Error::Failed(format!("removed {} ({why}), but couldn't {what} branch {branch}: {detail}", worktree.path));
        self.await_unlisted(primary, &worktree.path);
        if git::branch_exists(self.orca.shell(), primary, branch).map_err(|error| failed("check", error.to_string()))?
            && let Err(error) = git::force_delete_branch(self.orca.shell(), primary, branch)
        {
            // Orca may have deleted it since the check.
            if git::branch_exists(self.orca.shell(), primary, branch).unwrap_or(true) {
                return Err(failed("delete", error.to_string()));
            }
        }
        Ok(Some(removed(false, why)))
    }

    // Waits, a few seconds at most, until git's list in the primary checkout no longer names
    // `path`: Orca's `worktree rm` returns before git has let go of it, and git refuses to
    // delete a branch a listed worktree holds. A list git can't give ends the wait, and the
    // delete reports what git says.
    fn await_unlisted(&mut self, primary: &str, path: &str) {
        let same = |listed: &String| listed.replace('\\', "/").trim_end_matches('/').eq_ignore_ascii_case(path.replace('\\', "/").trim_end_matches('/'));
        for _ in 0..UNLISTED_POLLS {
            if git::worktree_paths(self.orca.shell(), primary).map_or(true, |listed| !listed.iter().any(same)) {
                return;
            }
            std::thread::sleep(UNLISTED_POLL);
        }
    }
}


// Whether the agent in `terminal` still sits idle in the worktree, since `idle_by` or
// before, as its host lists the agents and the terminals now. Read just before closing it.
fn still_idle(orca: &mut dyn Orca, worktree: &Worktree, terminal: &Terminal, idle_by: &str) -> Result<bool, Error> {
    let listing = orca.listing_at(&worktree.id)?;
    let Some(agents) = listing.worktree(&worktree.id).map(|w| w.agents.clone()) else { return Ok(false) };
    let terminals = listing.terminals;
    if agent_terminal(&terminals, &worktree.id).as_ref() != Some(terminal) {
        return Ok(false);
    }
    let fresh = Worktree { agents, ..worktree.clone() };
    Ok(idle_in(&fresh, &terminals).is_some_and(|since| since.as_str() <= idle_by))
}

// Each line whole, as an error carries them.
pub(super) fn joined(lines: &[Reap]) -> String {
    lines.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")
}

mod step;

pub use step::{KEPT, STEP_OPTIONS, Step, reap_here};

#[cfg(test)]
mod tests;
