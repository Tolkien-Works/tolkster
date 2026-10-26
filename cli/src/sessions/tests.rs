use super::*;
use crate::request::Delegated;
use crate::request::tests::{APP, OWNER, at};

// `node` as a session query answers it, decoded first.
fn parse_session(node: &Value) -> Result<Session, Error> {
    Ok(super::parse_session(&decode(node.clone(), "Linear's answer to ReadSession")?))
}

// The issue of the session `node` with its relations read to the end, as `session_from` reads
// them, with `fetch` answering each further page's `inverseRelations`.
fn completed(node: &Value, fetch: &mut dyn FnMut(&str, &str) -> Result<Value, Error>) -> Result<Issue, Error> {
    let mut issue: IssueNode = decode(node["issue"].clone(), "Linear's answer to ReadSession")?;
    complete_relations(&mut issue, &mut |id, after| fetch(id, after).map(|page| json!({ "issue": { "inverseRelations": page } })))?;
    Ok(parse_issue(&issue, Some(APP)))
}

// A session node on TW-1, delegated to `delegate`, with the issue's `history` and its
// newest `sessions` (created, app).
fn with_history(delegate: &str, history: Value, sessions: Value) -> Value {
    json!({
        "id": "s1", "status": "complete", "createdAt": at(0), "appUser": { "id": APP }, "creator": { "id": OWNER }, "comment": null,
        "issue": { "id": "i1", "identifier": "TW-1", "url": "https://linear.app/w/issue/TW-1", "branchName": "tw-1", "createdAt": at(0), "state": { "type": "unstarted" }, "team": { "id": "t1", "key": "TW", "issueEstimationType": "fibonacci", "issueEstimationExtended": false, "issueEstimationAllowZero": false },
            "delegate": { "id": delegate }, "project": null, "history": { "nodes": history }, "agentSessions": { "nodes": sessions } },
        "activities": { "nodes": [], "pageInfo": { "hasNextPage": false } }
    })
}

fn change(minute: u32, from: Option<&str>, to: Option<&str>) -> Value {
    json!({ "createdAt": at(minute), "fromDelegate": from.map(|id| json!({ "id": id })), "toDelegate": to.map(|id| json!({ "id": id })) })
}

// When the session's issue was re-delegated, asking it again.
fn redelegated_at(node: &Value) -> Option<String> {
    parse_session(node).unwrap().issue.unwrap().delegated.filter(|d| d.again).map(|d| d.at)
}

#[test]
fn a_redelegation_is_a_delegation_back_to_whoever_had_it_taken_away() {
    let this = json!([{ "createdAt": at(0), "appUser": { "id": APP } }]);
    // Delegated once: no re-delegation.
    assert_eq!(redelegated_at(&with_history(APP, json!([change(0, None, Some(APP))]), this.clone())), None);
    // Undelegated, then delegated back, newest first or not.
    let back = json!([change(0, None, Some(APP)), change(6, Some(APP), None), change(9, None, Some(APP)), change(8, None, None)]);
    assert_eq!(redelegated_at(&with_history(APP, back, this.clone())), Some(at(9)));
    // Handed to someone else and back.
    let via = json!([change(9, Some("other"), Some(APP)), change(6, Some(APP), Some("other")), change(0, None, Some(APP))]);
    assert_eq!(redelegated_at(&with_history(APP, via, this.clone())), Some(at(9)));
    // The undelegation came after the newest delegation: nothing to ask.
    let taken = json!([change(0, None, Some(APP)), change(6, Some(APP), None), change(7, None, Some("other"))]);
    assert_eq!(redelegated_at(&with_history("other", taken, this.clone())), None);
    // No history read: none.
    assert_eq!(redelegated_at(&with_history(APP, json!([]), this)), None);
}

#[test]
fn an_issue_handed_back_to_the_agent_it_came_from_asks_no_session_again() {
    let this = json!([{ "createdAt": at(0), "appUser": { "id": APP }, "comment": { "user": null } }]);
    // Handed from another agent to the app and back: the delegate is not the session's app.
    let away = json!([change(1, Some("other"), Some(APP)), change(6, Some(APP), Some("other"))]);
    let issue = parse_session(&with_history("other", away, this.clone())).unwrap().issue.unwrap();
    assert_eq!(issue.delegated.map(|d| (d.at, d.again)), Some((at(6), false)));
    // The same hand-over back to the app does ask again.
    let back = json!([change(1, Some("other"), Some(APP)), change(6, Some(APP), Some("other")), change(9, Some("other"), Some(APP))]);
    let issue = parse_session(&with_history(APP, back, this)).unwrap().issue.unwrap();
    assert_eq!(issue.delegated.map(|d| (d.at, d.again)), Some((at(9), true)));
}

#[test]
fn a_redelegation_that_linear_gave_a_new_session_is_that_sessions() {
    let back = json!([change(0, None, Some(APP)), change(6, Some(APP), None), change(9, None, Some(APP))]);
    let newer = json!([{ "createdAt": at(9), "appUser": { "id": APP } }, { "createdAt": at(0), "appUser": { "id": APP } }]);
    assert_eq!(redelegated_at(&with_history(APP, back.clone(), newer)), None);
    // Another app's newer session is none of Tolkster's business.
    let theirs = json!([{ "createdAt": at(10), "appUser": { "id": "other-app" } }, { "createdAt": at(0), "appUser": { "id": APP } }]);
    assert_eq!(redelegated_at(&with_history(APP, back, theirs)), Some(at(9)));
}

#[test]
fn a_redelegation_names_who_made_it_and_a_later_mention_does_not_take_it() {
    let back = json!([change(0, None, Some(APP)), change(6, Some(APP), None), { "createdAt": at(9), "actor": { "id": OWNER }, "fromDelegate": null, "toDelegate": { "id": APP } }]);
    let this = json!([{ "createdAt": at(0), "appUser": { "id": APP }, "comment": { "user": null } }]);
    let issue = parse_session(&with_history(APP, back.clone(), this)).unwrap().issue.unwrap();
    assert_eq!(issue.delegated, Some(Delegated { at: at(9), by: Some(OWNER.to_string()), again: true }));
    // The newest delegation, re-delegation or not, names who made it; aged out, nothing.
    let once = json!([{ "createdAt": at(2), "actor": { "id": "stranger" }, "fromDelegate": null, "toDelegate": { "id": APP } }]);
    let issue = parse_session(&with_history(APP, once, json!([]))).unwrap().issue.unwrap();
    assert_eq!(issue.delegated, Some(Delegated { at: at(2), by: Some("stranger".to_string()), again: false }));
    assert_eq!(parse_session(&with_history(APP, json!([]), json!([]))).unwrap().issue.unwrap().delegated, None);
    // A mention since, in a session of its own, asks its own question: the delegation's
    // session keeps the re-delegation.
    let mentioned = json!([{ "createdAt": at(10), "appUser": { "id": APP }, "comment": { "user": { "id": OWNER } } }, { "createdAt": at(0), "appUser": { "id": APP }, "comment": { "user": null } }]);
    assert_eq!(redelegated_at(&with_history(APP, back, mentioned)), Some(at(9)));
}

// An issue delegated to Tolkster that no entry read changed the delegate of names its creator:
// not with a change read, a full page, or another delegate.
#[test]
fn a_delegation_no_entry_changed_names_the_issues_creator() {
    let created = |delegate: &str, history: Value| {
        let mut node = with_history(delegate, history, json!([]));
        node["issue"]["creator"] = json!({ "id": OWNER });
        parse_session(&node).unwrap().issue.unwrap().created_delegated_by
    };
    assert_eq!(created(APP, json!([{ "createdAt": at(1) }])), Some(OWNER.to_string()));
    assert_eq!(created(APP, json!([change(1, None, Some(APP))])), None);
    assert_eq!(created(APP, json!((0..10).map(|n| json!({ "createdAt": at(n) })).collect::<Vec<_>>())), None);
    assert_eq!(created(OWNER, json!([])), None);
}

#[test]
fn a_delegation_linear_reopened_a_mentions_session_for_asks_it() {
    let delegated = json!([{ "createdAt": at(5), "actor": { "id": OWNER }, "fromDelegate": null, "toDelegate": { "id": APP } }]);
    let mention = |minute: u32| json!({ "createdAt": at(minute), "appUser": { "id": APP }, "comment": { "user": { "id": OWNER } } });
    let mut node = with_history(APP, delegated.clone(), json!([mention(0)]));
    node["comment"] = json!({ "user": { "id": OWNER } });
    // The mention's session is the app's only one on the issue: the delegation asks it.
    assert_eq!(redelegated_at(&node), Some(at(5)));
    // A newer mention's session takes it instead: one session asks, never two.
    let mut older = node.clone();
    older["issue"]["agentSessions"]["nodes"] = json!([mention(3), mention(0)]);
    assert_eq!(redelegated_at(&older), None);
    let mut newer = older.clone();
    newer["createdAt"] = json!(at(3));
    assert_eq!(redelegated_at(&newer), Some(at(5)));
    // A mention since the delegation can't be the session Linear reopened: the older keeps it.
    let mut since = node.clone();
    since["issue"]["agentSessions"]["nodes"] = json!([mention(8), mention(0)]);
    assert_eq!(redelegated_at(&since), Some(at(5)));
    // A session not among those read can't be told the newest: it asks nothing.
    let mut unread = node.clone();
    unread["issue"]["agentSessions"]["nodes"] = json!([{ "createdAt": at(4), "appUser": { "id": "other-app" }, "comment": { "user": null } }]);
    assert_eq!(redelegated_at(&unread), None);
    // A delegation's session on the issue holds its delegations: the mention's asks nothing.
    let mut held = node.clone();
    held["issue"]["agentSessions"]["nodes"] = json!([mention(0), { "createdAt": at(0), "appUser": { "id": APP }, "comment": { "user": null } }]);
    assert_eq!(redelegated_at(&held), None);
    // A delegation to another app asks nothing of Tolkster's session.
    let mut elsewhere = node.clone();
    elsewhere["issue"]["delegate"] = json!({ "id": "other-app" });
    elsewhere["issue"]["history"]["nodes"] = json!([{ "createdAt": at(5), "actor": { "id": OWNER }, "fromDelegate": null, "toDelegate": { "id": "other-app" } }]);
    assert_eq!(redelegated_at(&elsewhere), None);
    // Another app's session is none of Tolkster's business.
    let mut theirs = node.clone();
    theirs["issue"]["agentSessions"]["nodes"] = json!([{ "createdAt": at(4), "appUser": { "id": "other-app" }, "comment": { "user": null } }, mention(0)]);
    assert_eq!(redelegated_at(&theirs), Some(at(5)));
    // Sessions unread: a first delegation still asks, but a re-delegation, which an unread
    // delegation's session would take, asks nothing.
    let mut more = node.clone();
    more["issue"]["agentSessions"]["pageInfo"] = json!({ "hasNextPage": true });
    assert_eq!(redelegated_at(&more), Some(at(5)));
    let back = json!([change(1, None, Some(APP)), change(2, Some(APP), None), { "createdAt": at(5), "actor": { "id": OWNER }, "fromDelegate": null, "toDelegate": { "id": APP } }]);
    more["issue"]["history"]["nodes"] = back.clone();
    assert_eq!(redelegated_at(&more), None);
    let mut read = node;
    read["issue"]["history"]["nodes"] = back;
    assert_eq!(redelegated_at(&read), Some(at(5)));
}

#[test]
fn a_reply_under_an_authorless_root_is_a_mention_and_does_not_take_a_redelegation() {
    let back = json!([change(0, None, Some(APP)), change(6, Some(APP), None), change(9, None, Some(APP))]);
    // A reply that mentioned Tolkster in a synced thread: the root has no author, but the
    // session has a source comment.
    let replied = json!([{ "createdAt": at(10), "appUser": { "id": APP }, "comment": { "user": null }, "sourceComment": { "id": "c1" } }, { "createdAt": at(0), "appUser": { "id": APP }, "comment": { "user": null } }]);
    assert_eq!(redelegated_at(&with_history(APP, back, replied)), Some(at(9)));
}

#[test]
fn a_mention_is_the_source_comment_when_the_session_has_one() {
    let session = |comment: Value, source: Value| json!({ "id": "s1", "status": "pending", "createdAt": at(0), "appUser": { "id": APP }, "creator": { "id": OWNER }, "comment": comment, "sourceComment": source, "issue": null, "activities": { "nodes": [], "pageInfo": { "hasNextPage": false } } });
    let root = json!({ "body": "Synced", "user": null });
    // The owner's reply under an authorless root: a mention, theirs, with the reply's text.
    let s = parse_session(&session(root.clone(), json!({ "body": "@tolkster why?", "user": { "id": OWNER } }))).unwrap();
    let mention = s.mention.as_ref().unwrap();
    assert_eq!((mention.author_id.as_deref(), mention.body.as_str()), (Some(OWNER), "@tolkster why?"));
    assert!(s.is_owners(OWNER));
    // A reply under someone else's root is the reply's author's, not the root's.
    let theirs = json!({ "body": "Question", "user": { "id": "stranger" } });
    let s = parse_session(&session(theirs, json!({ "body": "@tolkster why?", "user": { "id": OWNER } }))).unwrap();
    assert_eq!(s.mention.unwrap().author_id.as_deref(), Some(OWNER));
    // A synced reply has no author either: still a mention, and nobody's, so not the owner's.
    let s = parse_session(&session(root.clone(), json!({ "body": "@tolkster why?", "user": null }))).unwrap();
    assert!(s.mention.as_ref().is_some_and(|m| m.author_id.is_none()) && !s.is_owners(OWNER));
    // No source comment and an authorless root: a delegation.
    assert!(parse_session(&session(root, Value::Null)).unwrap().mention.is_none());
}

// A session Linear opened with TW-1 at minute 0, as for TW-449: no source comment, a thread
// Linear wrote, the issue's creator as its own, and no delegation at its start.
fn opened_with_the_issue(history: Value, delegate: Option<&str>) -> Value {
    let mut node = with_history(APP, history, json!([{ "createdAt": at(0), "appUser": { "id": APP }, "creator": { "id": OWNER }, "comment": { "user": null } }]));
    node["comment"] = json!({ "body": "This thread is for an agent session with tolkster.", "user": null });
    node["issue"]["creator"] = json!({ "id": OWNER });
    node["issue"]["description"] = json!("@tolkster flesh this out");
    node["issue"]["delegate"] = delegate.map_or(Value::Null, |id| json!({ "id": id }));
    node
}

#[test]
fn a_mention_in_a_new_issues_description_is_a_mention_whose_request_is_the_issue() {
    let s = parse_session(&opened_with_the_issue(json!([]), None)).unwrap();
    let mention = s.mention.as_ref().unwrap();
    assert!(mention.in_description);
    // Its text is never read: anyone may have edited the description since.
    assert_eq!((mention.author_id.as_deref(), mention.body.as_str()), (Some(OWNER), ""));
    assert!(s.is_owners(OWNER));
    // Delegated since: Linear reopened that session for it, which then asks.
    let since = opened_with_the_issue(json!([change(5, None, Some(APP))]), Some(APP));
    assert!(parse_session(&since).unwrap().mention.is_some_and(|m| m.in_description));
    assert_eq!(redelegated_at(&since), Some(at(5)));
    // Someone else's issue: their mention, so not the owner's.
    let mut theirs = opened_with_the_issue(json!([]), None);
    theirs["issue"]["creator"] = json!({ "id": "stranger" });
    theirs["creator"] = json!({ "id": "stranger" });
    let s = parse_session(&theirs).unwrap();
    assert!(s.mention.as_ref().is_some_and(|m| m.author_id.as_deref() == Some("stranger")) && !s.is_owners(OWNER));
}

#[test]
fn a_session_that_began_with_a_delegation_or_after_the_issue_is_no_description_mention() {
    let delegation = |history: Value, delegate: Option<&str>| parse_session(&opened_with_the_issue(history, delegate)).unwrap().mention.is_none();
    // Created delegated: no change read, or one made with the issue.
    assert!(delegation(json!([]), Some(APP)));
    assert!(delegation(json!([change(0, None, Some(APP))]), Some(APP)));
    // Created delegated, and taken back since: the oldest change says who had it.
    assert!(delegation(json!([change(9, Some(APP), Some("other-app")), change(4, Some("other-app"), Some(APP)), change(2, Some(APP), Some("other-app"))]), Some("other-app")));
    // Taken back since, with that change gone from a full page of history: unknown, so no mention.
    let edits: Vec<Value> = (1..=10).map(|minute| json!({ "createdAt": at(minute), "actor": { "id": OWNER } })).collect();
    assert!(delegation(Value::Array(edits.clone()), None));
    assert!(parse_session(&opened_with_the_issue(Value::Array(edits[1..].to_vec()), None)).unwrap().mention.is_some());
    // Begun after the issue was created, or by someone other than its creator.
    let mut later = opened_with_the_issue(json!([]), None);
    later["createdAt"] = json!(at(3));
    assert!(parse_session(&later).unwrap().mention.is_none());
    let mut other = opened_with_the_issue(json!([]), None);
    other["creator"] = json!({ "id": "stranger" });
    assert!(parse_session(&other).unwrap().mention.is_none());
}

#[test]
fn a_session_opened_on_tolksters_own_comment_asks_the_reply_before_it() {
    let reply = |minute: u32, user: Value, bot: Value, body: &str| json!({ "createdAt": at(minute), "user": user, "botActor": bot, "body": body });
    let session = |creator: Value, source: Value, replies: Value| {
        json!({ "id": "s1", "status": "pending", "createdAt": at(5), "appUser": { "id": APP }, "creator": creator, "sourceComment": source, "issue": null,
                "comment": { "body": "Note", "user": { "id": APP }, "children": { "nodes": replies } }, "activities": { "nodes": [], "pageInfo": { "hasNextPage": false } } })
    };
    let owner = json!({ "id": OWNER });
    // The owner's replies before the session began are the request; one after it is a
    // prompt, and the app's and an integration's are nobody's request.
    let replies = json!([
        reply(1, owner.clone(), Value::Null, "Old"),
        reply(3, owner.clone(), Value::Null, "Are you listening?"),
        reply(4, json!({ "id": APP }), Value::Null, "Mine"),
        reply(4, owner.clone(), json!({ "id": "bot" }), "Synced"),
        reply(6, owner.clone(), Value::Null, "Later"),
    ]);
    let s = parse_session(&session(Value::Null, Value::Null, replies)).unwrap();
    assert!(s.on_own_note && s.is_owners(OWNER));
    // Every one of the owner's replies before it began, oldest first: none is lost to a later one.
    assert_eq!(s.messages_after(OWNER, None).iter().map(|m| m.body.as_str()).collect::<Vec<_>>(), ["Old", "Are you listening?"]);
    // Someone else's reply after the owner's leaves the owner's the request.
    let between = json!([reply(3, owner.clone(), Value::Null, "Mine"), reply(4, json!({ "id": "stranger" }), Value::Null, "Theirs")]);
    let s = parse_session(&session(Value::Null, Value::Null, between)).unwrap();
    assert_eq!(s.messages_after(OWNER, None).iter().map(|m| m.body.as_str()).collect::<Vec<_>>(), ["Mine"]);
    // Read without bodies: still Tolkster's own, with a request of no one's.
    let mut skipped = session(Value::Null, Value::Null, json!([]));
    skipped["comment"] = json!({ "user": { "id": APP } });
    let s = parse_session(&skipped).unwrap();
    assert!(s.on_own_note && s.is_owners(OWNER) && s.note_replies.is_empty());
    // A mention in the note's thread has a creator and a source comment: the mention's.
    let s = parse_session(&session(owner.clone(), json!({ "body": "@tolkster why?", "user": { "id": OWNER } }), json!([]))).unwrap();
    assert!(!s.on_own_note && s.mention.unwrap().body == "@tolkster why?");
    // No creator on someone else's root is an automation's: not Tolkster's own, nor the owner's.
    let mut theirs = session(Value::Null, Value::Null, json!([]));
    theirs["comment"]["user"] = json!({ "id": "stranger" });
    let s = parse_session(&theirs).unwrap();
    assert!(!s.on_own_note && !s.is_owners(OWNER));
}

#[test]
fn a_redelegation_belongs_to_the_newest_delegation_session_only() {
    // s1 at 0, then s2 at 3 (Linear gave an earlier re-delegation its own session), and
    // now a re-delegation at 9 that reopened s2: only s2 asks.
    let back = json!([change(0, None, Some(APP)), change(6, Some(APP), None), change(9, None, Some(APP))]);
    let sessions = json!([{ "createdAt": at(3), "appUser": { "id": APP }, "comment": { "user": null } }, { "createdAt": at(0), "appUser": { "id": APP }, "comment": { "user": null } }]);
    assert_eq!(redelegated_at(&with_history(APP, back.clone(), sessions.clone())), None);
    let mut s2 = with_history(APP, back, sessions);
    s2["createdAt"] = json!(at(3));
    assert_eq!(redelegated_at(&s2), Some(at(9)));
}

#[test]
fn sessions_read_through_their_issue_ask_for_it_once_and_parse_as_though_read_with_it() {
    let fields = Through { issue: issue_fields(), session: own_fields(20, Bodies::Skip) };
    let node = with_history(APP, json!([change(0, None, Some(APP))]), json!([]));
    let mut issue = node["issue"].clone();
    let mut own = node.clone();
    own.as_object_mut().unwrap().remove("issue");
    issue["sessions"] = json!({ "nodes": [own, { "id": "theirs", "createdAt": at(1), "appUser": { "id": "other-app" } }], "pageInfo": { "hasNextPage": false } });
    let asked = std::sync::Mutex::new(Vec::new());
    let fetch = |q: &str, _| {
        asked.lock().unwrap().push(q.to_string());
        Ok(json!({ "viewer": { "id": APP }, "issue": issue.clone() }))
    };
    let (found, issue_id, _) = issue_sessions::<SessionNode>(&fetch, "TW-1", &fields, None).unwrap();
    assert_eq!(issue_id, "i1");
    assert_eq!(found.len(), 1);
    // Parsed on the issue read above them, each is as though read with it.
    let issue_node: IssueNode = decode(node["issue"].clone(), "Linear's answer to IssueSessions").unwrap();
    let (through, with) = (parse_own(&found[0], Some(&issue_node)), parse_session(&node).unwrap());
    assert_eq!((through.id, through.created_at, through.app_user_id, through.mention.is_some()), (with.id, with.created_at, with.app_user_id, with.mention.is_some()));
    // The issue's fields once, above the sessions, none in each session's.
    let query = &asked.lock().unwrap()[0];
    assert_eq!(query.matches("inverseRelations").count(), 1);
    assert!(!fields.session.contains("issue {") && query.find("inverseRelations").unwrap() < query.find("sessions: agentSessions").unwrap());
    // A query of one session still reads its issue with it.
    assert_eq!(session_fields(20, Bodies::Skip), format!("{} issue {{ {} }}", fields.session, fields.issue));
}

#[test]
fn a_slim_read_asks_only_for_what_its_check_decides_on() {
    let owner = read_fields(Read::Owner);
    for left_out in ["inverseRelations", "labels", "project", "children", "agentSessions", "activities", "body"] {
        assert!(!owner.contains(left_out), "{left_out}");
    }
    for read in ["creator { id }", "sourceComment { user { id } }", "estimate", "history(first: 10)", "parent {", "delegate { id }", "team { id "] {
        assert!(owner.contains(read), "{read}");
    }
    let answer = read_fields(Read::Answer(20));
    for left_out in ["inverseRelations", "labels", "project", "body"] {
        assert!(!answer.contains(left_out), "{left_out}");
    }
    // What the request reads: activities, sub-issues and the issue's newest sessions.
    assert!(answer.starts_with(&owner[..owner.find(" issue {").unwrap()]));
    for read in ["activities(orderBy: createdAt, first: 20, filter: { type: { eq: \"prompt\" } })", "children(first: 50)", "agentSessions(first: 3"] {
        assert!(answer.contains(read), "{read}");
    }
}

#[test]
fn a_full_read_is_the_whole_session_and_asks_for_no_prompts_at_zero() {
    let full = read_fields(Read::Full { activities: 20, bodies: Bodies::Read });
    assert_eq!(full, session_fields(20, Bodies::Read));
    // `listen` reads no prompts.
    assert!(!read_fields(Read::Full { activities: 0, bodies: Bodies::Skip }).contains("activities"));
}

#[test]
fn a_slim_session_decides_the_owner_check_as_the_full_one_does() {
    let issue = |parent: Value| json!({ "id": "i1", "identifier": "TW-2", "url": "u", "branchName": "b", "createdAt": at(0), "state": { "type": "started" }, "team": { "id": "t1", "key": "TW", "issueEstimationType": "fibonacci", "issueEstimationExtended": false, "issueEstimationAllowZero": false }, "delegate": { "id": APP }, "parent": parent, "estimate": 3.0, "history": { "nodes": [] } });
    let node = |creator: &str| json!({ "id": "s1", "status": "active", "createdAt": at(1), "appUser": { "id": APP }, "creator": { "id": creator }, "comment": { "user": { "id": creator } }, "sourceComment": null, "issue": issue(Value::Null) });
    let owners = parse_session(&node(OWNER)).unwrap();
    let others = parse_session(&node("someone")).unwrap();
    assert!(owners.is_owners(OWNER) && !others.is_owners(OWNER));
    assert!(owners.activities.is_empty() && owners.issue.as_ref().unwrap().estimate == Some(3.0));
}

#[test]
fn the_finish_time_is_the_one_the_state_says() {
    let (canceled, completed) = (Some("c".to_string()), Some("d".to_string()));
    let at = |of: &str, state| finished_at(StateType::from(of), canceled.as_ref(), completed.as_ref(), state);
    assert_eq!(at("canceled", StateType::Canceled), Some("c".into()));
    assert_eq!(at("canceled", StateType::Completed), None);
    assert_eq!(at("completed", StateType::Completed), Some("d".into()));
    // Reopened: the old times stay on the issue, and don't count.
    assert_eq!(at("started", StateType::Canceled), None);
    assert_eq!(at("started", StateType::Completed), None);
    assert_eq!(at("started", StateType::Started), None);
}

#[test]
fn a_session_parses_from_linears_response() {
    let node = json!({
        "id": "s1", "status": "active", "createdAt": at(0), "appUser": { "id": APP }, "creator": { "id": OWNER },
        "comment": { "body": "@tolkster look", "user": { "id": OWNER } },
        "issue": { "id": "i1", "identifier": "TW-1", "url": "https://linear.app/w/issue/TW-1", "branchName": "tw-1", "createdAt": at(0), "state": { "type": "started" }, "team": { "id": "t1", "key": "TW", "issueEstimationType": "fibonacci", "issueEstimationExtended": false, "issueEstimationAllowZero": false }, "delegate": null, "project": { "id": "p1", "name": "Tolkster" }, "history": { "nodes": [] } },
        "activities": { "nodes": [
            { "id": "a1", "createdAt": at(1), "user": { "id": APP }, "signal": null, "content": { "__typename": "AgentActivityThoughtContent", "body": "Got it." } },
            { "id": "a3", "createdAt": at(3), "user": { "id": OWNER }, "signal": "stop", "content": { "__typename": "AgentActivityPromptContent", "body": "Stop" } },
            { "id": "a2", "createdAt": at(2), "user": { "id": APP }, "signal": null, "content": { "__typename": "AgentActivityActionContent" } },
            { "id": "a4", "createdAt": at(4), "user": { "id": APP }, "signal": null, "content": { "__typename": "AgentActivityElicitationContent" } },
        ], "pageInfo": { "hasNextPage": false } },
    });
    let s = parse_session(&node).unwrap();
    assert_eq!(s.mention.as_ref().unwrap().body, "@tolkster look");
    assert_eq!(s.issue.as_ref().unwrap().delegate_id, None);
    assert_eq!((s.issue.as_ref().unwrap().estimate, s.issue.as_ref().unwrap().labels.len()), (None, 0));
    assert_eq!(s.activities.iter().map(|a| a.kind).collect::<Vec<_>>(), [Kind::Answer, Kind::Prompt, Kind::Progress, Kind::Progress]);
    assert_eq!((s.activities[1].signal.as_deref(), s.activities[1].id.as_deref()), (Some("stop"), Some("a3")));
    assert_eq!(s.newest_request(OWNER), Some((Some("a3".into()), at(3))));
    assert_eq!(s.record, Record::default());

    // A delegation's thread comment has no author, so it's not a mention.
    let delegated = json!({ "id": "s2", "status": "pending", "createdAt": at(0), "appUser": { "id": APP }, "creator": { "id": OWNER }, "comment": { "body": "This thread is for an agent session with tolkster.", "user": null }, "issue": null, "activities": { "nodes": [], "pageInfo": { "hasNextPage": true } } });
    let s = parse_session(&delegated).unwrap();
    assert!(s.mention.is_none() && s.issue.is_none() && s.more_activities);
}

#[test]
fn a_session_is_read_with_its_prompts_alone() {
    // Linear filters the activities to prompts, so no ephemeral thought takes a place in the
    // page: what Tolkster said is read from its record.
    let prompts = "activities(orderBy: createdAt, first: 20, filter: { type: { eq: \"prompt\" } })";
    for query in [session_fields(20, Bodies::Read), session_fields(20, Bodies::Skip), own_fields(20, Bodies::Skip), read_fields(Read::Answer(20))] {
        let read = &query[query.find(prompts).unwrap_or_else(|| panic!("{query}"))..];
        // No cursor: nothing reads a further page of them.
        let read = &read[..read.find("pageInfo { hasNextPage }").unwrap()];
        assert!(!read.contains("ephemeral") && !read.contains("ThoughtContent"), "{read}");
    }
    assert!(!read_fields(Read::Owner).contains("activities"));
}

// A reply's session as Linear returns it: the owner's prompt alone, as the read is of prompts.
fn claimed_reply() -> Value {
    let nodes = vec![json!({ "id": "p2", "createdAt": at(2), "user": { "id": OWNER }, "signal": null, "content": { "__typename": "AgentActivityPromptContent", "body": "More" } })];
    json!({
        "id": "s1", "status": "active", "createdAt": at(0), "appUser": { "id": APP }, "creator": { "id": OWNER }, "comment": null,
        "issue": { "id": "i1", "identifier": "TW-1", "url": "https://linear.app/w/issue/TW-1", "branchName": "tw-1", "createdAt": at(0), "state": { "type": "started" }, "team": { "id": "t1", "key": "TW", "issueEstimationType": "fibonacci", "issueEstimationExtended": false, "issueEstimationAllowZero": false }, "delegate": { "id": APP }, "project": null, "history": { "nodes": [] } },
        "activities": { "nodes": nodes, "pageInfo": { "hasNextPage": false } },
    })
}

#[test]
fn only_a_read_with_bodies_asks_for_text() {
    assert!(session_fields(20, Bodies::Read).contains("PromptContent { body }"));
    assert!(session_fields(20, Bodies::Read).contains("comment { body user { id } children(first: 50, orderBy: createdAt) { nodes { createdAt body user { id } botActor { id } } } }"));
    assert!(!session_fields(20, Bodies::Read).contains("description"));
    let query = session_fields(20, Bodies::Skip);
    assert!(!query.contains("body") && !query.contains("parameter"));
    // Read without text, a session decides the same: kinds, authors and signals.
    let mut node = claimed_reply();
    node["comment"] = json!({ "user": { "id": OWNER } });
    for a in node["activities"]["nodes"].as_array_mut().unwrap() {
        a["content"] = json!({ "__typename": a["content"]["__typename"] });
    }
    let s = parse_session(&node).unwrap();
    assert_eq!(s.mention.as_ref().and_then(|m| m.author_id.as_deref()), Some(OWNER));
    assert_eq!(s.activities.iter().map(|a| a.kind).collect::<Vec<_>>(), [Kind::Prompt]);
    assert!(s.activities.iter().all(|a| a.body.is_none()));
}

#[test]
fn a_parent_and_sub_issues_parse_with_the_app_the_session_is_with() {
    let mut node = with_history(APP, json!([]), json!([]));
    node["issue"]["parent"] = json!({ "id": "i0", "identifier": "TW-0", "state": { "type": "started" }, "delegate": { "id": APP }, "priority": 2, "history": { "nodes": [] } });
    node["issue"]["priority"] = json!(0);
    node["issue"]["children"] = json!({ "nodes": [
        { "id": "c2", "identifier": "TW-2", "url": "u2", "updatedAt": at(2), "state": { "type": "backlog" } },
        { "identifier": "TW-3", "url": "u3", "state": { "type": "completed" }, "completedAt": at(4) },
        { "identifier": "TW-4", "url": "u4", "state": { "type": "canceled" }, "canceledAt": at(6) },
    ], "pageInfo": { "hasNextPage": true } });
    let s = parse_session(&node).unwrap();
    let issue = s.issue.as_ref().unwrap();
    let parent = issue.parent.as_ref().unwrap();
    assert_eq!((parent.id.as_str(), parent.identifier.as_str(), parent.delegate_id.as_deref()), ("i0", "TW-0", Some(APP)));
    assert_eq!(issue.children.iter().map(|c| c.identifier.as_str()).collect::<Vec<_>>(), ["TW-2", "TW-3", "TW-4"]);
    // Each sub-issue's ID and own clock, which version its state in the machine's read.
    assert_eq!((issue.children[0].id.as_deref(), issue.children[0].updated_at.as_deref()), (Some("c2"), Some(at(2).as_str())));
    assert_eq!(issue.children[1].id, None);
    assert_eq!((issue.priority, parent.priority), (0, 2));
    assert!(issue.more_children);
    assert_eq!(issue.children_finished_at, Some(at(6)));
    assert_eq!((s.app_user_id.as_deref(), s.delegation.as_ref()), (Some(APP), None));
    // A top-level issue has neither.
    let top = parse_session(&with_history(APP, json!([]), json!([]))).unwrap().issue.unwrap();
    assert!(top.parent.is_none() && top.children.is_empty() && !top.more_children);
    assert!(session_fields(20, Bodies::Skip).contains("parent { id identifier updatedAt canceledAt state { type } delegate { id } priority history(first: 10)"));
    // The parent's own change times, read as the issue's are: its last release from the app, and
    // its cancellation only while its state says so.
    node["issue"]["parent"] = json!({
        "id": "i0", "identifier": "TW-0", "state": { "type": "canceled" }, "delegate": null, "priority": 0, "canceledAt": at(7),
        "history": { "nodes": [change(5, Some(APP), None), change(3, None, Some(APP))] },
    });
    let parent = parse_session(&node).unwrap().issue.unwrap().parent.unwrap();
    assert_eq!((parent.undelegated_at, parent.canceled_at), (Some(at(5)), Some(at(7))));
    node["issue"]["parent"]["state"] = json!({ "type": "started" });
    assert_eq!(parse_session(&node).unwrap().issue.unwrap().parent.unwrap().canceled_at, None);
    // Its clock, and who made its delegation now: the newest change to its delegate.
    node["issue"]["parent"]["updatedAt"] = json!(at(8));
    node["issue"]["parent"]["delegate"] = json!({ "id": APP });
    node["issue"]["parent"]["history"]["nodes"] = json!([
        { "createdAt": at(6), "actor": { "id": OWNER }, "fromDelegate": null, "toDelegate": { "id": APP } },
        { "createdAt": at(2), "actor": { "id": APP }, "fromDelegate": null, "toDelegate": { "id": APP } },
    ]);
    let parent = parse_session(&node).unwrap().issue.unwrap().parent.unwrap();
    assert_eq!((parent.updated_at, parent.delegated_by.flatten().as_deref()), (Some(at(8)), Some(OWNER)));
    assert!(session_fields(20, Bodies::Skip).contains("estimate priority labels"));
}

#[test]
fn blockers_parse_from_inverse_relations() {
    let mut node = with_history(APP, json!([]), json!([]));
    node["issue"]["inverseRelations"] = json!({ "nodes": [
        { "type": "blocks", "issue": { "identifier": "TW-84", "url": "u84", "state": { "type": "started" } } },
        { "type": "related", "issue": { "identifier": "TW-85", "url": "u85", "state": { "type": "started" } } },
        { "type": "blocks", "issue": null },
    ] });
    let blockers = parse_session(&node).unwrap().issue.unwrap().blockers;
    assert_eq!(blockers.iter().map(|b| b.identifier.as_str()).collect::<Vec<_>>(), ["TW-84"]);
}

fn relation(n: u32, state: &str) -> Value {
    json!({ "type": "blocks", "issue": { "identifier": format!("TW-{n}"), "url": format!("u{n}"), "state": { "type": state } } })
}

fn related_node(first: Vec<Value>, more: bool) -> Value {
    let mut node = with_history(APP, json!([]), json!([]));
    node["issue"]["inverseRelations"] = json!({ "nodes": first, "pageInfo": { "hasNextPage": more, "endCursor": "c1" } });
    node
}

#[test]
fn a_blocker_past_the_first_page_of_relations_is_read_before_the_issue_is_parsed() {
    let query = more_relations_query();
    assert!(query.contains("issue(id: $id) { inverseRelations(first: 50, after: $after)") && query.contains("pageInfo { hasNextPage endCursor }"));
    assert!(issue_fields().contains("inverseRelations(first: 50) { nodes { type issue { id identifier url updatedAt state { type } } } pageInfo { hasNextPage endCursor } }"));

    // 50 finished blockers on the first page, an unfinished one on the second.
    let node = related_node((1..=50).map(|n| relation(n, "completed")).collect(), true);
    let mut asked = Vec::new();
    let mut fetch = |id: &str, after: &str| {
        asked.push(format!("{id}@{after}"));
        Ok(json!({ "nodes": [relation(51, "started")], "pageInfo": { "hasNextPage": false, "endCursor": "c2" } }))
    };
    let blockers = completed(&node, &mut fetch).unwrap().blockers;
    assert_eq!(asked, ["i1@c1"]);
    assert_eq!(blockers.len(), 51);
    assert_eq!(blockers.iter().filter(|b| !b.state_type.is_finished()).map(|b| b.identifier.as_str()).collect::<Vec<_>>(), ["TW-51"]);

    // An issue with one page costs no read.
    completed(&related_node(vec![relation(1, "started")], false), &mut |_, _| panic!("nothing more to read")).unwrap();
    // Nor does a session read without its issue's relations (a slim read).
    completed(&with_history(APP, json!([]), json!([])), &mut |_, _| panic!("no relations asked for")).unwrap();
}

#[test]
fn relations_left_unread_stay_marked_and_fail_no_read() {
    let endless = related_node(vec![relation(1, "completed")], true);
    let mut pages = 0;
    let issue = completed(&endless, &mut |_, _| {
        pages += 1;
        Ok(json!({ "nodes": [relation(2, "completed")], "pageInfo": { "hasNextPage": true, "endCursor": "c" } }))
    })
    .unwrap();
    // The first page was read with the issue, so RELATION_PAGES - 1 further ones.
    assert_eq!(pages, RELATION_PAGES - 1);
    assert!(issue.more_blockers);

    // No cursor, a page Linear won't give, one with no connection or none of its `pageInfo`:
    // left marked, not passed as clear.
    let mut cursorless = related_node(vec![relation(1, "completed")], true);
    cursorless["issue"]["inverseRelations"]["pageInfo"]["endCursor"] = Value::Null;
    let marked = related_node(vec![relation(1, "completed")], true);
    let left = [
        completed(&cursorless, &mut |_, _| panic!("no cursor to read by")),
        completed(&marked, &mut |_, _| Err(Error::Failed("Could not find cursor".into()))),
        completed(&marked, &mut |_, _| Ok(Value::Null)),
        completed(&marked, &mut |_, _| Ok(json!({ "nodes": [] }))),
    ];
    for issue in left {
        assert!(issue.unwrap().more_blockers);
    }

    // Any other failed page read is the read's failure.
    assert!(completed(&marked, &mut |_, _| Err(Error::Failed("down".into()))).is_err());

    // A read to the end is no longer marked.
    let whole = completed(&marked, &mut |_, _| Ok(json!({ "nodes": [], "pageInfo": { "hasNextPage": false, "endCursor": "c2" } }))).unwrap();
    assert!(!whole.more_blockers);
}

// An issue's read for `readIssue`, as `issue_facts` asks for it: its first page of relations,
// `more` past it, and a page of sessions, mine and another app's, with `sessions_after` the
// cursor of a further page.
fn read_issue_data(relations: Vec<Value>, more: bool, sessions_after: Option<&str>) -> Value {
    let mut issue = related_node(relations, more)["issue"].take();
    issue["updatedAt"] = json!(at(5));
    let mine = json!({ "id": "s1", "status": "complete", "createdAt": at(1), "appUser": { "id": APP }, "creator": { "id": OWNER }, "comment": { "user": null }, "sourceComment": null });
    let theirs = json!({ "id": "s2", "createdAt": at(2), "appUser": { "id": "other-app" } });
    issue["sessions"] = json!({ "nodes": [mine, theirs], "pageInfo": { "hasNextPage": sessions_after.is_some(), "endCursor": sessions_after } });
    json!({ "viewer": { "id": APP }, "issue": issue })
}

#[test]
fn an_issue_whose_finished_relations_run_past_a_page_reads_as_unblocked_in_one_query_and_one_page() {
    // A page of relations of another type, then a finished blocker on the next.
    let related = (1..=50).map(|n| {
        let mut r = relation(n, "started");
        r["type"] = json!("related");
        r
    });
    let first = read_issue_data(related.collect(), true, None);
    let asked = std::sync::Mutex::new(Vec::new());
    let fetch = |q: &str, v: Value| {
        asked.lock().unwrap().push((q.to_string(), v));
        Ok(match q.starts_with("query MoreRelations") {
            true => json!({ "issue": { "inverseRelations": { "nodes": [{ "type": "blocks", "issue": { "id": "b51", "identifier": "TW-51", "url": "u51", "updatedAt": at(4), "state": { "type": "canceled" } } }], "pageInfo": { "hasNextPage": false, "endCursor": "c2" } } } }),
            false => first.clone(),
        })
    };
    let (issue, updated_at, sessions, viewer_id) = issue_facts(&fetch, "TW-1").unwrap();
    assert!(!issue.more_blockers && issue.blockers.len() == 1);
    assert_eq!((updated_at.as_str(), viewer_id.as_str()), (at(5).as_str(), APP));
    let ids = crate::command::read::Ids { viewer: APP, owner: OWNER };
    assert_eq!(crate::command::read::issue_read(&issue, &updated_at, &ids, None, &sessions, None).unwrap()["moreBlockers"], json!(false));
    // Mine alone, without its issue, activities or record.
    assert_eq!(sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s1"]);
    assert!(sessions[0].issue.is_none() && sessions[0].activities.is_empty());
    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[1].1, json!({ "id": "i1", "after": "c1" }));
    // The sessions' fields are slim: no activities and no text.
    assert!(!asked[0].0.contains("activities") && !asked[0].0.contains("body") && asked[0].0.contains(SLIM_OWN));
}

#[test]
fn an_issue_with_one_page_of_sessions_is_one_query_and_a_further_page_asks_only_for_sessions() {
    let asked = std::sync::Mutex::new(Vec::new());
    let one = read_issue_data(vec![relation(1, "started")], false, None);
    let fetch = |q: &str, _| {
        asked.lock().unwrap().push(q.to_string());
        Ok(one.clone())
    };
    let (issue, _, sessions, _) = issue_facts(&fetch, "TW-1").unwrap();
    assert!(!issue.more_blockers && sessions.len() == 1);
    assert_eq!(asked.lock().unwrap().len(), 1);

    let asked = std::sync::Mutex::new(Vec::new());
    let paged = read_issue_data(vec![relation(1, "started")], false, Some("p1"));
    let fetch = |q: &str, v: Value| {
        asked.lock().unwrap().push((q.to_string(), v.clone()));
        Ok(match v["after"].is_null() {
            true => paged.clone(),
            false => json!({ "viewer": { "id": APP }, "issue": { "id": "i1", "sessions": { "nodes": [{ "id": "s3", "status": "pending", "createdAt": at(3), "appUser": { "id": APP }, "creator": { "id": OWNER }, "comment": { "user": null } }], "pageInfo": { "hasNextPage": false } } } }),
        })
    };
    let (_, _, sessions, _) = issue_facts(&fetch, "TW-1").unwrap();
    assert_eq!(sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s1", "s3"]);
    let asked = asked.lock().unwrap();
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[1].1, json!({ "id": "i1", "after": "p1" }));
    assert!(!asked[1].0.contains("inverseRelations") && !asked[1].0.contains("history") && !asked[1].0.contains("activities"));
}
