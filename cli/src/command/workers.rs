// The worker orders: a message delivered to a worker's agent, a stop, a close, a turn's stop and
// the observation of worktrees. Each reads only its worktree's host, just before it types,
// closes or removes (docs/dispatcher.md), and acts only on its own issue's worktree.

use serde_json::json;

use crate::error::{Error, Unsent};
use crate::executor;
use crate::linear::ActivityType;
use crate::machine::{Command, Finished, Observed};
use crate::orca::{self, Listing, Orca, Proof, Typed, Worktree, agent_terminal, agent_worktrees, host_of, open_terminals, waits_in};
use crate::plain::{Id, Terminal};
use crate::ports::Ports;
use crate::scheduler::{observed, turn_ended};
use crate::trace::{self, Purpose, Work};
use crate::voice;
use crate::watch::idle_in;

use super::notice::post;
use super::{on_issue, session_on_issue, thread_on_issue};

pub(super) fn deliver_message(ports: &mut Ports, command: &Command, worktree_id: &str, session_id: Option<&Id>, comment: bool) -> Result<Finished, Error> {
    if let Some(session_id) = session_id {
        session_on_issue(ports, command, session_id)?;
    }
    let Some((worktree, Listing { terminals, .. })) = worktree_on_issue(ports.orca, command, worktree_id)? else {
        trace::event(Work::NoWorktree { issue: command.identifier.clone(), purpose: Purpose::Message });
        return Ok(Finished::failed());
    };
    let Some(handle) = agent_terminal(&terminals, worktree_id) else {
        trace::event(Work::NoAgent { issue: command.identifier.clone() });
        return Ok(Finished::gone());
    };
    if waits_in(&worktree, &terminals) {
        return Ok(Finished::held());
    }
    let line = match (session_id, comment) {
        (Some(session_id), false) => voice::new_message(session_id),
        _ => voice::new_comment(&command.identifier),
    }?;
    // Delivered once Orca took the input, which an agent mid-turn reads when its turn ends; a
    // line Orca didn't show taken is never typed again.
    let sent = match ports.orca.send(worktree_id, &handle, &line, Proof::Input) {
        // The agent ended between the listing and the send, with proof nothing was typed.
        Err(Error::Unsent(Unsent::NoAgent, _)) => {
            trace::event(Work::NoAgent { issue: command.identifier.clone() });
            return Ok(Finished::gone());
        }
        sent => sent?,
    };
    Ok(match sent {
        Typed::Delivered => Finished::ok(json!({})),
        Typed::Waiting => Finished::held(),
        Typed::Unproven => Finished::failed_with(json!({ "typed": true })),
    })
}

pub(super) fn stop_worker(ports: &mut Ports, command: &Command, worktree_id: Option<&str>, session_id: Option<&Id>) -> Result<Finished, Error> {
    if let Some(session_id) = session_id {
        session_on_issue(ports, command, session_id)?;
    }
    // With none named, a start the stop overtook may have made one the machine hasn't
    // heard of yet: the worktree Orca links to the issue is the one it made.
    let worktree = match worktree_id {
        Some(worktree_id) => worktree_on_issue(ports.orca, command, worktree_id)?.map(|(worktree, listing)| (worktree.id, listing.terminals)),
        None => match executor::linked_worktree(ports, &command.identifier)? {
            Some(worktree) => Some((worktree.id.clone(), ports.orca.listing_at(&worktree.id)?.terminals)),
            None => None,
        },
    };
    if let Some((worktree_id, terminals)) = worktree {
        // The machine tells the owner once, and sends the stop again.
        if !close_agent(ports.orca, &worktree_id, &terminals)? {
            return Ok(Finished::failed_with(json!({ "unnamed": true })));
        }
    }
    match session_id {
        Some(session_id) => post(ports, command, session_id, ActivityType::Response, voice::STOPPED.into()),
        None => Ok(Finished::ok(json!({}))),
    }
}

pub(super) fn stop_turn(ports: &mut Ports, command: &Command, session_id: &Id, thread: bool, idle: bool) -> Result<Finished, Error> {
    // Under the checkouts' lock: a turn opening meanwhile either opened first, or opens
    // after, and its start's result has the machine stop it again. The close says
    // nothing: the machine says "Stopped." once the stop holds, once per request.
    let _checkouts = ports.shared.checkouts.lock();
    match turn_terminal(ports, command, session_id, thread)? {
        Some(terminal) => close(ports.orca, &terminal)?,
        // A warm turn already gone is reported ended, as its end may have been missed.
        // Only its close does: the report names no terminal, and the machine holds the
        // turn a warm close is for, where a stop's turn is let go, and a newer one may open.
        None if idle => turn_ended(session_id),
        None => {}
    }
    Ok(Finished::ok(json!({})))
}

pub(super) fn observe(ports: &mut Ports, command: &Command, worktree_ids: &[String], session_id: Option<&Id>, thread: bool) -> Result<Finished, Error> {
    // The turn's session is read, and checked to be the issue's, before anything is reported.
    let turn_open = match session_id {
        Some(session_id) => Some(turn_terminal(ports, command, session_id, thread)?.is_some()),
        None => None,
    };
    observe_worktrees(ports.orca, command, worktree_ids)?;
    if let (Some(session_id), Some(false)) = (session_id, turn_open) {
        turn_ended(session_id);
    }
    Ok(Finished::ok(json!({})))
}

pub(super) fn close_idle_agent(ports: &mut Ports, command: &Command, worktree_id: &str) -> Result<Finished, Error> {
    // Only an agent Orca shows idle (`done`) or held at a prompt is closed: one whose first turn
    // began after the machine gave up on it is its worker after all, and the reap keeps its worktree.
    if let Some((_, listing)) = worktree_on_issue(ports.orca, command, worktree_id)?.filter(|(w, _)| w.agents.iter().all(|p| matches!(p.state.as_str(), "done" | "waiting" | "blocked")))
        && !close_agent(ports.orca, worktree_id, &listing.terminals)?
    {
        return Err(unnamed_agent(command, worktree_id));
    }
    Ok(Finished::ok(json!({})))
}

pub(super) fn links(worktree: &Worktree, identifier: &str) -> bool {
    worktree.linked_issue.as_deref().is_some_and(|i| i.eq_ignore_ascii_case(identifier))
}

// The worktree `worktree_id` as its host lists it now, with that listing: None when it is gone,
// and Err when it isn't the command's issue's, which is never acted on, or its host wasn't heard.
pub(super) fn worktree_on_issue(orca: &mut dyn Orca, command: &Command, worktree_id: &str) -> Result<Option<(Worktree, Listing)>, Error> {
    let listing = orca.listing_at(worktree_id)?;
    match listing.worktree(worktree_id).cloned() {
        None => Ok(None),
        Some(worktree) if links(&worktree, &command.identifier) => Ok(Some((worktree, listing))),
        Some(_) => Err(Error::Failed(format!("{}: the command names another issue's worktree", command.identifier))),
    }
}

// The terminal of the session's conversation turn, if it is open, by its conversation's
// record. A note's thread's is recorded under its root, so a turn running across a restart of
// `listen` is still found. A root that is no note's thread on the command's issue is refused,
// as a session on another issue is.
pub(super) fn turn_terminal(ports: &mut Ports, command: &Command, session_id: &Id, thread: bool) -> Result<Option<Terminal>, Error> {
    let conversation = if thread {
        if thread_on_issue(ports, command, session_id)?.is_none() {
            return Err(Error::Failed(format!("{}: {session_id} is no note's thread on it", command.identifier)));
        }
        ports.records.conversation(session_id)?
    } else {
        let session = ports.linear.session_with_record(session_id)?;
        on_issue(command, session.issue.as_ref().map(|i| i.id.as_str()))?;
        session.record.conversation
    };
    // A turn whose terminal Orca no longer lists has ended. Turns run on this machine alone.
    let Some(terminal) = conversation.and_then(|c| c.terminal).as_deref().and_then(Terminal::parse) else { return Ok(None) };
    Ok(open_terminals(&ports.orca.listing_on(None)?.terminals, None).contains(&terminal).then_some(terminal))
}

// The worktree's terminals closed, each by its plain handle: its agents, and a worker's setup
// (`tolkster worker`), which Orca lists as no agent until the worker's agent starts. An agent Orca lists
// with no handle can't be named without naming the active terminal instead, so while one runs
// this is false, never taken as closed. `terminals` are its host's, read just before.
pub(super) fn close_agent(orca: &mut dyn Orca, worktree_id: &str, terminals: &[orca::Terminal]) -> Result<bool, Error> {
    let mut named = true;
    for listed in terminals.iter().filter(|t| t.worktree_id.as_deref() == Some(worktree_id)) {
        match (listed.handle.as_deref().and_then(Terminal::parse), listed.agent) {
            (Some(handle), _) => close(orca, &handle)?,
            (None, true) => named = false,
            (None, false) => {}
        }
    }
    Ok(named)
}

fn unnamed_agent(command: &Command, worktree_id: &str) -> Error {
    Error::Failed(format!("{}: an agent in {worktree_id} has no handle to close it by", command.identifier))
}

// A terminal that closed before this could close it counts as closed.
pub(super) fn close(orca: &mut dyn Orca, handle: &Terminal) -> Result<(), Error> {
    match orca.close(handle) {
        Err(error) if open_terminals(&orca.listing_of(handle)?.terminals, None).contains(handle) => Err(error),
        _ => Ok(()),
    }
}

// Each of `worktree_ids` as Orca lists it now, as an observation. One missing from the
// listing is left unsaid: only the Durable Object says a worktree is gone, after an hour of
// complete listings without it (src/worker/listing.ts, `reportGone`).
// One linked to another issue is left unsaid too, as it isn't this issue's worker.
// Each is read on its own host alone, and one on another host that can't be read is left
// unsaid too.
fn observe_worktrees(orca: &mut dyn Orca, command: &Command, worktree_ids: &[String]) -> Result<(), Error> {
    let mut hosts: Vec<Option<&str>> = worktree_ids.iter().map(|id| host_of(id)).collect();
    hosts.sort_unstable();
    hosts.dedup();
    for host in hosts {
        let listing = match orca.listing_on(host) {
            Err(_) if host.is_some() => continue,
            listing => listing?,
        };
        let running = agent_worktrees(&listing.terminals);
        for worktree in worktree_ids.iter().filter(|id| host_of(id) == host).filter_map(|id| listing.worktree(id)) {
            if !links(worktree, &command.identifier) {
                trace::event(Work::NotItsWorktree { path: worktree.path.clone(), issue: command.identifier.clone() });
                continue;
            }
            let seen = match () {
                () if waits_in(worktree, &listing.terminals) => Observed::AgentWaiting,
                () if idle_in(worktree, &listing.terminals).is_some() => Observed::AgentIdle,
                () if running.contains(&worktree.id) => Observed::AgentWorking,
                () => Observed::AgentExited,
            };
            observed(seen, &worktree.id);
        }
    }
    Ok(())
}
