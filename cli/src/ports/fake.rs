// In-memory Linear, Orca and records (ports.rs), for scenario tests of the command path with
// no network, Orca or Durable Object. Each models what the code relies on: Linear refuses a
// post's ID twice and answers a spent budget with `Error::RateLimited`, as the gateway does
// (gateway.rs); Orca's agents work, wait at a prompt or are done, a typed line's receipt
// says whether it shows the send's proof (`Orca::send`, after its own one resend), and a closed
// terminal is gone; a claim is held by one token at a time; git answers in a checkout given
// one (`Git`), and fails anywhere else. Linear's pages are the adapter's (sessions.rs): the
// facts come here whole. A claim's TTL isn't modelled, so none lapses. Each fake keeps what
// was done to it, to assert on.

mod orca;
pub use orca::*;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use serde_json::Value;

use crate::linear::{ActivityContent, ActivityInput, StateType};
use crate::config::{Config, Owner, Review, Triage};
use crate::error::Error;
use crate::estimates::Size;
use crate::github::Reader;
use crate::config::ladder::Ladder;
use crate::plain::{CommandId, Id, IssueKey, Sha};
use crate::ports::{Candidate, CommentFacts, IssueFacts, Linear as LinearPort, TriageQueue};
use crate::config::repo_map::RepoMap;
use crate::request::tests::{APP, OWNER};
use crate::request::{Issue, Session, Thread};
use crate::sessions::{ClosedRead, Tolksters};
use crate::state::{Change, CheckoutRecord, ContextComment, Conversation, Delegation, Record, Records as RecordsPort, Store, WorkerRecord};

// The settings a command is carried out with. Briefs go to `briefs_dir`.
pub fn config(briefs_dir: &str) -> Config {
    Config {
        owner: Owner { id: OWNER.into(), name: "the owner".into(), co_author: None },
        review: Review { reviewers: Vec::new(), max_rounds: 3, deep_at: Size::M },
        triage: Triage { batch: 5 },
        briefs_dir: briefs_dir.into(),
        slots: [(crate::config::LOCAL.to_string(), 6)].into(),
        timing: true,
        release_pipeline: None,
        host: None,
        machines: Default::default(),
        enrolled: Vec::new(),
        revisions: std::collections::BTreeMap::new(),
        ladder: Ladder::default(),
        repos: RepoMap::default(),
    }
}

// One post, as Linear keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct Post {
    pub session_id: String,
    // `thought`, `response` and the like, or `action`.
    pub kind: String,
    // A text's body, or an action's name.
    pub body: String,
    pub ephemeral: bool,
    // An elicitation's select options, by name.
    pub options: Vec<&'static str>,
}

// A session's record of its conversation is read from `records`, as `LinearApp` reads it
// from Tolkster's state, and a reply's claim is taken there.
#[derive(Default)]
pub struct Linear {
    pub records: Rc<Records>,
    pub sessions: RefCell<HashMap<String, Session>>,
    pub comments: RefCell<HashMap<String, CommentFacts>>,
    pub threads: RefCell<HashMap<String, Thread>>,
    // Issues Linear has beside those its sessions are on.
    pub issues: RefCell<Vec<Issue>>,
    // Issues labelled manual-merge, by ID.
    pub manual_merge: RefCell<HashSet<String>>,
    pub posts: RefCell<Vec<Post>>,
    // The replies posted in threads: their first claim, root and body, in the order posted.
    pub replies: RefCell<Vec<(String, String, String)>>,
    // The issues that are Tolkster's, by identifier, as reap reads them.
    pub tolksters: RefCell<Tolksters>,
    // Completed issues as their context reads them, by ID.
    pub closed: RefCell<HashMap<String, ClosedRead>>,
    pub started: RefCell<Vec<String>>,
    pub released: RefCell<Vec<String>>,
    // Every issue triage may read, by ID: the queue is those in Triage and not archived.
    pub triage_issues: RefCell<Vec<(Id, Candidate)>>,
    // The issues, by identifier, whose history shows them moved into Triage, not filed there.
    pub moved: RefCell<HashSet<String>>,
    // The issues, by identifier, whose history Linear can't read: a read naming one fails whole.
    pub unreadable: RefCell<HashSet<String>>,
    // What triage read, in order: "queue", "issues" and "histories", each one read.
    pub triage_reads: RefCell<Vec<&'static str>>,
    // How many of the next calls find the budget spent.
    pub spent: Cell<u32>,
    ids: RefCell<HashSet<String>>,
}

impl Linear {
    pub fn with(sessions: Vec<Session>) -> Linear {
        Linear { sessions: RefCell::new(sessions.into_iter().map(|s| (s.id.clone(), s)).collect()), ..Linear::default() }
    }

    pub fn posted(&self, kind: &str) -> Vec<Post> {
        self.posts.borrow().iter().filter(|p| p.kind == kind).cloned().collect()
    }

    fn budget(&self) -> Result<(), Error> {
        match self.spent.get() {
            0 => Ok(()),
            left => {
                self.spent.set(left - 1);
                Err(Error::RateLimited { message: "Linear's rate limit is spent".into(), wait: Duration::from_secs(60) })
            }
        }
    }

    fn keep(&self, input: &ActivityInput) {
        let (kind, body) = match &input.content {
            ActivityContent::Text { kind, body } => (kind.as_str().to_string(), body.clone()),
            ActivityContent::Action { action, .. } => ("action".to_string(), action.clone()),
        };
        self.posts.borrow_mut().push(Post { session_id: input.agent_session_id.clone(), kind, body, ephemeral: input.ephemeral, options: input.options.iter().map(|c| c.name).collect() });
    }

    fn issue_of(&self, identifier: &str) -> Option<Issue> {
        let sessions = self.sessions.borrow();
        sessions.values().filter_map(|s| s.issue.clone()).chain(self.issues.borrow().iter().cloned()).find(|i| i.id == identifier || i.identifier.eq_ignore_ascii_case(identifier))
    }
}

impl LinearPort for Linear {
    fn session(&self, session_id: &Id) -> Result<Session, Error> {
        self.budget()?;
        self.sessions.borrow().get(session_id.as_str()).cloned().ok_or_else(|| Error::Failed(format!("no session {session_id}")))
    }
    fn session_with_record(&self, session_id: &Id) -> Result<Session, Error> {
        let mut session = self.session(session_id)?;
        if let Some(conversation) = self.records.conversations.borrow().get(session_id.as_str()) {
            session.record.conversation = Some(conversation.clone());
        }
        Ok(session)
    }
    fn session_issue(&self, session_id: &Id) -> Result<Option<String>, Error> {
        Ok(self.session(session_id)?.issue.map(|issue| issue.id))
    }
    fn issue(&self, identifier: &IssueKey) -> Result<IssueFacts, Error> {
        self.budget()?;
        let issue = self.issue_of(identifier).ok_or_else(|| Error::Failed(format!("{identifier}: no such issue")))?;
        let sessions = self.sessions.borrow().values().filter(|s| s.issue.as_ref().is_some_and(|i| i.id == issue.id)).cloned().collect();
        Ok(IssueFacts { updated_at: issue.created_at.clone(), issue, sessions, viewer_id: APP.into() })
    }
    fn comment(&self, comment_id: &Id) -> Result<Option<CommentFacts>, Error> {
        self.budget()?;
        Ok(self.comments.borrow().get(comment_id.as_str()).cloned())
    }
    fn thread(&self, root: &Id) -> Result<Result<Thread, &'static str>, Error> {
        self.budget()?;
        Ok(self.threads.borrow().get(root.as_str()).cloned().ok_or("no note's thread"))
    }
    fn identifiers(&self, issue_ids: &[Id]) -> Result<Vec<String>, Error> {
        self.budget()?;
        let sessions = self.sessions.borrow();
        Ok(issue_ids.iter().filter_map(|id| sessions.values().filter_map(|s| s.issue.as_ref()).find(|i| i.id == id.as_str()).map(|i| i.identifier.clone())).collect())
    }
    fn manual_merge(&self, issue_id: &Id) -> Result<bool, Error> {
        self.budget()?;
        Ok(self.manual_merge.borrow().contains(issue_id.as_str()))
    }
    fn description_edits(&self, _issue_id: &Id) -> Result<Vec<String>, Error> {
        self.budget()?;
        Ok(Vec::new())
    }
    // Linear refuses an activity ID it has, which `post::post_once` takes as posted.
    fn post_once(&self, key: &str, input: ActivityInput) -> Result<(), Error> {
        self.budget()?;
        if self.ids.borrow_mut().insert(crate::state::derived_uuid(key)) {
            self.keep(&input);
        }
        Ok(())
    }
    fn send(&self, input: &ActivityInput) -> Result<(), Error> {
        self.budget()?;
        self.keep(input);
        Ok(())
    }
    // Linear refuses a comment ID it has, which `post::reply_once` takes as posted.
    fn reply_once(&self, _issue_id: &Id, root: &Id, body: &str, key: &str) -> Result<bool, Error> {
        self.budget()?;
        if !self.ids.borrow_mut().insert(crate::state::derived_uuid(key)) {
            return Ok(false);
        }
        self.replies.borrow_mut().push((key.into(), root.to_string(), body.into()));
        Ok(true)
    }
    fn tolksters(&self, identifiers: &[IssueKey]) -> Result<Tolksters, Error> {
        self.budget()?;
        Ok(self.tolksters.borrow().iter().filter(|(identifier, _)| identifiers.iter().any(|i| i == *identifier)).map(|(i, now)| (i.clone(), now.clone())).collect())
    }
    fn closed(&self, issue_id: &Id) -> Result<ClosedRead, Error> {
        self.budget()?;
        self.closed.borrow().get(issue_id.as_str()).cloned().ok_or_else(|| Error::Failed(format!("{issue_id}: no such issue")))
    }
    fn triage_queue(&self) -> Result<TriageQueue, Error> {
        self.budget()?;
        self.triage_reads.borrow_mut().push("queue");
        let candidates = self.triage_issues.borrow().iter().map(|(_, c)| c).filter(|c| c.state_type == StateType::Triage && !c.archived).cloned().collect();
        Ok(TriageQueue { candidates, viewer_id: APP.into() })
    }
    fn issues_closed(&self, issue_ids: &[Id]) -> Result<Vec<(Id, Candidate)>, Error> {
        self.budget()?;
        self.triage_reads.borrow_mut().push("issues");
        Ok(self.triage_issues.borrow().iter().filter(|(id, _)| issue_ids.contains(id)).cloned().collect())
    }
    fn state_histories(&self, identifiers: &[IssueKey]) -> Result<Vec<bool>, Error> {
        self.budget()?;
        self.triage_reads.borrow_mut().push("histories");
        if identifiers.iter().any(|i| self.unreadable.borrow().contains(i.as_str())) {
            return Err(Error::Failed("Linear had no history".into()));
        }
        Ok(identifiers.iter().map(|i| !self.moved.borrow().contains(i.as_str())).collect())
    }
    fn start_issue(&self, issue: &Issue) -> Result<(), Error> {
        self.budget()?;
        self.started.borrow_mut().push(issue.id.clone());
        Ok(())
    }
    fn release_issue(&self, issue_id: &Id) -> Result<(), Error> {
        self.budget()?;
        self.released.borrow_mut().push(issue_id.to_string());
        Ok(())
    }
}

// As the Durable Object keeps them (src/worker/state.ts): a worktree's worker record by its
// worktree.
#[derive(Default)]
pub struct Records {
    pub workers: RefCell<HashMap<String, WorkerRecord>>,
    // The hosts whose credential is gone since `listen` last listed: every other is admitted.
    pub revoked: RefCell<Vec<String>>,
    pub conversations: RefCell<HashMap<String, Conversation>>,
    pub checkouts: RefCell<Vec<CheckoutRecord>>,
    pub delegations: RefCell<HashMap<String, Delegation>>,
    pub context_comments: RefCell<HashMap<String, ContextComment>>,
    // Each claim held, by name: its token.
    pub claims: RefCell<HashMap<String, String>>,
}

impl Store for Records {
    fn take(&self, claims: &[(String, Option<Duration>)], token: &str) -> Result<bool, Error> {
        let mut held = self.claims.borrow_mut();
        if claims.iter().any(|(name, _)| held.get(name).is_some_and(|holder| holder != token)) {
            return Ok(false);
        }
        held.extend(claims.iter().map(|(name, _)| (name.clone(), token.to_string())));
        Ok(true)
    }
    fn release(&self, names: &[String], token: &str) -> Result<(), Error> {
        self.claims.borrow_mut().retain(|name, holder| !(names.contains(name) && holder == token));
        Ok(())
    }
    fn record(&self, _session_id: &str, _change: &Change) -> Result<Record, Error> {
        Err(Error::Failed("a session's record isn't modelled".into()))
    }
}

impl RecordsPort for Records {
    fn start_worker(&self, worker: &WorkerRecord) -> Result<(), Error> {
        self.workers.borrow_mut().insert(worker.worktree_id.clone(), worker.clone());
        Ok(())
    }
    fn workers(&self) -> Result<Vec<WorkerRecord>, Error> {
        Ok(self.workers.borrow().values().cloned().collect())
    }
    fn admitted(&self, host: &str) -> Result<bool, Error> {
        Ok(!self.revoked.borrow().iter().any(|id| id == host))
    }
    fn delegations(&self, issues: &[Id]) -> Result<HashMap<String, Delegation>, Error> {
        Ok(self.delegations.borrow().iter().filter(|(id, _)| issues.iter().any(|i| i.as_str() == id.as_str())).map(|(id, d)| (id.clone(), d.clone())).collect())
    }
    fn converse(&self, session_id: &str, conversation: &Conversation) -> Result<(), Error> {
        self.conversations.borrow_mut().insert(session_id.into(), conversation.clone());
        Ok(())
    }
    fn conversation(&self, session_id: &str) -> Result<Option<Conversation>, Error> {
        Ok(self.conversations.borrow().get(session_id).cloned())
    }
    fn checkouts(&self) -> Result<Vec<CheckoutRecord>, Error> {
        Ok(self.checkouts.borrow().clone())
    }
    fn set_checkout(&self, checkout: &CheckoutRecord) -> Result<(), Error> {
        let mut checkouts = self.checkouts.borrow_mut();
        checkouts.retain(|c| c.repo_id != checkout.repo_id);
        checkouts.push(checkout.clone());
        Ok(())
    }
    fn forget_worker(&self, worktree_id: &str) -> Result<(), Error> {
        self.workers.borrow_mut().remove(worktree_id);
        Ok(())
    }
    fn context_comment(&self, issue_id: &str) -> Result<Option<ContextComment>, Error> {
        Ok(self.context_comments.borrow().get(issue_id).cloned())
    }
    fn set_context_comment(&self, comment: &ContextComment) -> Result<(), Error> {
        self.context_comments.borrow_mut().insert(comment.issue_id.clone(), comment.clone());
        Ok(())
    }
}

// The recipe machines `listen` makes, in the World of the Orca they would pair with.
pub struct Machines(pub Rc<RefCell<World>>);

impl crate::ports::Machines for Machines {
    fn making(&self) -> bool {
        self.0.borrow().making
    }
    fn make(&self, repo: &str, _: &CommandId) -> crate::ports::Begun {
        let mut w = self.0.borrow_mut();
        if let Some(why) = w.declined.clone() {
            return Err(why);
        }
        w.made_for.push(repo.into());
        Ok(())
    }
    fn wake(&self, name: &str, _: &CommandId) -> crate::ports::Begun {
        let mut w = self.0.borrow_mut();
        if let Some(why) = w.declined.clone() {
            return Err(why);
        }
        w.woken.push(name.into());
        Ok(())
    }
    fn suspend(&self, name: &str, _: &str, rests: &mut dyn FnMut() -> Result<bool, Error>) -> Result<bool, Error> {
        let slept = rests()?;
        if slept {
            self.0.borrow_mut().asleep.push(name.into());
        }
        Ok(slept)
    }
    fn update(&self, name: &str) -> Result<bool, Error> {
        self.0.borrow_mut().updated.push(name.into());
        Ok(true)
    }
    // On the host named as its ID, as Orca's hosts are here, which answers once woken.
    fn retire(&self, name: &str, reaped: &mut crate::ports::Reaped) -> Result<bool, Error> {
        self.0.borrow_mut().unanswered.retain(|host| host != name);
        let gone = reaped(name, true)?;
        if gone {
            self.0.borrow_mut().destroyed.push(name.into());
        }
        Ok(gone)
    }
}

type Answer<'a> = Box<dyn FnMut(&str, &Value) -> Result<Value, Error> + 'a>;

// A fake `Reader` for tests: it records each read, as `graphql <repo> <variables>` less
// the owner and name, or `get <repo> <path>`, and answers with `answer`.
pub struct FakeReader<'a> {
    pub calls: Vec<String>,
    // Each GraphQL call's query.
    pub queries: Vec<String>,
    pub answer: Answer<'a>,
}

impl<'a> FakeReader<'a> {
    pub fn new(answer: impl FnMut(&str, &Value) -> Result<Value, Error> + 'a) -> FakeReader<'a> {
        FakeReader { calls: Vec::new(), queries: Vec::new(), answer: Box::new(answer) }
    }

    // Answers every read with `value`.
    pub fn always(value: Value) -> FakeReader<'a> {
        FakeReader::new(move |_, _| Ok(value.clone()))
    }
}

impl Reader for FakeReader<'_> {
    fn graphql(&mut self, repo: &str, query: &str, mut variables: Value) -> Result<Value, Error> {
        self.queries.push(query.to_string());
        if let Some(all) = variables.as_object_mut() {
            all.remove("owner");
            all.remove("name");
        }
        self.calls.push(format!("graphql {repo} {variables}"));
        (self.answer)("graphql", &variables)
    }

    fn update_branch(&mut self, repo: &str, pr: &str, head: &Sha) -> Result<Value, Error> {
        self.calls.push(format!("update_branch {repo} {pr} {head}"));
        (self.answer)("update_branch", &serde_json::json!({ "pr": pr, "head": head }))
    }

    fn merge_pr(&mut self, repo: &str, pr: &str, head: &Sha) -> Result<Value, Error> {
        self.calls.push(format!("merge_pr {repo} {pr} {head}"));
        (self.answer)("merge_pr", &serde_json::json!({ "pr": pr, "head": head }))
    }

    fn get(&mut self, repo: &str, path: &str) -> Result<Value, Error> {
        self.calls.push(format!("get {repo} {path}"));
        (self.answer)(path, &Value::Null)
    }

    fn app(&mut self) -> Result<crate::github::AppRecord, Error> {
        self.calls.push("app".into());
        crate::json::decode((self.answer)("app", &Value::Null)?, "the Worker's answer to githubApp")
    }
}
