use std::time::Duration;

use super::Mark;
use super::event::{Line, Parts, Show};
use crate::cli::BRANCH;
use crate::error::Error;

// The connection to the relay: on stderr, whether or not the rest is on.
#[derive(Debug, PartialEq)]
pub enum Realtime {
    Reconnecting { ended: Ended, delay: Duration },
    Connected { again: bool },
    // The relay closed this connection for a newer acting one's.
    Superseded,
    // A frame that didn't read, which nothing answers.
    Skipped(Error),
}

// How a wait said a terminal ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closed {
    // With the code its program ended with where Orca names one.
    Exited(Option<i32>),
    // Orca no longer knows the handle.
    Stale,
}

// Why a connection ended, to try again.
#[derive(Debug, PartialEq)]
pub enum Ended {
    // The Worker's 503 named a setting it lacks.
    Unset(Setting),
    Status(u16),
    NoAnswer,
    Closed,
    ReaderStopped,
    HandshakeTimedOut,
    Failed { what: Step, error: Error },
}

// The log `listen` keeps on disk (trace/disk.rs): it never opened, or a write ended it.
#[derive(Debug, PartialEq)]
pub enum Disk {
    Off(Error),
    Stopped(Error),
}

impl Disk {
    pub(super) fn line(&self) -> Line {
        match self {
            Disk::Off(error) => Line::warning(Parts::of("no log kept on disk").error(error)),
            Disk::Stopped(error) => Line::warning(Parts::of("the log on disk stopped for this run").error(error)),
        }
    }
}

// The Worker settings whose absence refuses /realtime (src/worker/realtime.ts, UNSET_SETTINGS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    OwnerUserId,
    RealtimeSecret,
}

impl Setting {
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "OWNER_USER_ID" => Some(Setting::OwnerUserId),
            "REALTIME_SECRET" => Some(Setting::RealtimeSecret),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Setting::OwnerUserId => "OWNER_USER_ID",
            Setting::RealtimeSecret => "REALTIME_SECRET",
        }
    }
}

// A step of the connection's that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Reader,
    Ping,
    Connection,
    Connecting,
    Config,
    Observation,
    Result,
    Listing,
}

// The watcher's listings of Orca and its waits on terminals (watch.rs).
#[derive(Debug, PartialEq)]
pub enum Watch {
    Listing(Error),
    // What the first listing can't see change (handlers::missed).
    Missed(Error),
    Wait(Error),
    EndedAtOnce,
    // A wait's answer that a terminal is gone (orca::wait_exit), before a listing checks it:
    // an issue's agent's where the listing knew it.
    Waited { issue: Option<String>, handle: String, said: Closed },
    // Its wait said so, but a listing still has it: it is waited on again.
    StillListed { handle: String },
}

#[derive(Debug, PartialEq)]
pub enum Host {
    // By its name, when Orca says it.
    NotAnswering { name: Option<String>, id: String, error: Error },
    // Its first answer after `after` of silence (health.rs).
    Answering { name: Option<String>, id: String, after: Duration },
    // The label its workers' issues show couldn't be made (offline.rs); tried again shortly.
    NotMarked { id: String, error: Error },
    // A machine a repo's recipe made, set up and listed for that repo (recipe.rs), or
    // the failure that destroyed it or left none.
    Made { name: String, repo: String },
    NotMade { repo: String, error: Error },
    // Each make or wake is also logged when asked for, as it runs minutes off the lanes.
    MakeAsked { repo: String },
    WakeAsked { name: String },
    // One woken for its repo's next worker, or put back to sleep by a failure, and one put
    // to sleep once idle past its time (recipe/sleep.rs).
    Woken { name: String },
    NotWoken { name: String, error: Error },
    Asleep { name: String },
    // The start of each step of a machine's life: a make claimed for `issue`, the fleet's
    // command, with the name it takes and how many of the repo's `machines` it makes
    // (`counted`, itself included); a wake; a sleep, once `suspendHost` passed its checks.
    Making { repo: String, name: String, counted: usize, machines: i64, issue: String },
    Waking { name: String, issue: String },
    Sleeping { name: String },
    // A `suspendHost` declined: by name when Orca says it.
    NotSleeping { name: Option<String>, id: String, why: Unslept },
    // A machine's destroy by its recipe (recipe.rs), from `host rm` or a failed make.
    Destroying { name: String },
    Destroyed { name: String },
    NotDestroyed { name: String },
    // A dormant machine's destroy that kept it (recipe/retire.rs).
    Kept { name: String },
    // An ask for a machine refused, a repo's make or a machine's wake: said once until its
    // reason changes.
    NotMaking { repo: String, why: Declined },
    NotWaking { name: String, why: Declined },
    // The fleet asks no machine for `repo` while its request waits, and why.
    NoMachine { repo: String, why: &'static str },
}

// Why a make or wake wasn't begun.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Declined {
    // A make or wake failed within its cool-down.
    CoolDown,
    // A make, wake or sleep is running.
    Running,
}

// Why a machine wasn't put to sleep.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unslept {
    NotAnswering,
    NotAdmitted,
    Running,
    NoSleep,
    AlreadyAsleep,
    Busy,
}

// `listen` keeping itself current (update.rs). A SHA is short, a path the primary checkout's.
// A development build makes some of these and any other build the rest, hence the allowance.
#[derive(Debug, PartialEq)]
#[allow(dead_code)]
pub enum Update {
    Off(Unfit),
    NotHeld { after: String },
    StillNotHeld { after: String },
    Unfetched(Unpulled),
    Skipped(Unpulled),
    At { head: String },
    Moved { from: String, head: String },
    BuildFailed(Unbuilt),
    Built { head: String },
    NotPlaced(Error),
    // The Worker's build couldn't be fetched.
    NotFetched(Error),
    Building,
    Draining,
    BuiltAlready { head: String },
    Restarting { head: String },
    // A Linear release of the build put in place, linking `issues`, or of it already.
    Released { head: String, issues: usize },
    ReleasedAlready { head: String },
    NotReleased(Error),
    Restart(Restart),
    // While draining: a command left for its resend, a held job dropped.
    LeftForResend { command: String },
    Dropped { job: String },
}

// Why the checkout `listen` runs from can't update it.
#[derive(Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Unfit {
    // A build the Worker serves is of a commit and a repo, which this one isn't stamped with.
    Unstamped,
    // CI makes no build for this platform.
    OtherPlatform,
    NotACheckout,
    Worktree { path: String },
    NotTolkster { path: String },
    NoOrigin { path: String },
}

// Why a pull left the checkout as it was, by git's exit codes: git's stderr can carry a
// remote URL, and with it a credential.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(feature = "develop"), allow(dead_code))]
pub enum Unpulled {
    NoBranch,
    OffBranch { branch: String },
    Status(i32),
    Dirty,
    Fetch(i32),
    Head(i32),
    Target(i32),
    Ahead,
    FastForward(i32),
}

#[derive(Debug, PartialEq)]
#[cfg_attr(not(feature = "develop"), allow(dead_code))]
pub enum Unbuilt {
    NotRun(Error),
    Exited(i32),
}

// Why `listen` starts again in its terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Restart {
    Built { head: String },
    Stuck { what: Stuck, minutes: u64 },
}

// A job, Orca's listing or a build stuck this long is most likely on a call that never
// returns: `listen` starts itself again, and a command is sent again.
pub const STUCK_AFTER: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stuck {
    Build,
    Job(String),
    Listing,
}

const IN_PLACE: &str = "; still running the binary in place";

impl Realtime {
    pub(super) fn line(&self) -> Line {
        match self {
            Realtime::Reconnecting { ended, delay } => {
                Line::warning(self::ended(ended).text(format!("; reconnecting in {} s", delay.as_secs()))).area("realtime").show(Show::Notice)
            }
            Realtime::Connected { again } => Line::new(if *again { Mark::Recovered } else { Mark::Routine }, Parts::of("connected")).area("realtime").show(Show::Notice),
            Realtime::Superseded => Line::warning(Parts::of("superseded by another listen; finishing what it holds, then exiting")).area("realtime").show(Show::Notice),
            Realtime::Skipped(error) => Line::warning(Parts::of("skipped a frame").error(error)),
        }
    }
}

impl Watch {
    pub(super) fn line(&self) -> Line {
        let line = match self {
            Watch::Listing(error) => Line::error(Parts::of("listing Orca failed").error(error)),
            // Each listing asks again; a failed wait is asked again, or left to the listings.
            Watch::Missed(error) => Line::warning(Parts::of("reading what the first listing can't see failed").error(error)),
            Watch::Wait(error) => Line::warning(Parts::of("a wait for a terminal's exit failed").error(error)),
            Watch::EndedAtOnce => Line::warning(Parts::of("a wait for a terminal's exit ended at once")),
            Watch::Waited { issue, handle, said } => {
                let said = match said {
                    Closed::Exited(Some(code)) => format!("closed (exit code {code})"),
                    Closed::Exited(None) => "closed".into(),
                    Closed::Stale => "is gone".into(),
                };
                let line = Line::routine(Parts::of(if issue.is_some() { "agent terminal" } else { "a terminal" }).opaque(handle).text(format!(" {said}")));
                return match issue {
                    Some(issue) => line.issue(issue),
                    None => line.area("watch"),
                };
            }
            Watch::StillListed { handle } => Line::warning(Parts::of("a wait said a terminal is gone but Orca still lists it").opaque(handle)),
        };
        line.area("watch")
    }
}

impl Host {
    pub(super) fn line(&self) -> Line {
        match self {
            Host::NotAnswering { name, id, error } => Line::error(host(name, id).text(" not answering").error(error)).area("host"),
            Host::Answering { name, id, after } => Line::routine(host(name, id).text(format!(" answering again after {} min", after.as_secs().div_ceil(60)))).area("host"),
            Host::NotMarked { id, error } => Line::warning(host(&None, id).text("'s issues couldn't be labelled").error(error)).area("host"),
            Host::MakeAsked { repo } => Line::routine(Parts::of(format!("a machine asked for from {repo}'s recipe"))).area("host"),
            Host::WakeAsked { name } => Line::routine(Parts::of(format!("{name} asked to wake"))).area("host"),
            Host::Made { name, repo } => Line::routine(Parts::of(format!("{name} made from {repo}'s recipe and takes its workers"))).area("host"),
            Host::NotMade { repo, error } => Line::error(Parts::of(format!("no machine made from {repo}'s recipe")).error(error)).area("host"),
            Host::Woken { name } => Line::routine(Parts::of(format!("{name} woken and takes its workers"))).area("host"),
            Host::NotWoken { name, error } => Line::error(Parts::of(format!("{name} not woken")).error(error)).area("host"),
            Host::Asleep { name } => Line::routine(Parts::of(format!("{name} idle past its time, and asleep"))).area("host"),
            Host::Making { repo, name, counted, machines, issue } => {
                Line::routine(Parts::of(format!("making {name} from {repo}'s recipe, machine {counted} of {machines}, for {issue}"))).area("host")
            }
            Host::Waking { name, issue } => Line::routine(Parts::of(format!("waking {name} for {issue}"))).area("host"),
            Host::Sleeping { name } => Line::routine(Parts::of(format!("putting {name} to sleep"))).area("host"),
            Host::NotSleeping { name, id, why } => {
                let why = match why {
                    Unslept::NotAnswering => "it isn't answering",
                    Unslept::NotAdmitted => "it holds no admitted credential",
                    Unslept::Running => "a make, wake or sleep is running",
                    Unslept::NoSleep => "no recipe machine that sleeps has it",
                    Unslept::AlreadyAsleep => "it is asleep already",
                    Unslept::Busy => "an agent works on it, or it changed",
                };
                Line::routine(host(name, id).text(format!(" not put to sleep: {why}"))).area("host")
            }
            // The owner's `host rm` has no log of its own, so these reach its stderr too.
            Host::Destroying { name } => Line::routine(Parts::of(format!("destroying {name} by its recipe's destroy"))).area("host").show(Show::Notice),
            Host::Destroyed { name } => Line::routine(Parts::of(format!("{name} destroyed"))).area("host").show(Show::Notice),
            Host::NotDestroyed { name } => Line::error(Parts::of(format!("{name} not destroyed: its recipe's destroy failed"))).area("host").show(Show::Notice),
            Host::NotMaking { repo, why } => Line::routine(Parts::of(format!("no machine made from {repo}'s recipe: {}", declined(why)))).area("host"),
            Host::NotWaking { name, why } => Line::routine(Parts::of(format!("{name} not woken: {}", declined(why)))).area("host"),
            Host::NoMachine { repo, why } => Line::routine(Parts::of(format!("no machine asked for {repo}: {why}"))).area("host"),
            Host::Kept { name } => Line::routine(Parts::of(format!("{name} dormant past its time, but kept"))).area("host"),
        }
    }
}

fn declined(why: &Declined) -> String {
    match why {
        Declined::CoolDown => "cooling down after a failure".into(),
        Declined::Running => "a make, wake or sleep is running".into(),
    }
}

fn host(name: &Option<String>, id: &str) -> Parts {
    match name {
        Some(name) => Parts::of(name.clone()),
        None => Parts::of("a host").opaque(id),
    }
}

impl Update {
    pub(super) fn line(&self) -> Line {
        let line = match self {
            Update::Off(unfit) => Line::warning(Parts::of(format!("{}; won't update itself", self::unfit(unfit)))),
            Update::NotHeld { after } => Line::routine(Parts::of(format!("{after} isn't there to take yet; trying again shortly"))),
            Update::StillNotHeld { after } => Line::routine(Parts::of(format!("{after} is still not there to take; waiting for the next push"))),
            Update::Unfetched(why) => Line::routine(Parts::of(format!("{}; trying again shortly", unpulled(why)))),
            Update::Skipped(why) => Line::routine(Parts::of(format!("skipped: {}", unpulled(why)))),
            Update::At { head } => Line::routine(Parts::of(format!("{BRANCH} is at {head}"))).show(Show::Verbose),
            Update::Moved { from, head } => Line::routine(Parts::of(format!("{BRANCH} {from} -> {head}"))),
            Update::BuildFailed(Unbuilt::NotRun(error)) => Line::routine(Parts::of("build failed").error(error).text(IN_PLACE)),
            Update::BuildFailed(Unbuilt::Exited(code)) => Line::routine(Parts::of(format!("build failed: cargo build exited with {code}{IN_PLACE}"))),
            Update::Built { head } => Line::routine(Parts::of(format!("built {head}, the binary in place"))),
            Update::NotPlaced(error) => Line::routine(Parts::of("couldn't put the new build in place").error(error).text(IN_PLACE)),
            Update::NotFetched(error) => Line::routine(Parts::of("couldn't fetch the new build").error(error).text(IN_PLACE)),
            Update::Building => Line::routine(Parts::of("a build is running; pulling once it ends")),
            Update::Draining => Line::routine(Parts::of("skipped: restarting on a new build, which pulls as it starts")),
            Update::BuiltAlready { head } => Line::routine(Parts::of(format!("{head} is built already"))).show(Show::Verbose),
            Update::Restarting { head } => Line::routine(Parts::of(format!("built {head}; restarting once what's in flight is done"))),
            Update::Released { head, issues } => Line::routine(Parts::of(format!("released {head} in Linear, linking {issues} issues"))),
            Update::ReleasedAlready { head } => Line::routine(Parts::of(format!("{head} is released in Linear already"))).show(Show::Verbose),
            Update::NotReleased(error) => Line::routine(Parts::of("couldn't sync the release to Linear").error(error)),
            Update::Restart(Restart::Built { head }) => Line::routine(Parts::of(format!("restarting on {head}"))),
            Update::Restart(Restart::Stuck { what, minutes }) => {
                let what = match what {
                    Stuck::Build => Parts::of("a build"),
                    Stuck::Job(id) => Parts::of("a job").opaque(id),
                    Stuck::Listing => Parts::of("Orca's listing"),
                };
                return Line::routine(what.text(format!(" has made no progress for {minutes} minutes or more; starting again")));
            }
            Update::LeftForResend { command } => Line::routine(Parts::of("draining; a command").opaque(command).text(" is left for its resend")),
            Update::Dropped { job } => Line::routine(Parts::of("draining; a held job").opaque(job).text(" is dropped")),
        };
        line.area("update")
    }
}

fn ended(ended: &Ended) -> Parts {
    match ended {
        Ended::Unset(setting) => Parts::of(format!("the Worker has no {} set", setting.name())),
        Ended::Status(status) => Parts::of(format!("the Worker answered HTTP {status}")),
        Ended::NoAnswer => Parts::of("no answer from the Worker"),
        Ended::Closed => Parts::of("the Worker closed the connection"),
        Ended::ReaderStopped => Parts::of("the reader stopped"),
        Ended::HandshakeTimedOut => Parts::of("the handshake timed out"),
        Ended::Failed { what, error } => Parts::of(match what {
            Step::Reader => "starting the reader failed",
            Step::Ping => "sending a ping failed",
            Step::Connection => "the connection failed",
            Step::Connecting => "connecting failed",
            Step::Config => "telling the relay the settings failed",
            Step::Observation => "sending an observation failed",
            Step::Result => "sending a command's result failed",
            Step::Listing => "sending the listing failed",
        })
        .error(error),
    }
}

impl Ended {
    pub fn failed(what: Step, error: impl std::fmt::Display) -> Ended {
        Ended::Failed { what, error: Error::Unavailable(error.to_string()) }
    }
}

fn unfit(unfit: &Unfit) -> String {
    match unfit {
        Unfit::Unstamped => "this build isn't stamped with its commit and repo".into(),
        Unfit::OtherPlatform => "no build is made for this platform".into(),
        Unfit::NotACheckout => "not in a git checkout".into(),
        Unfit::Worktree { path } => format!("{path} is a worktree, not the primary checkout"),
        Unfit::NotTolkster { path } => format!("{path} isn't a checkout of tolkster"),
        Unfit::NoOrigin { path } => format!("the origin of {path} isn't a GitHub repo"),
    }
}

fn unpulled(why: &Unpulled) -> String {
    match why {
        Unpulled::NoBranch => "the checkout has no branch checked out".into(),
        Unpulled::OffBranch { branch } => format!("the checkout is on '{branch}', not {BRANCH}"),
        Unpulled::Status(code) => format!("git status failed with {code}"),
        Unpulled::Dirty => "the checkout has uncommitted or untracked files".into(),
        Unpulled::Fetch(code) => format!("fetch failed with {code}; run git fetch in the checkout to see why"),
        Unpulled::Head(code) => format!("reading HEAD failed with {code}"),
        Unpulled::Target(code) => format!("reading origin/{BRANCH} failed with {code}"),
        Unpulled::Ahead => format!("{BRANCH} has commits origin/{BRANCH} hasn't, so it can't fast-forward"),
        Unpulled::FastForward(code) => format!("fast-forward failed with {code}"),
    }
}
