use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

use super::*;

// Each label created, as (name, team, group, parent), with IDs "new-1", "new-2", ...
#[derive(Default)]
struct Created(Vec<(String, Option<String>, bool, Option<String>)>);

impl Labels for Created {
    fn create(&mut self, name: &str, team_id: Option<&str>, group: bool, parent_id: Option<&str>) -> Result<String, Error> {
        self.0.push((name.into(), team_id.map(String::from), group, parent_id.map(String::from)));
        Ok(format!("new-{}", self.0.len()))
    }
}

// A team as `workspace::read_team` reads it, with `labels` as the workspace's and the team's flags.
fn team(labels: Value) -> TeamRead {
    crate::json::decode(json!({
        "teams": { "nodes": [{ "id": "t1", "key": "TW", "triageEnabled": true, "issueEstimationType": "notUsed", "issueEstimationExtended": false, "issueEstimationAllowZero": false, "states": { "nodes": [] }, "projects": { "nodes": [
            { "id": "p1", "name": "Tolkster", "completedAt": null, "canceledAt": null },
            { "id": "p2", "name": "Site redesign", "completedAt": null, "canceledAt": null },
            { "id": "p3", "name": "Old", "completedAt": "2026-01-01T00:00:00.000Z", "canceledAt": null },
        ] } }] },
        "issueLabels": { "nodes": labels },
    }), "Linear's answer to Doctor").unwrap()
}

fn orca() -> Vec<Repo> {
    [("r1", "Tolkster"), ("r2", "Site"), ("r3", "Docs")].iter().map(|(id, name)| Repo { id: id.to_string(), name: name.to_string() }).collect()
}

fn group(children: Value) -> Group {
    let mut labels = vec![json!({ "id": "g", "name": "tolkster", "isGroup": true, "parent": null })];
    labels.extend(children.as_array().unwrap().iter().map(|child| json!({ "isGroup": false, "id": child["id"], "name": child["name"], "parent": child["parent"] })));
    Group::parse(&crate::json::decode(json!({ "issueLabels": { "nodes": labels } }), "Linear's answer to Group").unwrap()).unwrap()
}

fn file(name: &str, config: Value) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tolkster-init-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("config.json");
    fs::write(&file, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    file
}

fn flags(default_repo: Option<&str>, yes: bool) -> Owner {
    Owner { default_repo: default_repo.map(String::from), yes, terminal: false }
}

// Only the flag the team lacks is created, on the team and outside the group; a second run,
// with it there, creates none.
#[test]
fn creates_only_the_flags_a_team_lacks() {
    let config = file("flags", json!({ "repos": { "Site": { "teams": ["TW"] } } }));
    let held = group(json!([{ "id": "off", "name": "host offline", "parent": { "id": "g" } }]));
    let lacking = team(json!([{ "name": "Plan-First", "team": null }, { "name": "manual-merge", "team": { "key": "WEB" } }]));
    let mut created = Created::default();
    let done = init("TW", &lacking, &orca(), Some(&held), &mut flags(None, true), &mut created, &config).unwrap();
    assert_eq!(done.created, vec![MANUAL_MERGE_LABEL]);
    // The team's default covers its projects, so even --yes maps none of them.
    assert!(!done.changed);
    assert_eq!(created.0, vec![(MANUAL_MERGE_LABEL.to_string(), Some("t1".to_string()), false, None)]);
    let full = team(json!([{ "name": "plan-first", "team": null }, { "name": "manual-merge", "team": { "key": "tw" } }]));
    let mut again = Created::default();
    assert_eq!(init("TW", &full, &orca(), Some(&held), &mut flags(None, true), &mut again, &config).unwrap().created, Vec::<&str>::new());
    assert!(again.0.is_empty());
}

// Without the group, it is created in the workspace with its states inside it, and no flag; a
// group that lacks a state gets that one alone; a whole group gets nothing.
#[test]
fn creates_the_group_and_its_states_once_with_no_flag_inside() {
    let config = file("group", json!({ "repos": { "Site": { "teams": ["TW"] } } }));
    let mut created = Created::default();
    init("TW", &team(json!([])), &orca(), None, &mut flags(None, true), &mut created, &config).unwrap();
    let in_group: Vec<&str> = created.0.iter().filter(|(_, _, _, parent)| parent.is_some()).map(|(name, ..)| name.as_str()).collect();
    assert_eq!(in_group, vec![HOST_OFFLINE]);
    let (_, team_id, is_group, parent) = created.0.iter().find(|(name, ..)| name == GROUP).unwrap();
    assert_eq!((team_id, is_group, parent), (&None, &true, &None));
    let group_id = format!("new-{}", created.0.iter().position(|(name, ..)| name == GROUP).unwrap() + 1);
    assert!(created.0.iter().all(|(name, team_id, _, parent)| (name == HOST_OFFLINE) == (parent.as_deref() == Some(group_id.as_str())) && (team_id.is_some() == [PLAN_FIRST_LABEL, MANUAL_MERGE_LABEL].contains(&name.as_str()))));
    let flagged = team(json!([{ "name": "plan-first", "team": null }, { "name": "manual-merge", "team": null }]));
    let mut child = Created::default();
    init("TW", &flagged, &orca(), Some(&group(json!([{ "id": "x", "name": "paused", "parent": { "id": "g" } }]))), &mut flags(None, true), &mut child, &config).unwrap();
    assert_eq!(child.0, vec![(HOST_OFFLINE.to_string(), None, false, Some("g".to_string()))]);
    let mut none = Created::default();
    init("TW", &flagged, &orca(), Some(&group(json!([{ "id": "off", "name": "Host Offline", "parent": { "id": "g" } }]))), &mut flags(None, true), &mut none, &config).unwrap();
    assert!(none.0.is_empty());
}

// Without a team default, a project nothing maps gets its proposed repo by ID; a project named
// after its repo, or finished, gets none; then the default merges, every other setting kept, and
// a second run changes nothing.
#[test]
fn merges_the_teams_repos_keeping_every_other_setting() {
    let config = file("merge", json!({ "owner": { "id": "u-1" }, "triage": { "batch": 2 }, "repos": { "Docs": { "projects": ["Manual"], "recipe": { "id": "cloud", "machines": 1, "slotsEach": 1 } } } }));
    let done = init("TW", &team(json!([])), &orca(), None, &mut flags(Some("none"), true), &mut Created::default(), &config).unwrap();
    assert!(done.changed);
    let done = init("TW", &team(json!([])), &orca(), None, &mut flags(Some("docs"), false), &mut Created::default(), &config).unwrap();
    assert!(done.changed);
    let written: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    assert_eq!(written, json!({
        "owner": { "id": "u-1" }, "triage": { "batch": 2 },
        "repos": {
            "Docs": { "projects": ["Manual"], "recipe": { "id": "cloud", "machines": 1, "slotsEach": 1 }, "teams": ["TW"] },
            "Site": { "projects": ["p2"] },
        },
    }));
    let bytes = fs::read(&config).unwrap();
    let again = init("TW", &team(json!([])), &orca(), None, &mut flags(None, false), &mut Created::default(), &config).unwrap();
    assert!(!again.changed);
    assert_eq!(fs::read(&config).unwrap(), bytes);
}

// An answer only fills in a setting still unset when the file is written.
#[test]
fn a_setting_set_meanwhile_is_kept() {
    let edits = vec![("Site".to_string(), Change { reviewers: Some(vec!["bob".into()]), max_rounds: Some(2), ..Change::default() })];
    let current = json!({ "site": { "review": { "reviewers": ["alice"] } } });
    let kept = unset(Some(&current), &edits);
    assert_eq!((kept[0].1.reviewers.clone(), kept[0].1.max_rounds), (None, Some(2)));
}

// Without a terminal, a missing --default-repo, or --yes for a project nothing maps, exits 2
// before any label or setting is written.
#[test]
fn a_missing_flag_without_a_terminal_writes_nothing() {
    let config = file("usage", json!({ "ownerId": "u-1" }));
    let bytes = fs::read(&config).unwrap();
    for mut owner in [flags(None, true), flags(Some("none"), false)] {
        let mut created = Created::default();
        assert!(matches!(init("TW", &team(json!([])), &orca(), None, &mut owner, &mut created, &config), Err(Error::Usage(_))));
        assert!(created.0.is_empty());
        assert_eq!(fs::read(&config).unwrap(), bytes);
    }
}

#[test]
fn proposes_the_one_repo_a_project_is_named_for() {
    assert_eq!(propose("Site redesign", &orca()), Some("Site"));
    assert_eq!(propose("Tolkster Docs", &orca()), None);
    assert_eq!(propose("Billing", &orca()), None);
    assert_eq!(propose("Websites", &orca()), None);
}
