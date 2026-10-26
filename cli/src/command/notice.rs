// What a command says in a session, or in a note's thread as a reply, and a notice's fixed text.

use serde_json::json;

use crate::error::Error;
use crate::linear::{ActivityInput, ActivityType};
use crate::machine::{Command, Finished, Notice};
use crate::plain::Id;
use crate::ports::Ports;
use crate::post::answer_key;
use crate::request::Thread;
use crate::review;
use crate::trace::{self, Work};
use crate::voice;

use super::thread_on_issue;

// In the session, or for a note's thread as a reply there, once the root is read again as a
// note's thread on the command's issue: one that no longer is gets nothing said. The reply
// is posted by a comment ID made from the command's, so a command run again doesn't say it
// twice. `stalled`, given the time of the request its turn was for, is that request's answer
// (post::answer_key): said already when the turn answered. A reply has no kind: an
// elicitation's or an error's words are said all the same.
pub(super) fn say(ports: &Ports, command: &Command, session_id: &Id, thread: bool, stalled: Option<&str>, kind: ActivityType, body: String) -> Result<Finished, Error> {
    if !thread {
        return post(ports, command, session_id, kind, body);
    }
    let Some(thread) = thread_on_issue(ports, command, session_id)? else {
        trace::event(Work::NoNoteThread { session_id: session_id.clone(), issue: command.identifier.clone() });
        return Ok(Finished::ok(json!({})));
    };
    reply(ports, command, session_id, &thread, stalled, body)
}

// A reply in `thread`, read by the caller as the note's thread on the command's issue, by the
// comment ID `say` describes.
fn reply(ports: &Ports, command: &Command, session_id: &Id, thread: &Thread, stalled: Option<&str>, body: String) -> Result<Finished, Error> {
    let key = stalled.map_or_else(|| post_key(command), |through| answer_key(&thread.root, through));
    ports.linear.reply_once(&command.issue_id, session_id, &body, &key)?;
    Ok(Finished::ok(json!({})))
}

// Its activity ID is made from the command's, so a command run again doesn't say it twice.
pub(super) fn post(ports: &Ports, command: &Command, session_id: &str, kind: ActivityType, body: String) -> Result<Finished, Error> {
    ports.linear.post_once(&post_key(command), ActivityInput::text(session_id, kind, &body, false))?;
    Ok(Finished::ok(json!({})))
}

pub(super) fn post_key(command: &Command) -> String {
    format!("command {}", command.id)
}

// A notice's fixed text (docs/voice.md), and what it is posted as: one that needs the
// owner at the terminal, or asks for a reply, is an elicitation, and `noRepo` an error,
// which hands the delegation back.
pub(super) fn notice_text(notice: Notice, blockers: &[String], pr: Option<u64>, rounds: Option<u32>) -> (ActivityType, String) {
    let pr = pr.unwrap_or_default();
    match notice {
        Notice::Blocked => {
            let (list, plural) = voice::listed(blockers);
            (ActivityType::Response, format!("{}{list}.{}", voice::BLOCKED_PREFIX, voice::BLOCKED_ENDINGS[usize::from(plural)]))
        }
        Notice::Busy => (ActivityType::Response, voice::BUSY.into()),
        Notice::Held => (ActivityType::Elicitation, voice::HELD.into()),
        Notice::NotDelegated => (ActivityType::Response, voice::NEVER_DELEGATED.into()),
        Notice::NoLongerMine => (ActivityType::Response, voice::STOPPED_WAITING_UNDELEGATED.into()),
        Notice::Cancelled => (ActivityType::Response, voice::STOPPED_WAITING_CANCELLED.into()),
        Notice::AlreadyDone => (ActivityType::Response, voice::STOPPED_WAITING_DONE.into()),
        Notice::NoRepo => (ActivityType::Error, voice::NO_REPO.into()),
        Notice::Stopped => (ActivityType::Response, voice::STOPPED.into()),
        Notice::Stalled => (ActivityType::Elicitation, voice::STALLED.into()),
        Notice::Paused => (ActivityType::Elicitation, voice::IDLE_PAUSED.into()),
        Notice::HeldAtPrompt => (ActivityType::Elicitation, voice::IDLE_WAITING.into()),
        Notice::CouldntPass => (ActivityType::Elicitation, voice::couldnt(voice::COULDNT_DELIVER, false)),
        Notice::RoundsCap => (ActivityType::Response, review::capped_number(pr, rounds.unwrap_or(1))),
        Notice::PrClosed => (ActivityType::Response, review::told(pr, crate::state::Told::Closed)),
        Notice::AutoMergeOff => (ActivityType::Response, review::told(pr, crate::state::Told::AutoMergeOff)),
        Notice::CiCouldntRun => (ActivityType::Response, review::told(pr, crate::state::Told::CiCouldntRun)),
        Notice::ChecksPending => (ActivityType::Response, review::told(pr, crate::state::Told::ChecksPending)),
        Notice::CopilotWontApprove => (ActivityType::Response, review::told(pr, crate::state::Told::CopilotWontApprove)),
        Notice::StopUnnamed => (ActivityType::Response, voice::STOP_UNNAMED.into()),
        Notice::RoundUnnamed => (ActivityType::Response, voice::ROUND_UNNAMED.into()),
        Notice::Unheard => (ActivityType::Elicitation, voice::UNHEARD.into()),
    }
}
