use super::*;
use crate::args::parse;
use crate::request::Session;
use crate::commands::messages::render_messages;
use crate::request::tests::{OWNER, at, session};
use crate::state::Record;
use std::cell::RefCell;
use std::rc::Rc;
use crate::orca::{Orca as _, Terminal};
use crate::ports::fake::{Git, Orca, World};

const TIP: &str = "abc1234000000000000000000000000000000000";
const MADE: &str = "r1::/wt/tolkster-conversations";

// Orca with the repo `r1`, where a new conversations checkout is a git checkout on Orca's
// branch of its name, and Orca opens a terminal in it, as it does in each worktree it makes.
fn world() -> Rc<RefCell<World>> {
    let world = World::tolkster();
    {
        let mut w = world.borrow_mut();
        w.tip = TIP.into();
        w.git.insert("/wt/tolkster-conversations".into(), Git { head: "1".repeat(40), branch: Some(CHECKOUT_NAME.into()), branches: vec![CHECKOUT_NAME.into()], ..Git::default() });
    }
    world
}

fn orca(world: &Rc<RefCell<World>>) -> Orca<'static> {
    let mut orca = Orca::on(world);
    let made = Rc::clone(world);
    orca.after_create = Some(Box::new(move || made.borrow_mut().terminals.push(Terminal { handle: Some("term_orca".into()), worktree_id: Some(MADE.into()), agent: false, pane: None })));
    orca
}

// A checkout `r1::/wt/c` Orca lists, on branch `feature`, recorded for `r1`.
fn recorded(world: &Rc<RefCell<World>>) -> [CheckoutRecord; 1] {
    let mut w = world.borrow_mut();
    w.worktrees.push(Worktree { id: "r1::/wt/c".into(), repo_id: "r1".into(), path: "/wt/c".into(), branch: Some("feature".into()), linked_issue: None, is_main: false, is_local: true, agents: Vec::new(), origin: None });
    w.git.insert("/wt/c".into(), Git { head: "2".repeat(40), branch: Some("feature".into()), branches: vec!["feature".into()], ..Git::default() });
    [CheckoutRecord { repo_id: "r1".into(), worktree_id: "r1::/wt/c".into(), at: iso(0) }]
}

fn terminal_in(world: &Rc<RefCell<World>>, worktree_id: &str) {
    world.borrow_mut().terminals.push(Terminal { handle: Some("term_1".into()), worktree_id: Some(worktree_id.into()), agent: false, pane: None });
}

fn listed(world: &Rc<RefCell<World>>) -> Vec<Worktree> {
    world.borrow().worktrees.clone()
}

#[test]
fn a_recorded_checkout_orca_lists_is_used_as_it_is() {
    let world = world();
    let records = recorded(&world);
    let (found, fresh) = checkout(&mut orca(&world), &records, "r1", &listed(&world), &mut |_| panic!("recorded again"), 0).unwrap();
    assert_eq!(found, Checkout { worktree_id: "r1::/wt/c".into(), path: "/wt/c".into() });
    let w = world.borrow();
    assert!(!fresh && w.ran.is_empty() && w.worktrees.len() == 2, "{:?}", w.ran);
}

#[test]
fn a_recorded_checkout_on_a_branch_with_a_terminal_open_gets_no_close_and_no_refresh() {
    // A turn switched branches in it, and another conversation's turn runs there.
    let world = world();
    let records = recorded(&world);
    terminal_in(&world, "r1::/wt/c");
    let (_, fresh) = checkout(&mut orca(&world), &records, "r1", &listed(&world), &mut |_| panic!("recorded again"), 0).unwrap();
    assert!(!fresh);
    assert!(refresh_idle(&Checkouts::default(), &mut orca(&world), &records, &listed(&world), None).is_empty());
    let w = world.borrow();
    assert!(w.ran.is_empty() && w.closed.is_empty(), "{:?}", w.ran);
    assert_eq!(w.git["/wt/c"].branch.as_deref(), Some("feature"));
}

#[test]
fn a_refresh_detaches_without_moving_the_branch() {
    let world = world();
    recorded(&world);
    let found = Checkout { worktree_id: "r1::/wt/c".into(), path: "/wt/c".into() };
    refresh(orca(&world).shell(), &found).unwrap();
    let w = world.borrow();
    let dir = "[/wt/c] git";
    assert_eq!(
        w.ran,
        [
            format!("{dir} -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=60 fetch --quiet origin"),
            format!("{dir} rev-parse --verify --quiet origin/HEAD^{{commit}}"),
            format!("{dir} switch --quiet --detach --discard-changes {TIP}"),
            format!("{dir} clean --quiet -fd"),
        ]
    );
    let git = &w.git["/wt/c"];
    assert!(git.head == TIP && git.branch.is_none() && git.branches == ["feature"]);
}

#[test]
fn a_missing_checkout_is_created_recorded_detached_and_refreshed() {
    // Recorded, but gone from Orca: made anew.
    let world = world();
    let records = [CheckoutRecord { repo_id: "r1".into(), worktree_id: "r1::/wt/old".into(), at: iso(0) }];
    let mut recorded = Vec::new();
    let mut record = |r: &CheckoutRecord| {
        // Orca's terminal in it is closed before it is recorded.
        assert!(!world.borrow().terminals.iter().any(|t| t.worktree_id.as_deref() == Some(MADE)));
        recorded.push(r.clone());
        Ok(())
    };
    let (made, fresh) = checkout(&mut orca(&world), &records, "r1", &listed(&world), &mut record, 60_000).unwrap();
    assert!(fresh);
    assert_eq!(made, Checkout { worktree_id: MADE.into(), path: "/wt/tolkster-conversations".into() });
    assert_eq!(recorded, [CheckoutRecord { repo_id: "r1".into(), worktree_id: MADE.into(), at: iso(60_000) }]);
    let w = world.borrow();
    // Linked to no issue and with no setup run, so it is never taken for a worker.
    assert_eq!(w.created, [("r1".to_string(), CHECKOUT_NAME.to_string(), vec!["--no-parent".to_string(), "--setup".into(), "skip".into()])]);
    assert_eq!(w.closed, ["term_orca"]);
    let dir = "[/wt/tolkster-conversations] git";
    // Fetched once, and Orca's branch goes only if merged.
    assert_eq!(
        w.ran,
        [
            format!("{dir} rev-parse --abbrev-ref HEAD"),
            format!("{dir} -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=60 fetch --quiet origin"),
            format!("{dir} rev-parse --verify --quiet origin/HEAD^{{commit}}"),
            format!("{dir} switch --quiet --detach --discard-changes {TIP}"),
            format!("{dir} clean --quiet -fd"),
            format!("{dir} branch --quiet -d tolkster-conversations"),
        ]
    );
    let git = &w.git["/wt/tolkster-conversations"];
    assert!(git.head == TIP && git.branch.is_none() && git.branches.is_empty());
}

#[test]
fn a_new_checkouts_branch_holding_commits_of_its_own_is_kept() {
    let world = world();
    world.borrow_mut().git.get_mut("/wt/tolkster-conversations").unwrap().unpushed = 1;
    assert!(checkout(&mut orca(&world), &[], "r1", &listed(&world), &mut |_| Ok(()), 0).is_ok());
    assert_eq!(world.borrow().git["/wt/tolkster-conversations"].branches, [CHECKOUT_NAME]);
}

#[test]
fn a_checkout_that_cant_be_recorded_is_removed() {
    let world = world();
    let made = checkout(&mut orca(&world), &[], "r1", &listed(&world), &mut |_| Err(Error::Failed("the state is down".into())), 0);
    assert!(made.is_err());
    let w = world.borrow();
    assert!(w.closed == ["term_orca"] && w.removed == [MADE] && w.ran.is_empty(), "{:?}", w.ran);
}

#[test]
fn only_an_idle_checkout_is_refreshed() {
    let world = world();
    let records = recorded(&world);
    terminal_in(&world, "r1::/wt/c");
    assert!(refresh_idle(&Checkouts::default(), &mut orca(&world), &records, &listed(&world), None).is_empty());
    assert!(world.borrow().ran.is_empty());
    // A terminal elsewhere doesn't keep it.
    world.borrow_mut().terminals[0].worktree_id = Some("r1::/wt/other".into());
    assert_eq!(refresh_idle(&Checkouts::default(), &mut orca(&world), &records, &listed(&world), None), [Refresh::Refreshed { repo_id: "r1".into() }]);
    assert_eq!(world.borrow().git["/wt/c"].head, TIP);
    // Not listed by Orca: left for the next turn to create.
    world.borrow_mut().ran.clear();
    assert!(refresh_idle(&Checkouts::default(), &mut orca(&world), &records, &[], None).is_empty());
    assert!(world.borrow().ran.is_empty());
}

#[test]
fn a_push_to_the_default_branch_refreshes_only_its_repos_checkout_not_at_the_tip() {
    let world = world();
    let records = recorded(&world);
    world.borrow_mut().worktrees.last_mut().unwrap().origin = Some("acme/tolkster".into());
    let tip = Sha::parse(TIP).unwrap();
    // Another repo's push.
    assert!(checkouts_of(&records, &listed(&world), "acme/other").is_empty());
    let ours = checkouts_of(&records, &listed(&world), "Acme/Tolkster");
    assert_eq!(ours, records);
    assert_eq!(refresh_idle(&Checkouts::default(), &mut orca(&world), &ours, &listed(&world), Some(&tip)), [Refresh::Refreshed { repo_id: "r1".into() }]);
    assert_eq!(world.borrow().git["/wt/c"].head, TIP);
    // At the tip already: HEAD is read, and nothing fetched.
    world.borrow_mut().ran.clear();
    assert!(refresh_idle(&Checkouts::default(), &mut orca(&world), &ours, &listed(&world), Some(&tip)).is_empty());
    assert_eq!(world.borrow().ran, ["[/wt/c] git rev-parse --verify --quiet HEAD^{commit}"]);
}

#[test]
fn a_turn_skips_the_fetch_only_at_the_known_tip() {
    let world = world();
    recorded(&world);
    let found = Checkout { worktree_id: "r1::/wt/c".into(), path: "/wt/c".into() };
    let head = Sha::parse(&"2".repeat(40)).unwrap();
    assert!(at_tip(orca(&world).shell(), &found, Some(&head)));
    // A tip it isn't at, or none known, as before the relay's first: refreshed.
    assert!(!at_tip(orca(&world).shell(), &found, Some(&Sha::parse(TIP).unwrap())));
    assert!(!at_tip(orca(&world).shell(), &found, None));
    // At the tip, the tree is still detached and cleaned, with no fetch.
    world.borrow_mut().ran.clear();
    reset(orca(&world).shell(), &found, &head).unwrap();
    let ran = world.borrow().ran.clone();
    assert_eq!(ran, [format!("[/wt/c] git switch --quiet --detach --discard-changes {head}"), "[/wt/c] git clean --quiet -fd".to_string()]);
    // A tip is new once per commit.
    let checkouts = Checkouts::default();
    assert!(checkouts.base_tip("acme/tip-test", &head) && !checkouts.base_tip("ACME/tip-test", &head));
    assert_eq!(checkouts.known_tip("acme/Tip-Test"), Some(head));
}

#[test]
fn a_turn_runs_on_ids_a_time_and_a_quoted_path() {
    let brief = PathBuf::from("C:/Users/Sam Smith/briefs/TW-1-s1.md");
    let (session_id, agent_session, after) = (Id::from("s1"), Id::from("0a1b2c3d-0000-4000-8000-000000000001"), Time::from("2026-09-26T01:00:00.000Z"));
    let turn = Turn { session_id: &session_id, agent_session: &agent_session, request: None, messages_after: Some(&after), brief: &brief, stale: false };
    assert_eq!(
        turn.command().unwrap().as_str(),
        "tolkster converse s1 --agent-session 0a1b2c3d-0000-4000-8000-000000000001 --messages-after 2026-09-26T01:00:00.000Z --brief \"C:/Users/Sam Smith/briefs/TW-1-s1.md\""
    );
    // The first turn: nothing handled before it.
    assert!(!Turn { messages_after: None, ..turn }.command().unwrap().as_str().contains("--messages-after"));
    assert!(Turn { stale: true, ..turn }.command().unwrap().as_str().ends_with(" --stale"));
    let quoted = PathBuf::from("C:/a\"b/TW-1.md");
    assert!(Turn { brief: &quoted, ..turn }.command().is_err());
}

// The line `command` writes, as `tolkster converse` receives it: its words after the program's name
// and the quotes a terminal takes off the path.
fn words(line: &str) -> Vec<String> {
    let rest = line.strip_prefix("tolkster converse ").unwrap();
    let (head, quoted) = rest.split_once(" --brief \"").unwrap();
    let (path, tail) = quoted.split_once('"').unwrap();
    head.split_whitespace().map(String::from).chain(["--brief".into(), path.into()]).chain(tail.split_whitespace().map(String::from)).collect()
}

#[test]
fn a_turns_command_line_parses_back_to_the_turn() {
    let brief = PathBuf::from("C:/Users/Sam Smith/briefs/TW-1-s1.md");
    let (session_id, agent_session, after) = (Id::from("s1"), Id::from("0a1b2c3d-0000-4000-8000-000000000001"), Time::from("2026-09-26T01:00:00.000Z"));
    let request = Time::from("2026-09-26T01:01:00.000Z");
    let full = Turn { session_id: &session_id, agent_session: &agent_session, request: Some(&request), messages_after: Some(&after), brief: &brief, stale: true };
    for turn in [
        Turn { request: None, messages_after: None, stale: false, ..full },
        Turn { messages_after: None, ..full },
        Turn { stale: false, ..full },
        full,
    ] {
        let argv = words(turn.command().unwrap().as_str());
        let parsed = parse(&argv, &TURN_OPTIONS, true).unwrap();
        let back = ParsedTurn::parse(&parsed).unwrap().unwrap();
        let back = back.turn();
        assert_eq!((back.session_id, back.agent_session, back.request, back.messages_after, back.brief, back.stale), (turn.session_id, turn.agent_session, turn.request, turn.messages_after, turn.brief, turn.stale));
    }
}

#[test]
fn a_turn_refuses_an_id_or_a_time_that_isnt_plain() {
    for bad in [["s1", "--agent-session=-x", "--brief", "b.md"], ["s1; rm", "--agent-session=c1", "--brief", "b.md"], ["s1", "--agent-session=--dangerously-skip-permissions", "--brief", "b.md"]] {
        let argv: Vec<String> = bad.map(String::from).into();
        let parsed = parse(&argv, &TURN_OPTIONS, true).unwrap();
        assert!(ParsedTurn::parse(&parsed).is_err(), "{bad:?}");
    }
    for after in ["--all", "now"] {
        let argv: Vec<String> = ["s1".to_string(), "--agent-session=c1".into(), "--brief".into(), "b.md".into(), format!("--messages-after={after}")].into();
        let parsed = parse(&argv, &TURN_OPTIONS, true).unwrap();
        assert!(ParsedTurn::parse(&parsed).is_err(), "{after}");
    }
}

#[test]
fn a_turns_brief_is_its_sessions_or_its_threads() {
    assert_eq!(brief_path("/briefs/<repo>", "tolkster", "TW-1", "s1"), Path::new("/briefs/tolkster/TW-1-s1.md"));
    assert_eq!(brief_path("/briefs/<repo>", "tolkster", "TW-1", ROOT), Path::new(&format!("/briefs/tolkster/TW-1-{ROOT}.md")));
}

#[test]
fn a_turns_prompt_holds_its_brief_and_messages_each_delimited() {
    let session = conversing(Some("term_1"));
    let messages = render_messages(&session, &session.messages_after(OWNER, Some(&at(0))), OWNER, false, true);
    let prompt = prompt(Request::Session("s1"), Brief::Text("# TW-1: agent conversation\n\nThe brief.\n"), Some(&messages), Some(&at(0)), false);
    let brief = prompt.split_once("\n<brief>\n").and_then(|(_, rest)| rest.split_once("\n</brief>\n")).map(|(brief, _)| brief);
    assert_eq!(brief, Some("# TW-1: agent conversation\n\nThe brief."));
    let (held, _) = prompt.split_once("\n<messages>\n").and_then(|(_, rest)| rest.split_once("\n</messages>\n")).unwrap();
    assert!(held.contains("One more thing") && held.contains("tolkster activity s1 response"), "{held}");
    // The messages are the request, and the command that prints them re-reads them.
    assert!(prompt.contains(&format!("as `tolkster messages s1 --after {}` prints them", at(0))) && !prompt.contains("up to date"), "{prompt}");
    // Unread: the turn reads them itself, and the brief is still there.
    let unread = super::prompt(Request::Session("s1"), Brief::Text("The brief."), None, None, true);
    assert!(unread.contains("read them with `tolkster messages s1`") && unread.contains("<brief>\nThe brief.\n</brief>") && !unread.contains("<messages>"), "{unread}");
    // A checkout that couldn't be refreshed: the turn is told it may be behind.
    assert!(unread.contains("may be behind the default branch"));
    // A resumed turn already holds the brief: it gets the file's path, not the text.
    let resumed = super::prompt(Request::Session("s1"), Brief::Held(Path::new("/briefs/TW-1-s1.md")), Some(&messages), Some(&at(0)), false);
    assert!(resumed.contains("the one in your first turn, also the file `/briefs/TW-1-s1.md`;") && !resumed.contains("<brief>") && !resumed.contains("The brief."), "{resumed}");
    assert!(resumed.contains("\n<messages>\n") && resumed.contains("One more thing"), "{resumed}");
}

// A conversation whose latest turn runs in `terminal`, with the owner's reply at 2.
fn conversing(terminal: Option<&str>) -> Session {
    let mut s = session(vec![crate::request::tests::prompt(2, OWNER, "One more thing")]);
    s.record = Record { conversation: Some(Conversation { agent_session: "0a1b2c3d-0000-4000-8000-000000000001".into(), terminal: terminal.map(String::from), at: at(1) }), ..Record::default() };
    s
}

const ROOT: &str = "0a1b2c3d-0000-4000-8000-00000000c001";

// Tolkster's comment ROOT on TW-1, with the owner's replies r1 to r<n>, a minute apart.
fn thread(replies: u32) -> Thread {
    let replies = (1..=replies).map(|n| crate::request::ThreadReply { id: format!("r{n}").as_str().into(), created_at: at(n).as_str().into(), body: format!("reply {n}") }).collect();
    Thread { root: ROOT.into(), issue_id: "i1".into(), identifier: "TW-1".into(), place: Default::default(), replies }
}

// A turn's batch, as the machine bounds it: the replies after the newest a turn was shown, up
// to its request. A reply after the request is the machine's next request.
#[test]
fn a_thread_turn_takes_the_replies_after_the_newest_shown_up_to_its_request() {
    let ids = |after: Option<&str>, request: &str| batch(&thread(4), after, request).iter().map(|r| r.id.to_string()).collect::<Vec<_>>();
    assert_eq!(ids(None, &at(2)), ["r1", "r2"]);
    assert_eq!(ids(Some(&at(1)), &at(3)), ["r2", "r3"]);
    assert!(ids(Some(&at(4)), &at(4)).is_empty());
}

#[test]
fn a_thread_turn_runs_on_ids_a_time_and_a_quoted_path() {
    let brief = PathBuf::from("C:/Users/Sam Smith/briefs/TW-1-c.md");
    let (root, request, after) = (Id::from(ROOT), Time::from("2026-09-26T01:02:00.000Z"), Time::from("2026-09-26T01:00:00.000Z"));
    let turn = ThreadTurn { root: &root, request: &request, replies_after: Some(&after), brief: &brief, stale: false };
    assert_eq!(
        turn.command().unwrap().as_str(),
        format!("tolkster converse --thread {ROOT} --request 2026-09-26T01:02:00.000Z --replies-after 2026-09-26T01:00:00.000Z --brief \"C:/Users/Sam Smith/briefs/TW-1-c.md\"")
    );
    assert!(!ThreadTurn { replies_after: None, ..turn }.command().unwrap().as_str().contains("--replies-after"));
    assert!(ThreadTurn { stale: true, ..turn }.command().unwrap().as_str().ends_with(" --stale"));
    // Claude takes only a UUID for its session's ID.
    assert!(ThreadTurn { root: &"c1".into(), ..turn }.command().is_err());
}

#[test]
fn a_thread_turns_prompt_holds_its_brief_and_replies_each_delimited() {
    // A reply after the request is the next request's.
    let next = crate::commands::messages::render_replies(&thread(3), Some(&at(1)), Some(&at(2)));
    assert!(next.contains("reply 2") && !next.contains("reply 3"), "{next}");
    let replies = crate::commands::messages::render_replies(&thread(2), Some(&at(1)), None);
    assert!(replies.contains("reply 2") && !replies.contains("reply 1"), "{replies}");
    assert!(replies.contains(&format!("tolkster comment TW-1 --reply-to {ROOT} --body-file -")), "{replies}");
    let prompt = prompt(Request::Thread(ROOT), Brief::Text("The brief.\n"), Some(&replies), Some(&at(1)), false);
    let (held, _) = prompt.split_once("\n<replies>\n").and_then(|(_, rest)| rest.split_once("\n</replies>\n")).unwrap();
    assert!(held.contains("reply 2"), "{held}");
    assert!(prompt.contains(&format!("as `tolkster messages --thread {ROOT} --after {}` prints them", at(1))), "{prompt}");
    assert!(prompt.contains("\n<brief>\nThe brief.\n</brief>\n"), "{prompt}");
    let unread = super::prompt(Request::Thread(ROOT), Brief::Text("The brief."), None, None, true);
    assert!(unread.contains(&format!("read them with `tolkster messages --thread {ROOT}`")) && !unread.contains("<replies>") && unread.contains("up to date"), "{unread}");
    let resumed = super::prompt(Request::Thread(ROOT), Brief::Held(Path::new("/briefs/TW-1.md")), Some(&replies), Some(&at(1)), false);
    assert!(resumed.contains("the file `/briefs/TW-1.md`;") && !resumed.contains("<brief>") && resumed.contains("\n<replies>\n"), "{resumed}");
    let none = crate::commands::messages::render_replies(&thread(0), None, None);
    assert!(none.starts_with(&format!("No replies from your owner in the thread of your comment {ROOT} on TW-1.")), "{none}");
}

// The line typed into a turn is two times and a flag, and nothing else is taken for one.
#[test]
fn a_handover_line_is_times_and_a_flag_only() {
    let (request, after) = (Time::from("2026-10-04T13:00:00.000Z"), "2026-10-04T12:00:00.000Z");
    for stale in [false, true] {
        let line = handover(&request, after, stale).unwrap();
        assert_eq!(handed(line.as_str()), Some(Handed { request: request.clone(), after: after.into(), stale }));
    }
    assert!(handover(&request, "now", false).is_err());
    for typed in ["", "2026-10-04T13:00:00.000Z", "now 2026-10-04T13:00:00.000Z", "2026-10-04T13:00:00.000Z 2026-10-04T12:00:00.000Z fresh", "2026-10-04T13:00:00.000Z 2026-10-04T12:00:00.000Z stale stale", "tolkster messages s1"] {
        assert!(handed(typed).is_none(), "{typed}");
    }
}

// Each handed request is taken once, and only one newer than the last; a thread's turn holds
// one handed while Claude works, the newest of them, until Claude is done.
#[test]
fn a_thread_turn_takes_a_handed_request_only_once_claude_is_idle() {
    let t = |hour: u32| format!("2026-10-04T{hour:02}:00:00.000Z");
    let line = |request: u32| format!("{} {}", t(request), t(10));
    let mut session = Takes::new(Some(Time::from(t(11).as_str())), false);
    assert_eq!(session.typed(&line(11), true), None);
    assert_eq!(session.typed(&line(12), true).map(|h| h.request), Some(Time::from(t(12).as_str())));
    assert_eq!(session.typed(&line(12), false), None);
    let mut thread = Takes::new(Some(Time::from(t(11).as_str())), true);
    assert_eq!(thread.typed(&line(12), true), None);
    assert_eq!(thread.typed(&line(13), true), None);
    assert_eq!(thread.typed(&line(12), true), None);
    assert_eq!(thread.idle().map(|h| h.request), Some(Time::from(t(13).as_str())));
    assert_eq!((thread.idle(), thread.typed(&line(13), false)), (None, None));
    assert_eq!(thread.typed(&line(14), false).map(|h| h.request), Some(Time::from(t(14).as_str())));
}
