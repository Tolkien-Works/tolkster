use super::*;
use crate::linear::Kind;
use crate::request;
use crate::request::tests::{OWNER, at, mentioned, prompt, redelegated, said, session, stop};

#[test]
fn messages_render_with_how_to_answer() {
    let s = mentioned(vec![prompt(2, OWNER, "And X\n"), stop(3, OWNER)]);
    let out = render_messages(&s, &s.messages_after(OWNER, None), OWNER, false, false);
    assert_eq!(
        out,
        format!(
            "Unanswered messages from your owner in session s1 on TW-1, oldest first:\n\n--- {} (the comment that mentioned you) ---\n@tolkster please look\n\n--- {} ---\nAnd X\n\n--- {} (asked you to stop) ---\nStop\n\nAnswer in this session: tolkster activity s1 response --body-file -\n",
            at(0),
            at(2),
            at(3)
        )
    );
    // A reply to Tolkster's own comment, which a session opened there asks, mentioned nothing.
    let note = Session { on_own_note: true, note_replies: vec![request::Reply { created_at: at(0), author_id: Some(OWNER.into()), body: "Listening?".into() }], ..session(vec![]) };
    assert!(render_messages(&note, &note.messages_after(OWNER, None), OWNER, false, false).contains(&format!("--- {} (a reply to your comment) ---
Listening?
", at(0))));
    // A mention in the issue's description: the issue is the request, and the description's
    // text, which anyone may have edited, is never given as the owner's.
    let described = mentioned(vec![]);
    let described = Session { mention: described.mention.map(|m| request::Mention { in_description: true, ..m }), ..described };
    let out = render_messages(&described, &described.messages_after(OWNER, None), OWNER, false, false);
    assert!(out.contains(&format!("--- {} (the issue's description mentioned you) ---\n{DESCRIBED}\n", at(0))) && !out.contains("please look"));
}

#[test]
fn a_delegation_with_nothing_answered_is_its_issue() {
    let delegated = session(vec![said(2, Kind::Progress, "Picked up the issue")]);
    assert!(render_messages(&delegated, &[], OWNER, false, false).contains("the issue is the request"));
    assert!(!render_messages(&delegated, &[], OWNER, false, true).contains("the issue is the request"));
    // Delegated back after the owner's newest message: the issue is the request again.
    let back = redelegated(5, session(vec![prompt(2, OWNER, "Hm")]));
    assert!(render_messages(&back, &[], OWNER, false, true).contains("delegated back to you at 2026-09-26T01:05:00.000Z"));
    // A message after the delegation is the newer request.
    let since = redelegated(5, session(vec![prompt(6, OWNER, "Actually")]));
    assert!(!render_messages(&since, &[], OWNER, false, true).contains("delegated back"));
    // A stop before the re-delegation is still listed, and the re-delegation said after it.
    let stopped = redelegated(5, session(vec![stop(2, OWNER)]));
    let out = render_messages(&stopped, &stopped.messages_after(OWNER, Some(&at(1))), OWNER, false, true);
    assert!(out.contains("(asked you to stop)") && out.find("asked you to stop") < out.find("delegated back to you at"), "{out}");
}

#[test]
fn a_worker_that_has_read_everything_is_told_nothing_is_new() {
    let s = mentioned(vec![]);
    assert!(render_messages(&s, &[], OWNER, true, true).starts_with("No new messages in session s1 on TW-1 since your last read.
"));
    // Before any read, or for anyone else, it is the unanswered.
    assert!(render_messages(&s, &[], OWNER, false, true).starts_with("No unanswered messages in session s1 on TW-1.
"));
}

#[test]
fn a_fresh_worker_is_shown_what_only_a_turn_read() {
    let s = mentioned(vec![prompt(2, OWNER, "And X
")]);
    // The machine holds no read of the worker's, though a turn read the prompt.
    let fresh = Reading { read_through: None, turn_read_through: None, answered_through: None };
    let shown = s.messages_after(OWNER, bound(None, Some(Reader::Worker), &fresh).as_deref());
    assert_eq!(shown.len(), 2);
    // After its own read, only what is newer.
    let read = Reading { read_through: Some(at(2)), turn_read_through: None, answered_through: None };
    assert!(s.messages_after(OWNER, bound(None, Some(Reader::Worker), &read).as_deref()).is_empty());
    // A turn is bound by what was answered alone.
    assert_eq!(bound(None, Some(Reader::Turn), &read), None);
    assert_eq!(bound(Some("t"), Some(Reader::Worker), &read).as_deref(), Some("t"));
}

#[test]
fn a_worker_counts_only_for_sessions_of_the_issue_its_worktree_was_marked_for() {
    let b = session(vec![]);
    // Its own session needs no read of the issue.
    assert!(same_issue("s1", &b, || panic!("asked")));
    // Another session of the same issue counts; one of another issue, or one that can't be read, doesn't.
    assert!(same_issue("s2", &b, || Some("i1".into())));
    assert!(!same_issue("s2", &b, || Some("i2".into())));
    assert!(!same_issue("s2", &b, || None));
    assert!(!same_issue("s2", &Session { issue: None, ..b }, || Some("i1".into())));
}
