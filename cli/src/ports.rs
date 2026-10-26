// The edges a command acts through (command.rs, executor.rs): Linear (`Linear`), Orca
// (`orca::Orca`), Tolkster's records (`state::Records`), GitHub (`github::Reader`) and the
// recipe machines `listen` makes (`Machines`), with what `listen` keeps from command to command
// (runtime::Shared), and the watcher, which waits on each turn or triage terminal a command opens.
// `listen` carries a command out through the real ones (live.rs), and tests through in-memory
// fakes (ports/fake.rs), with no network, Orca or Durable Object. Each answers with typed facts,
// so no code past this edge reads Linear's or Orca's JSON by pointer, and takes each ID as
// its plain type (plain.rs). The traits and their facts sit below the workflows that act through
// them, and the live ones above. A worker's setup (setup.rs) reads Linear through `Linear` too;
// other subcommands, and `listen`'s work off the command lanes, act on the agent instead,
// faking only the seam one decision needs (`context::Comments`, `offline::Issues`), as no
// command reaches it.
//
// The checks on what a command may act on (a session, comment, thread or worktree of its own
// issue) stay with the command: no port makes one, or skips one.

use crate::error::Error;
use crate::estimates::Scale;
use crate::github::Reader;
use crate::linear::{ActivityInput, Ref, StateType};
use crate::orca::Orca;
use crate::plain::{CommandId, Id, IssueKey, Time};
use crate::config::repo_map::Place;
use crate::request::{Issue, Session, Thread};
use crate::runtime::Shared;
use crate::sessions::{ClosedRead, CommentRead, CommentSession, Tolksters};
use crate::state::Records;
use crate::trace::Declined;

pub struct Ports<'p> {
    pub linear: &'p dyn Linear,
    pub records: &'p dyn Records,
    pub orca: &'p mut dyn Orca,
    pub github: &'p mut dyn Reader,
    pub machines: &'p dyn Machines,
    pub shared: &'p Shared,
    // Has the watcher wait on a terminal just opened, a turn's or triage's
    // (`scheduler::watch_terminal`).
    pub watch: &'p dyn Fn(&str),
}

// Whether a make or wake began: refused, why, or None for an ask on settings a make that ended
// since outdated, which the Durable Object sends again.
pub type Begun = Result<(), Option<Declined>>;

// Whether every worktree on the host, by its ID, was reaped and none is set up there. False
// when `reachable` is, for an asleep machine Orca no longer pairs, which can't be woken: then
// whether no worker's record names it.
pub type Reaped<'a> = dyn FnMut(&str, bool) -> Result<bool, Error> + 'a;

// A repo's recipe machines as `listen` makes them (recipe.rs): `make` returns at
// once, the machine made off the job lanes, and what it did goes to the trace.
pub trait Machines {
    // Whether this process is making one, for any repo.
    fn making(&self) -> bool;
    // Whether a make for `repo` began, as command `asked` asked (`recipe::spawn`).
    fn make(&self, repo: &str, asked: &CommandId) -> Begun;
    // The same for waking the machine `name` (`recipe::spawn_wake`).
    fn wake(&self, name: &str, asked: &CommandId) -> Begun;
    // The awake machine `name`, paired as `host`, put to sleep if `rests`, asked with
    // placement stopped, says no agent works on it (`recipe::sleep::suspend`): whether it slept.
    fn suspend(&self, name: &str, host: &str, rests: &mut dyn FnMut() -> Result<bool, Error>) -> Result<bool, Error>;
    // The dormant machine `name` destroyed once `reaped` says nothing on it is lost, else kept
    // (`recipe::retire::retire`): whether it went.
    fn retire(&self, name: &str, reaped: &mut Reaped) -> Result<bool, Error>;
}

// What a command reads from Linear and posts there.
pub trait Linear {
    // The session as Linear has it now, with its issue, and no prompts or record.
    fn session(&self, session_id: &Id) -> Result<Session, Error>;
    // `session` with Tolkster's record of its conversation.
    fn session_with_record(&self, session_id: &Id) -> Result<Session, Error>;
    // The ID of the issue the session is on: None when it is on none.
    fn session_issue(&self, session_id: &Id) -> Result<Option<String>, Error>;
    fn issue(&self, identifier: &IssueKey) -> Result<IssueFacts, Error>;
    // None when the comment is gone.
    fn comment(&self, comment_id: &Id) -> Result<Option<CommentFacts>, Error>;
    // The thread of Tolkster's note `root`, without the replies' text: Err, inside, says why
    // it is no note's thread (`sessions::read_thread`).
    fn thread(&self, root: &Id) -> Result<Result<Thread, &'static str>, Error>;
    // The identifiers of the issues with these IDs.
    fn identifiers(&self, issue_ids: &[Id]) -> Result<Vec<String>, Error>;
    // Whether the issue carries the manual-merge label.
    fn manual_merge(&self, issue_id: &Id) -> Result<bool, Error>;
    // The times the issue's description was changed.
    fn description_edits(&self, issue_id: &Id) -> Result<Vec<String>, Error>;
    // Posted once by an activity ID made from `key` (`post::post_once`).
    fn post_once(&self, key: &str, input: ActivityInput) -> Result<(), Error>;
    // Posted with no ID of its own, so a retry may post it again (`post::send`).
    fn send(&self, input: &ActivityInput) -> Result<(), Error>;
    // Posted once as Tolkster's reply in the thread of `root`, by a comment ID made from `key`
    // (`post::reply_once`): false when it was posted already, and nothing posted now.
    fn reply_once(&self, issue_id: &Id, root: &Id, body: &str, key: &str) -> Result<bool, Error>;
    // Which of these issues, by identifier, are Tolkster's, and whether each is finished
    // (`sessions::tolksters_issues`).
    fn tolksters(&self, identifiers: &[IssueKey]) -> Result<Tolksters, Error>;
    // A completed issue as its context reads it, with its relations and the viewer
    // (`sessions::read_closed`).
    fn closed(&self, issue_id: &Id) -> Result<ClosedRead, Error>;
    // The Triage queue, with the app's own user ID (`sessions::read_triage_queue`).
    fn triage_queue(&self) -> Result<TriageQueue, Error>;
    // Each issue with one of these IDs as triage reads it, by its ID, whatever it has become:
    // an ID Linear has no issue for is left out (`sessions::read_triage_issues`). One read.
    fn issues_closed(&self, issue_ids: &[Id]) -> Result<Vec<(Id, Candidate)>, Error>;
    // Whether each issue was filed in Triage, by its history (`triage::filed_in_triage`), in
    // the order asked; at most `sessions::HISTORIES_PER_READ`, in one read.
    fn state_histories(&self, identifiers: &[IssueKey]) -> Result<Vec<bool>, Error>;
    // Moved to its team's first started state, if it isn't started (`issue_state::start_issue`).
    fn start_issue(&self, issue: &Issue) -> Result<(), Error>;
    // Its delegation undone, if it is still Tolkster's (`issue_state::release_issue`).
    fn release_issue(&self, issue_id: &Id) -> Result<(), Error>;
}

// An issue as the triage rules read it.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub identifier: IssueKey,
    pub state_type: StateType,
    // Who created it, by Linear user id: Tolkster's own issues name its app user.
    pub creator: Option<String>,
    pub created_at: Option<String>,
    pub place: Place,
    // None if the team's estimate type isn't one Linear documents.
    pub scale: Option<Scale>,
    // Archived, or deleted, which Linear archives: the queue never lists one, but an issue
    // asked about by ID is read whatever it has become.
    pub archived: bool,
    // The issues it is related to, whichever side made the relation: a follow-up is related to
    // the issue it was filed from. Sorted and without repeats.
    pub related: Vec<IssueKey>,
}

// The issues in Triage, as triage reads them (`triage::queue` keeps those it may start),
// and the app's own user ID.
pub struct TriageQueue {
    pub candidates: Vec<Candidate>,
    pub viewer_id: String,
}

// The issue as `readIssue` reads it, at its `updatedAt`, with Tolkster's sessions on it and
// the app's own user ID, `viewer_id`.
pub struct IssueFacts {
    pub issue: Issue,
    pub updated_at: String,
    pub sessions: Vec<Session>,
    pub viewer_id: String,
}

// A comment as `readComment` reads it (docs/state-machine.md, Commands): the issue it is on,
// what it is (`comment_kind`), the time of the owner's newest comment on the issue and the
// issue's state.
#[derive(Clone, Debug, PartialEq)]
pub struct CommentFacts {
    pub issue_id: Option<String>,
    pub kind: CommentKind,
    pub newest_owner_at: Option<String>,
    pub state: StateType,
}

// What a comment is. A plain one on a finished issue is still `Plain`, as the facts carry the
// issue's state.
#[derive(Clone, Debug, PartialEq)]
pub enum CommentKind {
    // The owner's comment on an issue, for its worker to hear of.
    Plain,
    // The owner's reply in the thread of a comment of Tolkster's that no session began on,
    // whose root is `root`, which keys its request: answered in that thread.
    NoteReply { root: String },
    // The owner's top-level comment, no mention, on an issue Tolkster created, or their reply
    // under one: the thread `root` keys its request, answered as a `NoteReply` is, unless a
    // worker runs to be told of it.
    NoteRoot { root: String },
    // The owner's mention that began a session, or was its source: its session passes it on.
    Mention,
    // The owner's reply in a session's thread, passed on to it as a message.
    Reply,
    // Gone, or not the owner's, or on no issue, or unreadable: nothing to tell.
    Gone,
}

impl CommentKind {
    // As `readComment` names it.
    pub fn name(&self) -> &'static str {
        match self {
            CommentKind::Plain => "plain",
            CommentKind::NoteReply { .. } => "noteReply",
            CommentKind::NoteRoot { .. } => "noteRoot",
            CommentKind::Mention => "mention",
            CommentKind::Reply => "reply",
            CommentKind::Gone => "gone",
        }
    }
}

// What the comment in `data` (`sessions::read_comment`'s answer) is. Ownership is re-checked
// here because the Worker decides from the webhook, and a turn answers in the thread of
// Tolkster's own comment only a reply this finds the owner's. A mention or a reply in one of
// Tolkster's (the viewer's) sessions reaches the worker as a message (voice::new_message), so
// neither is passed on twice.
pub fn comment_kind(data: &CommentRead, owner_id: &str) -> CommentKind {
    let Some(comment) = &data.comment else { return CommentKind::Gone };
    let by = |user: &Option<Ref>, id: &str| user.as_ref().is_some_and(|u| u.id == id);
    // A comment an integration posted as the owner is the integration's.
    if owner_id.is_empty() || !by(&comment.user, owner_id) || comment.bot_actor.is_some() {
        return CommentKind::Gone;
    }
    let viewer = data.viewer.id.as_str();
    let Some(issue) = comment.issue.as_ref().filter(|i| IssueKey::parse(&i.identifier).is_some() && Time::parse(&comment.created_at).is_some()) else { return CommentKind::Gone };
    let id = comment.id.as_str();
    let sessions: Vec<&CommentSession> = issue.agent_sessions.nodes.iter().filter(|s| s.app_user.id == viewer).collect();
    let root = |s: &CommentSession| s.comment.as_ref().map(|c| c.id.clone());
    if sessions.iter().any(|s| root(s).as_deref() == Some(id) || s.source_comment.as_ref().is_some_and(|c| c.id == id)) {
        return CommentKind::Mention;
    }
    let created_by_me = by(&issue.creator, viewer);
    if let Some(parent) = &comment.parent {
        if sessions.iter().any(|s| root(s).as_deref() == Some(parent.id.as_str())) {
            return CommentKind::Reply;
        }
        if by(&parent.user, viewer) {
            return CommentKind::NoteReply { root: parent.id.clone() };
        }
        if by(&parent.user, owner_id) && parent.bot_actor.is_none() && created_by_me {
            return CommentKind::NoteRoot { root: parent.id.clone() };
        }
    } else if created_by_me {
        return CommentKind::NoteRoot { root: id.into() };
    }
    CommentKind::Plain
}

// `sessions::read_comment`'s answer, read with the owner's comments: None when the comment
// is gone.
pub fn comment_facts(data: &CommentRead, owner_id: &str) -> Option<CommentFacts> {
    let comment = data.comment.as_ref()?;
    let kind = comment_kind(data, owner_id);
    // A plain comment, or a note's root, is the owner's own, so the newest is never older than it, even
    // where Linear's list lags behind.
    let issue = comment.issue.as_ref();
    let listed = issue.and_then(|i| i.comments.as_ref()).and_then(|c| c.nodes.first()).map(|c| c.created_at.clone());
    let own = matches!(kind, CommentKind::Plain | CommentKind::NoteRoot { .. }).then(|| comment.created_at.clone());
    Some(CommentFacts { issue_id: issue.map(|i| i.id.clone()), kind, newest_owner_at: listed.max(own), state: issue.map_or(StateType::Other, |i| i.state.kind) })
}

#[cfg(test)]
pub mod fake;
#[cfg(test)]
mod tests;
