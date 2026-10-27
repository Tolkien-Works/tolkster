// Linear's reads beside a session's: the issues that are Tolkster's, an issue's identifiers,
// labels and description edits and what its closing reaches, and a comment and its thread.

use std::collections::HashMap;

use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::json;
use ureq::Agent;

use super::{Bodies, LABELS, SessionNode, finished_at, parse_session};
use crate::linear::{IssueRef, Named, Nodes, Project, Ref, StateOf, StateType, TeamKey, identifier_filter, read_as};
use crate::request::MANUAL_MERGE_LABEL;
use crate::request::{Session, Thread, ThreadReply};
use crate::error::Error;
use crate::estimates::with_team_fields;
use crate::issues::Relations;
use crate::config::repo_map::Place;
use crate::plain::{Id, IssueKey, Time};

// The identifiers of the issues `ids`, as a notice names them, in the order given: one Linear
// no longer has is left out.
pub fn identifiers(agent: &Agent, ids: &[Id]) -> Result<Vec<String>, Error> {
    #[derive(Deserialize)]
    struct Answer {
        issues: Nodes<IssueRef>,
    }
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let query = "query Identifiers($ids: [ID!]!) { issues(first: 50, filter: { id: { in: $ids } }) { nodes { id identifier } } }";
    let answer: Answer = read_as(agent, query, json!({ "ids": ids }))?;
    let found: HashMap<String, String> = answer.issues.nodes.into_iter().map(|n| (n.id, n.identifier)).collect();
    Ok(ids.iter().filter_map(|id| IssueKey::parse(found.get(id.as_str())?)).map(String::from).collect())
}

pub type Tolksters = HashMap<String, IssueNow>;

// An issue finished with no time Linear gives is `Open`, which only keeps its worktree.
#[derive(Clone, Debug, PartialEq)]
pub enum IssueNow {
    Open,
    Completed(String),
    Cancelled(String),
}

// Archived sessions too: a merged PR's may be archived. Only the first 20 are read, of any
// agent, so a missed one can only keep a worktree, never remove one (reap.rs), or refuse a
// closing comment (commands/comment.rs). `LINKED_PER_PAGE`
// issues at a time keeps the query well under Linear's complexity cap.
const LINKED_PER_PAGE: usize = 20;
const LINKED: &str = "query Linked($filter: IssueFilter!) {
  viewer { id }
  issues(first: PER_PAGE, filter: $filter) {
    nodes { identifier delegate { id } state { type } completedAt canceledAt agentSessions(first: 20, includeArchived: true) { nodes { id status createdAt appUser { id } creator { id } comment { user { id } } sourceComment { user { id } } } } }
  }
}";

#[derive(Debug, Deserialize)]
struct Linked {
    viewer: Ref,
    issues: Nodes<LinkedIssue>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinkedIssue {
    identifier: String,
    delegate: Option<Ref>,
    state: StateOf,
    completed_at: Option<String>,
    canceled_at: Option<String>,
    agent_sessions: Nodes<SessionNode>,
}

// Whatever their state: a merged PR has usually moved its issue to Done.
pub fn tolksters_issues(agent: &Agent, identifiers: &[IssueKey], owner_id: &str) -> Result<Tolksters, Error> {
    let query = LINKED.replace("PER_PAGE", &LINKED_PER_PAGE.to_string());
    let mut tolksters = Tolksters::new();
    for chunk in identifiers.chunks(LINKED_PER_PAGE) {
        let linked: Linked = read_as(agent, &query, json!({ "filter": identifier_filter(chunk) }))?;
        tolksters.extend(parse_tolksters(&linked, owner_id).into_iter().filter(|(i, _)| chunk.iter().any(|key| key == i)));
    }
    Ok(tolksters)
}

// Delegated to Tolkster (`viewer_id`), or with a Tolkster session the owner started, by the
// one owner check (`Session::is_owners`). `sessions` are Tolkster's alone. A mention doesn't
// delegate, so its session is all it leaves.
pub fn is_tolksters<'a>(delegate_id: Option<&str>, viewer_id: &str, sessions: impl IntoIterator<Item = &'a Session>, owner_id: &str) -> bool {
    delegate_id == Some(viewer_id) || sessions.into_iter().any(|s| s.is_owners(owner_id))
}

// Whatever the issue's state: a finished issue's worktree is still Tolkster's to remove.
fn parse_tolksters(linked: &Linked, owner_id: &str) -> Tolksters {
    let viewer_id = linked.viewer.id.as_str();
    let mut issues = Tolksters::new();
    for node in &linked.issues.nodes {
        let tolksters: Vec<Session> = node.agent_sessions.nodes.iter().filter(|s| s.began.app_user.id == viewer_id).map(parse_session).collect();
        if is_tolksters(node.delegate.as_ref().map(|d| d.id.as_str()), viewer_id, &tolksters, owner_id) {
            let at = |state| finished_at(node.state.kind, node.canceled_at.as_ref(), node.completed_at.as_ref(), state);
            let now = at(StateType::Canceled).map(IssueNow::Cancelled).or_else(|| at(StateType::Completed).map(IssueNow::Completed));
            issues.insert(node.identifier.to_ascii_uppercase(), now.unwrap_or(IssueNow::Open));
        }
    }
    issues
}

// Identifiers, ids, state types and times only, never text. Past the first 50 relations of
// a side, of any type or state, an issue it blocks starts at the relay's next blocker check (blockers.ts) and a
// related one hears nothing of it. Past the first 250 sub-issues, one keeps a delegation
// its parent's end would take back. Past the first 50 sub-issues of its parent, a sibling
// hears nothing of it.
const CLOSED: &str = "query Closed($id: String!) {
  viewer { id }
  issue(id: $id) {
    id identifier createdAt canceledAt archivedAt state { type } team { key TEAM_FIELDS } project { id name } delegate { id } creator { id }
    parent { identifier children(first: 50) { nodes { identifier archivedAt state { type } } } }
    children(first: 250) { nodes { id identifier delegate { id } } }
    relations(first: 50) { nodes { type relatedIssue { identifier archivedAt state { type } } } }
    inverseRelations(first: 50) { nodes { type issue { identifier archivedAt state { type } } } }
  }
}";

// A completed issue as its context reads it (context::parse_closed), with the app's own user
// ID.
#[derive(Clone, Debug, Deserialize)]
pub struct ClosedRead {
    pub viewer: Ref,
    pub issue: ClosedIssue,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ClosedIssue {
    pub identifier: String,
    pub state: StateOf,
    pub team: TeamKey,
    pub project: Option<Project>,
    pub delegate: Option<Ref>,
    pub parent: Option<ClosedParent>,
    #[serde(flatten)]
    pub relations: Relations<Neighbour>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ClosedParent {
    pub children: Nodes<Neighbour>,
}

// An issue a completed one is related to, or a sibling of it: open unless finished or archived.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Neighbour {
    pub identifier: String,
    pub archived_at: Option<String>,
    pub state: StateOf,
}

// Err when Linear has no issue `id`.
pub fn read_closed(agent: &Agent, id: &str) -> Result<ClosedRead, Error> {
    #[derive(Deserialize)]
    struct Answer {
        viewer: Ref,
        issue: Option<ClosedIssue>,
    }
    let answer: Answer = read_as(agent, &with_team_fields(CLOSED), json!({ "id": id }))?;
    let issue = answer.issue.ok_or_else(|| Error::Failed(format!("{id}: no such issue")))?;
    Ok(ClosedRead { viewer: answer.viewer, issue })
}

// Whether comment `comment_id` is on issue `issue_id`, by ID alone: false once it is deleted.
// Err when the issue can't be read, so a failed read is never taken for a deleted comment.
pub fn comment_on_issue(agent: &Agent, issue_id: &str, comment_id: &str) -> Result<bool, Error> {
    #[derive(Deserialize)]
    struct Issue {
        comments: Nodes<Ref>,
    }
    #[derive(Deserialize)]
    struct Answer {
        issue: Option<Issue>,
    }
    let query = "query CommentOn($id: String!, $comment: ID!) { issue(id: $id) { id comments(first: 1, filter: { id: { eq: $comment } }) { nodes { id } } } }";
    let answer: Answer = read_as(agent, query, json!({ "id": issue_id, "comment": comment_id }))?;
    let issue = answer.issue.ok_or_else(|| Error::Failed(format!("Linear had no issue {issue_id}")))?;
    Ok(issue.comments.nodes.iter().any(|c| c.id == comment_id))
}

// Whether the issue is left for the owner to merge (`request::MANUAL_MERGE_LABEL`), from
// its labels' names, which are compared and never passed on.
pub fn manual_merge(agent: &Agent, issue_id: &str) -> Result<bool, Error> {
    #[derive(Deserialize)]
    struct Issue {
        labels: Nodes<Named>,
    }
    #[derive(Deserialize)]
    struct Answer {
        issue: Option<Issue>,
    }
    let query = format!("query Labels($id: String!) {{ issue(id: $id) {{ labels(first: {LABELS}) {{ nodes {{ name }} }} }} }}");
    let answer: Answer = read_as(agent, &query, json!({ "id": issue_id }))?;
    let issue = answer.issue.ok_or_else(|| Error::Failed(format!("Linear had no issue {issue_id}")))?;
    Ok(issue.labels.nodes.iter().any(|l| l.name.eq_ignore_ascii_case(MANUAL_MERGE_LABEL)))
}

// Ids only, never the comment's text.
const COMMENT: &str = "query Comment($id: String!OWNER_VARIABLE) {
  viewer { id }
  comment(id: $id) {
    id createdAt user { id } botActor { id } parent { id user { id } botActor { id } }
    issue { id identifier creator { id } state { type } OWNER_COMMENTS agentSessions(first: COMMENT_SESSIONS, orderBy: createdAt) { nodes { id createdAt appUser { id } creator { id } comment { id } sourceComment { id } } } }
  }
}";

// The sessions read with a comment: a mention's is among the newest, since it began with
// the comment, and a thread with a reply is seldom older than this many sessions.
const COMMENT_SESSIONS: u32 = 50;

// A comment as `read_comment` reads it, None once it is gone, with the app's own user ID
// (ports::comment_kind).
#[derive(Debug, Deserialize)]
pub struct CommentRead {
    pub viewer: Ref,
    pub comment: Option<CommentNode>,
}

// An integration's comment posted as someone has a `botActor`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentNode {
    pub id: String,
    pub created_at: String,
    pub user: Option<Ref>,
    pub bot_actor: Option<IgnoredAny>,
    pub parent: Option<CommentParent>,
    pub issue: Option<CommentIssue>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentParent {
    pub id: String,
    pub user: Option<Ref>,
    pub bot_actor: Option<IgnoredAny>,
}

// `comments` only in a read with the owner's: their newest, as Linear lists the newest first.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentIssue {
    pub id: String,
    pub identifier: String,
    pub creator: Option<Ref>,
    pub state: StateOf,
    pub comments: Option<Nodes<Created>>,
    pub agent_sessions: Nodes<CommentSession>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Created {
    pub created_at: String,
}

// Which comment a session on the issue began on, and its source: IDs only.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentSession {
    pub app_user: Ref,
    pub comment: Option<Ref>,
    pub source_comment: Option<Ref>,
}

// With `owner`, also the `createdAt` of the issue's newest comment by them (`CommentIssue::comments`).
pub fn read_comment(agent: &Agent, id: &str, owner: Option<&str>) -> Result<CommentRead, Error> {
    let (variable, comments) = match owner {
        Some(_) => (", $owner: ID!", "comments(first: 1, orderBy: createdAt, filter: { user: { id: { eq: $owner } } }) { nodes { createdAt } }"),
        None => ("", ""),
    };
    let query = COMMENT.replace("COMMENT_SESSIONS", &COMMENT_SESSIONS.to_string()).replace("OWNER_VARIABLE", variable).replace("OWNER_COMMENTS", comments);
    let variables = match owner {
        Some(owner) => json!({ "id": id, "owner": owner }),
        None => json!({ "id": id }),
    };
    read_as(agent, &query, variables)
}

const THREAD: &str = "query Thread($id: String!) {
  viewer { id }
  comment(id: $id) {
    id createdAt user { id } botActor { id } parent { id } BODY
    issue { id identifier creator { id } team { key } project { id name } agentSessions(first: COMMENT_SESSIONS, orderBy: createdAt) { nodes { appUser { id } comment { id } } } }
    children(first: THREAD_REPLIES, orderBy: createdAt) { nodes { id createdAt user { id } botActor { id } BODY } }
  }
}";

// The replies read in a thread, newest first: an older reply was answered long before.
const THREAD_REPLIES: u32 = 100;

#[derive(Debug, Deserialize)]
struct ThreadRead {
    viewer: Ref,
    comment: Option<ThreadRoot>,
}

#[derive(Debug, Deserialize)]
struct ThreadRoot {
    #[serde(flatten)]
    note: Note,
    parent: Option<Ref>,
    issue: Option<ThreadIssue>,
    children: Nodes<Note>,
}

// The root or a reply, its `body` only in a read with bodies.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Note {
    id: String,
    created_at: String,
    user: Option<Ref>,
    bot_actor: Option<IgnoredAny>,
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadIssue {
    id: String,
    identifier: String,
    creator: Option<Ref>,
    team: TeamKey,
    project: Option<Project>,
    agent_sessions: Nodes<ThreadSession>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadSession {
    app_user: Ref,
    comment: Option<Ref>,
}

// The thread of Tolkster's comment `root`, with the owner's replies in it, their text only
// with `bodies` (`tolkster messages --thread`, and the prompt of the turn that answers
// them), so that `listen` reads none. Err with why it is no thread a turn answers.
pub fn read_thread(agent: &Agent, root: &str, owner_id: &str, bodies: Bodies) -> Result<Result<Thread, &'static str>, Error> {
    let query = THREAD
        .replace("COMMENT_SESSIONS", &COMMENT_SESSIONS.to_string())
        .replace("THREAD_REPLIES", &THREAD_REPLIES.to_string())
        .replace("BODY", if bodies == Bodies::Read { "body" } else { "" });
    let data: ThreadRead = read_as(agent, &query, json!({ "id": root }))?;
    Ok(parse_thread(&data, owner_id))
}

// `read_thread`, failing with why `root` is no thread a turn answers.
pub fn thread_of(agent: &Agent, root: &str, owner_id: &str, bodies: Bodies) -> Result<Thread, Error> {
    read_thread(agent, root, owner_id, bodies)?.map_err(|why| Error::Failed(format!("comment {root}: {why}")))
}

// The thread in `data`, if its root begins a thread, on an issue, and no session of
// Tolkster's began on (a session's thread is its session's), and is a comment of Tolkster's
// (the viewer's) or the owner's own on an issue Tolkster created (ports::comment_kind's
// `NoteRoot`), whose body is then the first request. The replies kept are the owner's own,
// not an integration's posting as them.
fn parse_thread(data: &ThreadRead, owner_id: &str) -> Result<Thread, &'static str> {
    let viewer = data.viewer.id.as_str();
    let Some(comment) = &data.comment else { return Err("unreadable") };
    let Some(root) = Id::parse(&comment.note.id) else { return Err("unreadable") };
    if owner_id.is_empty() {
        return Err("no owner");
    }
    let by = |c: &Note, user: &str| c.user.as_ref().is_some_and(|u| u.id == user);
    let owners = |c: &Note| by(c, owner_id) && c.bot_actor.is_none();
    let created_by_me = comment.issue.as_ref().and_then(|i| i.creator.as_ref()).is_some_and(|c| c.id == viewer);
    let owner_root = owners(&comment.note) && created_by_me;
    if !(by(&comment.note, viewer) || owner_root) || comment.parent.is_some() {
        return Err("not a thread of my comment");
    }
    let Some(issue) = &comment.issue else { return Err("not on an issue") };
    let (Some(issue_id), Some(identifier)) = (Id::parse(&issue.id), IssueKey::parse(&issue.identifier)) else { return Err("not on an issue") };
    let identifier = identifier.upper();
    if issue.agent_sessions.nodes.iter().any(|s| s.app_user.id == viewer && s.comment.as_ref().is_some_and(|c| c.id == root.as_str())) {
        return Err("a session's thread");
    }
    let mut replies: Vec<ThreadReply> = owner_root
        .then_some(&comment.note)
        .into_iter()
        .chain(comment.children.nodes.iter().filter(|r| owners(r)))
        .filter_map(|r| Some(ThreadReply { id: Id::parse(&r.id)?, created_at: Time::parse(&r.created_at)?, body: r.body.clone().unwrap_or_default() }))
        .collect();
    replies.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(Thread { root, issue_id, identifier, place: Place::of(Some(&issue.team.key), issue.project.as_ref().map(|p| p.parts())), replies })
}

// A description edit older than these newest history entries is missed, so the baseline is
// earlier than it should be: the worker checks more commits, never fewer.
const HISTORY_PAGE: u32 = 100;

#[derive(Debug, Deserialize)]
struct Edits {
    issue: Option<EditsIssue>,
}

#[derive(Debug, Deserialize)]
struct EditsIssue {
    history: Nodes<Edit>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Edit {
    created_at: String,
    updated_description: Option<bool>,
}

// Only times and flags are read, never what the description said.
pub fn description_edits(agent: &Agent, issue_id: &str) -> Result<Vec<String>, Error> {
    let query = format!("query History($id: String!) {{ issue(id: $id) {{ history(first: {HISTORY_PAGE}) {{ nodes {{ createdAt updatedDescription }} }} }} }}");
    Ok(parse_description_edits(&read_as(agent, &query, json!({ "id": issue_id }))?))
}

fn parse_description_edits(data: &Edits) -> Vec<String> {
    data.issue.iter().flat_map(|i| &i.history.nodes).filter(|entry| entry.updated_description == Some(true)).map(|entry| entry.created_at.clone()).collect()
}

#[cfg(test)]
mod tests;
