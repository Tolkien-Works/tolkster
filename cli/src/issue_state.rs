// Every rule for where Tolkster moves an issue in its team's workflow, and for taking its
// delegation back (docs/issue-states.md). Of the states only IDs, types, names and
// positions are read.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use ureq::Agent;

use crate::linear::{Nodes, Ref, Relation, StateType, answer_to, graphql, update_issue, with_token};
use crate::request::Issue;
use crate::error::Error;
use crate::json::decode;
use crate::plain::Commit;
use crate::sync::lock;

// `{ id name type position }`, as every read of a team's states asks for it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct WorkflowState {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub state_type: StateType,
    pub position: f64,
}

// `{ states(...) { nodes { id name type position } } }`, a team's.
#[derive(Deserialize)]
struct States {
    states: Nodes<WorkflowState>,
}

// `{ state { id name type position } }`, an issue's.
#[derive(Deserialize)]
struct Current {
    state: WorkflowState,
}

// The read of `query` decoded, with the error naming it.
#[track_caller]
fn read_as<T: DeserializeOwned>(agent: &Agent, token: &str, query: &str, variables: Value) -> Result<T, Error> {
    decode(graphql(agent, token, query, variables)?, &answer_to(query))
}

struct Read {
    current: WorkflowState,
    started: Vec<WorkflowState>,
}

const READ: &str = r#"query IssueState($id: String!) { issue(id: $id) {
  state { id name type position }
  team { states(first: 250, filter: { type: { eq: "started" } }) { nodes { id name type position } } }
} }"#;

fn read(agent: &Agent, token: &str, issue_id: &str) -> Result<Read, Error> {
    #[derive(Deserialize)]
    struct Issue {
        state: WorkflowState,
        team: States,
    }
    #[derive(Deserialize)]
    struct Answer {
        issue: Issue,
    }
    let issue = read_as::<Answer>(agent, token, READ, json!({ "id": issue_id }))?.issue;
    Ok(Read { current: issue.state, started: in_order(issue.team.states.nodes) })
}

// In position order: every rule here reads a team's states so.
pub fn in_order(mut states: Vec<WorkflowState>) -> Vec<WorkflowState> {
    states.sort_by(|a, b| a.position.partial_cmp(&b.position).unwrap_or(std::cmp::Ordering::Equal));
    states
}

// Each team's started states, by team id, read once for the run (runtime::Shared): a team's
// workflow changes rarely, and `tolkster listen` starts and resumes workers many times between
// restarts.
#[derive(Default)]
pub struct Started(Mutex<HashMap<String, Vec<WorkflowState>>>);

const TEAM_STARTED: &str = r#"query TeamStarted($id: String!) { team(id: $id) {
  states(first: 250, filter: { type: { eq: "started" } }) { nodes { id name type position } }
} }"#;

fn started_states(agent: &Agent, token: &str, known: &Started, team_id: &str) -> Result<Vec<WorkflowState>, Error> {
    if let Some(started) = lock(&known.0).get(team_id) {
        return Ok(started.clone());
    }
    #[derive(Deserialize)]
    struct Answer {
        team: States,
    }
    let started = in_order(read_as::<Answer>(agent, token, TEAM_STARTED, json!({ "id": team_id }))?.team.states.nodes);
    lock(&known.0).insert(team_id.into(), started.clone());
    Ok(started)
}

fn move_to(agent: &Agent, token: &str, issue_id: &str, state_id: &str) -> Result<(), Error> {
    update_issue(agent, token, issue_id, json!({ "stateId": state_id }))
}

const CURRENT: &str = "query IssueCurrent($id: String!) { issue(id: $id) { state { id name type position } } }";

// Updates the issue with the input `decide` makes of its state, read here just before the
// update, never taken from an earlier read: an issue finished since would otherwise be moved
// back, and one a worker's claim started meanwhile, out of Triage. `decide` gives no input
// when there is nothing to do, or an error when the state forbids it. True when it updated.
pub fn guarded_update(agent: &Agent, token: &str, issue_id: &str, decide: impl FnOnce(&WorkflowState) -> Result<Option<Value>, Error>) -> Result<bool, Error> {
    #[derive(Deserialize)]
    struct Answer {
        issue: Current,
    }
    let current = read_as::<Answer>(agent, token, CURRENT, json!({ "id": issue_id }))?.issue.state;
    let Some(input) = decide(&current)? else { return Ok(false) };
    update_issue(agent, token, issue_id, input)?;
    Ok(true)
}

// A finished issue stays.
fn start_target<'s>(current: &WorkflowState, started: &'s [WorkflowState]) -> Result<Option<&'s WorkflowState>, Error> {
    let Some(first) = started.first() else {
        return Err(Error::Failed("the issue's team has no started state".into()));
    };
    let moves = current.state_type.is_unstarted() || (current.state_type == StateType::Started && current.id != first.id);
    Ok(moves.then_some(first))
}

// True when it moved. The team's started states come from the cache, by the team id the
// issue was read with.
pub fn start_issue(agent: &Agent, token: &str, known: &Started, issue: &Issue) -> Result<bool, Error> {
    let Some(team_id) = &issue.team_id else {
        return Err(Error::Failed("the issue was read without its team's id".into()));
    };
    let started = started_states(agent, token, known, team_id)?;
    guarded_update(agent, token, &issue.id, |current| Ok(start_target(current, &started)?.map(|target| json!({ "stateId": target.id })))).inspect_err(|_| {
        // A failed move to a cached state may mean the workflow changed: the next start reads
        // it again.
        lock(&known.0).remove(team_id);
    })
}

// A triage session's verdict on an issue (commands/triage.rs). Only `accept` and
// `duplicate` name other issues.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    // No estimate when the team doesn't use estimates.
    Accept { estimate: Option<u32>, priority: u8, relations: Vec<(Relation, String)> },
    // `by`: the commit on the default branch that fixed it or removed the path.
    Fixed { by: Commit },
    Duplicate { of: String },
    // Left in Triage: only the owner closes an issue for not being worth doing.
    Decline,
}

impl Verdict {
    pub fn name(&self) -> &'static str {
        match self {
            Verdict::Accept { .. } => "accept",
            Verdict::Fixed { .. } => "fixed",
            Verdict::Duplicate { .. } => "duplicate",
            Verdict::Decline => "decline",
        }
    }
}

// A team without the state the verdict needs is an error: nothing else stands in for it.
pub fn verdict_target<'s>(states: &'s [WorkflowState], verdict: &Verdict) -> Result<Option<&'s WorkflowState>, Error> {
    let first = |state_type: StateType, what: &str| states.iter().find(|s| s.state_type == state_type).ok_or_else(|| Error::Failed(format!("the issue's team has no {what} state")));
    Ok(match verdict {
        Verdict::Accept { .. } => Some(first(StateType::Backlog, "backlog")?),
        Verdict::Fixed { .. } => Some(first(StateType::Canceled, "cancelled")?),
        Verdict::Duplicate { .. } => Some(match states.iter().find(|s| s.state_type == StateType::Duplicate || (s.state_type == StateType::Canceled && s.name.to_ascii_lowercase().contains("duplicate"))) {
            Some(duplicate) => duplicate,
            None => first(StateType::Canceled, "cancelled")?,
        }),
        Verdict::Decline => None,
    })
}

// Found by type, never by name, so it holds on every team.
pub fn filing_target(states: &[WorkflowState], triage_on: bool) -> Result<&WorkflowState, Error> {
    let first = |state_type: StateType| states.iter().find(|s| s.state_type == state_type);
    triage_on.then(|| first(StateType::Triage)).flatten().or_else(|| first(StateType::Backlog)).ok_or_else(|| Error::Failed("the team has neither a triage nor a backlog state".into()))
}

// What `in_review` did, as the CLI prints it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Review {
    Moved,
    // Already in review, past it, or finished.
    Unchanged,
    // The team has no started state with "review" in its name.
    NoReviewState,
}

impl Review {
    pub fn as_str(self) -> &'static str {
        match self {
            Review::Moved => "moved",
            Review::Unchanged => "unchanged",
            Review::NoReviewState => "no review state",
        }
    }
}

// The first started state with "review" in its name, ignoring case. `doctor` judges a team
// by the same rule.
pub fn review_state(started: &[WorkflowState]) -> Option<&WorkflowState> {
    started.iter().find(|s| s.name.to_ascii_lowercase().contains("review"))
}

// From an unstarted state or an earlier started one, never back.
fn review_target<'s>(current: &WorkflowState, started: &'s [WorkflowState]) -> Result<Option<&'s WorkflowState>, Review> {
    let Some(review) = review_state(started) else {
        return Err(Review::NoReviewState);
    };
    let before = current.state_type.is_unstarted() || (current.state_type == StateType::Started && current.id != review.id && current.position < review.position);
    Ok(before.then_some(review))
}

// Read here, not guarded: this runs once, in a worker's own `tolkster` process.
pub fn in_review(agent: &Agent, token: &str, issue_id: &str) -> Result<Review, Error> {
    let read = read(agent, token, issue_id)?;
    match review_target(&read.current, &read.started) {
        Err(review) => Ok(review),
        Ok(None) => Ok(Review::Unchanged),
        Ok(Some(target)) => {
            move_to(agent, token, issue_id, &target.id)?;
            Ok(Review::Moved)
        }
    }
}

const TEAM_COMPLETED: &str = r#"query IssueTeamCompleted($id: String!) { issue(id: $id) {
  team { states(first: 250, filter: { type: { eq: "completed" } }) { nodes { id name type position } } }
} }"#;

// A finished issue stays: one completed or cancelled since is never moved.
fn done_target<'s>(current: &WorkflowState, completed: &'s [WorkflowState]) -> Result<Option<&'s WorkflowState>, Error> {
    let Some(first) = completed.first() else {
        return Err(Error::Failed("the issue's team has no completed state".into()));
    };
    Ok((!current.state_type.is_finished()).then_some(first))
}

// The issue moved to its team's first completed state once its work has landed with no PR,
// which Linear's GitHub integration would otherwise do on a PR's merge. True when it moved.
pub fn complete_issue(agent: &Agent, token: &str, issue_id: &str) -> Result<bool, Error> {
    #[derive(Deserialize)]
    struct Issue {
        team: States,
    }
    #[derive(Deserialize)]
    struct Answer {
        issue: Issue,
    }
    let completed = in_order(read_as::<Answer>(agent, token, TEAM_COMPLETED, json!({ "id": issue_id }))?.issue.team.states.nodes);
    guarded_update(agent, token, issue_id, |current| Ok(done_target(current, &completed)?.map(|target| json!({ "stateId": target.id }))))
}

// Undoes Tolkster's own delegation only: a mentioned issue may be delegated to someone else.
pub fn release_issue(agent: &Agent, id: &str) -> Result<(), Error> {
    with_token(agent, |token| {
        #[derive(Deserialize)]
        struct Issue {
            id: String,
            delegate: Option<Ref>,
        }
        #[derive(Deserialize)]
        struct Answer {
            viewer: Ref,
            // None once the issue is gone, which leaves nothing to undo.
            issue: Option<Issue>,
        }
        let read: Answer = read_as(agent, token, "query Issue($id: String!) { viewer { id } issue(id: $id) { id delegate { id } } }", json!({ "id": id }))?;
        let Some(issue) = read.issue.filter(|issue| issue.delegate.as_ref().map(|d| &d.id) == Some(&read.viewer.id)) else { return Ok(()) };
        update_issue(agent, token, &issue.id, json!({ "delegateId": null }))
    })
}

#[cfg(test)]
mod tests;
