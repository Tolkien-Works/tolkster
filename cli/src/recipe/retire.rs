// A recipe machine dormant past its repo's `destroyAfter` destroyed (docs/hosts.md,
// Destroying), as `host rm` would, once nothing on it would be lost: an asleep one is woken with
// placement stopped there, and the caller reaps each of its worktrees by its rule's guards. One
// that keeps a worktree goes back to sleep, or takes work again, and is kept. A mark goes as it
// is: no `create` it records printed a machine anything ran on. The whole runs as the one
// machine under way (Making::alone), so no make, wake or sleep, nor a restart of `listen`, comes between its steps.

use std::sync::Arc;

use serde_json::{Map, Value};
use ureq::Agent;

use super::sleep::{ANSWER_EVERY, asleep, sleeper, woken};
use super::{Change, Entries, Script, destroyer, recorded, teardown, unshown, written};
use crate::cli::machines_file;
use crate::config::files;
use crate::error::{Error, Waits};
use crate::exec::{Run, beating};
use crate::orca::paired;
use crate::ports::Reaped;
use crate::runtime::{Making, Shared};
use crate::state::State;

// Some(whether the machine `name` went), or Err when another make, wake or sleep runs or ended
// since the settings the command checked were read, at `seen`, or a step failed, which the
// Durable Object asks again.
// Each step that may run long beats, so the lane it runs on isn't taken for stuck.
pub fn retire(agent: &Agent, shared: &Arc<Shared>, name: &str, seen: usize, reaped: &mut Reaped, beat: &dyn Fn()) -> Result<bool, Error> {
    let Some(_retiring) = Making::alone(shared).filter(|_| shared.recipes.ended() == seen) else { return Err(Error::Waits(Waits::Lock, "a recipe machine is being made, woken or put to sleep, or was since the settings were read".into())) };
    let files = files()?;
    let state = State::open(agent, &files)?;
    let path = machines_file()?;
    let mut entries = |change: &mut Change| written(&path, change);
    let mut revoke = |host: &str| state.revoke_host(host).map(|_| ());
    let mut script = |line: &str, folder: &str, env: &[(&str, &str)], stdin: &str| {
        beat();
        let out = unshown(line, folder, env, stdin);
        beat();
        out
    };
    let mut pause = || {
        beat();
        std::thread::sleep(ANSWER_EVERY);
    };
    let retired = retired(shared, &mut beating(beat), &mut script, name, &mut entries, reaped, &mut revoke, &mut pause);
    // An ask on settings read before it changed them is asked again, as after a make.
    shared.recipes.end();
    retired
}

// `retire`'s steps. Ok(true) once the machine is gone, its entry with it; Ok(false) when kept.
// After a wake or with placement stopped, a keep or a failure puts the machine back as it was:
// asleep again, or placed on, which, revoked, it never is.
#[allow(clippy::too_many_arguments)]
pub(crate) fn retired(shared: &Shared, run: &mut Run, script: &mut Script, name: &str, entries: &mut Entries, reaped: &mut Reaped, revoke: &mut dyn FnMut(&str) -> Result<(), Error>, pause: &mut dyn FnMut()) -> Result<bool, Error> {
    let Some(kept) = entry(entries, name)? else { return Ok(true) };
    let recipe = destroyer(name, &kept)?;
    let (made, asleep_now) = (kept["userData"].is_object(), kept["asleep"] == true);
    let environment = kept["environment"].as_str().unwrap_or_default();
    // An asleep one only as the host it slept as: another paired under its name since is none of
    // its, and two under it leave which is which to the owner (`host rm`).
    let named: Vec<String> = paired(run)?.into_iter().filter(|host| host.name.trim() == name).map(|host| host.id).collect();
    if named.len() > 1 {
        return Ok(false);
    }
    let host = named.into_iter().find(|host| !asleep_now || host == environment);
    let Some(host) = host.clone().filter(|_| made) else {
        // A mark, or one asleep Orca no longer pairs, which a sleep under an older `listen` left;
        // never one awake that Orca doesn't, which may yet answer.
        if made && !(asleep_now && !environment.is_empty() && reaped(environment, false)?) {
            return Ok(false);
        }
        if let Some(id) = host.as_deref().or(Some(environment).filter(|id| !id.is_empty())) {
            revoke(id)?;
        }
        return destroyed(run, script, name, &recipe, &kept, entries, host.is_some());
    };
    let kept = match sleeper(&kept) {
        Some((sleeping, true)) => {
            shared.pacing.nap(&host);
            if let Err(error) = woken(shared, run, script, &sleeping, name, &kept, entries, pause) {
                shared.pacing.placing_again(&host);
                return Err(error);
            }
            entry(entries, name).and_then(|now| now.ok_or_else(|| Error::Failed(format!("machines.json no longer holds {name} as it was woken"))))
        }
        _ => {
            let _acting = shared.pacing.stop_acting();
            shared.pacing.nap(&host);
            Ok(kept)
        }
    };
    let gone = kept.and_then(|kept| match reaped(&host, true) {
        Ok(true) => revoke(&host).and_then(|()| destroyed(run, script, name, &recipe, &kept, entries, true)),
        other => other.map(|_| false),
    });
    if !matches!(gone, Ok(true)) {
        let slept = asleep_now && asleep(shared, script, name, &host, entries, &mut || Ok(true)).is_ok_and(|slept| slept);
        if !slept {
            shared.pacing.placing_again(&host);
        }
    }
    gone
}

// The recorded `destroy`, then the pairing, when `paired_here` (Orca pairs it as its own), and
// the entry: Err when `destroy` failed, the entry kept for the next try.
fn destroyed(run: &mut Run, script: &mut Script, name: &str, recipe: &super::Recipe, kept: &Value, entries: &mut Entries, paired_here: bool) -> Result<bool, Error> {
    let mut record = |entry: Option<&Value>| entries(&mut |machines| recorded(machines, name, None, entry)).map(|_| ());
    let step = teardown(run, script, recipe, name, kept, (true, paired_here), &mut record);
    match step.verdict {
        "done" => Ok(true),
        _ => Err(Error::Failed(format!("{name}: {}", step.reason))),
    }
}

// `name`'s machines.json entry as it is now.
fn entry(entries: &mut Entries, name: &str) -> Result<Option<Value>, Error> {
    let mut found = None;
    entries(&mut |machines| {
        found = machines.get(name).filter(|entry| !entry.is_null()).cloned();
        (Map::new(), false)
    })?;
    Ok(found)
}

#[cfg(test)]
mod tests;
