// Wrangler, Cloudflare's CLI, run through npx in the checkout the Worker deploys from, whose
// wrangler.jsonc names the Worker. Its own lines go to stderr, so a command's stdout stays its
// JSON. A secret's value goes to wrangler's stdin alone: never to disk, output, an argument or
// the environment.

use std::io::{self, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::error::Error;

// Where wrangler reads the Worker's name, so every call runs in the directory that holds it.
pub const CONFIG: &str = "wrangler.jsonc";

fn wrangler(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(if cfg!(windows) { "npx.cmd" } else { "npx" });
    command.arg("wrangler").args(args).current_dir(dir);
    command
}

// The account wrangler is signed in to, as `whoami --json` prints it; None when it isn't.
pub fn whoami(dir: &Path) -> Result<Option<Value>, Error> {
    let output = wrangler(dir, &["whoami", "--json"]).stdin(Stdio::null()).stderr(Stdio::null()).output().map_err(|_| unrun())?;
    Ok(output.status.success().then(|| serde_json::from_slice(&output.stdout).ok()).flatten())
}

// Wrangler's own sign-in, in the browser, with its prompts on this terminal.
pub fn login(dir: &Path) -> Result<(), Error> {
    ran(wrangler(dir, &["login"]).stdout(Stdio::from(io::stderr())).status(), "wrangler couldn't sign in")
}

// Deploys the checkout's Worker, its lines shown as they come, and returns its workers.dev
// address when wrangler printed one.
pub fn deploy(dir: &Path) -> Result<Option<String>, Error> {
    let mut child = wrangler(dir, &["deploy"]).stdout(Stdio::piped()).spawn().map_err(|_| unrun())?;
    let mut printed = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        let mut chunk = [0u8; 4096];
        while let Ok(read @ 1..) = out.read(&mut chunk) {
            let _ = io::stderr().write_all(&chunk[..read]);
            printed.extend_from_slice(&chunk[..read]);
        }
    }
    match child.wait() {
        Ok(status) if status.success() => Ok(workers_dev(&String::from_utf8_lossy(&printed))),
        _ => Err(Error::Failed("wrangler couldn't deploy the Worker".into())),
    }
}

// Wrangler's own install, the checkout's packages, so npx finds it there.
pub fn install(dir: &Path) -> Result<(), Error> {
    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    ran(Command::new(npm).arg("ci").current_dir(dir).stdin(Stdio::null()).stdout(Stdio::from(io::stderr())).status(), "npm ci failed in the Worker's checkout")
}

// Whether the checkout's packages hold wrangler.
pub fn installed(dir: &Path) -> bool {
    dir.join("node_modules").join("wrangler").is_dir()
}

// The first workers.dev address in wrangler's output.
pub fn workers_dev(printed: &str) -> Option<String> {
    printed.split_whitespace().map(|word| word.trim_end_matches(|c: char| !c.is_ascii_alphanumeric())).find(|word| word.starts_with("https://") && word.ends_with(".workers.dev")).map(String::from)
}

// The names of the Worker's secrets, never their values: once it is deployed.
pub fn secret_names(dir: &Path) -> Result<Vec<String>, Error> {
    listed(dir)?.ok_or_else(|| Error::Failed("the Worker isn't deployed".into()))
}

// Whether the Worker is deployed: only Cloudflare's own answer that it isn't counts as no, as a
// failure to ask can't tell.
pub fn exists(dir: &Path) -> Result<bool, Error> {
    listed(dir).map(|names| names.is_some())
}

// The Worker's secrets' names, None for a Worker Cloudflare says isn't there.
fn listed(dir: &Path) -> Result<Option<Vec<String>>, Error> {
    let output = wrangler(dir, &["secret", "list", "--format", "json"]).stdin(Stdio::null()).output().map_err(|_| unrun())?;
    if !output.status.success() {
        return match absent(&String::from_utf8_lossy(&output.stderr)) {
            true => Ok(None),
            false => Err(Error::Failed("wrangler couldn't list the Worker's secrets".into())),
        };
    }
    names(&String::from_utf8_lossy(&output.stdout)).map(Some).ok_or_else(|| Error::Failed("wrangler listed the Worker's secrets in another shape".into()))
}

// Cloudflare's error for a Worker that doesn't exist on the account, as wrangler prints it.
pub fn absent(stderr: &str) -> bool {
    stderr.contains("[code: 10007]")
}

pub fn names(listed: &str) -> Option<Vec<String>> {
    serde_json::from_str::<Value>(listed).ok()?.as_array()?.iter().map(|secret| secret["name"].as_str().map(String::from)).collect()
}

// Worker secrets in one request, the values as JSON on wrangler's stdin.
pub fn put_secrets(dir: &Path, secrets: &[(&str, String)]) -> Result<(), Error> {
    let names = secrets.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(", ");
    let failed = || Error::Failed(format!("wrangler couldn't set {names}"));
    let body = Value::Object(secrets.iter().map(|(name, value)| (name.to_string(), Value::String(value.clone()))).collect()).to_string();
    let mut child = wrangler(dir, &["secret", "bulk"]).stdin(Stdio::piped()).stdout(Stdio::from(io::stderr())).stderr(Stdio::inherit()).spawn().map_err(|_| failed())?;
    let written = child.stdin.take().is_some_and(|mut stdin| stdin.write_all(body.as_bytes()).is_ok());
    match child.wait() {
        Ok(status) if status.success() && written => Ok(()),
        _ => Err(failed()),
    }
}

fn ran(status: io::Result<std::process::ExitStatus>, failed: &str) -> Result<(), Error> {
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err(Error::Failed(failed.into())),
        Err(_) => Err(unrun()),
    }
}

fn unrun() -> Error {
    Error::Failed("npx couldn't run wrangler: install Node 24, then npm ci in the checkout".into())
}

#[cfg(test)]
mod tests;
