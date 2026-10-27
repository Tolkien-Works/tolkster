// `tolkster messages`: the owner's messages in a session that its reader hasn't read or
// answered, as the session's machine holds them (state::Reading), or with `--thread`, the
// owner's replies in the thread of a comment of Tolkster's.

use std::io::{self, Write};

use ureq::Agent;

use crate::args::{Kind, parse};
use crate::config::{files, owner};
use crate::exec::run_command;
use crate::plain::Time;
use crate::cli::{conversation_session, logged, thread_turn};
use crate::coauthor::{self, marked};
use crate::request::{Message, Session, Thread};
use crate::error::Error;
use crate::machine::{Reader, worker_read};
use crate::state::{Reading, State, thread_request};
use crate::linear::{self, with_token};
use crate::sessions::{self, Bodies, MESSAGES_ACTIVITIES, Read, lookup, thread_of};

pub const OPTIONS: &[(&str, Kind)] = &[("after", Kind::String), ("thread", Kind::String)];

// With `--after <time>`, a conversation's turn reads the owner's messages after the newest
// request a turn or worker was shown, read or not (request::Session::messages_after).
pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, OPTIONS, true)?;
    let text = match (parsed.first(), parsed.non_empty("thread")) {
        (Some(session_id), None) => read(agent, session_id, parsed.non_empty("after"), own_reader(session_id))?,
        // In a reply's turn, only up to the request it answers.
        (None, Some(root)) => {
            let through = thread_turn()?.map(|(own, request)| thread_request(agent, &files()?, &own, request)).transpose()?;
            read_replies(agent, root, parsed.non_empty("after"), through.as_deref())?
        }
        _ => return Err(Error::Usage("messages needs <sessionId> or --thread, not both".into())),
    };
    let _ = write!(io::stdout(), "{text}");
    Ok(0)
}

// What `tolkster messages --thread <commentId> [--after <time>]` prints, read now: the
// owner's replies in the thread of Tolkster's comment, those after `after` if given. A
// thread's turn puts it in its prompt too (commands/converse.rs), up to the time of the
// request it answers, `through`. Only a turn's read, with `through`, is reported as read.
pub fn read_replies(agent: &Agent, root: &str, after: Option<&str>, through: Option<&str>) -> Result<String, Error> {
    let after = after.map(Time::check).transpose()?;
    let after = after.as_deref();
    let files = files()?;
    let owner = owner(&files)?;
    let thread = thread_of(agent, root, &owner, Bodies::Read)?;
    if let Some(newest) = through.and_then(|through| thread.replies.iter().rfind(|r| r.created_at.as_str() <= through)) {
        // The thread's request is keyed by its root, which stands for the session's ID. The
        // read keys the turn's answer (state::thread_request), so replies it couldn't report
        // aren't printed.
        State::open(agent, &files)?.reported(worker_read(root, &newest.id, &newest.created_at, Reader::Turn)).map_err(|error| Error::Failed(format!("reporting the read failed: {error}")))?;
    }
    Ok(render_replies(&thread, after, through))
}

// What `tolkster messages --thread` prints. The text is the owner's, passed to the turn as data.
pub fn render_replies(thread: &Thread, after: Option<&str>, through: Option<&str>) -> String {
    let (root, on) = (&thread.root, &thread.identifier);
    let replies: Vec<_> = thread.replies_after(after).into_iter().filter(|r| through.is_none_or(|through| r.created_at.as_str() <= through)).collect();
    let mut out = if replies.is_empty() {
        format!("No replies from your owner in the thread of your comment {root} on {on}.
")
    } else {
        format!("Your owner's replies in the thread of your comment {root} on {on}, oldest first:
")
    };
    for reply in replies {
        out.push_str(&format!("
--- {} ---
{}
", reply.created_at, reply.body.trim_end()));
    }
    out.push_str(&format!("
Answer in this thread: tolkster comment {on} --reply-to {root} --body-file -
"));
    out
}

// Who the caller is: a worker (in a worktree Tolkster marked, whichever session it was
// marked for: another session's request can be passed on to the issue's worker), or the
// session's own turn, whose reads alone are reported. The owner's checkout is never marked,
// so the owner's look at a busy worker's session reports nothing.
pub fn own_reader(session_id: &str) -> Option<Reader> {
    if conversation_session().as_deref() == Some(session_id) {
        Some(Reader::Turn)
    } else if marked(&mut run_command, None) {
        Some(Reader::Worker)
    } else {
        None
    }
}

// Whether a worker's worktree was marked for `session`'s issue: its own session, or another
// session of the same issue. A read or answer of another issue's session is no worker's to
// report, and one whose marked session's issue can't be read is none either.
pub fn works_on(agent: &Agent, session: &Session) -> bool {
    let Some(marked) = coauthor::marked_session(&mut run_command, None) else { return false };
    let issue_of_marked = || lookup::session_issue_id(agent, &marked).ok().flatten();
    same_issue(&marked, session, issue_of_marked)
}

// The marked session's issue is read only when it isn't the session itself.
fn same_issue(marked: &str, session: &Session, issue_of_marked: impl FnOnce() -> Option<String>) -> bool {
    marked == session.id || session.issue.as_ref().is_some_and(|issue| issue_of_marked().as_deref() == Some(issue.id.as_str()))
}

// What `tolkster messages <sessionId> [--after <time>]` prints, read now. With `after`, a
// conversation's turn puts what the owner wrote after the newest request a turn or worker was shown in its
// prompt (commands/converse.rs). Without, the messages are those after what the session's
// machine says was last answered, and for a worker after what it last read too, so it sees
// only what is new. A reader's read is reported (`workerRead`), by the owner's newest
// request it shows.
pub fn read(agent: &Agent, session_id: &str, after: Option<&str>, reader: Option<Reader>) -> Result<String, Error> {
    let after = after.map(Time::check).transpose()?;
    let after = after.as_deref();
    let files = files()?;
    let owner = owner(&files)?;
    let state = State::open(agent, &files)?;
    let session = with_token(agent, |token| sessions::read(agent, token, Some(&state), session_id, Read::Full { activities: MESSAGES_ACTIVITIES, bodies: Bodies::Read }))?;
    // Another issue's session is printed to a worker but reported as read by no one.
    let reader = reader.filter(|reader| *reader != Reader::Worker || works_on(agent, &session));
    // A reading that couldn't be had bounds nothing: the messages are all printed.
    let reading = logged(state.reading(session_id), "messages: what was read couldn't be asked, so every message is shown").unwrap_or_default();
    let messages = session.messages_after(&owner, bound(after, reader, &reading).as_deref());
    if let Some(reader) = reader
        && let Some((activity_id, at)) = session.newest_request(&owner)
    {
        // A request with no activity of its own, the session's start, is named by the session.
        state.report(worker_read(session_id, activity_id.as_deref().unwrap_or(session_id), &at, reader));
    }
    let unread = after.is_none() && reader == Some(Reader::Worker) && reading.read_through.is_some();
    Ok(render_messages(&session, &messages, &owner, unread, reading.answered_through.is_some()))
}

// Where the messages start: after `after`, or after what the session's machine says was
// answered and, for a worker, read by the worker itself: `read_through` is never a turn's, so
// a fresh worker is shown what a turn already showed the owner's prompts to.
fn bound(after: Option<&str>, reader: Option<Reader>, reading: &Reading) -> Option<String> {
    match (after, reader) {
        (Some(after), _) => Some(after.to_string()),
        (None, Some(Reader::Worker)) => reading.read_through.clone().max(reading.answered_through.clone()),
        (None, _) => reading.answered_through.clone(),
    }
}

const DESCRIBED: &str = "The issue is the request: read its description with `tolkster issue show`.";

// What `tolkster messages` prints. The text is the owner's, passed to the worker as data.
// `unread`: the messages are those the worker hasn't read, so none means none new.
// `answered`: something has answered the session.
pub fn render_messages(session: &Session, messages: &[Message], owner_id: &str, unread: bool, answered: bool) -> String {
    let on = session.issue.as_ref().map(|i| format!(" on {}", i.identifier)).unwrap_or_default();
    let mut out = String::new();
    // A delegation back after the session began, after the owner's newest message: the issue
    // is the request again.
    let newest = session.newest_request(owner_id).map(|(_, at)| at);
    let redelegated = session
        .issue
        .as_ref()
        .and_then(|i| i.delegated.as_ref())
        .filter(|d| d.again && session.mention.is_none() && d.by.as_deref() == Some(owner_id) && d.at > session.created_at && newest.as_ref().is_none_or(|n| d.at > *n))
        .map(|d| format!("The issue was delegated back to you at {}, after any messages above: the issue is the request again. Carry on with it from where the worktree is.\n", d.at));
    if messages.is_empty() {
        let delegated = session.mention.is_none() && !answered && session.activities.iter().all(|a| a.kind != linear::Kind::Prompt);
        let since = if unread { " since your last read" } else { "" };
        let none = if since.is_empty() { "No unanswered messages" } else { "No new messages" };
        out.push_str(&format!("{none} in session {}{on}{since}.\n", session.id));
        if let Some(line) = &redelegated {
            out.push_str(line);
        } else if delegated {
            out.push_str("It was started by delegating the issue to you: the issue is the request.\n");
        }
    } else {
        out.push_str(&format!("Unanswered messages from your owner in session {}{on}, oldest first:\n", session.id));
        let described = !session.on_own_note && session.mention.as_ref().is_some_and(|m| m.in_description);
        for message in messages {
            let what = match (message.mention, message.stop) {
                (true, _) if session.on_own_note => " (a reply to your comment)",
                (true, _) if described => " (the issue's description mentioned you)",
                (true, _) => " (the comment that mentioned you)",
                (_, true) => " (asked you to stop)",
                _ => "",
            };
            // Fixed text: anyone may have edited the description since, so its text is never
            // given as the owner's.
            let body = if message.mention && described { DESCRIBED } else { message.body.trim_end() };
            out.push_str(&format!("\n--- {}{what} ---\n{body}\n", message.created_at));
        }
        if let Some(line) = &redelegated {
            out.push('\n');
            out.push_str(line);
        }
    }
    out.push_str(&format!("\nAnswer in this session: tolkster activity {} response --body-file -\n", session.id));
    out
}

#[cfg(test)]
mod tests;
