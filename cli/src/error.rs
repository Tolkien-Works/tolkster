use std::fmt;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    // Bad input from the caller: exit 2.
    Usage(String),
    // Linear refused the app token. The caller mints a new one and retries once.
    Unauthorized(String),
    // GitHub's or Linear's budget for the app is spent: exit 1. `wait` is until it resets,
    // as they say (github/app.rs and linear/cost.rs, `Limits`), for a caller that can wait
    // rather than fail.
    RateLimited { message: String, wait: Duration },
    // GitHub's or Linear's server errored (500, 502, 503, 504) or the connection failed or
    // timed out: exit 1, though nothing was refused, so a call safe to repeat is worth
    // another try (`gateway::repeat_safe_call`).
    Unavailable(String),
    // Orca failed or refused a call (orca.rs, `call`): exit 1. Its text is Orca's, which can
    // carry the workstation's paths, so it is logged, never posted.
    Orca(String),
    // Orca refused a send, or the terminal it named is gone, with proof that nothing was
    // typed (orca/live.rs, `send`): as `Orca`, and a round may bring the worker back instead.
    Unsent(Unsent, String),
    // Nothing failed, but it can't be decided yet: a host that may hold what it acts on wasn't
    // heard, or what would decide it couldn't be read. A command's is `retryable`: exit 1.
    Later(String),
    // As `Later`, waiting on what `Waits` names, which the Durable Object sends the command
    // again on once it clears.
    Waits(Waits, String),
    // A file, pipe or process failed while `doing` it (as "reading machines.json"), kept as
    // the source (`Context`): exit 1, as `Failed`.
    Io { doing: String, source: IoSource },
    // Anything else: exit 1.
    Failed(String),
}

// An `io::Error`, shared so `Error` stays `Clone`, and equal by its kind and text.
#[derive(Clone, Debug)]
pub struct IoSource(Arc<std::io::Error>);

impl PartialEq for IoSource {
    fn eq(&self, other: &Self) -> bool {
        self.0.kind() == other.0.kind() && self.0.to_string() == other.0.to_string()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Usage(message) | Error::Unauthorized(message) | Error::RateLimited { message, .. } | Error::Unavailable(message) | Error::Orca(message) | Error::Unsent(_, message) | Error::Later(message) | Error::Waits(_, message) | Error::Failed(message) => f.write_str(message),
            Error::Io { doing, source } => write!(f, "{doing}: {}", source.0),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source.0.as_ref()),
            _ => None,
        }
    }
}

// What a failed step was doing, said before the failure, as `fs::read(path).context(path.display())`
// or `.context("reading the brief")`. An I/O failure keeps its source (`Error::Io`); a JSON one
// is `Failed`, its text after `doing`.
pub trait Context<T> {
    fn context(self, doing: impl fmt::Display) -> Result<T, Error>;
}

impl<T> Context<T> for Result<T, std::io::Error> {
    fn context(self, doing: impl fmt::Display) -> Result<T, Error> {
        self.map_err(|error| Error::Io { doing: doing.to_string(), source: IoSource(Arc::new(error)) })
    }
}

impl<T> Context<T> for Result<T, serde_json::Error> {
    fn context(self, doing: impl fmt::Display) -> Result<T, Error> {
        self.map_err(|error| Error::Failed(format!("{doing}: {error}")))
    }
}

// What a command that can't run yet waits on (docs/state-machine.md, Commands).
#[derive(Clone, Debug, PartialEq)]
pub enum Waits {
    // A paired host, by its ID, to answer: silent, or a recipe machine waking.
    Host(String),
    // No recipe machine being made, woken or put to sleep (`recipe::Making`).
    Lock,
    // This machine's Orca to answer: not found to run, or its app not running.
    Orca,
}

// Why Orca typed nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unsent {
    NoAgent,
    TerminalGone,
}

impl Error {
    // Whether Orca failed or refused the call, so its text may be logged but never posted.
    pub fn orcas(&self) -> bool {
        matches!(self, Error::Orca(_) | Error::Unsent(..))
    }

    // How long until the budget resets, when this is its being spent.
    pub fn rate_limited(&self) -> Option<Duration> {
        match self {
            Error::RateLimited { wait, .. } => Some(*wait),
            _ => None,
        }
    }

    // `host` isn't answering: what acts on it waits for it.
    pub fn silent(host: &str) -> Error {
        Error::Waits(Waits::Host(host.to_string()), format!("{host} isn't answering"))
    }

    // The host of a worktree, which didn't answer: the command waits for it.
    pub fn unheard(host: Option<&str>) -> Error {
        match host {
            Some(host) => Error::Waits(Waits::Host(host.to_string()), format!("{host}, the host of its worktree, didn't answer")),
            None => Error::Later("the host of its worktree didn't answer".into()),
        }
    }

    // Nothing failed, but it can't be decided yet (`Later`, `Waits`).
    pub fn later(&self) -> bool {
        matches!(self, Error::Later(_) | Error::Waits(..))
    }

    pub fn unavailable(&self) -> bool {
        matches!(self, Error::Unavailable(_))
    }

    // A program that couldn't be run because it isn't there.
    pub fn not_found(&self) -> bool {
        matches!(self, Error::Io { source, .. } if source.0.kind() == std::io::ErrorKind::NotFound)
    }

    // This error with what went wrong after it, keeping whether it was Orca's.
    pub fn and_then_failed(self, after: &str) -> Error {
        match self {
            Error::Orca(message) => Error::Orca(format!("{message}; {after}")),
            Error::Unsent(why, message) => Error::Unsent(why, format!("{message}; {after}")),
            error => Error::Failed(format!("{error}; {after}")),
        }
    }

    // This error, with a failed give-back of what the failed action had taken (`what`, as
    // "its claims") after it.
    pub fn and_give_back(self, given_back: Result<(), Error>, what: &str) -> Error {
        match given_back {
            Ok(()) => self,
            Err(releasing) => self.and_then_failed(&format!("and giving back {what} failed: {releasing}")),
        }
    }
}

impl From<ureq::Error> for Error {
    fn from(error: ureq::Error) -> Self {
        match error {
            ureq::Error::Timeout(_) | ureq::Error::Io(_) | ureq::Error::ConnectionFailed | ureq::Error::HostNotFound => Error::Unavailable(error.to_string()),
            error => Error::Failed(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests;
