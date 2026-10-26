use super::*;

#[test]
fn a_build_that_runs_no_command_for_the_watchdogs_wait_is_stuck() {
    let update = Update::default();
    let after = Duration::from_secs(10 * 60);
    assert!(!update.stuck(after));
    update.set_building(Some(Instant::now()));
    assert!(!update.stuck(after));
    update.set_building(Instant::now().checked_sub(after));
    assert!(update.stuck(after));
}

#[test]
fn a_tip_that_passed_isnt_passed_again_and_one_never_pulled_is_left_after_its_tries() {
    let (a, b) = (Sha::parse(&"a".repeat(40)).unwrap(), Sha::parse(&"b".repeat(40)).unwrap());
    let mut said = Vec::new();
    let mut tips = Tips::default();
    let mut passes = 0;
    // Pulled: done, and the same tip again, as a reconnect sends it, makes no pass.
    assert!(!tips.take(&a, &mut |line| said.push(line), || { passes += 1; Pass::Pulled }));
    assert!(!tips.take(&a, &mut |line| said.push(line), || { passes += 1; Pass::Pulled }));
    assert_eq!(passes, 1);
    // Never pulled: held, BASE_TRIES passes in all, then left, and the same tip makes no pass.
    passes = 0;
    for _ in 1..BASE_TRIES {
        assert!(tips.take(&b, &mut |line| said.push(line), || { passes += 1; Pass::Behind }));
    }
    assert!(!tips.take(&b, &mut |line| said.push(line), || { passes += 1; Pass::Behind }));
    assert!(!tips.take(&b, &mut |line| said.push(line), || { passes += 1; Pass::Behind }));
    assert_eq!(passes, BASE_TRIES);
    assert_eq!(said, [Said::StillNotHeld { after: "bbbbbbb".into() }]);
    // A newer tip starts afresh.
    assert!(!tips.take(&a, &mut |line| said.push(line), || Pass::Pulled));
    // A failed fetch, a build or a drain is held and counts no try; a guard holds nothing,
    // and leaves the tip to be passed again.
    let mut tips = Tips::default();
    for _ in 0..BASE_TRIES + 5 {
        assert!(tips.take(&b, &mut |line| said.push(line), || Pass::Unready));
    }
    assert!(!tips.take(&b, &mut |line| said.push(line), || Pass::Skipped));
    assert!(!tips.take(&b, &mut |line| said.push(line), || Pass::Pulled));
    assert!(!tips.take(&b, &mut |line| said.push(line), || panic!("passed again")));
}
