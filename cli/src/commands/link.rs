// `tolkster in-review`: the session's issue moved to its team's review state
// (issue_state::in_review), and what `pr create --session` does once the PR is open.

use serde_json::json;
use ureq::Agent;

use crate::args::parse;
use crate::cli::print_json;
use crate::commands::asker::session_issue;
use crate::config::{files, owner};
use crate::error::Error;
use crate::issue_state::{Review, in_review};
use crate::linear::{add_session_link, with_token};

// An external link on a session, e.g. the PR: what `tolkster pr create --session` adds.
pub fn add_link(agent: &Agent, token: &str, session_id: &str, label: &str, url: &str) -> Result<(), Error> {
    add_session_link(agent, token, session_id, label, url)
}

// `tolkster in-review <sessionId>`: prints {"review": "moved" | "unchanged" | "no review state"}.
pub fn run_in_review(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, &[], true)?;
    let Some(session_id) = parsed.first() else {
        return Err(Error::Usage("in-review needs <sessionId>".into()));
    };
    let owner = owner(&files()?)?;
    let review = with_token(agent, |token| {
        let issue_id = session_issue(agent, token, session_id, &owner)?;
        in_review(agent, token, &issue_id)
    })?;
    print_json(&json!({ "review": review.as_str() }))?;
    Ok(0)
}

// What `tolkster pr create --session` adds to its JSON once the PR is open: whether the
// PR was linked on the session, and what became of the issue's state. Neither undoes the
// PR, so a failure is reported, on stderr and as `false` or "failed", not raised.
pub fn pr_opened(agent: &Agent, session_id: &str, issue_id: &str, number: u64, url: &str) -> (bool, &'static str) {
    let linked = with_token(agent, |token| add_link(agent, token, session_id, &format!("PR #{number}"), url));
    let linked = linked.map_err(|error| eprintln!("the PR wasn't linked on the session: {error}")).is_ok();
    let review = with_token(agent, |token| in_review(agent, token, issue_id)).map_or_else(
        |error| {
            eprintln!("the issue wasn't moved to review: {error}");
            "failed"
        },
        Review::as_str,
    );
    (linked, review)
}
