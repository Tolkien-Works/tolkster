// What the panel shows, from what it holds and what it reads live as it draws: the budgets,
// the silent hosts and the update. Rows only: `draw` lays them out and colours them.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use super::frame::{Checks, DeadlineKind, Pr, TriageState, Why, Work};
use super::{Connection, Finish, Lane, State};
use crate::machine::{Observed, Outcome, Waiting};
use crate::time;

// How many failures the outcomes line names.
const FAILURES: usize = 3;
// The most blockers a queued row names, as the log's line does.
const NAMED: usize = 5;

// One API's budget, the scarcest of its budgets as its responses said.
#[derive(Clone, Debug, PartialEq)]
pub struct Budget {
    pub api: &'static str,
    pub limit: Option<u64>,
    pub remaining: u64,
    pub resets: Duration,
    pub spent: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Silence {
    pub host: String,
    pub name: Option<String>,
    // A recipe machine put to sleep: it needs nothing from the owner.
    pub asleep: bool,
    pub since: Instant,
    pub next: Instant,
}

// What `draw` reads live, beside the panel's state.
#[derive(Clone, Debug, Default)]
pub struct Live {
    pub budgets: Vec<Budget>,
    pub silences: Vec<Silence>,
    pub commit: Option<String>,
    pub building: Option<Instant>,
    pub draining: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Need {
    Waiting(String),
    Stalled(String),
    NoRounds(String),
    Anomaly(String),
    Silent(String),
    Refused(String),
    Spent(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Link {
    Connecting,
    Connected(Duration),
    Reconnecting(Duration),
    Superseded,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Agent {
    Working,
    // Running, idle at its prompt while a call it sent to the background runs.
    Background,
    Waiting,
    Idle,
    // A slot the frame holds whose worker no listing has: as the frame says, reserved or
    // launching; else unseen, or silent when its own host is, or host silent when its host
    // isn't known and some host is.
    Reserved,
    Launching,
    Unseen,
    Silent,
    HostSilent,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SlotRow {
    pub issue: Option<String>,
    pub host: String,
    pub agent: Agent,
    // In that state, as the listings saw it.
    pub for_: Duration,
    // How long its worker has run, whatever its state.
    pub ran: Duration,
    // Its issue's newest finished command, with how long ago.
    pub last: Option<(String, Outcome, Duration)>,
    // The issue's lane's job, with how long it has run.
    pub job: Option<(String, Duration)>,
    // The frame's: its work's next deadline, named, with the time left; and its PR.
    pub deadline: Option<(String, Duration)>,
    pub pr: Option<Pr>,
    // The frame's: held for an issue seen completed or cancelled.
    pub anomaly: bool,
    // The frame's: its work is `merged`, so the worker is on its way out.
    pub merged: bool,
    // The queued issues blocked by this slot's issue, drawn under it.
    pub waiting: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParkedRow {
    pub issue: String,
    pub work: Work,
    pub stalled: bool,
    pub pr: Option<Pr>,
    // At rest with its PR open and no auto-merge: the owner's steps hold it.
    pub owner: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChainRow {
    pub issue: String,
    pub answered: u32,
    pub issues: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QueuedRow {
    pub issue: String,
    pub reason: String,
    // Every blocker, where `reason` names a few.
    pub blockers: Vec<String>,
    pub age: Duration,
    // The parked issue whose wait on the owner this one waits through.
    pub through: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Also {
    Job { issue: Option<String>, label: String, for_: Duration },
    Held { issue: Option<String>, label: String, left: Duration },
}

#[derive(Clone, Debug, PartialEq)]
pub struct HostRow {
    pub name: String,
    pub used: i64,
    pub cap: i64,
    pub answering: bool,
    pub refused: bool,
    pub asleep: bool,
    pub silent_for: Option<Duration>,
    pub asked_in: Option<Duration>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcomes {
    pub ok: usize,
    pub failed: usize,
    pub retryable: usize,
    pub failures: Vec<(String, String, &'static str)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Update {
    pub commit: Option<String>,
    pub base: Option<String>,
    pub behind: Option<bool>,
    pub building: Option<Duration>,
    pub draining: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub needs: Vec<Need>,
    pub link: Link,
    pub reconnects: u32,
    pub ping: Option<Duration>,
    pub update: Update,
    pub cap: Option<i64>,
    pub hosts: Vec<HostRow>,
    pub budgets: Vec<Budget>,
    pub slots: Vec<SlotRow>,
    pub free: usize,
    pub queued: Vec<QueuedRow>,
    // The slots and queue are the coordinator's frame, rather than what this `listen` has
    // seen, which may be stale.
    pub framed: bool,
    pub parked: Vec<ParkedRow>,
    pub chains: Vec<ChainRow>,
    // Each repo the fleet asks no machine for while its request waits, and why.
    pub machines: Vec<(String, Why)>,
    // The triage machine when it isn't idle: its deadline's time left, and its failures.
    pub triage: Option<(TriageState, Option<Duration>, u32)>,
    pub also: Vec<Also>,
    pub conversations: Vec<(String, Duration)>,
    pub outcomes: Outcomes,
    // How long since the last listing, and how long it took.
    pub listing: Option<(Duration, Duration)>,
}

fn agent(observed: Observed) -> Agent {
    match observed {
        Observed::AgentWaiting => Agent::Waiting,
        Observed::AgentIdle => Agent::Idle,
        _ => Agent::Working,
    }
}

// What an agent's work state in the frame shows as; the other states say nothing of the agent.
fn worked(work: Work, background: bool) -> Option<Agent> {
    match work {
        Work::Running if background => Some(Agent::Background),
        Work::Running => Some(Agent::Working),
        Work::Idle => Some(Agent::Idle),
        Work::Held => Some(Agent::Waiting),
        _ => None,
    }
}

pub fn view(state: &State, live: &Live, now: Instant) -> View {
    let since = |at: Instant| now.saturating_duration_since(at);
    let listing = state.listing.as_ref();
    let mut slots: Vec<SlotRow> = listing.map(|l| l.slots.iter().map(|slot| {
        let host = slot.host.as_deref().and_then(|id| l.hosts.iter().find(|h| h.id.as_deref() == Some(id))).map_or_else(|| slot.host.clone().unwrap_or_else(|| "local".into()), |h| h.name.clone());
        let ran = since(state.began.get(&slot.worktree).copied().unwrap_or(l.at));
        let state_now = agent(slot.state);
        let from = state.changed.get(&slot.worktree).map_or(l.at, |(_, at)| *at);
        let job = slot.issue.as_ref().and_then(|issue| state.running.values().filter(|r| r.lane == Lane::Issue && r.issue.as_ref() == Some(issue)).min_by_key(|r| r.since)).map(|r| (r.label.clone(), since(r.since)));
        SlotRow { issue: slot.issue.clone(), host, agent: state_now, for_: since(from), ran, last: None, job, deadline: None, pr: None, anomaly: false, merged: false, waiting: vec![] }
    }).collect()).unwrap_or_default();
    slots.sort_by(|a, b| a.issue.cmp(&b.issue));
    let framed = state.frame.as_ref();
    let now_ms = time::now_ms();
    let left = |at_ms: i64| Duration::from_millis(u64::try_from(at_ms - now_ms).unwrap_or(0));
    let ago_ms = |at_ms: i64| Duration::from_millis(u64::try_from(now_ms - at_ms).unwrap_or(0));
    let name = |identifier: &Option<String>| identifier.clone().unwrap_or_else(|| "·".into());
    // The frame's slots fill in the listing's, and add each one no listing has a worker in
    // yet. One with no identifier known matches no row.
    let listed = slots.len();
    let host_silent = live.silences.iter().any(|s| !s.asleep) || listing.is_some_and(|l| l.hosts.iter().any(|h| !h.answering));
    for held in framed.iter().flat_map(|f| &f.slots) {
        let issue = name(&held.identifier);
        let deadline = held.deadline.map(|d| (deadline_name(d.kind, held.work), left(d.due_ms)));
        match slots[..listed].iter_mut().find(|s| held.identifier.is_some() && s.issue == held.identifier) {
            Some(row) => {
                // Held since before the listing saw its worker, or since before `listen` began.
                row.ran = row.ran.max(ago_ms(held.since_ms));
                // The agent's hooks reach the frame within seconds; the listing is on a backstop
                // and Orca's own reading of the agent can lag.
                if let Some(agent) = held.work.and_then(|work| worked(work, held.background)) {
                    row.agent = agent;
                }
                row.deadline = deadline;
                row.pr = held.pr;
                row.anomaly = held.anomaly;
                row.merged = held.work == Some(Work::Merged);
            }
            None => {
                // Its host by its paired name, from the listing or `listen`'s own count of silences.
                let paired = held.host.as_deref().map(|id| {
                    let share = listing.and_then(|l| l.hosts.iter().find(|h| h.id.as_deref() == Some(id)));
                    let silence = live.silences.iter().find(|s| s.host == id);
                    let name = share.map(|h| h.name.clone()).or_else(|| silence.and_then(|s| s.name.clone())).unwrap_or_else(|| id.into());
                    (name, silence.is_some_and(|s| !s.asleep) || share.is_some_and(|h| !h.answering))
                });
                let (host, agent) = match (held.work, paired) {
                    _ if held.reserved => ("·".into(), Agent::Reserved),
                    (Some(Work::Launching), _) => ("·".into(), Agent::Launching),
                    (_, Some((name, silent))) => (name, if silent { Agent::Silent } else { Agent::Unseen }),
                    _ if host_silent => ("·".into(), Agent::HostSilent),
                    _ => ("·".into(), Agent::Unseen),
                };
                slots.push(SlotRow { issue: Some(issue), host, agent, for_: ago_ms(held.since_ms), ran: ago_ms(held.since_ms), last: None, job: None, deadline, pr: held.pr, anomaly: held.anomaly, merged: held.work == Some(Work::Merged), waiting: vec![] });
            }
        }
    }
    for row in &mut slots {
        row.last = row.issue.as_ref().and_then(|issue| state.last.get(issue)).map(|f| (f.command.clone(), f.outcome, since(f.at)));
    }
    let slotted: HashSet<&str> = slots.iter().filter_map(|s| s.issue.as_deref()).collect();
    let cap = listing.map(|l| l.cap).or(framed.map(|f| f.cap));
    let free = cap.map_or(0, |cap| usize::try_from(cap).unwrap_or(0).saturating_sub(slots.len()));

    let mut queued: Vec<QueuedRow> = match framed {
        // In the coordinator's order, then the waits that aren't a slot's.
        Some(frame) => frame.queue.iter().map(|q| QueuedRow { issue: name(&q.identifier), reason: reason(q.waiting, &q.blockers), blockers: q.blockers.clone(), age: time::instant(&q.at).map_or(Duration::ZERO, ago_ms), through: None }).collect(),
        None => {
            let mut queued: Vec<QueuedRow> = state.queued.iter().filter(|(issue, _)| !slotted.contains(issue.as_str())).map(|(issue, q)| QueuedRow { issue: issue.clone(), reason: reason(q.waiting, &q.blocked_by), blockers: q.blocked_by.clone(), age: since(q.said), through: None }).collect();
            queued.sort_by(|a, b| a.age.cmp(&b.age).then_with(|| a.issue.cmp(&b.issue)));
            queued
        }
    };
    let parked: Vec<ParkedRow> = framed.iter().flat_map(|f| &f.parked).map(|p| ParkedRow { issue: name(&p.identifier), work: p.work, stalled: p.stalled, pr: p.pr, owner: p.work == Work::Rest && p.pr.is_some_and(|pr| pr.auto_merge == Some(false)) }).collect();
    through_the_owner(framed.map_or(&[], |f| &f.queue), &parked, &mut queued);
    let chains: Vec<ChainRow> = framed.iter().flat_map(|f| &f.chains).map(|c| ChainRow { issue: name(&c.identifier), answered: c.answered, issues: c.issues }).collect();
    let triage = framed.map(|f| f.triage).filter(|t| t.state != TriageState::Idle).map(|t| (t.state, t.due_ms.map(left), t.failures));
    let machines = framed.iter().flat_map(|f| f.why.iter().map(|(repo, why)| (repo.clone(), *why))).collect();

    let mut jobs: Vec<_> = state.running.values().filter(|r| r.lane != Lane::Issue || !r.issue.as_deref().is_some_and(|i| slotted.contains(i))).collect();
    jobs.sort_by_key(|r| r.since);
    // A queued row stays in Queued too: only a blocker that holds a slot has the row under it.
    for slot in &mut slots {
        slot.waiting = queued.iter().filter(|q| q.issue != "·" && slot.issue.as_ref().is_some_and(|i| q.blockers.contains(i))).map(|q| q.issue.clone()).collect();
    }
    let mut held: Vec<_> = state.held.iter().collect();
    held.sort_by_key(|h| h.due);
    let also = jobs.into_iter().map(|r| Also::Job { issue: r.issue.clone(), label: r.label.clone(), for_: since(r.since) }).chain(held.into_iter().map(|h| Also::Held { issue: h.issue.clone(), label: h.label.clone(), left: h.due.saturating_duration_since(now) })).collect();

    let silence = |id: &str| live.silences.iter().find(|s| s.host == id);
    let hosts: Vec<HostRow> = listing.map(|l| l.hosts.iter().map(|h| {
        let used = slots_on(l, h.id.as_deref());
        let silent = h.id.as_deref().and_then(silence);
        let asleep = silent.is_some_and(|s| s.asleep);
        HostRow { name: h.name.clone(), used, cap: h.cap, answering: h.answering && silent.is_none(), refused: h.refused, asleep, silent_for: silent.filter(|_| !asleep).map(|s| since(s.since)), asked_in: silent.filter(|_| !asleep).map(|s| s.next.saturating_duration_since(now)) }
    }).collect()).unwrap_or_default();

    let mut conversations: Vec<(String, Duration)> = state.conversations.values().map(|(issue, at)| (issue.clone(), since(*at))).collect();
    conversations.sort();

    let mut needs: Vec<Need> = slots.iter().filter(|s| s.agent == Agent::Waiting).map(|s| Need::Waiting(name(&s.issue))).collect();
    needs.extend(parked.iter().filter(|p| p.stalled && p.work == Work::Exited).map(|p| Need::Stalled(p.issue.clone())));
    // Failing checks with no review round left.
    let spent = |pr: &Option<Pr>| pr.is_some_and(|pr| pr.capped && pr.checks == Checks::Failed);
    needs.extend(slots.iter().filter(|s| spent(&s.pr)).map(|s| Need::NoRounds(name(&s.issue))));
    needs.extend(parked.iter().filter(|p| spent(&p.pr)).map(|p| Need::NoRounds(p.issue.clone())));
    needs.extend(slots.iter().filter(|s| s.anomaly).map(|s| Need::Anomaly(name(&s.issue))));
    needs.extend(hosts.iter().filter(|h| h.refused && !h.asleep).map(|h| Need::Refused(h.name.clone())));
    let mut silent: Vec<String> = hosts.iter().filter(|h| !h.answering && !h.asleep).map(|h| h.name.clone()).collect();
    // An asleep machine is none of it, whether the listing or `listen` says it is silent.
    for s in live.silences.iter().filter(|s| !s.asleep) {
        let name = s.name.clone().unwrap_or_else(|| s.host.clone());
        if !silent.contains(&name) {
            silent.push(name);
        }
    }
    needs.extend(silent.into_iter().map(Need::Silent));
    needs.extend(live.budgets.iter().filter(|b| b.spent).map(|b| Need::Spent(b.api)));

    let behind = live.commit.as_ref().zip(state.base.as_ref()).map(|(commit, base)| commit != base);
    let update = Update { commit: live.commit.clone(), base: state.base.clone(), behind, building: live.building.map(since), draining: live.draining || super::quitting() };
    let link = match state.connection {
        Connection::Connecting => Link::Connecting,
        Connection::Connected(at) => Link::Connected(since(at)),
        Connection::Reconnecting(until) => Link::Reconnecting(until.saturating_duration_since(now)),
        Connection::Superseded => Link::Superseded,
        Connection::Stopped => Link::Stopped,
    };
    View { needs, link, reconnects: state.reconnects, ping: state.ping, update, cap, hosts, budgets: live.budgets.clone(), slots, free, queued, framed: framed.is_some(), parked, chains, machines, triage, also, conversations, outcomes: outcomes(state.outcomes.iter(), now), listing: listing.map(|l| (since(l.at), l.took)) }
}

// The slots a listing saw on a host, this machine's when `host` is None.
fn slots_on(listing: &super::Listing, host: Option<&str>) -> i64 {
    listing.slots.iter().filter(|s| s.host.as_deref() == host).count() as i64
}

// A wait on blockers names, as `through`, the parked work waiting on the owner that it
// waits on, directly or through another queued wait: the frame's queue and `queued` are in
// step, and a blocker no identifier is known for is none.
fn through_the_owner(frame_queue: &[super::frame::Queued], parked: &[ParkedRow], queued: &mut [QueuedRow]) {
    let mut roots: std::collections::HashMap<&str, &str> = parked.iter().filter(|p| p.owner).map(|p| (p.issue.as_str(), p.issue.as_str())).collect();
    // Each pass adds the waits whose blocker is already known to be held; a chain is as long
    // as the passes it takes.
    loop {
        let before = roots.len();
        for q in frame_queue.iter().filter(|q| q.waiting == Waiting::Blockers) {
            let Some(issue) = q.identifier.as_deref() else { continue };
            if let Some(root) = q.blockers.iter().find_map(|b| roots.get(b.as_str()).copied()) {
                roots.entry(issue).or_insert(root);
            }
        }
        if roots.len() == before {
            break;
        }
    }
    for (q, row) in frame_queue.iter().zip(queued) {
        let issue = q.identifier.as_deref().filter(|_| q.waiting == Waiting::Blockers);
        row.through = issue.and_then(|i| roots.get(i).filter(|root| **root != i)).map(|root| root.to_string());
    }
}

// What a work's next deadline usually fires (docs/state-machine.md): "ends" only where that
// moves the work on, as a launch with no result does. Most states' own deadline is a notice
// or a look at the issue and its PR, which re-arms; `merged`'s reaps.
fn deadline_name(kind: DeadlineKind, work: Option<Work>) -> String {
    match kind {
        DeadlineKind::Own => match work {
            Some(Work::Launching) => "launching ends",
            Some(Work::Merged) => "reap",
            _ => "check",
        }
        .into(),
        DeadlineKind::Recheck => "recheck".into(),
        DeadlineKind::TakeBack => "take-back".into(),
        DeadlineKind::Quiet => "away notice".into(),
        DeadlineKind::Warm => "turn close".into(),
    }
}

// As the log's queued line says it, without its `queued: `.
fn reason(waiting: Waiting, blocked_by: &[String]) -> String {
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

// Over the last hour: a held command counts as retryable, as it is sent again; a gone one, which
// found no agent, is neither a success nor a failure.
fn outcomes<'f>(finished: impl DoubleEndedIterator<Item = &'f Finish>, now: Instant) -> Outcomes {
    let mut counts = Outcomes::default();
    let recent: Vec<&Finish> = finished.filter(|f| now.saturating_duration_since(f.at) <= super::OUTCOMES_FOR).collect();
    for f in &recent {
        match f.outcome {
            Outcome::Ok => counts.ok += 1,
            Outcome::Failed => counts.failed += 1,
            Outcome::Retryable | Outcome::Held => counts.retryable += 1,
            Outcome::Gone => {}
        }
    }
    counts.failures = recent.iter().rev().filter(|f| f.outcome == Outcome::Failed).take(FAILURES).map(|f| (f.issue.clone(), f.command.clone(), f.error.unwrap_or("failed"))).collect();
    counts
}
