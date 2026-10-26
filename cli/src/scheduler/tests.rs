use super::*;
use super::lanes::ISSUE_LANES;
use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::sleep;
use crate::machine::{Finished, Observed};
use crate::watch::{Nudge, Since};

// A job of the shared lane, named `name`: a terminal's end.
fn on(name: &str) -> Job {
    Job::Ended { terminal: name.into() }
}

// What a scheduler's test acts on reports: the terminal, the issue, or the command.
fn named(job: &Job) -> String {
    match job {
        Job::Base { repo, reference, .. } => format!("{repo}@{reference}"),
        Job::Closed { worktree_id } => format!("closed {worktree_id}"),
        Job::Ended { terminal } => terminal.clone(),
        Job::Update => "update".into(),
        Job::Command(command) => command.id.to_string(),
    }
}

// Queues `job` as the line `line`.
fn queue(scheduler: &mut Scheduler, id: &str, line: &str, job: Job) {
    assert!(scheduler.queue(id.into(), line.into(), job));
}

// Waits until nothing is in flight, or five seconds pass.
fn settled(scheduler: &mut Scheduler) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !scheduler.in_flight.is_empty() && Instant::now() < deadline {
        assert!(scheduler.collect() && scheduler.release_held());
        sleep(Duration::from_millis(10));
    }
}

// A job in an issue's lane, named for the PR the tests once took.
fn pr(number: u64) -> Job {
    on_issue(&format!("pr-{number}"), &format!("PR-{number}"))
}

// Collects until a job begins, as `rx` hears, or five seconds pass.
fn began<T>(scheduler: &mut Scheduler, rx: &mpsc::Receiver<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(began) = rx.try_recv() {
            return began;
        }
        assert!(scheduler.collect() && Instant::now() < deadline);
        sleep(Duration::from_millis(10));
    }
}

// A command of the Durable Object's on `issue`, named `id`, in the issue's lane.
fn on_issue(id: &str, issue: &str) -> Job {
    Job::Command(Command { id: id.into(), kind: crate::machine::Kind::ReadIssue, issue_id: "i1".into(), identifier: issue.into(), version: 1, urgent: false, order: Ok(crate::machine::Order::ReadIssue), queued: None })
}

// Acts by waiting for `gates`' release of the job named, and reports each start and end.
fn gated() -> (Scheduler, mpsc::Sender<String>, mpsc::Receiver<String>) {
    let (open, gates) = mpsc::channel::<String>();
    let (tx, rx) = mpsc::channel();
    let gates = Mutex::new((gates, HashSet::new()));
    let scheduler = Scheduler::spawn(Arc::default(), move |_, job, _| {
        let name = named(job);
        tx.send(format!("start {name}")).unwrap();
        loop {
            let mut gates = gates.lock().unwrap();
            if gates.1.remove(&name) {
                break;
            }
            if let Ok(opened) = gates.0.recv_timeout(Duration::from_millis(5)) {
                gates.1.insert(opened);
            }
            // Unlocked for a moment, so a waiting job gets the lock instead of this one taking it again.
            drop(gates);
            sleep(Duration::from_millis(1));
        }
        tx.send(format!("end {name}")).unwrap();
    });
    (scheduler, open, rx)
}

fn next(rx: &mpsc::Receiver<String>) -> String {
    rx.recv_timeout(Duration::from_secs(5)).unwrap()
}

fn command(id: &str, identifier: &str, urgent: bool) -> Command {
    let kind = if urgent { "stopWorker" } else { "readIssue" };
    let line = serde_json::json!({ "type": "command", "id": id, "kind": kind, "issueId": "i1", "identifier": identifier, "version": 1, "urgent": urgent, "args": { "sessionId": "s-1" } });
    crate::machine::parse_command(&line).unwrap().unwrap()
}

// The IDs of the results `step` sends, until `count` have come or a while has passed.
fn results(scheduler: &mut Scheduler, count: usize, within: Duration) -> Vec<String> {
    let (mut sent, deadline) = (Vec::new(), Instant::now() + within);
    while sent.len() < count && Instant::now() < deadline {
        assert!(scheduler.step(|line| { sent.push(serde_json::from_str::<serde_json::Value>(&line).unwrap()["id"].as_str().unwrap().to_string()); Ok::<(), String>(()) }).is_none());
        sleep(Duration::from_millis(10));
    }
    sent
}

// A command answered as soon as it runs, counting each run, except "slow", which waits for
// the gate first.
fn commanding() -> (Scheduler, Arc<AtomicU64>, mpsc::Sender<()>) {
    let (runs, gate) = (Arc::new(AtomicU64::new(0)), mpsc::channel::<()>());
    let (counted, waiting) = (Arc::clone(&runs), Mutex::new(gate.1));
    let scheduler = Scheduler::spawn(Arc::default(), move |_, job, _| {
        if let Job::Command(command) = job {
            counted.fetch_add(1, Ordering::SeqCst);
            if command.id == "slow" {
                waiting.lock().unwrap().recv().unwrap();
            }
            answer(command, &Finished::ok(serde_json::json!({})));
        }
    });
    (scheduler, runs, gate.0)
}

// The terminals a job's closes report as `terminalClosed`, in the order `step` sends them.
fn closed(steps: fn()) -> Vec<String> {
    let mut scheduler = Scheduler::spawn(Arc::default(), move |_, job, _| {
        if let Job::Command(command) = job {
            steps();
            answer(command, &Finished::ok(serde_json::json!({})));
        }
    });
    scheduler.connected();
    assert!(scheduler.take_command(command("c-1", "TW-1", false)));
    let (mut sent, deadline) = (Vec::new(), Instant::now() + Duration::from_secs(5));
    while !sent.iter().any(|v: &serde_json::Value| v["report"] == "commandResult") && Instant::now() < deadline {
        assert!(scheduler.step(|line| { sent.push(serde_json::from_str(&line).unwrap()); Ok::<(), String>(()) }).is_none());
        sleep(Duration::from_millis(10));
    }
    sent.iter().filter(|v| v["report"] == "terminalClosed").map(|v| v["terminal"].as_str().unwrap().to_string()).collect()
}


mod commands;
mod jobs;
mod lanes;
mod restarts;
