use super::frame::Work;
use super::view::{Agent, Also, Budget, Link, Live, Need, Silence, SlotRow, view};
use super::*;

const MIN: Duration = Duration::from_secs(60);
const AT: &str = "2026-10-20T10:00:00.000Z";

fn list(state: &mut State, listing: Listing) {
    state.list(listing, &HashSet::new());
}

fn slot(worktree: &str, issue: &str, host: Option<&str>, state: Observed) -> Slot {
    Slot { worktree: worktree.into(), issue: Some(issue.into()), host: host.map(String::from), state }
}

fn listing(at: Instant, slots: Vec<Slot>) -> Listing {
    let hosts = vec![HostShare { id: None, name: "local".into(), cap: 2, answering: true, refused: false }, HostShare { id: Some("h-wsl".into()), name: "wsl".into(), cap: 2, answering: true, refused: false }];
    Listing { at, took: Duration::from_millis(800), slots, hosts, cap: 4 }
}

fn running(issue: Option<&str>, label: &str, lane: Lane, since: Instant) -> Running {
    Running { issue: issue.map(String::from), label: label.into(), lane, since }
}

#[test]
fn a_slot_row_is_each_worker_listed_with_its_host_its_state_since_it_began_and_its_lanes_job() {
    let t0 = Instant::now();
    let mut state = State::default();
    list(&mut state, listing(t0, vec![slot("w-2", "TW-2", None, Observed::AgentWorking), slot("w-1", "TW-1", Some("h-wsl"), Observed::AgentWorking)]));
    list(&mut state, listing(t0 + MIN, vec![slot("w-2", "TW-2", None, Observed::AgentWorking), slot("w-1", "TW-1", Some("h-wsl"), Observed::AgentIdle)]));
    list(&mut state, listing(t0 + 2 * MIN, vec![slot("w-2", "TW-2", None, Observed::AgentWorking), slot("w-1", "TW-1", Some("h-wsl"), Observed::AgentIdle)]));
    state.running.insert(1, running(Some("TW-2"), "deliverMessage", Lane::Issue, t0 + 2 * MIN));
    let now = t0 + 3 * MIN;
    let shown = view(&state, &Live::default(), now);
    let rows: Vec<_> = shown.slots.iter().map(|s| (s.issue.as_deref(), s.host.as_str(), s.agent, s.for_, s.job.clone())).collect();
    assert_eq!(rows, vec![(Some("TW-1"), "wsl", Agent::Idle, 2 * MIN, None), (Some("TW-2"), "local", Agent::Working, 3 * MIN, Some(("deliverMessage".into(), MIN)))]);
    // Up to the cap, the rest are free.
    assert_eq!(shown.free, 2);
    let used: Vec<_> = shown.hosts.iter().map(|h| (h.name.as_str(), h.used, h.cap)).collect();
    assert_eq!(used, vec![("local", 1, 2), ("wsl", 1, 2)]);
    // A worktree the next listing lacks takes its times with it.
    list(&mut state, listing(now, vec![slot("w-2", "TW-2", None, Observed::AgentWorking)]));
    assert!(!state.began.contains_key("w-1") && !state.changed.contains_key("w-1"));
    assert_eq!(view(&state, &Live::default(), now).free, 3);
}

#[test]
fn a_queued_row_shows_its_last_reason_its_age_and_goes_once_its_issue_holds_a_slot() {
    let t0 = Instant::now();
    let mut state = State::default();
    state.queue("TW-9", Waiting::Blockers, AT, Some(vec!["TW-1".into(), "TW-2".into()]), t0);
    // A later command for the same wait, the names not read again, keeps them.
    state.queue("TW-9", Waiting::Blockers, AT, None, t0 + MIN);
    state.queue("TW-8", Waiting::Slot, AT, None, t0);
    state.queue("TW-1", Waiting::Slot, AT, None, t0);
    list(&mut state, listing(t0, vec![slot("w-1", "TW-1", None, Observed::AgentWorking)]));
    let shown = view(&state, &Live::default(), t0 + 3 * MIN);
    let rows: Vec<_> = shown.queued.iter().map(|q| (q.issue.as_str(), q.reason.as_str(), q.age)).collect();
    assert_eq!(rows, vec![("TW-9", "blocked by TW-1, TW-2", 2 * MIN), ("TW-8", "waiting for a slot", 3 * MIN)]);
    assert!(!state.queued.contains_key("TW-1"));
    // Nothing has said one for an hour: its request may have gone.
    list(&mut state, listing(t0 + 61 * MIN, Vec::new()));
    assert!(!state.queued.contains_key("TW-8") && !state.queued.contains_key("TW-9"));
    // Another wait drops the names.
    state.queue("TW-9", Waiting::Slot, AT, None, t0);
    state.queue("TW-9", Waiting::Blockers, AT, None, t0);
    assert!(state.queued["TW-9"].blocked_by.is_empty());
    // A reason older than the one held, from a command that ran late, changes nothing.
    state.queue("TW-8", Waiting::Blockers, "2026-10-20T11:00:00.000Z", Some(vec!["TW-3".into()]), t0);
    state.queue("TW-8", Waiting::Slot, AT, None, t0 + MIN);
    assert_eq!((state.queued["TW-8"].waiting, state.queued["TW-8"].said), (Waiting::Blockers, t0));
}

#[test]
fn a_conversation_goes_once_a_listing_lacks_its_terminal_unless_it_opened_just_now() {
    let t0 = Instant::now();
    let mut state = State::default();
    state.conversations.insert("t-old".into(), ("TW-1".into(), t0));
    state.conversations.insert("t-new".into(), ("TW-2".into(), t0 + 2 * MIN));
    state.conversations.insert("t-listed".into(), ("TW-3".into(), t0));
    state.list(listing(t0 + 3 * MIN, Vec::new()), &HashSet::from(["t-listed".to_string()]));
    let mut kept: Vec<_> = state.conversations.keys().cloned().collect();
    kept.sort();
    assert_eq!(kept, vec!["t-listed", "t-new"]);
}

#[test]
fn a_slots_run_time_counts_across_its_states_and_its_last_step_is_its_issues_newest_finish() {
    let t0 = Instant::now();
    let mut state = State::default();
    let finish = |at: Instant, issue: &str, command: &str, outcome: Outcome| Finish { at, issue: issue.into(), command: command.into(), outcome, error: None };
    // Finished before any listing had its worker.
    state.finish(finish(t0, "TW-1", "startWorker", Outcome::Ok));
    list(&mut state, listing(t0 + MIN, vec![slot("w-1", "TW-1", None, Observed::AgentWorking), slot("w-2", "TW-2", None, Observed::AgentWorking)]));
    assert_eq!(view(&state, &Live::default(), t0 + MIN).slots[0].last, Some(("startWorker".into(), Outcome::Ok, MIN)));
    state.finish(finish(t0 + 2 * MIN, "TW-1", "evaluatePr", Outcome::Failed));
    state.finish(finish(t0 + 3 * MIN, "TW-9", "readIssue", Outcome::Ok));
    for n in 2..90 {
        let seen = if n % 2 == 0 { Observed::AgentIdle } else { Observed::AgentWorking };
        list(&mut state, listing(t0 + n * MIN, vec![slot("w-1", "TW-1", None, seen), slot("w-2", "TW-2", None, Observed::AgentWorking)]));
    }
    let shown = view(&state, &Live::default(), t0 + 90 * MIN);
    let rows: Vec<_> = shown.slots.iter().map(|s| (s.for_, s.ran, s.last.clone())).collect();
    // Past the hour's counts, it keeps while its slot lives.
    assert_eq!(rows, vec![(MIN, 89 * MIN, Some(("evaluatePr".into(), Outcome::Failed, 88 * MIN))), (89 * MIN, 89 * MIN, None)]);
    // An issue no slot holds keeps its last step for the hour only.
    assert!(!state.last.contains_key("TW-9"));
    // One that finished first, locked last, keeps the newer.
    state.finish(finish(t0 + 2 * MIN, "TW-2", "observe", Outcome::Ok));
    state.finish(finish(t0 + MIN, "TW-2", "stopWorker", Outcome::Ok));
    assert_eq!(state.last["TW-2"].command, "observe");
}

#[test]
fn also_running_is_every_job_but_a_slotted_issues_own_lane_then_the_held_ones() {
    let t0 = Instant::now();
    let mut state = State::default();
    list(&mut state, listing(t0, vec![slot("w-1", "TW-1", None, Observed::AgentWorking)]));
    state.running.insert(0, running(None, "update check", Lane::Shared, t0));
    state.running.insert(1, running(Some("TW-1"), "deliverMessage", Lane::Issue, t0));
    state.running.insert(2, running(Some("TW-1"), "stopWorker", Lane::Urgent, t0 + MIN));
    state.running.insert(3, running(Some("TW-5"), "readIssue", Lane::Issue, t0 + MIN + Duration::from_secs(1)));
    state.held = vec![Held { issue: None, label: "o/tolkster pushed".into(), due: t0 + 3 * MIN }];
    let shown = view(&state, &Live::default(), t0 + 2 * MIN);
    assert_eq!(shown.also, vec![
        Also::Job { issue: None, label: "update check".into(), for_: 2 * MIN },
        Also::Job { issue: Some("TW-1".into()), label: "stopWorker".into(), for_: MIN },
        Also::Job { issue: Some("TW-5".into()), label: "readIssue".into(), for_: MIN - Duration::from_secs(1) },
        Also::Held { issue: None, label: "o/tolkster pushed".into(), left: MIN },
    ]);
}

#[test]
fn needs_you_is_a_waiting_agent_a_silent_host_or_a_spent_budget() {
    let t0 = Instant::now();
    let mut state = State::default();
    list(&mut state, listing(t0, vec![slot("w-1", "TW-1", None, Observed::AgentWaiting), slot("w-2", "TW-2", Some("h-wsl"), Observed::AgentIdle)]));
    let budgets = vec![Budget { api: "Linear", limit: Some(5000), remaining: 0, resets: MIN, spent: true }, Budget { api: "GitHub", limit: Some(5000), remaining: 10, resets: MIN, spent: false }];
    let silences = vec![Silence { host: "h-wsl".into(), name: Some("wsl".into()), asleep: false, since: t0, next: t0 + 2 * MIN }];
    let shown = view(&state, &Live { budgets, silences, ..Live::default() }, t0 + MIN);
    assert_eq!(shown.needs, vec![Need::Waiting("TW-1".into()), Need::Silent("wsl".into()), Need::Spent("Linear")]);
    let wsl = &shown.hosts[1];
    assert_eq!((wsl.answering, wsl.silent_for, wsl.asked_in), (false, Some(MIN), Some(MIN)));
    assert!(view(&State::default(), &Live::default(), t0).needs.is_empty());
}

// A sleep is planned: the machine stays listed as asleep, by its name, and asks nothing of the owner,
// whether the listing or `listen`'s silences say it is silent; a fault still does.
#[test]
fn an_asleep_machine_is_listed_asleep_and_needs_nobody_while_a_silent_one_still_does() {
    let t0 = Instant::now();
    let mut state = State::default();
    let mut asleep = listing(t0, vec![]);
    asleep.hosts[1].answering = false;
    list(&mut state, asleep);
    let sleeping = Silence { host: "h-wsl".into(), name: Some("wsl".into()), asleep: true, since: t0, next: t0 + 10 * MIN };
    let shown = view(&state, &Live { silences: vec![sleeping.clone()], ..Live::default() }, t0 + MIN);
    assert!(shown.needs.is_empty(), "{:?}", shown.needs);
    let wsl = &shown.hosts[1];
    assert_eq!((wsl.asleep, wsl.silent_for, wsl.asked_in), (true, None, None));
    let lines = super::draw::panel(&shown, &Default::default(), t0, 120).iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
    assert!(lines.contains("wsl asleep"), "{lines}");
    // Asleep and unnamed by the listing, `listen`'s silence alone is no need either.
    let shown = view(&State::default(), &Live { silences: vec![sleeping.clone()], ..Live::default() }, t0);
    assert!(shown.needs.is_empty());
    let faulted = Silence { asleep: false, ..sleeping };
    let shown = view(&state, &Live { silences: vec![faulted], ..Live::default() }, t0 + MIN);
    assert_eq!(shown.needs, vec![Need::Silent("wsl".into())]);
    assert!(!shown.hosts[1].asleep);
}

#[test]
fn the_outcomes_count_the_last_hour_and_name_the_newest_failures_by_kind() {
    let t0 = Instant::now();
    let mut state = State::default();
    let finish = |at: Instant, issue: &str, outcome: Outcome, error: Option<&'static str>| Finish { at, issue: issue.into(), command: "startWorker".into(), outcome, error };
    state.outcomes.extend([finish(t0, "TW-1", Outcome::Failed, Some("Orca")), finish(t0 + 30 * MIN, "TW-2", Outcome::Ok, None), finish(t0 + 40 * MIN, "TW-3", Outcome::Held, None), finish(t0 + 50 * MIN, "TW-4", Outcome::Failed, None), finish(t0 + 55 * MIN, "TW-5", Outcome::Retryable, Some("budget spent"))]);
    let o = view(&state, &Live::default(), t0 + 70 * MIN).outcomes;
    assert_eq!((o.ok, o.failed, o.retryable), (1, 1, 2));
    assert_eq!(o.failures, vec![("TW-4".into(), "startWorker".into(), "failed")]);
}

#[test]
fn the_connection_and_the_update_read_as_they_stand() {
    let t0 = Instant::now();
    let mut state = State { connection: Connection::Reconnecting(t0 + Duration::from_secs(8)), base: Some("b".repeat(40)), ..State::default() };
    let live = Live { commit: Some("a".repeat(40)), building: Some(t0), ..Live::default() };
    let shown = view(&state, &live, t0 + Duration::from_secs(3));
    assert_eq!(shown.link, Link::Reconnecting(Duration::from_secs(5)));
    assert_eq!((shown.update.behind, shown.update.building), (Some(true), Some(Duration::from_secs(3))));
    state.base = Some("a".repeat(40));
    assert_eq!(view(&state, &live, t0).update.behind, Some(false));
}

fn contract() -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../test/contract.json")).unwrap()).unwrap()
}

// The fixture's frame (test/contract.json, which test/frame.test.ts holds to the Worker's
// shape) parses, and nothing else is a frame.
#[test]
fn the_fixtures_frame_parses_and_a_base_tip_isnt_one() {
    let contract = contract();
    let frame = parse_frame(&contract["frame"]).unwrap().unwrap();
    assert_eq!((frame.cap, frame.slots.len(), frame.queue.len(), frame.parked.len(), frame.chains.len()), (6, 2, 2, 2, 1));
    assert_eq!(frame.queue[1].blockers, vec!["TW-8".to_string()]);
    assert_eq!((frame.slots[0].host.as_deref(), frame.slots[1].host.as_deref()), (Some("tolkster-1"), None));
    assert!(parse_frame(&contract["baseTip"]).is_none());
    let mut wrong = contract["frame"].clone();
    wrong["slots"][0]["work"] = "dancing".into();
    assert!(parse_frame(&wrong).unwrap().is_err());
    // Why the fleet asks no machine, per repo, in the Durable Object's own list of reasons.
    assert_eq!(frame.why.into_iter().collect::<Vec<_>>(), [("r9".to_string(), super::frame::Why::NoRecipe), ("tolkster".to_string(), super::frame::Why::AllMade)]);
    let reasons: Vec<super::frame::Why> = serde_json::from_value(contract["machine"]["fleetReasons"].clone()).unwrap();
    assert_eq!(reasons, super::frame::WHYS);
}

// A frame says each repo's reason the fleet asks no machine for it when it is new or changed,
// and a new connection's first frame says them all again.
#[test]
fn a_frame_names_each_repos_no_machine_reason_only_when_it_changed() {
    let mut state = State::default();
    let frame = parse_frame(&contract()["frame"]).unwrap().unwrap();
    let all = vec![("r9".to_string(), super::frame::Why::NoRecipe.said()), ("tolkster".to_string(), super::frame::Why::AllMade.said())];
    assert_eq!(state.reframe(Some(frame.clone())), all);
    assert!(state.reframe(Some(frame.clone())).is_empty());
    let mut changed = frame.clone();
    changed.why.insert("tolkster".into(), super::frame::Why::CoolingDown);
    assert_eq!(state.reframe(Some(changed)), vec![("tolkster".to_string(), super::frame::Why::CoolingDown.said())]);
    assert!(state.reframe(None).is_empty() && state.frame.is_none());
    state.reframe(Some(frame.clone()));
    state.connect(Connection::Connected(Instant::now()));
    assert_eq!(state.reframe(Some(frame)), all);
}

#[test]
fn with_a_frame_a_reserved_slot_shows_before_its_worker_the_queue_keeps_its_order_and_parked_work_needs_you() {
    let t0 = Instant::now();
    let mut state = State::default();
    list(&mut state, listing(t0, vec![slot("w-1", "TW-1", None, Observed::AgentWorking)]));
    state.queue("TW-9", Waiting::Slot, AT, None, t0);
    let frame = parse_frame(&contract()["frame"]).unwrap().unwrap();
    state.frame = Some(frame);
    let shown = view(&state, &Live::default(), t0);
    let rows: Vec<_> = shown.slots.iter().map(|s| (s.issue.as_deref(), s.host.as_str(), s.agent, s.pr.map(|p| p.number), s.deadline.as_ref().map(|d| d.0.as_str()))).collect();
    assert_eq!(rows, vec![(Some("TW-1"), "local", Agent::Background, Some(7), Some("check")), (Some("TW-2"), "·", Agent::Reserved, None, None)]);
    assert_eq!(shown.slots.iter().map(|s| s.anomaly).collect::<Vec<_>>(), vec![false, true]);
    // Its run time is the frame's, held since before the listing saw it.
    assert!(shown.slots[0].ran > Duration::ZERO);
    // The frame's queue replaces what commands said, in the coordinator's order.
    let queued: Vec<_> = shown.queued.iter().map(|q| q.issue.as_str()).collect();
    assert_eq!(queued, vec!["TW-3", "·"]);
    assert!(shown.framed);
    assert_eq!(shown.needs, vec![Need::Stalled("TW-6".into()), Need::NoRounds("TW-1".into()), Need::Anomaly("TW-2".into())]);
    // Only work at rest whose PR has auto-merge off waits on the owner; one not yet read doesn't.
    assert_eq!(shown.parked.iter().map(|p| (p.issue.as_str(), p.owner)).collect::<Vec<_>>(), vec![("TW-6", false), ("TW-8", true)]);
    // Two reserved for issues with no identifier known are two rows.
    let unnamed = state.frame.as_mut().unwrap();
    unnamed.slots[1].identifier = None;
    unnamed.slots.push(unnamed.slots[1].clone());
    assert_eq!(view(&state, &Live::default(), t0).slots.len(), 3);
    // A new connection's frame is its own: until it comes, the local view.
    state.connect(Connection::Connected(t0));
    assert!(!view(&state, &Live::default(), t0).framed);
}

#[test]
fn a_listed_slots_agent_is_the_frames_work_state_where_it_has_one_else_the_listings() {
    let t0 = Instant::now();
    let mut state = State::default();
    list(&mut state, listing(t0, vec![slot("w-1", "TW-1", None, Observed::AgentIdle)]));
    let mut frame = parse_frame(&contract()["frame"]).unwrap().unwrap();
    let agent = |state: &State| view(state, &Live::default(), t0).slots[0].agent;
    // Running while its agent waits on a call in the background, which Orca lists as idle.
    state.frame = Some(frame.clone());
    assert_eq!(agent(&state), Agent::Background);
    frame.slots[0].background = false;
    state.frame = Some(frame.clone());
    assert_eq!(agent(&state), Agent::Working);
    frame.slots[0].work = Some(Work::Held);
    state.frame = Some(frame.clone());
    assert_eq!(agent(&state), Agent::Waiting);
    // The reverse: the frame says idle while the listing says working.
    list(&mut state, listing(t0, vec![slot("w-1", "TW-1", None, Observed::AgentWorking)]));
    frame.slots[0].work = Some(Work::Idle);
    state.frame = Some(frame.clone());
    assert_eq!(agent(&state), Agent::Idle);
    // No work state, or one that says nothing of the agent, leaves the listing's.
    for work in [None, Some(Work::Exited)] {
        frame.slots[0].work = work;
        state.frame = Some(frame.clone());
        assert_eq!(agent(&state), Agent::Working);
    }
    state.frame = None;
    assert_eq!(agent(&state), Agent::Working);
}

// The wait on the owner passes down a chain of blockers, naming the parked work it starts at.
#[test]
fn work_blocked_on_a_pr_waiting_on_the_owner_waits_through_it_down_the_chain() {
    let mut state = State::default();
    let mut frame = parse_frame(&contract()["frame"]).unwrap().unwrap();
    let wait = |identifier: &str, blockers: &[&str]| super::frame::Queued { identifier: Some(identifier.into()), waiting: Waiting::Blockers, blockers: blockers.iter().map(|b| b.to_string()).collect(), at: AT.into() };
    frame.queue = vec![wait("TW-4", &["TW-8"]), wait("TW-5", &["TW-4"]), wait("TW-10", &["TW-6"]), wait("TW-11", &["TW-12"])];
    state.frame = Some(frame);
    let shown = view(&state, &Live::default(), Instant::now());
    let through: Vec<_> = shown.queued.iter().map(|q| (q.issue.as_str(), q.through.as_deref())).collect();
    // TW-6 is stalled, not waiting on the owner.
    assert_eq!(through, vec![("TW-4", Some("TW-8")), ("TW-5", Some("TW-8")), ("TW-10", None), ("TW-11", None)]);
}

// A parked PR names its CI state in words, and the summary lines sit under their own heading.
#[test]
fn a_parked_row_reads_its_ci_state_and_activity_heads_the_summary_lines() {
    let t0 = Instant::now();
    let state = State { frame: Some(parse_frame(&contract()["frame"]).unwrap().unwrap()), ..State::default() };
    let lines: Vec<String> = super::draw::panel(&view(&state, &Live::default(), t0), &Default::default(), t0, 100).iter().map(ToString::to_string).collect();
    let at = |starts: &str| lines.iter().position(|l| l.trim_start().starts_with(starts)).unwrap();
    assert_eq!(lines[at("‖ TW-6")].trim_end(), "  ‖ TW-6     stalled  #8 CI running");
    assert_eq!(lines[at("‖ TW-8")].trim_end(), "  ‖ TW-8     rest     #9 CI passed   waits on you");
    let (parked, activity, chains) = (at("─ Parked"), at("─ Activity"), at("TW-7 chain"));
    assert!(parked < at("‖ TW-8") && at("‖ TW-8") < activity && activity < chains && chains < at("last hour"));
}

// A host holding more workers than its cap reads held/cap and is marked, not shown as full at its count.
#[test]
fn a_host_over_its_cap_reads_held_over_cap_and_is_marked() {
    let t0 = Instant::now();
    let mut state = State::default();
    let hosts = vec![HostShare { id: Some("h-wsl".into()), name: "wsl".into(), cap: 2, answering: true, refused: false }];
    let slots = vec![slot("w-1", "TW-1", Some("h-wsl"), Observed::AgentWorking), slot("w-2", "TW-2", Some("h-wsl"), Observed::AgentWorking), slot("w-3", "TW-3", Some("h-wsl"), Observed::AgentWorking)];
    list(&mut state, Listing { at: t0, took: Duration::from_millis(800), slots, hosts, cap: 3 });
    let shown = view(&state, &Live::default(), t0);
    assert_eq!((shown.hosts[0].used, shown.hosts[0].cap), (3, 2));
    let lines: Vec<String> = super::draw::panel(&shown, &Default::default(), t0, 40).iter().map(ToString::to_string).collect();
    assert!(lines.iter().any(|l| l.contains("wsl") && l.contains("3/2 over")), "{lines:?}");
}

// A frame slot no listing has a worker in reads as the frame says it is, never as a worker
// that didn't launch, and its host column is unknown rather than the work's state.
#[test]
fn an_unlisted_frame_slot_reads_reserved_launching_or_unseen_and_its_host_is_unknown() {
    let t0 = Instant::now();
    let mut state = State::default();
    let mut frame = parse_frame(&contract()["frame"]).unwrap().unwrap();
    let held = |identifier: &str, reserved: bool, work: Option<super::frame::Work>| super::frame::Slot { identifier: Some(identifier.into()), reserved, work, deadline: None, pr: None, anomaly: false, ..frame.slots[1].clone() };
    frame.slots = vec![held("TW-1", true, None), held("TW-2", false, Some(super::frame::Work::Launching)), held("TW-3", false, Some(super::frame::Work::Running)), held("TW-4", false, Some(super::frame::Work::Exited))];
    state.frame = Some(frame);
    let rows = |state: &State, live: &Live| view(state, live, t0).slots.iter().map(|s| (s.host.clone(), s.agent)).collect::<Vec<_>>();
    let dot = |agent| ("·".to_string(), agent);
    assert_eq!(rows(&state, &Live::default()), vec![dot(Agent::Reserved), dot(Agent::Launching), dot(Agent::Unseen), dot(Agent::Unseen)]);
    // A paired host that is silent, by the listing or by `listen`'s own count, makes the rest "host silent".
    let hosts = vec![HostShare { id: Some("h-wsl".into()), name: "wsl".into(), cap: 2, answering: false, refused: false }];
    list(&mut state, Listing { at: t0, took: Duration::from_millis(800), slots: vec![], hosts, cap: 3 });
    let silent = vec![dot(Agent::Reserved), dot(Agent::Launching), dot(Agent::HostSilent), dot(Agent::HostSilent)];
    assert_eq!(rows(&state, &Live::default()), silent);
    state.listing.as_mut().unwrap().hosts[0].answering = true;
    assert_eq!(rows(&state, &Live::default())[2], dot(Agent::Unseen));
}

// After a restart with the host its worker started on silent, the row names that host and
// says it is silent; once it answers, the worker is unseen there.
#[test]
fn an_unlisted_worker_on_a_known_host_names_it_and_says_silent_while_it_is() {
    let t0 = Instant::now();
    let mut state = State::default();
    let mut frame = parse_frame(&contract()["frame"]).unwrap().unwrap();
    frame.slots = vec![super::frame::Slot { identifier: Some("TW-839".into()), reserved: false, host: Some("h-1".into()), work: Some(super::frame::Work::Running), deadline: None, pr: None, anomaly: false, ..frame.slots[0].clone() }];
    state.frame = Some(frame);
    let row = |state: &State, live: &Live| view(state, live, t0).slots.iter().map(|s| (s.host.clone(), s.agent)).next().unwrap();
    // Before any listing, by `listen`'s own count of silences.
    let silences = vec![Silence { host: "h-1".into(), name: Some("tolkster-1".into()), asleep: false, since: t0, next: t0 + MIN }];
    let live = Live { silences, ..Live::default() };
    assert_eq!(row(&state, &live), ("tolkster-1".to_string(), Agent::Silent));
    let mut shown = view(&state, &live, t0);
    shown.slots.truncate(1);
    let lines: Vec<String> = super::draw::panel(&shown, &Default::default(), t0, 120).iter().map(ToString::to_string).collect();
    assert!(lines.iter().any(|l| l.contains("TW-839") && l.contains("tolkster-1") && l.contains("silent") && !l.contains("host silent")), "{lines:?}");
    // By the listing, which names it.
    let hosts = vec![HostShare { id: Some("h-1".into()), name: "tolkster-1".into(), cap: 2, answering: false, refused: false }];
    list(&mut state, Listing { at: t0, took: Duration::from_millis(800), slots: vec![], hosts, cap: 3 });
    assert_eq!(row(&state, &Live::default()), ("tolkster-1".to_string(), Agent::Silent));
    state.listing.as_mut().unwrap().hosts[0].answering = true;
    assert_eq!(row(&state, &Live::default()), ("tolkster-1".to_string(), Agent::Unseen));
}

#[test]
fn slot_rows_line_up_whatever_their_hosts_names_lengths() {
    let t0 = Instant::now();
    let mut shown = view(&State::default(), &Live::default(), t0);
    let row = |host: &str, agent| SlotRow {
        issue: Some("TW-1".into()), host: host.into(), agent, for_: MIN, ran: MIN, last: None, job: None,
        deadline: Some(("check".into(), MIN)), pr: None, anomaly: false, merged: false, waiting: vec![],
    };
    shown.slots = vec![row("local", Agent::Working), row("tolkster-3", Agent::Idle), row("·", Agent::HostSilent)];
    let lines: Vec<String> = super::draw::panel(&shown, &Default::default(), t0, 120).iter().map(ToString::to_string).collect();
    let rows: Vec<&String> = lines.iter().filter(|l| l.contains("⏲")).collect();
    assert_eq!(rows.len(), 3, "{lines:?}");
    let at = |l: &str, what: &str| l.find(what).map(|i| l[..i].chars().count());
    assert!(rows.iter().all(|l| at(l, "⏲") == at(rows[0], "⏲")), "{rows:?}");
    let words: Vec<_> = rows.iter().map(|l| ["working", "idle", "host silent"].iter().find_map(|w| at(l, w))).collect();
    assert!(words.iter().all(|w| w.is_some() && *w == words[0]), "{rows:?}");
}

#[test]
fn a_merged_slot_reads_as_closing_not_idle() {
    let t0 = Instant::now();
    let mut shown = view(&State::default(), &Live::default(), t0);
    let row = |agent, merged| SlotRow {
        issue: Some("TW-1".into()), host: "local".into(), agent, for_: MIN, ran: MIN, last: None, job: None,
        deadline: None, pr: None, anomaly: false, merged, waiting: vec![],
    };
    shown.slots = vec![row(Agent::Idle, true), row(Agent::Idle, false), row(Agent::Working, true)];
    let lines: Vec<String> = super::draw::panel(&shown, &Default::default(), t0, 120).iter().map(ToString::to_string).collect();
    assert!(lines.iter().any(|l| l.contains("merged, closing") && !l.contains("idle")), "{lines:?}");
    assert_eq!(lines.iter().filter(|l| l.contains(" idle ")).count(), 1, "{lines:?}");
    assert!(lines.iter().any(|l| l.contains("working, merged")), "{lines:?}");
}

#[test]
fn an_activity_line_wider_than_the_box_keeps_whole_items_and_ends_in_how_many_more() {
    let t0 = Instant::now();
    let mut shown = view(&State::default(), &Live::default(), t0);
    shown.chains = (900..912).map(|n| super::view::ChainRow { issue: format!("TW-{n}"), answered: 1, issues: 3 }).collect();
    let item = |n: usize| format!("TW-{n} chain 1/3 answered");
    let width = 70;
    let lines: Vec<String> = super::draw::panel(&shown, &Default::default(), t0, width).iter().map(ToString::to_string).collect();
    let line = &lines[lines.iter().position(|l| l.contains("─ Activity")).unwrap() + 1];
    assert!(line.chars().count() <= width && line.ends_with(" more"), "{line}");
    let kept = (900..912).filter(|n| line.contains(&item(*n))).count();
    assert!(kept > 0 && kept < 12, "{line}");
    assert!(line.ends_with(&format!("+{} more", 12 - kept)), "{line}");
    // Whole, it has no `+N more`.
    let lines: Vec<String> = super::draw::panel(&shown, &Default::default(), t0, 400).iter().map(ToString::to_string).collect();
    assert!(lines.iter().any(|l| l.contains(&item(911)) && !l.contains("more")), "{lines:?}");
}

#[test]
fn each_host_is_named_on_the_slots_line_with_its_mark_and_not_in_the_footer() {
    let t0 = Instant::now();
    let mut state = State::default();
    let mut refused = listing(t0, vec![]);
    refused.hosts[1].refused = true;
    list(&mut state, refused);
    let shown = view(&state, &Live::default(), t0);
    let lines: Vec<String> = super::draw::panel(&shown, &Default::default(), t0, 120).iter().map(ToString::to_string).collect();
    assert!(lines[0].contains("local") && lines[0].contains("✔") && lines[0].contains("wsl") && lines[0].contains("✖ can't read config"), "{lines:?}");
    let footer = super::draw::footer(&shown).to_string();
    assert!(!footer.contains("local") && !footer.contains("wsl") && footer.contains("listing"), "{footer}");
}

#[test]
fn one_host_holding_slots_draws_one_bar_and_two_draw_one_each() {
    let t0 = Instant::now();
    let bars = |slots: Vec<Slot>| {
        let mut state = State::default();
        list(&mut state, listing(t0, slots));
        let shown = view(&state, &Live::default(), t0);
        super::draw::panel(&shown, &Default::default(), t0, 120)[0].to_string().matches('█').count()
    };
    let one = bars(vec![slot("w-1", "TW-1", Some("h-wsl"), Observed::AgentWorking)]);
    let two = bars(vec![slot("w-1", "TW-1", Some("h-wsl"), Observed::AgentWorking), slot("w-2", "TW-2", None, Observed::AgentWorking)]);
    assert!(two > one, "{one} {two}");
}

#[test]
fn a_budget_reads_what_is_left_and_a_queued_row_says_since_once() {
    let t0 = Instant::now();
    let mut state = State { frame: Some(parse_frame(&contract()["frame"]).unwrap().unwrap()), ..State::default() };
    state.frame.as_mut().unwrap().queue = vec![super::frame::Queued { identifier: Some("TW-4".into()), waiting: Waiting::Blockers, blockers: vec![], at: AT.into() }];
    let budgets = vec![Budget { api: "Linear", limit: Some(5000), remaining: 5000, resets: MIN, spent: false }];
    let shown = view(&state, &Live { budgets, ..Live::default() }, t0);
    let lines: Vec<String> = super::draw::panel(&shown, &Default::default(), t0, 120).iter().map(ToString::to_string).collect();
    assert!(lines.iter().any(|l| l.contains("Linear") && l.contains("5.0k left") && !l.contains("5.0k/")), "{lines:?}");
    let row = lines.iter().find(|l| l.contains("⧗ TW-4")).unwrap();
    assert!(row.contains("since") && !row.contains("ago"), "{row}");
}

#[test]
fn queued_work_blocked_by_a_slots_issue_is_drawn_under_it_and_stays_queued() {
    let t0 = Instant::now();
    let mut state = State::default();
    state.queue("TW-9", Waiting::Blockers, AT, Some(vec!["TW-1".into(), "TW-2".into()]), t0);
    state.queue("TW-7", Waiting::Blockers, AT, Some(vec!["TW-1".into()]), t0);
    state.queue("TW-6", Waiting::Blockers, AT, Some(vec!["TW-2".into()]), t0);
    list(&mut state, listing(t0, vec![slot("w-1", "TW-1", None, Observed::AgentWorking)]));
    let shown = view(&state, &Live::default(), t0);
    assert_eq!(shown.slots[0].waiting, vec!["TW-7", "TW-9"]);
    let lines: Vec<String> = super::draw::panel(&shown, &Default::default(), t0, 120).iter().map(ToString::to_string).collect();
    let at = lines.iter().position(|l| l.contains("TW-1 ")).unwrap();
    assert!(lines[at + 1].contains("└ TW-7, TW-9 wait on this"), "{lines:?}");
    // Blocked by TW-2, which holds no slot, TW-6 has nothing under it; all three are still queued.
    assert_eq!(lines.iter().filter(|l| l.contains("└")).count(), 1, "{lines:?}");
    assert_eq!(lines.iter().filter(|l| l.contains("⧗ TW-")).count(), 3, "{lines:?}");
}

#[test]
fn a_slot_without_a_pr_keeps_its_timer_where_one_with_a_pr_has_it() {
    let t0 = Instant::now();
    let mut shown = view(&State::default(), &Live::default(), t0);
    let row = |issue: &str, pr| SlotRow {
        issue: Some(issue.into()), host: "local".into(), agent: Agent::Working, for_: MIN, ran: MIN, last: None, job: None,
        deadline: Some(("check".into(), MIN)), pr, anomaly: false, merged: false, waiting: vec![],
    };
    let pr = super::frame::Pr { number: 702, checks: super::frame::Checks::Pending, merge_state: None, rounds: 0, capped: false, auto_merge: None };
    shown.slots = vec![row("TW-1", Some(pr)), row("TW-2", None)];
    let lines: Vec<String> = super::draw::panel(&shown, &Default::default(), t0, 120).iter().map(ToString::to_string).collect();
    let timers: Vec<_> = lines.iter().filter(|l| l.contains("⏲")).map(|l| l.find("⏲").map(|i| l[..i].chars().count())).collect();
    assert_eq!(timers.len(), 2, "{lines:?}");
    assert!(timers[0].is_some() && timers[0] == timers[1], "{lines:?}");
}

#[test]
fn a_zero_outcome_count_is_dim_and_a_failure_is_not() {
    let t0 = Instant::now();
    let mut shown = view(&State::default(), &Live::default(), t0);
    let dim = |shown: &super::view::View, said: &str| {
        let lines = super::draw::panel(shown, &Default::default(), t0, 120);
        let span = lines.last().unwrap().spans.iter().find(|s| s.content == said).unwrap_or_else(|| panic!("{said} in {lines:?}")).clone();
        span.style.add_modifier.contains(ratatui::style::Modifier::DIM)
    };
    shown.outcomes = super::view::Outcomes { ok: 6, failed: 0, retryable: 0, failures: vec![] };
    assert!(!dim(&shown, "6 ok") && dim(&shown, "0 failed") && dim(&shown, "0 retryable"));
    shown.outcomes.failed = 2;
    assert!(!dim(&shown, "2 failed") && dim(&shown, "0 retryable"));
}
