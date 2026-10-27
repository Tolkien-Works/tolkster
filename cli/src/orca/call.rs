// How a call to `orca` runs: what its failure says, Orca's own error read off its stdout, and
// which of its writes `listen` traces or a listing must see.

use std::cell::Cell;

use serde::{Deserialize, Deserializer};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::{Context, Error, Unsent, Waits};
use crate::exec::{Run, failure};
use crate::json::{decode, loose};
use crate::trace::{self, Call};

// Fails with Orca's own error, or with the program's stderr when it gave none: an Orca call's
// failure, its launch's included, is `Error::Orca`.
pub fn call(run: &mut Run, args: &[&str]) -> Result<String, Error> {
    call_said(run, args).0
}

// As `call`, with what a failed run printed on stdout, where `orca --json` puts its error.
pub(super) fn call_said(run: &mut Run, args: &[&str]) -> (Result<String, Error>, Option<String>) {
    let (result, said, _) = called(run, args);
    (result, said)
}

// As `call_said`, with whether Orca was absent: not found to run, or its runtime unreachable
// (`runtime_unavailable`, its app not running or a request it lost; a paired host's is
// `remote_runtime_unavailable`).
fn called(run: &mut Run, args: &[&str]) -> (Result<String, Error>, Option<String>, bool) {
    let (result, said, absent) = match run("orca", args, None) {
        Ok(output) if output.ok => (Ok(output.stdout), None, false),
        // On Windows every `orca` call writes a crashpad line to stderr, so Orca's own error comes first.
        Ok(output) => {
            let absent = Said::of(&output.stdout).is_some_and(|said| said.code() == Some("runtime_unavailable"));
            (Err(Error::Orca(failure("orca", args, &output, orca_error(&output.stdout)))), Some(output.stdout), absent)
        }
        Err(error) => (Err(Error::Orca(format!("orca {} failed: {error}", args[..args.len().min(2)].join(" ")))), None, error.not_found()),
    };
    let verb = args.get(..2).map(|verb| verb.join(" "));
    if verb.as_deref().is_some_and(|verb| LISTING_WRITES.contains(&verb)) {
        STALE.with(|stale| stale.set(true));
    }
    if verb.as_deref().is_none_or(|verb| !READS.contains(&verb)) {
        WRITES.with(|writes| writes.set(writes.get() + 1));
    }
    if let Some((site, detail)) = written(args) {
        trace::event(Call::Wrote { site, detail, failed: result.as_ref().err().cloned() });
    }
    (result, said, absent)
}

// What a failed `orca --json` call printed on stdout: Orca's error, or its refusal of a send.
// Read only where it is JSON of this shape: what isn't says nothing (`Said::of`). An error's
// fields are read loosely (`json::loose`), so one of another shape hides none of the others;
// an `error` that is there, null included, is an error, and one that is no object fails the
// whole read, so it is never taken for no error, which would let `refused` allow a second send.
#[derive(Debug, Deserialize)]
pub(super) struct Said {
    #[serde(default, deserialize_with = "present")]
    error: Option<OrcaError>,
    #[serde(default, deserialize_with = "loose")]
    result: Option<SaidResult>,
}

#[derive(Debug, Default, Deserialize)]
struct OrcaError {
    #[serde(default, deserialize_with = "loose")]
    code: Option<String>,
    #[serde(default, deserialize_with = "loose")]
    message: Option<String>,
    #[serde(default, deserialize_with = "loose")]
    data: Option<ErrorData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ErrorData {
    #[serde(default, deserialize_with = "loose")]
    pub(super) retry_safe: Option<bool>,
    #[serde(default, deserialize_with = "loose")]
    pub(super) orchestration_request_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SaidResult {
    send: Option<Verdict>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Verdict {
    accepted: bool,
    refused_reason: Option<String>,
}

fn present<'de, D: Deserializer<'de>>(error: D) -> Result<Option<OrcaError>, D::Error> {
    Ok(Some(Option::<OrcaError>::deserialize(error)?.unwrap_or_default()))
}

impl Said {
    pub(super) fn of(stdout: &str) -> Option<Said> {
        serde_json::from_str(stdout).ok()
    }

    pub(super) fn code(&self) -> Option<&str> {
        self.error.as_ref().and_then(|error| error.code.as_deref())
    }

    pub(super) fn data(&self) -> Option<&ErrorData> {
        self.error.as_ref().and_then(|error| error.data.as_ref())
    }

    // Whether Orca turned a send away before writing anything.
    pub(super) fn refused(&self) -> bool {
        self.error.is_none() && self.result.as_ref().and_then(|r| r.send.as_ref()).is_some_and(|send| !send.accepted)
    }

    pub(super) fn refused_reason(&self) -> Option<&str> {
        self.result.as_ref().and_then(|r| r.send.as_ref()).and_then(|send| send.refused_reason.as_deref())
    }
}

// Orca's own error, or its refusal of a send, on one line: Orca's text, never Linear's or
// GitHub's, so it may be logged. None when it printed neither.
pub(super) fn orca_error(stdout: &str) -> Option<String> {
    let said = Said::of(stdout)?;
    let detail = if let Some(error) = &said.error {
        match (error.code.as_deref().unwrap_or(""), error.message.as_deref().unwrap_or("")) {
            ("", "") => return None,
            (code, "") => code.to_string(),
            (code, message) if message.starts_with(code) => message.to_string(),
            (code, message) => format!("{code}: {message}"),
        }
    } else if said.refused() {
        format!("input refused{}", said.refused_reason().map_or(String::new(), |reason| format!(": {reason}")))
    } else {
        return None;
    };
    Some(detail.split_whitespace().collect::<Vec<_>>().join(" "))
}

// Why a failed send is known to have typed nothing: Orca refused it for want of an agent, or
// the terminal's handle no longer names one.
pub(super) fn unsent(said: &Said) -> Option<Unsent> {
    match (said.refused(), said.refused_reason(), said.code()) {
        (true, Some("no-agent"), _) => Some(Unsent::NoAgent),
        (_, _, Some("terminal_handle_stale")) => Some(Unsent::TerminalGone),
        _ => None,
    }
}

// The writes that change which worktrees and agent terminals a listing names. Counted whether
// or not the call failed: one that timed out may have gone through.
pub(super) const LISTING_WRITES: [&str; 4] = ["worktree create", "worktree rm", "terminal create", "terminal close"];

// Ambient: set by whichever Orca call writes, deep in the job a lane runs, and taken by the
// lane as the job ends; a thread's own, as two lanes' jobs write at once.
thread_local! {
    static STALE: Cell<bool> = const { Cell::new(false) };
}

// The Orca calls known to change nothing. Any other counts as a write, failed or not, so a
// read missing here only loses its command's wait on an absent Orca (`Cli::waiting`).
const READS: [&str; 9] = ["worktree ps", "worktree list", "worktree show", "terminal list", "terminal read", "terminal wait", "host list", "repo list", "project setups"];

// This thread's Orca writes so far, which a command's `Cli` compares with its own start.
thread_local! {
    static WRITES: Cell<u64> = const { Cell::new(0) };
}

pub(super) fn writes() -> u64 {
    WRITES.with(Cell::get)
}

// Whether this thread has made a listing write since it last asked; the lane that ran the job
// asks as the job ends, so the watcher lists once for all its writes.
pub fn take_listing_stale() -> bool {
    STALE.with(Cell::take)
}

const ORCA_WRITES: [&str; 6] = ["worktree create", "worktree rm", "worktree set", "terminal create", "terminal close", "terminal send"];

// What an Orca command changes, for `listen`'s line, as its site and what it names, or None
// for a read: what it names, never the text it types or a title it sets.
fn written(args: &[&str]) -> Option<(String, String)> {
    let verb = args.get(..2)?.join(" ");
    if !ORCA_WRITES.contains(&verb.as_str()) {
        return None;
    }
    let named: Vec<String> = args.windows(2).filter(|pair| matches!(pair[0], "--environment" | "--host" | "--repo" | "--project" | "--worktree" | "--terminal" | "--name" | "--retry-request")).map(|pair| format!("{}={}", pair[0].trim_start_matches('-'), pair[1])).collect();
    Some((format!("orca {verb}"), named.join(" ")))
}

pub(super) fn orca_json(run: &mut Run, args: &[&str]) -> Result<Value, Error> {
    json_of(&call(run, args)?, args)
}

// As `orca_json`, with whether Orca was absent (`called`), for a listing whose caller decides
// if it may wait (`Cli::failed`).
pub(super) fn orca_listed(run: &mut Run, args: &[&str]) -> (Result<Value, Error>, bool) {
    match called(run, args) {
        (Ok(answer), ..) => (json_of(&answer, args), false),
        (Err(error), _, absent) => (Err(error), absent),
    }
}

fn json_of(answer: &str, args: &[&str]) -> Result<Value, Error> {
    serde_json::from_str(answer).context(format!("Orca's answer to {}", args.first().unwrap_or(&"")))
}

// Orca's envelope around what a call answers.
#[derive(Debug, Deserialize)]
pub struct Answer<T> {
    pub result: T,
}

// `args`' answer, decoded at once as `T`: a field `T` relies on and the answer lacks fails
// naming the call's verb, e.g. "Orca's answer to repo list". A read opens and types nothing,
// so one that found Orca absent waits on it (`Waits::Orca`), for the Durable Object to send
// its command again once a listing hears Orca; a write's failure stays `Error::Orca`.
pub(super) fn orca_read<T: DeserializeOwned>(run: &mut Run, args: &[&str]) -> Result<T, Error> {
    let answer = match called(run, args) {
        (Err(error), _, true) => return Err(Error::Waits(Waits::Orca, error.to_string())),
        (answer, ..) => answer?,
    };
    decode(json_of(&answer, args)?, &answer_to(args))
}

pub(super) fn answer_to(args: &[&str]) -> String {
    format!("Orca's answer to {}", args[..args.len().min(2)].join(" "))
}
