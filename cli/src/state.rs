// Tolkster's own records in the owner's Durable Object (src/worker/state.ts), beside its
// machines: a session's conversation, the workers and checkouts, the delegations a chain
// made and the claim lock, reached with POST <worker>/state; and whether the triage
// machine's running session holds an issue (`triaging`). Workers' reports to the machines go the same way (`report`).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ureq::Agent;

use crate::config::{self, Files, WorkerUrl};
use crate::error::{Context, Error};
use crate::config::ladder::Launch;
use crate::http::{RETRIES, post_json};
use crate::plain::{Id, Time};
use crate::trace::{self, Call, Undone, Work};

// Tolkster's record of a session: each PR's feedback a round has read, and its
// conversation's agent session and latest turn. What was asked, read and answered is the
// machines' (docs/state-machine.md), never kept here.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<ReadRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<Conversation>,
}

// How far `tolkster pr feedback --session` last read PR `pr`'s feedback, so the next round's
// read prints only what came after (commands/pr.rs).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadRecord {
    pub pr: u64,
    pub at: String,
}

// A conversation's agent session, which each turn resumes, and its latest turn's Orca
// terminal as of `at`. The Durable Object keeps the session the first turn chose. `terminal`
// is None once that terminal has closed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    pub agent_session: String,
    pub terminal: Option<String>,
    pub at: String,
}

// A change to one session's record. The Durable Object keeps the later, so a late or
// repeated write can't take a record back. Only the contract test reads it back.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read: Option<ReadRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation: Option<Conversation>,
}

// Why a PR won't merge by itself, as a notice says it (command.rs, `notice_text`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Told {
    Closed,
    AutoMergeOff,
    CiCouldntRun,
    ChecksPending,
    CopilotWontApprove,
}

// What the session's request machine holds of its readers (docs/state-machine.md, Facts):
// the owner's newest request a worker, and a turn, has read, and the newest answered.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub read_through: Option<String>,
    pub turn_read_through: Option<String>,
    pub answered_through: Option<String>,
}

// Why a worker exists, recorded as `tolkster listen` starts it (executor.rs), once Orca has
// named the worktree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRecord {
    pub issue: String,
    pub worktree_id: String,
    pub session_id: String,
    pub at: String,
    pub launch: Launch,
    // Where the launch's size came from.
    pub rung_from: RungFrom,
}

// Where a worker's rung size came from (executor::relaunch): the issue's at the claim, none
// there, or the one its worker set. Only `NoEstimate` is ever picked again.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RungFrom {
    Claim,
    NoEstimate,
    Worker,
}

// A sub-issue Tolkster delegated to itself (`tolkster issue update --delegate`), recorded
// just before the delegation, so the session Linear starts for it, which the owner didn't
// start, is still Tolkster's to act on (Session::is_owners). Kept for 90 days.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Delegation {
    pub issue_id: String,
    pub parent_issue_id: String,
    pub session_id: String,
    pub at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutRecord {
    pub repo_id: String,
    pub worktree_id: String,
    pub at: String,
}

// Tolkster's one context comment on an issue, by both Linear IDs (src/worker/state.ts).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextComment {
    pub issue_id: String,
    pub comment_id: String,
    pub at: String,
}

pub trait Store {
    // Takes every claim, or none: `(name, ttl)`, under `token`. Without a TTL a claim is held
    // for good; one older than its holder's TTL is stale and taken over. Taking again under
    // the same token succeeds and sets the TTL again, so a lost response can be retried and
    // a claim held for a time held for good.
    fn take(&self, claims: &[(String, Option<Duration>)], token: &str) -> Result<bool, Error>;
    // Gives claims back, only those `token` still holds: a stale claim another run took over
    // stays its.
    fn release(&self, names: &[String], token: &str) -> Result<(), Error>;
    fn record(&self, session_id: &str, change: &Change) -> Result<Record, Error>;
}

// What a command records beside its claims (command.rs, executor.rs): `State` keeps them in
// the Durable Object, and tests in memory (ports/fake.rs).
pub trait Records: Store {
    fn start_worker(&self, worker: &WorkerRecord) -> Result<(), Error>;
    fn workers(&self) -> Result<Vec<WorkerRecord>, Error>;
    // Whether the worker host `host` holds an admitted credential now, read afresh: a start's
    // check before a worker goes to the host it was granted, as `listen`'s list is a listing old.
    fn admitted(&self, host: &str) -> Result<bool, Error>;
    fn delegations(&self, issues: &[Id]) -> Result<HashMap<String, Delegation>, Error>;
    fn converse(&self, session_id: &str, conversation: &Conversation) -> Result<(), Error>;
    // The conversation recorded under `session_id`, a session's or a note's thread's root.
    fn conversation(&self, session_id: &str) -> Result<Option<Conversation>, Error>;
    fn checkouts(&self) -> Result<Vec<CheckoutRecord>, Error>;
    fn set_checkout(&self, checkout: &CheckoutRecord) -> Result<(), Error>;
    // Once reap has removed its worktree.
    fn forget_worker(&self, worktree_id: &str) -> Result<(), Error>;
    fn context_comment(&self, issue_id: &str) -> Result<Option<ContextComment>, Error>;
    // A replacement overwrites the record of the comment it replaces.
    fn set_context_comment(&self, comment: &ContextComment) -> Result<(), Error>;
}

// The one way a claim guards an action: takes `claims`, runs `act` with them held and gives
// them back should it fail. None when another run holds them. What `act` returns stays
// claimed, for good or until its TTL. `act` must post nothing before it fails but what
// Linear refuses twice, by its ID: a claim given back after a post lets the next run post it again.
pub fn holding<T>(store: &dyn Store, claims: &[(String, Option<Duration>)], token: &str, act: impl FnOnce() -> Result<T, Error>) -> Result<Option<T>, Error> {
    if !store.take(claims, token)? {
        return Ok(None);
    }
    match act() {
        Ok(done) => Ok(Some(done)),
        Err(error) => {
            let names: Vec<String> = claims.iter().map(|(name, _)| name.clone()).collect();
            Err(error.and_give_back(store.release(&names, token), "its claim"))
        }
    }
}

// 64 bits, unique but not unpredictable: std seeds one pair of keys per thread and steps it on each call.
fn random() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new().build_hasher().finish()
}

pub fn new_token() -> Id {
    Id::hex(random(), random() ^ u64::from(std::process::id()))
}

pub fn new_uuid() -> String {
    uuid(random(), random())
}

// The same UUID for the same `seed`, so a retry's post carries its first try's ID. Shaped as
// version 4, as Linear's IDs are.
pub fn derived_uuid(seed: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, seed.as_bytes());
    let half = |bytes: &[u8]| u64::from_be_bytes(bytes.try_into().expect("8 bytes"));
    uuid(half(&digest.as_ref()[..8]), half(&digest.as_ref()[8..16]))
}

fn uuid(high: u64, low: u64) -> String {
    let high = (high & !0xf000) | 0x4000;
    let low = (low & !(0b11 << 62)) | (0b10 << 62);
    let hex = format!("{high:016x}{low:016x}");
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

// The request a thread's turn answers now: the newest it was shown, as `converse` reports each
// request's replies before Claude reads them, and a warm turn's follow-up has moved it; never
// older than `floor`, the one the turn started on. A failed read is an error, never `floor`,
// whose answer a warm turn has already posted.
pub fn thread_request(agent: &Agent, files: &Files, root: &str, floor: Time) -> Result<Time, Error> {
    let reading = State::open(agent, files)?.reading(root).map_err(|error| Error::Failed(format!("reading the request this turn answers failed: {error}")))?;
    Ok(newest_request(floor, reading.turn_read_through.as_deref()))
}

fn newest_request(floor: Time, shown: Option<&str>) -> Time {
    shown.and_then(Time::parse).filter(|shown| shown.as_str() > floor.as_str()).unwrap_or(floor)
}

pub struct State<'a> {
    agent: &'a Agent,
    url: String,
    secret: String,
}

const READ_CHUNK: usize = 500;

const READS: [&str; 8] = ["read", "workers", "delegations", "checkouts", "triaging", "session", "contextComment", "enrolledHosts"];

fn taken(body: &Value, result: &Result<Value, Error>) -> String {
    match result {
        Ok(data) if body["op"] == "take" => data.get("taken").map_or(String::new(), |taken| format!(" taken={taken}")),
        _ => String::new(),
    }
}

impl<'a> State<'a> {
    pub fn new(agent: &'a Agent, worker_url: &WorkerUrl, secret: String) -> State<'a> {
        State { agent, url: worker_url.state(), secret }
    }

    pub fn open(agent: &'a Agent, files: &Files) -> Result<State<'a>, Error> {
        let bearer = config::state_bearer(files)?;
        Ok(State::new(agent, &config::worker_url(files)?, bearer))
    }

    fn call(&self, body: Value) -> Result<Value, Error> {
        let text = body.to_string();
        self.call_as(&body, &|| text.clone())
    }

    // `body` as `text` writes it on each attempt, so a field that says how long ago something
    // was (`report_since`) is true of the attempt that lands.
    fn call_as(&self, body: &Value, text: &dyn Fn() -> String) -> Result<Value, Error> {
        let started = Instant::now();
        let result = self.call_once(text);
        let op = body["op"].as_str().unwrap_or_default();
        trace::slow(|| format!("state {op}"), started);
        if !READS.contains(&op) {
            trace::event(Call::Wrote { site: format!("state {op}"), detail: format!("{}{}", trace::ids(body), taken(body, &result)), failed: result.as_ref().err().cloned() });
        }
        result
    }

    fn call_once(&self, body: &dyn Fn() -> String) -> Result<Value, Error> {
        let mut pauses = RETRIES.iter();
        loop {
            match self.once(&body()) {
                Err((error, true)) => match pauses.next() {
                    Some(pause) => std::thread::sleep(*pause),
                    None => return Err(error),
                },
                other => return other.map_err(|(error, _)| error),
            }
        }
    }

    fn once(&self, body: &str) -> Result<Value, (Error, bool)> {
        let unreachable = |detail: String| Error::Failed(format!("couldn't reach Tolkster's state on the Worker: {detail}"));
        let (status, text) = post_json(self.agent, &self.url, &self.secret, &[], body).map_err(|error| (unreachable(error.to_string()), true))?;
        match status {
            200 => serde_json::from_str(&text).map_err(|_| (unreachable("the response was not JSON".into()), false)),
            401 => Err((unreachable("the Worker refused the realtime secret or this host's credential".into()), false)),
            403 => Err((unreachable("only the owner's machine may do that".into()), false)),
            // The Worker's own message: it names the field it refused, never a value.
            400 => Err((unreachable(format!("HTTP 400: {}", serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"].as_str().map(String::from)).unwrap_or_default())), false)),
            _ => Err((unreachable(format!("HTTP {status}")), status >= 500 || status == 429)),
        }
    }

    pub fn read(&self, sessions: &[String]) -> Result<HashMap<String, Option<Record>>, Error> {
        let mut records = HashMap::new();
        // An empty read still checks the Durable Object can be reached.
        let chunks: Vec<&[String]> = if sessions.is_empty() { vec![&[]] } else { sessions.chunks(READ_CHUNK).collect() };
        for chunk in chunks {
            let data = self.call(json!({ "op": "read", "sessions": chunk }))?;
            records.extend(parse_records(&data)?);
        }
        Ok(records)
    }

    // A worker's own report to the machines (machine.rs). The work goes on without it: the
    // machine reads the issue again at its deadline.
    pub fn report(&self, report: Option<Value>) {
        if let Err(error) = self.reported(report) {
            trace::event(Work::NotDone { what: Undone::Report, error });
        }
    }

    // `report` for one that says how long ago it was made (`elapsedMs`, since `began`): made
    // again for each attempt, as a retry lands later than the first try.
    pub fn report_since(&self, began: Instant, report: impl Fn(u64) -> Option<Value>) {
        let elapsed = || u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
        let Some(body) = report(elapsed()) else { return };
        let text = || report(elapsed()).map_or_else(String::new, |report| report.to_string());
        if let Err(error) = self.call_as(&body, &text) {
            trace::event(Work::NotDone { what: Undone::Report, error });
        }
    }

    // `report`, for one that what follows depends on.
    pub fn reported(&self, report: Option<Value>) -> Result<(), Error> {
        report.map_or(Ok(()), |report| self.call(report).map(|_| ()))
    }

    // `tolkster issue chain`'s ask of the parent's store (docs/state-machine.md, Parent
    // issues): those of `children` that took the chain, as it answers once every forward has
    // landed. Err when it refused or couldn't be asked, so nothing is delegated.
    pub fn chain_recorded(&self, report: Option<Value>, children: &[String]) -> Result<Vec<String>, Error> {
        let report = report.ok_or_else(|| Error::Failed("a chain's report of the wrong shape".into()))?;
        chain_taken(&self.call(report)?, children)
    }

    // Revokes a worker host's credential (docs/hosts.md); whether it held one, never for a
    // name it wasn't issued to, nor for a resend of a revoke that landed. The owner's alone:
    // the Worker refuses a host.
    pub fn revoke_host(&self, host: &str) -> Result<bool, Error> {
        let data = self.call(json!({ "op": "revokeHost", "host": host }))?;
        data.get("held").and_then(Value::as_bool).ok_or_else(|| Error::Failed("Tolkster's state didn't say whether the host held a credential".into()))
    }

    // The IDs of the worker hosts holding a credential `admit_host` admitted (docs/hosts.md): no
    // other takes a worker. The owner's alone.
    pub fn enrolled_hosts(&self) -> Result<Vec<String>, Error> {
        let data = self.call(json!({ "op": "enrolledHosts" }))?;
        serde_json::from_value(data.get("hosts").cloned().ok_or_else(|| Error::Failed("Tolkster's state listed no hosts".into()))?).context("Tolkster's state's hosts")
    }

    // Admits `host`'s credential once `host add` passed every step, so it takes work; whether it
    // held one. The owner's alone.
    pub fn admit_host(&self, host: &str) -> Result<bool, Error> {
        let data = self.call(json!({ "op": "admitHost", "host": host }))?;
        data.get("admitted").and_then(Value::as_bool).ok_or_else(|| Error::Failed("Tolkster's state didn't say whether the host was admitted".into()))
    }

    // A single-use enrolment code for `host` (docs/hosts.md), which `tolkster host join` trades
    // once, within minutes, for the host's credential and `config`. The owner's alone.
    pub fn enrol_host(&self, host: &str, config: &Value) -> Result<String, Error> {
        let data = self.call(json!({ "op": "enrolHost", "host": host, "config": config }))?;
        data.get("code").and_then(Value::as_str).map(String::from).ok_or_else(|| Error::Failed("Tolkster's state minted no enrolment code".into()))
    }

    pub fn delegate(&self, delegation: &Delegation) -> Result<(), Error> {
        self.call(json!({ "op": "delegate", "delegation": delegation })).map(|_| ())
    }

    // What the session's request machine holds of its reader, or nothing for a session the
    // Durable Object doesn't know.
    pub fn reading(&self, session_id: &str) -> Result<Reading, Error> {
        let data = self.call(json!({ "op": "session", "sessionId": session_id }))?;
        serde_json::from_value(data).context("Tolkster's state's session")
    }

    // Whether `session` is the triage session the triage machine holds as running, and was
    // given `identifier` (src/worker/machine/triage.ts).
    pub fn triaging(&self, session: &str, identifier: &str) -> Result<bool, Error> {
        let data = self.call(triaging_body(session, identifier))?;
        data.get("holds").and_then(Value::as_bool).ok_or_else(|| Error::Failed("Tolkster's state didn't say whether the triage session holds the issue".into()))
    }
}

// `taken: null` is a chain the parent's store refused, or one a sub-issue hasn't answered
// yet: an error, never every sub-issue refusing it.
fn chain_taken(data: &Value, children: &[String]) -> Result<Vec<String>, Error> {
    if data.get("taken").is_some_and(Value::is_null) {
        return Err(Error::Failed("the chain wasn't answered: the parent's store refused it or a sub-issue hasn't answered yet, so nothing was delegated".into()));
    }
    let taken = data.get("taken").and_then(Value::as_array).ok_or_else(|| Error::Failed("Tolkster's state didn't say which sub-issues took the chain".into()))?;
    Ok(children.iter().filter(|child| taken.iter().any(|t| t.as_str() == Some(child.as_str()))).cloned().collect())
}

fn triaging_body(session: &str, identifier: &str) -> Value {
    json!({ "op": "triaging", "session": session, "identifier": identifier })
}

fn parse_records(data: &Value) -> Result<HashMap<String, Option<Record>>, Error> {
    let Some(records) = data.get("records").and_then(Value::as_object) else {
        return Err(Error::Failed("Tolkster's state had no records".into()));
    };
    records.iter().map(|(id, value)| Ok((id.clone(), if value.is_null() { None } else { Some(serde_json::from_value(value.clone()).context(format!("Tolkster's state's record of {id}"))?) }))).collect()
}

impl Store for State<'_> {
    fn take(&self, claims: &[(String, Option<Duration>)], token: &str) -> Result<bool, Error> {
        let claims: Vec<Value> = claims.iter().map(|(name, ttl)| json!({ "name": name, "ttlMs": ttl.map(|t| t.as_millis() as u64) })).collect();
        let data = self.call(json!({ "op": "take", "claims": claims, "token": token }))?;
        data.get("taken").and_then(Value::as_bool).ok_or_else(|| Error::Failed("Tolkster's state didn't say whether the claim was taken".into()))
    }

    fn release(&self, names: &[String], token: &str) -> Result<(), Error> {
        self.call(json!({ "op": "release", "claims": names, "token": token })).map(|_| ())
    }

    fn record(&self, session_id: &str, change: &Change) -> Result<Record, Error> {
        let data = self.call(json!({ "op": "record", "sessionId": session_id, "change": change }))?;
        serde_json::from_value(data.get("record").cloned().unwrap_or(Value::Null)).context("Tolkster's state's record")
    }
}

impl Records for State<'_> {
    // A worktree started again at the same path replaces the old one's record.
    fn start_worker(&self, worker: &WorkerRecord) -> Result<(), Error> {
        self.call(json!({ "op": "startWorker", "worker": worker })).map(|_| ())
    }
    fn workers(&self) -> Result<Vec<WorkerRecord>, Error> {
        let data = self.call(json!({ "op": "workers" }))?;
        serde_json::from_value(data.get("workers").cloned().ok_or_else(|| Error::Failed("Tolkster's state had no workers".into()))?).context("Tolkster's state's workers")
    }
    fn admitted(&self, host: &str) -> Result<bool, Error> {
        Ok(self.enrolled_hosts()?.iter().any(|id| id == host))
    }
    fn delegations(&self, issues: &[Id]) -> Result<HashMap<String, Delegation>, Error> {
        let mut found = HashMap::new();
        for chunk in issues.chunks(READ_CHUNK) {
            let data = self.call(json!({ "op": "delegations", "issues": chunk }))?;
            let Some(delegations) = data.get("delegations").and_then(Value::as_object) else {
                return Err(Error::Failed("Tolkster's state had no delegations".into()));
            };
            for (id, value) in delegations {
                if !value.is_null() {
                    found.insert(id.clone(), serde_json::from_value(value.clone()).context(format!("Tolkster's state's delegation of {id}"))?);
                }
            }
        }
        Ok(found)
    }
    // A conversation's latest turn, recorded as it opens and as it ends.
    fn converse(&self, session_id: &str, conversation: &Conversation) -> Result<(), Error> {
        self.record(session_id, &Change { conversation: Some(conversation.clone()), ..Change::default() }).map(|_| ())
    }
    fn conversation(&self, session_id: &str) -> Result<Option<Conversation>, Error> {
        Ok(State::read(self, &[session_id.to_string()])?.remove(session_id).flatten().and_then(|record| record.conversation))
    }
    fn checkouts(&self) -> Result<Vec<CheckoutRecord>, Error> {
        let data = self.call(json!({ "op": "checkouts" }))?;
        serde_json::from_value(data.get("checkouts").cloned().ok_or_else(|| Error::Failed("Tolkster's state had no checkouts".into()))?).context("Tolkster's state's checkouts")
    }
    fn set_checkout(&self, checkout: &CheckoutRecord) -> Result<(), Error> {
        self.call(json!({ "op": "setCheckout", "checkout": checkout })).map(|_| ())
    }
    // Deletes a worktree's record.
    fn forget_worker(&self, worktree_id: &str) -> Result<(), Error> {
        self.call(json!({ "op": "forgetWorker", "worktreeId": worktree_id })).map(|_| ())
    }
    fn context_comment(&self, issue_id: &str) -> Result<Option<ContextComment>, Error> {
        let data = self.call(json!({ "op": "contextComment", "issueId": issue_id }))?;
        serde_json::from_value(data.get("comment").cloned().ok_or_else(|| Error::Failed("Tolkster's state didn't say whether the issue has a context comment".into()))?).context("Tolkster's state's context comment")
    }
    fn set_context_comment(&self, comment: &ContextComment) -> Result<(), Error> {
        self.call(json!({ "op": "setContextComment", "comment": comment })).map(|_| ())
    }
}

// What a claim is for: the first part of its name. The Durable Object checks only a
// name's characters and length, so a new kind needs nothing there.
#[derive(Clone, Copy)]
pub enum ClaimKind {
    Context,
    Comment,
    Verdict,
}

impl std::fmt::Display for ClaimKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ClaimKind::Context => "context",
            ClaimKind::Comment => "comment",
            ClaimKind::Verdict => "verdict",
        })
    }
}

pub fn claim_name(kind: ClaimKind, rest: &str) -> String {
    let safe: String = rest.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '@') { c } else { '_' }).collect();
    format!("{kind}-{safe}")
}

#[cfg(test)]
pub mod tests;
