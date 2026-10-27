// Carries out a start, a resume, a review round or a conversation's turn that the Durable
// Object's machine has decided (command.rs), in code with no agent in the way. Nothing is
// decided here from the session: the machine decided from the issue as read, and keeps one
// command in flight per issue, which `listen` runs in the issue's lane. Each is safe to run
// again by what it finds when it runs: a start finds the worktree it made, a resume or a
// turn the agent it opened, and every post carries an activity ID made from the command's.
//
// Linear text never reaches a terminal or a prompt: what is typed or passed is fixed text
// with IDs, names and paths in it. Workers read the owner's words themselves.

use serde_json::{Value, json};

use crate::brief::review_size;
use crate::review_pass::ReviewSize;
use crate::error::{Error, Waits};
use crate::config::ladder::{Ladder, Launch};
use crate::linear::{ActivityInput, ActivityType};
use crate::machine::{Command, Finished, Resume};
use crate::onboard;
use crate::orca::{Host, Listing, Orca, Place, Proof, Status, host_of, Typed, Worktree, agent_in, agent_terminal, find_repo, project_id, repo_name, waits_in};
use crate::plain::{BranchName, Id, IssueKey, RepoName, split_worktree};
use crate::ports::Ports;
use crate::prebuilt;
use crate::request::{Issue, MANUAL_MERGE_LABEL, PLAN_FIRST_LABEL, Session};
use crate::review;
use crate::config::Config;
use crate::setup::{How, Worker, worktree_name};
use crate::state::{Records, RungFrom, WorkerRecord};
use crate::time::{iso, now_ms};
use crate::trace::{self, Mode, Problem, Purpose, Undone, Work};
use crate::voice;


pub fn auto_merges(issue: &Issue) -> bool {
    !issue.labels.iter().any(|l| l.eq_ignore_ascii_case(MANUAL_MERGE_LABEL))
}

// Whether a new worker shows its plan and waits for the owner's go before it changes
// anything: the issue carries PLAN_FIRST_LABEL, whatever its size. Decided from the issue
// read as the worker starts, never by the worker.
pub fn plans_first(issue: &Issue) -> bool {
    issue.labels.iter().any(|l| l.eq_ignore_ascii_case(PLAN_FIRST_LABEL))
}

// What an exited worker is launched with again, and its record to write first when that
// changes. A rung picked with no size at the start gives way, once, to the ladder's rung for
// the issue's size as read now, the estimate its worker set, recorded as the worker's so
// later launches keep it, unless that rung is on another harness, which can't continue this
// one's session. Any other record's launch stands (config/ladder.rs, "Chosen once").
pub fn relaunch(record: Option<&WorkerRecord>, worktree_id: &str, issue: &Issue, ladder: &Ladder) -> (Option<Launch>, Option<WorkerRecord>) {
    let launch = record.map(|r| r.launch.clone());
    let (Some(record), Some(size)) = (record.filter(|r| r.rung_from == RungFrom::NoEstimate), issue.size()) else {
        return (launch, None);
    };
    let (rung, _) = ladder.choose(Some(size), &issue.labels);
    if rung.launch.harness != record.launch.harness {
        return (launch, None);
    }
    let chosen = WorkerRecord { worktree_id: worktree_id.into(), launch: rung.launch.clone(), rung_from: RungFrom::Worker, ..record.clone() };
    (Some(chosen.launch.clone()), Some(chosen))
}

// The session the command names, read fresh, on the command's own issue: a session on
// another is someone else's worker or turn, and refused.
fn session_on_issue(ports: &Ports, command: &Command, session_id: &Id, with_record: bool) -> Result<Session, Error> {
    let session = if with_record { ports.linear.session_with_record(session_id)? } else { ports.linear.session(session_id)? };
    if session.issue.as_ref().map(|i| i.id.as_str()) != Some(command.issue_id.as_str()) {
        return Err(Error::Failed(format!("{}: the session isn't on the command's issue", command.identifier)));
    }
    Ok(session)
}

// What a start, a resume, a round or a turn is carried out with, once its session is found on
// the command's issue.
struct Call<'a, 'p> {
    config: &'a Config,
    ports: &'a mut Ports<'p>,
    command: &'a Command,
    session_id: &'a Id,
    issue: &'a Issue,
}

fn issue_of(session: &Session) -> Result<&Issue, Error> {
    session.issue.as_ref().ok_or_else(|| Error::Failed(format!("session {} has no issue", session.id)))
}

// The worktree Orca links to the issue `identifier`, if any. None found while its worker's
// record names a worktree on a host that didn't answer, or isn't paired, decides nothing: it
// may be there.
pub fn linked_worktree(ports: &mut Ports, identifier: &str) -> Result<Option<Worktree>, Error> {
    let found = linked_in(&ports.orca.agents()?, identifier).cloned();
    if found.is_none()
        && let Some(host) = held_unheard(ports, identifier)?
    {
        return Err(Error::unheard(Some(&host)));
    }
    Ok(found)
}

// The host not heard this run that a worker's record of the issue names a worktree on. One
// that can't be read may, so nothing is decided.
pub(super) fn held_unheard(ports: &Ports, identifier: &str) -> Result<Option<String>, Error> {
    let records = ports.records.workers().map_err(|error| Error::Later(format!("the worker records couldn't be read: {error}")))?;
    Ok(records.iter().filter(|r| r.issue.eq_ignore_ascii_case(identifier) && !ports.orca.heard(&r.worktree_id)).find_map(|r| host_of(&r.worktree_id).map(String::from)))
}

// The same, in a listing already read.
pub(super) fn linked_in<'w>(worktrees: &'w [Worktree], identifier: &str) -> Option<&'w Worktree> {
    worktrees.iter().find(|w| w.linked_issue.as_deref().is_some_and(|i| i.eq_ignore_ascii_case(identifier)))
}

// The origin of this machine's repo `repo_id` as `Orca::worktrees` read it: every worktree of
// a repo shares it, and another host's repo may share its ID.
pub(super) fn local_origin(worktrees: &[Worktree], repo_id: &str) -> Option<String> {
    worktrees.iter().filter(|w| w.is_local && w.repo_id == repo_id).find_map(|w| w.origin.clone())
}

// Where the Durable Object granted a new worker of `repo_id` (docs/dispatcher.md, Hosts): this
// machine, or `host`, checked before anything is said or moved. A host that didn't answer,
// no longer holds an admitted credential, read afresh, or hasn't `project`, the repo's GitHub
// project, ready turns the start back for its resend: `listen` never picks another.
fn granted<'a>(ports: &mut Ports, repo_id: &'a str, project: Option<&'a str>, host: Option<&'a str>, answering: &[Host]) -> Result<Place<'a>, Error> {
    let Some(host) = host else { return Ok(Place::Here { repo_id }) };
    let refused = |why: &str| Err(Error::Later(format!("the start was granted {host}, which {why}")));
    let Some(project) = project else { return refused("can't be checked: the repo's origin wasn't read") };
    if !answering.iter().any(|answered| answered.id == host) {
        return Err(Error::Waits(Waits::Host(host.to_string()), format!("the start was granted {host}, which didn't answer")));
    }
    if !ports.records.admitted(host).map_err(|error| Error::Later(format!("the hosts admitted couldn't be read: {error}")))? {
        return refused("no longer holds an admitted credential");
    }
    if !ports.orca.ready(host, Some(project), onboard::reads_config).unwrap_or(false) {
        return refused("isn't ready for the repo");
    }
    Ok(Place::On { host, project })
}

// A start's result: the worktree its agent runs in, with its branch and its repo's GitHub
// name, by which the Durable Object maps its observations and its PR's events. A branch the
// machine wouldn't take (`BRANCH` in github.ts) leaves the worktree unsaid rather than the
// whole result refused.
// Orca's agent listing carries no origin, so it is the worktree's own, or this machine's repo's
// for one just made, as the start's one full listing read it (`local_origin`); None when it
// couldn't be, and the PR's events then map to no issue. `launched`: its agent
// was launched on its prompt, by this run or, its agent still there, one before it whose
// result was lost, and the machine waits to see it working.
fn started(worktree: &Worktree, origin: Option<String>, made: bool, launched: bool) -> Finished {
    let at = worktree.branch.as_deref().and_then(BranchName::parse).filter(|b| b.len() <= 200).map(|branch| json!({ "id": worktree.id, "branch": branch, "repo": origin }));
    Finished::ok(json!({ "worktree": at, "madeWorktree": made, "launched": launched }))
}

// Picks the session up: the action that takes it, by an activity ID made from the command's,
// so a command run again never posts it twice, and its wording as an ephemeral thought,
// which a failure leaves unsaid. A start posts its wording alone, as the worker's opening
// thought says what it took; a round's action names its PR and head.
fn pick_up(ports: &Ports, command: &Command, session_id: &str, what: Option<&str>, wording: &str) -> Result<(), Error> {
    if let Some(what) = what {
        ports.linear.post_once(&format!("command {} pickup", command.id), ActivityInput::action(session_id, voice::PICKED_UP, what))?;
    }
    if let Err(error) = ports.linear.send(&ActivityInput::text(session_id, ActivityType::Thought, wording, true)) {
        trace::event(Work::NotDone { what: Undone::PickupWording, error });
    }
    Ok(())
}

// A launch that failed, told the owner once per command as an elicitation whose reply, or its
// "Try again" option, tries it again (no retry here: a terminal Orca timed out on may have
// opened, and a second launch beside it would be two agents). It names Orca when the failure
// was Orca's, never with the error's text. `said` in the result keeps the machine from adding a notice of its own; a
// post that failed leaves the telling to that notice.
fn failed_and_said(ports: &Ports, command: &Command, session_id: &str, what: &str, error: &Error, mut detail: Value) -> Finished {
    trace::event(Work::Failed(error.clone()));
    let text = voice::couldnt(what, error.orcas());
    match ports.linear.post_once(&format!("command {} failed", command.id), ActivityInput { options: vec![&voice::RETRY], ..ActivityInput::text(session_id, ActivityType::Elicitation, &text, false) }) {
        Ok(()) => detail["said"] = json!(true),
        Err(posting) => trace::event(Work::NotDone { what: Undone::OwnerTold, error: posting }),
    }
    Finished::failed_with(detail)
}

// A launch's every failure after its session was checked is told, as `failed_and_said`,
// except a spent budget, which the command waits out.
fn said_if_failed(ports: &Ports, command: &Command, session_id: &str, what: &str, detail: Value, done: Result<Finished, Error>) -> Result<Finished, Error> {
    match done {
        Err(error) if matches!(error, Error::RateLimited { .. }) || error.later() => Err(error),
        Err(error) => Ok(failed_and_said(ports, command, session_id, what, &error, detail)),
        done => done,
    }
}

// Starts the issue's worker in a worktree of its own (`startWorker`). One already linked to
// the issue is answered with (docs/state-machine.md, Commands): with an agent in it, the
// worker a run of this command started before. With none, it holds a start that never began
// only when it is the worktree the machine named, in the issue's repo, with Tolkster's record
// of it for this issue: an agent is launched there on the brief, rendered again. Any other,
// made by hand, is answered with as it is. A failed start's
// result says whether a worktree may be there, which keeps the machine from counting one
// gone that isn't.
pub fn start(config: &Config, ports: &mut Ports, command: &Command, session_id: &Id, orchestrate: bool, named: Option<&str>, host: Option<&str>) -> Result<Finished, Error> {
    let session = session_on_issue(ports, command, session_id, false)?;
    let issue = issue_of(&session)?;
    // Listed once, with the origins: the worktree already linked, the repo's primary checkout and
    // the origin of the worker's PRs all come from it.
    let worktrees = match ports.orca.worktrees() {
        Ok(worktrees) => worktrees,
        // One it was named is there, though Orca couldn't say.
        Err(error) => return said_if_failed(ports, command, session_id, voice::COULDNT_START, json!({ "madeWorktree": named.is_some() }), Err(error)),
    };
    let existing = linked_in(&worktrees, &issue.identifier).cloned();
    // A worktree the machine named, or a worker's record holds, on a host not heard, isn't
    // made again: a start resent after its result was lost names none.
    if existing.is_none() {
        let named = named.filter(|id| !ports.orca.heard(id)).and_then(host_of).map(String::from);
        if let Some(host) = named.map_or_else(|| held_unheard(ports, &issue.identifier), |host| Ok(Some(host)))? {
            return Err(Error::unheard(Some(&host)));
        }
    }
    // A worktree that was there stays there whatever failed.
    let there = existing.is_some();
    let done = start_in(&mut Call { config, ports, command, session_id, issue }, orchestrate, named, host, existing, &worktrees);
    said_if_failed(ports, command, session_id, voice::COULDNT_START, json!({ "madeWorktree": there }), done)
}

fn start_in(call: &mut Call, orchestrate: bool, named: Option<&str>, granted_on: Option<&str>, existing: Option<Worktree>, worktrees: &[Worktree]) -> Result<Finished, Error> {
    let Call { config, command, session_id, issue, .. } = *call;
    let identifier = issue.identifier.as_str();
    // Orca's repos are listed only once a worktree there may be relaunched, so a start run
    // again beside its working agent is answered without them.
    let may_start = match &existing {
        Some(worktree) => named == Some(worktree.id.as_str()) && worktree.agents.is_empty() && record_of(call.ports.records, &worktree.id)?.is_some_and(|r| r.issue.eq_ignore_ascii_case(identifier)),
        None => true,
    };
    let resolved = if may_start { find_repo(&call.ports.orca.repos(None)?, config.repos.repo_for(&issue.place)) } else { None };
    // An origin not read is no answer: the start that never began is tried again later.
    if let (Some(worktree), Some((repo_id, _))) = (&existing, &resolved)
        && worktree.host().is_some()
        && (worktree.origin.is_none() || local_origin(worktrees, repo_id).is_none())
    {
        return Err(Error::Later("the origin of its worktree's repo couldn't be read".into()));
    }
    if let Some(worktree) = &existing {
        // A host's repo has an ID of its own: one there is the repo's by its origin.
        let ours = may_start
            && resolved.as_ref().is_some_and(|(repo_id, _)| match worktree.host() {
                None => *repo_id == worktree.repo_id,
                Some(_) => worktree.origin.as_deref().zip(local_origin(worktrees, repo_id)).is_some_and(|(theirs, ours)| theirs.eq_ignore_ascii_case(&ours)),
            });
        if !ours {
            trace::event(Work::HasWorktree { issue: command.identifier.clone() });
            let launched = !worktree.agents.is_empty();
            return Ok(started(worktree, worktree.origin.clone(), false, launched));
        }
    }
    let Some((repo_id, repo)) = resolved else {
        trace::event(Work::NoRepo { issue: command.identifier.clone() });
        return Ok(Finished::failed_with(json!({ "madeWorktree": false })));
    };
    let (answering, project) = (call.ports.orca.answering(), local_origin(worktrees, &repo_id).map(|origin| project_id(&origin)));
    let at = match existing {
        Some(worktree) => Where::Existing(worktree.id),
        None => Where::New(granted(call.ports, &repo_id, project.as_deref(), granted_on, &answering)?),
    };
    // Moved to started once, before the pickup, so a start run again finds it started.
    if issue.state_type.is_unstarted() {
        call.ports.linear.start_issue(issue)?;
    }
    let wording = format!("{}[{identifier}]({})…", voice::STARTING_IN, issue.url);
    pick_up(call.ports, command, session_id, None, &wording)?;
    let made = matches!(at, Where::New(_));
    let host = match &at {
        Where::Existing(worktree_id) => host_of(worktree_id).map(String::from),
        Where::New(Place::On { host, .. }) => Some(host.to_string()),
        Where::New(Place::Here { .. }) => None,
    };
    match start_worker(call, &repo, orchestrate, at, None) {
        Ok(worker) => {
            if let Work::Started { rung, .. } = &worker {
                call.ports.shared.timing.started(&command.issue_id, identifier, &command.id, issue.size(), rung);
            }
            trace::event(worker);
            // Read on its host alone. A listing that fails now doesn't undo the start: it is
            // said without its worktree.
            match call.ports.orca.listing_on(host.as_deref()).ok().and_then(|listing| linked_in(&listing.worktrees, identifier).cloned()) {
                Some(worktree) => Ok(started(&worktree, local_origin(worktrees, &repo_id), made, true)),
                None => Ok(Finished::ok(json!({ "worktree": Value::Null, "madeWorktree": made, "launched": true }))),
            }
        }
        Err(error) if matches!(error, Error::RateLimited { .. }) || error.later() => Err(error),
        Err(error) => {
            // None when Orca couldn't say, which counts as one made.
            let there = linked_worktree(call.ports, identifier).map_or(true, |w| w.is_some());
            Ok(failed_and_said(call.ports, command, session_id, voice::COULDNT_START, &error, json!({ "madeWorktree": there })))
        }
    }
}

// Whether a worker in `repo` traces its session: `listen`'s config decides, not the host's
// copy, so a change reaches every host at the next launch.
fn traces(config: &Config, repo: Option<&str>) -> bool {
    repo.and_then(|repo| config.repos.entry(repo)).is_some_and(|entry| entry.trace)
}

fn start_worker(call: &mut Call, repo: &str, orchestrate: bool, at: Where, then: Option<Resume>) -> Result<Work, Error> {
    let Call { config, session_id, issue, .. } = *call;
    // A name reaches a path or a command line, so it must be the plain value it claims to be.
    let (identifier, repo) = (&IssueKey::check(&issue.identifier)?, &RepoName::check(repo)?);
    let host = host_name(&at, &call.ports.orca.answering());
    // Chosen once, from the issue as read now: the worker keeps it, and its host's setup
    // renders its briefs from it.
    let (rung, notes) = config.ladder.choose(issue.size(), &issue.labels);
    let (plan_first, review) = (plans_first(issue), review_size(issue, config.review.deep_at));
    let how = How::Start { plan_first, orchestrate, auto_merge: auto_merges(issue), review, then };
    let worker = Worker { session_id: session_id.clone(), identifier: identifier.clone(), repo: Some(repo.clone()), launch: Some(rung.launch.clone()), build: prebuilt::commit(), trace: traces(config, Some(repo)), timing: config.timing, how };
    let rung_from = if review == ReviewSize::Unsized { RungFrom::NoEstimate } else { RungFrom::Claim };
    let mut problems: Vec<Problem> = notes.into_iter().map(Problem::Rung).collect();
    let worktree_id = create_worktree(call.ports, &worker, &rung.launch, rung_from, at)?;
    let name = worktree_name(issue);
    // The worker runs either way: a worktree not named only looks plainer.
    if let Err(error) = call.ports.orca.name_worktree(&worktree_id, &name) {
        problems.push(Problem::NotNamed(error));
    }
    // Only a phrase: should it fail, the worker runs all the same.
    let text = format!("{} ({})", voice::say(voice::READING_ISSUE), rung.running_as());
    if let Err(error) = call.ports.linear.send(&ActivityInput::text(session_id, ActivityType::Thought, &text, true)) {
        problems.push(Problem::FirstThought(error));
    }
    let mode = if orchestrate {
        Mode::SubIssues
    } else if plan_first {
        Mode::PlanFirst
    } else {
        Mode::Work
    };
    Ok(Work::Started { mode, rung: rung.name.clone(), effort: rung.launch.effort.clone(), worktree: name, host, problems })
}

// The name of the host a worker starts on, None on this machine; an ID Orca no longer lists for
// it stands for itself.
fn host_name(at: &Where, answering: &[Host]) -> Option<String> {
    let id = match at {
        Where::New(Place::Here { .. }) => return None,
        Where::New(Place::On { host, .. }) => *host,
        Where::Existing(worktree) => split_worktree(worktree)?.0?,
    };
    Some(answering.iter().find(|host| host.id == id).map_or_else(|| id.to_string(), |host| host.name.trim().to_string()))
}

// Where a worker starts: the worktree of a start that never began, or a new one at its place.
enum Where<'a> {
    Existing(String),
    New(Place<'a>),
}

// Starts the worker in its own worktree, made at its place or the one of a start that never
// began, on the line that sets it up on its own host (setup.rs). Any error after a worktree
// made here exists removes it first, so no worker is left on an issue whose start failed; an
// existing one stays as it was.
fn create_worktree(ports: &mut Ports, worker: &Worker, launch: &Launch, rung_from: RungFrom, at: Where) -> Result<String, Error> {
    let identifier = &worker.identifier;
    let command = worker.command()?;
    let made = matches!(at, Where::New(_));
    let worktree_id = match at {
        Where::Existing(id) => id,
        Where::New(place) => ports.orca.create_worktree(place, &identifier.to_ascii_lowercase(), &["--linear-issue", identifier.as_str(), "--no-parent"])?.0,
    };
    let failed = |ports: &mut Ports, error: Error| if made { removed_after(ports, &worktree_id, error) } else { error };
    // A setup still running there, which Orca lists as no agent until the worker's agent starts, or its
    // shell left after it stopped, is closed first: never two setups in one worktree.
    if !made {
        ports.orca.close_all(&worktree_id)?;
    }
    // Written once Orca has named the worktree, or again for this start in one that exists,
    // and before the launch, as the host's setup finds its worktree by it. Should this write
    // fail, the start fails: a worktree made here is removed, rather than left as one
    // Tolkster would never stop for its issue's sake.
    let record = WorkerRecord { issue: identifier.to_string(), worktree_id: worktree_id.clone(), session_id: worker.session_id.to_string(), at: iso(now_ms()), launch: launch.clone(), rung_from };
    if let Err(error) = ports.records.start_worker(&record) {
        return Err(failed(ports, error));
    }
    // The worker itself comes last, once every run that lists its worktree can find the
    // record. A terminal that fails to open fails the start the same way.
    if let Err(error) = ports.orca.start_agent(&worktree_id, &command) {
        return Err(failed(ports, error));
    }
    Ok(worktree_id)
}

// Should the removal fail too, the worktree stays, with no worker in it, as one made by hand.
// Its record goes with it, as one launched lately takes a slot listed or not; one that can't
// be forgotten holds it only until SETTING_UP_MS ends.
fn removed_after(ports: &mut Ports, worktree_id: &str, error: Error) -> Error {
    match ports.orca.remove_worktree(worktree_id) {
        Ok(()) => {
            let _ = ports.records.forget_worker(worktree_id);
            error
        }
        Err(removing) => error.and_then_failed(&format!("and removing its worktree {worktree_id} failed: {removing}")),
    }
}

// The repo's name on the worktree's host, which places the worker's settings there: None
// when Orca's repos can't be listed, and the worker is brought back without its hook.
fn repo_of(orca: &mut dyn Orca, worktree: &Worktree) -> Option<RepoName> {
    orca.repos(worktree.host()).ok().and_then(|repos| repo_name(&repos, &worktree.repo_id)).and_then(|name| RepoName::parse(&name))
}

// The worker's record of the worktree, for its launch.
fn record_of(records: &dyn Records, worktree_id: &str) -> Result<Option<WorkerRecord>, Error> {
    Ok(records.workers()?.into_iter().find(|r| r.worktree_id == worktree_id))
}

// The worker's record of the worktree, when it is for this issue.
fn recorded(records: &dyn Records, worktree_id: &str, identifier: &str) -> Result<Option<WorkerRecord>, Error> {
    Ok(record_of(records, worktree_id)?.filter(|r| r.issue.eq_ignore_ascii_case(identifier)))
}

// The record of a worktree whose agent is gone on a fresh listing of its host. None when one
// runs, when the listing or the record can't be read, or when the record isn't for this issue.
fn exited_since(ports: &mut Ports, worktree_id: &str, identifier: &str) -> Option<WorkerRecord> {
    let listed = ports.orca.listing_at(worktree_id).ok()?.terminals;
    if agent_in(&listed, worktree_id) {
        return None;
    }
    recorded(ports.records, worktree_id, identifier).ok().flatten()
}

// Brings the exited worker back in its worktree (`resumeWorker`), its session continued on
// its host (setup.rs), told by a fixed line to read the session's request with `tolkster messages`, or, for a
// parent, that its sub-issues have finished or that some were added it hasn't chained. One
// whose agent runs already was resumed by a run of this command before, and is `ok`. A parent's
// worker whose worktree is gone is started again, in a new one (`restart`).
pub fn resume(config: &Config, ports: &mut Ports, command: &Command, session_id: &Id, why: Resume) -> Result<Finished, Error> {
    let session = session_on_issue(ports, command, session_id, false)?;
    let issue = issue_of(&session)?;
    let done = resume_in(&mut Call { config, ports, command, session_id, issue }, why);
    said_if_failed(ports, command, session_id, voice::COULDNT_DELIVER, json!({}), done)
}

fn resume_in(call: &mut Call, why: Resume) -> Result<Finished, Error> {
    let Call { command, session_id, issue, .. } = *call;
    let identifier = issue.identifier.as_str();
    let (what, wording) = match why {
        Resume::Message => ("your reply", voice::say(voice::READING_REPLY)),
        Resume::ChildrenEnd => ("the sub-issues' end", voice::say(voice::READING_ISSUE)),
        Resume::ChildrenAdded => ("the sub-issues added", voice::say(voice::READING_ISSUE)),
    };
    let Some(worktree) = linked_worktree(call.ports, identifier)? else {
        // An orchestrator ends with no branch or PR, so its worktree holds no work and is easy
        // to lose: a resume for the sub-issues, or an owner's message on an issue that has some,
        // starts a new orchestrator on the session. Any other worker's work is lost with its worktree.
        if why == Resume::Message && issue.children.is_empty() && !issue.more_children {
            trace::event(Work::NoWorktree { issue: command.identifier.clone(), purpose: Purpose::Resume });
            return Ok(Finished::failed());
        }
        return restart(call, why, what, &wording);
    };
    if agent_in(&call.ports.orca.listing_at(&worktree.id)?.terminals, &worktree.id) {
        return Ok(Finished::ok(json!({ "launched": true })));
    }
    // Its host's setup runs only in a worktree Tolkster's record holds (setup.rs): one made by
    // hand has no worker to bring back.
    let Some(record) = recorded(call.ports.records, &worktree.id, identifier)? else {
        trace::event(Work::NoWorkerRecord { issue: command.identifier.clone(), pr: None });
        return Ok(Finished::failed());
    };
    pick_up(call.ports, command, session_id, Some(what), &wording)?;
    relaunched(call, &worktree, &record, IssueKey::check(identifier)?, How::Continue(why))?;
    Ok(Finished::ok(json!({ "launched": true })))
}

// A worker brought back in `worktree` from its record, which may re-pick its rung.
fn relaunched(call: &mut Call, worktree: &Worktree, record: &WorkerRecord, identifier: IssueKey, how: How) -> Result<(), Error> {
    let Call { config, session_id, issue, .. } = *call;
    let (launch, rerecord) = relaunch(Some(record), &worktree.id, issue, &config.ladder);
    // Should the new record not be written, the relaunch still takes the new rung, and the
    // next one picks it again.
    if let Some(Err(error)) = rerecord.map(|record| call.ports.records.start_worker(&record)) {
        trace::event(Work::NotDone { what: Undone::Rung, error });
    }
    let repo = repo_of(call.ports.orca, worktree);
    let trace = traces(config, repo.as_deref());
    let worker = Worker { session_id: session_id.clone(), identifier, repo, launch, build: prebuilt::commit(), trace, timing: config.timing, how }.command()?;
    // As a start's relaunch: no setup is left running beside it.
    call.ports.orca.close_all(&worktree.id)?;
    call.ports.orca.start_agent(&worktree.id, &worker)?;
    Ok(())
}

// A round's worker's card goes back to in progress with its issue. Should it not, the worker
// runs all the same.
fn in_progress(ports: &mut Ports, worktree_id: &str) {
    if let Err(error) = ports.orca.set_status(worktree_id, Status::InProgress) {
        trace::event(Work::NotDone { what: Undone::CardStatus, error });
    }
}

// A new orchestrator in a new worktree for a parent whose own is gone, on the brief and then
// the line a resume for `why` would have typed. Its result names the worktree, as a start's does. A run
// again finds the worktree it made, and its agent, in `resume_in`.
fn restart(call: &mut Call, why: Resume, what: &str, wording: &str) -> Result<Finished, Error> {
    let Call { config, command, session_id, issue, .. } = *call;
    let identifier = issue.identifier.as_str();
    let worktrees = call.ports.orca.worktrees()?;
    let Some((repo_id, repo)) = find_repo(&call.ports.orca.repos(None)?, config.repos.repo_for(&issue.place)) else {
        trace::event(Work::NoRepo { issue: command.identifier.clone() });
        return Ok(Finished::failed());
    };
    // Its slot is held already, as a resume's: it starts on this machine.
    pick_up(call.ports, command, session_id, Some(what), wording)?;
    let started_line = start_worker(call, &repo, true, Where::New(Place::Here { repo_id: &repo_id }), Some(why))?;
    trace::event(started_line);
    // A listing that fails now doesn't undo the start: it is said without its worktree.
    Ok(match linked_worktree(call.ports, identifier).ok().flatten() {
        Some(worktree) => started(&worktree, local_origin(&worktrees, &repo_id), true, true),
        None => Finished::ok(json!({ "worktree": Value::Null, "madeWorktree": true, "launched": true })),
    })
}

// A review round (`startRound`): a fixed line typed to the running worker, or the exited one
// brought back in a new session on its round brief, both sending it to `tolkster pr feedback`
// and `tolkster pr checks`, or for a rebase alone, on its rebase brief. Nothing is typed into
// an agent waiting at a prompt, which could answer it: that is `held`.
pub fn round(config: &Config, ports: &mut Ports, command: &Command, session_id: Option<&Id>, pr: &OnPr) -> Result<Finished, Error> {
    let Some(session_id) = session_id else {
        trace::event(Work::NoSession);
        return Ok(Finished::failed());
    };
    let session = session_on_issue(ports, command, session_id, false)?;
    let issue = issue_of(&session)?;
    round_in(&mut Call { config, ports, command, session_id, issue }, pr)
}

// The PR a round is on, as the machine ordered it.
pub struct OnPr<'a> {
    pub repo: &'a str,
    pub number: u64,
    pub head: Option<&'a str>,
    // The round's number, which says whether it is the repo's last.
    pub round: Option<u32>,
    pub rebase_only: bool,
}

fn round_in(call: &mut Call, &OnPr { repo, number, head, round, rebase_only }: &OnPr) -> Result<Finished, Error> {
    let Call { config, command, session_id, issue, .. } = *call;
    let identifier = command.identifier.as_str();
    // Found in a listing of every host, then read again on its own just before anything is typed.
    let listing = match linked_worktree(call.ports, identifier)? {
        Some(worktree) => call.ports.orca.listing_at(&worktree.id)?,
        None => Listing::default(),
    };
    let Some(worktree) = linked_in(&listing.worktrees, identifier).cloned() else {
        trace::event(Work::NoWorktree { issue: command.identifier.clone(), purpose: Purpose::Round(number) });
        return Ok(Finished::failed());
    };
    let max_rounds = config.review_for(crate::plain::split_repo(repo).map(|(_, name)| name)).max_rounds;
    let last = round.is_some_and(|r| i64::from(r) >= max_rounds);
    let round = if rebase_only { review::Round::Rebase } else { review::Round::Review { last } };
    let typed = review::line(number, &command.identifier, session_id, round)?;
    let terminals = listing.terminals;
    let running = agent_in(&terminals, &worktree.id);
    if running && waits_in(&worktree, &terminals) {
        return Ok(Finished::held());
    }
    // A running agent with no handle to type to isn't brought back beside itself either.
    let terminal = agent_terminal(&terminals, &worktree.id);
    if running && terminal.is_none() {
        trace::event(Work::NoHandle { issue: command.identifier.clone(), pr: number });
        return Ok(Finished::failed_with(json!({ "unnamed": true })));
    }
    // An exited worker is relaunched on its host only where Tolkster's record holds its worktree.
    let record = if terminal.is_none() { recorded(call.ports.records, &worktree.id, identifier)? } else { None };
    if terminal.is_none() && record.is_none() {
        trace::event(Work::NoWorkerRecord { issue: command.identifier.clone(), pr: Some(number) });
        return Ok(Finished::failed());
    }
    let short = head.map(crate::plain::short_sha);
    let what = review::picked_up(number, short, round);
    pick_up(call.ports, command, session_id, Some(&what), &format!("Picking up {what}…"))?;
    // The issue moves back to started with its worker; should the move fail, the worker is
    // still told.
    if let Err(error) = call.ports.linear.start_issue(issue) {
        trace::event(Work::NotDone { what: Undone::IssueStarted, error });
    }
    in_progress(call.ports, &worktree.id);
    // An exited worker is brought back on its host, in a new session on its round brief, not
    // the one that built the PR, and may re-pick its rung.
    let bring_back = |call: &mut Call, record: &WorkerRecord| -> Result<Option<Typed>, Error> {
        relaunched(call, &worktree, record, command.identifier.clone(), How::Round { number, round })?;
        Ok(None)
    };
    // A line typed into a running worker counts once its turn began. One left unproven may
    // sit in its input, or wait out the worker's turn, so it is never typed again, nor said
    // to have failed: the machine leaves the round in flight to it.
    let done = match (&terminal, &record) {
        (Some(terminal), _) => match call.ports.orca.send(&worktree.id, terminal, &typed, Proof::Turn) {
            // The agent exited after the listing, and Orca typed nothing: it is the exited
            // worker's case if none runs now and the record holds it. Any other failure
            // may have typed the line, which is never typed again.
            Err(unsent @ Error::Unsent(..)) => match exited_since(call.ports, &worktree.id, identifier) {
                Some(record) => bring_back(call, &record),
                None => Err(unsent),
            },
            sent => sent.map(Some),
        },
        (None, Some(record)) => bring_back(call, record),
        (None, None) => Err(Error::Failed(format!("{identifier}'s worker has neither a terminal nor a launch"))),
    };
    match done {
        Ok(None) => Ok(Finished::ok(json!({ "launched": true }))),
        Ok(Some(Typed::Delivered)) => Ok(Finished::ok(json!({}))),
        Ok(Some(Typed::Waiting)) => Ok(Finished::held()),
        Ok(Some(Typed::Unproven)) => Ok(Finished::failed_with(json!({ "typed": true }))),
        Err(error) => said_if_failed(call.ports, command, session_id, voice::COULDNT_RESUME, json!({}), Err(error)),
    }
}

pub mod machines;
mod turn;

pub use turn::turn;

#[cfg(test)]
pub mod tests;
