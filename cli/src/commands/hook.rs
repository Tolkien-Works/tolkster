use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Instant;

use ureq::Agent;

use crate::card::{self, Launched};
use crate::cli::{env, home_dir, token_file};
use crate::config::{files, owner};
use crate::error::Error;
use crate::exec::run_command;
use crate::git;
use crate::hook::redact::{self, Scrub};
use crate::hook::{self, BACKGROUND_FOR_MS, Call, Doing, MOST_BYTES, Report, Run, Tick, closes_terminal, commits, commits_since, committed, doing, ended, known, last_report, made_at, may_post_after, moments, moved_on, narrate, pairing, parse_call, parse_output, parse_state, pr_since, previous, reaches, report_through, run_of, sub_agents, tail_back, timing, trail, transcript_session, whereabouts};
use crate::linear::{ActivityContent, ActivityInput, ActivityType, PlanInput, update_plan, with_token};
use crate::machine::agent_state;
use crate::plain::{Id, relative};
use crate::post::{post_once, posted_of, send, send_all};
use crate::sessions;
use crate::state::{State, derived_uuid};
use crate::time::{iso, now_ms};
use crate::timing::{LAUNCH_ENV, launch_of};
use crate::voice::{self, At};

// The most of a hook's input read: a call's input can carry a whole file being written.
const INPUT_BYTES: u64 = 4 * 1024 * 1024;

// `tolkster hook tool`: a worker's hook, before each of its tool calls (harness::HOOKS), given
// the call on its standard input. Posts where the worker is and what the call does as an
// ephemeral action, so the next activity replaces it, in the session it reports in; for a
// call that pairs (hook::pairing), whose start `hook start` posts, it ticks instead. Exits
// 0 whatever happens to the post, so the worker never hears of it, and a failure prints one
// line of IDs only. Nothing is stored: where the worker is, the session and whether to stay
// quiet come from the transcript, and the worker from its launch's environment
// (card::launched). So its one call is the post, and a spent budget
// (`RateLimited`) is not tried again: the next call's post is the next try. In a traced
// session (hook::narrate) the trace's durable action stands for the call, so it posts no
// phrase, but for a paired call's ticks.
pub fn run(agent: &Agent, kind: &str, argv: &[String]) -> Result<u8, Error> {
    if !argv.is_empty() {
        return usage();
    }
    let started = now_ms();
    let began = Instant::now();
    let mut text = String::new();
    if io::stdin().take(INPUT_BYTES).read_to_string(&mut text).is_err() {
        return Ok(0);
    }
    match kind {
        "state" => {
            report_state(agent, &text, started, began);
            if let Some(transcript) = ended(&text) {
                report_timing(agent, &transcript);
            }
            return Ok(0);
        }
        "start" => {
            if report_start(agent, &text).is_err() {
                eprintln!("tolkster hook: no start posted");
            }
            // Last, so the trail and narration, which must land before the call's end, have
            // the hook's time first.
            if let Some(call) = parse_call(&text).filter(closes_terminal) {
                call.transcript.as_deref().into_iter().for_each(|transcript| report_timing(agent, transcript));
            }
            return Ok(0);
        }
        "after" => {
            let traced = parse_call(&text).is_some_and(|call| traced(&call));
            let committed = report_commit(agent, &text, traced).unwrap_or_else(|_| {
                eprintln!("tolkster hook: no Committed action");
                None
            });
            let reported = if traced { report_trace(agent, &text, committed.as_deref()) } else { report_outcome(agent, &text, started) };
            if reported.is_err() {
                eprintln!("tolkster hook: no outcome posted");
            }
            return Ok(0);
        }
        _ => {}
    }
    let Some(call) = parse_call(&text) else { return Ok(0) };
    if let Some(kind) = pairing(&call) {
        tick_call(agent, &call, kind);
        return Ok(0);
    }
    let Some(doing) = doing(&call) else { return Ok(0) };
    let Some((tail, whole)) = call.transcript.as_deref().and_then(|path| read_tail(path).ok()) else { return Ok(0) };
    let Some(newest) = last_report(&tail) else { return Ok(0) };
    if !may_post_after(&newest) || narrate::session().as_ref() == Some(&newest.session) {
        return Ok(0);
    }
    let at = whereabouts(&tail, whole, &call, &newest.session);
    if post(agent, &call, &doing, &at, &newest, started, (&tail, whole)).is_err() {
        eprintln!("tolkster hook: no post for tool call {}", Id::parse(&call.id).as_deref().unwrap_or("?"));
    }
    Ok(0)
}

// Never exit 2, which Claude Code takes as the hook blocking the agent: a worker's settings
// newer than the `tolkster` it runs only go unreported.
pub fn usage() -> Result<u8, Error> {
    eprintln!("usage: tolkster hook tool|start|state|after, with a harness hook's input on stdin");
    Ok(0)
}

// The transcript's end, back to the worker's newest post (hook::tail_back), and whether it
// is all of it.
fn read_tail(path: &Path) -> io::Result<(String, bool)> {
    read_back(path, |tail| last_report(tail).is_some())
}

fn read_back(path: &Path, holds: impl Fn(&str) -> bool) -> io::Result<(String, bool)> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut read = |from: u64| -> io::Result<String> {
        file.seek(SeekFrom::Start(from))?;
        let mut bytes = Vec::new();
        (&mut file).take(MOST_BYTES).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    let mut failed = None;
    let tail = tail_back(len, |from| read(from).map_err(|error| failed = Some(error)).ok(), holds);
    tail.ok_or_else(|| failed.unwrap_or_else(|| io::Error::other("unread")))
}

fn read_text(path: &Path) -> io::Result<String> {
    read_tail(path).map(|(text, _)| text)
}

// `tolkster hook state`: a worker's hook as its agent's state changes (harness::HOOKS), which
// tells the Durable Object at once, as the observation Orca's listing would make of it
// later (hook::parse_state). The hooks run in turn, the next only once this one has
// returned, so one agent's reports land in the order they happened, and the Durable Object
// drops one no newer than the last it took, by `started`, this machine's clock. It also says
// how long ago that was by `began`, the monotonic one, as that is what it sets against a
// listing, so this machine's clock is never compared with `listen`'s. A failure leaves it to
// the listing. Its one call is the report.
fn report_state(agent: &Agent, text: &str, started: i64, began: Instant) {
    let since = started - BACKGROUND_FOR_MS;
    let Some((observed, _)) = parse_state(text, since, |path| read_back(path, |tail| reaches(tail, since)).ok().map(|(tail, _)| tail)) else { return };
    let Some(worker) = card::launched() else { return };
    let reported = files().and_then(|files| {
        State::open(agent, &files)?.report_since(began, |elapsed| agent_state(observed, &worker.worktree_id, &iso(started), elapsed));
        Ok(())
    });
    if reported.is_err() {
        eprintln!("tolkster hook: no state reported");
    }
}

// The session's timing (hook::timing), from its whole transcript and its sub-agents', sent as
// the worker's report, which the relay forwards to `listen` (docs/timing.md): at each stop and
// the session's end, and before the call that closes its own terminal, after which no stop
// comes. `listen` keeps the fullest of a session's reports, so each is safe to repeat. Only a
// worker whose launch says how it was launched reports; a failure prints one fixed line.
fn report_timing(agent: &Agent, transcript: &Path) {
    let (Some(worker), Some(launch), Some(session)) = (card::launched(), env(LAUNCH_ENV).as_deref().and_then(launch_of), transcript_session(transcript)) else { return };
    let Ok(main) = std::fs::read_to_string(transcript) else { return };
    let since_launch = |path: &&std::path::PathBuf| std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).is_none_or(|t| t.as_millis() as i64 >= launch.1);
    let sub_agents = sub_agents(transcript).iter().filter(since_launch).filter_map(|path| std::fs::read_to_string(path).ok()).map(|text| moments(&text)).collect();
    let Some(summary) = timing::summary(&session, launch, moments(&main), sub_agents) else { return };
    let reported = files().and_then(|files| State::open(agent, &files)?.reported(crate::timing::report(&worker.worktree_id, &summary)));
    if reported.is_err() {
        eprintln!("tolkster hook: no timing reported");
    }
}

// How many of HEAD's newest moves are read for the commits a call or a trail line made: those
// since it, a rebase's picks included, can be many.
const MOVES: u32 = 200;

// `tolkster hook after`, a worker's hook after each call or sub-agent's start, whether or not
// it failed (harness::HOOKS), posts a commit's `Committed` action, then a paired call's
// outcome (`report_outcome`), or in a traced session the call's action (`report_trace`). After one that ran `git commit` and moved HEAD
// (hook::committed), it posts the `Committed` action with the commit's subject line,
// ephemeral, as the trail's line holds the commit (hook::trail), and marks the card as an
// action does. It posts in the session the worker was reporting in when it made the call,
// only one of the worker's issue that the owner started or that is Tolkster's own
// (`scoped`). It re-reads the transcript just before the post, and posts nothing once the
// worker's newest post is a question or an error. Traced, it posts nothing and reads nothing of
// Linear, and gives the trace's action the subject (`report_trace`).
fn report_commit(agent: &Agent, text: &str, traced: bool) -> Result<Option<String>, Error> {
    let Some(call) = parse_call(text).filter(commits) else { return Ok(None) };
    let Some(transcript) = call.transcript.as_deref() else { return Ok(None) };
    let Ok(tail) = read_text(transcript) else { return Ok(None) };
    let (Some(report), Some(made)) = (report_through(&tail, &call.id), made_at(&tail, &call.id)) else { return Ok(None) };
    let Some(session_id) = Id::parse(&report.session).filter(|_| may_post_after(&report)) else { return Ok(None) };
    let Some(worker) = card::launched() else { return Ok(None) };
    if !traced && !scoped(agent, &worker, &session_id)? {
        return Ok(None);
    }
    let Some((_, subject)) = committed(&git::last_moves(&mut run_command, &worker.root, MOVES)?, made) else { return Ok(None) };
    let Ok(now) = read_text(transcript) else { return Ok(None) };
    if last_report(&now).is_some_and(|newest| !may_post_after(&newest)) {
        return Ok(None);
    }
    if !traced {
        send(agent, &ActivityInput { ephemeral: true, ..ActivityInput::action(&session_id, voice::COMMITTED, &subject) })?;
    }
    // `pr create` sets the card after its answer, so one run since the commit may be setting
    // it now: the card then stays its.
    if !pr_since(&now, &call.id) {
        card::checkpoint(Some(&worker), &card::action(voice::COMMITTED, &subject));
    }
    Ok(Some(subject))
}

// It posts in the session it last posted in itself (hook::last_report), one its own
// `tolkster activity` posts in, and Linear takes an activity only in a session of Tolkster's.
// It posts nothing the call before it posted as it is (`repeats`): Linear would show no change.
fn post(agent: &Agent, call: &Call, doing: &Doing, at: &At, newest: &Report, started: i64, read: (&str, bool)) -> Result<(), Error> {
    let Some(worker) = card::launched() else { return Ok(()) };
    let root = worker.root.as_str();
    let Some(id) = Id::parse(&newest.session) else { return Ok(()) };
    let id = id.as_str();
    // Read again just before the post: a call since this one may have posted a
    // question or an error after the transcript was read. What is left is the moment between
    // this read and the post.
    let Some(transcript) = call.transcript.as_deref() else { return Ok(()) };
    if read_text(transcript).map_or(true, |now| moved_on(&now, &call.id, started)) {
        return Ok(());
    }
    let (Some(phrase), Some(head)) = (phrase(doing, root), voice::head(at)) else { return Ok(()) };
    if repeats(call, (&phrase, &head), read, &newest.session, root) {
        return Ok(());
    }
    send(agent, &ActivityInput { ephemeral: true, ..ActivityInput::action(id, &head, &phrase) }).map(|_| ())
}

// The phrase a call posts: its kind's, naming its file when plain and inside the worktree.
fn phrase(doing: &Doing, root: &str) -> Option<String> {
    let file = doing.file.as_deref().and_then(|file| relative(file, root));
    voice::tool(doing.kind, file.as_deref())
}

// Whether the call just before `call`, finished, posted this phrase under this head, with
// nothing posted between that replaces it: a paired call's tick or outcome, a commit's
// `Committed`, a trail line `call` is an edge for. Worked out from the transcript alone, as
// the hook stores nothing; a post that failed is seen as made, and the next call's is the
// next try.
fn repeats(call: &Call, (phrase_now, head_now): (&str, &str), (tail, whole): (&str, bool), session: &str, root: &str) -> bool {
    let Some(before) = previous(tail, &call.id).map(|before| before.as_call(call)) else { return false };
    if pairing(&before).is_some() || commits(&before) || trail::edge(tail, call).is_some() {
        return false;
    }
    let Some(doing) = doing(&before) else { return false };
    phrase(&doing, root).as_deref() == Some(phrase_now) && voice::head(&whereabouts(tail, whole, &before, session)).as_deref() == Some(head_now)
}

// `tolkster hook start`: a worker's hook before each shell call or sub-agent, which the call
// waits on (harness::HOOKS), so what it posts lands before anything the call's end posts, and
// before what the call itself sends, such as a response. It puts in the trail the lines the
// call is an edge for (`trail`), and in a traced session the text before the worker's own
// post (`narrate_post`); before a paired call, it then posts the call's action, ephemeral,
// with no result.
fn report_start(agent: &Agent, text: &str) -> Result<(), Error> {
    let Some(call) = parse_call(text).filter(|call| !call.sub_agent) else { return Ok(()) };
    let Some(worker) = card::launched() else { return Ok(()) };
    let Some((tail, whole)) = call.transcript.as_deref().and_then(|path| read_tail(path).ok()) else { return Ok(()) };
    trail(agent, &call, &tail, &worker);
    if narrate_post(agent, &call, &tail, &worker).is_err() {
        eprintln!("tolkster hook: no narration posted for tool call {}", Id::parse(&call.id).as_deref().unwrap_or("?"));
    }
    let Some(kind) = pairing(&call) else { return Ok(()) };
    let Some(newest) = last_report(&tail).filter(may_post_after) else { return Ok(()) };
    let Some(session) = Id::parse(&newest.session) else { return Ok(()) };
    let at = whereabouts(&tail, whole, &call, &session);
    match paired(&session, kind, &at, None, None) {
        Some(input) => send(agent, &input).map(|_| ()),
        None => Ok(()),
    }
}

// Puts in the trail, durable, the lines the call is an edge for (hook::trail), each once in
// the session, by an ID made from its key, and only where `scoped`, after the plan as the
// hook moved it on, when Linear's checklist lacks that. The tail as first read says whether
// it may be one, so most calls end there, having read nothing more and asked nothing of
// Linear; one that may be reads back to the session's first post. The lines
// Linear holds already, by one read, aren't tried again; a line that fails doesn't stop the
// rest, and the next edge puts it in, as the response's own puts in the plan.
fn trail(agent: &Agent, call: &Call, tail: &str, worker: &Launched) {
    let Some(session) = trail::edge(tail, call) else { return };
    let Some(id) = Id::parse(&session) else { return };
    let Some(transcript) = call.transcript.as_deref() else { return };
    let Ok((tail, _)) = read_back(transcript, |tail| trail::holds_start(tail, &session)) else { return };
    // Read where the hook runs, inside the worktree, to tell the commits made; none read
    // leaves a commit to a call that returned unfailed.
    let moves = git::last_moves(&mut run_command, &call.cwd, MOVES).ok();
    let trail::Edge { plan, lines } = trail::lines(&tail, call, &session, moves.as_deref());
    if plan.is_none() && lines.is_empty() {
        return;
    }
    let posted = scoped(agent, worker, &id).and_then(|scoped| {
        if !scoped {
            return Ok(0);
        }
        let planned = plan.map_or(Ok(()), |steps| {
            let input = PlanInput { agent_session_id: id.to_string(), steps };
            with_token(agent, |token| update_plan(agent, token, &input))
        });
        if lines.is_empty() {
            return planned.map(|_| 0);
        }
        let read;
        let moves = match &moves {
            Some(moves) => moves,
            None => {
                read = git::last_moves(&mut run_command, &worker.root, MOVES)?;
                &read
            }
        };
        let keys: Vec<String> = lines.iter().map(|line| format!("trail {id} {}", derived_uuid(line.key()))).collect();
        let held = posted_of(agent, &keys).unwrap_or_else(|| vec![false; keys.len()]);
        let failed = lines.iter().zip(&keys).zip(held).filter(|((line, key), held)| {
            let commits = line.since_ms.map_or_else(Vec::new, |since| commits_since(moves, since, line.until_ms));
            !held && trail::activity(&id, line, &commits).is_some_and(|input| post_once(agent, key, input).is_err())
        });
        Ok(failed.count() + usize::from(planned.is_err()))
    });
    if !matches!(posted, Ok(0)) {
        eprintln!("tolkster hook: the plan or a trail line wasn't posted for tool call {}", Id::parse(&call.id).as_deref().unwrap_or("?"));
    }
}

// Before the worker's own post in the session its launch traces, the text it wrote since its
// last call, durable.
fn narrate_post(agent: &Agent, call: &Call, tail: &str, worker: &Launched) -> Result<(), Error> {
    let Some(session) = narrate::posts_in(call).filter(|session| narrate::session().as_ref() == Some(session)) else { return Ok(()) };
    match narrate::thought(tail, &call.id, &scrub(worker)?) {
        Some(body) => send(agent, &ActivityInput::text(&session, ActivityType::Thought, &body, false)).map(drop),
        None => Ok(()),
    }
}

// Whether the call was made in the session the worker's launch traces (narrate::traced).
fn traced(call: &Call) -> bool {
    let Some(session) = narrate::session() else { return false };
    call.transcript.as_deref().and_then(|path| read_text(path).ok()).is_some_and(|tail| narrate::traced(&tail, &call.id, &session))
}

// `hook after` for a traced call: the text the worker wrote before it, as a durable thought,
// then its durable action with its outcome, a commit's named by `committed`, both in one
// request, only while the worker hasn't since posted a response, a question or an error. A
// call `narrate::left_out` posts neither. A paired call that went to the background gets its
// outcome from `tick_call`.
fn report_trace(agent: &Agent, text: &str, committed: Option<&str>) -> Result<(), Error> {
    let (Some(call), Some(output)) = (parse_call(text), parse_output(text)) else { return Ok(()) };
    if narrate::left_out(&call) {
        return Ok(());
    }
    let Some(worker) = card::launched() else { return Ok(()) };
    let Some(transcript) = call.transcript.as_deref() else { return Ok(()) };
    let Ok(tail) = read_text(transcript) else { return Ok(()) };
    let Some(session) = session_of(&tail, &call.id).filter(|session| narrate::session().as_deref() == Some(session.as_str())) else { return Ok(()) };
    let scrub = scrub(&worker)?;
    let thought = narrate::thought(&tail, &call.id, &scrub);
    let action = narrate::action(&call, &output, committed, &scrub);
    if read_text(transcript).map_or(true, |now| narrate::quieted(&now, &call.id)) {
        return Ok(());
    }
    let thought = thought.map(|body| ActivityInput::text(&session, ActivityType::Thought, &body, false));
    let action = action.map(|(action, parameter, result)| ActivityInput { content: ActivityContent::Action { action, parameter, result: Some(result) }, ..ActivityInput::action(&session, "", "") });
    send_all(agent, &thought.iter().chain(&action).collect::<Vec<_>>())
}

// The redactor for what this worker's trace posts: its worktree, this machine's home and
// names, and every secret it holds, in credentials.json, the cached token and the
// environment. Credentials that can't be read post nothing.
fn scrub(worker: &Launched) -> Result<Scrub, Error> {
    let files = files()?;
    let mut hosts: Vec<String> = ["COMPUTERNAME", "HOSTNAME"].into_iter().filter_map(env).collect();
    hosts.extend(std::fs::read_to_string("/etc/hostname").ok().map(|name| name.trim().to_string()));
    let token = token_file().ok().and_then(|path| std::fs::read_to_string(path).ok()).and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok()).unwrap_or_default();
    // The hook has the agent's whole environment: each value named as a secret's is one.
    // `vars_os`, as `vars` panics on a variable that isn't Unicode, printing it.
    let held: Vec<String> = std::env::vars_os().filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?))).filter(|(name, _)| redact::key(name) || name == "TOLKSTER_CLIENT_ID").map(|(_, value)| value).collect();
    let secrets = serde_json::json!([files.credentials, token, held]);
    Ok(Scrub::new(&worker.root, &home_dir()?.display().to_string(), &hosts, &secrets))
}

// After a paired call that returned, not one that went to the background, which `tick_call`
// sees out: its outcome, ephemeral, as the trail's line holds it, in the session the worker
// was reporting in when it made the call, and only while the worker hasn't moved on, whose
// next call's line it would cover.
fn report_outcome(agent: &Agent, text: &str, started: i64) -> Result<(), Error> {
    let Some(call) = parse_call(text).filter(|call| !call.launched) else { return Ok(()) };
    let Some(kind) = pairing(&call) else { return Ok(()) };
    let Some(transcript) = call.transcript.as_deref() else { return Ok(()) };
    let Ok(tail) = read_text(transcript) else { return Ok(()) };
    let Some(session) = session_of(&tail, &call.id) else { return Ok(()) };
    if card::launched().is_none() {
        return Ok(());
    }
    // Read again just before the post, as `post` does.
    let Some((now, whole)) = read_tail(transcript).ok().filter(|(now, _)| session_of(now, &call.id).as_ref() == Some(&session)) else { return Ok(()) };
    if moved_on(&now, &call.id, started) {
        return Ok(());
    }
    let at = whereabouts(&now, whole, &call, &session);
    post_outcome(agent, &session, kind, &at, known(call.tool, call.command.as_deref()).then_some(call.failed))
}

// The session the worker made the call `id` in, while it may still post there: its newest
// post is there, and is no question or error.
fn session_of(tail: &str, id: &str) -> Option<Id> {
    let then = report_through(tail, id)?;
    let newest = last_report(tail)?;
    (newest.session == then.session && may_post_after(&newest)).then(|| Id::parse(&then.session)).flatten()
}

// Ticks a paired call while it runs (hook::tick), in the session the worker made it in, and
// posts the outcome of one that went to the background. It stops once the worker's worktree
// is gone, as a reaped worker's call never ends in its transcript. Each post follows a fresh
// read of the transcript.
fn tick_call(agent: &Agent, call: &Call, kind: &str) {
    let Some(worker) = card::launched() else { return };
    let Some(transcript) = call.transcript.as_deref() else { return };
    let command = call.command.as_deref();
    hook::tick(&call.id, || read_text(transcript).ok(), std::thread::sleep, |_, tick| {
        if !Path::new(&worker.root).is_dir() {
            return false;
        }
        let Ok((tail, whole)) = read_tail(transcript) else { return false };
        let Some(session) = session_of(&tail, &call.id) else { return true };
        let at = whereabouts(&tail, whole, call, &session);
        let posted = match tick {
            Tick::Minutes(n) if run_of(&tail, &call.id) == Run::Running => paired(&session, kind, &at, Some(n), None).map_or(Ok(()), |input| send(agent, &input).map(|_| ())),
            Tick::Minutes(_) => Ok(()),
            Tick::Ended(failed) => post_outcome(agent, &session, kind, &at, known(call.tool, command).then_some(failed)),
        };
        if posted.is_err() {
            eprintln!("tolkster hook: no tick for tool call {}", Id::parse(&call.id).as_deref().unwrap_or("?"));
        }
        true
    });
}

// Whether a durable post may go in `session_id`, as for `tolkster activity`'s own: one on the
// worker's issue, that the owner started or that is Tolkster's own.
fn scoped(agent: &Agent, worker: &Launched, session_id: &Id) -> Result<bool, Error> {
    let files = files()?;
    let state = State::open(agent, &files)?;
    let owner = owner(&files)?;
    let session = with_token(agent, |token| sessions::read(agent, token, Some(&state), session_id, sessions::Read::Owner))?;
    let on_its_issue = session.issue.as_ref().is_some_and(|issue| issue.identifier == worker.issue);
    Ok(on_its_issue && (session.is_owners(&owner) || session.tolksters_own()))
}

// A paired call's action, ephemeral: where the worker is, then the call's phrase, with the
// minutes it has run while it ticks, and its outcome once it has ended (`failed`, None when
// its end doesn't say). None for a kind or a phase voice.json doesn't have.
fn paired(session: &str, kind: &str, at: &At, minutes: Option<u64>, failed: Option<Option<bool>>) -> Option<ActivityInput> {
    let (head, phrase) = (voice::head(at)?, voice::action(kind)?);
    Some(ActivityInput {
        agent_session_id: session.into(),
        content: ActivityContent::Action {
            action: head,
            parameter: minutes.map_or_else(|| phrase.to_string(), |n| format!("{phrase} · {n} min")),
            result: failed.map(|failed| voice::outcome(kind, failed).into()),
        },
        ephemeral: true,
        id: None,
        options: vec![],
    })
}

fn post_outcome(agent: &Agent, session: &Id, kind: &str, at: &At, failed: Option<bool>) -> Result<(), Error> {
    match paired(session, kind, at, None, Some(failed)) {
        Some(input) => send(agent, &input).map(|_| ()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests;
