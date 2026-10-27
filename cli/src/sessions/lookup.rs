// What subcommands read of issues and sessions before they write: the issue a write
// targets, and what decides whether it may.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Value, json};
use ureq::Agent;

use crate::error::Error;
use crate::estimates::with_team_fields;
use crate::issues::{Identified, Relations, RELATION_FIELDS};
use crate::linear::{IssueRef, Nodes, Ref, StateOf, graphql, read_as, read_with};
use crate::sessions::workspace::FilingTeam;
use crate::sessions::{OfApp, Through, issue_sessions};

// `{ issue }`, null when Linear has no such issue.
#[derive(Deserialize)]
struct Issue<T> {
    issue: Option<T>,
}

// The issue `id`, by its ID and identifier: Err when Linear has none.
#[track_caller]
pub fn issue_ref(agent: &Agent, token: &str, id: &str) -> Result<IssueRef, Error> {
    let found: Issue<IssueRef> = read_with(agent, token, "query Target($id: String!) { issue(id: $id) { id identifier } }", json!({ "id": id }))?;
    found.issue.ok_or_else(|| Error::Failed(format!("no issue {id}")))
}

// The issues `ids` names, in their order, each None when Linear has none.
#[track_caller]
pub fn issues_by_id(agent: &Agent, token: &str, ids: &[&str]) -> Result<Vec<Option<IssueRef>>, Error> {
    let declared: Vec<String> = (0..ids.len()).map(|i| format!("$o{i}: String!")).collect();
    let reads: Vec<String> = (0..ids.len()).map(|i| format!("o{i}: issue(id: $o{i}) {{ id identifier }}")).collect();
    let variables: serde_json::Map<String, Value> = ids.iter().enumerate().map(|(i, id)| (format!("o{i}"), json!(id))).collect();
    // Keyed by alias, `o0` the first.
    let mut found: HashMap<String, Option<IssueRef>> = read_with(agent, token, &format!("query Others({}) {{ {} }}", declared.join(", "), reads.join(" ")), Value::Object(variables))?;
    Ok((0..ids.len()).map(|i| found.remove(&format!("o{i}")).flatten()).collect())
}

// An issue with its relations, as `RELATION_FIELDS` reads them.
#[derive(Debug, Deserialize)]
pub struct Related {
    pub id: String,
    #[serde(flatten)]
    pub relations: Relations<Identified>,
}

#[derive(Debug, Deserialize)]
pub struct Pair {
    pub a: Related,
    pub b: IssueRef,
}

// `a` with its relations, to find one to `b`, and `b`.
#[track_caller]
pub fn relation_pair(agent: &Agent, token: &str, a: &str, b: &str) -> Result<Pair, Error> {
    #[derive(Deserialize)]
    struct Answer {
        a: Option<Related>,
        b: Option<IssueRef>,
    }
    let query = format!("query Pair($a: String!, $b: String!) {{ a: issue(id: $a) {{ id identifier {RELATION_FIELDS} }} b: issue(id: $b) {{ id identifier }} }}");
    let found: Answer = read_with(agent, token, &query, json!({ "a": a, "b": b }))?;
    Ok(Pair { a: found.a.ok_or_else(|| Error::Failed(format!("no issue {a}")))?, b: found.b.ok_or_else(|| Error::Failed(format!("no issue {b}")))? })
}

// What `issue update` reads of its issue, and the viewer.
#[derive(Debug, Deserialize)]
pub struct UpdateTarget {
    pub viewer: Ref,
    pub issue: Target,
}

#[derive(Debug, Deserialize)]
pub struct Target {
    pub id: String,
    pub identifier: String,
    pub team: FilingTeam,
    pub parent: Option<Ref>,
    pub state: StateOf,
    pub delegate: Option<Ref>,
}

// What `issue update` reads of its issue. Its team's estimate scale (estimates::TEAM_FIELDS)
// checks an --estimate.
#[track_caller]
pub fn update_target(agent: &Agent, token: &str, id: &str) -> Result<UpdateTarget, Error> {
    #[derive(Deserialize)]
    struct Answer {
        viewer: Ref,
        issue: Option<Target>,
    }
    let query = with_team_fields("query Target($id: String!) { viewer { id } issue(id: $id) { id identifier team { id TEAM_FIELDS } parent { id } state { type } delegate { id } } }");
    let found: Answer = read_with(agent, token, &query, json!({ "id": id }))?;
    Ok(UpdateTarget { viewer: found.viewer, issue: found.issue.ok_or_else(|| Error::Failed(format!("no issue {id}")))? })
}

// The issue a new one follows from, and the viewer.
#[derive(Debug, Deserialize)]
pub struct FilingContext {
    pub viewer: Ref,
    pub issue: Context,
}

#[derive(Debug, Deserialize)]
pub struct Context {
    pub id: String,
    pub team: ContextTeam,
    pub project: Option<Ref>,
    pub delegate: Option<Ref>,
}

#[derive(Debug, Deserialize)]
pub struct ContextTeam {
    pub key: String,
    #[serde(flatten)]
    pub team: FilingTeam,
}

// The issue a new one follows from, for its team, estimate scale and project.
#[track_caller]
pub fn filing_context(agent: &Agent, token: &str, id: &str) -> Result<FilingContext, Error> {
    #[derive(Deserialize)]
    struct Answer {
        viewer: Ref,
        issue: Option<Context>,
    }
    let query = with_team_fields("query Context($id: String!) { viewer { id } issue(id: $id) { id team { id key TEAM_FIELDS } project { id } delegate { id } } }");
    let found: Answer = read_with(agent, token, &query, json!({ "id": id }))?;
    Ok(FilingContext { viewer: found.viewer, issue: found.issue.ok_or_else(|| Error::Failed(format!("no issue {id}")))? })
}

// An issue, its parent's delegate, its sub-issues', which a team's auto-close cancels with
// it, and the issues it blocks with theirs.
const MY_WORK: &str = "query Work($id: String!) { viewer { id } issue(id: $id) { delegate { id } parent { delegate { id } } children(first: 50) { nodes { delegate { id } } pageInfo { hasNextPage } } blocking: relations(first: 50) { nodes { type relatedIssue { delegate { id } parent { delegate { id } } } } pageInfo { hasNextPage } } } }";

#[derive(Debug, Deserialize)]
pub struct MyWork {
    pub viewer: Ref,
    pub issue: Option<Work>,
}

#[derive(Debug, Deserialize)]
pub struct Work {
    pub delegate: Option<Ref>,
    pub parent: Option<ByDelegate>,
    pub children: Nodes<ByDelegate>,
    pub blocking: Nodes<Blocking>,
}

// An issue by its delegate alone.
#[derive(Debug, Deserialize)]
pub struct ByDelegate {
    pub delegate: Option<Ref>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Blocking {
    #[serde(rename = "type")]
    pub kind: String,
    pub related_issue: Option<Blocked>,
}

#[derive(Debug, Deserialize)]
pub struct Blocked {
    pub delegate: Option<Ref>,
    pub parent: Option<ByDelegate>,
}

#[track_caller]
pub fn my_work(agent: &Agent, token: &str, id: &str) -> Result<MyWork, Error> {
    read_with(agent, token, MY_WORK, json!({ "id": id }))
}

// What `issue chain` reads of the parent: its sub-issues, each with the issues blocking it.
const CHAIN_TARGET: &str = "query Chain($id: String!) { viewer { id } issue(id: $id) { id identifier children(first: 250) { nodes { id identifier state { type } delegate { id } inverseRelations(first: 250) { nodes { id type issue { identifier state { type } } } pageInfo { hasNextPage } } } pageInfo { hasNextPage } } } }";

#[derive(Debug, Deserialize)]
pub struct ChainTarget {
    pub viewer: Ref,
    pub issue: Option<ChainParent>,
}

#[derive(Debug, Deserialize)]
pub struct ChainParent {
    pub id: String,
    pub children: Nodes<ChainChild>,
}

#[derive(Debug, Deserialize)]
pub struct ChainChild {
    pub id: String,
    pub identifier: String,
    pub state: StateOf,
    pub delegate: Option<Ref>,
    #[serde(flatten)]
    pub relations: Relations<ChainBlocker>,
}

#[derive(Debug, Deserialize)]
pub struct ChainBlocker {
    pub identifier: String,
    pub state: StateOf,
}

#[track_caller]
pub fn chain_target(agent: &Agent, token: &str, id: &str) -> Result<ChainTarget, Error> {
    read_with(agent, token, CHAIN_TARGET, json!({ "id": id }))
}

// The ID of the issue of session `session_id`: None when Linear has no such session, or it
// is on no issue.
#[track_caller]
pub fn session_issue_id(agent: &Agent, session_id: &str) -> Result<Option<String>, Error> {
    #[derive(Deserialize)]
    struct Session {
        issue: Option<Ref>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Answer {
        agent_session: Option<Session>,
    }
    let found: Answer = read_as(agent, "query MarkedIssue($id: String!) { agentSession(id: $id) { issue { id } } }", json!({ "id": session_id }))?;
    Ok(found.agent_session.and_then(|s| s.issue).map(|i| i.id))
}

const SESSIONS_BY_UPDATE: &str = "query Sessions {
  agentSessions(first: 50, orderBy: updatedAt) {
    nodes { id status updatedAt creator { id } issue { id } }
  }
}";

// A session as `tolkster sessions` lists it. Read through its issue it has the app's ID and
// none of its issue's; read from the newest of all, the issue's and not the app's.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    pub id: String,
    pub status: String,
    pub updated_at: String,
    pub creator: Option<Ref>,
    pub issue: Option<Ref>,
    pub app_user: Option<Ref>,
}

impl OfApp for Listed {
    fn app_user_id(&self) -> Option<&str> {
        self.app_user.as_ref().map(|a| a.id.as_str())
    }
}

// Tolkster's sessions on issue `issue`, every page of them, each with the issue's id; without
// one, the newest page of all.
#[track_caller]
pub fn agent_sessions(agent: &Agent, token: &str, issue: Option<&str>) -> Result<Vec<Listed>, Error> {
    match issue {
        Some(id) => {
            let (nodes, issue_id, _) = issue_sessions::<Listed>(&|q, v| graphql(agent, token, q, v), id, &Through { issue: "id".into(), session: "id status updatedAt creator { id }".into() }, None)?;
            Ok(nodes.into_iter().map(|n| Listed { issue: Some(Ref { id: issue_id.clone() }), ..n }).collect())
        }
        None => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Answer {
                agent_sessions: Nodes<Listed>,
            }
            Ok(read_with::<Answer>(agent, token, SESSIONS_BY_UPDATE, json!({}))?.agent_sessions.nodes)
        }
    }
}

#[cfg(test)]
mod tests;
