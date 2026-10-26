// The worker's trail: one durable line per plan step it completes, else, while its session
// has no plan, per phase it moves on from (hook::whereabouts' phases, forward only, so a fix
// after a red test opens no new line). Behind the line's ▸, what happened in it: its commits'
// subject lines and the known outcome of each kind of check it ran. Read from the
// transcript's tool calls in the session, from its first post on, never from Linear.
//
// `hook start`, which the next call waits on, puts the lines in at an edge: the first shell
// call or sub-agent after a plan of the session that returned unfailed, so a plan auto mode
// denied or that failed is never shown, or after an event that may move the plan on
// (hook::whereabouts' `progress`); a shell call or sub-agent the phase has risen by since
// the last one; or a response or question that ends a session past its changes. Before the
// lines, it posts the plan as the events moved it, when Linear's checklist lacks that. Each
// edge puts in every line so far that Linear lacks, which `post_once` makes once each, so
// one an edge missed, a failed post or a hook cut off goes in at the next.

use std::collections::HashMap;
use std::iter::once;

use super::shell::events;
use super::transcript::{End, ToolUse, ends, known_outcome, tool_calls};
use super::whereabouts::{Mark, Progress, Span, credit, mark, one_line, plan_run, progress, report};
use super::{CHANGING, CHECKING, Call, Tool, Report, SHIPPING, UNDERSTANDING, may_post_after};
use crate::git::Move;
use crate::linear::{ActivityContent, ActivityInput, PlanStep};
use crate::voice;

// What a line stands for: a step's text, on one line, or a key of voice.json's `phases`.
#[derive(Debug, PartialEq)]
pub enum Done {
    Step(String),
    Phase(&'static str),
}

#[derive(Debug, PartialEq)]
pub struct Line {
    pub done: Done,
    // When what it stands for began and ended, for the commits made between; None for a step
    // never in progress, which holds no calls, and no end for the phase the session is in.
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    // The newest known outcome (hook::known) of each kind of check its calls ran, by kind in
    // the order first run: whether it failed.
    pub outcomes: Vec<(&'static str, bool)>,
}

impl Line {
    // What makes it once in its session: a step completed again, or a phase reached again,
    // adds no line.
    pub fn key(&self) -> &str {
        match &self.done {
            Done::Step(step) => step,
            Done::Phase(phase) => phase,
        }
    }
}

// Whether `hook start` runs before a call of the tool, as harness::HOOKS matches them.
fn started(tool: Tool) -> bool {
    matches!(tool, Tool::Shell | Tool::SubAgent)
}

// A line's action, durable: what it stands for, then its commits and outcomes in a few words
// ("2 commits · tests passed"), and as a list behind ▸, each commit's subject on one line with
// no markdown. None for a phase voice.json doesn't have.
pub fn activity(session: &str, line: &Line, commits: &[String]) -> Option<ActivityInput> {
    let commits: Vec<String> = commits.iter().filter_map(|subject| one_line(subject)).collect();
    let action = match &line.done {
        Done::Step(step) => step.clone(),
        Done::Phase(phase) => voice::head(&voice::At::Phase(phase))?,
    };
    let checks: Vec<String> = line.outcomes.iter().map(|(kind, failed)| format!("{} {}", voice::checked(kind), voice::outcome(kind, Some(*failed)))).collect();
    let count = match commits.len() {
        0 => None,
        1 => Some("1 commit".to_string()),
        n => Some(format!("{n} commits")),
    };
    let said: Vec<String> = count.into_iter().chain(checks.iter().cloned()).collect();
    let list: Vec<String> = commits.iter().chain(&checks).map(|item| format!("- {item}")).collect();
    Some(ActivityInput {
        agent_session_id: session.into(),
        content: ActivityContent::Action {
            action,
            parameter: if said.is_empty() { "done".into() } else { said.join(" · ") },
            result: (!list.is_empty()).then(|| list.join("\n")),
        },
        ephemeral: false,
        id: None,
        options: vec![],
    })
}

// The session `call` may be an edge in, by the tail as first read: cheap, so most calls end
// here. It may say so of a call that isn't one, as the tail may not reach the session's
// start, but never misses one that is, but for a first edit after a check or push before
// any, whose line the next edge puts in.
pub fn edge(tail: &str, call: &Call) -> Option<String> {
    let calls = tool_calls(tail);
    let upto = before(&calls, &call.id);
    let session = open_session(upto)?;
    let ended = ends(tail);
    let upto = in_session(upto, &session)?;
    let since = last_hooked(upto, &session).unwrap_or(0);
    let event = |c: &ToolUse| matches!(ended.get(&c.id), Some(End::Returned(_))) && c.command.as_deref().is_some_and(|command| !events(command, &session).is_empty());
    let stepped = upto[since..].iter().any(|c| (returned(c, &ended) && sent_plan(c, &session)) || event(c));
    let marks: Vec<Option<Mark>> = upto.iter().map(|c| mark(c.tool, c.command.as_deref())).collect();
    let last = upto.iter().rposition(|c| started(c.tool)).map_or(0, |j| j + 1);
    let fold = |marks: &mut dyn Iterator<Item = Option<Mark>>| marks.fold(UNDERSTANDING, |phase, m| rise(phase, m, false));
    let rose = fold(&mut marks.iter().copied().chain(once(mark(call.tool, call.command.as_deref())))) != fold(&mut marks[..last].iter().copied());
    (stepped || rose || closes(call, &session)).then_some(session)
}

// Whether `tail` holds `session`'s first post: one of another session's comes before it.
pub fn holds_start(tail: &str, session: &str) -> bool {
    let reports: Vec<Report> = tool_calls(tail).iter().filter_map(|c| report(c.command.as_deref()?)).collect();
    reports.iter().position(|r| r.session == session).is_some_and(|first| reports[..first].iter().any(|r| !r.session.is_empty()))
}

// What the edge at a call puts in, in its session: the plan as the hook moved it on, when it
// differs from what Linear's checklist has, then the trail's lines.
#[derive(Debug, Default, PartialEq)]
pub struct Edge {
    pub plan: Option<Vec<PlanStep>>,
    pub lines: Vec<Line>,
}

// What the edge at `call` puts in, in `session`. With a plan (`progress`), from plans that
// returned unfailed and the commits in HEAD's reflog (`moves`), finished when the call is
// the session's response: the plan when it moved since Linear's checklist was last set, by
// the worker's plan or by an edge, and every step completed so far when one is completed
// that wasn't at the last edge, or before the plan sent since. The response's edge puts in
// both whatever moved, so a post an earlier edge lost goes in there. Else, with no plan in the session, every phase passed, and the one it
// is in when the call ends the session past its changes. Read from the session's first post
// in `tail` on.
pub fn lines(tail: &str, call: &Call, session: &str, moves: Option<&[Move]>) -> Edge {
    let calls = tool_calls(tail);
    let Some(calls) = in_session(before(&calls, &call.id), session) else { return Edge::default() };
    let ended = ends(tail);
    let counts = |c: &ToolUse| returned(c, &ended);
    let closing = finishes(call, session);
    let credit = moves.map(|moves| credit(calls, moves));
    let credit = credit.as_ref();
    if let Some(now) = progress(calls, &ended, credit, session, counts, closing) {
        // The last edge's, or for a plan sent since, the one before it; and what the checklist
        // shows: the worker's plan as it sent it, or as the last edge moved it.
        let hooked = last_hooked(calls, session).unwrap_or(0);
        let then = progress(&calls[..hooked.max(now.sent)], &ended, credit, session, counts, false);
        let shown = if hooked <= now.sent { Some(now.shown_as_sent()) } else { then.as_ref().map(Progress::shown) };
        let plan = (closing || shown != Some(now.shown())).then(|| now.spans.iter().map(|s| s.step.clone()).collect());
        let completed = || now.spans.iter().filter(|s| s.step.status == "completed");
        let completes = closing || completed().any(|s| then.as_ref().is_none_or(|then| status(then, &s.step.content) != Some("completed")));
        let lines = if completes { completed().filter_map(|s| step_line(s, calls, &ended)).collect() } else { Vec::new() };
        return Edge { plan, lines };
    }
    let planned = call.command.as_deref().and_then(plan_run).is_some_and(|(of, _)| of == session);
    if planned || calls.iter().any(|c| counts(c) && sent_plan(c, session)) {
        return Edge::default();
    }
    Edge { plan: None, lines: phase_lines(calls, call, session, &ended) }
}

fn phase_lines(calls: &[ToolUse], call: &Call, session: &str, ended: &HashMap<String, End>) -> Vec<Line> {
    // The phases entered, each by the index of the call that entered it; the call itself is
    // calls.len().
    let mut entered = vec![(UNDERSTANDING, 0)];
    let marks = calls.iter().map(|c| mark(c.tool, c.command.as_deref())).chain(once(mark(call.tool, call.command.as_deref())));
    for (i, m) in marks.enumerate() {
        let phase = entered[entered.len() - 1].0;
        let next = rise(phase, m, true);
        if next != phase {
            entered.push((next, i));
        }
    }
    let last = calls.iter().rposition(|c| started(c.tool));
    let then = last.map_or(UNDERSTANDING, |j| entered.iter().rev().find(|(_, at)| *at <= j).map_or(UNDERSTANDING, |e| e.0));
    let now = entered[entered.len() - 1].0;
    let closing = closes(call, session) && now == SHIPPING;
    if now == then && !closing {
        return Vec::new();
    }
    let passed = if closing { entered.len() } else { entered.len() - 1 };
    (0..passed)
        .map(|n| {
            let (phase, at) = entered[n];
            let end = entered.get(n + 1).map_or(calls.len(), |e| e.1).min(calls.len());
            Line { done: Done::Phase(phase), since_ms: calls.get(at).map(|c| c.at), until_ms: entered.get(n + 1).and_then(|e| calls.get(e.1)).map(|c| c.at), outcomes: outcomes(&calls[at.min(end)..end], ended) }
        })
        .collect()
}

// A completed step's line, over the calls from the one that put it in progress to the one
// that ended it.
fn step_line(span: &Span, calls: &[ToolUse], ended: &HashMap<String, End>) -> Option<Line> {
    let done = Done::Step(one_line(&span.step.content)?);
    let Some(from) = span.from else { return Some(Line { done, since_ms: None, until_ms: None, outcomes: Vec::new() }) };
    let to = span.to.unwrap_or(calls.len()).min(calls.len());
    let at = |i: usize| calls.get(i).map(|c| c.at);
    Some(Line { done, since_ms: at(from), until_ms: span.to.and_then(at), outcomes: outcomes(&calls[from.min(to)..to], ended) })
}

// The session the newest post before the call names, unless its newest activity there is a
// question or an error, or one whose session isn't plain: a plan sent after one doesn't open
// it again.
fn open_session(calls: &[ToolUse]) -> Option<String> {
    let reports: Vec<Report> = calls.iter().rev().filter_map(|c| report(c.command.as_deref()?)).collect();
    let newest = reports.first().filter(|r| !r.session.is_empty())?;
    reports.iter().filter(|r| r.session == newest.session || r.session.is_empty()).find(|r| r.kind.is_some()).is_none_or(may_post_after).then(|| newest.session.clone())
}

// The last call `hook start` ran before, a plan of the session's aside: what it and the
// calls after it did, a plan sent among them or an event, is the edge's.
fn last_hooked(calls: &[ToolUse], session: &str) -> Option<usize> {
    calls.iter().rposition(|c| started(c.tool) && !sent_plan(c, session))
}

fn sent_plan(call: &ToolUse, session: &str) -> bool {
    call.command.as_deref().and_then(plan_run).is_some_and(|(of, _)| of == session)
}

fn returned(call: &ToolUse, ended: &HashMap<String, End>) -> bool {
    matches!(ended.get(&call.id), Some(End::Returned(false)))
}

// Whether the call is the worker's response or question in `session`.
fn closes(call: &Call, session: &str) -> bool {
    call.command.as_deref().and_then(report).is_some_and(|r| r.session == session && matches!(r.kind.as_deref(), Some("response" | "elicitation")))
}

// Whether the call is the worker's response in `session`, which finishes its plan.
fn finishes(call: &Call, session: &str) -> bool {
    call.command.as_deref().and_then(report).is_some_and(|r| r.session == session && r.kind.as_deref() == Some("response"))
}

fn status<'a>(plan: &'a Progress, content: &str) -> Option<&'a str> {
    plan.spans.iter().find(|s| s.step.content == content).map(|s| s.step.status)
}

// The phases in order: a session only moves forward through them.
const PHASES: [&str; 4] = [UNDERSTANDING, CHANGING, CHECKING, SHIPPING];

// The phase after a call's mark. Before anything changed, a check or a push, as of the empty
// branch at the start, is still getting to know the task, unless `strict` is off, as when
// the tail may have lost the edits before it.
fn rise(phase: &'static str, mark: Option<Mark>, strict: bool) -> &'static str {
    let early = strict && phase == UNDERSTANDING;
    let to = match mark {
        Some(Mark::Edit | Mark::Commit) => CHANGING,
        Some(Mark::Check) if !early => CHECKING,
        Some(Mark::Ship) if !early => SHIPPING,
        _ => phase,
    };
    let rank = |p: &str| PHASES.iter().position(|q| *q == p);
    if rank(to) > rank(phase) { to } else { phase }
}

// The calls before `id`, or all of them when the tail doesn't hold it yet.
fn before<'a>(calls: &'a [ToolUse], id: &str) -> &'a [ToolUse] {
    calls.iter().position(|c| c.id == id).map_or(calls, |this| &calls[..this])
}

// The calls from the session's first post in them on.
fn in_session<'a>(calls: &'a [ToolUse], session: &str) -> Option<&'a [ToolUse]> {
    let start = calls.iter().position(|c| c.command.as_deref().and_then(report).is_some_and(|r| r.session == session))?;
    Some(&calls[start..])
}

fn outcomes(calls: &[ToolUse], ended: &HashMap<String, End>) -> Vec<(&'static str, bool)> {
    let mut outcomes: Vec<(&'static str, bool)> = Vec::new();
    for (kind, failed) in calls.iter().filter_map(|c| known_outcome(c, ended)) {
        match outcomes.iter_mut().find(|(k, _)| *k == kind) {
            Some(newest) => newest.1 = failed,
            None => outcomes.push((kind, failed)),
        }
    }
    outcomes
}

#[cfg(test)]
mod tests;
