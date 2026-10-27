// The GitHub App's tokens and its transport: every request to GitHub's API. A token is
// narrowed to one repo and to the permissions its call needs (`Purpose`), and minted by the
// Worker, which alone holds the app's key (src/worker/mint.ts) and hands a token out only for
// a repo of the owner asked for. The tokens never reach disk, the environment, output, logs
// or errors, and no token's call pushes but a branch's update, GitHub's own merge of the
// base, and a PR's merge, each leased on the head (`Reader::update_branch`, `merge_pr`).
//
// Visible only inside `github`; what the rest of the crate needs is re-exported there.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::{Value, json};
use ureq::Agent;
use ureq::http::Method;

use super::cost;
use super::{AppRecord, Reader};
use crate::plain::split_repo;
use crate::config::{Files, files};
use crate::error::Error;
use crate::gateway::{self, Api};
use crate::http::{self, send_json};
use crate::json::{decode, loose};
use crate::plain::Sha;
use crate::sync::lock;
use crate::time::now_secs;
use crate::trace::{self, Call};
use crate::trade;

pub(super) const API: &str = "https://api.github.com";

// The app as this host reaches it: through the Worker, with the realtime secret or this
// host's credential, from credentials.json.
pub struct App {
    files: Files,
}

impl App {
    fn configured() -> Result<App, Error> {
        Ok(App { files: files()? })
    }
}

// What an installation token is for. Each asks for only the permissions its call needs,
// which the Worker sets by its name (src/worker/mint.ts, PURPOSES).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Purpose {
    Create,
    Reply,
    Resolve,
    AutoMerge,
    // Replacing an open PR's description.
    Describe,
    // Merging a PR's base into its branch (`Reader::update_branch`).
    UpdateBranch,
    // Merging a PR into its base (`Reader::merge_pr`).
    Merge,
    // A branch pushed through the `git-credential` helper (`push_token`).
    Push,
    // Reading PRs, their reviews, how their branch compares with its base, and whether a
    // required check failed.
    Read,
}

impl Purpose {
    #[cfg(test)]
    const ALL: [Purpose; 9] = [Purpose::Create, Purpose::Reply, Purpose::Resolve, Purpose::AutoMerge, Purpose::Describe, Purpose::UpdateBranch, Purpose::Merge, Purpose::Push, Purpose::Read];

    fn name(self) -> &'static str {
        match self {
            Purpose::Create => "create",
            Purpose::Reply => "reply",
            Purpose::Resolve => "resolve",
            Purpose::AutoMerge => "autoMerge",
            Purpose::Describe => "describe",
            Purpose::UpdateBranch => "updateBranch",
            Purpose::Merge => "merge",
            Purpose::Push => "push",
            Purpose::Read => "read",
        }
    }
}

// How long a token is used for: GitHub's last an hour.
const TOKEN_LIFE_SECS: u64 = 50 * 60;

fn fresh(minted: u64, now: u64) -> bool {
    now.saturating_sub(minted) < TOKEN_LIFE_SECS
}

// Installation tokens, each for one repo (owner and name, lower case) and one purpose, with
// when it was minted. The owner is in the key, for a token is minted by name alone and the
// Worker has checked it only against the owner it was asked for.
#[derive(Default)]
struct Tokens(HashMap<Key, (String, u64)>);

type Key = (String, String, Purpose);

impl Tokens {
    // The token held for `key` while it is fresh, else None, with a stale one taken out for
    // the caller to revoke.
    fn fresh(&mut self, key: &Key, now: u64) -> (Option<String>, Option<String>) {
        match self.0.get(key) {
            Some((token, minted)) if fresh(*minted, now) => (Some(token.clone()), None),
            Some(_) => (None, self.0.remove(key).map(|(token, _)| token)),
            None => (None, None),
        }
    }

    // Revokes every token. Best effort: they expire within the hour anyway.
    fn revoke(&mut self, agent: &Agent) {
        for (_, (token, _)) in self.0.drain() {
            let _ = delete(agent, &format!("{API}/installation/token"), &token);
        }
    }
}

// The app and its tokens, held for the process once `hold` is called: `tolkster listen`
// does, a command never. The app is loaded on first use, and a token is kept per repo and
// purpose until it is TOKEN_LIFE_SECS old. The lock guards the map alone: a mint, a trade
// with the Worker that may retry, and a call with the token are made outside it, so one slow
// mint never holds up a thread whose token is held. Two threads may then mint for the same
// repo at once; the later replaces the earlier, which expires within the hour.
#[derive(Default)]
struct Held {
    app: Option<Arc<App>>,
    tokens: Tokens,
}

// Per process, as the app's tokens are what the process holds: `listen`'s for as long as it runs.
static HELD: Mutex<Option<Held>> = Mutex::new(None);

fn held() -> MutexGuard<'static, Option<Held>> {
    lock(&HELD)
}

// Holds the app and its tokens for the rest of the process (see `Held`). For `tolkster
// listen` alone.
pub fn hold() {
    *held() = Some(Held::default());
}

// Revokes every token the process holds and stops holding, as `tolkster listen` ends. A
// process that dies instead leaves them to expire within the hour.
pub fn revoke_held(agent: &Agent) {
    // Taken out under the lock, revoked outside it.
    let taken = held().take();
    if let Some(mut held) = taken {
        held.tokens.revoke(agent);
    }
}

// The app as credentials.json reaches it: held for the process (`hold`), loaded once, or
// loaded now.
pub fn app() -> Result<Arc<App>, Error> {
    let mut held = held();
    let Some(held) = held.as_mut() else { return App::configured().map(Arc::new) };
    if let Some(app) = &held.app {
        return Ok(app.clone());
    }
    let app = Arc::new(App::configured()?);
    held.app = Some(app.clone());
    Ok(app)
}

// A token for `owner/name` and `purpose`, minted only for a repo of that owner: the
// process's, when it holds them (`hold`), else one of `own`, the caller's, which the caller
// revokes when it is done. A stale one is revoked, and `beat` called before a mint, for
// `listen`'s watchdog.
fn token_for(agent: &Agent, app: &App, (owner, name): (&str, &str), purpose: Purpose, now: u64, beat: &dyn Fn(), own: &mut Tokens) -> Result<String, Error> {
    let key = (owner.to_ascii_lowercase(), name.to_ascii_lowercase(), purpose);
    let found = held().as_mut().map(|held| held.tokens.fresh(&key, now));
    let holding = found.is_some();
    let (token, stale) = found.unwrap_or_else(|| own.fresh(&key, now));
    if let Some(stale) = stale {
        let _ = delete(agent, &format!("{API}/installation/token"), &stale);
    }
    if let Some(token) = token {
        return Ok(token);
    }
    beat();
    let token = trade::github(agent, &app.files, (owner, name), purpose.name())?;
    // Kept where it was looked for; if `listen` stopped holding meanwhile, the caller's, to
    // revoke.
    let mut held = held();
    let tokens = match held.as_mut() {
        Some(held) if holding => &mut held.tokens,
        _ => own,
    };
    tokens.0.insert(key, (token.clone(), now));
    Ok(token)
}

// A token for a push, which git uses after the helper that asked for it has exited: neither
// held nor revoked here, so it lasts its hour.
pub fn push_token(agent: &Agent, files: &Files, repo: (&str, &str)) -> Result<String, Error> {
    trade::github(agent, files, repo, Purpose::Push.name())
}

// Runs `act` with an installation token for the one repo and `purpose`'s permissions, and
// nothing else. The token is the process's, when it holds them, else minted for this call
// and revoked after it.
pub(super) fn as_app<T>(agent: &Agent, app: &App, owner: &str, repo_name: &str, purpose: Purpose, now_secs: u64, act: impl FnOnce(&str) -> Result<T, Error>) -> Result<T, Error> {
    let mut own = Tokens::default();
    let token = token_for(agent, app, (owner, repo_name), purpose, now_secs, &|| {}, &mut own)?;
    let result = act(&token);
    own.revoke(agent);
    result
}

// Reads as the app, with a token per repo for `Purpose::Read` alone. The app is loaded on the
// first read, so a run with nothing to read never needs it. `beat` is called before
// each request, for `listen`'s watchdog. Its tokens are the process's when it holds them
// (`hold`); else its own, minted for this reader and revoked when it is dropped.
pub struct AppReader<'a> {
    agent: &'a Agent,
    app: Option<Arc<App>>,
    own: Tokens,
    beat: &'a dyn Fn(),
}

impl<'a> AppReader<'a> {
    pub fn new(agent: &'a Agent, beat: &'a dyn Fn()) -> AppReader<'a> {
        AppReader { agent, app: None, own: Tokens::default(), beat }
    }

    fn loaded(&mut self) -> Result<Arc<App>, Error> {
        if self.app.is_none() {
            self.app = Some(app()?);
        }
        Ok(self.app.clone().expect("loaded above"))
    }

    fn token_as(&mut self, repo: &str, purpose: Purpose) -> Result<(String, String, String), Error> {
        let (owner, name) = split_repo(repo).ok_or_else(|| Error::Usage(format!("not a GitHub repo, owner/name: {repo}")))?;
        let (owner, name) = (owner.to_string(), name.to_string());
        let app = self.loaded()?;
        let token = token_for(self.agent, &app, (&owner, &name), purpose, now_secs(), self.beat, &mut self.own)?;
        Ok((token, owner, name))
    }

    // One of `Reader`'s writes, its `query` fixed by its caller: GitHub's whole answer, its
    // `errors` included.
    fn write(&mut self, repo: &str, purpose: Purpose, query: &str, variables: Value) -> Result<Value, Error> {
        let (token, _, _) = self.token_as(repo, purpose)?;
        (self.beat)();
        let body = json!({ "query": query, "variables": variables }).to_string();
        let (status, response) = send(self.agent, Method::POST, &format!("{API}/graphql"), &token, &HEADERS, Some(&body), false)?;
        if status != 200 {
            return Err(refused("the GraphQL call", status, &response));
        }
        Ok(response)
    }
}

impl Reader for AppReader<'_> {
    fn graphql(&mut self, repo: &str, query: &str, variables: Value) -> Result<Value, Error> {
        let (token, _, _) = self.token_as(repo, Purpose::Read)?;
        (self.beat)();
        graphql_with(self.agent, &token, query, variables, &READ_HEADERS)
    }

    fn update_branch(&mut self, repo: &str, pr: &str, head: &Sha) -> Result<Value, Error> {
        self.write(repo, Purpose::UpdateBranch, super::pr::UPDATE, json!({ "pr": pr, "head": head }))
    }

    fn merge_pr(&mut self, repo: &str, pr: &str, head: &Sha) -> Result<Value, Error> {
        self.write(repo, Purpose::Merge, super::pr::MERGE, json!({ "pr": pr, "head": head }))
    }

    fn app(&mut self) -> Result<AppRecord, Error> {
        let app = self.loaded()?;
        (self.beat)();
        app_record(self.agent, &app.files)
    }

    fn get(&mut self, repo: &str, path: &str) -> Result<Value, Error> {
        let (token, owner, name) = self.token_as(repo, Purpose::Read)?;
        (self.beat)();
        let (status, response) = get(self.agent, &format!("{API}/repos/{owner}/{name}/{path}"), &token)?;
        if status != 200 {
            return Err(refused(&format!("reading {path}"), status, &response));
        }
        Ok(response)
    }
}

impl Drop for AppReader<'_> {
    fn drop(&mut self) {
        self.own.revoke(self.agent);
    }
}

// The Worker's record of the app (`AppRecord`), read with credentials.json's `files`.
pub fn app_record(agent: &Agent, files: &Files) -> Result<AppRecord, Error> {
    decode(trade::github_app(agent, files)?, "the Worker's answer to githubApp")
}

// One GraphQL call: its `data`. GitHub answers most refusals with 200 and `errors`,
// whose messages hold no credential.
pub(super) fn graphql(agent: &Agent, token: &str, query: &str, variables: Value) -> Result<Value, Error> {
    graphql_with(agent, token, query, variables, &HEADERS)
}

fn graphql_with(agent: &Agent, token: &str, query: &str, variables: Value, headers: &[(&str, &str)]) -> Result<Value, Error> {
    let body = json!({ "query": cost::with_rate_limit(query), "variables": variables }).to_string();
    let read = query.trim_start().starts_with("query");
    let (status, response) = send(agent, Method::POST, &format!("{API}/graphql"), token, headers, Some(&body), read)?;
    if status != 200 {
        return Err(refused("the GraphQL call", status, &response));
    }
    graphql_data(response)
}

// A GraphQL answer: its `data`, null when GitHub refused all of it, and the `errors` it
// refused any of it with. `data` is open (`Value`) until the call's caller decodes it.
#[derive(Debug, Deserialize)]
pub(super) struct Answer<T> {
    pub data: Option<T>,
    #[serde(default)]
    pub errors: Vec<Explained>,
}

fn graphql_data(response: Value) -> Result<Value, Error> {
    let answer: Answer<Value> = decode(response, "GitHub's answer to POST /graphql")?;
    let errors: Vec<&str> = answer.errors.iter().filter_map(Explained::message).collect();
    if !errors.is_empty() {
        return Err(Error::Failed(format!("GitHub refused the GraphQL call: {}", errors.join("; "))));
    }
    answer.data.ok_or_else(|| Error::Failed("GitHub's GraphQL response had no data".into()))
}

// What GitHub explains an answer with: `message`, a REST refusal's or a secondary limit's, and
// `errors`, a 422's detail or GraphQL's. Read from any answer, so a body that isn't an object
// explains nothing, and an error that isn't one, as a 422's may be a string, says nothing.
// Each field is read loosely (`json::loose`), so one of another shape can't hide the other.
#[derive(Debug, Default, Deserialize)]
pub(super) struct Said {
    #[serde(default, deserialize_with = "loose")]
    pub message: Option<String>,
    #[serde(default, deserialize_with = "loose")]
    pub errors: Vec<Explained>,
}

impl Said {
    pub(super) fn of(body: &Value) -> Said {
        if !body.is_object() {
            return Said::default();
        }
        Said::deserialize(body).unwrap_or_default()
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum Explained {
    Error {
        #[serde(default, deserialize_with = "loose")]
        message: Option<String>,
        #[serde(rename = "type", default, deserialize_with = "loose")]
        kind: Option<String>,
    },
    Other(IgnoredAny),
}

impl Explained {
    pub(super) fn message(&self) -> Option<&str> {
        match self {
            Explained::Error { message, .. } => message.as_deref(),
            Explained::Other(_) => None,
        }
    }

    fn kind(&self) -> Option<&str> {
        match self {
            Explained::Error { kind, .. } => kind.as_deref(),
            Explained::Other(_) => None,
        }
    }
}

// What every call to GitHub's API sends, besides the token.
pub(super) const HEADERS: [(&str, &str); 3] = [("Accept", "application/vnd.github+json"), ("X-GitHub-Api-Version", "2022-11-28"), ("User-Agent", "tolkster")];

// A query with READ_HEADERS, as one reading `mergeStateStatus` needs.
pub(super) fn graphql_read(agent: &Agent, token: &str, query: &str, variables: Value) -> Result<Value, Error> {
    graphql_with(agent, token, query, variables, &READ_HEADERS)
}

// A read's: `mergeStateStatus` is still a preview in GraphQL.
const READ_HEADERS: [(&str, &str); 3] = [("Accept", "application/vnd.github.merge-info-preview+json"), ("X-GitHub-Api-Version", "2022-11-28"), ("User-Agent", "tolkster")];

// One POST to GitHub's API. The response body, or null when it isn't JSON.
pub(super) fn post(agent: &Agent, url: &str, bearer: &str, body: Value) -> Result<(u16, Value), Error> {
    send(agent, Method::POST, url, bearer, &HEADERS, Some(&body.to_string()), false)
}

pub(super) fn patch(agent: &Agent, url: &str, bearer: &str, body: Value) -> Result<(u16, Value), Error> {
    send(agent, Method::PATCH, url, bearer, &HEADERS, Some(&body.to_string()), false)
}

pub(super) fn get(agent: &Agent, url: &str, bearer: &str) -> Result<(u16, Value), Error> {
    send(agent, Method::GET, url, bearer, &HEADERS, None, true)
}

// A token's revoke, made whatever the gateway holds: it shortens a token's life, and costs
// at most one call a budget that is spent refuses.
fn delete(agent: &Agent, url: &str, bearer: &str) -> Result<(), Error> {
    let started = Instant::now();
    let deleted = send_json(agent, Method::DELETE, url, bearer, &HEADERS, None);
    trace::slow(|| format!("github {}", cost::current()), started);
    deleted?;
    Ok(())
}

// One call to GitHub's API, timed under its call site (`cost::site`) to the last byte of
// its response. `read` says it changes nothing, so a normal lane's waits on a low budget.
// The gateway admits it first (gateway.rs), and its response's count is the gateway's.
fn send(agent: &Agent, method: Method, url: &str, bearer: &str, headers: &[(&str, &str)], body: Option<&str>, read: bool) -> Result<(u16, Value), Error> {
    let budget = budget(url);
    gateway::admit(Api::GitHub, &[budget], read)?;
    let started = Instant::now();
    let responded = send_json(agent, method, url, bearer, headers, body).map_err(Error::from).and_then(|response| respond(response, budget));
    trace::slow(|| format!("github {}", cost::current()), started);
    responded
}

// The installation's budget a call to `url` spends: GraphQL's, else REST's (`core`).
fn budget(url: &str) -> &'static str {
    match url.strip_prefix(API) {
        Some("/graphql") => "graphql",
        _ => "core",
    }
}

// Every response from GitHub's API but a delete's: its status, and its body, or null when
// that isn't JSON. A spent budget is `Error::RateLimited` whatever the call (`spent`), a
// low one is logged (`gateway::log_low`) and so is each call's cost (`cost::line`). The count of
// `budget`, the one the call spent, is noted in the gateway, as is its being spent.
fn respond(mut response: ureq::http::Response<ureq::Body>, budget: &'static str) -> Result<(u16, Value), Error> {
    let status = response.status().as_u16();
    let limits = Limits::read(|name| http::header(&response, name));
    let text = response.body_mut().read_to_string()?;
    let body = serde_json::from_str(&text).unwrap_or(Value::Null);
    let now = now_secs();
    if let Some(line) = cost::line(cost::current(), &limits, &body) {
        trace::event(Call::Cost(line));
    }
    let spent = spent(status, &limits, &body, now);
    if limits.resource.as_deref().is_none_or(|r| r == budget) {
        gateway::note(Api::GitHub, budget, limits.limit, limits.remaining, limits.reset.map(|reset| reset.saturating_mul(1000)));
        if let Some(wait) = spent.as_ref().and_then(Error::rate_limited) {
            gateway::spent(Api::GitHub, &[budget], wait);
        }
    } else {
        // A resource the gateway doesn't note (`search`) is still logged when low.
        gateway::log_low(Api::GitHub, limits.resource.as_deref().unwrap_or("api"), limits.limit, limits.remaining, limits.reset.map(|reset| reset.saturating_mul(1000)));
    }
    match spent {
        Some(error) => Err(error),
        None => Ok((status, body)),
    }
}

// GitHub's rate-limit headers on a response, those it sent. `reset` is a Unix time and
// `retry_after` seconds from now. `resource` names the budget: GraphQL's and REST's
// (`core`) are counted apart.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Limits {
    pub limit: Option<u64>,
    pub used: Option<u64>,
    pub remaining: Option<u64>,
    pub reset: Option<u64>,
    pub retry_after: Option<u64>,
    pub resource: Option<String>,
}

impl Limits {
    pub(super) fn read(header: impl Fn(&str) -> Option<String>) -> Limits {
        let number = |name: &str| header(name).and_then(|v| v.trim().parse().ok());
        // It reaches a log line, so only a plain word.
        let resource = header("x-ratelimit-resource").filter(|r| !r.is_empty() && r.len() <= 40 && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'));
        Limits { limit: number("x-ratelimit-limit"), used: number("x-ratelimit-used"), remaining: number("x-ratelimit-remaining"), reset: number("x-ratelimit-reset"), retry_after: number("retry-after"), resource }
    }

    // Until the budget resets, as of `now`: `retry-after` first, as GitHub asks, else the
    // reset when the window is spent (`primary`), a second over so a retry lands after it,
    // else the gateway's minute, as a secondary limit with some left asks; within the
    // gateway's bounds.
    fn wait(&self, now: u64, primary: bool) -> Duration {
        let reset = self.reset.filter(|_| primary).map(|reset| reset.saturating_sub(now).saturating_add(1));
        gateway::wait(self.retry_after.or(reset))
    }
}

// The error a response is when it says the budget is spent. REST: a 403 or 429 with none
// remaining, a time to retry after, or GitHub's message for a secondary limit, which may
// come with neither. GraphQL, answered with a 200: an error of type `RATE_LIMITED`, or
// any error beside none remaining, as GitHub sends a spent budget's error untyped.
pub(super) fn spent(status: u16, limits: &Limits, body: &Value, now: u64) -> Option<Error> {
    let said = Said::of(body);
    let secondary = said.message.as_deref().is_some_and(|m| m.to_ascii_lowercase().contains("secondary rate limit"));
    let rest = matches!(status, 403 | 429) && (limits.remaining == Some(0) || limits.retry_after.is_some() || secondary);
    let typed = said.errors.iter().any(|e| e.kind() == Some("RATE_LIMITED"));
    let graphql = typed || (limits.remaining == Some(0) && !said.errors.is_empty());
    if !rest && !graphql {
        return None;
    }
    // GraphQL's own RATE_LIMITED is its window spent, though a query cheaper than the
    // last may still fit: some points left is no secondary limit.
    let wait = limits.wait(now, limits.remaining == Some(0) || (typed && !secondary));
    Some(Error::RateLimited { message: format!("GitHub's rate limit is spent: it resets in {} s", wait.as_secs()), wait })
}

// An answer that isn't the one wanted. GitHub's 500, 502, 503 and 504 are `Unavailable`:
// nothing was refused, and a try again may work. Any other status is a refusal, which GitHub
// explains in "message", and a 422's detail in "errors". Neither holds a credential.
pub(super) fn refused(what: &str, status: u16, response: &Value) -> Error {
    if matches!(status, 500 | 502 | 503 | 504) {
        return Error::Unavailable(format!("GitHub couldn't answer {what}: HTTP {status}"));
    }
    let said = Said::of(response);
    let mut detail: Vec<&str> = said.message.as_deref().into_iter().collect();
    detail.extend(said.errors.iter().filter_map(Explained::message));
    let detail = if detail.is_empty() { String::new() } else { format!(": {}", detail.join("; ")) };
    Error::Failed(format!("GitHub refused {what}: HTTP {status}{detail}"))
}

#[cfg(test)]
mod tests;
