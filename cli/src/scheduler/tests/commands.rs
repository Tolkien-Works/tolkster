use super::*;

#[test]
fn a_commands_result_goes_back_once_and_a_resend_after_it_is_answered_not_carried_out_again() {
    let (mut scheduler, runs, _gate) = commanding();
    scheduler.connected();
    assert!(scheduler.take_command(command("c-1", "TW-1", false)));
    assert_eq!(results(&mut scheduler, 1, Duration::from_secs(5)), ["c-1"]);
    assert!(results(&mut scheduler, 1, Duration::from_millis(100)).is_empty());
    // Its result lost with the connection, the Durable Object sends it again on the next.
    scheduler.connected();
    assert!(scheduler.take_command(command("c-1", "TW-1", false)));
    assert_eq!(results(&mut scheduler, 1, Duration::from_secs(5)), ["c-1"]);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[test]
fn a_result_goes_back_only_on_the_connection_its_newest_copy_came_on() {
    let (mut scheduler, runs, gate) = commanding();
    scheduler.connected();
    assert!(scheduler.take_command(command("slow", "TW-1", false)));
    // Resent on a new connection while it runs: answered there, once, and not run twice.
    scheduler.connected();
    assert!(scheduler.take_command(command("slow", "TW-1", false)));
    gate.send(()).unwrap();
    assert_eq!(results(&mut scheduler, 2, Duration::from_millis(500)), ["slow"]);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    // Not resent: the connection it came on has gone, so its result waits for the resend.
    let (mut later, _, gate) = commanding();
    later.connected();
    assert!(later.take_command(command("slow", "TW-1", false)));
    later.connected();
    gate.send(()).unwrap();
    assert!(results(&mut later, 1, Duration::from_millis(300)).is_empty());
    assert!(later.take_command(command("slow", "TW-1", false)));
    assert_eq!(results(&mut later, 1, Duration::from_secs(5)), ["slow"]);
}

#[test]
fn one_command_runs_at_a_time_on_an_issue_and_an_urgent_one_runs_beside_it() {
    let (mut scheduler, _, gate) = commanding();
    scheduler.connected();
    assert!(scheduler.take_command(command("slow", "TW-1", false)));
    sleep(Duration::from_millis(50));
    assert!(scheduler.take_command(command("next", "TW-1", false)));
    assert!(scheduler.take_command(command("stop", "tw-1", true)));
    assert!(scheduler.take_command(command("other", "TW-2", false)));
    let mut first = results(&mut scheduler, 2, Duration::from_secs(5));
    first.sort();
    assert_eq!(first, ["other", "stop"]);
    assert!(results(&mut scheduler, 1, Duration::from_millis(200)).is_empty(), "next waits for slow");
    gate.send(()).unwrap();
    assert_eq!(results(&mut scheduler, 2, Duration::from_secs(5)), ["slow", "next"]);
}

#[test]
fn the_newest_listing_is_sent_first_on_each_connection_when_it_changes_and_each_backstop() {
    let mut scheduler = Scheduler::spawn(Arc::default(), |_, _, _| ());
    let (_found, seen) = mpsc::channel::<Seen>();
    let (listed, snapshots) = mpsc::channel();
    scheduler.watcher = Some(Watcher { seen, snapshots, since: Since::default(), nudge: mpsc::channel().0 });
    scheduler.connected();
    let sent = |scheduler: &mut Scheduler, fails: bool| {
        let mut lines = Vec::new();
        let end = scheduler.step(|line| if fails { Err("closed".to_string()) } else { lines.push(line); Ok(()) });
        assert_eq!(end.is_some(), fails);
        lines.iter().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()).map(|v| format!("{} {} {}", v["report"].as_str().unwrap(), v["at"].as_str().unwrap(), v["agents"])).collect::<Vec<_>>()
    };
    let start = Instant::now();
    let snapshot = |after: u64, at: &str, state: Observed| Snapshot { began: start + Duration::from_secs(after), at: at.into(), agents: vec![("r::a".into(), state)], listed: None, hosts: Vec::new(), paired: Vec::new(), fleet: Default::default(), config: String::new(), idle_hosts: Default::default(), dormant: None };
    assert!(sent(&mut scheduler, false).is_empty(), "nothing listed yet");
    listed.send(snapshot(0, "t0", Observed::AgentWorking)).unwrap();
    assert_eq!(sent(&mut scheduler, false), [r#"listing t0 {"r::a":"working"}"#]);
    // Only the newest counts.
    listed.send(snapshot(1, "t1", Observed::AgentWorking)).unwrap();
    listed.send(snapshot(2, "t2", Observed::AgentIdle)).unwrap();
    assert_eq!(sent(&mut scheduler, false), [r#"listing t2 {"r::a":"idle"}"#]);
    // The same again, sooner than the backstop: not sent; at the backstop, sent all the same.
    listed.send(snapshot(30, "t3", Observed::AgentIdle)).unwrap();
    assert!(sent(&mut scheduler, false).is_empty());
    listed.send(snapshot(62, "t4", Observed::AgentIdle)).unwrap();
    assert_eq!(sent(&mut scheduler, false), [r#"listing t4 {"r::a":"idle"}"#]);
    // A new connection is sent the newest first, and one not sent is sent on the next, with
    // what was queued behind it.
    scheduler.connected();
    scheduler.lanes.0.outbox.lock().unwrap().observations.push(Outgoing::TurnEnded("s-1".into()));
    assert!(sent(&mut scheduler, true).is_empty());
    scheduler.connected();
    let mut lines = Vec::new();
    assert!(scheduler.step(|line| { lines.push(line); Ok::<(), String>(()) }).is_none());
    let reports: Vec<String> = lines.iter().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["report"].as_str().unwrap().to_string()).collect();
    assert_eq!(reports, ["listing", "turnEnded"]);
    assert!(sent(&mut scheduler, false).is_empty());
}

// The config report is `listen`'s settings as it started until a listing reads them again, told
// to each connection first and again only when they change.
#[test]
fn the_config_is_told_on_each_connection_and_whenever_it_changes() {
    let mut scheduler = Scheduler::spawn(Arc::default(), |_, _, _| ()).telling(r#"{"report":"config","cap":6}"#.into());
    let (_found, seen) = mpsc::channel::<Seen>();
    let (listed, snapshots) = mpsc::channel();
    scheduler.watcher = Some(Watcher { seen, snapshots, since: Since::default(), nudge: mpsc::channel().0 });
    scheduler.connected();
    let told = |scheduler: &mut Scheduler| {
        let mut lines = Vec::new();
        assert!(scheduler.step(|line| { lines.push(line); Ok::<(), String>(()) }).is_none());
        lines.iter().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()).filter(|v| v["report"] == "config").map(|v| v["cap"].as_i64().unwrap()).collect::<Vec<_>>()
    };
    const NINE: &str = r#"{"report":"config","cap":9}"#;
    let snapshot = |at: &str, config: &str| Snapshot { began: Instant::now(), at: at.into(), agents: Vec::new(), listed: None, hosts: vec!["mac".into()], paired: vec!["mac".into()], fleet: Default::default(), config: config.into(), idle_hosts: Default::default(), dormant: None };
    assert_eq!(told(&mut scheduler), [6]);
    listed.send(snapshot("t0", NINE)).unwrap();
    assert_eq!(told(&mut scheduler), [9]);
    listed.send(snapshot("t1", NINE)).unwrap();
    assert!(told(&mut scheduler).is_empty());
    scheduler.connected();
    assert_eq!(told(&mut scheduler), [9]);
}

#[test]
fn a_stop_runs_while_every_issue_lane_is_busy() {
    let (hold, held) = mpsc::channel::<()>();
    let held = Arc::new(Mutex::new(held));
    let mut busy = Scheduler::spawn(Arc::default(), {
        let held = Arc::clone(&held);
        move |_, job, _| {
            if let Job::Command(command) = job {
                if command.id.starts_with("busy") {
                    let _ = held.lock().unwrap().recv();
                }
                answer(command, &Finished::ok(serde_json::json!({})));
            }
        }
    });
    busy.connected();
    for n in 1..=ISSUE_LANES {
        assert!(busy.take_command(command(&format!("busy-{n}"), &format!("TW-{n}"), false)));
    }
    sleep(Duration::from_millis(50));
    assert!(busy.take_command(command("stop", "TW-1", true)));
    assert_eq!(results(&mut busy, 1, Duration::from_secs(5)), ["stop"]);
    for _ in 1..=ISSUE_LANES {
        hold.send(()).unwrap();
    }
    assert_eq!(results(&mut busy, ISSUE_LANES, Duration::from_secs(5)).len(), ISSUE_LANES);
}

#[test]
fn a_command_that_ends_with_no_result_or_a_retryable_one_runs_again_on_its_resend() {
    let runs = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&runs);
    let mut scheduler = Scheduler::spawn(Arc::default(), move |_, job, _| {
        if let Job::Command(command) = job {
            let n = counted.fetch_add(1, Ordering::SeqCst);
            match (command.id.as_str(), n) {
                ("panics", 0) => panic!("no result"),
                ("budget", _) => answer(command, &Finished::of(&Error::RateLimited { message: "spent".into(), wait: Duration::from_secs(1) })),
                _ => answer(command, &Finished::ok(serde_json::json!({}))),
            }
        }
    });
    scheduler.connected();
    assert!(scheduler.take_command(command("panics", "TW-1", false)));
    settled(&mut scheduler);
    assert!(results(&mut scheduler, 1, Duration::from_millis(100)).is_empty());
    assert!(scheduler.take_command(command("panics", "TW-1", false)));
    assert_eq!(results(&mut scheduler, 1, Duration::from_secs(5)), ["panics"]);
    assert!(scheduler.take_command(command("budget", "TW-1", false)));
    assert_eq!(results(&mut scheduler, 1, Duration::from_secs(5)), ["budget"]);
    assert!(scheduler.take_command(command("budget", "TW-1", false)));
    assert_eq!(results(&mut scheduler, 1, Duration::from_secs(5)), ["budget"]);
    assert_eq!(runs.load(Ordering::SeqCst), 4);
}

#[test]
fn an_observation_not_sent_waits_for_the_next_connection_and_a_gone_or_ended_one_goes_once() {
    let mut scheduler = Scheduler::spawn(Arc::default(), |_, _, _| ());
    scheduler.connected();
    {
        let mut outbox = scheduler.lanes.0.outbox.lock().unwrap();
        outbox.observations.push(Outgoing::Worktree(Observed::WorktreeGone, "r::x".into()));
        outbox.observations.push(Outgoing::TurnEnded("s-1".into()));
    }
    assert!(matches!(scheduler.step(|_| Err::<(), String>("closed".into())), Some(End::Retry(_))));
    scheduler.connected();
    let mut lines = Vec::new();
    assert!(scheduler.step(|line| { lines.push(line); Ok::<(), String>(()) }).is_none());
    let reports: Vec<String> = lines.iter().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()).map(|v| format!("{} {}", v["report"].as_str().unwrap(), v["sequence"])).collect();
    assert_eq!(reports, ["worktreeGone 1", "turnEnded 2"]);
    scheduler.connected();
    let mut again = Vec::new();
    assert!(scheduler.step(|line| { again.push(line); Ok::<(), String>(()) }).is_none());
    assert!(again.is_empty(), "{again:?}");
}

#[test]
fn a_resend_between_a_runs_answer_and_its_end_runs_again_once_it_has_ended() {
    let (runs, gate) = (Arc::new(AtomicU64::new(0)), mpsc::channel::<()>());
    let (counted, waiting) = (Arc::clone(&runs), Mutex::new(gate.1));
    let mut scheduler = Scheduler::spawn(Arc::default(), move |_, job, _| {
        if let Job::Command(command) = job {
            counted.fetch_add(1, Ordering::SeqCst);
            answer(command, &Finished::of(&Error::RateLimited { message: "spent".into(), wait: Duration::from_secs(1) }));
            waiting.lock().unwrap().recv().unwrap();
        }
    });
    scheduler.connected();
    assert!(scheduler.take_command(command("budget", "TW-1", false)));
    assert_eq!(results(&mut scheduler, 1, Duration::from_secs(5)), ["budget"]);
    assert!(scheduler.take_command(command("budget", "TW-1", false)));
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    gate.0.send(()).unwrap();
    assert_eq!(results(&mut scheduler, 1, Duration::from_secs(5)), ["budget"]);
    gate.0.send(()).unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 2);
}

// Nothing is remembered of what a terminal ran, so a terminal opened before a restart of
// `listen` is reported as one opened since: the Durable Object, which holds each turn's and
// triage session's terminal, says what it ended.
#[test]
fn a_terminals_close_is_reported_by_its_handle_whatever_ran_in_it() {
    assert_eq!(closed(|| {
        terminal_closed("t1");
        terminal_closed("t2");
        terminal_closed("t1");
    }), ["t1", "t2", "t1"]);
}

#[test]
fn the_machines_commands_and_a_terminals_end_call_in_the_urgent_lane_and_listens_other_jobs_in_the_normal_one() {
    let (seen, rx) = mpsc::channel();
    let seen = Mutex::new(seen);
    let mut scheduler = Scheduler::spawn(Arc::default(), move |_, job, _| {
        seen.lock().unwrap().send((named(job), crate::gateway::lane())).unwrap();
    });
    scheduler.connected();
    assert!(scheduler.take_command(command("c-1", "TW-2", false)));
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ("c-1".to_string(), crate::gateway::Lane::Urgent));
    queue(&mut scheduler, "end:t", "t", on("t"));
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ("t".to_string(), crate::gateway::Lane::Urgent));
    queue(&mut scheduler, "closed:r::/w/c", "closed-line", Job::Closed { worktree_id: "r::/w/c".into() });
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ("closed r::/w/c".to_string(), crate::gateway::Lane::Normal));
}
