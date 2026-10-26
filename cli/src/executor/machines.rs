// The recipe machines' own commands (docs/hosts.md), each the fleet's (src/worker/machine/
// fleet.ts), which alone decides them: making one, waking one, putting one to sleep and
// destroying one. Each is checked here against what is listed now, not what `listen` read at
// startup, and is carried out or kept, never another chosen.

use serde_json::json;

use crate::ports::Begun;
use crate::config::Config;
use crate::error::{Error, Waits};
use crate::github;
use crate::hosts;
use crate::machine::{Command, Finished};
use crate::orca::Worktree;
use crate::plain::{IssueKey, split_worktree};
use crate::ports::Ports;
use crate::reap;
use crate::time::now_ms;
use std::time::Instant;
use crate::trace::{self, Declined, Unslept};

// What a make or wake asked off the job lanes answers: `began`, or not begun, `coolDown` when
// `listen`'s cool-down refused it; one refused while another make, wake or sleep runs waits on the
// lock, and one on settings a make that ended since outdated is sent again.
fn begun(asked: Begun) -> Result<Finished, Error> {
    match asked {
        Ok(()) => Ok(Finished::ok(json!({ "began": true }))),
        Err(Some(Declined::Running)) => Err(Error::Waits(Waits::Lock, "a recipe machine is being made, woken or put to sleep".into())),
        Err(Some(Declined::CoolDown)) => Ok(Finished::ok(json!({ "began": false, "coolDown": true }))),
        Err(None) => Err(Error::Later("a make ended since the settings were read".into())),
    }
}

// `makeHost` (docs/hosts.md): a machine for `repo` from its recipe, made off the job lanes under
// its `machines` (`recipe::make`). A repo with no recipe begins none.
pub fn make_host(config: &Config, ports: &mut Ports, command: &Command, repo: &str) -> Result<Finished, Error> {
    let Some(entry) = config.repos.entry(repo).filter(|entry| entry.recipe.is_some()) else { return Ok(Finished::ok(json!({ "began": false }))) };
    begun(ports.machines.make(&entry.repo, &command.id))
}

// `wakeHost` (docs/hosts.md, Sleep and wake): the machine `name` woken off the job lanes, asleep
// or marked awake while Orca doesn't hear it. Its credential is read afresh: one a destroy
// revoked would wake to cap 0, so it isn't woken. Its host is asked every 10 s while it wakes.
pub fn wake_host(config: &Config, ports: &mut Ports, command: &Command, name: &str) -> Result<Finished, Error> {
    let Some(made) = config.machines.get(name).filter(|made| made.sleeps) else { return Ok(Finished::ok(json!({ "began": false }))) };
    ports.orca.worktrees()?;
    let paired = ports.orca.answering().into_iter().chain(ports.orca.silent()).find(|host| host.name.trim() == name).map(|host| host.id);
    let Some(host) = made.environment.clone().or(paired) else { return Ok(Finished::ok(json!({ "began": false }))) };
    if !ports.records.admitted(&host)? {
        return Ok(Finished::ok(json!({ "began": false })));
    }
    let woken = begun(ports.machines.wake(name, &command.id));
    if matches!(&woken, Ok(finished) if finished.detail.get("began") == Some(&json!(true))) {
        ports.shared.health.waking(&host, Instant::now());
    }
    woken
}

// `suspendHost` (docs/hosts.md, Sleep and wake): the recipe machine paired as `host` put to
// sleep, checked here: it answers, it is an awake machine that sleeps, holding an admitted
// credential, read afresh, so no make, wake or `host rm` is at work on it, and with placement
// and workers' commands stopped no agent works on it, nor is a worker set up or acted for
// there lately (`hosts::rests`). `asleep` says whether it went to sleep; one found otherwise
// is kept, and the nap machine's clock begins again once a listing names it idle.
pub fn suspend_host(config: &Config, ports: &mut Ports, host: &str) -> Result<Finished, Error> {
    // The port knows which hosts answer once it has listed.
    ports.orca.worktrees()?;
    let name = ports.orca.answering().into_iter().find(|answering| answering.id == host).map(|answering| answering.name.trim().to_string());
    let declined = |name: Option<&String>, why| {
        trace::event(trace::Host::NotSleeping { name: name.cloned(), id: host.into(), why });
        Ok(Finished::ok(json!({ "asleep": false })))
    };
    let Some(name) = name else { return declined(None, Unslept::NotAnswering) };
    let made = config.machines.get(&name);
    if ports.machines.making() {
        return declined(Some(&name), Unslept::Running);
    }
    match made {
        None => return declined(Some(&name), Unslept::NoSleep),
        Some(made) if !made.sleeps => return declined(Some(&name), Unslept::NoSleep),
        Some(made) if made.asleep => return declined(Some(&name), Unslept::AlreadyAsleep),
        Some(_) => {}
    }
    if !ports.records.admitted(host)? {
        return declined(Some(&name), Unslept::NotAdmitted);
    }
    let (orca, records, pacing) = (&mut *ports.orca, ports.records, &ports.shared.pacing);
    // A host gone silent during the listing holds worktrees unknown, never none.
    let mut rests = || {
        let worktrees = orca.worktrees()?;
        Ok(orca.answering().iter().any(|answering| answering.id == host) && hosts::rests(&worktrees, &records.workers()?, host, now_ms()) && !pacing.acted_lately(host, now_ms()))
    };
    let asleep = ports.machines.suspend(&name, host, &mut rests)?;
    if !asleep {
        return declined(Some(&name), Unslept::Busy);
    }
    trace::event(trace::Host::Asleep { name });
    Ok(Finished::ok(json!({ "asleep": true })))
}

// `destroyHost` (docs/hosts.md, Destroying): the recipe machine `name` destroyed, checked here:
// no make, wake or sleep runs, and it is dormant as `listen` reads it now (`hosts::dormant`),
// and one awake that sleeps unadmitted afresh; a wake since the settings were read stops it
// (`recipe::retire`). Every worktree on it is reaped first, there, each on the rule GitHub and Linear give it, and
// only a machine its own complete listing then shows with none, and no worker being set up,
// goes (`recipe::retire`). A reap that failed, or a listing that couldn't say, fails the
// command, which is tried again. `destroyed` says whether it went; one kept begins its clock
// again once a listing names it dormant.
pub fn destroy_host(config: &Config, ports: &mut Ports, name: &str) -> Result<Finished, Error> {
    let worktrees = ports.orca.worktrees()?;
    if ports.machines.making() {
        return Err(Error::Waits(Waits::Lock, "a recipe machine is being made, woken or put to sleep".into()));
    }
    let answering = ports.orca.answering();
    let kept = Finished::ok(json!({ "destroyed": false }));
    let Some(made) = config.machines.get(name).filter(|_| hosts::dormant(config, &ports.shared.pacing, &answering, &worktrees).contains_key(name)) else { return Ok(kept) };
    if let Some(host) = answering.iter().find(|host| host.name.trim() == name)
        && made.sleeps
        && !made.asleep
        && ports.records.admitted(&host.id)?
    {
        return Ok(kept);
    }
    let (orca, records, linear, github, pacing) = (&mut *ports.orca, ports.records, ports.linear, &mut *ports.github, &ports.shared.pacing);
    let mut reaped = |host: &str, reachable: bool| -> Result<bool, Error> {
        let on = |id: &str| split_worktree(id).and_then(|(at, _, _)| at) == Some(host);
        if !reachable {
            return Ok(!records.workers()?.iter().any(|r| on(&r.worktree_id)));
        }
        // A command acting there, a start's before it writes its record included, keeps it: one
        // after the nap finds it napping and waits.
        if pacing.acted_lately(host, now_ms()) {
            return Ok(false);
        }
        // Silent when this command first listed, if it was just woken.
        orca.ask_again();
        let theirs: Vec<Worktree> = orca.worktrees()?.into_iter().filter(|w| on(&w.id)).collect();
        let unknown = || Error::Waits(Waits::Host(host.to_string()), format!("{host} didn't answer a complete listing, so what it holds is unknown"));
        if !orca.answering().iter().any(|answering| answering.id == host) {
            return Err(unknown());
        }
        let mut branches = github::Branches::default();
        let done = reap::reap(orca, &mut reap::read_prs(github, &mut branches), &mut |id| records.forget_worker(id), &theirs, |issues: &[IssueKey]| linear.tolksters(issues), false)?;
        let failed = done.failed;
        done.lines.into_iter().for_each(trace::event);
        if let Some(wait) = done.spent {
            return Err(Error::RateLimited { message: format!("a reap on {host} waits for GitHub's budget"), wait });
        }
        if failed {
            return Err(Error::Later(format!("a reap on {host} failed")));
        }
        // The host's own listing complete, whichever others are silent.
        let (now, listed) = orca.agents_listed()?;
        if !orca.answering().iter().any(|answering| answering.id == host) {
            return Err(unknown());
        }
        // Another host's listing cut short decides nothing here, and isn't this host's to wait on.
        if listed.is_none() {
            return Err(Error::Later("a listing wasn't complete, so what the machine holds is unknown".into()));
        }
        Ok(hosts::holds_nothing(&now, &records.workers()?, host, now_ms()))
    };
    let destroyed = ports.machines.retire(name, &mut reaped)?;
    // Its destroy is traced as it runs (recipe::teardown).
    if !destroyed {
        trace::event(trace::Host::Kept { name: name.into() });
    }
    Ok(Finished::ok(json!({ "destroyed": destroyed })))
}
