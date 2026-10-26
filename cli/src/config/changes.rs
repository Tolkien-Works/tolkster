// What `listen` says of config.json as each listing re-reads it: the keys an edit changed, once,
// and a read that failed, once until it reads again or fails another way.

use serde_json::Value;

use super::Config;
use crate::error::Error;
use crate::trace::Work;

// The most keys one line names.
const NAMED: usize = 6;

// `worker` is read once, at startup, for the realtime connection, so it is the one key an edit
// doesn't reach until `listen` restarts.
const STARTUP_ONLY: &str = "worker";

pub struct Reread {
    applied: Value,
    // The failure last said, by its text, which no line carries.
    refused: Option<String>,
}

impl Reread {
    pub fn new(config: &Config, worker: Option<&str>) -> Reread {
        Reread { applied: snapshot(config, worker), refused: None }
    }

    // What a re-read came to: the line it earns, if any. A config that reads is the one in use
    // from now on, whatever it changed.
    pub fn after(&mut self, read: Result<(&Config, Option<&str>), &Error>) -> Option<Work> {
        match read {
            Ok((config, worker)) => {
                self.refused = None;
                let now = snapshot(config, worker);
                let mut changes = Vec::new();
                diff(&self.applied, &now, "", &mut changes);
                self.applied = now;
                (!changes.is_empty()).then_some(Work::ConfigChanged { changes: named(changes) })
            }
            Err(error) => {
                let text = error.to_string();
                (self.refused.replace(text.clone()) != Some(text)).then(|| Work::ConfigKept(error.clone()))
            }
        }
    }
}

// The settings as `tolkster config` prints them, with `worker` beside them.
fn snapshot(config: &Config, worker: Option<&str>) -> Value {
    let mut settings = serde_json::to_value(config).unwrap_or_default();
    if let Some(settings) = settings.as_object_mut() {
        settings.insert(STARTUP_ONLY.into(), worker.map_or(Value::Null, Value::from));
    }
    settings
}

// Each leaf `after` differs in, by its path. An object that appears or goes is one change, not
// every default it carries.
fn diff(before: &Value, after: &Value, path: &str, changes: &mut Vec<String>) {
    if before == after {
        return;
    }
    if let (Value::Object(before), Value::Object(after)) = (before, after) {
        for key in before.keys().chain(after.keys().filter(|key| !before.contains_key(*key))) {
            let at = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
            diff(before.get(key).unwrap_or(&Value::Null), after.get(key).unwrap_or(&Value::Null), &at, changes);
        }
        return;
    }
    changes.push(describe(path, before, after));
}

// A number or a flag by its value; any other by its path alone, as a string could be a secret.
fn describe(path: &str, before: &Value, after: &Value) -> String {
    let late = if path == STARTUP_ONLY { ", read at startup only" } else { "" };
    match (before, after) {
        (_, Value::Bool(_) | Value::Number(_)) => format!("{path} = {after}"),
        (_, Value::Null) => format!("{path} removed{late}"),
        (Value::Null, _) => format!("{path} added{late}"),
        _ => format!("{path} changed{late}"),
    }
}

fn named(mut changes: Vec<String>) -> Vec<String> {
    if changes.len() > NAMED {
        let more = changes.len() - NAMED;
        changes.truncate(NAMED);
        changes.push(format!("{more} more"));
    }
    changes
}

#[cfg(test)]
mod tests;
