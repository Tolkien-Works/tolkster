// A recipe machine's sleep and wake (docs/hosts.md, Sleep and wake). A sleeping machine keeps
// its disk, its pairing, its credential and its machines.json entry, marked `asleep`, which
// gives it a cap of 0: a wake runs its `resume` (none when Orca already hears it) and marks it
// awake once Orca hears it, so its worktrees keep their IDs and its workers their records. What
// `resume` prints of a pairing is never used. The lines run are those recorded with the
// machine, never what orca.yaml says today.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use super::{Change, Entries, Printed, Recipe, Script, USER_DATA_MAX, env, payload, recorded, recorded_machines, unshown, written};
use crate::cli::machines_file;
use crate::error::Error;
use crate::exec::{Run, run_command};
use crate::orca::{paired, ready_on};
use crate::plain::plain_host;
use crate::runtime::{Making, Shared};
use crate::trace;

// What a sleep or a wake needs of a machine's entry: the recipe it was made from, with the
// lines recorded with it, and whether it is asleep. None for a mark, or an entry without both
// lines or what `create` printed, which never sleeps.
pub(crate) fn sleeper(kept: &Value) -> Option<(Recipe, bool)> {
    let text = |key: &str| kept[key].as_str().filter(|text| !text.is_empty()).map(String::from);
    kept["userData"].as_object()?;
    let sleep = Some((text("suspend")?, text("resume")?));
    let recipe = Recipe { id: text("recipe")?, repo: text("repo")?, github: String::new(), folder: text("folder")?, create: String::new(), destroy: text("destroy")?, sleep };
    Some((recipe, kept["asleep"] == true))
}

// The machine `name` woken for the fleet's command `issue`: Some(its name) once Orca hears it
// and it is marked awake, None when it is no machine that sleeps. An asleep one is `woken`, a
// failure naming the step that failed, the machine asleep again; one marked awake that Orca
// doesn't hear is `revived`.
pub fn wake(shared: &Shared, name: &str, issue: &str) -> Result<Option<String>, Error> {
    if !plain_host(name) {
        return Ok(None);
    }
    let Some(kept) = recorded_machines()?.get(name).cloned() else { return Ok(None) };
    let Some((recipe, asleep)) = sleeper(&kept) else { return Ok(None) };
    let path = machines_file()?;
    let mut entries = |change: &mut Change| written(&path, change);
    trace::event(trace::Host::Waking { name: name.into(), issue: issue.into() });
    let mut pause = || std::thread::sleep(ANSWER_EVERY);
    let host = match asleep {
        true => woken(shared, &mut run_command, &mut unshown, &recipe, name, &kept, &mut entries, &mut pause)?,
        false => revived(shared, &mut run_command, &mut unshown, &recipe, name, &kept, &mut pause)?,
    };
    shared.pacing.placing_again(&host);
    Ok(Some(name.to_string()))
}

// The machine `name`, marked awake, that Orca doesn't hear, as when its platform restarted it
// without Orca's server: its recorded `resume` run once, unless Orca hears it by now, then the
// host's ID once Orca hears it. Its entry is left as it is.
pub(crate) fn revived(shared: &Shared, run: &mut Run, script: &mut Script, recipe: &Recipe, name: &str, kept: &Value, pause: &mut dyn FnMut()) -> Result<String, Error> {
    let Some((_, resume)) = &recipe.sleep else { return Err(Error::Failed(format!("{name}'s recipe has no suspend and resume"))) };
    let host = paired(run)?.into_iter().find(|host| host.name.trim() == name).ok_or_else(|| Error::Failed(format!("Orca no longer pairs {name}: tolkster host rm {name} destroys it")))?;
    if !ready_on(run, &host.id, None).unwrap_or(false) {
        let resumed = script(resume, &recipe.folder, &env(recipe, name), &payload(recipe, kept))?;
        if !resumed.ok {
            return Err(Error::Failed(format!("{name}'s resume failed{}", resumed.code.map_or(String::new(), |code| format!(" (exit {code})")))));
        }
        let answers = (0..ANSWER_TRIES).any(|tried| {
            if tried > 0 {
                pause();
            }
            ready_on(run, &host.id, None).unwrap_or(false)
        });
        if !answers {
            return Err(Error::Failed(format!("Orca didn't hear {name} after its resume")));
        }
    }
    shared.health.woken(&host.id, Instant::now());
    Ok(host.id)
}

// How often, and how many times, a woken machine is asked whether it answers.
pub(super) const ANSWER_EVERY: Duration = Duration::from_secs(10);
const ANSWER_TRIES: usize = 30;

// The recorded `resume` unless Orca already hears the host, then the entry, still asleep, with
// what `resume` printed of where the machine is, as `destroy` finds it by that; then, once Orca
// hears the host it stayed paired as, the entry awake: the host's ID. A failure past `resume`
// runs `suspend` again, the entry left asleep, so the next ask wakes it again.
#[allow(clippy::too_many_arguments)]
pub(crate) fn woken(shared: &Shared, run: &mut Run, script: &mut Script, recipe: &Recipe, name: &str, kept: &Value, entries: &mut Entries, pause: &mut dyn FnMut()) -> Result<String, Error> {
    let Some((suspend, resume)) = &recipe.sleep else { return Err(Error::Failed(format!("{name}'s recipe has no suspend and resume"))) };
    let host = paired(run)?.into_iter().find(|host| host.name.trim() == name).ok_or_else(|| Error::Failed(format!("Orca no longer pairs {name}: tolkster host rm {name} destroys it")))?;
    // Agent37 wakes an instance by itself and the image starts Orca's server on every boot, so Orca
    // may already hear the host: no `resume` then, which would run on a running instance.
    let heard = ready_on(run, &host.id, None).unwrap_or(false);
    let mut moved = kept.clone();
    if !heard {
        let resumed = script(resume, &recipe.folder, &env(recipe, name), &payload(recipe, kept))?;
        if !resumed.ok {
            return Err(Error::Failed(format!("{name}'s resume failed{}", resumed.code.map_or(String::new(), |code| format!(" (exit {code})")))));
        }
        let printed = Printed::of(&resumed.stdout);
        if let Some(data) = printed.user_data.filter(|data| data.to_string().len() <= USER_DATA_MAX) {
            moved["userData"] = data;
        }
        if let Some(root) = printed.project_root {
            moved["projectRoot"] = json!(root);
        }
    }
    let mut awake = moved.clone();
    if let Some(entry) = awake.as_object_mut() {
        entry.remove("asleep");
        entry.remove("environment");
    }
    let mut answers = |run: &mut Run| {
        (0..ANSWER_TRIES).any(|tried| {
            if tried > 0 {
                pause();
            }
            ready_on(run, &host.id, None).unwrap_or(false)
        })
    };
    let failed = if !entries(&mut |machines| recorded(machines, name, Some(kept), Some(&moved))).unwrap_or(false) {
        format!("machines.json no longer holds {name} as it went to sleep")
    } else if !heard && !answers(run) {
        format!("Orca didn't hear {name} after its resume")
    } else {
        match entries(&mut |machines| recorded(machines, name, Some(&moved), Some(&awake))) {
            // Heard, so no command waits out the silent host's back-off.
            Ok(true) => {
                shared.health.woken(&host.id, Instant::now());
                return Ok(host.id);
            }
            _ => format!("machines.json no longer holds {name} as it was woken"),
        }
    };
    if heard {
        return Err(Error::Failed(failed));
    }
    let slept = script(suspend, &recipe.folder, &env(recipe, name), &payload(recipe, &moved)).is_ok_and(|out| out.ok);
    Err(Error::Failed(match slept {
        true => format!("{failed}, so it was put to sleep again"),
        false => format!("{failed}, and its suspend failed: the next ask wakes it again"),
    }))
}

// The awake machine `name`, paired as `host`, put to sleep: placement stops there first, and
// only if `rests` then says no agent of Tolkster's works on it does its recorded `suspend` run.
// Ok(false): one was busy, or it is no awake machine that sleeps.
pub fn suspend(shared: &Arc<Shared>, name: &str, host: &str, rests: &mut dyn FnMut() -> Result<bool, Error>) -> Result<bool, Error> {
    let path = machines_file()?;
    let mut entries = |change: &mut Change| written(&path, change);
    // Its machine is marked asleep while `suspend` still runs, so no make or wake starts, nor
    // does `listen` restart, until it ends.
    let sleeping = Making::hold(shared);
    let slept = asleep(shared, &mut unshown, name, host, &mut entries, rests);
    // An ask on settings read before it changed them is asked again, as after a make.
    if !matches!(slept, Ok(false)) {
        shared.recipes.end();
    }
    drop(sleeping);
    slept
}

// `suspend`'s steps. One write marks the machine asleep, so its cap is 0 and placement stops,
// with the host it stays paired as; then the recorded `suspend` runs. A failed `suspend` marks
// it awake again. A run cut short leaves it marked asleep, which the next ask wakes.
pub(crate) fn asleep(shared: &Shared, script: &mut Script, name: &str, host: &str, entries: &mut Entries, rests: &mut dyn FnMut() -> Result<bool, Error>) -> Result<bool, Error> {
    let mark = |kept: &Value| {
        let mut marked = kept.clone();
        marked["asleep"] = json!(true);
        marked["environment"] = json!(host);
        marked
    };
    let kept = {
        let _acting = shared.pacing.stop_acting();
        if !rests()? {
            return Ok(false);
        }
        let mut kept = None;
        entries(&mut |machines| match machines.get(name).filter(|entry| sleeper(entry).is_some_and(|(_, asleep)| !asleep)) {
            Some(entry) => {
                kept = Some(entry.clone());
                recorded(machines, name, Some(entry), Some(&mark(entry)))
            }
            None => (Map::new(), false),
        })?;
        if kept.is_some() {
            trace::event(trace::Host::Sleeping { name: name.into() });
            shared.pacing.nap(host);
            shared.health.slept(host, name, Instant::now());
        }
        kept
    };
    let Some((kept, (recipe, _))) = kept.and_then(|kept| sleeper(&kept).map(|sleeps| (kept, sleeps))) else { return Ok(false) };
    let Some((suspend, _)) = &recipe.sleep else { return Ok(false) };
    if !script(suspend, &recipe.folder, &env(&recipe, name), &payload(&recipe, &kept)).is_ok_and(|out| out.ok) {
        let marked = mark(&kept);
        let _ = entries(&mut |machines| recorded(machines, name, Some(&marked), Some(&kept)));
        shared.pacing.placing_again(host);
        shared.health.woken(host, Instant::now());
        return Err(Error::Failed(format!("the recipe's suspend failed for {name}")));
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
