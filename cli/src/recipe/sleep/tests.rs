use std::cell::RefCell;
use std::sync::Arc;

use serde_json::Map;

use super::*;
use crate::exec::Output;
use crate::runtime::Shared;

fn recipe() -> Recipe {
    let sleep = Some(("./suspend.sh".into(), "./resume.sh".into()));
    Recipe { id: "cloud".into(), repo: "tolkster".into(), github: "Tolkien-Works/tolkster".into(), folder: "/src/tolkster".into(), create: String::new(), destroy: "./destroy.sh".into(), sleep, prepare: None }
}

fn sleeping() -> Value {
    json!({ "recipe": "cloud", "repo": "tolkster", "folder": "/src/tolkster", "destroy": "./destroy.sh", "suspend": "./suspend.sh", "resume": "./resume.sh", "projectRoot": "/home/node/tolkster", "userData": { "resourceId": "i-1" }, "asleep": true })
}

// This machine's Orca, the recipe's lines as they ran (with their stdin) and machines.json.
// `answers`: the ask the host first answers after its `resume`, None for never.
struct World {
    // The context of the `listen` it runs in.
    shared: Arc<Shared>,
    resumes: bool,
    answers: Option<usize>,
    asked: usize,
    scripts: Vec<(String, String)>,
    config: Map<String, Value>,
}

fn world() -> RefCell<World> {
    let config = Map::from_iter([("box".into(), sleeping())]);
    RefCell::new(World { shared: Arc::default(), resumes: true, answers: Some(2), asked: 0, scripts: Vec::new(), config })
}

// Orca is never asked to pair or unpair: the machine stays paired as e1 while it sleeps.
fn woke(world: &RefCell<World>) -> Result<String, Error> {
    let mut run = |program: &str, args: &[&str], _: Option<&str>| -> Result<Output, Error> {
        assert_eq!(program, "orca");
        let mut w = world.borrow_mut();
        let out = match args {
            ["host", "list", "--json"] => json!({ "result": { "hosts": [{ "kind": "local", "id": "local", "name": "Local" }, { "kind": "environment", "id": "e1", "name": "box" }] } }),
            ["project", "setups", "--host", "runtime:e1", "--json"] => {
                w.asked += 1;
                if !w.answers.is_some_and(|first| w.asked >= first) {
                    return Ok(Output::failed(1, ""));
                }
                json!({ "result": { "setups": [{ "projectId": "p1", "setupState": "ready", "path": "/home/node/tolkster", "kind": "git" }] } })
            }
            _ => panic!("{}", args.join(" ")),
        };
        Ok(Output::ok(out.to_string()))
    };
    let mut script = |line: &str, folder: &str, _: &[(&str, &str)], stdin: &str| {
        assert_eq!(folder, "/src/tolkster");
        let mut w = world.borrow_mut();
        w.scripts.push((line.into(), stdin.into()));
        Ok(match line {
            "./resume.sh" if !w.resumes => Output::failed(4, ""),
            "./resume.sh" => Output::ok(json!({ "pairingCode": "orca://pair?code=fresh", "projectRoot": "/home/node/tolkster", "userData": { "resourceId": "i-2" } }).to_string()),
            _ => Output::ok(""),
        })
    };
    let mut entries = |change: &mut Change| {
        let mut w = world.borrow_mut();
        let (changed, made) = change(&w.config);
        w.config.extend(changed);
        Ok(made)
    };
    let shared = Arc::clone(&world.borrow().shared);
    woken(&shared, &mut run, &mut script, &recipe(), "box", &sleeping(), &mut entries, &mut || {})
}

fn lines(world: &RefCell<World>) -> Vec<String> {
    world.borrow().scripts.iter().map(|(line, _)| line.clone()).collect()
}

// A wake runs the recorded `resume` on what `create` printed, waits for Orca to hear the host
// it stayed paired as, then marks the entry awake, with where `resume` says the machine now
// is.
#[test]
fn a_wake_resumes_and_marks_the_machine_awake_once_it_answers() {
    let world = world();
    assert_eq!(woke(&world).unwrap(), "e1");
    assert_eq!(lines(&world), ["./resume.sh"]);
    let w = world.borrow();
    assert!(w.scripts[0].1.contains("\"resourceId\":\"i-1\""));
    assert_eq!(w.asked, 2);
    assert_eq!(w.config["box"]["asleep"], Value::Null);
    assert_eq!(w.config["box"]["userData"], json!({ "resourceId": "i-2" }));
}

// A machine its platform woke by itself, which Orca already hears, is marked awake with no
// `resume`, and a failed write runs no `suspend`.
#[test]
fn a_machine_orca_already_hears_is_marked_awake_without_a_resume() {
    let world = world();
    world.borrow_mut().answers = Some(1);
    assert_eq!(woke(&world).unwrap(), "e1");
    assert!(lines(&world).is_empty());
    let w = world.borrow();
    assert_eq!(w.asked, 1);
    assert_eq!(w.config["box"]["asleep"], Value::Null);
    assert_eq!(w.config["box"]["userData"], json!({ "resourceId": "i-1" }));
}

// A woken machine Orca never hears is put to sleep again, still asleep, with where `resume`
// said it is; one whose `resume` failed is left as it was.
#[test]
fn a_wake_that_fails_leaves_the_machine_asleep() {
    let world = world();
    world.borrow_mut().answers = None;
    assert!(woke(&world).is_err());
    assert_eq!(lines(&world), ["./resume.sh", "./suspend.sh"]);
    let w = world.borrow();
    assert!(w.scripts[1].1.contains("\"resourceId\":\"i-2\""));
    assert_eq!(w.config["box"]["asleep"], true);
    assert_eq!(w.config["box"]["userData"], json!({ "resourceId": "i-2" }));
    drop(w);
    let world = self::world();
    world.borrow_mut().resumes = false;
    assert!(woke(&world).is_err());
    assert_eq!(lines(&world), ["./resume.sh"]);
    assert_eq!(world.borrow().config["box"], sleeping());
}

// Only a made machine with both lines recorded sleeps; a mark never does.
#[test]
fn only_a_machine_with_both_lines_sleeps() {
    assert!(sleeper(&sleeping()).is_some_and(|(recipe, asleep)| asleep && recipe.sleep == Some(("./suspend.sh".into(), "./resume.sh".into()))));
    let mut awake = sleeping();
    awake["asleep"] = json!(false);
    assert!(sleeper(&awake).is_some_and(|(_, asleep)| !asleep));
    for key in ["resume", "userData"] {
        let mut lacking = sleeping();
        lacking.as_object_mut().unwrap().remove(key);
        assert!(sleeper(&lacking).is_none(), "{key}");
    }
}

fn slept(world: &RefCell<World>, host: &str, rests: bool, suspends: bool) -> Result<bool, Error> {
    // Each line with its stdin, and the entry as it stood while the line ran.
    let mut script = |line: &str, _: &str, _: &[(&str, &str)], stdin: &str| {
        let mut w = world.borrow_mut();
        let entry = w.config["box"].clone();
        w.scripts.push((line.into(), format!("{stdin} {entry}")));
        Ok(if suspends { Output::ok("") } else { Output::failed(1, "") })
    };
    let mut entries = |change: &mut Change| {
        let mut w = world.borrow_mut();
        let (changed, made) = change(&w.config);
        w.config.extend(changed);
        Ok(made)
    };
    let shared = Arc::clone(&world.borrow().shared);
    asleep(&shared, &mut script, "box", host, &mut entries, &mut || Ok(rests))
}

fn awake() -> RefCell<World> {
    let world = world();
    let mut awake = sleeping();
    awake.as_object_mut().unwrap().remove("asleep");
    world.borrow_mut().config = Map::from_iter([("box".into(), awake)]);
    world
}

// A machine whose agents rest is marked asleep, so its cap is 0, with the host it stays paired
// as, before its recorded `suspend` runs, and nothing asks Orca to unpair it; a busy one is
// left as it was.
#[test]
fn a_resting_machine_goes_to_sleep_and_a_busy_one_stays() {
    let world = awake();
    assert!(matches!(slept(&world, "e3", false, true), Ok(false)));
    assert!(world.borrow().scripts.is_empty());
    assert_eq!(world.borrow().config["box"]["asleep"], Value::Null);
    assert!(matches!(slept(&world, "e3", true, true), Ok(true)));
    let w = world.borrow();
    assert_eq!(w.scripts.iter().map(|(line, _)| line.as_str()).collect::<Vec<_>>(), ["./suspend.sh"]);
    assert!(w.scripts[0].1.contains("\"resourceId\":\"i-1\"") && w.scripts[0].1.contains("\"asleep\":true") && w.scripts[0].1.contains("\"environment\":\"e3\""), "{}", w.scripts[0].1);
    assert_eq!(w.config["box"]["asleep"], true);
    assert!(w.shared.pacing.napping("e3"));
    drop(w);
    // Asleep already: nothing to do.
    assert!(matches!(slept(&world, "e3", true, true), Ok(false)));
}

// A failed `suspend` marks the machine awake again.
#[test]
fn a_failed_suspend_marks_the_machine_awake_again() {
    let world = awake();
    let other = world.borrow_mut().config.remove("box").unwrap();
    world.borrow_mut().config.insert("box2".into(), other);
    assert!(matches!(slept(&world, "e3", true, true), Ok(false)), "no entry of its name");
    let world = awake();
    assert!(slept(&world, "e4", true, false).is_err());
    let w = world.borrow();
    assert_eq!(w.config["box"]["asleep"], Value::Null);
    assert!(!w.shared.pacing.napping("e4"));
}
