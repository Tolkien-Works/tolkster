// A worker host's credential traded at the Worker's POST /token (docs/hosts.md) for the
// tokens it acts with: the Linear app token, which linear/token.rs caches as it caches one it mints,
// and the owner's Claude token, which goes into a worker's agent's environment alone
// (harness::exec) and is never kept. Every host, the `listen` host with the realtime secret, trades too for the GitHub
// App's tokens, which github/app.rs holds, and the app's record. Also which secrets the Worker
// lacks, for `doctor`. No token reaches output, logs or errors.

use serde_json::{Value, json};
use ureq::Agent;

use crate::config::{Files, host_credential, realtime, worker_url};
use crate::error::Error;
use crate::gateway;
use crate::http::{get_bytes, post_json, retried};
use crate::plain::Sha;

// The Worker's answer for the Linear app token, as token.json holds one (`linear::token`), for
// a host with a credential, never `rejected`, the one Linear last refused it; None for a host
// without.
pub fn linear(agent: &Agent, files: &Files, rejected: Option<&str>) -> Result<Option<Value>, Error> {
    let Some(credential) = host_credential(files) else { return Ok(None) };
    trade(agent, files, &credential, json!({ "for": "linear", "rejected": rejected })).map(Some)
}

// The owner's Claude token, for a host with a credential; None for one without, which signs
// its agent in itself, as the `listen` host, where every conversation turn runs, does.
pub fn claude(agent: &Agent, files: &Files) -> Result<Option<String>, Error> {
    let Some(credential) = host_credential(files) else { return Ok(None) };
    let data = trade(agent, files, &credential, json!({ "for": "claude" }))?;
    data["token"].as_str().filter(|t| !t.is_empty()).map(|t| Some(t.to_string())).ok_or_else(|| Error::Failed("the Worker's Claude token had another shape".into()))
}

// An installation token for `owner/name` with `purpose`'s permissions, which the Worker mints
// only for a repo of that owner.
pub fn github(agent: &Agent, files: &Files, (owner, name): (&str, &str), purpose: &str) -> Result<String, Error> {
    let data = trade(agent, files, &bearer(files)?, json!({ "for": "github", "repo": format!("{owner}/{name}"), "purpose": purpose }))?;
    data["token"].as_str().filter(|t| !t.is_empty()).map(String::from).ok_or_else(|| Error::Failed("the Worker's github token had another shape".into()))
}

// The GitHub App's own record, as far as the Worker gives it: the events it is subscribed to,
// its slug and its installations' IDs. It needs no installation.
pub fn github_app(agent: &Agent, files: &Files) -> Result<Value, Error> {
    trade(agent, files, &bearer(files)?, json!({ "for": "githubApp" }))
}

// The names of the secrets the Worker hands out that it lacks.
pub fn missing(agent: &Agent, files: &Files) -> Result<Vec<String>, Error> {
    names(&trade(agent, files, &bearer(files)?, json!({ "for": "missing" }))?).ok_or_else(|| Error::Failed("the Worker didn't say which secrets it lacks".into()))
}

// Why a host can't get its token, from what `missing` said: the secrets the Worker lacks, or
// that it couldn't be asked. None when it holds every one.
pub fn lacking(missing: &Result<Vec<String>, Error>) -> Option<String> {
    match missing {
        Ok(names) if names.is_empty() => None,
        Ok(names) => Some(format!("the Worker has no {} set, so a host can't get its token: set it with wrangler secret put", names.join(", "))),
        Err(error) => Some(format!("the Worker couldn't be asked which secrets it lacks: {error}")),
    }
}

// The prebuilt `tolkster` for `target` at `commit` (prebuilt.rs), as the tarball CI kept, for the
// `listen` host (its realtime secret) or a worker host (its credential); None when the Worker has
// no build of that commit for that target.
pub fn build(agent: &Agent, files: &Files, target: &str, commit: &Sha) -> Result<Option<Vec<u8>>, Error> {
    let bearer = bearer(files)?;
    let url = worker_url(files)?.build(target, commit);
    retried(|| match get_bytes(agent, &url, &bearer, MAX_BUILD) {
        Ok((200, bytes)) => Ok(Some(bytes)),
        Ok((404, _)) => Ok(None),
        Ok((status, bytes)) => Err(answer("build", status, &String::from_utf8_lossy(&bytes)).err().unwrap_or_else(|| failed("build", format!("HTTP {status}")))),
        Err(error) => Err(unreached("build", error)),
    })
}

// A build is a few MB: more than this is not one.
const MAX_BUILD: u64 = 128 * 1024 * 1024;

// The realtime secret, or else this host's credential.
fn bearer(files: &Files) -> Result<String, Error> {
    match realtime(files) {
        Ok((_, secret)) => Ok(secret),
        Err(error) => host_credential(files).ok_or(error),
    }
}

fn names(data: &Value) -> Option<Vec<String>> {
    data["missing"].as_array()?.iter().map(|name| name.as_str().map(String::from)).collect()
}

// Sent again after RETRIES' pauses while it is unavailable (`answer`): every launch on a host
// starts here.
fn trade(agent: &Agent, files: &Files, bearer: &str, body: Value) -> Result<Value, Error> {
    let (url, kind) = (worker_url(files)?.token(), body["for"].as_str().unwrap_or_default().to_string());
    retried(|| match post_json(agent, &url, bearer, &[], &body.to_string()) {
        Ok((status, text)) => answer(&kind, status, &text),
        Err(error) => Err(unreached(&kind, error)),
    })
}

// Classed as every call's transport error is (`Error::from`): a Worker that can't be reached
// may answer later, a malformed URL won't.
fn unreached(kind: &str, error: ureq::Error) -> Error {
    match Error::from(error) {
        Error::Unavailable(detail) => Error::Unavailable(format!("the Worker's {} {detail}", noun(kind))),
        other => failed(kind, other.to_string()),
    }
}

// What a kind of call is called in a message, followed by its colon.
fn noun(kind: &str) -> String {
    if kind == "build" { "build:".into() } else { format!("{kind} token:") }
}

fn failed(kind: &str, detail: String) -> Error {
    Error::Failed(format!("the Worker's {} {detail}", noun(kind)))
}

// What the Worker answered, or why it refused: a missing secret by its name, as the Worker
// sends it, or GitHub's mint refused in the Worker's own words, never the body. A 5xx but a
// missing secret's, or a 429 without a wait, may pass (`Unavailable`); a 429 with GitHub's
// wait is its budget spent (`RateLimited`).
fn answer(kind: &str, status: u16, text: &str) -> Result<Value, Error> {
    let data = serde_json::from_str::<Value>(text).ok();
    let refused = data.as_ref().and_then(|d| d["refused"].as_str()).map(String::from);
    let missing = data.as_ref().and_then(names).filter(|n| !n.is_empty());
    match (status, data) {
        (200, Some(data)) => Ok(data),
        (200, None) => Err(failed(kind, "the response was not JSON".into())),
        (401, _) => Err(failed(kind, "the Worker refused this host's credential: revoked, replaced or never issued".into())),
        (503, _) if missing.is_some() => Err(failed(kind, format!("the Worker has no {} set", missing.unwrap_or_default().join(", ")))),
        (429, Some(data)) if data["retryAfter"].is_u64() => {
            Err(Error::RateLimited { message: format!("the Worker's {} {}", noun(kind), refused.unwrap_or_default()), wait: gateway::wait(data["retryAfter"].as_u64()) })
        }
        (429 | 500.., _) => Err(Error::Unavailable(format!("the Worker's {} {}", noun(kind), refused.unwrap_or_else(|| format!("HTTP {status}"))))),
        (_, _) if refused.is_some() => Err(failed(kind, refused.unwrap_or_default())),
        (status, _) => Err(failed(kind, format!("HTTP {status}"))),
    }
}

#[cfg(test)]
mod tests;
