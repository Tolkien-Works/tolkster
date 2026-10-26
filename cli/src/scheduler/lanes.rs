// The scheduler's lanes (scheduler.rs): the board their threads take jobs from, and the
// outbox a job's calls put results and observations in, from the lane's thread, until
// `Scheduler::step` sends them.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use crate::machine::{Command, Finished, Observed, Outcome, result_line};
use crate::panel;
use crate::sync::{lock, wait};
use crate::watch::{Nudge, Since};

use super::Job;

// How many issues' jobs run at once, beside the shared lane's one, on threads of their
// own, up to LANE_THREADS: a stop's lane needs no room among them.
pub(super) const ISSUE_LANES: usize = 4;
pub(super) const LANE_THREADS: usize = 12;

// Where a job runs: an issue's lane, keyed by the issue's identifier, or the one lane for
// what spans issues.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Lane {
    Issue(String),
    Shared,
}

impl Lane {
    pub(super) fn urgent(&self) -> bool {
        matches!(self, Lane::Issue(key) if key.starts_with(URGENT))
    }

    pub(super) fn shown(&self) -> panel::Lane {
        match self {
            Lane::Shared => panel::Lane::Shared,
            lane if lane.urgent() => panel::Lane::Urgent,
            Lane::Issue(_) => panel::Lane::Issue,
        }
    }
}

pub(super) const URGENT: &str = "urgent:";

pub(super) struct Queued {
    pub(super) id: String,
    pub(super) lane: Lane,
    pub(super) owners: bool,
    pub(super) job: Job,
}

// The jobs sent to the lanes and not yet taken, the lanes with a job running, and how many
// of the issues' jobs aren't waiting for a lock.
#[derive(Default)]
pub(super) struct Board {
    pub(super) waiting: VecDeque<Queued>,
    pub(super) running: HashSet<Lane>,
    pub(super) active: usize,
    pub(super) closed: bool,
}

#[derive(Default)]
pub(super) struct Lanes {
    pub(super) board: Mutex<Board>,
    pub(super) changed: Condvar,
    pub(super) outbox: Mutex<Outbox>,
    // Wakes the connection's loop once a job has ended, so its result goes out at once
    // (`Scheduler::waking`).
    pub(super) wake: Mutex<Option<Box<dyn Fn() + Send>>>,
    // The watcher's, for the terminals a lane opens (`watch_terminal`) and a job's listing writes (`list_now`).
    pub(super) nudge: Mutex<Option<Sender<Nudge>>>,
}

impl Lanes {
    pub(super) fn board(&self) -> MutexGuard<'_, Board> {
        lock(&self.board)
    }

    // False once the lanes have closed.
    pub(super) fn put(&self, queued: Queued) -> bool {
        let mut board = self.board();
        board.waiting.push_back(queued);
        self.changed.notify_all();
        !board.closed
    }

    // Waits for a job whose lane runs nothing, of the shared lane or of an issue's as
    // `shared` says, and for an issue's, room under ISSUE_LANES: the owner's oldest, else
    // the oldest. None once the scheduler has gone.
    pub(super) fn take(&self, shared: bool) -> Option<Queued> {
        let mut board = self.board();
        loop {
            if board.closed {
                return None;
            }
            let room = shared || board.active < ISSUE_LANES;
            // A stop never waits for room behind the commands it may overtake.
            let free = |q: &Queued| (room || q.lane.urgent()) && (q.lane == Lane::Shared) == shared && !board.running.contains(&q.lane);
            let at = board.waiting.iter().position(|q| q.owners && free(q)).or_else(|| board.waiting.iter().position(free));
            if let Some(queued) = at.and_then(|at| board.waiting.remove(at)) {
                board.running.insert(queued.lane.clone());
                board.active += usize::from(!shared);
                return Some(queued);
            }
            board = wait(&self.changed, board);
        }
    }

    pub(super) fn done(&self, lane: &Lane, shared: bool) {
        let mut board = self.board();
        board.running.remove(lane);
        board.active -= usize::from(!shared);
        self.changed.notify_all();
    }

    // A job changed which worktrees or agents Orca has: the watcher lists at once, not at its next backstop.
    pub(super) fn list_now(&self) {
        if let Some(nudge) = lock(&self.nudge).as_ref() {
            // A watcher gone has nothing to list.
            let _ = nudge.send(Nudge::Exited(None));
        }
    }

    pub(super) fn close(&self) {
        self.board().closed = true;
        self.changed.notify_all();
    }
}

// The scheduler's hold on its lanes: once it is dropped, their threads end.
pub(super) struct Owned(pub(super) Arc<Lanes>);

impl Drop for Owned {
    fn drop(&mut self) {
        self.0.close();
    }
}

// A lane's thread: since when it has acted on its job, and which, for the watchdog.
#[derive(Clone, Default)]
pub(super) struct Acting {
    pub(super) since: Since,
    pub(super) id: Arc<Mutex<String>>,
}

// Ambient: a job's calls reach its lanes (the outbox, the watchdog) from deep in whatever the
// job runs, which no signature carries.
thread_local! {
    // A lane's thread's lanes, its watchdog entry, and whether it is the shared lane's.
    pub(super) static LANE: RefCell<Option<(Arc<Lanes>, Acting, bool)>> = const { RefCell::new(None) };
}

// What `listen` saw in Orca, until `step` sends it, numbered on its connection.
pub(super) enum Outgoing {
    Worktree(Observed, String),
    TurnEnded(String),
    // A terminal Orca stopped listing, by its handle: the Durable Object finds what it ran.
    TerminalClosed(String),
}

// The lanes' outbox: each command's result as its lane's thread carried it out, and
// what a command observed, until `step` sends them.
#[derive(Default)]
pub(super) struct Outbox {
    // (command ID, result line, whether it settled the command)
    pub(super) results: Vec<(String, String, bool)>,
    pub(super) observations: Vec<Outgoing>,
}

// Runs `with` on the lanes of the run this thread is a lane of; None off a lane's thread,
// where there is no connection to send anything on, and it is dropped.
pub(super) fn with_lanes<R>(with: impl FnOnce(&Lanes) -> R) -> Option<R> {
    LANE.with(|l| l.borrow().as_ref().map(|(lanes, _, _)| with(lanes)))
}

// Called on a lane's thread, like every function below that puts or reads a lane's state.
pub(super) fn outbox(put: impl FnOnce(&mut Outbox)) {
    with_lanes(|lanes| put(&mut lock(&lanes.outbox)));
}

// A terminal a lane opened, a turn's or triage's: the watcher waits on it at once, and says
// when it ends (`Nudge::Own`), though no listing may have seen it.
pub fn watch_terminal(terminal: &str) {
    panel::opened(terminal);
    with_lanes(|lanes| {
        if let Some(nudge) = lock(&lanes.nudge).as_ref() {
            // A watcher gone has nothing to wait for.
            let _ = nudge.send(Nudge::Own(terminal.into()));
        }
    });
}

// The watcher saw `terminal` close. Whether it ran a turn or the triage session, and whether
// that is still the one the Durable Object holds, is its to say: it holds each one's terminal.
pub fn terminal_closed(terminal: &str) {
    outbox(|o| o.observations.push(Outgoing::TerminalClosed(terminal.into())));
}

pub fn observed(seen: Observed, worktree_id: &str) {
    outbox(|o| o.observations.push(Outgoing::Worktree(seen, worktree_id.into())));
}

pub fn turn_ended(session_id: &str) {
    outbox(|o| o.observations.push(Outgoing::TurnEnded(session_id.into())));
}

// Once `command` is carried out.
pub fn answer(command: &Command, finished: &Finished) {
    let line = result_line(command, finished);
    let settled = matches!(finished.outcome, Outcome::Ok | Outcome::Failed | Outcome::Gone);
    outbox(|o| o.results.push((command.id.to_string(), line, settled)));
}
