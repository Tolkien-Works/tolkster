// `listen` keeping itself current (docs/realtime.md, "Keeping up with main"). A development
// build (`--features develop`) pulls and rebuilds from its checkout (`develop`); every other
// build takes the Worker's build of the commit a push names (`release`). What both share:
// the pass's outcome, what the lane, the loop and the build share, the held base tips, and
// the restart onto a new build.

use std::fs;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cli::BRANCH;
use crate::error::{Context, Error};
use crate::plain::{Sha, short_sha};
use crate::sync::lock;
use crate::trace::{self, Update as Said};

#[cfg(feature = "develop")]
mod develop;
#[cfg(feature = "develop")]
pub use develop::*;
#[cfg(not(feature = "develop"))]
mod release;
#[cfg(not(feature = "develop"))]
pub use release::*;

// Whether a push's `reference` is to the branch `listen` keeps itself current with.
fn own_branch(reference: &str) -> bool {
    reference.strip_prefix("refs/heads/") == Some(BRANCH)
}

// Why `listen` restarts: a new build put in place, or a run that is stuck.
pub use crate::trace::Restart;

// What a pass came to, for a base tip (`Tips`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pass {
    // Pulled, and built or building, or built already.
    Pulled,
    // Left as it was: a guard, which a retry won't change, or no tip to wait for.
    Skipped,
    // origin/main doesn't hold the tip's commit.
    Behind,
    // Not tried: the fetch failed, or a build or a drain kept the pass from pulling.
    Unready,
}

// What the shared lane, the build's thread and the loop (listen.rs) share.
#[derive(Clone, Default)]
pub struct Update {
    // When the build began or last ran a command, or None when none runs. Set in the
    // shared lane before the build's thread starts and cleared by that thread as it
    // ends, so no two builds, nor a pull and a build, ever overlap.
    building: Arc<Mutex<Option<Instant>>>,
    // A pass put off while a build ran.
    put_off: Arc<AtomicBool>,
    // The HEAD this run last built and put in place, or found in place: a pass that finds
    // HEAD there builds nothing. None after a build that failed, so the next pass builds.
    #[cfg(feature = "develop")]
    built: Arc<Mutex<Option<String>>>,
    // The build is being put in place: a restart now could find the installed path empty.
    placing: Arc<AtomicBool>,
    // A new build in place: the restart `listen` drains for.
    restart: Arc<Mutex<Option<Restart>>>,
}

impl Update {
    pub fn building_since(&self) -> Option<Instant> {
        self.building()
    }

    fn building(&self) -> Option<Instant> {
        *lock(&self.building)
    }

    #[cfg(any(feature = "develop", test))]
    pub fn set_building(&self, at: Option<Instant>) {
        *lock(&self.building) = at;
    }

    pub fn builds(&self) -> bool {
        self.building().is_some()
    }

    pub fn placing(&self) -> bool {
        self.placing.load(Ordering::SeqCst)
    }

    pub fn stuck(&self, after: Duration) -> bool {
        self.building().is_some_and(|since| since.elapsed() >= after)
    }

    pub fn draining(&self) -> bool {
        lock(&self.restart).is_some()
    }

    pub fn in_place(&self, restart: Restart) {
        *lock(&self.restart) = Some(restart);
    }

    pub fn take_restart(&self) -> Option<Restart> {
        lock(&self.restart).take()
    }

    // Whether to make the pass a build put off, once: after the build has ended, and not
    // while draining, as the restart makes one as it starts.
    pub fn take_put_off(&self) -> bool {
        self.building().is_none() && !self.draining() && self.put_off.swap(false, Ordering::SeqCst)
    }
}

// Times a base tip may be held before it is left for the next push (`Tips`), a minute
// apart (BASE_AGAIN in scheduler.rs): long enough to outlast GitHub's lag and a build.
pub const BASE_TRIES: u32 = 30;

// The newest of Tolkster's own base tips acted on. One that pulled isn't passed again when
// a reconnect sends it again, and one `origin/main` hasn't held after BASE_TRIES fetches,
// as after a rewrite of main, is left for the next push, as the relay keeps sending it. A
// failed fetch, a build or a drain counts no try.
#[derive(Default)]
pub struct Tips {
    after: String,
    held: u32,
    passed: bool,
}

impl Tips {
    // `pass` makes the update's pass for `after`. Whether to hold the tip and try again.
    pub fn take(&mut self, after: &Sha, say: &mut dyn FnMut(Said), pass: impl FnOnce() -> Pass) -> bool {
        if self.after != after.as_str() {
            *self = Tips { after: after.to_string(), held: 0, passed: false };
        }
        if self.passed || self.held >= BASE_TRIES {
            return false;
        }
        match pass() {
            Pass::Pulled => {
                self.passed = true;
                return false;
            }
            Pass::Skipped => return false,
            Pass::Unready => return true,
            Pass::Behind => {}
        }
        self.held += 1;
        if self.held >= BASE_TRIES {
            say(Said::StillNotHeld { after: short_sha(after.as_str()).into() });
            return false;
        }
        true
    }
}

// One an exiting run still holds waits for the next start.
pub fn clear_old() {
    let Ok(exe) = std::env::current_exe() else { return };
    let (Some(dir), Some(name)) = (exe.parent(), exe.file_name().and_then(|n| n.to_str())) else { return };
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let file = entry.file_name();
        let file = file.to_string_lossy();
        if file.starts_with(&format!("{name}.")) && file.ends_with(".old") {
            let _ = fs::remove_file(entry.path());
        }
    }
}

// What the acting `listen` exits with to restart onto the build now installed (`supervise`).
const RESTARTING: u8 = 75;
// Set on the acting `listen` its supervisor starts.
const ACTING: &str = "TOLKSTER_LISTEN_ACTING";

pub fn restart(restart: &Restart) -> Result<u8, Error> {
    trace::event(Said::Restart(restart.clone()));
    Ok(RESTARTING)
}

pub fn acting() -> bool {
    std::env::var_os(ACTING).is_some()
}

// The `listen` the owner starts only starts the one that acts, in the same console, and starts
// it again, from the installed path, each time it exits to restart. It waits on each, so the
// shell never takes the console back between two runs, and it ends as the last one does.
pub fn supervise() -> Result<u8, Error> {
    let exe = crate::prebuilt::own_exe().context("finding this tolkster")?;
    loop {
        let status = Command::new(&exe).args(std::env::args_os().skip(1)).env(ACTING, "1").status().context(format!("starting {}", exe.display()))?;
        match status.code() {
            Some(code) if code == i32::from(RESTARTING) => {}
            code => return Ok(code.and_then(|code| u8::try_from(code).ok()).unwrap_or(1)),
        }
    }
}

#[cfg(test)]
mod tests;
