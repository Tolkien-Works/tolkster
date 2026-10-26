// The read orders: an issue's facts for the machine (`readIssue`) and a comment's
// (`readComment`), as IDs, enums and times, never text.

use serde_json::{Value, json};

use crate::linear::StateType;
use crate::error::Error;
use crate::machine::{Command, Finished};
use crate::orca::find_repo;
use crate::plain::Id;
use crate::ports::{CommentKind, IssueFacts, Ports};
use crate::request::Issue;
use crate::config::Config;

// The facts `readIssue` answers with (docs/state-machine.md, Commands), as the machine's
// `IssueRead` has them (src/worker/machine/types.ts), at the issue's `updatedAt`: IDs, enums
// and times, never its text.
pub(super) fn read_issue(config: &Config, ports: &mut Ports, command: &Command) -> Result<Finished, Error> {
    let IssueFacts { issue, updated_at, sessions, viewer_id } = ports.linear.issue(&command.identifier)?;
    if issue.id != command.issue_id {
        return Err(Error::Failed(format!("{}: read as another issue", command.identifier)));
    }
    let repo = find_repo(&ports.orca.repos(None)?, config.repos.repo_for(&issue.place)).map(|(id, _)| id);
    // Tolkster's record of its chain's delegation under the parent it has now, wanted only for a
    // sub-issue of Tolkster's that no history entry read says who delegated.
    let recorded = match &issue.parent {
        Some(parent) if issue.delegated.is_none() && issue.delegate_id.as_deref() == Some(viewer_id.as_str()) => {
            ports.records.delegations(&[Id::check(&issue.id)?])?.remove(&issue.id).filter(|d| d.parent_issue_id == parent.id).map(|d| d.at)
        }
        _ => None,
    };
    let read = issue_read(&issue, &updated_at, &Ids { viewer: &viewer_id, owner: &config.owner.id }, repo, &sessions, recorded.as_deref()).map_err(|why| Error::Failed(format!("{}: {why}", command.identifier)))?;
    Ok(Finished::ok(json!({ "read": read })))
}

pub(super) fn read_comment(ports: &mut Ports, command: &Command, comment_id: &Id) -> Result<Finished, Error> {
    // A gone comment's issue isn't read: the machine reads its state only for a plain one.
    let Some(comment) = ports.linear.comment(comment_id)? else {
        return Ok(Finished::ok(json!({ "comment": { "kind": "gone", "newestOwnerAt": null, "state": "open" } })));
    };
    if comment.issue_id.as_deref() != Some(command.issue_id.as_str()) {
        return Err(Error::Failed(format!("{}: the comment is on another issue", command.identifier)));
    }
    let mut read = json!({
        "kind": comment.kind.name(),
        "newestOwnerAt": comment.newest_owner_at,
        "state": state_of(comment.state),
    });
    if let CommentKind::NoteReply { root } | CommentKind::NoteRoot { root } = &comment.kind {
        read["root"] = json!(root);
    }
    Ok(Finished::ok(json!({ "comment": read })))
}

// The newest sessions a `readIssue` result names, well inside a result's size.
const SESSIONS_KEPT: usize = 50;
// The most members of a read set the machine takes (src/worker/machine/validate.ts).
const MEMBERS_KEPT: usize = 50;

pub struct Ids<'a> {
    pub viewer: &'a str,
    pub owner: &'a str,
}

// Err when a member was read without its ID or clock: the machine versions each by its own
// `updatedAt`, so one without can't be sent. `recorded`: when Tolkster's record says its chain
// delegated the issue.
pub fn issue_read(issue: &Issue, updated_at: &str, ids: &Ids, repo: Option<String>, sessions: &[crate::request::Session], recorded: Option<&str>) -> Result<Value, String> {
    // Unfinished blockers first: past MEMBERS_KEPT, only finished ones go unsaid, and
    // `moreBlockers` says some did.
    let mut blockers: Vec<&crate::request::Blocker> = issue.blockers.iter().collect();
    blockers.sort_by_key(|b| b.state_type.is_finished());
    let more_blockers = issue.more_blockers || blockers.len() > MEMBERS_KEPT;
    let blockers = members(blockers.into_iter().take(MEMBERS_KEPT), "a blocker", false)?;
    // A sub-issue in Triage, which nobody has accepted, doesn't hold up its parent's end.
    let children = members(issue.children.iter().take(MEMBERS_KEPT), "a sub-issue", true)?;
    let parent = match &issue.parent {
        None => Value::Null,
        Some(p) => json!({
            "id": p.id,
            "delegated": p.delegate_id.as_deref() == Some(ids.viewer),
            "by": delegated_by(p.delegate_id.as_deref(), p.delegated_by.as_ref().map(Option::as_deref), false, ids),
            "state": state_of(p.state_type),
            "priority": p.priority,
            "updatedAt": p.updated_at.as_deref().ok_or("its parent was read without its clock")?,
        }),
    };
    // Linear has no time for every change of state: an open issue's is its `updatedAt`, which
    // is no earlier.
    let changed_at = match issue.state_type {
        StateType::Completed => issue.completed_at.as_deref(),
        StateType::Canceled | StateType::Duplicate => issue.canceled_at.as_deref(),
        _ => None,
    };
    let delegator = issue.delegated.as_ref().map(|d| d.by.as_deref());
    // No entry read made the delegation: aged out when the page is full and nothing in it took
    // the issue from Tolkster, else not written yet, so newer than every entry read.
    let aged = issue.history_full && issue.undelegated_at.is_none();
    // No entry changed its delegate at all: delegated as the owner created it, unless an entry
    // is unwritten, which the machine waits out (docs/state-machine.md, rule 6).
    let created = delegator.is_none() && issue.created_delegated_by.as_deref() == Some(ids.owner);
    // No entry read made it, and nothing read took it from Tolkster since its chain's record:
    // the chain's. Delegating an issue already Tolkster's writes no entry at all.
    let ours = issue.delegate_id.as_deref() == Some(ids.viewer);
    let recorded = ours && delegator.is_none() && recorded.is_some_and(|at| issue.undelegated_at.as_ref().map_or(!issue.history_full, |taken| taken.as_str() < at));
    let delegate = if recorded { json!("chain") } else if created { json!("created") } else { delegated_by(issue.delegate_id.as_deref(), delegator, aged, ids) };
    let by_chain = recorded || (ours && issue.delegated.as_ref().and_then(|d| d.by.as_deref()) == Some(ids.viewer));
    Ok(json!({
        "updatedAt": updated_at,
        "delegate": delegate,
        "state": state_of(issue.state_type),
        "triage": issue.state_type == StateType::Triage,
        "stateChangedAt": changed_at.unwrap_or(updated_at),
        "priority": issue.priority,
        "blockers": blockers,
        "moreBlockers": more_blockers,
        "parent": parent,
        "children": children,
        "moreChildren": issue.more_children || issue.children.len() > MEMBERS_KEPT,
        "repo": repo,
        "sessions": sessions.iter().filter_map(|s| session_entry(s, by_chain, ids)).take(SESSIONS_KEPT).collect::<Vec<_>>(),
    }))
}

fn members<'b>(list: impl Iterator<Item = &'b crate::request::Blocker>, what: &str, triage: bool) -> Result<Vec<Value>, String> {
    list.map(|b| match (&b.id, &b.updated_at) {
        (Some(id), Some(at)) if triage => Ok(json!({ "id": id, "done": b.state_type.is_finished(), "triage": b.state_type == StateType::Triage, "updatedAt": at })),
        (Some(id), Some(at)) => Ok(json!({ "id": id, "done": b.state_type.is_finished(), "updatedAt": at })),
        _ => Err(format!("{what} was read without its ID or clock")),
    })
    .collect()
}

// Who delegated it to Tolkster, as the Worker's `delegateChanged` has it: null when it isn't
// Tolkster's, and `other` for anyone but the owner and Tolkster's chain, an automation
// included, which the machine never trusts. `by` is None when no entry read made the
// delegation: `aged` when it is older than the entries read, else `unknown`, never `other`,
// which only an entry naming someone else says.
fn delegated_by(delegate_id: Option<&str>, by: Option<Option<&str>>, aged: bool, ids: &Ids) -> Value {
    if delegate_id != Some(ids.viewer) {
        return Value::Null;
    }
    json!(match by {
        None if aged => "aged",
        None => "unknown",
        Some(Some(by)) if by == ids.owner => "owner",
        Some(Some(by)) if by == ids.viewer => "chain",
        Some(_) => "other",
    })
}

pub fn state_of(state: StateType) -> &'static str {
    match state {
        StateType::Completed => "completed",
        StateType::Canceled | StateType::Duplicate => "cancelled",
        _ => "open",
    }
}

// As the Worker's ingress tells them apart (src/worker/events.ts, `fromSession`): the owner's
// mention or delegation, a reply on a note of Tolkster's, and the chain's, which Tolkster's
// own delegation of the issue opens. None for anyone else's, an automation's included: the
// Worker refused it, and the machine counts every session it is sent toward which one a
// delegation asks (rule 3).
fn session_kind(session: &crate::request::Session, by_chain: bool, ids: &Ids) -> Option<&'static str> {
    match session.creator_id.as_deref() {
        Some(creator) if creator == ids.owner => Some(if session.mention.as_ref().is_some_and(|m| !m.in_description) { "mention" } else { "delegation" }),
        None if session.on_own_note => Some("reply"),
        Some(creator) if creator == ids.viewer => Some("chain"),
        None if session.mention.is_none() && by_chain => Some("chain"),
        _ => None,
    }
}

// A session of the read: its kind as the Worker's ingress has it, and `inDescription` when
// the owner's mention that began it is in the issue's description, which ingress can't tell
// from a delegation. Only then is it present, so the machine can narrow a delegation's
// session to a mention's, never widen one.
fn session_entry(session: &crate::request::Session, by_chain: bool, ids: &Ids) -> Option<Value> {
    let mut entry = json!({ "id": session.id, "kind": session_kind(session, by_chain, ids)?, "createdAt": session.created_at });
    if session.creator_id.as_deref() == Some(ids.owner) && session.mention.as_ref().is_some_and(|m| m.in_description) {
        entry["inDescription"] = json!(true);
    }
    Some(entry)
}
