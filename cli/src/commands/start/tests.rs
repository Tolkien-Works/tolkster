use super::*;

fn issue() -> Value {
    json!({ "issue": { "identifier": "TW-1" }, "comments": [] })
}

#[test]
fn the_issue_and_the_messages_print_as_one_object() {
    let (out, failed) = printed(Ok(issue()), Ok("No unanswered messages.\n".into())).unwrap();
    assert_eq!(out, json!({ "issue": { "identifier": "TW-1" }, "comments": [], "messages": "No unanswered messages.\n" }));
    assert!(failed.is_none());
}

#[test]
fn half_read_prints_that_half_and_fails() {
    // The messages may be reported as read already, so they print without the issue.
    let (out, failed) = printed(Err(Error::Failed("no issue".into())), Ok("New.\n".into())).unwrap();
    assert_eq!(out, json!({ "messages": "New.\n" }));
    assert!(failed.is_some());
    let (out, failed) = printed(Ok(issue()), Err(Error::Failed("down".into()))).unwrap();
    assert_eq!(out, issue());
    assert!(failed.is_some());
    assert!(printed(Err(Error::Failed("a".into())), Err(Error::Failed("b".into()))).is_err());
}
