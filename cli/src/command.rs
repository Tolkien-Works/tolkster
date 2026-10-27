// Carries out one command from the Durable Object's machines (machine.rs), and answers it
// with its result (scheduler::answer). Each is safe to run again by what it finds when it
// runs (docs/state-machine.md, Commands): a terminal already closed counts as closed, a
// post carries an activity ID made from the command's ID, and an agent is listed again
// just before anything is typed into it. A start, a resume, a review round and a session's
// turn are executor.rs's; a reap is reap.rs's, a note's thread turn converse.rs's, a
// completed issue's context context.rs's and a triage batch triage.rs's.
//
// Every session, comment, worktree and PR a command names is checked to be its own issue's
// before anything is done to it: one on another issue is someone else's worker or turn, and
// refused (`on_issue`). The triage machine's commands name no issue: what a batch may take
// is triage.rs's to decide, from each issue as read here.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use serde_json::{Value, json};
use ureq::Agent;

use crate::cli::write_file;
use crate::config::Config;
use crate::error::{Error, Waits};
use crate::context;
use crate::exec::{beating, run_command};
use crate::executor;
use crate::github::AppReader;
use crate::gateway::Failure;
use crate::machine::{Command, Finished, Notice, Order, Outcome, Queued, ReapRule, Waiting};
use crate::time::now_ms;
use crate::panel;
use crate::orca::{self, host_of, open_terminals};
use crate::live::{LinearApp, Recipes};
use crate::ports::{Candidate, Ports};
use crate::request::Thread;
use crate::reap;
use crate::runtime::{Runtime, Shared, load};
use crate::scheduler::{Acted, answer, turn_ended, watch_terminal};
use crate::sessions::HISTORIES_PER_READ;
use crate::state::new_token;
use crate::converse::{self, InThread};
use crate::plain::{Id, IssueKey, Time};
use crate::hosts::{self, Pacing};
use crate::trace::{self, Triage, Work};
use crate::triage::{self, Opened};

use notice::{notice_text, say};

pub fn carry_out(agent: &Agent, shared: &Arc<Shared>, command: &Command, beat: &dyn Fn()) -> Acted {
    let mut error_kind = None;
    let finished = match &command.order {
        Err(reason) => {
            let error = Error::Failed(reason.clone());
            error_kind = Some(trace::error_kind(&error));
            trace::event(Work::Failed(error));
            Finished::failed()
        }
        // A panic still answers, so the command isn't left in flight for good.
        Ok(order) => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let seen = shared.recipes.ended();
            load(agent, shared).and_then(|rt| run(&rt, command, order, beat, seen))
        })).unwrap_or_else(|_| Err(Error::Failed("panicked".into()))) {
            Ok(finished) => finished,
            // The error's text stays here: the result carries none.
            Err(error) => {
                let finished = Finished::of(&error);
                error_kind = Some(trace::error_kind(&error));
                trace::event(if finished.outcome == Outcome::Retryable { Work::Transient(error) } else { Work::Failed(error) });
                finished
            }
        },
    };
    let unconfirmed = finished.detail.get("typed") == Some(&json!(true));
    if unconfirmed {
        error_kind = Some(trace::UNCONFIRMED);
    }
    panel::finished(panel::Finish { at: Instant::now(), issue: command.identifier.to_string(), command: command.kind.name(), outcome: finished.outcome, error: error_kind });
    trace::event(match (unconfirmed, finished.awaits) {
        (true, _) => trace::Outcome::Unconfirmed,
        (false, true) => trace::Outcome::Awaits,
        (false, false) => trace::Outcome::Answered(finished.outcome),
    });
    answer(command, &finished);
    Acted::Done
}

fn run(rt: &Runtime, command: &Command, order: &Order, beat: &dyn Fn(), seen: usize) -> Result<Finished, Error> {
    let mut shell = beating(beat);
    let linear = LinearApp { agent: rt.agent, state: &rt.state, owner_id: &rt.config.owner.id, started: &rt.shared.started };
    let mut ports = Ports { linear: &linear, records: &rt.state, orca: &mut orca::Cli::paired(&mut shell, &rt.shared.health).at_once(run_command).waiting(), github: &mut AppReader::new(rt.agent, beat), machines: &Recipes { agent: rt.agent, shared: rt.shared, seen, beat }, shared: rt.shared, watch: &watch_terminal };
    carry(&rt.config, &mut ports, command, order, beat)
}

// The command carried out through `ports` (ports.rs), with the settings in `config`. One for a
// worker marks its machine acted on from before it acts until it ends, a start's the host its
// slot was granted on too, which a sleep's check orders against (`Pacing::acting`), so no
// worker is placed on a machine a sleep found resting; one whose machine sleeps, or is going to
// sleep, waits for it instead (`waits`) (docs/hosts.md, Sleep and wake).
pub fn carry(config: &Config, ports: &mut Ports, command: &Command, order: &Order, beat: &dyn Fn()) -> Result<Finished, Error> {
    say_queued(ports, command);
    let named = match order {
        Order::StartWorker { worktree_id, .. } | Order::StopWorker { worktree_id, .. } => worktree_id.as_deref(),
        Order::DeliverMessage { worktree_id, .. } | Order::CloseAgent { worktree_id } | Order::ReapWorktree { worktree_id, .. } => Some(worktree_id.as_str()),
        Order::ResumeWorker { .. } | Order::StartRound { .. } => None,
        _ => return carried(config, ports, command, order, beat),
    };
    let mut on = worker_hosts(ports, &command.identifier, named)?;
    if let Order::StartWorker { host: Some(host), .. } = order
        && !on.contains(host)
    {
        on.push(host.clone());
    }
    let _on = {
        let _acting = ports.shared.pacing.acting();
        if let Some(waits) = waits(config, &ports.shared.pacing, &on) {
            return Ok(Finished::waking(&waits));
        }
        ports.shared.pacing.act_on(&on)
    };
    carried(config, ports, command, order, beat)
}

// The hosts the command acts on: the worktree's it names, else those of the issue's workers'
// records. None on this machine.
fn worker_hosts(ports: &Ports, identifier: &str, named: Option<&str>) -> Result<Vec<String>, Error> {
    let records = match named {
        Some(_) => Vec::new(),
        None => ports.records.workers().map_err(|error| Error::Later(format!("the worker records couldn't be read: {error}")))?,
    };
    let ids = records.iter().filter(|r| r.issue.eq_ignore_ascii_case(identifier)).map(|r| r.worktree_id.as_str()).chain(named);
    let mut on: Vec<String> = ids.filter_map(|id| host_of(id)).map(String::from).collect();
    on.sort();
    on.dedup();
    Ok(on)
}

// What the command waits on when one of `on` is a recipe machine asleep, which the fleet wakes
// on hearing so, and the command waits for it to answer; or one a sleep is putting to sleep,
// which the send after the sleep ends finds asleep.
fn waits(config: &Config, pacing: &Pacing, on: &[String]) -> Option<Waits> {
    if let Some(host) = on.iter().find(|host| hosts::asleep_as(config, host).is_some()) {
        return Some(Waits::Host(host.clone()));
    }
    on.iter().any(|host| pacing.napping(host)).then_some(Waits::Lock)
}


fn carried(config: &Config, ports: &mut Ports, command: &Command, order: &Order, beat: &dyn Fn()) -> Result<Finished, Error> {
    match order {
        Order::ReadIssue => read::read_issue(config, ports, command),
        Order::StartWorker { session_id, orchestrate, worktree_id, host } => executor::start(config, ports, command, session_id, *orchestrate, worktree_id.as_deref(), host.as_deref()),
        Order::ResumeWorker { session_id, why } => executor::resume(config, ports, command, session_id, *why),
        Order::StartTurn { session_id, thread: false, request, after } => executor::turn(config, ports, command, session_id, request.as_ref(), after.as_ref()),
        Order::StartTurn { session_id, thread: true, request, after } => thread_turn(config, ports, command, session_id, request.as_ref(), after.as_ref(), beat),
        Order::DeliverMessage { worktree_id, session_id, comment } => workers::deliver_message(ports, command, worktree_id, session_id.as_ref(), *comment),
        Order::ReadComment { comment_id } => read::read_comment(ports, command, comment_id),
        Order::StartRound { repo, number, head, round, session_id, rebase_only } => executor::round(config, ports, command, session_id.as_ref(), &executor::OnPr { repo, number: *number, head: head.as_deref(), round: *round, rebase_only: *rebase_only }),
        Order::UpdateBranch { repo, number, head } => pr::update_branch(ports, command, repo, *number, head),
        Order::MergePr { repo, number, head } => pr::merge_pr(ports, command, repo, *number, head),
        Order::StopWorker { worktree_id, session_id } => workers::stop_worker(ports, command, worktree_id.as_deref(), session_id.as_ref()),
        Order::StopTurn { session_id, thread, idle } => workers::stop_turn(ports, command, session_id, *thread, *idle),
        Order::EvaluatePr { repo, number } => pr::evaluate_pr(config, ports, command, repo, *number),
        Order::Observe { worktree_ids, session_id, thread } => workers::observe(ports, command, worktree_ids, session_id.as_ref(), *thread),
        Order::CloseAgent { worktree_id } => workers::close_idle_agent(ports, command, worktree_id),
        Order::ReapWorktree { worktree_id, rule } => {
            match rule {
                ReapRule::Merged { number, .. } => ports.shared.timing.merged(&command.issue_id, &command.identifier, Some(*number), now_ms(), false),
                ReapRule::Landed => ports.shared.timing.merged(&command.issue_id, &command.identifier, None, now_ms(), false),
                _ => {}
            }
            Ok(Finished::ok(json!({ "reaped": reap::reap_for(ports, worktree_id, &command.identifier, rule)? })))
        }
        Order::PostNotice { session_id, thread, notice, blockers, pr, rounds, through } => {
            // A work notice for a worker no session gave a request to is said nowhere.
            let Some(session_id) = session_id else { return Ok(Finished::ok(json!({}))) };
            if !thread {
                session_on_issue(ports, command, session_id)?;
            }
            let (kind, body) = notice_text(*notice, &ports.linear.identifiers(blockers)?, *pr, *rounds);
            let stalled = (*notice == Notice::Stalled).then_some(through.as_deref()).flatten();
            say(ports, command, session_id, *thread, stalled, kind, body)
        }
        Order::TakeBackChain { children } => {
            // Only those Tolkster's record says this parent's chain delegated. Each is tried, and the first failure
            // returned after. One already taken back, or delegated to someone else since, is
            // left as it is (`release_issue`).
            let delegations = ports.records.delegations(children)?;
            let ours = children.iter().filter(|child| delegations.get(child.as_str()).is_some_and(|d| d.parent_issue_id == command.issue_id));
            let failures: Vec<Error> = ours.filter_map(|child| ports.linear.release_issue(child).err()).collect();
            match failures.into_iter().next() {
                Some(error) => Err(error),
                None => Ok(Finished::ok(json!({}))),
            }
        }
        // Nothing asks for a completion's context again, so a transient failure is tried
        // again on the command's resends, and its claim, given back, lets one start it. Not
        // Orca's: a terminal it opened before failing would be opened twice. A wait keeps what it
        // waits on (`Finished::of`).
        Order::StartContext => match context::context_for(config, ports, &command.issue_id, beat) {
            Ok(context) => {
                trace::event(context);
                Ok(Finished::ok(json!({})))
            }
            Err(error) if matches!(Failure::of(&error), Failure::Transient) && !error.orcas() && !error.later() => {
                trace::event(Work::Transient(error));
                Ok(Finished::retryable())
            }
            Err(error) => Err(error),
        },
        Order::StartTriage { issue_ids } => start_triage(config, ports, issue_ids, beat),
        Order::MakeHost { repo } => executor::machines::make_host(config, ports, command, repo),
        Order::WakeHost { name } => executor::machines::wake_host(config, ports, command, name),
        Order::SuspendHost { host } => executor::machines::suspend_host(config, ports, host),
        Order::UpdateHost { name } => executor::machines::update_host(config, ports, name),
        Order::DestroyHost { name } => executor::machines::destroy_host(config, ports, name),
        // A terminal Orca no longer lists has closed, and its session with it. Triage runs on
        // this machine alone.
        Order::ObserveTriage { terminal, .. } => Ok(Finished::ok(json!({ "open": open_terminals(&ports.orca.listing_on(None)?.terminals, None).contains(terminal.as_str()) }))),
    }
}

// Says why a command's queued request waits when that has changed. Naming the blockers is a
// Linear read that may fail: they are then said unnamed, and said again, named, by the next
// command for the request. A command is never failed by it.
fn say_queued(ports: &Ports, command: &Command) {
    let Some(Queued { waiting, blockers, at }) = &command.queued else { return };
    let seen = format!("{at} {waiting:?} {}", blockers.iter().map(Id::as_str).collect::<Vec<_>>().join(" "));
    panel::queued(command.identifier.as_str(), *waiting, at.as_str(), None);
    if !ports.shared.queued.changed(&command.identifier, seen) {
        return;
    }
    let blocked_by = if *waiting == Waiting::Blockers {
        ports.linear.identifiers(blockers).unwrap_or_else(|_| {
            ports.shared.queued.forget(command.identifier.as_str());
            Vec::new()
        })
    } else {
        Vec::new()
    };
    panel::queued(command.identifier.as_str(), *waiting, at.as_str(), Some(blocked_by.clone()));
    trace::event(Work::Queued { waiting: *waiting, blocked_by });
}

// A triage batch (docs/triage.md): each issue asked, read fresh, ahead of one page of the
// Triage queue, oldest first, each kept only if the owner or Tolkster created it
// (triage::may_triage); triage::start takes the batch and opens its session, whose terminal the
// watcher waits on (`watch_terminal`). An asked issue that can't be read is left to the queue,
// unless Linear's budget is spent, which fails the start for the machine to send again.
fn start_triage(config: &Config, ports: &mut Ports, issue_ids: &[Id], beat: &dyn Fn()) -> Result<Finished, Error> {
    let queue = ports.linear.triage_queue()?;
    let viewer_id = &queue.viewer_id;
    let mut candidates: Vec<Candidate> = Vec::new();
    // Each asked issue a candidate came from, by its identifier, to say which still wait.
    let mut asked: Vec<(IssueKey, &Id)> = Vec::new();
    if !issue_ids.is_empty() {
        beat();
        let read = match ports.linear.issues_closed(issue_ids) {
            Ok(read) => read,
            Err(error) if error.rate_limited().is_some() => return Err(error),
            Err(error) => {
                trace::event(Triage::Unread(error));
                Vec::new()
            }
        };
        for issue_id in issue_ids {
            let Some((_, candidate)) = read.iter().find(|(id, _)| id == issue_id) else {
                trace::event(Triage::UnreadOne);
                continue;
            };
            match triage::may_triage(candidate, &config.owner.id, viewer_id) {
                Ok(()) if !candidates.iter().any(|c| c.identifier == candidate.identifier) => {
                    asked.push((candidate.identifier.clone(), issue_id));
                    candidates.push(candidate.clone());
                }
                Ok(()) => {}
                Err(why) => trace::event(Triage::Refused { issue: candidate.identifier.clone(), why }),
            }
        }
    }
    for candidate in triage::queue(queue.candidates, &config.owner.id, viewer_id) {
        if !candidates.iter().any(|c| c.identifier == candidate.identifier) {
            candidates.push(candidate);
        }
    }
    let batch = usize::try_from(config.triage.batch).unwrap_or(usize::MAX);
    let start = triage::Start { candidates: &candidates, batch, owner_name: &config.owner.name, briefs_dir: &config.briefs_dir, repos: &config.repos };
    // Whether a candidate was filed in Triage, by its history, read as the batch reaches it:
    // one read takes the next few candidates' too, as many as the batch could still take.
    let (linear, window) = (ports.linear, batch.clamp(1, HISTORIES_PER_READ));
    let mut known: HashMap<IssueKey, bool> = HashMap::new();
    let mut filed = |candidate: &Candidate| {
        if let Some(filed) = known.get(&candidate.identifier) {
            return Ok(*filed);
        }
        beat();
        let next: Vec<IssueKey> = candidates.iter().skip_while(|c| c.identifier != candidate.identifier).filter(|c| !known.contains_key(&c.identifier)).take(window).map(|c| c.identifier.clone()).collect();
        // One candidate Linear can't read fails the whole read, which must not take its
        // neighbours' histories with it: the candidate is read alone, and a spent budget ends all.
        let histories = match linear.state_histories(&next) {
            Err(error) if next.len() > 1 && error.rate_limited().is_none() => linear.state_histories(&next[..1])?,
            read => read?,
        };
        let next = &next[..histories.len().min(next.len())];
        known.extend(next.iter().cloned().zip(histories));
        known.get(&candidate.identifier).copied().ok_or_else(|| Error::Failed(format!("Linear had no history for {}", candidate.identifier)))
    };
    let started = triage::start(&start, &mut filed, ports.orca, &mut write_file, ports.records, &new_token())?;
    trace::event(Triage::Started(started.notes));
    let waiting: Vec<&Id> = asked.iter().filter(|(identifier, _)| started.waiting.contains(identifier)).map(|(_, id)| *id).collect();
    Ok(match started.opened {
        Some(opened) => {
            (ports.watch)(&opened.terminal);
            Finished::ok(json!({ "session": session_json(&opened), "waiting": waiting }))
        }
        None => Finished::ok(json!({ "waiting": waiting })),
    })
}

fn session_json(opened: &Opened) -> Value {
    json!({ "id": opened.session, "terminal": opened.terminal, "issues": opened.issues })
}

// The session `session_id`, read from Linear, is on the command's issue.
fn session_on_issue(ports: &Ports, command: &Command, session_id: &Id) -> Result<(), Error> {
    on_issue(command, ports.linear.session_issue(session_id)?.as_deref())
}

// A turn in a note's thread, whose root is `root` (converse.rs, `thread_turn`), on the replies
// after `after` up to the request decided, `request`, which the machine always names.
// - A turn opened, or the one the thread's record names still running: `ok`, with its
//   terminal.
// - No reply to answer, or no repo (said in the thread as the request's answer): `ok`, with
//   the turn ended at once.
fn thread_turn(config: &Config, ports: &mut Ports, command: &Command, root: &Id, request: Option<&Time>, after: Option<&Time>, beat: &dyn Fn()) -> Result<Finished, Error> {
    let Some(request) = request else { return Ok(Finished::failed()) };
    let done = converse::thread_turn(config, ports, root, &command.issue_id, request, after, beat)?;
    trace::event(Work::InThread(done.turn()));
    Ok(match done {
        InThread::Opened(terminal, _) => Finished::ok(json!({ "terminal": terminal })),
        InThread::Answered | InThread::NoRepo(_) => {
            turn_ended(root);
            Finished::ok(json!({}))
        }
        InThread::Unanswered(_) => Finished::failed(),
    })
}

// The thread of Tolkster's comment `root`, read again: None when it is no note's thread on
// the command's issue, which the command then leaves alone.
fn thread_on_issue(ports: &Ports, command: &Command, root: &Id) -> Result<Option<Thread>, Error> {
    Ok(ports.linear.thread(root)?.ok().filter(|thread| thread.issue_id == command.issue_id))
}

// A session, comment or worktree a command names must be on its own issue: one on another is a
// worker or a turn of someone else's, and refused (docs/state-machine.md, Commands).
fn on_issue(command: &Command, issue_id: Option<&str>) -> Result<(), Error> {
    match issue_id == Some(command.issue_id.as_str()) {
        true => Ok(()),
        false => Err(Error::Failed(format!("{}: the command names something of another issue", command.identifier))),
    }
}

mod notice;
pub mod pr;
pub mod read;
mod workers;

#[cfg(test)]
mod tests;
