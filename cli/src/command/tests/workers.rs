use super::*;

// A post's activity ID is made from its command's ID alone, so a command run again posts the
// same ID, which Linear refuses, and no two commands post one.
#[test]
fn a_posts_activity_id_is_made_from_its_commands() {
    let (first, again, other) = (command("c-1", "TW-1"), command("c-1", "TW-1"), command("c-2", "TW-1"));
    let id = |command: &Command| crate::state::derived_uuid(&post_key(command));
    assert_eq!(id(&first), id(&again));
    assert_ne!(id(&first), id(&other));
    // Not the command's issue, version or kind, which another command shares.
    let later = Command { version: 2, ..command("c-1", "TW-2") };
    assert_eq!(id(&first), id(&later));
}

// `listen` decides nothing: a session, comment or worktree a command names is acted on only
// when it is on the command's own issue, whatever else it is told.
#[test]
fn a_command_acts_only_on_its_own_issues_things() {
    let command = command("c-1", "TW-1");
    assert!(on_issue(&command, Some("i1")).is_ok());
    assert!(on_issue(&command, Some("i2")).is_err());
    assert!(on_issue(&command, None).is_err());
    let worktree = |issue: Option<&str>| fake::row(issue, "/wt/a");
    let listing = |row: Option<Value>| move |program: &str, args: &[&str], _: Option<&str>| {
        assert!(program == "orca" && matches!(args[..2], ["worktree", "ps"] | ["terminal", "list"]));
        let rows: Vec<Value> = row.clone().into_iter().collect();
        Ok(crate::exec::Output::ok(json!({ "result": { "worktrees": rows, "terminals": [] } }).to_string()))
    };
    assert!(worktree_on_issue(&mut crate::orca::Cli::alone(&mut listing(Some(worktree(Some("tw-1"))))), &command, "repo::/wt/a").unwrap().is_some());
    assert!(worktree_on_issue(&mut crate::orca::Cli::alone(&mut listing(Some(worktree(Some("TW-2"))))), &command, "repo::/wt/a").is_err());
    assert!(worktree_on_issue(&mut crate::orca::Cli::alone(&mut listing(Some(worktree(None)))), &command, "repo::/wt/a").is_err());
    assert!(worktree_on_issue(&mut crate::orca::Cli::alone(&mut listing(None)), &command, "repo::/wt/a").unwrap().is_none());
}

// The stop overtakes the start's result: a stop runs in the issue's urgent lane, beside the
// start's (scheduler.rs), so it can run while the start is in flight, its agent just
// launched. It closes the agent of the worktree Orca links to the issue, here one Orca
// already shows as an agent. A stop sent again closes nothing more and says "Stopped." once.
#[test]
fn a_stop_mid_start_closes_the_agent_the_start_launched() {
    let config = fake::config(&briefs("stop-mid-start"));
    let (linear, records, world) = fakes();
    let stopped = RefCell::new(None);
    let mut lane = fake::Orca::on(&world);
    lane.after_launch = Some(Box::new(|| *stopped.borrow_mut() = Some(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &stop("c-stop", None)))));
    let started = carried(&config, &linear, &records, &mut lane, &start()).unwrap();
    drop(lane);
    assert_eq!(stopped.into_inner().unwrap().unwrap().outcome, Outcome::Ok);
    assert_eq!((started.outcome, &started.detail["madeWorktree"], &started.detail["launched"], &started.detail["worktree"]["id"]), (Outcome::Ok, &json!(true), &json!(true), &json!(WORKTREE)));
    assert_eq!((world.borrow().launched.len(), world.borrow().closed.clone()), (1, vec!["term_1".to_string()]));
    assert!(world.borrow().agents_in(WORKTREE).is_empty());
    assert_eq!(records.workers.borrow()[WORKTREE].session_id, "s1");
    assert_eq!(*linear.started.borrow(), ["i1"]);
    assert_eq!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &stop("c-stop", None)).unwrap().outcome, Outcome::Ok);
    let said: Vec<String> = linear.posted("response").into_iter().map(|p| p.body).collect();
    assert_eq!((said, world.borrow().closed.len()), (vec![voice::STOPPED.to_string()], 1));
}

// A stop that runs before the start launched its agent closes nothing, and the start goes on
// to launch it: the machine sends the stop again once the start's result names the worktree
// (docs/state-machine.md, `commandResult` with the stop unconfirmed), which closes it.
#[test]
fn a_stop_before_the_launch_is_sent_again_and_closes_the_agent() {
    let config = fake::config(&briefs("stop-before-launch"));
    let (linear, records, world) = fakes();
    let mut lane = fake::Orca::on(&world);
    lane.after_create = Some(Box::new(|| assert_eq!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &stop("c-stop", None)).unwrap().outcome, Outcome::Ok)));
    let started = carried(&config, &linear, &records, &mut lane, &start()).unwrap();
    drop(lane);
    assert_eq!((&started.detail["launched"], world.borrow().closed.len(), world.borrow().agents_in(WORKTREE).len()), (&json!(true), 0, 1));
    assert_eq!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &stop("c-stop-2", Some(WORKTREE))).unwrap().outcome, Outcome::Ok);
    assert!(world.borrow().agents_in(WORKTREE).is_empty());
    assert_eq!(linear.posted("response").len(), 2);
}

// A start's result names the branch Orca made, which the worker pushes as it is (setup.rs):
// the PR's events map to the issue by it.
#[test]
fn a_starts_result_names_the_branch_orca_made() {
    let config = fake::config(&briefs("start-branch"));
    let (linear, records, world) = fakes();
    let started = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    let made = world.borrow().worktrees.iter().find(|w| w.id == WORKTREE).and_then(|w| w.branch.clone()).unwrap();
    assert_eq!(started.detail["worktree"]["branch"], json!(made));
}

// A start whose worktree is there already was run before, and is answered with it: no
// second worktree or agent. A start's pickup is its ephemeral wording alone, with no action.
#[test]
fn a_start_run_again_finds_its_worker() {
    let config = fake::config(&briefs("start-again"));
    let (linear, records, world) = fakes();
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    let again = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    assert_eq!((&again.detail["madeWorktree"], &again.detail["launched"]), (&json!(false), &json!(true)));
    assert_eq!((world.borrow().launched.len(), linear.posted("action").len()), (1, 0));
}

// A card follows its issue: a start leaves it to Orca, which puts it in progress, and a review
// round's worker, typed to or brought back, has it moved there again. A board that fails
// never fails the round.
#[test]
fn a_rounds_worker_has_its_card_moved_to_in_progress() {
    let config = fake::config(&briefs("card-status"));
    let round = order("c-round", Order::StartRound { repo: "acme/tolkster".into(), number: 7, head: None, round: Some(1), session_id: Some("s1".into()), rebase_only: false });
    let exited = |world: &Rc<RefCell<World>>| {
        let mut w = world.borrow_mut();
        w.terminals.clear();
        w.worktrees.iter_mut().for_each(|w| w.agents.clear());
    };
    let (linear, records, world) = fakes();
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    assert!(world.borrow().statuses.is_empty());
    assert_eq!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &round).unwrap().outcome, Outcome::Ok);
    exited(&world);
    assert_eq!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &round).unwrap().outcome, Outcome::Ok);
    assert_eq!(world.borrow().statuses, vec![(WORKTREE.to_string(), crate::orca::Status::InProgress); 2]);
    // A board that fails leaves the round done.
    let (linear, records, world) = fakes();
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    world.borrow_mut().status_fails = true;
    exited(&world);
    let done = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &round).unwrap();
    assert_eq!((done.outcome, &done.detail["launched"], world.borrow().launched.len()), (Outcome::Ok, &json!(true), 2));
    assert!(world.borrow().statuses.is_empty() && linear.posted("elicitation").is_empty());
}

// A start whose agent never began, and was closed for it, leaves Tolkster's worktree with no
// agent: the next start, naming it, launches one there on the brief, making no worktree. One
// the machine didn't name, or with no record of Tolkster's, made by hand, is answered with as
// it is, nothing launched.
#[test]
fn a_start_that_never_began_is_launched_again_on_its_brief() {
    let config = fake::config(&briefs("start-never-began"));
    let (linear, records, world) = fakes();
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    fake::Orca::on(&world).close_all(WORKTREE).unwrap();
    let next = order("c-start-2", Order::StartWorker { session_id: "s1".into(), orchestrate: false, worktree_id: Some(WORKTREE.into()), host: None });
    let again = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &next).unwrap();
    assert_eq!((again.outcome, &again.detail["madeWorktree"], &again.detail["launched"], &again.detail["worktree"]["id"]), (Outcome::Ok, &json!(false), &json!(true), &json!(WORKTREE)));
    {
        let w = world.borrow();
        assert_eq!((w.created.len(), w.removed.len(), w.launched.len(), w.agents_in(WORKTREE).len()), (1, 0, 2, 1));
        assert_eq!((w.launched[1].0.as_str(), &w.launched[1].1), (WORKTREE, &w.launched[0].1));
    }
    assert_eq!(records.workers.borrow()[WORKTREE].session_id.as_str(), "s1");
    fake::Orca::on(&world).close_all(WORKTREE).unwrap();
    let unnamed = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &order("c-start-unnamed", Order::StartWorker { session_id: "s1".into(), orchestrate: false, worktree_id: None, host: None })).unwrap();
    assert_eq!((&unnamed.detail["launched"], world.borrow().launched.len()), (&json!(false), 2));
    records.workers.borrow_mut().clear();
    let by_hand = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &order("c-start-3", Order::StartWorker { session_id: "s1".into(), orchestrate: false, worktree_id: Some(WORKTREE.into()), host: None })).unwrap();
    assert_eq!((&by_hand.detail["madeWorktree"], &by_hand.detail["launched"], world.borrow().launched.len()), (&json!(false), &json!(false), 2));
}

// A worker's setup (`tolkster worker`) runs in a terminal Orca lists as no agent until
// `claude` starts. A start that never began closes it before launching again, and a stop
// closes it all the same, so no setup starts `claude` beside another or after a stop.
#[test]
fn a_setup_not_yet_an_agent_is_closed_by_a_relaunch_and_by_a_stop() {
    let config = fake::config(&briefs("setup-window"));
    let (linear, records, world) = fakes();
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    let setting_up = |world: &Rc<RefCell<World>>| {
        let mut w = world.borrow_mut();
        w.terminals.iter_mut().filter(|t| t.worktree_id.as_deref() == Some(WORKTREE)).for_each(|t| t.agent = false);
        w.worktrees.iter_mut().for_each(|w| w.agents.clear());
    };
    let in_worktree = |world: &Rc<RefCell<World>>| world.borrow().terminals.iter().filter(|t| t.worktree_id.as_deref() == Some(WORKTREE)).count();
    setting_up(&world);
    let next = order("c-start-2", Order::StartWorker { session_id: "s1".into(), orchestrate: false, worktree_id: Some(WORKTREE.into()), host: None });
    assert_eq!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &next).unwrap().detail["launched"], json!(true));
    assert_eq!((world.borrow().closed.clone(), world.borrow().launched.len(), in_worktree(&world)), (vec!["term_1".to_string()], 2, 1));
    setting_up(&world);
    assert_eq!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &stop("c-stop", Some(WORKTREE))).unwrap().outcome, Outcome::Ok);
    assert_eq!((world.borrow().closed.clone(), in_worktree(&world)), (vec!["term_1".to_string(), "term_2".to_string()], 0));
}

// An exited worker is brought back only in a worktree Tolkster's record holds, as its host's
// setup runs nowhere else: a resume or a round in one without fails at once, launching nothing.
#[test]
fn a_worktree_with_no_workers_record_is_never_relaunched() {
    let config = fake::config(&briefs("no-record"));
    let (linear, records, world) = fakes();
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    fake::Orca::on(&world).close_all(WORKTREE).unwrap();
    world.borrow_mut().worktrees.iter_mut().for_each(|w| w.agents.clear());
    records.workers.borrow_mut().clear();
    let resume = order("c-resume", Order::ResumeWorker { session_id: "s1".into(), why: Resume::Message });
    let round = order("c-round", Order::StartRound { repo: "acme/tolkster".into(), number: 7, head: None, round: Some(1), session_id: Some("s1".into()), rebase_only: false });
    for command in [resume, round] {
        assert_eq!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &command).unwrap().outcome, Outcome::Failed, "{:?}", command.kind);
    }
    assert_eq!(world.borrow().launched.len(), 1);
}

// A line is typed only to an agent at work, and counts once Orca took it, with no turn
// awaited: one waiting at a prompt is `held`, nothing typed, and a send that fails is an error.
#[test]
fn a_message_is_typed_only_to_a_working_agent() {
    let config = fake::config(&briefs("deliver"));
    let (linear, records, world) = fakes();
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    let deliver = order("c-deliver", Order::DeliverMessage { worktree_id: WORKTREE.into(), session_id: Some("s1".into()), comment: false });
    let deliver_now = || carried(&config, &linear, &records, &mut fake::Orca::on(&world), &deliver);
    world.borrow_mut().set_agents(WORKTREE, "waiting");
    assert_eq!(deliver_now().unwrap().outcome, Outcome::Held);
    assert!(world.borrow().typed.is_empty());
    world.borrow_mut().set_agents(WORKTREE, "working");
    world.borrow_mut().send = fake::Send::Unproven;
    let taken = deliver_now().unwrap();
    assert_eq!((taken.outcome, taken.detail.get("typed")), (Outcome::Ok, None));
    assert_eq!(world.borrow().typed.len(), 1);
    world.borrow_mut().send = fake::Send::Fails;
    assert!(deliver_now().is_err());
    world.borrow_mut().send = fake::Send::Begins;
    assert_eq!(deliver_now().unwrap().outcome, Outcome::Ok);
    assert_eq!(world.borrow().typed.len(), 2);
}

// A round whose agent exits between the listing and the send, which Orca refuses with proof
// nothing was typed, brings the worker back on its record, saying nothing failed. A send that
// fails without that proof fails as it does, launching nothing: the line may have been typed.
#[test]
fn a_round_whose_agent_exited_during_the_send_relaunches_it() {
    let config = fake::config(&briefs("exits-during-send"));
    let round = order("c-round", Order::StartRound { repo: "acme/tolkster".into(), number: 7, head: None, round: Some(1), session_id: Some("s1".into()), rebase_only: false });
    let (linear, records, world) = fakes();
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    world.borrow_mut().send = fake::Send::Fails;
    let failed = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &round).unwrap();
    assert_eq!((failed.outcome, &failed.detail["said"]), (Outcome::Failed, &json!(true)));
    assert_eq!((world.borrow().launched.len(), world.borrow().typed.len()), (1, 0));
    let asked = linear.posted("elicitation").len();
    world.borrow_mut().send = fake::Send::Exits;
    let launched = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &round).unwrap();
    assert_eq!((launched.outcome, &launched.detail["launched"]), (Outcome::Ok, &json!(true)));
    assert_eq!((world.borrow().launched.len(), world.borrow().typed.len(), linear.posted("elicitation").len()), (2, 0, asked));
    // An agent still listed afterwards is never launched beside.
    let (linear, records, world) = fakes();
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    world.borrow_mut().send = fake::Send::Refuses;
    let failed = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &round).unwrap();
    assert_eq!((failed.outcome, &failed.detail["said"], world.borrow().launched.len()), (Outcome::Failed, &json!(true), 1));
    // With no record of the worktree, nothing is launched and the refusal is told as before.
    let (linear, records, world) = fakes();
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    records.workers.borrow_mut().clear();
    world.borrow_mut().send = fake::Send::Exits;
    let failed = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &round).unwrap();
    assert_eq!((failed.outcome, &failed.detail["said"], world.borrow().launched.len()), (Outcome::Failed, &json!(true), 1));
}

// A message to a worktree with no agent, or whose agent exits as the send reaches it, ends
// `gone`, not `failed`: nothing was typed, and the machine observes the exit.
#[test]
fn a_message_to_a_worktree_with_no_agent_ends_gone() {
    let config = fake::config(&briefs("message-gone"));
    let deliver = order("c-deliver", Order::DeliverMessage { worktree_id: WORKTREE.into(), session_id: None, comment: true });
    let (linear, records, world) = fakes();
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    world.borrow_mut().send = fake::Send::Exits;
    let gone = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &deliver).unwrap();
    assert_eq!((gone.outcome, world.borrow().typed.len()), (Outcome::Gone, 0));
    let again = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &deliver).unwrap();
    assert_eq!(again.outcome, Outcome::Gone);
}

// A command that names its worktree reads that worktree's host alone, just before it types or
// closes, and again after a write of its own; a start and a round find the issue's worktree in
// one listing of every host, a start then its own on the host it placed it. A paired host no
// worktree here is on is never read alone.
#[test]
fn a_command_reads_each_orca_listing_once() {
    use fake::{AT, PS, REPOS};
    let config = fake::config(&briefs("spawns"));
    let (linear, records, world) = fakes();
    world.borrow_mut().hosts = vec!["mac".into()];
    let now = |command: &Command| carried(&config, &linear, &records, &mut fake::Orca::on(&world), command);
    let started = spawned(&world, || {
        now(&start()).unwrap();
    });
    assert_eq!(started, [PS, REPOS, AT]);
    let deliver = order("c-deliver", Order::DeliverMessage { worktree_id: WORKTREE.into(), session_id: Some("s1".into()), comment: false });
    assert_eq!(spawned(&world, || assert_eq!(now(&deliver).unwrap().outcome, Outcome::Ok)), [AT]);
    world.borrow_mut().set_agents(WORKTREE, "waiting");
    assert_eq!(spawned(&world, || assert_eq!(now(&deliver).unwrap().outcome, Outcome::Held)), [AT]);
    let round = order("c-round", Order::StartRound { repo: "acme/tolkster".into(), number: 7, head: None, round: Some(1), session_id: Some("s1".into()), rebase_only: false });
    assert_eq!(spawned(&world, || assert_eq!(now(&round).unwrap().outcome, Outcome::Held)), [PS, AT]);
    world.borrow_mut().set_agents(WORKTREE, "working");
    assert_eq!(spawned(&world, || assert_eq!(now(&round).unwrap().outcome, Outcome::Ok)), [PS, AT]);
    // A line Orca typed nothing of, as the agent exited, is followed by its host read again
    // before the worker is brought back, which lists the repos once.
    world.borrow_mut().send = fake::Send::Exits;
    assert_eq!(spawned(&world, || assert_eq!(now(&round).unwrap().detail["launched"], json!(true))), [PS, AT, AT, REPOS]);
    // One working is left; one held at a prompt is closed, as an idle one is.
    let close = order("c-close", Order::CloseAgent { worktree_id: WORKTREE.into() });
    world.borrow_mut().set_agents(WORKTREE, "working");
    assert_eq!(spawned(&world, || assert_eq!(now(&close).unwrap().outcome, Outcome::Ok)), [AT]);
    assert_eq!(world.borrow().closed.len(), 0);
    world.borrow_mut().set_agents(WORKTREE, "waiting");
    assert_eq!(spawned(&world, || assert_eq!(now(&close).unwrap().outcome, Outcome::Ok)), [AT]);
    assert_eq!(world.borrow().closed.len(), 1);
    assert!(world.borrow().listed_at.iter().all(Option::is_none));
}

// A spent budget fails the command as worth sending again, before anything is done.
#[test]
fn a_spent_budget_leaves_the_command_to_be_sent_again() {
    let config = fake::config(&briefs("spent"));
    let (linear, records, world) = fakes();
    linear.spent.set(1);
    let error = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap_err();
    assert_eq!(Finished::of(&error).outcome, Outcome::Retryable);
    assert!(world.borrow().launched.is_empty() && records.workers.borrow().is_empty());
}

// A branch is updated only on the PR of the branch Orca links to the issue, open, behind and
// at the head the machine read: another branch's fails, a conflict is `refused`, and a moved
// head or one not behind writes nothing. A write GitHub turns down is `refused` at the same
// head, and `moved` when a push landed meanwhile.
#[test]
fn a_branch_update_writes_only_the_issues_pr_at_the_head_read() {
    let config = fake::config(&briefs("update-branch"));
    let (linear, records, world) = fakes();
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    let branch = world.borrow().worktrees.iter().find(|w| w.id == WORKTREE).and_then(|w| w.branch.clone()).unwrap();
    let (head, pushed) = ("2878d3d000000000000000000000000000000000", "f".repeat(40));
    let update = order("c-update", Order::UpdateBranch { repo: "o/tolkster".into(), number: 7, head: head.into() });
    let pr = |name: &str, oid: &str, merge: &str| json!({ "repository": { "pullRequest": { "id": "PR_1", "state": "OPEN", "headRefName": name, "headRefOid": oid, "mergeStateStatus": merge, "headRepository": { "nameWithOwner": "o/tolkster" } } } });
    // `reads` answers each read in turn, the last one again; `write` answers the update.
    let run = |reads: Vec<Value>, write: Result<Value, Error>| {
        let (mut reads, mut write) = (reads.into_iter(), Some(write));
        let mut last = Value::Null;
        let mut gh = FakeReader::new(move |kind, _| {
            if kind == "update_branch" {
                return write.take().unwrap();
            }
            if let Some(next) = reads.next() {
                last = next;
            }
            Ok(last.clone())
        });
        let done = carried_with(&config, &linear, &records, &mut fake::Orca::on(&world), &mut gh, &update).map(|f| (f.outcome, Value::Object(f.detail)));
        (done.ok(), gh.calls.into_iter().filter(|c| c.starts_with("update_branch")).count())
    };
    let wrote = || Ok(json!({ "data": { "updatePullRequestBranch": { "pullRequest": { "headRefOid": "f".repeat(40) } } } }));
    assert_eq!(run(vec![pr(&branch, head, "BEHIND")], wrote()), (Some((Outcome::Ok, json!({}))), 1));
    assert_eq!(run(vec![pr(&branch, &pushed, "BEHIND")], wrote()), (Some((Outcome::Ok, json!({ "moved": true }))), 0));
    assert_eq!(run(vec![pr(&branch, head, "CLEAN")], wrote()), (Some((Outcome::Ok, json!({}))), 0));
    assert_eq!(run(vec![pr(&branch, head, "DIRTY")], wrote()), (Some((Outcome::Failed, json!({ "refused": true }))), 0));
    assert_eq!(run(vec![pr("another-branch", head, "BEHIND")], wrote()), (None, 0));
    let turned_down = || Ok(json!({ "data": { "updatePullRequestBranch": null }, "errors": [{ "message": "merge conflict" }] }));
    assert_eq!(run(vec![pr(&branch, head, "BEHIND")], turned_down()), (Some((Outcome::Failed, json!({ "refused": true }))), 1));
    assert_eq!(run(vec![pr(&branch, head, "BEHIND"), pr(&branch, &pushed, "CLEAN")], turned_down()), (Some((Outcome::Ok, json!({ "moved": true }))), 1));
    // A call that failed on the way, its token's mint included, is no refusal: it is read again later.
    assert_eq!(run(vec![pr(&branch, head, "BEHIND")], Err(Error::Failed("the token wasn't minted".into()))), (None, 1));
}

// A PR is merged only on the branch Orca links to the issue, at the head the machine read:
// another branch's fails and a moved head writes nothing. One merged already, before the write
// or as GitHub turned it down, as by its own auto-merge, is done; a write turned down at the
// same head is `refused`, as is one the owner held since the read, and nothing is said.
#[test]
fn a_merge_writes_only_the_issues_pr_at_the_head_read_and_counts_one_merged_already() {
    let config = fake::config(&briefs("merge-pr"));
    let (linear, records, world) = fakes();
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    let branch = world.borrow().worktrees.iter().find(|w| w.id == WORKTREE).and_then(|w| w.branch.clone()).unwrap();
    let (head, pushed) = ("2878d3d000000000000000000000000000000000", "f".repeat(40));
    let merge = order("c-merge", Order::MergePr { repo: "o/tolkster".into(), number: 7, head: head.into() });
    let pr = |name: &str, oid: &str, state: &str| json!({ "repository": { "pullRequest": { "id": "PR_1", "state": state, "headRefName": name, "headRefOid": oid, "headRepository": { "nameWithOwner": "o/tolkster" }, "autoMergeRequest": null, "timelineItems": { "nodes": [] } } } });
    // `reads` answers each read in turn, the last one again; `write` answers the merge.
    let run = |reads: Vec<Value>, write: Result<Value, Error>| {
        let (mut reads, mut write) = (reads.into_iter(), Some(write));
        let mut last = Value::Null;
        let mut gh = FakeReader::new(move |kind, args| {
            if kind == "merge_pr" {
                assert_eq!(args["head"], json!(head), "pinned to the head read");
                return write.take().unwrap();
            }
            if let Some(next) = reads.next() {
                last = next;
            }
            Ok(last.clone())
        });
        let done = carried_with(&config, &linear, &records, &mut fake::Orca::on(&world), &mut gh, &merge).map(|f| (f.outcome, Value::Object(f.detail)));
        (done.ok(), gh.calls.into_iter().filter(|c| c.starts_with("merge_pr")).count())
    };
    let merged = || Ok(json!({ "data": { "mergePullRequest": { "pullRequest": { "state": "MERGED" } } } }));
    let (done, refused) = (Some((Outcome::Ok, json!({}))), Some((Outcome::Failed, json!({ "refused": true }))));
    assert_eq!(run(vec![pr(&branch, head, "OPEN")], merged()), (done.clone(), 1));
    assert_eq!(run(vec![pr(&branch, head, "MERGED")], merged()), (done.clone(), 0));
    assert_eq!(run(vec![pr(&branch, &pushed, "OPEN")], merged()), (Some((Outcome::Ok, json!({ "moved": true }))), 0));
    assert_eq!(run(vec![pr(&branch, head, "CLOSED")], merged()), (None, 0));
    assert_eq!(run(vec![pr("another-branch", head, "OPEN")], merged()), (None, 0));
    let turned_down = || Ok(json!({ "data": { "mergePullRequest": null }, "errors": [{ "message": "Pull request is not mergeable" }] }));
    assert_eq!(run(vec![pr(&branch, head, "OPEN")], turned_down()), (refused.clone(), 1));
    assert_eq!(run(vec![pr(&branch, head, "OPEN"), pr(&branch, head, "MERGED")], turned_down()), (done, 1));
    // The owner's hold since the read: auto-merge turned off after it was on, or `manual-merge`.
    let mut off = pr(&branch, head, "OPEN");
    off["repository"]["pullRequest"]["timelineItems"]["nodes"] = json!([{ "__typename": "AutoMergeDisabledEvent" }]);
    assert_eq!(run(vec![off], merged()), (refused.clone(), 0));
    linear.manual_merge.borrow_mut().insert("i1".into());
    assert_eq!(run(vec![pr(&branch, head, "OPEN")], merged()), (refused, 0));
    assert!(linear.posted("response").is_empty());
}

// An agent Orca lists with an empty handle runs, though nothing can name it: `--terminal ""`
// is Orca's active terminal. A stop closes the agents it can name and fails rather than say
// "Stopped.", a resume finds it running and launches none beside it, and a round neither
// types to it nor launches one.
#[test]
fn an_agent_with_an_empty_handle_runs_and_is_never_typed_to_or_closed() {
    let config = fake::config(&briefs("empty-handle"));
    let (linear, records, world) = fakes();
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    world.borrow_mut().terminals.iter_mut().filter(|t| t.worktree_id.as_deref() == Some(WORKTREE)).for_each(|t| t.handle = Some(String::new()));
    let named = crate::orca::Terminal { handle: Some("term_9".into()), worktree_id: Some(WORKTREE.into()), agent: true, pane: None };
    world.borrow_mut().terminals.push(named);
    let now = |command: &Command| carried(&config, &linear, &records, &mut fake::Orca::on(&world), command);
    let stopped = now(&stop("c-stop", Some(WORKTREE))).unwrap();
    assert_eq!((stopped.outcome, &stopped.detail["unnamed"]), (Outcome::Failed, &json!(true)));
    assert_eq!(world.borrow().closed, ["term_9"]);
    assert!(linear.posted("response").is_empty());
    let resumed = now(&order("c-resume", Order::ResumeWorker { session_id: "s1".into(), why: Resume::Message })).unwrap();
    assert_eq!((resumed.outcome, &resumed.detail["launched"]), (Outcome::Ok, &json!(true)));
    let round = order("c-round", Order::StartRound { repo: "acme/tolkster".into(), number: 7, head: None, round: Some(1), session_id: Some("s1".into()), rebase_only: false });
    let refused = now(&round).unwrap();
    assert_eq!((refused.outcome, &refused.detail["unnamed"]), (Outcome::Failed, &json!(true)));
    world.borrow_mut().set_agents(WORKTREE, "waiting");
    assert_eq!(now(&round).unwrap().outcome, Outcome::Held);
    let world = world.borrow();
    assert!(world.typed.is_empty());
    assert_eq!((world.closed.len(), world.launched.len()), (1, 1));
}

// A worktree linked to another issue is never acted on, nor a session on another issue.
#[test]
fn a_stop_never_closes_another_issues_worker() {
    let config = fake::config(&briefs("another"));
    let (linear, records, world) = fakes();
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    world.borrow_mut().worktrees.iter_mut().filter(|w| w.id == WORKTREE).for_each(|w| w.linked_issue = Some("TW-2".into()));
    assert!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &stop("c-stop", Some(WORKTREE))).is_err());
    let elsewhere = Session { id: "s2".into(), issue: Some(Issue { id: "i2".into(), identifier: "TW-2".into(), ..issue() }), ..session(Vec::new()) };
    linear.sessions.borrow_mut().insert("s2".into(), elsewhere);
    let theirs = order("c-stop-2", Order::StopWorker { worktree_id: None, session_id: Some("s2".into()) });
    assert!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &theirs).is_err());
    assert!(world.borrow().closed.is_empty() && linear.posted("response").is_empty());
}

// An agent whose terminal fails to open fails the start: its worktree is removed, the owner
// asked once, naming Orca, and the result says no worktree is there, and that it was said.
#[test]
fn a_launch_that_fails_removes_its_worktree() {
    let config = fake::config(&briefs("launch-fails"));
    let (linear, records, world) = fakes();
    world.borrow_mut().launch_fails = true;
    let failed = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    assert_eq!((failed.outcome, &failed.detail["madeWorktree"], &failed.detail["said"]), (Outcome::Failed, &json!(false), &json!(true)));
    assert_eq!(world.borrow().removed, [WORKTREE]);
    // The worktree's record, written before the launch, goes with it: it would hold a slot.
    assert!(!records.workers.borrow().contains_key(WORKTREE));
    let asked: Vec<String> = linear.posted("elicitation").into_iter().map(|p| p.body).collect();
    assert_eq!(asked, ["Something broke on my side, so I couldn't get started on this. Reply here and I'll try again."]);
    assert!(linear.posted("error").is_empty() && linear.posted("response").is_empty());
    // Sent again, it asks no second time.
    world.borrow_mut().worktrees.retain(|w| w.id != WORKTREE);
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    assert_eq!(linear.posted("elicitation").len(), 1);
}

// A resume, a round and a turn whose terminal Orca fails to open each ask the owner once,
// naming Orca, as an elicitation offering to try again, and say so in their result.
#[test]
fn a_launch_that_fails_in_orca_asks_once_naming_it() {
    let config = fake::config(&briefs("relaunch-fails"));
    let (linear, records, world) = fakes();
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    {
        let mut w = world.borrow_mut();
        w.terminals.clear();
        w.worktrees.iter_mut().for_each(|w| w.agents.clear());
        w.launch_fails = true;
    }
    let resume = order("c-resume", Order::ResumeWorker { session_id: "s1".into(), why: Resume::Message });
    let round = order("c-round", Order::StartRound { repo: "acme/tolkster".into(), number: 7, head: None, round: Some(1), session_id: Some("s1".into()), rebase_only: false });
    for (command, said) in [(resume, voice::COULDNT_DELIVER), (round, voice::COULDNT_RESUME)] {
        let failed = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &command).unwrap();
        assert_eq!((failed.outcome, &failed.detail["said"]), (Outcome::Failed, &json!(true)), "{:?}", command.kind);
        assert_eq!(linear.posted("elicitation").last().map(|p| (p.body.clone(), p.options.clone())), Some((voice::couldnt(said, true), vec!["retry"])));
    }
    assert_eq!(linear.posted("elicitation").len(), 2);
    let (linear, records, _) = fakes();
    let world = conversing_world();
    world.borrow_mut().launch_fails = true;
    let failed = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &turn("c-turn", "s1", false, None)).unwrap();
    assert_eq!((failed.outcome, &failed.detail["said"]), (Outcome::Failed, &json!(true)));
    let asked: Vec<String> = linear.posted("elicitation").into_iter().map(|p| p.body).collect();
    assert_eq!(asked, [voice::couldnt(voice::COULDNT_START, true)]);
    assert!(linear.posted("error").is_empty());
}

// A parent's resume with its worktree gone, the sub-issues' end, added ones, or the owner's
// message on an issue that has sub-issues, starts a new orchestrator on the session in a new
// worktree, on the brief's prompt and then the resume's line, which its host builds. Any other worker's resume with
// no worktree fails, and says nothing.
#[test]
fn a_parents_resume_with_no_worktree_starts_a_new_orchestrator() {
    let config = fake::config(&briefs("parent-restart"));
    let parent = || Session { issue: Some(Issue { children: vec![member("TW-2", StateType::Completed, 2)], ..issue() }), ..session(Vec::new()) };
    let lose = |world: &Rc<RefCell<World>>| {
        let mut w = world.borrow_mut();
        w.terminals.clear();
        w.worktrees.clear();
    };
    for (why, then) in [(Resume::ChildrenEnd, "children-end"), (Resume::ChildrenAdded, "children-added"), (Resume::Message, "message")] {
        let (linear, records, world) = fakes();
        linear.sessions.borrow_mut().insert("s1".into(), parent());
        let orchestrate = order("c-start", Order::StartWorker { session_id: "s1".into(), orchestrate: true, worktree_id: None, host: None });
        carried(&config, &linear, &records, &mut fake::Orca::on(&world), &orchestrate).unwrap();
        lose(&world);
        let resume = order("c-resume", Order::ResumeWorker { session_id: "s1".into(), why });
        let resumed = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &resume).unwrap();
        assert_eq!((resumed.outcome, &resumed.detail["launched"], &resumed.detail["worktree"]["id"]), (Outcome::Ok, &json!(true), &json!(WORKTREE)), "{why:?}");
        let launched = world.borrow().launched.clone();
        assert_eq!(launched.len(), 2, "{why:?}");
        assert!(launched[1].1.starts_with("tolkster worker s1 --issue TW-1 ") && launched[1].1.contains(" --orchestrate") && launched[1].1.ends_with(&format!(" --then {then}")), "{why:?}: {}", launched[1].1);
        assert!(linear.posted("elicitation").is_empty(), "{why:?}");
    }
    let (linear, records, world) = fakes();
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    lose(&world);
    let resume = order("c-resume", Order::ResumeWorker { session_id: "s1".into(), why: Resume::Message });
    assert_eq!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &resume).unwrap().outcome, Outcome::Failed);
    assert_eq!(world.borrow().launched.len(), 1);
}

// A comment on another issue is refused, and a line is never typed into another issue's worker.
#[test]
fn a_comment_or_worktree_of_another_issue_is_refused() {
    let config = fake::config(&briefs("another-comment"));
    let (linear, records, world) = fakes();
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    let theirs = CommentFacts { issue_id: Some("i2".into()), kind: CommentKind::Plain, newest_owner_at: None, state: StateType::Started };
    linear.comments.borrow_mut().insert("c-9".into(), theirs);
    let read = order("c-read", Order::ReadComment { comment_id: "c-9".into() });
    assert!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &read).is_err());
    world.borrow_mut().worktrees.iter_mut().filter(|w| w.id == WORKTREE).for_each(|w| w.linked_issue = Some("TW-2".into()));
    let deliver = order("c-deliver", Order::DeliverMessage { worktree_id: WORKTREE.into(), session_id: None, comment: true });
    assert!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &deliver).is_err());
    assert!(world.borrow().typed.is_empty());
}
