use super::*;
use crate::exec;

#[test]
fn one_lanes_jobs_run_in_order_and_the_same_line_in_flight_runs_once() {
    let (tx, rx) = mpsc::channel();
    let mut scheduler = Scheduler::spawn(Arc::default(), move |_, job, _| tx.send(named(job)).unwrap());
    // One issue's, so in its lane's order.
    queue(&mut scheduler, "a", "a", on_issue("1", "TW-1"));
    queue(&mut scheduler, "b", "b", on_issue("2", "TW-1"));
    // The same line again while in flight isn't run again.
    queue(&mut scheduler, "a", "a", on_issue("1", "TW-1"));
    assert_eq!((rx.recv().unwrap(), rx.recv().unwrap()), ("1".to_string(), "2".to_string()));
    settled(&mut scheduler);
    assert!(scheduler.idle());
    assert!(rx.try_recv().is_err());
}

#[test]
fn a_job_that_never_finishes_is_noticed() {
    let (release, wait) = mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    let mut scheduler = Scheduler::spawn(Arc::default(), move |_, _, _| {
        let _ = wait.lock().unwrap().recv();
    });
    assert!(scheduler.stuck(Duration::ZERO).is_none());
    queue(&mut scheduler, "a", "a", on("1"));
    let deadline = Instant::now() + Duration::from_secs(5);
    while scheduler.stuck(Duration::ZERO).is_none() && Instant::now() < deadline {
        sleep(Duration::from_millis(10));
    }
    assert_eq!(scheduler.stuck(Duration::ZERO), Some(Stuck::Job("a".into())));
    assert!(scheduler.stuck(STUCK_AFTER).is_none());
    release.send(()).unwrap();
    settled(&mut scheduler);
    assert!(scheduler.stuck(Duration::ZERO).is_none());
}

#[test]
fn a_burst_arriving_while_its_first_run_is_in_flight_runs_once_more_as_its_newest_line() {
    let (release, wait) = mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    let (tx, rx) = mpsc::channel();
    let mut scheduler = Scheduler::spawn(Arc::default(), move |_, job, _| {
        let Job::Command(command) = job else { panic!("not an issue's") };
        tx.send(command.id.to_string()).unwrap();
        let _ = wait.lock().unwrap().recv();
    });
    // The line is told by its job's ID, as a base tip's is by its commit.
    let changed = |at: &str| on_issue(at, "PR-7");
    // Acted on as it comes, with no settle first.
    queue(&mut scheduler, "pr:o/r#7", "line 0", changed("0"));
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "0");
    // The same line again changes nothing.
    queue(&mut scheduler, "pr:o/r#7", "line 0", changed("0"));
    assert!(scheduler.again.is_empty());
    // Twenty more while it runs: none runs beside it.
    for i in 1..=20 {
        queue(&mut scheduler, "pr:o/r#7", &format!("line {i}"), changed(&i.to_string()));
    }
    assert!(scheduler.collect());
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    // Once it ends, one more run, of the newest, and none after it.
    release.send(()).unwrap();
    assert_eq!(began(&mut scheduler, &rx), "20");
    assert!(!scheduler.idle());
    release.send(()).unwrap();
    settled(&mut scheduler);
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    assert!(scheduler.idle());
}

#[test]
fn a_job_that_panics_is_done_with_and_the_next_runs() {
    let (tx, rx) = mpsc::channel();
    let mut scheduler = Scheduler::spawn(Arc::default(), move |id, _, _| {
        if id == "start:a" {
            panic!("boom");
        }
        tx.send(id.to_string()).unwrap();
    });
    queue(&mut scheduler, "start:a", "start:a", on("a"));
    queue(&mut scheduler, "start:b", "start:b", on("b"));
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "start:b");
    settled(&mut scheduler);
    assert!(scheduler.idle());
}

#[test]
fn a_job_that_changes_the_worktrees_or_terminals_nudges_the_watcher_to_list_once_and_a_read_does_not() {
    let mut scheduler = Scheduler::spawn(Arc::default(), |id, _, _| {
        let calls: &[&[&str]] = match id {
            "start:read" => &[&["worktree", "ps", "--json"], &["terminal", "send", "--terminal", "t"]],
            "start:one" => &[&["worktree", "create", "--name", "w"], &["terminal", "close", "--terminal", "t"]],
            _ => &[&["terminal", "create", "--worktree", "w"]],
        };
        for args in calls {
            let _ = orca::call(&mut |_, _, _| Ok(exec::Output::ok("{}")), args);
        }
    });
    let (nudge, nudges) = mpsc::channel();
    *scheduler.lanes.0.nudge.lock().unwrap() = Some(nudge);
    for name in ["read", "one", "two"] {
        queue(&mut scheduler, &format!("start:{name}"), &format!("start:{name}"), on(name));
        settled(&mut scheduler);
    }
    let nudged: Vec<Nudge> = nudges.try_iter().collect();
    assert_eq!(nudged.len(), 2);
    assert!(nudged.iter().all(|n| matches!(n, Nudge::Exited(None))));
}

#[test]
fn a_listing_write_that_failed_still_nudges_the_watcher() {
    let mut scheduler = Scheduler::spawn(Arc::default(), |_, _, _| {
        let _ = orca::call(&mut |_, _, _| Ok(exec::Output::failed(1, "timed out")), &["worktree", "create", "--name", "w"]);
    });
    let (nudge, nudges) = mpsc::channel();
    *scheduler.lanes.0.nudge.lock().unwrap() = Some(nudge);
    queue(&mut scheduler, "start:a", "start:a", on("a"));
    settled(&mut scheduler);
    assert!(matches!(nudges.try_recv(), Ok(Nudge::Exited(None))));
}

#[test]
fn a_job_that_comes_again_once_it_has_ended_runs_again() {
    let (tx, rx) = mpsc::channel();
    let mut scheduler = Scheduler::spawn(Arc::default(), move |_, job, _| tx.send(named(job)).unwrap());
    queue(&mut scheduler, "pr:o/r#7", "first", pr(7));
    settled(&mut scheduler);
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "pr-7");
    queue(&mut scheduler, "pr:o/r#7", "first", pr(7));
    assert!(scheduler.again.is_empty() && scheduler.in_flight.contains_key("pr:o/r#7"));
    settled(&mut scheduler);
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "pr-7");
}

#[test]
fn a_long_job_that_shows_progress_isnt_stuck() {
    let (release, wait) = mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    let (beaten, beats) = mpsc::channel::<()>();
    let mut scheduler = Scheduler::spawn(Arc::default(), move |_, _, beat| {
        sleep(Duration::from_millis(300));
        beat();
        beaten.send(()).unwrap();
        let _ = wait.lock().unwrap().recv();
    });
    queue(&mut scheduler, "a", "a", on("1"));
    beats.recv_timeout(Duration::from_secs(5)).unwrap();
    // Began over 300 ms ago, but showed progress since.
    assert!(scheduler.stuck(Duration::from_millis(250)).is_none());
    release.send(()).unwrap();
}

#[test]
fn a_held_local_job_is_acted_on_again_after_its_wait() {
    let (tx, rx) = mpsc::channel();
    let tries = Arc::new(Mutex::new(0));
    let counted = Arc::clone(&tries);
    let mut scheduler = Scheduler::spawn(Arc::default(), move |id, job, _| {
        let mut tries = counted.lock().unwrap();
        *tries += 1;
        tx.send((id.to_string(), named(job))).unwrap();
        if *tries == 1 { Acted::Held(Duration::from_millis(300)) } else { Acted::Done }
    });
    assert!(scheduler.queue_local("closed:r::/w/tw-1".into(), Job::Closed { worktree_id: "r::/w/tw-1".into() }));
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    settled(&mut scheduler);
    assert_eq!(scheduler.held.len(), 1);
    sleep(Duration::from_millis(400));
    assert!(scheduler.release_held());
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ("closed:r::/w/tw-1".to_string(), "closed r::/w/tw-1".to_string()));
    settled(&mut scheduler);
    assert!(scheduler.held.is_empty() && scheduler.idle());
}

#[test]
fn a_worktree_seen_closed_is_acted_on_at_once() {
    let (tx, rx) = mpsc::channel();
    let mut scheduler = Scheduler::spawn(Arc::default(), move |id, job, _| tx.send((id.to_string(), named(job))).unwrap());
    let (found, seen) = mpsc::channel();
    scheduler.watcher = Some(Watcher { seen, snapshots: mpsc::channel().1, since: Since::default(), nudge: mpsc::channel().0 });
    found.send(Seen::Closed("r::/w/conversations".into())).unwrap();
    assert!(scheduler.watch_agents());
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ("closed:r::/w/conversations".to_string(), "closed r::/w/conversations".to_string()));
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
}

#[test]
fn a_terminal_seen_ended_is_acted_on_at_once() {
    let (tx, rx) = mpsc::channel();
    let mut scheduler = Scheduler::spawn(Arc::default(), move |id, job, _| tx.send((id.to_string(), named(job))).unwrap());
    let (found, seen) = mpsc::channel();
    scheduler.watcher = Some(Watcher { seen, snapshots: mpsc::channel().1, since: Since::default(), nudge: mpsc::channel().0 });
    found.send(Seen::Ended("term_1".into())).unwrap();
    assert!(scheduler.watch_agents());
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ("ended:term_1".to_string(), "term_1".to_string()));
}
