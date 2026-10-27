// The scheduler behind `tolkster listen` (listen.rs): it carries out the Durable Object's
// commands and runs `listen`'s own jobs (a base tip, the watcher's findings, an update
// pass), and decides when and in which lane each runs (`Lane`): one job at a time per
// lane, an issue's lanes up to ISSUE_LANES at once, beside the one lane for what isn't
// one issue's. An issue's commands come one at a time from the Durable Object, and its
// lane runs them one at a time: what keeps two of them apart on one issue.
//
// One of `listen`'s own jobs runs as it comes, with no settle: one that comes again, as
// another line, while it is queued or running runs once more after, as its newest line, so
// a burst runs at most twice and the last run reads the newest state.
//
// Nothing polls Linear or GitHub. `--print` never watches.

use std::collections::{HashMap, VecDeque};
use std::fmt::Display;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::Error;
use crate::gateway;
use crate::orca;
use crate::panel;
use crate::plain::{Id, IssueKey, Sha};
use crate::runtime::Shared;
use crate::timing::Summary;
use crate::machine::{BaseTip, Command, listing_line, observation_line, terminal_closed_line, turn_ended_line};
use crate::sync::lock;
use crate::trace::{self, Call, Ended, Kind, STUCK_AFTER, Step, Stuck, Update as Said, Work};
use crate::update::{Restart, Update};
use crate::watch::{Agents, Missed, Seen, Snapshot, WATCH_EVERY, Watcher};

mod lanes;

pub use lanes::{answer, observed, terminal_closed, turn_ended, watch_terminal};
use lanes::{Acting, LANE, LANE_THREADS, Lane, Lanes, Outbox, Outgoing, Owned, Queued, URGENT};

#[derive(Clone, Debug, PartialEq)]
pub enum Job {
    // A repo's base tip, which the relay sends on connect and on each push: `after` is its SHA.
    Base { repo: String, reference: String, after: Sha },
    Closed { worktree_id: String },
    Ended { terminal: String },
    Update,
    // From the Durable Object's machines (docs/state-machine.md, Commands): carried out and
    // answered with its result.
    Command(Command),
}

impl Job {
    fn lane(&self) -> Lane {
        match self {
            // One command in flight per issue, and a class 1 event's beside it, in a lane of
            // its own, so a stop never waits behind the command it overtakes.
            Job::Command(command) if command.urgent => Lane::Issue(format!("{URGENT}{}", command.identifier.to_ascii_uppercase())),
            Job::Command(command) => Lane::Issue(command.identifier.to_ascii_uppercase()),
            Job::Base { .. } | Job::Closed { .. } | Job::Ended { .. } | Job::Update => Lane::Shared,
        }
    }

    // Its issue, for its lines to lead with, and its kind.
    fn named(&self) -> (Option<IssueKey>, Kind) {
        match self {
            Job::Command(command) => (Some(command.identifier.clone()), Kind::Command(command.kind.name())),
            Job::Base { repo, .. } => (None, Kind::Pushed(repo.clone())),
            Job::Closed { .. } => (None, Kind::WorktreeClosed),
            Job::Ended { .. } => (None, Kind::TerminalClosed),
            Job::Update => (None, Kind::UpdateCheck),
        }
    }

    // A class 1 event's command, the owner's stop above all, taken before the rest.
    fn owners(&self) -> bool {
        matches!(self, Job::Command(command) if command.urgent)
    }

    // The gateway's lane for the job's calls. Every command of the machine's is urgent: one
    // held keeps its issue's place for three timeouts, a reply to the owner queued behind it,
    // and then ends lost. A terminal's end is too, as it starts the next triage batch and
    // is acted on once, held or not. `listen`'s other jobs wait out a low budget here, as Held.
    fn calls(&self) -> gateway::Lane {
        match self {
            Job::Command(_) | Job::Ended { .. } => gateway::Lane::Urgent,
            _ => gateway::Lane::Normal,
        }
    }
}

// Held must wait, for a spent budget, or a low one in a normal lane, or for a base tip
// `origin` doesn't hold yet: run again here after the wait.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Acted {
    Done,
    Held(Duration),
}

impl From<()> for Acted {
    fn from((): ()) -> Acted {
        Acted::Done
    }
}

// A build still making progress holds the restart this long (`Scheduler::stuck`).
pub const BUILD_HOLDS: Duration = Duration::from_secs(3 * 60);

// A command's ID in the scheduler's maps, apart from `listen`'s own jobs'.
const COMMAND: &str = "command:";
const DRAIN_POLL: Duration = Duration::from_millis(200);
// The results last sent, by command: a resend of one already carried out is answered
// with its result again, not carried out again.
const ANSWERED_KEEP: usize = 64;
// A base tip `listen` couldn't pull yet is tried again after this: `origin` may not hold it.
pub const BASE_AGAIN: Duration = Duration::from_secs(60);

struct Waiting {
    due: Instant,
    line: String,
    job: Job,
}

// How a connection to the Durable Object ends, for `listen` to act on: what the scheduler's
// step returns, and what a connection returns.
pub enum End {
    Retry(Ended),
    // Retrying can't help.
    Stop(Error),
    Restart(Restart),
    // Another `listen` took over: this one drains and exits, never reconnecting.
    Superseded,
    // `q` on the panel, once drained: what it held ran, and its results went out.
    Quit,
}

impl End {
    pub fn stuck(what: Stuck, after: Duration) -> End {
        End::Restart(Restart::Stuck { what, minutes: after.as_secs() / 60 })
    }
}

// What queues returns false once the lanes have closed.
pub struct Scheduler {
    lanes: Owned,
    done: Receiver<(String, Acted, Job)>,
    // Queued or running, with the line it came as: the same line again isn't queued again.
    in_flight: HashMap<String, String>,
    // Came again, as a different line, while in flight (a newer base tip): run once more,
    // as the newest line.
    again: HashMap<String, (String, Job)>,
    // Must wait (`Acted::Held`): run again once due. A restart drops them.
    held: HashMap<String, Waiting>,
    // One per lane's thread. Its `id` is the latest job it began, so it names the one in
    // `since` while that is set.
    acting: Vec<Acting>,
    watcher: Option<Watcher>,
    update: Option<Update>,
    // What `listen` keeps (runtime::Shared), for the recipe machines under way.
    shared: Arc<Shared>,
    // The config report (machine.rs, `config_line`), as `listen` started and then as each
    // listing read the settings, told to each new connection first and again whenever it
    // changes (`told`), as the Durable Object's slots count by it.
    config: Option<String>,
    told: Option<String>,
    // Counts the connections: a command's result goes back only on the one its newest
    // copy came on, as the Durable Object counts only the newest's.
    connection: u64,
    // The commands queued or being carried out, by ID, with the connection their newest
    // copy came on.
    commands: HashMap<String, u64>,
    // Results to send, with the connection each is for, and those sent (ANSWERED_KEEP).
    results: Vec<(String, u64, String)>,
    answered: VecDeque<(String, String)>,
    // The last observation's number on this connection.
    sequence: u64,
    // The watcher's newest listing, and the last one sent on this connection (`send_listing`).
    listing: Option<Snapshot>,
    listing_sent: Option<Snapshot>,
    // A command resent after its last run answered and before that run ended, with the
    // connection it came on: run again once that run has ended.
    resent: HashMap<String, (u64, Command)>,
    // The relay closed the connection for a newer `listen`'s: drained like a restart, but
    // for an exit (`drain_superseded`).
    superseded: bool,
    // `q` on the panel: drained like a restart, on the connection, for an exit (`drained`).
    quitting: bool,
}

fn due(waiting: &mut HashMap<String, Waiting>, now: Instant) -> Vec<(String, Waiting)> {
    let ids: Vec<String> = waiting.iter().filter(|(_, s)| s.due <= now).map(|(id, _)| id.clone()).collect();
    ids.into_iter().filter_map(|id| waiting.remove(&id).map(|s| (id, s))).collect()
}

impl Scheduler {
    // `act` gets each job, and a function to call as it goes, to show it's still making
    // progress: the watchdog counts from then. One whose `act` panics is logged and done
    // with. It runs on every lane's thread at once.
    pub fn spawn<R: Into<Acted>>(shared: Arc<Shared>, act: impl Fn(&str, &Job, &dyn Fn()) -> R + Send + Sync + 'static) -> Scheduler {
        let (finished, done) = mpsc::channel();
        let (lanes, act) = (Arc::new(Lanes::default()), Arc::new(act));
        let acting: Vec<Acting> = (0..=LANE_THREADS).map(|_| Acting::default()).collect();
        for (n, mine) in acting.iter().enumerate() {
            let (lanes, act, finished, mine) = (Arc::clone(&lanes), Arc::clone(&act), finished.clone(), mine.clone());
            // The first thread is the shared lane's.
            let shared = n == 0;
            std::thread::spawn(move || {
                LANE.with(|l| *l.borrow_mut() = Some((Arc::clone(&lanes), mine.clone(), shared)));
                while let Some(Queued { id, lane, job, .. }) = lanes.take(shared) {
                    id.clone_into(&mut lock(&mine.id));
                    let (issue, kind) = job.named();
                    let started = Instant::now();
                    panel::job(n, Some(panel::Running { issue: issue.as_ref().map(ToString::to_string), label: kind.label(), lane: lane.shown(), since: started }));
                    let _acting = trace::acting_as(&id, issue.as_deref(), kind);
                    mine.since.set(Some(started));
                    // A panic ends the job, not the thread: a thread that ended would leave its
                    // lanes unserved, and a command's resend would come into the same panic.
                    let acting = || gateway::in_lane(job.calls(), || act(&id, &job, &|| mine.since.set(Some(Instant::now()))).into());
                    let acted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(acting)).unwrap_or_else(|_| {
                        trace::event(trace::Outcome::Panicked);
                        Acted::Done
                    });
                    trace::took(started.elapsed());
                    if orca::take_listing_stale() {
                        lanes.list_now();
                    }
                    mine.since.set(None);
                    panel::job(n, None);
                    lanes.done(&lane, shared);
                    if finished.send((id, acted, job)).is_err() {
                        return;
                    }
                    if let Some(wake) = lock(&lanes.wake).as_ref() {
                        wake();
                    }
                }
            });
        }
        let (in_flight, again) = (HashMap::new(), HashMap::new());
        Scheduler { lanes: Owned(lanes), done, in_flight, again, held: HashMap::new(), acting, watcher: None, update: None, shared, config: None, told: None, connection: 0, commands: HashMap::new(), results: Vec::new(), answered: VecDeque::new(), sequence: 0, listing: None, listing_sent: None, resent: HashMap::new(), superseded: false, quitting: false }
    }

    pub fn updating(self, update: Update) -> Scheduler {
        Scheduler { update: Some(update), ..self }
    }

    pub fn telling(self, config: String) -> Scheduler {
        Scheduler { config: Some(config), ..self }
    }

    // A new build is in place: no command is taken from the relay. What was taken, and the
    // watcher's jobs, still run.
    fn draining(&self) -> bool {
        self.superseded || self.quitting || self.update.as_ref().is_some_and(Update::draining)
    }

    // Takes no new command and starts no update; what it holds runs, and its results go out.
    pub fn quit(&mut self) {
        self.quitting = true;
    }

    // Quitting, once nothing is queued or running and no build runs.
    pub fn drained(&self) -> bool {
        self.quitting && self.idle() && !self.update.as_ref().is_some_and(Update::builds)
    }

    // Another `listen` took over: takes no new command and starts no update, and returns
    // once what it held, the watcher's jobs and a running build are done, or the run is stuck.
    // Results aren't sent: the relay sent each command to the newer `listen` too.
    pub fn drain_superseded(&mut self) -> Result<(), Error> {
        self.superseded = true;
        loop {
            match self.step(|_| Ok::<(), Error>(())) {
                Some(End::Stop(error)) => return Err(error),
                Some(_) => return Ok(()),
                None if self.idle() && !self.update.as_ref().is_some_and(Update::builds) => return Ok(()),
                None => std::thread::sleep(DRAIN_POLL),
            }
        }
    }

    // The restart onto a new build, once nothing is queued or running. The result of the
    // command just finished goes out first.
    pub fn restart_due(&self) -> Option<Restart> {
        if !self.idle() || self.update.as_ref().is_some_and(Update::builds) {
            return None;
        }
        self.update.as_ref()?.take_restart()
    }

    fn update_put_off(&mut self) -> bool {
        if self.superseded || self.quitting || !self.update.as_ref().is_some_and(Update::take_put_off) {
            return true;
        }
        self.queue_local("update".into(), Job::Update)
    }

    // `wake` is called on a lane's thread each time a job ends, after its result is in the
    // outbox and its end on `done`, replacing the last connection's.
    // Has the watcher list at once from off the lanes, as a silent host answered (health.rs).
    pub fn lister(&self) -> impl Fn() + Send + 'static {
        let lanes = Arc::downgrade(&self.lanes.0);
        move || {
            if let Some(lanes) = lanes.upgrade() {
                lanes.list_now();
            }
        }
    }

    pub fn waking(&self, wake: impl Fn() + Send + 'static) {
        *lock(&self.lanes.0.wake) = Some(Box::new(wake));
    }

    // A new connection: what it carries supersedes the last one's.
    pub fn connected(&mut self) {
        self.connection += 1;
        self.told = None;
        self.sequence = 0;
        self.listing_sent = None;
    }

    // A command from the Durable Object, which sends it again, with the same ID, until it
    // has its result: one queued or being carried out is answered on this connection once
    // done, and one already done is answered again, so none is carried out twice at once.
    // Draining leaves it for the resend after the restart.
    pub fn take_command(&mut self, command: Command) -> bool {
        let id = command.id.to_string();
        if let Some(on) = self.commands.get_mut(&id) {
            *on = self.connection;
            return true;
        }
        if let Some((_, line)) = self.answered.iter().find(|(done, _)| *done == id) {
            self.results.push((id.clone(), self.connection, line.clone()));
            return true;
        }
        // Its last run answered, unsettled, but hasn't ended yet: run again once it has
        // (`collect`), so two runs never overlap and the resend isn't lost.
        if self.in_flight.contains_key(&format!("{COMMAND}{id}")) {
            self.resent.insert(id, (self.connection, command));
            return true;
        }
        if self.draining() {
            trace::event(Said::LeftForResend { command: id.to_string() });
            return true;
        }
        self.commands.insert(id.clone(), self.connection);
        let key = format!("{COMMAND}{id}");
        self.send(key.clone(), key, Job::Command(command))
    }

    // `list` names the worktrees an agent runs in and those whose agents sit idle;
    // `missed` what closed before the first listing, from the records; `wait` blocks until
    // an agent's terminal exits (see `Watcher`).
    pub fn watching(self, list: impl FnMut() -> Result<Agents, Error> + Send + 'static, missed: impl FnMut(&Agents) -> Result<Missed, Error> + Send + 'static, wait: impl Fn(&str) -> Result<bool, Error> + Send + Sync + 'static) -> Scheduler {
        let watcher = Watcher::spawn(list, missed, wait, WATCH_EVERY);
        *lock(&self.lanes.0.nudge) = Some(watcher.nudge.clone());
        Scheduler { watcher: Some(watcher), ..self }
    }

    // The frame shows a worker the newest listing lacks: list again now, not at the backstop.
    pub fn list_now(&self) {
        self.lanes.0.list_now();
    }

    fn watch_agents(&mut self) -> bool {
        let Some(watcher) = &self.watcher else { return true };
        let seen: Vec<Seen> = watcher.seen.try_iter().collect();
        if let Some(newest) = watcher.snapshots.try_iter().last() {
            if self.config.is_some() {
                self.config = Some(newest.config.clone());
            }
            self.listing = Some(newest);
        }
        let (mut closed, mut ended) = (Vec::new(), Vec::new());
        for seen in seen {
            match seen {
                Seen::Closed(worktree_id) => closed.push(worktree_id),
                Seen::Ended(terminal) => {
                    panel::ended(&terminal);
                    ended.push(terminal);
                }
            }
        }
        for worktree_id in closed {
            if !self.queue_local(format!("closed:{worktree_id}"), Job::Closed { worktree_id }) {
                return false;
            }
        }
        for terminal in ended {
            if !self.queue_local(format!("ended:{terminal}"), Job::Ended { terminal }) {
                return false;
            }
        }
        true
    }

    // A session's timing as the relay forwards it (listen.rs), kept in the timing file on a
    // thread of its own, as it reads the whole file. The setting is read as it is now, so a
    // report from a worker launched before it was turned off is dropped.
    pub fn keep_timing(&self, issue: Id, summary: Summary) {
        let shared = Arc::clone(&self.shared);
        std::thread::spawn(move || {
            let _ = crate::config::files().and_then(|files| shared.settings(&files));
            shared.timing.session(&issue, &summary)
        });
    }

    // A repo's base tip, which nothing answers. Its line carries the commit, so a newer tip
    // coming while the one before runs, or is held, runs after it, or now.
    pub fn take_base(&mut self, tip: BaseTip) -> bool {
        let BaseTip { repo, reference, after } = tip;
        let id = format!("base:{}", repo.to_ascii_lowercase());
        let line = format!("{id}@{after}");
        trace::event(Call::Inbound { job: line.clone() });
        self.queue(id, line, Job::Base { repo, reference, after })
    }

    fn queue_local(&mut self, id: String, job: Job) -> bool {
        trace::event(Call::Inbound { job: id.clone() });
        self.queue(id.clone(), id, job)
    }

    // What has made no progress for `after`, for the restart's line: the build, the job by
    // its ID, or Orca's listing. A build still making progress holds the restart up to
    // BUILD_HOLDS more, for it to end: the run that starts builds HEAD again. A swap under
    // way always holds it, as the installed path may be empty until it ends.
    fn stuck(&self, after: Duration) -> Option<Stuck> {
        if self.update.as_ref().is_some_and(Update::placing) {
            return None;
        }
        let building = self.update.as_ref().is_some_and(Update::builds);
        if self.update.as_ref().is_some_and(|u| u.stuck(after)) {
            return Some(Stuck::Build);
        }
        let after = if building { after + BUILD_HOLDS } else { after };
        for acting in self.acting.iter().filter(|a| a.since.older_than(after)) {
            let id = lock(&acting.id).clone();
            // The job may have ended, and the next begun, between the two reads.
            if acting.since.older_than(after) {
                return Some(Stuck::Job(id));
            }
        }
        self.watcher.as_ref().is_some_and(|w| w.since.older_than(after)).then_some(Stuck::Listing)
    }

    // One already in flight is queued again after, if it came as a different line: the
    // same line again isn't. A base tip isn't while quitting: the next run takes it on connect.
    fn queue(&mut self, id: String, line: String, job: Job) -> bool {
        if self.quitting && matches!(job, Job::Base { .. }) {
            return true;
        }
        if let Some(current) = self.in_flight.get(&id) {
            if *current != line {
                self.again.insert(id, (line, job));
            }
            return true;
        }
        // The same line as one held waits out its hold; a newer one runs now.
        if self.held.get(&id).is_some_and(|h| h.line == line) {
            return true;
        }
        self.held.remove(&id);
        self.send(id, line, job)
    }

    fn send(&mut self, id: String, line: String, job: Job) -> bool {
        self.in_flight.insert(id.clone(), line);
        let (lane, owners) = (job.lane(), job.owners());
        self.lanes.0.put(Queued { id, lane, owners, job })
    }

    // Draining drops what is held: a base tip comes again on the next connection.
    fn release_held(&mut self) -> bool {
        let now = Instant::now();
        if self.draining() {
            for id in self.held.drain().map(|(id, _)| id) {
                trace::event(Said::Dropped { job: id });
            }
        }
        for (id, Waiting { line, job, .. }) in due(&mut self.held, now) {
            if !self.send(id, line, job) {
                return false;
            }
        }
        true
    }

    // The jobs ended since last asked: one that came again meanwhile runs again, and one
    // held runs again after its wait.
    fn collect(&mut self) -> bool {
        loop {
            match self.done.try_recv() {
                // A command's result came through `answer`, and goes out in `step`.
                // One that ended with no result, as on a panic, is carried out again on its
                // resend rather than waited on for good.
                Ok((id, _, _)) if id.starts_with(COMMAND) => {
                    self.in_flight.remove(&id);
                    let command_id = &id[COMMAND.len()..];
                    if !lock(&self.lanes.0.outbox).results.iter().any(|(done, _, _)| done == command_id) {
                        self.commands.remove(command_id);
                    }
                    if let Some((on, command)) = self.resent.remove(command_id) {
                        self.commands.insert(command_id.to_string(), on);
                        if !self.send(id.clone(), id, Job::Command(command)) {
                            return false;
                        }
                    }
                }
                Ok((id, acted, job)) => {
                    let line = self.in_flight.remove(&id).unwrap_or_default();
                    if let Some((line, job)) = self.again.remove(&id) {
                        if !self.queue(id, line, job) {
                            return false;
                        }
                    } else if let Acted::Held(wait) = acted {
                        let (issue, kind) = job.named();
                        trace::event(Work::Retrying { issue, kind, job: id.clone(), wait });
                        self.held.insert(id, Waiting { due: Instant::now() + wait, line, job });
                    }
                }
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
    }

    // One turn of the connection's loop, in the order it depends on: what finished is
    // collected, what held is due is queued, the watcher's findings and the put-off pass are
    // queued, a stuck run ends, then the slots, the listing, the observations and the
    // results go out by `send`, and only then a restart is taken, so a drained restart
    // never leaves a result unsent. Returns why the connection must end.
    pub fn step<E: Display>(&mut self, mut send: impl FnMut(String) -> Result<(), E>) -> Option<End> {
        if !self.collect() || !self.release_held() || !self.watch_agents() || !self.update_put_off() {
            return Some(End::Stop(Error::Failed("the scheduler stopped".into())));
        }
        panel::held(self.held.values().map(|w| {
            let (issue, kind) = w.job.named();
            panel::Held { issue: issue.map(String::from), label: kind.label(), due: w.due }
        }).collect());
        if let Some(what) = self.stuck(STUCK_AFTER) {
            return Some(End::stuck(what, STUCK_AFTER));
        }
        if let Some(config) = self.config.clone().filter(|line| self.told.as_ref() != Some(line)) {
            if let Err(error) = send(config.clone()) {
                return Some(End::Retry(Ended::failed(Step::Config, error)));
            }
            self.told = Some(config);
        }
        if let Some(end) = self.send_results(&mut send) {
            return Some(end);
        }
        self.restart_due().map(End::Restart)
    }

    // Each result goes back on the connection its command's newest copy came on, and is
    // dropped if that has gone: the Durable Object sends the command again to the newest,
    // which `answered` answers.
    fn send_results<E: Display>(&mut self, send: &mut impl FnMut(String) -> Result<(), E>) -> Option<End> {
        // Before the outbox is taken, so a failed send leaves it whole for the next connection.
        if let Some(end) = self.send_listing(send) {
            return Some(end);
        }
        let Outbox { results, observations } = std::mem::take(&mut *lock(&self.lanes.0.outbox));
        for (id, line, settled) in results {
            let on = self.commands.remove(&id).unwrap_or(self.connection);
            // A retryable or held one is carried out again on its resend.
            if settled {
                if self.answered.len() == ANSWERED_KEEP {
                    self.answered.pop_front();
                }
                self.answered.push_back((id.clone(), line.clone()));
            }
            self.results.push((id, on, line));
        }
        // One not sent waits for the next connection (`connected`).
        let mut observations = VecDeque::from(observations);
        while let Some(seen) = observations.pop_front() {
            let line = match &seen {
                Outgoing::Worktree(observed, worktree_id) => observation_line(*observed, worktree_id, self.sequence + 1),
                Outgoing::TurnEnded(session_id) => turn_ended_line(session_id, self.sequence + 1),
                Outgoing::TerminalClosed(terminal) => terminal_closed_line(terminal, self.sequence + 1),
            };
            if let Err(error) = send(line) {
                // Kept for the next connection, ahead of anything queued since.
                let mut outbox = lock(&self.lanes.0.outbox);
                let since = std::mem::take(&mut outbox.observations);
                outbox.observations = std::iter::once(seen).chain(observations).chain(since).collect();
                return Some(End::Retry(Ended::failed(Step::Observation, error)));
            }
            self.sequence += 1;
        }
        while let Some((id, on, line)) = self.results.first().cloned() {
            if on == self.connection {
                if let Err(error) = send(line) {
                    return Some(End::Retry(Ended::failed(Step::Result, error)));
                }
            } else {
                trace::event(Work::ResultHeld { command: id.clone() });
            }
            self.results.remove(0);
        }
        None
    }

    // The newest listing, first on a new connection, then whenever it shows anything else,
    // and at least once each WATCH_EVERY all the same: the Durable Object drops what a hook
    // reported since a listing began, so the next listing is what undoes a hook's state its
    // agent left without another hook, though it shows what the last one did.
    fn send_listing<E: Display>(&mut self, send: &mut impl FnMut(String) -> Result<(), E>) -> Option<End> {
        let newest = self.listing.as_ref()?;
        if self.listing_sent.as_ref().is_some_and(|sent| sent.same(newest) && newest.began.saturating_duration_since(sent.began) < WATCH_EVERY) {
            return None;
        }
        if let Err(error) = send(listing_line(newest, u64::try_from(newest.began.elapsed().as_millis()).unwrap_or(u64::MAX))) {
            return Some(End::Retry(Ended::failed(Step::Listing, error)));
        }
        self.listing_sent = Some(newest.clone());
        None
    }

    // A machine being made, off the lanes, holds a restart or a quit as a job does.
    fn idle(&self) -> bool {
        self.in_flight.is_empty() && !self.shared.recipes.making()
    }
}


#[cfg(test)]
mod tests;
