// Which paired hosts haven't answered, kept in `listen`'s memory (runtime::Shared) and shared
// by every `orca::Cli::paired` it builds: each command's and the watcher's. A
// silent host is asked by nothing but `ask_again`, which `listen` alone runs, off the job
// lanes, once it is due: at each listing, and every SOON while a wake runs for it, as a
// command waits on it. One that answers is listed at once (`on_answer`), so the Durable Object
// sends what waits on it. Nothing is stored: a restart asks each host once, except a
// recipe machine machines.json marks `asleep`, which each listing takes as slept (`marked_asleep`). The label its
// workers' issues show is owed here too, and made by `offline::settle`, off the same lanes.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::trace;
use crate::sync::lock;

// A silent host is asked again at each listing, or after AGAIN if none comes first; a recipe
// machine asleep only after ASLEEP_AGAIN, as nothing but a wake brings it back; and one being
// woken every SOON, for WAKE_PROBES at most.
pub const AGAIN: Duration = Duration::from_secs(60);
pub const ASLEEP_AGAIN: Duration = Duration::from_secs(10 * 60);
pub const SOON: Duration = Duration::from_secs(10);
pub const WAKE_PROBES: Duration = Duration::from_secs(30 * 60);
pub const TICK: Duration = Duration::from_secs(5);

// A host's `tolkster config` verdict (`Health::reads`) holds this long when it read its
// config.json, and this long when it refused it, so a rejoined host is taken up soon.
pub const READS_FOR: Duration = Duration::from_secs(10 * 60);
pub const REFUSED_FOR: Duration = Duration::from_secs(60);

// A failed edit of a host's issues' labels is tried again after this.
pub const MARK_AGAIN: Duration = Duration::from_secs(60);

// One silent host: by its name where Orca said it, since when, when it is next asked, and
// until when it is asked every SOON, while a wake runs. A recipe machine asleep carries its
// paired name, as the panel shows it asleep, not faulted.
struct Silence {
    name: Option<String>,
    asleep: bool,
    since: Instant,
    next: Instant,
    soon: Option<Instant>,
}

impl Silence {
    fn new(name: Option<String>, asleep: bool, now: Instant) -> Silence {
        Silence { name, asleep, since: now, next: now + if asleep { ASLEEP_AGAIN } else { AGAIN }, soon: None }
    }
}

// What the issues of a host's workers show in Linear (offline.rs): `host offline` while it is
// silent, nothing once it is heard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mark {
    Offline,
    Clear,
}

// What each host's issues are known to show, and the edit still owed, not before the instant.
// Nothing is stored: a restart knows nothing, so the first listing to hear a host owes a clear.
#[derive(Default)]
struct Marks {
    shown: HashMap<String, Mark>,
    owed: HashMap<String, (Mark, Instant)>,
}

impl Marks {
    // A mark in flight is replaced, never kept, as its edit may land after this is owed.
    fn owe(&mut self, host: &str, mark: Mark, now: Instant) {
        match self.owed.get(host) {
            Some((owed, _)) if *owed == mark => {}
            None if self.shown.get(host) == Some(&mark) => {}
            _ => {
                self.owed.insert(host.to_string(), (mark, now));
            }
        }
    }
}

#[derive(Default)]
pub struct Health {
    silent: Mutex<HashMap<String, Silence>>,
    marks: Mutex<Marks>,
    // Whether each host's `tolkster` read its config.json when last asked, and when.
    configs: Mutex<HashMap<String, (bool, Instant)>>,
    // Run when a silent host answers: `listen` lists at once.
    answered: Mutex<Option<Box<dyn Fn() + Send>>>,
}

impl Health {
    // Taken before `marks`, wherever both are held.
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Silence>> {
        lock(&self.silent)
    }

    fn marks(&self) -> MutexGuard<'_, Marks> {
        lock(&self.marks)
    }

    pub fn silent(&self, host: &str) -> bool {
        self.lock().contains_key(host)
    }

    // The verdict on whether `host`'s `tolkster` reads its config.json, None once it is stale.
    pub fn reads(&self, host: &str, now: Instant) -> Option<bool> {
        let configs = lock(&self.configs);
        configs.get(host).filter(|(reads, at)| now.saturating_duration_since(*at) < if *reads { READS_FOR } else { REFUSED_FOR }).map(|(reads, _)| *reads)
    }

    pub fn read(&self, host: &str, reads: bool, now: Instant) {
        lock(&self.configs).insert(host.to_string(), (reads, now));
    }

    // Whether `host` is refusing its config now, for the panel.
    pub fn refused(&self, host: &str, now: Instant) -> bool {
        self.reads(host, now) == Some(false)
    }

    // `host` didn't answer at `now`: true when it had been answering, which the caller says.
    pub fn stopped(&self, host: &str, name: Option<String>, now: Instant) -> bool {
        let mut silent = self.lock();
        if silent.contains_key(host) {
            return false;
        }
        silent.insert(host.to_string(), Silence::new(name, false, now));
        self.marks().owe(host, Mark::Offline, now);
        true
    }

    // `host`, the recipe machine `name`, went to sleep at `now`: silent, asked again only at
    // ASLEEP_AGAIN, and its workers' issues aren't labelled, as it is no fault.
    pub fn slept(&self, host: &str, name: &str, now: Instant) {
        self.lock().insert(host.to_string(), Silence::new(Some(name.to_string()), true, now));
    }

    // A wake began for `host` at `now`, which a command waits on: while it is silent it is asked
    // every SOON, until WAKE_PROBES have passed.
    pub fn waking(&self, host: &str, now: Instant) {
        if let Some(s) = self.lock().get_mut(host) {
            s.soon = Some(now + WAKE_PROBES);
            s.next = s.next.min(now + SOON);
        }
    }

    // A listing began at `now`: each silent host but an asleep one is asked at the next TICK.
    pub fn listed(&self, now: Instant) {
        for s in self.lock().values_mut().filter(|s| !s.asleep) {
            s.next = s.next.min(now);
        }
    }

    // `answered` runs each time a silent host answers again.
    pub fn on_answer(&self, answered: impl Fn() + Send + 'static) {
        *lock(&self.answered) = Some(Box::new(answered));
    }

    // Each of `hosts`, the recipe machines machines.json marks `asleep` now as (name, host), as each
    // listing begins: one slept before this process began, which a restart forgot, is asleep,
    // never faulted. One already silent keeps when it is next asked.
    pub fn marked_asleep<'h>(&self, hosts: impl IntoIterator<Item = (&'h str, &'h str)>, now: Instant) {
        let mut silent = self.lock();
        for (name, host) in hosts {
            silent.entry(host.to_string()).or_insert_with(|| Silence::new(Some(name.to_string()), true, now));
        }
    }

    // `host` answered a wake, or never slept: back in rotation at once, and listed now that the
    // wake has marked it awake, though a probe may have heard it first.
    pub fn woken(&self, host: &str, now: Instant) {
        self.asked(host, true, now);
        self.list();
    }

    fn list(&self) {
        if let Some(answered) = lock(&self.answered).as_ref() {
            answered();
        }
    }

    // A listing heard `host` at `now`. Not only `asked` clears its issues: a restart forgets
    // that it was silent, and a clear that failed is owed again by the next listing.
    pub fn heard(&self, host: &str, now: Instant) {
        let silent = self.lock();
        if !silent.contains_key(host) {
            self.marks().owe(host, Mark::Clear, now);
        }
    }

    // The edits owed at `now`: each host and what its issues are to show.
    pub fn owed(&self, now: Instant) -> Vec<(String, Mark)> {
        self.marks().owed.iter().filter(|(_, (_, at))| *at <= now).map(|(host, (mark, _))| (host.clone(), *mark)).collect()
    }

    // `paired` and every host marked that isn't, so far as its issues still show `host offline`
    // or an edit is owed: an unpaired host's workers are released by the listing that leaves it
    // out, and its issues' labels are cleared first, while their records name them. The clear
    // is owed here, and the host is left out once it is made.
    pub fn withholding(&self, mut paired: Vec<String>, now: Instant) -> Vec<String> {
        let mut marks = self.marks();
        let mut unpaired: Vec<String> = marks.shown.iter().filter(|(_, mark)| **mark == Mark::Offline).map(|(host, _)| host).chain(marks.owed.keys()).filter(|host| !paired.contains(host)).cloned().collect();
        // Sorted, so that the same hosts held make the same listing.
        unpaired.sort();
        unpaired.dedup();
        for host in unpaired {
            marks.owe(&host, Mark::Clear, now);
            if marks.owed.contains_key(&host) && !paired.contains(&host) {
                paired.push(host);
            }
        }
        paired
    }

    // The edit of `host`'s issues to `mark` is made. One owed since, the other way, stays owed.
    pub fn marked(&self, host: &str, mark: Mark) {
        let mut marks = self.marks();
        marks.shown.insert(host.to_string(), mark);
        if marks.owed.get(host).is_some_and(|(owed, _)| *owed == mark) {
            marks.owed.remove(host);
        }
    }

    // It failed: tried again after MARK_AGAIN.
    pub fn unmarked(&self, host: &str, mark: Mark, now: Instant) {
        if let Some(owed) = self.marks().owed.get_mut(host).filter(|(owed, _)| *owed == mark) {
            owed.1 = now + MARK_AGAIN;
        }
    }

    // Each silent host, by its ID and name, whether it is asleep, since when, and when it is next asked.
    pub fn silences(&self) -> Vec<(String, Option<String>, bool, Instant, Instant)> {
        self.lock().iter().map(|(host, s)| (host.clone(), s.name.clone(), s.asleep, s.since, s.next)).collect()
    }

    // When `host` is next asked, None while it answers.
    #[cfg(test)]
    pub fn next(&self, host: &str) -> Option<Instant> {
        self.lock().get(host).map(|s| s.next)
    }

    // Each silent host due at `now`.
    fn due(&self, now: Instant) -> Vec<String> {
        self.lock().iter().filter(|(_, s)| s.next <= now).map(|(host, _)| host.clone()).collect()
    }

    // A host no longer paired is forgotten, as nothing is sent to it.
    fn keep(&self, paired: &[String]) {
        self.lock().retain(|host, _| paired.contains(host));
    }

    // `host` answered: back in rotation, said once, and listed, unless a wake runs for it, which
    // lists once it has marked the machine awake (`woken`). Still silent: asked again after SOON
    // while a wake runs, else at the next listing.
    fn asked(&self, host: &str, answered: bool, now: Instant) {
        let mut silent = self.lock();
        if answered {
            if let Some(was) = silent.remove(host) {
                self.marks().owe(host, Mark::Clear, now);
                drop(silent);
                trace::event(trace::Host::Answering { name: was.name, id: host.to_string(), after: now.saturating_duration_since(was.since) });
                if !was.soon.is_some_and(|until| until > now) {
                    self.list();
                }
            }
        } else if let Some(s) = silent.get_mut(host) {
            s.soon = s.soon.filter(|until| *until > now);
            s.next = now + if s.soon.is_some() { SOON } else if s.asleep { ASLEEP_AGAIN } else { AGAIN };
        }
    }
}

// One pass of `listen`'s background asks, every TICK: each silent host still `paired` that is
// due is asked once (`answers`), without the lock held, so no command waits on
// it. The paired hosts are read only when one is due.
pub fn ask_again(health: &Health, paired: impl FnOnce() -> Option<Vec<String>>, mut answers: impl FnMut(&str) -> bool, now: impl Fn() -> Instant) {
    let due = health.due(now());
    if due.is_empty() {
        return;
    }
    let Some(paired) = paired() else { return };
    health.keep(&paired);
    for host in due.iter().filter(|host| paired.contains(host)) {
        let answered = answers(host);
        health.asked(host, answered, now());
    }
}

#[cfg(test)]
mod tests;
