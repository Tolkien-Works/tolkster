use super::*;
use serde_json::json;

#[test]
fn the_worker_url_gives_the_realtime_and_state_endpoints() {
    let url = WorkerUrl::parse(" https://tolkster.example.workers.dev/ ").unwrap();
    assert_eq!((url.realtime().as_str(), url.state().as_str()), ("wss://tolkster.example.workers.dev/realtime", "https://tolkster.example.workers.dev/state"));
    let url = WorkerUrl::parse("http://localhost:8787").unwrap();
    assert_eq!((url.realtime().as_str(), url.state().as_str()), ("ws://localhost:8787/realtime", "http://localhost:8787/state"));
    assert_eq!(WorkerUrl::parse("http://127.0.0.1:8787").unwrap().realtime(), "ws://127.0.0.1:8787/realtime");
    // Never the secret in the clear across a network.
    for bad in ["http://tolkster.example.workers.dev", "http://localhost.evil.com", "wss://x.dev", "tolkster.example.workers.dev", "https://", "https://x.dev/realtime", "https://x.dev?a"] {
        assert!(matches!(WorkerUrl::parse(bad), Err(Error::Usage(_))), "{bad}");
    }
}

fn files(config: Value, credentials: Value) -> Files {
    Files { config, credentials }
}

fn read(files: &Files) -> Result<Config, Error> {
    from_files(files, "owner-1".into(), Path::new("/home/someone"), None)
}

// From config.json alone.
fn set(config: Value) -> Result<Config, Error> {
    read(&files(config, Value::Null))
}

fn usage(config: Value) -> String {
    match set(config) {
        Err(Error::Usage(why)) => why,
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_owner_s_settings_come_from_config_json_alone() {
    let both = files(json!({ "owner": { "id": "from-config", "coAuthor": "" } }), json!({ "realtimeSecret": "s" }));
    assert_eq!(both.string("/owner/id").unwrap().as_deref(), Some("from-config"));
    // Empty is unset.
    assert_eq!(read(&both).unwrap().owner.co_author, None);
    // A setting left in credentials.json from before it moved exits 2 rather than going unread.
    for (stale, named) in [(json!({ "ownerId": "u-1" }), "\"ownerId\" in credentials.json is now \"owner.id\""), (json!({ "coAuthor": "A <a@b>" }), "\"owner.coAuthor\" in config.json")] {
        match read(&files(json!({}), stale.clone())) {
            Err(Error::Usage(why)) => assert!(why.contains(named), "{stale}: {why}"),
            other => panic!("{stale}: {other:?}"),
        }
    }
    let set_here = files(json!({ "owner": { "coAuthor": "A <a@b>" } }), Value::Null);
    assert_eq!(read(&set_here).unwrap().owner.co_author.as_deref(), Some("A <a@b>"));
    // TOLKSTER_CO_AUTHOR comes before the file.
    let from_env = from_files(&set_here, "owner-1".into(), Path::new("/home/someone"), Some("B <b@c>".into())).unwrap();
    assert_eq!(from_env.owner.co_author.as_deref(), Some("B <b@c>"));
    assert!(files(json!({ "owner": { "name": 3 } }), Value::Null).string("/owner/name").is_err());
    let stale = files(Value::Null, json!({ "review": { "reviewers": ["a-reviewer"] }, "slots": { "local": 9 }, "owner": { "name": "Someone" } }));
    let got = read(&stale).unwrap();
    assert!(got.review.reviewers.is_empty());
    assert_eq!(got.local_slots(), 6);
    assert_eq!(got.owner.name, "the owner");
}

#[test]
fn secrets_come_from_credentials_json_only() {
    let both = files(json!({ "realtimeSecret": "from-config" }), json!({ "clientId": "id-1", "clientSecret": "", "other": 3 }));
    assert_eq!(both.secret("clientId").as_deref(), Some("id-1"));
    assert_eq!(both.secret("clientSecret"), None);
    assert_eq!(both.secret("other"), None);
    assert_eq!(both.secret("realtimeSecret"), None);
}

#[test]
fn defaults_name_no_one_and_print_grouped() {
    let home = Path::new("/home/someone");
    let got = read(&files(Value::Null, Value::Null)).unwrap();
    assert_eq!(
        got,
        Config {
            owner: Owner { id: "owner-1".into(), name: "the owner".into(), co_author: None },
            review: Review { reviewers: vec![], max_rounds: 3, deep_at: Size::M },
            triage: Triage { batch: 5 },
            ladder: Ladder::default(),
            repos: RepoMap::default(),
            slots: [("local".to_string(), 6)].into(),
            timing: true,
            release_pipeline: None,
            host: None,
            machines: Default::default(),
            enrolled: vec![],
            revisions: std::collections::BTreeMap::new(),
            briefs_dir: home.join(".claude").join("<repo>-briefs").display().to_string(),
        }
    );
    let json = serde_json::to_value(&got).unwrap();
    assert_eq!(json["owner"], json!({ "id": "owner-1", "name": "the owner", "coAuthor": null }));
    assert_eq!(json["review"], json!({ "reviewers": [], "maxRounds": 3, "deepAt": "M" }));
    assert_eq!(json["triage"], json!({ "batch": 5 }));
    assert_eq!(json["repos"], json!({}));
    assert!(json.get("briefsDir").is_none());
}

#[test]
fn timing_is_on_unless_set_false() {
    assert!(set(json!({})).unwrap().timing);
    assert!(set(json!({ "timing": null })).unwrap().timing);
    assert!(set(json!({ "timing": true })).unwrap().timing);
    assert!(!set(json!({ "timing": false })).unwrap().timing);
    assert!(usage(json!({ "timing": "off" })).contains("\"timing\" in config.json must be true or false"));
}

#[test]
fn a_release_pipeline_is_a_linear_id_or_unset() {
    assert_eq!(set(json!({})).unwrap().release_pipeline, None);
    assert_eq!(set(json!({ "releasePipeline": "68bc4e59-41d7" })).unwrap().release_pipeline.as_deref(), Some("68bc4e59-41d7"));
    assert!(usage(json!({ "releasePipeline": "a b" })).contains("\"releasePipeline\" in config.json must be a Linear ID"));
}

// A key no reader takes exits 2 naming it, and an old one its replacement.
#[test]
fn an_unknown_or_old_key_exits_2_naming_it() {
    for (config, named) in [
        (json!({ "ownerId": "u-1" }), "\"ownerId\" in config.json is now \"owner.id\""),
        (json!({ "workerUrl": "https://x.dev" }), "\"workerUrl\" in config.json is now \"worker\""),
        (json!({ "deepReviewEstimate": "M" }), "\"deepReviewEstimate\" in config.json is now \"review.deepAt\""),
        (json!({ "triageBatch": 2 }), "\"triageBatch\" in config.json is now \"triage.batch\""),
        (json!({ "briefsDir": "~/b" }), "\"briefsDir\" in config.json is no longer a setting"),
        (json!({ "reviewer": [] }), "\"reviewer\" in config.json is not a setting"),
        (json!({ "owner": { "ID": "u-1" } }), "\"owner.ID\""),
        (json!({ "review": { "deepReviewEstimate": "M" } }), "\"review.deepReviewEstimate\" in config.json is now \"review.deepAt\""),
        (json!({ "triage": { "size": 2 } }), "\"triage.size\""),
        (json!({ "ladder": { "rungs": [{ "name": "A", "model": "opus", "effort": "high", "up": "S" }], "default": 1 } }), "\"ladder.rungs[0].up\""),
        (json!({ "repos": { "Site": { "teams": ["WEB"], "maxRounds": 2 } } }), "\"repos.Site.maxRounds\" in config.json is now \"review.maxRounds\""),
        (json!({ "repos": { "Site": { "review": { "maxRound": 2 } } } }), "\"repos.Site.review.maxRound\""),
        (json!({ "repos": { "Site": { "recipe": { "id": "cloud", "machines": 1, "slotsEach": 1, "idle": 5 } } } }), "\"repos.Site.recipe.idle\""),
        (json!({ "repos": { "Site": { "recipe": { "id": "cloud", "cap": 1 } } } }), "\"repos.Site.recipe.cap\" in config.json is now \"machines\""),
    ] {
        let why = usage(config.clone());
        assert!(why.contains(named), "{config}: {why}");
    }
    assert!(usage(json!({ "repos": [{ "repo": "Site", "teams": ["WEB"] }] })).contains("keyed by"));
    // A group that isn't an object would leave each of its settings on its default.
    for group in [json!({ "review": ["alice"] }), json!({ "review": "XS" }), json!({ "triage": 2 }), json!({ "owner": "u-1" })] {
        assert!(usage(group.clone()).contains("must be an object"), "{group}");
    }
    assert!(usage(json!({ "owner": { "deepReviewEstimate": "M" } })).contains("is not a setting"));
    assert!(read(&files(json!({}), json!({ "ownerId": null, "coAuthor": "" }))).is_ok());
    // Null is unset, at every level.
    assert!(set(json!({ "ladder": null, "repos": null, "review": null, "owner": { "name": null } })).is_ok());
    assert!(matches!(check(&json!([1])), Err(Error::Usage(_))));
    // What moved out of config.json: an admitted credential, machines.json and credentials.json.
    assert!(usage(json!({ "hosts": [] })).contains("is no longer a setting"));
    assert!(usage(json!({ "hostRecipes": {} })).contains("machines.json: move its value there"));
    assert!(usage(json!({ "host": "e1" })).contains("credentials.json"));
    // The caps moved into `slots`, and an old file names it.
    for old in ["maxRunning", "hostCaps"] {
        assert!(usage(json!({ old: 2 })).contains("\"slots\""), "{old}");
    }
}

#[test]
fn whole_numbers_are_checked() {
    // A size by name alone, and never zero: no setting turns the deep pass off.
    assert_eq!(set(json!({ "review": { "deepAt": "l" } })).unwrap().review.deep_at, Size::L);
    assert_eq!(set(json!({ "review": { "deepAt": "XS" } })).unwrap().review.deep_at, Size::XS);
    for bad in [json!("zero"), json!(3), json!("huge"), json!(true)] {
        assert!(set(json!({ "review": { "deepAt": bad } })).is_err(), "{bad}");
    }
    assert_eq!(set(json!({ "ladder": { "rungs": [{ "name": "Opus", "model": "opus", "effort": "high" }], "default": 1 } })).unwrap().ladder.rungs.len(), 1);
    assert!(set(json!({ "ladder": { "rungs": [{ "name": "Opus", "model": "opus; rm", "effort": "high" }], "default": 1 } })).is_err());
    assert_eq!(set(json!({ "review": { "maxRounds": 5 } })).unwrap().review.max_rounds, 5);
    assert!(usage(json!({ "review": { "maxRounds": 0 } })).contains("\"review.maxRounds\""));
    assert_eq!(set(json!({ "triage": { "batch": 2 } })).unwrap().triage.batch, 2);
    assert!(set(json!({ "triage": { "batch": 0 } })).is_err());
}

// A host's name goes into its worktrees' IDs, so only an Orca environment's name is taken.
#[test]
fn slots_map_names_to_whole_numbers_and_local_defaults_to_six() {
    let caps = set(json!({ "slots": { "mac": 2, "pi": 0 } })).unwrap().slots;
    assert_eq!(caps.into_iter().collect::<Vec<_>>(), [("local".to_string(), 6), ("mac".to_string(), 2), ("pi".to_string(), 0)]);
    assert_eq!(set(json!({ "slots": { "local": 0 } })).unwrap().local_slots(), 0);
    for bad in [json!({ "mac": -1 }), json!({ "mac": 1.5 }), json!({ "mac": "2" }), json!({ " ": 2 }), json!([2])] {
        assert!(matches!(set(json!({ "slots": bad })), Err(Error::Usage(_))), "{bad}");
    }
}

// credentials.json's `host`, as `host join` writes it there, the ID its credential was issued
// to; none, with no credential, on the `listen` machine. A credential without its `host`, or
// beside another's, is refused rather than read as the `listen` machine.
#[test]
fn the_host_is_the_environment_its_credential_was_issued_to() {
    let credentials = |host: Value, credential: Value| files(json!({}), json!({ "host": host, "hostCredential": credential }));
    assert_eq!(read(&credentials(json!("e-1.mac"), json!("e-1.mac:secret"))).unwrap().host.as_deref(), Some("e-1.mac"));
    assert_eq!(read(&credentials(Value::Null, Value::Null)).unwrap().host, None);
    for (host, credential) in [(Value::Null, json!("e1:s")), (json!("e1"), Value::Null), (json!("e2"), json!("e1:s")), (json!("my mac"), json!("my mac:s")), (json!(3), json!("3:s"))] {
        assert!(read(&credentials(host.clone(), credential.clone())).is_err(), "{host} {credential}");
    }
}

#[test]
fn reviewers_are_a_list_of_logins() {
    let set = files(json!({ "review": { "reviewers": ["a-reviewer", "copilot-pull-request-reviewer"] } }), Value::Null);
    assert_eq!(reviewers(&set).unwrap(), ["a-reviewer", "copilot-pull-request-reviewer"]);
    assert!(reviewers(&files(Value::Null, Value::Null)).unwrap().is_empty());
    for bad in [json!("a-reviewer"), json!([1]), json!([""]), json!(["  "])] {
        assert!(reviewers(&files(json!({ "review": { "reviewers": bad } }), Value::Null)).is_err(), "{bad}");
    }
}

#[test]
fn the_global_reviewers_are_trimmed_like_a_repo_s() {
    let global = files(json!({ "review": { "reviewers": [" alice", "bob "] } }), Value::Null);
    assert_eq!(reviewers(&global).unwrap(), ["alice", "bob"]);
    let repo = set(json!({ "repos": { "Plan Portal": { "review": { "reviewers": [" alice", "bob "] } } } })).unwrap();
    assert_eq!(repo.repos.0[0].reviewers().map(Vec::as_slice), Some(&["alice".to_string(), "bob".to_string()][..]));
}

#[test]
fn whole_numbers_and_names_are_read_one_way() {
    assert_eq!(whole_number(&json!(3), 1), Some(3));
    assert_eq!(whole_number(&json!(3.0), 1), Some(3));
    for bad in [json!(0), json!(1.5), json!("3"), json!(null)] {
        assert_eq!(whole_number(&bad, 1), None, "{bad}");
    }
    assert_eq!(whole_number(&json!(0), 0), Some(0));
    assert_eq!(names(&json!([" a ", "b"])), Some(vec!["a".to_string(), "b".to_string()]));
    for bad in [json!("a"), json!([1]), json!([" "])] {
        assert_eq!(names(&bad), None, "{bad}");
    }
}

#[test]
fn some_repo_s_reviewers_differ_from_the_global_ones_only_when_one_is_set_apart() {
    assert!(!set(json!({ "review": { "reviewers": ["a"] } })).unwrap().repo_reviewers_differ());
    let same = json!({ "review": { "reviewers": ["a"] }, "repos": { "P": { "review": { "reviewers": [" a"] } }, "Q": { "review": { "maxRounds": 2 } } } });
    assert!(!set(same).unwrap().repo_reviewers_differ());
    let apart = json!({ "review": { "reviewers": ["a"] }, "repos": { "P": { "review": { "reviewers": ["b"] } } } });
    assert!(set(apart).unwrap().repo_reviewers_differ());
}

// `tolkster config` prints a repo's recipe with the default filled in. Each machine a recipe
// made is read from machines.json with the repo it was made for; an entry of another shape is
// left out.
#[test]
fn recipe_settings_are_printed_and_made_machines_read() {
    let config = set(json!({ "repos": { "Site": { "recipe": { "id": "cloud", "machines": 2, "slotsEach": 3 } } } })).unwrap();
    let json = serde_json::to_value(&config).unwrap();
    assert_eq!(json["repos"]["Site"]["recipe"], json!({ "id": "cloud", "machines": 2, "slotsEach": 3, "idleMinutes": DEFAULT_IDLE_MINUTES, "destroyAfter": DEFAULT_DESTROY_AFTER }));
    let made = machines(&json!({ "site-1": { "recipe": "cloud", "repo": "Site", "destroy": "x", "userData": { "k": 1 } } }));
    assert_eq!(made.into_iter().collect::<Vec<_>>(), [("site-1".to_string(), Machine { recipe: "cloud".into(), repo: "Site".into(), made: true, sleeps: false, asleep: false, environment: None, revision: None })]);
    // The revision is the one `userData` holds, when the Durable Object would take it.
    let stamped = |revision: Value| machines(&json!({ "site-1": { "recipe": "cloud", "userData": { "revision": revision } } }))["site-1"].revision.clone();
    assert_eq!(stamped(json!("4")), Some("4".into()));
    for odd in [json!(4), json!(""), json!("4 5"), json!("x".repeat(101))] {
        assert_eq!(stamped(odd.clone()), None, "{odd}");
    }
    let lines = json!({ "recipe": "cloud", "repo": "Site", "folder": "/s", "destroy": "d", "suspend": "s", "resume": "r", "userData": {}, "asleep": true });
    assert!(machines(&json!({ "site-1": lines }))["site-1"].sleeps && machines(&json!({ "site-1": lines }))["site-1"].asleep);
    for odd in [json!([]), json!({ "site-1": "cloud" }), json!({ "site-1": { "repo": "Site" } })] {
        assert!(machines(&odd).is_empty(), "{odd}");
    }
}

#[test]
fn a_repo_s_review_settings_override_the_global_ones_and_fall_back_to_them() {
    let config = set(json!({
        "review": { "reviewers": ["the-owner", "copilot-pull-request-reviewer"], "maxRounds": 3 },
        "repos": {
            "Plan Portal": { "teams": ["PP"], "review": { "reviewers": ["a-lead"], "maxRounds": 1 } },
            "Site": { "projects": ["Web"] },
            "Quiet": { "review": { "reviewers": [] } },
        },
    }))
    .unwrap();
    let got = |repo: Option<&str>| {
        let settings = config.review_for(repo);
        (settings.reviewers, settings.max_rounds)
    };
    let global = (vec!["the-owner".to_string(), "copilot-pull-request-reviewer".into()], 3);
    // The repo's own, ignoring case.
    assert_eq!(got(Some("Plan Portal")), (vec!["a-lead".to_string()], 1));
    assert_eq!(got(Some(" PLAN PORTAL ")), (vec!["a-lead".to_string()], 1));
    // An entry that sets neither, a repo no entry names, and no repo: the global ones.
    assert_eq!(got(Some("Site")), global);
    assert_eq!(got(Some("Tolkster")), global);
    assert_eq!(got(None), global);
    assert_eq!(got(Some("Quiet")), (vec![], 3));
    assert!(config.per_repo_review());
    // `tolkster config` prints what each entry's repo gets.
    let printed = serde_json::to_value(&config).unwrap();
    assert_eq!(printed["repos"]["Plan Portal"]["review"], json!({ "reviewers": ["a-lead"], "maxRounds": 1 }));
    assert_eq!(printed["repos"]["Site"], json!({ "projects": ["Web"], "teams": [], "review": { "reviewers": ["the-owner", "copilot-pull-request-reviewer"], "maxRounds": 3 }, "land": "pr", "trace": false }));
}

#[test]
fn a_repo_needs_its_own_review_settings_only_when_they_differ() {
    let config = |value: Value| set(value).unwrap();
    // Settings the same as the global ones need no repo to pick them.
    assert!(!config(json!({ "review": { "reviewers": ["a"] }, "repos": { "Site": { "review": { "reviewers": ["a"], "maxRounds": 3 } } } })).per_repo_review());
    assert!(!config(json!({ "repos": { "Site": { "projects": ["Web"] } } })).per_repo_review());
}

#[test]
fn a_worktree_s_repo_is_looked_up_only_when_a_repo_has_its_own_settings() {
    let orca = crate::orca::parse_repos(json!({ "result": { "repos": [{ "id": "r1", "displayName": "Site", "path": "/src/site" }, { "id": "r2", "displayName": "Tolkster", "path": "/src/tolkster" }] } })).unwrap();
    let config = set(json!({ "review": { "reviewers": ["the-owner"] }, "repos": { "Site": { "review": { "reviewers": ["a-lead"] } } } })).unwrap();
    let named = |id: &str| crate::orca::repo_name(&orca, id);
    assert_eq!(config.review_in(&mut || Ok(named("r1"))).unwrap().reviewers, ["a-lead"]);
    assert_eq!(config.review_in(&mut || Ok(named("r2"))).unwrap().reviewers, ["the-owner"]);
    assert_eq!(config.review_in(&mut || Ok(named("gone"))).unwrap().reviewers, ["the-owner"]);
    // A listing that fails is an error, never the global reviewers.
    assert!(config.review_in(&mut || Err(Error::Failed("orca".into()))).is_err());
    let global = set(json!({ "review": { "reviewers": ["the-owner"] }, "repos": { "Site": { "projects": ["Web"] } } })).unwrap();
    assert_eq!(global.review_in(&mut || panic!("listed Orca's repos")).unwrap().reviewers, ["the-owner"]);
}

// Every key the loader reads, each at its level.
fn everything() -> Value {
    json!({
        "$schema": SCHEMA_REF,
        "owner": { "id": "u-1", "name": "Someone", "coAuthor": "A <a@b>" },
        "worker": "https://tolkster.example.workers.dev",
        "review": { "reviewers": ["a-lead"], "maxRounds": 2, "deepAt": "L" },
        "triage": { "batch": 4 },
        "ladder": { "rungs": [{ "name": "Sonnet 5.5", "harness": "claude", "model": "claude-sonnet-5-5", "effort": "high", "upTo": "M" }, { "name": "Opus 5.5", "model": "claude-opus-5-5", "effort": "high" }], "default": 1 },
        "repos": { "Site": { "projects": ["Web"], "teams": ["WEB"], "review": { "reviewers": ["a-lead"], "maxRounds": 1 }, "recipe": { "id": "box", "machines": 1, "slotsEach": 2, "idleMinutes": 20, "destroyAfter": 600 }, "land": "direct", "trace": true } },
        "slots": { "local": 4, "wsl": 2 },
        "timing": false,
        "releasePipeline": "68bc4e59-41d7-46ea-bdd7-0e0aac1d96e5",
    })
}

fn keys_of(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

fn sorted(keys: &[&str]) -> Vec<String> {
    let mut keys: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
    keys.sort();
    keys
}

// The schema is what the loader reads: at each level the same keys, and `check` refuses what the
// schema's `additionalProperties: false` does. A key added to a loader's list but not the
// schema, or the other way round, fails here.
#[test]
fn the_schema_allows_the_keys_the_loader_reads_and_no_others() {
    let schema: Value = serde_json::from_str(SCHEMA).unwrap();
    let full = everything();
    assert!(check(&full).is_ok());
    // The config pointer, the schema's, and the loader's list at that level.
    let levels: [(&str, &str, &[&str]); 10] = [
        ("", "", TOP),
        ("/owner", "/properties/owner", OWNER),
        ("/review", "/properties/review", REVIEW),
        ("/triage", "/properties/triage", TRIAGE),
        ("/ladder", "/properties/ladder", crate::config::ladder::LADDER),
        ("/ladder/rungs/0", "/properties/ladder/properties/rungs/items", crate::config::ladder::RUNG),
        ("/repos/Site", "/properties/repos/additionalProperties", crate::config::repo_map::ENTRY),
        ("/repos/Site/review", "/properties/repos/additionalProperties/properties/review", crate::config::repo_map::REVIEW),
        ("/repos/Site/recipe", "/properties/repos/additionalProperties/properties/recipe", crate::config::repo_map::RECIPE),
        ("/slots", "/properties/slots", &[]),
    ];
    for (at, in_schema, known) in levels {
        let level = schema.pointer(in_schema).unwrap_or(&schema);
        let allowed = level.get("properties").map(keys_of).unwrap_or_default();
        // `slots` is open-ended: the schema names no keys of it.
        if at != "/slots" {
            assert_eq!(allowed, sorted(known), "{at}");
            assert_eq!(level["additionalProperties"], json!(false), "{at}");
            assert_eq!(keys_of(full.pointer(at).unwrap_or(&full)), sorted(known), "{at}: everything() lacks a key");
            let mut stray = full.clone();
            let target = if at.is_empty() { &mut stray } else { stray.pointer_mut(at).unwrap() };
            target["nothing_reads_this"] = json!(1);
            assert!(matches!(check(&stray), Err(Error::Usage(_))), "{at}");
        }
    }
}

// The enums are the sizes and harnesses the loader accepts, all of them.
#[test]
fn the_schema_s_enums_are_what_the_loader_accepts() {
    let schema: Value = serde_json::from_str(SCHEMA).unwrap();
    let enum_at = |pointer: &str| -> Vec<String> { schema.pointer(pointer).unwrap()["enum"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect() };
    let sizes: Vec<String> = (0..=Size::LARGEST.0).map(|n| Size(n).name().to_string()).collect();
    assert_eq!(enum_at("/properties/ladder/properties/rungs/items/properties/upTo"), sizes);
    assert_eq!(enum_at("/properties/review/properties/deepAt"), sizes[1..]);
    let harnesses: Vec<String> = crate::harnesses::KNOWN.iter().map(|h| h.name.to_string()).collect();
    assert_eq!(enum_at("/properties/ladder/properties/rungs/items/properties/harness"), harnesses);
    assert_eq!(enum_at("/properties/repos/additionalProperties/properties/land"), ["pr", "direct"]);
    for land in ["pr", "direct"] {
        assert!(set(json!({ "repos": { "Site": { "land": land } } })).is_ok(), "{land}");
    }
    for size in &sizes {
        let rung = json!({ "name": "Opus 5.5", "model": "claude-opus-5-5", "effort": "high", "upTo": size });
        assert!(set(json!({ "ladder": { "rungs": [rung], "default": 1 } })).is_ok(), "{size}");
        assert_eq!(set(json!({ "review": { "deepAt": size } })).is_ok(), size != "zero", "{size}");
    }
}

#[test]
fn the_schema_is_written_beside_the_config_file_and_named_by_it() {
    let dir = std::env::temp_dir().join(format!("tolkster-schema-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let file = dir.join("config.json");
    adopt_schema(&file).unwrap();
    assert_eq!(fs::read_to_string(dir.join(SCHEMA_FILE)).unwrap(), SCHEMA);
    let written: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(written, json!({ "$schema": SCHEMA_REF }));
    // The owner's settings stay, a stale schema is replaced, and the loader takes the file.
    fs::write(&file, r#"{ "$schema": "other", "triage": { "batch": 2 } }"#).unwrap();
    fs::write(dir.join(SCHEMA_FILE), "old").unwrap();
    adopt_schema(&file).unwrap();
    assert_eq!(fs::read_to_string(dir.join(SCHEMA_FILE)).unwrap(), SCHEMA);
    let written: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(written, json!({ "$schema": SCHEMA_REF, "triage": { "batch": 2 } }));
    assert!(check(&written).is_ok());
    let _ = fs::remove_dir_all(&dir);
}
