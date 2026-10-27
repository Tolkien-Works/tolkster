// The process's ground: its environment, its home and settings files, stdout and stdin.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Serialize;
use serde_json::Value;

use crate::error::{Context, Error};
use crate::plain::{Id, Time};
use crate::sync::lock;

// The branch `listen` keeps itself current with (update.rs).
pub const BRANCH: &str = "main";

// Exit code for "nothing to do", e.g. `issue unrelate` finding no such relation.
pub const NOTHING_TO_DO: u8 = 3;

// No home directory is an error, not the current one: the token cache must never land
// inside a checkout, where it could be committed.
pub fn home_dir() -> Result<PathBuf, Error> {
    #[allow(deprecated)]
    let home = std::env::home_dir().filter(|h| h.is_absolute());
    home.ok_or_else(|| Error::Failed("no home directory: set USERPROFILE (Windows) or HOME".into()))
}

pub fn home() -> Result<PathBuf, Error> {
    Ok(home_dir()?.join(".tolkster"))
}

// An environment variable, unless it is unset or empty.
pub fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

pub fn config_file() -> Result<PathBuf, Error> {
    Ok(home()?.join("config.json"))
}

pub fn credentials_file() -> Result<PathBuf, Error> {
    Ok(home()?.join("credentials.json"))
}

// The log `listen` keeps (trace/disk.rs), with `listen.log.1` beside it.
pub fn listen_log_file() -> Result<PathBuf, Error> {
    Ok(home()?.join("listen.log"))
}

// Each worker session's timing and its issue's start, PR and merge (timing.rs), on the
// `listen` host.
pub fn timing_file() -> Result<PathBuf, Error> {
    Ok(home()?.join("timing.jsonl"))
}

// The record of machines a recipe made (recipe.rs), on the `listen` host.
pub fn machines_file() -> Result<PathBuf, Error> {
    Ok(home()?.join("machines.json"))
}

pub fn token_file() -> Result<PathBuf, Error> {
    Ok(home()?.join("token.json"))
}

// A file's path for a message, or `otherwise` without a home directory.
pub fn shown(file: Result<PathBuf, Error>, otherwise: &str) -> String {
    file.map_or(otherwise.into(), |f| f.display().to_string())
}

pub fn print(text: &str) {
    let _ = writeln!(io::stdout(), "{text}");
}

pub fn print_json(value: &impl Serialize) -> Result<(), Error> {
    print(&serde_json::to_string_pretty(value).context("encoding JSON")?);
    Ok(())
}

// A command's last step: `value` printed, then `code` as its exit.
pub fn print_exit(value: &impl Serialize, code: u8) -> Result<u8, Error> {
    print_json(value)?;
    Ok(code)
}

// Exit 3, with `why` on stderr.
pub fn nothing_to_do(why: &str) -> Result<u8, Error> {
    eprintln!("{why}");
    Ok(NOTHING_TO_DO)
}

// A failure that doesn't stop the command: on stderr as "<what>: <error>", then None. Only
// where its error may be printed: the hook's `missed` prints none.
pub fn logged<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> Option<T> {
    result.map_err(|error| eprintln!("{what}: {error}")).ok()
}

// Set to a conversation turn's own session: `tolkster` then runs only the allowlist's table.
pub const CONVERSATION_ENV: &str = "TOLKSTER_CONVERSATION";

// Set by `tolkster worker` in its agent's environment to the worker's worktree ID and issue,
// which its hooks and checkpoints run for (card::launched).
pub const WORKTREE_ENV: &str = "TOLKSTER_WORKTREE";
pub const ISSUE_ENV: &str = "TOLKSTER_ISSUE";
// The session a worker's launch traces, in its environment, when its repo's `trace` is on
// (hook::narrate).
pub const TRACE_ENV: &str = "TOLKSTER_TRACE";

pub fn conversation_session() -> Option<String> {
    env(CONVERSATION_ENV)
}

// Set to a thread's turn's `<root>@<request>`, the thread of Tolkster's comment it answers in
// and the time of the request it started on: `tolkster` then runs only the allowlist's thread
// table, and the one answer it posts is the request's it answers (`state::thread_request`).
pub const THREAD_ENV: &str = "TOLKSTER_THREAD";

// Set but unreadable is refused (exit 2), never taken as no turn, which would lift the gate.
pub fn thread_turn() -> Result<Option<(Id, Time)>, Error> {
    let Some(value) = env(THREAD_ENV) else { return Ok(None) };
    match value.split_once('@').and_then(|(root, request)| Some((Id::parse(root)?, Time::parse(request)?))) {
        Some(turn) => Ok(Some(turn)),
        None => Err(Error::Usage(format!("{THREAD_ENV} isn't <commentId>@<time>"))),
    }
}

pub fn read_body(source: &str) -> Result<String, Error> {
    let bytes = if source == "-" {
        let mut bytes = Vec::new();
        io::stdin().read_to_end(&mut bytes).context("reading stdin")?;
        bytes
    } else {
        fs::read(source).context(source)?
    };
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

pub fn write_file(path: &std::path::Path, text: &str) -> Result<(), Error> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).context(dir.display())?;
    }
    fs::write(path, text).context(path.display())
}

// Writes `text` to `path` aside, as `<path>.<pid>`, and renames it over, so a reader never sees
// half a file. The caller keeps this process's writes to `path` one at a time, as they share
// the part file's name; a failed rename removes it.
pub fn replace_file(path: &Path, text: &str) -> Result<(), Error> {
    let mut part = path.as_os_str().to_owned();
    part.push(format!(".{}", std::process::id()));
    let part = PathBuf::from(part);
    write_file(&part, text)?;
    fs::rename(&part, path).context(path.display()).inspect_err(|_| {
        let _ = fs::remove_file(&part);
    })
}

// The text of `path`, or `None` when there is no such file.
pub fn read_if_there(path: &Path) -> Result<Option<String>, Error> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context(path.display()),
    }
}

// Sets the entries `entries` makes from the JSON object in `path`, as it is at the write, a
// null removing its key, and keeps every other key, unknown ones included: the one writer of
// the owner's settings files, so no entry is made from a stale read. Written aside and
// renamed, so a reader never sees half a file; `private`, readable by the user alone on Unix
// (on Windows the profile's ACL keeps it so). Whether anything changed: an unchanged file
// isn't written. This process's writes go one at a time, so a thread's read is never stale
// at its write and no two share the file written aside.
#[cfg_attr(not(unix), allow(unused_variables))]
pub fn merge_json_file(path: &Path, private: bool, entries: impl FnOnce(&serde_json::Map<String, Value>) -> serde_json::Map<String, Value>) -> Result<bool, Error> {
    // This process's threads' writes: other processes' are what the rename keeps whole.
    static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _held = lock(&WRITING);
    let mut file = match read_if_there(path)? {
        Some(text) => serde_json::from_str::<Value>(&text).context(path.display())?,
        None => Value::Object(serde_json::Map::new()),
    };
    let object = file.as_object_mut().ok_or_else(|| Error::Failed(format!("{}: not a JSON object", path.display())))?;
    let before = object.clone();
    for (key, value) in &entries(&before) {
        match value {
            Value::Null => object.remove(key),
            value => object.insert(key.clone(), value.clone()),
        };
    }
    if *object == before {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).context(dir.display())?;
    }
    let text = serde_json::to_string_pretty(&file).context(path.display())?;
    let part = path.with_extension(format!("json.tolkster-{}", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    }
    if let Err(error) = options.open(&part).and_then(|mut file| file.write_all(text.as_bytes())).and_then(|_| fs::rename(&part, path)) {
        let _ = fs::remove_file(&part);
        return Err(error).context(path.display());
    }
    Ok(true)
}

// Best effort: the line printed before it names the address too.
pub fn open_browser(url: &str) {
    let (program, args): (&str, &[&str]) = if cfg!(windows) {
        ("rundll32", &["url.dll,FileProtocolHandler"])
    } else if cfg!(target_os = "macos") {
        ("open", &[])
    } else {
        ("xdg-open", &[])
    };
    let _ = Command::new(program).args(args).arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

#[cfg(test)]
mod tests;
