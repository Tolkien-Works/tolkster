// `host offline`: the label of the workspace group `tolkster` that a worker's issue carries
// while its host doesn't answer (docs/dispatcher.md). Display only: nothing reads it back, and
// Tolkster's own state is the Durable Object's. `health::Health` owes the edits and `settle`
// makes them, on a thread of `listen`'s of its own.
//
// A Linear group allows one label of its own per issue, so the group holds only states; an
// owner's flags stay outside it, and no edit here names one.

use std::time::Instant;

use serde::Deserialize;
use serde_json::{Value, json};
use ureq::Agent;

use crate::error::Error;
use crate::health::{Health, Mark};
use crate::linear::{Nodes, Ref, identifier_filter, read_as, update_issue, with_token};
use crate::plain::{Id, IssueKey};
use crate::state::WorkerRecord;
use crate::trace;

pub const GROUP: &str = "tolkster";
pub const HOST_OFFLINE: &str = "host offline";

// The group's own ID, and its labels by name and ID.
pub struct Group {
    pub id: String,
    labels: Vec<(String, String)>,
}

// What an edit changes on an issue: the IDs of labels to add and to remove.
#[derive(Debug, PartialEq)]
pub struct Edit {
    pub add: Vec<String>,
    pub remove: Vec<String>,
}

// What `LABELS` reads: the workspace's labels named GROUP and their children.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Labels {
    pub issue_labels: Nodes<GroupLabel>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupLabel {
    pub id: String,
    pub name: String,
    pub is_group: bool,
    pub parent: Option<Ref>,
}

impl Group {
    // The workspace label named GROUP that is a group, with its children. None without one.
    pub fn parse(data: &Labels) -> Option<Group> {
        let labels = &data.issue_labels.nodes;
        let group = labels.iter().find(|l| l.is_group && l.name.trim().eq_ignore_ascii_case(GROUP))?;
        let children = labels.iter().filter(|l| l.parent.as_ref().is_some_and(|p| p.id == group.id));
        Some(Group { id: group.id.clone(), labels: children.map(|l| (l.name.clone(), l.id.clone())).collect() })
    }

    pub fn has(&self, state: &str) -> bool {
        self.id_of(state).is_some()
    }

    fn id_of(&self, state: &str) -> Option<&str> {
        self.labels.iter().find(|(name, _)| name.trim().eq_ignore_ascii_case(state)).map(|(_, id)| id.as_str())
    }

    // `state` is the issue's: it is added and the group's other labels are removed. None when
    // the group has no such label.
    pub fn set(&self, state: &str) -> Option<Edit> {
        let id = self.id_of(state)?;
        Some(Edit { add: vec![id.to_string()], remove: self.labels.iter().map(|(_, other)| other.clone()).filter(|other| other != id).collect() })
    }

    // `state` no longer holds: only its label is removed, never another of the group's.
    pub fn clear(&self, state: &str) -> Option<Edit> {
        Some(Edit { add: Vec::new(), remove: vec![self.id_of(state)?.to_string()] })
    }

    fn edit(&self, mark: Mark) -> Option<Edit> {
        match mark {
            Mark::Offline => self.set(HOST_OFFLINE),
            Mark::Clear => self.clear(HOST_OFFLINE),
        }
    }
}

impl Edit {
    fn input(&self) -> Value {
        let mut input = serde_json::Map::new();
        if !self.add.is_empty() {
            input.insert("addedLabelIds".into(), json!(self.add));
        }
        if !self.remove.is_empty() {
            input.insert("removedLabelIds".into(), json!(self.remove));
        }
        Value::Object(input)
    }
}

// What the labels are read and written through, so a test needs no Linear.
pub trait Issues {
    fn group(&self) -> Result<Option<Group>, Error>;
    // The IDs of the issues with these identifiers.
    fn ids(&self, identifiers: &[IssueKey]) -> Result<Vec<Id>, Error>;
    // Adding or removing a label again changes nothing, so a retry is safe.
    fn edit(&self, issue_id: &Id, edit: &Edit) -> Result<(), Error>;
}

// Linear as the app.
pub struct App<'a>(pub &'a Agent);

// The workspace's labels named GROUP and their children, by name, so a workspace of many labels
// can't page the group out of the read, and `init` create it twice.
const LABELS: &str = "query Group($name: String!) { issueLabels(first: 250, filter: { team: { null: true }, or: [{ name: { eqIgnoreCase: $name } }, { parent: { name: { eqIgnoreCase: $name } } }] }) { nodes { id name isGroup parent { id } } } }";

pub fn group(agent: &Agent) -> Result<Option<Group>, Error> {
    Ok(Group::parse(&read_as(agent, LABELS, json!({ "name": GROUP }))?))
}

impl Issues for App<'_> {
    fn group(&self) -> Result<Option<Group>, Error> {
        group(self.0)
    }
    fn ids(&self, identifiers: &[IssueKey]) -> Result<Vec<Id>, Error> {
        #[derive(Deserialize)]
        struct Answer {
            issues: Nodes<Ref>,
        }
        let answer: Answer = read_as(self.0, "query Ids($filter: IssueFilter!) { issues(first: 250, filter: $filter) { nodes { id } } }", json!({ "filter": identifier_filter(identifiers) }))?;
        Ok(answer.issues.nodes.iter().filter_map(|n| Id::parse(&n.id)).collect())
    }
    fn edit(&self, issue_id: &Id, edit: &Edit) -> Result<(), Error> {
        with_token(self.0, |token| update_issue(self.0, token, issue_id, edit.input()))
    }
}

// One pass of `listen`'s labelling, every `health::TICK`: each edit owed at `now` is made to
// the issues of the workers the records put on that host, and the records and the group are
// read only when one is owed. A failed edit is owed again after `health::MARK_AGAIN`; the
// issues it did reach are edited again, which changes nothing. Without the group, or without
// its `host offline`, nothing is labelled and the edit counts as made.
pub fn settle(health: &Health, now: Instant, workers: &dyn Fn() -> Result<Vec<WorkerRecord>, Error>, linear: &dyn Issues) {
    let owed = health.owed(now);
    if owed.is_empty() {
        return;
    }
    let facts = workers().and_then(|records| Ok((records, linear.group()?)));
    for (host, mark) in owed {
        let made = facts.as_ref().map_err(Clone::clone).and_then(|(records, group)| label(&host, mark, records, group.as_ref(), &|at| health.silent(at), linear));
        match made {
            Ok(()) => health.marked(&host, mark),
            Err(error) => {
                health.unmarked(&host, mark, now);
                trace::event(trace::Host::NotMarked { id: host, error });
            }
        }
    }
}

fn label(host: &str, mark: Mark, records: &[WorkerRecord], group: Option<&Group>, silent: &dyn Fn(&str) -> bool, linear: &dyn Issues) -> Result<(), Error> {
    let Some(edit) = group.and_then(|group| group.edit(mark)) else { return Ok(()) };
    let on_host = |r: &&WorkerRecord| r.worktree_id.host() == Some(host);
    // An issue with a worker on another host that is silent keeps its label when this one is heard.
    let also_silent = |issue: &str| records.iter().any(|r| r.issue == issue && r.worktree_id.host().is_some_and(|at| at != host && silent(at)));
    let identifiers: Vec<IssueKey> = records.iter().filter(on_host).filter_map(|r| IssueKey::parse(&r.issue)).filter(|issue| mark == Mark::Offline || !also_silent(issue)).collect();
    if identifiers.is_empty() {
        return Ok(());
    }
    // Every issue is tried, so one Linear refuses never keeps the rest of the host's as they were.
    let mut failed = Ok(());
    for id in linear.ids(&identifiers)? {
        if let Err(error) = linear.edit(&id, &edit) {
            failed = failed.and(Err(error));
        }
    }
    failed
}

#[cfg(test)]
mod tests;
