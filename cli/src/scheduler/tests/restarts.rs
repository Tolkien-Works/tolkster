use super::*;

#[test]
fn a_restart_waits_until_nothing_is_in_flight_and_is_taken_once() {
    let update = Update::default();
    let (release, wait) = mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    let mut scheduler = Scheduler::spawn(Arc::default(), move |_, _, _| {
        let _ = wait.lock().unwrap().recv();
    })
    .updating(update.clone());
    assert!(scheduler.restart_due().is_none());
    // Set while a job runs, as the job that built it would: not until it is done.
    queue(&mut scheduler, "a", "a", on("1"));
    update.in_place(Restart::Built { head: "b41c2f0".into() });
    assert!(scheduler.restart_due().is_none());
    release.send(()).unwrap();
    settled(&mut scheduler);
    assert_eq!(scheduler.restart_due(), Some(Restart::Built { head: "b41c2f0".into() }));
    // Taken: the loop returns it once.
    assert!(scheduler.restart_due().is_none() && !update.draining());
    // Without one set, an idle scheduler goes on.
    assert!(Scheduler::spawn(Arc::default(), |_, _, _| {}).restart_due().is_none());
}

#[test]
fn a_drain_takes_no_new_command_and_restarts_once_what_it_took_is_done() {
    let update = Update::default();
    let (release, wait) = mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    let (acted, seen) = mpsc::channel::<String>();
    let mut scheduler = Scheduler::spawn(Arc::default(), move |_, job, _| {
        let _ = wait.lock().unwrap().recv();
        let _ = acted.send(named(job));
    })
    .updating(update.clone());
    // Taken before the build was in place: run during the drain.
    queue(&mut scheduler, "a", "a", on("1"));
    queue(&mut scheduler, "p", "p", pr(7));
    update.in_place(Restart::Built { head: "b41c2f0".into() });
    // A command while draining isn't queued: it is left for its resend after the restart.
    assert!(scheduler.take_command(command("c-1", "TW-3", false)));
    assert!(!scheduler.in_flight.contains_key("command:c-1") && !scheduler.commands.contains_key("c-1"));
    // The watcher's jobs still run.
    assert!(scheduler.queue_local("ended:w1".into(), Job::Ended { terminal: "w1".into() }));
    assert!(scheduler.restart_due().is_none());
    for _ in 0..3 {
        release.send(()).unwrap();
    }
    let mut all = [seen.recv().unwrap(), seen.recv().unwrap(), seen.recv().unwrap()];
    all.sort_unstable();
    assert_eq!(all, ["1", "pr-7", "w1"]);
    settled(&mut scheduler);
    assert_eq!(scheduler.restart_due(), Some(Restart::Built { head: "b41c2f0".into() }));
}

#[test]
fn a_superseded_run_takes_no_new_command_and_ends_once_what_it_took_is_done() {
    let (release, wait) = mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    let (acted, seen) = mpsc::channel::<String>();
    let mut scheduler = Scheduler::spawn(Arc::default(), move |_, job, _| {
        let _ = wait.lock().unwrap().recv();
        let _ = acted.send(named(job));
    });
    queue(&mut scheduler, "a", "a", on("1"));
    let drained = std::thread::scope(|scope| {
        let waiting = scope.spawn(|| scheduler.drain_superseded());
        sleep(Duration::from_millis(400));
        // Still running the job it took.
        assert!(!waiting.is_finished());
        release.send(()).unwrap();
        waiting.join().unwrap()
    });
    assert!(drained.is_ok());
    assert_eq!(seen.recv().unwrap(), "1");
    // Nothing more is taken, and no update pass is made.
    assert!(scheduler.take_command(command("c-1", "TW-3", false)));
    assert!(!scheduler.in_flight.contains_key("command:c-1") && !scheduler.commands.contains_key("c-1"));
}

#[test]
fn the_restarts_line_names_the_job_the_listing_or_the_build_that_made_no_progress() {
    let stuck_for = |scheduler: &Scheduler| scheduler.stuck(STUCK_AFTER).map(|what| match End::stuck(what, STUCK_AFTER) {
        End::Restart(restart) => restart,
        _ => panic!("not a restart"),
    });
    let long_ago = Instant::now().checked_sub(STUCK_AFTER);
    let update = Update::default();
    let mut scheduler = Scheduler::spawn(Arc::default(), |_, _, _| {}).updating(update.clone());
    assert_eq!(stuck_for(&scheduler), None);

    *scheduler.acting[2].id.lock().unwrap() = "pr:o/r#7".into();
    scheduler.acting[2].since.set(long_ago);
    assert_eq!(stuck_for(&scheduler), Some(Restart::Stuck { what: Stuck::Job("pr:o/r#7".into()), minutes: 10 }));
    scheduler.acting[2].since.set(None);

    let since = Since::default();
    since.set(long_ago);
    scheduler.watcher = Some(Watcher { seen: mpsc::channel().1, snapshots: mpsc::channel().1, since, nudge: mpsc::channel().0 });
    assert_eq!(stuck_for(&scheduler), Some(Restart::Stuck { what: Stuck::Listing, minutes: 10 }));
    scheduler.watcher = None;

    update.set_building(long_ago);
    assert_eq!(stuck_for(&scheduler), Some(Restart::Stuck { what: Stuck::Build, minutes: 10 }));
}

#[test]
fn a_build_making_progress_holds_the_watchdog_for_a_while() {
    let update = Update::default();
    let scheduler = Scheduler::spawn(Arc::default(), |_, _, _| {}).updating(update.clone());
    let after = Duration::from_secs(10 * 60);
    scheduler.acting[0].since.set(Instant::now().checked_sub(after));
    assert!(scheduler.stuck(after).is_some());
    update.set_building(Some(Instant::now()));
    assert!(scheduler.stuck(after).is_none());
    // Only for BUILD_HOLDS more.
    scheduler.acting[0].since.set(Instant::now().checked_sub(after + BUILD_HOLDS));
    assert!(scheduler.stuck(after).is_some());
    scheduler.acting[0].since.set(Instant::now().checked_sub(after));
    // A build stuck itself doesn't hold it.
    update.set_building(Instant::now().checked_sub(after));
    assert!(scheduler.stuck(after).is_some());
    update.set_building(None);
    assert!(scheduler.stuck(after).is_some());
    // The drain's restart waits for a build to end too.
    update.set_building(Some(Instant::now()));
    update.in_place(Restart::Built { head: "b41c2f0".into() });
    assert!(scheduler.restart_due().is_none());
    update.set_building(None);
    assert!(scheduler.restart_due().is_some());
}

#[test]
fn a_base_tip_not_pulled_is_held_and_a_newer_one_runs_now() {
    let (tx, rx) = mpsc::channel();
    let acted = AtomicU64::new(0);
    let mut scheduler = Scheduler::spawn(Arc::default(), move |id, job, _| {
        let Job::Base { after, .. } = job else { panic!("not a base tip") };
        tx.send((id.to_string(), after.to_string())).unwrap();
        if acted.fetch_add(1, Ordering::SeqCst) == 0 { Acted::Held(Duration::from_secs(600)) } else { Acted::Done }
    });
    let tip = |after: char| crate::machine::BaseTip { repo: "O/R".into(), reference: "refs/heads/main".into(), after: Sha::parse(&after.to_string().repeat(40)).unwrap() };
    assert!(scheduler.take_base(tip('a')));
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), ("base:o/r".to_string(), "a".repeat(40)));
    let deadline = Instant::now() + Duration::from_secs(5);
    while scheduler.held.is_empty() && Instant::now() < deadline {
        assert!(scheduler.collect() && scheduler.release_held());
        sleep(Duration::from_millis(10));
    }
    assert_eq!(scheduler.held.keys().collect::<Vec<_>>(), ["base:o/r"]);
    // The same tip again, as a reconnect sends it, waits out the hold.
    assert!(scheduler.take_base(tip('a')));
    assert!(scheduler.release_held());
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    // A newer one runs now.
    assert!(scheduler.take_base(tip('b')));
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap().1, "b".repeat(40));
    settled(&mut scheduler);
    assert!(scheduler.held.is_empty());
}

// Runs `scheduler.step` until it says to end, sending each line to `send`, or five seconds pass.
fn stepped(scheduler: &mut Scheduler, mut send: impl FnMut(String) -> Result<(), String>) -> Option<End> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(end) = scheduler.step(&mut send) {
            return Some(end);
        }
        sleep(Duration::from_millis(10));
    }
    None
}

#[test]
fn the_step_takes_the_restart_once_the_job_running_has_ended() {
    let update = Update::default();
    let (release, wait) = mpsc::channel::<()>();
    let wait = Mutex::new(wait);
    let mut scheduler = Scheduler::spawn(Arc::default(), move |_, _, _| {
        let _ = wait.lock().unwrap().recv();
    })
    .updating(update.clone());
    queue(&mut scheduler, "a", "a", on("1"));
    update.in_place(Restart::Built { head: "b41c2f0".into() });
    // Still running: the restart waits.
    let mut sent = Vec::new();
    assert!(scheduler.step(|text| { sent.push(text); Ok::<(), String>(()) }).is_none());
    release.send(()).unwrap();
    let end = stepped(&mut scheduler, |text| { sent.push(text); Ok(()) });
    assert!(matches!(end, Some(End::Restart(restart)) if restart == Restart::Built { head: "b41c2f0".into() }));
    assert!(sent.is_empty());
}

#[test]
fn a_run_that_is_stuck_ends_before_anything_is_sent() {
    let mut scheduler = Scheduler::spawn(Arc::default(), |_, _, _| {});
    queue(&mut scheduler, "a", "a", on("1"));
    settled(&mut scheduler);
    scheduler.watcher = Some(Watcher { seen: mpsc::channel().1, snapshots: mpsc::channel().1, since: Since::default(), nudge: mpsc::channel().0 });
    scheduler.watcher.as_ref().unwrap().since.set(Some(Instant::now().checked_sub(STUCK_AFTER + Duration::from_secs(1)).unwrap()));
    let mut sent = Vec::new();
    let end = scheduler.step(|text| { sent.push(text); Ok::<(), String>(()) });
    assert!(matches!(end, Some(End::Restart(restart)) if matches!(restart, Restart::Stuck { what: Stuck::Listing, .. })));
    assert!(sent.is_empty());
}
