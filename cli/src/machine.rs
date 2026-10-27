// The wire between the Durable Object's machines (docs/state-machine.md) and `listen`: a
// command as the relay sends it, and the reports `listen` and a worker's `tolkster` send
// back, which the machines take as events. Beside them, the relay sends each repo's base
// tip, level state that nothing answers (`parse_base`).
// test/contract.json's `commands`, `reports` and `baseTip` hold one of each,
// which src/worker/machine.ts and realtime.ts read the same. A command carries IDs, enums,
// numbers and times only, and every one that reaches a terminal, a command line or a post
// has a plain shape, checked here (plain.rs).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::config::Config;
use crate::config::repo_map::Entry as RepoEntry;
use crate::error::{Error, Waits};
use crate::hosts::{Fleet, MAX_SLOTS};
use crate::gateway::Failure;
use crate::plain::split_repo;
use crate::json::{non_empty, str};
use crate::plain::{BranchName, CommandId, Id, IssueKey, Sha, Terminal, Time, plain_host};
// Defined in trace, the lower layer, which logs them.
pub use crate::trace::{Answer as Outcome, Waiting};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    ReadIssue,
    StartWorker,
    DeliverMessage,
    ReadComment,
    ResumeWorker,
    StartRound,
    StopWorker,
    StartTurn,
    StopTurn,
    EvaluatePr,
    Observe,
    CloseAgent,
    ReapWorktree,
    PostNotice,
    TakeBackChain,
    StartContext,
    StartTriage,
    ObserveTriage,
    UpdateBranch,
    MergePr,
    MakeHost,
    WakeHost,
    SuspendHost,
    UpdateHost,
    DestroyHost,
}

// What the machine decided a reap on, which `reap::Rule` takes: PR `number` merged at
// `head`, the work landed, or the issue cancelled, `no_pr` its store holding no PR for it, or
// completed.
#[derive(Clone, Debug, PartialEq)]
pub enum ReapRule {
    Merged { number: u64, head: Sha },
    Landed,
    Cancelled { no_pr: bool },
    Completed,
}

// A repo's default branch at commit `after`, as the relay last heard it pushed.
#[derive(Clone, Debug, PartialEq)]
pub struct BaseTip {
    pub repo: String,
    pub reference: String,
    pub after: Sha,
}

// The issue and identifier the triage machine's commands carry, which no issue's are: they
// run in a lane of their own (src/worker/machine/triage.ts, TRIAGE).
pub const TRIAGE: &str = "triage";
// Likewise the fleet's (src/worker/machine/fleet.ts, FLEET).
pub const FLEET: &str = "fleet";

// Every command but `postAway`, which the Durable Object's alarm posts itself.
#[cfg(test)]
pub const KINDS: [Kind; 25] = [
    Kind::ReadIssue,
    Kind::StartWorker,
    Kind::DeliverMessage,
    Kind::ReadComment,
    Kind::ResumeWorker,
    Kind::StartRound,
    Kind::StopWorker,
    Kind::StartTurn,
    Kind::StopTurn,
    Kind::EvaluatePr,
    Kind::Observe,
    Kind::CloseAgent,
    Kind::ReapWorktree,
    Kind::PostNotice,
    Kind::TakeBackChain,
    Kind::StartContext,
    Kind::StartTriage,
    Kind::ObserveTriage,
    Kind::UpdateBranch,
    Kind::MergePr,
    Kind::MakeHost,
    Kind::WakeHost,
    Kind::SuspendHost,
    Kind::UpdateHost,
    Kind::DestroyHost,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Notice {
    Blocked,
    Busy,
    Held,
    NotDelegated,
    NoLongerMine,
    Cancelled,
    AlreadyDone,
    NoRepo,
    Stopped,
    Stalled,
    Paused,
    HeldAtPrompt,
    CouldntPass,
    RoundsCap,
    PrClosed,
    AutoMergeOff,
    CiCouldntRun,
    ChecksPending,
    CopilotWontApprove,
    StopUnnamed,
    RoundUnnamed,
    Unheard,
}

#[cfg(test)]
pub const NOTICES: [Notice; 22] = [
    Notice::Blocked,
    Notice::Busy,
    Notice::Held,
    Notice::NotDelegated,
    Notice::NoLongerMine,
    Notice::Cancelled,
    Notice::AlreadyDone,
    Notice::NoRepo,
    Notice::Stopped,
    Notice::Stalled,
    Notice::Paused,
    Notice::HeldAtPrompt,
    Notice::CouldntPass,
    Notice::RoundsCap,
    Notice::PrClosed,
    Notice::AutoMergeOff,
    Notice::CiCouldntRun,
    Notice::ChecksPending,
    Notice::CopilotWontApprove,
    Notice::StopUnnamed,
    Notice::RoundUnnamed,
    Notice::Unheard,
];

// What `listen` sees of a worktree in Orca. A turn's end is `turnEnded`, by its session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Observed {
    AgentWorking,
    // Idle at its prompt while a call it sent to the background runs: only its hook says so.
    AgentBackground,
    AgentIdle,
    AgentWaiting,
    AgentExited,
    WorktreeGone,
}

#[cfg(test)]
pub const OBSERVED: [Observed; 5] = [Observed::AgentWorking, Observed::AgentIdle, Observed::AgentWaiting, Observed::AgentExited, Observed::WorktreeGone];

// Why a worker is resumed: a message in its session, or, for a parent working through its
// sub-issues (docs/parent-issues.md), their end or one added since it chained them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Resume {
    Message,
    ChildrenEnd,
    ChildrenAdded,
}

// `thread`: the request is a note's thread's, whose root comment stands for `session_id`
// (docs/state-machine.md, The machines): its turn and its notices are the thread's.
#[derive(Clone, Debug, PartialEq)]
pub enum Order {
    ReadIssue,
    // `orchestrate`: the worker works through the issue's sub-issues.
    // `worktree_id`: the worktree the machine holds, in which a start that never began is
    // launched again (executor::start); None names none. `host`: the paired host the Durable
    // Object granted its slot on, None for this machine.
    StartWorker { session_id: Id, orchestrate: bool, worktree_id: Option<String>, host: Option<String> },
    // `session_id` None, for the owner's comment, when no session has given the worker a
    // request: the line names the issue, not a session.
    DeliverMessage { worktree_id: String, session_id: Option<Id>, comment: bool },
    ReadComment { comment_id: Id },
    // `why`: what the fixed line that brings it back says.
    ResumeWorker { session_id: Id, why: Resume },
    // `head` and `round`, the PR's head and the round's number, which says whether it is the
    // last, `session_id`, the session the worker reports in, and `rebase_only`, a round for a
    // rebase alone, which has a brief of its own.
    StartRound { repo: String, number: u64, head: Option<Sha>, round: Option<u32>, session_id: Option<Id>, rebase_only: bool },
    // `worktree_id` None: no worktree is recorded, so there is no terminal to close.
    // `session_id` None: no session gave the worker a request, so "Stopped." is said nowhere.
    StopWorker { worktree_id: Option<String>, session_id: Option<Id> },
    // `request`: the time of the request decided, which a note's thread's turn takes replies
    // up to. `after`: the newest request a reader was shown, which a turn reads after.
    StartTurn { session_id: Id, thread: bool, request: Option<Time>, after: Option<Time> },
    // `idle`: a warm turn's close, which reports one already gone ended.
    StopTurn { session_id: Id, thread: bool, idle: bool },
    EvaluatePr { repo: String, number: u64 },
    Observe { worktree_ids: Vec<String>, session_id: Option<Id>, thread: bool },
    CloseAgent { worktree_id: String },
    ReapWorktree { worktree_id: String, rule: ReapRule },
    // `blockers`, their issue IDs, for `blocked`, `pr` for a PR's notice and `rounds` for
    // `roundsCap`. `through`, for `stalled`, the time of the request the turn was for.
    // `session_id` None: no session gave the worker a request, so the notice is said nowhere.
    PostNotice { session_id: Option<Id>, thread: bool, notice: Notice, blockers: Vec<Id>, pr: Option<u64>, rounds: Option<u32>, through: Option<Time> },
    // The sub-issues' IDs the parent's chain delegated.
    TakeBackChain { children: Vec<Id> },
    // The command's issue, completed: its context session, if it still applies.
    StartContext,
    // The issues created in Triage since the last start, by ID, read fresh beside the queue.
    StartTriage { issue_ids: Vec<Id> },
    // The running triage session, by its id, and its terminal.
    ObserveTriage { session_id: Id, terminal: Terminal },
    // `head`: the one the machine read the PR at, GitHub's lease on the update.
    UpdateBranch { repo: String, number: u64, head: Sha },
    // `head`: the one the machine read the PR mergeable at, which GitHub merges only while it holds.
    MergePr { repo: String, number: u64, head: Sha },
    // A machine for the repo, by its name in config.json, from its recipe.
    MakeHost { repo: String },
    // The recipe machine to wake, by its name in machines.json.
    WakeHost { name: String },
    // The recipe machine to put to sleep, by the ID Orca pairs it under.
    SuspendHost { host: String },
    // The asleep recipe machine to move onto its recipe's image, by its name in machines.json.
    UpdateHost { name: String },
    // The recipe machine to destroy, by its name in machines.json.
    DestroyHost { name: String },
}

// `id` is the command's, the same on every resend. `urgent` is a class 1 event's command,
// run beside the one in flight on the issue (docs/state-machine.md, Precedence). `order`
// is Err with why its `args` didn't read, which ends it `failed`.
#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub id: CommandId,
    pub kind: Kind,
    pub issue_id: Id,
    pub identifier: IssueKey,
    pub version: u64,
    pub urgent: bool,
    pub order: Result<Order, String>,
    pub queued: Option<Queued>,
}

// What a queued request waits for, when the command was sent for one (src/worker/route.ts,
// `queued`): only said in `listen`'s log, so one that doesn't read is left out, never failing
// the command. `blockers` are the unfinished ones' issue IDs, for a wait on them; `at` is the
// request's time.
#[derive(Clone, Debug, PartialEq)]
pub struct Queued {
    pub waiting: Waiting,
    pub blockers: Vec<Id>,
    pub at: Time,
}

fn queued(args: &Value) -> Option<Queued> {
    let queued = args.get("queued")?;
    let waiting = match str(queued, "/waiting")? {
        "slot" => Waiting::Slot,
        "held" => Waiting::Held,
        "merged" => Waiting::Merged,
        "blockers" => Waiting::Blockers,
        _ => return None,
    };
    let blockers = match queued.get("blockers") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items.iter().map(|b| b.as_str().and_then(Id::parse)).collect::<Option<_>>()?,
        Some(_) => return None,
    };
    Some(Queued { waiting, blockers, at: str(queued, "/at").and_then(Time::parse)? })
}

// None for anything but a command, such as a base tip or "pong"; Err for a command whose
// envelope doesn't read, which has nothing to answer with, so it is only logged and the
// Durable Object's timeout ends it.
pub fn parse_command(value: &Value) -> Option<Result<Command, String>> {
    if str(value, "/type") != Some("command") {
        return None;
    }
    Some(envelope(value))
}

// A repo's default branch as the relay last heard it pushed, sent on connect and on each
// push (src/worker/realtime.ts, `BaseTip`). None for anything else; Err with why a tip is
// skipped. `after` reaches a git command line.
pub fn parse_base(value: &Value) -> Option<Result<BaseTip, String>> {
    if value.get("type")?.as_str()? != "base" || value.get("id").is_some() {
        return None;
    }
    let repo = non_empty(value, "/repo").filter(|r| split_repo(r).is_some());
    let reference = non_empty(value, "/ref").filter(|r| r.strip_prefix("refs/heads/").and_then(BranchName::parse).is_some());
    let after = value.get("after").and_then(Value::as_str).and_then(Sha::parse);
    Some(match (repo, reference, after) {
        (Some(repo), Some(reference), Some(after)) => Ok(BaseTip { repo, reference, after }),
        _ => Err("a base tip without a plain repo, branch and commit".into()),
    })
}

// The shape the Durable Object stores a worktree ID in (plain::split_worktree).
pub fn worktree_shape(worktree_id: &str) -> bool {
    crate::plain::split_worktree(worktree_id).is_some()
}

fn envelope(value: &Value) -> Result<Command, String> {
    let id = str(value, "/id").and_then(CommandId::parse).ok_or("a command without a plain id")?;
    let kind = value.get("kind").and_then(|k| serde_json::from_value::<Kind>(k.clone()).ok()).ok_or_else(|| format!("{id}: a command of an unknown kind"))?;
    let issue_id = str(value, "/issueId").and_then(Id::parse).ok_or_else(|| format!("{id}: a command without a plain issue id"))?;
    // The triage machine's and the fleet's commands, and only they, carry `triage` or `fleet` for both.
    let own = match kind {
        Kind::StartTriage | Kind::ObserveTriage => Some(TRIAGE),
        Kind::MakeHost | Kind::WakeHost | Kind::SuspendHost | Kind::UpdateHost | Kind::DestroyHost => Some(FLEET),
        _ => None,
    };
    let identifier = str(value, "/identifier")
        .and_then(|i| match own {
            Some(own) => (i == own && issue_id == own).then(|| IssueKey::machine(own)),
            None => IssueKey::parse(i),
        })
        .ok_or_else(|| format!("{id}: a command without a plain identifier"))?;
    let version = value.get("version").and_then(Value::as_u64).ok_or_else(|| format!("{id}: a command without a version"))?;
    let urgent = value.get("urgent").and_then(Value::as_bool).unwrap_or(false);
    let order = order(kind, value.get("args").unwrap_or(&Value::Null)).ok_or_else(|| format!("{id}: a {} command without its plain args", kind.name()));
    Ok(Command { id, kind, issue_id, identifier, version, urgent, order, queued: queued(value.get("args").unwrap_or(&Value::Null)) })
}

// Some(None) for a key missing or null, `parse` of its value otherwise; None for a value of
// the wrong shape.
fn optional<T>(args: &Value, key: &str, parse: impl FnOnce(&Value) -> Option<T>) -> Option<Option<T>> {
    match args.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(value) => parse(value).map(Some),
    }
}

fn order(kind: Kind, args: &Value) -> Option<Order> {
    let session = || str(args, "/sessionId").and_then(Id::parse);
    let maybe_session = || optional(args, "sessionId", |_| session());
    let flag = |key: &str| optional(args, key, Value::as_bool).map(Option::unwrap_or_default);
    let worktree = || non_empty(args, "/worktreeId").filter(|w| worktree_shape(w));
    let maybe_worktree = || optional(args, "worktreeId", |_| worktree());
    let repo = || non_empty(args, "/repo").filter(|r| split_repo(r).is_some());
    let number = || args.get("number").and_then(Value::as_u64).filter(|n| *n > 0);
    let time = |key: &str| optional(args, key, |at| at.as_str().and_then(Time::parse));
    let list = |key: &str| -> Option<Vec<&str>> {
        match args.get(key) {
            None | Some(Value::Null) => Some(Vec::new()),
            Some(Value::Array(items)) => items.iter().map(Value::as_str).collect(),
            Some(_) => None,
        }
    };
    let ids = |key: &str| -> Option<Vec<Id>> { list(key)?.into_iter().map(Id::parse).collect() };
    Some(match kind {
        Kind::ReadIssue => Order::ReadIssue,
        Kind::StartWorker => Order::StartWorker {
            session_id: session()?,
            orchestrate: flag("orchestrate")?,
            worktree_id: maybe_worktree()?,
            host: optional(args, "host", |host| host.as_str().filter(|host| plain_host(host)).map(String::from))?,
        },
        Kind::DeliverMessage => {
            let comment = flag("comment")?;
            let session_id = maybe_session()?;
            // A session's message names its session; only the owner's comment may name none.
            (comment || session_id.is_some()).then_some(Order::DeliverMessage { worktree_id: worktree()?, session_id, comment })?
        }
        Kind::ReadComment => Order::ReadComment { comment_id: str(args, "/commentId").and_then(Id::parse)? },
        Kind::ResumeWorker => {
            let why = match (flag("childrenEnd")?, flag("childrenAdded")?) {
                (true, _) => Resume::ChildrenEnd,
                (false, true) => Resume::ChildrenAdded,
                (false, false) => Resume::Message,
            };
            Order::ResumeWorker { session_id: session()?, why }
        }
        Kind::StartRound => Order::StartRound {
            repo: repo()?,
            number: number()?,
            head: optional(args, "head", |head| head.as_str().and_then(Sha::parse))?,
            round: optional(args, "round", |round| round.as_u64().and_then(|r| u32::try_from(r).ok()).filter(|r| *r > 0))?,
            session_id: maybe_session()?,
            rebase_only: flag("rebaseOnly")?,
        },
        Kind::StopWorker => Order::StopWorker {
            worktree_id: maybe_worktree()?,
            session_id: maybe_session()?,
        },
        Kind::StartTurn => Order::StartTurn { session_id: session()?, thread: flag("thread")?, request: time("request")?, after: time("after")? },
        Kind::StopTurn => Order::StopTurn { session_id: session()?, thread: flag("thread")?, idle: flag("idle")? },
        Kind::EvaluatePr => Order::EvaluatePr { repo: repo()?, number: number()? },
        Kind::Observe => Order::Observe {
            worktree_ids: list("worktreeIds")?.into_iter().map(|w| worktree_shape(w).then(|| w.to_string())).collect::<Option<_>>()?,
            session_id: maybe_session()?,
            thread: flag("thread")?,
        },
        Kind::CloseAgent => Order::CloseAgent { worktree_id: worktree()? },
        Kind::ReapWorktree => {
            let rule = args.get("rule")?;
            let rule = match rule.get("kind").and_then(Value::as_str)? {
                "merged" => ReapRule::Merged { number: rule.get("number").and_then(Value::as_u64).filter(|n| *n > 0)?, head: rule.get("head").and_then(Value::as_str).and_then(Sha::parse)? },
                "landed" => ReapRule::Landed,
                "cancelled" => ReapRule::Cancelled { no_pr: rule.get("noPr").and_then(Value::as_bool).unwrap_or(false) },
                "completed" => ReapRule::Completed,
                _ => return None,
            };
            Order::ReapWorktree { worktree_id: worktree()?, rule }
        }
        Kind::PostNotice => {
            let notice = serde_json::from_value::<Notice>(args.get("notice")?.clone()).ok()?;
            let blockers = ids("blockers")?;
            let pr = args.get("pr").and_then(Value::as_u64).filter(|n| *n > 0);
            let rounds = args.get("rounds").and_then(Value::as_u64).and_then(|r| u32::try_from(r).ok()).filter(|r| *r > 0);
            let needs = match notice {
                Notice::Blocked => !blockers.is_empty(),
                Notice::RoundsCap => pr.is_some() && rounds.is_some(),
                Notice::PrClosed | Notice::AutoMergeOff | Notice::CiCouldntRun | Notice::ChecksPending | Notice::CopilotWontApprove => pr.is_some(),
                _ => true,
            };
            let through = time("through")?;
            needs.then_some(Order::PostNotice { session_id: maybe_session()?, thread: flag("thread")?, notice, blockers, pr, rounds, through })?
        }
        Kind::TakeBackChain => Order::TakeBackChain { children: ids("children")? },
        Kind::StartContext => Order::StartContext,
        Kind::StartTriage => Order::StartTriage { issue_ids: ids("issueIds")? },
        Kind::ObserveTriage => Order::ObserveTriage { session_id: session()?, terminal: str(args, "/terminal").and_then(Terminal::parse)? },
        Kind::UpdateBranch => Order::UpdateBranch { repo: repo()?, number: number()?, head: str(args, "/head").and_then(Sha::parse)? },
        Kind::MergePr => Order::MergePr { repo: repo()?, number: number()?, head: str(args, "/head").and_then(Sha::parse)? },
        Kind::MakeHost => Order::MakeHost { repo: str(args, "/repo").filter(|repo| crate::hosts::plain_name(repo))?.to_string() },
        Kind::WakeHost => Order::WakeHost { name: str(args, "/name").filter(|name| plain_host(name))?.to_string() },
        Kind::SuspendHost => Order::SuspendHost { host: str(args, "/host").filter(|host| plain_host(host))?.to_string() },
        Kind::UpdateHost => Order::UpdateHost { name: str(args, "/name").filter(|name| plain_host(name))?.to_string() },
        Kind::DestroyHost => Order::DestroyHost { name: str(args, "/name").filter(|name| plain_host(name))?.to_string() },
    })
}

impl Kind {
    pub fn name(self) -> String {
        serde_json::to_value(self).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
    }
}

// How a command ended, and what its result carries, as IDs, enums, numbers and times.
#[derive(Clone, Debug, PartialEq)]
pub struct Finished {
    pub outcome: Outcome,
    pub detail: Map<String, Value>,
    // Given back while a machine it needs is made, woken or put to sleep: `listen` says so, not
    // `retryable`. Never sent.
    pub awaits: bool,
}

impl Finished {
    pub fn ok(detail: Value) -> Finished {
        Finished { outcome: Outcome::Ok, detail: detail.as_object().cloned().unwrap_or_default(), awaits: false }
    }

    pub fn held() -> Finished {
        Finished { outcome: Outcome::Held, detail: Map::new(), awaits: false }
    }

    pub fn gone() -> Finished {
        Finished { outcome: Outcome::Gone, detail: Map::new(), awaits: false }
    }

    // A spent budget, what can't be done yet, or a server or connection that failed
    // (`Unavailable`) is worth sending again, as every command is safe to run again; anything
    // else failed (`gateway::Failure`). Not every `Transient`: an `Error::Failed` is as often a
    // refusal (another issue's session) as a blip, and Orca's may have opened or typed into a
    // terminal. The error's text stays in `listen`'s own log: the Durable Object keeps no text.
    pub fn of(error: &Error) -> Finished {
        let outcome = match Failure::of(error) {
            Failure::Spent(_) => Outcome::Retryable,
            _ if error.later() || error.unavailable() => Outcome::Retryable,
            Failure::Transient | Failure::Refused => Outcome::Failed,
        };
        let detail = match error {
            Error::Waits(waits, _) => Map::from_iter([waits_on(waits)]),
            _ => Map::new(),
        };
        Finished { outcome, detail, awaits: false }
    }

    // Not answered yet: the Durable Object sends it again at its timeout, while it has sends.
    pub fn retryable() -> Finished {
        Finished { outcome: Outcome::Retryable, detail: Map::new(), awaits: false }
    }

    // Retryable while its worker's machine wakes, or is put to sleep, which `waits` names: the
    // send is given back, for a while.
    pub fn waking(waits: &Waits) -> Finished {
        Finished { outcome: Outcome::Retryable, detail: Map::from_iter([("waking".into(), Value::Bool(true)), waits_on(waits)]), awaits: true }
    }

    pub fn failed() -> Finished {
        Finished { outcome: Outcome::Failed, detail: Map::new(), awaits: false }
    }

    pub fn failed_with(detail: Value) -> Finished {
        Finished { outcome: Outcome::Failed, detail: detail.as_object().cloned().unwrap_or_default(), awaits: false }
    }
}

// A retryable result's `waitsOn` (src/worker/machine/types.ts, ResultData): the Durable Object
// sends the command again once it clears.
fn waits_on(waits: &Waits) -> (String, Value) {
    let on = match waits {
        Waits::Host(host) => json!({ "host": host }),
        Waits::Lock => json!({ "lock": true }),
        Waits::Orca => json!({ "orca": true }),
    };
    ("waitsOn".into(), on)
}

pub fn result_line(command: &Command, finished: &Finished) -> String {
    json!({ "report": "commandResult", "id": command.id, "issueId": command.issue_id, "version": command.version, "result": finished.outcome, "detail": finished.detail }).to_string()
}

// `sequence` counts from 1 on each connection (src/worker/machine.ts, Report).
pub fn observation_line(observed: Observed, worktree_id: &str, sequence: u64) -> String {
    json!({ "report": observed, "worktreeId": worktree_id, "sequence": sequence }).to_string()
}

// More worktrees than the relay takes in one listing (src/worker/machine.ts, MAX_LISTED).
pub const MAX_LISTED: usize = 1000;

// Orca's listing whole (src/worker/machine.ts, Listing): `at` when it began by the
// workstation's clock, which only names it, and `elapsed_ms` how long before this it was, by
// the monotonic one, which is what the Durable Object orders it by; each agent's state by
// worktree, every worktree, null when incomplete or more than MAX_LISTED, so that a
// worktree missing from it is never taken as gone, the paired hosts that answered, as a
// worktree on any other is in neither, and every paired host, so that a worktree on one in
// neither list is on a host unpaired, which is gone at once. Only the agents' states are said, and a worktree ID
// of a shape the relay would refuse the whole listing for is left out: none such is ever a
// worker's. `idle`: the recipe machines that hold nothing, by host ID (hosts.rs, `idle`), and
// `dormant` those whose destroy clock runs, by name, None while one is made, woken, slept or
// destroyed (hosts.rs, `dormant`). `fleet`: this machine's workers and each paired host's
// entry (hosts.rs, `Fleet`).
#[allow(clippy::too_many_arguments)]
pub fn listing_line(at: &str, elapsed_ms: u64, agents: &[(String, Observed)], listed: Option<&[String]>, hosts: &[String], paired: &[String], idle: &BTreeMap<String, i64>, dormant: Option<&BTreeMap<String, i64>>, fleet: &Fleet) -> String {
    let agents: serde_json::Map<String, Value> = agents
        .iter()
        .filter(|(id, _)| worktree_shape(id))
        .filter_map(|(id, observed)| {
            let state = match observed {
                Observed::AgentWorking => "working",
                Observed::AgentIdle => "idle",
                Observed::AgentWaiting => "waiting",
                Observed::AgentBackground | Observed::AgentExited | Observed::WorktreeGone => return None,
            };
            Some((id.clone(), json!(state)))
        })
        .collect();
    let listed: Option<Vec<&String>> = listed.filter(|ids| ids.len() <= MAX_LISTED).map(|ids| ids.iter().filter(|id| worktree_shape(id)).collect());
    json!({ "report": "listing", "at": at, "elapsedMs": elapsed_ms, "agents": agents, "listed": listed, "hosts": hosts, "paired": paired, "idle": idle, "dormant": dormant, "fleet": fleet }).to_string()
}

pub fn turn_ended_line(session_id: &str, sequence: u64) -> String {
    json!({ "report": "turnEnded", "sessionId": session_id, "sequence": sequence }).to_string()
}

pub fn terminal_closed_line(terminal: &str, sequence: u64) -> String {
    json!({ "report": "terminalClosed", "terminal": terminal, "sequence": sequence }).to_string()
}

// `listen`'s settings, as the Durable Object's fleet keeps them (src/worker/machine/fleet.ts):
// this machine's cap, `slots` by host name, each repo's recipe settings by repo, with the
// revision its machines are brought to, and each machine machines.json records, by name. Names
// and numbers only. Sent as `listen` connects and whenever it changes.
pub fn config_line(config: &Config) -> String {
    let recipe = |entry: &RepoEntry| {
        let mut recipe = json!(entry.recipe.as_ref()?);
        recipe["revision"] = json!(config.revisions.get(&entry.repo));
        Some((entry.repo.clone(), recipe))
    };
    let repos: Map<String, Value> = config.repos.0.iter().filter_map(recipe).collect();
    let machines: Map<String, Value> = config.machines.iter().map(|(name, made)| (name.clone(), json!({ "repo": made.repo, "made": made.made, "asleep": made.asleep, "environment": made.environment, "revision": made.revision }))).collect();
    json!({ "report": "config", "cap": config.local_slots().min(MAX_SLOTS), "slots": config.slots, "repos": repos, "machines": machines }).to_string()
}

// Who reads or answers a session: the issue's worker, or the session's or a note's
// thread's turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Reader {
    Worker,
    Turn,
}

// A worker's own `tolkster` reports, sent to POST /state as `op: "report"`. None for a
// value of the wrong shape, which is never sent.
pub fn worker_read(session_id: &str, activity_id: &str, at: &str, reader: Reader) -> Option<Value> {
    worker_report("workerRead", session_id, activity_id, at, reader)
}

pub fn worker_answered(session_id: &str, activity_id: &str, at: &str, reader: Reader) -> Option<Value> {
    worker_report("workerAnswered", session_id, activity_id, at, reader)
}

fn worker_report(report: &str, session_id: &str, activity_id: &str, at: &str, reader: Reader) -> Option<Value> {
    let (session_id, activity_id, at) = (Id::parse(session_id)?, Id::parse(activity_id)?, Time::parse(at)?);
    Some(json!({ "op": "report", "report": report, "sessionId": session_id, "activityId": activity_id, "at": at, "reader": reader }))
}

// An agent's state, from its own hook at `at` by this machine's clock, which orders one
// worktree's hooks, and `elapsed_ms` before this by the monotonic one, which is all the
// Durable Object sets against a listing (`tolkster hook state`): never `worktreeGone`, which
// only Orca's listing says.
pub fn agent_state(observed: Observed, worktree_id: &str, at: &str, elapsed_ms: u64) -> Option<Value> {
    let at = Time::parse(at)?;
    (observed != Observed::WorktreeGone && worktree_shape(worktree_id)).then(|| json!({ "op": "report", "report": observed, "worktreeId": worktree_id, "at": at, "elapsedMs": elapsed_ms }))
}

// A session's turn in `terminal` is done with Claude's input and waits for the next
// (`converse`), at its time on the workstation's clock, as a hook's is.
pub fn turn_idle(session_id: &str, terminal: &str, at: &str) -> Option<Value> {
    let (session_id, terminal, at) = (Id::parse(session_id)?, crate::plain::Terminal::parse(terminal)?, Time::parse(at)?);
    Some(json!({ "op": "report", "report": "turnIdle", "sessionId": session_id, "terminal": terminal, "at": at }))
}

pub fn chain_recorded(issue_id: &str, session_id: &str, run: &str, children: &[String]) -> Option<Value> {
    let (issue_id, session_id, run) = (Id::parse(issue_id)?, Id::parse(session_id)?, Id::parse(run)?);
    let children = children.iter().map(|c| Id::parse(c)).collect::<Option<Vec<_>>>()?;
    Some(json!({ "op": "report", "report": "chainRecorded", "issueId": issue_id, "sessionId": session_id, "run": run, "children": children }))
}

// A worker's own report that it pushed its change to the default branch with no PR: the
// issue, the worker's session, and the commit pushed.
pub fn landed(issue_id: &str, session_id: &str, head: &str) -> Option<Value> {
    let (issue_id, session_id, head) = (Id::parse(issue_id)?, Id::parse(session_id)?, Sha::parse(head)?);
    Some(json!({ "op": "report", "report": "landed", "issueId": issue_id, "sessionId": session_id, "head": head }))
}

// A worker's own report that GitHub refused auto-merge on its PR as mergeable already, so the
// machine merges it: the issue, the worker's session, and the PR.
pub fn merge_asked(issue_id: &str, session_id: &str, pr: u64) -> Option<Value> {
    let (issue_id, session_id) = (Id::parse(issue_id)?, Id::parse(session_id)?);
    (pr > 0).then(|| json!({ "op": "report", "report": "mergeAsked", "issueId": issue_id, "sessionId": session_id, "pr": pr }))
}

#[cfg(test)]
mod tests;
