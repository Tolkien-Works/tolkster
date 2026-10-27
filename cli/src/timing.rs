// A worker session's timing, and what `listen` keeps beside it for each issue
// (docs/timing.md): the summary a worker's hook makes of its own transcript (hook::timing),
// the report its `tolkster` sends, the line the relay forwards to `listen`, and the file
// `listen` keeps on its host. test/contract.json's `timing` holds the report and the forward,
// which src/worker/timing.ts reads the same. IDs, enums, counts, durations and offsets only:
// never a command's text, a path, output, or Linear or GitHub text.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::Error;
use crate::estimates::Size;
use crate::plain::{Id, Sha, Time, split_worktree};
use crate::state::derived_uuid;
use crate::sync::lock;
use crate::time::{instant, iso, now_ms};
use crate::trace::{self, Undone, Work};

// How a worker's agent was launched (setup.rs), in its environment for its hooks: the kind,
// the launch's time in ms and the build, `-` for none.
pub const LAUNCH_ENV: &str = "TOLKSTER_LAUNCH";

// How long a line is kept: one older is dropped as the file is written.
pub const WINDOW_MS: i64 = 90 * 86_400_000;

// The most check runs one summary carries, as the relay takes them.
pub const MOST_CHECKS: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Start,
    Resume,
    Round,
    Rebase,
}

const KINDS: [(Kind, &str); 4] = [(Kind::Start, "start"), (Kind::Resume, "resume"), (Kind::Round, "round"), (Kind::Rebase, "rebase")];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckKind {
    CargoTest,
    Clippy,
    CargoCheck,
    NodeTest,
    Simulator,
    Typecheck,
    Gate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum By {
    Main,
    SubAgent,
}

// `failed` None when the call's end doesn't say (hook::known); `edited`, whether a file was
// edited since the session's previous check run; `at`, seconds from the session's start.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckRun {
    pub kind: CheckKind,
    pub failed: Option<bool>,
    pub edited: bool,
    pub seconds: u64,
    pub at: u64,
    pub by: By,
}

// Seconds the main session's tool calls ran, by kind; calls at once each count.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tools {
    pub read: u64,
    pub edit: u64,
    pub check: u64,
    pub git: u64,
    pub bookkeeping: u64,
    pub agent: u64,
    pub other: u64,
}

// Seconds from the session's start to each.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Offsets {
    pub first_edit: Option<u64>,
    pub first_commit: Option<u64>,
    pub first_gate: Option<u64>,
    pub review_start: Option<u64>,
    pub review_end: Option<u64>,
    pub pr: Option<u64>,
}

// `session` is Claude's session ID, `launched` the launch's time, `at` the session's first
// entry since it; `active` leaves out idle gaps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Summary {
    pub session: String,
    pub launched: String,
    pub kind: Kind,
    pub build: Option<String>,
    pub at: String,
    pub wall: u64,
    pub active: u64,
    pub model: u64,
    pub tools: Tools,
    pub api_turns: u64,
    pub tool_turns: u64,
    pub offsets: Offsets,
    pub checks: Vec<CheckRun>,
}

impl Summary {
    // Whether its strings have their plain shapes, as the relay checks them: a build is
    // lowercase, as git names a commit.
    fn plain(&self) -> bool {
        let build = |b: &str| Sha::parse(b).is_some() && !b.bytes().any(|c| c.is_ascii_uppercase());
        Id::parse(&self.session).is_some() && Time::parse(&self.launched).is_some() && Time::parse(&self.at).is_some() && self.build.as_deref().is_none_or(build) && self.checks.len() <= MOST_CHECKS
    }
}

// What `LAUNCH_ENV` holds.
pub fn launch_value(kind: Kind, launched_ms: i64, build: Option<&Sha>) -> String {
    let name = KINDS.iter().find(|(k, _)| *k == kind).map_or("", |(_, name)| name);
    format!("{name} {launched_ms} {}", build.map_or("-", |b| b.as_str()))
}

// The kind, the launch's time and the build `launch_value` wrote; None for any other text.
pub fn launch_of(text: &str) -> Option<(Kind, i64, Option<Sha>)> {
    let mut words = text.split(' ');
    let name = words.next()?;
    let kind = KINDS.iter().find(|(_, n)| *n == name)?.0;
    let launched = words.next()?.parse().ok()?;
    let build = match words.next()? {
        "-" => None,
        build => Some(Sha::parse(build)?),
    };
    words.next().is_none().then_some((kind, launched, build))
}

// The worker's report of its session in the worktree `worktree_id`, which the relay forwards
// to `listen` without it. None for one of the wrong shape, which is never sent.
pub fn report(worktree_id: &str, summary: &Summary) -> Option<Value> {
    (split_worktree(worktree_id).is_some() && summary.plain()).then(|| json!({ "op": "report", "report": "sessionTiming", "worktreeId": worktree_id, "timing": summary }))
}

// A session's timing as the relay forwards it, with its issue (src/worker/timing.ts). None for
// anything else; Err for one whose shape doesn't read.
pub fn forwarded(value: &Value) -> Option<Result<(Id, Summary), String>> {
    if value.get("type")?.as_str()? != "timing" {
        return None;
    }
    let issue = value.get("issueId").and_then(Value::as_str).and_then(Id::parse);
    let summary = value.get("timing").cloned().and_then(|t| serde_json::from_value::<Summary>(t).ok()).filter(Summary::plain);
    Some(issue.zip(summary).ok_or_else(|| "a session's timing of the wrong shape".to_string()))
}

// How full a line is, which a fuller one of its key replaces: a session's report runs longer,
// or as long with more check runs, as a sub-agent's may end after the session's own last
// moment; a merge at GitHub's time is fuller than one at the time `listen` heard of it.
type Fullness = (u64, usize);

fn fullness(wall: Option<u64>, exact: bool, checks: usize) -> Fullness {
    (wall.unwrap_or(u64::from(exact)), checks)
}

// The file `listen` keeps (docs/timing.md), one JSON line per session, start or merge, each
// with its `key` and `at`. A line is written once by its key, and again only by a fuller one,
// which replaces it. The file is read only to keep each line so, and for reports. None: a
// store that keeps nothing, as a command run on its own or a test has. One writer at a time in
// this process; a `listen` draining beside its successor may lose a line to it.
#[derive(Default)]
pub struct Timing {
    file: Option<PathBuf>,
    // How full each start and merge this process kept is, by key, so one said again reads the
    // file once. A session's report, which grows, reads it each time.
    held: Mutex<HashMap<String, Fullness>>,
    // The `timing` setting, off: writes nothing, a late report included, and what is kept stays.
    off: AtomicBool,
}

// A part file older than this was left by a process cut off before its rename.
const PART_LEFT: std::time::Duration = std::time::Duration::from_secs(10 * 60);

impl Timing {
    // Clears the part files a cut-off `listen` left beside `file`.
    pub fn at(file: PathBuf) -> Timing {
        let name = file.file_name().map(|n| format!("{}.", n.to_string_lossy())).unwrap_or_default();
        let parts = file.parent().and_then(|dir| std::fs::read_dir(dir).ok()).into_iter().flatten().filter_map(Result::ok);
        for part in parts.filter(|e| e.file_name().to_string_lossy().strip_prefix(&name).is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()))) {
            if part.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > PART_LEFT) {
                let _ = std::fs::remove_file(part.path());
            }
        }
        Timing { file: Some(file), held: Mutex::default(), off: AtomicBool::new(false) }
    }

    // The `timing` setting as `listen` last read it (Shared::settings).
    pub fn allow(&self, on: bool) {
        self.off.store(!on, Ordering::Relaxed);
    }

    // Keyed by its issue too, so one host's report can't replace another issue's line.
    pub fn session(&self, issue_id: &Id, summary: &Summary) {
        let Ok(Value::Object(fields)) = serde_json::to_value(summary) else { return };
        let mut line = json!({ "line": "session", "key": derived_uuid(&format!("session {issue_id} {} {}", summary.session, summary.launched)), "issueId": issue_id });
        if let Some(object) = line.as_object_mut() {
            object.extend(fields);
        }
        self.keep(line, fullness(Some(summary.wall), false, summary.checks.len()));
    }

    // A worker started for the issue by the command `command_id`, on the rung `listen`'s config
    // names `rung`.
    pub fn started(&self, issue_id: &str, identifier: &str, command_id: &str, size: Option<Size>, rung: &str) {
        let key = derived_uuid(&format!("start {command_id}"));
        self.keep(json!({ "line": "start", "key": key, "at": iso(now_ms()), "issueId": issue_id, "identifier": identifier, "size": size, "rung": rung }), fullness(None, false, 0));
    }

    // The issue's work merged at `at_ms`: its PR `pr`, or landed with none. `exact`: the time is
    // GitHub's; otherwise when `listen` heard of it, kept only until GitHub's is read.
    pub fn merged(&self, issue_id: &str, identifier: &str, pr: Option<u64>, at_ms: i64, exact: bool) {
        let key = derived_uuid(&format!("merge {issue_id} {}", pr.map_or_else(|| "landed".into(), |n| n.to_string())));
        self.keep(json!({ "line": "merge", "key": key, "at": iso(at_ms), "issueId": issue_id, "identifier": identifier, "pr": pr, "exact": exact }), fullness(None, exact, 0));
    }

    // A failure costs only the line: it goes to the trace.
    fn keep(&self, line: Value, full: Fullness) {
        let Some(file) = self.file.as_ref().filter(|_| !self.off.load(Ordering::Relaxed)) else { return };
        let key = line.get("key").and_then(Value::as_str).unwrap_or_default().to_string();
        let mut held = lock(&self.held);
        let session = line.get("line").and_then(Value::as_str) == Some("session");
        if !session && held.get(&key).is_some_and(|was| *was >= full) {
            return;
        }
        let kept = (|| -> Result<(), Error> {
            let text = crate::cli::read_if_there(file)?.unwrap_or_default();
            let Some(text) = kept_in(&text, &line, full, now_ms()) else { return Ok(()) };
            // Written aside, under this process's own name, so a `listen` draining beside its
            // successor never sees half of theirs.
            crate::cli::replace_file(file, &text)
        })();
        match kept {
            Ok(()) => {
                held.insert(key, full);
            }
            Err(error) => trace::event(Work::NotDone { what: Undone::Timing, error }),
        }
    }
}

// What a held line is read for: never the whole of it.
#[derive(Deserialize)]
struct Held {
    key: Option<String>,
    at: Option<String>,
    wall: Option<u64>,
    #[serde(default)]
    exact: bool,
    #[serde(default)]
    checks: Vec<IgnoredAny>,
}

// The file's text with `line` kept, and every line older than the window, or unreadable,
// dropped; None when `line` is held already, as fully, and nothing is dropped.
pub fn kept_in(text: &str, line: &Value, full: Fullness, now_ms: i64) -> Option<String> {
    let key = line.get("key").and_then(Value::as_str)?;
    let fresh = |at: Option<&str>| at.and_then(instant).is_some_and(|at| at >= now_ms - WINDOW_MS);
    let new = line.to_string();
    let read: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let mut kept: Vec<(Option<Held>, &str)> = read.iter().filter_map(|l| serde_json::from_str::<Held>(l).ok().filter(|h| fresh(h.at.as_deref())).map(|h| (Some(h), *l))).collect();
    let dropped = kept.len() < read.len();
    let held = kept.iter().position(|(h, _)| h.as_ref().and_then(|h| h.key.as_deref()) == Some(key));
    let fuller = |held: &Held| full > fullness(held.wall, held.exact, held.checks.len());
    match held {
        Some(at) if kept[at].0.as_ref().is_some_and(fuller) => kept[at].1 = &new,
        None if fresh(line.get("at").and_then(Value::as_str)) => kept.push((None, &new)),
        _ if !dropped => return None,
        _ => {}
    }
    Some(kept.iter().map(|(_, l)| format!("{l}\n")).collect())
}

pub mod report;

#[cfg(test)]
mod tests;
