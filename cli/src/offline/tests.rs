use std::cell::{Cell, RefCell};
use std::time::Duration;

use super::*;
use crate::health::{AGAIN, MARK_AGAIN, ask_again};
use crate::config::ladder::Launch;
use crate::state::RungFrom;

// The workspace's labels: the group with two states, and the owner's flags outside it.
fn labels() -> Value {
    json!({ "issueLabels": { "nodes": [
        { "id": "g", "name": "tolkster", "isGroup": true, "parent": null },
        { "id": "off", "name": "Host offline", "isGroup": false, "parent": { "id": "g" } },
        { "id": "other", "name": "paused", "isGroup": false, "parent": { "id": "g" } },
        { "id": "pf", "name": "plan-first", "isGroup": false, "parent": null },
        { "id": "mm", "name": "manual-merge", "isGroup": false, "parent": null },
    ] } })
}

fn record(issue: &str, worktree_id: &str) -> WorkerRecord {
    WorkerRecord { issue: issue.into(), worktree_id: worktree_id.into(), session_id: "s".into(), at: "2026-10-20T00:00:00.000Z".into(), launch: Launch::new(crate::harnesses::DEFAULT, "claude-opus-5-5", "medium"), rung_from: RungFrom::Claim }
}

// Workers on the host `mac`, on `pi` and here.
fn records() -> Vec<WorkerRecord> {
    vec![record("TW-1", "mac@r::/wt/tw-1"), record("TW-2", "pi@r::/wt/tw-2"), record("TW-3", "r::/wt/tw-3"), record("TW-4", "mac@r::/wt/tw-4")]
}

type Made = (String, Vec<String>, Vec<String>);

// Linear with the group in `labels()`: every edit is kept, as (issue ID, add, remove), and the
// first `fail` of them fail. It can't be asked what an issue carries.
struct Fake {
    group: bool,
    edits: RefCell<Vec<Made>>,
    fail: Cell<usize>,
}

impl Fake {
    fn new() -> Fake {
        Fake { group: true, edits: RefCell::default(), fail: Cell::new(0) }
    }
    fn made(&self) -> Vec<Made> {
        self.edits.take()
    }
}

impl Issues for Fake {
    fn group(&self) -> Result<Option<Group>, Error> {
        Ok(Group::parse(&crate::json::decode(labels(), "Linear's answer to Group").unwrap()).filter(|_| self.group))
    }
    fn ids(&self, identifiers: &[IssueKey]) -> Result<Vec<Id>, Error> {
        Ok(identifiers.iter().map(|i| Id::from(format!("id-{i}").as_str())).collect())
    }
    fn edit(&self, issue_id: &Id, edit: &Edit) -> Result<(), Error> {
        if self.fail.get() > 0 {
            self.fail.set(self.fail.get() - 1);
            return Err(Error::Failed("Linear is down".into()));
        }
        self.edits.borrow_mut().push((issue_id.to_string(), edit.add.clone(), edit.remove.clone()));
        Ok(())
    }
}

fn settled(health: &Health, now: Instant, linear: &Fake) {
    settle(health, now, &|| Ok(records()), linear);
}

fn strings(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_state_replaces_the_groups_others_and_never_touches_the_owners_flags() {
    let group = Group::parse(&crate::json::decode(labels(), "Linear's answer to Group").unwrap()).unwrap();
    assert_eq!(group.set(HOST_OFFLINE), Some(Edit { add: strings(&["off"]), remove: strings(&["other"]) }));
    // Clearing one removes that label alone, not another state the issue has since taken.
    assert_eq!(group.clear(HOST_OFFLINE), Some(Edit { add: Vec::new(), remove: strings(&["off"]) }));
    assert_eq!(group.set("no such state"), None);
    for edit in [group.set(HOST_OFFLINE).unwrap(), group.clear(HOST_OFFLINE).unwrap()] {
        assert!(!edit.add.iter().chain(&edit.remove).any(|id| id == "pf" || id == "mm"));
    }
    // A label named like the group that isn't one, or a group of another name, is no group.
    assert!(Group::parse(&crate::json::decode(json!({ "issueLabels": { "nodes": [{ "id": "x", "name": "tolkster", "isGroup": false, "parent": null }] } }), "Linear's answer to Group").unwrap()).is_none());
}

#[test]
fn a_silent_hosts_workers_gain_the_label_and_lose_it_when_the_host_answers() {
    let (health, linear, start) = (Health::default(), Fake::new(), Instant::now());
    settled(&health, start, &linear);
    assert!(linear.made().is_empty());

    health.stopped("mac", None, start);
    settled(&health, start, &linear);
    // Only the host's own workers, once; the others' hosts and this machine's are left alone.
    assert_eq!(linear.made(), [("id-TW-1".into(), strings(&["off"]), strings(&["other"])), ("id-TW-4".into(), strings(&["off"]), strings(&["other"]))]);
    settled(&health, start, &linear);
    assert!(linear.made().is_empty(), "made again with nothing owed");

    ask_again(&health, || Some(strings(&["mac"])), |_| true, || start + AGAIN);
    settled(&health, start + AGAIN, &linear);
    assert_eq!(linear.made(), [("id-TW-1".into(), Vec::new(), strings(&["off"])), ("id-TW-4".into(), Vec::new(), strings(&["off"]))]);
    settled(&health, start + AGAIN, &linear);
    assert!(linear.made().is_empty());
}

#[test]
fn a_host_heard_after_a_restart_is_cleared_once() {
    let (health, linear, start) = (Health::default(), Fake::new(), Instant::now());
    // A restart forgot that `mac` was silent: its first listing that hears it owes a clear.
    health.heard("mac", start);
    settled(&health, start, &linear);
    assert_eq!(linear.made().len(), 2);
    health.heard("mac", start);
    health.heard("mac", start);
    settled(&health, start, &linear);
    assert!(linear.made().is_empty(), "every listing cleared it again");
    // A host that is silent now isn't heard by a listing that raced its failure.
    health.stopped("pi", None, start);
    health.heard("pi", start);
    settled(&health, start, &linear);
    assert_eq!(linear.made(), [("id-TW-2".into(), strings(&["off"]), strings(&["other"]))]);
}

#[test]
fn a_failed_edit_is_owed_again_after_a_pause_and_the_other_way_wins_over_one_in_flight() {
    let (health, linear, start) = (Health::default(), Fake::new(), Instant::now());
    health.stopped("mac", None, start);
    linear.fail.set(1);
    settled(&health, start, &linear);
    assert_eq!(linear.made().len(), 1);
    // Not before the pause; then the whole host again, which changes nothing on the issues it reached.
    settled(&health, start + MARK_AGAIN - Duration::from_secs(1), &linear);
    assert!(linear.made().is_empty());
    settled(&health, start + MARK_AGAIN, &linear);
    assert_eq!(linear.made().len(), 2);

    // Answering while an offline edit is in flight: that one lands, and the clear still follows.
    health.stopped("pi", None, start);
    let owed = health.owed(start);
    assert_eq!(owed, [("pi".to_string(), Mark::Offline)]);
    ask_again(&health, || Some(strings(&["pi"])), |_| true, || start + AGAIN);
    health.marked("pi", Mark::Offline);
    assert_eq!(health.owed(start + AGAIN), [("pi".to_string(), Mark::Clear)]);
}

#[test]
fn records_that_cant_be_read_leave_every_edit_owed() {
    let (health, linear, start) = (Health::default(), Fake::new(), Instant::now());
    health.stopped("mac", None, start);
    settle(&health, start, &|| Err(Error::Later("the Durable Object is down".into())), &linear);
    assert!(linear.made().is_empty() && health.owed(start).is_empty() && health.owed(start + MARK_AGAIN).len() == 1);
}

#[test]
fn without_the_group_nothing_is_labelled_and_nothing_stays_owed() {
    let (health, linear, start) = (Health::default(), Fake { group: false, ..Fake::new() }, Instant::now());
    health.stopped("mac", None, start);
    settled(&health, start, &linear);
    assert!(linear.made().is_empty() && health.owed(start + MARK_AGAIN).is_empty());
}

// One issue Linear refuses doesn't hold back the host's others, and an issue that also has a
// worker on a silent host keeps the label when the first host is heard.
#[test]
fn one_refused_issue_leaves_the_rest_edited_and_a_silent_second_host_keeps_the_label() {
    let (health, linear, start) = (Health::default(), Fake::new(), Instant::now());
    health.stopped("mac", None, start);
    linear.fail.set(1);
    settled(&health, start, &linear);
    assert_eq!(linear.made().len(), 1, "TW-4 is edited though TW-1 was refused");

    let both = || Ok(vec![record("TW-1", "mac@r::/wt/tw-1"), record("TW-1", "pi@r::/wt/tw-5"), record("TW-4", "mac@r::/wt/tw-4")]);
    let (health, start) = (Health::default(), Instant::now());
    health.stopped("pi", None, start);
    health.heard("mac", start);
    settle(&health, start, &both, &linear);
    // TW-4 is cleared; TW-1 is labelled for `pi` and not cleared for `mac`.
    let mut made = linear.made();
    made.sort();
    assert_eq!(made, [("id-TW-1".into(), strings(&["off"]), strings(&["other"])), ("id-TW-4".into(), Vec::new(), strings(&["off"]))]);
}
