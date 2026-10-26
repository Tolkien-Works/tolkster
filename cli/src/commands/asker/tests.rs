use super::*;

#[test]
fn without_a_session_only_the_owners_own_agent_writes() {
    // The owner's agent at their checkout: no session, unmarked, no conversation.
    assert!(matches!(Asker::of(None, || own_agent(false, false, false)), Ok(Asker::Owner)));
    // A worker's worktree, and a conversation's turn, each refused the session-less path.
    for (marked, conversation) in [(true, false), (false, true), (true, true)] {
        let refused = Asker::of(None, || own_agent(marked, conversation, false));
        assert!(matches!(refused, Err(Error::Failed(ref why)) if why.contains("--session")), "{marked} {conversation}");
    }
    // Named, the session is checked as before, wherever the call runs, and nothing asks
    // where that is.
    let named = Asker::of(Some("s1"), || unreachable!("a named session asks nothing of the directory"));
    assert!(matches!(named, Ok(Asker::Session(ref id)) if id == "s1"));
    assert!(matches!(Asker::of(Some(""), || None), Err(Error::Usage(_))));
    // A reply's turn names its thread instead.
    assert!(matches!(Asker::of(None, || own_agent(false, false, true)), Err(Error::Usage(ref why)) if why.contains("--thread")));
}

#[test]
fn a_thread_asks_only_in_its_own_replys_turn() {
    assert!(matches!(Asker::in_thread("c1", false, Some("c1")), Ok(Asker::Thread(ref root)) if root == "c1"));
    for (thread, session, turn) in [("c2", false, Some("c1")), ("c1", false, None), ("c1", true, Some("c1"))] {
        assert!(matches!(Asker::in_thread(thread, session, turn), Err(Error::Usage(_))), "{thread} {session} {turn:?}");
    }
}

#[test]
fn a_thread_is_the_owners_once_they_replied_by_the_request() {
    use crate::plain::{Id, IssueKey, Time};
    use crate::request::ThreadReply;
    let reply = |at: &str| ThreadReply { id: Id::parse("r1").unwrap(), created_at: Time::parse(at).unwrap(), body: String::new() };
    let thread = |replies| Thread { root: Id::parse("c1").unwrap(), issue_id: Id::parse("i1").unwrap(), identifier: IssueKey::parse("TW-1").unwrap(), place: Default::default(), replies };
    let request = "2026-10-05T01:00:00.000Z";
    assert!(owner_asked(&thread(vec![reply(request)]), request));
    assert!(owner_asked(&thread(vec![reply("2026-10-05T00:59:00.000Z")]), request));
    // No reply of the owner's, or only one written after the turn's request.
    assert!(!owner_asked(&thread(vec![]), request));
    assert!(!owner_asked(&thread(vec![reply("2026-10-05T01:00:01.000Z")]), request));
}
