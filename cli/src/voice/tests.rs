use super::*;

#[test]
fn every_state_has_phrases_and_none_is_empty() {
    assert!(states().count() > 0);
    for state in states() {
        let list = phrases(state).unwrap();
        assert!(!list.is_empty(), "{state} has no phrases");
        for p in list {
            assert!(!p.trim().is_empty() && p.trim() == p, "{state}: {p:?}");
        }
    }
}

#[test]
fn the_states_the_code_uses_exist() {
    // The CLI's, and those the briefs name. The Worker's are checked by tsc: its
    // `phrase` takes only the file's states.
    let dir = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/briefs")).unwrap();
    let briefs = dir.map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap()).collect::<Vec<_>>().join("\n");
    let mut named: Vec<String> = vec![LOOKING.into(), READING_REPLY.into(), READING_ISSUE.into()];
    named.extend(briefs.split("--state ").skip(1).map(|rest| rest.chars().take_while(|c| c.is_ascii_lowercase() || *c == '-').collect()));
    for state in named.iter().filter(|s| !s.is_empty() && *s != "name") {
        assert!(phrases(state).is_some(), "{state} isn't in src/shared/voice.json");
    }
}

#[test]
fn a_phrase_comes_from_its_states_list() {
    for state in states() {
        for _ in 0..20 {
            let p = phrase(state).unwrap();
            assert!(phrases(state).unwrap().iter().any(|x| x == p), "{state}: {p}");
        }
    }
    assert_eq!(phrase("no-such-state"), None);
}

#[test]
fn the_worker_reads_the_same_file() {
    // The parity test, like SCOPES's: the Worker imports this very file.
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../src/shared/voice.ts")).unwrap();
    assert!(source.contains(r#"import voice from "./voice.json""#), "src/shared/voice.ts must read src/shared/voice.json");
}

#[test]
fn a_tool_phrase_names_its_file_once_and_never_the_machinery() {
    assert!(tools().count() > 0);
    for kind in tools() {
        let plain = tool(kind, None).unwrap();
        let named = tool(kind, Some("cli/src/hook.rs")).unwrap();
        for p in [&plain, &named] {
            assert!(!p.is_empty() && p.trim() == p && !p.contains('{'), "{kind}: {p:?}");
            let lower = p.to_lowercase();
            assert!(["workstation", "orca", "worktree", "terminal", "the worker"].iter().all(|w| !lower.contains(w)), "{kind}: {p}");
        }
        assert!(named == plain || named.matches("cli/src/hook.rs").count() == 1, "{kind}: {named}");
    }
    assert_eq!(tool("editing", Some("a.rs")).as_deref(), Some("Editing `a.rs`"));
    assert_eq!(tool("no-such-kind", None), None);
}

#[test]
fn every_paired_kind_has_an_action_and_an_outcome() {
    assert!(actions().count() > 0);
    for kind in actions() {
        let action = action(kind).unwrap();
        assert!(!action.is_empty() && action.trim() == action && !action.ends_with('…'), "{kind}: {action:?}");
    }
    assert_eq!((outcome("testing", Some(false)), outcome("building", Some(true))), ("passed", "failed"));
    assert_eq!((outcome("pushing", Some(false)), outcome("delegating", Some(true))), ("done", "failed"));
    // An end that doesn't say how it went, as a filtered run's, only finishes.
    assert_eq!((outcome("pushing", None), outcome("testing", None)), ("finished", "finished"));
}

#[test]
fn the_sub_issues_added_line_names_only_the_parent_and_the_session() {
    let line = children_added(&IssueKey::check("TW-1").unwrap(), &"s-1".into()).unwrap();
    assert!(line.as_str().contains("tolkster issue chain TW-1 --session s-1"), "{}", line.as_str());
}
