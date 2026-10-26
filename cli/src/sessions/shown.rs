// What `issue show`, `start` and `issue list` read: an issue as it is printed, and a page
// of issues found.

use serde::Deserialize;
use serde_json::{Value, json};
use ureq::Agent;

use crate::error::Error;
use crate::linear::{Nodes, read_as};

// What every issue is read with: its fields, its parent and both ends of its relations.
const ISSUE_FIELDS: &str = "id identifier title url description priority priorityLabel estimate dueDate branchName sortOrder
    createdAt updatedAt completedAt canceledAt
    state { name type } team { key name } project { name } assignee { id displayName } delegate { id name }
    labels { nodes { name } } parent { identifier title url }
    relations(first: 250) { nodes { type relatedIssue { identifier url state { type } } } }
    inverseRelations(first: 250) { nodes { type issue { identifier url state { type } } } }";

// How many comments, sub-issues and links are read: one page each.
const PAGE: u32 = 250;
const LINKS: u32 = 50;

// The read of an issue, with what the flags ask for, built once for every issue shown.
pub struct Show(String);

impl Show {
    pub fn new(comments: bool, children: bool, attachments: bool) -> Show {
        Show(format!("query Show($id: String!) {{ issue(id: $id) {{ {} }} }}", fields(comments, children, attachments)))
    }

    // The issue as Linear answers it, None when it has none: printed as read
    // (commands/show.rs), so its shape stays open here.
    #[track_caller]
    pub fn read(&self, agent: &Agent, id: &str) -> Result<Option<Value>, Error> {
        #[derive(Deserialize)]
        struct Answer {
            issue: Option<Value>,
        }
        Ok(read_as::<Answer>(agent, &self.0, json!({ "id": id }))?.issue)
    }
}

// An agent session's issue with its comments, as `tolkster start` reads it.
fn session_query() -> String {
    format!("query StartIssue($id: String!) {{ agentSession(id: $id) {{ issue {{ {} }} }} }}", fields(true, false, false))
}

// None when the session is on no issue. Printed as read, as `Show::read`'s is.
#[track_caller]
pub fn session_issue(agent: &Agent, session_id: &str) -> Result<Option<Value>, Error> {
    #[derive(Deserialize)]
    struct Session {
        issue: Option<Value>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Answer {
        agent_session: Option<Session>,
    }
    Ok(read_as::<Answer>(agent, &session_query(), json!({ "id": session_id }))?.agent_session.and_then(|s| s.issue))
}

fn fields(comments: bool, children: bool, attachments: bool) -> String {
    let links = format!("attachments(first: {LINKS}) {{ nodes {{ title url }} }}");
    let mut fields = ISSUE_FIELDS.to_string();
    if comments {
        fields.push_str(&format!("\n    comments(first: {PAGE}) {{ nodes {{ id body createdAt updatedAt parent {{ id }} user {{ id displayName }} }} }}"));
    }
    if children {
        fields.push_str(&format!("\n    children(first: {PAGE}) {{ nodes {{ {ISSUE_FIELDS} {links} }} }}"));
    }
    if attachments {
        fields.push_str(&format!("\n    {links}"));
    }
    fields
}

// Linear's own search ranks by the term, so `issues` (newest first) is the read without one.
fn list_query(searching: bool) -> String {
    let (field, args, order) = if searching { ("searchIssues", "term: $term, ", "") } else { ("issues", "", ", orderBy: updatedAt") };
    let declared = if searching { "$term: String!, " } else { "" };
    format!(
        "query List({declared}$filter: IssueFilter, $first: Int!) {{
  {field}({args}filter: $filter, first: $first{order}) {{
    nodes {{ identifier title priority estimate updatedAt state {{ name type }} parent {{ identifier }} }}
    pageInfo {{ hasNextPage }}
  }}
}}"
    )
}

// Up to `first` issues `filter` matches, under `searchIssues` with a `term`, else `issues`,
// each printed as read (commands/list.rs), and whether there are more.
#[track_caller]
pub fn list(agent: &Agent, filter: Value, first: u32, term: Option<&str>) -> Result<Nodes<Value>, Error> {
    #[derive(Deserialize)]
    struct Answer {
        #[serde(alias = "searchIssues")]
        issues: Nodes<Value>,
    }
    let mut variables = json!({ "filter": filter, "first": first });
    if let Some(term) = term {
        variables["term"] = json!(term);
    }
    Ok(read_as::<Answer>(agent, &list_query(term.is_some()), variables)?.issues)
}

#[cfg(test)]
mod tests;
