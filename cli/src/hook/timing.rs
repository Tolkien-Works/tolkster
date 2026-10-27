// A worker session's timing (docs/timing.md), summarised from its transcript and its
// sub-agents' since its launch: only the times of the moments the event adapter reads
// (harness::events::moments), and each call's tool kind and what a shell command runs, never
// its text. What the summary holds is timing::Summary's.

use std::collections::{HashMap, HashSet};

use crate::harness::events::{End, Moment};
use crate::plain::Sha;
use crate::time::iso;
use crate::timing::{By, CheckKind, CheckRun, Kind, MOST_CHECKS, Offsets, Summary, Tools};

use super::shell::{check_of, command_kind, runs, runs_program, runs_words};
use super::{BACKGROUND_FOR_MS, Tool};

// A gap this long between two moments, with no call running across it, is idle: the agent
// waited for input.
pub const IDLE_MS: i64 = 4 * 60 * 1000;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Spent {
    Read,
    Edit,
    Check,
    Git,
    Bookkeeping,
    Agent,
    Other,
}

fn spent(tool: Tool, command: Option<&str>) -> Spent {
    match tool {
        Tool::Read | Tool::Search | Tool::LookUp => Spent::Read,
        Tool::Edit => Spent::Edit,
        Tool::SubAgent => Spent::Agent,
        Tool::Other => Spent::Other,
        Tool::Shell => {
            let command = command.unwrap_or_default();
            if runs_program(command, &["tolkster", "orca"]) {
                return Spent::Bookkeeping;
            }
            if check_of(command).is_some() {
                return Spent::Check;
            }
            match command_kind(command) {
                Some("testing" | "building") => Spent::Check,
                Some("committing" | "pushing" | "rebasing" | "history") => Spent::Git,
                Some("editing") => Spent::Edit,
                Some("reading" | "searching") => Spent::Read,
                _ => Spent::Other,
            }
        }
    }
}

// One call, from its start to its end: its result's, or a background call's notice's. None for
// one with neither yet, or that went to the background and has no notice.
struct Run {
    start: i64,
    end: Option<i64>,
    // For idle gaps: the end, else as long as a background call may run unseen, else running.
    holds_to: i64,
    spent: Spent,
    check: Option<(CheckKind, bool)>,
    failed: Option<bool>,
    command: Option<String>,
}

fn calls_of(moments: &[(i64, Moment)], last: i64) -> Vec<Run> {
    let mut ends: HashMap<&str, (i64, Option<bool>, bool)> = HashMap::new();
    for (at, moment) in moments {
        if let Moment::Ended(id, end) = moment {
            match end {
                End::Notice(failed) => {
                    ends.insert(id, (*at, *failed, false));
                }
                End::Returned(failed) => {
                    ends.entry(id).or_insert((*at, Some(*failed), false));
                }
                End::Launched => {
                    ends.entry(id).or_insert((*at, None, true));
                }
            }
        }
    }
    moments
        .iter()
        .filter_map(|(at, moment)| match moment {
            Moment::Call(call) => Some((*at, call)),
            _ => None,
        })
        .map(|(start, call)| {
            let ended = ends.get(call.id.as_str());
            let end = ended.filter(|(_, _, launched)| !launched).map(|(end, _, _)| *end);
            let holds_to = end.unwrap_or(if ended.is_some() { (start + BACKGROUND_FOR_MS).min(last) } else { last });
            let command = call.command.clone();
            Run { start, end, holds_to, spent: spent(call.tool, command.as_deref()), check: command.as_deref().filter(|_| call.tool == Tool::Shell).and_then(check_of), failed: ended.and_then(|(_, failed, _)| *failed), command }
        })
        .collect()
}

fn seconds(ms: i64) -> u64 {
    u64::try_from((ms + 500) / 1000).unwrap_or(0)
}

// The session `session`, launched as `launch` says, from its moments and each sub-agent's;
// None when nothing happened since the launch.
pub fn summary(session: &str, (kind, launched, build): (Kind, i64, Option<Sha>), main: Vec<(i64, Moment)>, sub_agents: Vec<Vec<(i64, Moment)>>) -> Option<Summary> {
    let since = |moments: Vec<(i64, Moment)>| -> Vec<(i64, Moment)> {
        let mut moments: Vec<(i64, Moment)> = moments.into_iter().filter(|(at, _)| *at >= launched).collect();
        moments.sort_by_key(|(at, _)| *at);
        moments
    };
    let main = since(main);
    let (start, last) = (main.first()?.0, main.last()?.0);
    let calls = calls_of(&main, last);

    let idle_before = |i: usize| -> bool {
        let (from, to) = (main[i - 1].0, main[i].0);
        to - from >= IDLE_MS && !calls.iter().any(|run| run.start <= from && run.holds_to > from)
    };
    let (mut idle, mut model) = (0, 0);
    let (mut replies, mut tool_replies, mut reply): (HashSet<&str>, HashSet<&str>, Option<&str>) = (HashSet::new(), HashSet::new(), None);
    for i in 0..main.len() {
        let gap = if i == 0 { 0 } else { main[i].0 - main[i - 1].0 };
        let idles = i > 0 && idle_before(i);
        if idles {
            idle += gap;
        }
        match &main[i].1 {
            Moment::Reply(id) => {
                if !idles {
                    model += gap;
                }
                replies.insert(id);
                reply = Some(id);
            }
            Moment::Call(_) => tool_replies.extend(reply),
            _ => {}
        }
    }

    let mut tools = Tools::default();
    for run in &calls {
        let took = seconds(run.end.map_or(0, |end| end - run.start));
        *match run.spent {
            Spent::Read => &mut tools.read,
            Spent::Edit => &mut tools.edit,
            Spent::Check => &mut tools.check,
            Spent::Git => &mut tools.git,
            Spent::Bookkeeping => &mut tools.bookkeeping,
            Spent::Agent => &mut tools.agent,
            Spent::Other => &mut tools.other,
        } += took;
    }

    let offset = |at: i64| seconds(at - start);
    let first = |is: &dyn Fn(&Run) -> bool| calls.iter().find(|run| is(run)).map(|run| run.start);
    let shell = |run: &Run, words: &[&str]| run.command.as_deref().is_some_and(|c| runs_words(c, "tolkster", words));
    let review_start = first(&|run| shell(run, &["pr", "review-pass"]));
    let pr = first(&|run| shell(run, &["pr", "create"]));
    let review_end = review_start.and_then(|from| calls.iter().filter(|run| run.spent == Spent::Agent && run.start >= from && pr.is_none_or(|pr| run.start < pr)).filter_map(|run| run.end).max());
    let offsets = Offsets {
        first_edit: first(&|run| run.spent == Spent::Edit).map(offset),
        first_commit: first(&|run| run.command.as_deref().is_some_and(|c| runs(c, &["committing"]))).map(offset),
        first_gate: first(&|run| matches!(run.check, Some((CheckKind::Gate, _)))).map(offset),
        review_start: review_start.map(offset),
        review_end: review_end.map(offset),
        pr: pr.map(offset),
    };

    // Every check run and edit, the main session's and its sub-agents', in order.
    let mut all: Vec<(Run, By)> = calls.into_iter().map(|run| (run, By::Main)).collect();
    for moments in sub_agents.into_iter().map(since) {
        let end = moments.last().map_or(last, |(at, _)| *at);
        all.extend(calls_of(&moments, end).into_iter().map(|run| (run, By::SubAgent)));
    }
    all.sort_by_key(|(run, _)| run.start);
    let (mut checks, mut edited) = (Vec::new(), false);
    for (run, by) in &all {
        if run.spent == Spent::Edit {
            edited = true;
        }
        let Some((kind, told)) = run.check else { continue };
        if checks.len() < MOST_CHECKS {
            checks.push(CheckRun { kind, failed: run.failed.filter(|_| told), edited, seconds: seconds(run.end.map_or(0, |end| end - run.start)), at: offset(run.start), by: *by });
        }
        edited = false;
    }

    Some(Summary {
        session: session.to_string(),
        launched: iso(launched),
        kind,
        build: build.map(String::from),
        at: iso(start),
        wall: seconds(last - start),
        active: seconds(last - start - idle),
        model: seconds(model),
        tools,
        api_turns: replies.len() as u64,
        tool_turns: tool_replies.len() as u64,
        offsets,
        checks,
    })
}

#[cfg(test)]
mod tests;
