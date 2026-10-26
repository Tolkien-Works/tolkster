// The ports' live implementations (ports.rs), which `listen` carries commands out through and a
// worker's setup reads Linear with: above the workflows, as each calls down into one.

use std::sync::Arc;

use ureq::Agent;

use crate::error::Error;
use crate::issue_state::{self, Started};
use crate::linear::{ActivityInput, with_token};
use crate::plain::{CommandId, Id, IssueKey};
use crate::ports::{Begun, Candidate, CommentFacts, IssueFacts, Linear, Machines, Reaped, TriageQueue, comment_facts};
use crate::post;
use crate::request::{Issue, Session, Thread};
use crate::runtime::Shared;
use crate::sessions::{self, Bodies, ClosedRead, Read, Tolksters};
use crate::state::State;
use crate::triage;

// `seen` is `Progress::ended` as read before the settings the ask decides on.
pub struct Recipes<'a> {
    pub agent: &'a Agent,
    pub shared: &'a Arc<Shared>,
    pub seen: usize,
    pub beat: &'a dyn Fn(),
}

impl Machines for Recipes<'_> {
    fn making(&self) -> bool {
        self.shared.recipes.making()
    }
    fn make(&self, repo: &str, asked: &CommandId) -> Begun {
        crate::recipe::spawn(self.agent, self.shared, repo, asked, self.seen)
    }
    fn wake(&self, name: &str, asked: &CommandId) -> Begun {
        crate::recipe::spawn_wake(self.shared, name, asked, self.seen)
    }
    fn suspend(&self, name: &str, host: &str, rests: &mut dyn FnMut() -> Result<bool, Error>) -> Result<bool, Error> {
        crate::recipe::sleep::suspend(self.shared, name, host, rests)
    }
    fn retire(&self, name: &str, reaped: &mut Reaped) -> Result<bool, Error> {
        crate::recipe::retire::retire(self.agent, self.shared, name, self.seen, reaped, self.beat)
    }
}

// Linear as the app, with Tolkster's records the sessions' reads carry, and the claims a
// reply is posted under.
pub struct LinearApp<'a> {
    pub agent: &'a Agent,
    pub state: &'a State<'a>,
    pub owner_id: &'a str,
    pub started: &'a Started,
}

// What `listen` reads of a session: its issue, with no prompts, which nothing it decides reads.
const LISTEN_READ: Read = Read::Full { activities: 0, bodies: Bodies::Skip };

impl Linear for LinearApp<'_> {
    fn session(&self, session_id: &Id) -> Result<Session, Error> {
        with_token(self.agent, |token| sessions::read(self.agent, token, None, session_id, LISTEN_READ))
    }
    fn session_with_record(&self, session_id: &Id) -> Result<Session, Error> {
        with_token(self.agent, |token| sessions::read(self.agent, token, Some(self.state), session_id, LISTEN_READ))
    }
    fn session_issue(&self, session_id: &Id) -> Result<Option<String>, Error> {
        let session = with_token(self.agent, |token| sessions::read(self.agent, token, None, session_id, Read::Owner))?;
        Ok(session.issue.map(|issue| issue.id))
    }
    fn issue(&self, identifier: &IssueKey) -> Result<IssueFacts, Error> {
        let (issue, updated_at, sessions, viewer_id) = sessions::read_issue_facts(self.agent, identifier)?;
        Ok(IssueFacts { issue, updated_at, sessions, viewer_id })
    }
    fn comment(&self, comment_id: &Id) -> Result<Option<CommentFacts>, Error> {
        Ok(comment_facts(&sessions::read_comment(self.agent, comment_id, Some(self.owner_id))?, self.owner_id))
    }
    fn thread(&self, root: &Id) -> Result<Result<Thread, &'static str>, Error> {
        sessions::read_thread(self.agent, root, self.owner_id, Bodies::Skip)
    }
    fn identifiers(&self, issue_ids: &[Id]) -> Result<Vec<String>, Error> {
        sessions::identifiers(self.agent, issue_ids)
    }
    fn manual_merge(&self, issue_id: &Id) -> Result<bool, Error> {
        sessions::manual_merge(self.agent, issue_id)
    }
    fn description_edits(&self, issue_id: &Id) -> Result<Vec<String>, Error> {
        sessions::description_edits(self.agent, issue_id)
    }
    fn post_once(&self, key: &str, input: ActivityInput) -> Result<(), Error> {
        post::post_once(self.agent, key, input)
    }
    fn send(&self, input: &ActivityInput) -> Result<(), Error> {
        post::send(self.agent, input).map(|_| ())
    }
    fn reply_once(&self, issue_id: &Id, root: &Id, body: &str, key: &str) -> Result<bool, Error> {
        post::reply_once(self.agent, issue_id, root, body, key).map(|reply| reply.is_some())
    }
    fn tolksters(&self, identifiers: &[IssueKey]) -> Result<Tolksters, Error> {
        sessions::tolksters_issues(self.agent, identifiers, self.owner_id)
    }
    fn closed(&self, issue_id: &Id) -> Result<ClosedRead, Error> {
        sessions::read_closed(self.agent, issue_id)
    }
    fn triage_queue(&self) -> Result<TriageQueue, Error> {
        let (nodes, viewer_id) = sessions::read_triage_queue(self.agent)?;
        // One whose identifier isn't plain is no candidate.
        Ok(TriageQueue { candidates: nodes.iter().filter_map(|n| triage::parse_candidate(n).ok()).collect(), viewer_id })
    }
    fn issues_closed(&self, issue_ids: &[Id]) -> Result<Vec<(Id, Candidate)>, Error> {
        let nodes = sessions::read_triage_issues(self.agent, issue_ids)?;
        Ok(nodes.iter().filter_map(|n| Some((Id::parse(&n.id)?, triage::parse_candidate(&n.issue).ok()?))).collect())
    }
    fn state_histories(&self, identifiers: &[IssueKey]) -> Result<Vec<bool>, Error> {
        Ok(sessions::state_histories(self.agent, identifiers)?.iter().map(triage::filed_in_triage).collect())
    }
    fn start_issue(&self, issue: &Issue) -> Result<(), Error> {
        with_token(self.agent, |token| issue_state::start_issue(self.agent, token, self.started, issue)).map(|_| ())
    }
    fn release_issue(&self, issue_id: &Id) -> Result<(), Error> {
        issue_state::release_issue(self.agent, issue_id)
    }
}
