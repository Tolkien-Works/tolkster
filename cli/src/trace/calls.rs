use std::fmt;
use std::time::Duration;

use super::event::{Line, Parts, Show};
use super::seconds;
use crate::error::Error;

// A call to Linear, GitHub, the Durable Object, Orca or git. `site` is the call site's fixed
// name, e.g. `linear commentCreate` or `orca worktree create`, never a path, a branch or an ID.
#[derive(Debug, PartialEq)]
pub enum Call {
    // A write, with the IDs it named: shown with `--verbose`, and a failed one always.
    Wrote { site: String, detail: String, failed: Option<Error> },
    // A git write. Its error can carry a remote URL, and with it a credential, so a failure
    // says only that it failed.
    Git { site: String, detail: String, ok: bool },
    // One that took `SLOW` or longer, so a slow job shows where its time went.
    Slow { site: String, took: Duration },
    // A budget running low, once a window.
    BudgetLow { api: &'static str, budget: String, remaining: u64, limit: u64, resets_in: Duration },
    Cost(Cost),
    // A job as it came in.
    Inbound { job: String },
}

// A call's cost: in `listen`'s terminal with `--verbose`, else on stderr when `log_env` is
// set, so a worker's output stays as it was. `counts` are numbers and ISO times.
#[derive(Debug, PartialEq)]
pub struct Cost {
    pub api: &'static str,
    pub site: String,
    pub resource: Option<String>,
    pub counts: Vec<(&'static str, String)>,
    pub log_env: &'static str,
}

impl Cost {
    fn text(&self) -> String {
        let resource = self.resource.as_ref().map(|r| format!(" {r}")).unwrap_or_default();
        let counts: String = self.counts.iter().map(|(name, value)| format!(" {name}={value}")).collect();
        format!("{}{resource}{counts}", self.site)
    }
}

impl fmt::Display for Cost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.api, self.text())
    }
}

impl Call {
    pub(super) fn line(&self) -> Line {
        match self {
            Call::Wrote { site, detail, failed: None } => Line::routine(Parts::of(site.clone()).opaque(detail)).show(Show::Verbose),
            Call::Wrote { site, detail, failed: Some(error) } => Line::error(Parts::of(site.clone()).opaque(detail).text(": failed").error(error)),
            Call::Git { site, detail, ok: true } => Line::routine(Parts::of(site.clone()).opaque(detail)).show(Show::Verbose),
            Call::Git { site, detail, ok: false } => Line::error(Parts::of(site.clone()).opaque(detail).text(": failed")),
            Call::Slow { site, took } => Line::warning(Parts::of(format!("slow: {site} took {}", seconds(*took)))),
            Call::BudgetLow { api, budget, remaining, limit, resets_in } => {
                Line::warning(Parts::of(format!("{budget} budget low: {remaining} of {limit} left, resets in {} s", resets_in.as_secs()))).area(api)
            }
            Call::Cost(cost) => Line::routine(Parts::of(cost.text())).area(cost.api).show(Show::Verbose),
            Call::Inbound { job } => Line::routine(Parts::of("in").opaque(job)).show(Show::Verbose),
        }
    }
}
