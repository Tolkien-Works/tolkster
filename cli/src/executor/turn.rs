// A conversation's turn, as the machine decided it (executor.rs): carried out in the repo's
// shared checkout (converse.rs), with no agent in the way.

use serde_json::json;

use crate::brief::TurnBrief;
use crate::cli::write_file;
use crate::config::Config;
use crate::converse::{self, Turn};
use crate::error::Error;
use crate::harness;
use crate::machine::{Command, Finished};
use crate::orca::{Terminal, find_repo, open_terminals};
use crate::plain::{Id, IssueKey, RepoName, Time};
use crate::ports::Ports;
use crate::request::{Issue, Session};
use crate::state::Conversation;
use crate::time::{iso, now_ms};
use crate::trace::{self, Undone, Work};
use crate::voice;

use super::{issue_of, pick_up, said_if_failed, session_on_issue};

// A conversation's turn (`startTurn` on a session) in the repo's shared checkout, under the
// checkouts' lock, which a stop takes too: one already open is this command's run before,
// or the turn the machine counts as running, and is answered with. The agent session is
// recorded before the terminal opens, so every later turn resumes it however this one ends,
// and the terminal's handle once open, so a stop has exactly one terminal to close. `after`
// is the newest request a turn or worker was shown, which the turn reads the owner's words after.
// A session's turn open in the checkout, a warm one or one the machine no longer names, is
// handed `request` (`converse::handover`), which it takes once, so a resend hands it nothing new.
pub fn turn(config: &Config, ports: &mut Ports, command: &Command, session_id: &Id, request: Option<&Time>, after: Option<&Time>) -> Result<Finished, Error> {
    let session = session_on_issue(ports, command, session_id, true)?;
    let issue = issue_of(&session)?;
    let done = turn_in(config, ports, command, session_id, &session, issue, request, after);
    said_if_failed(ports, command, session_id, voice::COULDNT_START, json!({}), done)
}

#[allow(clippy::too_many_arguments)]
fn turn_in(config: &Config, ports: &mut Ports, command: &Command, session_id: &Id, session: &Session, issue: &Issue, request: Option<&Time>, after: Option<&Time>) -> Result<Finished, Error> {
    let Some((repo_id, repo)) = find_repo(&ports.orca.repos(None)?, config.repos.repo_for(&issue.place)) else {
        trace::event(Work::NoRepo { issue: command.identifier.clone() });
        return Ok(Finished::failed());
    };
    let _checkouts = ports.shared.checkouts.lock();
    let terminals = ports.orca.terminals()?;
    // Only a terminal in the conversations checkout is the turn's: a handle Orca gave again
    // elsewhere is never typed into.
    let checkout = converse::recorded(ports.orca, ports.records, &repo_id)?;
    let open = checkout.as_ref().map(|c| open_terminals(&terminals, Some(&c.id))).unwrap_or_default();
    let recorded = session.record.conversation.as_ref().and_then(|c| c.terminal.clone());
    // One listed elsewhere, or with no worktree, still runs: it is answered with, as a turn
    // open, and nothing is typed into it or opened beside it.
    if let Some(terminal) = recorded.as_ref().filter(|t| !open.contains(t.as_str()) && open_terminals(&terminals, None).contains(t.as_str())) {
        return Ok(Finished::ok(json!({ "terminal": terminal })));
    }
    if let Some(terminal) = recorded.filter(|t| open.contains(t.as_str())) {
        let (Some(request), Some(checkout)) = (request, checkout.as_ref()) else { return Ok(Finished::ok(json!({ "terminal": terminal }))) };
        let line = converse::handover(request, after.map_or(converse::EVERY_MESSAGE, |t| t.as_str()), converse::behind(&ports.shared.checkouts, ports.orca.shell(), checkout))?;
        let handle = crate::plain::Terminal::check(&terminal)?;
        // A line that may not have reached the turn is never typed again: the turn is closed,
        // and a new one opens in its place, but never beside one still open.
        match ports.orca.type_line(&handle, &line) {
            Ok(()) => {
                // The turn has the request: a pickup that couldn't be posted fails nothing.
                if let Err(error) = pick_up(ports, command, session_id, Some("your reply"), &voice::say(voice::READING_REPLY)) {
                    trace::event(Work::NotDone { what: Undone::Pickup, error });
                }
                return Ok(Finished::ok(json!({ "terminal": terminal })));
            }
            Err(error) => {
                trace::event(Work::HandoverFailed { thread: None, error });
                if let Err(closing) = ports.orca.close(&handle) {
                    trace::event(Work::NotDone { what: Undone::TurnClosed, error: closing });
                    return Ok(Finished::failed());
                }
            }
        }
    }
    let before = session.record.conversation.clone();
    let (what, wording) = if before.is_none() { ("your message", voice::say(voice::LOOKING)) } else { ("your reply", voice::say(voice::READING_REPLY)) };
    pick_up(ports, command, session_id, Some(what), &wording)?;
    let handle = open_turn(config, ports, session_id, &issue.identifier, &repo, &repo_id, request, after, before.as_ref(), &terminals)?;
    Ok(Finished::ok(json!({ "terminal": handle })))
}

// Opens the turn's terminal, its every error the caller's to report. `terminals` are those
// listed under the lock, when the turn was decided.
#[allow(clippy::too_many_arguments)]
fn open_turn(config: &Config, ports: &mut Ports, session_id: &Id, identifier: &str, repo: &str, repo_id: &str, request: Option<&Time>, after: Option<&Time>, before: Option<&Conversation>, terminals: &[Terminal]) -> Result<String, Error> {
    let (identifier, repo) = (&IssueKey::check(identifier)?, &RepoName::check(repo)?);
    let brief = TurnBrief { identifier, session_id, repo, owner_name: &config.owner.name };
    let path = converse::brief_path(&config.briefs_dir, repo, identifier, session_id);
    let agent_session = converse::turn_id(&harness::turn::session_id(before.map(|c| c.agent_session.as_str())))?;
    write_file(&path, &brief.render())?;
    // Never older than what the record holds, or the write would be ignored.
    let at = before.map(|c| c.at.clone()).into_iter().chain([iso(now_ms())]).max().unwrap_or_default();
    let turn = |stale| Turn { session_id, agent_session: &agent_session, request, messages_after: after, brief: &path, stale };
    // Checked before anything is recorded.
    turn(false).command()?;
    ports.records.converse(session_id, &Conversation { agent_session: agent_session.to_string(), terminal: None, at: at.clone() })?;
    let (handle, behind) = converse::open_in_checkout(ports, repo_id, terminals, identifier, |stale| turn(stale).command())?;
    if let Some(error) = behind {
        trace::event(Work::Behind { issue: identifier.clone(), error });
    }
    // A turn nothing can find, to stop it or to know it runs, isn't left running.
    if let Err(error) = ports.records.converse(session_id, &Conversation { agent_session: agent_session.into(), terminal: Some(handle.clone()), at }) {
        if let Err(closing) = crate::plain::Terminal::check(&handle).and_then(|plain| ports.orca.close(&plain)) {
            return Err(error.and_then_failed(&format!("and closing the turn's terminal {handle} failed: {closing}")));
        }
        return Err(error);
    }
    Ok(handle)
}
