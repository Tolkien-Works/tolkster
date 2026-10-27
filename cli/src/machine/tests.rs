use super::*;
use std::collections::BTreeMap;
use crate::command::pr::{Open, pr_read};
use crate::github::{Head, Pr, PrState};
use crate::review::Checked;
use crate::state::tests::contract;
use crate::hosts::{Fleet, Heard};
use crate::time::instant;

fn names<T: Serialize>(items: &[T]) -> Vec<String> {
    items.iter().map(|i| serde_json::to_value(i).unwrap().as_str().unwrap().to_string()).collect()
}

fn listed(value: &Value) -> Vec<String> {
    value.as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect()
}

// The fixture's `machine` is docs/state-machine.md's (test/state-machine.test.ts): these
// enums name the same commands, notices and results, and `postAway` is the alarm's.
#[test]
fn the_enums_are_the_machines() {
    let machine = &contract()["machine"];
    let mut commands = listed(&machine["commands"]);
    assert_eq!(commands.pop().as_deref(), Some("postAway"));
    assert_eq!(names(&KINDS), commands);
    assert_eq!(names(&NOTICES), listed(&machine["notices"]));
    assert_eq!(names(&[Outcome::Ok, Outcome::Failed, Outcome::Retryable, Outcome::Held, Outcome::Gone]), listed(&machine["results"]));
    for observed in names(&OBSERVED).iter().chain(&["turnEnded".to_string(), "commandResult".into(), "workerRead".into(), "workerAnswered".into(), "chainRecorded".into()]) {
        assert!(machine["events"].get(observed).is_some(), "{observed} is an event");
    }
}

// A spent budget, a wait or a server that failed is sent again; a refusal, which is an
// `Error::Failed`, and Orca's failure, which may have opened or typed into a terminal, never are.
#[test]
fn a_server_that_failed_is_sent_again_but_a_refusal_or_orca_is_not() {
    let of = |error: Error| Finished::of(&error).outcome;
    let spent = Error::RateLimited { message: "spent".into(), wait: std::time::Duration::from_secs(1) };
    for error in [spent, Error::Unavailable("502".into()), Error::Later("not yet".into())] {
        assert_eq!(of(error.clone()), Outcome::Retryable, "{error:?}");
    }
    for error in [Error::Failed("another issue's session".into()), Error::Orca("timed out".into()), Error::Usage("bad".into()), Error::Unauthorized("refused".into())] {
        assert_eq!(of(error.clone()), Outcome::Failed, "{error:?}");
    }
    // What a wait names goes with it, for the Durable Object to send it again on, in the shapes
    // the contract pins.
    let pinned = |name: &str| Value::Object(contract()["resultData"][name].as_object().unwrap().clone());
    assert_eq!(Value::Object(Finished::waking(&crate::error::Waits::Host("mini".into())).detail), pinned("resumeWorkerWaking"));
    assert_eq!(Value::Object(Finished::of(&Error::Waits(crate::error::Waits::Lock, "a make runs".into())).detail), pinned("destroyHostLocked"));
    let waits = |error: Error| Finished::of(&error).detail.get("waitsOn").cloned();
    assert_eq!(waits(Error::silent("mac")), Some(json!({ "host": "mac" })));
    assert_eq!(waits(Error::Waits(crate::error::Waits::Lock, "a make runs".into())), Some(json!({ "lock": true })));
    assert_eq!(Value::Object(Finished::of(&Error::Waits(crate::error::Waits::Orca, "orca: program not found".into())).detail), pinned("readIssueOrcaAbsent"));
    assert_eq!(waits(Error::Later("not yet".into())), None);
}

#[test]
fn the_contract_fixture_has_every_kind_of_command_and_each_reads() {
    let c = contract();
    let commands: Vec<Command> = c["commands"].as_array().unwrap().iter().map(|v| parse_command(v).unwrap().unwrap()).collect();
    let mut kinds: Vec<Kind> = commands.iter().map(|c| c.kind).collect();
    kinds.dedup();
    assert_eq!(kinds, KINDS);
    for command in &commands {
        assert!(command.order.is_ok(), "{}: {:?}", command.id, command.order);
    }
    let by_id = |id: &str| commands.iter().find(|c| c.id == id).unwrap();
    let worktree = "r1::D:/wt/tw-1".to_string();
    // The wait a queued request's command carries, which the Worker's `queued` sends.
    let queued = by_id("c-13").queued.as_ref().unwrap();
    assert_eq!((queued.waiting, queued.blockers.len()), (Waiting::Blockers, 2));
    assert_eq!(by_id("c-2").order, Ok(Order::StartWorker { session_id: "s-1".into(), orchestrate: true, worktree_id: None, host: None }));
    assert_eq!(by_id("c-20").order, Ok(Order::StartWorker { session_id: "s-1".into(), orchestrate: false, worktree_id: None, host: Some("mac".into()) }));
    assert_eq!(by_id("c-3").order, Ok(Order::DeliverMessage { worktree_id: worktree.clone(), session_id: Some("s-1".into()), comment: false }));
    assert_eq!(by_id("c-16").order, Ok(Order::DeliverMessage { worktree_id: worktree.clone(), session_id: None, comment: true }));
    assert_eq!(by_id("c-15").order, Ok(Order::ReadComment { comment_id: "cm-1".into() }));
    assert_eq!(by_id("c-4").order, Ok(Order::ResumeWorker { session_id: "s-1".into(), why: Resume::Message }));
    assert_eq!(by_id("c-5").order, Ok(Order::StartRound { repo: "Tolkien-Works/tolkster".into(), number: 7, head: Some("2878d3d000000000000000000000000000000000".into()), round: Some(2), session_id: Some("s-1".into()), rebase_only: false }));
    assert_eq!(by_id("c-18").order, Ok(Order::UpdateBranch { repo: "Tolkien-Works/tolkster".into(), number: 7, head: "2878d3d000000000000000000000000000000000".into() }));
    assert_eq!(by_id("c-19").order, Ok(Order::MergePr { repo: "Tolkien-Works/tolkster".into(), number: 7, head: "2878d3d000000000000000000000000000000000".into() }));
    assert_eq!(by_id("c-6").order, Ok(Order::StopWorker { worktree_id: Some(worktree.clone()), session_id: Some("s-1".into()) }));
    assert!(by_id("c-6").urgent && !by_id("c-2").urgent);
    assert_eq!(by_id("c-7").order, Ok(Order::StartTurn { session_id: "s-2".into(), thread: false, request: Some("2026-09-26T01:01:00.000Z".into()), after: Some("2026-09-26T01:00:00.000Z".into()) }));
    assert_eq!(by_id("c-10").order, Ok(Order::Observe { worktree_ids: vec![worktree], session_id: Some("s-2".into()), thread: false }));
    let blockers = vec!["1b2c3d4e-0000-4000-8000-000000000084".into(), "1b2c3d4e-0000-4000-8000-000000000085".into()];
    assert_eq!(by_id("c-13").order, Ok(Order::PostNotice { session_id: Some("s-1".into()), thread: false, notice: Notice::Blocked, blockers, pr: None, rounds: None, through: None }));
    assert_eq!(by_id("c-14").order, Ok(Order::TakeBackChain { children: vec!["0a1b-2d".into(), "0a1b-2e".into()] }));
    assert_eq!(by_id("c-17").order, Ok(Order::StartContext));
    assert_eq!(by_id("triage.1").order, Ok(Order::StartTriage { issue_ids: vec!["0a1b-2c".into()] }));
    assert_eq!(by_id("triage.2").order, Ok(Order::ObserveTriage { session_id: "s-3".into(), terminal: "term_2".into() }));
    assert_eq!((by_id("triage.1").issue_id.as_str(), by_id("triage.1").identifier.as_str()), (TRIAGE, TRIAGE));
}

#[test]
fn only_the_triage_and_nap_machines_commands_carry_their_own_issue() {
    let c = contract();
    let with = |i: usize, issue_id: &str, identifier: &str| {
        let mut command = c["commands"][i].clone();
        command["issueId"] = issue_id.into();
        command["identifier"] = identifier.into();
        parse_command(&command).unwrap()
    };
    let at = |kind: &str| c["commands"].as_array().unwrap().iter().position(|command| command["kind"] == kind).unwrap();
    let (start, suspend, read) = (at("startTriage"), at("suspendHost"), 0);
    assert!(with(start, "0a1b-2c", "TW-1").is_err());
    assert!(with(start, TRIAGE, "TW-1").is_err());
    assert!(with(start, "0a1b-2c", TRIAGE).is_err());
    assert!(with(read, TRIAGE, TRIAGE).is_err());
    assert!(with(start, TRIAGE, TRIAGE).is_ok());
    assert!(with(start, FLEET, FLEET).is_err());
    assert!(with(suspend, TRIAGE, TRIAGE).is_err());
    assert!(with(read, FLEET, FLEET).is_err());
    assert!(with(suspend, FLEET, FLEET).is_ok());
}

#[test]
fn the_contract_fixture_has_each_report_as_this_writes_it() {
    let c = contract();
    let reports = c["reports"].as_array().unwrap();
    let line = |text: String| serde_json::from_str::<Value>(&text).unwrap();
    let first = parse_command(&c["commands"][0]).unwrap().unwrap();
    assert_eq!(line(result_line(&first, &Finished::ok(json!({ "stateType": "started" })))), reports[0]);
    for (i, observed) in OBSERVED.into_iter().enumerate() {
        assert_eq!(line(observation_line(observed, "r1::D:/wt/tw-1", i as u64 + 1)), reports[i + 1]);
    }
    assert_eq!(line(turn_ended_line("s-2", 6)), reports[6]);
    let without_op = |mut v: Value| {
        assert_eq!(v.as_object_mut().unwrap().remove("op"), Some(json!("report")));
        v
    };
    assert_eq!(without_op(worker_read("s-1", "a-1", "2026-09-26T01:01:00.000Z", Reader::Worker).unwrap()), reports[7]);
    assert_eq!(without_op(worker_answered("s-1", "a-2", "2026-09-26T01:02:00.000Z", Reader::Turn).unwrap()), reports[8]);
    assert_eq!(without_op(chain_recorded("0a1b-2c", "s-1", "r-1", &["0a1b-2d".into(), "0a1b-2e".into()]).unwrap()), reports[9]);
    let mut config = crate::ports::fake::config("/briefs");
    config.slots = BTreeMap::from([("local".into(), 6), ("mac".into(), 2)]);
    config.repos = crate::hosts::tests::opted_in(2);
    config.machines.insert("tolkster-1".into(), crate::config::Machine { recipe: "cloud".into(), repo: "Tolkster".into(), made: true, sleeps: true, asleep: true, environment: Some("tolkster-1".into()), revision: Some("4".into()) });
    config.revisions.insert("Tolkster".into(), "5".into());
    assert_eq!(line(config_line(&config)), reports[10]);
    // A cap past what the relay takes goes as the most it takes, as `hosts::total` sums it.
    config.slots.insert("local".into(), MAX_SLOTS + 50);
    assert_eq!(line(config_line(&config))["cap"], MAX_SLOTS);
    assert_eq!(line(terminal_closed_line("term_2", 7)), reports[13]);
    assert_eq!(without_op(turn_idle("s-2", "term_2", "2026-09-26T01:03:00.000Z").unwrap()), reports[18]);
    assert_eq!(without_op(landed("0a1b-2c", "s-1", "2878d3d000000000000000000000000000000000").unwrap()), reports[19]);
    assert_eq!(without_op(merge_asked("0a1b-2c", "s-1", 7).unwrap()), reports[21]);
    assert_eq!(merge_asked("0a1b-2c", "s-1", 0), None);
    let start = parse_command(c["commands"].as_array().unwrap().iter().find(|c| c["kind"] == "startTriage").unwrap()).unwrap().unwrap();
    assert_eq!(line(result_line(&start, &Finished::ok(json!({ "session": { "id": "s-3", "terminal": "term_2", "issues": ["TW-1", "TW-2"] }, "waiting": ["0a1b2c3d-0000-4000-8000-000000000002"] })))), reports[14]);
    assert_eq!(without_op(agent_state(Observed::AgentIdle, "r1::D:/wt/tw-1", "2026-09-26T01:05:00.000Z", 2000).unwrap()), reports[12]);
    assert_eq!(without_op(agent_state(Observed::AgentBackground, "r1::D:/wt/tw-1", "2026-09-26T01:05:00.000Z", 2000).unwrap()), reports[20]);
    // Only Orca's listing says a worktree is gone, and a value of the wrong shape is never sent.
    assert_eq!(agent_state(Observed::WorktreeGone, "r1::D:/wt/tw-1", "2026-09-26T01:05:00.000Z", 0), None);
    assert_eq!(agent_state(Observed::AgentIdle, "no-separator", "2026-09-26T01:05:00.000Z", 0), None);
    assert_eq!(agent_state(Observed::AgentIdle, "r1::D:/wt/tw-1", "soon", 0), None);
    // Orca's listing whole; one cut off, or too long, names no worktree as listed.
    let wt = |n: u32| format!("r1::D:/wt/tw-{n}");
    let agents = [(wt(1), Observed::AgentWorking), (wt(2), Observed::AgentIdle), (wt(3), Observed::AgentWaiting), ("no-separator".into(), Observed::AgentWorking)];
    let ids: Vec<String> = (1..=4).map(wt).chain(["no-separator".to_string()]).collect();
    let heard = |id: &str, cap, admitted, answering, ready: &[&str], held| Heard { id: id.into(), name: id.into(), cap, admitted, answering, ready: ready.iter().map(|p| p.to_string()).collect(), held };
    let projects = BTreeMap::from([("r1".to_string(), "github:acme/app".to_string())]);
    let fleet = Fleet { held: 1, hosts: vec![heard("mac", 2, true, true, &["github:acme/app"], 1), heard("pi", 0, false, false, &[], 0)], projects, recipes: BTreeMap::from([("r1".to_string(), "tolkster".to_string())]) };
    let empty = Snapshot { began: std::time::Instant::now(), at: "2026-09-26T01:07:00.000Z".into(), agents: vec![], listed: None, hosts: vec![], paired: vec![], fleet: Fleet::default(), config: String::new(), idle_hosts: BTreeMap::new(), dormant: None };
    let full = Snapshot { at: "2026-09-26T01:06:00.000Z".into(), agents: agents.to_vec(), listed: Some(ids), hosts: vec!["mac".into()], paired: vec!["mac".into(), "pi".into()], fleet, idle_hosts: BTreeMap::from([("mac".into(), 30)]), dormant: Some(BTreeMap::from([("tolkster-1".into(), 1440)])), ..empty.clone() };
    assert_eq!(line(listing_line(&full, 3000)), reports[16]);
    assert_eq!(line(listing_line(&empty, 0)), reports[17]);
    let too_many: Vec<String> = (0..=MAX_LISTED as u32).map(wt).collect();
    assert_eq!(line(listing_line(&Snapshot { listed: Some(too_many), ..empty }, 0)), reports[17]);
    assert_eq!(c["machine"]["maxListed"].as_u64(), Some(MAX_LISTED as u64));
    assert_eq!(c["machine"]["maxSlots"].as_i64(), Some(MAX_SLOTS));
    // `evaluatePr`'s result, the machine's `PrRead`, from GitHub's times to the second.
    let evaluate = c["commands"].as_array().unwrap().iter().map(|v| parse_command(v).unwrap().unwrap()).find(|c| c.id == "c-9").unwrap();
    let read = &reports[11]["detail"]["pr"];
    let head = Head { number: 7, branch: "tw-1".into(), same_repo: true, state: PrState::Open, sha: read["head"].as_str().unwrap().into(), at: instant("2026-09-26T01:04:00Z") };
    let pr = Pr {
        number: 7,
        repo: "Tolkien-Works/tolkster".into(),
        head: head.sha.clone(),
        merge_state: "BEHIND".into(),
        base: "main".into(),
        base_head: read["base"].as_str().unwrap().into(),
        base_at: None,
        bot_author: None,
        checks_green: false,
        auto_merge_off: false,
        auto_merge_on: true,
        reviews: Vec::new(),
        threads: Vec::new(),
        comments: Vec::new(),
    };
    let open = Open { checked: Checked { pending: true, ..Checked::default() }, feedback: Some((instant("2026-09-26T01:03:00Z").unwrap(), instant("2026-09-26T01:02:00Z"))), copilot_wont_approve: false, manual_merge: false };
    let detail = json!({ "pr": pr_read(&head, &pr, Some(open), 5).unwrap() });
    assert_eq!(line(result_line(&evaluate, &Finished::ok(detail))), reports[11]);
}

#[test]
fn only_a_command_parses_as_one_and_its_args_must_be_plain() {
    assert!(parse_command(&json!({ "id": "start:s-1", "type": "start", "sessionId": "s-1", "at": "2026-09-26T01:00:00.000Z" })).is_none());
    assert!(parse_command(&json!("pong")).is_none());
    let command = |kind: &str, args: Value| json!({ "type": "command", "id": "c-1", "kind": kind, "issueId": "0a1b-2c", "identifier": "TW-1", "version": 1, "args": args });
    assert!(parse_command(&command("postAway", json!({}))).unwrap().is_err());
    let order = |kind: &str, args: Value| parse_command(&command(kind, args)).unwrap().unwrap().order;
    assert!(order("deliverMessage", json!({ "worktreeId": "no-separator", "sessionId": "s-1" })).is_err());
    assert!(order("startWorker", json!({ "sessionId": "s 1; rm" })).is_err());
    assert!(order("postNotice", json!({ "sessionId": "s-1", "notice": "blocked" })).is_err());
    assert!(order("postNotice", json!({ "sessionId": "s-1", "notice": "prClosed" })).is_err());
    assert!(order("postNotice", json!({ "sessionId": "s-1", "notice": "roundsCap", "pr": 7, "rounds": 3 })).is_ok());
    assert!(order("takeBackChain", json!({ "children": ["TW 1"] })).is_err());
    assert_eq!(order("stopWorker", json!({ "sessionId": "s-1" })), Ok(Order::StopWorker { worktree_id: None, session_id: Some("s-1".into()) }));
    assert_eq!(order("stopWorker", json!({ "sessionId": null })), Ok(Order::StopWorker { worktree_id: None, session_id: None }));
    // A session's message names its session; only the owner's comment may name none.
    assert!(order("deliverMessage", json!({ "worktreeId": "r1::D:/wt/tw-1" })).is_err());
    assert!(order("startRound", json!({ "repo": "o/r", "number": 7, "head": "not a sha" })).is_err());
    // A note's thread's request, keyed by its root comment, which stands for the session's ID.
    let at = "2026-10-02T01:00:00.000Z";
    assert_eq!(order("startTurn", json!({ "sessionId": "c-root", "thread": true, "request": at })), Ok(Order::StartTurn { session_id: "c-root".into(), thread: true, request: Some(at.into()), after: None }));
    assert_eq!(order("startTurn", json!({ "sessionId": "s-1" })), Ok(Order::StartTurn { session_id: "s-1".into(), thread: false, request: None, after: None }));
    assert_eq!(order("stopTurn", json!({ "sessionId": "s-1", "idle": true })), Ok(Order::StopTurn { session_id: "s-1".into(), thread: false, idle: true }));
    assert!(order("startTurn", json!({ "sessionId": "c-root", "thread": true, "request": "now" })).is_err());
    assert!(order("startTurn", json!({ "sessionId": "c-root", "thread": "c-root" })).is_err());
    assert_eq!(order("postNotice", json!({ "sessionId": "c-root", "thread": true, "notice": "noRepo" })), Ok(Order::PostNotice { session_id: Some("c-root".into()), thread: true, notice: Notice::NoRepo, blockers: vec![], pr: None, rounds: None, through: None }));
    let stalled = json!({ "sessionId": "c-root", "thread": true, "notice": "stalled", "through": "2026-10-02T01:00:00.000Z" });
    assert_eq!(order("postNotice", stalled), Ok(Order::PostNotice { session_id: Some("c-root".into()), thread: true, notice: Notice::Stalled, blockers: vec![], pr: None, rounds: None, through: Some("2026-10-02T01:00:00.000Z".into()) }));
    assert!(order("postNotice", json!({ "sessionId": "c-root", "notice": "stalled", "through": "soon" })).is_err());
    assert_eq!(order("observe", json!({ "sessionId": "c-root", "thread": true })), Ok(Order::Observe { worktree_ids: vec![], session_id: Some("c-root".into()), thread: true }));
    assert_eq!(order("stopTurn", json!({ "sessionId": "c-root", "thread": true })), Ok(Order::StopTurn { session_id: "c-root".into(), thread: true, idle: false }));
    // A read sent once more, after one failed, is the same read.
    assert_eq!(order("readComment", json!({ "commentId": "cm-1", "again": true })), Ok(Order::ReadComment { comment_id: "cm-1".into() }));
    let unplain = json!({ "type": "command", "id": "c 1", "kind": "readIssue", "issueId": "0a1b-2c", "identifier": "TW-1", "version": 1 });
    assert!(parse_command(&unplain).unwrap().is_err());
    assert!(worker_read("s-1", "a-1", "yesterday", Reader::Worker).is_none());
    assert!(turn_idle("s-1", "term_1", "yesterday").is_none() && turn_idle("s-1", "-x", "2026-09-26T01:03:00.000Z").is_none());
}

#[test]
fn what_a_queued_request_waits_for_reads_from_a_commands_args_and_never_fails_it() {
    let at = "2026-10-02T01:00:00.000Z";
    let queued = |args: Value| parse_command(&json!({ "type": "command", "id": "c-q", "kind": "readIssue", "issueId": "0a1b-2c", "identifier": "TW-1", "version": 1, "args": args })).unwrap().unwrap();
    assert_eq!(queued(json!({})).queued, None);
    assert_eq!(queued(json!({ "queued": { "waiting": "slot", "at": at } })).queued, Some(Queued { waiting: Waiting::Slot, blockers: vec![], at: at.into() }));
    let blocked = queued(json!({ "queued": { "waiting": "blockers", "at": at, "blockers": ["b-1", "b-2"] } }));
    assert_eq!(blocked.queued, Some(Queued { waiting: Waiting::Blockers, blockers: vec!["b-1".into(), "b-2".into()], at: at.into() }));
    assert!(blocked.order.is_ok());
    // One that doesn't read is left out, and the command goes on.
    for unread in [json!({ "waiting": "later", "at": at }), json!({ "waiting": "blockers", "at": at, "blockers": ["b 1"] }), json!({ "waiting": "slot", "at": "soon" })] {
        let command = queued(json!({ "queued": unread }));
        assert!(command.queued.is_none() && command.order.is_ok());
    }
}

#[test]
fn a_resume_says_why_and_an_older_worker_s_command_is_a_message() {
    let with = |args: Value| parse_command(&json!({ "type": "command", "id": "c-r", "kind": "resumeWorker", "issueId": "0a1b-2c", "identifier": "TW-1", "version": 6, "urgent": false, "args": args })).unwrap().unwrap().order;
    let resume = |why| Ok(Order::ResumeWorker { session_id: "s-1".into(), why });
    assert_eq!(with(json!({ "sessionId": "s-1", "childrenEnd": false, "childrenAdded": true })), resume(Resume::ChildrenAdded));
    assert_eq!(with(json!({ "sessionId": "s-1", "childrenEnd": true, "childrenAdded": false })), resume(Resume::ChildrenEnd));
    assert_eq!(with(json!({ "sessionId": "s-1", "childrenEnd": false })), resume(Resume::Message));
    assert!(with(json!({ "sessionId": "s-1", "childrenAdded": "yes" })).is_err());
}

fn base_tip(text: &str) -> Option<Result<BaseTip, String>> {
    parse_base(&serde_json::from_str(text).ok()?)
}

// The fixture's base tip (test/contract.json, which test/realtime.test.ts holds to the
// Worker's shape) parses into its job, and isn't a command.
#[test]
fn the_fixtures_base_tip_parses() {
    let contract = contract();
    assert!(parse_command(&contract["baseTip"]).is_none(), "a base tip isn't a command");
    assert_eq!(parse_base(&contract["baseTip"]), Some(Ok(BaseTip { repo: "Tolkien-Works/tolkster".into(), reference: "refs/heads/main".into(), after: "a1fe200000000000000000000000000000000000".into() })));
}

#[test]
fn a_base_tip_names_a_plain_repo_branch_and_commit() {
    let after = "a1fe200000000000000000000000000000000000";
    assert_eq!(base_tip(&format!(r#"{{"type":"base","repo":"o/r.js","ref":"refs/heads/main","after":"{after}"}}"#)), Some(Ok(BaseTip { repo: "o/r.js".into(), reference: "refs/heads/main".into(), after: after.into() })));
    for bad in [
        r#"{"type":"base","repo":"o","ref":"refs/heads/main","after":"a1fe200000000000000000000000000000000000"}"#,
        r#"{"type":"base","repo":"o/r; rm","ref":"refs/heads/main","after":"a1fe200000000000000000000000000000000000"}"#,
        r#"{"type":"base","repo":"o/r","ref":"main","after":"a1fe200000000000000000000000000000000000"}"#,
        r#"{"type":"base","repo":"o/r","ref":"refs/tags/v1","after":"a1fe200000000000000000000000000000000000"}"#,
        r#"{"type":"base","repo":"o/r","ref":"refs/heads/a b","after":"a1fe200000000000000000000000000000000000"}"#,
        r#"{"type":"base","repo":"o/r","ref":"refs/heads/-x","after":"a1fe200000000000000000000000000000000000"}"#,
        r#"{"type":"base","repo":"o/r","ref":"refs/heads/main","after":"HEAD"}"#,
        r#"{"type":"base","repo":"o/r","ref":"refs/heads/main","after":null}"#,
        r#"{"type":"base","repo":"o/r","ref":"refs/heads/main"}"#,
    ] {
        assert!(base_tip(bad).is_some_and(|tip| tip.is_err()), "{bad}");
    }
    // One with an `id`, as an older Worker queued a base action, and anything else, isn't a tip.
    for other in [r#"{"id":"base:o/r","type":"base","repo":"o/r","ref":"refs/heads/main","after":"a1fe200000000000000000000000000000000000","at":"x"}"#, r#"{"type":"command"}"#, "{}", "pong"] {
        assert!(base_tip(other).is_none(), "{other}");
    }
}
