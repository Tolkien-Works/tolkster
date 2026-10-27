// `tolkster logs`: the Worker's own log lines for a session or an issue, from Workers
// Observability (cloudflare.rs), oldest first, as JSON (docs/observability.md). A line's
// fields are what the Worker logged, IDs, enums, counts and times, and only its scalar ones
// are printed. Read-only.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};
use ureq::Agent;

use crate::args::{Kind, Parsed, parse};
use crate::cli::print_json;
use crate::cloudflare::{Filter, LIMIT, events, query};
use crate::config::files;
use crate::error::Error;
use crate::sessions::lookup::issue_ref;
use crate::linear::with_token;
use crate::plain::{Id, IssueKey};
use crate::time::{instant, iso, now_ms};

pub const OPTIONS: &[(&str, Kind)] = &[("session", Kind::String), ("issue", Kind::String), ("since", Kind::String), ("until", Kind::String)];

// How long Workers Logs keeps a line, and so the default reach back.
const KEPT_MS: i64 = 7 * DAY_MS;
const DAY_MS: i64 = 24 * 60 * 60_000;
// The issue's sessions asked for in one query; past it, `more`.
const SESSIONS: usize = 50;

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, OPTIONS, false)?;
    let session = parsed.non_empty("session").map(Id::check).transpose()?;
    let issue = parsed.non_empty("issue").map(IssueKey::check).transpose()?.map(|key| key.upper());
    if session.is_none() && issue.is_none() {
        return Err(Error::Usage("logs takes --session <id>, --issue <TW-123> or both".into()));
    }
    let now = now_ms();
    let (from, to) = window(&parsed, now, KEPT_MS)?;
    let files = files()?;
    let missing = |key: &str| Error::Usage(format!("no \"{key}\" in credentials.json: docs/observability.md says how to make it"));
    // Fixed text, not the value: a token pasted under the wrong key would be printed.
    let account = Id::parse(&files.secret("cloudflareAccountId").ok_or_else(|| missing("cloudflareAccountId"))?).ok_or_else(|| Error::Usage("\"cloudflareAccountId\" in credentials.json isn't an account ID".into()))?;
    let token = files.secret("cloudflareLogsToken").ok_or_else(|| missing("cloudflareLogsToken"))?;
    let issue_id = match &issue {
        Some(key) => Some(issue_uuid(agent, key)?),
        None => None,
    };
    let mut asked = Vec::new();
    if let Some(session) = &session {
        asked.push(Filter { key: "sessionId", value: session.as_str() });
    }
    if let (Some(key), Some(id)) = (&issue, &issue_id) {
        asked.push(Filter { key: "issue", value: key.as_str() });
        asked.push(Filter { key: "issueId", value: id.as_str() });
    }
    let mut found = events(agent, &account, &token, &query(&asked, from, to))?;
    let mut more = found.len() >= LIMIT;
    // An issue's lines name its sessions, whose own lines (`activity_posted`, `relay_ingested`)
    // don't name the issue.
    let sessions = sessions_named(&found, session.as_ref().map(Id::as_str));
    if issue.is_some() && !sessions.is_empty() {
        let filters: Vec<Filter> = sessions.iter().take(SESSIONS).map(|value| Filter { key: "sessionId", value }).collect();
        let next = events(agent, &account, &token, &query(&filters, from, to))?;
        more |= next.len() >= LIMIT || sessions.len() > SESSIONS;
        found.extend(next);
    }
    print_json(&json!({ "lines": lines(found), "more": more }))?;
    Ok(0)
}

// The issue's Linear ID, which the Durable Object's lines carry.
fn issue_uuid(agent: &Agent, key: &IssueKey) -> Result<Id, Error> {
    Id::check(&with_token(agent, |token| issue_ref(agent, token, key.as_str()))?.id)
}

// The `--since` and `--until` asked for, from `back_ms` before `--until` to now when not given.
pub fn window(parsed: &Parsed, now: i64, back_ms: i64) -> Result<(i64, i64), Error> {
    let time = |name: &str, default: i64| match parsed.non_empty(name) {
        Some(text) => since_ms(text, now).ok_or_else(|| Error::Usage(format!("--{name} takes a time with its zone, 2026-10-22T07:30:00Z, or a span back from now, 30m, 6h or 2d"))),
        None => Ok(default),
    };
    let to = time("until", now)?;
    let from = time("since", to - back_ms)?;
    if from >= to {
        return Err(Error::Usage("--since must be before --until, which is now when not given".into()));
    }
    Ok((from, to))
}

// `--since` and `--until`: an instant with its zone, or `<n>m`, `<n>h` or `<n>d` back from `now`.
pub fn since_ms(since: &str, now: i64) -> Option<i64> {
    if let Some(at) = instant(since) {
        return Some(at);
    }
    let unit = match since.chars().last()? {
        'm' => 60_000,
        'h' => 60 * 60_000,
        'd' => DAY_MS,
        _ => return None,
    };
    let count: i64 = since[..since.len() - 1].parse().ok().filter(|n| *n > 0)?;
    Some(now - count.checked_mul(unit)?)
}

// The plain session IDs `events` name, but `known`'s.
pub fn sessions_named(events: &[Value], known: Option<&str>) -> BTreeSet<String> {
    events.iter().filter_map(|e| e["source"]["sessionId"].as_str()).filter(|id| Some(*id) != known && Id::parse(id).is_some()).map(String::from).collect()
}

// Each event once, by Cloudflare's ID, oldest first: `at` and the line's scalar fields.
pub fn lines(events: Vec<Value>) -> Vec<Value> {
    let mut seen = BTreeSet::new();
    let mut kept: Vec<(i64, Value)> = events
        .into_iter()
        .filter(|e| e["$metadata"]["id"].as_str().is_none_or(|id| seen.insert(id.to_string())))
        .filter_map(|e| {
            let at = e["timestamp"].as_i64()?;
            let Value::Object(source) = &e["source"] else { return None };
            let mut line: Map<String, Value> = source.iter().filter(|(_, v)| !v.is_object() && !v.is_array()).map(|(k, v)| (k.clone(), v.clone())).collect();
            // Cloudflare's time, over a line's own `at` (a report's).
            line.insert("at".into(), Value::String(iso(at)));
            Some((at, Value::Object(line)))
        })
        .collect();
    kept.sort_by_key(|(at, _)| *at);
    kept.into_iter().map(|(_, line)| line).collect()
}

#[cfg(test)]
mod tests;
