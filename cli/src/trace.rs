// What `tolkster listen` shows in its terminal (docs/realtime.md). A call site says what
// happened, as an `Event` of IDs, identifiers, enums, durations, paths and errors, never
// Linear or GitHub text or a secret but what an error's text quotes; how it reads, here and
// in the modules below alone.
// `ids` picks what a request's JSON may show.

use std::cell::RefCell;
use std::io::{self, IsTerminal, Write};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::cli::env;
use crate::time::{local_clock, now_iso};

mod calls;
mod disk;
mod event;
mod listen;
mod notes;
mod work;

pub use calls::{Call, Cost};
pub use event::{Event, kind as error_kind};
use event::Show;
pub use listen::{Closed, Declined, Disk, Ended, Host, Realtime, Restart, STUCK_AFTER, Setting, Step, Stuck, Unfit, Unslept, Update, Watch};
#[cfg(feature = "develop")]
pub use listen::Unbuilt;
#[cfg(any(feature = "develop", test))]
pub use listen::Unpulled;
pub use notes::{Context, Keep, Missing, Reap, Reaping, Refusal, Rule, Triage, TriageNote, Why};
pub use work::{UNCONFIRMED, Answer, Mode, Outcome, Problem, Purpose, Refresh, RungNote, Turn, Undone, Waiting, Work};

// How this process logs, set once as it starts: every line, from any thread, reads them.
static ENABLED: AtomicBool = AtomicBool::new(false);
static VERBOSE: AtomicBool = AtomicBool::new(false);
static COLOUR: AtomicBool = AtomicBool::new(false);
// Where a line goes instead of stdout or stderr, such as a log pane: false when it doesn't
// take it, and the line is then the caller's to write.
static SINK: OnceLock<fn(Logged) -> bool> = OnceLock::new();

// What a line that isn't routine is, shown by its sign: the meaning never rests on colour.
// `Quiet` is a routine summing-up line, which the panel dims.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Routine,
    Quiet,
    Error,
    Warning,
    Recovered,
}

impl Mark {
    fn sign(self) -> Option<(&'static str, &'static str)> {
        match self {
            Mark::Routine | Mark::Quiet => None,
            Mark::Error => Some(("✖", "31")),
            Mark::Warning => Some(("!", "33")),
            Mark::Recovered => Some(("✔", "32")),
        }
    }
}

// A line, as this module shapes it, for a sink.
#[derive(Clone, Debug, PartialEq)]
pub struct Logged {
    pub clock: String,
    pub mark: Mark,
    pub lead: String,
    pub issue: bool,
    pub text: String,
}

// The kind of job a thread acts on, as its line names it: a plain label, its ID shown only
// with `--verbose`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    // A command of the Durable Object's, by its kind's name, e.g. `readIssue`.
    Command(String),
    TerminalClosed,
    WorktreeClosed,
    // A push to a repo's default branch, by the repo's `owner/name`.
    Pushed(String),
    UpdateCheck,
}

impl Kind {
    pub fn label(&self) -> String {
        match self {
            Kind::Command(name) => name.clone(),
            Kind::TerminalClosed => "terminal closed".into(),
            Kind::WorktreeClosed => "worktree closed".into(),
            Kind::Pushed(repo) => format!("{repo} pushed"),
            Kind::UpdateCheck => "update check".into(),
        }
    }

    // The label as the kind's column holds it, which a repo's `owner/` and any overrun don't fill.
    fn column(&self) -> String {
        let label = match self {
            Kind::Pushed(repo) => format!("{} pushed", repo.rsplit('/').next().unwrap_or(repo)),
            other => other.label(),
        };
        if label.chars().count() > KIND { format!("{}…", label.chars().take(KIND - 1).collect::<String>()) } else { label }
    }

    // The area its lines are of already, so they don't name it again.
    fn area(&self) -> Option<&'static str> {
        matches!(self, Kind::UpdateCheck).then_some("update")
    }
}

// The job this thread acts on: jobs run at once in lanes (scheduler.rs), so each line
// names the one it is for, by its issue, and one line says what it came to.
struct Job {
    id: String,
    issue: Option<String>,
    kind: Kind,
    started: Instant,
    summed_up: bool,
}

// Ambient, as every line any call writes names it: passed, it would be in every signature
// that logs.
thread_local! {
    static JOB: RefCell<Option<Job>> = const { RefCell::new(None) };
}

// Each line this thread writes names `id`, and leads with `issue`, until the guard drops.
pub fn acting_as(id: &str, issue: Option<&str>, kind: Kind) -> Acting {
    let job = Job { id: id.into(), issue: issue.map(String::from), kind, started: Instant::now(), summed_up: false };
    Acting(JOB.with(|a| a.replace(Some(job))))
}

// This thread's job, for a thread it starts to act as (`acting_as`).
pub fn job() -> Option<(String, Option<String>, Kind)> {
    JOB.with(|a| a.borrow().as_ref().map(|job| (job.id.clone(), job.issue.clone(), job.kind.clone())))
}

pub struct Acting(Option<Job>);

impl Drop for Acting {
    fn drop(&mut self) {
        JOB.with(|a| *a.borrow_mut() = self.0.take());
    }
}

pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

// `listen`'s lines also go to `path` (`disk`), after an earlier run's.
pub fn keep_on_disk(path: std::path::PathBuf) -> io::Result<()> {
    disk::open(path)
}

// Set once: a later sink is ignored.
pub fn sink(sink: fn(Logged) -> bool) {
    let _ = SINK.set(sink);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

// `verbose` shows each job's ID, what came in, every write and every call's cost; colour is
// off with `plain`, with NO_COLOR set or when stdout isn't a terminal.
pub fn configure(verbose: bool, plain: bool) {
    VERBOSE.store(verbose, Ordering::Relaxed);
    COLOUR.store(coloured(plain, env("NO_COLOR").is_some(), io::stdout().is_terminal()), Ordering::Relaxed);
}

fn coloured(plain: bool, no_colour: bool, terminal: bool) -> bool {
    !plain && !no_colour && terminal
}

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

pub fn colour() -> bool {
    COLOUR.load(Ordering::Relaxed)
}

// A frame as it came, whether or not the line is on: `--print`'s output, and `--verbose`'s.
pub fn stamped_inbound(text: &str) -> String {
    formatted(&clock(), Mark::Routine, "in", false, text, colour())
}

// A line for the sink, when it takes it: the line is then the sink's to write.
fn to_sink(mark: Mark, lead: &str, issue: bool, text: &str) -> bool {
    SINK.get().is_some_and(|sink| sink(Logged { clock: clock(), mark, lead: lead.into(), issue, text: text.into() }))
}

// A line for the disk log, dated to the millisecond to order runs over days; plain, as
// `--print` and a pipe have it.
fn to_disk(mark: Mark, lead: &str, issue: bool, text: &str) {
    if let Err(error) = crate::error::Context::context(disk::keep(&formatted(&now_iso(), mark, lead, issue, text, false)), "writing listen.log") {
        event(Disk::Stopped(error));
    }
}

// A job's line to the disk log, shaped as with `--verbose`.
fn keep(mark: Mark, area: Option<&str>, issue: Option<&str>, full: &str, last: bool) {
    let (lead, issue, text) = JOB.with(|a| shaped(a.borrow().as_ref(), area, issue, full, true, last));
    to_disk(mark, &lead, issue, &text);
}

// A line a sink kept, as stdout would have had it.
pub fn plain(line: &Logged) -> String {
    formatted(&line.clock, line.mark, &line.lead, line.issue, &line.text, colour())
}

// Text a program printed, each line to the sink when it takes it, else on stderr.
#[cfg(feature = "develop")]
pub fn printed(text: &str) {
    // In one write, so no other thread's line lands inside it.
    let unsunk: Vec<&str> = text.lines().filter(|line| !to_sink(Mark::Error, "·", false, line)).collect();
    if !unsunk.is_empty() {
        let _ = writeln!(io::stderr(), "{}", unsunk.join("\n"));
    }
}

// A recipe line's stderr under `listen`, for the machine `name`: its last lines, each cut short,
// to the disk log alone, as the owner's own script's text is never the panel's.
pub fn recipe_stderr(name: &str, stderr: &str) {
    for line in tail(stderr) {
        to_disk(Mark::Routine, "host", false, &format!("{name}'s recipe said: {line}"));
    }
}

const TAIL_LINES: usize = 20;
const TAIL_WIDTH: usize = 400;

fn tail(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().map(str::trim_end).filter(|line| !line.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].iter().map(|line| line.chars().take(TAIL_WIDTH).collect()).collect()
}

// `--verbose`'s frame, to the sink: false when it doesn't take it.
pub fn inbound_to_sink(text: &str) -> bool {
    to_sink(Mark::Routine, "in", false, text)
}

// The one way a line is written: what happened, which this module alone shows.
pub fn event(event: impl Into<Event>) {
    let event = event.into();
    let line = event.line();
    if !enabled() {
        // A worker's command shows only a call's cost, on stderr, when asked to.
        if let Event::Call(Call::Cost(cost)) = &event
            && env(cost.log_env).is_some()
        {
            let _ = writeln!(io::stderr(), "{} {cost}", now_iso());
        }
        if line.show != Show::Notice {
            return;
        }
    }
    let text = line.parts.render(verbose());
    // The disk log keeps errors' text and opaque IDs whatever the terminal shows, as the panel
    // names an error by its kind only; a line only `--verbose` shows (a write, a cost) it never
    // keeps.
    let full = line.parts.render(true);
    match line.show {
        Show::Notice => {
            let lead = line.area.unwrap_or("·");
            to_disk(line.mark, lead, false, &full);
            if to_sink(line.mark, lead, false, &text) {
                return;
            }
            let _ = writeln!(io::stderr(), "{}", formatted(&clock(), line.mark, lead, false, &text, colour() && io::stderr().is_terminal()));
        }
        Show::Verbose if !verbose() => {}
        Show::Outcome => outcome(line.mark, &text, &full),
        Show::Always | Show::Verbose => {
            if line.show == Show::Always {
                keep(line.mark, line.area, line.issue.as_deref(), &full, false);
            }
            write(line.mark, line.area, line.issue.as_deref(), &text, false);
        }
    }
}

// What the job came to: its summing-up line, with its time so far, written at once, before
// a command's answer lets the next job on its issue begin; with `--verbose`, a plain line.
fn outcome(mark: Mark, text: &str, full: &str) {
    let started = JOB.with(|a| a.borrow_mut().as_mut().map(|job| {
        job.summed_up = true;
        job.started
    }));
    match started {
        Some(started) => keep(mark, None, None, &format!("{full:<9} {}", seconds(started.elapsed())), true),
        None => keep(mark, None, None, full, false),
    }
    match started {
        Some(started) if !verbose() => write(mark, None, None, &format!("{text:<9} {}", seconds(started.elapsed())), true),
        _ => write(mark, None, None, text, false),
    }
}

// A call to Linear, GitHub, the Durable Object or git that took longer than this is logged
// with its time (`Call::Slow`), so a slow job shows where its time went. Orca's cutoffs are
// longer (`exec::slow_cutoff`).
pub const SLOW: Duration = Duration::from_secs(2);

// `site` is the call site: fixed names only, e.g. `linear Recent` or `orca worktree ps`.
pub fn slow(site: impl FnOnce() -> String, started: Instant) {
    slow_after(site, started, SLOW);
}

// `slow` for a call that is slow later than most: `cutoff` instead of `SLOW`.
pub fn slow_after(site: impl FnOnce() -> String, started: Instant, cutoff: Duration) {
    let took = started.elapsed();
    if enabled() && took >= cutoff {
        event(Call::Slow { site: site(), took });
    }
}

// A job's end: what it took in all, as its summing-up line, e.g.
// `TW-694   readIssue          done      0.6 s`, when no outcome summed it up already.
pub fn took(took: Duration) {
    let unsummed = JOB.with(|a| a.borrow().as_ref().is_some_and(|job| !job.summed_up));
    if unsummed {
        keep(Mark::Quiet, None, None, &format!("{:<9} {}", "done", seconds(took)), true);
    }
    if verbose() {
        write(Mark::Routine, None, None, &format!("took {}", seconds(took)), false);
    } else if unsummed {
        write(Mark::Quiet, None, None, &format!("{:<9} {}", "done", seconds(took)), true);
    }
}

// The local time for a person; with `--verbose`, UTC to the millisecond, to order lines over
// days and match them with the Durable Object's and Linear's.
fn clock() -> String {
    if verbose() { now_iso() } else { local_clock() }
}

fn seconds(took: Duration) -> String {
    format!("{:.1} s", took.as_secs_f64())
}

// One line, whole: the lock is held for the write. A job's lines lead with its issue, and
// its last, or each of a job with no issue, give its kind a column.
fn write(mark: Mark, area: Option<&str>, issue: Option<&str>, text: &str, last: bool) {
    if !enabled() {
        return;
    }
    let (lead, issue, text) = JOB.with(|a| shaped(a.borrow().as_ref(), area, issue, text, verbose(), last));
    if to_sink(mark, &lead, issue, &text) {
        return;
    }
    let shown = formatted(&clock(), mark, &lead, issue, &text, colour());
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{shown}");
    let _ = out.flush();
}

// A line's lead, whether that is an issue's identifier, and its text. A line of no job leads
// with its issue, else its area, e.g. `watch`, else `·`; a job's names its area in its text.
fn shaped(job: Option<&Job>, area: Option<&str>, issue: Option<&str>, text: &str, verbose: bool, last: bool) -> (String, bool, String) {
    let Some(job) = job else {
        return match (issue, area) {
            (Some(issue), _) => (issue.into(), true, text.into()),
            (None, Some(area)) => (area.into(), false, text.into()),
            (None, None) => ("·".into(), false, text.into()),
        };
    };
    let text = match area {
        Some(area) if job.kind.area() != Some(area) => format!("{area}: {text}"),
        _ => text.to_string(),
    };
    let text = if verbose && !last { format!("{}: {text}", job.id) } else { text };
    let text = if last || (job.issue.is_none() && !verbose) { format!("{:<KIND$} {text}", if verbose { job.kind.label() } else { job.kind.column() }) } else { text };
    match &job.issue {
        Some(issue) => (issue.clone(), true, text),
        None => ("·".into(), false, text),
    }
}

// The kind's column, after a job's lead.
const KIND: usize = 18;

// The lead's column, sign included.
const LEAD: usize = 8;

// E.g. `14:51:37  TW-694   readIssue …` or `12:21:47  ✖ host   …`: colour goes on the sign
// and an issue's identifier only, never inside an ID, and the padding is outside it.
fn formatted(clock: &str, mark: Mark, lead: &str, issue: bool, text: &str, colour: bool) -> String {
    let paint = |text: &str, code: &str| if colour { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_string() };
    let (sign, width) = match mark.sign() {
        Some((sign, code)) => (format!("{} ", paint(sign, code)), LEAD - 2),
        None => (String::new(), LEAD),
    };
    let pad = " ".repeat(width.saturating_sub(lead.chars().count()));
    let lead = if issue { paint(lead, "36") } else { lead.to_string() };
    format!("{clock}  {sign}{lead}{pad} {text}")
}

// A body, a title, a description or a display name never has one of these keys, so the
// walk can't pick up text.
const SHOWN: [&str; 16] = ["id", "issue", "claims", "issueId", "agentSessionId", "sessionId", "stateId", "delegateId", "parentIssueId", "worktreeId", "repoId", "name", "type", "ephemeral", "kind", "status"];

// E.g. `agentSessionId=s-1 type=thought`; a shown key holding a list shows each item.
pub fn ids(value: &Value) -> String {
    let mut found = Vec::new();
    collect(value, &mut found);
    found.join(" ")
}

fn collect(value: &Value, found: &mut Vec<String>) {
    let Value::Object(fields) = value else { return };
    for (key, value) in fields {
        let shown = SHOWN.contains(&key.as_str());
        let items = match value {
            Value::Array(items) => items.iter().collect(),
            other => vec![other],
        };
        for item in items {
            match item {
                Value::String(s) if shown => found.push(format!("{key}={s}")),
                Value::Number(_) | Value::Bool(_) if shown => found.push(format!("{key}={item}")),
                Value::Object(_) => collect(item, found),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests;
