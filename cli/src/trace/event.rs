use std::borrow::Cow;

use super::Mark;
use super::calls::Call;
use super::listen::{Disk, Host, Realtime, Update, Watch};
use super::notes::{Context, Reap, Triage};
use super::work::{Outcome, Refresh, Work};
use crate::error::{Error, Unsent};

// What happened, for `listen`'s terminal: a call site says which, with its IDs, identifiers,
// enums, durations and errors, and only this module says how it reads.
#[derive(Debug, PartialEq)]
pub enum Event {
    Call(Call),
    Work(Work),
    Triage(Triage),
    Context(Context),
    Reap(Reap),
    Refresh(Refresh),
    Outcome(Outcome),
    Update(Update),
    Realtime(Realtime),
    Watch(Watch),
    Host(Host),
    Disk(Disk),
}

macro_rules! from {
    ($($family:ident),*) => {
        $(impl From<$family> for Event {
            fn from(event: $family) -> Event {
                Event::$family(event)
            }
        })*
    };
}

from!(Call, Work, Triage, Context, Reap, Refresh, Outcome, Update, Realtime, Watch, Host, Disk);

impl Event {
    pub(super) fn line(&self) -> Line {
        match self {
            Event::Call(event) => event.line(),
            Event::Work(event) => event.line(),
            Event::Triage(event) => event.line(),
            Event::Context(event) => event.line(),
            Event::Reap(event) => event.line(),
            Event::Refresh(event) => event.line(),
            Event::Outcome(event) => event.line(),
            Event::Update(event) => event.line(),
            Event::Realtime(event) => event.line(),
            Event::Watch(event) => event.line(),
            Event::Host(event) => event.line(),
            Event::Disk(event) => event.line(),
        }
    }
}

// Where a line goes: `Verbose` only with `--verbose`; `Notice` on stderr, whether or not the
// rest is on; `Outcome` sums up the job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Show {
    Always,
    Verbose,
    Notice,
    Outcome,
}

// An event as it is shown: `area` leads a line of no job, e.g. `watch`, and `issue` one of
// no job that is an issue's.
pub(super) struct Line {
    pub area: Option<&'static str>,
    pub issue: Option<String>,
    pub mark: Mark,
    pub parts: Parts,
    pub show: Show,
}

impl Line {
    pub(super) fn new(mark: Mark, parts: Parts) -> Line {
        Line { area: None, issue: None, mark, parts, show: Show::Always }
    }

    pub(super) fn routine(parts: Parts) -> Line {
        Line::new(Mark::Routine, parts)
    }

    pub(super) fn warning(parts: Parts) -> Line {
        Line::new(Mark::Warning, parts)
    }

    pub(super) fn error(parts: Parts) -> Line {
        Line::new(Mark::Error, parts)
    }

    pub(super) fn area(self, area: &'static str) -> Line {
        Line { area: Some(area), ..self }
    }

    pub(super) fn issue(self, issue: &str) -> Line {
        Line { issue: Some(issue.into()), ..self }
    }

    pub(super) fn show(self, show: Show) -> Line {
        Line { show, ..self }
    }
}

// A line's pieces. An opaque ID (a terminal handle; a worktree, agent, session, command or
// Linear ID; a claim holding one) shows only with `--verbose`, after a space; a plain line
// drops it, never shortens it. An error shows by its kind on a plain line, as its text can
// hold one; with `--verbose`, its text.
#[derive(Debug, Default)]
pub(super) struct Parts(Vec<Part>);

#[derive(Debug)]
enum Part {
    Text(Cow<'static, str>),
    Opaque(String),
    Error(&'static str, String),
}

impl Parts {
    pub(super) fn of(text: impl Into<Cow<'static, str>>) -> Parts {
        Parts::default().text(text)
    }

    pub(super) fn text(mut self, text: impl Into<Cow<'static, str>>) -> Parts {
        self.0.push(Part::Text(text.into()));
        self
    }

    pub(super) fn opaque(mut self, id: impl ToString) -> Parts {
        self.0.push(Part::Opaque(id.to_string()));
        self
    }

    pub(super) fn error(mut self, error: &Error) -> Parts {
        self.0.push(Part::Error(kind(error), error.to_string()));
        self
    }

    pub(super) fn then(mut self, more: Parts) -> Parts {
        self.0.extend(more.0);
        self
    }

    pub(super) fn render(&self, verbose: bool) -> String {
        self.0.iter().fold(String::new(), |mut shown, part| {
            match part {
                Part::Text(text) => shown.push_str(text),
                Part::Opaque(id) if verbose && !id.is_empty() => {
                    shown.push(' ');
                    shown.push_str(id);
                }
                Part::Opaque(_) => {}
                Part::Error(_, text) if verbose => {
                    shown.push_str(": ");
                    shown.push_str(text);
                }
                Part::Error(kind, _) => {
                    shown.push_str(" (");
                    shown.push_str(kind);
                    shown.push(')');
                }
            }
            shown
        })
    }
}

pub fn kind(error: &Error) -> &'static str {
    match error {
        Error::Usage(_) => "bad input",
        Error::Unauthorized(_) => "unauthorized",
        Error::RateLimited { .. } => "budget spent",
        Error::Unavailable(_) => "unavailable",
        Error::Orca(_) => "Orca",
        // Orca's own reason, which holds no one's text, so the plain line can name it.
        Error::Unsent(Unsent::NoAgent, _) => "Orca refused: no agent",
        Error::Unsent(Unsent::TerminalGone, _) => "Orca refused: terminal gone",
        Error::Later(_) | Error::Waits(..) => "not yet",
        Error::Failed(_) | Error::Io { .. } => "error",
    }
}

// `PR #7`, as a line names a PR.
pub(super) fn pr(number: u64) -> String {
    format!("PR #{number}")
}

// `TW-1, TW-2`.
pub(super) fn listed(items: &[impl AsRef<str>]) -> String {
    items.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(", ")
}
