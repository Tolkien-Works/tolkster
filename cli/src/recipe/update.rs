// A sleeping recipe machine moved onto the image its recipe now builds (docs/hosts.md, Rolling
// out): the recipe's `update` runs on the stopped machine, which keeps its disk, its worktrees
// and its pairing, and its next wake boots the new image. Only an asleep machine is updated, as
// no agent can be at work on it. The line run is `update` as orca.yaml has it today, as for
// `prepare`: it brings the machine up to the repo, and machines made before it have none
// recorded.

use std::sync::Arc;

use serde_json::Value;

use super::sleep::sleeper;
use super::{Change, Entries, NOTHING_TO_DO, Printed, Script, USER_DATA_MAX, env, find, payload, recorded, recorded_machines, unshown, written};
use crate::cli::machines_file;
use crate::error::{Error, Waits};
use crate::exec::run_command;
use crate::plain::plain_host;
use crate::runtime::{Making, Shared};
use crate::trace;

// The machine `name` updated: whether it was. No make, wake, sleep or destroy runs meanwhile,
// so it stays asleep throughout: one asked while another runs waits on the lock. Ok(false): it
// is no asleep machine, its recipe has no `update`, or the recipe found it running.
pub fn update(shared: &Arc<Shared>, name: &str) -> Result<bool, Error> {
    if !plain_host(name) {
        return Ok(false);
    }
    let Some(updating) = Making::alone(shared) else { return Err(Error::Waits(Waits::Lock, "a recipe machine is being made, woken or put to sleep".into())) };
    let Some(kept) = recorded_machines()?.get(name).cloned() else { return Ok(false) };
    let Some((recipe, true)) = sleeper(&kept) else { return Ok(false) };
    let Some(line) = find(&mut run_command, &recipe.id, Some(&recipe.repo))?.update else { return Ok(false) };
    let path = machines_file()?;
    let mut entries = |change: &mut Change| written(&path, change);
    trace::event(trace::Host::Updating { name: name.into() });
    let updated = updated(&mut unshown, &line, name, &kept, &mut entries);
    // An ask on settings read before it changed them is asked again, as after a make.
    if !matches!(updated, Ok(false)) {
        shared.recipes.end();
    }
    drop(updating);
    if matches!(updated, Ok(true)) {
        trace::event(trace::Host::Updated { name: name.into() });
    }
    updated
}

// `update`'s steps: `line` run on the stopped machine with what `create` printed, then its entry,
// still asleep, with the `userData` it printed, which holds the revision it now runs.
pub(crate) fn updated(script: &mut Script, line: &str, name: &str, kept: &Value, entries: &mut Entries) -> Result<bool, Error> {
    let Some((recipe, true)) = sleeper(kept) else { return Ok(false) };
    let out = script(line, &recipe.folder, &env(&recipe, name), &payload(&recipe, kept))?;
    if out.code == Some(NOTHING_TO_DO) {
        return Ok(false);
    }
    if !out.ok {
        return Err(Error::Failed(format!("{name}'s update failed{}", out.code.map_or(String::new(), |code| format!(" (exit {code})")))));
    }
    let Some(data) = Printed::of(&out.stdout).user_data.filter(|data| data.to_string().len() <= USER_DATA_MAX) else {
        return Err(Error::Failed(format!("{name}'s update printed no userData object of up to {USER_DATA_MAX} bytes")));
    };
    let mut moved = kept.clone();
    moved["userData"] = data;
    if !entries(&mut |machines| recorded(machines, name, Some(kept), Some(&moved)))? {
        return Err(Error::Failed(format!("machines.json no longer holds {name} as it was updated")));
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
