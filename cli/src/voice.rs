// A phrase is fixed text: never build one from issue, comment or review text. Nothing reads
// them back, so they can change freely.

use std::collections::BTreeMap;
use std::hash::{BuildHasher, RandomState};
use std::sync::OnceLock;

use serde::Deserialize;

use crate::error::Error;
use crate::linear::Choice;
use crate::plain::{Id, IssueKey, Line};

const VOICE: &str = include_str!("../../src/shared/voice.json");

#[derive(Deserialize)]
struct Voice {
    states: BTreeMap<String, StateVoice>,
    tools: BTreeMap<String, ToolVoice>,
    // Where a worker is, when its plan doesn't say (At::Phase).
    phases: BTreeMap<String, String>,
    // A call that posts an action as it starts and its outcome once it ends (hook::pairing).
    actions: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct StateVoice {
    phrases: Vec<String>,
}

// What a worker's hook says it is doing (hook.rs), as an action: one phrase per kind of tool call, with
// `{path}` in `path` for the file it names, when it names one.
#[derive(Deserialize)]
struct ToolVoice {
    path: Option<String>,
    plain: String,
}

fn voice() -> &'static Voice {
    // Parsed once: the file is compiled in.
    static VOICE_FILE: OnceLock<Voice> = OnceLock::new();
    // A test reads the same file, so a malformed one fails `cargo test`, not a worker.
    VOICE_FILE.get_or_init(|| serde_json::from_str(VOICE).expect("src/shared/voice.json is valid"))
}

pub fn states() -> impl Iterator<Item = &'static str> {
    voice().states.keys().map(String::as_str)
}

pub fn phrases(state: &str) -> Option<&'static [String]> {
    voice().states.get(state).map(|s| s.phrases.as_slice())
}

// One of the state's phrases at random. None for a state the file doesn't have.
pub fn phrase(state: &str) -> Option<&'static str> {
    let list = phrases(state)?;
    list.get(random_below(list.len())).map(|p| p.as_str())
}

// The hook's phrase for a kind of tool call, naming `path` where the kind has a phrase for
// one. `path` is repo-relative and plain (plain::RepoPath), checked by the caller. None
// for a kind the file doesn't have.
pub fn tool(kind: &str, path: Option<&str>) -> Option<String> {
    let voice = voice().tools.get(kind)?;
    Some(match (&voice.path, path) {
        (Some(phrase), Some(path)) => phrase.replace("{path}", path),
        _ => voice.plain.clone(),
    })
}

// Where the worker is, the head of the hook's line: the step its newest plan before the call
// marks `inProgress`, else the phase its calls show.
#[derive(Debug, PartialEq)]
pub enum At {
    Step(String),
    // A key of voice.json's `phases`.
    Phase(&'static str),
}

pub fn head(at: &At) -> Option<String> {
    Some(match at {
        At::Step(step) => step.clone(),
        At::Phase(phase) => voice().phases.get(*phase)?.clone(),
    })
}

// The hook's action for a kind of call that pairs. None for a kind the file doesn't have.
pub fn action(kind: &str) -> Option<&'static str> {
    voice().actions.get(kind).map(String::as_str)
}

// A paired call's outcome, the action's result: one whose end doesn't say how it went
// (hook::known) only finishes.
pub fn outcome(kind: &str, failed: Option<bool>) -> &'static str {
    match (kind, failed) {
        (_, None) => "finished",
        (_, Some(true)) => "failed",
        ("testing" | "building", Some(false)) => "passed",
        _ => "done",
    }
}

// What a paired call checked, in a trail line's outcomes ("tests passed").
pub fn checked(kind: &str) -> &'static str {
    match kind {
        "testing" => "tests",
        "building" => "build",
        "pushing" => "push",
        "rebasing" => "rebase",
        _ => "sub-agent",
    }
}

// Every kind of call the hook pairs, in order.
#[cfg(test)]
pub fn actions() -> impl Iterator<Item = &'static str> {
    voice().actions.keys().map(String::as_str)
}

// Every kind of tool call the hook has a phrase for, in order.
#[cfg(test)]
pub fn tools() -> impl Iterator<Item = &'static str> {
    voice().tools.keys().map(String::as_str)
}

// For a state the code names itself: a test checks each is in the file, so the panic can't
// reach a worker.
pub fn say(state: &str) -> String {
    phrase(state).unwrap_or_else(|| panic!("no state {state} in src/shared/voice.json")).into()
}

pub const LOOKING: &str = "looking";
pub const READING_REPLY: &str = "reading-reply";
pub const READING_ISSUE: &str = "reading-issue";

// RandomState is seeded at random for each one made, which is plenty for choosing a phrase.
fn random_below(n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    (RandomState::new().hash_one(std::time::SystemTime::now()) % n as u64) as usize
}

// What takes the session is the machine's decision, never this wording. A wording is under
// way, so it ends with an ellipsis; settled messages keep a full stop.
pub const PICKED_UP: &str = "Picked up";

// The hook's ephemeral action after each commit of a worker's, the commit's subject its
// parameter.
pub const COMMITTED: &str = "Committed";

// The link names the issue by its identifier, never the worktree (docs/voice.md).
pub const STARTING_IN: &str = "Starting in ";

// Answers nothing: the owner's reply resumes the worker, which still sees what it hadn't
// answered.
pub const STALLED: &str = "I stopped before finishing. Reply here and I'll pick it back up.";

// A worker waiting on a permission prompt needs the owner at its terminal, so it isn't asked
// for a reply, and is posted as an elicitation, so the card needs input.
pub const IDLE_PAUSED: &str = "I've paused before finishing. Reply here and I'll carry on.";
pub const IDLE_WAITING: &str = "I'm held at a prompt only you can answer. Answer it and I'll carry on.";
// A running agent nothing names can't be closed or typed to; told once, however often the stop
// or round is tried again.
pub const STOP_UNNAMED: &str = "I couldn't stop this yet: the work hasn't settled where I can reach it. I'll keep trying.";
pub const ROUND_UNNAMED: &str = "I couldn't pass the review on yet: the work hasn't settled where I can reach it. I'll try again.";
// A reply typed in would have its Enter answer the prompt. An elicitation, as IDLE_WAITING is.
pub const HELD: &str = "I'm held at a prompt only you can answer. I'll read your message once it's answered.";

pub const STOPPED: &str = "Stopped. Say the word if you want me back on it.";
pub const STOPPED_WAITING_UNDELEGATED: &str = "No longer mine, so I've stopped waiting.";
pub const STOPPED_WAITING_CANCELLED: &str = "Cancelled, so I've stopped waiting.";
pub const STOPPED_WAITING_DONE: &str = "Already done, so I've stopped waiting.";
// A delegation whose session the intake never mapped: the owner's reply begins it.
pub const UNHEARD: &str = "This delegation slipped past me. Reply here and I'll start on it.";
// A session that reads as a delegation's on an issue never delegated to Tolkster.
pub const NEVER_DELEGATED: &str = "This issue isn't delegated to me, so there's nothing for me to start. Mention me in a comment, or delegate it.";
// A launch that failed, its worker's, a round's or a turn's, asks for a reply, which tries it
// again: `couldnt` ends each with TRY_AGAIN, and says so when the failure was Orca's, never
// naming Orca or giving its text.
pub const COULDNT_DELIVER: &str = "I couldn't pass your message on.";
pub const COULDNT_START: &str = "I couldn't get started on this.";
pub const COULDNT_RESUME: &str = "I couldn't pick up this review.";
pub const ORCA_FAILED: &str = "Something broke on my side, so ";
pub const TRY_AGAIN: &str = " Reply here and I'll try again.";
pub const NO_REPO: &str = "I can't tell which repo this is for. Map its project or team to a repo, or put it in a project named after one, then ask me again.";

pub fn couldnt(what: &str, orca: bool) -> String {
    format!("{}{what}{TRY_AGAIN}", if orca { ORCA_FAILED } else { "" })
}

// The options an elicitation offers (`linear::Choice`).
pub static GO: Choice = Choice { name: "go", label: "Go", value: "go" };
pub static APPROVE: Choice = Choice { name: "approve", label: "Approve plan", value: "go" };
pub static SKIP: Choice = Choice { name: "skip", label: "Skip it", value: "skip it" };
pub static RETRY: Choice = Choice { name: "retry", label: "Try again", value: "try again" };
pub static CHOICES: [&Choice; 4] = [&GO, &APPROVE, &SKIP, &RETRY];

pub fn choice(name: &str) -> Option<&'static Choice> {
    CHOICES.into_iter().find(|c| c.name == name)
}

pub const BLOCKED_PREFIX: &str = "Waiting on ";
pub const BLOCKED_ENDINGS: [&str; 2] = [" I'll start once it's done.", " I'll start once they're done."];
pub const BUSY: &str = "Every perch is taken. I'll start the moment one frees up.";

// DEEPER, then the trigger's ending, with a plain path or a count, never Linear or GitHub
// text.
pub const DEEPER: &str = "Reviewing this one deeply: it ";
pub const DEEPER_TRUST_FILE: &str = "changes <path>, where ordering and trust live.";
pub const DEEPER_TRUST: &str = "changes code where ordering and trust live.";
pub const DEEPER_LOCK_IN: &str = "adds a lock or a permission in <path>.";
pub const DEEPER_LOCK: &str = "adds a lock or a permission.";
pub const DEEPER_LINES: &str = "adds <count> new lines of code.";
pub const SIZED_NOW: &str = "It had no estimate when I started, so I'm reviewing it as the <size> it is now.";

// The lines typed into a worker's terminal, each plain, so it types as it is. Never Linear
// text: the worker reads that itself.

// The owner wrote in `session_id`.
pub fn new_message(session_id: &Id) -> Result<Line, Error> {
    Line::check(&format!("New message in your Linear session. Read it with tolkster messages {session_id}."))
}

// The owner commented on the issue, outside any session.
pub fn new_comment(identifier: &IssueKey) -> Result<Line, Error> {
    Line::check(&format!("New comment on {identifier}. Read it with tolkster issue show {identifier} --comments."))
}

// Every sub-issue of a parent worked through them has finished (docs/parent-issues.md).
pub fn children_finished(identifier: &IssueKey, session_id: &Id) -> Result<Line, Error> {
    Line::check(&format!("Every sub-issue of {identifier} has finished. Carry on as your task brief's Sub-issues section says for once they have. Report in Linear session {session_id}."))
}

// A sub-issue was added to a parent after its worker chained them (docs/parent-issues.md).
pub fn children_added(identifier: &IssueKey, session_id: &Id) -> Result<Line, Error> {
    Line::check(&format!("{identifier} has sub-issues you haven't chained yet. Set them going as your task brief's Sub-issues section says, with tolkster issue chain {identifier} --session {session_id}, which leaves those already running alone. Report in Linear session {session_id}."))
}

// "a", "a and b" or "x, y and z", and whether it names more than one.
pub fn listed(items: &[String]) -> (String, bool) {
    match items {
        [] => (String::new(), false),
        [one] => (one.clone(), false),
        [rest @ .., last] => (format!("{} and {last}", rest.join(", ")), true),
    }
}

#[cfg(test)]
mod tests;
