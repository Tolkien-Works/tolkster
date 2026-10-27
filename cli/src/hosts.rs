// What `listen` tells the Durable Object of its hosts, which decides how many workers run at
// once and where (docs/dispatcher.md, Who waits). A host is a paired Orca server (orca::paired), and its cap
// is its `slots` entry, 0 without one, or a recipe machine's its repo's `slotsEach`, 0 while
// it is asleep; this machine's is `slots`' `local`. A paired host without an admitted
// credential in the Durable Object has a cap of 0: Orca lists a host as soon as it pairs, a
// credential is admitted only once `host add` passed every step, and `host rm` or a sleep
// revokes it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};

use serde::Serialize;

use crate::config::{Config, DEFAULT_DESTROY_AFTER, DEFAULT_IDLE_MINUTES, Machine};
use crate::error::Error;
use crate::orca::{Host, Pane, Worktree, plain_project, project_id};
use crate::state::WorkerRecord;
use crate::sync::{lock, read, write};
use crate::time::instant;

// How long a worker launched counts as running before its agent is listed: its setup on its
// host runs first, which Orca lists as no agent.
const SETTING_UP_MS: i64 = 5 * 60 * 1000;

// How `listen` paces its hosts (runtime::Shared): which a sleep stopped work on, which its
// commands act on, and which hold an admitted credential.
#[derive(Default)]
pub struct Pacing {
    // The machines a sleep stopped work on, by the host ID they were paired as: a command may
    // hold settings read before the sleep marked the machine asleep. A wake keeps the ID and
    // takes it out again; a machine made again under the name pairs as another.
    napping: Mutex<BTreeSet<String>>,
    // A worker's command marking its machine acted on (`acting`), a start's the host its slot
    // was granted on, and a sleep's check that the machine rests (`stop_acting`), never at once:
    // a command marks it before it acts, or finds it napping, so no worker starts on a machine a
    // sleep found resting. Only `listen` acts and sleeps, so a lock in its context is all it
    // takes.
    acting: RwLock<()>,
    // For each host, the commands acting there and when the last one ended, so an agent it
    // launched that Orca doesn't list yet keeps the machine awake.
    acted: Mutex<BTreeMap<String, (usize, i64)>>,
    // The hosts holding an admitted credential, as `listen` last read them
    // (State::enrolled_hosts): a read that fails keeps the last, so a blip in the Durable Object
    // takes no host's work away. Empty only until `start_enrolled` reads them, before any
    // command runs.
    enrolled: Mutex<Vec<String>>,
}

// A worker's command acting on its hosts until dropped, a panic's unwinding included.
pub struct ActingOn<'p>(&'p Pacing, Vec<String>);

impl Drop for ActingOn<'_> {
    fn drop(&mut self) {
        let mut acted = lock(&self.0.acted);
        let now = crate::time::now_ms();
        for host in &self.1 {
            if let Some(on) = acted.get_mut(host) {
                *on = (on.0.saturating_sub(1), now);
            }
        }
    }
}

impl Pacing {
    // Held by a worker's command while it checks and marks its machine (command::carry).
    pub fn acting(&self) -> RwLockReadGuard<'_, ()> {
        read(&self.acting)
    }

    pub fn stop_acting(&self) -> RwLockWriteGuard<'_, ()> {
        write(&self.acting)
    }

    pub fn act_on(&self, hosts: &[String]) -> ActingOn<'_> {
        let mut acted = lock(&self.acted);
        hosts.iter().for_each(|host| acted.entry(host.clone()).or_insert((0, 0)).0 += 1);
        ActingOn(self, hosts.to_vec())
    }

    // Whether a worker's command acts on `host`, or ended within SETTING_UP_MS of `now_ms`.
    pub fn acted_lately(&self, host: &str, now_ms: i64) -> bool {
        lock(&self.acted).get(host).is_some_and(|(acting, at)| *acting > 0 || now_ms - at < SETTING_UP_MS)
    }

    pub fn nap(&self, host: &str) {
        lock(&self.napping).insert(host.to_string());
    }

    pub fn placing_again(&self, host: &str) {
        lock(&self.napping).remove(host);
    }

    pub fn napping(&self, host: &str) -> bool {
        lock(&self.napping).contains(host)
    }

    // The first read, before `listen` connects or runs a command: tried `tries` times with
    // `pause` between, and an error if none answers, never an empty list that declines every host.
    pub fn start_enrolled(&self, mut read: impl FnMut() -> Result<Vec<String>, Error>, tries: u32, pause: impl Fn()) -> Result<(), Error> {
        let mut last = Error::Failed("the admitted hosts weren't read".into());
        for attempt in 0..tries {
            if attempt > 0 {
                pause();
            }
            match read() {
                Ok(hosts) => {
                    self.read_enrolled(Ok(hosts));
                    return Ok(());
                }
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    pub fn enrolled(&self) -> Vec<String> {
        lock(&self.enrolled).clone()
    }

    pub fn read_enrolled(&self, read: Result<Vec<String>, Error>) {
        if let Ok(hosts) = read {
            *lock(&self.enrolled) = hosts;
        }
    }

    // A machine `listen` made was admitted as `host` (recipe::make): the next start may place on
    // it before a listing reads it.
    pub fn admitted(&self, host: &str) {
        let mut enrolled = lock(&self.enrolled);
        if !enrolled.iter().any(|id| id == host) {
            enrolled.push(host.to_string());
        }
    }
}

pub fn cap(config: &Config, host: &Host) -> i64 {
    let name = host.name.trim();
    if !config.enrolled.contains(&host.id) {
        return 0;
    }
    match config.machines.get(name) {
        Some(made) if made.asleep => 0,
        Some(made) => config.repos.entry(&made.repo).and_then(|entry| entry.recipe.as_ref()).map_or(0, |recipe| recipe.slots_each),
        None => config.slots.get(name).copied().unwrap_or(0),
    }
}

// What each host adds to the Durable Object's slots, this machine (None) first: its cap, or the
// workers it holds when more, since their slots are held whatever the cap says, so a host over
// its cap uses up only its own. A paired host, silent or with no repo set up (`holding`), is
// not placed on and adds the workers it holds as last listed.
pub fn shares<'a>(config: &Config, local_held: i64, ready: &'a [(Host, i64)], holding: &'a [(Host, i64)]) -> Vec<(Option<&'a Host>, i64)> {
    let local = (None, config.local_slots().max(local_held));
    let ready = ready.iter().map(|(host, held)| (Some(host), cap(config, host).max(*held)));
    std::iter::once(local).chain(ready).chain(holding.iter().map(|(host, held)| (Some(host), *held))).collect()
}

// The most workers the relay takes as running at once (src/worker/machine.ts, MAX_SLOTS).
pub const MAX_SLOTS: i64 = 100;

// The Durable Object's slots, at most what it takes.
pub fn total(config: &Config, local_held: i64, ready: &[(Host, i64)], holding: &[(Host, i64)]) -> i64 {
    shares(config, local_held, ready, holding).into_iter().map(|(_, share)| share).fold(0, i64::saturating_add).min(MAX_SLOTS)
}

// A paired host as a listing tells the Durable Object of it (src/worker/machine/fleet.ts): its
// cap, whether its credential is admitted, whether it answered, the projects ready there
// (`Orca::ready_projects`, none for a silent one), and the workers it holds. The Durable
// Object sums them as `total` does: `ready` is what `shares` reads as a host's room.
// `admitted` is as `listen` last read it, for display: the Durable Object's own records decide.
// A name of no plain shape (`plain_name`) goes as the host's ID.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Heard {
    pub id: String,
    pub name: String,
    pub cap: i64,
    pub admitted: bool,
    pub answering: bool,
    pub ready: Vec<String>,
    pub held: i64,
}

// This machine's workers, then every paired host: those that answered, with their ready
// projects, then the silent ones; the project (`project_id`) of each of this machine's repos,
// by its Orca ID, as its origin says, by which the Durable Object finds a host ready for an
// issue's repo; and of those, each whose `repos` entry has a recipe, by that entry's name, by
// which its fleet finds the recipe.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Fleet {
    pub held: i64,
    pub hosts: Vec<Heard>,
    pub projects: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub recipes: BTreeMap<String, String>,
}

// As the Durable Object takes a name (src/worker/machine/fleet.ts, NAME).
pub fn plain_name(name: &str) -> bool {
    (1..=100).contains(&name.chars().count()) && !name.chars().any(char::is_control)
}

// As it takes a repo's ID (REPO_ID).
fn plain_repo(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

// `origins`: this machine's repos' `owner/name` origins, by Orca ID, and `names` their display
// names; one of no plain shape is left out.
pub fn fleet(config: &Config, pacing: &Pacing, origins: &BTreeMap<String, String>, names: &BTreeMap<String, String>, local_held: i64, answering: &[(Host, Vec<String>, i64)], silent: &[(Host, i64)]) -> Fleet {
    let name = |host: &Host| Some(host.name.trim()).filter(|name| plain_name(name)).unwrap_or(&host.id).to_string();
    // A machine a sleep or destroy has stopped work on takes none, as the Durable Object grants by it.
    let cap = |host: &Host| if pacing.napping(&host.id) { 0 } else { cap(config, host) };
    let heard = |host: &Host, answering: bool, ready: Vec<String>, held: i64| Heard { id: host.id.clone(), name: name(host), cap: cap(host), admitted: config.enrolled.contains(&host.id), answering, ready, held };
    let answered = answering.iter().map(|(host, ready, held)| heard(host, true, ready.clone(), *held));
    let projects: BTreeMap<String, String> = origins.iter().map(|(repo_id, origin)| (repo_id.clone(), project_id(origin))).filter(|(repo_id, project)| plain_repo(repo_id) && plain_project(project)).collect();
    let recipe = |repo_id: &String| config.repos.entry(names.get(repo_id)?).filter(|entry| entry.recipe.is_some() && plain_name(&entry.repo)).map(|entry| (repo_id.clone(), entry.repo.clone()));
    let recipes = projects.keys().filter_map(recipe).collect();
    Fleet { held: local_held, hosts: answered.chain(silent.iter().map(|(host, held)| heard(host, false, Vec::new(), *held))).collect(), projects, recipes }
}

// One line of `tolkster hosts`: this machine (no name) or a paired host, whether it answered
// the listing, its cap, the issues of the workers the Durable Object holds on it, and for a
// host a recipe made, the recipe's id.
#[derive(Debug, PartialEq, Serialize)]
pub struct Row {
    pub name: Option<String>,
    pub answers: bool,
    pub cap: i64,
    pub workers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipe: Option<String>,
}

// This machine, then each host that answered, then each that didn't. A worker is held on the
// host its record's worktree names, whether or not the host answers; a record on a host no
// longer paired shows nowhere.
pub fn report(config: &Config, answering: &[Host], silent: &[Host], records: &[WorkerRecord]) -> Vec<Row> {
    let held = |host: Option<&str>| records.iter().filter(|r| r.worktree_id.host() == host).map(|r| r.issue.clone()).collect();
    let row = |host: &Host, answers| Row { name: Some(host.name.clone()), answers, cap: cap(config, host), workers: held(Some(&host.id)), recipe: config.machines.get(host.name.trim()).map(|made| made.recipe.clone()) };
    let here = Row { name: None, answers: true, cap: config.local_slots(), workers: held(None), recipe: None };
    std::iter::once(here).chain(answering.iter().map(|host| row(host, true))).chain(silent.iter().map(|host| row(host, false))).collect()
}

// The recipe machine marked asleep that stays paired as `host`, by name.
pub fn asleep_as<'a>(config: &'a Config, host: &str) -> Option<&'a String> {
    config.machines.iter().find(|(_, made)| made.asleep && made.environment.as_deref() == Some(host)).map(|(name, _)| name)
}

// Whether each agent Orca lists in a worktree on `host` is done or waits at a prompt. With no
// worker `launched_lately` there, as its setup runs before its agent is listed, no agent works
// on it: a worker waiting on the owner, or exited, keeps no machine awake, as a command for it
// wakes the machine (docs/hosts.md, Sleep and wake).
pub fn resting(worktrees: &[Worktree], host: &str) -> bool {
    let resting = |pane: &Pane| matches!(pane.state.as_str(), "done" | "waiting" | "blocked");
    worktrees.iter().filter(|w| w.id.host() == Some(host)).all(|w| w.agents.iter().all(resting))
}

// Whether a worker's record on `host` was launched within SETTING_UP_MS.
pub fn launched_lately(records: &[WorkerRecord], host: &str, now_ms: i64) -> bool {
    records.iter().any(|r| r.worktree_id.host() == Some(host) && instant(&r.at).is_some_and(|at| now_ms - at < SETTING_UP_MS))
}

// Whether `host` holds no worktree but its checkouts, and no worker was `launched_lately` there:
// a destroy takes nothing with it (docs/hosts.md, Destroying).
pub fn holds_nothing(worktrees: &[Worktree], records: &[WorkerRecord], host: &str, now_ms: i64) -> bool {
    worktrees.iter().all(|w| w.is_main || w.id.host() != Some(host)) && !launched_lately(records, host, now_ms)
}

// The recipe machines `listen`'s listing says are idle, by host ID, with their repo's idle
// minutes: each awake one that sleeps, admitted, answering and resting as Orca
// lists it. The Durable Object's clock runs while a host stays so (docs/hosts.md, Sleep and wake).
pub fn idle(config: &Config, pacing: &Pacing, answering: &[Host], worktrees: &[Worktree]) -> BTreeMap<String, i64> {
    let minutes = |made: &Machine| config.repos.entry(&made.repo).and_then(|entry| entry.recipe.as_ref()).map_or(DEFAULT_IDLE_MINUTES, |machines| machines.idle_minutes);
    let idle = |host: &Host| {
        let name = host.name.trim();
        let made = config.machines.get(name).filter(|made| made.sleeps && !made.asleep)?;
        (config.enrolled.contains(&host.id) && !pacing.napping(&host.id) && resting(worktrees, &host.id)).then(|| (host.id.clone(), minutes(made)))
    };
    answering.iter().filter_map(idle).collect()
}

// A mark no make is behind is destroyed after an hour, whatever its repo's `destroyAfter`.
const MARK_MINUTES: i64 = 60;

// The recipe machines whose destroy clock runs (docs/hosts.md, Destroying), by name, with their
// repo's `destroyAfter`: each asleep, each mark, as none is being made when this is asked, with
// MARK_MINUTES at most, and each made and awake that answers holding no worktree but its
// checkouts, unadmitted or with no `suspend`. The Durable Object's clock runs while one stays so.
pub fn dormant(config: &Config, pacing: &Pacing, answering: &[Host], worktrees: &[Worktree]) -> BTreeMap<String, i64> {
    let minutes = |made: &Machine| config.repos.entry(&made.repo).and_then(|entry| entry.recipe.as_ref()).map_or(DEFAULT_DESTROY_AFTER, |machines| machines.destroy_after);
    let empty = |host: &Host| !pacing.napping(&host.id) && worktrees.iter().all(|w| w.is_main || w.host() != Some(host.id.as_str()));
    let awake = |name: &str, made: &Machine| answering.iter().find(|host| host.name.trim() == name).is_some_and(|host| (!made.sleeps || !config.enrolled.contains(&host.id)) && empty(host));
    config.machines.iter().filter(|(name, made)| !made.made || made.asleep || awake(name, made)).map(|(name, made)| (name.clone(), if made.made { minutes(made) } else { minutes(made).min(MARK_MINUTES) })).collect()
}

#[cfg(test)]
pub mod tests;
