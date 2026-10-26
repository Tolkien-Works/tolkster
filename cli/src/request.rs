// A session and its issue as read from Linear (sessions.rs parses into these), the owner
// check, and the owner's messages in it. What the owner asks, and what Tolkster does about
// it, is the Durable Object's machines' (docs/state-machine.md): nothing here decides it.

use crate::estimates::{Scale, Size};
use crate::linear::{Kind, StateType};
use crate::plain::{Id, IssueKey, Time};
use crate::config::repo_map::Place;
use crate::state::{Delegation, Record};

// An issue with this label, ignoring case, is planned first; nothing else is.
pub const PLAN_FIRST_LABEL: &str = "plan-first";

// An issue with this label, ignoring case, is left for the owner to merge.
pub const MANUAL_MERGE_LABEL: &str = "manual-merge";

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub id: String,
    pub identifier: String,
    pub url: String,
    pub branch_name: String,
    pub state_type: StateType,
    // None from a query that doesn't read it.
    pub team_id: Option<String>,
    pub delegate_id: Option<String>,
    pub place: Place,
    pub estimate: Option<f64>,
    // Linear's 0 (none), 1 (urgent) to 4 (low), which `readIssue` gives the queue.
    pub priority: u8,
    pub scale: Option<Scale>,
    pub labels: Vec<String>,
    pub created_at: String,
    // None when it has aged out of the history entries read.
    pub delegated: Option<Delegated>,
    // When the issue was last taken from Tolkster (undelegated, or delegated to someone else):
    // the change a halt is for. None when it never was, among the history entries read.
    pub undelegated_at: Option<String>,
    // The history read is a full page, so an older entry may be unread.
    pub history_full: bool,
    // Its creator, when it is delegated to Tolkster and no entry in a history read that isn't a
    // full page changed its delegate: delegated as it was created, unless an entry is unwritten.
    pub created_delegated_by: Option<String>,
    // When it was cancelled, while its state is cancelled. A reopened issue may keep its old
    // time, so only the state says. The same holds for `completed_at`. `readIssue` gives the
    // one its state has as when its state changed (command.rs).
    pub canceled_at: Option<String>,
    pub completed_at: Option<String>,
    // Every issue that blocks this one, finished or not. Only identifiers, links and
    // state types are read: a blocker's text is untrusted like any issue text.
    pub blockers: Vec<Blocker>,
    // It has more relations than were read (`sessions::RELATION_PAGES`): an unseen one may be
    // an unfinished blocker, so the issue counts as blocked, with none to name.
    pub more_blockers: bool,
    pub parent: Option<Parent>,
    pub children: Vec<Blocker>,
    // It has more sub-issues than were read: those count as unfinished.
    pub more_children: bool,
    pub children_finished_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Parent {
    pub id: String,
    pub identifier: String,
    pub state_type: StateType,
    pub delegate_id: Option<String>,
    // Its own priority, which a sub-issue without one is queued by (docs/dispatcher.md).
    pub priority: u8,
    // As on `Issue`: when it was last taken from Tolkster, and when it was cancelled while its
    // state is cancelled. The change a halt is for once it ends a sub-issue's chain.
    pub undelegated_at: Option<String>,
    pub canceled_at: Option<String>,
    // Its own `updatedAt`, which versions what is read of it (docs/state-machine.md, Facts),
    // and who made its delegation, from its history: None from a query that doesn't read it,
    // and the second also when no entry read made it, Some(None) for an automation's.
    pub updated_at: Option<String>,
    pub delegated_by: Option<Option<String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Blocker {
    pub identifier: String,
    pub url: String,
    pub state_type: StateType,
    // Its ID and own `updatedAt`, which versions its state (docs/state-machine.md, Facts):
    // None from a query that doesn't read them.
    pub id: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Delegated {
    pub at: String,
    // Who made it; None for an automation's.
    pub by: Option<String>,
    // It asks this session again, which `tolkster messages` says (commands/messages.rs). For a
    // delegation's session: the issue was given back after an older entry had taken it from the
    // delegate. For a mention's session: Linear reopened it for the delegation.
    pub again: bool,
}

impl Issue {
    // None: no estimate, the team doesn't use estimates, or its scale wasn't read.
    pub fn size(&self) -> Option<Size> {
        self.scale.zip(self.estimate).and_then(|(scale, points)| scale.size(points))
    }

}

// The unfinished sub-issues a parent's worker may chain: none in Triage, which nobody has
// accepted yet (`triaged_children`).
pub fn unfinished_children(issue: &Issue) -> Vec<(String, String)> {
    unfinished(issue.children.iter().filter(|c| c.state_type != StateType::Triage))
}

pub fn triaged_children(issue: &Issue) -> Vec<String> {
    unfinished(issue.children.iter().filter(|c| c.state_type == StateType::Triage)).into_iter().map(|(identifier, _)| identifier).collect()
}

fn unfinished<'a>(issues: impl Iterator<Item = &'a Blocker>) -> Vec<(String, String)> {
    let mut unfinished: Vec<(String, String)> = issues.filter(|i| !i.state_type.is_finished()).map(|i| (i.identifier.clone(), i.url.clone())).collect();
    natural_dedup(&mut unfinished, |(identifier, _)| identifier.as_str());
    unfinished
}

fn natural(identifier: &str) -> (&str, u64) {
    crate::plain::split_identifier(identifier).unwrap_or((identifier, u64::MAX))
}

pub fn natural_dedup<T>(items: &mut Vec<T>, identifier: impl Fn(&T) -> &str) {
    items.sort_by(|a, b| natural(identifier(a)).cmp(&natural(identifier(b))));
    items.dedup_by(|a, b| identifier(a) == identifier(b));
}

#[derive(Clone, Debug)]
pub struct Activity {
    // Linear's id. Only prompts' are used: a handled request is recorded by its id.
    pub id: Option<String>,
    pub created_at: String,
    pub author_id: Option<String>,
    pub kind: Kind,
    pub body: Option<String>,
    pub signal: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Mention {
    // None for a comment without an author, e.g. one synced from another tool.
    pub author_id: Option<String>,
    pub body: String,
    // The mention is in the issue's description, which Linear gives no comment: its author is
    // the issue's creator, and its body empty, the issue being the request
    // (`sessions::in_description`).
    pub in_description: bool,
}

#[derive(Clone, Debug)]
pub struct Reply {
    pub created_at: String,
    pub author_id: Option<String>,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub id: String,
    pub created_at: String,
    pub creator_id: Option<String>,
    pub app_user_id: Option<String>,
    pub issue: Option<Issue>,
    // The comment that mentioned Tolkster, or the issue's description: the request. A
    // delegation's session has a comment too, but Linear writes it itself, so it has no author
    // and is no mention (`sessions::mentioning_comment`, `sessions::in_description`).
    pub mention: Option<Mention>,
    // Tolkster opened it on a comment of its own, for the owner's reply there
    // (`sessions::on_own_note`). Its mention is that comment, and its request the replies
    // before it began, read only with bodies, whoever wrote them.
    pub on_own_note: bool,
    pub note_replies: Vec<Reply>,
    // The session's prompts, newest first: Linear filters the read to them (sessions.rs), so no
    // ephemeral thought takes a place. `more_activities`: older prompts are unread.
    pub activities: Vec<Activity>,
    pub more_activities: bool,
    // Empty until loaded: its conversation's turn.
    pub record: Record,
    // None until loaded, and for every issue Tolkster didn't delegate to itself.
    pub delegation: Option<Delegation>,
}

impl Session {
    // The owner started it (Linear's creator, and the mention's author when there is one),
    // Tolkster opened it on its own comment, which `listen` does only for the owner's reply
    // (ports::comment_kind), or
    // Tolkster delegated the issue to itself as a sub-issue of one the owner (or such a
    // delegation, in turn) gave it, and the chain still holds (`chain_holds`). The chain part is
    // decided from Tolkster's record of the delegation and the parent read fresh from Linear,
    // never from a worker's word, whoever Linear names as its creator. A delegation someone else
    // has made since isn't Tolkster's.
    pub fn is_owners(&self, owner_id: &str) -> bool {
        let started = self.on_own_note || self.creator_id.as_deref() == Some(owner_id) && self.mention.as_ref().is_none_or(|m| m.author_id.as_deref() == Some(owner_id));
        let by = self.issue.as_ref().and_then(|i| i.delegated.as_ref()).and_then(|d| d.by.as_deref());
        let someone_elses = by.is_some_and(|by| by != owner_id && Some(by) != self.app_user_id.as_deref());
        started || (self.chain_holds() && !someone_elses)
    }

    // A session on a sub-issue Tolkster delegated to itself: its record names the issue's parent
    // as it is now. True whether or not the chain still holds, so a worker started for it can be
    // stopped once it doesn't (the machine's `parentHalted`). Only a session Tolkster's delegation could have
    // started, so one someone else started before never becomes Tolkster's because it delegated
    // the issue since.
    pub fn tolksters_own(&self) -> bool {
        let (Some(delegation), Some(issue), None) = (&self.delegation, &self.issue, &self.mention) else { return false };
        let by_tolkster = self.creator_id.is_none() || self.creator_id == self.app_user_id;
        by_tolkster && delegation.issue_id == issue.id && issue.parent.as_ref().is_some_and(|p| p.id == delegation.parent_issue_id)
    }

    // The parent is still delegated to Tolkster and hasn't been cancelled. Undelegating or
    // cancelling it breaks the chain, and nothing more in it starts.
    pub fn chain_holds(&self) -> bool {
        let parent = self.issue.as_ref().and_then(|i| i.parent.as_ref());
        self.tolksters_own() && parent.is_some_and(|p| p.delegate_id.is_some() && p.delegate_id == self.app_user_id && !p.state_type.is_cancelled())
    }

    // The owner's newest request: their newest prompt, with its activity, or else the session's
    // start, named by the session. None when the activities read hold no prompt of the owner's
    // but there are older ones. What `tolkster messages` reports as read (`workerRead`).
    pub fn newest_request(&self, owner_id: &str) -> Option<(Option<String>, String)> {
        if !self.is_owners(owner_id) {
            return None;
        }
        match self.activities.iter().find(|a| a.kind == Kind::Prompt && a.author_id.as_deref() == Some(owner_id)) {
            Some(prompt) => Some((prompt.id.clone(), prompt.created_at.clone())),
            None => (!self.more_activities).then(|| (None, self.created_at.clone())),
        }
    }

    // The owner's messages after `after`, oldest first: those after what its reader last read
    // or answered (commands/messages.rs), or every one. The mention counts from when the
    // session began.
    pub fn messages_after(&self, owner_id: &str, after: Option<&str>) -> Vec<Message> {
        self.owner_messages(owner_id, after)
    }

    // The owner's prompts after `after`, or all when nothing bounds it, and the mention, or a
    // note's replies, when the session began after the bound too and every activity was read.
    // Oldest first.
    fn owner_messages(&self, owner_id: &str, after: Option<&str>) -> Vec<Message> {
        if !self.is_owners(owner_id) {
            return Vec::new();
        }
        let mut messages: Vec<Message> = self
            .activities
            .iter()
            .filter(|a| a.kind == Kind::Prompt && a.author_id.as_deref() == Some(owner_id) && after.is_none_or(|after| a.created_at.as_str() > after))
            .map(|a| Message { created_at: a.created_at.clone(), body: a.body.clone().unwrap_or_default(), stop: a.signal.as_deref() == Some("stop"), mention: false })
            .collect();
        let unbounded = !self.more_activities && after.is_none_or(|after| self.created_at.as_str() > after);
        if self.on_own_note {
            // Each of the owner's replies to the note before it began, so two in a row, or
            // someone else's between, lose none. Newest first, as the prompts are here.
            let replies = self.note_replies.iter().rev().filter(|r| unbounded && r.author_id.as_deref() == Some(owner_id));
            messages.extend(replies.map(|r| Message { created_at: r.created_at.clone(), body: r.body.clone(), stop: false, mention: true }));
        } else if let Some(mention) = self.mention.as_ref().filter(|_| unbounded) {
            messages.push(Message { created_at: self.created_at.clone(), body: mention.body.clone(), stop: false, mention: true });
        }
        messages.reverse();
        messages
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub created_at: String,
    pub body: String,
    pub stop: bool,
    pub mention: bool,
}

// The thread of a comment of Tolkster's that no session began on, as read for the owner's
// replies in it (sessions::read_thread): a turn answers them there with a plain reply.
#[derive(Clone, Debug, PartialEq)]
pub struct Thread {
    // Tolkster's comment, or the owner's own on an issue Tolkster created, the thread's root.
    pub root: Id,
    pub issue_id: Id,
    pub identifier: IssueKey,
    pub place: Place,
    // The owner's replies, oldest first, the root first when it is theirs. Each body is
    // empty unless read with bodies.
    pub replies: Vec<ThreadReply>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThreadReply {
    pub id: Id,
    pub created_at: Time,
    pub body: String,
}

impl Thread {
    // The owner's replies after `after`, or all of them.
    pub fn replies_after(&self, after: Option<&str>) -> Vec<&ThreadReply> {
        self.replies.iter().filter(|r| after.is_none_or(|after| r.created_at.as_str() > after)).collect()
    }
}

#[cfg(test)]
pub mod tests;
