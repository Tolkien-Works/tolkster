use super::*;

// The worktrees in `ids`, as the watcher's listing would name them, with none idle.
fn running(ids: &[&str]) -> Agents {
    Agents { running: ids.iter().map(|id| WorktreeId::from(*id)).collect(), open: HashSet::new(), terminals: HashSet::new(), agents: HashSet::new(), idle: HashSet::new(), waiting: HashSet::new(), listed: None, others: HashSet::new(), hosts: Vec::new(), paired: Vec::new(), fleet: Default::default(), config: String::new(), idle_hosts: Default::default(), dormant: None }
}

// A host that didn't answer keeps what it showed last, so nothing there is seen to close, end
// or exit, and its agents' terminals aren't waited on; what this machine and the hosts that
// answered show is taken as it is.
#[test]
fn a_host_that_did_not_answer_keeps_what_it_showed_last() {
    let mut before = running(&["mac@r9::/a", "pi@r9::/b", "r1::/c"]);
    before.terminals = HashSet::from(["mac@term_1".to_string(), "pi@term_1".into(), "term_1".into()]);
    before.agents = before.terminals.clone();
    let now = running(&[]).keeping(&before, &["mac".into()]);
    assert_eq!(now.running, HashSet::from(["mac@r9::/a".into()]));
    assert_eq!((now.terminals, now.agents), (HashSet::from(["mac@term_1".to_string()]), HashSet::new()));
}

#[test]
fn the_watcher_sends_each_listing_whole() {
    // A failed listing is skipped; an agent gone is only missing from the next; a worktree
    // that isn't a worker's, such as the owner's primary checkout, is never named.
    let with_main = Agents { running: ["r::c".into(), "r::main".into()].into(), listed: Some(["r::c".into(), "r::main".into()].into()), others: ["r::main".into()].into(), ..running(&["r::c"]) };
    let mut listings = vec![Ok(running(&["r::b", "r::a"])), Err(Error::Failed("orca".into())), Ok(with_main), Ok(running(&[]))].into_iter();
    let watcher = Watcher::spawn(move || listings.next().unwrap_or_else(|| Ok(running(&[]))), |_| Ok(Missed::default()), |_| Ok(false), Duration::from_millis(1));
    let snapshot = || watcher.snapshots.recv_timeout(Duration::from_secs(5)).unwrap();
    let (first, second, third) = (snapshot(), snapshot(), snapshot());
    assert_eq!(first.agents, [("r::a".into(), Observed::AgentWorking), ("r::b".into(), Observed::AgentWorking)]);
    assert_eq!(first.listed, None);
    assert!(first.at.len() == 24 && first.at.ends_with('Z') && first.at <= second.at, "{}", first.at);
    assert_eq!((second.agents, second.listed), (vec![("r::c".into(), Observed::AgentWorking)], Some(vec!["r::c".into()])));
    assert!(third.agents.is_empty());
    assert!(second.began <= third.began);
    // A worktree gone is the Durable Object's to find from the snapshots: nothing else says it.
    assert!(watcher.seen.recv_timeout(Duration::from_millis(100)).is_err());
}

#[test]
fn the_first_listing_sends_what_missed_names_once_it_answers() {
    // A `missed` that fails is asked again of the next listing, and never again once it answers.
    let mut listings = vec![Ok(running(&["r::a"])), Ok(running(&["r::b"])), Ok(running(&["r::b"])), Ok(running(&[]))].into_iter();
    let answer = Missed { closed: vec!["r::k".into()] };
    let mut answers = vec![Err(Error::Failed("state".into())), Ok(answer)].into_iter();
    let (asked, asks) = mpsc::channel();
    let missed = move |now: &Agents| {
        asked.send(now.running.len()).unwrap();
        answers.next().unwrap_or_else(|| Ok(Missed { closed: vec!["r::never".into()] }))
    };
    let watcher = Watcher::spawn(move || listings.next().unwrap_or_else(|| Ok(running(&[]))), missed, |_| Ok(false), Duration::from_millis(1));
    assert_eq!(watcher.seen.recv_timeout(Duration::from_secs(5)).unwrap(), Seen::Closed("r::k".into()));
    assert!(watcher.seen.recv_timeout(Duration::from_millis(100)).is_err());
    assert_eq!(asks.try_iter().collect::<Vec<_>>(), [1, 1]);
}

#[test]
fn the_watcher_sends_each_worktree_whose_last_terminal_closed() {
    // Open in the first listing is only compared against; a failed listing is skipped.
    let open = |ids: &[&str]| Agents { open: ids.iter().map(|id| WorktreeId::from(*id)).collect(), ..running(&[]) };
    let mut listings = vec![Ok(open(&["r::a", "r::b"])), Err(Error::Failed("orca".into())), Ok(open(&["r::b", "r::c"])), Ok(open(&["r::c"]))].into_iter();
    let watcher = Watcher::spawn(move || listings.next().unwrap_or_else(|| Ok(open(&["r::c"]))), |_| Ok(Missed::default()), |_| Ok(false), Duration::from_millis(1));
    let seen = || watcher.seen.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!((seen(), seen()), (Seen::Closed("r::a".into()), Seen::Closed("r::b".into())));
    assert!(watcher.seen.recv_timeout(Duration::from_millis(100)).is_err());
}

#[test]
fn the_watcher_sends_each_terminal_that_closed() {
    // Its worktree still has others open, as when the owner's own session runs beside a
    // triage session in the primary checkout.
    let handles = |ids: &[&str]| Agents { open: ["r::main".into()].into(), terminals: ids.iter().map(|id| id.to_string()).collect(), ..running(&[]) };
    let mut listings = vec![Ok(handles(&["term_1", "term_2"])), Ok(handles(&["term_2"]))].into_iter();
    let watcher = Watcher::spawn(move || listings.next().unwrap_or_else(|| Ok(handles(&["term_2"]))), |_| Ok(Missed::default()), |_| Ok(false), Duration::from_millis(1));
    assert_eq!(watcher.seen.recv_timeout(Duration::from_secs(5)).unwrap(), Seen::Ended("term_1".into()));
    assert!(watcher.seen.recv_timeout(Duration::from_millis(100)).is_err());
}

#[test]
fn a_listing_states_each_agent_as_working_idle_or_waiting() {
    let set = |ids: &[&str]| ids.iter().map(|i| WorktreeId::from(*i)).collect::<HashSet<WorktreeId>>();
    let mut now = Agents { running: set(&["r::a", "r::b", "r::c"]), waiting: set(&["r::b"]), ..Agents::default() };
    now.idle.insert("r::c".into());
    let named = |states: Vec<(WorktreeId, Observed)>| states.into_iter().map(|(id, s)| format!("{id} {s:?}")).collect::<Vec<_>>();
    assert_eq!(named(states_of(&now)), ["r::a AgentWorking", "r::b AgentWaiting", "r::c AgentIdle"]);
    now.listed = Some(set(&["r::a", "r::b", "r::c", "r::d"]));
    assert_eq!(named(states_of(&now)), ["r::a AgentWorking", "r::b AgentWaiting", "r::c AgentIdle"]);
}

#[test]
fn an_agent_terminal_s_exit_lists_at_once() {
    // Only the backstop's hour would list again, but the terminal's wait answers first.
    let agent = |ids: &[&str]| Agents { terminals: ids.iter().map(|id| id.to_string()).collect(), agents: ids.iter().map(|id| id.to_string()).collect(), ..running(&[]) };
    let mut listings = vec![Ok(agent(&["term_1"])), Ok(agent(&[]))].into_iter();
    let watcher = Watcher::spawn(move || listings.next().unwrap_or_else(|| Ok(agent(&[]))), |_| Ok(Missed::default()), |handle| Ok(handle == "term_1"), Duration::from_secs(3600));
    assert_eq!(watcher.seen.recv_timeout(Duration::from_secs(5)).unwrap(), Seen::Ended("term_1".into()));
}

#[test]
fn a_terminal_still_listed_after_its_exit_is_not_waited_on_again() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let agent = Agents { terminals: ["term_1".to_string()].into(), agents: ["term_1".to_string()].into(), ..running(&[]) };
    let waits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&waits);
    let _watcher = Watcher::spawn(move || Ok(agent.clone()), |_| Ok(Missed::default()), move |_| Ok(counted.fetch_add(1, Ordering::SeqCst) < 100), Duration::from_millis(1));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(waits.load(Ordering::SeqCst), 1);
}

#[test]
fn the_end_of_a_terminal_listen_opened_is_said_though_no_listing_saw_it() {
    // It opens and closes between two listings: only its wait can say it ended, and once.
    let (close, closed) = mpsc::channel::<()>();
    let closed = Mutex::new(closed);
    let watcher = Watcher::spawn(|| Ok(running(&[])), |_| Ok(Missed::default()), move |_| Ok(closed.lock().unwrap().recv().is_ok()), Duration::from_secs(3600));
    watcher.nudge.send(Nudge::Own("term_9".into())).unwrap();
    close.send(()).unwrap();
    assert_eq!(watcher.seen.recv_timeout(Duration::from_secs(5)).unwrap(), Seen::Ended("term_9".into()));
    assert!(watcher.seen.recv_timeout(Duration::from_millis(100)).is_err());
}

#[test]
fn a_terminal_listen_opened_that_a_listing_saw_go_ends_once() {
    let handles = |ids: &[&str]| Agents { terminals: ids.iter().map(|id| id.to_string()).collect(), ..running(&[]) };
    let mut listings = vec![Ok(handles(&["term_9"])), Ok(handles(&[]))].into_iter();
    let watcher = Watcher::spawn(move || listings.next().unwrap_or_else(|| Ok(handles(&[]))), |_| Ok(Missed::default()), |_| Ok(true), Duration::from_secs(3600));
    watcher.nudge.send(Nudge::Own("term_9".into())).unwrap();
    assert_eq!(watcher.seen.recv_timeout(Duration::from_secs(5)).unwrap(), Seen::Ended("term_9".into()));
    assert!(watcher.seen.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn a_wait_that_returns_at_once_still_open_is_not_asked_again() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let agent = Agents { agents: ["term_1".to_string()].into(), ..running(&[]) };
    let waits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&waits);
    let _watcher = Watcher::spawn(move || Ok(agent.clone()), |_| Ok(Missed::default()), move |_| Ok(counted.fetch_add(1, Ordering::SeqCst) > 100), Duration::from_secs(3600));
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(waits.load(Ordering::SeqCst), 1);
}

#[test]
fn a_terminal_listen_opened_whose_wait_gives_up_after_a_listing_saw_it_go_still_ends() {
    let handles = |ids: &[&str]| Agents { terminals: ids.iter().map(|id| id.to_string()).collect(), ..running(&[]) };
    let (fail, failing) = mpsc::channel::<()>();
    let failing = Mutex::new(failing);
    let mut listings = vec![Ok(handles(&["term_9"])), Ok(handles(&[]))].into_iter();
    let watcher = Watcher::spawn(move || listings.next().unwrap_or_else(|| Ok(handles(&[]))), |_| Ok(Missed::default()), move |_| {
        let _ = failing.lock().unwrap().recv();
        Ok(false)
    }, Duration::from_secs(3600));
    // Taken in order: waited on as its own, then the listing that sees it go.
    watcher.nudge.send(Nudge::Own("term_9".into())).unwrap();
    watcher.nudge.send(Nudge::Exited(None)).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert!(watcher.seen.try_recv().is_err(), "the listing leaves it to its wait");
    fail.send(()).unwrap();
    assert_eq!(watcher.seen.recv_timeout(Duration::from_secs(5)).unwrap(), Seen::Ended("term_9".into()));
}

#[test]
fn an_own_terminal_whose_wait_says_exited_while_a_listing_has_it_is_waited_on_again() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let handles = |ids: &[&str]| Agents { terminals: ids.iter().map(|id| id.to_string()).collect(), ..running(&[]) };
    let waits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&waits);
    // Listed after the first wait's answer.
    let listings = Arc::clone(&waits);
    let watcher = Watcher::spawn(move || Ok(if listings.load(Ordering::SeqCst) < 2 { handles(&["term_9"]) } else { handles(&[]) }), |_| Ok(Missed::default()), move |_| Ok(counted.fetch_add(1, Ordering::SeqCst) < 1), Duration::from_secs(3600));
    watcher.nudge.send(Nudge::Own("term_9".into())).unwrap();
    assert!(watcher.seen.recv_timeout(Duration::from_millis(300)).is_err(), "the first answer ends nothing");
    // The second wait answers open.
    assert_eq!(waits.load(Ordering::SeqCst), 2);
}

#[test]
fn an_own_terminal_missing_from_a_fresh_listing_ends_once() {
    let handles = |ids: &[&str]| Agents { terminals: ids.iter().map(|id| id.to_string()).collect(), ..running(&[]) };
    let watcher = Watcher::spawn(move || Ok(handles(&[])), |_| Ok(Missed::default()), |_| Ok(true), Duration::from_secs(3600));
    watcher.nudge.send(Nudge::Own("term_9".into())).unwrap();
    assert_eq!(watcher.seen.recv_timeout(Duration::from_secs(5)).unwrap(), Seen::Ended("term_9".into()));
    assert!(watcher.seen.recv_timeout(Duration::from_millis(300)).is_err());
}

#[test]
fn an_own_terminal_is_not_ended_while_its_host_does_not_answer() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    // The first listing sees it; every one after fails.
    let handles = Agents { terminals: ["term_9".to_string()].into(), ..running(&[]) };
    let mut calls = 0;
    let waits = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&waits);
    let watcher = Watcher::spawn(move || {
        calls += 1;
        if calls == 1 { Ok(handles.clone()) } else { Err(Error::Orca("no answer".into())) }
    }, |_| Ok(Missed::default()), move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }, Duration::from_secs(3600));
    watcher.nudge.send(Nudge::Own("term_9".into())).unwrap();
    assert!(watcher.seen.recv_timeout(Duration::from_millis(300)).is_err());
    assert!(waits.load(Ordering::SeqCst) >= 2, "it is waited on again");
}

#[test]
fn an_own_terminal_a_listing_keeps_is_waited_on_a_few_times_then_left_to_the_listings() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let handles = Agents { terminals: ["term_9".to_string()].into(), ..running(&[]) };
    let (waits, lists) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (waited, listed) = (Arc::clone(&waits), Arc::clone(&lists));
    let watcher = Watcher::spawn(move || {
        listed.fetch_add(1, Ordering::SeqCst);
        Ok(handles.clone())
    }, |_| Ok(Missed::default()), move |_| {
        waited.fetch_add(1, Ordering::SeqCst);
        Ok(true)
    }, Duration::from_secs(3600));
    watcher.nudge.send(Nudge::Own("term_9".into())).unwrap();
    std::thread::sleep(Duration::from_millis(1000));
    assert!(watcher.seen.try_recv().is_err());
    assert_eq!(waits.load(Ordering::SeqCst), 4);
    // The first, four checks, and the one that has the listings hold it.
    assert_eq!(lists.load(Ordering::SeqCst), 6);
}
