// A worker host on a machine an Orca environment recipe in a repo's orca.yaml makes
// (docs/hosts.md): `host add --recipe` on the owner's go, or `make` for `listen` off a repo's
// opt-in, then its sleep, wake and retirement. Its `create` costs money, so it runs only once
// Orca's own checks pass the recipe. What `create` prints holds the pairing code, a secret: it
// goes to `orca environment add` and nowhere else. The host is remembered with its recipe in
// machines.json before it is paired, so a run cut short leaves `host rm` what it needs to
// destroy the machine, and a failure after `create` destroys it at once. A mark in
// machines.json counts the machine against the repo's `machines` from before `create` runs.

use std::io::Write;
use std::path::Path;
use std::process::{Command as Process, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};
use ureq::Agent;

use crate::cli::{machines_file, merge_json_file};
use crate::config::{Files, files, plain_revision, realtime};
use crate::error::{Context, Error};
use crate::exec::{Output, Run, run_command};
use crate::plain::split_repo;
use crate::onboard::{Author, NO_AUTHOR, Step, passed, set_up, step, unstamped};
use crate::orca::{Host, github_repos, import_folder, pair, paired, project_id, recipe_passes, repo_folders, setup_at, unpair};
use crate::plain::{plain_destination, plain_host};
use crate::prebuilt;
use crate::ports::Begun;
use crate::runtime::{Making, Shared, settings};
use crate::state::State;
use crate::sync::lock;
use crate::trace;

// A recipe as one repo's orca.yaml has it: the repo by its display name here and on GitHub,
// the folder its scripts run in, and its `create` and `destroy` lines, `sleep`, its
// `suspend` and `resume` when it has both: a machine with only one would sleep for good,
// `prepare`, which brings what its machines are made from up to the repo (`spawn_prepare`), and
// `update`, which moves a stopped machine onto that in place (`update::update`).
pub(crate) struct Recipe {
    pub id: String,
    pub repo: String,
    pub github: String,
    pub folder: String,
    pub create: String,
    pub destroy: String,
    pub sleep: Option<(String, String)>,
    pub prepare: Option<String>,
    pub update: Option<String>,
}

// A recipe's script line run on this machine in `folder`, with `env` and `stdin`: how it ended
// and its stdout. Its stderr is the owner's to read: their terminal's, or under `listen`, the
// disk log's (`unshown`). Its stdout holds the pairing code, so is never shown.
pub(crate) type Script<'a> = dyn FnMut(&str, &str, &[(&str, &str)], &str) -> Result<Output, Error> + 'a;
// Writes the host's machines.json entry, refusing one already there, or with None takes it out.
pub(crate) type Record<'a> = dyn FnMut(Option<&Value>) -> Result<(), Error> + 'a;
// The steps of `host add` on the paired host, with the recipe's checkout as (`owner/name`, folder).
pub(crate) type SetUp<'a> = dyn FnMut(&mut Run, &Host, (&str, &str)) -> Result<Vec<Step>, Error> + 'a;
// What `recorded` gives for machines.json as it is: the change, and whether it is made.
pub(crate) type Change<'a> = dyn FnMut(&Map<String, Value>) -> (Map<String, Value>, bool) + 'a;
// One read-then-write of machines.json by a Change: whether it was made.
pub(crate) type Entries<'a> = dyn FnMut(&mut Change) -> Result<bool, Error> + 'a;

// The credential of the environment the machine paired as, by its ID: none when it never paired.
pub(crate) fn revoke(state: &State, environment: &Option<String>) {
    if let Some(id) = environment {
        let _ = state.revoke_host(id);
    }
}

// A machine for `repo` from its opt-in, made, set up and listed on a thread of its own, off
// `listen`'s job lanes, as it takes minutes. What it did goes to the trace, never what
// `create` printed. The ask read its settings when `ended` was `seen` (`claim`). `issue`
// names what asked for it in the trace: the fleet's.
pub fn spawn(agent: &Agent, shared: &Arc<Shared>, repo: &str, issue: &str, seen: usize) -> Begun {
    let (agent, issue, making) = (agent.clone(), issue.to_string(), Arc::clone(shared));
    let (made, failed, refused) = (repo.to_string(), repo.to_string(), repo.to_string());
    off_lanes(
        shared,
        format!("make {repo}"),
        seen,
        trace::Host::MakeAsked { repo: repo.to_string() },
        move || Ok(make(&agent, &making, &made, &issue)?.map(|name| trace::Host::Made { name, repo: made })),
        move |error| trace::Host::NotMade { repo: failed, error },
        move |why| trace::Host::NotMaking { repo: refused, why },
    )
}

// `spawn` for the machine `name`, woken (sleep::wake): as long as a make, and one at a time with
// them.
pub fn spawn_wake(shared: &Arc<Shared>, name: &str, issue: &str, seen: usize) -> Begun {
    let (woken, failed, refused, issue, waking) = (name.to_string(), name.to_string(), name.to_string(), issue.to_string(), Arc::clone(shared));
    off_lanes(
        shared,
        format!("wake {name}"),
        seen,
        trace::Host::WakeAsked { name: name.to_string() },
        move || Ok(sleep::wake(&waking, &woken, &issue)?.map(|name| trace::Host::Woken { name })),
        move |error| trace::Host::NotWoken { name: failed, error },
        move |why| trace::Host::NotWaking { name: refused, why },
    )
}

fn off_lanes(
    shared: &Arc<Shared>,
    key: String,
    seen: usize,
    asked: trace::Host,
    job: impl FnOnce() -> Result<Option<trace::Host>, Error> + Send + 'static,
    failed: impl FnOnce(Error) -> trace::Host + Send + 'static,
    refused: impl FnOnce(trace::Declined) -> trace::Host,
) -> Begun {
    let making = match claim(shared, seen) {
        Ok(making) => making,
        Err(why) => {
            if let Some(why) = why.clone() {
                shared.recipes.decline(key, why.clone(), refused(why));
            }
            return Err(why);
        }
    };
    shared.recipes.began(&key);
    trace::event(asked);
    std::thread::spawn(move || {
        let recipes = making.recipes();
        // A panic counts as a failure, so it ends and cools down like one.
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).unwrap_or_else(|_| Err(Error::Failed("panicked".into()))) {
            Ok(Some(done)) => trace::event(done),
            Ok(None) => {}
            Err(error) => {
                recipes.fail();
                trace::event(failed(error));
            }
        }
        // Before `making` counts it out.
        recipes.end();
        drop(making);
    });
    Ok(())
}

// Each repo's opted-in recipe's `prepare` run, on a thread of its own, as it may rebuild for
// many minutes: `listen` asks as it starts and after each pull it doesn't restart for, so a
// merged change to what the recipe's machines are made from rolls out with no one running it.
// The recipe decides what changed, exiting 3 when nothing did, and prints the image revision its
// machines are brought to, which the config report carries (`Progress::prepared`). One run at a
// time: an ask while one runs makes one more after it, as the run may have read the checkout
// before the pull.
pub fn spawn_prepare(shared: &Arc<Shared>) {
    // Whether a run is going, and whether another was asked for since it began.
    static PREPARING: Mutex<(bool, bool)> = Mutex::new((false, false));
    {
        let mut preparing = lock(&PREPARING);
        if preparing.0 {
            preparing.1 = true;
            return;
        }
        *preparing = (true, false);
    }
    let shared = Arc::clone(shared);
    std::thread::spawn(move || loop {
        let _ = std::panic::catch_unwind(|| prepare_all(&shared));
        let mut preparing = lock(&PREPARING);
        if !preparing.1 {
            preparing.0 = false;
            return;
        }
        preparing.1 = false;
    });
}

fn prepare_all(shared: &Shared) {
    let config = match files().and_then(|files| settings(&files)) {
        Ok(config) => config,
        Err(error) => return trace::event(trace::Host::NotPrepared { repo: None, error }),
    };
    let mut run = run_command;
    for (repo, id) in config.repos.0.iter().filter_map(|entry| Some((&entry.repo, &entry.recipe.as_ref()?.id))) {
        let prepared = find(&mut run, id, Some(repo)).and_then(|recipe| match &recipe.prepare {
            Some(line) => unshown(line, &recipe.folder, &[("ORCA_RECIPE_ID", &recipe.id)], "").map(Some),
            None => Ok(None),
        });
        if let Some(revision) = prepared.as_ref().ok().and_then(Option::as_ref).and_then(prepared_revision) {
            shared.recipes.prepared(repo, revision);
        }
        if let Some(said) = prepare_said(repo, prepared) {
            trace::event(said);
        }
    }
}

// What a repo's `prepare` run comes to in the log, None for a recipe without one or with
// nothing to do, as on most pulls.
pub(crate) fn prepare_said(repo: &str, prepared: Result<Option<Output>, Error>) -> Option<trace::Host> {
    let error = match prepared {
        Ok(None) => return None,
        Ok(Some(out)) if out.ok => return Some(trace::Host::Prepared { repo: repo.into() }),
        Ok(Some(out)) if out.code == Some(NOTHING_TO_DO) => return None,
        Ok(Some(out)) => Error::Failed(format!("its prepare failed{}", out.code.map_or(String::new(), |code| format!(" (exit {code})")))),
        Err(error) => error,
    };
    Some(trace::Host::NotPrepared { repo: Some(repo.into()), error })
}

// The revision a `prepare` that ran or had nothing to do printed, of the shape the Durable
// Object takes.
pub(crate) fn prepared_revision(out: &Output) -> Option<String> {
    if !out.ok && out.code != Some(NOTHING_TO_DO) {
        return None;
    }
    let printed: Value = serde_json::from_str(out.stdout.trim()).ok()?;
    printed["revision"].as_str().filter(|revision| plain_revision(revision)).map(String::from)
}

// A recipe line's exit when it had nothing to do, as `tolkster`'s own.
const NOTHING_TO_DO: i32 = 3;

// A make that fails after `create` costs money, so none starts within this of one
// (`Progress::failed`).
const COOL_DOWN: Duration = Duration::from_secs(10 * 60);

// A make or wake's claim (`Making::alone`), refused too when a make ended since the ask's
// settings were read, at `seen`, as they lack its machine, or in a cool-down. A refusal says
// why, but for a stale ask, which is made again at once.
fn claim(shared: &Arc<Shared>, seen: usize) -> Result<Making, Option<trace::Declined>> {
    let making = Making::alone(shared).ok_or(Some(trace::Declined::Running))?;
    let (failed, ended) = (shared.recipes.failed(), shared.recipes.ended());
    match fresh(seen, ended, failed, Instant::now()) {
        true => Ok(making),
        false if ended == seen => Err(Some(trace::Declined::CoolDown)),
        false => Err(None),
    }
}

// Whether an ask whose settings were read at `seen` may make one, `ended` makes having ended
// and the last that failed at `failed`.
pub(crate) fn fresh(seen: usize, ended: usize, failed: Option<Instant>, now: Instant) -> bool {
    ended == seen && failed.is_none_or(|at| now.saturating_duration_since(at) >= COOL_DOWN)
}


// `add` for `repo`'s recipe opt-in with no prompt, naming the machine itself and setting it up
// in the recipe's `projectRoot`: Some(its name), or None when the repo has no opt-in or its
// `machines` are all made. A failure names the step that failed, its machine already destroyed.
// Its name is said once it is marked, as `issue` asked for it.
pub fn make(agent: &Agent, shared: &Shared, repo: &str, issue: &str) -> Result<Option<String>, Error> {
    let files = files()?;
    realtime(&files)?;
    unstamped(prebuilt::commit())?;
    let config = settings(&files)?;
    let Some(machines) = config.repos.entry(repo).and_then(|entry| entry.recipe.clone()) else { return Ok(None) };
    if Author::read(&mut run_command).is_none() {
        return Err(Error::Failed(NO_AUTHOR.into()));
    }
    let mut run = run_command;
    let recipe = find(&mut run, &machines.id, Some(repo))?;
    if !recipe_passes(&mut run, &recipe.id, &recipe.folder) {
        return Err(Error::Failed(format!("Orca's checks fail the recipe {}", recipe.id)));
    }
    let state = State::open(agent, &files)?;
    let path = machines_file()?;
    let mut entries = |change: &mut Change| written(&path, change);
    let mut environment: Option<String> = None;
    let mut setting_up = |run: &mut Run, host: &Host, checkout: (&str, &str)| {
        environment = Some(host.id.clone());
        let steps = set_up(agent, &files, run, host, checkout.1, Some(checkout), false)?;
        if passed(&steps) {
            shared.pacing.admitted(&host.id);
        }
        Ok(steps)
    };
    let mut marked = |name: &str, counted: usize| trace::event(trace::Host::Making { repo: repo.into(), name: name.into(), counted, machines: machines.machines, issue: issue.into() });
    let Some((name, steps)) = made(&mut run, &mut unshown, &recipe, Claim::Capped(machines.machines), &mut entries, &mut setting_up, &mut marked)? else { return Ok(None) };
    match steps.iter().find(|step| step.verdict == "fail") {
        None => Ok(Some(name)),
        Some(failed) => {
            revoke(&state, &environment);
            Err(Error::Failed(format!("{name}'s {}: {}", failed.step, failed.reason)))
        }
    }
}

// The machine `name` from `recipe` for `host add --recipe`, its lines' stderr on the owner's
// terminal: its steps, or None when a machines.json entry or Orca environment has the name.
pub(crate) fn make_named(agent: &Agent, files: &Files, run: &mut Run, recipe: &Recipe, name: &str) -> Result<Option<Vec<Step>>, Error> {
    let state = State::open(agent, files)?;
    let path = machines_file()?;
    let mut entries = |change: &mut Change| written(&path, change);
    // The environment the machine paired as, whose ID its credential is minted for.
    let mut environment: Option<String> = None;
    let mut setting_up = |run: &mut Run, host: &Host, checkout: (&str, &str)| {
        environment = Some(host.id.clone());
        set_up(agent, files, run, host, checkout.1, Some(checkout), false)
    };
    let Some((_, steps)) = made(run, &mut script, recipe, Claim::Named(name), &mut entries, &mut setting_up, &mut |_, _| {})? else { return Ok(None) };
    // A host whose machine is gone keeps no credential, whether or not it joined.
    if !passed(&steps) {
        revoke(&state, &environment);
    }
    Ok(Some(steps))
}

// The name a run marks: the owner's, or the next free one while the repo is under its
// `machines`.
pub(crate) enum Claim<'a> {
    Named(&'a str),
    Capped(i64),
}

// `provision` for a Claim's name. A mark in machines.json, with the recipe and repo it counts
// against and what `host rm` runs, goes in before `create` and gives way to the machine's
// entry. It comes out with any failure that destroys the machine or ends before there is one,
// and stays while one may be left; `marked` is told its name and the repo's machines counted,
// itself included, once it is in. None when the name is taken or the cap is full.
pub(crate) fn made(run: &mut Run, script: &mut Script, recipe: &Recipe, claim: Claim, entries: &mut Entries, set_up: &mut SetUp, marked: &mut dyn FnMut(&str, usize)) -> Result<Option<(String, Vec<Step>)>, Error> {
    let taken: Vec<String> = paired(run)?.into_iter().map(|host| host.name).collect();
    let mark = json!({ "recipe": recipe.id, "repo": recipe.repo, "folder": recipe.folder, "destroy": recipe.destroy });
    let pick = |config: &Map<String, Value>| match claim {
        Claim::Named(name) => (!named(config, name, &taken)).then(|| name.to_string()),
        Claim::Capped(cap) => free_name(config, &recipe.repo, cap, &taken),
    };
    let mut name = None;
    let mut counted = 0;
    entries(&mut |config| match pick(config) {
        Some(free) => {
            let change = recorded(config, &free, None, Some(&mark));
            counted = counting(config, &recipe.repo) + 1;
            name = Some(free);
            change
        }
        None => (Map::new(), false),
    })?;
    let Some(name) = name else { return Ok(None) };
    marked(&name, counted);
    let mut record = |entry: Option<&Value>| match entries(&mut |config| recorded(config, &name, Some(&mark), entry))? {
        true => Ok(()),
        false => Err(Error::Failed(format!("machines.json no longer marks {name}"))),
    };
    let provisioned = provision(run, script, recipe, &name, &mut record, set_up);
    let gone = match &provisioned {
        Ok(steps) => steps.last().is_some_and(|last| last.step == "destroyed" && last.verdict == "done"),
        Err(_) => true,
    };
    if gone {
        let _ = entries(&mut |config| match config.get(&name) == Some(&mark) {
            true => recorded(config, &name, None, None),
            false => (Map::new(), false),
        });
    }
    provisioned.map(|steps| Some((name, steps)))
}

// The first `<repo>-<n>` no machines.json entry or paired environment has, ignoring case, or
// None once the repo's entries, marks included, reach `cap`.
pub(crate) fn free_name(machines: &Map<String, Value>, repo: &str, cap: i64, taken: &[String]) -> Option<String> {
    if counting(machines, repo) as i64 >= cap {
        return None;
    }
    // One of the first names past every one taken is free.
    let mut names = (1..=machines.len() + taken.len() + 1).map(|n| format!("{repo}-{n}"));
    names.find(|name| !named(machines, name, taken)).filter(|name| plain_host(name))
}

// The machines.json entries, marks included, that count against `repo`'s `machines`.
fn counting(machines: &Map<String, Value>, repo: &str) -> usize {
    machines.values().filter(|entry| entry["repo"].as_str().is_some_and(|of| of.eq_ignore_ascii_case(repo))).count()
}

// Whether a machines.json entry or a paired environment has `name`, ignoring case.
fn named(machines: &Map<String, Value>, name: &str, taken: &[String]) -> bool {
    machines.keys().chain(taken).any(|other| other.eq_ignore_ascii_case(name))
}

// machines.json as it is, empty when there is none.
pub(crate) fn recorded_machines() -> Result<Map<String, Value>, Error> {
    let path = machines_file()?;
    match crate::cli::read_if_there(&path)? {
        Some(text) => serde_json::from_str(&text).context(path.display()),
        None => Ok(Map::new()),
    }
}

// `merge_json_file` takes this process's writes one at a time, so two of `listen`'s threads
// never both read the cap before either marks. Private, as `userData` is the provider's handle
// on the machine.
fn written(machines: &Path, change: &mut Change) -> Result<bool, Error> {
    let mut done = false;
    merge_json_file(machines, true, |old| {
        let (change, made) = change(old);
        done = made;
        change
    })?;
    Ok(done)
}

// After `host rm` revoked `host`: its machine destroyed when a recipe made it, Some(whether it
// was), or None for a host no recipe made. It runs the `destroy` line recorded with the
// machine, in the folder recorded with it, never what orca.yaml says today.
pub(crate) fn destroy(host: &str) -> Result<Option<bool>, Error> {
    let Some(kept) = recorded_machines()?.get(host).cloned() else { return Ok(None) };
    let recipe = destroyer(host, &kept)?;
    let path = machines_file()?;
    let mut record = |entry: Option<&Value>| keep(&path, host, entry);
    Ok(Some(teardown(&mut run_command, &mut script, &recipe, host, &kept, (true, true), &mut record).verdict == "done"))
}

// The recipe `kept`, `host`'s entry, was recorded with, enough to run its `destroy`.
pub(crate) fn destroyer(host: &str, kept: &Value) -> Result<Recipe, Error> {
    let text = |key: &str| kept[key].as_str().map(String::from);
    let (Some(id), Some(folder), Some(destroy)) = (text("recipe"), text("folder"), text("destroy")) else {
        return Err(Error::Failed(format!("machines.json's entry for {host} has another shape: destroy its machine by hand, then take the entry out")));
    };
    Ok(Recipe { id, repo: text("repo").unwrap_or_default(), github: String::new(), folder, create: String::new(), destroy, sleep: None, prepare: None, update: None })
}

// From `create` to a host `set_up` made, or, after a failure past `create`, to a destroyed
// machine.
pub(crate) fn provision(
    run: &mut Run,
    script: &mut Script,
    recipe: &Recipe,
    name: &str,
    record: &mut Record,
    set_up: &mut SetUp,
) -> Result<Vec<Step>, Error> {
    let made = script(&recipe.create, &recipe.folder, &env(recipe, name), "")?;
    if !made.ok {
        return Err(Error::Failed(format!("the recipe's create failed{}", made.code.map_or(String::new(), |code| format!(" (exit {code})")))));
    }
    let printed = Printed::of(&made.stdout);
    // `destroy` finds the machine by it, and a large one is the recipe's to keep: without one,
    // nothing is recorded or paired, and `destroy` is tried once with what there is.
    let user_data = printed.user_data.as_ref().filter(|data| data.to_string().len() <= USER_DATA_MAX);
    let mut kept = json!({ "recipe": recipe.id, "repo": recipe.repo, "folder": recipe.folder, "destroy": recipe.destroy, "projectRoot": printed.project_root, "userData": user_data });
    if let Some((suspend, resume)) = &recipe.sleep {
        kept["suspend"] = json!(suspend);
        kept["resume"] = json!(resume);
    }
    if user_data.is_none() {
        let why = format!("the recipe's create printed no userData object of up to {USER_DATA_MAX} bytes");
        let tried = json!({ "projectRoot": printed.project_root, "userData": printed.user_data });
        return Ok(vec![step("machine", "fail", why), teardown(run, script, recipe, name, &tried, (false, false), record)]);
    }
    let recorded = record(Some(&kept)).is_ok();
    let mut paired_here = false;
    let outcome = match recorded {
        true => onboard(run, recipe, name, &printed, &mut paired_here, set_up),
        false => Err(Error::Failed("the machine couldn't be recorded in machines.json".into())),
    };
    let mut steps = outcome.unwrap_or_else(|error| vec![step("machine", "fail", error.to_string())]);
    if !passed(&steps) {
        steps.push(teardown(run, script, recipe, name, &kept, (recorded, paired_here), record));
    }
    Ok(steps)
}

const USER_DATA_MAX: usize = 2048;

// What a recipe's `create` or `resume` prints, as Orca's recipe protocol has it: the pairing
// code, the image's checkout, and `userData`, the recipe's own object, open to it, which its
// later scripts get back as it is. Each field is read alone, as `create` has run and paid for a
// machine whatever it printed: one of another shape is none, and costs the others nothing.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Printed {
    #[serde(default, deserialize_with = "text")]
    pairing_code: Option<String>,
    #[serde(default, deserialize_with = "text")]
    pub project_root: Option<String>,
    #[serde(default, deserialize_with = "object")]
    pub user_data: Option<Value>,
}

impl Printed {
    // Stdout that is no JSON object holds none of it. Read as a `Value` first, which keeps a
    // repeated key's last value where a struct refuses the whole object: a pairing code read
    // as none would go unmasked in `unpaired`.
    pub(crate) fn of(stdout: &str) -> Printed {
        serde_json::from_str::<Value>(stdout.trim()).ok().and_then(|value| Printed::deserialize(value).ok()).unwrap_or_default()
    }
}

fn text<'de, D: Deserializer<'de>>(field: D) -> Result<Option<String>, D::Error> {
    Ok(Value::deserialize(field)?.as_str().map(String::from))
}

fn object<'de, D: Deserializer<'de>>(field: D) -> Result<Option<Value>, D::Error> {
    Ok(Some(Value::deserialize(field)?).filter(Value::is_object))
}

fn onboard(run: &mut Run, recipe: &Recipe, name: &str, printed: &Printed, paired_here: &mut bool, set_up: &mut SetUp) -> Result<Vec<Step>, Error> {
    let shape = || Error::Failed("the recipe's create printed no pairingCode and absolute projectRoot of plain characters".into());
    let code = printed.pairing_code.as_deref().filter(|code| !code.is_empty()).ok_or_else(shape)?;
    let root = printed.project_root.as_deref().filter(|root| plain_destination(root)).ok_or_else(shape)?;
    // Orca may pair it though the call fails: an environment of the name is this run's only
    // when none had it just before.
    if paired(run)?.iter().any(|host| host.name == name) {
        return Err(Error::Failed(format!("Orca has an environment named {name} made since host add began")));
    }
    *paired_here = true;
    // Orca's error could repeat the code.
    pair(run, name, code).map_err(|_| Error::Failed("Orca couldn't pair with the machine".into()))?;
    let host = paired(run)?.into_iter().find(|host| host.name == name).ok_or_else(|| Error::Failed(format!("Orca lists no environment named {name} after pairing")))?;
    // Orca opens a terminal there only in a folder set up in it, and the image's checkout is one.
    if setup_at(run, &host.id, root)?.is_none() {
        import_folder(run, &host.id, &project_id(&recipe.github), root, "git", &recipe.repo)?;
    }
    set_up(run, &host, (&recipe.github, root))
}

// The recipe's `destroy`, given what `create` printed less its pairing code, then the machine's
// pairing and its entry. `ours` is (whether the entry was recorded, whether the pairing is this
// machine's): another's is never touched. A failed `destroy` keeps both, so `host rm` can run
// it again.
pub(crate) fn teardown(run: &mut Run, script: &mut Script, recipe: &Recipe, name: &str, kept: &Value, ours: (bool, bool), record: &mut Record) -> Step {
    let (recorded, paired_here) = ours;
    trace::event(trace::Host::Destroying { name: name.into() });
    let destroyed = script(&recipe.destroy, &recipe.folder, &env(recipe, name), &payload(recipe, kept)).is_ok_and(|out| out.ok);
    if !destroyed {
        let then = if recorded { format!("tolkster host rm {name} runs it again") } else { "nothing recorded the machine, so delete it by hand".into() };
        trace::event(trace::Host::NotDestroyed { name: name.into() });
        return step("destroyed", "fail", format!("the recipe's destroy failed: {then}"));
    }
    trace::event(trace::Host::Destroyed { name: name.into() });
    if paired_here && let Some(host) = paired(run).ok().and_then(|hosts| hosts.into_iter().find(|host| host.name == name)) {
        let _ = unpair(run, &host.id);
    }
    match !recorded || record(None).is_ok() {
        true => step("destroyed", "done", "by the recipe's destroy"),
        false => step("destroyed", "fail", format!("destroyed, but machines.json still names {name}: take it out")),
    }
}

fn env<'a>(recipe: &'a Recipe, name: &'a str) -> [(&'a str, &'a str); 2] {
    [("ORCA_RECIPE_ID", recipe.id.as_str()), ("ORCA_VM_INSTANCE_ID", name)]
}

// What a lifecycle line after `create` reads on stdin: what `create` printed, less its pairing
// code, as `kept` holds it.
fn payload(recipe: &Recipe, kept: &Value) -> String {
    json!({ "recipeId": recipe.id, "recipeResult": { "schemaVersion": 1, "projectRoot": kept["projectRoot"], "userData": kept["userData"] } }).to_string()
}

// Writes `name`'s machines.json entry, refusing one already there, or with None takes it out.
fn keep(machines: &Path, name: &str, entry: Option<&Value>) -> Result<(), Error> {
    let mut taken = false;
    merge_json_file(machines, true, |old| {
        let (change, done) = recorded(old, name, None, entry);
        taken = !done;
        change
    })?;
    match taken {
        true => Err(Error::Failed(format!("machines.json already names {name}"))),
        false => Ok(()),
    }
}

// The change to machines.json that sets `name`'s entry, or with None takes it out, and whether
// it is made. An entry is set only over `over`, None for none: another is never replaced, as it
// is all that can destroy its machine, and only a run's own mark gives way to its entry.
pub(crate) fn recorded(machines: &Map<String, Value>, name: &str, over: Option<&Value>, entry: Option<&Value>) -> (Map<String, Value>, bool) {
    match entry {
        Some(_) if machines.get(name) != over => (Map::new(), false),
        entry => (Map::from_iter([(name.into(), entry.cloned().unwrap_or(Value::Null))]), true),
    }
}

// The recipe `id` in `repo`'s orca.yaml, or with None in the one repo Orca has that has it.
pub(crate) fn find(run: &mut Run, id: &str, repo: Option<&str>) -> Result<Recipe, Error> {
    let github = github_repos(run)?;
    let found: Vec<Recipe> = repo_folders(run)?
        .into_iter()
        .filter(|(name, _)| repo.is_none_or(|repo| repo.eq_ignore_ascii_case(name)))
        .filter_map(|(name, folder)| {
            let Lines { create, destroy, sleep, prepare, update } = recipe_lines(&std::fs::read_to_string(Path::new(&folder).join("orca.yaml")).ok()?, id)?;
            let github = github.iter().find(|(n, repo)| *n == name && split_repo(repo).is_some())?.1.clone();
            Some(Recipe { id: id.into(), repo: name, github, folder, create, destroy, sleep, prepare, update })
        })
        .collect();
    let mut found = found.into_iter();
    match (found.next(), found.next()) {
        (Some(recipe), None) if plain_host(&recipe.repo) => Ok(recipe),
        (Some(_), None) => Err(Error::Usage("the recipe's repo needs a plain display name in Orca: letters, digits, '_', '.' and '-'".into())),
        (None, _) => Err(Error::Usage(format!("no {} has the recipe {id} in its orca.yaml, with a destroy, and an origin on GitHub", repo.map_or("repo in Orca".into(), |repo| format!("repo {repo}"))))),
        (Some(_), Some(_)) => Err(Error::Usage(format!("more than one repo has the recipe {id}: pick one with --repo"))),
    }
}

// A recipe's lines, as `Recipe` holds them.
#[derive(Debug, PartialEq)]
pub(crate) struct Lines {
    pub create: String,
    pub destroy: String,
    pub sleep: Option<(String, String)>,
    pub prepare: Option<String>,
    pub update: Option<String>,
}

// A recipe's lines, read as Orca writes the list: items of plain `key: value` lines under
// `environmentRecipes:`, with its legacy `command` and `cleanup`; a line nested deeper in an
// item is skipped, so it can't pass for a recipe's. None without a `destroy` (`none` is none),
// as a paid machine must be one host add can take down, for an id two items share, or for a
// `create` or `destroy` it can't read as one plain scalar; `suspend`, `resume`, `prepare` and
// `update` so are none.
pub(crate) fn recipe_lines(yaml: &str, id: &str) -> Option<Lines> {
    let header = |line: &str| line.split(" #").next().unwrap_or_default().trim_end() == "environmentRecipes:";
    let mut items: Vec<Vec<(&str, String)>> = Vec::new();
    let mut at: Option<usize> = None;
    for line in yaml.lines().skip_while(|line| !header(line)).skip(1) {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if !line.starts_with([' ', '-']) {
            break;
        }
        let entry = match trimmed.strip_prefix("- ") {
            Some(entry) if *at.get_or_insert(indent) == indent => {
                items.push(Vec::new());
                entry
            }
            _ if at.is_some_and(|at| indent == at + 2) => trimmed,
            _ => continue,
        };
        if let (Some(item), Some((key, value))) = (items.last_mut(), entry.split_once(':')) {
            item.push((key.trim(), scalar(value)));
        }
    }
    let mut matching = items.iter().filter(|item| item.iter().any(|(key, value)| *key == "id" && value == id));
    let (Some(item), None) = (matching.next(), matching.next()) else { return None };
    let get = |key: &str| item.iter().find(|(k, _)| *k == key).map(|(_, value)| value.clone()).filter(|value| !value.is_empty() && !value.starts_with(['|', '>']));
    let create = get("create").or_else(|| get("command"))?;
    let destroy = get("destroy").or_else(|| get("cleanup")).filter(|destroy| destroy != "none")?;
    Some(Lines { create, destroy, sleep: get("suspend").zip(get("resume")), prepare: get("prepare"), update: get("update") })
}

fn scalar(value: &str) -> String {
    let value = value.trim();
    ['"', '\''].iter().find_map(|quote| value.strip_prefix(*quote)?.strip_suffix(*quote)).unwrap_or(value).to_string()
}

fn script(line: &str, folder: &str, env: &[(&str, &str)], stdin: &str) -> Result<Output, Error> {
    shown(line, folder, env, stdin, Stdio::inherit())
}

// `script` for `listen`, whose terminal is its log: what the line writes on stderr goes to the
// disk log alone, its last lines, less a pairing code it printed. Stderr goes to a file, never a
// pipe, so a process the line leaves running can't hold the make, and its claim, open.
fn unshown(line: &str, folder: &str, env: &[(&str, &str)], stdin: &str) -> Result<Output, Error> {
    // Per process, as the file's name holds the process's ID.
    static RUNS: AtomicUsize = AtomicUsize::new(0);
    let path = std::env::temp_dir().join(format!("tolkster-recipe-{}-{}.err", std::process::id(), RUNS.fetch_add(1, Ordering::SeqCst)));
    // A file that can't be made costs only the log's lines, never the make.
    let (stderr, made) = match private(&path) {
        Ok(file) => (Stdio::from(file), true),
        Err(_) => (Stdio::null(), false),
    };
    let output = shown(line, folder, env, stdin, stderr);
    let said = if made { tail_of(&path).unwrap_or_default() } else { String::new() };
    if made {
        let _ = std::fs::remove_file(&path);
    }
    let output = output?;
    let name = ["ORCA_VM_INSTANCE_ID", "ORCA_RECIPE_ID"].iter().find_map(|wanted| env.iter().find(|(key, _)| key == wanted)).map_or("", |(_, name)| *name);
    trace::recipe_stderr(name, &unpaired(&said, &output.stdout));
    Ok(output)
}

// A new file only this user reads, never one already there, so another can't read the stderr
// before it is masked nor point the name elsewhere.
fn private(path: &Path) -> Result<std::fs::File, Error> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path).context(path.display())
}

// The file's last 64 KB, which hold more than the lines the log keeps, from a line's start:
// a cut inside one could split a pairing code past masking.
fn tail_of(path: &Path) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(64 * 1024)))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(whole_lines(&bytes, size > 64 * 1024))
}

pub(crate) fn whole_lines(bytes: &[u8], cut: bool) -> String {
    let from = if cut { bytes.iter().position(|b| *b == b'\n').map_or(bytes.len(), |at| at + 1) } else { 0 };
    String::from_utf8_lossy(&bytes[from..]).into_owned()
}

// `stderr` with the pairing code `stdout` printed, if any, masked.
pub(crate) fn unpaired(stderr: &str, stdout: &str) -> String {
    match Printed::of(stdout).pairing_code.filter(|code| !code.is_empty()) {
        Some(code) => stderr.replace(&code, "<pairing code>"),
        None => stderr.to_string(),
    }
}

fn shown(line: &str, folder: &str, env: &[(&str, &str)], stdin: &str, stderr: Stdio) -> Result<Output, Error> {
    let mut child = shell(line).current_dir(folder).envs(env.iter().copied()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(stderr).spawn().context("starting the recipe's line")?;
    if let Some(mut input) = child.stdin.take() {
        let _ = input.write_all(stdin.as_bytes());
    }
    let output = child.wait_with_output().context("running the recipe's line")?;
    Ok(Output { ok: output.status.success(), code: output.status.code(), stdout: String::from_utf8_lossy(&output.stdout).into_owned(), stderr: String::new() })
}

// The shell Orca runs a recipe's line in: cmd on Windows, else sh.
#[cfg(windows)]
fn shell(line: &str) -> Process {
    use std::os::windows::process::CommandExt;
    let mut command = Process::new("cmd.exe");
    command.args(["/d", "/s", "/c"]).raw_arg(format!("\"{line}\""));
    command
}

#[cfg(not(windows))]
fn shell(line: &str) -> Process {
    let mut command = Process::new("sh");
    command.args(["-c", line]);
    command
}

pub(crate) mod retire;
pub(crate) mod sleep;
pub(crate) mod update;

#[cfg(test)]
mod tests;
