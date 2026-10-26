use serde_json::{Value, json};

use super::*;

const OWNER: &str = "u-owner";
const TOLKSTER: &str = "u-tolkster";

// `data` as `sessions::read_comment` answers it, decoded first.
fn decoded(data: &Value) -> CommentRead {
    crate::json::decode(data.clone(), "Linear's answer to Comment").unwrap()
}

fn comment_kind(data: &Value, owner_id: &str) -> CommentKind {
    super::comment_kind(&decoded(data), owner_id)
}

fn comment_facts(data: &Value, owner_id: &str) -> Option<CommentFacts> {
    super::comment_facts(&decoded(data), owner_id)
}

// A comment as sessions::read_comment reads it: `id`, by `user`, on TW-192, with Tolkster's
// session `s-1` on the issue, begun by the comment `c-root`, and another agent's, begun by
// `c-other`.
fn comment(id: &str, user: &str, parent: Option<&str>) -> Value {
    json!({
        "viewer": { "id": TOLKSTER },
        "comment": {
            "id": id, "createdAt": "2026-09-29T03:00:00.000Z", "user": { "id": user }, "parent": parent.map(|p| json!({ "id": p })),
            "issue": { "id": "i-192", "identifier": "TW-192", "creator": null, "state": { "type": "started" }, "agentSessions": { "nodes": [
                { "appUser": { "id": TOLKSTER }, "comment": { "id": "c-root" }, "sourceComment": { "id": "c-source" } },
                { "appUser": { "id": "u-another-agent" }, "comment": { "id": "c-other" }, "sourceComment": null },
            ] } },
        },
    })
}

// The owner's reply `c-new` to Tolkster's comment `c-note`, with the sessions of Tolkster's
// begun on that thread: (id, began, creator).
fn note_reply(rooted: &[(&str, &str, Option<&str>)]) -> Value {
    let mut data = comment("c-new", OWNER, Some("c-note"));
    data["comment"]["parent"]["user"] = json!({ "id": TOLKSTER });
    let nodes = data["comment"]["issue"]["agentSessions"]["nodes"].as_array_mut().unwrap();
    for (id, began, creator) in rooted {
        nodes.push(json!({ "id": id, "createdAt": began, "appUser": { "id": TOLKSTER }, "creator": creator.map(|c| json!({ "id": c })), "comment": { "id": "c-note" }, "sourceComment": null }));
    }
    data
}

#[test]
fn the_owners_reply_to_tolksters_comment_is_answered_in_its_thread() {
    let reply = CommentKind::NoteReply { root: "c-note".into() };
    assert_eq!(comment_kind(&note_reply(&[]), OWNER), reply);
    // A thread a session began on, before or after the reply, and a mention's, is the session's.
    assert_eq!(comment_kind(&note_reply(&[("s-9", "2026-09-29T02:59:00.000Z", None)]), OWNER), CommentKind::Reply);
    assert_eq!(comment_kind(&note_reply(&[("s-9", "2026-09-29T03:00:02.000Z", None)]), OWNER), CommentKind::Reply);
    assert_eq!(comment_kind(&note_reply(&[("s-9", "2026-09-29T03:00:02.000Z", Some(OWNER))]), OWNER), CommentKind::Reply);
    // On a finished issue too, as a mention would be.
    let mut done = note_reply(&[]);
    done["comment"]["issue"]["state"] = json!({ "type": "completed" });
    assert_eq!(comment_kind(&done, OWNER), reply);
    // Someone else's reply there, or the owner's to someone else's comment, is not.
    assert_eq!(comment_kind(&comment("c-new", "u-someone", Some("c-note")), OWNER), CommentKind::Gone);
    let mut theirs = note_reply(&[]);
    theirs["comment"]["parent"]["user"] = json!({ "id": "u-someone" });
    assert_eq!(comment_kind(&theirs, OWNER), CommentKind::Plain);
}

#[test]
fn the_owners_comment_on_an_issue_is_plain() {
    assert_eq!(comment_kind(&comment("c-new", OWNER, None), OWNER), CommentKind::Plain);
    // A reply in someone's thread, or in another agent's session's, is just a comment.
    assert_eq!(comment_kind(&comment("c-new", OWNER, Some("c-plain")), OWNER), CommentKind::Plain);
    assert_eq!(comment_kind(&comment("c-new", OWNER, Some("c-other")), OWNER), CommentKind::Plain);
    // On a finished issue too: the facts carry its state.
    for finished in ["completed", "canceled"] {
        let mut done = comment("c-new", OWNER, None);
        done["comment"]["issue"]["state"] = json!({ "type": finished });
        assert_eq!(comment_kind(&done, OWNER), CommentKind::Plain);
    }
}

#[test]
fn the_owners_top_level_comment_on_an_issue_tolkster_created_roots_its_own_thread() {
    let on_mine = |id: &str, user: &str, parent: Option<&str>| {
        let mut data = comment(id, user, parent);
        data["comment"]["issue"]["creator"] = json!({ "id": TOLKSTER });
        comment_kind(&data, OWNER)
    };
    assert_eq!(on_mine("c-new", OWNER, None), CommentKind::NoteRoot { root: "c-new".into() });
    // Not on an issue the owner created, nor a mention or a reply on Tolkster's.
    let mut theirs = comment("c-new", OWNER, None);
    theirs["comment"]["issue"]["creator"] = json!({ "id": OWNER });
    assert_eq!(comment_kind(&theirs, OWNER), CommentKind::Plain);
    assert_eq!(on_mine("c-root", OWNER, None), CommentKind::Mention);
    assert_eq!(on_mine("c-new", OWNER, Some("c-plain")), CommentKind::Plain);
    // The owner's later reply in the thread they rooted keys the same request.
    let mut reply = comment("c-new", OWNER, Some("c-mine"));
    (reply["comment"]["parent"]["user"], reply["comment"]["issue"]["creator"]) = (json!({ "id": OWNER }), json!({ "id": TOLKSTER }));
    assert_eq!(comment_kind(&reply, OWNER), CommentKind::NoteRoot { root: "c-mine".into() });
    // Not under an integration's comment posted as the owner, which roots no thread.
    reply["comment"]["parent"]["botActor"] = json!({ "id": "b-1" });
    assert_eq!(comment_kind(&reply, OWNER), CommentKind::Plain);
    assert_eq!(on_mine("c-new", "u-someone", None), CommentKind::Gone);
}

#[test]
fn a_mention_or_a_reply_in_tolksters_thread_already_reaches_the_worker() {
    // The comment that began Tolkster's session, a mention in a reply that reopened it,
    // and a reply in its thread.
    assert_eq!(comment_kind(&comment("c-root", OWNER, None), OWNER), CommentKind::Mention);
    assert_eq!(comment_kind(&comment("c-source", OWNER, Some("c-plain")), OWNER), CommentKind::Mention);
    assert_eq!(comment_kind(&comment("c-new", OWNER, Some("c-root")), OWNER), CommentKind::Reply);
}

#[test]
fn anyone_elses_comment_or_one_off_an_issue_is_gone() {
    assert_eq!(comment_kind(&comment("c-new", "u-someone", None), OWNER), CommentKind::Gone);
    assert_eq!(comment_kind(&comment("c-new", OWNER, None), ""), CommentKind::Gone);
    // An integration posting as the owner.
    let mut bot = comment("c-new", OWNER, None);
    bot["comment"]["botActor"] = json!({ "id": "b-1" });
    assert_eq!(comment_kind(&bot, OWNER), CommentKind::Gone);
    let mut off = comment("c-new", OWNER, None);
    off["comment"]["issue"] = Value::Null;
    assert_eq!(comment_kind(&off, OWNER), CommentKind::Gone);
    // An identifier that isn't plain never reaches a terminal line.
    let mut odd = comment("c-new", OWNER, None);
    odd["comment"]["issue"]["identifier"] = json!("TW-1; rm");
    assert_eq!(comment_kind(&odd, OWNER), CommentKind::Gone);
}

#[test]
fn a_kind_is_named_as_read_comment_has_it() {
    let names = [CommentKind::Plain, CommentKind::NoteReply { root: "c-1".into() }, CommentKind::NoteRoot { root: "c-1".into() }, CommentKind::Mention, CommentKind::Reply, CommentKind::Gone].map(|kind| kind.name());
    assert_eq!(names, ["plain", "noteReply", "noteRoot", "mention", "reply", "gone"]);
}

// The comment `c-new` by `user` as sessions::read_comment reads it with the owner's comments,
// on TW-192 (`i-192`), in `state`, whose newest comment by the owner is at `listed`.
fn read(user: &str, parent: Option<Value>, state: &str, listed: Option<&str>) -> Value {
    json!({
        "viewer": { "id": TOLKSTER },
        "comment": {
            "id": "c-new", "createdAt": "2026-09-29T03:00:00.000Z", "user": { "id": user }, "parent": parent,
            "issue": {
                "id": "i-192", "identifier": "TW-192", "state": { "type": state },
                "agentSessions": { "nodes": [{ "appUser": { "id": TOLKSTER }, "comment": { "id": "c-root" }, "sourceComment": { "id": "c-source" } }] },
                "comments": { "nodes": listed.map(|at| vec![json!({ "createdAt": at })]).unwrap_or_default() },
            },
        },
    })
}

// The adapter answers `readComment`'s facts, so command.rs reads no JSON: the newest owner's
// comment is never older than a plain one itself, where Linear's list lags.
#[test]
fn a_comments_facts_are_read_at_the_edge() {
    assert_eq!(comment_facts(&json!({ "viewer": { "id": TOLKSTER }, "comment": null }), OWNER), None);
    let plain = comment_facts(&read(OWNER, None, "started", Some("2026-09-29T02:00:00.000Z")), OWNER).unwrap();
    assert_eq!(plain, CommentFacts { issue_id: Some("i-192".into()), kind: CommentKind::Plain, newest_owner_at: Some("2026-09-29T03:00:00.000Z".into()), state: StateType::Started });
    let later = comment_facts(&read(OWNER, None, "started", Some("2026-09-29T04:00:00.000Z")), OWNER).unwrap();
    assert_eq!(later.newest_owner_at.as_deref(), Some("2026-09-29T04:00:00.000Z"));
    // Only a plain one counts itself: a reply in a session's thread has Linear's list's newest.
    let reply = comment_facts(&read(OWNER, Some(json!({ "id": "c-root" })), "started", None), OWNER).unwrap();
    assert_eq!((reply.kind, reply.newest_owner_at), (CommentKind::Reply, None));
    let note = comment_facts(&read(OWNER, Some(json!({ "id": "c-note", "user": { "id": TOLKSTER } })), "completed", None), OWNER).unwrap();
    assert_eq!((note.kind, note.state), (CommentKind::NoteReply { root: "c-note".into() }, StateType::Completed));
    let theirs = comment_facts(&read("u-someone", None, "started", None), OWNER).unwrap();
    assert_eq!((theirs.kind, theirs.issue_id.as_deref()), (CommentKind::Gone, Some("i-192")));
}
