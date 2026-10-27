// `tolkster sessions`: Tolkster's agent sessions, newest first.

use serde::Serialize;
use ureq::Agent;

use crate::args::{Kind, parse};
use crate::cli::print_exit;
use crate::error::Error;
use crate::sessions::lookup::agent_sessions;
use crate::linear::with_token;

pub const OPTIONS: &[(&str, Kind)] = &[("issue", Kind::String)];

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Row {
        id: String,
        status: String,
        issue_id: Option<String>,
        // Tolkster acts only on sessions the owner started.
        creator_id: Option<String>,
        updated_at: String,
    }

    let parsed = parse(argv, OPTIONS, false)?;
    let issue = parsed.non_empty("issue");
    let rows = with_token(agent, |token| {
        let mut rows: Vec<Row> = agent_sessions(agent, token, issue)?
            .into_iter()
            .map(|node| Row { id: node.id, status: node.status, issue_id: node.issue.map(|i| i.id), creator_id: node.creator.map(|c| c.id), updated_at: node.updated_at })
            .collect();
        // Linear's timestamps share one ISO format, so text order is time order.
        rows.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(rows)
    })?;
    print_exit(&rows, 0)
}
