// The app token Tolkster acts with, cached in ~/.tolkster/token.json and minted again
// from the client credentials when it expires or Linear rejects it; on a worker host, which
// holds none, traded with the Worker for its own (trade.rs).

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use ureq::Agent;

use crate::cli::{credentials_file, env, home, shown, token_file};
use crate::config::files;
use crate::time::now_ms;
use crate::error::{Context, Error};
use crate::http::base64;
use crate::json::decode;
use crate::sync::lock;
use crate::trade;

use super::SCOPES;

const TOKEN_URL: &str = "https://api.linear.app/oauth/token";

const DAY_MS: f64 = 24.0 * 60.0 * 60.0 * 1000.0;

// The token.json format: `expiresAt` is in epoch milliseconds.
#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppToken {
    pub access_token: String,
    pub expires_at: f64,
}

impl AppToken {
    pub fn from_json(text: &str) -> Option<AppToken> {
        serde_json::from_str(text).ok()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

// A token within a day of expiry is treated as expired, so a long worker session
// never starts with one that runs out part way through.
pub fn is_usable(token: Option<&AppToken>, now_ms: f64) -> bool {
    token.is_some_and(|t| t.expires_at - now_ms > DAY_MS)
}

// `error_description` explains a refusal and never holds the secret.
#[derive(Deserialize)]
struct Refused {
    error_description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Granted {
    access_token: String,
    expires_in: f64,
}

pub fn fetch_app_token(agent: &Agent, client_id: &str, client_secret: &str, now_ms: f64) -> Result<AppToken, Error> {
    let mut response = agent
        .post(TOKEN_URL)
        .header("Authorization", format!("Basic {}", base64(format!("{client_id}:{client_secret}").as_bytes())))
        .send_form([("grant_type", "client_credentials"), ("scope", SCOPES)])?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string()?;
    if !(200..300).contains(&status) {
        let detail = serde_json::from_str::<Refused>(&text).ok().and_then(|body| body.error_description).filter(|d| !d.is_empty()).map(|d| format!(": {d}")).unwrap_or_default();
        return Err(Error::Failed(format!("Linear token request failed: HTTP {status}{detail}")));
    }
    let granted: Granted = decode(serde_json::from_str(&text).context("Linear's answer to POST /oauth/token")?, "Linear's answer to POST /oauth/token")?;
    Ok(AppToken { access_token: granted.access_token, expires_at: now_ms + granted.expires_in * 1000.0 })
}

// The token this process last used, per process as each of the CLI's holds its own. token.json
// is what they share, read again when this one is unusable or rejected: another process may
// have minted a new one.
static HELD: Mutex<Option<AppToken>> = Mutex::new(None);

// Runs `call` with a usable token, and again with another if Linear rejects it: it
// revoked the token, as a rotated client secret does.
pub fn with_token<T>(agent: &Agent, mut call: impl FnMut(&str) -> Result<T, Error>) -> Result<T, Error> {
    let mut rejected = Vec::new();
    loop {
        let (token, minted) = token(agent, &rejected)?;
        match call(&token) {
            Err(Error::Unauthorized(_)) if retries(&rejected, minted) => rejected.push(token),
            other => return other,
        }
    }
}

// Whether to try again after Linear rejected a token (`minted` if it was new), having
// already rejected `rejected`. A second rejection of a token from token.json is retried
// too, as the revocation that hit the held one may have hit it: the next is minted.
// At most three calls.
fn retries(rejected: &[String], minted: bool) -> bool {
    rejected.is_empty() || (rejected.len() == 1 && !minted)
}

// The held token, else token.json's, else a new one, never one of `rejected`, and whether
// it was minted. Minting holds the lock, so `listen`'s threads don't each mint one.
fn token(agent: &Agent, rejected: &[String]) -> Result<(String, bool), Error> {
    let now = now_ms() as f64;
    let usable = |t: &AppToken| is_usable(Some(t), now) && !rejected.contains(&t.access_token);
    let mut held = lock(&HELD);
    if let Some(token) = held.as_ref().filter(|t| usable(t)) {
        return Ok((token.access_token.clone(), false));
    }
    let (fresh, minted) = match read_cached_token().filter(|t| usable(t)) {
        Some(cached) => (cached, false),
        None => (mint(agent, now, rejected)?, true),
    };
    let access_token = fresh.access_token.clone();
    *held = Some(fresh);
    Ok((access_token, minted))
}

// Never one of `rejected`, those Linear refused: the Worker is told the last, and one of its
// isolates may still hold an earlier one, which is never kept.
fn mint(agent: &Agent, now: f64, rejected: &[String]) -> Result<AppToken, Error> {
    let fresh = match credentials()? {
        Some((client_id, client_secret)) => fetch_app_token(agent, &client_id, &client_secret, now)?,
        None => {
            let traded = trade::linear(agent, &files()?, rejected.last().map(String::as_str))?.ok_or_else(|| {
                Error::Usage(format!("no credentials: set TOLKSTER_CLIENT_ID and TOLKSTER_CLIENT_SECRET, or write {}", shown(credentials_file(), "")))
            })?;
            AppToken::from_json(&traded.to_string()).ok_or_else(|| Error::Failed("the Worker's Linear token had another shape".into()))?
        }
    };
    if rejected.contains(&fresh.access_token) {
        return Err(Error::Unauthorized("the Worker handed back a Linear token Linear had refused".into()));
    }
    let home = home()?;
    fs::create_dir_all(&home).context(home.display())?;
    // Written aside and renamed, so a reader in another process never sees half a file.
    // The mode protects it on Unix only; on Windows the profile's ACL keeps it private.
    let token_file = token_file()?;
    // Named by process and count, so mints never share a partial file.
    static MINTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = MINTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let partial = PathBuf::from(format!("{}.{}.{count}.tmp", token_file.display(), std::process::id()));
    // A partial file holds the token, so a failed write or rename removes it.
    if let Err(error) = write_private(&partial, &fresh.to_json()).context(partial.display()).and_then(|()| fs::rename(&partial, &token_file).context(token_file.display())) {
        let _ = fs::remove_file(&partial);
        return Err(error);
    }
    Ok(fresh)
}

fn write_private(path: &PathBuf, contents: &str) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(contents.as_bytes())
}

fn read_cached_token() -> Option<AppToken> {
    AppToken::from_json(&fs::read_to_string(token_file().ok()?).ok()?)
}

fn credentials() -> Result<Option<(String, String)>, Error> {
    if let (Some(id), Some(secret)) = (env("TOLKSTER_CLIENT_ID"), env("TOLKSTER_CLIENT_SECRET")) {
        return Ok(Some((id, secret)));
    }
    let files = files()?;
    Ok(files.secret("clientId").zip(files.secret("clientSecret")))
}

#[cfg(test)]
mod tests;
