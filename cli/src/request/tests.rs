use super::*;

pub const APP: &str = "tolkster";
pub const OWNER: &str = "owner";
pub const STRANGER: &str = "stranger";

pub fn issue() -> Issue {
    Issue {
        id: "i1".into(),
        identifier: "TW-1".into(),
        url: "https://linear.app/w/issue/TW-1".into(),
        branch_name: "tw-1".into(),
        state_type: "unstarted".into(),
        team_id: Some("team-1".into()),
        delegate_id: Some(APP.into()),
        place: Place { project: Some("Tolkster".into()), ..Place::default() },
        estimate: None,
        priority: 0,
        scale: Some(crate::estimates::Scale { kind: crate::estimates::Kind::TShirt, extended: false, zero: false }),
        labels: Vec::new(),
        created_at: at(0),
        delegated: None,
        undelegated_at: None,
        history_full: false,
        created_delegated_by: None,
        canceled_at: None,
        completed_at: None,
        blockers: Vec::new(),
        more_blockers: false,
        parent: None,
        children: Vec::new(),
        more_children: false,
        children_finished_at: None,
    }
}

pub fn at(minute: u32) -> String {
    format!("2026-09-26T01:{minute:02}:00.000Z")
}

pub fn said(minute: u32, kind: Kind, body: &str) -> Activity {
    Activity { id: Some(format!("a{minute}")), created_at: at(minute), author_id: Some(APP.into()), kind, body: Some(body.into()), signal: None }
}

pub fn prompt(minute: u32, author: &str, body: &str) -> Activity {
    Activity { id: Some(format!("p{minute}")), created_at: at(minute), author_id: Some(author.into()), kind: Kind::Prompt, body: Some(body.into()), signal: None }
}

pub fn stop(minute: u32, author: &str) -> Activity {
    Activity { signal: Some("stop".into()), ..prompt(minute, author, "Stop") }
}

// Activities are given oldest first, as they happened, and stored newest first. Only prompts
// are read from Linear: what Tolkster posted among them is dropped, as it is never read back.
pub fn session(activities: Vec<Activity>) -> Session {
    let mut activities: Vec<Activity> = activities.into_iter().filter(|a| a.kind == Kind::Prompt).collect();
    activities.reverse();
    Session {
        id: "s1".into(),
        created_at: at(0),
        creator_id: Some(OWNER.into()),
        app_user_id: Some(APP.into()),
        issue: Some(issue()),
        mention: None,
        on_own_note: false,
        note_replies: Vec::new(),
        activities,
        more_activities: false,
        record: Record::default(),
        delegation: None,
    }
}

pub fn mentioned(activities: Vec<Activity>) -> Session {
    Session { mention: Some(Mention { author_id: Some(OWNER.into()), body: "@tolkster please look".into(), in_description: false }), ..session(activities) }
}


// `s`'s issue was delegated back to Tolkster at `minute`, having been taken back before.
pub fn redelegated(minute: u32, s: Session) -> Session {
    Session { issue: Some(Issue { delegated: Some(Delegated { at: at(minute), by: Some(OWNER.into()), again: true }), ..s.issue.clone().unwrap() }), ..s }
}

#[test]
fn only_the_owners_sessions_have_their_messages_read() {
    let theirs = Session { creator_id: Some(STRANGER.into()), ..session(vec![prompt(1, OWNER, "x")]) };
    assert!(theirs.messages_after(OWNER, None).is_empty());
    assert_eq!(theirs.newest_request(OWNER), None);
    // A mention is the owner's only if the owner wrote the comment too.
    let theirs = Session { mention: Some(Mention { author_id: Some(STRANGER.into()), body: "x".into(), in_description: false }), ..session(vec![]) };
    assert!(theirs.messages_after(OWNER, None).is_empty());
}

#[test]
fn the_messages_are_the_owners_after_the_bound_oldest_first() {
    let s = session(vec![
        prompt(1, OWNER, "Old, answered"),
        said(2, Kind::Answer, "Done"),
        prompt(3, OWNER, "First"),
        prompt(5, STRANGER, "Not the owner"),
        prompt(8, OWNER, "Second"),
    ]);
    let bodies = |after: Option<&str>| s.messages_after(OWNER, after).into_iter().map(|m| m.body).collect::<Vec<_>>();
    assert_eq!(bodies(Some(&at(2))), ["First", "Second"]);
    assert_eq!(bodies(None), ["Old, answered", "First", "Second"]);
    // A stop is a message too, and says so.
    let stopped = session(vec![prompt(1, OWNER, "Go"), stop(2, OWNER)]);
    assert!(stopped.messages_after(OWNER, Some(&at(1)))[0].stop);
}

#[test]
fn a_mention_is_the_first_message_until_a_bound_passes_it() {
    let s = mentioned(vec![said(2, Kind::Progress, "Working"), prompt(3, OWNER, "And X")]);
    assert_eq!(s.messages_after(OWNER, None)[0], Message { created_at: at(0), body: "@tolkster please look".into(), stop: false, mention: true });
    let after = s.messages_after(OWNER, Some(&at(2)));
    assert_eq!(after.len(), 1);
    assert!(!after[0].mention);
}

#[test]
fn the_newest_request_is_the_owners_newest_prompt_or_the_start() {
    assert_eq!(session(vec![]).newest_request(OWNER), Some((None, at(0))));
    let s = session(vec![prompt(1, OWNER, "A"), prompt(3, STRANGER, "B"), prompt(5, OWNER, "C")]);
    assert_eq!(s.newest_request(OWNER), Some((Some("p5".into()), at(5))));
    // With older prompts unread and none of the owner's read, it can't be told.
    let busy = Session { more_activities: true, ..session(vec![prompt(3, STRANGER, "B")]) };
    assert_eq!(busy.newest_request(OWNER), None);
}

#[test]
fn a_sub_issue_in_triage_is_never_chained() {
    let child = |identifier: &str, state: &str| Blocker { identifier: identifier.into(), url: format!("https://linear.app/w/issue/{identifier}"), state_type: state.into(), id: None, updated_at: None };
    let parent = Issue { children: vec![child("TW-4", "triage"), child("TW-2", "backlog"), child("TW-3", "completed"), child("TW-10", "started")], ..issue() };
    let chained: Vec<String> = unfinished_children(&parent).into_iter().map(|(identifier, _)| identifier).collect();
    assert_eq!(chained, ["TW-2", "TW-10"]);
    assert_eq!(triaged_children(&parent), ["TW-4"]);
}

#[test]
fn a_duplicate_has_ended_as_a_cancelled_issue_has() {
    for ended in ["completed", "canceled", "duplicate"] {
        assert!(StateType::from(ended).is_finished(), "{ended}");
    }
    for open in ["triage", "backlog", "unstarted", "started", "anything-new"] {
        assert!(!StateType::from(open).is_finished(), "{open}");
    }
    assert!(StateType::from("duplicate").is_cancelled() && !StateType::from("completed").is_cancelled());
    let child = Blocker { identifier: "TW-3".into(), url: "https://linear.app/w/issue/TW-3".into(), state_type: "duplicate".into(), id: None, updated_at: None };
    assert!(unfinished_children(&Issue { children: vec![child], ..issue() }).is_empty());
    assert_eq!(crate::command::read::state_of(StateType::Duplicate), "cancelled");
}
