// `tolkster host`: a worker host's lifecycle (docs/hosts.md). `add` (host/add.rs) makes a
// paired Orca server one, or with `--recipe` (host/recipe.rs) a machine a recipe makes; `rm
// <host>` revokes its credential, so it takes no new worker and its next call to the Worker is
// refused, and destroys a recipe's machine. `join` and `clone` run on the new host, typed there
// by `add`: `join` trades an enrolment code for the host's credential, identity and settings,
// `clone` clones a repo through the app's token, and `identify` sets the owner's git author
// identity and checks the co-author. `add` and `rm` are the owner's alone.

use std::path::Path;

use serde_json::{Map, Value, json};
use ureq::Agent;

use crate::args::{Kind, parse};
use crate::cli::{config_file, credentials_file, merge_json_file, print_json};
use crate::config::{WorkerUrl, files, unchecked_files};
use crate::runtime::settings;
use crate::credential::git_env;
use crate::exec::run_command;
use crate::git;
use crate::github::remote_repo;
use crate::plain::split_repo;
use crate::error::{Context, Error};
use crate::http::post_json;
use crate::orca::{Host, paired};
use crate::plain::{plain_code, plain_host, unhex};
use crate::state::State;

pub const JOIN: &[(&str, Kind)] = &[("worker-url", Kind::String)];
pub const CLONE: &[(&str, Kind)] = &[("into", Kind::String)];
pub const IDENTIFY: &[(&str, Kind)] = &[("name", Kind::String), ("email", Kind::String), ("co-author", Kind::String), ("check", Kind::Boolean)];

pub fn rm(agent: &Agent, rest: &[String]) -> Result<u8, Error> {
    let parsed = parse(rest, &[], true)?;
    let host = match parsed.positionals.as_slice() {
        [host] if plain_host(host) => host,
        _ => return Err(Error::Usage("host rm needs one host, as Orca names it".into())),
    };
    let files = files()?;
    let id = credential_id(&paired(&mut run_command)?, host);
    let held = State::open(agent, &files)?.revoke_host(&id)?;
    let destroyed = crate::recipe::destroy(host)?;
    print_json(&json!({ "host": host, "held": held, "destroyed": destroyed }))?;
    Ok(if destroyed == Some(false) { 1 } else { 0 })
}

// The credential is the environment ID's: the one Orca pairs under the name, else the
// argument, for an ID or a host already unpaired.
fn credential_id(paired: &[Host], host: &str) -> String {
    paired.iter().find(|paired| paired.name == host).map_or_else(|| host.to_string(), |paired| paired.id.clone())
}

// The Worker's answer is the only source of the credential, which goes to credentials.json and
// nowhere else: not to output, and not to an error.
pub fn join(agent: &Agent, rest: &[String]) -> Result<u8, Error> {
    let parsed = parse(rest, JOIN, true)?;
    let code = match parsed.positionals.as_slice() {
        [code] if plain_code(code) => code,
        _ => return Err(Error::Usage("host join needs one enrolment code, as host add printed it".into())),
    };
    let url = WorkerUrl::parse(parsed.non_empty("worker-url").ok_or_else(|| Error::Usage("host join needs --worker-url".into()))?)?;
    // Both files are read before the trade, which can't be taken back: one that isn't a JSON
    // object fails here, while the code is still good. config.json is read unchecked, as the
    // join replaces it whole.
    let files = unchecked_files()?;
    if ![&files.config, &files.credentials].iter().all(|file| file.is_object() || file.is_null()) {
        return Err(Error::Failed("config.json and credentials.json must each be a JSON object".into()));
    }
    let (status, text) = post_json(agent, &url.enrol(), "", &[], &json!({ "code": code }).to_string())?;
    let (host, credential, mut config) = joined(status, &text, &url)?;
    // The credential first, as the Worker already holds it in place of the old one, with the
    // environment ID it was minted for, which this host's worktrees' IDs carry; settings come
    // again with any new code.
    merge_json_file(&credentials_file()?, true, |_| Map::from_iter([("hostCredential".into(), json!(credential)), ("host".into(), json!(host))]))?;
    // Settings this build refuses would leave every command failing: named here, the credential
    // kept. The owner's settings whole: a key they no longer set goes too.
    crate::config::check(&Value::Object(config.clone()))?;
    merge_json_file(&config_file()?, false, |old| {
        for key in old.keys() {
            config.entry(key.clone()).or_insert(Value::Null);
        }
        config
    })?;
    // The editor's aid alone: the code is spent and the settings written, so a failure here is not the join's.
    let _ = crate::config::write_schema(&config_file()?);
    print_json(&json!({ "host": host, "joined": true }))?;
    Ok(0)
}

// What POST /enrol answered, as the host's name, its credential and the settings config.json
// takes, with `worker` the address the code was traded at.
pub(crate) fn joined(status: u16, text: &str, url: &WorkerUrl) -> Result<(String, String, Map<String, Value>), Error> {
    match status {
        200 => {}
        404 => return Err(Error::Failed("the Worker knows no such enrolment code: it was used, it expired or it was never minted. Run tolkster host add again".into())),
        status => return Err(Error::Failed(format!("the Worker's enrolment: HTTP {status}"))),
    }
    let shape = || Error::Failed("the Worker's enrolment had another shape".into());
    let data: Value = serde_json::from_str(text).map_err(|_| shape())?;
    let host = data["host"].as_str().filter(|host| plain_host(host)).ok_or_else(shape)?;
    let credential = data["credential"].as_str().filter(|c| c.strip_prefix(host).is_some_and(|rest| rest.starts_with(':'))).ok_or_else(shape)?;
    let mut config = data["config"].as_object().cloned().ok_or_else(shape)?;
    config.insert("worker".into(), json!(url.base()));
    config.insert("$schema".into(), json!(crate::config::SCHEMA_REF));
    Ok((host.into(), credential.into(), config))
}

// Over https through the git credential helper (credential.rs), so the host needs no git login
// of its own. A folder already holding a whole clone of the repo is fetched, its files left as
// they are; one holding a clone cut off, or another repo's, is refused, never removed.
pub fn clone(rest: &[String]) -> Result<u8, Error> {
    let parsed = parse(rest, CLONE, true)?;
    let (repo, into) = match (parsed.positionals.as_slice(), parsed.non_empty("into")) {
        ([repo], Some(into)) if split_repo(repo).is_some() => (repo, into),
        _ => return Err(Error::Usage("host clone needs one owner/name and --into <folder>".into())),
    };
    let exe = std::env::current_exe().context("finding this tolkster")?.display().to_string();
    let env = git_env(std::env::var("GIT_CONFIG_COUNT").ok().as_deref(), &exe, repo).ok_or_else(|| Error::Failed("this tolkster's path can't go in a git credential helper".into()))?;
    if Path::new(into).join(".git").exists() {
        let mut run = run_command;
        let origin = git::origin_url(&mut run, Some(into)).ok().and_then(|url| remote_repo(&url));
        if git::head(&mut run, into).is_err() || !origin.is_some_and(|origin| origin.eq_ignore_ascii_case(repo)) {
            return Err(Error::Failed(format!("{into} holds something other than a whole clone of {repo}: move it aside, then run host add again")));
        }
        git::fetch_with(&env, into)?;
        print_json(&json!({ "path": into, "cloned": false, "fetched": true }))?;
        return Ok(0);
    }
    git::clone_with(&env, &format!("https://github.com/{repo}.git"), into)?;
    print_json(&json!({ "path": into, "cloned": true }))?;
    Ok(0)
}

// Typed on the host by `add`, its values as hex so the line stays plain. `--name` and `--email`
// set the global git author identity, left alone where it already matches; `--co-author` only
// asks whether this host's settings resolve to it. `--check` changes nothing: the exit code is
// whether the host is already so.
pub fn identify(rest: &[String]) -> Result<u8, Error> {
    let parsed = parse(rest, IDENTIFY, true)?;
    let shape = || Error::Usage("host identify needs --name and --email, or --check --co-author, each as hex".into());
    let decode = |flag: &str| parsed.non_empty(flag).map(|value| unhex(value).filter(|text| !text.is_empty()).ok_or_else(shape)).transpose();
    let mut run = run_command;
    match (decode("name")?, decode("email")?, decode("co-author")?) {
        (Some(name), Some(email), None) if parsed.positionals.is_empty() => {
            let matches = git::global_config(&mut run, "user.name").as_deref() == Some(&name) && git::global_config(&mut run, "user.email").as_deref() == Some(&email);
            if !matches && !parsed.flag("check") {
                git::set_global_config(&mut run, "user.name", &name)?;
                git::set_global_config(&mut run, "user.email", &email)?;
            }
            Ok(u8::from(!matches && parsed.flag("check")))
        }
        (None, None, Some(co_author)) if parsed.positionals.is_empty() && parsed.flag("check") => {
            let files = files()?;
            let settled = settings(&files)?;
            Ok(u8::from(settled.owner.co_author.as_deref() != Some(&co_author)))
        }
        _ => Err(shape()),
    }
}

pub mod add;
mod recipe;

#[cfg(test)]
mod tests;
