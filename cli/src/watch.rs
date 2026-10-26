// What `tolkster listen` sees of Orca: the agents' listing, on a thread of its own, and
// what changed between two listings. It only reports, each listing whole as a `Snapshot`,
// whose agents' states the Durable Object derives each worker's from, and what changed
// (`Seen`): what to do about each is the machines' (scheduler.rs, `watch_agents`). What
// happened while nothing listened is read from the Durable Object's records (`Missed`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};


use crate::error::Error;
use crate::hosts::Fleet;
use crate::machine::Observed;
use crate::orca::{Terminal, Worktree, agent_panes, candidates, waits_in};
use crate::plain::{split_terminal, split_worktree};
use crate::sync::lock;
use crate::time::{iso, now_ms};
use crate::trace::{self, Watch};

// The backstop: a terminal's close or a job's Orca write lists at once (`Watcher`), so this only catches what no
// wait saw, and undoes a hook's state its agent left without another hook.
pub const WATCH_EVERY: Duration = Duration::from_secs(60);

// Whether the agents in a worktree sit idle, from Orca's own reading of each agent's hooks in
// `orca worktree ps --json`, matched to its terminal by pane. Waiting if any waits, since the
// latest of them changed. None when no agent terminal is open, any agent is working or in a
// state Orca adds later, or Orca says nothing of one: only a positive idle counts. The
// terminal's contents are never read.
pub fn idle_in(worktree: &Worktree, terminals: &[Terminal]) -> Option<String> {
    let mut since: Option<String> = None;
    for pane in agent_panes(terminals, &worktree.id) {
        let agent = worktree.agents.iter().find(|a| a.key == pane)?;
        if !matches!(agent.state.as_str(), "done" | "waiting" | "blocked") {
            return None;
        }
        let at = iso(agent.since_ms?);
        since = Some(since.map_or(at.clone(), |other| other.max(at)));
    }
    since
}

// Each candidate worktree with an agent waiting at a prompt (`waits_in`).
pub fn waiting_worktrees(worktrees: &[Worktree], terminals: &[Terminal]) -> HashSet<String> {
    candidates(worktrees).into_iter().filter(|w| waits_in(w, terminals)).map(|w| w.id.clone()).collect()
}

// Each candidate worktree whose open agents all sit idle (`idle_in`).
pub fn idle_worktrees(worktrees: &[Worktree], terminals: &[Terminal]) -> HashSet<String> {
    candidates(worktrees).into_iter().filter(|w| idle_in(w, terminals).is_some()).map(|w| w.id.clone()).collect()
}

// When something running began or last showed progress, None when nothing runs; shared
// with the thread that runs it.
#[derive(Clone, Default)]
pub struct Since(Arc<Mutex<Option<Instant>>>);

impl Since {
    pub fn set(&self, at: Option<Instant>) {
        *lock(&self.0) = at;
    }

    pub fn get(&self) -> Option<Instant> {
        *lock(&self.0)
    }

    pub fn older_than(&self, after: Duration) -> bool {
        self.get().is_some_and(|since| since.elapsed() >= after)
    }
}

// `idle` holds the worktrees whose agents all sit idle (`idle_in`). `listed` is None when the listing
// was incomplete (orca::complete_listing).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Agents {
    pub running: HashSet<String>,
    pub open: HashSet<String>,
    pub terminals: HashSet<String>,
    // The agents' terminals, each waited on for its exit.
    pub agents: HashSet<String>,
    pub idle: HashSet<String>,
    pub waiting: HashSet<String>,
    pub listed: Option<HashSet<String>>,
    // The worktrees Orca listed that aren't workers', not linked to an issue
    // (orca::candidates): a snapshot names none, as the owner's other checkouts are none of
    // the Durable Object's. Only a worktree known to be another's is left out, so a listing
    // that failed or was cut off never hides a worker's agent.
    pub others: HashSet<String>,
    // The paired hosts that answered.
    pub hosts: Vec<String>,
    // Every host paired, answering or not, but for one unpaired whose issues still show
    // `host offline` (health.rs, `withholding`): a worktree on any other is on an unpaired host.
    pub paired: Vec<String>,
    // This machine's workers and each paired host's entry (hosts.rs, `Fleet`), and the
    // settings as config.json and machines.json have them now (machine.rs, `config_line`).
    pub fleet: Fleet,
    pub config: String,
    // The recipe machines that hold nothing, by host ID, with their idle minutes (hosts.rs,
    // `idle`): none from a listing that failed. The dormant ones, by name, with their minutes
    // (hosts.rs, `dormant`): None while one is made, woken, slept or destroyed.
    pub idle_hosts: BTreeMap<String, i64>,
    pub dormant: Option<BTreeMap<String, i64>>,
}

impl Agents {
    // With what `before` held on each host in `silent`, which didn't answer this listing:
    // what is there is unknown, so nothing there is seen to close, end or exit. Its agents'
    // terminals aren't waited on (`agents`) until it answers again.
    pub fn keeping(mut self, before: &Agents, silent: &[String]) -> Agents {
        let on = |host: Option<&str>| host.is_some_and(|host| silent.iter().any(|s| s == host));
        let worktree_on = |id: &&String| on(split_worktree(id).and_then(|(host, _, _)| host));
        let terminal_on = |handle: &&String| on(split_terminal(handle).and_then(|(host, _)| host));
        for (now, then) in [(&mut self.running, &before.running), (&mut self.open, &before.open), (&mut self.idle, &before.idle), (&mut self.waiting, &before.waiting), (&mut self.others, &before.others)] {
            now.extend(then.iter().filter(worktree_on).cloned());
        }
        self.terminals.extend(before.terminals.iter().filter(terminal_on).cloned());
        self
    }
}

// What happened before the watcher's first listing (commands/listen/handlers.rs, `missed`): each
// conversations checkout with no terminal open. An agent's exit is the first snapshot's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Missed {
    pub closed: Vec<String>,
}

// One listing whole, but for `Agents::others`, for the Durable Object (machine.rs,
// `listing_line`): `began` by this process's monotonic clock and `at` by the wall clock, which
// only names it; each an agent runs in, with its state (`states_of`); every one
// Orca has on the hosts that answered, sorted, None when the listing was incomplete; those
// hosts, the paired ones and each one's entry, the settings, and the idle and dormant recipe
// machines.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub began: Instant,
    pub at: String,
    pub agents: Vec<(String, Observed)>,
    pub listed: Option<Vec<String>>,
    pub hosts: Vec<String>,
    pub paired: Vec<String>,
    pub fleet: Fleet,
    pub config: String,
    pub idle_hosts: BTreeMap<String, i64>,
    pub dormant: Option<BTreeMap<String, i64>>,
}

impl Snapshot {
    // Whether it shows what `other` does, whenever each began.
    pub fn same(&self, other: &Snapshot) -> bool {
        self.agents == other.agents && self.listed == other.listed && self.hosts == other.hosts && self.paired == other.paired && self.fleet == other.fleet && self.idle_hosts == other.idle_hosts && self.dormant == other.dormant
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Seen {
    Closed(String),
    Ended(String),
}

// What reaches the watcher's thread between two listings.
#[derive(Debug)]
pub enum Nudge {
    // A terminal `listen` opened itself, a turn's or triage's: waited on at once, and its end
    // said as `Seen::Ended` by its wait, whether or not a listing ever saw it.
    Own(String),
    // A wait saw its terminal exit, named when it was one of `listen`'s own: its end is said
    // once a fresh listing lacks it. Unnamed, it is only a listing now: a non-own terminal
    // exited, or a job's Orca write changed the worktrees or terminals (`orca::take_listing_stale`).
    Exited(Option<String>),
    // The wait on one of `listen`'s own failed for good.
    GaveUp(String),
}

// Lists on a thread of its own every WATCH_EVERY, and at once when a terminal it waits on
// exits or a job changes the worktrees or terminals. It sends each listing whole, and says what changed. It waits (`wait`, on a thread of its own for each,
// true once the terminal has exited, false when to ask again) on each agent's terminal a
// listing names and on each of `listen`'s own (`Nudge::Own`). The first
// listing has nothing to compare against, so what changed before it is what `missed`
// names. A `missed` that fails is asked again of each listing until it answers. A failed
// listing is skipped.
pub struct Watcher {
    pub seen: Receiver<Seen>,
    pub snapshots: Receiver<Snapshot>,
    // When the running listing began: one that never returns restarts `listen`.
    pub since: Since,
    pub nudge: Sender<Nudge>,
}

impl Watcher {
    pub fn spawn(mut list: impl FnMut() -> Result<Agents, Error> + Send + 'static, mut missed: impl FnMut(&Agents) -> Result<Missed, Error> + Send + 'static, wait: impl Fn(&str) -> Result<bool, Error> + Send + Sync + 'static, every: Duration) -> Watcher {
        let (found, seen) = mpsc::channel();
        let (snapshot, snapshots) = mpsc::channel();
        let since = Since::default();
        let listing = since.clone();
        let wait: Wait = Arc::new(wait);
        // Held here too, so `recv_timeout` never finds it disconnected.
        let (nudge, nudges) = mpsc::channel::<Nudge>();
        let nudged = nudge.clone();
        std::thread::spawn(move || {
            let waits = Arc::new(Waits::default());
            // `listen`'s own terminals whose wait said they ended: a listing that sees one go
            // says nothing more of it, nor of one whose wait is still to say it (`Waits::own`).
            let mut said_ended: HashSet<String> = HashSet::new();
            // Those a listing saw go while their wait was still to say it: said by the watcher
            // itself if that wait gives up, as no later listing will.
            let mut passed_over: HashSet<String> = HashSet::new();
            // How often a listing has contradicted an own terminal's wait since it was last watched.
            let mut rechecks: HashMap<String, u32> = HashMap::new();
            let mut last: Option<Agents> = None;
            let mut caught_up = false;
            loop {
                let began = Instant::now();
                let at = iso(now_ms());
                listing.set(Some(began));
                let listed = list();
                listing.set(None);
                match listed {
                    Ok(now) => {
                        let mut first = Missed::default();
                        if !caught_up {
                            listing.set(Some(Instant::now()));
                            let answer = missed(&now);
                            listing.set(None);
                            match answer {
                                Ok(answer) => (first, caught_up) = (answer, true),
                                Err(error) => trace::event(Watch::Missed(error)),
                            }
                        }
                        let closed = first.closed.into_iter().chain(last.iter().flat_map(|before| before.open.difference(&now.open)).cloned()).map(Seen::Closed);
                        let ended: Vec<Seen> = last.iter().flat_map(|before| before.terminals.difference(&now.terminals)).filter(|handle| {
                                if said_ended.remove(*handle) {
                                    return false;
                                }
                                let own = waits.is_own(handle);
                                if own {
                                    passed_over.insert((*handle).clone());
                                }
                                !own
                            })
                            .map(|handle| Seen::Ended(handle.clone())).collect();
                        // Kept only while listed: one gone from this listing has been passed over.
                        said_ended.retain(|handle| now.terminals.contains(handle));
                        if closed.chain(ended).any(|seen| found.send(seen).is_err()) {
                            return;
                        }
                        let mut listed: Option<Vec<String>> = now.listed.as_ref().map(|ids| ids.difference(&now.others).cloned().collect());
                        if let Some(ids) = listed.as_mut() {
                            ids.sort();
                        }
                        // Only reported: a scheduler gone ends the thread through `found`.
                        let agents = states_of(&now).into_iter().filter(|(id, _)| !now.others.contains(id)).collect();
                        let _ = snapshot.send(Snapshot { began, at, agents, listed, hosts: now.hosts.clone(), paired: now.paired.clone(), fleet: now.fleet.clone(), config: now.config.clone(), idle_hosts: now.idle_hosts.clone(), dormant: now.dormant.clone() });
                        for handle in waits.to_start(&now.agents) {
                            wait_for(handle, Arc::clone(&wait), Arc::downgrade(&waits), nudged.clone(), Duration::ZERO);
                        }
                        last = Some(now);
                    }
                    Err(error) => trace::event(Watch::Listing(error)),
                }
                // Until the backstop is due or a nudge asks to list: many at once make one listing.
                let due = Instant::now() + every;
                let mut list_now = false;
                loop {
                    let next = if list_now { nudges.try_recv().ok() } else { nudges.recv_timeout(due.saturating_duration_since(Instant::now())).ok() };
                    let Some(next) = next else { break };
                    match next {
                        Nudge::Own(handle) => {
                            rechecks.remove(&handle);
                            if waits.own(&handle) {
                                wait_for(handle, Arc::clone(&wait), Arc::downgrade(&waits), nudged.clone(), Duration::ZERO);
                            }
                        }
                        Nudge::Exited(Some(handle)) => {
                            // Orca's wait answers `exited` for a terminal still open, so the end
                            // is said only when a fresh listing lacks the terminal. A listing
                            // that fails, or a host that is silent (`Agents::keeping`), says
                            // nothing: the wait starts again.
                            listing.set(Some(Instant::now()));
                            let fresh = list();
                            listing.set(None);
                            // A listing that fails says nothing, so it is not counted against the terminal.
                            let (still, failed) = match fresh {
                                Ok(now) => (now.terminals.contains(&handle), false),
                                Err(error) => {
                                    trace::event(Watch::Listing(error));
                                    (true, true)
                                }
                            };
                            if still {
                                trace::event(Watch::StillListed { handle: handle.clone() });
                                let checked = rechecks.entry(handle.clone()).or_default();
                                if !failed {
                                    *checked += 1;
                                }
                                let delay = RECHECK_AFTER * (*checked + u32::from(failed));
                                waits.lock().exited.remove(&handle);
                                if *checked > OWN_RETRIES {
                                    // A tab Orca keeps listed once it has exited would loop for
                                    // good: left to the listings, which end it when it goes, the
                                    // next one at once so that it holds the terminal.
                                    rechecks.remove(&handle);
                                    waits.lock().own.remove(&handle);
                                    list_now = true;
                                } else if waits.own(&handle) {
                                    wait_for(handle, Arc::clone(&wait), Arc::downgrade(&waits), nudged.clone(), delay);
                                }
                                continue;
                            }
                            rechecks.remove(&handle);
                            waits.lock().own.remove(&handle);
                            passed_over.remove(&handle);
                            if found.send(Seen::Ended(handle.clone())).is_err() {
                                return;
                            }
                            said_ended.insert(handle);
                            list_now = true;
                        }
                        Nudge::GaveUp(handle) => {
                            if passed_over.remove(&handle) && found.send(Seen::Ended(handle)).is_err() {
                                return;
                            }
                        }
                        Nudge::Exited(None) => list_now = true,
                    }
                }
            }
        });
        Watcher { seen, snapshots, since, nudge }
    }
}

// Blocks until a terminal exits (`Watcher::spawn`'s `wait`).
type Wait = Arc<dyn Fn(&str) -> Result<bool, Error> + Send + Sync>;

// A wait that says its terminal is still open sooner than this isn't asked again: it would spin.
const ASK_AGAIN_AFTER: Duration = Duration::from_secs(1);
const OWN_RETRIES: u32 = 3;
// A wait that said exited while a listing still has the terminal is asked again after this,
// longer each time.
const RECHECK_AFTER: Duration = if cfg!(test) { Duration::from_millis(20) } else { ASK_AGAIN_AFTER };
const OWN_RETRY_AFTER: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Waits(Mutex<WaitSets>);

// The terminals being waited on; those whose wait said they exited, one still listed after
// its exit never waited on again, or each listing would start the next; the agents'
// terminals the last listing named; and `listen`'s own.
#[derive(Default)]
struct WaitSets {
    waiting: HashSet<String>,
    exited: HashSet<String>,
    listed: HashSet<String>,
    own: HashSet<String>,
}

impl Waits {
    fn lock(&self) -> MutexGuard<'_, WaitSets> {
        lock(&self.0)
    }

    fn to_start(&self, listed: &HashSet<String>) -> Vec<String> {
        let mut sets = self.lock();
        sets.listed.clone_from(listed);
        sets.exited.retain(|handle| listed.contains(handle));
        let start: Vec<String> = listed.iter().filter(|handle| !sets.waiting.contains(*handle) && !sets.exited.contains(*handle)).cloned().collect();
        sets.waiting.extend(start.iter().cloned());
        start
    }

    // Whether to start its wait: not while one runs already.
    fn own(&self, handle: &str) -> bool {
        let mut sets = self.lock();
        sets.own.insert(handle.to_string());
        sets.waiting.insert(handle.to_string())
    }

    // Asked again only while an agent's terminal the last listing named, or one of `listen`'s own.
    fn wanted(&self, handle: &str) -> bool {
        let sets = self.lock();
        sets.own.contains(handle) || sets.listed.contains(handle)
    }

    fn is_own(&self, handle: &str) -> bool {
        self.lock().own.contains(handle)
    }

    // Whether it was one of `listen`'s own. One that exited stays so until the watcher's thread
    // has said its end, so no listing says it first; one whose wait failed is left to the
    // listings.
    fn ended(&self, handle: &str, gone: bool) -> bool {
        let mut sets = self.lock();
        sets.waiting.remove(handle);
        if gone {
            sets.exited.insert(handle.to_string());
            sets.own.contains(handle)
        } else {
            sets.own.remove(handle)
        }
    }
}

// Waits for `handle` to exit, asking again while it is still open and wanted, the first time
// after `delay`. A failed wait leaves it to the listings. Released before it says so, so the
// listing it starts finds it exited. Ends with the watcher, whose `Waits` it only borrows.
fn wait_for(handle: String, wait: Wait, waits: Weak<Waits>, nudge: Sender<Nudge>, delay: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        let mut failed = 0;
        let gone = loop {
            let asked = Instant::now();
            match wait(&handle) {
                Ok(true) => break true,
                Ok(false) if asked.elapsed() >= ASK_AGAIN_AFTER => failed = 0,
                Ok(false) => {
                    trace::event(Watch::EndedAtOnce);
                    break false;
                }
                Err(error) => {
                    trace::event(Watch::Wait(error));
                    // One of `listen`'s own may never be listed, so its end has no other source:
                    // Orca restarting is waited out a little.
                    failed += 1;
                    if failed > OWN_RETRIES || !waits.upgrade().is_some_and(|w| w.is_own(&handle)) {
                        break false;
                    }
                    std::thread::sleep(OWN_RETRY_AFTER);
                    continue;
                }
            }
            if !waits.upgrade().is_some_and(|w| w.wanted(&handle)) {
                break false;
            }
        };
        let Some(waits) = waits.upgrade() else { return };
        let own = waits.ended(&handle, gone);
        // A watcher gone has nothing to list.
        let _ = match (gone, own) {
            (true, _) => nudge.send(Nudge::Exited(own.then_some(handle))),
            (false, true) => nudge.send(Nudge::GaveUp(handle)),
            (false, false) => Ok(()),
        };
    });
}

// Each worktree an agent runs in, as working, idle or waiting, sorted.
pub fn states_of(now: &Agents) -> Vec<(String, Observed)> {
    let state = |id: &String| match () {
        () if now.waiting.contains(id) => Observed::AgentWaiting,
        () if now.idle.contains(id) => Observed::AgentIdle,
        () => Observed::AgentWorking,
    };
    let mut states: Vec<(String, Observed)> = now.running.iter().map(|id| (id.clone(), state(id))).collect();
    states.sort_by(|a, b| a.0.cmp(&b.0));
    states
}

#[cfg(test)]
mod tests;
