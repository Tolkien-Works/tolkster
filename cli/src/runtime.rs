// What `tolkster listen` carries out commands and handles events through (command.rs,
// executor.rs, commands/listen/handlers.rs): the settings and Tolkster's state, read fresh for each
// (`Runtime`), and one log line per outcome (`log`).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use ureq::Agent;

use crate::cli::home_dir;
use crate::config::{self, Config, Files, files, owner};
use crate::error::Error;
use crate::health::Health;
use crate::hosts::Pacing;
use crate::issue_state::Started;
use crate::plain::Sha;
use crate::state::State;
use crate::sync::lock;
use crate::timing::Timing;
use crate::trace::{self, Outcome, Refresh};

// What one `listen` keeps from command to command, which its commands, its watcher and the
// threads they start reach through `Runtime` and the ports, never a static: the hosts' health
// and pacing, the recipe machines under way, and the caches. A command run on its own builds one
// of its own, as a test does. Each part is defined here, below the workflows that use it.
#[derive(Default)]
pub struct Shared {
    pub health: Health,
    pub pacing: Pacing,
    pub recipes: Progress,
    pub checkouts: Checkouts,
    pub queued: QueuedSaid,
    pub started: Started,
    pub timing: Timing,
}

impl Shared {
    // `settings` with the hosts holding an admitted credential as `listen` last read them
    // (Pacing::enrolled), and the revisions the recipes last prepared (Progress::prepared).
    pub fn settings(&self, files: &Files) -> Result<Config, Error> {
        let config = Config { enrolled: self.pacing.enrolled(), revisions: self.recipes.revisions(), ..settings(files)? };
        self.timing.allow(config.timing);
        Ok(config)
    }
}

// The recipe machines under way (recipe.rs).
#[derive(Default)]
pub struct Progress {
    // Machines `spawn` is making or waking, and sleeps and destroys running: `listen` neither
    // restarts nor quits while one is, as a run cut short past `create` leaves a paid machine
    // only its mark knows of.
    making: AtomicUsize,
    // Makes, wakes and sleeps ended, and when the last make or wake that failed did: a make that
    // fails after `create` costs money, so none starts within recipe's COOL_DOWN of one.
    made: AtomicUsize,
    failed: Mutex<Option<Instant>>,
    // What each key's last refusal was: one repeated on every pass of the queue is said once,
    // until its reason changes or a make or wake for the key begins.
    declined: Mutex<BTreeMap<String, trace::Declined>>,
    // The image revision each repo's recipe last said its machines are brought to, by repo, as
    // its `prepare` printed it (recipe.rs, `spawn_prepare`): none until one has run.
    revisions: Mutex<BTreeMap<String, String>>,
}

impl Progress {
    pub fn making(&self) -> bool {
        self.making.load(Ordering::SeqCst) > 0
    }

    pub fn ended(&self) -> usize {
        self.made.load(Ordering::SeqCst)
    }

    // A make, wake, sleep or destroy ended, before its `Making` counts it out.
    pub fn end(&self) {
        self.made.fetch_add(1, Ordering::SeqCst);
    }

    pub fn fail(&self) {
        *lock(&self.failed) = Some(Instant::now());
    }

    pub fn failed(&self) -> Option<Instant> {
        *lock(&self.failed)
    }

    // Whether `said` was said.
    pub fn decline(&self, key: String, why: trace::Declined, said: trace::Host) -> bool {
        let new = lock(&self.declined).insert(key, why.clone()) != Some(why);
        if new {
            trace::event(said);
        }
        new
    }

    // A make or wake for `key` began, so its next refusal is said.
    pub fn began(&self, key: &str) {
        lock(&self.declined).remove(key);
    }

    pub fn prepared(&self, repo: &str, revision: String) {
        lock(&self.revisions).insert(repo.to_string(), revision);
    }

    pub fn revisions(&self) -> BTreeMap<String, String> {
        lock(&self.revisions).clone()
    }
}

// A machine in `Progress::making`, counted out when dropped, so a panic can't hold `listen` for
// good: the only way the count moves. It holds the context, as a make's thread outlives the
// command that asked.
pub struct Making(Arc<Shared>);

impl Making {
    // Counted in as the only one, in one step, so two asks at once never both make one: None
    // while another is.
    pub fn alone(shared: &Arc<Shared>) -> Option<Making> {
        let counted = shared.recipes.making.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst).is_ok();
        counted.then(|| Making(Arc::clone(shared)))
    }

    // Counted in beside any other.
    pub fn hold(shared: &Arc<Shared>) -> Making {
        shared.recipes.making.fetch_add(1, Ordering::SeqCst);
        Making(Arc::clone(shared))
    }

    pub fn recipes(&self) -> &Progress {
        &self.0.recipes
    }
}

impl Drop for Making {
    fn drop(&mut self) {
        self.0.recipes.making.fetch_sub(1, Ordering::SeqCst);
    }
}

// The conversations checkouts (converse.rs).
#[derive(Default)]
pub struct Checkouts {
    // Held while a checkout is looked at, refreshed, or has a turn's terminal opened in it, so a
    // refresh never runs between a check that no turn is running and that turn's terminal
    // opening. Only one `listen` runs on a workstation.
    held: Mutex<()>,
    // The tip of each repo's default branch, by its GitHub name in lower case, as the relay last
    // sent it (`base_tip`). A turn starts without a fetch in a checkout already at it.
    tips: Mutex<BTreeMap<String, Sha>>,
}

impl Checkouts {
    pub fn lock(&self) -> MutexGuard<'_, ()> {
        lock(&self.held)
    }

    // True when `after` is new for `repo`.
    pub fn base_tip(&self, repo: &str, after: &Sha) -> bool {
        lock(&self.tips).insert(repo.to_ascii_lowercase(), after.clone()).as_ref() != Some(after)
    }

    pub fn known_tip(&self, repo: &str) -> Option<Sha> {
        lock(&self.tips).get(&repo.to_ascii_lowercase()).cloned()
    }
}

// What each issue's queued request last waited for, as said, so the five-minute blocker
// re-check says it again only when it changes (or a newer request waits), by issue: IDs only
// (command.rs).
#[derive(Default)]
pub struct QueuedSaid(Mutex<BTreeMap<String, String>>);

impl QueuedSaid {
    // Whether `seen` differs from what was last said of `issue`; it is then what was.
    pub fn changed(&self, issue: &str, seen: String) -> bool {
        let mut said = lock(&self.0);
        said.insert(issue.to_string(), seen.clone()).is_none_or(|before| before != seen)
    }

    // Said again by the next re-check, whatever it sees.
    pub fn forget(&self, issue: &str) {
        lock(&self.0).remove(issue);
    }
}

// The settings, read fresh for each command. `listen` reads them at startup too, so
// a missing one stops it at once. No host is admitted in them (`Shared::settings`).
pub fn settings(files: &Files) -> Result<Config, Error> {
    config::config(files, owner(files)?, &home_dir()?)
}

// What a command or an event acts with: the settings and Tolkster's state, read fresh for
// each (`load`), the agent, and what `listen` keeps.
pub struct Runtime<'a> {
    pub agent: &'a Agent,
    pub config: Config,
    pub state: State<'a>,
    pub shared: &'a Arc<Shared>,
}

// config.json, credentials.json and Tolkster's state, read fresh for each.
pub fn load<'a>(agent: &'a Agent, shared: &'a Arc<Shared>) -> Result<Runtime<'a>, Error> {
    let files = files()?;
    Ok(Runtime { agent, config: shared.settings(&files)?, state: State::open(agent, &files)?, shared })
}

// What refreshing conversations checkouts came to: a line for each, and the job's outcome.
pub fn log(result: Result<Vec<Refresh>, Error>) {
    let outcome = match result {
        Ok(done) if done.is_empty() => Outcome::NothingToDo,
        Ok(done) => {
            let failed = done.iter().any(|refresh| !matches!(refresh, Refresh::Refreshed { .. }));
            done.into_iter().for_each(trace::event);
            if failed { Outcome::Failed } else { Outcome::Refreshed }
        }
        Err(error) => Outcome::Error(error),
    };
    trace::event(outcome);
}
