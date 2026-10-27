// A conversation runs as turns (docs/dispatcher.md), each in a terminal of its own in the
// repo's one shared conversations checkout: an Orca worktree linked to no issue, so it is
// never taken for a worker.
//
// Linear text never reaches the command line: only IDs, times and paths do. The owner's
// messages reach the turn on Claude's standard input, and nothing else from Linear does.

use std::path::{Path, PathBuf};

use crate::brief::{self, ThreadBrief};
use crate::exec::Run;
use crate::harness;
use crate::cli::write_file;
use crate::config::Config;
use crate::error::Error;
use crate::git;
use crate::orca::{Orca, Place, Terminal, Worktree, find_repo, open_terminals, terminal_worktrees};
use crate::args::{Kind, Parsed};
use crate::plain::{CommandLine, Id, IssueKey, Line, RepoName, Sha, Time, WorktreeId};
use crate::ports::Ports;
use crate::runtime::Checkouts;
use crate::post::answer_key;
use crate::request::{Thread, ThreadReply};
use crate::state::{CheckoutRecord, Conversation, Records, State};
use crate::time::{iso, now_ms};
use crate::trace::{self, Refresh, Work};
use crate::voice;

pub const CHECKOUT_NAME: &str = "tolkster-conversations";

#[derive(Clone, Debug, PartialEq)]
pub struct Checkout {
    pub worktree_id: WorktreeId,
    pub path: String,
}

// Created when the repo has none recorded, or Orca no longer lists it. A recorded checkout
// is never touched here, whatever HEAD is on: a terminal may be open in it. True when just
// created, and so refreshed. The caller holds `Checkouts::lock`.
pub fn checkout(orca: &mut dyn Orca, records: &[CheckoutRecord], repo_id: &str, worktrees: &[Worktree], record: &mut dyn FnMut(&CheckoutRecord) -> Result<(), Error>, now_ms: i64) -> Result<(Checkout, bool), Error> {
    let recorded = records.iter().find(|r| r.repo_id == repo_id).and_then(|r| worktrees.iter().find(|w| w.id == r.worktree_id && w.is_local));
    match recorded {
        Some(worktree) => Ok((Checkout { worktree_id: worktree.id.clone(), path: worktree.path.clone() }, false)),
        None => create(orca, repo_id, record, now_ms).map(|checkout| (checkout, true)),
    }
}

// The terminal Orca opens in a new worktree would keep it busy, and never refreshed, so it
// is closed before the checkout is recorded: after that, no terminal in it is closed from
// here. A step failing after recording fails the turn, and the next refreshes the checkout.
fn create(orca: &mut dyn Orca, repo_id: &str, record: &mut dyn FnMut(&CheckoutRecord) -> Result<(), Error>, now_ms: i64) -> Result<Checkout, Error> {
    let (worktree_id, path) = orca.create_worktree(Place::Here { repo_id }, CHECKOUT_NAME, &["--no-parent", "--setup", "skip"])?;
    let checkout = Checkout { worktree_id, path };
    // Unrecorded, it would never be found again, and its name would stand in the way of the
    // next one.
    let recorded = orca.close_all(&checkout.worktree_id).and_then(|()| record(&CheckoutRecord { repo_id: repo_id.into(), worktree_id: checkout.worktree_id.clone(), at: iso(now_ms) }));
    if let Err(error) = recorded {
        if let Err(removing) = orca.remove_worktree(&checkout.worktree_id) {
            return Err(error.and_then_failed(&format!("and removing the conversations checkout {} failed: {removing}", checkout.worktree_id)));
        }
        return Err(error);
    }
    let run = orca.shell();
    let branch = git::current_branch(run, &checkout.path)?;
    refresh(run, &checkout)?;
    // Orca's branch is its name and nothing more, but one holding commits of its own is kept.
    if let Some(branch) = branch {
        let _ = git::delete_merged_branch(run, &checkout.path, &branch);
    }
    Ok(checkout)
}

// Any terminal open there, whatever its handle, as one listed with none still runs.
pub fn busy(terminals: &[Terminal], checkout: &Checkout) -> bool {
    terminal_worktrees(terminals).contains(&checkout.worktree_id)
}

// Whatever HEAD was on, no branch moves or goes, and ignored build output stays. Only while
// nothing runs there (`busy`), as the caller has checked under `Checkouts::lock`.
pub fn refresh(run: &mut Run, checkout: &Checkout) -> Result<(), Error> {
    git::fetch(run, &checkout.path)?;
    let tip = git::default_tip(run, &checkout.path)?;
    reset(run, checkout, &tip)
}

// A refresh without the fetch, for a checkout already at `tip`.
pub fn reset(run: &mut Run, checkout: &Checkout, tip: &Sha) -> Result<(), Error> {
    git::detach(run, &checkout.path, tip)?;
    git::clean(run, &checkout.path)
}

// Whether a fetch can be skipped: HEAD is at `tip`, where a refresh detached it. A turn still
// starts on a detached and cleaned tree (`reset`), as its last close's refresh may have failed.
pub fn at_tip(run: &mut Run, checkout: &Checkout, tip: Option<&Sha>) -> bool {
    tip.is_some_and(|tip| git::head(run, &checkout.path).is_ok_and(|head| head == *tip))
}

// So a checkout no turn starts in doesn't fall behind: after a turn's terminal closes, which
// cleans up after the turn, and on a push to the default branch, at `tip`, which a checkout
// already there skips. Takes `Checkouts::lock` for each; a line each for what it did.
pub fn refresh_idle(checkouts: &Checkouts, orca: &mut dyn Orca, records: &[CheckoutRecord], worktrees: &[Worktree], tip: Option<&Sha>) -> Vec<Refresh> {
    let mut done = Vec::new();
    for record in records {
        let Some(worktree) = worktrees.iter().find(|w| w.id == record.worktree_id && w.is_local) else { continue };
        let checkout = Checkout { worktree_id: worktree.id.clone(), path: worktree.path.clone() };
        let _checkouts = checkouts.lock();
        // Listed under the lock, so a turn opened since counts.
        let repo_id = record.repo_id.clone();
        done.push(match orca.terminals().map(|listed| busy(&listed, &checkout)) {
            Ok(true) => continue,
            Ok(false) if at_tip(orca.shell(), &checkout, tip) => continue,
            Ok(false) => match refresh(orca.shell(), &checkout) {
                Ok(()) => Refresh::Refreshed { repo_id },
                Err(error) => Refresh::Failed { repo_id, error },
            },
            Err(error) => Refresh::Unlisted { repo_id, error },
        });
    }
    done
}

// The checkouts of `records` whose worktree's origin is `repo`, a GitHub `owner/name`.
pub fn checkouts_of(records: &[CheckoutRecord], worktrees: &[Worktree], repo: &str) -> Vec<CheckoutRecord> {
    records.iter().filter(|r| worktrees.iter().any(|w| w.id == r.worktree_id && w.origin.as_deref().is_some_and(|o| o.eq_ignore_ascii_case(repo)))).cloned().collect()
}

// One per session, or per thread (`key` is the session's ID or the root comment's), as
// several conversations can run on one issue.
pub fn brief_path(briefs_dir: &str, repo: &str, identifier: &str, key: &str) -> PathBuf {
    brief::path(briefs_dir, repo, &format!("{identifier}-{key}"))
}

// A time before every session: for a fresh turn, all the owner's messages are after it.
pub const EVERY_MESSAGE: &str = "1970-01-01T00:00:00.000Z";

// What a turn answers: a session's messages, or a thread's replies, which its prompt names
// and tells how to read again.
#[derive(Clone, Copy)]
pub enum Request<'a> {
    Session(&'a str),
    // The root comment's ID.
    Thread(&'a str),
}

impl<'a> Request<'a> {
    // What the turn's request machine is keyed by: the session's ID, or the root's.
    pub fn id(self) -> &'a str {
        match self {
            Request::Session(id) | Request::Thread(id) => id,
        }
    }
}

// How a turn gets its brief: a fresh session is sent the text, and a resumed one already
// holds it from its first turn, so it is only pointed at the file.
#[derive(Clone, Copy)]
pub enum Brief<'a> {
    Text(&'a str),
    Held(&'a Path),
}

// None for `requests` when they couldn't be read: the turn reads them itself. A reply that
// comes in while the turn runs is the machine's next request, handed to the turn once it
// has answered (`follow_up`), or given a turn of its own.
pub fn prompt(request: Request, brief: Brief, requests: Option<&str>, after: Option<&str>, stale: bool) -> String {
    let opening = match request {
        Request::Session(id) => format!("This is a turn of the conversation in Linear agent session {id}."),
        Request::Thread(root) => format!("This is a turn in the thread of your Linear comment {root}."),
    };
    let stale = if stale { " Your checkout couldn't be brought up to date this turn, so it may be behind the default branch: say so if your answer rests on the code." } else { "" };
    let (said, requests) = said(request, requests, after);
    match brief {
        Brief::Text(text) => format!("{opening} Your brief for it is between <brief> and </brief>. {said}{stale}\n\n<brief>\n{}\n</brief>\n{requests}", text.trim_end()),
        Brief::Held(path) => format!("{opening} Your brief is the one in your first turn, also the file `{}`; read it again only if you no longer have it. {said}{stale}\n{requests}", path.display()),
    }
}

// What a prompt says of the owner's words, and the words themselves.
fn said(request: Request, requests: Option<&str>, after: Option<&str>) -> (String, String) {
    let (noun, read) = match request {
        Request::Session(id) => ("messages", format!("tolkster messages {id}")),
        Request::Thread(root) => ("replies", format!("tolkster messages --thread {root}")),
    };
    let command = format!("{read}{}", after.map_or(String::new(), |after| format!(" --after {after}")));
    match requests {
        Some(requests) => (format!("The owner's {noun}, the request this turn answers, are between <{noun}> and </{noun}>, as `{command}` prints them; run it only to read them again."), format!("\n<{noun}>\n{}\n</{noun}>\n", requests.trim_end())),
        None => (format!("The owner's {noun} couldn't be read for this prompt: read them with `{command}`, and answer them."), String::new()),
    }
}

// A warm turn's next request, as Claude's next message: the owner's words after `after`.
// `stale`: the default branch has moved since the checkout was brought up to date.
pub fn follow_up(request: Request, requests: Option<&str>, after: &str, stale: bool) -> String {
    let stale = if stale { " The default branch has moved since your checkout was brought up to date: say so if your answer rests on the code." } else { "" };
    let (said, requests) = said(request, requests, Some(after));
    format!("The owner has written again: this is the conversation's next turn, under the same brief. {said}{stale}\n{requests}")
}

// The line `listen` types into a session's open turn to hand it a request: times and a flag,
// never Linear text. `request` is the request's own time, the same on every send, which the
// turn takes once; `after` is the newest request a reader was shown.
pub fn handover(request: &Time, after: &str, stale: bool) -> Result<Line, Error> {
    Line::check(&format!("{request} {}{}", Time::check(after)?, if stale { " stale" } else { "" }))
}

// A request handed to a turn, as `handover` typed it.
#[derive(Clone, Debug, PartialEq)]
pub struct Handed {
    pub request: Time,
    pub after: Time,
    pub stale: bool,
}

// A line typed into a turn, as `handover` made it; None for anything else.
pub fn handed(line: &str) -> Option<Handed> {
    let mut words = line.split_whitespace();
    let (request, after) = (Time::parse(words.next()?)?, Time::parse(words.next()?)?);
    let stale = match words.next() {
        None => false,
        Some("stale") => true,
        Some(_) => return None,
    };
    words.next().is_none().then_some(Handed { request, after, stale })
}

// Which request handed to a turn it takes, and when: each once, and only one newer than the
// last taken, at first the one the turn opened for. With `hold`, a thread's turn, one handed
// while Claude works waits until it is done: its read moves the answer's key
// (state::thread_request), which the answer being written must not take.
pub struct Takes {
    taken: Option<Time>,
    held: Option<Handed>,
    hold: bool,
}

impl Takes {
    pub fn new(taken: Option<Time>, hold: bool) -> Takes {
        Takes { taken, held: None, hold }
    }

    // The request a typed line hands over, to take now.
    pub fn typed(&mut self, line: &str, working: bool) -> Option<Handed> {
        let handed = handed(line)?;
        let newest = self.held.as_ref().map(|held| &held.request).or(self.taken.as_ref());
        if newest.is_some_and(|newest| handed.request.as_str() <= newest.as_str()) {
            return None;
        }
        if self.hold && working {
            self.held = Some(handed);
            return None;
        }
        self.taken = Some(handed.request.clone());
        Some(handed)
    }

    // Claude is done with every message written to it: the request held meanwhile, to take now.
    pub fn idle(&mut self) -> Option<Handed> {
        let handed = self.held.take()?;
        self.taken = Some(handed.request.clone());
        Some(handed)
    }
}

// The repo's recorded conversations checkout, as Orca lists it.
pub fn recorded(orca: &mut dyn Orca, records: &dyn Records, repo_id: &str) -> Result<Option<Worktree>, Error> {
    let worktrees = orca.worktrees()?;
    let record = records.checkouts()?.into_iter().find(|r| r.repo_id == repo_id);
    Ok(record.and_then(|r| worktrees.into_iter().find(|w| w.id == r.worktree_id && w.is_local)))
}

// Whether a checkout is behind its default branch's known tip (`base_tip`): false with none
// known, as its turn's start brought it up to date.
pub fn behind(checkouts: &Checkouts, run: &mut Run, worktree: &Worktree) -> bool {
    let Some(tip) = worktree.origin.as_deref().and_then(|repo| checkouts.known_tip(repo)) else { return false };
    !at_tip(run, &Checkout { worktree_id: worktree.id.clone(), path: worktree.path.clone() }, Some(&tip))
}

// The turn's command line, the same for both kinds of turn: `words` are its checked words.
fn command_line(words: &str, brief: &Path, stale: bool) -> Result<CommandLine, Error> {
    let brief = Line::check(&brief.display().to_string())?;
    Ok(CommandLine::new(&Line::check(words)?).quoted(&brief).flag(stale, "--stale"))
}

// The brief's path is quoted, so it is a `Line`, which holds no quote.
pub struct Turn<'a> {
    pub session_id: &'a Id,
    pub agent_session: &'a Id,
    // The request it opens for, which a hand-over of the same request repeats.
    pub request: Option<&'a Time>,
    pub messages_after: Option<&'a Time>,
    pub brief: &'a Path,
    // The checkout may be behind the default branch: the prompt says so.
    pub stale: bool,
}

pub const TURN_OPTIONS: [(&str, Kind); 5] = [("agent-session", Kind::String), ("request", Kind::String), ("messages-after", Kind::String), ("brief", Kind::String), ("stale", Kind::Boolean)];

// A turn as `tolkster converse` reads it from its command line.
pub struct ParsedTurn {
    pub session_id: Id,
    pub agent_session: Id,
    pub request: Option<Time>,
    pub messages_after: Option<Time>,
    pub brief: PathBuf,
    pub stale: bool,
}

impl ParsedTurn {
    // None when the session, the agent session or the brief is missing.
    pub fn parse(parsed: &Parsed) -> Result<Option<ParsedTurn>, Error> {
        let (Some(session_id), Some(agent_session), Some(brief)) = (parsed.first(), parsed.non_empty("agent-session"), parsed.non_empty("brief")) else {
            return Ok(None);
        };
        let (request, messages_after) = (parsed.non_empty("request").map(Time::check).transpose()?, parsed.non_empty("messages-after").map(Time::check).transpose()?);
        let turn = ParsedTurn { session_id: turn_id(session_id)?, agent_session: turn_id(agent_session)?, request, messages_after, brief: PathBuf::from(brief), stale: parsed.flag("stale") };
        turn.turn().command()?;
        Ok(Some(turn))
    }

    pub fn turn(&self) -> Turn<'_> {
        Turn { session_id: &self.session_id, agent_session: &self.agent_session, request: self.request.as_ref(), messages_after: self.messages_after.as_ref(), brief: &self.brief, stale: self.stale }
    }
}

pub fn turn_id(id: &str) -> Result<Id, Error> {
    Id::parse(id).ok_or_else(|| Error::Usage(format!("a turn's ID has characters a command can't take: {id}")))
}

impl Turn<'_> {
    pub fn command(&self) -> Result<CommandLine, Error> {
        let request = self.request.map_or(String::new(), |time| format!(" --request {time}"));
        let messages = self.messages_after.map_or(String::new(), |time| format!(" --messages-after {time}"));
        command_line(&format!("tolkster converse {} --agent-session {}{request}{messages} --brief", self.session_id, self.agent_session), self.brief, self.stale)
    }
}

// Opens a turn's terminal in the repo's conversations checkout, made if there is none and
// refreshed unless it was just made or another turn is open in it, without the fetch when
// it is at its default branch's known tip (`base_tip`). `terminals` were listed
// under `Checkouts::lock`, which the caller holds. `command` is the turn's, given whether the checkout
// may be behind the default branch (the turn is told so). The terminal is watched at once,
// before the caller records it, as the watcher may see it end first. Returns the terminal, and
// why the checkout couldn't be brought up to date, when it couldn't.
pub fn open_in_checkout(ports: &mut Ports, repo_id: &str, terminals: &[Terminal], identifier: &IssueKey, command: impl Fn(bool) -> Result<CommandLine, Error>) -> Result<(String, Option<Error>), Error> {
    let (orca, records, checkouts) = (&mut *ports.orca, ports.records, &ports.shared.checkouts);
    let worktrees = orca.worktrees()?;
    let (checkout, fresh) = checkout(orca, &records.checkouts()?, repo_id, &worktrees, &mut |record| records.set_checkout(record), now_ms())?;
    let tip = worktrees.iter().find(|w| w.id == checkout.worktree_id).and_then(|w| w.origin.as_deref()).and_then(|repo| checkouts.known_tip(repo));
    let mut behind = None;
    // A turn open in it keeps it as it is, a warm one for minutes: the new turn is told when
    // that leaves it behind.
    let held = !fresh && busy(terminals, &checkout) && tip.as_ref().is_some_and(|tip| !at_tip(orca.shell(), &checkout, Some(tip)));
    if !fresh
        && !busy(terminals, &checkout)
        && let Err(error) = match tip.filter(|tip| at_tip(orca.shell(), &checkout, Some(tip))) {
            Some(tip) => reset(orca.shell(), &checkout, &tip),
            None => refresh(orca.shell(), &checkout),
        }
    {
        behind = Some(error);
    }
    let handle = orca.open(&checkout.worktree_id, Some(identifier), &command(behind.is_some() || held)?)?;
    (ports.watch)(&handle);
    Ok((handle, behind))
}

// So a turn whose terminal couldn't be closed isn't taken as running. The record keeps its
// time, so a newer turn's write still replaces it. True when it cleared the handle.
pub fn forget_turn(state: &State, session_id: &str, conversation: Option<Conversation>, handle: &str) -> Result<bool, Error> {
    let Some(turn) = conversation.filter(|c| c.terminal.as_deref() == Some(handle)) else { return Ok(false) };
    state.converse(session_id, &Conversation { terminal: None, ..turn }).map(|()| true)
}

// A turn in a thread of Tolkster's comment runs in the same checkout, keyed on the thread:
// its agent session's ID is the root comment's, and its conversation is recorded under the
// root, as a session's under its ID. It posts nothing in any session.

// `request` is the time of the request the turn answers, which keys its one answer
// (post::answer_key), and it reads the owner's replies after `replies_after` up to it. The
// root is a UUID, as Claude takes for a session's ID.
pub struct ThreadTurn<'a> {
    pub root: &'a Id,
    pub request: &'a Time,
    pub replies_after: Option<&'a Time>,
    pub brief: &'a Path,
    pub stale: bool,
}

impl ThreadTurn<'_> {
    pub fn command(&self) -> Result<CommandLine, Error> {
        if !harness::turn::takes_id(self.root) {
            return Err(Error::Usage(format!("a thread's root isn't a UUID: {}", self.root)));
        }
        let after = self.replies_after.map_or(String::new(), |time| format!(" --replies-after {time}"));
        command_line(&format!("tolkster converse --thread {} --request {}{after} --brief", self.root, self.request), self.brief, self.stale)
    }
}

// The owner's replies in `thread` a turn for the request at `request` answers, oldest first:
// those after the newest a turn was shown, `after`, up to the request.
pub fn batch<'t>(thread: &'t Thread, after: Option<&str>, request: &str) -> Vec<&'t ThreadReply> {
    thread.replies_after(after).into_iter().filter(|r| r.created_at.as_str() <= request).collect()
}

// What a turn in a note's thread came to.
#[derive(Debug)]
pub enum InThread {
    // Why the thread, or the reply that asked, is none a turn answers.
    Unanswered(&'static str),
    // No repo for its issue: true when this said so in the thread.
    NoRepo(bool),
    // No reply of the owner's after the newest a turn was shown, up to the request.
    Answered,
    // The turn's terminal, and why its checkout couldn't be brought up to date, when it couldn't.
    Opened(String, Option<Error>),
}

impl InThread {
    // What it came to, for the log.
    pub fn turn(&self) -> trace::Turn {
        match self {
            InThread::Unanswered(why) => trace::Turn::Unanswered(why),
            InThread::NoRepo(posted) => trace::Turn::NoRepo { posted: *posted },
            InThread::Answered => trace::Turn::Answered,
            InThread::Opened(_, behind) => trace::Turn::Opened { behind: behind.clone() },
        }
    }
}

// A turn in the thread of Tolkster's comment `root`, for the machine's `startTurn`, which
// decided the reply that asked, at `request`: one batch, the owner's replies after the newest
// a turn was shown, `after`, up to `request`, as a session's turn takes its messages. A reply
// after it is the machine's next request. `issue_id` is the issue the thread must be on. Under
// the checkouts' lock, which a stop takes too, a turn already open by the thread's record, a
// warm one or this command's run before, is handed the request (`handed_over`) and answered
// with, as a session's is (executor::turn).
pub fn thread_turn(config: &Config, ports: &mut Ports, root: &Id, issue_id: &str, request: &Time, after: Option<&Time>, beat: &dyn Fn()) -> Result<InThread, Error> {
    let thread = match ports.linear.thread(root)? {
        Ok(thread) => thread,
        Err(why) => return Ok(InThread::Unanswered(why)),
    };
    if issue_id != thread.issue_id {
        return Ok(InThread::Unanswered("on another issue"));
    }
    beat();
    let _checkouts = ports.shared.checkouts.lock();
    let terminals = ports.orca.terminals()?;
    let open = open_terminals(&terminals, None);
    let before = ports.records.conversation(root)?;
    if let Some(terminal) = before.as_ref().and_then(|c| c.terminal.clone()).filter(|t| open.contains(t.as_str())) {
        // Watched again, as a restarted `listen` doesn't watch it yet.
        (ports.watch)(&terminal);
        // Failed, never closed here: it may be working, and the machine closes a warm one.
        let Some((repo_id, _)) = find_repo(&ports.orca.repos(None)?, config.repos.repo_for(&thread.place)) else { return Ok(InThread::Unanswered("no repo for its issue, with its turn open")) };
        if handed_over(ports, &thread, &repo_id, &terminal, &terminals, request, after)? {
            return Ok(InThread::Opened(terminal, None));
        }
    }
    if batch(&thread, after.map(|t| t.as_str()), request).is_empty() {
        return Ok(InThread::Answered);
    }
    let Some((repo_id, repo)) = find_repo(&ports.orca.repos(None)?, config.repos.repo_for(&thread.place)) else {
        // Said once, as the request's answer (post::answer_key), so its `stalled` says nothing
        // more. The next reply, once a repo is mapped, is answered.
        let posted = ports.linear.reply_once(&thread.issue_id, root, voice::NO_REPO, &answer_key(root, request))?;
        return Ok(InThread::NoRepo(posted));
    };
    let (handle, behind) = open_thread_turn(config, ports, &thread, &repo, &repo_id, request, after, before.as_ref(), &terminals)?;
    Ok(InThread::Opened(handle, behind))
}

// The thread's open turn is handed `request` by a line typed into its terminal (`handover`),
// which it takes once, so a resend hands it nothing new. One open outside the repo's
// conversations checkout is answered with, and nothing is typed into it. False when the line
// may not have reached it: it is closed, never typed into again, and a new turn opens in its
// place, but never beside one still open.
fn handed_over(ports: &mut Ports, thread: &Thread, repo_id: &str, terminal: &str, terminals: &[Terminal], request: &Time, after: Option<&Time>) -> Result<bool, Error> {
    let Some(checkout) = recorded(ports.orca, ports.records, repo_id)? else { return Ok(true) };
    if !open_terminals(terminals, Some(&checkout.id)).contains(terminal) {
        return Ok(true);
    }
    let line = handover(request, after.map_or(EVERY_MESSAGE, |t| t.as_str()), behind(&ports.shared.checkouts, ports.orca.shell(), &checkout))?;
    let handle = crate::plain::Terminal::check(terminal)?;
    let Err(error) = ports.orca.type_line(&handle, &line) else { return Ok(true) };
    trace::event(Work::HandoverFailed { thread: Some(thread.root.clone()), error: error.clone() });
    ports.orca.close(&handle).map_err(|closing| error.and_then_failed(&format!("and closing the turn's terminal {terminal} failed: {closing}")))?;
    Ok(false)
}

// Opens a thread's turn in the repo's conversations checkout, as a session's turn opens
// (executor.rs), with `Checkouts::lock` held by the caller, and records its terminal under the thread's
// root. Its terminal, and why its checkout couldn't be brought up to date, when it couldn't.
#[allow(clippy::too_many_arguments)]
fn open_thread_turn(config: &Config, ports: &mut Ports, thread: &Thread, repo: &str, repo_id: &str, request: &Time, after: Option<&Time>, before: Option<&Conversation>, terminals: &[Terminal]) -> Result<(String, Option<Error>), Error> {
    let repo = RepoName::check(repo)?;
    let brief = ThreadBrief { identifier: &thread.identifier, comment_id: &thread.root, repo: &repo, owner_name: &config.owner.name };
    let path = brief_path(&config.briefs_dir, &repo, &thread.identifier, &thread.root);
    let turn = |stale| ThreadTurn { root: &thread.root, request, replies_after: after, brief: &path, stale };
    // Checked before anything is written.
    turn(false).command()?;
    write_file(&path, &brief.render())?;
    // Never older than what the record holds, or the write would be ignored.
    let at = before.map(|c| c.at.clone()).into_iter().chain([iso(now_ms())]).max().unwrap_or_default();
    let (handle, behind) = open_in_checkout(ports, repo_id, terminals, &thread.identifier, |stale| turn(stale).command())?;
    // A turn nothing can find, to stop it or to know it runs, isn't left running.
    if let Err(error) = ports.records.converse(&thread.root, &Conversation { agent_session: thread.root.to_string(), terminal: Some(handle.clone()), at }) {
        if let Err(closing) = crate::plain::Terminal::check(&handle).and_then(|plain| ports.orca.close(&plain)) {
            return Err(error.and_then_failed(&format!("and closing the turn's terminal {handle} failed: {closing}")));
        }
        return Err(error);
    }
    Ok((handle, behind))
}

#[cfg(test)]
mod tests;
