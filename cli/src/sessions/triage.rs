// What triage reads of Linear: the queue, the issues asked about by ID, and each one's
// history of state types.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Value, json};
use ureq::Agent;

use crate::error::Error;
use crate::estimates::{Estimation, with_team_fields};
use crate::issue_state::WorkflowState;
use crate::issues::{Identified, Relations};
use crate::linear::{Nodes, PageInfo, Project, Ref, StateOf, read_as};
use crate::plain::{Id, IssueKey};

// What triage reads of an issue (`triage::parse_candidate`). A page of relations is few: a
// follow-up relates to the issue it was filed from, and the queue reads 250 issues at once.
const TRIAGE_ISSUE: &str = "identifier createdAt archivedAt state { type } team { key TEAM_FIELDS } project { id name } creator { id }
    relations(first: 3) { nodes { type relatedIssue { identifier } } } inverseRelations(first: 3) { nodes { type issue { identifier } } }";

// An issue as `TRIAGE_ISSUE` or `TRIAGED` reads it: the relations only in the first, the team's
// ID and states only in the second.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriageNode {
    pub identifier: String,
    pub created_at: String,
    pub archived_at: Option<String>,
    pub state: StateOf,
    pub team: TriageTeam,
    pub project: Option<Project>,
    pub creator: Option<Ref>,
    #[serde(flatten)]
    pub relations: Relations<Identified>,
}

#[derive(Debug, Deserialize)]
pub struct TriageTeam {
    pub id: Option<String>,
    pub key: String,
    #[serde(flatten)]
    pub estimation: Estimation,
    pub states: Option<Nodes<WorkflowState>>,
}

// What a verdict reads of the issue it is on, and of each issue it names.
const TRIAGED: &str = "query Triaged($id: String!) { viewer { id } issue(id: $id) {
  id identifier createdAt archivedAt state { id name type position } project { id name } creator { id }
  team { id key TEAM_FIELDS states(first: 250) { nodes { id name type position } } }
} }";
const OTHER: &str = "query Other($id: String!) { issue(id: $id) { id identifier archivedAt state { type } team { id } } }";

// An issue by its ID, with what `TRIAGE_ISSUE` or `TRIAGED` reads.
#[derive(Debug, Deserialize)]
pub struct WithId {
    pub id: String,
    #[serde(flatten)]
    pub issue: TriageNode,
}

#[derive(Debug, Deserialize)]
pub struct Triaged {
    pub viewer: Ref,
    pub issue: Option<WithId>,
}

#[track_caller]
pub fn read_triaged(agent: &Agent, id: &str) -> Result<Triaged, Error> {
    read_as(agent, &with_team_fields(TRIAGED), json!({ "id": id }))
}

// An issue a verdict names, as `may_relate` checks it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Other {
    pub id: String,
    pub identifier: String,
    pub archived_at: Option<String>,
    pub state: StateOf,
    pub team: Ref,
}

// None when Linear has no such issue.
#[track_caller]
pub fn read_other(agent: &Agent, id: &str) -> Result<Option<Other>, Error> {
    #[derive(Deserialize)]
    struct Answer {
        issue: Option<Other>,
    }
    Ok(read_as::<Answer>(agent, OTHER, json!({ "id": id }))?.issue)
}

// One page, Linear's largest, newest first: a queue longer than the page hides its oldest
// until the newer ones have moved, which a queue this is for never reaches.
const TRIAGE_QUEUE: &str = "query TriageQueue {
  viewer { id }
  issues(first: 250, filter: { state: { type: { eq: \"triage\" } } }, orderBy: createdAt) {
    nodes { TRIAGE_ISSUE }
  }
}";

pub fn read_triage_queue(agent: &Agent) -> Result<(Vec<TriageNode>, String), Error> {
    #[derive(Deserialize)]
    struct Answer {
        viewer: Ref,
        issues: Nodes<TriageNode>,
    }
    let answer: Answer = read_as(agent, &with_team_fields(&TRIAGE_QUEUE.replace("TRIAGE_ISSUE", TRIAGE_ISSUE)), json!({}))?;
    Ok((answer.issues.nodes, answer.viewer.id))
}

// The issues with these IDs, archived or deleted ones too, which the queue never lists but
// an issue asked about by ID may have become: `id` and what `TRIAGE_ISSUE` reads. An ID
// Linear has no issue for is left out. At most a page's worth of IDs.
const TRIAGE_ISSUES: &str = "query TriageIssues($ids: [ID!]) {
  issues(first: 250, includeArchived: true, filter: { id: { in: $ids } }) {
    nodes { id TRIAGE_ISSUE }
  }
}";

pub fn read_triage_issues(agent: &Agent, issue_ids: &[Id]) -> Result<Vec<WithId>, Error> {
    #[derive(Deserialize)]
    struct Answer {
        issues: Nodes<WithId>,
    }
    let answer: Answer = read_as(agent, &with_team_fields(&TRIAGE_ISSUES.replace("TRIAGE_ISSUE", TRIAGE_ISSUE)), json!({ "ids": issue_ids }))?;
    Ok(answer.issues.nodes)
}

// Linear's largest page: an issue in Triage with more history than this counts as moved
// (triage::filed_in_triage).
const STATE_HISTORY: u32 = 250;

// The most histories one `state_histories` asks for: each is a page of entries, and Linear
// refuses a query past its complexity limit, which many full pages would pass.
pub const HISTORIES_PER_READ: usize = 8;

fn history_selection() -> String {
    format!("history(first: {STATE_HISTORY}) {{ nodes {{ fromState {{ type }} toState {{ type }} }} pageInfo {{ hasNextPage }} }}")
}

// An issue's history as triage reads it: the state types each entry moved it between, null
// for an entry that changed no state, and whether there is more of it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateHistory {
    pub nodes: Vec<StateChange>,
    pub page_info: PageInfo,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateChange {
    pub from_state: Option<StateOf>,
    pub to_state: Option<StateOf>,
}

#[derive(Debug, Deserialize)]
struct HistoryOf {
    history: StateHistory,
}

// The issue's history as triage reads it: the state types each entry moved it between,
// nothing more. `identifier` is plain (triage::parse_candidate).
pub fn state_history(agent: &Agent, identifier: &str) -> Result<StateHistory, Error> {
    #[derive(Deserialize)]
    struct Answer {
        issue: Option<HistoryOf>,
    }
    let query = format!("query StateHistory($id: String!) {{ issue(id: $id) {{ {} }} }}", history_selection());
    let answer: Answer = read_as(agent, &query, json!({ "id": identifier }))?;
    answer.issue.map(|i| i.history).ok_or_else(|| Error::Failed(format!("Linear had no history for {identifier}")))
}

// `state_history` of up to `HISTORIES_PER_READ` issues in one query, in the order asked.
pub fn state_histories(agent: &Agent, identifiers: &[IssueKey]) -> Result<Vec<StateHistory>, Error> {
    let aliases: Vec<String> = (0..identifiers.len()).map(|i| format!("h{i}")).collect();
    let declared: Vec<String> = aliases.iter().map(|a| format!("${a}: String!")).collect();
    let asked: Vec<String> = aliases.iter().map(|a| format!("{a}: issue(id: ${a}) {{ {} }}", history_selection())).collect();
    let query = format!("query StateHistories({}) {{ {} }}", declared.join(", "), asked.join(" "));
    let variables: serde_json::Map<String, Value> = aliases.iter().zip(identifiers).map(|(a, id)| (a.clone(), json!(id))).collect();
    // Keyed by alias, each null when Linear has no such issue.
    let mut answer: HashMap<String, Option<HistoryOf>> = read_as(agent, &query, Value::Object(variables))?;
    aliases
        .iter()
        .zip(identifiers)
        .map(|(alias, identifier)| answer.remove(alias).flatten().map(|i| i.history).ok_or_else(|| Error::Failed(format!("Linear had no history for {identifier}"))))
        .collect()
}

#[cfg(test)]
mod tests;
