use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};

use super::*;
use crate::error::Error;

fn file(name: &str, config: Value) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tolkster-repo-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let file = dir.join("config.json");
    fs::write(&file, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    file
}

fn read(file: &PathBuf) -> Value {
    serde_json::from_slice(&fs::read(file).unwrap()).unwrap()
}

fn projects(names: &[&str]) -> Change {
    Change { projects: names.iter().map(|n| n.to_string()).collect(), ..Change::default() }
}

// A map merges into the repo's entry, found ignoring case, and keeps every other setting, in the
// file and in the entry; the same map again leaves the file byte for byte and exits 3.
#[test]
fn map_merges_and_a_second_changes_nothing() {
    let file = file("merge", json!({
        "owner": { "id": "u-1" }, "triage": { "batch": 2 },
        "repos": { "web": { "projects": ["Site"], "recipe": { "id": "cloud", "machines": 1, "slotsEach": 1 } }, "api": { "teams": ["API"] } },
    }));
    let change = Change { projects: vec!["Shop".into(), "site".into()], teams: vec!["WEB".into()], reviewers: Some(vec![]), max_rounds: Some(2), ..Change::default() };
    assert_eq!(write(&file, "Web", Edit::Map, &change).unwrap(), 0);
    assert_eq!(read(&file), json!({
        "owner": { "id": "u-1" }, "triage": { "batch": 2 },
        "repos": {
            "web": { "projects": ["Site", "Shop"], "recipe": { "id": "cloud", "machines": 1, "slotsEach": 1 }, "teams": ["WEB"], "review": { "reviewers": [], "maxRounds": 2 } },
            "api": { "teams": ["API"] },
        },
    }));
    let written = fs::read(&file).unwrap();
    assert_eq!(write(&file, "web", Edit::Map, &change).unwrap(), NOTHING_TO_DO);
    assert_eq!(fs::read(&file).unwrap(), written);
    // A new repo gets its own entry, and a setting replaces the one its entry holds.
    assert_eq!(write(&file, "docs", Edit::Map, &projects(&["Docs"])).unwrap(), 0);
    let repos = edited(read(&file).get("repos"), "WEB", Edit::Map, &Change { max_rounds: Some(5), ..projects(&["Blog"]) }).unwrap();
    assert_eq!(repos["web"]["review"], json!({ "reviewers": [], "maxRounds": 5 }));
    assert_eq!(repos["docs"], json!({ "projects": ["Docs"] }));
}

// An entry that holds only a recipe stays when its last project is unmapped: the recipe is a setting.
#[test]
fn unmap_keeps_an_entry_that_still_sets_a_recipe() {
    let recipe = json!({ "id": "cloud", "machines": 1, "slotsEach": 1 });
    let repos = json!({ "web": { "projects": ["Site"], "recipe": recipe } });
    let kept = edited(Some(&repos), "web", Edit::Unmap, &projects(&["Site"])).unwrap();
    assert_eq!(kept, json!({ "web": { "recipe": recipe } }));
}

// A project or team already in another entry is refused with exit 2, and nothing is written.
#[test]
fn a_project_or_team_in_two_entries_writes_nothing() {
    let file = file("twice", json!({ "repos": { "api": { "projects": ["Core"], "teams": ["API"] } } }));
    let written = fs::read(&file).unwrap();
    for change in [projects(&["core"]), Change { teams: vec!["api".into()], ..Change::default() }] {
        assert!(matches!(write(&file, "web", Edit::Map, &change), Err(Error::Usage(_))));
        assert_eq!(fs::read(&file).unwrap(), written);
    }
    // The same project by its ID, as Linear pairs it with the name another entry has.
    let by_id = Change { known: vec![("p-2".into(), "Core".into())], ..projects(&["p-2"]) };
    assert!(matches!(write(&file, "web", Edit::Map, &by_id), Err(Error::Usage(_))));
    assert_eq!(write(&file, "api", Edit::Map, &by_id).unwrap(), NOTHING_TO_DO);
    assert_eq!(fs::read(&file).unwrap(), written);
}

// Unmap with flags takes out only those values, dropping an entry left with nothing; with none,
// the repo's whole entry goes, and `repos` with the last one.
#[test]
fn unmap_removes_the_values_or_the_entry() {
    let config = json!({ "repos": {
        "web": { "projects": ["Site", "Shop"], "teams": ["WEB"], "review": { "reviewers": ["a"], "maxRounds": 2 } },
        "api": { "teams": ["API"] },
    } });
    let change = Change { projects: vec!["site".into()], teams: vec!["WEB".into()], reviewers: Some(vec![]), ..Change::default() };
    assert_eq!(edited(config.get("repos"), "web", Edit::Unmap, &change).unwrap(), json!({
        "web": { "projects": ["Shop"], "review": { "maxRounds": 2 } },
        "api": { "teams": ["API"] },
    }));
    assert_eq!(edited(config.get("repos"), "api", Edit::Unmap, &Change { teams: vec!["api".into()], ..Change::default() }).unwrap()["api"], Value::Null);
    // The list `repos` once was is refused, not rewritten.
    assert!(matches!(edited(Some(&json!([{ "repo": "web", "teams": ["WEB"] }])), "web", Edit::Map, &projects(&["Site"])), Err(Error::Usage(_))));
    let file = file("unmap", config);
    assert_eq!(write(&file, "WEB", Edit::Unmap, &Change::default()).unwrap(), 0);
    assert_eq!(read(&file), json!({ "repos": { "api": { "teams": ["API"] } } }));
    assert_eq!(write(&file, "web", Edit::Unmap, &Change::default()).unwrap(), NOTHING_TO_DO);
    assert_eq!(write(&file, "api", Edit::Unmap, &Change::default()).unwrap(), 0);
    assert_eq!(read(&file), json!({}));
}

// A team the app can't see, or a project none of the given teams has, is refused; the teams
// come back by Linear's keys.
#[test]
fn teams_and_projects_are_checked_against_linear() {
    let data: Vec<crate::sessions::workspace::KeyedTeam> = crate::json::decode(json!([
        { "key": "WEB", "projects": { "nodes": [{ "id": "p-1", "name": "Site" }] } },
        { "key": "API", "projects": { "nodes": [{ "id": "p-2", "name": "Core" }] } },
    ]), "Linear's answer to RepoMap").unwrap();
    let pair = ("p-1".to_string(), "Site".to_string());
    assert_eq!(checked(&data, &Change { teams: vec!["web".into()], ..projects(&["site", "P-1"]) }).unwrap(), (vec!["WEB".to_string()], vec![pair.clone(), pair]));
    assert_eq!(checked(&data, &projects(&["Core"])).unwrap().0, Vec::<String>::new());
    assert!(checked(&data, &Change { teams: vec!["WEB".into()], ..projects(&["Core"]) }).is_err());
    assert!(checked(&data, &Change { teams: vec!["OPS".into()], ..Change::default() }).is_err());
}

#[test]
fn reviewers_are_github_logins() {
    assert!(["octo-cat", "a1", "copilot-pull-request-reviewer[bot]"].iter().all(|l| login(l)));
    assert!(["", "a b", "-x", "x-", "a--b", "[bot]", "`x`"].iter().all(|l| !login(l)));
}
