// The GitHub App made by GitHub's manifest flow (docs/setup.md, step 4): the manifest the
// owner's browser posts, the code GitHub sends back exchanged for the app's ID, key and
// webhook secret, and the app's bot user. None of these calls carries a credential: the
// code is the only authority, and it works once, within an hour.
//
// The key and the webhook secret are held in `Created` alone, which neither prints nor
// derives Debug, until the caller hands them to the Worker.

use serde::Deserialize;
use serde_json::{Value, json};
use ureq::Agent;
use ureq::http::Method;

use super::GITHUB_EVENTS;
use super::app::{API, HEADERS};
use crate::error::Error;
use crate::http::{call_public, retried};
use crate::json::decode;

// What the app asks for (docs/github-app.md, Permissions). Issues: read is for the webhook's
// `issue_comment` alone, Actions: read for a host's download of CI's build.
pub const PERMISSIONS: [(&str, &str); 7] = [("pull_requests", "write"), ("contents", "write"), ("checks", "read"), ("statuses", "read"), ("actions", "read"), ("issues", "read"), ("metadata", "read")];

// The manifest: a private app named `name`, with `homepage` as its URL, its webhook at
// `webhook` subscribed to every event the Worker takes, and GitHub sending the owner back to
// `redirect` with the code.
pub fn manifest(name: &str, homepage: &str, webhook: &str, redirect: &str) -> Value {
    json!({
        "name": name,
        "url": homepage,
        "hook_attributes": { "url": webhook, "active": true },
        "redirect_url": redirect,
        "public": false,
        "default_permissions": PERMISSIONS.iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<serde_json::Map<_, _>>(),
        "default_events": GITHUB_EVENTS,
    })
}

// Where the manifest is posted: the org's new-app page, or with None the owner's own.
pub fn new_app_url(org: Option<&str>, state: &str) -> String {
    match org {
        Some(org) => format!("https://github.com/organizations/{org}/settings/apps/new?state={state}"),
        None => format!("https://github.com/settings/apps/new?state={state}"),
    }
}

pub fn install_url(slug: &str) -> String {
    format!("https://github.com/apps/{slug}/installations/new")
}

// Where the app is deleted.
pub fn advanced_url(org: Option<&str>, slug: &str) -> String {
    match org {
        Some(org) => format!("https://github.com/organizations/{org}/settings/apps/{slug}/advanced"),
        None => format!("https://github.com/settings/apps/{slug}/advanced"),
    }
}

// A slug as GitHub makes one, lowercase letters, digits and hyphens, since it goes into a URL
// path and the co-author line.
pub fn plain_slug(slug: &str) -> bool {
    !slug.is_empty() && !slug.starts_with('-') && slug.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

// A code as GitHub sends one back, since it goes into a URL path: anything else, from
// whatever reached the localhost page, is never sent on.
pub fn plain_code(code: &str) -> bool {
    (1..=100).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

// The app GitHub made from the manifest, and the account it is under, its owner's login.
pub struct Created {
    pub id: u64,
    pub slug: String,
    pub owner: String,
    pub key: String,
    pub webhook_secret: String,
}

// The code exchanged for the app GitHub made, tried again while GitHub can't be reached: a
// code is used once, so a retry after a lost answer is refused rather than made twice.
// GitHub's text is never quoted.
pub fn convert(agent: &Agent, code: &str) -> Result<Created, Error> {
    if !plain_code(code) {
        return Err(Error::Failed("GitHub's code had another shape".into()));
    }
    let url = format!("{API}/app-manifests/{code}/conversions");
    let (status, text) = retried(|| call_public(agent, Method::POST, &url, &HEADERS).map_err(Error::from))?;
    if status != 201 {
        return Err(Error::Failed(format!("GitHub answered HTTP {status} for the app's code: it works once, within an hour")));
    }
    created(serde_json::from_str(&text).unwrap_or(Value::Null))
}

// The conversion's answer, its key and webhook secret held as `Created` holds them: neither
// printed nor Debug.
#[derive(Deserialize)]
struct Conversion {
    id: u64,
    slug: String,
    owner: Login,
    pem: String,
    webhook_secret: String,
}

#[derive(Deserialize)]
struct Login {
    login: String,
}

pub fn created(data: Value) -> Result<Created, Error> {
    let made: Conversion = decode(data, "GitHub's answer to POST /app-manifests/{code}/conversions")?;
    let whole = plain_slug(&made.slug) && !made.owner.login.is_empty() && !made.pem.trim().is_empty() && !made.webhook_secret.trim().is_empty();
    if !whole {
        return Err(Error::Failed("GitHub's new app had another shape".into()));
    }
    Ok(Created { id: made.id, slug: made.slug, owner: made.owner.login, key: made.pem, webhook_secret: made.webhook_secret })
}

// The app's bot user's ID, which its co-author address carries.
pub fn bot_id(agent: &Agent, slug: &str) -> Result<u64, Error> {
    if !plain_slug(slug) {
        return Err(Error::Failed("the app's slug had another shape".into()));
    }
    let (status, text) = call_public(agent, Method::GET, &format!("{API}/users/{slug}%5Bbot%5D"), &HEADERS)?;
    if status != 200 {
        return Err(Error::Failed(format!("the app's bot user couldn't be read: GitHub answered HTTP {status}")));
    }
    user_id(&text)
}

fn user_id(text: &str) -> Result<u64, Error> {
    #[derive(Deserialize)]
    struct User {
        id: u64,
    }
    let user: User = decode(serde_json::from_str(text).unwrap_or(Value::Null), "GitHub's answer to GET /users/{slug}[bot]")?;
    Ok(user.id)
}

// The co-author line for the agent `name` and the app's bot (docs/co-author.md).
pub fn co_author(name: &str, bot_id: u64, slug: &str) -> String {
    format!("{name} <{bot_id}+{slug}[bot]@users.noreply.github.com>")
}

// Whether a co-author line credits the app's bot, whatever name it gives.
pub fn credits(co_author: &str, bot_id: u64, slug: &str) -> bool {
    co_author.ends_with(&format!(" <{bot_id}+{slug}[bot]@users.noreply.github.com>"))
}

#[cfg(test)]
mod tests;
