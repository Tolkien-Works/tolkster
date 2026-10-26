// `tolkster listen`'s panel (docs/realtime.md): what it holds now, beside its log. Each
// call site below tells it what it already knows, as it happens, and the panel only keeps
// it: nothing here decides what is carried out or when. Beside the log's own lines, it holds
// IDs, enums, hosts and times only, never Linear or GitHub text. `screen` draws it, when the
// terminal shows it; with it off, each record is kept, and the log goes to stdout as ever.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use self::frame::Work;
use crate::machine::{Observed, Outcome, Waiting};
use crate::sync::lock;
use crate::trace::Logged;

mod draw;
mod frame;
mod screen;
mod view;

pub use frame::{Frame, parse as parse_frame};
pub use screen::start;

// The log pane's lines, the oldest dropped first.
const LOG_LINES: usize = 2000;
// What finished, for the counts over the last hour, and an issue's last step while no slot
// holds it.
const OUTCOMES_FOR: Duration = Duration::from_secs(60 * 60);
// A queued reason no command has said for this long is dropped: its request may have gone.
const QUEUED_FOR: Duration = Duration::from_secs(60 * 60);
// A terminal a lane opened that no listing has yet is kept this long: its wait may never say
// it ended.
const UNLISTED_FOR: Duration = Duration::from_secs(2 * 60);

// The panel is this process's terminal, so one per process: what any thread logs or reports
// reaches it here (trace.rs), with no handle passed.
static ON: AtomicBool = AtomicBool::new(false);
static QUIT: AtomicBool = AtomicBool::new(false);
static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));
// Wakes the connection's loop, so `q` is taken at once.
static WAKE: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

// Ambient: set where a lane's thread takes its job, and read where the job opens a terminal,
// which no signature between them carries.
thread_local! {
    // The issue of the job this lane's thread runs, for a terminal it opens.
    static ISSUE: RefCell<Option<String>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Connection {
    #[default]
    Connecting,
    Connected(Instant),
    Reconnecting(Instant),
    Superseded,
    Stopped,
}

// A lane's kind: an issue's, an issue's urgent one, or the shared lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Issue,
    Urgent,
    Shared,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Running {
    pub issue: Option<String>,
    pub label: String,
    pub lane: Lane,
    pub since: Instant,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    pub issue: Option<String>,
    pub label: String,
    pub due: Instant,
}

// One worker worktree with an agent, as a listing had it.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub worktree: String,
    pub issue: Option<String>,
    pub host: Option<String>,
    pub state: Observed,
}

// A paired host or this machine (`id` None), with its configured cap, not its share of the slots
// (hosts.rs, `shares`), which is more when it holds more workers than that.
#[derive(Clone, Debug, PartialEq)]
pub struct HostShare {
    pub id: Option<String>,
    pub name: String,
    pub cap: i64,
    pub answering: bool,
    // Its `tolkster` can't read its config.json, so it takes no work until rejoined.
    pub refused: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Listing {
    pub at: Instant,
    pub took: Duration,
    pub slots: Vec<Slot>,
    pub hosts: Vec<HostShare>,
    pub cap: i64,
}

// `at` is when the Durable Object queued the request, as it says it.
#[derive(Clone, Debug, PartialEq)]
pub struct Queued {
    pub waiting: Waiting,
    pub blocked_by: Vec<String>,
    pub at: String,
    pub said: Instant,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finish {
    pub at: Instant,
    pub issue: String,
    pub command: String,
    pub outcome: Outcome,
    // An error's kind, never its text.
    pub error: Option<&'static str>,
}

#[derive(Default)]
pub struct State {
    pub connection: Connection,
    pub reconnects: u32,
    pub pinged: Option<Instant>,
    pub ping: Option<Duration>,
    pub listing: Option<Listing>,
    // Each worktree since the first listing that saw it.
    pub began: HashMap<String, Instant>,
    // Each worktree's agent's state, and since the first listing that saw it so.
    pub changed: HashMap<String, (Observed, Instant)>,
    // By lane thread.
    pub running: HashMap<usize, Running>,
    pub held: Vec<Held>,
    pub queued: HashMap<String, Queued>,
    // A turn's or triage's terminal `listen` opened, by its handle, with its issue.
    pub conversations: HashMap<String, (String, Instant)>,
    pub outcomes: VecDeque<Finish>,
    // Each issue's newest finished command.
    pub last: HashMap<String, Finish>,
    // Tolkster's own main, as its newest base tip said.
    pub base: Option<String>,
    // The coordinator's, as the relay last sent it on this connection.
    pub frame: Option<Frame>,
    // The issues whose frame slot a listing was asked for because no listing had its worker.
    pub asked: HashSet<String>,
    pub log: VecDeque<Logged>,
}

impl State {
    // A worktree's times go once a listing no longer has it, an issue's last step once no
    // slot, listed or framed, holds it and it is older than OUTCOMES_FOR, a conversation once a listing no longer
    // has its terminal, `terminals`, and a queued reason once its issue holds a slot or nothing
    // has said it for QUEUED_FOR.
    fn list(&mut self, listing: Listing, terminals: &HashSet<String>) {
        let listed = |worktree: &String| listing.slots.iter().any(|s| &s.worktree == worktree);
        let slotted = |issue: &String| listing.slots.iter().any(|s| s.issue.as_ref() == Some(issue));
        let framed = |issue: &String| self.frame.iter().flat_map(|f| &f.slots).any(|s| s.identifier.as_ref() == Some(issue));
        self.last.retain(|issue, f| slotted(issue) || framed(issue) || listing.at.saturating_duration_since(f.at) < OUTCOMES_FOR);
        self.queued.retain(|issue, q| !slotted(issue) && listing.at.saturating_duration_since(q.said) < QUEUED_FOR);
        self.asked.retain(|issue| !slotted(issue));
        self.began.retain(|worktree, _| listed(worktree));
        self.changed.retain(|worktree, _| listed(worktree));
        self.conversations.retain(|terminal, (_, opened)| terminals.contains(terminal) || listing.at.saturating_duration_since(*opened) < UNLISTED_FOR);
        for slot in &listing.slots {
            self.began.entry(slot.worktree.clone()).or_insert(listing.at);
            let changed = self.changed.entry(slot.worktree.clone()).or_insert((slot.state, listing.at));
            if changed.0 != slot.state {
                *changed = (slot.state, listing.at);
            }
        }
        self.listing = Some(listing);
    }

    fn connect(&mut self, now: Connection) {
        if matches!(now, Connection::Reconnecting(_)) {
            self.reconnects += 1;
        }
        // Each connection is sent its own frame as it opens.
        self.frame = None;
        self.connection = now;
    }

    // `blocked_by` None, when the names weren't read again, keeps those last said for the
    // same wait. One older than the reason held, from a command that ran late, is dropped.
    fn queue(&mut self, issue: &str, waiting: Waiting, at: &str, blocked_by: Option<Vec<String>>, now: Instant) {
        let held = self.queued.get(issue);
        if held.is_some_and(|q| q.at.as_str() > at) {
            return;
        }
        let kept = held.filter(|q| q.waiting == waiting && q.at == at).map(|q| q.blocked_by.clone());
        let blocked_by = blocked_by.or(kept).unwrap_or_default();
        self.queued.insert(issue.to_string(), Queued { waiting, blocked_by, at: at.into(), said: now });
    }

    // The newest frame, and each repo's reason the fleet asks no machine for it that this frame
    // changed, for the caller to log once it has let go of the state.
    fn reframe(&mut self, frame: Option<Frame>) -> Vec<(String, &'static str)> {
        let before = self.frame.as_ref().map(|f| &f.why);
        let changed = frame.iter().flat_map(|now| &now.why).filter(|(repo, why)| before.and_then(|b| b.get(*repo)) != Some(*why)).map(|(repo, why)| (repo.clone(), why.said())).collect();
        self.frame = frame;
        changed
    }

    // Whether the newest frame holds a worker running, idle or held that the newest listing has
    // none for, and no listing was asked for since it began being so. A listing that still lacks
    // it leaves it unseen without another ask (`asked`), until a listing has it or the frame
    // no longer does.
    fn unlisted(&mut self) -> bool {
        let Some(listing) = &self.listing else { return false };
        let missing: HashSet<&String> = self.frame.iter().flat_map(|f| &f.slots)
            .filter(|s| !s.reserved && matches!(s.work, Some(Work::Running | Work::Idle | Work::Held)))
            .filter_map(|s| s.identifier.as_ref())
            .filter(|issue| !listing.slots.iter().any(|l| l.issue.as_ref() == Some(*issue)))
            .collect();
        self.asked.retain(|issue| missing.contains(issue));
        let before = self.asked.len();
        self.asked.extend(missing.into_iter().cloned());
        self.asked.len() > before
    }

    fn finish(&mut self, finish: Finish) {
        while self.outcomes.front().is_some_and(|f| finish.at.saturating_duration_since(f.at) > OUTCOMES_FOR) {
            self.outcomes.pop_front();
        }
        // Two lanes of one issue can finish at once: the older, locked last, keeps the newer.
        if self.last.get(&finish.issue).is_none_or(|l| l.at <= finish.at) {
            self.last.insert(finish.issue.clone(), finish.clone());
        }
        self.outcomes.push_back(finish);
    }
}

fn state() -> MutexGuard<'static, State> {
    lock(&STATE)
}

// Whether the panel shows, and the log goes to it rather than to stdout.
pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

fn set_on(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

// `q` was pressed: `listen` drains and exits, as a supersede does.
pub fn quitting() -> bool {
    QUIT.load(Ordering::Relaxed)
}

fn quit() {
    QUIT.store(true, Ordering::Relaxed);
    if let Some(wake) = lock(&WAKE).as_ref() {
        wake();
    }
}

pub fn waking(wake: impl Fn() + Send + 'static) {
    *lock(&WAKE) = Some(Box::new(wake));
}

// False once the panel is off: the line is then the caller's to write.
pub fn log(line: Logged) -> bool {
    // Every log line asks: a look without the lock first, then again under it.
    if !on() {
        return false;
    }
    let mut state = state();
    if !on() {
        return false;
    }
    log_into(&mut state, line);
    true
}

fn log_into(state: &mut State, line: Logged) {
    if state.log.len() == LOG_LINES {
        state.log.pop_front();
    }
    state.log.push_back(line);
}

pub fn connection(now: Connection) {
    state().connect(now);
}

pub fn pinged() {
    state().pinged = Some(Instant::now());
}

// The relay's answer to the last ping.
pub fn ponged() {
    let mut state = state();
    if let Some(sent) = state.pinged.take() {
        state.ping = Some(sent.elapsed());
    }
}

pub fn listed(listing: Listing, terminals: &HashSet<String>) {
    state().list(listing, terminals);
}

// A lane's thread began a job, or ended one (`None`).
pub fn job(thread: usize, running: Option<Running>) {
    ISSUE.with(|i| *i.borrow_mut() = running.as_ref().and_then(|r| r.issue.clone()));
    let mut state = state();
    match running {
        Some(running) => state.running.insert(thread, running),
        None => state.running.remove(&thread),
    };
}

pub fn held(held: Vec<Held>) {
    state().held = held;
}

// A command carried the request's queued reason.
pub fn queued(issue: &str, waiting: Waiting, at: &str, blocked_by: Option<Vec<String>>) {
    state().queue(issue, waiting, at, blocked_by, Instant::now());
}

// A terminal a lane's job opened, of its issue.
pub fn opened(terminal: &str) {
    let Some(issue) = ISSUE.with(|i| i.borrow().clone()) else { return };
    state().conversations.entry(terminal.to_string()).or_insert((issue, Instant::now()));
}

pub fn ended(terminal: &str) {
    state().conversations.remove(terminal);
}

pub fn finished(finish: Finish) {
    state().finish(finish);
}

// None for one that couldn't be read.
// The newest frame. Each repo's reason the fleet asks no machine for it goes to the log as it
// changes, so `listen.log` says why a request waits. Logged once the state is let go: with the
// panel on, a log line takes it again (`log`), and a std Mutex isn't reentrant.
// True when the frame shows a worker the newest listing lacks, for the caller to ask for
// another listing (`unlisted`).
pub fn framed(frame: Option<Frame>) -> bool {
    let (changed, unlisted) = {
        let mut state = state();
        (state.reframe(frame), state.unlisted())
    };
    for (repo, why) in changed {
        crate::trace::event(crate::trace::Host::NoMachine { repo, why });
    }
    unlisted
}

pub fn base(after: &str) {
    state().base = Some(after.to_string());
}

#[cfg(test)]
mod tests;
