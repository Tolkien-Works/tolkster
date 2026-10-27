use ureq::Agent;

use crate::args::{self, Kind};
use crate::cli::{conversation_session, thread_turn};
use crate::coauthor;
use crate::config::files;
use crate::error::Error;
use crate::exec::run_command;
use crate::request::{Session, Thread};
use crate::sessions::{self, Bodies, Read, load_delegations, thread_of};
use crate::state::State;

// The option every change takes: the owner's session it answers.
pub const SESSION_OPTION: (&str, Kind) = ("session", Kind::String);

// What a reply's turn names in its place: its own thread.
pub const THREAD_OPTION: (&str, Kind) = ("thread", Kind::String);

// Who asks for a change: the owner's session, the owner's reply in the thread of Tolkster's
// comment that a reply's turn answers, or the owner's own agent, which has none to name
// (docs/cli.md, "The owner and sessions"). Refusing the session-less path to a worker or a
// turn guards against a slip onto it, not a security boundary: a worker shares the
// workstation and its credentials.
pub enum Asker {
    Session(String),
    Thread(String),
    Owner,
}

impl Asker {
    pub fn parse(parsed: &args::Parsed) -> Result<Asker, Error> {
        if let Some(thread) = parsed.string("thread") {
            let turn = thread_turn()?.map(|(root, _)| root.to_string());
            return Asker::in_thread(thread, parsed.string("session").is_some(), turn.as_deref());
        }
        Asker::of(parsed.string("session"), || own_agent(coauthor::marked(&mut run_command, None), conversation_session().is_some(), !matches!(thread_turn(), Ok(None))))
    }

    // `--thread` names the reply's turn's own thread, `turn`, and is taken nowhere else.
    fn in_thread(thread: &str, session: bool, turn: Option<&str>) -> Result<Asker, Error> {
        match turn {
            _ if session => Err(Error::Usage("--thread and --session don't go together".into())),
            Some(root) if root == thread => Ok(Asker::Thread(thread.into())),
            Some(root) => Err(Error::Usage(format!("a reply's turn changes issues only from its own thread: {root}"))),
            None => Err(Error::Usage("--thread is for a reply's turn: pass --session <sessionId>".into())),
        }
    }

    // `own_agent` is why the caller is one of Tolkster's own agents, if it is: asked only
    // without a session, so a named session costs no call to git.
    fn of(session: Option<&str>, own_agent: impl FnOnce() -> Option<&'static str>) -> Result<Asker, Error> {
        match session {
            Some("") => Err(Error::Usage("--session is empty: pass the owner's session that asked for the change, or leave it out".into())),
            Some(id) => Ok(Asker::Session(id.into())),
            None => match own_agent() {
                Some(REPLYS_TURN) => Err(Error::Usage(format!("{REPLYS_TURN} names its thread: pass --thread <commentId>, the thread it answers in"))),
                Some(what) => Err(Error::Failed(format!("{what} names its session: pass --session <sessionId>, the owner's session that asked for the change"))),
                None => Ok(Asker::Owner),
            },
        }
    }

    pub fn is_owner(&self) -> bool {
        matches!(self, Asker::Owner)
    }

    pub fn is_session(&self) -> bool {
        matches!(self, Asker::Session(_))
    }

    // The session, read fresh, if it is the owner's; None for the owner's own agent, and for
    // a thread once it is the owner's (`thread_issue`).
    pub fn trusted(&self, agent: &Agent, token: &str, owner: &str) -> Result<Option<Session>, Error> {
        match self {
            Asker::Session(id) => trusted_session(agent, token, id, owner).map(Some),
            Asker::Thread(root) => thread_issue(agent, root, owner).map(|_| None),
            Asker::Owner => Ok(None),
        }
    }

    // The session's issue id, if it has one, or the one a thread's root comment is on.
    pub fn issue(&self, agent: &Agent, token: &str, owner: &str) -> Result<Option<String>, Error> {
        match self {
            Asker::Thread(root) => thread_issue(agent, root, owner).map(Some),
            _ => Ok(self.trusted(agent, token, owner)?.and_then(|s| s.issue.map(|i| i.id))),
        }
    }
}

const REPLYS_TURN: &str = "a reply's turn";

fn own_agent(marked: bool, conversation: bool, thread: bool) -> Option<&'static str> {
    if marked {
        Some("a worker")
    } else if conversation {
        Some("a conversation's turn")
    } else if thread {
        Some(REPLYS_TURN)
    } else {
        None
    }
}

// The issue of the thread of Tolkster's comment `root`, read fresh, if the owner asked there:
// the reply's turn's own thread (cli::thread_turn), one no session began on
// (sessions::parse_thread), with a reply of the owner's, not an integration's, by the time of
// the request the turn started on. What `listen` checked before it opened the turn, checked
// again here, in code.
fn thread_issue(agent: &Agent, root: &str, owner: &str) -> Result<String, Error> {
    let Some((own, request)) = thread_turn()? else { return Err(Error::Usage("--thread is for a reply's turn: pass --session <sessionId>".into())) };
    if own.as_str() != root {
        return Err(Error::Usage(format!("a reply's turn changes issues only from its own thread: {own}")));
    }
    let thread = thread_of(agent, root, owner, Bodies::Skip)?;
    if !owner_asked(&thread, &request) {
        return Err(Error::Failed(format!("comment {root} has no reply of the owner's: I change issues only when the owner asks")));
    }
    Ok(thread.issue_id.to_string())
}

fn owner_asked(thread: &Thread, request: &str) -> bool {
    thread.replies.iter().any(|r| r.created_at.as_str() <= request)
}

// The issue of `session_id`, which must be the owner's session: Tolkster changes issues
// only when the owner asks.
pub fn session_issue(agent: &Agent, token: &str, session_id: &str, owner: &str) -> Result<String, Error> {
    trusted_session(agent, token, session_id, owner)?.issue.map(|i| i.id).ok_or_else(|| Error::Failed(format!("session {session_id} has no issue")))
}

// The session, read fresh, if it is the owner's (`Session::is_owners`): one they started,
// or the session on a sub-issue Tolkster delegated to itself while the chain holds, for
// which Tolkster's record of the delegation is read from its state.
pub fn trusted_session(agent: &Agent, token: &str, session_id: &str, owner: &str) -> Result<Session, Error> {
    let mut session = sessions::read(agent, token, None, session_id, Read::Owner)?;
    if !session.is_owners(owner) && session.mention.is_none() && session.issue.as_ref().is_some_and(|i| i.parent.is_some()) {
        load_delegations(&State::open(agent, &files()?)?, std::slice::from_mut(&mut session))?;
    }
    if !session.is_owners(owner) {
        return Err(Error::Failed(format!("session {session_id} wasn't started by the owner: I change issues only when the owner asks")));
    }
    Ok(session)
}

#[cfg(test)]
mod tests;
