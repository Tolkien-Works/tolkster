// What the CLI needs from Linear: an app-actor token, raw GraphQL, and the shape of an
// agent activity. Linear has no Rust SDK; this mirrors src/shared/linear.ts.

use std::collections::HashMap;
use std::panic::Location;
use std::time::Instant;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use ureq::Agent;
use ureq::http::Method;

use crate::args::js_trim;
use crate::config::WorkerUrl;
use crate::error::Error;
use crate::gateway::{self, Api};
use crate::http::{self, send_json, url_part};
use crate::json::{decode, loose, loose_each};
use crate::plain::IssueKey;
use crate::time::now_ms;
use crate::trace::{self, Call};

mod cost;
mod release;
mod token;
use cost::Limits;
pub use release::create_release_pipeline;
#[cfg(feature = "develop")]
pub use release::sync_release;
pub use token::with_token;

const GRAPHQL_URL: &str = "https://api.linear.app/graphql";

// Identical to SCOPES in src/shared/linear.ts, which a test enforces: Linear revokes all
// of an app's actor tokens when one is requested with a different set.
pub const SCOPES: &str = "read,write,app:assignable,app:mentionable";

// Where an admin installs the app, or grants it another team. The client ID is in every such
// URL, so not a secret.
pub fn install_url(client_id: &str, worker: &WorkerUrl) -> String {
    format!("https://linear.app/oauth/authorize?client_id={client_id}&redirect_uri={}&response_type=code&scope={SCOPES}&actor=app", worker.oauth_callback())
}

// The webhook types the Worker handles (`WEBHOOK_TYPES` in src/shared/linear.ts, pinned in
// test/contract.json): the Linear app is subscribed to these and no others.
pub const WEBHOOK_TYPES: [&str; 3] = ["AgentSessionEvent", "Comment", "Issue"];

// The OAuth app manifest (https://linear.app/developers/oauth-app-manifests) for an agent named
// `name`: private, with the Worker's callback and webhook, and both grants the Worker and the CLI use.
pub fn manifest(name: &str, worker: &WorkerUrl) -> Value {
    json!({
        "schemaVersion": "1.0.0",
        "distribution": "private",
        "oauth": {
            "client_name": name,
            "client_uri": worker.base(),
            "redirect_uris": [worker.oauth_callback()],
            "grant_types": ["authorization_code", "client_credentials"],
        },
        "developer": { "name": name },
        "webhook": { "url": worker.linear_webhook(), "enabled": true, "resourceTypes": WEBHOOK_TYPES },
    })
}

// Where an admin makes the app, its form filled in from the manifest.
pub fn create_url(name: &str, worker: &WorkerUrl) -> String {
    format!("https://linear.app/settings/api/applications/new?manifest={}", url_part(&manifest(name, worker).to_string()))
}

// The activity types Tolkster posts, as `src/shared/linear.ts` has them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ActivityType {
    Thought,
    Elicitation,
    Response,
    Error,
    Action,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Prompt,
    // A thought or an action: work in progress, not an answer.
    Progress,
    Answer,
}

impl ActivityType {
    // Ordered as `tolkster activity`'s usage lists them.
    pub const ALL: [ActivityType; 5] = [ActivityType::Thought, ActivityType::Elicitation, ActivityType::Response, ActivityType::Error, ActivityType::Action];

    pub fn as_str(self) -> &'static str {
        match self {
            ActivityType::Thought => "thought",
            ActivityType::Elicitation => "elicitation",
            ActivityType::Response => "response",
            ActivityType::Error => "error",
            ActivityType::Action => "action",
        }
    }

    pub fn answers(self) -> bool {
        self.kind() == Kind::Answer
    }

    // Linear accepts ephemeral only on thoughts and actions.
    pub fn may_be_ephemeral(self) -> bool {
        matches!(self, ActivityType::Thought | ActivityType::Action)
    }

    pub fn kind(self) -> Kind {
        match self {
            ActivityType::Thought | ActivityType::Action => Kind::Progress,
            ActivityType::Elicitation | ActivityType::Response | ActivityType::Error => Kind::Answer,
        }
    }

    // None for a prompt, or for a type Linear adds later.
    pub fn from_typename(typename: &str) -> Option<ActivityType> {
        match typename {
            "AgentActivityThoughtContent" => Some(ActivityType::Thought),
            "AgentActivityElicitationContent" => Some(ActivityType::Elicitation),
            "AgentActivityResponseContent" => Some(ActivityType::Response),
            "AgentActivityErrorContent" => Some(ActivityType::Error),
            "AgentActivityActionContent" => Some(ActivityType::Action),
            _ => None,
        }
    }
}

impl std::fmt::Display for ActivityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ActivityType {
    type Err = ();

    fn from_str(s: &str) -> Result<ActivityType, ()> {
        ActivityType::ALL.into_iter().find(|t| t.as_str() == s).ok_or(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ActivityContent {
    // thought, elicitation, response or error
    Text { kind: ActivityType, body: String },
    Action { action: String, parameter: String, result: Option<String> },
}

// An elicitation's select options, which Linear offers as buttons. A pick comes back as the
// owner's prompt, holding its label or its value, so the two read as the same reply, the one
// the brief waits for; typing it does the same. A worker names an option, never writes one.
#[derive(Debug, PartialEq)]
pub struct Choice {
    pub name: &'static str,
    pub label: &'static str,
    pub value: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActivityInput {
    pub agent_session_id: String,
    pub content: ActivityContent,
    pub ephemeral: bool,
    // Chosen beforehand so a writer can find the activity again (state::Posting); None
    // lets Linear choose.
    pub id: Option<String>,
    // An elicitation's select options; none sends no signal.
    pub options: Vec<&'static Choice>,
}

impl ActivityInput {
    pub fn text(session_id: &str, kind: ActivityType, body: &str, ephemeral: bool) -> ActivityInput {
        ActivityInput { agent_session_id: session_id.into(), content: ActivityContent::Text { kind, body: body.into() }, ephemeral, id: None, options: vec![] }
    }

    // Durable, with no result.
    pub fn action(session_id: &str, action: &str, parameter: &str) -> ActivityInput {
        ActivityInput {
            agent_session_id: session_id.into(),
            content: ActivityContent::Action { action: action.into(), parameter: parameter.into(), result: None },
            ephemeral: false,
            id: None,
            options: vec![],
        }
    }

    fn to_json(&self) -> Value {
        let content = match &self.content {
            ActivityContent::Text { kind, body } => json!({ "type": kind.as_str(), "body": body }),
            ActivityContent::Action { action, parameter, result } => {
                let mut content = json!({ "type": ActivityType::Action.as_str(), "action": action, "parameter": parameter });
                if let Some(result) = result {
                    content["result"] = json!(result);
                }
                content
            }
        };
        let mut input = json!({ "agentSessionId": self.agent_session_id, "content": content });
        if self.ephemeral {
            input["ephemeral"] = json!(true);
        }
        if let Some(id) = &self.id {
            input["id"] = json!(id);
        }
        if !self.options.is_empty() {
            let options: Vec<Value> = self.options.iter().map(|c| json!({ "label": c.label, "value": c.value })).collect();
            input["signal"] = json!("select");
            input["signalMetadata"] = json!({ "options": options });
        }
        input
    }
}

// A session's plan: the checklist Linear shows on the session. Linear takes the whole
// plan on every update, never one step.
pub const PLAN_STATUSES: [&str; 4] = ["pending", "inProgress", "completed", "canceled"];

#[derive(Clone, Debug, PartialEq)]
pub struct PlanStep {
    pub content: String,
    pub status: &'static str,
    // What ends the step, for the hook to move the plan on (hook::whereabouts). Never sent:
    // Linear's plan has no such field.
    pub until: Option<&'static str>,
}

// A plan's steps from its JSON, as `tolkster plan` takes them, or the usage error that says
// what is wrong. The hook reads a worker's plan with it too (hook::whereabouts).
pub fn steps(value: &Value) -> Result<Vec<PlanStep>, Error> {
    let Some(items) = value.as_array() else {
        return Err(Error::Usage("the plan must be a JSON array of steps".into()));
    };
    if items.is_empty() {
        return Err(Error::Usage("the plan has no steps".into()));
    }
    let mut steps = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let n = i + 1;
        let Some(fields) = item.as_object() else {
            return Err(Error::Usage(format!("step {n} must be an object with content and status")));
        };
        if let Some(other) = fields.keys().find(|k| !matches!(k.as_str(), "content" | "status" | "until")) {
            return Err(Error::Usage(format!("step {n} has an unknown field '{other}': a step has only content, status and until")));
        }
        let Some(content) = fields.get("content").and_then(Value::as_str).map(js_trim).filter(|c| !c.is_empty()) else {
            return Err(Error::Usage(format!("step {n} needs its content as text")));
        };
        let status = fields.get("status").and_then(Value::as_str).and_then(plan_status);
        let Some(status) = status else {
            return Err(Error::Usage(format!("step {n}'s status must be one of: {}", PLAN_STATUSES.join(", "))));
        };
        let until = match fields.get("until") {
            None => None,
            Some(until) => match until.as_str().and_then(|u| UNTILS.iter().find(|known| **known == u)) {
                Some(until) => Some(*until),
                None => return Err(Error::Usage(format!("step {n}'s until must be one of: {}", UNTILS.join(", ")))),
            },
        };
        steps.push(PlanStep { content: content.into(), status, until });
    }
    if steps.iter().filter(|s| s.status == "inProgress").count() > 1 {
        return Err(Error::Usage("only one step can be inProgress".into()));
    }
    Ok(steps)
}

// The events that end a step, as the hook sees them (hook::whereabouts): a commit, the
// pre-push review's verdict and the PR's opening. Each is a git commit or a `tolkster`
// call, so any repo has them; a repo's gate is its own commands, and the PR opens only once
// it has passed.
pub const UNTILS: [&str; 3] = ["commit", "review", "pr"];

// A step's status as Linear spells it, ignoring case, spaces, `-` and `_`.
fn plan_status(status: &str) -> Option<&'static str> {
    let key: String = status.chars().filter(|c| !matches!(c, ' ' | '-' | '_')).flat_map(char::to_lowercase).collect();
    match key.as_str() {
        "pending" | "todo" => Some("pending"),
        "inprogress" => Some("inProgress"),
        "completed" | "complete" | "done" => Some("completed"),
        "canceled" | "cancelled" => Some("canceled"),
        _ => None,
    }
}

#[derive(Debug, PartialEq)]
pub struct PlanInput {
    pub agent_session_id: String,
    pub steps: Vec<PlanStep>,
}

impl PlanInput {
    pub fn to_json(&self) -> Value {
        let plan: Vec<Value> = self.steps.iter().map(|s| json!({ "content": s.content, "status": s.status })).collect();
        json!({ "id": self.agent_session_id, "input": { "plan": plan } })
    }
}

const UPDATE_SESSION: &str = "mutation SessionUpdate($id: String!, $input: AgentSessionUpdateInput!) {
  agentSessionUpdate(id: $id, input: $input) { success }
}";

pub fn update_plan(agent: &Agent, access_token: &str, input: &PlanInput) -> Result<(), Error> {
    mutate::<Success>(agent, access_token, "agentSessionUpdate", UPDATE_SESSION, input.to_json()).map(|_| ())
}

// An external link on session `session_id`, e.g. the PR.
pub fn add_session_link(agent: &Agent, access_token: &str, session_id: &str, label: &str, url: &str) -> Result<(), Error> {
    let variables = json!({ "id": session_id, "input": { "addedExternalUrls": [{ "label": label, "url": url }] } });
    mutate::<Success>(agent, access_token, "agentSessionUpdate", UPDATE_SESSION, variables).map(|_| ())
}

const CREATE_ACTIVITY: &str = "mutation AgentActivityCreate($input: AgentActivityCreateInput!) {
  agentActivityCreate(input: $input) { success agentActivity { createdAt } }
}";

// Returns when Linear created the activity, in its timestamp format, which state.rs
// keeps.
pub fn create_activity(agent: &Agent, access_token: &str, input: &ActivityInput) -> Result<String, Error> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Activity {
        created_at: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Created {
        agent_activity: Activity,
    }
    let created: Created = mutate(agent, access_token, "agentActivityCreate", CREATE_ACTIVITY, json!({ "input": input.to_json() }))?;
    Ok(created.agent_activity.created_at)
}

// Activities in one request, in their order, as a mutation's fields run in turn: a traced
// call's thought and action spend one request of the budget. Each must be created.
pub fn create_activities(agent: &Agent, access_token: &str, inputs: &[&ActivityInput]) -> Result<(), Error> {
    let (names, query, variables) = activities_request(inputs);
    let created = graphql(agent, access_token, &query, variables.clone()).and_then(|data| {
        // Each alias's payload, by alias.
        let created: HashMap<String, Success> = decode(data, &answer_to(&query))?;
        match names.iter().find(|a| !created.get(*a).is_some_and(|c| c.success)) {
            Some(_) => Err(Error::Failed("agentActivityCreate failed: Linear answered success: false".into())),
            None => Ok(()),
        }
    });
    trace::event(Call::Wrote { site: "linear agentActivityCreate".into(), detail: trace::ids(&variables), failed: created.as_ref().err().cloned() });
    created
}

// Each input under its own alias, `a0` first, and the mutation that creates them in turn.
fn activities_request(inputs: &[&ActivityInput]) -> (Vec<String>, String, Value) {
    let names: Vec<String> = (0..inputs.len()).map(|n| format!("a{n}")).collect();
    let params: Vec<String> = names.iter().map(|a| format!("${a}: AgentActivityCreateInput!")).collect();
    let fields: Vec<String> = names.iter().map(|a| format!("{a}: agentActivityCreate(input: ${a}) {{ success }}")).collect();
    let query = format!("mutation AgentActivitiesCreate({}) {{ {} }}", params.join(", "), fields.join(" "));
    let variables = Value::Object(names.iter().cloned().zip(inputs.iter().map(|input| input.to_json())).collect());
    (names, query, variables)
}

const CREATE_COMMENT: &str = "mutation Comment($input: CommentCreateInput!) { commentCreate(input: $input) { success comment { id createdAt } } }";

// A comment's ID and Linear's time for it, each empty when Linear's answer left the comment
// out: a post that landed is never failed by its answer.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reply {
    pub id: String,
    #[serde(rename = "createdAt")]
    pub at: String,
}

// Posts `body` as the app's comment on issue `issue_id`, in the thread whose root is
// `parent_id` when there is one. `id`, when given, is the comment's own, which Linear refuses
// once it has it.
#[track_caller]
pub fn create_comment(agent: &Agent, access_token: &str, issue_id: &str, parent_id: Option<&str>, body: &str, id: Option<&str>) -> Result<Reply, Error> {
    let mut input = json!({ "issueId": issue_id, "body": body });
    if let Some(parent_id) = parent_id {
        input["parentId"] = json!(parent_id);
    }
    if let Some(id) = id {
        input["id"] = json!(id);
    }
    #[derive(Deserialize)]
    struct Posted {
        comment: Option<Reply>,
    }
    let posted: Posted = mutate(agent, access_token, "commentCreate", CREATE_COMMENT, json!({ "input": input }))?;
    Ok(posted.comment.unwrap_or_default())
}

const UPDATE_COMMENT: &str = "mutation CommentUpdate($id: String!, $input: CommentUpdateInput!) { commentUpdate(id: $id, input: $input) { success } }";

// Replaces the body of the app's comment `comment_id`. Linear notifies no one of an edit.
#[track_caller]
pub fn update_comment(agent: &Agent, access_token: &str, comment_id: &str, body: &str) -> Result<(), Error> {
    mutate::<Success>(agent, access_token, "commentUpdate", UPDATE_COMMENT, json!({ "id": comment_id, "input": { "body": body } })).map(|_| ())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Relation {
    Blocks,
    BlockedBy,
    Related,
    DuplicateOf,
}

impl Relation {
    // Linear has no blocked-by type: "A is blocked by B" is "B blocks A".
    pub fn input(self, issue: &str, other: &str) -> Value {
        let (from, to, kind) = match self {
            Relation::Blocks => (issue, other, "blocks"),
            Relation::BlockedBy => (other, issue, "blocks"),
            Relation::Related => (issue, other, "related"),
            Relation::DuplicateOf => (issue, other, "duplicate"),
        };
        json!({ "issueId": from, "relatedIssueId": to, "type": kind })
    }
}

const RELATE: &str = "mutation Relate($input: IssueRelationCreateInput!) { issueRelationCreate(input: $input) { success } }";

// Relates issue `a` to `b` as `relation` says: `Relation::BlockedBy` is "a is blocked by b".
#[track_caller]
pub fn relate(agent: &Agent, access_token: &str, relation: Relation, a: &str, b: &str) -> Result<(), Error> {
    mutate::<Success>(agent, access_token, "issueRelationCreate", RELATE, json!({ "input": relation.input(a, b) })).map(|_| ())
}

const CREATE_ISSUE: &str = "mutation Create($input: IssueCreateInput!) { issueCreate(input: $input) { success issue { id identifier url } } }";

#[derive(Debug, Deserialize)]
pub struct CreatedIssue {
    pub id: String,
    pub identifier: String,
    pub url: String,
}

// The new issue's id, identifier and url.
#[track_caller]
pub fn create_issue(agent: &Agent, access_token: &str, input: Value) -> Result<CreatedIssue, Error> {
    #[derive(Deserialize)]
    struct Created {
        issue: CreatedIssue,
    }
    Ok(mutate::<Created>(agent, access_token, "issueCreate", CREATE_ISSUE, json!({ "input": input }))?.issue)
}

const DELETE_RELATION: &str = "mutation Unrelate($id: String!) { issueRelationDelete(id: $id) { success } }";

#[track_caller]
pub fn delete_relation(agent: &Agent, access_token: &str, relation_id: &str) -> Result<(), Error> {
    mutate::<Success>(agent, access_token, "issueRelationDelete", DELETE_RELATION, json!({ "id": relation_id })).map(|_| ())
}

const UPDATE_ISSUE: &str = "mutation IssueUpdate($id: String!, $input: IssueUpdateInput!) { issueUpdate(id: $id, input: $input) { success } }";

#[track_caller]
pub fn update_issue(agent: &Agent, access_token: &str, issue_id: &str, input: Value) -> Result<(), Error> {
    mutate::<Success>(agent, access_token, "issueUpdate", UPDATE_ISSUE, json!({ "id": issue_id, "input": input })).map(|_| ())
}

const CREATE_LABEL: &str = "mutation LabelCreate($input: IssueLabelCreateInput!) { issueLabelCreate(input: $input) { success issueLabel { id } } }";

// A label named `name` on the team `team_id`, or the workspace's without one; a group when
// `group`, else a child of `parent_id` when one is given. Its ID.
#[track_caller]
pub fn create_label(agent: &Agent, access_token: &str, name: &str, team_id: Option<&str>, group: bool, parent_id: Option<&str>) -> Result<String, Error> {
    let mut input = json!({ "name": name });
    for (key, value) in [("teamId", team_id.map(Value::from)), ("parentId", parent_id.map(Value::from)), ("isGroup", group.then_some(Value::Bool(true)))] {
        if let Some(value) = value {
            input[key] = value;
        }
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Created {
        issue_label: Ref,
    }
    Ok(mutate::<Created>(agent, access_token, "issueLabelCreate", CREATE_LABEL, json!({ "input": input }))?.issue_label.id)
}

// One read of `query` under a token the caller holds, decoded into its shape
// (`json::decode`), the error naming the query. Logged under the module that called it
// (`cost::site`).
#[track_caller]
pub fn read_with<T: DeserializeOwned>(agent: &Agent, token: &str, query: &str, variables: Value) -> Result<T, Error> {
    decode(graphql_at(agent, token, query, variables, Location::caller())?, &answer_to(query))
}

// `read_with` under a token of its own. `with_token` runs a closure, which `#[track_caller]`
// doesn't pass through, so the caller is taken here and handed to `graphql_at`.
#[track_caller]
pub fn read_as<T: DeserializeOwned>(agent: &Agent, query: &str, variables: Value) -> Result<T, Error> {
    let caller = Location::caller();
    with_token(agent, |token| decode(graphql_at(agent, token, query, variables.clone(), caller)?, &answer_to(query)))
}

// What a decode error names a query by, e.g. "Linear's answer to Closed".
pub fn answer_to(query: &str) -> String {
    format!("Linear's answer to {}", operation(query))
}

// A connection: its `nodes`, and its `pageInfo` where the query asks for it, else no more.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Nodes<T> {
    pub nodes: Vec<T>,
    #[serde(default)]
    pub page_info: PageInfo,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub has_next_page: bool,
    pub end_cursor: Option<String>,
}

// What a node names another by, e.g. `delegate { id }`.
#[derive(Clone, Debug, Deserialize)]
pub struct Ref {
    pub id: String,
}

// An issue named by `{ id identifier }`.
#[derive(Clone, Debug, Deserialize)]
pub struct IssueRef {
    pub id: String,
    pub identifier: String,
}

// A label, or anything else, by its name alone.
#[derive(Clone, Debug, Deserialize)]
pub struct Named {
    pub name: String,
}

// Decoded by name, so a type Linear adds later is `Other` rather than a failed read.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(from = "String")]
pub enum StateType {
    Triage,
    Backlog,
    Unstarted,
    Started,
    Completed,
    Canceled,
    // Linear's Duplicate status: it has ended, as a cancelled issue has.
    Duplicate,
    // Anything Linear adds later: neither unstarted nor finished.
    Other,
}

impl StateType {
    const NAMED: [(StateType, &str); 7] = [
        (StateType::Triage, "triage"),
        (StateType::Backlog, "backlog"),
        (StateType::Unstarted, "unstarted"),
        (StateType::Started, "started"),
        (StateType::Completed, "completed"),
        (StateType::Canceled, "canceled"),
        (StateType::Duplicate, "duplicate"),
    ];

    pub fn is_unstarted(self) -> bool {
        matches!(self, StateType::Triage | StateType::Backlog | StateType::Unstarted)
    }

    pub fn is_finished(self) -> bool {
        matches!(self, StateType::Completed) || self.is_cancelled()
    }

    pub fn is_cancelled(self) -> bool {
        matches!(self, StateType::Canceled | StateType::Duplicate)
    }
}

impl From<String> for StateType {
    fn from(name: String) -> StateType {
        StateType::from(name.as_str())
    }
}

impl From<&str> for StateType {
    fn from(name: &str) -> StateType {
        Self::NAMED.iter().find(|(_, n)| *n == name).map_or(StateType::Other, |(t, _)| *t)
    }
}

// `state { type }`.
#[derive(Clone, Debug, Deserialize)]
pub struct StateOf {
    #[serde(rename = "type")]
    pub kind: StateType,
}

// `project { id name }`, which `repo_map::Place` maps by.
#[derive(Clone, Debug, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
}

impl Project {
    pub fn parts(&self) -> (&str, &str) {
        (&self.id, &self.name)
    }
}

// `team { key }`.
#[derive(Clone, Debug, Deserialize)]
pub struct TeamKey {
    pub key: String,
}

// Where Linear keeps what people upload. Its files are private: reading one takes the app
// token, as the API does.
pub const UPLOADS: &str = "https://uploads.linear.app/";

// Whether `url` is an upload: on Linear's upload host, with a path of URL characters only,
// so no backslash, `@` or space can make a client read another host.
pub fn is_upload(url: &str) -> bool {
    url.strip_prefix(UPLOADS).is_some_and(|path| !path.is_empty() && path.chars().all(is_url_char))
}

pub fn is_url_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-._~/%?=&+".contains(c)
}

// An upload's content type and bytes, at most `max_bytes` of them. The token goes to
// `UPLOADS` alone: any other URL is refused before a request, and `file_agent` drops it
// on a redirect. Not a GraphQL call, so neither budget is spent.
pub fn download(agent: &Agent, url: &str, max_bytes: u64) -> Result<(String, Vec<u8>), Error> {
    if !is_upload(url) {
        return Err(Error::Usage("only a Linear upload is fetched".into()));
    }
    let files = http::file_agent();
    with_token(agent, |token| {
        let mut response = send_json(&files, Method::GET, url, token, &[], None)?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(Error::Failed(format!("HTTP {status}")));
        }
        let mime = response.body().mime_type().unwrap_or_default().to_string();
        let bytes = response.body_mut().with_config().limit(max_bytes).read_to_vec()?;
        Ok((mime, bytes))
    })
}

// Linear's two budgets, both of which every call spends.
const BUDGETS: [&str; 2] = ["requests", "complexity"];

// The status, the body and the budgets' headers, which are logged under `site`
// (`cost::line`), whatever the status. The gateway admits the call first (gateway.rs), a
// query as a read, and the response's counts are the gateway's.
fn post_graphql(agent: &Agent, access_token: &str, query: &str, variables: Value, site: &str) -> Result<(u16, String, Limits), Error> {
    gateway::admit(Api::Linear, &BUDGETS, !query.trim_start().starts_with("mutation"))?;
    let body = json!({ "query": query, "variables": variables }).to_string();
    let started = Instant::now();
    let sent = send_json(agent, Method::POST, GRAPHQL_URL, access_token, &[], Some(&body));
    trace::slow(|| format!("linear {}", operation(query)), started);
    let mut response = sent?;
    let status = response.status().as_u16();
    let limits = Limits::read(|name| http::header(&response, name));
    if let Some(cost) = cost::line(site, &limits) {
        trace::event(Call::Cost(cost));
    }
    gateway::note(Api::Linear, "requests", limits.requests_limit, limits.requests_remaining, limits.requests_reset);
    gateway::note(Api::Linear, "complexity", limits.complexity_limit, limits.complexity_remaining, limits.complexity_reset);
    let text = response.body_mut().read_to_string()?;
    Ok((status, text, limits))
}

// The name a query or mutation gives itself in the code, e.g. `Recent` for `query
// Recent($after: String)`, which a slow call and a call's cost are logged under.
fn operation(query: &str) -> &str {
    let query = query.trim_start();
    let named = query.strip_prefix("query").or_else(|| query.strip_prefix("mutation")).map_or("", str::trim_start);
    let end = named.find(|c: char| !c.is_ascii_alphanumeric() && c != '_').unwrap_or(named.len());
    Some(&named[..end]).filter(|name| !name.is_empty()).unwrap_or("unnamed")
}

// A mutation's payload, whatever else it returns.
#[derive(Deserialize)]
struct Success {
    success: bool,
}

// Linear answers `success: false` for some refusals, so every mutation goes through
// here. Returns `data.<field>` as `T`. The trace line holds the field and the variables'
// IDs, never a body.
#[track_caller]
pub fn mutate<T: DeserializeOwned>(agent: &Agent, access_token: &str, field: &str, query: &str, variables: Value) -> Result<T, Error> {
    let result = mutate_once(agent, access_token, field, query, &variables);
    trace::event(Call::Wrote { site: format!("linear {field}"), detail: trace::ids(&variables), failed: result.as_ref().err().cloned() });
    decode(result?, &answer_to(query))
}

#[track_caller]
fn mutate_once(agent: &Agent, access_token: &str, field: &str, query: &str, variables: &Value) -> Result<Value, Error> {
    let data = graphql(agent, access_token, query, variables.clone()).map_err(|e| match e {
        Error::Failed(message) => Error::Failed(format!("{field} failed: {message}")),
        other => other,
    })?;
    // The mutation's payloads, by field.
    let mut data: HashMap<String, Value> = decode(data, &answer_to(query))?;
    let payload = data.remove(field).unwrap_or_default();
    if !decode::<Success>(payload.clone(), &answer_to(query)).is_ok_and(|p| p.success) {
        return Err(Error::Failed(format!("{field} failed: Linear answered success: false")));
    }
    Ok(payload)
}

// Logged under the module that called it and the operation's name (`cost::site`).
#[track_caller]
pub fn graphql(agent: &Agent, access_token: &str, query: &str, variables: Value) -> Result<Value, Error> {
    graphql_at(agent, access_token, query, variables, Location::caller())
}

fn graphql_at(agent: &Agent, access_token: &str, query: &str, variables: Value, caller: &Location) -> Result<Value, Error> {
    let site = cost::site(query, caller);
    let (status, text, limits) = post_graphql(agent, access_token, query, variables, &site)?;
    let answered = answer(status, &text, &limits, now_ms() as u64);
    // Linear says which budget is spent only by its count, so both wait: a call spends both.
    if let Some(wait) = answered.as_ref().err().and_then(Error::rate_limited) {
        gateway::spent(Api::Linear, &BUDGETS, wait);
    }
    answered
}

// A GraphQL response. `data` is the query's own answer, which its caller decodes into the
// query's shape (`read_as`), so it is open here. Each error and its fields are read loosely:
// one of another shape mustn't hide a rejected token or a spent budget another names, and
// still counts as an error.
#[derive(Deserialize)]
struct Response {
    data: Option<Value>,
    #[serde(default, deserialize_with = "loose_each")]
    errors: Vec<GraphqlError>,
}

#[derive(Default, Deserialize)]
struct GraphqlError {
    #[serde(default, deserialize_with = "loose")]
    message: Option<String>,
    #[serde(default, deserialize_with = "loose")]
    extensions: Extensions,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Extensions {
    #[serde(rename = "type", default, deserialize_with = "loose")]
    kind: Option<String>,
    #[serde(default, deserialize_with = "loose")]
    code: Option<String>,
    #[serde(default, deserialize_with = "loose")]
    user_presentable_message: Option<String>,
}

// `data`, or why not. A rejected token is HTTP 401 or an error of type "authentication
// error" (Linear sends a bad token as HTTP 400 with one, code AUTHENTICATION_ERROR too,
// as src/shared/linear.ts checks). Nothing else counts, so a retry mints a new token only
// when Linear blames the token. A spent budget is `Error::RateLimited` with its wait: Linear
// answers it HTTP 400 with an error of code `RATELIMITED`.
fn answer(status: u16, text: &str, limits: &Limits, now_ms: u64) -> Result<Value, Error> {
    let rejected = || Error::Unauthorized("Linear rejected the app token".into());
    if status == 401 {
        return Err(rejected());
    }
    if status == 429 {
        return Err(spent(limits, now_ms));
    }
    let body = serde_json::from_str::<Response>(text).ok();
    let errors = body.as_ref().map(|b| b.errors.as_slice()).filter(|e| !e.is_empty());
    if let Some(errors) = errors {
        let code = |e: &GraphqlError, code: &str| e.extensions.code.as_deref() == Some(code);
        if errors.iter().any(|e| e.extensions.kind.as_deref() == Some("authentication error") || code(e, "AUTHENTICATION_ERROR")) {
            return Err(rejected());
        }
        if errors.iter().any(|e| code(e, "RATELIMITED")) {
            return Err(spent(limits, now_ms));
        }
        let first = &errors[0];
        if let Some(message) = first.extensions.user_presentable_message.as_ref().or(first.message.as_ref()) {
            return Err(Error::Failed(message.clone()));
        }
    }
    let erred = errors.is_some();
    match body.and_then(|b| b.data).filter(|d| !d.is_null()) {
        Some(data) if (200..300).contains(&status) && !erred => Ok(data),
        _ => Err(Error::Failed(format!("HTTP {status}: {text}"))),
    }
}

fn spent(limits: &Limits, now_ms: u64) -> Error {
    let wait = limits.wait(now_ms);
    Error::RateLimited { message: format!("Linear's rate limit is spent: it resets in {} s", wait.as_secs()), wait }
}

// An IssueFilter for `identifiers`. Each pair goes inside `and`: as sibling fields inside
// `or`, Linear drops `number` and matches the whole team.
pub fn identifier_filter(identifiers: &[IssueKey]) -> Value {
    let issues: Vec<Value> = identifiers
        .iter()
        .filter_map(|i| crate::plain::split_identifier(i))
        .map(|(key, number)| json!({ "and": [{ "team": { "key": { "eq": key.to_ascii_uppercase() } } }, { "number": { "eq": number } }] }))
        .collect();
    json!({ "or": issues })
}

#[cfg(test)]
mod tests;
