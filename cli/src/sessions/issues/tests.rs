use serde_json::Value;

use super::*;
use crate::request::tests::{APP, OWNER, at};
use crate::json::decode;

// Each recorded answer is decoded into its query's shape first.
fn parse_tolksters(data: &Value, viewer_id: &str, owner_id: &str) -> Result<Tolksters, Error> {
    let mut data = data.clone();
    data["viewer"] = json!({ "id": viewer_id });
    Ok(super::parse_tolksters(&decode(data, "Linear's answer to Linked")?, owner_id))
}

fn parse_description_edits(data: &Value) -> Vec<String> {
    super::parse_description_edits(&decode(data.clone(), "Linear's answer to History").unwrap())
}

fn parse_session(node: &Value) -> Result<Session, Error> {
    Ok(super::parse_session(&decode(node.clone(), "Linear's answer to Linked")?))
}

fn parse_thread(data: &Value, owner_id: &str) -> Result<Thread, &'static str> {
    super::parse_thread(&decode(data.clone(), "Linear's answer to Thread").unwrap(), owner_id)
}

#[test]
fn tolksters_issues_are_delegated_to_it_or_have_a_session_the_owner_started() {
    let session = |app: &str, creator: Option<&str>, mentioned_by: Option<&str>| {
        json!({ "id": "s", "status": "complete", "createdAt": at(0), "appUser": { "id": app }, "creator": creator.map(|c| json!({ "id": c })), "comment": { "user": mentioned_by.map(|m| json!({ "id": m })) } })
    };
    let issue = |identifier: &str, delegate: Option<&str>, sessions: Vec<Value>| {
        json!({ "identifier": identifier, "delegate": delegate.map(|d| json!({ "id": d })), "state": { "type": "started" }, "completedAt": null, "canceledAt": null, "agentSessions": { "nodes": sessions } })
    };
    let data = json!({ "issues": { "nodes": [
        issue("TW-1", Some("app"), vec![]),
        // A mention: never delegated, but the owner started a session with Tolkster.
        issue("TW-2", None, vec![session("app", Some("owner"), Some("owner"))]),
        issue("TW-3", Some("someone"), vec![session("app", Some("owner"), None)]),
        // Sessions someone else started, with no creator, or with another app.
        issue("TW-4", None, vec![session("app", Some("stranger"), None), session("app", None, None), session("other-app", Some("owner"), None)]),
        issue("tw-5", None, vec![]),
        // As `Session::is_owners` has it: a mention someone else wrote in a session Linear
        // names the owner as creator of is theirs, not the owner's.
        issue("TW-6", None, vec![session("app", Some("owner"), Some("stranger"))]),
    ] } });
    let mut picked: Vec<String> = parse_tolksters(&data, "app", "owner").unwrap().into_keys().collect();
    picked.sort();
    assert_eq!(picked, ["TW-1", "TW-2", "TW-3"]);
}

#[test]
fn an_issue_is_finished_as_its_state_says_now_with_when() {
    const AT: &str = "2026-09-01T00:00:00.000Z";
    let issue = |identifier: &str, state: &str, at: Option<&str>| json!({ "identifier": identifier, "delegate": { "id": "app" }, "state": { "type": state }, "canceledAt": at, "completedAt": at, "agentSessions": { "nodes": [] } });
    let data = json!({ "issues": { "nodes": [
        issue("TW-1", "canceled", Some(AT)),
        // Reopened: Linear may keep the old times, but the state says it's live.
        issue("TW-2", "started", Some(AT)),
        issue("TW-3", "completed", Some(AT)),
        // Finished, but with no time to go by.
        issue("TW-4", "completed", None),
        issue("TW-5", "canceled", None),
    ] } });
    let picked = parse_tolksters(&data, "app", "owner").unwrap();
    assert_eq!(picked["TW-1"], IssueNow::Cancelled(AT.into()));
    assert_eq!(picked["TW-2"], IssueNow::Open);
    assert_eq!(picked["TW-3"], IssueNow::Completed(AT.into()));
    assert_eq!(picked["TW-4"], IssueNow::Open);
    assert_eq!(picked["TW-5"], IssueNow::Open);
}

#[test]
fn only_description_edits_count() {
    let data = json!({ "issue": { "history": { "nodes": [
        { "createdAt": "2026-09-27T02:00:00.000Z", "updatedDescription": null },
        { "createdAt": "2026-09-27T01:00:00.000Z", "updatedDescription": true },
        { "createdAt": "2026-09-26T05:00:00.000Z", "updatedDescription": true },
        { "createdAt": "2026-09-27T03:00:00.000Z", "updatedDescription": false },
    ] } } });
    assert_eq!(parse_description_edits(&data), ["2026-09-27T01:00:00.000Z", "2026-09-26T05:00:00.000Z"]);
    assert_eq!(crate::setup::baseline("2026-09-26T01:00:00.000Z", &parse_description_edits(&data)), "2026-09-27T01:00:00.000Z");
    assert!(parse_description_edits(&json!({ "issue": null })).is_empty());
}

#[test]
fn an_issue_is_tolksters_when_delegated_or_a_session_of_its_own_is_the_owners() {
    let session = |creator: &str| parse_session(&json!({ "id": "s", "status": "complete", "createdAt": at(0), "appUser": { "id": "app" }, "creator": { "id": creator }, "comment": { "user": null } })).unwrap();
    let (owners, strangers) = (session("owner"), session("stranger"));
    assert!(is_tolksters(Some("app"), "app", [], "owner"));
    assert!(!is_tolksters(Some("someone"), "app", [], "owner"));
    assert!(!is_tolksters(None, "app", [], "owner"));
    assert!(is_tolksters(Some("someone"), "app", [&strangers, &owners], "owner"));
    assert!(!is_tolksters(None, "app", [&strangers], "owner"));
}

// Tolkster's comment `c1` on TW-1, as sessions::read_thread reads it, with replies by
// (id, author, integration).
fn thread_data(replies: &[(&str, &str, bool)]) -> Value {
    let replies: Vec<Value> = replies
        .iter()
        .enumerate()
        .map(|(n, (id, user, bot))| json!({ "id": id, "createdAt": at(9 - n as u32), "user": { "id": user }, "botActor": if *bot { json!({ "id": "b1" }) } else { Value::Null }, "body": format!("reply {id}") }))
        .collect();
    json!({
        "viewer": { "id": APP },
        "comment": {
            "id": "c1", "createdAt": at(0), "user": { "id": APP }, "botActor": null, "parent": null,
            "issue": { "id": "i1", "identifier": "tw-1", "team": { "key": "TW" }, "project": null, "agentSessions": { "nodes": [
                { "appUser": { "id": APP }, "comment": { "id": "c-other" } },
            ] } },
            "children": { "nodes": replies },
        },
    })
}

#[test]
fn a_thread_keeps_the_owners_own_replies_oldest_first() {
    let thread = parse_thread(&thread_data(&[("r3", OWNER, false), ("r2", "stranger", false), ("r1", OWNER, true), ("r0", OWNER, false)]), OWNER).unwrap();
    assert_eq!((thread.root.as_str(), thread.issue_id.as_str(), thread.identifier.as_str(), thread.place.team.as_deref()), ("c1", "i1", "TW-1", Some("TW")));
    let replies: Vec<(&str, &str)> = thread.replies.iter().map(|r| (r.id.as_str(), r.body.as_str())).collect();
    assert_eq!(replies, [("r0", "reply r0"), ("r3", "reply r3")]);
    assert_eq!(thread.replies_after(Some(&at(6))).len(), 1);
    // A reply whose ID couldn't be a claim's or a command's is left out.
    assert!(parse_thread(&thread_data(&[("r 1", OWNER, false)]), OWNER).unwrap().replies.is_empty());
}

#[test]
fn the_owners_own_comment_on_an_issue_tolkster_created_roots_a_thread_and_is_its_first_request() {
    let mut data = thread_data(&[("r0", OWNER, false)]);
    let root = &mut data["comment"];
    (root["user"], root["createdAt"], root["body"], root["issue"]["creator"]) = (json!({ "id": OWNER }), json!(at(1)), json!("the root"), json!({ "id": APP }));
    let thread = parse_thread(&data, OWNER).unwrap();
    let replies: Vec<(&str, &str)> = thread.replies.iter().map(|r| (r.id.as_str(), r.body.as_str())).collect();
    assert_eq!(replies, [("c1", "the root"), ("r0", "reply r0")]);
}

#[test]
fn only_a_thread_of_tolksters_own_comment_that_no_session_began_on_is_one() {
    let mut someones = thread_data(&[]);
    someones["comment"]["user"]["id"] = json!(OWNER);
    assert_eq!(parse_thread(&someones, OWNER), Err("not a thread of my comment"));
    // Nor an integration's posting as the owner, on an issue Tolkster created.
    someones["comment"]["issue"]["creator"] = json!({ "id": APP });
    someones["comment"]["botActor"] = json!({ "id": "b1" });
    assert_eq!(parse_thread(&someones, OWNER), Err("not a thread of my comment"));
    let mut a_reply = thread_data(&[]);
    a_reply["comment"]["parent"] = json!({ "id": "c0" });
    assert_eq!(parse_thread(&a_reply, OWNER), Err("not a thread of my comment"));
    let mut a_sessions = thread_data(&[]);
    a_sessions["comment"]["issue"]["agentSessions"]["nodes"][0]["comment"]["id"] = json!("c1");
    assert_eq!(parse_thread(&a_sessions, OWNER), Err("a session's thread"));
    let mut off = thread_data(&[]);
    off["comment"]["issue"] = Value::Null;
    assert_eq!(parse_thread(&off, OWNER), Err("not on an issue"));
    assert_eq!(parse_thread(&thread_data(&[]), ""), Err("no owner"));
}
