// A traced session (a repo's `trace`, docs/voice.md): the worker's text between calls as one
// durable thought ahead of each call's durable action, the call and a short outcome. Only the
// session the worker's launch names (`session`) is traced, as `listen` put it on the launch
// line, never one the transcript names; a call made in any other posts as untraced. Posted
// by `hook after`, which has the call's output, and for the text ahead of the worker's own
// post by `hook start`, which that post waits on, so it lands before a response. Everything
// that came from the transcript or a call goes through the redactor (hook::redact), and what
// a call printed only as a count, but for the tail of a failed test or build: a call that
// runs `tolkster` or `orca`, which print Linear's and GitHub's text, never shows more than its
// lines.

use super::redact::{Scrub, cap};
use super::shell::{known, pair_of, program, segments};
use super::transcript::tool_calls;
use super::whereabouts::report;
use super::{Call, Tool};
use crate::harness::events::{Output, narration};

const NARRATION_MOST: usize = 2000;
const PARAMETER_MOST: usize = 200;
const TAIL_LINES: usize = 20;
const TAIL_MOST: usize = 1500;

// The session the worker's launch traces (setup.rs), when its repo's `trace` is on.
pub fn session() -> Option<String> {
    crate::cli::env(crate::cli::TRACE_ENV).and_then(|id| crate::plain::Id::parse(&id).map(String::from))
}

// The worker's text before the call `id`, as a thought's body. None when it wrote none.
pub fn thought(tail: &str, id: &str, scrub: &Scrub) -> Option<String> {
    let text = narration(tail, id).join("\n\n");
    (!text.is_empty()).then(|| scrub.text(&text, NARRATION_MOST))
}

// A call the trace leaves out whole, its text before it included: the worker's own post,
// whose text before it `hook start` posts, and one that runs `orca`, as the closing command
// does after the final response, which a post would reopen.
pub fn left_out(call: &Call) -> bool {
    call.sub_agent || call.command.as_deref().is_some_and(|command| report(command).is_some() || runs(command, &["orca"]))
}

// Whether the call `id` was made in `session`, the one the launch traces, by the worker's
// newest post through it: a call in any other posts as untraced.
pub fn traced(tail: &str, id: &str, session: &str) -> bool {
    super::report_through(tail, id).is_some_and(|report| report.session == session)
}

// Whether the call is the worker's own post (`tolkster activity` or `plan`), and its session.
pub fn posts_in(call: &Call) -> Option<String> {
    call.command.as_deref().and_then(report).map(|r| r.session).filter(|s| !s.is_empty())
}

fn runs(command: &str, programs: &[&str]) -> bool {
    segments(command).iter().any(|words| programs.contains(&program(words).as_str()))
}

// A call's durable action: what it did, what on, and how it went; a commit's names its subject
// line (`committed`), read from git, as its command may not hold it. None for a tool the trace
// has no word for.
pub fn action(call: &Call, output: &Output, committed: Option<&str>, scrub: &Scrub) -> Option<(String, String, String)> {
    if let Some(subject) = committed.filter(|_| call.tool == Tool::Shell) {
        return Some((crate::voice::COMMITTED.into(), scrub.text(subject, PARAMETER_MOST), outcome(call, output, scrub)));
    }
    let verb = match call.tool {
        Tool::Shell => "Ran",
        Tool::Edit => "Edited",
        Tool::Read => "Read",
        Tool::Search => "Searched",
        Tool::LookUp => "Looked up",
        Tool::SubAgent => "Delegated",
        Tool::Other => return None,
    };
    let on = match call.tool {
        Tool::Shell => call.command.as_deref().map(first_line),
        Tool::Edit | Tool::Read => call.file.clone(),
        _ => call.detail.clone(),
    };
    let parameter = on.map_or_else(String::new, |on| scrub.text(&on, PARAMETER_MOST));
    Some((verb.into(), parameter, outcome(call, output, scrub)))
}

fn first_line(command: &str) -> String {
    let mut lines = command.trim().lines();
    let first = lines.next().unwrap_or_default().trim().to_string();
    if lines.next().is_some() { format!("{first} …") } else { first }
}

fn lines(n: u64, of: &str) -> String {
    format!("{n} {of}{}", if n == 1 { "" } else { "s" })
}

fn outcome(call: &Call, output: &Output, scrub: &Scrub) -> String {
    let count = output.count.unwrap_or_else(|| output.text.lines().count() as u64);
    let command = call.command.as_deref();
    match call.tool {
        _ if output.launched => "running in the background".into(),
        Tool::Shell if command.is_some_and(|c| runs(c, &["tolkster", "orca"])) => {
            let read = format!("read {}", lines(count, "line"));
            if output.failed { format!("failed · {read}") } else { read }
        }
        Tool::Shell => {
            let status = output.exit.map_or_else(|| if output.failed { "failed".into() } else { "exit 0".to_string() }, |n| format!("exit {n}"));
            let said = format!("{status} · {}", lines(count, "line"));
            let check = matches!(pair_of(call.tool, command), Some("testing" | "building")) && known(call.tool, command);
            if output.failed && check { format!("{said}\n```\n{}\n```", tail(&output.text, scrub)) } else { said }
        }
        _ if output.failed => "failed".into(),
        Tool::Read => format!("read {}", lines(count, "line")),
        Tool::Search => lines(count, "result"),
        _ => "done".into(),
    }
}

// The output's last lines, redacted, with no fence of its own. Redacted over more lines than
// are kept, so a key block the cut would leave without its start is still known as one.
fn tail(text: &str, scrub: &Scrub) -> String {
    let all: Vec<&str> = text.lines().collect();
    let scrubbed = scrub.text(&all[all.len().saturating_sub(TAIL_LINES * 10)..].join("\n"), usize::MAX);
    let kept: Vec<&str> = scrubbed.lines().collect();
    let tail = kept[kept.len().saturating_sub(TAIL_LINES)..].join("\n").replace("```", "'''");
    // The end is what tells why it failed.
    let chars: Vec<char> = tail.chars().collect();
    if chars.len() > TAIL_MOST { format!("…{}", chars[chars.len() - TAIL_MOST..].iter().collect::<String>()) } else { cap(&tail, TAIL_MOST) }
}

// Whether the worker has posted a response, a question or an error since the call `id`: a
// trace post now would land after it, and reopen a finished session or talk over a question.
pub fn quieted(tail: &str, id: &str) -> bool {
    let calls = tool_calls(tail);
    let after = calls.iter().position(|c| c.id == id).map_or(0, |this| this + 1);
    calls[after..].iter().filter_map(|c| report(c.command.as_deref()?)).any(|r| matches!(r.kind.as_deref(), Some("response" | "elicitation" | "error")))
}

#[cfg(test)]
mod tests;
