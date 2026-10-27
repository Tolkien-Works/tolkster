// `tolkster timing`'s report (docs/timing.md) of the file `listen` keeps: each issue's first
// run, from its first start to the next, and per-issue medians, never means, for each group of
// issues. A wait on the owner, a gap before a session resumed (a plan-first reply, a parked
// terminal answered), is taken out of the worker's elapsed time and reported apart.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::{By, CheckKind, Kind, Summary};
use crate::estimates::Size;
use crate::time::instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Size,
    Model,
    Build,
}

pub const GROUPS: [(Group, &str); 3] = [(Group::Size, "size"), (Group::Model, "model"), (Group::Build, "build")];

const CHECKS: [(CheckKind, &str); 7] = [(CheckKind::CargoTest, "cargoTest"), (CheckKind::Clippy, "clippy"), (CheckKind::CargoCheck, "cargoCheck"), (CheckKind::NodeTest, "nodeTest"), (CheckKind::Simulator, "simulator"), (CheckKind::Typecheck, "typecheck"), (CheckKind::Gate, "gate")];

// An issue's first run: its first start line's size and rung, the sessions launched from its
// first start to the next, and the first merge before that, as `until` saw them.
pub struct Run {
    start: i64,
    until: i64,
    size: Option<Size>,
    rung: Option<String>,
    sessions: Vec<Summary>,
    merged: Option<i64>,
}

impl Run {
    fn group(&self, by: Group) -> (u8, String) {
        let none = || (u8::MAX, "none".to_string());
        match by {
            Group::Size => self.size.map_or_else(none, |s| (s.0, s.name().to_string())),
            Group::Model => self.rung.clone().map_or_else(none, |r| (0, r)),
            Group::Build => self.sessions.first().and_then(|s| s.build.clone()).map_or_else(none, |b| (0, crate::plain::short_sha(&b).to_string())),
        }
    }

    // The sessions a start or resume launched: the run's own line, without its rounds.
    fn main_line(&self) -> impl Iterator<Item = &Summary> {
        self.sessions.iter().filter(|s| matches!(s.kind, Kind::Start | Kind::Resume))
    }

    // From the last end of any session before each resume, a round's too, to its launch, in ms.
    fn owner_waits(&self) -> Vec<(i64, i64)> {
        let resumes = self.sessions.iter().filter(|s| s.kind == Kind::Resume).filter_map(|s| instant(&s.launched));
        resumes.filter_map(|to| Some((self.sessions.iter().filter(|s| instant(&s.launched).is_some_and(|l| l < to)).filter_map(ended).max()?, to))).filter(|(from, to)| to > from).collect()
    }

    // The earliest of a moment the main line's sessions give as an offset, before `until`.
    fn first(&self, offset: impl Fn(&Summary) -> Option<u64>) -> Option<i64> {
        self.main_line().filter_map(|s| Some(began(s)?.saturating_add(ms(offset(s)?)))).filter(|at| *at < self.until).min()
    }

    // The gate's or the review's start, whichever came first.
    fn review(&self) -> Option<i64> {
        [self.first(|s| s.offsets.first_gate), self.first(|s| s.offsets.review_start)].into_iter().flatten().min()
    }

    // Seconds from `from` to `to`, less the owner's waits between.
    fn elapsed(&self, from: Option<i64>, to: Option<i64>) -> Option<f64> {
        let (from, to) = (from?, to?);
        let waited = self.owner_waits().iter().fold(0i64, |sum, (a, b)| sum.saturating_add((to.min(*b) - from.max(*a)).max(0)));
        (to >= from).then(|| (to - from).saturating_sub(waited) as f64 / 1000.0)
    }
}

fn began(summary: &Summary) -> Option<i64> {
    instant(&summary.at)
}

fn ended(summary: &Summary) -> Option<i64> {
    Some(began(summary)?.saturating_add(ms(summary.wall)))
}

fn ms(seconds: u64) -> i64 {
    i64::try_from(seconds).unwrap_or(i64::MAX).saturating_mul(1000)
}

fn total(values: impl Iterator<Item = u64>) -> u64 {
    values.fold(0, u64::saturating_add)
}

type ByIssue<T> = BTreeMap<String, Vec<(i64, T)>>;

// Each issue's first run that started from `since_ms` to before `until_ms` and has a session
// read, from the file's text. A line that doesn't read or isn't of its plain shape, or is at or
// after `until_ms`, is passed over.
pub fn runs(text: &str, since_ms: i64, until_ms: i64) -> Vec<Run> {
    let (mut starts, mut merges, mut sessions): (ByIssue<Value>, ByIssue<()>, ByIssue<Summary>) = Default::default();
    for line in text.lines().filter_map(|l| serde_json::from_str::<Map<String, Value>>(l).ok()) {
        let text_of = |key: &str| line.get(key).and_then(Value::as_str).map(String::from);
        let (Some(issue), Some(kind)) = (text_of("issueId"), text_of("line")) else { continue };
        let at = text_of("at").as_deref().and_then(instant);
        match (kind.as_str(), at) {
            ("start", Some(at)) if at < until_ms => starts.entry(issue).or_default().push((at, Value::Object(line))),
            ("merge", Some(at)) if at < until_ms => merges.entry(issue).or_default().push((at, ())),
            ("session", _) => {
                let mut fields = line.clone();
                for key in ["line", "key", "issueId"] {
                    fields.remove(key);
                }
                let Some(summary) = serde_json::from_value::<Summary>(Value::Object(fields)).ok().filter(Summary::plain) else { continue };
                if let Some(launched) = instant(&summary.launched).filter(|l| *l < until_ms) {
                    sessions.entry(issue).or_default().push((launched, summary));
                }
            }
            _ => {}
        }
    }
    let mut runs = Vec::new();
    for (issue, mut held) in sessions {
        held.sort_by_key(|(launched, _)| *launched);
        let mut started = starts.remove(&issue).unwrap_or_default();
        started.sort_by_key(|(at, _)| *at);
        // A start line is written as its worker's terminal opens, and the worker stamps its
        // launch after that, on its own host's clock: a run ends at the next start line, else,
        // with none, at the next start launch.
        let mut launches: Vec<i64> = held.iter().filter(|(_, s)| s.kind == Kind::Start).map(|(l, _)| *l).collect();
        launches.dedup();
        let Some(from) = launches.first().copied().into_iter().chain(started.first().map(|(at, _)| *at)).min() else { continue };
        let next = if started.is_empty() { launches.get(1) } else { started.get(1).map(|(at, _)| at) }.copied().unwrap_or(i64::MAX);
        let held: Vec<(i64, Summary)> = held.into_iter().filter(|(l, _)| (from..next).contains(l)).collect();
        let first_start = held.iter().find(|(_, s)| s.kind == Kind::Start).map(|(l, _)| *l);
        let Some(start) = first_start.into_iter().chain(started.first().map(|(at, _)| *at)).min().filter(|_| !held.is_empty()) else { continue };
        if !(since_ms..until_ms).contains(&start) {
            continue;
        }
        let line = started.first().map(|(_, line)| line);
        let named = |key: &str| line.and_then(|l| l.get(key)).and_then(Value::as_str);
        runs.push(Run {
            start,
            until: until_ms,
            size: named("size").and_then(Size::parse),
            rung: named("rung").map(String::from),
            sessions: held.into_iter().map(|(_, s)| s).collect(),
            merged: merges.remove(&issue).unwrap_or_default().into_iter().map(|(at, _)| at).filter(|at| *at >= start && *at < next).min(),
        });
    }
    runs
}

// The median of what is known, rounded to `places`; null when nothing is.
fn median(values: impl IntoIterator<Item = Option<f64>>, places: i32) -> Value {
    let mut known: Vec<f64> = values.into_iter().flatten().collect();
    if known.is_empty() {
        return Value::Null;
    }
    known.sort_by(f64::total_cmp);
    let mid = known.len() / 2;
    let value = if known.len().is_multiple_of(2) { (known[mid - 1] + known[mid]) / 2.0 } else { known[mid] };
    let scale = 10f64.powi(places);
    json!((value * scale).round() / scale)
}

fn grouped(runs: &[Run], by: Group) -> Vec<(String, Vec<&Run>)> {
    let mut groups: BTreeMap<(u8, String), Vec<&Run>> = BTreeMap::new();
    for run in runs {
        groups.entry(run.group(by)).or_default().push(run);
    }
    groups.into_iter().map(|((_, name), runs)| (name, runs)).collect()
}

// Per-issue medians for each group: elapsed time, phases, shares of active time, turns.
pub fn report(runs: &[Run], by: Group) -> Vec<Value> {
    grouped(runs, by)
        .into_iter()
        .map(|(name, runs)| {
            let each = |f: &dyn Fn(&Run) -> Option<f64>| median(runs.iter().map(|r| f(r)), 0);
            let sum = |run: &Run, f: &dyn Fn(&Summary) -> u64| total(run.sessions.iter().map(f));
            let active = |run: &Run| Some(sum(run, &|s| s.active) as f64).filter(|a| *a > 0.0);
            let share = |f: &dyn Fn(&Summary) -> u64| median(runs.iter().map(|r| active(r).map(|a| sum(r, f) as f64 / a)), 3);
            let pr = |r: &Run| r.first(|s| s.offsets.pr);
            json!({
                "group": name,
                "n": runs.len(),
                "elapsed": {
                    "startToPr": each(&|r| r.elapsed(Some(r.start), pr(r))),
                    "prToMerge": each(&|r| r.elapsed(pr(r), r.merged)),
                    "startToMerge": each(&|r| r.elapsed(Some(r.start), r.merged)),
                },
                "phases": {
                    "orienting": each(&|r| r.elapsed(Some(r.start), r.first(|s| s.offsets.first_edit))),
                    "implementing": each(&|r| r.elapsed(r.first(|s| s.offsets.first_edit), r.review())),
                    "reviewAndGate": each(&|r| r.elapsed(r.review(), pr(r))),
                    "deepReviewShare": median(runs.iter().map(|r| deep_share(r, pr(r))), 3),
                },
                "active": each(&active),
                "ownerWait": each(&|r| Some(r.owner_waits().iter().map(|(a, b)| b - a).sum::<i64>() as f64 / 1000.0)),
                "shares": {
                    "model": share(&|s| s.model),
                    "check": share(&|s| s.tools.check),
                    "read": share(&|s| s.tools.read),
                    "edit": share(&|s| s.tools.edit),
                    "git": share(&|s| s.tools.git),
                    "bookkeeping": share(&|s| s.tools.bookkeeping),
                    "agent": share(&|s| s.tools.agent),
                    "other": share(&|s| s.tools.other),
                },
                "toolTurns": each(&|r| Some(sum(r, &|s| s.tool_turns) as f64)),
                "checkRuns": each(&|r| Some(ran(r).len() as f64)),
            })
        })
        .collect()
}

// The deep review's part of the review and gate: its span in the first session that has one.
fn deep_share(run: &Run, pr: Option<i64>) -> Option<f64> {
    let span = run.main_line().find_map(|s| Some(s.offsets.review_end?.checked_sub(s.offsets.review_start?)? as f64))?;
    let whole = run.elapsed(run.review(), pr).filter(|w| *w > 0.0)?;
    Some((span / whole).min(1.0))
}

// Each check run of the run, in order, with its phase and what came before it.
struct Ran {
    kind: CheckKind,
    by: By,
    failed: Option<bool>,
    phase: &'static str,
    edit: &'static str,
    seconds: u64,
}

fn ran(run: &Run) -> Vec<Ran> {
    let (review, pr) = (run.review(), run.first(|s| s.offsets.pr));
    let mut previous_failed = false;
    let mut all = Vec::new();
    for session in &run.sessions {
        let begun = began(session);
        for check in &session.checks {
            let at = begun.map(|b| b.saturating_add(ms(check.at)));
            if at.is_some_and(|at| at >= run.until) {
                continue;
            }
            let past = |point: Option<i64>| point.zip(at).is_some_and(|(point, at)| at >= point);
            let phase = if matches!(session.kind, Kind::Round | Kind::Rebase) || past(pr) {
                "after"
            } else if past(review) {
                "review"
            } else {
                "beforeReview"
            };
            let edit = match (check.edited, previous_failed) {
                (false, _) => "noEdit",
                (true, true) => "fixAfterFailure",
                (true, false) => "afterEdit",
            };
            previous_failed = check.failed == Some(true);
            all.push(Ran { kind: check.kind, by: check.by, failed: check.failed, phase, edit, seconds: check.seconds });
        }
    }
    all
}

// For each group, the median check runs per issue and how they split, and check minutes per
// issue by kind.
pub fn checks(runs: &[Run], by: Group) -> Vec<Value> {
    grouped(runs, by)
        .into_iter()
        .map(|(name, runs)| {
            let ran: Vec<Vec<Ran>> = runs.iter().map(|r| ran(r)).collect();
            let count = |is: &dyn Fn(&Ran) -> bool| median(ran.iter().map(|runs| Some(runs.iter().filter(|r| is(r)).count() as f64)), 1);
            let minutes = |is: &dyn Fn(&Ran) -> bool| median(ran.iter().map(|runs| Some(runs.iter().filter(|r| is(r)).map(|r| r.seconds).fold(0, u64::saturating_add) as f64 / 60.0)), 1);
            let split = |names: &[&'static str], of: fn(&Ran) -> &'static str| -> Value { names.iter().map(|name| (name.to_string(), count(&|r| of(r) == *name))).collect::<Map<_, _>>().into() };
            let by_kind: Map<String, Value> = CHECKS.iter().map(|(kind, name)| (name.to_string(), count(&|r| r.kind == *kind))).collect();
            let mut by_minutes: Map<String, Value> = CHECKS.iter().map(|(kind, name)| (name.to_string(), minutes(&|r| r.kind == *kind))).collect();
            by_minutes.insert("total".into(), minutes(&|_| true));
            json!({
                "group": name,
                "n": runs.len(),
                "checkRuns": count(&|_| true),
                "byKind": by_kind,
                "by": { "main": count(&|r| r.by == By::Main), "subAgent": count(&|r| r.by == By::SubAgent) },
                "byPhase": split(&["beforeReview", "review", "after"], |r| r.phase),
                "byOutcome": { "failed": count(&|r| r.failed == Some(true)), "passed": count(&|r| r.failed == Some(false)), "unknown": count(&|r| r.failed.is_none()) },
                "byEdit": split(&["noEdit", "afterEdit", "fixAfterFailure"], |r| r.edit),
                "minutes": by_minutes,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
