use super::*;

fn handed_turn(id: &str, request: &str, after: &str) -> Command {
    Command { kind: Kind::StartTurn, ..order(id, Order::StartTurn { session_id: "s1".into(), thread: false, request: Some(request.into()), after: Some(after.into()) }) }
}

// A conversation's turn on the fakes alone: the repo's conversations checkout is made,
// recorded and brought to origin's tip, and the turn's terminal opens there, recorded on the
// session. Sent again, it answers with the turn already open.
#[test]
fn a_sessions_turn_opens_in_a_conversations_checkout_made_for_it() {
    let config = fake::config(&briefs("session-turn"));
    let (linear, records, _) = fakes();
    let world = conversing_world();
    let opened = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &turn("c-turn", "s1", false, None)).unwrap();
    let terminal = opened.detail["terminal"].as_str().unwrap().to_string();
    {
        let w = world.borrow();
        assert_eq!(w.opened.len(), 1);
        let (worktree_id, title, command) = &w.opened[0];
        assert_eq!((worktree_id.as_str(), title.as_deref()), (CONVERSATIONS, Some("TW-1")));
        assert!(command.starts_with("tolkster converse s1 ") && !command.contains("--stale"), "{command}");
        assert_eq!(w.git["/wt/tolkster-conversations"].head, TIP);
    }
    assert_eq!(records.checkouts.borrow().iter().map(|c| (c.repo_id.as_str(), c.worktree_id.as_str())).collect::<Vec<_>>(), [("r1", CONVERSATIONS)]);
    assert_eq!(records.conversations.borrow()["s1"].terminal.as_deref(), Some(terminal.as_str()));
    let again = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &turn("c-turn", "s1", false, None)).unwrap();
    assert_eq!(again.detail["terminal"], json!(terminal));
    assert_eq!(world.borrow().opened.len(), 1);
}

// A session's turn open in the checkout is handed the request by one line typed into its
// terminal, times and nothing of Linear's, and opens nothing; once its terminal has gone, a
// new turn opens. Its close says nothing. A line that couldn't be typed isn't tried
// again: the turn is closed and replaced.
#[test]
fn an_open_turn_is_handed_the_request_and_its_close_says_nothing() {
    let config = fake::config(&briefs("warm-turn"));
    let (linear, records, _) = fakes();
    let world = conversing_world();
    let (first, request, after) = ("2026-10-04T12:00:00.000Z", "2026-10-04T13:00:00.000Z", "2026-10-04T12:30:00.000Z");
    let opened = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &handed_turn("c-turn", first, after)).unwrap();
    let terminal = opened.detail["terminal"].as_str().unwrap().to_string();
    assert!(world.borrow().opened[0].2.contains(&format!(" --request {first} ")), "{:?}", world.borrow().opened);
    let handed = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &handed_turn("c-warm", request, after)).unwrap();
    assert_eq!(handed.detail["terminal"], json!(terminal));
    assert_eq!((world.borrow().opened.len(), world.borrow().typed.clone()), (1, vec![(terminal.clone(), format!("{request} {after}"))]));
    let close = Command { kind: Kind::StopTurn, ..order("c-idle", Order::StopTurn { session_id: "s1".into(), thread: false, idle: true }) };
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &close).unwrap();
    assert_eq!(world.borrow().closed, std::slice::from_ref(&terminal));
    assert!(linear.posted("response").is_empty());
    let reopened = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &handed_turn("c-warm-2", request, after)).unwrap();
    let second = reopened.detail["terminal"].as_str().unwrap().to_string();
    assert_ne!(second, terminal);
    assert_eq!((world.borrow().opened.len(), world.borrow().typed.len()), (2, 1));
    world.borrow_mut().send = fake::Send::Fails;
    let replaced = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &handed_turn("c-warm-3", request, after)).unwrap();
    assert_ne!(replaced.detail["terminal"], json!(second));
    assert_eq!((world.borrow().opened.len(), world.borrow().typed.len(), world.borrow().closed.last().cloned()), (3, 1, Some(second)));
}

const ROOT: &str = "0a1b2c3d-0000-4000-8000-00000000c513";

// A note's thread on TW-1 with one fresh reply of the owner's.
fn note_thread(linear: &fake::Linear, project: &str) -> String {
    let created = crate::time::iso(crate::time::now_ms() - 60_000);
    let reply = crate::request::ThreadReply { id: "r1".into(), created_at: created.as_str().into(), body: String::new() };
    let place = crate::config::repo_map::Place { project: Some(project.into()), ..crate::config::repo_map::Place::default() };
    linear.threads.borrow_mut().insert(ROOT.into(), Thread { root: ROOT.into(), issue_id: "i1".into(), identifier: "TW-1".into(), place, replies: vec![reply] });
    created
}

fn thread_turn(id: &str, request: &str, after: Option<&str>) -> Command {
    Command { kind: Kind::StartTurn, ..order(id, Order::StartTurn { session_id: ROOT.into(), thread: true, request: Some(request.into()), after: after.map(Into::into) }) }
}

fn thread_stop(id: &str) -> Command {
    Command { kind: Kind::StopTurn, ..order(id, Order::StopTurn { session_id: ROOT.into(), thread: true, idle: false }) }
}

fn thread_stalled(id: &str, through: &str) -> Command {
    let notice = Order::PostNotice { session_id: Some(ROOT.into()), thread: true, notice: Notice::Stalled, blockers: Vec::new(), pr: None, rounds: None, through: Some(through.into()) };
    Command { kind: Kind::PostNotice, ..order(id, notice) }
}

// The conversations checkout, made and recorded for the repo.
fn conversations(world: &Rc<RefCell<World>>, records: &fake::Records) {
    world.borrow_mut().worktrees.push(Worktree { id: CONVERSATIONS.into(), repo_id: "r1".into(), path: "/wt/tolkster-conversations".into(), branch: None, linked_issue: None, is_main: false, is_local: true, agents: Vec::new(), origin: None });
    records.checkouts.borrow_mut().push(crate::state::CheckoutRecord { repo_id: "r1".into(), worktree_id: CONVERSATIONS.into(), at: at(0) });
}

// A note's thread's turn on the fakes alone: it opens on its batch in the recorded
// conversations checkout, refreshed while nothing runs there, takes no claim, and answers
// with its terminal, recorded under the root. Sent again while it runs, it answers with that
// turn and opens nothing. With no reply after the newest a turn was shown it opens nothing,
// and with no repo for the issue it says so once, as the request's answer.
#[test]
fn a_thread_turn_opens_on_its_batch_and_answers_with_its_terminal() {
    let config = fake::config(&briefs("thread-turn"));
    let (linear, records, _) = fakes();
    let world = conversing_world();
    let through = note_thread(&linear, "Tolkster");
    conversations(&world, &records);
    let opened = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &thread_turn("c-thread", &through, None)).unwrap();
    assert_eq!(opened.outcome, Outcome::Ok);
    let terminal = opened.detail["terminal"].as_str().unwrap().to_string();
    {
        let w = world.borrow();
        assert_eq!(w.opened.len(), 1);
        assert!(w.opened[0].0 == CONVERSATIONS && w.opened[0].2.contains(&format!("tolkster converse --thread {ROOT} --request {through} --brief")), "{:?}", w.opened);
        assert!(w.terminals.iter().any(|t| t.handle.as_deref() == Some(terminal.as_str())));
        assert_eq!(w.git["/wt/tolkster-conversations"].head, TIP);
    }
    assert!(records.claims.borrow().is_empty(), "{:?}", records.claims.borrow());
    assert_eq!(records.conversations.borrow()[ROOT].terminal.as_deref(), Some(terminal.as_str()));
    // Sent again, even with a newer cursor, it is the turn that runs.
    let again = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &thread_turn("c-thread", &through, Some(&through))).unwrap();
    assert_eq!((again.detail["terminal"].clone(), world.borrow().opened.len()), (json!(terminal), 1));
    // Once it has ended, a reply a turn was shown already opens nothing.
    world.borrow_mut().terminals.retain(|t| t.handle.as_deref() != Some(terminal.as_str()));
    let answered = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &thread_turn("c-answered", &through, Some(&through))).unwrap();
    assert_eq!((answered.outcome, world.borrow().opened.len()), (Outcome::Ok, 1));
    // No repo for its issue: said once, as the request's answer, and no turn opens.
    let (linear, records, _) = fakes();
    let world = conversing_world();
    let through = note_thread(&linear, "Elsewhere");
    for _ in 0..2 {
        assert_eq!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &thread_turn("c-norepo", &through, None)).unwrap().outcome, Outcome::Ok);
    }
    assert_eq!(linear.replies.borrow().iter().map(|(key, root, body)| (key.clone(), root.as_str(), body.as_str())).collect::<Vec<_>>(), [(answer_key(ROOT, &through), ROOT, voice::NO_REPO)]);
    assert!(world.borrow().opened.is_empty());
}

// A note's thread's turn open in the checkout, a warm one, is handed a newer reply's request
// by one line typed into its terminal, times and nothing of Linear's, and opens nothing. A
// line that couldn't be typed isn't tried again: the turn is closed and replaced. One whose
// issue maps to no repo fails, and is left to the machine: it may be working.
#[test]
fn an_open_thread_turn_is_handed_the_request_or_replaced() {
    let config = fake::config(&briefs("thread-warm"));
    let (linear, records, _) = fakes();
    let world = conversing_world();
    let through = note_thread(&linear, "Tolkster");
    conversations(&world, &records);
    let opened = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &thread_turn("c-thread", &through, None)).unwrap();
    let terminal = opened.detail["terminal"].as_str().unwrap().to_string();
    let request = crate::time::iso(crate::time::now_ms());
    linear.threads.borrow_mut().get_mut(ROOT).unwrap().replies.push(crate::request::ThreadReply { id: "r2".into(), created_at: request.as_str().into(), body: String::new() });
    let handed = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &thread_turn("c-warm", &request, Some(&through))).unwrap();
    assert_eq!(handed.detail["terminal"], json!(terminal));
    assert_eq!((world.borrow().opened.len(), world.borrow().typed.clone()), (1, vec![(terminal.clone(), format!("{request} {through}"))]));
    world.borrow_mut().send = fake::Send::Fails;
    let replaced = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &thread_turn("c-warm-2", &request, Some(&through))).unwrap();
    assert_ne!(replaced.detail["terminal"], json!(terminal));
    assert_eq!((world.borrow().opened.len(), world.borrow().closed.last().cloned()), (2, Some(terminal)));
    assert_eq!(records.conversations.borrow()[ROOT].terminal.as_deref(), replaced.detail["terminal"].as_str());
    let third = replaced.detail["terminal"].as_str().unwrap().to_string();
    linear.threads.borrow_mut().get_mut(ROOT).unwrap().place.project = Some("Elsewhere".into());
    let unmapped = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &thread_turn("c-warm-3", &request, Some(&through))).unwrap();
    assert_eq!((unmapped.outcome, world.borrow().opened.len(), world.borrow().closed.len()), (Outcome::Failed, 2, 1));
    assert!(world.borrow().terminals.iter().any(|t| t.handle.as_deref() == Some(third.as_str())));
    assert!(linear.replies.borrow().is_empty());
}

// A restart of `listen` mid-turn neither loses the turn nor repeats its answer. Nothing this
// run remembers names the turn, as after a restart: the stop finds its terminal by the
// thread's record, and closes it. Its answer and its `stalled` share the request's key, so
// whichever is posted first is the only one, however often either is sent.
#[test]
fn a_restart_mid_turn_neither_loses_nor_repeats_the_answer() {
    let config = fake::config(&briefs("thread-restart"));
    let (linear, records, _) = fakes();
    let world = conversing_world();
    let through = note_thread(&linear, "Tolkster");
    conversations(&world, &records);
    let opened = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &thread_turn("c-thread", &through, None)).unwrap();
    let terminal = opened.detail["terminal"].as_str().unwrap().to_string();
    // The turn answers (`tolkster comment --reply-to`, post::reply_once), and its end's
    // `stalled`, sent twice, says nothing more.
    assert!(crate::ports::Linear::reply_once(&linear, &Id::from("i1"), &Id::from(ROOT), "The answer.", &answer_key(ROOT, &through)).unwrap());
    for id in ["c-stalled", "c-stalled-again"] {
        assert_eq!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &thread_stalled(id, &through)).unwrap().outcome, Outcome::Ok);
    }
    assert_eq!(linear.replies.borrow().iter().map(|(_, _, body)| body.as_str()).collect::<Vec<_>>(), ["The answer."]);
    // The machine's stop closes the turn by its record, though this run never opened it, and
    // says nothing: the machine says "Stopped." once the stop holds.
    assert_eq!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &thread_stop("c-stop")).unwrap().outcome, Outcome::Ok);
    assert_eq!(world.borrow().closed, [terminal]);
    assert_eq!(linear.replies.borrow().len(), 1);
    // A turn that answered nothing: its `stalled` is said once, and the turn's own late answer
    // is refused.
    let (linear, records, _) = fakes();
    let through = note_thread(&linear, "Tolkster");
    for id in ["c-stalled", "c-stalled-again"] {
        assert_eq!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &thread_stalled(id, &through)).unwrap().outcome, Outcome::Ok);
    }
    assert!(!crate::ports::Linear::reply_once(&linear, &Id::from("i1"), &Id::from(ROOT), "The answer.", &answer_key(ROOT, &through)).unwrap());
    assert_eq!(linear.replies.borrow().iter().map(|(_, _, body)| body.as_str()).collect::<Vec<_>>(), [voice::STALLED]);
    // A thread with no turn recorded: the stop closes nothing.
    assert_eq!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &thread_stop("c-stop-none")).unwrap().outcome, Outcome::Ok);
    assert_eq!(world.borrow().closed.len(), 1);
}
