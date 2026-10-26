// The coordinator's frame, as the relay sends it on connect and on each change
// (src/worker/machine/frame.ts): the slot ledger, the queue in its order, each held issue's
// deadline and PR, the parked work, the parent chains, the triage machine and why the fleet asks
// for no machine per repo. Level state, read by the panel and the log alone, never by what
// `listen` carries out. test/contract.json's `frame`
// holds one. Times are the Durable Object's clock's milliseconds, and Linear's for a
// request's `at`.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use crate::machine::Waiting;
use crate::plain::{plain_host, IssueKey};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Work {
    Launching,
    Running,
    Idle,
    Held,
    Exited,
    Rest,
    Halted,
    Merged,
    Reaped,
}

impl Work {
    pub fn name(self) -> &'static str {
        match self {
            Work::Launching => "launching",
            Work::Running => "running",
            Work::Idle => "idle",
            Work::Held => "held",
            Work::Exited => "exited",
            Work::Rest => "rest",
            Work::Halted => "halted",
            Work::Merged => "merged",
            Work::Reaped => "reaped",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Checks {
    Passed,
    Failed,
    CouldntRun,
    Pending,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MergeState {
    Clean,
    Behind,
    Dirty,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeadlineKind {
    Own,
    Recheck,
    TakeBack,
    Quiet,
    Warm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TriageState {
    Idle,
    Starting,
    Running,
    Retrying,
}

// `capped`: its review rounds are spent at its head. `auto_merge`: GitHub's, None until a read
// of the PR shows it.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pr {
    pub number: u64,
    pub checks: Checks,
    pub merge_state: Option<MergeState>,
    pub rounds: u32,
    pub capped: bool,
    pub auto_merge: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deadline {
    pub kind: DeadlineKind,
    pub due_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Slot {
    pub identifier: Option<String>,
    pub reserved: bool,
    pub since_ms: i64,
    // The paired host its worker started on; None on this machine or before a start named one.
    pub host: Option<String>,
    pub work: Option<Work>,
    // Its agent's hook says it waits on a call in the background; absent from an older Worker.
    #[serde(default)]
    pub background: bool,
    pub deadline: Option<Deadline>,
    pub pr: Option<Pr>,
    // Held for an issue the store has seen completed or cancelled.
    pub anomaly: bool,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Queued {
    pub identifier: Option<String>,
    pub waiting: Waiting,
    pub blockers: Vec<String>,
    pub at: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Parked {
    pub identifier: Option<String>,
    pub work: Work,
    pub stalled: bool,
    pub pr: Option<Pr>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chain {
    pub identifier: Option<String>,
    pub issues: u32,
    pub answered: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Triage {
    pub state: TriageState,
    pub due_ms: Option<i64>,
    pub failures: u32,
}

// Why the fleet's last reconcile asked for no machine for a repo whose request waits
// (src/worker/machine/fleet.ts, REASONS).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Why {
    HereHasRoom,
    HostHasRoom,
    Running,
    HostComing,
    AllMade,
    CoolingDown,
    NoRecipe,
}

#[cfg(test)]
pub const WHYS: [Why; 7] = [Why::HereHasRoom, Why::HostHasRoom, Why::Running, Why::HostComing, Why::AllMade, Why::CoolingDown, Why::NoRecipe];

impl Why {
    pub fn said(self) -> &'static str {
        match self {
            Why::HereHasRoom => "this machine has room",
            Why::HostHasRoom => "a host has room",
            Why::Running => "a make or wake is running",
            Why::HostComing => "a machine of it went silent moments ago",
            Why::AllMade => "its machines are made",
            Why::CoolingDown => "cooling down after a failure",
            Why::NoRecipe => "it has no recipe",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    pub cap: i64,
    pub slots: Vec<Slot>,
    pub queue: Vec<Queued>,
    pub parked: Vec<Parked>,
    pub chains: Vec<Chain>,
    pub triage: Triage,
    // By repo: its `repos` name, or the Orca ID of one with no recipe.
    #[serde(default)]
    pub why: BTreeMap<String, Why>,
}

// None for anything but a frame; Err with why one is skipped. An identifier or host that isn't
// plain is dropped, as one the Durable Object doesn't know is.
pub fn parse(value: &Value) -> Option<Result<Frame, String>> {
    if value.get("type")?.as_str()? != "frame" {
        return None;
    }
    let plain = |i: &mut Option<String>| *i = i.take().filter(|i| IssueKey::parse(i).is_some());
    Some(Frame::deserialize(value).map_err(|_| "a frame of the wrong shape".to_string()).map(|mut frame| {
        for s in &mut frame.slots {
            plain(&mut s.identifier);
            s.host = s.host.take().filter(|h| plain_host(h));
        }
        frame.parked.iter_mut().for_each(|p| plain(&mut p.identifier));
        frame.chains.iter_mut().for_each(|c| plain(&mut c.identifier));
        for q in &mut frame.queue {
            plain(&mut q.identifier);
            q.blockers.retain(|b| IssueKey::parse(b).is_some());
        }
        frame.why.retain(|repo, _| crate::hosts::plain_name(repo));
        frame
    }))
}
