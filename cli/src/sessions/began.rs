// How a session began, and what the issue's history says of its delegations: a delegation's
// session or a mention's, told apart by who and when, never by the text of a comment.

use crate::linear::Ref;
use crate::request::{Delegated, Reply};

use super::{Began, HISTORY, HistoryEntry, IssueNode, SessionComment};

// The issue's newest delegation to its delegate, if among the history read
// (`Issue::delegated`), and whether it asks the session that began as `node` again: a
// delegation's session, by a re-delegation to its app after the issue was taken from it
// (`superseded`); a mention's, by a delegation Linear gave no session of its own
// (`reopened_for`). A re-delegation whose undelegation is older than the entries read goes
// unseen, so it never makes a request. With no session, it never asks again.
pub(super) fn delegated(issue: &IssueNode, node: Option<&Began>) -> Option<Delegated> {
    let delegate = issue.delegate.as_ref()?.id.as_str();
    let changes = delegate_changes(&issue.history.nodes);
    let newest = changes.iter().position(|c| c.to.as_deref() == Some(delegate))?;
    let taken_before = changes[newest + 1..].iter().any(|c| c.from.as_deref() == Some(delegate));
    let again = node.is_some_and(|node| {
        let app = node.app_user.id.as_str();
        app == delegate
            && match is_mention(issue, node) {
                true => reopened_for(issue, app, &node.created_at, &changes[newest].at, taken_before),
                false => taken_before && !superseded(issue, app, &node.created_at),
            }
    });
    let back = &changes[newest];
    Some(Delegated { at: back.at.clone(), by: back.actor.clone(), again })
}

// Who made the delegation to `delegate` now: the newest change to it among the history read.
// None when there is none, or the change aged out; Some(None) for an automation's.
pub(super) fn delegator(delegate: Option<&Ref>, history: &[HistoryEntry]) -> Option<Option<String>> {
    let delegate = delegate?.id.as_str();
    Some(delegate_changes(history).into_iter().find(|c| c.to.as_deref() == Some(delegate))?.actor)
}

// When the issue was last taken from the app `app_user_id`, among the history read.
pub(super) fn undelegated_at(history: &[HistoryEntry], app_user_id: Option<&str>) -> Option<String> {
    let app_user_id = app_user_id?;
    delegate_changes(history).into_iter().find(|c| c.from.as_deref() == Some(app_user_id)).map(|c| c.at)
}

fn delegate_changes(history: &[HistoryEntry]) -> Vec<DelegateChange> {
    let id = |r: &Option<Ref>| r.as_ref().map(|r| r.id.clone());
    let mut changes: Vec<DelegateChange> = history.iter().map(|h| DelegateChange { at: h.created_at.clone(), actor: id(&h.actor), from: id(&h.from_delegate), to: id(&h.to_delegate) }).collect();
    // Newest first. Linear's timestamps share one ISO format, so text order is time order.
    changes.sort_by(|a, b| b.at.cmp(&a.at));
    changes
}

struct DelegateChange {
    at: String,
    actor: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

// The app `app`'s sessions among the issue's newest read.
fn apps_sessions<'a>(issue: &'a IssueNode, app: &'a str) -> impl Iterator<Item = &'a Began> {
    issue.agent_sessions.iter().flat_map(|s| &s.nodes).filter(move |n| n.app_user.id == app)
}

// Whether the app has a delegation's session on the issue newer than the one that began at
// `created`. A re-delegation belongs to the newest: an older one would ask too, and its
// claim, named after its own session, would take the same request again. A mention's
// session asks something else, so it doesn't count.
fn superseded(issue: &IssueNode, app: &str, created: &str) -> bool {
    apps_sessions(issue, app).filter(|n| !is_mention(issue, n)).any(|n| n.created_at.as_str() > created)
}

// Whether a delegation belongs to the mention's session that began at `created`. Linear
// gives a delegation no session of its own when the app already has one, and reopens that
// one. So the mention's session takes it when it is the newest of the app's sessions read
// that began before the delegation (`delegated_at`) and none of them is a delegation's. A
// delegation's session reopened by a re-delegation (`taken_before`) may be older than those
// read and would take it too, so then every session must have been read.
fn reopened_for(issue: &IssueNode, app: &str, created: &str, delegated_at: &str, taken_before: bool) -> bool {
    let more = issue.agent_sessions.as_ref().is_some_and(|s| s.page_info.has_next_page);
    let apps: Vec<&Began> = apps_sessions(issue, app).collect();
    (!taken_before || !more)
        && apps.iter().any(|n| is_mention(issue, n) && n.created_at == created)
        && apps.iter().all(|n| is_mention(issue, n) && (n.created_at.as_str() <= created || n.created_at.as_str() > delegated_at))
}

// The comment that mentioned Tolkster in the session that began as `node`, if a mention
// started it, as the Worker tells them apart (`src/worker/decide.ts`): its source comment,
// else its comment when that has an author. A delegation's comment is written by Linear
// itself, so it has none.
pub(super) fn mentioning_comment(node: &Began) -> Option<&SessionComment> {
    node.source_comment.as_ref().or_else(|| node.comment.as_ref().filter(|c| c.user.is_some()))
}

// Whether a mention started the session `node` on `issue`, in a comment or in the description.
fn is_mention(issue: &IssueNode, node: &Began) -> bool {
    mentioning_comment(node).is_some() || in_description(issue, node)
}

// Whether the session `node` began because the issue's creator mentioned the app `node` is of
// in its description. Linear marks such a session no differently from a delegation's: no source
// comment, a thread it writes itself, and no source metadata. Only its start tells them apart:
// it began with the issue, at the same instant, its creator is the issue's, and the app wasn't
// the issue's delegate then (`delegate_when_created`); when that can't be known, it is no
// mention. A mention added to the description later isn't told apart, and reads as a
// delegation's.
pub(super) fn in_description(issue: &IssueNode, node: &Began) -> bool {
    let creator = node.creator.as_ref().map(|c| c.id.as_str());
    mentioning_comment(node).is_none()
        && creator.is_some()
        && creator == issue.creator.as_ref().map(|c| c.id.as_str())
        && node.created_at == issue.created_at
        && delegate_when_created(issue).is_some_and(|d| d.as_deref() != Some(node.app_user.id.as_str()))
}

// The issue's delegate when it was created: the one an entry made then gave it, else the one the
// oldest change of delegate among the history read took it from, else, with no change read, the
// delegate it has now. None when that can't be known: the history read is a full page, so an
// older change may be unread, and none of it was made at the creation.
fn delegate_when_created(issue: &IssueNode) -> Option<Option<String>> {
    let changes: Vec<DelegateChange> = delegate_changes(&issue.history.nodes).into_iter().filter(|c| c.from.is_some() || c.to.is_some()).collect();
    if let Some(at_creation) = changes.last().filter(|c| c.at == issue.created_at) {
        return Some(at_creation.to.clone());
    }
    if issue.history.nodes.len() >= HISTORY as usize {
        return None;
    }
    Some(changes.last().map_or_else(|| issue.delegate.as_ref().map(|d| d.id.clone()), |oldest| oldest.from.clone()))
}

// The issue's creator, when it was delegated to the app `app_user_id` as it was created, as
// far as the history read shows: `Issue::created_delegated_by`.
pub(super) fn created_delegated_by(issue: &IssueNode, app_user_id: Option<&str>) -> Option<String> {
    let app = app_user_id?;
    let changed = delegate_changes(&issue.history.nodes).iter().any(|c| c.from.is_some() || c.to.is_some());
    let full = issue.history.nodes.len() >= HISTORY as usize;
    (issue.delegate.as_ref().map(|d| d.id.as_str()) == Some(app) && !changed && !full).then(|| issue.creator.as_ref().map(|c| c.id.clone())).flatten()
}

// Whether Tolkster opened the session that began as `node` itself, on a comment of its own
// (ports::comment_kind, for the owner's reply to it): no one created it, it has no source
// comment, and its comment is the app's. A delegation's comment has no author, and a
// mention's session has its author as creator.
pub(super) fn on_own_note(node: &Began) -> bool {
    node.creator.is_none() && node.source_comment.is_none() && node.comment.as_ref().and_then(|c| c.user.as_ref()).is_some_and(|u| u.id == node.app_user.id)
}

// The request of a session Tolkster opened on its own comment: the replies in the thread
// written before the session began, oldest first, by people, not an integration. Only a read
// with bodies has them; `Session::owner_messages` keeps the owner's.
pub(super) fn replies_before(node: &Began) -> Vec<Reply> {
    let began = node.created_at.as_str();
    let mut replies: Vec<Reply> = node
        .comment
        .iter()
        .flat_map(|c| &c.children)
        .flat_map(|c| &c.nodes)
        .filter(|r| r.bot_actor.is_none() && r.created_at.as_str() <= began)
        .map(|r| Reply { created_at: r.created_at.clone(), author_id: r.user.as_ref().map(|u| u.id.clone()), body: r.body.clone().unwrap_or_default() })
        .collect();
    replies.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    replies
}
