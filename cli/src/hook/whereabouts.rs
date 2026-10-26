// Where the worker is, and its newest post of its own, by the transcript's tool calls.

use std::collections::HashMap;

use serde_json::Value;

use super::{Call, Tool};
use super::shell::{events, heredoc, program, script_writes, segment_kind, segments};
use super::transcript::{End, ToolUse, ends, tool_calls};
use crate::git::Move;
use crate::linear::{PlanStep, steps};
use crate::plain::Id;
use crate::voice::At;

// The worker's newest post of its own up to the call `id`, as `last_report` reads one: the
// session it was reporting in when it made that call, not one it has moved to since. None
// for a call the tail doesn't hold.
pub fn report_through(tail: &str, id: &str) -> Option<Report> {
    let calls = tool_calls(tail);
    let this = calls.iter().position(|c| c.id == id)?;
    calls[..=this].iter().rev().find_map(|call| report(call.command.as_deref()?))
}

pub const UNDERSTANDING: &str = "understanding";
pub const CHANGING: &str = "changing";
pub const CHECKING: &str = "checking";
pub const SHIPPING: &str = "shipping";

// Where the worker is at `call`, by the calls in `tail` up to it, and the call itself when
// the tail doesn't hold it yet; `whole` when the tail is the whole transcript. Its plan
// counts only when its newest one was piped into its run (`plan_run`) in `session`, the one
// the line posts in, moved on by the events since (`progress`): one sent from a file, one
// scrolled out of the tail, one for another session, or a newest one that doesn't parse or
// has no step in progress leaves the phase. The step is the worker's own text, on one line
// with no markdown.
pub fn whereabouts(tail: &str, whole: bool, call: &Call, session: &str) -> At {
    let calls = tool_calls(tail);
    let upto = calls.iter().position(|c| c.id == call.id).map_or(&calls[..], |this| &calls[..=this]);
    let newest = upto.iter().rev().find_map(|c| plan_run(c.command.as_deref()?));
    if newest.is_some_and(|(of, _)| of == session) {
        let plan = progress(upto, &ends(tail), None, session, |_| true, false);
        let step = plan.as_ref().and_then(|plan| plan.spans.iter().find(|s| s.step.status == "inProgress"));
        if let Some(content) = step.and_then(|s| one_line(&s.step.content)) {
            return At::Step(content);
        }
    }
    let this = (!upto.iter().any(|c| c.id == call.id)).then(|| mark(call.tool, call.command.as_deref()));
    let marks = upto.iter().map(|c| mark(c.tool, c.command.as_deref())).chain(this);
    At::Phase(marks.flatten().fold(UNDERSTANDING, |phase, mark| match mark {
        Mark::Edit => CHANGING,
        Mark::Commit if phase == UNDERSTANDING => CHANGING,
        Mark::Commit => phase,
        // Before the first edit of a whole transcript, a check or a push, as of the empty
        // branch at the start, is still getting to know the task. A cut tail may have lost
        // the edits before them.
        Mark::Check | Mark::Ship if phase == UNDERSTANDING && whole => UNDERSTANDING,
        Mark::Check => CHECKING,
        Mark::Ship => SHIPPING,
    }))
}

// A `tolkster plan` run that is the command's own, by the session it names, and the plan
// piped into it (`piped_run`). A plan sent from a file has none.
pub(super) fn plan_run(command: &str) -> Option<(String, Option<&str>)> {
    let (words, body) = piped_run(command)?;
    let session = words.get(1).filter(|verb| *verb == "plan").and(words.get(2)).and_then(|id| Id::parse(id))?;
    let stdin = words.windows(2).any(|w| w[0] == "--file" && w[1] == "-");
    Some((session.to_string(), body.filter(|_| stdin)))
}

// The command's own `tolkster` run, its words, and what is piped into it: a heredoc's body
// after the run's line, up to its delimiter, `tolkster plan <id> --file - <<'EOF'`, or a
// PowerShell here-string before it, `@'` to `'@ | tolkster plan <id> --file -`. A `tolkster`
// line in another heredoc's body isn't a run.
fn piped_run(command: &str) -> Option<(Vec<String>, Option<&str>)> {
    let command = command.trim_start();
    let (line, body) = match command.strip_prefix("@'").and_then(|rest| rest.strip_prefix("\r\n").or_else(|| rest.strip_prefix('\n'))) {
        Some(rest) => {
            let end = rest.find("\n'@")?;
            (rest[end + 3..].lines().next().unwrap_or_default(), Some(&rest[..end]))
        }
        None => {
            let (line, rest) = command.split_once('\n').unwrap_or((command, ""));
            (line, line.split_once("<<").and_then(|(_, after)| heredoc(after, rest)))
        }
    };
    let words = segments(line).into_iter().find(|words| program(words) == "tolkster")?;
    Some((words, body))
}

// A plan's steps from its JSON, the first value in `body`.
pub(super) fn plan_of(body: &str) -> Option<Vec<PlanStep>> {
    let value = serde_json::Deserializer::from_str(body).into_iter::<Value>().next()?.ok()?;
    steps(&value).ok()
}

// The session's plan as it stands after `calls`: its newest plan piped into a run that
// `counts`, moved on by each event (shell::events) of a call since. A commit counts by the
// commits HEAD's reflog credits the call (`credit`), so one in a call that failed after it
// still counts and one that never happened doesn't; without the reflog, only in a call that
// returned unfailed. `pr create` counts once it returned unfailed. A response in the session
// that returned, or the call being one (`closing`), finishes it once its PR is open, or when
// it has no step that ends on one: so an answer to a message mid-task, or a response that
// stops short of the PR, moves nothing. None with no plan, or when the newest one doesn't
// parse. Worked out from the transcript and git alone, so nothing is stored.
pub(super) fn progress(calls: &[ToolUse], ended: &HashMap<String, End>, credit: Option<&Credit>, session: &str, counts: impl Fn(&ToolUse) -> bool, closing: bool) -> Option<Progress> {
    let mut plan: Option<Progress> = None;
    for (i, call) in calls.iter().enumerate() {
        let Some(command) = call.command.as_deref() else { continue };
        if let Some((of, body)) = plan_run(command) {
            if of == session && counts(call) {
                plan = body.and_then(plan_of).map(|steps| Progress::sent(steps, plan.as_ref(), i));
            }
            continue;
        }
        let Some(plan) = plan.as_mut() else { continue };
        let end = ended.get(&call.id);
        let returned = matches!(end, Some(End::Returned(false)));
        if report(command).is_some_and(|r| r.session == session && r.kind.as_deref() == Some("response")) {
            if returned {
                plan.finish(i);
            }
            continue;
        }
        let mut commits = match credit {
            Some(credit) => credit.get(&call.id).copied().unwrap_or(0),
            None if returned => usize::MAX,
            None => 0,
        };
        for event in events(command, session) {
            match event {
                "commit" if commits == 0 => continue,
                "commit" => commits -= 1,
                _ if !returned => continue,
                _ => {}
            }
            plan.on(event, i + 1);
        }
    }
    if closing && let Some(plan) = plan.as_mut() {
        plan.finish(calls.len());
    }
    plan
}

// How many commits each call that runs `git commit` made, by id: each commit in HEAD's
// reflog (`moves`), oldest first, goes to the newest such call made no later than its second
// with a `git commit` left, so a commit counts once, for a call that can have made it,
// whatever the calls after it.
pub(super) type Credit = HashMap<String, usize>;

pub(super) fn credit(calls: &[ToolUse], moves: &[Move]) -> Credit {
    let commits = |c: &ToolUse| c.command.as_deref().map_or(0, |command| events(command, "").iter().filter(|e| **e == "commit").count());
    let committing: Vec<(&ToolUse, usize)> = calls.iter().map(|c| (c, commits(c))).filter(|(_, n)| *n > 0).collect();
    let mut credit = Credit::new();
    for m in moves.iter().rev().filter(|m| m.why.split_once(": ").is_some_and(|(kind, _)| matches!(kind, "commit" | "commit (amend)" | "commit (initial)"))) {
        let made = committing.iter().rev().find(|(c, n)| c.at.div_euclid(1000) <= m.at && credit.get(&c.id).copied().unwrap_or(0) < *n);
        if let Some((call, _)) = made {
            *credit.entry(call.id.clone()).or_default() += 1;
        }
    }
    credit
}

#[derive(Debug)]
pub(super) struct Progress {
    pub(super) spans: Vec<Span>,
    // The index of the plan's own call, and each step's status as it sent it.
    pub(super) sent: usize,
    as_sent: Vec<&'static str>,
}

// A step and the calls it ran over, by index: from the one it was put in progress at to the
// one that ended it; no start for a step never in progress, and no end for one still open.
#[derive(Debug)]
pub(super) struct Span {
    pub(super) step: PlanStep,
    pub(super) from: Option<usize>,
    pub(super) to: Option<usize>,
}

fn open(status: &str) -> bool {
    matches!(status, "inProgress" | "pending")
}

impl Progress {
    // A plan sent at call `at`, each step keeping its span from the plan before it: one in
    // progress since then keeps its start, and one it completes ends at `at`. A step the plan
    // before it had completed stays so, as the worker never sees the hook's moves and may
    // send one again as open; the first pending one then takes the place in progress.
    fn sent(steps: Vec<PlanStep>, before: Option<&Progress>, at: usize) -> Progress {
        let mut moved = false;
        let as_sent = steps.iter().map(|s| s.status).collect();
        let mut spans: Vec<Span> = steps
            .into_iter()
            .map(|mut step| {
                let was = before.and_then(|p| p.spans.iter().find(|s| s.step.content == step.content));
                let (from, to) = match (step.status, was.map(|s| s.step.status)) {
                    ("inProgress", Some("inProgress")) => (was.and_then(|s| s.from), None),
                    (status, Some("completed")) if open(status) => {
                        moved |= status == "inProgress";
                        step.status = "completed";
                        (was.and_then(|s| s.from), was.and_then(|s| s.to))
                    }
                    ("inProgress", _) => (Some(at), None),
                    ("completed", Some("completed")) => (was.and_then(|s| s.from), was.and_then(|s| s.to)),
                    ("completed", Some("inProgress")) => (was.and_then(|s| s.from), Some(at)),
                    _ => (None, None),
                };
                Span { step, from, to }
            })
            .collect();
        if moved && !spans.iter().any(|s| s.step.status == "inProgress") && let Some(next) = spans.iter_mut().find(|s| s.step.status == "pending") {
            (next.step.status, next.from) = ("inProgress", Some(at));
        }
        Progress { spans, sent: at, as_sent }
    }

    // The event at call `at`: a commit ends the step in progress when it ends on one; the
    // review's verdict or the PR ends the first open step from there that ends on it, and
    // every open one before it, as the PR opens only once that work is done.
    fn on(&mut self, event: &str, at: usize) {
        let Some(now) = self.spans.iter().position(|s| s.step.status == "inProgress").or_else(|| self.spans.iter().position(|s| open(s.step.status))) else { return };
        let ends = match event {
            "commit" => (self.spans[now].step.until == Some(event)).then_some(now),
            _ => self.spans[now..].iter().position(|s| open(s.step.status) && s.step.until == Some(event)).map(|k| now + k),
        };
        let Some(ends) = ends else { return };
        for span in self.spans[now..=ends].iter_mut().filter(|s| open(s.step.status)) {
            (span.step.status, span.to) = ("completed", Some(at));
            span.from = span.from.or(Some(at));
        }
        if let Some(next) = self.spans[ends + 1..].iter_mut().find(|s| s.step.status == "pending") {
            (next.step.status, next.from) = ("inProgress", Some(at));
        }
    }

    // Once its PR is open, or with no step that ends on one: the step in progress is
    // completed and those pending canceled.
    fn finish(&mut self, at: usize) {
        if self.spans.iter().any(|s| open(s.step.status) && s.step.until == Some("pr")) {
            return;
        }
        for span in &mut self.spans {
            match span.step.status {
                "inProgress" => (span.step.status, span.to) = ("completed", Some(at)),
                "pending" => span.step.status = "canceled",
                _ => {}
            }
        }
    }

    // Each step's text and status, as Linear's checklist shows them.
    pub(super) fn shown(&self) -> Vec<(&str, &str)> {
        self.spans.iter().map(|s| (s.step.content.as_str(), s.step.status)).collect()
    }

    // As the worker's own call set the checklist.
    pub(super) fn shown_as_sent(&self) -> Vec<(&str, &str)> {
        self.spans.iter().zip(&self.as_sent).map(|(s, status)| (s.step.content.as_str(), *status)).collect()
    }
}

// The longest a step is shown, in characters, its ellipsis included.
const STEP_CHARS: usize = 100;

// The worker's text on one line with no markdown, no longer than STEP_CHARS; None when
// nothing is left.
pub(super) fn one_line(text: &str) -> Option<String> {
    let words = text.replace(['*', '_', '`', '[', ']', '<', '>', '#', '~', '\\'], " ");
    let content: String = words.split_whitespace().collect::<Vec<_>>().join(" ");
    let content = match content.char_indices().nth(STEP_CHARS - 1).filter(|_| content.chars().count() > STEP_CHARS) {
        Some((cut, _)) => format!("{}…", content[..cut].trim_end()),
        None => content,
    };
    (!content.is_empty()).then_some(content)
}

// What a call says of the phase, most telling last: `git commit && git push` ships.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Mark {
    Edit,
    Commit,
    Check,
    Ship,
}

pub(super) fn mark(tool: Tool, command: Option<&str>) -> Option<Mark> {
    match tool {
        Tool::Edit => Some(Mark::Edit),
        Tool::Shell => {
            let command = command?;
            segments(command).iter().filter_map(|words| segment_mark(words)).chain(script_writes(command).then_some(Mark::Edit)).max()
        }
        _ => None,
    }
}

// A run's mark. One that runs another, as `pwsh -Command <command>` does, is that command's.
fn segment_mark(words: &[String]) -> Option<Mark> {
    let after = |flag: &str| words.iter().position(|w| w == flag).map(|i| &words[i + 1..]).filter(|rest| !rest.is_empty());
    match program(words).as_str() {
        "tolkster" => {
            return match (words.get(1).map(String::as_str), words.get(2).map(String::as_str)) {
                (Some("pr"), Some("create")) => Some(Mark::Ship),
                _ => None,
            };
        }
        "pwsh" | "powershell" => return after("-command").or_else(|| after("-c")).and_then(segment_mark),
        _ => {}
    }
    match segment_kind(words)? {
        "testing" | "building" => Some(Mark::Check),
        "pushing" => Some(Mark::Ship),
        "committing" => Some(Mark::Commit),
        "editing" => Some(Mark::Edit),
        _ => None,
    }
}

// The worker's newest post of its own: its newest run of `tolkster activity <session>
// <kind>` or `tolkster plan <session>` in `tail`, whether or not it succeeded. None, as for a
// review round's new session before its first post, or once the post has left the tail,
// keeps the hook quiet: it never guesses a session, which may have ended.
#[derive(Debug, PartialEq)]
pub struct Report {
    // Only an ID in a plain shape is taken from the command; empty when it gave none.
    pub session: String,
    // The activity's type as the command gave it, lowercased; None for a plan.
    pub kind: Option<String>,
}

pub fn last_report(tail: &str) -> Option<Report> {
    tool_calls(tail).iter().rev().find_map(|call| report(call.command.as_deref()?))
}

// The first run in `command`, as a heredoc's or a pipe's body lines come after it. A run
// whose session isn't plain still counts, with an empty session that posts nothing, so a
// question written another way silences rather than reopens the hook; and a kind of
// `QUIET` anywhere among its words counts, whatever options come before it.
pub(super) fn report(command: &str) -> Option<Report> {
    segments(command).into_iter().find_map(|words| {
        let verb = words.get(1).map(String::as_str).filter(|v| program(&words) == "tolkster" && matches!(*v, "activity" | "plan"))?;
        let session = words.get(2).and_then(|id| Id::parse(id)).map(String::from).unwrap_or_default();
        let quiet = words.iter().skip(2).find(|w| QUIET.contains(&w.as_str()));
        Some(Report { session, kind: (verb == "activity").then(|| quiet.or(words.get(3)).cloned()).flatten() })
    })
}

// The kinds `report` takes from a run: those that end a round, or wait on the owner.
const QUIET: &[&str] = &["response", "elicitation", "error"];

// Whether the worker's newest post leaves room for one: not after an elicitation or an error,
// which wait on the owner or end the run. A response doesn't: one that answers a message
// mid-task is followed by more work, and the hook's next line is then what keeps Linear from
// showing the session as finished. A final one is followed only by the closing command, which
// the hook never posts for (`doing`). Read from the transcript, never from Linear: an error or a
// stop `listen` posts isn't seen, and the worker is being closed then.
pub fn may_post_after(newest: &Report) -> bool {
    !newest.kind.as_deref().is_some_and(|kind| matches!(kind, "elicitation" | "error"))
}
