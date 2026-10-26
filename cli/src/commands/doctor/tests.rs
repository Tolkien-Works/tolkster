use serde_json::{Value, json};

use super::*;
use crate::exec::Output;
use crate::ports::fake::FakeReader;

fn read(data: &Value) -> TeamRead {
    crate::json::decode(data.clone(), "Linear's answer to Doctor").unwrap()
}

fn team_checks(key: &str, data: &Value) -> Vec<Check> {
    super::team_checks(key, &read(data))
}

fn mapping_checks(map: &RepoMap, repos: &[crate::orca::Repo], key: &str, team: &Value) -> (Vec<Check>, Vec<String>) {
    super::mapping_checks(map, repos, key, &crate::json::decode(team["projects"].clone(), "Linear's answer to Doctor").unwrap())
}

// A team as `workspace::read_team` reads it, ready in every way, with `labels` as the workspace's.
fn team(states: Value, labels: Value) -> Value {
    json!({
        "teams": { "nodes": [{
            "id": "t1", "key": "TW", "triageEnabled": true,
            "issueEstimationType": "tShirt", "issueEstimationExtended": false, "issueEstimationAllowZero": false,
            "states": { "nodes": states },
            "projects": { "nodes": [{ "id": "p1", "name": "Tolkster" }, { "id": "p2", "name": "Website" }], "pageInfo": { "hasNextPage": false } },
        }] },
        "issueLabels": { "nodes": labels },
    })
}

fn states() -> Value {
    json!([
        { "id": "s1", "name": "Triage", "type": "triage", "position": 0 },
        { "id": "s2", "name": "Backlog", "type": "backlog", "position": 1 },
        { "id": "s3", "name": "In Progress", "type": "started", "position": 2 },
        { "id": "s4", "name": "In Review", "type": "started", "position": 3 },
    ])
}

fn labels() -> Value {
    json!([{ "name": "plan-first", "team": null }, { "name": "Manual-Merge", "team": { "key": "tw" } }])
}

fn ready() -> Value {
    team(states(), labels())
}

// The verdict of the check named `name`, and its reason.
fn verdict(checks: &[Check], name: &str) -> (Verdict, String) {
    let found = checks.iter().find(|c| c.check == name).unwrap_or_else(|| panic!("no {name} check in {checks:?}"));
    (found.verdict, found.reason.clone())
}

fn repos() -> Vec<crate::orca::Repo> {
    crate::orca::parse_repos(orca()).unwrap()
}

fn orca() -> Value {
    json!({ "result": { "repos": [
        { "id": "r1", "displayName": "Tolkster", "path": "D:/src/r1", "gitRemoteIdentity": null },
        { "id": "r2", "displayName": "Site", "path": "D:/src/r2", "gitRemoteIdentity": null },
        { "id": "r3", "displayName": "Twice", "path": "D:/src/r3", "gitRemoteIdentity": null },
        { "id": "r4", "displayName": "twice", "path": "D:/src/r4", "gitRemoteIdentity": null },
    ] } })
}

fn primary(repo_id: &str, origin: Option<&str>) -> Worktree {
    Worktree { id: format!("{repo_id}::main"), repo_id: repo_id.into(), path: format!("D:/src/{repo_id}"), branch: Some("main".into()), linked_issue: None, is_main: true, is_local: true, agents: Vec::new(), origin: origin.map(String::from) }
}

// GitHub as the app reads a repo: `repository` for the GraphQL query, `rules` for the
// branch's rules and `branch` for its classic protection.
fn github<'a>(repository: Value, rules: Value, branch: Value) -> FakeReader<'a> {
    FakeReader::new(move |path, _| {
        Ok(match path {
            "graphql" => json!({ "repository": repository }),
            "app" => json!({ "events": GITHUB_EVENTS }),
            p if p.starts_with("rules/branches/") => rules.clone(),
            _ => branch.clone(),
        })
    })
}

fn repository() -> Value {
    json!({ "autoMergeAllowed": true, "defaultBranchRef": { "name": "main" }, "agents": { "__typename": "Blob" }, "claude": null })
}

fn required() -> Value {
    json!([{ "type": "pull_request" }, { "type": "required_status_checks", "parameters": {} }])
}

#[test]
fn a_ready_team_passes_every_check() {
    let checks = team_checks("TW", &ready());
    assert!(checks.iter().all(|c| c.verdict == Verdict::Ok && c.area == "team" && c.subject == "TW"), "{checks:?}");
    let names: Vec<&str> = checks.iter().map(|c| c.check).collect();
    assert_eq!(names, ["access", "started-state", "review-state", "triage-state", "estimates", "plan-first-label", "manual-merge-label"]);
    assert_eq!(verdict(&checks, "estimates").1, "sized on 1 (XS), 2 (S), 3 (M), 5 (L) or 8 (XL)");
    assert!(verdict(&checks, "triage-state").1.contains("triage state"));
}

#[test]
fn a_team_the_app_isnt_granted_fails_and_ends_its_checks() {
    let checks = team_checks("OPS", &json!({ "teams": { "nodes": [] }, "issueLabels": { "nodes": [] } }));
    assert_eq!(checks.len(), 1);
    assert_eq!(verdict(&checks, "access").0, Verdict::Fail);
}

#[test]
fn a_team_without_a_started_or_a_review_state_fails() {
    let no_review = team(json!([{ "id": "s3", "name": "In Progress", "type": "started", "position": 2 }, { "id": "s2", "name": "Backlog", "type": "backlog", "position": 1 }]), labels());
    let checks = team_checks("TW", &no_review);
    assert_eq!(verdict(&checks, "started-state").0, Verdict::Ok);
    assert_eq!(verdict(&checks, "review-state").0, Verdict::Fail);
    // A state named for review that isn't started doesn't count.
    let unstarted = team(json!([{ "id": "s1", "name": "Review queue", "type": "unstarted", "position": 1 }]), labels());
    let checks = team_checks("TW", &unstarted);
    assert_eq!(verdict(&checks, "started-state").0, Verdict::Fail);
    assert_eq!(verdict(&checks, "review-state").0, Verdict::Fail);
    // A review state first among the started ones is where work starts.
    let review_first = team(json!([{ "id": "s4", "name": "In Review", "type": "started", "position": 1 }, { "id": "s3", "name": "In Progress", "type": "started", "position": 5 }]), labels());
    assert_eq!(verdict(&team_checks("TW", &review_first), "review-state").0, Verdict::Fail);
}

#[test]
fn follow_ups_go_to_triage_or_else_the_backlog() {
    let mut off = ready();
    off["teams"]["nodes"][0]["triageEnabled"] = json!(false);
    let (verdict_off, reason) = verdict(&team_checks("TW", &off), "triage-state");
    assert_eq!(verdict_off, Verdict::Ok);
    assert!(reason.contains("backlog"), "{reason}");
    let neither = team(json!([{ "id": "s3", "name": "Doing", "type": "started", "position": 2 }]), labels());
    assert_eq!(verdict(&team_checks("TW", &neither), "triage-state").0, Verdict::Fail);
}

#[test]
fn a_team_without_estimates_is_fine_and_an_unknown_type_warns() {
    let mut unused = ready();
    unused["teams"]["nodes"][0]["issueEstimationType"] = json!("notUsed");
    assert_eq!(verdict(&team_checks("TW", &unused), "estimates").0, Verdict::Ok);
    let mut unknown = ready();
    unknown["teams"]["nodes"][0]["issueEstimationType"] = json!("cubits");
    assert_eq!(verdict(&team_checks("TW", &unknown), "estimates").0, Verdict::Warn);
}

#[test]
fn a_missing_plan_first_label_warns_a_missing_manual_merge_fails_and_another_teams_label_doesnt_count() {
    let checks = team_checks("TW", &team(states(), json!([{ "name": "manual-merge", "team": { "key": "OPS" } }])));
    assert_eq!(verdict(&checks, "plan-first-label").0, Verdict::Warn);
    assert_eq!(verdict(&checks, "manual-merge-label").0, Verdict::Fail);
}

#[test]
fn a_reason_never_holds_linear_text() {
    let secret = "Zanzibar";
    let named = team(json!([{ "id": "s3", "name": format!("{secret} review"), "type": "started", "position": 2 }, { "id": "s2", "name": secret, "type": "backlog", "position": 1 }]), json!([{ "name": "plan-first", "team": null }]));
    let mut named = named;
    named["teams"]["nodes"][0]["projects"]["nodes"] = json!([{ "id": "p9", "name": secret }]);
    let mut checks = team_checks("TW", &named);
    checks.extend(mapping_checks(&RepoMap::default(), &repos(), "TW", &named["teams"]["nodes"][0]).0);
    let printed = serde_json::to_string(&checks).unwrap();
    assert!(!printed.contains(secret), "{printed}");
}

#[test]
fn each_project_and_the_team_default_resolve_through_the_mapping() {
    let map = RepoMap::from_config(Some(&json!({ "Site": { "projects": ["p2"] } }))).unwrap();
    let data = ready();
    let (checks, reached) = mapping_checks(&map, &repos(), "TW", &data["teams"]["nodes"][0]);
    let by: Vec<(&str, &str, Verdict)> = checks.iter().map(|c| (c.check, c.subject.as_str(), c.verdict)).collect();
    // p1 is named after an Orca repo; p2 is mapped; the team has no default.
    assert_eq!(by, [("project", "p1", Verdict::Ok), ("project", "p2", Verdict::Ok), ("team-default", "TW", Verdict::Warn)]);
    assert!(checks[1].reason.ends_with("the repo Site"), "{}", checks[1].reason);
    assert_eq!(reached, ["Tolkster", "Site"]);
}

#[test]
fn a_mapping_that_points_nowhere_fails_where_an_unmapped_project_warns() {
    let map = RepoMap::from_config(Some(&json!({ "Gone": { "teams": ["TW"] }, "Twice": { "projects": ["p1"] } }))).unwrap();
    let mut data = ready();
    data["teams"]["nodes"][0]["projects"] = json!({ "nodes": [{ "id": "p1", "name": "Tolkster" }], "pageInfo": { "hasNextPage": true } });
    let (checks, reached) = mapping_checks(&map, &repos(), "TW", &data["teams"]["nodes"][0]);
    assert_eq!(verdict(&checks, "project"), (Verdict::Fail, checks[0].reason.clone()));
    assert_eq!(verdict(&checks, "team-default").0, Verdict::Fail);
    assert_eq!(verdict(&checks, "projects").0, Verdict::Warn);
    assert!(reached.is_empty());
    let (checks, _) = mapping_checks(&RepoMap::default(), &repos(), "TW", &json!({ "projects": { "nodes": [{ "id": "p5", "name": "Nothing" }] } }));
    assert_eq!(verdict(&checks, "project").0, Verdict::Warn);
}

#[test]
fn a_finished_project_isnt_checked() {
    let map = RepoMap::from_config(Some(&json!({ "Gone": { "projects": ["p1", "p2"] } }))).unwrap();
    let projects = json!({ "projects": { "nodes": [{ "id": "p1", "name": "Old", "completedAt": "2026-01-01T00:00:00.000Z" }, { "id": "p2", "name": "Dropped", "canceledAt": "2026-01-01T00:00:00.000Z" }] } });
    let (checks, _) = mapping_checks(&map, &repos(), "TW", &projects);
    assert_eq!(checks.iter().map(|c| c.check).collect::<Vec<_>>(), ["team-default"]);
}

#[test]
fn a_ready_repo_passes_every_check() {
    let mut gh = github(repository(), required(), json!({}));
    let checks = repo_checks("tolkster", &repos(), &[primary("r1", Some("owner/tolkster"))], &mut gh);
    assert!(checks.iter().all(|c| c.verdict == Verdict::Ok && c.area == "repo" && c.subject == "tolkster"), "{checks:?}");
    let names: Vec<&str> = checks.iter().map(|c| c.check).collect();
    assert_eq!(names, ["orca", "checkout", "origin", "github-app", "default-branch", "auto-merge", "required-check", "instructions"]);
    assert_eq!(gh.calls, ["graphql owner/tolkster {}", "get owner/tolkster rules/branches/main?per_page=100"]);
}

#[test]
fn each_repo_check_that_fails_ends_the_ones_after_it() {
    let last = |checks: Vec<Check>| checks.last().map(|c| (c.check, c.verdict)).unwrap();
    let mut gh = github(repository(), required(), json!({}));
    assert_eq!(last(repo_checks("Nowhere", &repos(), &[], &mut gh)), ("orca", Verdict::Fail));
    assert_eq!(last(repo_checks("Twice", &repos(), &[], &mut gh)), ("orca", Verdict::Fail));
    assert_eq!(last(repo_checks("Site", &repos(), &[primary("r1", Some("o/t"))], &mut gh)), ("checkout", Verdict::Fail));
    let elsewhere = Worktree { is_local: false, ..primary("r2", Some("o/site")) };
    assert_eq!(last(repo_checks("Site", &repos(), &[elsewhere], &mut gh)), ("checkout", Verdict::Fail));
    assert_eq!(last(repo_checks("Site", &repos(), &[primary("r2", None)], &mut gh)), ("origin", Verdict::Fail));
    assert!(gh.calls.is_empty());
    let mut refused = FakeReader::new(|_, _| Err(Error::Failed("GitHub refused an installation token: HTTP 422".into())));
    let checks = repo_checks("Site", &repos(), &[primary("r2", Some("o/site"))], &mut refused);
    assert_eq!(verdict(&checks, "github-app").0, Verdict::Fail);
    assert!(checks.last().unwrap().reason.contains("HTTP 422"));
    let mut foreign = FakeReader::new(|_, _| Err(Error::Failed("the GitHub App's token does not cover other/site".into())));
    let checks = repo_checks("Site", &repos(), &[primary("r2", Some("other/site"))], &mut foreign);
    assert_eq!(verdict(&checks, "github-app").0, Verdict::Fail);
    assert!(checks.last().unwrap().reason.contains("does not cover other/site"));
    let mut empty = github(json!({ "autoMergeAllowed": false, "defaultBranchRef": null, "agents": null, "claude": null }), json!([]), json!({}));
    assert_eq!(last(repo_checks("Site", &repos(), &[primary("r2", Some("o/site"))], &mut empty)), ("default-branch", Verdict::Fail));
    let mut unreadable = github(Value::Null, json!([]), json!({}));
    assert_eq!(last(repo_checks("Site", &repos(), &[primary("r2", Some("o/site"))], &mut unreadable)), ("github-app", Verdict::Fail));
}

#[test]
fn auto_merge_off_fails_and_no_required_check_or_instructions_warns() {
    let bare = json!({ "autoMergeAllowed": false, "defaultBranchRef": { "name": "trunk" }, "agents": null, "claude": null });
    let unprotected = json!({ "protection": { "enabled": false, "required_status_checks": { "enforcement_level": "off", "contexts": [] } } });
    let mut gh = github(bare, json!([{ "type": "pull_request" }]), unprotected);
    let checks = repo_checks("Site", &repos(), &[primary("r2", Some("o/site"))], &mut gh);
    assert_eq!(verdict(&checks, "auto-merge").0, Verdict::Fail);
    assert_eq!(verdict(&checks, "required-check").0, Verdict::Warn);
    assert_eq!(verdict(&checks, "instructions").0, Verdict::Warn);
    assert_eq!(gh.calls[1..], ["get o/site rules/branches/trunk?per_page=100", "get o/site branches/trunk"]);
}

#[test]
fn rules_that_cant_be_read_warn_and_keep_the_other_checks() {
    let mut gh = FakeReader::new(|path, _| match path {
        "graphql" => Ok(json!({ "repository": repository() })),
        p if p.starts_with("rules/") => Err(Error::Failed("GitHub refused reading rules: HTTP 403".into())),
        _ => Ok(json!({ "protection": { "enabled": false } })),
    });
    let checks = repo_checks("Site", &repos(), &[primary("r2", Some("o/site"))], &mut gh);
    let (warned, reason) = verdict(&checks, "required-check");
    assert_eq!(warned, Verdict::Warn);
    assert!(reason.contains("HTTP 403"), "{reason}");
    assert_eq!(verdict(&checks, "github-app").0, Verdict::Ok);
    assert_eq!(verdict(&checks, "instructions").0, Verdict::Ok);
    // Classic protection read after all still counts.
    let protected = json!({ "protection": { "required_status_checks": { "enforcement_level": "everyone", "contexts": [], "checks": [{ "context": "ci" }] } } });
    let mut gh = FakeReader::new(move |path, _| match path {
        "graphql" => Ok(json!({ "repository": repository() })),
        p if p.starts_with("rules/") => Err(Error::Failed("HTTP 403".into())),
        _ => Ok(protected.clone()),
    });
    let checks = repo_checks("Site", &repos(), &[primary("r2", Some("o/site"))], &mut gh);
    assert_eq!(verdict(&checks, "required-check").0, Verdict::Ok);
}

#[test]
fn a_fail_reason_says_which_rule_points_nowhere() {
    let by_team = RepoMap::from_config(Some(&json!({ "Gone": { "teams": ["TW"] } }))).unwrap();
    let by_both = RepoMap::from_config(Some(&json!({ "Site": { "projects": ["p1"] }, "Tolkster": { "projects": ["Web"] } }))).unwrap();
    let team = json!({ "projects": { "nodes": [{ "id": "p1", "name": "Web" }] } });
    let reason = |map: &RepoMap| mapping_checks(map, &repos(), "TW", &team).0[0].reason.clone();
    assert!(reason(&by_team).contains("team's default"), "{}", reason(&by_team));
    assert!(reason(&by_both).contains("two entries"), "{}", reason(&by_both));
}

#[test]
fn classic_branch_protection_counts_as_a_required_check() {
    let claude_only = json!({ "autoMergeAllowed": true, "defaultBranchRef": { "name": "release/v1" }, "agents": null, "claude": { "__typename": "Blob" } });
    let protected = json!({ "protection": { "enabled": true, "required_status_checks": { "enforcement_level": "non_admins", "contexts": ["ci"], "checks": [] } } });
    let mut gh = github(claude_only, json!([]), protected);
    let checks = repo_checks("Site", &repos(), &[primary("r2", Some("o/site"))], &mut gh);
    assert!(checks.iter().all(|c| c.verdict == Verdict::Ok), "{checks:?}");
    assert_eq!(gh.calls[2], "get o/site branches/release/v1");
}

// A fake shell for Orca: its repos, and one primary checkout per repo whose origin is
// `o/<name>`, or failing when `up` is false.
fn shell(up: bool) -> impl FnMut(&str, &[&str], Option<&str>) -> Result<Output, Error> {
    move |program, args, dir| {
        let stdout = match (program, args.first().copied(), args.get(1).copied()) {
            ("orca", Some("repo"), Some("list")) => orca().to_string(),
            ("orca", Some("worktree"), Some("ps")) => json!({ "result": { "worktrees": [
                { "worktreeId": "r1::main", "repoId": "r1", "path": "D:/src/Tolkster", "isMainWorktree": true, "agents": [] },
                { "worktreeId": "r2::main", "repoId": "r2", "path": "D:/src/Site", "isMainWorktree": true, "agents": [] },
            ] } })
            .to_string(),
            ("git", Some("remote"), _) => format!("https://github.com/o/{}.git\n", dir.unwrap_or_default().rsplit('/').next().unwrap_or_default()),
            _ => String::new(),
        };
        Ok(Output { ok: up, code: Some(if up { 0 } else { 1 }), stdout, stderr: if up { String::new() } else { "orca isn't running".into() } })
    }
}

#[test]
fn a_team_alone_checks_each_repo_its_mapping_reaches() {
    let map = RepoMap::from_config(Some(&json!({ "Site": { "projects": ["p2"] }, "Tolkster": { "teams": ["TW"] } }))).unwrap();
    let mut gh = github(repository(), required(), json!({}));
    let checks = doctor(Some("TW"), None, &map, &|key| Ok(read(&if key == "TW" { ready() } else { json!({ "teams": { "nodes": [] }, "issueLabels": { "nodes": [] } }) })), &mut shell(true), &mut gh).unwrap();
    let repos: Vec<&str> = checks.iter().filter(|c| c.area == "repo" && c.check == "orca").map(|c| c.subject.as_str()).collect();
    assert_eq!(repos, ["Tolkster", "Site"]);
    assert!(checks.iter().all(|c| c.verdict == Verdict::Ok), "{checks:?}");
    assert_eq!(gh.calls[0], "graphql o/Tolkster {}");
}

#[test]
fn a_repo_alone_reads_no_linear_and_orca_down_fails_after_the_team() {
    let fail_linear = |_: &str| -> Result<TeamRead, Error> { panic!("Linear read without --team") };
    let mut gh = github(repository(), required(), json!({}));
    let checks = doctor(None, Some("Site"), &RepoMap::default(), &fail_linear, &mut shell(true), &mut gh).unwrap();
    assert!(checks.iter().all(|c| (c.area == "repo" || c.area == "app") && c.verdict == Verdict::Ok), "{checks:?}");
    let checks = doctor(Some("TW"), Some("Site"), &RepoMap::default(), &|_| Ok(read(&ready())), &mut shell(false), &mut gh).unwrap();
    let last = checks.last().unwrap();
    assert_eq!((last.area, last.check, last.verdict), ("orca", "listing", Verdict::Fail));
    assert!(checks.iter().any(|c| c.area == "team"));
    assert!(doctor(Some("TW"), None, &RepoMap::default(), &|_| Err(Error::Failed("Linear is down".into())), &mut shell(true), &mut gh).is_err());
}

#[test]
fn an_event_the_app_isnt_subscribed_to_fails_and_an_unread_app_fails_once() {
    let mut gh = FakeReader::always(json!({ "events": ["pull_request", "pull_request_review", "pull_request_review_comment", "pull_request_review_thread", "issue_comment", "push", "status"] }));
    let checks = app_checks(&mut gh);
    assert_eq!(checks.len(), GITHUB_EVENTS.len());
    let failed: Vec<&str> = checks.iter().filter(|c| c.verdict == Verdict::Fail).map(|c| c.subject.as_str()).collect();
    assert_eq!(failed, ["check_suite"]);
    assert!(checks.iter().all(|c| c.area == "app" && c.check == "event"), "{checks:?}");
    let mut gh = FakeReader::new(|_, _| Err(Error::Failed("no GitHub App".into())));
    let checks = app_checks(&mut gh);
    assert_eq!(checks.iter().map(|c| (c.area, c.check, c.verdict)).collect::<Vec<_>>(), [("app", "events", Verdict::Fail)]);
}

// A missing Worker secret, or a refused ask, fails only a worker host, which can't start
// `claude` without it.
#[test]
fn a_missing_worker_secret_fails_a_worker_host_alone() {
    let missing = || Ok(vec!["CLAUDE_TOKEN".to_string()]);
    assert_eq!(secrets_check(missing(), true).verdict, Verdict::Fail);
    assert_eq!(secrets_check(missing(), false).verdict, Verdict::Warn);
    assert!(secrets_check(missing(), true).reason.contains("CLAUDE_TOKEN"));
    assert_eq!(secrets_check(Ok(Vec::new()), true).verdict, Verdict::Ok);
    // Every host's GitHub tokens are minted with the app's key, the `listen` host's too.
    assert_eq!(secrets_check(Ok(vec!["GITHUB_APP_KEY".into()]), false).verdict, Verdict::Fail);
    assert_eq!(secrets_check(Err(Error::Failed("refused".into())), true).verdict, Verdict::Fail);
    assert_eq!(secrets_check(Err(Error::Failed("down".into())), false).verdict, Verdict::Warn);
}

#[test]
fn a_harness_that_cant_run_unattended_safely_fails() {
    use crate::harnesses::tests::{BARE, STUB};
    let checks = harness_checks(&[&crate::harnesses::CLAUDE, &STUB, &BARE]);
    let by: Vec<(&str, Verdict)> = checks.iter().map(|c| (c.subject.as_str(), c.verdict)).collect();
    assert_eq!(by, [("claude", Verdict::Ok), ("stub", Verdict::Ok), ("bare", Verdict::Fail)]);
    assert!(checks.iter().all(|c| c.area == "harness" && c.check == "unattended"), "{checks:?}");
    assert!(checks[2].reason.contains("MCP"), "{}", checks[2].reason);
}

// One check for the workspace, and only a warning: without the group nothing is labelled.
#[test]
fn the_label_group_is_checked_once_for_the_workspace() {
    let found = Group::parse(&crate::json::decode(json!({ "issueLabels": { "nodes": [
        { "id": "g", "name": "tolkster", "isGroup": true, "parent": null },
        { "id": "o", "name": "host offline", "isGroup": false, "parent": { "id": "g" } },
    ] } }), "Linear's answer to Group").unwrap());
    assert_eq!(group_check(Ok(found)).verdict, Verdict::Ok);
    let bare = Group::parse(&crate::json::decode(json!({ "issueLabels": { "nodes": [{ "id": "g", "name": "tolkster", "isGroup": true, "parent": null }] } }), "Linear's answer to Group").unwrap());
    assert_eq!(group_check(Ok(bare)).verdict, Verdict::Warn);
    assert_eq!(group_check(Ok(None)).verdict, Verdict::Warn);
    assert_eq!(group_check(Err(Error::Failed("down".into()))).verdict, Verdict::Warn);
    assert_eq!(group_check(Ok(None)).area, "workspace");
}
