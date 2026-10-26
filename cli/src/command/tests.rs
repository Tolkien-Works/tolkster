use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use super::*;
use super::pr::{Open, pr_read};
use super::read::{Ids, issue_read};
use crate::linear::{ActivityType, StateType};
use crate::machine::Notice;
use crate::orca::{Orca, Worktree};
use crate::github::{Head, Pr, PrState};
use crate::review::Checked;
use crate::post::answer_key;
use crate::voice;
use super::notice::post_key;
use super::workers::worktree_on_issue;
use crate::github::Reader;
use crate::ports::fake::FakeReader;
use crate::machine::{Kind, NOTICES, Outcome, Resume, parse_command};
use crate::orca::Pane;
use crate::ports::{CommentFacts, CommentKind};
use crate::ports::fake::{self, World};
use crate::request::tests::{APP, OWNER, at, issue, mentioned, session};
use crate::machine::ReapRule as Rule;
use crate::request::{Blocker, Delegated, Issue, Parent, Session};

fn member(identifier: &str, state: StateType, minute: u32) -> Blocker {
    Blocker { identifier: identifier.into(), url: format!("https://linear.app/x/issue/{identifier}"), state_type: state, id: Some(format!("id-{}", identifier.len())), updated_at: Some(at(minute)) }
}

const HEAD_SHA: &str = "2878d3d0c7b1f9e4a5d6c3b2a1f0e9d8c7b6a5f4";
const BASE_SHA: &str = "ba5e1111111111111111111111111111111111aa";

fn command(id: &str, identifier: &str) -> Command {
    let line = json!({ "type": "command", "id": id, "kind": "readIssue", "issueId": "i1", "identifier": identifier, "version": 1, "urgent": false, "args": { "sessionId": "s-1" } });
    crate::machine::parse_command(&line).unwrap().unwrap()
}

// Scenarios carried out through the fakes (ports/fake.rs) alone: TW-1 (`i1`), its session
// `s1`, in the repo Tolkster.

fn order(id: &str, order: Order) -> Command {
    let kind = match &order {
        Order::StartWorker { .. } => Kind::StartWorker,
        Order::StopWorker { .. } => Kind::StopWorker,
        Order::ResumeWorker { .. } => Kind::ResumeWorker,
        Order::StartRound { .. } => Kind::StartRound,
        Order::StartTriage { .. } => Kind::StartTriage,
        Order::UpdateBranch { .. } => Kind::UpdateBranch,
        Order::MergePr { .. } => Kind::MergePr,
        _ => Kind::DeliverMessage,
    };
    Command { id: id.into(), kind, issue_id: "i1".into(), identifier: "TW-1".into(), version: 1, urgent: false, order: Ok(order), queued: None }
}

fn start() -> Command {
    order("c-start", Order::StartWorker { session_id: "s1".into(), orchestrate: false, worktree_id: None, host: None })
}

// None: a stop as the machine sends it before the start's result has named the worktree.
fn stop(id: &str, worktree_id: Option<&str>) -> Command {
    order(id, Order::StopWorker { worktree_id: worktree_id.map(String::from), session_id: Some("s1".into()) })
}

// Each test's briefs in a directory of its own, as tests run at once.
fn briefs(test: &str) -> String {
    format!("{}/target/scenarios/{test}/<repo>-briefs", env!("CARGO_MANIFEST_DIR"))
}

fn carried(config: &Config, linear: &fake::Linear, records: &fake::Records, orca: &mut fake::Orca, command: &Command) -> Result<Finished, Error> {
    let mut github = FakeReader::new(|_, _| Err(Error::Failed("GitHub isn't there".into())));
    carried_with(config, linear, records, orca, &mut github, command)
}

fn carried_with(config: &Config, linear: &fake::Linear, records: &fake::Records, orca: &mut fake::Orca, github: &mut dyn Reader, command: &Command) -> Result<Finished, Error> {
    let machines = fake::Machines(Rc::clone(&orca.world));
    let shared = Arc::clone(&orca.world.borrow().shared);
    let mut ports = Ports { linear, records, orca, github, machines: &machines, shared: &shared, watch: &|_| {} };
    carry(config, &mut ports, command, command.order.as_ref().unwrap(), &|| {})
}

const WORKTREE: &str = "r1::/wt/tw-1";

// Linear with the session `s1`, the records Linear reads its conversation and claims from,
// and Orca with the repo.
fn fakes() -> (fake::Linear, Rc<fake::Records>, Rc<RefCell<World>>) {
    let linear = fake::Linear::with(vec![session(Vec::new())]);
    let records = Rc::clone(&linear.records);
    (linear, records, World::tolkster())
}

// The listings a command asked of Orca, in order: each is a spawn. A command reads each listing
// once and passes it down, except `remove`'s deliberate re-check of the terminals.
fn spawned(world: &Rc<RefCell<World>>, run: impl FnOnce()) -> Vec<&'static str> {
    world.borrow_mut().spawns.clear();
    run();
    std::mem::take(&mut world.borrow_mut().spawns)
}

const TIP: &str = "abc1234000000000000000000000000000000000";
const CONVERSATIONS: &str = "r1::/wt/tolkster-conversations";

// Orca's repo, where a conversations checkout made anew is a git checkout on Orca's branch
// of its name, behind origin's tip.
fn conversing_world() -> Rc<RefCell<World>> {
    let world = World::tolkster();
    {
        let mut w = world.borrow_mut();
        w.tip = TIP.into();
        w.git.insert("/wt/tolkster-conversations".into(), fake::Git { head: BASE_SHA.into(), branch: Some("tolkster-conversations".into()), branches: vec!["tolkster-conversations".into()], ..fake::Git::default() });
    }
    world
}


fn turn(id: &str, session_id: &str, thread: bool, request: Option<&str>) -> Command {
    Command { kind: Kind::StartTurn, ..order(id, Order::StartTurn { session_id: session_id.into(), thread, request: request.map(Into::into), after: None }) }
}

mod hosts;
mod reads;
mod reaps;
mod replays;
mod triage;
mod turns;
mod workers;
