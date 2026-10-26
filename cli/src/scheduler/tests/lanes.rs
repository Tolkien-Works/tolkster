use super::*;

#[test]
fn different_issues_are_acted_on_at_once_and_one_issues_in_turn() {
    let (mut scheduler, open, rx) = gated();
    queue(&mut scheduler, "start:a", "a", on_issue("a", "TW-1"));
    assert_eq!(next(&rx), "start a");
    // Another issue starts while TW-1's job runs; TW-1's next waits for it, in order.
    queue(&mut scheduler, "start:b", "b", on_issue("b", "tw-1"));
    queue(&mut scheduler, "start:c", "c", on_issue("c", "TW-1"));
    queue(&mut scheduler, "start:d", "d", on_issue("d", "TW-2"));
    assert_eq!(next(&rx), "start d");
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    open.send("a".into()).unwrap();
    assert_eq!((next(&rx), next(&rx)), ("end a".to_string(), "start b".to_string()));
    open.send("b".into()).unwrap();
    assert_eq!((next(&rx), next(&rx)), ("end b".to_string(), "start c".to_string()));
    open.send("c".into()).unwrap();
    open.send("d".into()).unwrap();
    settled(&mut scheduler);
}

#[test]
fn a_slow_job_of_the_shared_lane_holds_up_no_issue() {
    let (mut scheduler, open, rx) = gated();
    assert!(scheduler.queue_local("ended:w1".into(), Job::Ended { terminal: "w1".into() }));
    assert_eq!(next(&rx), "start w1");
    // The shared lane runs one job at a time: the next end waits, an issue's command doesn't.
    assert!(scheduler.queue_local("ended:w2".into(), Job::Ended { terminal: "w2".into() }));
    queue(&mut scheduler, "start:a", "a", on_issue("a", "TW-1"));
    assert_eq!(next(&rx), "start a");
    open.send("a".into()).unwrap();
    assert_eq!(next(&rx), "end a");
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    open.send("w1".into()).unwrap();
    assert_eq!((next(&rx), next(&rx)), ("end w1".to_string(), "start w2".to_string()));
    open.send("w2".into()).unwrap();
    settled(&mut scheduler);
}

#[test]
fn a_job_queued_again_while_it_waits_for_its_lane_runs_once() {
    let (mut scheduler, open, rx) = gated();
    queue(&mut scheduler, "start:a", "a", on_issue("a", "TW-1"));
    assert_eq!(next(&rx), "start a");
    queue(&mut scheduler, "start:b", "b", on_issue("b", "TW-1"));
    // The same line again, while it waits for the lane.
    queue(&mut scheduler, "start:b", "b", on_issue("b", "TW-1"));
    open.send("a".into()).unwrap();
    open.send("b".into()).unwrap();
    settled(&mut scheduler);
    assert_eq!([next(&rx), next(&rx), next(&rx)], ["end a", "start b", "end b"]);
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
}

#[test]
fn a_quitting_run_takes_no_new_command_and_is_drained_once_what_it_took_has_answered() {
    let mut scheduler = Scheduler::spawn(Arc::default(), |_, job, _| {
        if let Job::Command(command) = job {
            sleep(Duration::from_millis(100));
            answer(command, &Finished::ok(serde_json::json!({})));
        }
    });
    assert!(scheduler.take_command(command("c-1", "TW-1", false)));
    scheduler.quit();
    assert!(!scheduler.drained());
    // Left for its resend to the next run.
    assert!(scheduler.take_command(command("c-2", "TW-2", false)));
    assert!(!scheduler.commands.contains_key("c-2"));
    // A base tip, which could start a build, is left for the next run.
    assert!(scheduler.take_base(crate::machine::BaseTip { repo: "o/r".into(), reference: "refs/heads/main".into(), after: crate::plain::Sha::parse(&"a".repeat(40)).unwrap() }));
    assert!(!scheduler.in_flight.contains_key("base:o/r"));
    // What it took answers on the connection still open, and then it is drained.
    assert_eq!(results(&mut scheduler, 1, Duration::from_secs(5)), vec!["c-1".to_string()]);
    // Its result can go out before its job's end is collected.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !scheduler.drained() && Instant::now() < deadline {
        assert!(results(&mut scheduler, 1, Duration::from_millis(10)).is_empty());
    }
    assert!(scheduler.drained());
}
