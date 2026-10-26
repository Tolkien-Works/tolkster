// `tolkster start <sessionId>`: a worker's first read, in one process. The session's own
// issue, with its comments, as `issue show` prints it, and the owner's messages, as
// `tolkster messages` prints and reports them, read at once into one JSON object. The issue
// is the one Linear names for the session, never one the caller names. What it prints is
// untrusted, as `issue show`'s is.

use serde_json::{Value, json};
use ureq::Agent;

use crate::args::parse;
use crate::cli::print_json;
use crate::commands::messages::{own_reader, read as read_messages};
use crate::commands::show::render;
use crate::error::Error;
use crate::sessions::shown::session_issue;

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, &[], true)?;
    let Some(session_id) = parsed.first() else {
        return Err(Error::Usage("start needs <sessionId>".into()));
    };
    let issue = || -> Result<Value, Error> {
        let issue = session_issue(agent, session_id)?.ok_or_else(|| Error::Failed(format!("no issue for session {session_id}")))?;
        render(&issue, true, false, false)
    };
    let (issue, messages) = std::thread::scope(|scope| {
        let issue = scope.spawn(issue);
        let messages = read_messages(agent, session_id, None, own_reader(session_id));
        // A panic is a failed half, never a lost print of messages already reported read.
        (issue.join().unwrap_or_else(|_| Err(Error::Failed("reading the issue panicked".into()))), messages)
    });
    let (out, failed) = printed(issue, messages)?;
    print_json(&out)?;
    if let Some(error) = failed {
        eprintln!("{error}");
        return Ok(1);
    }
    Ok(0)
}

// The issue's `{"issue", "comments"}` with `"messages"` beside them, and what failed. Half
// read is printed: the messages may already be reported as read, so a failed issue read
// never hides them.
pub fn printed(issue: Result<Value, Error>, messages: Result<String, Error>) -> Result<(Value, Option<Error>), Error> {
    Ok(match (issue, messages) {
        (Ok(mut issue), Ok(messages)) => {
            issue["messages"] = Value::String(messages);
            (issue, None)
        }
        (Ok(issue), Err(error)) => (issue, Some(error)),
        (Err(error), Ok(messages)) => (json!({ "messages": messages }), Some(error)),
        (Err(error), Err(_)) => return Err(error),
    })
}

#[cfg(test)]
mod tests;
