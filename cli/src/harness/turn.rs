// Claude Code's conversation adapter (docs/harness.md#turns): a turn's process, its stream of
// messages in and out, the signal that it is done with each, whether its session resumes, and
// whose ID that session takes. The one place that knows `claude -p`'s stream-json; an event
// it can't read is skipped, never taken for a message done.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde::Deserialize;
use serde_json::json;

use crate::error::{Context, Error};
use crate::plain::Id;
use crate::state::new_uuid;

use super::{NO_MCP, STRICT_MCP, config_dir};
use crate::harnesses::PROGRAM;

// A turn runs in auto mode, as a session of the owner's does, with no tool list: one fixed
// for every repo refused what turns needed, a call at a time.
pub const PERMISSION_MODE: &str = "auto";

// Tolkster chooses a turn's session ID, so every later turn resumes it however this one ends:
// a session's conversation keeps the one it recorded, else a new one.
pub fn session_id(recorded: Option<&str>) -> String {
    recorded.map_or_else(new_uuid, String::from)
}

// Whether Claude takes `value` as a session's ID, a UUID, as a thread's root comment's is.
pub fn takes_id(value: &str) -> bool {
    value.len() == 36 && value.char_indices().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == '-' } else { c.is_ascii_hexdigit() })
}

// A turn resumes its session only when this transcript is there: a first turn that died
// before Claude saved anything starts the session again, with the same ID.
pub fn resumes(folder: &Path, session: &Id) -> Result<bool, Error> {
    let dir = config_dir().map_or_else(|| crate::cli::home_dir().map(|home| home.join(".claude")), Ok)?;
    Ok(transcript(&dir, folder, session).exists())
}

fn transcript(config_dir: &Path, folder: &Path, session: &str) -> PathBuf {
    let slug: String = folder.display().to_string().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    config_dir.join("projects").join(slug).join(format!("{session}.jsonl"))
}

// With no MCP servers, as a worker (NO_MCP), given as text: no shell reads it. Its input
// stays open for the turn's next message, and its output says when Claude is done with each.
fn args(session: &str, resume: bool) -> Vec<String> {
    let resume = if resume { "--resume" } else { "--session-id" };
    [STRICT_MCP, "--mcp-config", NO_MCP, "-p", resume, session, "--permission-mode", PERMISSION_MODE, "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"].map(String::from).into()
}

pub struct Started {
    pub process: Child,
    pub input: Input,
    pub output: ChildStdout,
}

// The turn's agent in this terminal, on `session`, with `env` set and `unset` removed.
pub fn start(session: &Id, resume: bool, env: &[(&str, String)], unset: &[&str]) -> Result<Started, Error> {
    let mut command = Command::new(PROGRAM);
    command.args(args(session, resume)).envs(env.iter().map(|(k, v)| (k, v)));
    unset.iter().for_each(|name| {
        command.env_remove(name);
    });
    let mut process = command.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().context(PROGRAM)?;
    let (Some(input), Some(output)) = (process.stdin.take(), process.stdout.take()) else {
        return Err(Error::Failed(format!("{PROGRAM}: no input or output")));
    };
    Ok(Started { process, input: Input(input), output })
}

// The turn's input; dropping it ends the agent once it is done.
pub struct Input(ChildStdin);

impl Input {
    // One message, as the owner's.
    pub fn send(&mut self, text: &str) -> Result<(), Error> {
        message(&mut self.0, text)
    }
}

fn message(input: &mut impl Write, text: &str) -> Result<(), Error> {
    let message = json!({ "type": "user", "message": { "role": "user", "content": text } });
    writeln!(input, "{message}").and_then(|()| input.flush()).context("writing to Claude")?;
    Ok(())
}

// What the turn's output says: text the agent wrote, or that it is done with a message
// written to it, one each, even for a message written while it works.
#[derive(Debug, PartialEq)]
pub enum Heard {
    Text(String),
    Done,
}

// One line of the stream, the fixed parts of the events read here; any other type is `Other`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
    Assistant { message: Said },
    // The turn's end of a message, whatever its subtype.
    Result,
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct Said {
    content: Vec<Block>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Block {
    Text {
        text: String,
    },
    #[serde(other)]
    Other,
}

// Each thing the output says, in order, until it ends, as the agent has exited.
pub fn hear(output: impl Read, mut heard: impl FnMut(Heard)) {
    for line in BufReader::new(output).lines().map_while(Result::ok) {
        let Ok(event) = serde_json::from_str::<Event>(&line) else { continue };
        match event {
            Event::Assistant { message } => message.content.into_iter().filter_map(|block| if let Block::Text { text } = block { Some(text) } else { None }).for_each(|text| heard(Heard::Text(text))),
            Event::Result => heard(Heard::Done),
            Event::Other => {}
        }
    }
}

#[cfg(test)]
mod tests;
