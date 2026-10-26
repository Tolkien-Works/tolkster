// `tolkster repo list|map|unmap`: the `repos` entries in config.json (docs/setup.md, step 6),
// the owner's alone. `map` checks its repo against Orca and its teams and projects against
// Linear before writing, and every write goes through `merge_json_file`, its entries made from
// the file as it is then: the rest of the file stays as it was, and the result must pass the
// check `tolkster config` reads `repos` with (exit 2), so no project or team ends up in two
// entries. A write that changes nothing exits 3.

use std::path::Path;

use serde_json::{Map, Value, json};
use ureq::Agent;

use crate::args::{Kind, parse};
use crate::cli::{NOTHING_TO_DO, config_file, conversation_session, home_dir, merge_json_file, print_json};
use crate::coauthor;
use crate::config::{self, files, owner};
use crate::error::Error;
use crate::exec::run_command;
use crate::sessions::workspace::{KeyedTeam, teams_with_projects};
use crate::orca::{find_repo, repos};
use crate::config::repo_map::RepoMap;

pub const MAP_OPTIONS: &[(&str, Kind)] = &[("project", Kind::List), ("team", Kind::List), ("reviewers", Kind::String), ("max-rounds", Kind::String)];
pub const UNMAP_OPTIONS: &[(&str, Kind)] = &[("project", Kind::List), ("team", Kind::List), ("reviewers", Kind::Boolean), ("max-rounds", Kind::Boolean)];

// What `map` adds to a repo's entry, or `unmap` takes out of it: for `unmap`, `reviewers` and
// `max_rounds` are Some to remove them, and nothing at all removes the whole entry.
// `known` is each project's ID and name as Linear has them, so a project an entry names one
// way and the change the other is still the same project.
#[derive(Clone, Debug, Default)]
pub(crate) struct Change {
    pub projects: Vec<String>,
    pub teams: Vec<String>,
    pub reviewers: Option<Vec<String>>,
    pub max_rounds: Option<i64>,
    pub known: Vec<Project>,
}

// A project's ID and name.
pub(crate) type Project = (String, String);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Edit {
    Map,
    Unmap,
}

// Each entry as `tolkster config` prints it, and whether Orca has its repo, once.
pub fn list(rest: &[String]) -> Result<u8, Error> {
    parse(rest, &[], false)?;
    let files = files()?;
    let config = config::config(&files, owner(&files)?, &home_dir()?)?;
    let orca = repos(&mut run_command, None)?;
    let entries = config.repos.0.iter().map(|entry| {
        let mut row = serde_json::to_value(entry).unwrap_or_default();
        row["repo"] = json!(entry.repo);
        row["inOrca"] = json!(find_repo(&orca, Some(&entry.repo)).is_some());
        row
    });
    print_json(&entries.collect::<Vec<_>>())?;
    Ok(0)
}

pub fn map(agent: &Agent, rest: &[String]) -> Result<u8, Error> {
    let parsed = parse(rest, MAP_OPTIONS, true)?;
    let [repo] = parsed.positionals.as_slice() else { return Err(Error::Usage("repo map needs one repo, by its Orca display name".into())) };
    let reviewers = parsed.string("reviewers").map(reviewers).transpose()?;
    let max_rounds = parsed.string("max-rounds").map(max_rounds).transpose()?;
    let mut change = Change { projects: trimmed(parsed.list("project")), teams: trimmed(parsed.list("team")), reviewers, max_rounds, known: vec![] };
    if change.projects.is_empty() && change.teams.is_empty() && change.reviewers.is_none() && change.max_rounds.is_none() {
        return Err(Error::Usage("repo map needs --project, --team, --reviewers or --max-rounds".into()));
    }
    refuse_own_agent()?;
    let (_, repo) = find_repo(&repos(&mut run_command, None)?, Some(repo)).ok_or_else(|| Error::Failed(format!("Orca has no repo, or more than one, with the display name {repo}")))?;
    if !change.projects.is_empty() || !change.teams.is_empty() {
        let filter = match change.teams.as_slice() {
            [] => json!({}),
            teams => json!({ "or": teams.iter().map(|key| json!({ "key": { "eqIgnoreCase": key } })).collect::<Vec<_>>() }),
        };
        (change.teams, change.known) = checked(&teams_with_projects(agent, filter)?, &change)?;
    }
    write(&config_file()?, &repo, Edit::Map, &change)
}

pub fn unmap(rest: &[String]) -> Result<u8, Error> {
    let parsed = parse(rest, UNMAP_OPTIONS, true)?;
    let [repo] = parsed.positionals.as_slice() else { return Err(Error::Usage("repo unmap needs one repo, as its entry names it".into())) };
    let change = Change {
        projects: trimmed(parsed.list("project")),
        teams: trimmed(parsed.list("team")),
        reviewers: parsed.flag("reviewers").then(Vec::new),
        max_rounds: parsed.flag("max-rounds").then_some(0),
        known: vec![],
    };
    refuse_own_agent()?;
    write(&config_file()?, repo.trim(), Edit::Unmap, &change)
}

// The owner's alone: a worker or a turn running it is a slip, as on asker.rs's session-less
// path, and not a security boundary. `reviewers` decides whose GitHub text reaches a worker.
pub(crate) fn refuse_own_agent() -> Result<(), Error> {
    match coauthor::marked(&mut run_command, None) || conversation_session().is_some() {
        true => Err(Error::Failed("this command is the owner's: run it from the owner's checkout".into())),
        false => Ok(()),
    }
}

// `--reviewers`: GitHub logins, comma-separated, or `none` for no one.
pub(crate) fn reviewers(logins: &str) -> Result<Vec<String>, Error> {
    match logins.trim() {
        "none" => Ok(vec![]),
        logins => {
            let logins: Vec<String> = logins.split(',').map(|l| l.trim().to_string()).collect();
            match logins.iter().all(|l| login(l)) {
                true => Ok(logins),
                false => Err(Error::Usage("--reviewers takes GitHub logins, comma-separated, or none".into())),
            }
        }
    }
}

pub(crate) fn max_rounds(n: &str) -> Result<i64, Error> {
    n.trim().parse::<i64>().ok().filter(|n| *n >= 1).ok_or_else(|| Error::Usage("--max-rounds takes a whole number of at least 1".into()))
}

// A GitHub login: letters, digits and single hyphens, or an app's `<name>[bot]`.
fn login(text: &str) -> bool {
    let name = text.strip_suffix("[bot]").unwrap_or(text);
    (1..=39).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') && !name.starts_with('-') && !name.ends_with('-') && !name.contains("--")
}

fn trimmed(values: &[String]) -> Vec<String> {
    values.iter().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).collect()
}

// The change's teams by their keys as Linear has them, once each is a team the app can see and
// each project is one of theirs by ID or name, or with no team given, one of any team's; and
// each project's ID and name.
pub(crate) fn checked(teams: &[KeyedTeam], change: &Change) -> Result<(Vec<String>, Vec<Project>), Error> {
    let keys = change.teams.iter().map(|key| {
        teams.iter().map(|t| t.key.clone()).find(|k| same(k, key)).ok_or_else(|| Error::Failed(format!("the app can see no team with the key {key}")))
    });
    let keys = keys.collect::<Result<Vec<_>, Error>>()?;
    let theirs = teams.iter().filter(|t| change.teams.is_empty() || keys.contains(&t.key));
    let projects: Vec<Project> = theirs.flat_map(|t| &t.projects.nodes).map(|p| (p.id.clone(), p.name.clone())).collect();
    let whose = if change.teams.is_empty() { "any team the app can see" } else { "the teams given" };
    let known = change.projects.iter().map(|project| {
        projects.iter().find(|(id, name)| same(id, project) || same(name, project)).cloned().ok_or_else(|| Error::Failed(format!("no project of {whose} has the ID or name {project}")))
    });
    Ok((keys, known.collect::<Result<Vec<_>, Error>>()?))
}

// The edit, merged into the file: exit 0 when it changed, 3 when it didn't.
pub(crate) fn write(file: &Path, repo: &str, edit: Edit, change: &Change) -> Result<u8, Error> {
    let changed = merge(file, |repos| edited(repos, repo, edit, change))?;
    print_json(&json!({ "repo": repo, "changed": changed }))?;
    Ok(if changed { 0 } else { NOTHING_TO_DO })
}

// `repos` as `edit` makes it from `repos` as the file has it then, in one write; whether it
// changed. A refused edit writes nothing.
pub(crate) fn merge(file: &Path, edit: impl FnOnce(Option<&Value>) -> Result<Value, Error>) -> Result<bool, Error> {
    let mut refused = None;
    let changed = merge_json_file(file, false, |config| match edit(config.get("repos")) {
        Ok(repos) => Map::from_iter([("repos".into(), repos)]),
        Err(error) => {
            refused = Some(error);
            Map::new()
        }
    })?;
    match refused {
        Some(error) => Err(error),
        None => Ok(changed),
    }
}

// `repos` with each of the edits made in turn, as `edited` makes one.
pub(crate) fn all_edited(repos: Option<&Value>, edits: &[(String, Change)]) -> Result<Value, Error> {
    let mut repos = repos.cloned().unwrap_or(Value::Null);
    for (repo, change) in edits {
        repos = edited(Some(&repos), repo, Edit::Map, change)?;
    }
    Ok(repos)
}

// `repos` with the edit made, null once no entry is left, checked as `tolkster config` reads it.
// An entry's keys the edit doesn't name stay.
pub(crate) fn edited(repos: Option<&Value>, repo: &str, edit: Edit, change: &Change) -> Result<Value, Error> {
    let mut entries = match repos {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(entries)) => entries.clone(),
        Some(other) => return RepoMap::from_config(Some(other)).map(|_| Value::Null),
    };
    // The repo's key as the file has it, or its name for a new entry.
    let key = entries.keys().find(|k| same(k, repo)).cloned().unwrap_or_else(|| repo.to_string());
    if entries.get(&key).is_some_and(|entry| !entry.is_object()) {
        return RepoMap::from_config(repos).map(|_| Value::Null);
    }
    match edit {
        Edit::Map => {
            let others: Vec<Value> = entries.iter().filter(|(k, _)| **k != key).map(|(_, e)| e.clone()).collect();
            let entry = entries.entry(key.clone()).or_insert_with(|| json!({}));
            for (list, values) in [("projects", &change.projects), ("teams", &change.teams)] {
                for value in values {
                    // A project by its ID or its name, whichever an entry has it by.
                    let known = change.known.iter().find(|(id, name)| list == "projects" && (same(id, value) || same(name, value)));
                    let names: Vec<&str> = std::iter::once(value.as_str()).chain(known.into_iter().flat_map(|(id, name)| [id.as_str(), name.as_str()])).collect();
                    let named = |e: &Value| listed(e, list).any(|v| names.iter().any(|n| same(v, n)));
                    if others.iter().any(named) {
                        return Err(Error::Usage(format!("\"repos\" in config.json claims the {} \"{value}\" more than once", list.trim_end_matches('s'))));
                    }
                    if !named(entry) {
                        push(entry, list, value);
                    }
                }
            }
            let settings = [("reviewers", change.reviewers.as_ref().map(|r| json!(r))), ("maxRounds", change.max_rounds.map(|n| json!(n)))];
            for (field, value) in settings.into_iter().filter_map(|(field, value)| Some((field, value?))) {
                if !entry["review"].is_object() {
                    entry["review"] = json!({});
                }
                entry["review"][field] = value;
            }
        }
        Edit::Unmap if change.projects.is_empty() && change.teams.is_empty() && change.reviewers.is_none() && change.max_rounds.is_none() => {
            entries.remove(&key);
        }
        Edit::Unmap => {
            if let Some(fields) = entries.get_mut(&key).and_then(Value::as_object_mut) {
                for (list, values) in [("projects", &change.projects), ("teams", &change.teams)] {
                    let Some(items) = fields.get_mut(list).and_then(Value::as_array_mut) else { continue };
                    let before = items.len();
                    items.retain(|v| !v.as_str().is_some_and(|v| values.iter().any(|value| same(v, value))));
                    if items.is_empty() && before > 0 {
                        fields.remove(list);
                    }
                }
                if let Some(review) = fields.get_mut("review").and_then(Value::as_object_mut) {
                    for (field, removed) in [("reviewers", change.reviewers.is_some()), ("maxRounds", change.max_rounds.is_some())] {
                        if removed {
                            review.remove(field);
                        }
                    }
                    if review.is_empty() {
                        fields.remove("review");
                    }
                }
            }
            if entries.get(&key).is_some_and(empty) {
                entries.remove(&key);
            }
        }
    }
    if entries.is_empty() {
        return Ok(Value::Null);
    }
    let repos = Value::Object(entries);
    RepoMap::from_config(Some(&repos))?;
    Ok(repos)
}

fn listed<'a>(entry: &'a Value, key: &str) -> impl Iterator<Item = &'a str> {
    entry.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str)
}

fn push(entry: &mut Value, key: &str, value: &str) {
    match entry.get_mut(key).and_then(Value::as_array_mut) {
        Some(list) => list.push(json!(value)),
        None => entry[key] = json!([value]),
    }
}

// Mapping nothing and setting nothing.
fn empty(entry: &Value) -> bool {
    listed(entry, "projects").next().is_none() && listed(entry, "teams").next().is_none() && ["review", "recipe"].iter().all(|key| entry.get(*key).is_none_or(Value::is_null))
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

#[cfg(test)]
mod tests;
