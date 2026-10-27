use std::collections::HashSet;

use serde_json::{Value, json};

use super::*;
use crate::ports::fake::{primary, row};

#[test]
fn only_a_live_agent_counts_as_running_in_a_worktree() {
    let terminals = parse_terminals(json!({ "result": { "terminals": [
        { "handle": "t1", "worktreeId": "repo::a", "agentIdentity": "claude" },
        { "handle": "t2", "worktreeId": "repo::b", "agentIdentity": "" },
        { "handle": "t3", "worktreeId": "repo::c", "agentIdentity": "claude", "orphaned": true },
        { "handle": "t4", "worktreeId": "repo::d" },
    ] } }), None).unwrap();
    assert_eq!(agent_worktrees(&terminals), HashSet::from(["repo::a".into()]));
    assert_eq!(agent_terminal(&terminals, &WorktreeId::from("repo::a")).as_deref(), Some("t1"));
    assert!(["repo::b", "repo::c", "repo::d"].iter().all(|w| agent_terminal(&terminals, &WorktreeId::from(*w)).is_none()));
    // Open terminals are any whose process is there, agent or not.
    assert_eq!(open_terminals(&terminals, None), HashSet::from(["t1".into(), "t2".into(), "t4".into()]));
    assert_eq!(open_terminals(&terminals, Some(&WorktreeId::from("repo::b"))), HashSet::from(["t2".into()]));
}

// An empty handle names Orca's active terminal: no terminal is found by one, but its agent
// still runs there, and its worktree still has a terminal open.
#[test]
fn an_agent_listed_with_an_empty_handle_runs_but_has_no_handle() {
    let terminals = parse_terminals(json!({ "result": { "terminals": [
        { "handle": "", "worktreeId": "repo::a", "agentIdentity": "claude" },
        { "worktreeId": "repo::b", "agentIdentity": "claude" },
        { "handle": "", "worktreeId": "repo::c" },
        { "handle": "t4", "worktreeId": "repo::d", "agentIdentity": "claude" },
    ] } }), None).unwrap();
    assert!(["repo::a", "repo::b", "repo::d"].iter().all(|w| agent_in(&terminals, &WorktreeId::from(*w))));
    assert!(!agent_in(&terminals, &WorktreeId::from("repo::c")));
    assert!(["repo::a", "repo::b", "repo::c"].iter().all(|w| agent_terminal(&terminals, &WorktreeId::from(*w)).is_none()));
    assert_eq!(agent_terminal(&terminals, &WorktreeId::from("repo::d")).as_deref(), Some("t4"));
    assert_eq!(open_terminals(&terminals, None), HashSet::from(["t4".into()]));
    assert!(open_terminals(&terminals, Some(&WorktreeId::from("repo::a"))).is_empty());
    let all = HashSet::from(["repo::a".into(), "repo::b".into(), "repo::d".into()]);
    assert_eq!(agent_handles(&terminals, &all), HashSet::from(["t4".into()]));
    assert!(["repo::a", "repo::c"].iter().all(|w| terminal_worktrees(&terminals).contains(*w)));
}

#[test]
fn primary_checkouts_and_rows_another_host_owns_are_not_candidates() {
    let mut main = row(Some("TW-1"), "D:/main");
    main["isMainWorktree"] = json!(true);
    let mut remote = row(Some("TW-2"), "/home/x/tw-2");
    remote["hostId"] = json!("ssh-box");
    let worktrees = Ps::read(json!({ "result": { "worktrees": [main, remote, row(Some("tw-3"), "D:/wt/tw-3")] } })).unwrap().worktrees(None);
    let picked: Vec<_> = candidates(&worktrees).iter().map(|w| w.linked_issue.clone().unwrap()).collect();
    assert_eq!(picked, ["TW-3"]);
}

#[test]
fn a_repos_primary_checkout_is_its_local_main_worktree() {
    let mut remote_main = primary();
    remote_main["worktreeId"] = json!("repo::/home/x/main");
    remote_main["hostId"] = json!("ssh-box");
    let mut other = primary();
    other["worktreeId"] = json!("other::D:/other");
    other["repoId"] = json!("other");
    let worktrees = Ps::read(json!({ "result": { "worktrees": [row(Some("TW-1"), "D:/wt/tw-1"), remote_main, other, primary()] } })).unwrap().worktrees(None);
    assert_eq!(primary_checkout(&worktrees, "repo").map(|w| w.id.as_str()), Some("repo::D:/main"));
    assert_eq!(primary_checkout(&worktrees, "other").map(|w| w.id.as_str()), Some("other::D:/other"));
    assert_eq!(primary_checkout(&worktrees, "missing"), None);
    assert_eq!(primary_checkout(&worktrees[..2], "repo"), None);
}

#[test]
fn only_a_complete_listing_says_which_worktrees_exist() {
    let listing = |truncated: Value, omitted: Value| {
        json!({ "result": { "truncated": truncated, "hostScope": { "omittedHostIds": omitted }, "worktrees": [row(Some("TW-1"), "D:/wt/tw-1"), primary()] } })
    };
    let complete = |ps: Value, host: Option<&str>| Ps::read(ps).unwrap().complete(host);
    assert_eq!(complete(listing(json!(false), json!([])), None), Some(vec!["repo::D:/wt/tw-1".into(), "repo::D:/main".into()]));
    // Another host it omits is that host's own call's to list.
    assert_eq!(complete(listing(json!(false), json!(["runtime:1"])), None).map(|ids| ids.len()), Some(2));
    assert_eq!(complete(listing(json!(false), json!([])), Some("mac")).unwrap()[0], "mac@repo::D:/wt/tw-1");
    // Cut off at its limit, or not saying: neither.
    assert_eq!(complete(listing(json!(true), json!([])), None), None);
    assert_eq!(complete(json!({ "result": { "worktrees": [] } }), None), None);
    // A row without an ID can't be matched: the listing fails, naming what it lacks.
    let mut odd = listing(json!(false), json!([]));
    odd["result"]["worktrees"][0].as_object_mut().unwrap().remove("worktreeId");
    assert_eq!(Ps::read(odd).unwrap_err().to_string(), "Orca's answer to worktree ps didn't decode: it had no `worktreeId`");
}

// A listing as Orca 1.4 prints it, with fields Tolkster doesn't read, decodes into the facts
// it does.
#[test]
fn a_recorded_listing_decodes_into_its_worktrees_and_terminals() {
    let ps = json!({ "id": "c1", "ok": true, "result": {
        "worktrees": [{
            "workspaceKind": "git", "worktreeId": "r1::D:/wt/tw-9", "repoId": "r1", "hostId": "local", "terminalPlatform": "win32",
            "repo": "tolkster", "path": "D:/wt/tw-9", "branch": "refs/heads/someone/tw-9", "isArchived": false, "isMainWorktree": false,
            "displayName": "tw-9", "comment": "", "linkedIssue": null, "linkedPR": null, "linkedLinearIssue": "tw-9",
            "liveTerminalCount": 1, "status": "working",
            "agents": [
                { "paneKey": "tab1:leaf1", "parentPaneKey": null, "state": "waiting", "agentType": "claude", "toolName": "Bash", "stateStartedAt": 1792710982042_i64, "updatedAt": 1792715409844_i64, "subagents": [] },
                { "paneKey": "", "state": "working" },
            ],
        }],
        "totalCount": 1, "truncated": false,
        "hostScope": { "hostIds": ["local"], "omittedHostIds": ["runtime:mac"], "omittedHostSelectors": [{ "hostId": "runtime:mac", "selector": null }] },
    } });
    let ps = Ps::read(ps).unwrap();
    let worktree = Worktree { id: "r1::D:/wt/tw-9".into(), repo_id: "r1".into(), path: "D:/wt/tw-9".into(), branch: Some("someone/tw-9".into()), linked_issue: Some("TW-9".into()), is_main: false, is_local: true, agents: vec![Pane { key: "tab1:leaf1".into(), state: "waiting".into(), since_ms: Some(1792710982042) }], origin: None };
    assert_eq!((ps.worktrees(None), ps.complete(None)), (vec![worktree], Some(vec!["r1::D:/wt/tw-9".into()])));
    let terminals = parse_terminals(json!({ "ok": true, "result": {
        "terminals": [
            { "handle": "term_1", "ptyId": "r1::D:/wt/tw-9@@cfa5", "incarnationId": "i1", "orphaned": false, "worktreeId": "r1::D:/wt/tw-9", "worktreePath": "D:/wt/tw-9", "branch": "refs/heads/someone/tw-9", "tabId": "tab1", "leafId": "leaf1", "connected": true, "writable": true, "lastOutputAt": 1792715418969_i64, "executionHostId": "local", "agentIdentity": "claude" },
            { "handle": "term_2", "orphaned": false, "worktreeId": "r1::D:/wt/tw-9", "tabId": "tab2", "leafId": "leaf2", "agentIdentity": null },
        ],
        "totalCount": 2, "truncated": false, "hostScope": { "hostIds": ["local"] }, "topologyRevisions": {},
    } }), Some("mac")).unwrap();
    assert_eq!(terminals[0], Terminal { handle: Some("mac@term_1".into()), worktree_id: Some("mac@r1::D:/wt/tw-9".into()), agent: true, pane: Some("tab1:leaf1".into()) });
    assert!(!terminals[1].agent);
    // A listing with no terminals list fails rather than reading as none open.
    assert_eq!(parse_terminals(json!({ "ok": true, "result": {} }), None).unwrap_err().to_string(), "Orca's answer to terminal list didn't decode: it had no `terminals`");
}

#[test]
fn a_checkout_is_placed_by_its_path_and_origin() {
    let at = |path: &str, origin: Option<&str>| Worktree { id: WorktreeId::from(format!("r1::{path}").as_str()), repo_id: "r1".into(), path: path.into(), branch: None, linked_issue: None, is_main: false, is_local: true, agents: Vec::new(), origin: origin.map(String::from) };
    let worktrees = [at("D:/wt/tw-1", Some("Tolkien-Works/tolkster")), at("D:/wt/tw-2", None)];
    let found = |top: &str, repo: &str| checkout_of(&worktrees, top, repo).map(|w| w.path.as_str());
    assert_eq!(found("D:/wt/tw-1\n", "tolkien-works/Tolkster"), Some("D:/wt/tw-1"));
    assert_eq!(found("d:\\WT\\tw-1\\", "Tolkien-Works/tolkster"), Some("D:/wt/tw-1"));
    // Another repo's PR, a checkout Orca doesn't list, or an origin that couldn't be read.
    assert_eq!(found("D:/wt/tw-1", "Tolkien-Works/site"), None);
    assert_eq!(found("D:/wt/tw-10", "Tolkien-Works/tolkster"), None);
    assert_eq!(found("D:/wt/tw-2", "Tolkien-Works/tolkster"), None);
    // A POSIX path, as on a case-sensitive volume, matches only exactly; another host's never
    // does, as `top` is a path on this machine.
    let posix = [at("/Users/x/tw-1", Some("Tolkien-Works/tolkster")), Worktree { is_local: false, ..at("/Users/x/tw-2", Some("Tolkien-Works/tolkster")) }];
    let found = |top: &str| checkout_of(&posix, top, "Tolkien-Works/tolkster").map(|w| w.path.as_str());
    assert_eq!(found("/Users/x/tw-1/"), Some("/Users/x/tw-1"));
    assert_eq!(found("/users/x/TW-1"), None);
    assert_eq!(found("/Users/x/tw-2"), None);
}

#[test]
fn only_a_workers_agent_terminals_are_waited_on() {
    let terminals = parse_terminals(json!({"result": {"terminals": [
        {"handle": "term_1", "worktreeId": "r::w", "agentIdentity": "claude"},
        {"handle": "term_2", "worktreeId": "r::w"},
        {"handle": "term_3", "worktreeId": "r::main", "agentIdentity": "claude"},
        {"handle": "", "worktreeId": "r::w", "agentIdentity": "claude"},
        {"handle": "term_4", "worktreeId": "r::w", "agentIdentity": "claude", "orphaned": true}
    ]}}), None).unwrap();
    assert_eq!(agent_handles(&terminals, &["r::w".into()].into()), ["term_1".into()].into());
}
