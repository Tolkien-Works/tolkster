use std::cell::RefCell;

use serde_json::{Map, json};

use super::*;
use crate::exec::Output;

fn sleeping() -> Value {
    json!({ "recipe": "cloud", "repo": "tolkster", "folder": "/src/tolkster", "destroy": "./destroy.sh", "suspend": "./suspend.sh", "resume": "./resume.sh", "projectRoot": "/home/node/tolkster", "userData": { "resourceId": "i-1", "revision": "4" }, "asleep": true, "environment": "e1" })
}

// The lines run with their stdin, and machines.json.
struct World {
    scripts: Vec<(String, String)>,
    config: Map<String, Value>,
}

fn update(world: &RefCell<World>, kept: &Value, out: Output) -> Result<bool, Error> {
    let mut out = Some(out);
    let mut script = |line: &str, folder: &str, env: &[(&str, &str)], stdin: &str| {
        assert_eq!(folder, "/src/now");
        assert!(env.contains(&("TOLKSTER_REVISION", "5")));
        world.borrow_mut().scripts.push((line.into(), stdin.into()));
        Ok(out.take().expect("one run"))
    };
    let mut entries = |change: &mut Change| {
        let mut w = world.borrow_mut();
        let (changed, made) = change(&w.config);
        w.config.extend(changed);
        Ok(made)
    };
    updated(&mut script, ("./update.sh", "/src/now"), "box", "5", kept, &mut entries)
}

fn world() -> RefCell<World> {
    RefCell::new(World { scripts: Vec::new(), config: Map::from_iter([("box".into(), sleeping())]) })
}

// An update runs, in the folder its line was read from and asking for the recipe's revision, on
// what `create` printed, and records the revision it printed, the machine still asleep and
// paired as it was, so its next wake boots the new image.
#[test]
fn an_update_records_the_new_revision_and_keeps_the_machine_asleep() {
    let world = world();
    let printed = json!({ "schemaVersion": 1, "userData": { "resourceId": "i-1", "revision": "5" } }).to_string();
    assert!(update(&world, &sleeping(), Output::ok(printed)).unwrap());
    let w = world.borrow();
    assert_eq!(w.scripts.len(), 1);
    assert_eq!(serde_json::from_str::<Value>(&w.scripts[0].1).unwrap()["recipeResult"]["userData"]["resourceId"], "i-1");
    let entry = &w.config["box"];
    assert_eq!(entry["userData"]["revision"], "5");
    assert_eq!((&entry["asleep"], &entry["environment"]), (&json!(true), &json!("e1")));
}

// A machine found running, a failure, an update that prints no userData and an entry changed
// since it was read each leave the entry as it was; one awake runs nothing.
#[test]
fn an_update_that_did_nothing_or_failed_leaves_the_entry() {
    for (out, ok) in [(Output::failed(3, ""), true), (Output::failed(1, ""), false), (Output::ok("{}"), false)] {
        let world = world();
        assert_eq!(update(&world, &sleeping(), out).is_ok_and(|updated| !updated), ok);
        assert_eq!(world.borrow().config["box"], sleeping());
    }
    let world = world();
    let mut read = sleeping();
    read["userData"]["revision"] = json!("3");
    assert!(update(&world, &read, Output::ok(json!({ "userData": { "resourceId": "i-1", "revision": "5" } }).to_string())).is_err());
    assert_eq!(world.borrow().config["box"], sleeping());
    let world = self::world();
    let mut awake = sleeping();
    awake.as_object_mut().unwrap().remove("asleep");
    assert!(!update(&world, &awake, Output::ok("")).unwrap());
    assert_eq!(world.borrow().scripts.len(), 0);
}
