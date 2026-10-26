// What each call to Linear's API costs the app's two hourly budgets, requests and
// complexity (docs/linear-api.md, "The budget"). `graphql` logs one line per call under the
// call's site, read from the response's headers. A line holds fixed names and numbers only:
// never Linear's text.

use std::panic::Location;
use std::time::Duration;

use crate::gateway;
use crate::time::iso;
use crate::trace::Cost;

// A worker's CLI prints this to stderr only when it is set, so its output stays as it was;
// `listen` logs without it.
pub(super) const LOG_ENV: &str = "TOLKSTER_LOG_LINEAR";

// Linear's rate-limit headers on a response, those it sent. Both resets are UTC epoch
// milliseconds; `complexity` is what this call cost, and each `_limit` its budget's size.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Limits {
    pub complexity: Option<u64>,
    pub requests_remaining: Option<u64>,
    pub complexity_remaining: Option<u64>,
    pub requests_reset: Option<u64>,
    pub complexity_reset: Option<u64>,
    pub requests_limit: Option<u64>,
    pub complexity_limit: Option<u64>,
}

impl Limits {
    pub(super) fn read(header: impl Fn(&str) -> Option<String>) -> Limits {
        let number = |name: &str| header(name).and_then(|v| v.trim().parse().ok());
        Limits {
            complexity: number("x-complexity"),
            requests_remaining: number("x-ratelimit-requests-remaining"),
            complexity_remaining: number("x-ratelimit-complexity-remaining"),
            requests_reset: number("x-ratelimit-requests-reset"),
            complexity_reset: number("x-ratelimit-complexity-reset"),
            requests_limit: number("x-ratelimit-requests-limit"),
            complexity_limit: number("x-ratelimit-complexity-limit"),
        }
    }

    // Until the budget that is spent resets, as of `now_ms`, a second over so a retry lands
    // after it, as the gateway bounds a wait. When neither says it is spent, the later
    // reset: holding for the shorter would only be refused again.
    pub(super) fn wait(&self, now_ms: u64) -> Duration {
        let spent = |remaining: Option<u64>, reset: Option<u64>| if remaining == Some(0) { reset } else { None };
        let reset = spent(self.requests_remaining, self.requests_reset)
            .into_iter()
            .chain(spent(self.complexity_remaining, self.complexity_reset))
            .max()
            .or_else(|| self.requests_reset.max(self.complexity_reset));
        reset.map_or_else(|| gateway::wait(None), |reset| gateway::until(reset, now_ms))
    }
}

// The call site: the module of the code that called `graphql` or `mutate` and the name of
// the GraphQL operation it sent (`unnamed` when it has none), both fixed in the code. `Location` is the caller's
// because `graphql` and `mutate` are `#[track_caller]`.
pub(super) fn site(query: &str, caller: &Location) -> String {
    let file = caller.file().replace('\\', "/");
    let module = file.split_once("src/").map_or(file.as_str(), |(_, rest)| rest);
    let module = module.strip_suffix(".rs").unwrap_or(module);
    format!("{module}.{}", super::operation(query))
}

// The cost of one response, or None when it carried no count at all.
pub(super) fn line(site: &str, limits: &Limits) -> Option<Cost> {
    let counts: Vec<(&'static str, String)> = [
        ("complexity", limits.complexity),
        ("requests_remaining", limits.requests_remaining),
        ("complexity_remaining", limits.complexity_remaining),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.map(|n| (name, n.to_string())))
    .chain([("requests_reset", limits.requests_reset), ("complexity_reset", limits.complexity_reset)].into_iter().filter_map(|(name, at)| {
        let at = i64::try_from(at?).ok().filter(|ms| *ms < 253_402_300_800_000)?;
        Some((name, iso(at)))
    }))
    .collect();
    (!counts.is_empty()).then(|| Cost { api: "linear", site: site.into(), resource: None, counts, log_env: LOG_ENV })
}

#[cfg(test)]
mod tests;
