use ureq::Agent;

use crate::args::{self, Kind, Parsed, parse};
use crate::card;
use crate::cli::read_body;
use crate::commands::messages::{own_reader, works_on};
use crate::config::{files, owner};
use crate::error::Error;
use crate::linear::{ActivityContent, ActivityInput, ActivityType, Choice, with_token};
use crate::machine::{Reader, worker_answered};
use crate::post::send;
use crate::state::{State, new_uuid};
use crate::sessions::{self, PENDING_ACTIVITIES, Read};
use crate::voice;

// `tolkster activity`. A response, an elicitation or an error answers the owner: posted in
// the owner's sessions only, and reported to the session's machine (`workerAnswered`) when
// the session's worker or turn posts it, which handles the request its read reached. A
// thought or an action answers nothing, so a reply sent while a worker is busy still waits.
pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let input = parse_activity(argv, read_body)?;
    if input.ephemeral {
        send(agent, &input)?;
        return Ok(0);
    }
    let answers = matches!(&input.content, ActivityContent::Text { kind, .. } if kind.answers());
    if !answers {
        send(agent, &input)?;
        // A durable action marks a checkpoint: a milestone or a commit.
        if let ActivityContent::Action { action, parameter, .. } = &input.content {
            card::checkpoint(card::launched().as_ref(), &card::action(action, parameter));
        }
        return Ok(0);
    }
    let files = files()?;
    let state = State::open(agent, &files)?;
    let owner = owner(&files)?;
    let session = with_token(agent, |token| sessions::read(agent, token, Some(&state), &input.agent_session_id, Read::Answer(PENDING_ACTIVITIES)))?;
    // Only the owner's sessions, and Tolkster's own, even once their chain has stopped:
    // "Stopped." is said there too.
    if !session.is_owners(&owner) && !session.tolksters_own() {
        return Err(Error::Failed(format!("session {} wasn't started by the owner: I answer only in the owner's sessions", session.id)));
    }
    // The answer's ID and Linear's time for it, for the Durable Object's `workerAnswered`.
    let id = new_uuid();
    let at = send(agent, &ActivityInput { id: Some(id.clone()), ..input })?;
    if let Some(reader) = own_reader(&session.id).filter(|reader| *reader != Reader::Worker || works_on(agent, &session)) {
        state.report(worker_answered(&session.id, &id, &at, reader));
    }
    Ok(0)
}

pub const OPTIONS: &[(&str, Kind)] = &[
    ("body", Kind::String),
    ("body-file", Kind::String),
    ("state", Kind::String),
    ("action", Kind::String),
    ("parameter", Kind::String),
    ("result", Kind::String),
    ("ephemeral", Kind::Boolean),
    ("option", Kind::List),
];

// Turns `tolkster activity <sessionId> <type> ...` into an activity. With `--state`, the
// body is one of the state's phrases (see voice.rs), and the thought is ephemeral.
fn parse_activity(argv: &[String], read_body: impl FnOnce(&str) -> Result<String, Error>) -> Result<ActivityInput, Error> {
    let parsed = parse(argv, OPTIONS, true)?;
    if let Some(state) = parsed.string("state") {
        return parse_state(&parsed, state);
    }
    parse_input(&parsed, read_body)
}

fn parse_state(parsed: &Parsed, state: &str) -> Result<ActivityInput, Error> {
    let usage = |message: &str| Err(Error::Usage(message.into()));
    let Some((agent_session_id, kind)) = session_and_kind(parsed) else {
        return usage("activity --state needs <sessionId> thought");
    };
    if kind.parse() != Ok(ActivityType::Thought) {
        return usage("--state applies only to thought");
    }
    if parsed.positionals.len() > 2 {
        return usage("--state takes no other arguments");
    }
    if ["body", "body-file", "action", "parameter", "result"].iter().any(|o| parsed.string(o).is_some()) {
        return usage("pass --state or --body, not both");
    }
    if !parsed.list("option").is_empty() {
        return usage("--option applies only to elicitation");
    }
    let Some(body) = voice::phrase(state) else {
        return Err(Error::Usage(format!("state must be one of: {}", voice::states().collect::<Vec<_>>().join(", "))));
    };
    // Under way by definition, so the next activity replaces it.
    Ok(ActivityInput::text(agent_session_id, ActivityType::Thought, body, true))
}

// `<sessionId> <type>`, neither empty.
fn session_and_kind(parsed: &Parsed) -> Option<(&str, &str)> {
    Some((parsed.first()?, parsed.positionals.get(1).filter(|k| !k.is_empty())?))
}

fn parse_input(parsed: &Parsed, read_body: impl FnOnce(&str) -> Result<String, Error>) -> Result<ActivityInput, Error> {
    let Some((agent_session_id, kind)) = session_and_kind(parsed) else {
        return Err(Error::Usage("activity needs <sessionId> <type>".into()));
    };
    let Ok(kind) = kind.parse::<ActivityType>() else {
        let types: Vec<&str> = ActivityType::ALL.map(ActivityType::as_str).into();
        return Err(Error::Usage(format!("type must be one of: {}", types.join(", "))));
    };
    // Refuse ephemeral here rather than let the API reject it.
    let ephemeral = parsed.flag("ephemeral");
    if ephemeral && !kind.may_be_ephemeral() {
        return Err(Error::Usage("--ephemeral applies only to thought and action".into()));
    }
    let options = options(parsed, kind)?;

    if kind == ActivityType::Action {
        let (Some(action), Some(parameter)) = (parsed.non_empty("action"), parsed.non_empty("parameter")) else {
            return Err(Error::Usage("an action needs --action and --parameter".into()));
        };
        return Ok(ActivityInput {
            agent_session_id: agent_session_id.into(),
            content: ActivityContent::Action {
                action: action.into(),
                parameter: parameter.into(),
                result: parsed.non_empty("result").map(Into::into),
            },
            ephemeral,
            id: None,
            options,
        });
    }

    let body = args::body(parsed, kind.as_str(), read_body)?;
    Ok(ActivityInput { options, ..ActivityInput::text(agent_session_id, kind, &body, ephemeral) })
}

// `--option`s by name, from voice.rs's fixed choices, so no label is built from Linear text.
// Each value once: two options that send the same reply would be one button twice.
fn options(parsed: &Parsed, kind: ActivityType) -> Result<Vec<&'static Choice>, Error> {
    let names = parsed.list("option");
    if !names.is_empty() && kind != ActivityType::Elicitation {
        return Err(Error::Usage("--option applies only to elicitation".into()));
    }
    let mut options: Vec<&'static Choice> = vec![];
    for name in names {
        let Some(choice) = voice::choice(name) else {
            return Err(Error::Usage(format!("option must be one of: {}", voice::CHOICES.map(|c| c.name).join(", "))));
        };
        if options.iter().any(|o| o.value == choice.value) {
            return Err(Error::Usage(format!("option {name} sends the same reply as another one given")));
        }
        options.push(choice);
    }
    Ok(options)
}

#[cfg(test)]
mod tests;
