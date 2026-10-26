// `tolkster plan`: sets a session's plan, the checklist Linear shows.

use ureq::Agent;

use serde_json::Value;

use crate::args::{Kind, parse};
use crate::cli::read_body;
use crate::error::Error;
use crate::linear::{PlanInput, steps, update_plan, with_token};

pub const OPTIONS: &[(&str, Kind)] = &[("file", Kind::String)];

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let input = parse_plan(argv, read_body)?;
    with_token(agent, |token| update_plan(agent, token, &input))?;
    Ok(0)
}

// Turns `tolkster plan <sessionId> --file <path|->` into a plan. The file is a JSON array
// of steps, `{"content": "...", "status": "...", "until": "..."}`, `until` optional. A status may be spelt as Linear does
// or loosely ("in_progress", "done", "cancelled"); it is sent as Linear's. Bad input is a
// usage error here, not a request Linear would reject.
fn parse_plan(argv: &[String], read_body: impl FnOnce(&str) -> Result<String, Error>) -> Result<PlanInput, Error> {
    let parsed = parse(argv, OPTIONS, true)?;
    let usage = || Error::Usage("plan needs one <sessionId> and --file".into());
    let (Some(agent_session_id), Some(source)) = (parsed.first(), parsed.non_empty("file")) else {
        return Err(usage());
    };
    if parsed.positionals.len() > 1 {
        return Err(usage());
    }
    let text = read_body(source)?;
    let value: Value = serde_json::from_str(&text).map_err(|error| Error::Usage(format!("the plan isn't JSON: {error}")))?;
    Ok(PlanInput { agent_session_id: agent_session_id.into(), steps: steps(&value)? })
}

#[cfg(test)]
mod tests;
