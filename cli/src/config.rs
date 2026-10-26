// The settings in ~/.tolkster/config.json (docs/setup.md), read strictly: a key it doesn't
// know exits 2 naming it and what replaced it, so a misspelt or moved setting never leaves a
// default the owner didn't choose.
//
// A display name never decides anything: the owner check is keyed on `owner.id`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::cli::{config_file, credentials_file, env, machines_file, merge_json_file, shown};
use crate::error::{Context, Error};
use crate::estimates::Size;
use ladder::Ladder;
use crate::plain::{Id, plain_host};
use repo_map::{RepoMap, RepoReview};

pub const DEFAULT_OWNER_NAME: &str = "the owner";
// This machine's `slots` entry, and its cap when `slots` sets none.
pub const LOCAL: &str = "local";
pub const DEFAULT_LOCAL_SLOTS: i64 = 6;
pub const DEFAULT_DEEP_AT: Size = Size::M;
pub const DEFAULT_MAX_ROUNDS: i64 = 3;
// How long a repo's recipe machine stays idle before it sleeps (`repos`' `recipe`).
pub const DEFAULT_IDLE_MINUTES: i64 = 30;
// How long one stays asleep, or idle when it never sleeps, before it is destroyed.
pub const DEFAULT_DESTROY_AFTER: i64 = 24 * 60;
pub const DEFAULT_TRIAGE_BATCH: i64 = 5;

// Both files as read, `Value::Null` for one that doesn't exist.
pub struct Files {
    pub config: Value,
    pub credentials: Value,
}

impl Files {
    // A setting by its path in config.json, `/owner/id`. A null or empty value counts as unset.
    pub fn get(&self, path: &str) -> Option<&Value> {
        self.config.pointer(path).filter(|v| !v.is_null() && v.as_str() != Some(""))
    }

    pub fn string(&self, path: &str) -> Result<Option<String>, Error> {
        match self.get(path) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(Error::Usage(format!("\"{}\" in config.json must be a string", dotted(path)))),
        }
    }

    // A secret, from credentials.json only, and only as a non-empty string.
    pub fn secret(&self, name: &str) -> Option<String> {
        self.credentials.get(name).and_then(Value::as_str).filter(|v| !v.is_empty()).map(String::from)
    }
}

// A path as the owner writes it: `/review/deepAt` is `review.deepAt`.
pub fn dotted(path: &str) -> String {
    path.trim_start_matches('/').replace('/', ".")
}

pub(crate) const TOP: &[&str] = &["$schema", "owner", "worker", "review", "triage", "ladder", "repos", "slots", "timing", "releasePipeline"];
pub(crate) const OWNER: &[&str] = &["id", "name", "coAuthor"];
pub(crate) const REVIEW: &[&str] = &["reviewers", "maxRounds", "deepAt"];
pub(crate) const TRIAGE: &[&str] = &["batch"];
// Old keys, each with its replacement, or None for one no longer a setting.
type Moved<'a> = &'a [(&'a str, Option<&'a str>)];

const MOVED: Moved = &[
    ("ownerId", Some("owner.id")),
    ("ownerName", Some("owner.name")),
    ("coAuthor", Some("owner.coAuthor")),
    ("workerUrl", Some("worker")),
    ("reviewers", Some("review.reviewers")),
    ("maxRounds", Some("review.maxRounds")),
    ("deepReviewEstimate", Some("review.deepAt")),
    ("triageBatch", Some("triage.batch")),
    ("maxRunning", Some("slots")),
    ("hostCaps", Some("slots")),
    ("briefsDir", None),
    ("hosts", None),
    // The only record that can destroy a paid machine: moved, never dropped.
    ("hostRecipes", Some("~/.tolkster/machines.json: move its value there as it is")),
    ("host", Some("host in credentials.json: rejoin this host with tolkster host add --rejoin")),
];

// Every key of a setting is one a reader takes, or this exits 2, and a group is an object: read
// through a path, one that isn't would leave every setting in it on its default. `ladder` and
// `repos` are read whole here; the other values where they are read.
pub fn check(config: &Value) -> Result<(), Error> {
    if !config.is_object() && !config.is_null() {
        return Err(Error::Usage("config.json must be a JSON object".into()));
    }
    keys(config, "", TOP, MOVED)?;
    let groups: [(&str, &[&str], Moved); 3] = [
        ("owner", OWNER, &[]),
        ("review", REVIEW, &[("deepReviewEstimate", Some("review.deepAt"))]),
        ("triage", TRIAGE, &[]),
    ];
    for (group, known, moved) in groups {
        let value = &config[group];
        if !value.is_object() && !value.is_null() {
            return Err(Error::Usage(format!("\"{group}\" in config.json must be an object of {}", known.join(", "))));
        }
        keys(value, group, known, moved)?;
    }
    Ladder::from_config(config.get("ladder").filter(|v| !v.is_null()))?;
    RepoMap::from_config(config.get("repos"))?;
    Ok(())
}

// The schema editors validate config.json against: the keys above and in `ladder` and `repos`
// are its keys (a test pins the two), so a setting is added to both in one change.
pub const SCHEMA: &str = include_str!("config/schema.json");
pub const SCHEMA_FILE: &str = "config.schema.json";
// What config.json's `$schema` holds: the copy beside it.
pub const SCHEMA_REF: &str = "./config.schema.json";

// This build's schema, beside the config file at `file`, so an editor validates offline against
// the keys this binary reads. Written before config.json names it.
pub fn write_schema(file: &Path) -> Result<(), Error> {
    let path = file.with_file_name(SCHEMA_FILE);
    if fs::read_to_string(&path).is_ok_and(|text| text == SCHEMA) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).context(dir.display())?;
    }
    fs::write(&path, SCHEMA).context(path.display())
}

// `write_schema`, and `$schema` in the config file at `file`.
pub fn adopt_schema(file: &Path) -> Result<(), Error> {
    write_schema(file)?;
    merge_json_file(file, false, |_| Map::from_iter([("$schema".into(), json!(SCHEMA_REF))])).map(drop)
}

// credentials.json holds secrets and this machine's `host` alone: a setting left there from
// before it moved exits 2, rather than being dropped unread.
pub fn check_credentials(credentials: &Value) -> Result<(), Error> {
    match [("ownerId", "owner.id"), ("coAuthor", "owner.coAuthor")].into_iter().find(|(old, _)| credentials.get(old).is_some_and(|v| !v.is_null() && v.as_str() != Some(""))) {
        Some((old, new)) => Err(Error::Usage(format!("\"{old}\" in credentials.json is now \"{new}\" in config.json"))),
        None => Ok(()),
    }
}

// Each key of the object at `at` is in `known`, or this exits 2 naming it, with its replacement
// where `moved` has one. A value that isn't an object is left to its reader.
pub fn keys(value: &Value, at: &str, known: &[&str], moved: Moved) -> Result<(), Error> {
    let Some(key) = value.as_object().into_iter().flat_map(|o| o.keys()).find(|k| !known.contains(&k.as_str())) else { return Ok(()) };
    let name = if at.is_empty() { key.clone() } else { format!("{at}.{key}") };
    Err(Error::Usage(match moved.iter().find(|(old, _)| old == key) {
        Some((_, Some(new))) => format!("\"{name}\" in config.json is now \"{new}\""),
        Some((_, None)) => format!("\"{name}\" in config.json is no longer a setting: remove it"),
        None => format!("\"{name}\" in config.json is not a setting"),
    }))
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub owner: Owner,
    pub review: Review,
    pub triage: Triage,
    pub ladder: Ladder,
    pub repos: RepoMap,
    // Work workers at once by host name, LOCAL for this machine, which is always set: a
    // paired host with no entry, or 0, takes no work.
    pub slots: BTreeMap<String, i64>,
    // Whether workers' sessions are timed and `listen` keeps it (docs/timing.md); on unless false.
    pub timing: bool,
    // The Linear release pipeline a development build's rebuild syncs a release to
    // (docs/realtime.md); none unless set.
    pub release_pipeline: Option<String>,
    // Not a setting: credentials.json's (`host`).
    #[serde(skip)]
    pub host: Option<String>,
    // Not a setting: the recipe, and the repo it came from, that made each recipe machine, by
    // its name, from machines.json.
    #[serde(skip)]
    pub machines: BTreeMap<String, Machine>,
    // Not a setting: the IDs of the hosts holding an admitted credential, as `listen` last read them
    // (Pacing::enrolled). No other host takes a worker.
    #[serde(skip)]
    pub enrolled: Vec<String>,
    // Not a setting: the harness's, holding `<repo>`, filled in with the repo's display name
    // for each brief.
    #[serde(skip)]
    pub briefs_dir: String,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Owner {
    pub id: String,
    pub name: String,
    pub co_author: Option<String>,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub reviewers: Vec<String>,
    pub max_rounds: i64,
    pub deep_at: Size,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Triage {
    pub batch: i64,
}

// TOLKSTER_CO_AUTHOR comes before `owner.coAuthor`.
pub fn config(files: &Files, owner_id: String, home: &Path) -> Result<Config, Error> {
    let path = machines_file()?;
    // One that doesn't read stops the load: counted as none, it would let a repo's `machines` be
    // passed.
    let made = match fs::read_to_string(&path) {
        Ok(text) => Value::Object(serde_json::from_str(&text).context(path.display())?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(error) => return Err(error).context(path.display()),
    };
    Ok(Config { machines: machines(&made), ..from_files(files, owner_id, home, env("TOLKSTER_CO_AUTHOR"))? })
}

// `config`, with TOLKSTER_CO_AUTHOR passed in, so tests don't depend on the environment.
fn from_files(files: &Files, owner_id: String, home: &Path, co_author: Option<String>) -> Result<Config, Error> {
    check(&files.config)?;
    check_credentials(&files.credentials)?;
    let co_author = match co_author {
        Some(co_author) => Some(co_author),
        None => files.string("/owner/coAuthor")?,
    };
    Ok(Config {
        owner: Owner { id: owner_id, name: files.string("/owner/name")?.unwrap_or_else(|| DEFAULT_OWNER_NAME.into()), co_author },
        review: Review { reviewers: reviewers(files)?, max_rounds: whole(files, "/review/maxRounds", DEFAULT_MAX_ROUNDS)?, deep_at: deep_at(files)? },
        triage: Triage { batch: whole(files, "/triage/batch", DEFAULT_TRIAGE_BATCH)? },
        ladder: Ladder::from_config(files.get("/ladder"))?,
        repos: RepoMap::from_config(files.get("/repos"))?,
        slots: slots(files)?,
        timing: timing(files)?,
        release_pipeline: release_pipeline(files)?,
        host: host(files)?,
        machines: BTreeMap::new(),
        enrolled: Vec::new(),
        briefs_dir: briefs_dir(home).display().to_string(),
    }
    .settled())
}

// The briefs directory when config.json names none, under Claude Code's own.
pub fn briefs_dir(home: &Path) -> PathBuf {
    home.join(".claude").join("<repo>-briefs")
}

#[derive(Debug, PartialEq)]
pub struct Machine {
    pub recipe: String,
    pub repo: String,
    // Past `create`, with what it printed: false for a mark (recipe.rs, `made`).
    pub made: bool,
    // Made, with its `suspend` and `resume` recorded (recipe/sleep.rs), and asleep now.
    pub sleeps: bool,
    pub asleep: bool,
    // The host ID an asleep machine stays paired as, which its worktrees' IDs carry.
    pub environment: Option<String>,
    // The image revision its recipe's `create` or `resume` printed in `userData`, if any of the
    // shape the Durable Object takes: one of another would drop the machine from its report.
    pub revision: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReviewSettings {
    pub reviewers: Vec<String>,
    pub max_rounds: i64,
}

impl Config {
    // With each `repos` entry's review settings filled in with what its repo gets, so
    // `tolkster config` prints them: its own, or else the global ones.
    fn settled(mut self) -> Config {
        let settled: Vec<ReviewSettings> = self.repos.0.iter().map(|e| self.review_for(Some(&e.repo))).collect();
        for (entry, review) in self.repos.0.iter_mut().zip(settled) {
            entry.review = Some(RepoReview { reviewers: Some(review.reviewers), max_rounds: Some(review.max_rounds) });
        }
        self
    }

    // This machine's cap.
    pub fn local_slots(&self) -> i64 {
        self.slots.get(LOCAL).copied().unwrap_or(DEFAULT_LOCAL_SLOTS)
    }

    // The review settings for the Orca repo with this display name, or the global ones
    // for None or a repo no entry names.
    pub fn review_for(&self, repo: Option<&str>) -> ReviewSettings {
        let own = repo.and_then(|repo| self.repos.entry(repo)).and_then(|e| e.review.as_ref());
        ReviewSettings {
            reviewers: own.and_then(|r| r.reviewers.clone()).unwrap_or_else(|| self.review.reviewers.clone()),
            max_rounds: own.and_then(|r| r.max_rounds).unwrap_or(self.review.max_rounds),
        }
    }

    // The review settings for a worker in a worktree of the repo `repo_name` names. It is
    // asked only when some repo has its own, and a read that fails is an error rather than
    // the global settings, which could bless a reviewer the repo left out.
    pub fn review_in(&self, repo_name: &mut dyn FnMut() -> Result<Option<String>, Error>) -> Result<ReviewSettings, Error> {
        if !self.per_repo_review() {
            return Ok(self.review_for(None));
        }
        Ok(self.review_for(repo_name()?.as_deref()))
    }

    // Whether some repo's review settings differ from the global ones, so a worker's repo
    // has to be known to pick them.
    pub fn per_repo_review(&self) -> bool {
        let global = self.review_for(None);
        self.repos.0.iter().any(|e| self.review_for(Some(&e.repo)) != global)
    }

    // Whether some repo's reviewers differ from the global ones, so a worker's repo has to
    // be known to pick them. Read through review_for, not from the entries' own fields.
    pub fn repo_reviewers_differ(&self) -> bool {
        self.repos.0.iter().any(|e| self.review_for(Some(&e.repo)).reviewers != self.review.reviewers)
    }
}

// A whole number of at least `min`, `3` or `3.0`; each caller gives its own error.
pub fn whole_number(value: &Value, min: i64) -> Option<i64> {
    value.as_f64().filter(|n| n.is_finite() && n.fract() == 0.0 && *n >= min as f64).map(|n| n as i64)
}

// Each name trimmed, so a login is the same in the global `reviewers` as in a repo's.
// None unless every item is a non-empty string.
pub fn names(value: &Value) -> Option<Vec<String>> {
    value.as_array()?.iter().map(|s| s.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from)).collect()
}

fn whole(files: &Files, path: &str, default: i64) -> Result<i64, Error> {
    let Some(value) = files.get(path) else { return Ok(default) };
    whole_number(value, 1).ok_or_else(|| Error::Usage(format!("\"{}\" in config.json must be a whole number of at least 1", dotted(path))))
}

// A size by name, and no value turns the deep pass off.
fn deep_at(files: &Files) -> Result<Size, Error> {
    let Some(value) = files.get("/review/deepAt") else { return Ok(DEFAULT_DEEP_AT) };
    value.as_str().and_then(Size::parse).filter(|size| *size >= Size::XS).ok_or_else(|| Error::Usage("\"review.deepAt\" in config.json must be a size, XS, S, M, L, XL, XXL or XXXL".into()))
}

fn reviewers(files: &Files) -> Result<Vec<String>, Error> {
    let Some(value) = files.get("/review/reviewers") else { return Ok(vec![]) };
    names(value).ok_or_else(|| Error::Usage("\"review.reviewers\" in config.json must be a list of GitHub logins".into()))
}

fn timing(files: &Files) -> Result<bool, Error> {
    match files.get("/timing") {
        None => Ok(true),
        Some(Value::Bool(on)) => Ok(*on),
        Some(_) => Err(Error::Usage("\"timing\" in config.json must be true or false".into())),
    }
}

pub fn release_pipeline(files: &Files) -> Result<Option<String>, Error> {
    match files.string("/releasePipeline")? {
        Some(id) if Id::parse(&id).is_none() => Err(Error::Usage("\"releasePipeline\" in config.json must be a Linear ID".into())),
        id => Ok(id),
    }
}

fn slots(files: &Files) -> Result<BTreeMap<String, i64>, Error> {
    let bad = || Error::Usage(format!("\"slots\" in config.json must map host names, \"{LOCAL}\" for this machine, to whole numbers of at least 0"));
    let mut caps: BTreeMap<String, i64> = match files.get("/slots") {
        Some(value) => value.as_object().ok_or_else(bad)?.iter().map(|(name, cap)| whole_number(cap, 0).map(|cap| (name.trim().to_string(), cap)).filter(|(name, _)| !name.is_empty()).ok_or_else(bad)).collect::<Result<_, _>>()?,
        None => BTreeMap::new(),
    };
    caps.entry(LOCAL.into()).or_insert(DEFAULT_LOCAL_SLOTS);
    Ok(caps)
}

// machines.json's entries (recipe.rs). An entry of another shape is left out
// rather than refused: `host rm` reports it.
pub(crate) fn machines(made: &Value) -> BTreeMap<String, Machine> {
    let text = |entry: &Value, key: &str| entry.get(key).and_then(Value::as_str).map(String::from);
    let kept = made.as_object().into_iter().flatten();
    // As recipe/sleep.rs's `sleeper` reads an entry.
    let sleeps = |entry: &Value| entry["userData"].is_object() && ["recipe", "repo", "folder", "destroy", "suspend", "resume"].iter().all(|key| text(entry, key).is_some_and(|text| !text.is_empty()));
    let made = |entry: &Value| Some(Machine { recipe: text(entry, "recipe")?, repo: text(entry, "repo").unwrap_or_default(), made: entry["userData"].is_object(), sleeps: sleeps(entry), asleep: entry["asleep"] == true, environment: text(entry, "environment"), revision: text(&entry["userData"], "revision").filter(|revision| plain_revision(revision)) });
    kept.filter_map(|(name, entry)| Some((name.clone(), made(entry)?))).collect()
}

// As `REVISION` in src/worker/machine/fleet.ts.
fn plain_revision(revision: &str) -> bool {
    (1..=100).contains(&revision.len()) && revision.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
}

// Both files, checked as config::check and check_credentials read them.
pub fn files() -> Result<Files, Error> {
    let files = unchecked_files()?;
    check(&files.config)?;
    check_credentials(&files.credentials)?;
    Ok(files)
}

// Both files as they are: for `host join`, which replaces config.json whole.
pub fn unchecked_files() -> Result<Files, Error> {
    let read = |file: PathBuf| -> Result<Value, Error> {
        if !file.exists() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&fs::read_to_string(&file).context(file.display())?).context(file.display())
    };
    Ok(Files { config: read(config_file()?)?, credentials: read(credentials_file()?)? })
}

// The owner: TOLKSTER_OWNER_ID, or "owner.id" in config.json. There is no flag for it: every
// security check keys on it.
pub fn owner(files: &Files) -> Result<String, Error> {
    if let Some(owner) = env("TOLKSTER_OWNER_ID") {
        return Ok(owner);
    }
    if let Some(owner) = files.string("/owner/id")? {
        return Ok(owner);
    }
    Err(Error::Usage(format!("no owner: set TOLKSTER_OWNER_ID, or add \"owner.id\" to {}", config_file()?.display())))
}

// The Worker's address: TOLKSTER_WORKER_URL or "worker" in config.json.
pub fn worker_url(files: &Files) -> Result<WorkerUrl, Error> {
    match env("TOLKSTER_WORKER_URL") {
        Some(url) => WorkerUrl::parse(&url),
        None => WorkerUrl::parse(&files.string("/worker")?.ok_or_else(|| Error::Usage(format!("no Worker URL: set TOLKSTER_WORKER_URL, or add \"worker\" to {}", shown(config_file(), ""))))?),
    }
}

// The Worker's address, and the secret `listen` connects with: TOLKSTER_REALTIME_SECRET or
// "realtimeSecret" in credentials.json.
pub fn realtime(files: &Files) -> Result<(WorkerUrl, String), Error> {
    let secret = realtime_secret(files).ok_or_else(|| {
        Error::Usage(format!("no realtime secret: set TOLKSTER_REALTIME_SECRET, or add \"realtimeSecret\" to {}", shown(credentials_file(), "")))
    })?;
    Ok((worker_url(files)?, secret))
}

fn realtime_secret(files: &Files) -> Option<String> {
    env("TOLKSTER_REALTIME_SECRET").or_else(|| files.secret("realtimeSecret"))
}

// What reaches Tolkster's state: the realtime secret or, on a worker host, its credential.
pub fn state_bearer(files: &Files) -> Result<String, Error> {
    realtime_secret(files).or_else(|| host_credential(files)).ok_or_else(|| {
        Error::Usage(format!("no realtime secret or host credential: set TOLKSTER_REALTIME_SECRET, or add \"realtimeSecret\" or \"hostCredential\" to {}", shown(credentials_file(), "")))
    })
}

// A worker host's own credential (docs/hosts.md), "hostCredential" in credentials.json, which
// it reaches Tolkster's state with and trades for its tokens (trade.rs). The `listen` host has
// the realtime secret and its own credentials instead.
pub fn host_credential(files: &Files) -> Option<String> {
    files.secret("hostCredential")
}

// `host` in credentials.json, beside `hostCredential`: the Orca environment ID this machine is
// paired as, on a remote host, which `host join` writes and its worktrees' IDs carry
// (plain::split_worktree). None on the machine that runs `listen`, which holds no credential: a
// host whose `host` is missing, or isn't the one its credential was issued to, is refused, as
// it would read the `listen` machine's workers as its own.
pub fn host(files: &Files) -> Result<Option<String>, Error> {
    let issued = files.secret("hostCredential").map(|credential| credential.split(':').next().unwrap_or_default().to_string());
    match (files.credentials.get("host").filter(|v| !v.is_null()), issued) {
        (None, None) => Ok(None),
        (Some(Value::String(host)), Some(issued)) if plain_host(host) && *host == issued => Ok(Some(host.clone())),
        _ => Err(Error::Usage("\"host\" in credentials.json must be the Orca environment ID its hostCredential was issued to: rejoin this host with tolkster host add --rejoin".into())),
    }
}

// The Worker's address, "worker": https, or plain http only to this machine, for
// `wrangler dev`. Anywhere else the realtime secret would cross the network in the clear.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkerUrl {
    secure: bool,
    host: String,
}

impl WorkerUrl {
    pub fn parse(worker_url: &str) -> Result<WorkerUrl, Error> {
        let base = worker_url.trim().trim_end_matches('/');
        let (secure, host) = if let Some(host) = base.strip_prefix("https://") {
            (true, host)
        } else if let Some(host) = base.strip_prefix("http://").filter(|h| is_local(h)) {
            (false, host)
        } else {
            return Err(Error::Usage(format!("\"worker\" must start with https:// (http:// only for localhost): {base}")));
        };
        if host.is_empty() || host.contains(['/', '?', '#']) {
            return Err(Error::Usage(format!("\"worker\" must be the Worker's address alone, e.g. https://tolkster.example.workers.dev: {base}")));
        }
        Ok(WorkerUrl { secure, host: host.into() })
    }

    pub fn realtime(&self) -> String {
        format!("{}://{}/realtime", if self.secure { "wss" } else { "ws" }, self.host)
    }

    pub fn state(&self) -> String {
        format!("{}://{}/state", if self.secure { "https" } else { "http" }, self.host)
    }

    // Where Linear sends the owner back once the app is installed (docs/setup.md).
    pub fn oauth_callback(&self) -> String {
        format!("{}://{}/oauth/callback", if self.secure { "https" } else { "http" }, self.host)
    }

    pub fn token(&self) -> String {
        format!("{}://{}/token", if self.secure { "https" } else { "http" }, self.host)
    }

    // The prebuilt `tolkster` for a target at a commit (src/worker/build.ts).
    pub fn build(&self, target: &str, commit: &str) -> String {
        format!("{}://{}/build/{target}/{commit}", if self.secure { "https" } else { "http" }, self.host)
    }

    // Where the Linear app's webhook delivers (docs/setup.md).
    pub fn linear_webhook(&self) -> String {
        format!("{}/webhooks/linear", self.base())
    }

    // Where the GitHub App's webhook delivers (docs/github-app.md, The webhook).
    pub fn github_webhook(&self) -> String {
        format!("{}/webhooks/github", self.base())
    }

    pub fn enrol(&self) -> String {
        format!("{}://{}/enrol", if self.secure { "https" } else { "http" }, self.host)
    }

    // The address as config.json's "worker" holds it.
    pub fn base(&self) -> String {
        format!("{}://{}", if self.secure { "https" } else { "http" }, self.host)
    }
}

fn is_local(host: &str) -> bool {
    let name = host.rsplit_once(':').filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit())).map_or(host, |(name, _)| name);
    matches!(name, "localhost" | "127.0.0.1" | "[::1]")
}

pub mod changes;
pub mod ladder;
pub mod repo_map;

#[cfg(test)]
mod tests;
