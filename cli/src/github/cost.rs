// What each call to GitHub's API costs the installation's budget (docs/github-app.md, "The
// budget"). Each public function that calls GitHub names itself once (`site`), and
// `app::respond` logs one line per call under that name. A line holds fixed names and
// numbers only: never a PR's, a repo's or a person's text.

use std::cell::Cell;

use serde::Deserialize;
use serde_json::Value;

use super::app::Limits;
use crate::trace::Cost;

// A worker's CLI prints this to stderr only when it is set, so its output stays as it was;
// `listen` logs without it.
pub(super) const LOG_ENV: &str = "TOLKSTER_LOG_GITHUB";

// Ambient: named by a function at its top and read by the GitHub call it makes, frames down.
thread_local! {
    static SITE: Cell<&'static str> = const { Cell::new("unnamed") };
}

// The call site's name until it is dropped, then the one before it: a function that calls
// another that names itself is logged under the inner name.
pub(super) struct Site(&'static str);

pub(super) fn site(name: &'static str) -> Site {
    Site(SITE.replace(name))
}

impl Drop for Site {
    fn drop(&mut self) {
        SITE.set(self.0);
    }
}

pub(super) fn current() -> &'static str {
    SITE.get()
}

// A query with `rateLimit { cost remaining resetAt }` beside its data, which GitHub answers
// at no cost. Only a query: `rateLimit` is not a field of a mutation, whose call is logged
// from the response's headers instead.
pub(super) fn with_rate_limit(query: &str) -> String {
    let query = query.trim_end();
    match query.strip_suffix('}') {
        Some(head) if query.starts_with("query") => format!("{} rateLimit {{ cost remaining resetAt }} }}", head.trim_end()),
        _ => query.into(),
    }
}

// `data.rateLimit`, which a query asks for beside its data (`with_rate_limit`). Read from any
// answer: one without it, or with it in another shape, has none.
#[derive(Deserialize)]
struct Rated {
    data: Option<RatedData>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RatedData {
    rate_limit: Option<RateLimit>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimit {
    cost: u64,
    remaining: u64,
    reset_at: Option<String>,
}

// The line for one response, or None when it carried no count at all. A GraphQL query's
// own `rateLimit` gives its exact cost; any other response gives the budget's `used` and
// `remaining` as of the call.
pub(super) fn line(site: &str, limits: &Limits, body: &Value) -> Option<Cost> {
    let resource = limits.resource.as_deref();
    let cost = |resource: &str, counts| Cost { api: "github", site: site.into(), resource: Some(resource.into()), counts, log_env: LOG_ENV };
    if let Some(rate) = Rated::deserialize(body).ok().and_then(|rated| rated.data?.rate_limit) {
        let reset = rate.reset_at.filter(|at| plain_time(at)).map(|at| ("resetAt", at));
        return Some(cost(resource.unwrap_or("graphql"), [("cost", rate.cost.to_string()), ("remaining", rate.remaining.to_string())].into_iter().chain(reset).collect()));
    }
    let counts: Vec<(&'static str, String)> = [("used", limits.used), ("remaining", limits.remaining)].into_iter().filter_map(|(name, n)| n.map(|n| (name, n.to_string()))).collect();
    (!counts.is_empty()).then(|| cost(resource.unwrap_or("api"), counts))
}

// It reaches a log line, so only the digits and marks of an ISO time.
fn plain_time(at: &str) -> bool {
    at.len() <= 30 && at.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'-' | b':' | b'T' | b'Z' | b'.'))
}

#[cfg(test)]
mod tests;
