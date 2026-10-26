// Claude Code's event adapter (docs/harness.md): its hooks' input and its transcript's lines,
// read into the neutral calls, ends and states the hook decides its posts from (hook.rs). The
// one place that knows either format; the transcript's is internal to Claude Code and may
// change with any release, so each read here fails closed, to nothing, never to a wrong
// call. Only a tool's kind, a shell call's command, a file, a result's flags and a notice's status are
// read, never what a call or its result says, but for a traced session's (hook::narrate): the
// worker's text between calls, a call's detail and its output, which go to Linear only redacted.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::Value;

use crate::json::{nodes, non_empty, optional_text, str};
use crate::machine::Observed;
use crate::time::instant;

// What a tool is, as far as the posts tell tools apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Shell,
    SubAgent,
    Edit,
    Read,
    Search,
    LookUp,
    Other,
}

fn tool(name: &str) -> Tool {
    match name {
        "Bash" | "PowerShell" => Tool::Shell,
        "Agent" | "Task" => Tool::SubAgent,
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => Tool::Edit,
        "Read" => Tool::Read,
        "Grep" | "Glob" | "LSP" => Tool::Search,
        "WebFetch" | "WebSearch" => Tool::LookUp,
        _ => Tool::Other,
    }
}

// One tool call, as a hook's input gives it.
#[derive(Debug, PartialEq)]
pub struct Call {
    pub tool: Tool,
    // A shell call's command, and the file a call names, as the tool gave it.
    pub command: Option<String>,
    pub file: Option<String>,
    // What a search, a look-up or a sub-agent was for: its pattern, query, address or
    // description.
    pub detail: Option<String>,
    pub id: String,
    pub transcript: Option<PathBuf>,
    pub cwd: String,
    // Made by a sub-agent, whose calls the worker's transcript may not hold, so they post
    // nothing.
    pub sub_agent: bool,
    // After the call: it failed (`PostToolUseFailure`), or it went to the background, to end
    // later with a notice (hook::tick).
    pub failed: bool,
    pub launched: bool,
}

// What Claude Code gives a tool call's hooks on stdin (PreToolUse, PostToolUse,
// PostToolUseFailure), its fixed fields. Decoded leniently, as each hook input here is: a
// hook never fails the agent's call, so input that doesn't decode reports nothing.
#[derive(Debug, Deserialize)]
struct ToolInput {
    hook_event_name: String,
    cwd: String,
    transcript_path: Option<String>,
    tool_name: String,
    tool_use_id: String,
    // Set on a sub-agent's call.
    agent_id: Option<String>,
    // The tool's own arguments and what it returned, any tool's: open shapes, read by pointer.
    #[serde(default)]
    tool_input: Value,
    #[serde(default)]
    tool_response: Value,
    // PostToolUseFailure's.
    error: Option<String>,
}

// What Claude Code gives the state hooks on stdin (UserPromptSubmit, Stop, Notification,
// SessionEnd), its fixed fields, decoded leniently as `ToolInput` is.
#[derive(Debug, Deserialize)]
struct StateInput {
    hook_event_name: String,
    cwd: String,
    transcript_path: Option<String>,
    // Notification's and SessionEnd's.
    notification_type: Option<String>,
    reason: Option<String>,
}

// A field Claude Code sent empty, read as one it didn't send.
fn filled(text: &str) -> Option<&str> {
    (!text.is_empty()).then_some(text)
}

// None for input that isn't a tool call's.
pub fn parse_call(text: &str) -> Option<Call> {
    let input: ToolInput = serde_json::from_str(text).ok()?;
    let tool = tool(filled(&input.tool_name)?);
    let args = &input.tool_input;
    Some(Call {
        tool,
        command: command(tool, args),
        file: file(args),
        detail: ["/pattern", "/query", "/url", "/description"].iter().find_map(|f| non_empty(args, f)),
        id: filled(&input.tool_use_id)?.to_string(),
        transcript: input.transcript_path.as_deref().and_then(filled).map(PathBuf::from),
        cwd: filled(&input.cwd)?.to_string(),
        sub_agent: input.agent_id.as_deref().and_then(filled).is_some(),
        failed: input.hook_event_name == "PostToolUseFailure",
        launched: launch(&input.tool_response),
    })
}

// The file a call's input names.
fn file(input: &Value) -> Option<String> {
    ["/file_path", "/notebook_path"].iter().find_map(|f| optional_text(input, f))
}

// A shell call's command; none for another tool's, whatever its input holds, so only a shell
// call can read as the worker's own `tolkster` run.
fn command(tool: Tool, input: &Value) -> Option<String> {
    (tool == Tool::Shell).then(|| str(input, "/command").map(String::from)).flatten()
}

// Whether a call's result says it went to the background: a sub-agent or a shell command
// started there, whose notice comes when it ends. The result is its tool's own, an open shape.
fn launch(result: &Value) -> bool {
    result.get("isAsync").and_then(Value::as_bool) == Some(true) || non_empty(result, "/backgroundTaskId").is_some()
}

// The agent's state as one of its hooks says it changed, and the directory it runs in. None
// for any other input, so a hook that can't say for sure reports nothing and leaves it to
// Orca's listing: a prompt taken is working, a turn ended idle, or in the background while a
// call it sent there since `since_ms` has no notice yet by the transcript's end (`read`), a
// permission prompt or a question waiting, and an exit exited, but not `/clear` or `/resume`,
// after which the agent goes on.
pub fn parse_state(text: &str, since_ms: i64, read: impl FnOnce(&Path) -> Option<String>) -> Option<(Observed, String)> {
    let input: StateInput = serde_json::from_str(text).ok()?;
    let observed = match input.hook_event_name.as_str() {
        "UserPromptSubmit" => Observed::AgentWorking,
        "Stop" if input.transcript_path.as_deref().and_then(filled).and_then(|path| read(Path::new(path))).is_some_and(|tail| backgrounded(&tail, since_ms)) => Observed::AgentBackground,
        "Stop" => Observed::AgentIdle,
        "Notification" if matches!(input.notification_type.as_deref()?, "permission_prompt" | "elicitation_dialog") => Observed::AgentWaiting,
        "SessionEnd" if matches!(input.reason.as_deref()?, "prompt_input_exit" | "logout" | "other") => Observed::AgentExited,
        _ => return None,
    };
    Some((observed, filled(&input.cwd)?.to_string()))
}

// The transcript of a stop's or the session's end's input; None for any other.
pub fn ended(text: &str) -> Option<PathBuf> {
    let input: StateInput = serde_json::from_str(text).ok()?;
    matches!(input.hook_event_name.as_str(), "Stop" | "SessionEnd").then(|| input.transcript_path.as_deref().and_then(filled).map(PathBuf::from)).flatten()
}

// A tool call in the transcript, at its entry's time.
pub struct ToolUse {
    pub at: i64,
    pub id: String,
    pub tool: Tool,
    // As `Call`'s.
    pub command: Option<String>,
    pub file: Option<String>,
}

impl ToolUse {
    // The call as a hook's input would give it, in the hook `of`'s transcript and directory.
    pub fn as_call(&self, of: &Call) -> Call {
        Call { tool: self.tool, command: self.command.clone(), file: self.file.clone(), detail: None, id: self.id.clone(), transcript: of.transcript.clone(), cwd: of.cwd.clone(), sub_agent: false, failed: false, launched: false }
    }
}

// A transcript line, the fixed parts of the entries read here; any other type is `Other`.
// Decoded leniently: a line that doesn't, as the first of a cut tail may not, is skipped.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum Entry {
    #[serde(rename_all = "camelCase")]
    Assistant {
        timestamp: String,
        #[serde(default)]
        is_sidechain: bool,
        message: Message,
    },
    #[serde(rename_all = "camelCase")]
    User {
        timestamp: Option<String>,
        message: Message,
        // What the call returned, its tool's own: an open shape.
        #[serde(default)]
        tool_use_result: Value,
    },
    // Either carries the notice Claude Code queues for the worker once a background call ends.
    QueueOperation { content: Option<String>, timestamp: Option<String> },
    Attachment { attachment: Queued, timestamp: Option<String> },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
struct Queued {
    prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Message {
    // An assistant message's, which each of its entries shares.
    id: Option<String>,
    content: Content,
}

// A message's content: its parts; anything else, as a prompt's text alone, holds none.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Content {
    Parts(Vec<Part>),
    Other(IgnoredAny),
}

impl Message {
    fn parts(&self) -> &[Part] {
        match &self.content {
            Content::Parts(parts) => parts,
            Content::Other(_) => &[],
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Part {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        // The tool's own arguments, any tool's: an open shape.
        #[serde(default)]
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        is_error: Option<bool>,
    },
    #[serde(other)]
    Other,
}

// The tool calls in the transcript's lines: each `tool_use` in an assistant entry.
pub fn tool_calls(tail: &str) -> Vec<ToolUse> {
    let mut calls = Vec::new();
    for line in tail.lines() {
        let Ok(Entry::Assistant { timestamp, message, .. }) = serde_json::from_str(line) else { continue };
        let Some(at) = instant(&timestamp) else { continue };
        for part in message.parts() {
            if let Part::ToolUse { id, name, input } = part {
                let tool = tool(name);
                calls.push(ToolUse { at, id: id.clone(), tool, command: command(tool, input), file: file(input) });
            }
        }
    }
    calls
}

// The worker's text between the tool call before `id` and `id`, or the tail's end when it
// doesn't hold `id`: each `text` part of its own assistant entries, a sub-agent's aside. Its
// `thinking` parts are left out: Claude Code writes them empty.
pub fn narration(tail: &str, id: &str) -> Vec<String> {
    let mut texts = Vec::new();
    for line in tail.lines() {
        let Ok(Entry::Assistant { is_sidechain: false, message, .. }) = serde_json::from_str(line) else { continue };
        for part in message.parts() {
            match part {
                Part::ToolUse { id: this, .. } if this == id => return texts,
                Part::ToolUse { .. } => texts.clear(),
                Part::Text { text } => texts.extend(Some(text.trim()).filter(|t| !t.is_empty()).map(String::from)),
                _ => {}
            }
        }
    }
    texts
}

// What a call printed, as the hook after it is given it: whether it failed, its exit code
// when its output starts by saying it, its text, and its count of lines or results when its
// tool says one.
#[derive(Debug, Default, PartialEq)]
pub struct Output {
    pub failed: bool,
    pub exit: Option<i64>,
    pub text: String,
    pub count: Option<u64>,
    // It went to the background, and ends later with a notice.
    pub launched: bool,
}

pub fn parse_output(text: &str) -> Option<Output> {
    let input: ToolInput = serde_json::from_str(text).ok()?;
    // What the tool returned, each tool its own shape: an open one, read by pointer.
    let response = &input.tool_response;
    let failed = input.hook_event_name == "PostToolUseFailure";
    let joined = |parts: &[&str]| parts.iter().filter_map(|p| str(response, p)).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("
");
    let text = match (input.error.as_deref(), response) {
        (Some(error), _) => error.to_string(),
        (None, Value::String(text)) => text.clone(),
        (None, _) if response.get("stdout").is_some() => joined(&["/stdout", "/stderr"]),
        (None, _) => match str(response, "/file/content").or_else(|| str(response, "/content")) {
            Some(text) => text.to_string(),
            None => nodes(response, "/content").iter().filter_map(|p| str(p, "/text")).collect::<Vec<_>>().join("
"),
        },
    };
    let exit = text.strip_prefix("Exit code ").and_then(|rest| rest.split_whitespace().next()).and_then(|n| n.parse().ok());
    let count = ["/file/numLines", "/numLines", "/numFiles"].iter().find_map(|p| response.pointer(p).and_then(Value::as_u64)).or_else(|| response.get("filenames").and_then(Value::as_array).map(|f| f.len() as u64));
    Some(Output { failed, exit, text, count, launched: launch(response) })
}

// How a call ended: its result, whether it failed; a result that says it went to the
// background; or the notice that came once it ended there, whether it failed, None when it
// was stopped rather than finished.
pub enum End {
    Returned(bool),
    Launched,
    Notice(Option<bool>),
}

// How each call in `tail` ended, by id. The notice Claude Code queues for the worker when a
// call in the background ends wins over its result.
pub fn ends(tail: &str) -> HashMap<String, End> {
    let mut ends = HashMap::new();
    for line in tail.lines() {
        match serde_json::from_str(line) {
            Ok(Entry::QueueOperation { content: Some(notice), .. } | Entry::Attachment { attachment: Queued { prompt: Some(notice) }, .. }) => {
                if let Some((id, failed)) = notices(&notice) {
                    ends.insert(id, End::Notice(failed));
                }
            }
            Ok(Entry::User { message, tool_use_result, .. }) => {
                let launched = launch(&tool_use_result);
                for part in message.parts() {
                    if let Part::ToolResult { tool_use_id, is_error } = part {
                        ends.entry(tool_use_id.clone()).or_insert(if launched { End::Launched } else { End::Returned(*is_error == Some(true)) });
                    }
                }
            }
            _ => {}
        }
    }
    ends
}

// What a transcript says happened, as the timing reads it (hook::timing): text given to the
// agent, a part of its reply by the reply's ID, a call, and a call's end, by its ID.
pub enum Moment {
    Prompt,
    Reply(String),
    Call(ToolUse),
    Ended(String, End),
}

// Each moment of the transcript's lines, at its entry's time, in order; an entry with no time
// says none.
pub fn moments(text: &str) -> Vec<(i64, Moment)> {
    let mut moments = Vec::new();
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Entry>(line) else { continue };
        let (at, mut these) = match entry {
            Entry::Assistant { timestamp, message, .. } => {
                let calls = message.parts().iter().filter_map(|part| match part {
                    Part::ToolUse { id, name, input } => {
                        let tool = tool(name);
                        Some(Moment::Call(ToolUse { at: 0, id: id.clone(), tool, command: command(tool, input), file: file(input) }))
                    }
                    _ => None,
                });
                (Some(timestamp), message.id.clone().map(Moment::Reply).into_iter().chain(calls).collect::<Vec<_>>())
            }
            Entry::User { timestamp, message, tool_use_result } => {
                let launched = launch(&tool_use_result);
                let ended: Vec<Moment> = message.parts().iter().filter_map(|part| match part {
                    Part::ToolResult { tool_use_id, is_error } => Some(Moment::Ended(tool_use_id.clone(), if launched { End::Launched } else { End::Returned(*is_error == Some(true)) })),
                    _ => None,
                }).collect();
                (timestamp, if ended.is_empty() { vec![Moment::Prompt] } else { ended })
            }
            Entry::QueueOperation { content: Some(notice), timestamp } | Entry::Attachment { attachment: Queued { prompt: Some(notice) }, timestamp } => (timestamp, notices(&notice).map(|(id, failed)| Moment::Ended(id, End::Notice(failed))).into_iter().collect()),
            _ => continue,
        };
        let Some(at) = at.as_deref().and_then(instant) else { continue };
        for moment in &mut these {
            if let Moment::Call(call) = moment {
                call.at = at;
            }
        }
        moments.extend(these.into_iter().map(|moment| (at, moment)));
    }
    moments
}

// The session a transcript is of, by its file's name, and its sub-agents' transcripts beside it.
pub fn transcript_session(transcript: &Path) -> Option<String> {
    transcript.file_stem()?.to_str().map(String::from)
}

pub fn sub_agents(transcript: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(transcript.with_extension("").join("subagents")) else { return Vec::new() };
    entries.filter_map(|entry| entry.ok().map(|e| e.path())).filter(|path| path.extension().is_some_and(|e| e == "jsonl")).collect()
}

// How far back a call sent to the background counts: one whose process died with no notice,
// as a closed agent's does before a resume on its transcript, would otherwise hold every later
// stop. The relay holds a listing's idle as long (src/worker/listing.ts, BACKGROUND_HOLD_MS).
pub const BACKGROUND_FOR_MS: i64 = 30 * 60 * 1000;

// Whether a call in `tail` made since `since_ms` went to the background and its notice hasn't come.
fn backgrounded(tail: &str, since_ms: i64) -> bool {
    let ends = ends(tail);
    tool_calls(tail).iter().any(|call| call.at >= since_ms && matches!(ends.get(&call.id), Some(End::Launched)))
}

// Whether `tail` reaches back before `since_ms`: it holds a call made before it.
pub fn reaches(tail: &str, since_ms: i64) -> bool {
    tool_calls(tail).first().is_some_and(|call| call.at < since_ms)
}

// The call a task notice names, and whether it failed: None for one stopped or killed rather
// than finished. Only a `text` that is a notice, from its start, so one quoted in a notice's
// result can't end another call.
fn notices(text: &str) -> Option<(String, Option<bool>)> {
    let tag = |notice: &str, name: &str| -> Option<String> {
        let (_, rest) = notice.split_once(&format!("<{name}>"))?;
        Some(rest.split_once(&format!("</{name}>"))?.0.trim().to_string())
    };
    let notice = text.trim_start().strip_prefix("<task-notification>")?;
    let notice = notice.split("</task-notification>").next()?;
    let failed = match tag(notice, "status")?.as_str() {
        "completed" => Some(false),
        "failed" => Some(true),
        _ => None,
    };
    Some((tag(notice, "tool-use-id")?, failed))
}
