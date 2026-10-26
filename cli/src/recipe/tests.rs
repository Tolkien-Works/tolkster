use std::cell::RefCell;

use super::*;

const CODE: &str = "orca://pair?code=s3cret";

// This machine's Orca and the recipe's scripts: what was paired, imported, unpaired and run.
#[derive(Default)]
struct World {
    created: String,
    pair_fails: bool,
    // A failed pair that Orca kept all the same.
    pair_sticks: bool,
    destroy_fails: bool,
    paired: bool,
    imported: bool,
    unpaired: bool,
    // Each script run: its line, its environment as `K=V` words, and its stdin.
    scripts: Vec<(String, String, String)>,
    entry: Option<Value>,
    // An entry another run of host add holds, which this one may not replace.
    taken: bool,
    // The paired environment's name.
    name: String,
}

fn recipe() -> Recipe {
    Recipe { id: "cloud".into(), repo: "tolkster".into(), github: "Tolkien-Works/tolkster".into(), folder: "/src/tolkster".into(), create: "./create.sh".into(), destroy: "./destroy.sh".into(), sleep: None, prepare: None }
}

fn created() -> String {
    json!({ "schemaVersion": 1, "pairingCode": CODE, "projectRoot": "/home/node/tolkster", "userData": { "provider": "x", "resourceId": "i-1" } }).to_string()
}

fn orca(world: &RefCell<World>) -> impl FnMut(&str, &[&str], Option<&str>) -> Result<Output, Error> + '_ {
    move |program, args, _| {
        assert_eq!(program, "orca");
        let mut w = world.borrow_mut();
        let out = match args {
            ["environment", "add", "--name", name, "--pairing-code", code, "--json"] => {
                assert_eq!(*code, CODE);
                w.name = name.to_string();
                if w.pair_fails {
                    w.paired = w.pair_sticks;
                    return Ok(Output { ok: false, code: Some(1), stdout: json!({ "error": { "code": "bad", "message": format!("bad code {CODE}") } }).to_string(), stderr: String::new() });
                }
                w.paired = true;
                json!({ "result": {} })
            }
            ["host", "list", "--json"] => {
                let mut hosts = vec![json!({ "kind": "local", "id": "local", "name": "Local" })];
                if w.paired {
                    hosts.push(json!({ "kind": "environment", "id": "e1", "name": w.name }));
                }
                json!({ "result": { "hosts": hosts } })
            }
            ["project", "setups", "--host", "runtime:e1", "--json"] => json!({ "result": { "setups": [] } }),
            ["project", "setup-existing-folder", "--project", "github:tolkien-works/tolkster", "--host", "runtime:e1", "--path", "/home/node/tolkster", "--kind", "git", "--display-name", "tolkster", "--json"] => {
                w.imported = true;
                json!({ "result": {} })
            }
            ["environment", "rm", "--environment", "e1", "--json"] => {
                w.unpaired = true;
                w.paired = false;
                json!({ "result": {} })
            }
            _ => panic!("{}", args.join(" ")),
        };
        Ok(Output::ok(out.to_string()))
    }
}

fn provisioned(world: &RefCell<World>, outcome: &'static str) -> Vec<Step> {
    let mut run = orca(world);
    let mut script = |line: &str, folder: &str, env: &[(&str, &str)], stdin: &str| {
        assert_eq!(folder, "/src/tolkster");
        let mut w = world.borrow_mut();
        w.scripts.push((line.into(), env.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(" "), stdin.into()));
        Ok(match line {
            "./create.sh" => Output::ok(w.created.clone()),
            _ if w.destroy_fails => Output::failed(1, ""),
            _ => Output::ok(""),
        })
    };
    let mut record = |entry: Option<&Value>| {
        let mut w = world.borrow_mut();
        if w.taken {
            return Err(Error::Failed("taken".into()));
        }
        w.entry = entry.cloned();
        Ok(())
    };
    let mut set_up = |_: &mut Run, host: &Host, checkout: (&str, &str)| {
        assert_eq!((host.id.as_str(), checkout), ("e1", ("Tolkien-Works/tolkster", "/home/node/tolkster")));
        Ok(vec![step("tolkster", "done", "fetched and joined"), step("checkout", outcome, "")])
    };
    provision(&mut run, &mut script, &recipe(), "box", &mut record, &mut set_up).unwrap()
}

fn verdicts(steps: &[Step]) -> Vec<(&str, &str)> {
    steps.iter().map(|s| (s.step.as_str(), s.verdict)).collect()
}

// A machine `create` made is paired, its checkout set up in Orca and handed to the steps, and
// remembered with its recipe and what destroys it, never its pairing code.
#[test]
fn a_recipe_host_is_paired_set_up_and_remembered_without_its_code() {
    let world = RefCell::new(World { created: created(), ..World::default() });
    let steps = provisioned(&world, "done");
    assert_eq!(verdicts(&steps), [("tolkster", "done"), ("checkout", "done")]);
    let w = world.borrow();
    assert!(w.paired && w.imported && !w.unpaired);
    assert_eq!(w.scripts.len(), 1);
    assert_eq!(w.scripts[0].1, "ORCA_RECIPE_ID=cloud ORCA_VM_INSTANCE_ID=box");
    assert_eq!(w.entry, Some(json!({ "recipe": "cloud", "repo": "tolkster", "folder": "/src/tolkster", "destroy": "./destroy.sh", "projectRoot": "/home/node/tolkster", "userData": { "provider": "x", "resourceId": "i-1" } })));
}

// A step failing after `create` destroys the machine with what `create` printed less its code,
// unpairs it and forgets it, so no paid machine is left half set up.
#[test]
fn a_failure_after_create_destroys_the_machine() {
    let world = RefCell::new(World { created: created(), ..World::default() });
    let steps = provisioned(&world, "fail");
    assert_eq!(verdicts(&steps), [("tolkster", "done"), ("checkout", "fail"), ("destroyed", "done")]);
    let w = world.borrow();
    assert!(w.unpaired && w.entry.is_none());
    let (line, _, stdin) = &w.scripts[1];
    assert_eq!(line, "./destroy.sh");
    let payload: Value = serde_json::from_str(stdin).unwrap();
    assert_eq!(payload["recipeResult"]["userData"]["resourceId"], "i-1");
    assert!(!stdin.contains("s3cret"));
}

// Pairing refused, or output `create` printed of another shape, still destroys the machine,
// and the report never carries the code, even where Orca's error repeats it.
#[test]
fn a_refused_pairing_or_a_bad_result_destroys_and_never_shows_the_code() {
    let world = RefCell::new(World { created: created(), pair_fails: true, ..World::default() });
    let steps = provisioned(&world, "done");
    assert_eq!(verdicts(&steps), [("machine", "fail"), ("destroyed", "done")]);
    assert!(!serde_json::to_string(&steps).unwrap().contains("s3cret"));
    let world = RefCell::new(World { created: json!({ "pairingCode": CODE, "projectRoot": "relative", "userData": { "resourceId": "i-2" } }).to_string(), ..World::default() });
    let steps = provisioned(&world, "done");
    assert_eq!(verdicts(&steps), [("machine", "fail"), ("destroyed", "done")]);
    assert!(world.borrow().scripts[1].2.contains("i-2") && !world.borrow().paired);
}

// A failed `destroy` keeps the entry and the pairing, so `host rm` can run it again.
#[test]
fn a_failed_destroy_keeps_what_host_rm_needs() {
    let world = RefCell::new(World { created: created(), destroy_fails: true, ..World::default() });
    let steps = provisioned(&world, "fail");
    assert_eq!(steps.last().map(|s| (s.verdict, s.reason.contains("host rm box"))), Some(("fail", true)));
    assert!(world.borrow().entry.is_some() && !world.borrow().unpaired);
}

// An entry another run holds is neither replaced nor taken out, and the machine this run
// made is destroyed without pairing, or said to be the owner's to delete when that fails.
#[test]
fn a_name_another_run_recorded_is_left_alone() {
    let world = RefCell::new(World { created: created(), taken: true, ..World::default() });
    let steps = provisioned(&world, "done");
    assert_eq!(verdicts(&steps), [("machine", "fail"), ("destroyed", "done")]);
    assert!(!world.borrow().paired && world.borrow().entry.is_none());
    let world = RefCell::new(World { created: created(), taken: true, destroy_fails: true, ..World::default() });
    let steps = provisioned(&world, "done");
    assert!(steps[1].reason.contains("by hand") && !steps[1].reason.contains("host rm"), "{}", steps[1].reason);
}

// Without userData nothing is recorded or paired, `destroy` is tried with what there is, and
// when it fails the owner is told to delete the machine.
#[test]
fn a_result_without_user_data_is_not_recorded() {
    let world = RefCell::new(World { created: json!({ "pairingCode": CODE, "projectRoot": "/home/node/tolkster" }).to_string(), destroy_fails: true, ..World::default() });
    let steps = provisioned(&world, "done");
    assert_eq!(verdicts(&steps), [("machine", "fail"), ("destroyed", "fail")]);
    assert!(steps[1].reason.contains("by hand") && !serde_json::to_string(&steps).unwrap().contains("s3cret"));
    assert!(!world.borrow().paired && world.borrow().entry.is_none() && !world.borrow().scripts[1].2.contains("s3cret"));
}

// A pairing that errs once Orca has it is still undone, while one of the name made by
// anything else after host add began is never touched.
#[test]
fn only_this_run_s_pairing_is_undone() {
    let world = RefCell::new(World { created: created(), pair_fails: true, pair_sticks: true, ..World::default() });
    provisioned(&world, "done");
    assert!(world.borrow().unpaired);
    let world = RefCell::new(World { created: created(), paired: true, name: "box".into(), ..World::default() });
    let steps = provisioned(&world, "done");
    assert_eq!(verdicts(&steps), [("machine", "fail"), ("destroyed", "done")]);
    assert!(!world.borrow().unpaired && world.borrow().paired);
}

// `create` failing leaves nothing to destroy: it stops before any entry or pairing.
#[test]
fn a_failed_create_stops_there() {
    let mut run = |_: &str, _: &[&str], _: Option<&str>| -> Result<Output, Error> { panic!("no Orca call") };
    let mut script = |_: &str, _: &str, _: &[(&str, &str)], _: &str| Ok(Output::failed(3, ""));
    let mut record = |_: Option<&Value>| -> Result<(), Error> { panic!("nothing to record") };
    let mut set_up = |_: &mut Run, _: &Host, _: (&str, &str)| -> Result<Vec<Step>, Error> { panic!("nothing to set up") };
    let error = provision(&mut run, &mut script, &recipe(), "box", &mut record, &mut set_up).err().unwrap();
    assert!(error.to_string().contains("exit 3"));
}

// A recipe's `prepare` that changed nothing, the run on most pulls, logs nothing; one that
// rebuilt says so, and a failure says it failed.
#[test]
fn a_prepare_with_nothing_to_do_logs_nothing() {
    let ended = |ok: bool, code: i32| Ok(Some(Output { ok, code: Some(code), stdout: String::new(), stderr: String::new() }));
    assert_eq!(prepare_said("Tolkster", ended(false, 3)), None);
    assert_eq!(prepare_said("Tolkster", Ok(None)), None);
    assert_eq!(prepare_said("Tolkster", ended(true, 0)), Some(trace::Host::Prepared { repo: "Tolkster".into() }));
    assert!(matches!(prepare_said("Tolkster", ended(false, 1)), Some(trace::Host::NotPrepared { repo: Some(_), .. })));
}

// The repo's own orca.yaml reads as Orca writes it; a recipe without a destroy is no recipe
// host add runs.
#[test]
fn a_recipe_is_read_from_orca_yaml_only_with_a_destroy() {
    let ours = "scripts:\n  setup: |\n    x\nenvironmentRecipes:\n  - id: agent37\n    name: Agent37\n    create: node scripts/orca-vm/agent37-recipe.ts create\n    suspend: node scripts/orca-vm/agent37-recipe.ts suspend\n    resume: node scripts/orca-vm/agent37-recipe.ts resume\n    destroy: node scripts/orca-vm/agent37-recipe.ts destroy\n    prepare: node scripts/orca-vm/agent37-recipe.ts prepare\n";
    let line = |phase: &str| format!("node scripts/orca-vm/agent37-recipe.ts {phase}");
    let sleep = Some((line("suspend"), line("resume")));
    assert_eq!(recipe_lines(ours, "agent37"), Some(Lines { create: line("create"), destroy: line("destroy"), sleep, prepare: Some(line("prepare")) }));
    let yaml = "scripts:\n  setup: x\nenvironmentRecipes: # ours\n- id: a\n  command: \"./a.sh\"\n  cleanup: ./b.sh\n  env:\n    - id: n\n      create: ./n.sh\n      destroy: ./n.sh\n- id: b\n  create: ./c.sh\n  destroy: none\n- id: c\n  create: |\n    ./c.sh\n  destroy: ./d.sh\n- id: d\n  create: ./e.sh\n  destroy: ./f.sh\n- id: d\n  create: ./g.sh\n  destroy: ./h.sh\nother: 1\n";
    assert_eq!(recipe_lines(yaml, "a"), Some(Lines { create: "./a.sh".into(), destroy: "./b.sh".into(), sleep: None, prepare: None }));
    for id in ["b", "c", "d", "n", "missing"] {
        assert_eq!(recipe_lines(yaml, id), None, "{id}");
    }
}

// The change touches `name`'s entry alone; another entry is never replaced.
#[test]
fn an_entry_is_set_and_taken_out_leaving_the_others() {
    let machines = Map::from_iter([("old".into(), json!({ "recipe": "r" }))]);
    let (set, done) = recorded(&machines, "box", None, Some(&json!({ "recipe": "cloud" })));
    assert!(done);
    assert_eq!(Value::Object(set), json!({ "box": { "recipe": "cloud" } }));
    assert_eq!(recorded(&machines, "old", None, Some(&json!({ "recipe": "new" }))), (Map::new(), false));
    assert_eq!(Value::Object(recorded(&machines, "old", None, None).0), json!({ "old": null }));
}

// `made` against the fakes, its machines.json in `config`, with the cap at 1. `nested` runs a
// second `made` from inside the first's `create`, as a trigger arriving while it is under way.
fn made_with(world: &RefCell<World>, config: &RefCell<Map<String, Value>>, outcome: &'static str, nested: bool) -> Option<(String, Vec<Step>)> {
    let mut entries = |change: &mut Change| {
        let (changed, done) = change(&config.borrow());
        let mut config = config.borrow_mut();
        for (key, value) in changed {
            match value {
                Value::Null => config.remove(&key),
                value => config.insert(key, value),
            };
        }
        Ok(done)
    };
    let mut run = orca(world);
    let mut script = |line: &str, _: &str, _: &[(&str, &str)], _: &str| {
        if line == "./create.sh" && nested {
            assert!(made_with(world, config, outcome, false).is_none(), "a second machine past the cap");
        }
        let mut w = world.borrow_mut();
        w.scripts.push((line.into(), String::new(), String::new()));
        Ok(if line == "./create.sh" { Output::ok(w.created.clone()) } else { Output::ok("") })
    };
    let mut set_up = |_: &mut Run, host: &Host, checkout: (&str, &str)| {
        assert_eq!((host.name.as_str(), checkout.1), ("tolkster-1", "/home/node/tolkster"));
        Ok(vec![step("checkout", outcome, "")])
    };
    let mut marked = |name: &str, counted: usize| assert_eq!((name, counted), ("tolkster-1", 1));
    made(&mut run, &mut script, &recipe(), Claim::Capped(1), &mut entries, &mut set_up, &mut marked).unwrap()
}

// One call names, records, pairs and lists one machine; a second while it is under way finds
// the cap taken by its mark and makes nothing, nor does one once it is made.
#[test]
fn a_machine_is_made_unprompted_once_under_its_cap() {
    let world = RefCell::new(World { created: created(), ..World::default() });
    let config = RefCell::new(Map::new());
    let (name, steps) = made_with(&world, &config, "done", true).unwrap();
    assert_eq!((name.as_str(), verdicts(&steps)), ("tolkster-1", vec![("checkout", "done")]));
    assert!(world.borrow().paired && world.borrow().scripts.len() == 1);
    assert_eq!(config.borrow()["tolkster-1"]["userData"]["resourceId"], "i-1");
    assert!(made_with(&world, &config, "done", false).is_none());
}

// A failure after `create` destroys the machine and takes its entry out, so the cap frees.
#[test]
fn a_failed_machine_frees_its_cap() {
    let world = RefCell::new(World { created: created(), ..World::default() });
    let config = RefCell::new(Map::new());
    let (_, steps) = made_with(&world, &config, "fail", false).unwrap();
    assert_eq!(steps.last().map(|s| (s.step.as_str(), s.verdict)), Some(("destroyed", "done")));
    assert!(config.borrow().is_empty());
    let world = RefCell::new(World { created: json!({ "pairingCode": CODE }).to_string(), ..World::default() });
    made_with(&world, &config, "done", false).unwrap();
    assert!(config.borrow().is_empty(), "a mark outlived a destroyed machine");
}

// The first `<repo>-<n>` no entry or paired environment has, ignoring case, while the repo's
// entries, marks included, are under its `machines`; another repo's never count.
#[test]
fn a_machine_s_name_and_cap_count_its_repo_s_entries() {
    let machines = Map::from_iter([("Tolkster-1".into(), json!({ "recipe": "cloud", "repo": "tolkster" })), ("other-1".into(), json!({ "recipe": "x", "repo": "other" }))]);
    assert_eq!(free_name(&machines, "tolkster", 3, &["TOLKSTER-2".into()]).as_deref(), Some("tolkster-3"));
    assert_eq!(free_name(&machines, "tolkster", 3, &["tolkster-2".into(), "tolkster-3".into()]).as_deref(), Some("tolkster-4"));
    assert_eq!(free_name(&machines, "TOLKSTER", 1, &[]), None);
    assert_eq!(free_name(&Map::new(), "tolkster", 1, &[]).as_deref(), Some("tolkster-1"));
    assert_eq!(free_name(&Map::new(), &"r".repeat(63), 1, &[]), None, "a name host rm refuses");
}

// The owner's name is marked like `listen`'s, so neither runs `create` under the other's: one
// an entry or paired environment has, ignoring case, makes nothing.
#[test]
fn a_named_machine_is_marked_unless_its_name_is_taken() {
    let config = RefCell::new(Map::from_iter([("BOX".into(), json!({ "recipe": "cloud", "repo": "tolkster" }))]));
    let world = RefCell::new(World { created: created(), ..World::default() });
    let mut entries = |change: &mut Change| Ok(change(&config.borrow()).1);
    let mut run = orca(&world);
    let mut script = |_: &str, _: &str, _: &[(&str, &str)], _: &str| -> Result<Output, Error> { panic!("no create under a taken name") };
    let mut set_up = |_: &mut Run, _: &Host, _: (&str, &str)| -> Result<Vec<Step>, Error> { panic!("nothing to set up") };
    assert!(made(&mut run, &mut script, &recipe(), Claim::Named("box"), &mut entries, &mut set_up, &mut |_, _| panic!("nothing marked")).unwrap().is_none());
}

// A refusal repeated on every pass of the queue is said once, until its reason changes or a
// make for the key begins.
#[test]
fn a_repeated_refusal_is_said_once_until_its_reason_changes() {
    let (capped, cooling) = (trace::Declined::Running, trace::Declined::CoolDown);
    let progress = crate::runtime::Progress::default();
    let said = |why: &trace::Declined| progress.decline("make refusals".into(), why.clone(), trace::Host::NotMaking { repo: "refusals".into(), why: why.clone() });
    assert_eq!([said(&capped), said(&capped), said(&cooling), said(&cooling), said(&capped)], [true, false, true, false, true]);
    progress.began("make refusals");
    assert!(said(&capped));
}

// None on settings read before the last make ended, and none in the cool-down after one that
// failed.
#[test]
fn a_make_is_refused_on_old_settings_and_after_a_failure() {
    let now = Instant::now();
    assert!(fresh(3, 3, None, now));
    assert!(!fresh(2, 3, None, now));
    assert!(!fresh(3, 3, Some(now - Duration::from_secs(60)), now));
    assert!(fresh(3, 3, Some(now - COOL_DOWN), now));
}

// A recipe line's stderr reaches the disk log less the pairing code its stdout printed.
#[test]
fn a_recipe_s_stderr_never_logs_its_pairing_code() {
    let stdout = r#"{"pairingCode":"pc-secret","projectRoot":"/w"}"#;
    assert_eq!(unpaired("+ echo pc-secret\nmade\n", stdout), "+ echo <pairing code>\nmade\n");
    assert_eq!(unpaired("made\n", "not json"), "made\n");
    // A repeated key still masks the code it ends with.
    assert_eq!(unpaired("+ echo pc-secret\n", r#"{"pairingCode": "x", "pairingCode": "pc-secret"}"#), "+ echo <pairing code>\n");
    // A tail cut inside a line starts at the next whole one, so no part of a code is left.
    assert_eq!(whole_lines(b"secret\nmade\n", true), "made\n");
    assert_eq!(whole_lines(b"progress", true), "");
    assert_eq!(whole_lines(b"made\n", false), "made\n");
}
