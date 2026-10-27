// `tolkster issue list`: issues found by team, project, state, parent or text, read as
// Tolkster and printed as JSON, a title and its state and no more. What people wrote
// reaches an agent through this print in its own terminal, as with `issue show`.

use serde_json::{Value, json};
use ureq::Agent;

use crate::args::{Kind, js_trim, parse};
use crate::cli::print_exit;
use crate::error::Error;
use crate::linear::Nodes;
use crate::sessions::shown;
use crate::plain::split_identifier;


// One page: Linear's largest is 250, and `more` says when there was a next one.
const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 250;

pub const OPTIONS: &[(&str, Kind)] = &[("team", Kind::String), ("project", Kind::String), ("state", Kind::String), ("query", Kind::String), ("parent", Kind::String), ("limit", Kind::String)];

// An IssueFilter from the options: every one given narrows it. No state, or `open`, is every
// state that isn't finished, Triage and Backlog included; any other value is a state's name.
fn filter(team: Option<&str>, project: Option<&str>, state: Option<&str>, parent: Option<&str>) -> Result<Value, Error> {
    let mut all = Vec::new();
    if let Some(team) = team {
        all.push(json!({ "team": { "key": { "eq": team.to_ascii_uppercase() } } }));
    }
    if let Some(project) = project {
        all.push(json!({ "project": { "name": { "eqIgnoreCase": project } } }));
    }
    all.push(match state {
        None => json!({ "state": { "type": { "nin": ["completed", "canceled", "duplicate"] } } }),
        Some(s) if s.eq_ignore_ascii_case("open") => json!({ "state": { "type": { "nin": ["completed", "canceled", "duplicate"] } } }),
        Some(name) => json!({ "state": { "name": { "eqIgnoreCase": name } } }),
    });
    if let Some(parent) = parent {
        let (key, number) = split_identifier(parent).ok_or_else(|| Error::Usage(format!("--parent {parent} is not an issue identifier like TW-123")))?;
        all.push(json!({ "parent": { "and": [{ "team": { "key": { "eq": key.to_ascii_uppercase() } } }, { "number": { "eq": number } }] } }));
    }
    Ok(json!({ "and": all }))
}

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, OPTIONS, false)?;
    let limit = match parsed.non_empty("limit") {
        None => DEFAULT_LIMIT,
        Some(n) => n.parse().ok().filter(|n| (1..=MAX_LIMIT).contains(n)).ok_or_else(|| Error::Usage(format!("--limit takes a number from 1 to {MAX_LIMIT}")))?,
    };
    if parsed.string("query").is_some_and(|q| js_trim(q).is_empty()) {
        return Err(Error::Usage("--query needs some text".into()));
    }
    let query_text = parsed.non_empty("query").map(js_trim);
    let filter = filter(parsed.non_empty("team"), parsed.non_empty("project"), parsed.non_empty("state"), parsed.non_empty("parent"))?;
    print_exit(&render(&shown::list(agent, filter, limit, query_text)?), 0)
}

// The issues as listed, each with the fields Linear read, and whether another page exists.
// Each is printed as read, so its shape stays open.
fn render(found: &Nodes<Value>) -> Value {
    let issues: Vec<Value> = found
        .nodes
        .iter()
        .map(|i| {
            json!({
                "identifier": i["identifier"], "title": i["title"],
                "state": { "name": i["state"]["name"], "type": i["state"]["type"] },
                "priority": i["priority"], "estimate": i["estimate"],
                "parent": i["parent"]["identifier"], "updatedAt": i["updatedAt"],
            })
        })
        .collect();
    json!({ "issues": issues, "more": found.page_info.has_next_page })
}

#[cfg(test)]
mod tests;
