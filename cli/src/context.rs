// Whether and when a context session starts for a completed issue, what it may comment
// on, and how its one comment on each issue is written (docs/context.md). The rules are in
// code; `tolkster comment --closed` checks them again.
//
// Nothing here reads issue text: only identifiers, state types, archive times, the team's
// key and the project's id and name.

use std::time::{Duration, Instant};

use crate::brief::{self, Checkouts, open_session, reading_prompt};
use crate::cli::write_file;
use crate::config::Config;
use crate::linear::{Relation, StateType};
use crate::error::Error;
use crate::sessions::{ClosedIssue, ClosedRead, Neighbour};
use crate::git::{self, Merge};
use crate::orca::Orca;
use crate::ports::Ports;
use crate::config::repo_map::{Place, RepoMap};
use crate::plain::{Id, IssueKey, RepoName, Time};
use crate::time::now_iso;
use crate::state::{ClaimKind, ContextComment, Records, Store, claim_name, holding, new_token, new_uuid};
use crate::trace::Context;

const CONTEXT: &str = include_str!("../briefs/context.md");

// Duplicates and similar issues don't lead anywhere.
const RELATIONS: [Relation; 3] = [Relation::Blocks, Relation::BlockedBy, Relation::Related];

#[derive(Clone, Debug, PartialEq)]
pub struct Closed {
    pub identifier: IssueKey,
    pub completed: bool,
    pub place: Place,
    // Each sorted and without repeats, and a sibling related to it only in `open_related`.
    pub open_related: Vec<IssueKey>,
    // The other open sub-issues of its parent.
    pub open_siblings: Vec<IssueKey>,
}

impl Closed {
    pub fn has_open(&self) -> bool {
        !self.open_related.is_empty() || !self.open_siblings.is_empty()
    }
}

pub fn parse_closed(issue: &ClosedIssue) -> Result<Closed, Error> {
    let identifier = IssueKey::parse(&issue.identifier).ok_or_else(|| Error::Failed("an issue's identifier isn't plain".into()))?.upper();
    let keys = |others: Vec<&Neighbour>, except: &[IssueKey]| {
        let mut keys: Vec<IssueKey> = others.into_iter().filter_map(open).filter(|i| *i != identifier && !except.contains(i)).collect();
        keys.sort();
        keys.dedup();
        keys
    };
    let open_related = keys(RELATIONS.iter().flat_map(|relation| relation.related(&issue.relations)).map(|r| r.other).collect(), &[]);
    let siblings = issue.parent.iter().flat_map(|p| &p.children.nodes).collect();
    let open_siblings = keys(siblings, &open_related);
    Ok(Closed { identifier, completed: issue.state.kind == StateType::Completed, place: Place::of(Some(&issue.team.key), issue.project.as_ref().map(|p| p.parts())), open_related, open_siblings })
}

// The issue's identifier, upper-cased, when it is open: not completed, cancelled or archived.
fn open(other: &Neighbour) -> Option<IssueKey> {
    if other.state.kind.is_finished() || other.archived_at.is_some() {
        return None;
    }
    Some(IssueKey::parse(&other.identifier)?.upper())
}

// Err says why not.
pub fn may_comment(closed: &Closed, target: &IssueKey) -> Result<(), String> {
    if !closed.completed {
        return Err(format!("{} isn't completed", closed.identifier));
    }
    if !closed.open_related.iter().chain(&closed.open_siblings).any(|i| i.eq_ignore_ascii_case(target)) {
        return Err(format!("{target} isn't an open issue related to {}, nor an open sub-issue of its parent", closed.identifier));
    }
    Ok(())
}

// Taken for good once a context session starts for a completed issue: one per issue, so a
// resent or repeated `startContext` never starts a second.
pub fn context_claim(closed: &IssueKey) -> String {
    claim_name(ClaimKind::Context, closed)
}

// Taken while an issue's context comment is written, by the issue's Linear ID, which a move
// to another team keeps: two context sessions writing to one issue at once would each find
// no record and create a comment. Given back once written; kept until it lapses, after
// COMMENT_HOLD, when a write failed, so a create that lands late is never raced by a
// replacement. Longer than a write's calls can take, with their retries.
pub fn comment_claim(issue_id: &Id) -> String {
    claim_name(ClaimKind::Comment, issue_id)
}

pub const COMMENT_HOLD: Duration = Duration::from_secs(300);

// What a write needs left of the claim: the record's write and Linear's, with their retries.
const WRITE_TIME: Duration = Duration::from_secs(120);

// Linear's side of a context comment (commands/comment.rs).
pub trait Comments {
    // Whether comment `comment_id` is still on issue `issue_id`: false once it is deleted.
    fn on_issue(&self, issue_id: &Id, comment_id: &Id) -> Result<bool, Error>;
    // With `comment_id` as its ID, which Linear refuses once it has it.
    fn create(&self, issue_id: &Id, comment_id: &Id, body: &str) -> Result<(), Error>;
    fn update(&self, comment_id: &Id, body: &str) -> Result<(), Error>;
}

#[derive(Clone, Debug, PartialEq)]
pub enum Wrote {
    Created,
    Updated,
    // The recorded comment was gone, so a new one took its place.
    Replaced,
    // Nothing written: the comment was written at this time, after what the session read.
    Changed(Time),
}

// Writes `body` as Tolkster's one context comment on issue `issue_id`: updates the recorded
// comment, or creates one and records it, replacing a record whose comment is gone. `seen` is
// when the session read the issue, or the time a `Changed` gave it: a comment written since
// holds what the session hasn't folded in, so nothing is written over it. None when another
// run holds the issue's claim, and nothing was written.
//
// The record, with its time, `now` as it is written, is written before the comment, and a
// create with the record's ID, so a create whose answer was lost is found and updated by the next run, never posted
// twice; a record whose create failed outright is replaced as a deleted comment's is.
pub fn write_comment<R: Records>(records: &R, comments: &dyn Comments, issue_id: &Id, body: &str, seen: &Time, now: &dyn Fn() -> Result<Time, Error>, token: &str) -> Result<Option<Wrote>, Error> {
    let claims = [(comment_claim(issue_id), Some(COMMENT_HOLD))];
    // Before the take, so the claim lapses no sooner than this measures.
    let taken = Instant::now();
    let outcome = holding(records, &claims, token, || {
        let recorded = records.context_comment(issue_id)?;
        if let Some(recorded) = &recorded
            && recorded.at.as_str() > seen.as_str()
        {
            return Ok(Ok(Wrote::Changed(Time::check(&recorded.at)?)));
        }
        // A recorded ID that isn't plain names no comment Linear could have.
        let live = match recorded.as_ref().and_then(|recorded| Id::parse(&recorded.comment_id)) {
            Some(id) => comments.on_issue(issue_id, &id)?.then_some(id),
            None => None,
        };
        let (comment_id, wrote) = match (&recorded, live) {
            (_, Some(id)) => (id, Wrote::Updated),
            (Some(_), None) => (Id::check(&new_uuid())?, Wrote::Replaced),
            (None, None) => (Id::check(&new_uuid())?, Wrote::Created),
        };
        // A take again keeps the claim's first time, so it can't extend it: with too little
        // of it left for the writes, another run could take it over mid-write, so nothing is
        // written.
        if taken.elapsed() > COMMENT_HOLD - WRITE_TIME {
            return Err(Error::Failed("too little of the claim on the issue's context comment was left to write it".into()));
        }
        records.set_context_comment(&ContextComment { issue_id: issue_id.to_string(), comment_id: comment_id.to_string(), at: now()?.to_string() })?;
        // A write that failed may yet land: Ok, so the claim stays held until it lapses. Once
        // it has landed, the record's time is taken again, so a session that read the issue
        // while it was in flight finds it newer than its `--seen`.
        let written = if wrote == Wrote::Updated { comments.update(&comment_id, body) } else { comments.create(issue_id, &comment_id, body) };
        Ok(written.and_then(|()| {
            records.set_context_comment(&ContextComment { issue_id: issue_id.to_string(), comment_id: comment_id.to_string(), at: now()?.to_string() })?;
            Ok(wrote)
        }))
    })?;
    match outcome {
        None => Ok(None),
        Some(Ok(wrote)) => {
            // Left held, it lapses after COMMENT_HOLD.
            let _ = records.release(&[claims[0].0.clone()], token);
            Ok(Some(wrote))
        }
        Some(Err(error)) => Err(error),
    }
}

pub struct Start<'a> {
    pub closed: &'a Closed,
    pub owner_name: &'a str,
    pub briefs_dir: &'a str,
    pub repos: &'a RepoMap,
    // When the brief is written, which the session's first `comment --closed` passes as
    // `--seen`: it reads each issue after that.
    pub at: &'a Time,
}

impl Start<'_> {
    pub fn render(&self, repo: &str, merge: &Merge) -> String {
        let list = |keys: &[IssueKey]| if keys.is_empty() { "none".to_string() } else { keys.iter().map(|i| format!("`{i}`")).collect::<Vec<_>>().join(", ") };
        let identifier = &self.closed.identifier;
        let merged = match merge {
            Merge::Found(sha) => format!("{identifier} merged on the default branch as `{sha}`: read what it changed with `git show {sha}`."),
            Merge::Absent => format!("No squash merge for {identifier} was on the default branch when this brief was written, so what it and its comments say was done is all there is."),
            Merge::Unknown => format!("Looking for {identifier}'s merge failed, so find it: `git fetch origin`, then `git log origin/HEAD --grep \"{identifier}\"` (a squash merge's title carries the identifier and the PR number), and read what it changed with `git show <sha>`. If nothing merged, what the issue and its comments say was done is all there is."),
        };
        brief::session_brief(CONTEXT).replace("<MERGED>", &merged).replace("<related>", &list(&self.closed.open_related)).replace("<siblings>", &list(&self.closed.open_siblings)).replace("<started>", self.at).replace("<identifier>", identifier).replace("<repo displayName>", repo).replace("<OWNER NAME>", self.owner_name)
    }
}

// Starts a context session for `start.closed`, once, on its completion's `startContext`.
// Should opening it fail, the claim is given back, so the command's resend can start it
// (a transient failure answers `retryable`, command.rs).
// `tolksters` says whether the issue is Tolkster's. Returns what it came to, for the log.
pub fn start(start: &Start, orca: &mut dyn Orca, write: &mut dyn FnMut(&std::path::Path, &str) -> Result<(), Error>, store: &dyn Store, token: &str, tolksters: impl FnOnce() -> Result<bool, Error>) -> Result<Context, Error> {
    let closed = start.closed;
    let identifier = &closed.identifier;
    let issue = identifier.clone();
    if !closed.completed {
        return Ok(Context::NotCompleted { issue });
    }
    if !closed.has_open() {
        return Ok(Context::NoneOpen { issue });
    }
    if !tolksters()? {
        return Ok(Context::NotMine { issue });
    }
    let found = match Checkouts::list(orca)?.find(start.repos, &closed.place) {
        Ok(found) => found,
        Err(missing) => return Ok(Context::Missing { issue, missing: missing.into() }),
    };
    let repo = RepoName::check(&found.repo)?;
    let path = brief::path(start.briefs_dir, &repo, &format!("{identifier}-context"));
    let reads: Vec<&IssueKey> = std::iter::once(identifier).chain(&closed.open_related).chain(&closed.open_siblings).collect();
    let prompt = reading_prompt(&path, false, &reads)?;
    // The lookup is inside the claim, so a redelivered start fetches nothing.
    let opened = holding(store, &[(context_claim(identifier), None)], token, || {
        let merge = git::merges(orca.shell(), &found.path, &[identifier]).pop().unwrap_or(Merge::Unknown);
        open_session(orca, write, &found.primary, &format!("{identifier} context"), &path, &prompt, &start.render(&repo, &merge))
    })?;
    if opened.is_none() {
        return Ok(Context::Already { issue });
    }
    Ok(Context::Started { issue, issues: closed.open_related.iter().chain(&closed.open_siblings).cloned().collect() })
}

// A `startContext` command's context session, for an issue the relay judged a completed
// one of Tolkster's (src/worker/issue.ts, `contextAsked`). The issue is read fresh, then
// its context session (`start`), deciding from that read whether it still applies; the
// context's claim makes a second one start nothing.
pub fn context_for(config: &Config, ports: &mut Ports, issue_id: &Id, beat: &dyn Fn()) -> Result<Context, Error> {
    let ClosedRead { viewer, issue } = ports.linear.closed(issue_id)?;
    let linear = ports.linear;
    let identifier = IssueKey::parse(&issue.identifier).ok_or_else(|| Error::Failed("the issue's identifier isn't plain".into()))?;
    // As sessions::tolksters_issues decides it: delegated to Tolkster, or with a session the
    // owner started.
    let tolksters = || -> Result<bool, Error> {
        if issue.delegate.as_ref().is_some_and(|d| d.id == viewer.id) {
            return Ok(true);
        }
        Ok(linear.tolksters(std::slice::from_ref(&identifier))?.contains_key(identifier.as_str()))
    };
    beat();
    let closed = parse_closed(&issue)?;
    let at = Time::check(&now_iso())?;
    let from = Start { closed: &closed, owner_name: &config.owner.name, briefs_dir: &config.briefs_dir, repos: &config.repos, at: &at };
    start(&from, ports.orca, &mut write_file, ports.records, &new_token(), tolksters)
}

#[cfg(test)]
mod tests;
