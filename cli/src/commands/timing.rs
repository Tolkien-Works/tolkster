// `tolkster timing`: where workers' time goes (docs/timing.md), from the file `listen` keeps on
// its host, as a table or, with --json, the report's groups. Read-only.

use serde_json::{Value, json};
use ureq::Agent;

use super::logs::since_ms;
use crate::args::{Kind, parse};
use crate::cli::{NOTHING_TO_DO, print, print_json, read_if_there, timing_file};
use crate::error::Error;
use crate::time::{iso, now_ms};
use crate::timing::report::{GROUPS, Group, checks, report, runs};

pub const OPTIONS: &[(&str, Kind)] = &[("since", Kind::String), ("until", Kind::String), ("by", Kind::String), ("checks", Kind::Boolean), ("json", Kind::Boolean)];

// How far back the report reaches when --since isn't given.
const BACK_MS: i64 = 30 * 86_400_000;

pub fn run(_: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, OPTIONS, false)?;
    let now = now_ms();
    let time = |name: &str, default: i64| match parsed.non_empty(name) {
        Some(text) => since_ms(text, now).ok_or_else(|| Error::Usage(format!("--{name} takes a time with its zone, 2026-10-22T07:30:00Z, or a span back from now, 30m, 6h or 7d"))),
        None => Ok(default),
    };
    let to = time("until", now)?;
    let from = time("since", to - BACK_MS)?;
    if from >= to {
        return Err(Error::Usage("--since must be before --until, which is now when not given".into()));
    }
    let by = match parsed.non_empty("by") {
        Some(name) => GROUPS.iter().find(|(_, n)| *n == name).map(|(g, _)| *g).ok_or_else(|| Error::Usage("--by takes size, model or build".into()))?,
        None => Group::Size,
    };
    let file = timing_file()?;
    let text = read_if_there(&file)?.unwrap_or_default();
    let runs = runs(&text, from, to);
    if runs.is_empty() {
        eprintln!("no worker started from {} to {}", iso(from), iso(to));
        return Ok(NOTHING_TO_DO);
    }
    let (groups, rows) = if parsed.flag("checks") { (checks(&runs, by), CHECK_ROWS) } else { (report(&runs, by), ROWS) };
    let name = GROUPS.iter().find(|(g, _)| *g == by).map_or("", |(_, n)| n);
    if parsed.flag("json") {
        print_json(&json!({ "since": iso(from), "until": iso(to), "by": name, "groups": groups }))?;
    } else {
        print(&format!("from {} to {}, by {name}\n{}", iso(from), iso(to), table(&groups, rows)));
    }
    Ok(0)
}

#[derive(Clone, Copy)]
enum Unit {
    Count,
    Seconds,
    Share,
    Minutes,
}

type Row = (&'static str, &'static str, Unit);

const ROWS: &[Row] = &[
    ("issues", "/n", Unit::Count),
    ("start to PR", "/elapsed/startToPr", Unit::Seconds),
    ("PR to merge", "/elapsed/prToMerge", Unit::Seconds),
    ("start to merge", "/elapsed/startToMerge", Unit::Seconds),
    ("orienting", "/phases/orienting", Unit::Seconds),
    ("implementing", "/phases/implementing", Unit::Seconds),
    ("review and gate", "/phases/reviewAndGate", Unit::Seconds),
    ("  deep review", "/phases/deepReviewShare", Unit::Share),
    ("active", "/active", Unit::Seconds),
    ("owner wait", "/ownerWait", Unit::Seconds),
    ("model", "/shares/model", Unit::Share),
    ("checks", "/shares/check", Unit::Share),
    ("reads", "/shares/read", Unit::Share),
    ("edits", "/shares/edit", Unit::Share),
    ("git", "/shares/git", Unit::Share),
    ("bookkeeping", "/shares/bookkeeping", Unit::Share),
    ("sub-agents", "/shares/agent", Unit::Share),
    ("other", "/shares/other", Unit::Share),
    ("tool turns", "/toolTurns", Unit::Count),
    ("check runs", "/checkRuns", Unit::Count),
];

const CHECK_ROWS: &[Row] = &[
    ("issues", "/n", Unit::Count),
    ("check runs", "/checkRuns", Unit::Count),
    ("  cargo test", "/byKind/cargoTest", Unit::Count),
    ("  clippy", "/byKind/clippy", Unit::Count),
    ("  cargo check", "/byKind/cargoCheck", Unit::Count),
    ("  node test", "/byKind/nodeTest", Unit::Count),
    ("  simulator", "/byKind/simulator", Unit::Count),
    ("  typecheck", "/byKind/typecheck", Unit::Count),
    ("  gate", "/byKind/gate", Unit::Count),
    ("main session", "/by/main", Unit::Count),
    ("sub-agent", "/by/subAgent", Unit::Count),
    ("before review", "/byPhase/beforeReview", Unit::Count),
    ("review", "/byPhase/review", Unit::Count),
    ("after", "/byPhase/after", Unit::Count),
    ("failed", "/byOutcome/failed", Unit::Count),
    ("passed", "/byOutcome/passed", Unit::Count),
    ("unknown", "/byOutcome/unknown", Unit::Count),
    ("no edit since", "/byEdit/noEdit", Unit::Count),
    ("after an edit", "/byEdit/afterEdit", Unit::Count),
    ("fix after fail", "/byEdit/fixAfterFailure", Unit::Count),
    ("check minutes", "/minutes/total", Unit::Minutes),
    ("  cargo test", "/minutes/cargoTest", Unit::Minutes),
    ("  clippy", "/minutes/clippy", Unit::Minutes),
    ("  cargo check", "/minutes/cargoCheck", Unit::Minutes),
    ("  node test", "/minutes/nodeTest", Unit::Minutes),
    ("  simulator", "/minutes/simulator", Unit::Minutes),
    ("  typecheck", "/minutes/typecheck", Unit::Minutes),
    ("  gate", "/minutes/gate", Unit::Minutes),
];

fn cell(value: Option<&Value>, unit: Unit) -> String {
    let Some(value) = value.and_then(Value::as_f64) else { return "-".into() };
    match unit {
        Unit::Count => value.to_string(),
        Unit::Share => format!("{:.0}%", value * 100.0),
        Unit::Minutes => format!("{value:.1}m"),
        Unit::Seconds => {
            let seconds = value.round() as i64;
            if seconds < 60 {
                format!("{seconds}s")
            } else if seconds < 3600 {
                format!("{}m{:02}s", seconds / 60, seconds % 60)
            } else {
                format!("{}h{:02}m", seconds / 3600, seconds % 3600 / 60)
            }
        }
    }
}

// A row per figure, a column per group.
fn table(groups: &[Value], rows: &[Row]) -> String {
    let label = rows.iter().map(|(l, _, _)| l.len()).max().unwrap_or(0);
    let cells: Vec<Vec<String>> = groups.iter().map(|g| rows.iter().map(|(_, path, unit)| cell(g.pointer(path), *unit)).collect()).collect();
    let names: Vec<&str> = groups.iter().map(|g| g["group"].as_str().unwrap_or("")).collect();
    let widths: Vec<usize> = cells.iter().zip(&names).map(|(column, name)| column.iter().map(String::len).chain([name.len()]).max().unwrap_or(0)).collect();
    let mut lines = vec![format!("{:label$}{}", "", names.iter().zip(&widths).map(|(n, w)| format!("  {n:>w$}")).collect::<String>())];
    for (r, (name, _, _)) in rows.iter().enumerate() {
        lines.push(format!("{name:label$}{}", cells.iter().zip(&widths).map(|(column, w)| format!("  {:>w$}", column[r])).collect::<String>()));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests;
