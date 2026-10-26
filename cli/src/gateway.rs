// The gateway to the APIs whose budget Tolkster spends, GitHub's and Linear's, and the one
// policy for a call that fails (docs/budget.md). Each budget is kept as its API's responses
// last said, and each call runs in a lane: an urgent one waits only on a spent budget, a
// normal read on a low one too. What a failure comes to and how long it waits are decided
// here: nothing else sleeps for a budget or tries a call again. Orca has no budget, and its
// one resend is `Orca::send`'s, as Orca's own answer allows.

use std::cell::Cell;
use std::sync::Mutex;
use std::time::Duration;

use crate::error::Error;
use crate::sync::lock;
use crate::time;
use crate::trace::{self, Call};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    GitHub,
    Linear,
}

impl Api {
    fn name(self) -> &'static str {
        match self {
            Api::GitHub => "GitHub",
            Api::Linear => "Linear",
        }
    }
}

// The lane a call runs in. Urgent unless set, so a worker's command, a post and every
// command of the machine's go ahead; `listen`'s own jobs run Normal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    Urgent,
    Normal,
}

// Ambient, never passed: the lane is the job's, set once where a lane's thread takes it
// (scheduler.rs), and read here under every Linear and GitHub call the job makes, many frames
// and modules down, which no signature between them carries.
thread_local! {
    static LANE: Cell<Lane> = const { Cell::new(Lane::Urgent) };
}

// Runs `run` with the calls it makes on this thread in `lane`, and gives the thread its lane
// back after, a panic's unwinding included. A thread `run` starts is Urgent.
pub fn in_lane<T>(lane: Lane, run: impl FnOnce() -> T) -> T {
    struct Restore(Lane);
    impl Drop for Restore {
        fn drop(&mut self) {
            LANE.with(|l| l.set(self.0));
        }
    }
    let _restore = Restore(LANE.with(|l| l.replace(lane)));
    run()
}

pub fn lane() -> Lane {
    LANE.with(Cell::get)
}

// Below this share of a budget left, in percent, it is low: logged, and a normal read waits
// for its reset.
const LOW_BUDGET: u64 = 20;

pub fn is_low(limit: u64, remaining: u64) -> bool {
    limit != 0 && remaining.saturating_mul(100) < limit.saturating_mul(LOW_BUDGET)
}

// How long a spent budget waits when its API gave no time: GitHub's secondary limits ask a
// minute, and Linear's budgets are hourly, so a minute is the least worth holding for.
const UNSAID_WAIT: u64 = 60;
// The bounds of any wait: a clock ahead of the API's would otherwise try again at once, and
// a wild header wait for ever. Every budget resets within the hour.
const WAIT_BOUNDS: (u64, u64) = (5, 60 * 60);

// The one wait: `secs` as the API said, else UNSAID_WAIT, within WAIT_BOUNDS.
pub fn wait(secs: Option<u64>) -> Duration {
    Duration::from_secs(secs.unwrap_or(UNSAID_WAIT).clamp(WAIT_BOUNDS.0, WAIT_BOUNDS.1))
}

// Until `reset_ms`, as of `now_ms`, a second over so a call made then lands after it.
pub fn until(reset_ms: u64, now_ms: u64) -> Duration {
    wait(Some((reset_ms.saturating_sub(now_ms) / 1000).saturating_add(1)))
}

// One budget as its API's responses said: `remaining` of `limit` until `reset_ms`, a Unix
// time in milliseconds by the API's clock, first said at `noted_ms` by this one. `limit` is
// None when no response said it. `held_ms` is until when a spent answer holds it, by this
// clock, apart from the window: a count never lifts it, and it never stretches to the
// window's reset.
#[derive(Clone, Debug, PartialEq)]
struct Budget {
    api: Api,
    name: &'static str,
    limit: Option<u64>,
    remaining: u64,
    reset_ms: u64,
    noted_ms: u64,
    held_ms: u64,
}

impl Budget {
    // Until when its count is believed: its reset, and no more than an hour after it was
    // first said, as every budget resets within the hour. A wild header, or a clock behind
    // the API's, holds nothing longer. The windows are still ordered by `reset_ms` itself.
    fn ends(&self) -> u64 {
        self.reset_ms.min(self.noted_ms.saturating_add(WAIT_BOUNDS.1 * 1000))
    }
}

#[derive(Debug, Default)]
pub struct Budgets(Vec<Budget>);

// One API's budget as `listen`'s panel shows it (`Budgets::shown`).
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub api: &'static str,
    pub limit: Option<u64>,
    pub remaining: u64,
    pub resets: Duration,
    pub spent: bool,
}

impl Budgets {
    // A response's count of one budget. Responses to calls made at once land in any order,
    // and a budget only falls within its window, so a count for the window held is kept only
    // when lower, one for an earlier window never, and one for a later window replaces it.
    // A count no longer believed (`Budget::ends`) is replaced by any.
    pub fn note(&mut self, api: Api, name: &'static str, limit: Option<u64>, remaining: Option<u64>, reset_ms: Option<u64>, now_ms: u64) {
        let (Some(remaining), Some(reset_ms)) = (remaining, reset_ms) else { return };
        match self.0.iter_mut().find(|b| b.api == api && b.name == name) {
            Some(held) if held.ends() > now_ms && reset_ms < held.reset_ms => {}
            Some(held) if held.ends() > now_ms && reset_ms == held.reset_ms => {
                held.remaining = held.remaining.min(remaining);
                held.limit = limit.or(held.limit);
            }
            Some(held) => (held.limit, held.remaining, held.reset_ms, held.noted_ms) = (limit, remaining, reset_ms, now_ms),
            None => self.0.push(Budget { api, name, limit, remaining, reset_ms, noted_ms: now_ms, held_ms: 0 }),
        }
    }

    // A response that said the budgets `names` are spent: each is held for `wait` from
    // `now_ms`, the wait the API gave, and no longer, whatever its window's reset.
    pub fn spent(&mut self, api: Api, names: &[&'static str], wait: Duration, now_ms: u64) {
        let until_ms = now_ms.saturating_add(u64::try_from(wait.as_millis()).unwrap_or(u64::MAX));
        for &name in names {
            match self.0.iter_mut().find(|b| b.api == api && b.name == name) {
                Some(held) => held.held_ms = held.held_ms.max(until_ms),
                None => self.0.push(Budget { api, name, limit: None, remaining: 0, reset_ms: 0, noted_ms: now_ms, held_ms: until_ms }),
            }
        }
    }

    // Each API's scarcest budget still believed, a spent one first, for `listen`'s panel. A
    // spent API resets when the last of its spent budgets does, as `admit` waits for it.
    pub fn shown(&self, now_ms: u64) -> Vec<Shown> {
        [Api::GitHub, Api::Linear].into_iter().filter_map(|api| {
            let ours = self.0.iter().filter(|b| b.api == api && (b.ends() > now_ms || b.held_ms > now_ms));
            let spent = |b: &Budget| b.held_ms > now_ms || (b.ends() > now_ms && b.remaining == 0);
            let until = |b: &Budget| if b.held_ms > now_ms { b.held_ms.max(if b.remaining == 0 { b.ends() } else { 0 }) } else { b.ends() };
            // The share left, in thousandths: a budget whose limit is unsaid ranks by its count alone.
            let share = |b: &Budget| b.limit.filter(|&l| l > 0).map_or(b.remaining, |l| b.remaining.saturating_mul(1000) / l);
            let scarcest = ours.clone().min_by_key(|b| (!spent(b), share(b)))?;
            let spent_now = spent(scarcest);
            let until_ms = if spent_now { ours.filter(|b| spent(b)).map(until).max().unwrap_or(0) } else { until(scarcest) };
            Some(Shown { api: api.name(), limit: scarcest.limit, remaining: scarcest.remaining, resets: Duration::from_millis(until_ms.saturating_sub(now_ms)), spent: spent_now })
        }).collect()
    }

    // Whether a call that spends the budgets `names` may be made now: `Error::RateLimited`,
    // with the wait until the reset, when one is spent, or, for a read in the normal lane,
    // low. A budget past its reset says nothing, so the first call after it goes, and its
    // response says the next window's count.
    pub fn admit(&self, api: Api, names: &[&str], read: bool, lane: Lane, now_ms: u64) -> Result<(), Error> {
        let ours = self.0.iter().filter(|b| b.api == api && names.contains(&b.name));
        let held = ours.clone().map(|b| b.held_ms).filter(|&held| held > now_ms);
        let current = ours.filter(|b| b.ends() > now_ms);
        let spent = current.clone().filter(|b| b.remaining == 0).map(Budget::ends).chain(held).max();
        if let Some(reset_ms) = spent {
            let wait = until(reset_ms, now_ms);
            return Err(Error::RateLimited { message: format!("{}'s rate limit is spent: it resets in {} s", api.name(), wait.as_secs()), wait });
        }
        if !read || lane == Lane::Urgent {
            return Ok(());
        }
        let low = current.filter(|b| b.limit.is_some_and(|limit| is_low(limit, b.remaining))).max_by_key(|b| b.ends());
        match low {
            Some(budget) => {
                let wait = until(budget.ends(), now_ms);
                Err(Error::RateLimited { message: format!("{}'s {} budget is low, kept for urgent work: it resets in {} s", api.name(), budget.name, wait.as_secs()), wait })
            }
            None => Ok(()),
        }
    }
}

// The budgets of this process: `listen`'s, for as long as it runs, or a worker's command's.
// Static, not in runtime::Shared: every call the process makes, whatever command made it,
// reads its API's headers here, and a budget is the token's, which the process holds.
static BUDGETS: Mutex<Budgets> = Mutex::new(Budgets(Vec::new()));

fn now_ms() -> u64 {
    u64::try_from(time::now_ms()).unwrap_or(0)
}

fn budgets() -> std::sync::MutexGuard<'static, Budgets> {
    lock(&BUDGETS)
}

// The reset of each budget last logged as low, so each is logged once a window: static with
// the budgets.
static LOGGED_LOW: Mutex<Vec<(Api, String, u64)>> = Mutex::new(Vec::new());

// A budget that is low (`is_low`), or None when it isn't, when a count is missing, or when
// `logged` holds its window already.
fn low_line(api: Api, name: &str, limit: Option<u64>, remaining: Option<u64>, reset_ms: Option<u64>, now_ms: u64, logged: &mut Vec<(Api, String, u64)>) -> Option<Call> {
    let (Some(limit), Some(remaining), Some(reset_ms)) = (limit, remaining, reset_ms) else { return None };
    if !is_low(limit, remaining) || logged.iter().any(|(a, n, at)| *a == api && n == name && *at == reset_ms) {
        return None;
    }
    logged.retain(|(a, n, _)| *a != api || n != name);
    logged.push((api, name.to_string(), reset_ms));
    let api = match api {
        Api::GitHub => "github",
        Api::Linear => "linear",
    };
    Some(Call::BudgetLow { api, budget: name.into(), remaining, limit, resets_in: Duration::from_secs(reset_ms.saturating_sub(now_ms) / 1000) })
}

// Logs `name`'s budget once a window when it is low. Only `listen` logs it (trace.rs): a
// worker's command prints nothing of it.
pub fn log_low(api: Api, name: &str, limit: Option<u64>, remaining: Option<u64>, reset_ms: Option<u64>) {
    if !trace::enabled() {
        return;
    }
    if let Some(low) = low_line(api, name, limit, remaining, reset_ms, now_ms(), &mut lock(&LOGGED_LOW)) {
        trace::event(low);
    }
}

pub fn note(api: Api, name: &'static str, limit: Option<u64>, remaining: Option<u64>, reset_ms: Option<u64>) {
    log_low(api, name, limit, remaining, reset_ms);
    budgets().note(api, name, limit, remaining, reset_ms, now_ms());
}

pub fn shown(now_ms: u64) -> Vec<Shown> {
    budgets().shown(now_ms)
}

pub fn spent(api: Api, names: &[&'static str], wait: Duration) {
    budgets().spent(api, names, wait, now_ms());
}

// Before each call to `api`, in this thread's lane.
pub fn admit(api: Api, names: &[&str], read: bool) -> Result<(), Error> {
    budgets().admit(api, names, read, lane(), now_ms())
}

// What a failed call comes to, the one classification every caller acts on. Spent: its
// budget is, and the call is worth making again after the wait. Transient: worth making
// again, as a fresh read decides then. Refused: never, as bad input stays bad.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Failure {
    Spent(Duration),
    Transient,
    Refused,
}

impl Failure {
    pub fn of(error: &Error) -> Failure {
        match error {
            Error::RateLimited { wait, .. } => Failure::Spent(*wait),
            Error::Failed(_) | Error::Io { .. } | Error::Unavailable(_) | Error::Orca(_) | Error::Unsent(..) | Error::Unauthorized(_) | Error::Later(_) | Error::Waits(..) => Failure::Transient,
            Error::Usage(_) => Failure::Refused,
        }
    }
}

// The pauses before each retry of a call to an unavailable server: two retries, so a
// blip passes and an outage isn't waited on.
const UNAVAILABLE_PAUSES: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(5)];

// One try, then more after a failure that may pass: a spent budget's wait once, and an
// unavailable server's short pauses (`UNAVAILABLE_PAUSES`). None when it is still spent or
// unavailable after those, with the last error on stderr; any other error is the answer at
// once. Only for a call that is safe to make twice. `doing` names it on stderr, e.g.
// "turning on auto-merge".
pub fn repeat_safe_call<T>(api: Api, doing: &str, attempt: &mut dyn FnMut() -> Result<T, Error>, sleep: &mut dyn FnMut(Duration)) -> Result<Option<T>, Error> {
    let (mut waited, mut pauses) = (false, UNAVAILABLE_PAUSES.iter());
    loop {
        let error = match attempt() {
            Err(error) => error,
            done => return done.map(Some),
        };
        let pause = if let Some(reset) = error.rate_limited().filter(|_| !waited) {
            waited = true;
            eprintln!("{}'s rate limit is spent: {doing} again in {} s", api.name(), reset.as_secs());
            reset.max(Duration::from_secs(1))
        } else if error.unavailable() && let Some(pause) = pauses.next() {
            eprintln!("{} is unavailable: {doing} again in {} s", api.name(), pause.as_secs());
            *pause
        } else if error.rate_limited().is_some() || error.unavailable() {
            eprintln!("{error}");
            return Ok(None);
        } else {
            return Err(error);
        };
        sleep(pause);
    }
}

#[cfg(test)]
mod tests;
