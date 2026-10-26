// The rules for `tolkster issue`'s changes to Linear issues, and the reads of an issue's
// relations that `sessions.rs`, `triage.rs` and `context.rs` share.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::Error;
use crate::estimates::{Estimation, Scale};
use crate::issue_state::WorkflowState;
use crate::linear::{Nodes, Ref, Relation};

pub fn priority(text: &str) -> Result<u8, Error> {
    Ok(match text.trim().to_ascii_lowercase().as_str() {
        "0" | "none" => 0,
        "1" | "urgent" => 1,
        "2" | "high" => 2,
        "3" | "medium" | "normal" => 3,
        "4" | "low" => 4,
        _ => return Err(Error::Usage(format!("--priority must be 0 to 4, or none, urgent, high, medium or low, not {text:?}"))),
    })
}

pub fn estimate(text: &str) -> Result<Value, Error> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("none") {
        return Ok(Value::Null);
    }
    text.parse::<u32>().map(|n| json!(n)).map_err(|_| Error::Usage(format!("--estimate must be a whole number or none, not {text:?}")))
}

// An --assignee as Linear's `assigneeId`: `me` is the owner, `none` clears it, and anything
// else must be a user's ID, as `tolkster members` prints it.
pub fn assignee(text: &str, owner: &str) -> Result<Value, Error> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("me") {
        return Ok(json!(owner));
    }
    if text.eq_ignore_ascii_case("none") {
        return Ok(Value::Null);
    }
    let shaped = text.len() == 36 && text.chars().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == '-' } else { c.is_ascii_hexdigit() });
    if !shaped {
        return Err(Error::Usage(format!("--assignee must be me, none or a user's ID from tolkster members, not {text:?}")));
    }
    Ok(json!(text.to_ascii_lowercase()))
}

pub const RELATIONS: [(&str, Relation); 4] =
    [("blocks", Relation::Blocks), ("blocked-by", Relation::BlockedBy), ("related", Relation::Related), ("duplicate-of", Relation::DuplicateOf)];

pub const RELATION_FIELDS: &str =
    "relations(first: 250) { nodes { id type relatedIssue { identifier } } } inverseRelations(first: 250) { nodes { id type issue { identifier } } }";

impl Relation {
    // A blocks relation sits on the blocking issue, so "is blocked by" is read from the
    // inverse side, and a related pair counts whichever side made it. One whose other end is
    // null, an issue since deleted, is left out.
    pub fn related<T>(self, issue: &Relations<T>) -> Vec<Related<'_, T>> {
        fn side<'a, T>(list: &'a Option<Nodes<Link<T>>>, kind: &str) -> Vec<Related<'a, T>> {
            list.iter().flat_map(|l| &l.nodes).filter(|r| r.kind == kind).filter_map(|r| Some(Related { relation: r, other: r.other.as_ref()? })).collect()
        }
        let (out, inverse) = (&issue.relations, &issue.inverse_relations);
        match self {
            Relation::Blocks => side(out, "blocks"),
            Relation::BlockedBy => side(inverse, "blocks"),
            Relation::Related => [side(out, "related"), side(inverse, "related")].concat(),
            Relation::DuplicateOf => side(out, "duplicate"),
        }
    }

    // The IDs of this relation's links to `other`, as `RELATION_FIELDS` reads them.
    pub fn find(self, issue: &Relations<Identified>, other: &str) -> Vec<String> {
        self.related(issue).into_iter().filter(|r| r.other.identifier == other).filter_map(|r| r.relation.id.clone()).collect()
    }
}

// An issue's relations, flattened into the issue a query reads them with: `relations` and
// `inverseRelations`, each where the query asks for it, with the other end's fields `T`.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Relations<T> {
    pub relations: Option<Nodes<Link<T>>>,
    pub inverse_relations: Option<Nodes<Link<T>>>,
}

// One relation, from either side: its other end is `relatedIssue` on `relations` and `issue`
// on `inverseRelations`, null once that issue is deleted.
#[derive(Clone, Debug, Deserialize)]
pub struct Link<T> {
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "relatedIssue", alias = "issue")]
    pub other: Option<T>,
}

// The other end of a relation read by its identifier alone.
#[derive(Clone, Debug, Deserialize)]
pub struct Identified {
    pub identifier: String,
}

pub struct Related<'a, T> {
    pub relation: &'a Link<T>,
    pub other: &'a T,
}

impl<T> Clone for Related<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Related<'_, T> {}

pub fn pick_state(states: &[WorkflowState], name: &str) -> Result<String, Error> {
    pick(states.iter().map(|s| (s.name.as_str(), s.id.as_str())), name, "state")
}

// Clearing an estimate is always allowed; a value only when it is on the team's scale.
pub fn check_estimate(team: &Estimation, estimate: &Value) -> Result<(), Error> {
    let Some(points) = estimate.as_u64() else { return Ok(()) };
    let scale = Scale::of(team).ok_or_else(|| Error::Failed("the issue's team's estimate scale couldn't be read".into()))?;
    scale.check(u32::try_from(points).unwrap_or(u32::MAX)).map_err(Error::Usage)
}

// A label by name, on the team `team` or, without one, the workspace's.
#[derive(Debug, Deserialize)]
pub struct Label {
    pub id: String,
    pub name: String,
    pub team: Option<Ref>,
}

// A team's project, open until it is completed or cancelled.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamProject {
    pub id: String,
    pub name: String,
    pub completed_at: Option<String>,
    pub canceled_at: Option<String>,
}

impl TeamProject {
    pub fn is_open(&self) -> bool {
        self.completed_at.is_none() && self.canceled_at.is_none()
    }
}

pub fn pick_label(labels: &[Label], team_id: &str, name: &str) -> Result<String, Error> {
    pick(labels.iter().filter(|l| l.team.as_ref().is_none_or(|t| t.id == team_id)).map(|l| (l.name.as_str(), l.id.as_str())), name, "label")
}

// A completed or cancelled project takes no new issues.
pub fn pick_project(projects: &[TeamProject], name: &str) -> Result<Value, Error> {
    if name.trim().eq_ignore_ascii_case("none") {
        return Ok(Value::Null);
    }
    pick(projects.iter().filter(|p| p.is_open()).map(|p| (p.name.as_str(), p.id.as_str())), name, "open project").map(Value::from)
}

// The ID of the one candidate, by (name, ID), named `name`, ignoring case.
fn pick<'a>(candidates: impl Iterator<Item = (&'a str, &'a str)>, name: &str, what: &str) -> Result<String, Error> {
    let found: Vec<&str> = candidates.filter(|(n, _)| n.eq_ignore_ascii_case(name.trim())).map(|(_, id)| id).collect();
    match found.as_slice() {
        [id] => Ok(id.to_string()),
        [] => Err(Error::Failed(format!("no {what} named {name:?}"))),
        _ => Err(Error::Failed(format!("several {what}s are named {name:?}"))),
    }
}

#[cfg(test)]
mod tests;
