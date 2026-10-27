// `tolkster converse`: a conversation's turn, run by `tolkster listen` in a terminal of the
// repo's conversations checkout. Not for people. With `--thread`, a turn in a thread of
// Tolkster's comment (`run_thread`), keyed on the thread.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use ureq::Agent;

use crate::args::{Kind, parse};
use crate::cli::{CONVERSATION_ENV, THREAD_ENV, env, logged};
use crate::commands::messages;
use crate::exec::run_command;
use crate::machine::{Reader, turn_idle};
use crate::time::{iso, now_ms};
use crate::converse::{self, Brief, Request, ThreadTurn};
use crate::error::{Context, Error};
use crate::harness::turn::{self, Heard, Input};
use crate::plain::{Id, Terminal, Time};
use crate::orca::close_terminal;
use crate::runtime::load;

const CLOSE_PAUSE: Duration = Duration::from_secs(5);
const CLOSE_TRIES: u32 = 3;

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    if argv.iter().any(|a| a == "--thread") {
        return run_thread(agent, argv);
    }
    let parsed = parse(argv, &converse::TURN_OPTIONS, true)?;
    let Some(converse::ParsedTurn { session_id, agent_session, request, messages_after, brief, stale }) = converse::ParsedTurn::parse(&parsed)? else {
        return Err(Error::Usage("converse needs <sessionId>, --agent-session and --brief".into()));
    };
    // One turn: its end, the terminal's close, is the machine's `turnEnded`, which says it
    // stopped when it answered nothing, and starts the next turn for a reply that came meanwhile.
    let code = run_turn(agent, Request::Session(&session_id), &agent_session, request, messages_after.as_deref(), &brief, stale);
    end_turn(agent, &session_id);
    code
}

// The turn recorded under `session_id`, a session's or a note's thread's root, is over. The
// record stops naming this terminal first, or it would still count as running its turn.
// Last, whatever happened, the terminal goes with it: a close that failed retries the
// forgetting, should that have failed too.
fn end_turn(agent: &Agent, session_id: &str) {
    let Some(handle) = env("ORCA_TERMINAL_HANDLE") else { return };
    logged(forget_turn(agent, session_id, &handle), "converse");
    if !close(&handle) {
        logged(forget_turn(agent, session_id, &handle), "converse: the terminal wasn't closed, and the record still names it");
    }
}

fn forget_turn(agent: &Agent, session_id: &str, handle: &str) -> Result<bool, Error> {
    let shared = Arc::default();
    let rt = load(agent, &shared)?;
    let conversation = rt.state.read(&[session_id.to_string()])?.remove(session_id).flatten().and_then(|record| record.conversation);
    converse::forget_turn(&rt.state, session_id, conversation, handle)
}

// A handle that isn't plain is never tried: no retry makes it one.
fn close(handle: &str) -> bool {
    let Some(handle) = Terminal::parse(handle) else { return false };
    (0..CLOSE_TRIES).any(|n| {
        if n > 0 {
            std::thread::sleep(CLOSE_PAUSE);
        }
        close_terminal(&mut run_command, &handle).is_ok()
    })
}

pub const THREAD_OPTIONS: &[(&str, Kind)] = &[("thread", Kind::String), ("request", Kind::String), ("replies-after", Kind::String), ("brief", Kind::String), ("stale", Kind::Boolean)];

// A turn as a session's, warm too: its first batch is the replies after `--replies-after` up
// to the request at `--request`. Its end, the terminal's close, is the machine's `turnEnded`,
// which says it stopped when it answered nothing, and starts the next turn for a reply that
// came meanwhile.
fn run_thread(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, THREAD_OPTIONS, false)?;
    let (Some(root), Some(request), Some(brief)) = (parsed.non_empty("thread"), parsed.non_empty("request"), parsed.non_empty("brief")) else {
        return Err(Error::Usage("converse --thread needs --request and --brief".into()));
    };
    let (root, request) = (converse::turn_id(root)?, Time::check(request)?);
    let brief = PathBuf::from(brief);
    let (replies_after, stale) = (parsed.non_empty("replies-after").map(Time::check).transpose()?, parsed.flag("stale"));
    // The same checks the command line was made with.
    ThreadTurn { root: &root, request: &request, replies_after: replies_after.as_ref(), brief: &brief, stale }.command()?;
    let code = run_turn(agent, Request::Thread(&root), &root, Some(request), replies_after.as_deref(), &brief, stale);
    end_turn(agent, &root);
    code
}

// A resumed session holds its brief from its first turn, so only a fresh one has its file read.
fn held_or_text<'a>(text: Option<&'a str>, path: &'a Path) -> Brief<'a> {
    text.map_or(Brief::Held(path), Brief::Text)
}

// The owner's words a turn answers, read here, never by `listen`: a session's messages after
// `after`, or a thread's replies after it up to the request at `through`, which reports the
// newest of them shown (state::thread_request).
fn read(agent: &Agent, request: Request, after: Option<&str>, through: Option<&str>) -> Result<String, Error> {
    match request {
        Request::Session(id) => messages::read(agent, id, after, Some(Reader::Turn)),
        Request::Thread(root) => messages::read_replies(agent, root, after, through),
    }
}

// The owner's words are read just before Claude starts, on the harness's turn adapter
// (harness::turn). The turn stays warm (docs/dispatcher.md): Claude's input stays open, the
// machine is told each time it is done with every message written to it (`turnIdle`), and a
// request `listen` hands over by a line typed into this terminal (`converse::handover`) is
// Claude's next message. The machine closes the terminal once it has been idle long enough;
// `BACKSTOP` ends one the machine lost track of.
fn run_turn(agent: &Agent, asked: Request, agent_session: &Id, request: Option<Time>, after: Option<&str>, brief: &Path, stale: bool) -> Result<u8, Error> {
    let resume = turn::resumes(&std::env::current_dir().context("reading the current folder")?, agent_session)?;
    // A turn started afresh remembers nothing, so it reads every message or reply: a session's
    // first turn that died before Claude saved anything had already taken the mention.
    let after = match (resume, asked) {
        (true, _) => after,
        (false, Request::Session(_)) => Some(converse::EVERY_MESSAGE),
        (false, Request::Thread(_)) => None,
    };
    let text = if resume { None } else { Some(std::fs::read_to_string(brief).context(brief.display())?) };
    let said = logged(read(agent, asked, after, request.as_deref()), "converse: reading the owner's words failed, so the turn reads them itself");
    let prompt = converse::prompt(asked, held_or_text(text.as_deref(), brief), said.as_deref(), after, stale);
    let (set, unset): (_, &[&str]) = match (asked, &request) {
        (Request::Session(id), _) => ([(CONVERSATION_ENV, id.to_string())], &[]),
        // Without its terminal's handle, Claude can't type itself a hand-over, which would move
        // its answer's key (state::thread_request).
        (Request::Thread(root), Some(request)) => ([(THREAD_ENV, format!("{root}@{request}"))], &[CONVERSATION_ENV, "ORCA_TERMINAL_HANDLE"]),
        (Request::Thread(_), None) => return Err(Error::Usage("converse --thread needs --request".into())),
    };
    let turn::Started { mut process, mut input, output } = turn::start(agent_session, resume, &set, unset)?;
    input.send(&prompt)?;
    let (tx, rx) = mpsc::channel();
    let typed = tx.clone();
    // Never joined: it waits on this terminal's input, which outlives Claude.
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if typed.send(Turned::Typed(line)).is_err() {
                return;
            }
        }
    });
    std::thread::scope(|scope| {
        scope.spawn(move || shown(output, &tx));
        scope.spawn(move || follow_ups(agent, asked, request, input, rx));
        let status = process.wait().context("waiting on Claude")?;
        Ok(status.code().and_then(|c| u8::try_from(c).ok()).unwrap_or(1))
    })
}

// A warm turn left this long after Claude was last done with its input, with nothing handed
// to it, closes that input itself, so Claude exits and the turn ends: the machine closes it at
// 5 minutes (`WARM_MS`), and this is for a turn the machine lost track of.
const BACKSTOP: Duration = Duration::from_secs(10 * 60);

// What a turn's `converse` hears: a line typed into its terminal, Claude done with its input,
// or Claude's output ended, as it has exited.
enum Turned {
    Typed(String),
    Idle,
    Ended,
}

// Claude's output: its text is shown in the terminal, and each message it is done with is
// passed on, and then its end.
fn shown(output: impl std::io::Read, tx: &mpsc::Sender<Turned>) {
    turn::hear(output, |heard| match heard {
        Heard::Text(text) => println!("{text}"),
        Heard::Done => {
            let _ = tx.send(Turned::Idle);
        }
    });
    let _ = tx.send(Turned::Ended);
}

// Claude is idle once each message written to it is done (`Heard::Done`), one each even for a message
// written while it works, and only then is that reported to the machine, with this terminal,
// so one from another turn is told apart. Each request `listen` hands over is taken once
// (`converse::Takes`), a thread's only once Claude is idle. Returning drops `input`, which
// ends Claude.
fn follow_ups(agent: &Agent, asked: Request, taken: Option<Time>, mut input: Input, rx: mpsc::Receiver<Turned>) {
    let terminal = env("ORCA_TERMINAL_HANDLE").and_then(|t| Terminal::parse(&t));
    let mut takes = converse::Takes::new(taken, matches!(asked, Request::Thread(_)));
    // The messages written that aren't done yet, the turn's first included.
    let mut owed: u32 = 1;
    let mut idle_since: Option<Instant> = None;
    loop {
        let heard = match idle_since {
            Some(since) => rx.recv_timeout(BACKSTOP.saturating_sub(since.elapsed())).ok(),
            None => rx.recv().ok(),
        };
        let Some(heard) = heard else { return };
        match heard {
            Turned::Ended => return,
            Turned::Idle => {
                owed = owed.saturating_sub(1);
                if owed > 0 {
                    continue;
                }
                let Some(handed) = takes.idle() else {
                    idle_since = Some(Instant::now());
                    report_idle(agent, asked.id(), terminal.as_ref());
                    continue;
                };
                if !follow_up(agent, asked, &handed, &mut input) {
                    return;
                }
                (idle_since, owed) = (None, owed + 1);
            }
            Turned::Typed(line) => {
                let Some(handed) = takes.typed(&line, owed > 0) else { continue };
                if !follow_up(agent, asked, &handed, &mut input) {
                    return;
                }
                (idle_since, owed) = (None, owed + 1);
            }
        }
    }
}

// Writes the handed request's words to Claude as its next message, read here as for the
// turn's first. A thread's that can't be read, or whose read can't be reported, ends the turn:
// its answer is keyed by the newest request it was shown, which the read moves
// (state::thread_request). False when the turn ends.
fn follow_up(agent: &Agent, asked: Request, handed: &converse::Handed, input: &mut Input) -> bool {
    let said = match (read(agent, asked, Some(&handed.after), Some(&handed.request)), asked) {
        (Ok(said), _) => Some(said),
        (Err(error), Request::Thread(_)) => {
            eprintln!("converse: reading the replies failed, so the turn ends: {error}");
            return false;
        }
        (Err(error), Request::Session(_)) => {
            eprintln!("converse: reading the messages failed, so the turn reads them itself: {error}");
            None
        }
    };
    let prompt = converse::follow_up(asked, said.as_deref(), &handed.after, handed.stale);
    input.send(&prompt).is_ok()
}

fn report_idle(agent: &Agent, session_id: &str, terminal: Option<&Terminal>) {
    let Some(terminal) = terminal else { return eprintln!("converse: no terminal handle, so the turn's idle isn't reported") };
    let shared = Arc::default();
    match load(agent, &shared) {
        Ok(rt) => rt.state.report(turn_idle(session_id, terminal.as_str(), &iso(now_ms()))),
        Err(error) => eprintln!("converse: the turn's idle couldn't be reported: {error}"),
    }
}
