// `tolkster github app create`: the GitHub App made by GitHub's manifest flow (docs/setup.md,
// step 4). A localhost page posts the manifest; GitHub sends the owner back to it with a code,
// exchanged for the app's ID, key and webhook secret, which become the Worker's secrets in
// one request, so the Worker never holds part of an app. The installation's ID is set once
// the owner installs it, and `owner.coAuthor` is written from the app's bot. Safe to run again: an
// app whose ID and key the Worker holds is found, never made a second time, and only what
// isn't this app's yet is done.
//
// The key and the webhook secret go from GitHub's answer to wrangler's stdin alone: never to
// disk, output, an argument or the environment.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{Map, Value, json};
use ureq::Agent;

use crate::args::{Kind, parse};
use crate::cli::{config_file, merge_json_file, open_browser, print_exit};
use crate::coauthor::{self, hook_command};
use crate::config::{Files, WorkerUrl, files, worker_url};
use crate::error::{Context, Error};
use crate::exec::run_command;
use crate::github::manifest::{Created, advanced_url, bot_id, co_author, convert, credits, install_url, manifest, new_app_url, plain_code, plain_slug};
use crate::github::{AppRecord, app_record};
use crate::plain::split_repo;
use crate::sessions::workspace::viewer;
use crate::trade;
use crate::wrangler::{CONFIG, put_secrets};

pub const OPTIONS: &[(&str, Kind)] = &[("name", Kind::String), ("org", Kind::String)];

// How long GitHub's code lives, and so how long the page waits for it.
const CODE_LIFE: Duration = Duration::from_secs(60 * 60);
// How long the owner has to install the app, and how often the Worker is asked meanwhile. For
// SECRETS_LIVE after they are set the Worker may not serve its new secrets yet, so any failed
// ask is asked again; after it, only an unavailable one is.
const INSTALL_WAIT: Duration = Duration::from_secs(15 * 60);
const INSTALL_POLL: Duration = Duration::from_secs(5);
const SECRETS_LIVE: Duration = Duration::from_secs(60);

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, OPTIONS, false)?;
    let org = parsed.non_empty("org");
    if org.is_some_and(|org| split_repo(&format!("{org}/x")).is_none()) {
        return Err(Error::Usage("--org must be a GitHub organization's login".into()));
    }
    // wrangler runs from here with the key on its stdin: never a worker's branch's.
    if !Path::new(CONFIG).is_file() || coauthor::marked(&mut run_command, None) {
        return Err(Error::Usage(format!("run it from the checkout the Worker deploys from, where {CONFIG} is, never a worker's worktree")));
    }
    let files = files()?;
    let worker = worker_url(&files)?;
    let missing = trade::missing(agent, &files)?;
    let lacks = |name: &str| missing.iter().any(|m| m == name);
    let (slug, installed) = match lacks("GITHUB_APP_ID") || lacks("GITHUB_APP_KEY") {
        false => {
            let record = app_record(agent, &files)?;
            (record_slug(&record)?, Some(installed(&record)?))
        }
        true => {
            let name = parsed.non_empty("name").ok_or_else(|| Error::Usage("--name is needed to make the app: a name unused across GitHub".into()))?;
            let app = create(agent, name, org, &worker)?;
            let secrets = [("GITHUB_APP_ID", app.id.to_string()), ("GITHUB_WEBHOOK_SECRET", app.webhook_secret), ("GITHUB_APP_KEY", app.key)];
            put_secrets(Path::new("."), &secrets).map_err(|error| orphaned(error, org, &app.slug))?;
            (app.slug, None)
        }
    };
    let created = installed.is_none();
    let installation = match installed {
        Some(true) => None,
        _ => Some(install(agent, &files, &slug)?),
    };
    let wrote = write_co_author(agent, &files, &slug)?;
    print_exit(&json!({ "slug": slug, "created": created, "coAuthor": wrote, "installation": installation }), if created || wrote || installation.is_some() { 0 } else { 3 })
}

fn orphaned(error: Error, org: Option<&str>, slug: &str) -> Error {
    Error::Failed(format!("{error}. GitHub made the app {slug}, but the Worker doesn't hold it: delete it at {}, then run this again", advanced_url(org, slug)))
}

fn record_slug(record: &AppRecord) -> Result<String, Error> {
    record.slug.clone().filter(|s| plain_slug(s)).ok_or_else(|| Error::Failed("the Worker's record of the GitHub App had no slug".into()))
}

// Whether the Worker's GITHUB_INSTALLATION_ID is one of this app's installations.
pub fn installed(record: &AppRecord) -> Result<bool, Error> {
    record.installed.ok_or_else(older)
}

pub fn installations(record: &AppRecord) -> Result<Vec<u64>, Error> {
    record.installations.clone().ok_or_else(older)
}

fn older() -> Error {
    Error::Failed("the Worker's record of the GitHub App lacks its installations: deploy the Worker from this checkout first".into())
}

// `owner.coAuthor` from the app's bot, as the agent's name in Linear, unless it credits that bot
// already: by its ID too, as a slug freed by a deleted app is another bot's. Whether
// config.json changed.
fn write_co_author(agent: &Agent, files: &Files, slug: &str) -> Result<bool, Error> {
    let bot = bot_id(agent, slug)?;
    if files.string("/owner/coAuthor")?.is_some_and(|line| credits(&line, bot, slug)) {
        return Ok(false);
    }
    let viewer = viewer(agent)?;
    let line = co_author(&viewer.name, bot, slug);
    hook_command(&line)?;
    merge_json_file(&config_file()?, false, |current| {
        let mut owner = current.get("owner").and_then(Value::as_object).cloned().unwrap_or_default();
        owner.insert("coAuthor".into(), Value::String(line));
        Map::from_iter([("owner".into(), Value::Object(owner))])
    })
}

// The installation's ID, set as the Worker's secret, once the app has exactly one.
fn install(agent: &Agent, files: &Files, slug: &str) -> Result<u64, Error> {
    let url = install_url(slug);
    let started = Instant::now();
    let mut asked = false;
    loop {
        let ids = match app_record(agent, files) {
            Ok(record) => Ok(installations(&record)?),
            Err(error) if error.unavailable() || started.elapsed() < SECRETS_LIVE => Err(error),
            Err(error) => return Err(error),
        };
        match ids {
            Ok(ids) if ids.len() == 1 => {
                put_secrets(Path::new("."), &[("GITHUB_INSTALLATION_ID", ids[0].to_string())])?;
                return Ok(ids[0]);
            }
            Ok(ids) if ids.len() > 1 => return Err(Error::Failed("the app is installed on more than one account: set GITHUB_INSTALLATION_ID to the one with the repos, with npx wrangler secret put".into())),
            _ if started.elapsed() < INSTALL_WAIT => {}
            Ok(_) => return Err(Error::Failed(format!("the app isn't installed yet: install it at {url}, then run this again"))),
            Err(error) => return Err(error),
        }
        if !asked {
            eprintln!("Install the app on the repos the agent works in: {url}");
            open_browser(&url);
            asked = true;
        }
        std::thread::sleep(INSTALL_POLL);
    }
}

// Serves the page that posts the manifest, until GitHub sends the owner back with a code
// carrying this run's state that is exchanged for an app under `org`, or the code's life is
// over. A failed exchange keeps the page listening: a forged return can't end the wait for
// GitHub's.
fn create(agent: &Agent, name: &str, org: Option<&str>, worker: &WorkerUrl) -> Result<Created, Error> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).context("listening for GitHub's return")?;
    listener.set_nonblocking(true).context("listening for GitHub's return")?;
    let authority = format!("127.0.0.1:{}", listener.local_addr().context("listening for GitHub's return")?.port());
    let state = random_state()?;
    let manifest = manifest(name, &worker.base(), &worker.github_webhook(), &format!("http://{authority}/created"));
    let action = new_app_url(org, &state);
    eprintln!("Make the GitHub App at http://{authority}/ (opening it in your browser), then come back here.");
    open_browser(&format!("http://{authority}/"));
    let deadline = Instant::now() + CODE_LIFE;
    let mut posted = false;
    loop {
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(_) if Instant::now() >= deadline => return Err(Error::Failed("GitHub sent no app back within the hour: run this again".into())),
            // None waiting, or a failure accepting one, which passes.
            Err(_) => {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
        };
        let Some((target, host)) = request(&mut stream) else { continue };
        // A page elsewhere that rebinds a name to 127.0.0.1 sends its own name.
        if host != authority {
            respond(&mut stream, "400 Bad Request", "");
            continue;
        }
        match route(&target, &state) {
            Route::Page => {
                respond(&mut stream, "200 OK", &page(&manifest, &action, !posted));
                posted = true;
            }
            Route::Created(code) => match convert(agent, &code).and_then(|app| under(app, org)) {
                Ok(app) => {
                    respond(&mut stream, "200 OK", "<p>The GitHub App is made. Back to the terminal.</p>");
                    return Ok(app);
                }
                Err(error) => {
                    respond(&mut stream, "200 OK", "<p>GitHub didn't hand over the app. Back to the terminal.</p>");
                    eprintln!("{error}. If GitHub made the app, delete it in its settings' Advanced tab. Still waiting for GitHub; Ctrl-C to stop.");
                }
            },
            Route::Refused => respond(&mut stream, "400 Bad Request", "<p>Not this run's.</p>"),
            Route::NotFound => respond(&mut stream, "404 Not Found", ""),
        }
    }
}

// The app, if it is under `org`, the account the owner asked for.
fn under(app: Created, org: Option<&str>) -> Result<Created, Error> {
    match org {
        Some(org) if !app.owner.eq_ignore_ascii_case(org) => Err(Error::Failed(format!("the app {} GitHub handed over isn't under {org}", app.slug))),
        _ => Ok(app),
    }
}

#[derive(Debug, PartialEq)]
pub enum Route {
    Page,
    Created(String),
    Refused,
    NotFound,
}

// What a GET's target asks for. GitHub's return counts only with this run's state and a plain
// code: anything else that reaches the port is refused.
pub fn route(target: &str, state: &str) -> Route {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    match path {
        "/" => Route::Page,
        "/created" => {
            let param = |key: &str| query.split('&').find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='));
            match (param("code"), param("state")) {
                (Some(code), Some(given)) if given == state && plain_code(code) => Route::Created(code.into()),
                _ => Route::Refused,
            }
        }
        _ => Route::NotFound,
    }
}

// The page: a form holding the manifest, posted to GitHub as it first loads. Loaded again, it
// waits for a click, as each post makes another app.
pub fn page(manifest: &Value, action: &str, submit: bool) -> String {
    format!(
        r#"<!doctype html><meta charset="utf-8"><title>The GitHub App</title><form method="post" action="{}"><input type="hidden" name="manifest" value="{}"><button>Make the GitHub App</button></form>{}"#,
        escape(action),
        escape(&manifest.to_string()),
        if submit { "<script>document.forms[0].submit()</script>" } else { "" }
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('"', "&quot;").replace('\'', "&#39;").replace('<', "&lt;").replace('>', "&gt;")
}

// A GET's target and its Host header, from a request read within a few seconds; None for
// anything else.
fn request(stream: &mut TcpStream) -> Option<(String, String)> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut buffer = [0u8; 4096];
    let read = stream.read(&mut buffer).ok()?;
    parse_request(&String::from_utf8_lossy(&buffer[..read]))
}

pub fn parse_request(head: &str) -> Option<(String, String)> {
    let mut lines = head.lines();
    let mut words = lines.next()?.split(' ');
    let target = match (words.next(), words.next()) {
        (Some("GET"), Some(target)) => target,
        _ => return None,
    };
    let host = lines.find_map(|line| line.split_once(':').filter(|(name, _)| name.eq_ignore_ascii_case("host")).map(|(_, value)| value.trim()))?;
    Some((target.into(), host.into()))
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}", body.len());
}

fn random_state() -> Result<String, Error> {
    let mut bytes = [0u8; 16];
    SystemRandom::new().fill(&mut bytes).map_err(|_| Error::Failed("no random state for GitHub's return".into()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests;
