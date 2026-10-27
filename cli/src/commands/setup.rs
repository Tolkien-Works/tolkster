// `tolkster setup`: docs/setup.md run in order on the owner's terminal, from Cloudflare to the
// first team. Each step checks first whether it is done and skips it, so a run stopped anywhere
// finishes when run again, and a rerun changes nothing that is set. What only the owner can do,
// Linear's OAuth app and each approval, it prints with the values to paste, and waits for.
//
// The Worker it deploys is this build's own commit, so the Worker and the CLI keep one
// contract: from the checkout it runs in when that is at the commit, else from setup's own
// clone of the build's repo at it, where `github app create` then runs too. A Worker that
// exists is never deployed again, which could put an older one over CI's.
//
// A secret typed in is read with no echo and goes to wrangler's stdin and, for the CLI's own, to
// credentials.json, never to output. OWNER_USER_ID names the relay's Durable Object, so it is
// set only while the Worker lacks it, never changed.

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Serialize;
use serde_json::{Map, Value, json};
use ureq::Agent;
use ureq::http::Method;

use crate::args::{Kind, Parsed, parse};
use crate::cli::{self, config_file, credentials_file, home, merge_json_file, open_browser, print_json};
use crate::commands::init::shown;
use crate::commands::members;
use crate::commands::repo::refuse_own_agent;
use crate::config::{self, Files, WorkerUrl, files, worker_url};
use crate::error::{Context, Error};
use crate::exec::run_command;
use crate::git;
use crate::http::call_public;
use crate::linear::{create_release_pipeline, create_url, install_url, with_token};
use crate::sessions::workspace::{self, User};
use crate::plain::{Id, Sha};
use crate::prebuilt;
use crate::trade;
use crate::wrangler::{self, CONFIG};

pub const OPTIONS: &[(&str, Kind)] = &[("team", Kind::String), ("agent-name", Kind::String), ("app-name", Kind::String), ("org", Kind::String)];

// How long secrets just set, or an install just approved, may take to show, and how often
// they're asked after meanwhile.
const SETTLE: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Outcome {
    // Done by this run.
    Done,
    // Found done.
    Set,
    Failed,
}

#[derive(Serialize)]
struct Step {
    step: &'static str,
    outcome: Outcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

type Done = (Outcome, Option<String>);

// The steps taken, and what is left to the owner.
#[derive(Default)]
struct Steps {
    taken: Vec<Step>,
    next: Vec<String>,
}

impl Steps {
    // A failure ends the run: each step needs the ones before it.
    fn run(&mut self, step: &'static str, act: impl FnOnce(&mut Vec<String>) -> Result<Done, Error>) -> Result<(), Error> {
        eprintln!("\n== {step}");
        let (outcome, detail, result) = match act(&mut self.next) {
            Ok((outcome, detail)) => (outcome, detail, Ok(())),
            Err(error) => (Outcome::Failed, Some(error.to_string()), Err(error)),
        };
        eprintln!("{step}: {}", detail.as_deref().unwrap_or(match outcome {
            Outcome::Done => "done",
            Outcome::Set => "already set",
            Outcome::Failed => "failed",
        }));
        self.taken.push(Step { step, outcome, detail });
        result
    }
}

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, OPTIONS, false)?;
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return Err(Error::Usage("setup asks on a terminal: run it in one".into()));
    }
    refuse_own_agent()?;
    let mut steps = Steps::default();
    let ok = setup(agent, &parsed, &mut steps).is_ok();
    print_json(&json!({ "ok": ok, "steps": steps.taken, "next": steps.next }))?;
    Ok(if ok { 0 } else { 1 })
}

fn setup(agent: &Agent, parsed: &Parsed, steps: &mut Steps) -> Result<(), Error> {
    steps.run("prerequisites", |_| prerequisites())?;
    let mut dir = PathBuf::from(".");
    steps.run("checkout", |_| checkout(&mut dir))?;
    steps.run("cloudflare", |_| cloudflare(&dir))?;
    steps.run("worker", |_| deploy(agent, &dir))?;
    steps.run("linear app", |next| linear_app(&dir, parsed, next))?;
    steps.run("secrets", |_| secrets(&dir))?;
    steps.run("linear install", |_| linear_install(agent))?;
    steps.run("owner", |_| owner(agent, &dir))?;
    steps.run("github app", |_| github_app(agent, &dir, parsed))?;
    steps.next.push(match cfg!(feature = "develop") {
        true => "keep tolkster listen running in a terminal, from this checkout on main".into(),
        false => "keep tolkster listen running in a terminal".into(),
    });
    let mut key = String::new();
    steps.run("team", |next| {
        let done = team(parsed, next)?;
        key = done.1.clone().unwrap_or_default();
        Ok(done)
    })?;
    // Last, as nothing needs it, and a workspace whose plan lacks releases fails only here.
    if cfg!(feature = "develop") {
        steps.run("release pipeline", |_| release_pipeline(agent, &key))?;
    }
    Ok(())
}

fn prerequisites() -> Result<Done, Error> {
    // 1 is no hook listed; a git without `hook list` exits 129.
    let git = run_command("git", &["hook", "list", "pre-commit"], None);
    if !git.is_ok_and(|output| output.code.is_some_and(|code| code <= 1)) {
        return Err(Error::Failed("git can't run hooks from config (git hook list pre-commit fails): install a newer git".into()));
    }
    let node = run_command("node", &["--version"], None).ok().filter(|output| output.ok).and_then(|output| node_major(&output.stdout));
    match node {
        Some(major) if major >= 24 => Ok((Outcome::Set, None)),
        _ => Err(Error::Failed("wrangler needs Node 24 or later: install it".into())),
    }
}

pub fn node_major(version: &str) -> Option<u32> {
    version.trim().strip_prefix('v')?.split('.').next()?.parse().ok()
}

// Where the Worker deploys from.
#[derive(Debug, PartialEq)]
pub enum Source {
    Here,
    Clone { repo: String, commit: Sha },
    Nowhere,
}

// The checkout this runs in when it is at the build's commit, or the build has none (a
// debug build), or is `develop`'s, whose `listen` keeps that checkout and the Worker on main;
// else a clone of the repo the build follows.
pub fn source(commit: Option<&Sha>, here: Option<&Sha>, repo: Option<&str>, develop: bool) -> Source {
    match (commit, here, repo) {
        (_, Some(_), _) if develop => Source::Here,
        (None, Some(_), _) => Source::Here,
        (Some(commit), Some(here), _) if commit == here => Source::Here,
        (Some(commit), _, Some(repo)) => Source::Clone { repo: repo.into(), commit: commit.clone() },
        _ => Source::Nowhere,
    }
}

fn checkout(dir: &mut PathBuf) -> Result<Done, Error> {
    // Tolkster's checkout holds the Worker's config and the CLI both.
    let ours = Path::new(CONFIG).is_file() && Path::new("cli/Cargo.toml").is_file();
    let here = ours.then(|| git::head(&mut run_command, ".").ok()).flatten();
    match source(prebuilt::commit().as_ref(), here.as_ref(), option_env!("TOLKSTER_REPO"), cfg!(feature = "develop")) {
        Source::Here => {
            let installed = wrangler::installed(dir);
            if !installed {
                wrangler::install(dir)?;
            }
            Ok((if installed { Outcome::Set } else { Outcome::Done }, Some("this checkout".into())))
        }
        Source::Clone { repo, commit } => {
            *dir = home()?.join("worker");
            let moved = clone(dir, &repo, &commit)?;
            if moved || !wrangler::installed(dir) {
                wrangler::install(dir)?;
            }
            Ok((if moved { Outcome::Done } else { Outcome::Set }, Some(format!("{} at {}", dir.display(), crate::plain::short_sha(&commit)))))
        }
        Source::Nowhere => Err(Error::Usage("this build doesn't say which Worker is its own: run setup from a checkout of Tolkster at the commit it was built from".into())),
    }
}

// Setup's own clone at `commit`, whatever it was on before; whether it moved.
fn clone(dir: &Path, repo: &str, commit: &Sha) -> Result<bool, Error> {
    let path = dir.to_string_lossy();
    let mut moved = false;
    if !dir.join(".git").exists() {
        git::clone_with(&[], &format!("https://github.com/{repo}.git"), &path)?;
        moved = true;
    }
    if git::head(&mut run_command, &path).ok().as_ref() != Some(commit) {
        git::fetch(&mut run_command, &path)?;
        git::detach(&mut run_command, &path, commit).map_err(|_| Error::Failed(format!("{} has no commit {}", dir.display(), commit.as_str())))?;
        moved = true;
    }
    Ok(moved)
}

fn cloudflare(dir: &Path) -> Result<Done, Error> {
    let (whoami, outcome) = match wrangler::whoami(dir)? {
        Some(whoami) => (whoami, Outcome::Set),
        None => {
            wrangler::login(dir)?;
            (wrangler::whoami(dir)?.ok_or_else(|| Error::Failed("wrangler isn't signed in".into()))?, Outcome::Done)
        }
    };
    let accounts = accounts(&whoami);
    if accounts.len() > 1 && std::env::var_os("CLOUDFLARE_ACCOUNT_ID").is_none() {
        return Err(Error::Failed(format!("wrangler reaches more than one Cloudflare account ({}): set CLOUDFLARE_ACCOUNT_ID to the one to deploy to", accounts.join(", "))));
    }
    Ok((outcome, Some(format!("account {}: check it is the one you mean", accounts.join(", ")))))
}

// Each account wrangler reaches, as its name and ID.
pub fn accounts(whoami: &Value) -> Vec<String> {
    let name = |account: &Value| Some(format!("{} ({})", account["name"].as_str()?, account["id"].as_str()?));
    whoami["accounts"].as_array().into_iter().flatten().filter_map(name).collect()
}

// Only a Worker that doesn't exist yet: an existing one may be newer than this build, as CI
// deploys each push to main, and an older one would reset the Durable Object's storage.
fn deploy(agent: &Agent, dir: &Path) -> Result<Done, Error> {
    let worker = worker_url(&files()?).ok();
    if let Some(worker) = &worker
        && answers(agent, worker)
    {
        return Ok((Outcome::Set, Some(worker.base())));
    }
    if wrangler::exists(dir)? {
        return Err(Error::Failed(match worker {
            Some(worker) => format!("the Worker is deployed but doesn't answer at {}: check \"worker\" in config.json, then run setup again", worker.base()),
            None => "the Worker is deployed already: add its address to config.json as \"worker\"".into(),
        }));
    }
    let url = wrangler::deploy(dir)?.ok_or_else(|| Error::Failed("wrangler printed no workers.dev address: add the Worker's address to config.json as \"worker\"".into()))?;
    let worker = WorkerUrl::parse(&url)?;
    merge_json_file(&config_file()?, false, |_| Map::from_iter([("worker".into(), Value::String(worker.base()))]))?;
    Ok((Outcome::Done, Some(worker.base())))
}

// Whether a Tolkster Worker answers there, asked three times: the page Linear's install
// returns to.
fn answers(agent: &Agent, worker: &WorkerUrl) -> bool {
    (0..3).any(|n| {
        if n > 0 {
            std::thread::sleep(POLL);
        }
        matches!(call_public(agent, Method::GET, &worker.oauth_callback(), &[]), Ok((200, _)))
    })
}

// The Linear OAuth app, which only an admin can make: its create page opened with the manifest
// filled in, and what it hands over set on the Worker, over any it holds, so an app made again
// and typed in here reaches both ends; its client ID and secret are then kept in credentials.json,
// as the realtime secret is (`Origin::Made`). Linear shows the secrets on its page alone, so they
// are pasted.
fn linear_app(dir: &Path, parsed: &Parsed, next: &mut Vec<String>) -> Result<Done, Error> {
    let files = files()?;
    let held = held(&files);
    let text = |key: &str| held.get(key).and_then(Value::as_str).filter(|value| !value.is_empty()).map(String::from);
    let (Some(id), Some(_)) = (text("clientId"), text("clientSecret")) else {
        let worker = worker_url(&files)?;
        let name = match parsed.non_empty("agent-name") {
            Some(name) => name.to_string(),
            None => ask("The agent's name, as people will see it on the Linear app")?,
        };
        let url = create_url(&agent_name(&name)?, &worker);
        eprintln!("{}", app_settings(&url, &worker));
        open_browser(&url);
        next.push("setup asked you to paste the Linear app's client secret and webhook signing secret from its page, the only place Linear shows them".into());
        let id = required(ask("The app's client ID")?, "client ID")?;
        let secret = required(hidden("The app's client secret")?, "client secret")?;
        let webhook = required(hidden("The app's webhook signing secret")?, "webhook signing secret")?;
        wrangler::put_secrets(dir, &[("LINEAR_CLIENT_ID", id.clone()), ("LINEAR_CLIENT_SECRET", secret.clone()), ("LINEAR_WEBHOOK_SECRET", webhook)])?;
        merge_json_file(&credentials_file()?, true, |_| Map::from_iter([("clientId".into(), Value::String(id)), ("clientSecret".into(), Value::String(secret))]))?;
        return Ok((Outcome::Done, None));
    };
    if !wrangler::secret_names(dir)?.iter().any(|name| name == "LINEAR_WEBHOOK_SECRET") {
        eprintln!("The webhook signing secret is the Linear app's with client ID {id}.");
    }
    Ok((Outcome::Set, None))
}

// The agent's name as Linear takes it for an app: two to eighty characters, with no "Linear" and
// no URL, and no control character, which would reach the terminal.
pub fn agent_name(name: &str) -> Result<String, Error> {
    let name = name.trim();
    let lower = name.to_lowercase();
    let length = name.chars().count();
    if !(2..=80).contains(&length) || lower.contains("linear") || lower.contains("http://") || lower.contains("https://") || name.chars().any(char::is_control) {
        return Err(Error::Usage("the agent's name is two to eighty characters, with no \"Linear\" or URL in it".into()));
    }
    Ok(name.into())
}

pub fn app_settings(create_url: &str, worker: &WorkerUrl) -> String {
    format!(
        "Make the Linear app, as a workspace admin: open {create_url}\n  Its form is filled in; create it, and give it the agent's avatar.\n  If the form comes up empty, fill it in: callback URL {}, webhook URL {} with Agent session events, Issues and Comments, client credentials on.\nDon't install it yet. Paste its client ID, client secret and webhook signing secret here.",
        worker.oauth_callback(),
        worker.linear_webhook()
    )
}

// credentials.json as the CLI reads it, the environment's values first (`config::realtime`,
// `token::credentials`).
fn held(files: &Files) -> Value {
    let mut held = files.credentials.as_object().cloned().unwrap_or_default();
    if let Some(secret) = cli::env("TOLKSTER_REALTIME_SECRET") {
        held.insert("realtimeSecret".into(), Value::String(secret));
    }
    if let (Some(id), Some(secret)) = (cli::env("TOLKSTER_CLIENT_ID"), cli::env("TOLKSTER_CLIENT_SECRET")) {
        held.insert("clientId".into(), Value::String(id));
        held.insert("clientSecret".into(), Value::String(secret));
    }
    Value::Object(held)
}

fn required(text: String, what: &str) -> Result<String, Error> {
    if text.is_empty() { Err(Error::Failed(format!("no {what} given"))) } else { Ok(text) }
}

// Where a secret the Worker lacks comes from.
#[derive(Debug, PartialEq)]
pub enum Origin {
    // credentials.json's, by its key.
    Credentials(&'static str),
    // REALTIME_SECRET, made here, set on the Worker alone and only then kept in
    // credentials.json: a run that stops between leaves credentials.json without it, and the
    // next makes another.
    Made,
    // Typed in, with its prompt, and whether Enter skips it.
    Typed(&'static str, bool),
}

// The Worker's secrets to set, from the names it holds and credentials.json. A realtime secret
// credentials.json lacks is made afresh, as the Worker's value can't be read back.
pub fn wanted(listed: &[String], credentials: &Value) -> Vec<(&'static str, Origin)> {
    let lacks = |name: &str| !listed.iter().any(|held| held == name);
    let holds = |key: &str| credentials.get(key).and_then(Value::as_str).is_some_and(|value| !value.is_empty());
    let mut wanted = Vec::new();
    match holds("realtimeSecret") {
        true if lacks("REALTIME_SECRET") => wanted.push(("REALTIME_SECRET", Origin::Credentials("realtimeSecret"))),
        true => {}
        false => wanted.push(("REALTIME_SECRET", Origin::Made)),
    }
    for (name, key) in [("LINEAR_CLIENT_ID", "clientId"), ("LINEAR_CLIENT_SECRET", "clientSecret")] {
        if lacks(name) && holds(key) {
            wanted.push((name, Origin::Credentials(key)));
        }
    }
    if lacks("LINEAR_WEBHOOK_SECRET") {
        wanted.push(("LINEAR_WEBHOOK_SECRET", Origin::Typed("The Linear app's webhook signing secret", false)));
    }
    if lacks("CLAUDE_TOKEN") {
        wanted.push(("CLAUDE_TOKEN", Origin::Typed("A Claude token for worker hosts, from claude setup-token (Enter to skip)", true)));
    }
    wanted
}

fn secrets(dir: &Path) -> Result<Done, Error> {
    let held = held(&files()?);
    let wanted = wanted(&wrangler::secret_names(dir)?, &held);
    let mut secrets = Vec::new();
    let mut made = Vec::new();
    let mut skipped = Vec::new();
    for (name, origin) in wanted {
        let value = match origin {
            Origin::Credentials(key) => held[key].as_str().unwrap_or_default().to_string(),
            Origin::Made => {
                let secret = random_secret()?;
                wrangler::put_secrets(dir, &[(name, secret.clone())])?;
                merge_json_file(&credentials_file()?, true, |_| Map::from_iter([("realtimeSecret".into(), Value::String(secret))]))?;
                made.push(name);
                continue;
            }
            Origin::Typed(prompt, optional) => match hidden(prompt)? {
                typed if typed.is_empty() && optional => {
                    skipped.push(name);
                    continue;
                }
                typed => required(typed, name)?,
            },
        };
        secrets.push((name, value));
    }
    if !secrets.is_empty() {
        wrangler::put_secrets(dir, &secrets)?;
    }
    let set = made.into_iter().chain(secrets.iter().map(|(name, _)| *name)).collect::<Vec<_>>().join(", ");
    if set.is_empty() {
        return Ok((Outcome::Set, None));
    }
    Ok((Outcome::Done, Some(match skipped.is_empty() {
        true => format!("set {set}"),
        false => format!("set {set}; skipped {}", skipped.join(", ")),
    })))
}

fn random_secret() -> Result<String, Error> {
    let mut bytes = [0u8; 32];
    SystemRandom::new().fill(&mut bytes).map_err(|_| Error::Failed("no randomness for the realtime secret".into()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

// Installed once the app user can see a team: an install grants it each team it works in.
fn linear_install(agent: &Agent) -> Result<Done, Error> {
    if let Ok(name) = app_user(agent) {
        return Ok((Outcome::Set, Some(name)));
    }
    let files = files()?;
    let client_id = held(&files)["clientId"].as_str().map(String::from).ok_or_else(|| Error::Failed("credentials.json has no clientId".into()))?;
    let url = install_url(&client_id, &worker_url(&files)?);
    eprintln!("Install the Linear app, granting it the teams it works in: {url}");
    open_browser(&url);
    ask("Press Enter once it is approved")?;
    let started = Instant::now();
    loop {
        match app_user(agent) {
            Ok(name) => return Ok((Outcome::Done, Some(name))),
            Err(_) if started.elapsed() < SETTLE => std::thread::sleep(POLL),
            Err(_) => return Err(Error::Failed(format!("the Linear app isn't installed, or its client ID or secret is wrong: approve it at {url}"))),
        }
    }
}

// The app user's name, once it can see a team.
fn app_user(agent: &Agent) -> Result<String, Error> {
    match workspace::installed(agent)? {
        (name, true) => Ok(name),
        _ => Err(Error::Failed("the app sees no team".into())),
    }
}

// The owner, as every check of the CLI's reads it, and the Worker's OWNER_USER_ID, the owner
// picked from the workspace's people only while neither end has one: the Worker's can't be
// read back, so a pick beside it could name someone else.
fn owner(agent: &Agent, dir: &Path) -> Result<Done, Error> {
    let files = files()?;
    let lacks = !wrangler::secret_names(dir)?.iter().any(|name| name == "OWNER_USER_ID");
    let id = match config::owner(&files) {
        Ok(id) => id,
        Err(_) if lacks => pick_owner(agent)?,
        Err(_) => return Err(Error::Failed("the Worker's OWNER_USER_ID is set, and can't be read back: add that ID to config.json as \"owner.id\" (tolkster members lists the IDs)".into())),
    };
    if !lacks {
        return Ok((Outcome::Set, Some(id)));
    }
    wrangler::put_secrets(dir, &[("OWNER_USER_ID", id.clone())])?;
    Ok((Outcome::Done, Some(id)))
}

// The owner's ID, written as `owner.id` with their first name as `owner.name` unless one is set.
// Never the app's own user. Names are Linear's, so shown plain (`init::shown`).
fn pick_owner(agent: &Agent) -> Result<String, Error> {
    let app = workspace::viewer(agent)?.id;
    let people: Vec<User> = members::workspace(agent)?.into_iter().filter(|user| !user.guest && user.id != app).collect();
    for (n, user) in people.iter().enumerate() {
        eprintln!("  {}. {} ({})", n + 1, shown(&user.name), shown(&user.display_name));
    }
    let proposal = proposal(&people);
    let prompt = match proposal {
        Some(n) => format!("Which of these are you? [{}]", n + 1),
        None => "Which of these are you?".into(),
    };
    let answer = ask(&prompt)?;
    let chosen = match answer.as_str() {
        "" => proposal,
        number => number.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).filter(|&n| n < people.len()),
    };
    let user = &people[chosen.ok_or_else(|| Error::Failed("no one picked as the owner".into()))?];
    let id = user.id.clone();
    let first = shown(&user.name).split_whitespace().next().map(String::from);
    merge_json_file(&config_file()?, false, |current| {
        let mut owner = current.get("owner").and_then(Value::as_object).cloned().unwrap_or_default();
        owner.insert("id".into(), Value::String(id.clone()));
        if let Some(first) = first.filter(|_| owner.get("name").is_none_or(|name| name.is_null() || name == "")) {
            owner.insert("name".into(), Value::String(first));
        }
        Map::from_iter([("owner".into(), Value::Object(owner))])
    })?;
    Ok(id)
}

// What a development build's rebuild syncs its releases to (docs/realtime.md): the pipeline
// config.json names, else the workspace's named RELEASE_PIPELINE, else a new one on the team
// `team_key` names, written as `releasePipeline`. Linear's release API is in alpha, on some
// plans only.
const RELEASE_PIPELINE: &str = "Tolkster";

fn release_pipeline(agent: &Agent, team_key: &str) -> Result<Done, Error> {
    if let Some(id) = config::release_pipeline(&files()?)? {
        return Ok((Outcome::Set, Some(id)));
    }
    let id = match workspace::release_pipelines(agent)?.into_iter().find(|pipeline| pipeline.name.eq_ignore_ascii_case(RELEASE_PIPELINE)) {
        Some(pipeline) => pipeline.id,
        None => with_token(agent, |token| {
            let team = workspace::team_by_key(agent, token, team_key)?.ok_or_else(|| Error::Failed(format!("no team with key {team_key}")))?;
            create_release_pipeline(agent, token, RELEASE_PIPELINE, &team.id)
        })?,
    };
    let id = Id::check(&id)?.to_string();
    merge_json_file(&config_file()?, false, |_| Map::from_iter([("releasePipeline".into(), Value::String(id.clone()))]))?;
    Ok((Outcome::Done, Some(id)))
}

// The one admin among `people`, who installed the app.
pub fn proposal(people: &[User]) -> Option<usize> {
    let mut admins = people.iter().enumerate().filter(|(_, user)| user.admin);
    match (admins.next(), admins.next()) {
        (Some((n, _)), None) => Some(n),
        _ => None,
    }
}

// `github app create`, in the checkout, with a name asked for only when there's an app to make.
fn github_app(agent: &Agent, dir: &Path, parsed: &Parsed) -> Result<Done, Error> {
    let files = files()?;
    let started = Instant::now();
    let missing = loop {
        match trade::missing(agent, &files) {
            Ok(missing) => break missing,
            Err(_) if started.elapsed() < SETTLE => std::thread::sleep(POLL),
            Err(error) => return Err(error),
        }
    };
    let name = match missing.iter().any(|name| name == "GITHUB_APP_ID" || name == "GITHUB_APP_KEY") {
        true => Some(match parsed.non_empty("app-name") {
            Some(name) => name.to_string(),
            None => required(ask("A name for the GitHub App, unused across GitHub")?, "app name")?,
        }),
        false => None,
    };
    let mut args = vec!["github", "app", "create"];
    if let Some(name) = &name {
        args.extend(["--name", name]);
    }
    if let Some(org) = parsed.non_empty("org") {
        args.extend(["--org", org]);
    }
    let (code, report) = tolkster(dir, &args)?;
    let slug = report["slug"].as_str().map(String::from);
    match code {
        0 => Ok((Outcome::Done, slug)),
        3 => Ok((Outcome::Set, slug)),
        _ => Err(Error::Failed("tolkster github app create failed, as it says above".into())),
    }
}

// `init --team`, whose closing checks are doctor's, and whose steps for the owner are setup's.
fn team(parsed: &Parsed, next: &mut Vec<String>) -> Result<Done, Error> {
    let key = match parsed.non_empty("team") {
        Some(key) => key.to_string(),
        None => required(ask("The key of the first Linear team Tolkster works in, e.g. ENG")?, "team key")?,
    };
    let (code, report) = tolkster(Path::new("."), &["init", "--team", &key])?;
    next.extend(report["steps"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from));
    match code {
        0 => Ok((Outcome::Done, Some(key))),
        _ => Err(Error::Failed(format!("tolkster init --team {key} ended with a fail: its steps are under next, and tolkster doctor --team {key} shows each check"))),
    }
}

// This build run on `args` in `dir`, on this terminal, its exit code and the JSON it printed.
fn tolkster(dir: &Path, args: &[&str]) -> Result<(i32, Value), Error> {
    let output = Command::new(std::env::current_exe().context("finding this tolkster")?).args(args).current_dir(dir).stdin(Stdio::inherit()).stderr(Stdio::inherit()).output().context(format!("running tolkster {}", args.first().unwrap_or(&"")))?;
    Ok((output.status.code().unwrap_or(1), serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)))
}

fn ask(prompt: &str) -> Result<String, Error> {
    eprint!("{prompt}: ");
    io::stderr().flush().context("writing the prompt")?;
    let mut text = String::new();
    match io::stdin().lock().read_line(&mut text).context("reading the answer")? {
        0 => Err(Error::Failed("setup got no answer".into())),
        _ => Ok(text.trim().into()),
    }
}

// A line typed with no echo, for a secret.
fn hidden(prompt: &str) -> Result<String, Error> {
    eprint!("{prompt} (not shown): ");
    io::stderr().flush().context("writing the prompt")?;
    enable_raw_mode().context("hiding the typing")?;
    let typed = keys();
    let _ = disable_raw_mode();
    eprintln!();
    typed
}

fn keys() -> Result<String, Error> {
    let mut text = String::new();
    loop {
        let Event::Key(key) = event::read().context("reading the typing")? else { continue };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        match key.code {
            KeyCode::Enter => return Ok(text.trim().into()),
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Err(Error::Failed("stopped".into())),
            // A chord isn't text: Ctrl-V where the terminal doesn't paste would add a `v`. AltGr
            // comes as both, and types text.
            KeyCode::Char(_) if key.modifiers.contains(KeyModifiers::CONTROL) != key.modifiers.contains(KeyModifiers::ALT) => {}
            KeyCode::Char(c) => text.push(c),
            KeyCode::Backspace => {
                text.pop();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
