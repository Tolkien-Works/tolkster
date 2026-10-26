use std::cell::RefCell;
use std::sync::Arc;

use serde_json::{Map, json};

use super::*;
use crate::exec::Output;
use crate::runtime::Shared;

fn asleep_entry(environment: &str) -> Value {
    json!({ "recipe": "cloud", "repo": "tolkster", "folder": "/src/tolkster", "destroy": "./destroy.sh", "suspend": "./suspend.sh", "resume": "./resume.sh", "projectRoot": "/home/node/tolkster", "userData": { "resourceId": "i-1" }, "asleep": true, "environment": environment })
}

fn mark() -> Value {
    json!({ "recipe": "cloud", "repo": "tolkster", "folder": "/src/tolkster", "destroy": "./destroy.sh" })
}

// This machine's Orca, pairing `box` as `host` unless None, the recipe's lines as they ran,
// machines.json, the credentials revoked and what `reaped` was asked, answering `reaps`.
struct World {
    // The context of the `listen` it runs in.
    shared: Arc<Shared>,
    host: Option<String>,
    twin: bool,
    reaps: bool,
    destroys: bool,
    scripts: Vec<String>,
    unpaired: Vec<String>,
    revoked: Vec<String>,
    asked: Vec<(String, bool)>,
    config: Map<String, Value>,
}

fn world(entry: Value, host: Option<&str>) -> RefCell<World> {
    let config = Map::from_iter([("box".into(), entry)]);
    RefCell::new(World { shared: Arc::default(), host: host.map(String::from), twin: false, reaps: true, destroys: true, scripts: Vec::new(), unpaired: Vec::new(), revoked: Vec::new(), asked: Vec::new(), config })
}

fn retire(world: &RefCell<World>) -> Result<bool, Error> {
    let mut run = |program: &str, args: &[&str], _: Option<&str>| -> Result<Output, Error> {
        assert_eq!(program, "orca");
        let mut w = world.borrow_mut();
        let out = match args {
            ["host", "list", "--json"] => {
                let twin = w.twin.then(|| "e-twin".to_string());
                let paired = w.host.iter().chain(twin.iter()).map(|id| json!({ "kind": "environment", "id": id, "name": "box" }));
                let hosts: Vec<Value> = std::iter::once(json!({ "kind": "local", "id": "local", "name": "Local" })).chain(paired).collect();
                json!({ "result": { "hosts": hosts } })
            }
            // Orca hears the host only once its `resume` has run.
            ["project", "setups", "--host", _, "--json"] => {
                let setups = if w.scripts.iter().any(|line| line == "./resume.sh") { vec![json!({ "projectId": "p1", "setupState": "ready", "path": "/home/node/tolkster", "kind": "git" })] } else { Vec::new() };
                json!({ "result": { "setups": setups } })
            }
            ["environment", "rm", "--environment", id, "--json"] => {
                w.unpaired.push(id.to_string());
                json!({ "result": {} })
            }
            _ => panic!("{}", args.join(" ")),
        };
        Ok(Output::ok(out.to_string()))
    };
    let mut script = |line: &str, _: &str, _: &[(&str, &str)], _: &str| {
        let mut w = world.borrow_mut();
        w.scripts.push(line.into());
        Ok(match line {
            "./destroy.sh" if !w.destroys => Output::failed(1, ""),
            "./resume.sh" => Output::ok(json!({ "projectRoot": "/home/node/tolkster", "userData": { "resourceId": "i-2" } }).to_string()),
            _ => Output::ok(""),
        })
    };
    let mut entries = |change: &mut Change| {
        let mut w = world.borrow_mut();
        let (changed, made) = change(&w.config);
        w.config.extend(changed);
        Ok(made)
    };
    let mut reaped = |host: &str, reachable: bool| {
        let mut w = world.borrow_mut();
        w.asked.push((host.into(), reachable));
        Ok(w.reaps)
    };
    let mut revoke = |host: &str| {
        world.borrow_mut().revoked.push(host.into());
        Ok(())
    };
    let shared = Arc::clone(&world.borrow().shared);
    retired(&shared, &mut run, &mut script, "box", &mut entries, &mut reaped, &mut revoke, &mut || {})
}

// An asleep machine is woken with placement stopped there, its worktrees reaped on it, then
// revoked, destroyed by its recorded line with what its `resume` printed, unpaired and dropped.
#[test]
fn an_asleep_machine_is_woken_reaped_then_destroyed() {
    let world = world(asleep_entry("e-retire-1"), Some("e-retire-1"));
    assert!(retire(&world).unwrap());
    let w = world.borrow();
    assert_eq!(w.scripts, ["./resume.sh", "./destroy.sh"]);
    assert_eq!(w.asked, [("e-retire-1".to_string(), true)]);
    assert_eq!((w.revoked.as_slice(), w.unpaired.as_slice()), (["e-retire-1".to_string()].as_slice(), ["e-retire-1".to_string()].as_slice()));
    assert_eq!(w.config["box"], Value::Null);
    assert!(w.shared.pacing.napping("e-retire-1"));
}

// A worktree its reap kept keeps the machine: it goes back to sleep, its credential and
// entry as they were. A destroy that fails, after the revoke, puts it back to sleep too, its
// entry kept for the next try.
#[test]
fn a_kept_worktree_or_a_failed_destroy_puts_the_machine_back_to_sleep() {
    let world = world(asleep_entry("e-retire-2"), Some("e-retire-2"));
    world.borrow_mut().reaps = false;
    assert!(!retire(&world).unwrap());
    let w = world.borrow();
    assert_eq!(w.scripts, ["./resume.sh", "./suspend.sh"]);
    assert!(w.revoked.is_empty() && w.unpaired.is_empty());
    assert_eq!(w.config["box"]["asleep"], true);
    drop(w);
    let world = self::world(asleep_entry("e-retire-3"), Some("e-retire-3"));
    world.borrow_mut().destroys = false;
    assert!(retire(&world).is_err());
    let w = world.borrow();
    assert_eq!(w.scripts, ["./resume.sh", "./destroy.sh", "./suspend.sh"]);
    assert_eq!(w.config["box"]["asleep"], true);
}

// An awake machine is reaped with placement stopped there and needs no wake; kept, it takes
// work again.
#[test]
fn an_awake_machine_is_reaped_without_a_wake_and_kept_takes_work_again() {
    let mut awake = asleep_entry("e-retire-4");
    let entry = awake.as_object_mut().unwrap();
    entry.remove("asleep");
    entry.remove("environment");
    let world = world(awake.clone(), Some("e-retire-4"));
    world.borrow_mut().reaps = false;
    assert!(!retire(&world).unwrap());
    assert!(world.borrow().scripts.is_empty());
    assert!(!world.borrow().shared.pacing.napping("e-retire-4"));
    let world = self::world(awake, Some("e-retire-4"));
    assert!(retire(&world).unwrap());
    assert_eq!(world.borrow().scripts, ["./destroy.sh"]);
}

// A mark is destroyed as it is, nothing asked of a host it never had. An asleep machine Orca no
// longer pairs as the host it slept as can't be woken: it goes only when no worker's record
// names that host, and another paired under its name since is never touched. One with no such
// host recorded, or awake and unpaired, is left alone. A machine already gone is gone.
#[test]
fn a_mark_goes_as_it_is_and_an_unpaired_machine_only_with_no_worker_on_it() {
    let world = world(mark(), None);
    assert!(retire(&world).unwrap());
    let w = world.borrow();
    assert_eq!(w.scripts, ["./destroy.sh"]);
    assert!(w.asked.is_empty() && w.revoked.is_empty());
    assert_eq!(w.config["box"], Value::Null);
    drop(w);
    let world = self::world(asleep_entry("e-retire-5"), Some("e-other"));
    world.borrow_mut().reaps = false;
    assert!(!retire(&world).unwrap());
    assert_eq!(world.borrow().asked, [("e-retire-5".to_string(), false)]);
    assert!(world.borrow().scripts.is_empty());
    world.borrow_mut().reaps = true;
    assert!(retire(&world).unwrap());
    assert_eq!((world.borrow().scripts.as_slice(), world.borrow().revoked.as_slice()), (["./destroy.sh".to_string()].as_slice(), ["e-retire-5".to_string()].as_slice()));
    assert!(world.borrow().unpaired.is_empty());
    assert!(retire(&world).unwrap());
    let mut awake = asleep_entry("e-retire-6");
    awake["asleep"] = json!(false);
    let world = self::world(awake, None);
    assert!(!retire(&world).unwrap());
    assert!(world.borrow().asked.is_empty() && world.borrow().scripts.is_empty());
    let mut nowhere = asleep_entry("");
    nowhere.as_object_mut().unwrap().remove("environment");
    let world = self::world(nowhere, None);
    assert!(!retire(&world).unwrap());
    assert!(world.borrow().asked.is_empty() && world.borrow().scripts.is_empty());
}

// Two hosts paired under the machine's name leave which is which to the owner: nothing runs.
#[test]
fn two_hosts_under_its_name_keep_the_machine() {
    let world = world(asleep_entry("e-retire-7"), Some("e-retire-7"));
    world.borrow_mut().twin = true;
    assert!(!retire(&world).unwrap());
    let w = world.borrow();
    assert!(w.scripts.is_empty() && w.asked.is_empty() && w.revoked.is_empty() && w.unpaired.is_empty());
}
