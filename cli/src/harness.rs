// The agent harness a worker runs on (docs/harness.md): its program, how it is launched and
// resumed, its model and effort flags, the settings and MCP files it starts with, the trust a
// worktree needs before its first prompt, what it leaves in a worktree, its events (`events`),
// and a conversation's turn (`turn`). Claude Code is its one implementation; a launch names its
// `Harness` (harnesses.rs).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use serde_json::{Value, json};

use crate::cli::replace_file;
use crate::error::{Context, Error};
use crate::config::ladder::{Launch, plain_model};
use crate::harnesses::{DEFAULT, Harness, named};
use crate::plain::{CommandLine, Line};
use crate::sync::lock;

fn launch_flags(harness: &Harness, launch: &Launch) -> [String; 4] {
    [harness.model_flag.into(), launch.model.clone(), harness.effort_flag.into(), launch.effort.clone()]
}

// A launch's harness, once it meets the unattended-safety requirements, its model is a plain
// name and its effort one of the harness's, as a record read back may not have them. None: the
// default.
pub fn check(launch: Option<&Launch>) -> Result<&'static Harness, Error> {
    let Some(launch) = launch else { return Ok(DEFAULT) };
    let harness = named(&launch.harness).filter(|harness| harness.name == launch.harness).ok_or_else(|| Error::Failed(format!("the worker's harness isn't one this build launches: {:?}", launch.harness)))?;
    if !harness.unmet().is_empty() {
        return Err(Error::Failed(format!("{} can't run unattended safely: {}", harness.name, harness.unmet().join("; "))));
    }
    if !plain_model(&launch.model) {
        return Err(Error::Failed(format!("the worker's model isn't a plain name: {:?}", launch.model)));
    }
    if harness.effort(&launch.effort) != Some(launch.effort.as_str()) {
        return Err(Error::Failed(format!("the worker's effort isn't one of {}'s: {:?}", harness.name, launch.effort)));
    }
    Ok(harness)
}

// Typed into the terminal's shell as it is, so it carries only values from code or
// configuration, never Linear's or GitHub's text, and stays one plain line: a launch read
// back from a record is checked (`check`), so it adds no flag. None: the default harness,
// plain, with no recorded launch to repeat. The plugins are the repo's
// `.claude/settings.json`'s to choose, not a flag here.
pub fn command(launch: Option<&Launch>) -> Result<CommandLine, Error> {
    line(false, launch)
}

// Launched as it started, so a resumed worker's harness, model and effort never change.
pub fn continued(launch: Option<&Launch>) -> Result<CommandLine, Error> {
    line(true, launch)
}

fn line(continued: bool, launch: Option<&Launch>) -> Result<CommandLine, Error> {
    let harness = check(launch)?;
    let flags = launch.map(|launch| launch_flags(harness, launch)).into_iter().flatten();
    let own = harness.unattended.iter().map(|flag| flag.to_string());
    let words: Vec<String> = [harness.program.to_string()].into_iter().chain(continued.then(|| harness.resume.to_string())).chain(own).chain(flags).collect();
    Ok(CommandLine::new(&Line::check(&words.join(" "))?))
}

// A worker's arguments as its host's setup starts it (setup.rs, `exec`), with no shell in
// between, so the settings files' path may hold any character: continued or not, with its
// model and effort, its settings `hooks` (`write_settings`) on a harness that takes them, and
// its first prompt.
pub fn args(harness: &Harness, continued: bool, launch: Option<&Launch>, hooks: Option<&Path>, prompt: &Line) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if continued {
        args.push(harness.resume.into());
    }
    args.extend(harness.unattended.iter().map(|flag| flag.to_string()));
    if let Some(launch) = launch {
        args.extend(launch_flags(harness, launch));
    }
    if let Some(hooks) = hooks.filter(|_| harness.settings) {
        // `--mcp-config` takes every word up to the next flag, the prompt after it included,
        // so `--settings` follows it.
        args.extend([STRICT_MCP.into(), "--mcp-config".into(), mcp_path(hooks).display().to_string(), "--settings".into(), hooks.display().to_string()]);
    }
    args.push(prompt.to_string());
    args
}

// The worker's agent, its harness's program on `args`, with `env` and the owner's `token`
// (trade::claude) in its environment alone: in this process's place where the system allows
// it, so the terminal's signals reach it alone; elsewhere a child with this terminal, whose
// exit code is this one's.
pub fn exec(harness: &Harness, args: &[String], env: &[(String, String)], token: Option<String>) -> Result<u8, Error> {
    let mut command = Command::new(harness.program);
    command.args(args);
    command.envs(env.iter().map(|(k, v)| (k, v)));
    if let (Some(name), Some(token)) = (harness.token_env, token) {
        command.env(name, token);
    }
    replace(harness.program, &mut command)
}

#[cfg(unix)]
fn replace(program: &str, command: &mut Command) -> Result<u8, Error> {
    use std::os::unix::process::CommandExt;
    Err(command.exec()).context(program)
}

#[cfg(not(unix))]
fn replace(program: &str, command: &mut Command) -> Result<u8, Error> {
    let status = command.status().context(program)?;
    Ok(status.code().and_then(|c| u8::try_from(c).ok()).unwrap_or(1))
}

// The Claude Code settings every worker is launched with (`args`): the `tolkster hook`
// commands (commands/hook.rs). Async, so the call never waits on them, but for `hook start`,
// whose post must land before the call's end posts; `hook tool`'s long timeout lets it tick
// through a long call (hook::TICK_FOR). Fixed text, compiled in like the briefs, written beside them on
// the worker's host as it starts or resumes, so a rebuilt CLI's text reaches the next worker. Hooks from `--settings`
// add to those of the user and the repo and replace none.
pub const HOOKS: &str = include_str!("../hooks.json");

// The MCP servers a worker or a turn (`turn`) starts with, beside the hooks: none. A server such as a
// plugin's Linear acts as the owner, where Tolkster posts only through `tolkster`, and Claude
// starts seconds sooner without connecting to them. Plugins' skills and hooks still load.
pub const NO_MCP: &str = "{\"mcpServers\":{}}";
pub const STRICT_MCP: &str = "--strict-mcp-config";

pub fn mcp_path(hooks: &Path) -> PathBuf {
    hooks.with_file_name("mcp.json")
}

// Shared by the repo's workers, so a file is rewritten only when a rebuilt CLI changed it,
// and whole, by a rename, so a worker starting meanwhile never reads half of one. `listen`'s
// lanes share its process, and so a part file's name: one writes at a time.
static WRITING: Mutex<()> = Mutex::new(());

pub fn write_settings(hooks: &Path) -> Result<(), Error> {
    let _writing = lock(&WRITING);
    settings(hooks).iter().filter(|(path, text)| std::fs::read_to_string(path).ok().as_deref() != Some(text)).try_for_each(|(path, text)| replace_file(path, text))
}

fn settings(hooks: &Path) -> [(PathBuf, &'static str); 2] {
    [(hooks.to_path_buf(), HOOKS), (mcp_path(hooks), NO_MCP)]
}

// `hooks.json` in the repo's briefs directory (brief::path).
pub fn hooks_path(briefs_dir: &str, repo: &str) -> PathBuf {
    Path::new(&briefs_dir.replace("<repo>", repo)).join("hooks.json")
}

// Ignored files Claude itself leaves in a worktree that hold nothing of the work (reap.rs).
pub const LEFTOVERS: [&str; 1] = [".claude/scheduled_tasks.lock"];

// The directory Claude Code is told to keep its state in, if any.
fn config_dir() -> Option<PathBuf> {
    crate::cli::env("CLAUDE_CONFIG_DIR").map(PathBuf::from)
}

// Claude Code's own settings, where it keeps which folders are trusted: `.claude.json` in
// CLAUDE_CONFIG_DIR, else in the home directory.
pub fn settings_file() -> Result<PathBuf, Error> {
    Ok(match config_dir() {
        Some(dir) => dir.join(".claude.json"),
        None => crate::cli::home_dir()?.join(".claude.json"),
    })
}

// Claude stops at its trust dialog on a folder it hasn't seen, and runs no prompt until it is
// answered, so the worktree is trusted before `claude` starts: false when it, or a folder it
// is in, was already, and nothing is written. Claude keys a folder by its path, with `/` on
// Windows. The file is Claude's, rewritten whole by a rename, so a session starting meanwhile
// never reads half of one, and read back, so another setup's write that dropped the entry is
// written over again. A running session that saves a copy it read before, after the read back
// and before this worker's Claude reads it, still drops it, and the dialog is back.
pub fn trust(file: &Path, dir: &str) -> Result<bool, Error> {
    let folder = if cfg!(windows) { dir.replace('\\', "/") } else { dir.to_string() };
    let folder = folder.trim_end_matches('/');
    let failed = |error: &dyn std::fmt::Display| Error::Failed(format!("{}: {error}", file.display()));
    for attempt in 0..=TRUST_WRITES {
        let text = crate::cli::read_if_there(file)?.unwrap_or_else(|| "{}".into());
        // Claude's own file, every key of which is kept: an open shape, read by index.
        let mut settings: Value = serde_json::from_str(&text).context(file.display())?;
        let trusted = settings["projects"].as_object().is_some_and(|projects| projects.iter().any(|(path, project)| project["hasTrustDialogAccepted"] == json!(true) && crate::plain::inside(folder, path)));
        if trusted {
            return Ok(attempt > 0);
        }
        if attempt == TRUST_WRITES {
            break;
        }
        let not_an_object = || failed(&"not the JSON object Claude writes");
        let projects = settings.as_object_mut().ok_or_else(not_an_object)?.entry("projects").or_insert_with(|| json!({}));
        let project = projects.as_object_mut().ok_or_else(not_an_object)?.entry(folder).or_insert_with(|| json!({}));
        project.as_object_mut().ok_or_else(not_an_object)?.insert("hasTrustDialogAccepted".into(), json!(true));
        replace_file(file, &serde_json::to_string_pretty(&settings).context(file.display())?)?;
    }
    Err(failed(&"another program rewrote it each time"))
}

// How many times `trust` writes before it gives up.
const TRUST_WRITES: u32 = 3;

pub mod events;
pub mod turn;

#[cfg(test)]
pub mod tests;
