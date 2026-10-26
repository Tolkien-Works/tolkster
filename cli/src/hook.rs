// A worker's hooks (harness::HOOKS), before each tool call or as the agent's state changes,
// on the calls, ends and states its harness's adapter reads from their input and the
// transcript it points at (harness::events). What a post says is decided here from the
// call's tool kind, command and file, as one of the fixed phrases (voice::tool,
// voice::action), after where the worker is (`whereabouts`), and how a call ended from its result's flags
// and its notice's status alone (`ends`): never the worker's prose, a command's text, a
// tool's output or anything from Linear or GitHub. The exceptions are the worker's own text it
// has posted or will post itself: its plan's steps, and the subject lines of its commits,
// read from git (`committed`).

use crate::git::Move;
use crate::plain::Sha;

// What a call is doing: a kind with a phrase in voice.json's `tools`, and the file it names,
// as the tool gave it, for `plain::relative` to place in the worktree. None for a call that posts
// nothing: a tool with no phrase, or a command that runs `tolkster` or `orca`, as the
// worker's own reporting does.
#[derive(Debug, PartialEq)]
pub struct Doing {
    pub kind: &'static str,
    pub file: Option<String>,
}

pub fn doing(call: &Call) -> Option<Doing> {
    if call.sub_agent {
        return None;
    }
    let file = || call.file.clone();
    let (kind, file) = match call.tool {
        Tool::Edit => ("editing", file()),
        Tool::Read => ("reading", file()),
        Tool::Search => ("searching", None),
        Tool::Shell => (shell::command_kind(call.command.as_deref()?)?, None),
        Tool::SubAgent => ("delegating", None),
        Tool::LookUp => ("looking-up", None),
        Tool::Other => return None,
    };
    Some(Doing { kind, file })
}

// The commit a call made at `made_ms` left, and its subject line, by HEAD's newest reflog
// entries (`git::last_moves`, newest first): the oldest commit, an amend's or a first
// commit's included, among those no older than the second the call was made in, so a
// commit made after it isn't taken for its own. None when the commit failed, as HEAD then
// moved before it or for something else, a merge or a rebase. The subject is the worker's
// own line, the one it would have posted itself.
pub fn committed(moves: &[Move], made_ms: i64) -> Option<(Sha, String)> {
    let since = made_ms.div_euclid(1000);
    moves
        .iter()
        .take_while(|m| m.at >= since)
        .filter_map(|m| {
            let (kind, subject) = m.why.split_once(": ")?;
            let subject = subject.trim();
            (matches!(kind, "commit" | "commit (amend)" | "commit (initial)") && !subject.is_empty()).then(|| (m.sha.clone(), subject.to_string()))
        })
        .last()
}

// The subject lines of the commits made from `since_ms` to `until_ms`, by HEAD's newest
// reflog entries (`git::last_moves`, newest first), oldest first: an amend replaces the
// commit before it.
pub fn commits_since(moves: &[Move], since_ms: i64, until_ms: Option<i64>) -> Vec<String> {
    let since = since_ms.div_euclid(1000);
    let until = until_ms.map_or(i64::MAX, |until| until.div_euclid(1000));
    let mut subjects: Vec<String> = Vec::new();
    for m in moves.iter().take_while(|m| m.at >= since).filter(|m| m.at <= until).collect::<Vec<_>>().into_iter().rev() {
        let Some((kind, subject)) = m.why.split_once(": ") else { continue };
        let subject = subject.trim().to_string();
        match kind {
            _ if subject.is_empty() => {}
            "commit" | "commit (initial)" => subjects.push(subject),
            "commit (amend)" => {
                subjects.pop();
                subjects.push(subject);
            }
            _ => {}
        }
    }
    subjects
}

pub mod narrate;
pub mod redact;
mod shell;
pub mod timing;
mod transcript;
mod whereabouts;
pub mod trail;

pub use crate::harness::events::{BACKGROUND_FOR_MS, Call, Tool, ended, moments, parse_call, parse_output, parse_state, reaches, sub_agents, transcript_session};
pub use shell::{closes_terminal, commits, known, pairing};
#[cfg(test)]
pub use transcript::TICK_FOR;
pub use transcript::{MOST_BYTES, Run, Tick, made_at, moved_on, pr_since, previous, run_of, tail_back, tick};
pub use whereabouts::{CHANGING, CHECKING, Report, SHIPPING, UNDERSTANDING, last_report, may_post_after, report_through, whereabouts};

#[cfg(test)]
mod tests;
