// How the transcript's tool calls stand, read through the harness's adapter (harness::events).

use std::collections::HashMap;
use std::time::Duration;

use super::shell::{known, pair_of, program, segments};
pub(super) use crate::harness::events::{End, ToolUse, ends, tool_calls};

// How much of the transcript's end is read for the tool calls before this one, at first and
// at most: a run of big reads can push the worker's newest post far back (`tail_back`).
pub const TAIL_BYTES: u64 = 256 * 1024;
pub const MOST_BYTES: u64 = 16 * 1024 * 1024;

// The end of a transcript `len` bytes long, read by `read(from)`: TAIL_BYTES, doubling until
// it `holds` what is wanted, such as the worker's newest post of its own (`last_report`), or
// reaches the transcript's start or MOST_BYTES. Whether it is the whole transcript beside it.
pub fn tail_back(len: u64, mut read: impl FnMut(u64) -> Option<String>, holds: impl Fn(&str) -> bool) -> Option<(String, bool)> {
    let mut size = TAIL_BYTES;
    loop {
        let from = len.saturating_sub(size);
        let tail = read(from)?;
        if from == 0 || size >= MOST_BYTES || holds(&tail) {
            return Some((tail, from == 0));
        }
        size *= 2;
    }
}

// How a call stands by the transcript's tail.
#[derive(Debug, PartialEq)]
pub enum Run {
    // Neither the call nor its end is in the tail: not written yet, or scrolled out.
    Gone,
    Running,
    // Its result is in, and the hook after the call posts its outcome.
    Returned,
    // It went to the background and its notice has come: whether it failed, None when it
    // was stopped rather than finished.
    Finished(Option<bool>),
}

pub fn run_of(tail: &str, id: &str) -> Run {
    match ends(tail).get(id) {
        Some(End::Notice(failed)) => Run::Finished(*failed),
        Some(End::Returned(_)) => Run::Returned,
        Some(End::Launched) => Run::Running,
        None if tool_calls(tail).iter().any(|c| c.id == id) => Run::Running,
        None => Run::Gone,
    }
}

fn outcome(end: &End) -> Option<bool> {
    match end {
        End::Returned(failed) => Some(*failed),
        End::Notice(failed) => *failed,
        End::Launched => None,
    }
}

// A call's kind and its known outcome (`known`): whether it failed.
pub(super) fn known_outcome(call: &ToolUse, ends: &HashMap<String, End>) -> Option<(&'static str, bool)> {
    let kind = pair_of(call.tool, call.command.as_deref())?;
    let failed = known(call.tool, call.command.as_deref()).then(|| ends.get(&call.id).and_then(outcome)).flatten()?;
    Some((kind, failed))
}

// How often the ticker reads the transcript, how often it says the call still runs, and for
// how long at most: harness::HOOKS gives it a little longer.
pub const POLL: Duration = Duration::from_secs(10);
pub const TICK_EVERY: Duration = Duration::from_secs(180);
pub const TICK_FOR: Duration = Duration::from_secs(115 * 60);

#[derive(Debug, PartialEq)]
pub enum Tick {
    // The call has run this many minutes.
    Minutes(u64),
    // A call that went to the background has ended, failed or not.
    Ended(bool),
}

// Ticks the call `id` until it ends: reads the transcript (`read`) each POLL, says each
// TICK_EVERY that the call still runs, and, for one that went to the background, how it
// ended, with the tail it read. It stops once the call's result is in, as the hook after the
// call posts its outcome, at a call it can't find by the first tick or a transcript it can't
// read, once `say` answers false, and after TICK_FOR.
pub fn tick(id: &str, mut read: impl FnMut() -> Option<String>, mut wait: impl FnMut(Duration), mut say: impl FnMut(&str, Tick) -> bool) {
    let (mut ran, mut next) = (Duration::ZERO, TICK_EVERY);
    while ran < TICK_FOR {
        wait(POLL);
        ran += POLL;
        let Some(tail) = read() else { return };
        match run_of(&tail, id) {
            Run::Returned | Run::Finished(None) => return,
            Run::Finished(Some(failed)) => {
                say(&tail, Tick::Ended(failed));
                return;
            }
            Run::Gone if ran >= TICK_EVERY => return,
            Run::Running if ran >= next => {
                if !say(&tail, Tick::Minutes(ran.as_secs() / 60)) {
                    return;
                }
                next += TICK_EVERY;
            }
            Run::Gone | Run::Running => {}
        }
    }
}

// When the call `id` was made, by the tool calls in `tail`, the transcript's end. None for a
// call it doesn't hold.
pub fn made_at(tail: &str, id: &str) -> Option<i64> {
    tool_calls(tail).into_iter().find(|c| c.id == id).map(|c| c.at)
}

// The tool call before `id` in `tail`, or the newest when the tail doesn't hold `id` yet, once
// its result is in: the worker then made `id` after the call's hook had run, which a call
// made in the same message as it didn't.
pub fn previous(tail: &str, id: &str) -> Option<ToolUse> {
    let calls = tool_calls(tail);
    let this = calls.iter().position(|c| c.id == id).unwrap_or(calls.len());
    let before = calls.into_iter().take(this).next_back()?;
    ends(tail).contains_key(&before.id).then_some(before)
}

// Whether a call after `id` in `tail`, or any when the tail doesn't hold `id`, runs `tolkster
// pr`, which may have set the worktree card to the PR's checkpoint: an action's card then
// stays as it is.
pub fn pr_since(tail: &str, id: &str) -> bool {
    let calls = tool_calls(tail);
    let after = calls.iter().position(|c| c.id == id).map_or(0, |this| this + 1);
    calls[after..].iter().any(|call| call.command.as_deref().is_some_and(|command| segments(command).iter().any(|words| program(words) == "tolkster" && words.get(1).is_some_and(|verb| verb == "pr"))))
}

// Whether `tail` holds a tool call after `id`: the worker has moved on, and may have posted
// since its session was read, so a post now could land after its question or its error.
// After it in the transcript's order, as calls made in one message share their time and may
// all be written before the first runs; for a call not yet written, any made after
// `since_ms`.
pub fn moved_on(tail: &str, id: &str, since_ms: i64) -> bool {
    let calls = tool_calls(tail);
    match calls.iter().position(|c| c.id == id) {
        Some(this) => this + 1 < calls.len(),
        None => calls.iter().any(|c| c.at > since_ms),
    }
}
