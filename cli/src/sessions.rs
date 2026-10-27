// Reading sessions and issues from Linear, parsed into request.rs's `Session` with its
// record from the Durable Object, and every issue read the handlers decide on. A read that
// only guards a write of its own (issue_state.rs) stays with that write. The reads beside a
// session's are in sessions/issues.rs, triage's in sessions/triage.rs, and how a session
// began in sessions/began.rs.

mod began;
mod issues;
pub mod lookup;
pub mod shown;
mod triage;
pub mod workspace;

use serde::Deserialize;
use serde::de::{DeserializeOwned, IgnoredAny};
use serde_json::{Value, json};
use ureq::Agent;

pub use issues::{ClosedIssue, ClosedRead, CommentRead, CommentSession, IssueNow, Neighbour, Tolksters, comment_on_issue, description_edits, identifiers, manual_merge, read_closed, read_comment, read_thread, tolksters_issues};
use began::{created_delegated_by, delegated, delegator, in_description, mentioning_comment, on_own_note, replies_before, undelegated_at};
pub use triage::{HISTORIES_PER_READ, Other, StateHistory, TriageNode, read_other, read_triage_issues, read_triage_queue, read_triaged, state_histories, state_history};
use crate::request::{Activity, Blocker, Issue, Mention, Parent, Session};
use crate::error::Error;
use crate::estimates::{Estimation, Scale, TEAM_FIELDS};
use crate::issues::{Link, Relations};
use crate::config::repo_map::Place;
use crate::json::decode;
use crate::plain::Id;
use crate::linear::{ActivityType, Kind, Named, Nodes, PageInfo, Project, Ref, Relation, StateOf, StateType, answer_to, graphql, read_with, with_token};
use crate::state::{Record, Records, State};

// The fields every session query reads, with the newest `activities` activities: the
// session's own and its issue's.
pub fn session_fields(activities: u32, bodies: Bodies) -> String {
    format!("{} issue {{ {} }}", own_fields(activities, bodies), issue_fields())
}

// A session's fields without its issue's, for a query that reads the issue once, above its
// sessions (`Through`).
fn own_fields(activities: u32, bodies: Bodies) -> String {
    let comment = match bodies {
        Bodies::Read => "comment { body user { id } children(first: NOTE_REPLIES, orderBy: createdAt) { nodes { createdAt body user { id } botActor { id } } } } sourceComment { body user { id } }",
        Bodies::Skip => "comment { user { id } } sourceComment { user { id } }",
    }
    .replace("NOTE_REPLIES", &NOTE_REPLIES.to_string());
    format!("id status createdAt appUser {{ id }} creator {{ id }} {comment} {}", prompts(activities, bodies))
}

// The issue's fields `parse_session` reads.
//
// "A blocks B" is a relation on A of type `blocks` whose relatedIssue is B, so B's
// blockers are its inverse relations of that type: the API has no `blocked_by`. Blockers,
// parent and sub-issues are read by id, identifier, link and state type, never their text;
// the estimate, team scale and label names are compared, never passed on.
fn issue_fields() -> String {
    format!(
        "id identifier url branchName createdAt updatedAt creator {{ id }} canceledAt completedAt state {{ type }} team {{ id key {TEAM_FIELDS} }} delegate {{ id }} project {{ id name }}
        estimate priority labels(first: {LABELS}) {{ nodes {{ name }} }}
        {}
        parent {{ id identifier updatedAt canceledAt state {{ type }} delegate {{ id }} priority history(first: {HISTORY}) {{ nodes {{ createdAt actor {{ id }} fromDelegate {{ id }} toDelegate {{ id }} }} }} }}
        children(first: {CHILDREN}) {{ nodes {{ id identifier url updatedAt state {{ type }} completedAt canceledAt }} pageInfo {{ hasNextPage }} }}
        history(first: {HISTORY}) {{ nodes {{ createdAt actor {{ id }} fromDelegate {{ id }} toDelegate {{ id }} }} }}
        agentSessions(first: {NEWEST_SESSIONS}, orderBy: createdAt) {{ nodes {{ createdAt appUser {{ id }} creator {{ id }} comment {{ user {{ id }} }} sourceComment {{ id }} }} pageInfo {{ hasNextPage }} }}",
        relations_page(None)
    )
}

fn relations_page(after: Option<&str>) -> String {
    let after = after.map_or(String::new(), |a| format!(", after: {a}"));
    format!("inverseRelations(first: {RELATIONS}{after}) {{ nodes {{ type issue {{ id identifier url updatedAt state {{ type }} }} }} pageInfo {{ hasNextPage endCursor }} }}")
}

fn more_relations_query() -> String {
    format!("query MoreRelations($id: String!, $after: String!) {{ issue(id: $id) {{ {} }} }}", relations_page(Some("$after")))
}

// What a read of one session (`read`) asks for. `Owner` and `Answer` are slim: only what a check
// decides on, which parses as `Full`'s. `Owner` is the owner check (`Session::is_owners`) and the
// issue's size: who started the session, the issue's delegate, history and parent, its estimate
// and the team's scale, no activities. `Answer(n)` is the answer check (`post::given_answer`),
// which adds the newest `n` activities and what its request reads from the issue: its
// sub-issues and newest sessions. Both leave out the blockers, labels, project and priority,
// and the text of comments and activities. `Full` is the whole session and issue, with the
// newest `activities` prompts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Read {
    Owner,
    Answer(u32),
    Full { activities: u32, bodies: Bodies },
}

// A session's own fields without its activities or any text: who started it, and on what.
const SLIM_OWN: &str = "id status createdAt appUser { id } creator { id } comment { user { id } } sourceComment { user { id } }";

fn read_fields(what: Read) -> String {
    let own = SLIM_OWN;
    let issue = format!(
        "id identifier url branchName createdAt creator {{ id }} canceledAt completedAt state {{ type }} team {{ id {TEAM_FIELDS} }} delegate {{ id }} estimate
        parent {{ id identifier updatedAt canceledAt state {{ type }} delegate {{ id }} priority history(first: {HISTORY}) {{ nodes {{ createdAt actor {{ id }} fromDelegate {{ id }} toDelegate {{ id }} }} }} }}
        history(first: {HISTORY}) {{ nodes {{ createdAt actor {{ id }} fromDelegate {{ id }} toDelegate {{ id }} }} }}"
    );
    match what {
        Read::Full { activities, bodies } => session_fields(activities, bodies),
        Read::Owner => format!("{own} issue {{ {issue} }}"),
        Read::Answer(activities) => format!(
            "{own} {} issue {{ {issue}
            children(first: {CHILDREN}) {{ nodes {{ id identifier url updatedAt state {{ type }} completedAt canceledAt }} pageInfo {{ hasNextPage }} }}
            agentSessions(first: {NEWEST_SESSIONS}, orderBy: createdAt) {{ nodes {{ createdAt appUser {{ id }} creator {{ id }} comment {{ user {{ id }} }} sourceComment {{ id }} }} pageInfo {{ hasNextPage }} }} }}",
            prompts(activities, Bodies::Skip)
        ),
    }
}

// What a read of sessions through their issue asks for: the issue's fields once, at the
// issue, and each session's own (`issue_sessions`).
pub struct Through {
    pub issue: String,
    pub session: String,
}

// The replies read on the thread of a session Tolkster opened on its own comment
// (`on_own_note`), whose request is those before it began: newest first, as Linear orders by
// `createdAt`, so the replies nearest its start are read while few came after it.
const NOTE_REPLIES: u32 = 50;

// Whether a session is read with what people and Tolkster wrote: the mention's comment and
// each activity's text. Only `tolkster messages` and `start` read it, so that untrusted text never
// passes through `tolkster listen`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Bodies {
    Read,
    Skip,
}

// The session's newest `activities` prompts, none when 0. Linear filters them, so no ephemeral thought takes
// a place in the page and a page with more after it means older prompts are unread
// (`Session::more_activities`). Prompts are never ephemeral: Linear allows that only on thoughts
// and actions. What Tolkster said is read from its record (`Record::said`), never from here.
fn prompts(activities: u32, bodies: Bodies) -> String {
    if activities == 0 {
        return String::new();
    }
    let body = match bodies {
        Bodies::Read => "... on AgentActivityPromptContent { body }",
        Bodies::Skip => "",
    };
    format!(
        "activities(orderBy: createdAt, first: {activities}, filter: {{ type: {{ eq: \"prompt\" }} }}) {{
        nodes {{
          id createdAt user {{ id }} signal
          content {{
            __typename {body}
          }}
        }}
        pageInfo {{ hasNextPage }}
      }}"
    )
}


// The cursor of a connection's next page, if it has one.
pub fn next_cursor(page: &PageInfo) -> Option<String> {
    page.end_cursor.clone().filter(|c| page.has_next_page && !c.is_empty())
}

// A session as every session query reads it (`session_fields`, `read_fields`, `Through`):
// what a slimmer read leaves out decodes as none.
#[derive(Debug, Deserialize)]
pub struct SessionNode {
    pub id: String,
    #[serde(flatten)]
    pub began: Began,
    pub activities: Option<Nodes<ActivityNode>>,
    pub issue: Option<IssueNode>,
}

// How a session began, which tells a mention's from a delegation's: read so of the session
// itself and of each of its issue's newest sessions.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Began {
    pub created_at: String,
    pub app_user: Ref,
    pub creator: Option<Ref>,
    pub comment: Option<SessionComment>,
    pub source_comment: Option<SessionComment>,
}

// A session's comment or source comment: its text and replies only in a read with bodies.
#[derive(Clone, Debug, Deserialize)]
pub struct SessionComment {
    pub body: Option<String>,
    pub user: Option<Ref>,
    pub children: Option<Nodes<NoteReply>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteReply {
    created_at: String,
    body: Option<String>,
    user: Option<Ref>,
    bot_actor: Option<IgnoredAny>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityNode {
    id: String,
    created_at: String,
    user: Option<Ref>,
    signal: Option<String>,
    content: ActivityContent,
}

// Only a prompt's body is asked for (`prompts`); an action's fields are read where an answer
// has them.
#[derive(Debug, Deserialize)]
struct ActivityContent {
    #[serde(rename = "__typename")]
    typename: String,
    body: Option<String>,
    action: Option<String>,
    parameter: Option<String>,
}

// A session's issue as `issue_fields` and `read_fields` ask for it: what only some reads ask
// for decodes as none.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueNode {
    pub id: String,
    pub identifier: String,
    pub url: String,
    pub branch_name: String,
    pub created_at: String,
    pub updated_at: Option<String>,
    pub creator: Option<Ref>,
    pub canceled_at: Option<String>,
    pub completed_at: Option<String>,
    pub state: StateOf,
    pub team: SessionTeam,
    pub delegate: Option<Ref>,
    pub project: Option<Project>,
    pub estimate: Option<f64>,
    pub priority: Option<f64>,
    pub labels: Option<Nodes<Named>>,
    pub parent: Option<ParentNode>,
    pub children: Option<Nodes<MemberNode>>,
    pub history: Nodes<HistoryEntry>,
    pub agent_sessions: Option<Nodes<Began>>,
    #[serde(flatten)]
    pub relations: Relations<MemberNode>,
}

// Its key only where a read asks for it: a slim one leaves it out.
#[derive(Clone, Debug, Deserialize)]
pub struct SessionTeam {
    pub id: String,
    pub key: Option<String>,
    #[serde(flatten)]
    pub estimation: Estimation,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParentNode {
    pub id: String,
    pub identifier: String,
    pub updated_at: Option<String>,
    pub canceled_at: Option<String>,
    pub state: StateOf,
    pub delegate: Option<Ref>,
    pub priority: Option<f64>,
    pub history: Nodes<HistoryEntry>,
}

// A sub-issue, or a blocker at the other end of a relation: its ID, identifier, link, clock
// and state, never its text.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberNode {
    pub id: Option<String>,
    pub identifier: String,
    pub url: String,
    pub updated_at: Option<String>,
    pub state: StateOf,
    pub completed_at: Option<String>,
    pub canceled_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    created_at: String,
    actor: Option<Ref>,
    from_delegate: Option<Ref>,
    to_delegate: Option<Ref>,
}

// A blocker past the first page must be read before the issue is acted on: an unfinished one
// unseen would start the issue early. So this reads each further page, with `fetch` (issue id,
// cursor) returning MoreRelations' answer, up to RELATION_PAGES pages in all. What is left
// unread, past that or a page Linear won't give or that doesn't decode, stays marked by the
// connection's `hasNextPage` (`Issue::more_blockers`), never an error: one issue with many
// relations must not fail the read of every other. An issue with one page costs no read.
fn complete_relations(issue: &mut IssueNode, fetch: &mut dyn FnMut(&str, &str) -> Result<Value, Error>) -> Result<(), Error> {
    // Its `pageInfo` is required, so a page without one is left unread, never taken for the last.
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Page {
        nodes: Vec<Link<MemberNode>>,
        page_info: PageInfo,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Inverse {
        inverse_relations: Page,
    }
    #[derive(Deserialize)]
    struct Answer {
        issue: Inverse,
    }
    let Some(relations) = issue.relations.inverse_relations.as_mut() else { return Ok(()) };
    for _ in 1..RELATION_PAGES {
        let Some(cursor) = next_cursor(&relations.page_info) else { return Ok(()) };
        let page = match fetch(&issue.id, &cursor) {
            Err(Error::Failed(message)) if message.contains("Could not find cursor") => return Ok(()),
            page => page?,
        };
        let Ok(Answer { issue: Inverse { inverse_relations: page } }) = decode::<Answer>(page, "Linear's answer to MoreRelations") else { return Ok(()) };
        relations.nodes.extend(page.nodes);
        relations.page_info = page.page_info;
    }
    Ok(())
}

fn parse_children(issue: &IssueNode) -> Vec<Blocker> {
    issue.children.iter().flat_map(|c| &c.nodes).map(member).collect()
}

fn parse_parent(issue: &IssueNode, app_user_id: Option<&str>) -> Option<Parent> {
    let parent = issue.parent.as_ref()?;
    Some(Parent {
        id: parent.id.clone(),
        identifier: parent.identifier.clone(),
        state_type: parent.state.kind,
        delegate_id: parent.delegate.as_ref().map(|d| d.id.clone()),
        priority: priority(parent.priority),
        undelegated_at: undelegated_at(&parent.history.nodes, app_user_id),
        canceled_at: finished_at(parent.state.kind, parent.canceled_at.as_ref(), None, StateType::Canceled),
        updated_at: parent.updated_at.clone(),
        delegated_by: delegator(parent.delegate.as_ref(), &parent.history.nodes),
    })
}

// 0 when the issue has no priority or an unknown one.
fn priority(priority: Option<f64>) -> u8 {
    priority.filter(|p| (0.0..=4.0).contains(p)).map_or(0, |p| p as u8)
}

// A relation whose issue is gone (null) holds nothing back: `Relation::related` leaves it out.
fn parse_blockers(issue: &IssueNode) -> Vec<Blocker> {
    Relation::BlockedBy.related(&issue.relations).into_iter().map(|r| member(r.other)).collect()
}

fn member(node: &MemberNode) -> Blocker {
    Blocker { identifier: node.identifier.clone(), url: node.url.clone(), state_type: node.state.kind, id: node.id.clone(), updated_at: node.updated_at.clone() }
}

// An issue's newest history entries read, for `delegated`. Few, as every session carries
// them and Linear caps a query's complexity: an undelegation and the re-delegation after
// it are among the newest changes when an action reads them.
const HISTORY: u32 = 10;
// A parent with more sub-issues than this has some unread, which count as unfinished
// (`Issue::more_children`), so it never picks up again early.
const CHILDREN: u32 = 50;
// An issue's newest sessions read, for `superseded`.
const NEWEST_SESSIONS: u32 = 3;
// An issue's labels read, Linear's default page: one past it goes unseen.
const LABELS: u32 = 50;
// An issue's inverse relations are read a page of RELATIONS at a time, at most RELATION_PAGES
// pages, as the relay reads them (BLOCKER_PAGE, BLOCKER_PAGES in src/shared/linear.ts).
const RELATIONS: u32 = 50;
const RELATION_PAGES: usize = 5;


// Only the state says whether an issue is finished: a reopened one may keep its old
// `canceledAt` or `completedAt`.
fn finished_at(of: StateType, canceled_at: Option<&String>, completed_at: Option<&String>, state: StateType) -> Option<String> {
    match state {
        StateType::Canceled if of.is_cancelled() => canceled_at.cloned(),
        StateType::Completed if of == StateType::Completed => completed_at.cloned(),
        _ => None,
    }
}

// The issue as the app `app_user_id` reads it, with no session: its delegation never asks
// again (`Delegated::again`), which only a session's read can say.
pub fn parse_issue(issue: &IssueNode, app_user_id: Option<&str>) -> Issue {
    let children = issue.children.iter().flat_map(|c| &c.nodes);
    let state = issue.state.kind;
    Issue {
        blockers: parse_blockers(issue),
        more_blockers: issue.relations.inverse_relations.as_ref().is_some_and(|r| r.page_info.has_next_page),
        parent: parse_parent(issue, app_user_id),
        children: parse_children(issue),
        more_children: issue.children.as_ref().is_some_and(|c| c.page_info.has_next_page),
        children_finished_at: children.flat_map(|c| [c.completed_at.clone(), c.canceled_at.clone()]).flatten().max(),
        id: issue.id.clone(),
        identifier: issue.identifier.clone(),
        url: issue.url.clone(),
        branch_name: issue.branch_name.clone(),
        state_type: state,
        team_id: Some(issue.team.id.clone()),
        delegate_id: issue.delegate.as_ref().map(|d| d.id.clone()),
        place: Place::of(issue.team.key.as_deref(), issue.project.as_ref().map(|p| p.parts())),
        estimate: issue.estimate,
        priority: priority(issue.priority),
        scale: Scale::of(&issue.team.estimation),
        labels: issue.labels.iter().flat_map(|l| &l.nodes).map(|l| l.name.clone()).collect(),
        created_at: issue.created_at.clone(),
        delegated: delegated(issue, None),
        undelegated_at: undelegated_at(&issue.history.nodes, app_user_id),
        history_full: issue.history.nodes.len() >= HISTORY as usize,
        created_delegated_by: created_delegated_by(issue, app_user_id),
        canceled_at: finished_at(state, issue.canceled_at.as_ref(), None, StateType::Canceled),
        completed_at: finished_at(state, None, issue.completed_at.as_ref(), StateType::Completed),
    }
}

pub fn parse_session(node: &SessionNode) -> Session {
    let mut session = parse_own(node, node.issue.as_ref());
    session.issue = node.issue.as_ref().map(|issue| Issue { delegated: delegated(issue, Some(&node.began)), ..parse_issue(issue, session.app_user_id.as_deref()) });
    session
}

// The session `node` on `issue_node`, the issue it was read with or through, without parsing
// the issue into it.
fn parse_own(node: &SessionNode, issue_node: Option<&IssueNode>) -> Session {
    let began = &node.began;
    let own_note = on_own_note(began);
    let mention = match mentioning_comment(began) {
        Some(_) if own_note => Some(Mention { author_id: None, body: String::new(), in_description: false }),
        Some(c) => Some(Mention { author_id: c.user.as_ref().map(|u| u.id.clone()), body: c.body.clone().unwrap_or_default(), in_description: false }),
        // Its author is the issue's creator. It has no text: anyone may edit a description since,
        // so it is never passed on as the owner's words, and the issue is the request, as for a
        // delegation.
        None => issue_node.filter(|i| in_description(i, began)).map(|i| Mention { author_id: i.creator.as_ref().map(|c| c.id.clone()), body: String::new(), in_description: true }),
    };
    let mut activities: Vec<Activity> = node
        .activities
        .iter()
        .flat_map(|a| &a.nodes)
        .map(|a| {
            let typename = a.content.typename.as_str();
            let activity_type = ActivityType::from_typename(typename);
            // Anything Linear adds later reads as an answer.
            let kind = match (typename, activity_type) {
                ("AgentActivityPromptContent", _) => Kind::Prompt,
                (_, Some(t)) => t.kind(),
                (_, None) => Kind::Answer,
            };
            let body = match activity_type {
                Some(ActivityType::Action) => a.content.action.clone().map(|action| a.content.parameter.as_deref().filter(|p| !p.is_empty()).map_or_else(|| action.clone(), |p| format!("{action} {p}"))),
                _ => a.content.body.clone(),
            };
            Activity { id: Some(a.id.clone()), created_at: a.created_at.clone(), author_id: a.user.as_ref().map(|u| u.id.clone()), kind, body, signal: a.signal.clone() }
        })
        .collect();
    // Stable, so a tie keeps its order.
    activities.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Session {
        id: node.id.clone(),
        created_at: began.created_at.clone(),
        creator_id: began.creator.as_ref().map(|c| c.id.clone()),
        app_user_id: Some(began.app_user.id.clone()),
        issue: None,
        mention,
        on_own_note: own_note,
        note_replies: if own_note { replies_before(began) } else { Vec::new() },
        activities,
        more_activities: node.activities.as_ref().is_some_and(|a| a.page_info.has_next_page),
        record: Record::default(),
        delegation: None,
    }
}

pub type Fetch<'a> = dyn Fn(&str, Value) -> Result<Value, Error> + Sync + 'a;

// Ordered by createdAt, not updated, so a session updated while the pages are read doesn't
// move between them.
const ISSUE_SESSIONS: &str = "query IssueSessions($id: String!, $after: String) {
  viewer { id }
  issue(id: $id) {
    ISSUE
    sessions: agentSessions(first: PAGE, after: $after, orderBy: createdAt) { nodes { appUser { id } FIELDS } pageInfo { hasNextPage endCursor } }
  }
}";

pub const ISSUE_SESSIONS_PER_PAGE: u32 = 25;

// A session as a read through its issue finds it, whatever else it reads: the app it is of.
pub trait OfApp {
    fn app_user_id(&self) -> Option<&str>;
}

impl OfApp for SessionNode {
    fn app_user_id(&self) -> Option<&str> {
        Some(&self.began.app_user.id)
    }
}

// An ISSUE_SESSIONS answer: the issue as `I`, and its page of sessions as `S`.
#[derive(Deserialize)]
struct SessionsPage<I, S> {
    viewer: Ref,
    issue: Option<WithSessions<I, S>>,
}

#[derive(Deserialize)]
struct WithSessions<I, S> {
    #[serde(flatten)]
    issue: I,
    sessions: Nodes<S>,
}

// Returns Tolkster's sessions on the issue `id`, every page from `after`, with the issue's
// id and Tolkster's app user id.
pub fn issue_sessions<S: DeserializeOwned + OfApp>(fetch: &Fetch, id: &str, fields: &Through, mut after: Option<String>) -> Result<(Vec<S>, String, String), Error> {
    let query = ISSUE_SESSIONS.replace("PAGE", &ISSUE_SESSIONS_PER_PAGE.to_string()).replace("ISSUE", &fields.issue).replace("FIELDS", &fields.session);
    let mut found = Vec::new();
    loop {
        let page: SessionsPage<Ref, S> = decode(fetch(&query, json!({ "id": id, "after": after }))?, &answer_to(&query))?;
        let Some(WithSessions { issue, sessions }) = page.issue else { return Err(Error::Failed(format!("{id}: no such issue"))) };
        let (sessions, cursor) = sessions_of(sessions, &page.viewer.id);
        found.extend(sessions);
        after = cursor;
        if after.is_none() {
            return Ok((found, issue.id, page.viewer.id));
        }
    }
}

// The app `viewer_id`'s sessions of a page read with `Through`, and the cursor of the issue's
// next page. An issue lists every agent's sessions.
fn sessions_of<S: OfApp>(page: Nodes<S>, viewer_id: &str) -> (Vec<S>, Option<String>) {
    let cursor = next_cursor(&page.page_info);
    (page.nodes.into_iter().filter(|s| s.app_user_id() == Some(viewer_id)).collect(), cursor)
}

// The session `session_id` as Linear has it now, as `what` asks, with Tolkster's record of its
// conversation, and the delegation of its issue, when `records` is given (`load_records`).
pub fn read(agent: &Agent, token: &str, records: Option<&State>, session_id: &str, what: Read) -> Result<Session, Error> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Answer {
        agent_session: Option<SessionNode>,
    }
    let query = format!("query ReadSession($id: String!) {{ agentSession(id: $id) {{ {} }} }}", read_fields(what));
    let answer: Answer = read_with(agent, token, &query, json!({ "id": session_id }))?;
    let mut session = session_from(agent, token, answer.agent_session)?;
    if let Some(state) = records {
        load_records(state, std::slice::from_mut(&mut session))?;
    }
    Ok(session)
}

pub fn session_from(agent: &Agent, token: &str, node: Option<SessionNode>) -> Result<Session, Error> {
    let Some(mut node) = node else { return Err(Error::Failed("Linear's response had no session".into())) };
    if let Some(issue) = node.issue.as_mut() {
        complete_relations(issue, &mut more_relations(&|query, variables| graphql(agent, token, query, variables)))?;
    }
    Ok(parse_session(&node))
}

fn more_relations<'a>(fetch: &'a Fetch) -> impl FnMut(&str, &str) -> Result<Value, Error> + 'a {
    let query = more_relations_query();
    move |id, after| fetch(&query, json!({ "id": id, "after": after }))
}

// A session with no record has had nothing recorded yet, and reads as an empty one.
pub fn load_records(state: &State, sessions: &mut [Session]) -> Result<(), Error> {
    load_delegations(state, sessions)?;
    let ids: Vec<String> = sessions.iter().map(|s| s.id.clone()).collect();
    let mut records = state.read(&ids)?;
    for session in sessions.iter_mut() {
        session.record = records.remove(&session.id).flatten().unwrap_or_default();
    }
    Ok(())
}

// Gives each session on a sub-issue that isn't a mention's, whoever started it, the
// delegation Tolkster recorded for its issue, if it made one.
pub fn load_delegations(state: &State, sessions: &mut [Session]) -> Result<(), Error> {
    let wanted = |s: &Session| s.mention.is_none() && s.issue.as_ref().is_some_and(|i| i.parent.is_some());
    let mut issues: Vec<Id> = sessions.iter().filter(|s| wanted(s)).filter_map(|s| s.issue.as_ref().map(|i| Id::check(&i.id))).collect::<Result<_, _>>()?;
    issues.sort();
    issues.dedup();
    if issues.is_empty() {
        return Ok(());
    }
    let delegations = state.delegations(&issues)?;
    for session in sessions.iter_mut().filter(|s| wanted(s)) {
        session.delegation = session.issue.as_ref().and_then(|i| delegations.get(&i.id).cloned());
    }
    Ok(())
}

// How many of a session's newest activities a read takes: a command's, and `tolkster
// messages`'s, which prints them.
pub const PENDING_ACTIVITIES: u32 = 20;
pub const MESSAGES_ACTIVITIES: u32 = 100;


// The issue `identifier`, its `updatedAt`, Tolkster's sessions on it and the viewer id: what
// a command's `readIssue` answers, for an issue with no session of Tolkster's too.
pub fn read_issue_facts(agent: &Agent, identifier: &str) -> Result<(Issue, String, Vec<Session>, String), Error> {
    with_token(agent, |token| issue_facts(&|query, variables| graphql(agent, token, query, variables), identifier))
}

// One query reads the issue, its first page of relations and of sessions; only more relations
// or sessions than a page cost a further read, and a session's is of the session alone. Each
// session is read without its activities, its issue or a record, which `readIssue` never
// sends.
fn issue_facts(fetch: &Fetch, identifier: &str) -> Result<(Issue, String, Vec<Session>, String), Error> {
    let query = ISSUE_SESSIONS.replace("PAGE", &ISSUE_SESSIONS_PER_PAGE.to_string()).replace("ISSUE", &issue_fields()).replace("FIELDS", SLIM_OWN);
    let page: SessionsPage<IssueNode, SessionNode> = decode(fetch(&query, json!({ "id": identifier, "after": null }))?, &answer_to(&query))?;
    let Some(WithSessions { issue: mut issue_node, sessions }) = page.issue else { return Err(Error::Failed(format!("{identifier}: no such issue"))) };
    let viewer_id = page.viewer.id;
    let (mut nodes, after) = sessions_of(sessions, &viewer_id);
    complete_relations(&mut issue_node, &mut more_relations(fetch))?;
    // `issue_fields` asks for it, which a session's slim read doesn't.
    let updated_at = issue_node.updated_at.clone().ok_or_else(|| Error::Failed(format!("{} didn't decode: it had no `updatedAt`", answer_to(&query))))?;
    let issue = parse_issue(&issue_node, Some(&viewer_id));
    if after.is_some() {
        let (more, _, _) = issue_sessions::<SessionNode>(fetch, &issue.id, &Through { issue: "id".into(), session: SLIM_OWN.into() }, after)?;
        nodes.extend(more);
    }
    let sessions = nodes.iter().map(|n| parse_own(n, Some(&issue_node))).collect();
    Ok((issue, updated_at, sessions, viewer_id))
}

#[cfg(test)]
mod tests;
