use serde_json::{Value, json};

use super::*;
use crate::orca::{Repo, find_repo};

fn orca() -> Vec<Repo> {
    crate::orca::parse_repos(json!({ "result": { "repos": [
        { "id": "r1", "displayName": "Tolkster", "path": "/src/r1" },
        { "id": "r2", "displayName": "Plan Portal", "path": "/src/r2" },
        { "id": "r3", "displayName": "Site", "path": "/src/r3" },
        { "id": "r4", "displayName": "Twice", "path": "/src/r4" },
        { "id": "r5", "displayName": "twice", "path": "/src/r5" },
    ] } }))
    .unwrap()
}

fn map(repos: Value) -> RepoMap {
    RepoMap::from_config(Some(&repos)).unwrap()
}

fn place(team: Option<&str>, project_id: Option<&str>, project: Option<&str>) -> Place {
    Place { team: team.map(String::from), project_id: project_id.map(String::from), project: project.map(String::from) }
}

// What an issue at `place` runs in, looked up in `orca()` as callers do.
fn resolve(map: &RepoMap, place: &Place) -> Option<(String, String)> {
    find_repo(&orca(), map.repo_for(place))
}

fn found(id: &str, name: &str) -> Option<(String, String)> {
    Some((id.into(), name.into()))
}

#[test]
fn with_no_repos_set_a_project_named_after_a_repo_is_the_only_way() {
    let none = RepoMap::from_config(None).unwrap();
    assert_eq!(none, RepoMap::default());
    assert_eq!(resolve(&none, &place(Some("TW"), Some("p1"), Some("tolkster"))), found("r1", "Tolkster"));
    assert_eq!(resolve(&none, &place(Some("TW"), Some("p1"), Some("Portal v2"))), None);
    assert_eq!(resolve(&none, &place(Some("TW"), None, None)), None);
}

#[test]
fn a_project_maps_by_its_id_or_its_name() {
    let repos = map(json!({ "Plan Portal": { "projects": ["portal v2", "p-id-9"] } }));
    assert_eq!(resolve(&repos, &place(None, Some("p1"), Some("Portal V2"))), found("r2", "Plan Portal"));
    assert_eq!(resolve(&repos, &place(None, Some("p-id-9"), Some("Renamed"))), found("r2", "Plan Portal"));
}

#[test]
fn a_team_maps_to_its_default_repo() {
    let repos = map(json!({ "Site": { "teams": ["web"] } }));
    assert_eq!(resolve(&repos, &place(Some("WEB"), None, None)), found("r3", "Site"));
    assert_eq!(resolve(&repos, &place(Some("OPS"), None, None)), None);
}

#[test]
fn the_project_comes_before_the_team_and_the_team_before_the_project_name() {
    let repos = map(json!({
        "Plan Portal": { "projects": ["Portal v2"] },
        "Site": { "teams": ["WEB"] },
    }));
    assert_eq!(resolve(&repos, &place(Some("WEB"), None, Some("Portal v2"))), found("r2", "Plan Portal"));
    assert_eq!(resolve(&repos, &place(Some("WEB"), None, Some("Tolkster"))), found("r3", "Site"));
    assert_eq!(resolve(&repos, &place(Some("TW"), None, Some("Tolkster"))), found("r1", "Tolkster"));
}

#[test]
fn a_matching_rule_decides_even_when_its_repo_cant_be_found() {
    let repos = map(json!({
        "Gone": { "projects": ["Tolkster"] },
        "Twice": { "teams": ["TW"] },
    }));
    // Not in Orca: no fallback to the team or the project's name.
    assert_eq!(resolve(&repos, &place(Some("TW"), None, Some("Tolkster"))), None);
    // Two Orca repos share the name.
    assert_eq!(resolve(&repos, &place(Some("TW"), None, Some("Other"))), None);
}

#[test]
fn a_project_two_entries_match_by_id_and_by_name_has_no_repo() {
    let repos = map(json!({
        "Plan Portal": { "projects": ["p-id-9"] },
        "Site": { "projects": ["Portal v2"] },
    }));
    assert_eq!(resolve(&repos, &place(None, Some("p-id-9"), Some("Portal v2"))), None);
}

#[test]
fn an_unmapped_issue_has_no_repo() {
    let repos = map(json!({ "Site": { "projects": ["Web"], "teams": ["WEB"] } }));
    assert_eq!(resolve(&repos, &place(Some("OPS"), Some("p1"), Some("Infra"))), None);
    assert_eq!(resolve(&repos, &Place::default()), None);
}

#[test]
fn an_entry_claims_a_place_by_its_project_or_its_team_found_or_not() {
    let repos = map(json!({ "Gone": { "projects": ["p-id-9", "Web"], "teams": ["WEB"] } }));
    assert!(repos.claims(&place(None, Some("p-id-9"), Some("Other"))));
    assert!(repos.claims(&place(None, None, Some("web"))));
    assert!(repos.claims(&place(Some("web"), None, None)));
    assert!(!repos.claims(&place(Some("OPS"), Some("p1"), Some("Tolkster"))));
    // A project named after an Orca repo reaches it, but no entry claims it.
    assert_eq!(resolve(&repos, &place(Some("OPS"), None, Some("Tolkster"))), found("r1", "Tolkster"));
}

#[test]
fn two_entries_claiming_a_project_or_a_team_is_a_config_error() {
    let error = |repos: Value| match RepoMap::from_config(Some(&repos)) {
        Err(Error::Usage(why)) => why,
        other => panic!("{other:?}"),
    };
    let project = error(json!({ "Site": { "projects": ["Web"] }, "Plan Portal": { "projects": ["web"] } }));
    assert!(project.contains("project \"web\""), "{project}");
    let team = error(json!({ "Site": { "teams": ["WEB"] }, "Plan Portal": { "teams": ["web"] } }));
    assert!(team.contains("team \"web\""), "{team}");
}

#[test]
fn entries_are_checked_for_shape() {
    for bad in [
        json!([{ "repo": "Site", "projects": ["Web"] }]),
        json!({ " ": { "projects": ["Web"] } }),
        json!({ "Site": {} }),
        json!({ "Site": "Web" }),
        json!({ "Site": { "projects": [] } }),
        json!({ "Site": { "projects": "Web" } }),
        json!({ "Site": { "teams": [""] } }),
        json!({ "Site": { "teams": [7] } }),
        json!({ "Site": { "repo": "Site", "teams": ["WEB"] } }),
        // One entry per repo, ignoring case.
        json!({ "Site": { "teams": ["WEB"] }, "site ": { "projects": ["Web"] } }),
        json!({ "Site": { "trace": "yes" } }),
    ] {
        assert!(matches!(RepoMap::from_config(Some(&bad)), Err(Error::Usage(_))), "{bad}");
    }
    assert_eq!(map(json!({ " Site ": { "projects": [" Web "], "teams": null } })), RepoMap(vec![Entry { repo: "Site".into(), projects: vec!["Web".into()], teams: vec![], review: None, recipe: None, land: Land::Pr, trace: false }]));
}

#[test]
fn an_entry_may_set_its_repos_review_settings() {
    let entry = |value: Value| map(json!({ "Site": value })).0.remove(0);
    let set = entry(json!({ "teams": ["WEB"], "review": { "reviewers": [" a-lead"], "maxRounds": 2 } }));
    assert_eq!((set.reviewers().cloned(), set.max_rounds()), (Some(vec!["a-lead".into()]), Some(2)));
    // Settings alone, for a repo reached by its project's name, or none at all.
    assert_eq!(entry(json!({ "review": { "reviewers": [] } })).reviewers(), Some(&vec![]));
    assert_eq!(entry(json!({ "review": { "maxRounds": 1 } })).max_rounds(), Some(1));
    assert!(entry(json!({ "trace": true })).trace && !entry(json!({ "trace": false })).trace && !entry(json!({ "teams": ["WEB"] })).trace);
    let unset = entry(json!({ "teams": ["WEB"], "review": { "reviewers": null } }));
    assert_eq!(unset.review, None);
    for bad in [
        json!({ "Site": { "review": { "reviewers": "a-lead" } } }),
        json!({ "Site": { "review": { "reviewers": [" "] } } }),
        json!({ "Site": { "review": { "reviewers": [7] } } }),
        json!({ "Site": { "review": { "maxRounds": 0 } } }),
        json!({ "Site": { "review": { "maxRounds": 2.5 } } }),
        json!({ "Site": { "review": { "maxRounds": "2" } } }),
        json!({ "Site": { "review": ["a"] } }),
        json!({ "Site": { "review": {} } }),
    ] {
        assert!(matches!(RepoMap::from_config(Some(&bad)), Err(Error::Usage(_))), "{bad}");
    }
}

#[test]
fn an_entry_may_opt_its_repo_in_to_recipe_machines() {
    let entry = |value: Value| map(json!({ "Site": value })).0.remove(0);
    let machines = |id: &str, machines, slots_each, idle_minutes| Some(RecipeMachines { id: id.into(), machines, slots_each, idle_minutes, destroy_after: DEFAULT_DESTROY_AFTER });
    assert_eq!(entry(json!({ "recipe": { "id": " cloud ", "machines": 2, "slotsEach": 3, "idleMinutes": 5 } })).recipe, machines("cloud", 2, 3, 5));
    assert_eq!(entry(json!({ "recipe": { "id": "cloud", "machines": 1, "slotsEach": 1, "destroyAfter": 60 } })).recipe.map(|recipe| recipe.destroy_after), Some(60));
    // Idle minutes have a default, and a recipe alone maps nothing yet is a whole entry.
    assert_eq!(entry(json!({ "recipe": { "id": "cloud", "machines": 1, "slotsEach": 1, "idleMinutes": null } })).recipe, machines("cloud", 1, 1, DEFAULT_IDLE_MINUTES));
    assert_eq!(entry(json!({ "teams": ["WEB"], "recipe": null })).recipe, None);
    for bad in [
        json!({ "Site": { "recipe": "cloud" } }),
        json!({ "Site": { "recipe": {} } }),
        json!({ "Site": { "recipe": { "machines": 1, "slotsEach": 1 } } }),
        json!({ "Site": { "recipe": { "id": "cloud", "machines": 1 } } }),
        json!({ "Site": { "recipe": { "id": "cloud", "slotsEach": 1 } } }),
        json!({ "Site": { "recipe": { "id": " ", "machines": 1, "slotsEach": 1 } } }),
        json!({ "Site": { "recipe": { "id": "cloud", "machines": 0, "slotsEach": 1 } } }),
        json!({ "Site": { "recipe": { "id": "cloud", "machines": 1, "slotsEach": 0 } } }),
        json!({ "Site": { "recipe": { "id": "cloud", "machines": 1.5, "slotsEach": 1 } } }),
        json!({ "Site": { "recipe": { "id": "cloud", "machines": 1, "slotsEach": "2" } } }),
        json!({ "Site": { "recipe": { "id": "cloud", "machines": 1, "slotsEach": 1, "idleMinutes": 0 } } }),
        json!({ "Site": { "recipe": { "id": "cloud", "machines": 1, "slotsEach": 1, "destroyAfter": 59 } } }),
        json!({ "Site": { "recipe": { "id": "cloud", "machines": 1, "slotsEach": 1, "idle": 5 } } }),
        json!({ "Site": { "recipe": { "id": "cloud", "cap": 1 } } }),
    ] {
        assert!(matches!(RepoMap::from_config(Some(&bad)), Err(Error::Usage(_))), "{bad}");
    }
}

#[test]
fn an_entry_lands_through_a_pr_unless_it_says_direct() {
    let entry = |value: Value| map(json!({ "Site": value })).0.remove(0);
    assert_eq!(entry(json!({ "teams": ["WEB"] })).land, Land::Pr);
    assert_eq!(entry(json!({ "land": "direct" })).land, Land::Direct);
    assert_eq!(entry(json!({ "teams": ["WEB"], "land": null })).land, Land::Pr);
    for bad in [json!({ "Site": { "land": "Direct" } }), json!({ "Site": { "land": true } }), json!({ "Site": { "land": null } })] {
        assert!(matches!(RepoMap::from_config(Some(&bad)), Err(Error::Usage(_))), "{bad}");
    }
}

#[test]
fn a_place_is_read_from_the_issue_as_linear_gives_it() {
    let project: crate::linear::Project = crate::json::decode(json!({ "id": "p1", "name": "Tolkster" }), "a project").unwrap();
    assert_eq!(Place::of(Some("TW"), Some(project.parts())), place(Some("TW"), Some("p1"), Some("Tolkster")));
    assert_eq!(Place::of(Some("TW"), None), place(Some("TW"), None, None));
}
