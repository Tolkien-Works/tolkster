use std::time::Duration;

use ureq::Agent;

use crate::args::{self, Kind, parse};
use crate::cli::{NOTHING_TO_DO, read_body, thread_turn};
use crate::config::{Files, files, owner};
use crate::commands::asker::{Asker, SESSION_OPTION};
use crate::context::{COMMENT_HOLD, Comments, Wrote, may_comment, parse_closed, write_comment};
use crate::error::Error;
use crate::plain::{Id, IssueKey, Time};
use crate::linear::{create_comment, update_comment, with_token};
use crate::machine::{Reader, worker_answered};
use crate::post::{answer_key, reply_once};
use crate::state::{State, derived_uuid, new_token, thread_request};
use crate::request::Thread;
use crate::sessions::{Bodies, comment_on_issue, read_closed, thread_of, tolksters_issues};
use crate::sessions::lookup::issue_ref;
use crate::time::{instant, iso, now_iso};

pub const OPTIONS: &[(&str, Kind)] = &[SESSION_OPTION, ("closed", Kind::String), ("seen", Kind::String), ("reply-to", Kind::String), ("none", Kind::Boolean), ("body", Kind::String), ("body-file", Kind::String)];

// `tolkster comment`: a comment on any issue, as Tolkster. With `--session`, one the
// owner's session asked for; without it, one the owner's own agent posts (asker::Asker).
// With `--closed`, a context session's comment on an issue related to one just completed,
// or a sibling of it (context.rs), checked here, in code: the completed issue is
// Tolkster's, and the target is related to it, or under the same parent, and still open.
// It is Tolkster's one context comment on the target, updated in place
// (`context::write_comment`), unless it was written after `--seen`, when the session read
// the target: then it exits 3 and writes nothing. With `--reply-to`, a reply in the thread of a comment of
// Tolkster's (`reply`), or, with `--none` instead of a body, a turn's word that it needs none.
pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, OPTIONS, true)?;
    let usage = || Error::Usage("comment needs the issue's ID, and --closed with --seen, never beside --session or --reply-to".into());
    let Some(id) = parsed.first() else { return Err(usage()) };
    let files = files()?;
    if parsed.flag("none") {
        let (Some(root), None, None) = (parsed.non_empty("reply-to"), parsed.string("body"), parsed.string("body-file")) else {
            return Err(Error::Usage("--none goes with --reply-to, never with a body".into()));
        };
        return unanswered(agent, id, root, &files);
    }
    let body = args::body(&parsed, "comment", read_body)?;
    let owner = owner(&files)?;
    if parsed.string("closed").is_some() != parsed.string("seen").is_some() {
        return Err(usage());
    }
    let closed = match (parsed.string("session"), parsed.string("closed"), parsed.string("reply-to")) {
        (Some(_), Some(_), _) | (_, Some(""), _) | (_, Some(_), Some(_)) | (_, _, Some("")) => return Err(usage()),
        (_, _, Some(root)) => return reply(agent, id, root, &parsed, &body, &files, &owner),
        (_, closed, None) => closed,
    };
    let Some(closed) = closed else {
        let asker = Asker::parse(&parsed)?;
        with_token(agent, |token| {
            asker.trusted(agent, token, &owner)?;
            create_comment(agent, token, &issue_ref(agent, token, id)?.id, None, &body, None).map(|_| ())
        })?;
        return Ok(0);
    };

    let seen = Time::check(parsed.string("seen").unwrap_or_default())?;
    let state = State::open(agent, &files)?;
    let token = new_token();
    // Another context session writing to the same issue holds it for seconds; one that died
    // holding it, or whose write failed, for COMMENT_HOLD at most. The checks are made again
    // after each wait, so a target closed meanwhile gets nothing.
    let mut waited = Duration::ZERO;
    loop {
        let closed = parse_closed(&read_closed(agent, closed)?.issue)?;
        let found = with_token(agent, |token| issue_ref(agent, token, id))?;
        let target = IssueKey::parse(&found.identifier).ok_or_else(|| Error::Failed("the issue's identifier isn't plain".into()))?;
        let (target_id, target) = (Id::parse(&found.id).ok_or_else(|| Error::Failed("the issue's ID isn't plain".into()))?, target.upper());
        may_comment(&closed, &target).map_err(Error::Failed)?;
        if !tolksters_issues(agent, std::slice::from_ref(&closed.identifier), &owner)?.contains_key(closed.identifier.as_str()) {
            return Err(Error::Failed(format!("{} isn't mine: I add context only for issues I worked on", closed.identifier)));
        }
        let how = match write_comment(&state, &Linear(agent), &target_id, &body, &seen, &|| Time::check(&now_iso()), &token)? {
            Some(Wrote::Created) => "posted",
            Some(Wrote::Updated) => "updated",
            Some(Wrote::Replaced) => "posted anew, the last one gone,",
            Some(Wrote::Changed(at)) => {
                eprintln!("{target}: my context comment was written at {at}, after --seen: read it again, fold it in, then write with --seen {at}");
                return Ok(NOTHING_TO_DO);
            }
            None if waited > COMMENT_HOLD => return Err(Error::Failed(format!("{target}: another run is still writing my context comment"))),
            None => {
                std::thread::sleep(BUSY_WAIT);
                waited += BUSY_WAIT;
                continue;
            }
        };
        eprintln!("{target}: {how} my context comment, for {}", closed.identifier);
        return Ok(0);
    }
}

// The thread of `root`, which must be on issue `id`.
fn thread_on(agent: &Agent, id: &str, root: &str, owner: &str) -> Result<Thread, Error> {
    let thread = thread_of(agent, root, owner, Bodies::Skip)?;
    if !thread.identifier.eq_ignore_ascii_case(id) && thread.issue_id != id {
        return Err(Error::Failed(format!("comment {root} is on {}, not {id}", thread.identifier)));
    }
    Ok(thread)
}

const BUSY_WAIT: Duration = Duration::from_secs(10);

struct Linear<'a>(&'a Agent);

impl Comments for Linear<'_> {
    fn on_issue(&self, issue_id: &Id, comment_id: &Id) -> Result<bool, Error> {
        comment_on_issue(self.0, issue_id, comment_id)
    }
    fn create(&self, issue_id: &Id, comment_id: &Id, body: &str) -> Result<(), Error> {
        with_token(self.0, |token| create_comment(self.0, token, issue_id, None, body, Some(comment_id))).map(|_| ())
    }
    fn update(&self, comment_id: &Id, body: &str) -> Result<(), Error> {
        with_token(self.0, |token| update_comment(self.0, token, comment_id, body))
    }
}

// A reply as Tolkster in the thread of its own comment `root` on issue `id`, a thread no
// session began on (sessions/issues.rs, parse_thread). In a thread's turn (cli::thread_turn), only in
// its own thread (allowlist::gate_thread), and only the one answer to the request it answers
// now (state::thread_request), by a comment ID made from it (post::answer_key): a second exits
// 3 and posts nothing. Elsewhere, the asker's
// (asker::Asker), as any comment is.
fn reply(agent: &Agent, id: &str, root: &str, parsed: &args::Parsed, body: &str, files: &Files, owner: &str) -> Result<u8, Error> {
    let turn = thread_turn()?;
    if turn.is_none() {
        let asker = Asker::parse(parsed)?;
        with_token(agent, |token| asker.trusted(agent, token, owner))?;
    }
    let thread = thread_on(agent, id, root, owner)?;
    let Some((own, request)) = turn else {
        with_token(agent, |token| create_comment(agent, token, &thread.issue_id, Some(root), body, None))?;
        return Ok(0);
    };
    if own != root {
        return Err(Error::Usage(format!("a reply's turn answers only in its own thread: {own}")));
    }
    let request = thread_request(agent, files, root, request)?;
    let Some(posted) = reply_once(agent, &thread.issue_id, root, body, &answer_key(&thread.root, &request))? else {
        eprintln!("{}: this turn has already answered in the thread of {root}", thread.identifier);
        return Ok(NOTHING_TO_DO);
    };
    let state = State::open(agent, files)?;
    // The thread's request is keyed by its root, which stands for the session's ID.
    state.report(worker_answered(root, &posted.id, &posted.at, Reader::Turn));
    Ok(0)
}

// A thread's turn that read the request it answers now and found nothing in it to answer
// (cli/briefs/thread.md): the request is answered, as by a reply, with nothing posted, so its
// end is no stall. Whether a request needs a reply is the turn's judgement alone. Reported
// under the ID its one answer would take (post::answer_key), at a millisecond past the
// request, on Linear's clock as the machine compares it, never this host's; Err when the
// report didn't land, so the turn may say it again.
fn unanswered(agent: &Agent, id: &str, root: &str, files: &Files) -> Result<u8, Error> {
    let Some((own, request)) = thread_turn()? else { return Err(Error::Usage("--none is a reply's turn's only".into())) };
    if own != root {
        return Err(Error::Usage(format!("a reply's turn answers only in its own thread: {own}")));
    }
    let thread = thread_on(agent, id, root, &owner(files)?)?;
    let request = thread_request(agent, files, root, request)?;
    let mark = derived_uuid(&answer_key(&thread.root, &request));
    let at = instant(request.as_str()).map(|ms| iso(ms + 1)).ok_or_else(|| Error::Failed(format!("the request's time isn't Linear's: {request}")))?;
    State::open(agent, files)?.reported(worker_answered(root, &mark, &at, Reader::Turn))?;
    eprintln!("{}: no reply needed in the thread of {root}", thread.identifier);
    Ok(0)
}
