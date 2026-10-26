// The steps that make a paired Orca server a worker host (docs/hosts.md), for `host add` and a
// recipe's machine (recipe.rs). Each step on the host runs in a terminal there,
// opened in the folder of a repo already set up on it in Orca, as Orca opens a terminal there
// only in a worktree it knows, and checks before it acts, so a second run changes nothing: git (stopping, before anything else changes, when it can't run hooks from config),
// the agent harness (its installer when it is missing) and its unattended mode, `tolkster` (fetched from the
// Worker with a single-use enrolment code, then `host join`), the owner's git author identity and
// the co-author the hook credits (docs/co-author.md), and each repo of this machine's
// that `repos` maps, cloned into the destination through the host's `tolkster` and set up
// under the same name.
// Only once every step and the closing checks pass is the host's credential admitted, so
// placement skips it until then: Orca lists a host as soon as it pairs, and it holds its
// credential from the join, partway through. A rejoin joins it again whatever its
// `tolkster` says, for a host whose settings this build changed. Nothing typed on the host is a secret or Linear or GitHub text:
// the code is single-use and lives minutes, and the host is handed its credential and settings
// by the Worker.

use serde::Serialize;
use serde_json::{Value, json};
use ureq::Agent;

use crate::config::{Config, Files, WorkerUrl, realtime};
use crate::error::Error;
use crate::exec::Run;
use crate::git;
use crate::orca::{Host, github_repos, import_folder, platform, project_id, ready_folder, ready_on, run_on, setup_at};
use crate::plain::split_repo;
use crate::harnesses::HOST;
use crate::plain::{CommandLine, Sha, hex, plain_code, plain_host};
use crate::prebuilt;
use crate::runtime::settings;
use crate::state::State;
use crate::trade;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Os {
    Linux,
    Mac,
    Windows,
}

impl Os {
    fn of(platform: &str) -> Option<Os> {
        match platform {
            "linux" => Some(Os::Linux),
            "darwin" => Some(Os::Mac),
            "win32" => Some(Os::Windows),
            _ => None,
        }
    }

    // The build the Worker serves for it (.github/workflows/build.yml).
    fn target(self) -> &'static str {
        match self {
            Os::Linux => "linux-x64",
            Os::Mac => "macos-arm64",
            Os::Windows => "windows-x64",
        }
    }

    fn git_advice(self) -> &'static str {
        match self {
            Os::Linux => "install git with the system's package manager, a release whose `git hook list` works",
            Os::Mac => "install git, a release whose `git hook list` works: Homebrew's, or xcode-select --install",
            Os::Windows => "install Git for Windows, a release whose `git hook list` works",
        }
    }

    // Each command for its shell: PowerShell on Windows, else the login shell. The POSIX ones
    // end on the status they mean; the PowerShell ones set `$LASTEXITCODE`, which a missing
    // program leaves alone.
    fn command(self, step: Command) -> &'static str {
        let windows = self == Os::Windows;
        match (step, windows) {
            (Command::Git, false) => "git hook list pre-commit >/dev/null 2>&1; test $? -le 1",
            (Command::Git, true) => "$LASTEXITCODE = 127; git hook list pre-commit *> $null; $LASTEXITCODE = [int]($LASTEXITCODE -gt 1)",
            (Command::Installed, windows) => HOST.installed.on(windows),
            (Command::Install, windows) => HOST.install.on(windows),
            (Command::Unattended, windows) => HOST.unattended.on(windows),
            (Command::Joined, false) => "\"$HOME/.local/bin/tolkster\" whoami >/dev/null 2>&1",
            (Command::Joined, true) => "$LASTEXITCODE = 1; & \"$HOME/.local/bin/tolkster.exe\" whoami *> $null",
            // The code, the Worker's URL, the target, the commit, and the code and URL again.
            (Command::Join, false) => "mkdir -p \"$HOME/.local/bin\" && curl -fsSL -H \"Authorization: Bearer {}\" \"{}/build/{}/{}\" | tar -xz -C \"$HOME/.local/bin\" && \"$HOME/.local/bin/tolkster\" host join {} --worker-url {}",
            (Command::Join, true) => "try { New-Item -ItemType Directory -Force \"$HOME/.local/bin\" | Out-Null; Invoke-WebRequest -Headers @{ Authorization = \"Bearer {}\" } -Uri \"{}/build/{}/{}\" -OutFile \"$env:TEMP/tolkster.tar.gz\" -ErrorAction Stop; tar -xzf \"$env:TEMP/tolkster.tar.gz\" -C \"$HOME/.local/bin\"; if ($LASTEXITCODE -ne 0) { throw 'tar' }; & \"$HOME/.local/bin/tolkster.exe\" host join {} --worker-url {} } catch { $LASTEXITCODE = 1 }",
            (Command::Config, false) => "\"$HOME/.local/bin/tolkster\" config >/dev/null 2>&1",
            (Command::Config, true) => "$LASTEXITCODE = 1; & \"$HOME/.local/bin/tolkster.exe\" config *> $null",
            // The name and email as hex, which the host's `tolkster` decodes; or the co-author.
            (Command::Author, false) => "\"$HOME/.local/bin/tolkster\" host identify --check --name {} --email {} >/dev/null 2>&1",
            (Command::Author, true) => "$LASTEXITCODE = 1; & \"$HOME/.local/bin/tolkster.exe\" host identify --check --name {} --email {} *> $null",
            (Command::Identify, false) => "\"$HOME/.local/bin/tolkster\" host identify --name {} --email {} >/dev/null 2>&1",
            (Command::Identify, true) => "$LASTEXITCODE = 1; & \"$HOME/.local/bin/tolkster.exe\" host identify --name {} --email {} *> $null",
            (Command::CoAuthor, false) => "\"$HOME/.local/bin/tolkster\" host identify --check --co-author {} >/dev/null 2>&1",
            (Command::CoAuthor, true) => "$LASTEXITCODE = 1; & \"$HOME/.local/bin/tolkster.exe\" host identify --check --co-author {} *> $null",
            (Command::OnPath, false) => "command -v tolkster >/dev/null 2>&1",
            (Command::OnPath, true) => "$LASTEXITCODE = [int](-not (Get-Command tolkster -ErrorAction SilentlyContinue))",
            // The repo's `owner/name` and the folder it is cloned to.
            (Command::Clone, false) => "\"$HOME/.local/bin/tolkster\" host clone {} --into \"{}\"",
            (Command::Clone, true) => "$LASTEXITCODE = 1; & \"$HOME/.local/bin/tolkster.exe\" host clone {} --into \"{}\"",
        }
    }

    fn join(self, destination: &str, name: &str) -> String {
        let separator = if self == Os::Windows { '\\' } else { '/' };
        format!("{}{separator}{name}", destination.trim_end_matches(['/', '\\']))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Command {
    Git,
    Installed,
    Install,
    Unattended,
    Joined,
    Join,
    Config,
    Author,
    Identify,
    CoAuthor,
    OnPath,
    Clone,
}

// One line of the report: a step, `ok` when it was already so, `done` when this run made it
// so, or `fail` with what to do.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct Step {
    pub step: String,
    pub verdict: &'static str,
    pub reason: String,
}

pub(crate) fn step(name: &str, verdict: &'static str, reason: impl Into<String>) -> Step {
    Step { step: name.into(), verdict, reason: reason.into() }
}

// Whether `host`'s own `tolkster` reads its config.json: its `config` exits 0. A settings file
// this build refuses, or a `tolkster` too old for it, exits otherwise, and the host takes no work
// until it is rejoined. Err where the host can't be asked or Orca names no exit code.
pub(crate) fn reads_config(run: &mut Run, host: &str) -> Result<bool, Error> {
    let os = Os::of(&platform(run, host)?).ok_or_else(|| Error::Failed("Orca names a platform host add doesn't know".into()))?;
    let folder = ready_folder(run, host)?.ok_or_else(|| Error::Failed("no repo is set up on the host".into()))?;
    let command = CommandLine::template(os.command(Command::Config), &[])?.exiting();
    run_on(run, host, &format!("path:{folder}"), &command)?.map(|code| code == 0).ok_or_else(|| Error::Failed("Orca named no exit code for the host's tolkster config".into()))
}

// The owner's git author identity, from this machine's global config: what a commit on the
// host is authored as, like one made here. Both parts are one plain line of at most 100
// characters, as a host's `git config` takes them from `host identify`.
#[derive(Debug, PartialEq)]
pub(crate) struct Author {
    pub name: String,
    pub email: String,
}

pub(crate) const NO_AUTHOR: &str = "this machine's global git config needs a user.name and a user.email, each one plain line of at most 100 characters: the host's commits are authored as them";

impl Author {
    pub(crate) fn read(run: &mut Run) -> Option<Author> {
        let line = |value: String| Some(value).filter(|v| v.len() <= 100 && !v.chars().any(char::is_control));
        let (name, email) = (git::global_config(run, "user.name").and_then(line)?, git::global_config(run, "user.email").and_then(line)?);
        Some(Author { name, email })
    }
}

// What `add` acts on, read on the `listen` host.
pub(crate) struct Target<'a> {
    pub host: &'a Host,
    pub destination: &'a str,
    pub worker_url: &'a WorkerUrl,
    // The commit of this machine's `tolkster`, whose build the host fetches.
    pub commit: &'a Sha,
    // Each repo to set up there, as (display name, `owner/name`).
    pub repos: &'a [(String, String)],
    // A recipe's checkout, as (`owner/name`, folder): its machine is made from an image, so
    // its clone is as old as the image.
    pub checkout: Option<(&'a str, &'a str)>,
    pub rejoin: bool,
    // The identity the host's commits are authored as; None where this machine has none, which
    // `set_up` reports.
    pub author: Option<&'a Author>,
    // The trailer the worker's hook credits, as this machine resolves it (`owner.coAuthor` or
    // TOLKSTER_CO_AUTHOR): None where it credits nobody.
    pub co_author: Option<&'a str>,
}

// Every step of `add` on a paired host, then the Worker's secrets, then the credential admitted
// only once all of them pass. `checkout` is a recipe's (`owner/name`, folder), fetched there:
// its repo is the host's only one, so placement never sends it another repo's worker.
// `rejoin`: the host joins again whatever `tolkster` there says (`host add --rejoin`).
pub(crate) fn set_up(agent: &Agent, files: &Files, run: &mut Run, host: &Host, destination: &str, checkout: Option<(&str, &str)>, rejoin: bool) -> Result<Vec<Step>, Error> {
    // The owner's machine alone mints a code; a worker host would be refused by the Worker.
    let (worker_url, _) = realtime(files)?;
    let config = settings(files)?;
    let (repos, unknown) = match checkout {
        Some(_) => (Vec::new(), Vec::new()),
        None => mapped_repos(&config, &github_repos(run)?),
    };
    let commit = unstamped(prebuilt::commit())?;
    let author = Author::read(run);
    let target = Target { host, destination, worker_url: &worker_url, commit: &commit, repos: &repos, checkout, rejoin, author: author.as_ref(), co_author: config.owner.co_author.as_deref() };
    let enrolled = enrolled(&files.config, target.co_author);
    let state = State::open(agent, files)?;
    let mut steps: Vec<Step> = author.is_none().then(|| step("git author", "fail", NO_AUTHOR)).into_iter().collect();
    steps.extend(unknown.iter().map(|name| step(&format!("repo {name}"), "fail", "this machine's Orca has no repo of that name on GitHub, or its name isn't plain")));
    steps.extend(add(run, &target, &mut || enrol(&state, host, &enrolled))?);
    if passed(&steps) && let Some(why) = trade::lacking(&trade::missing(agent, files)) {
        steps.push(step("worker secrets", "fail", why));
    }
    if passed(&steps) {
        steps.push(match state.admit_host(&host.id)? {
            true => step("admitted", "done", "takes workers"),
            false => step("admitted", "fail", "its credential is gone: run host add again"),
        });
    }
    Ok(steps)
}

// By the environment's ID, which its worktrees' IDs carry, so `/state` finds them its own, with
// the owner's config.json as it is: `host join` sets only `worker`, and nothing in it is
// machine-specific (`slots` is read by `listen` alone).
pub(crate) fn enrol(state: &State, host: &Host, config: &Value) -> Result<String, Error> {
    state.enrol_host(&host.id, config)
}

// The owner's config.json for a host, with `owner.coAuthor` as this machine resolves it: a
// TOLKSTER_CO_AUTHOR set here credits there too.
fn enrolled(config: &Value, co_author: Option<&str>) -> Value {
    let Some(co_author) = co_author else { return config.clone() };
    let mut config = config.clone();
    if !config.is_object() {
        config = json!({});
    }
    if !config["owner"].is_object() {
        config["owner"] = json!({});
    }
    config["owner"]["coAuthor"] = json!(co_author);
    config
}

pub(crate) fn unstamped(commit: Option<Sha>) -> Result<Sha, Error> {
    commit.ok_or_else(|| Error::Usage("this tolkster is not stamped with a commit, so the Worker has no build of it to give the host: run the one `listen` built".into()))
}

pub(crate) fn passed(steps: &[Step]) -> bool {
    steps.iter().all(|s| s.verdict != "fail")
}

// The steps on the host, in order, each checked before it acts. Stops at the first that leaves
// the host unable to go on; `enrol` mints the code `tolkster` joins with, only when it hasn't.
pub(crate) fn add(run: &mut Run, target: &Target, enrol: &mut dyn FnMut() -> Result<String, Error>) -> Result<Vec<Step>, Error> {
    let (host, mut steps) = (target.host.id.as_str(), Vec::new());
    let os = platform(run, host)?;
    let Some(os) = Os::of(&os) else { return Ok(vec![step("os", "fail", "Orca names a platform host add doesn't know")]) };
    let Some(folder) = ready_folder(run, host)? else { return Ok(vec![step("first repo", "fail", "set up one repo on the host in Orca first: host add runs its steps in its folder")]) };
    let worktree = format!("path:{folder}");
    let on = |run: &mut Run, command: CommandLine| run_on(run, host, &worktree, &command.exiting()).map(|code| code == Some(0));
    let fixed = |step: Command| CommandLine::template(os.command(step), &[]);
    if !on(run, fixed(Command::Git)?)? {
        steps.push(step("git", "fail", format!("git is missing or can't run hooks from config: {}", os.git_advice())));
        return Ok(steps);
    }
    steps.push(step("git", "ok", "runs hooks from config"));
    if on(run, fixed(Command::Installed)?)? {
        steps.push(step(HOST.step, "ok", "installed"));
    } else if on(run, fixed(Command::Install)?)? && on(run, fixed(Command::Installed)?)? {
        steps.push(step(HOST.step, "done", HOST.installed_by));
    } else {
        steps.push(step(HOST.step, "fail", HOST.install_failed));
        return Ok(steps);
    }
    // It stops before joining, as each step above does: nothing is minted for a host that can't
    // run a worker unattended.
    if !on(run, fixed(Command::Unattended)?)? {
        steps.push(step(HOST.unattended_step, "fail", HOST.unattended_advice));
        return Ok(steps);
    }
    steps.push(step(HOST.unattended_step, "ok", HOST.unattended_ok));
    let fetching_failed = "fetching tolkster or joining failed: the Worker needs a CI build of this machine's commit for the host's OS (a private repo's macOS and Windows builds come from a manual Build all targets run); run host add again for a new code";
    // Whether this run joined: the host's settings are then this run's.
    let mut joined_now = target.rejoin;
    if !target.rejoin && on(run, fixed(Command::Joined)?)? {
        steps.push(step("tolkster", "ok", "joined"));
    } else {
        joined_now = true;
        let code = enrol()?;
        if !plain_code(&code) {
            return Err(Error::Failed("Tolkster's state minted an enrolment code of another shape".into()));
        }
        let url = target.worker_url.base();
        let joined = on(run, CommandLine::template(os.command(Command::Join), &[&code, &url, os.target(), target.commit, &code, &url])?)?;
        if !(joined && on(run, fixed(Command::Joined)?)?) {
            steps.push(step("tolkster", "fail", fetching_failed));
            return Ok(steps);
        }
        steps.push(step("tolkster", "done", "fetched and joined"));
    }
    // The hook that credits the co-author reads the host's own settings at each worker's start.
    // A host joined before the owner set one, or on a build that can't say, is joined again by
    // `--rejoin` alone: a join replaces its credential, which takes it out of placement.
    if let Some(co_author) = target.co_author {
        if on(run, CommandLine::template(os.command(Command::CoAuthor), &[&hex(co_author)])?)? {
            steps.push(step("co-author", "ok", "the host's settings credit it"));
        } else {
            let reason = if joined_now { "the host's settings don't credit it: its tolkster or its environment overrides owner.coAuthor" } else { "the host's settings don't credit it: run host add --rejoin" };
            steps.push(step("co-author", "fail", reason));
            return Ok(steps);
        }
    }
    if let Some(author) = target.author {
        let fills = [&hex(&author.name)[..], &hex(&author.email)[..]];
        let set = |run: &mut Run, command: Command| on(run, CommandLine::template(os.command(command), &fills)?);
        if set(run, Command::Author)? {
            steps.push(step("git author", "ok", "the owner's"));
        } else if set(run, Command::Identify)? && set(run, Command::Author)? {
            steps.push(step("git author", "done", "set to the owner's"));
        } else {
            steps.push(step("git author", "fail", "setting the owner's git identity on the host failed: a tolkster of an older build can't, so run host add --rejoin"));
            return Ok(steps);
        }
    }
    if !on(run, fixed(Command::OnPath)?)? {
        steps.push(step("tolkster on PATH", "fail", "add ~/.local/bin to PATH on the host"));
    }
    if let Some((repo, path)) = target.checkout {
        let fetched = on(run, CommandLine::template(os.command(Command::Clone), &[repo, path])?)?;
        steps.push(if fetched { step("checkout", "done", "fetched through tolkster") } else { step("checkout", "fail", "fetching the recipe's checkout through tolkster failed") });
    }
    for (name, repo) in target.repos {
        let project = project_id(repo);
        if ready_on(run, host, Some(&project))? {
            steps.push(step(&format!("repo {name}"), "ok", "set up"));
            continue;
        }
        let path = os.join(target.destination, name);
        let cloned = on(run, CommandLine::template(os.command(Command::Clone), &[repo, &path])?)?;
        if !cloned {
            steps.push(step(&format!("repo {name}"), "fail", "cloning it through tolkster failed"));
            continue;
        }
        if setup_at(run, host, &path)?.is_none() {
            import_folder(run, host, &project, &path, "git", name)?;
        }
        let ready = ready_on(run, host, Some(&project))?;
        steps.push(step(&format!("repo {name}"), if ready { "done" } else { "fail" }, if ready { "cloned and set up" } else { "cloned, but Orca doesn't list it ready" }));
    }
    Ok(steps)
}

// Each repo `repos` maps, by its display name, with its `owner/name` from this machine's Orca,
// and the names of those it can't set up: none there on GitHub, or a name or `owner/name` a
// folder or shell line can't take.
pub(crate) fn mapped_repos(config: &Config, github: &[(String, String)]) -> (Vec<(String, String)>, Vec<String>) {
    let mut names: Vec<&str> = config.repos.0.iter().map(|entry| entry.repo.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    let (mut found, mut unknown) = (Vec::new(), Vec::new());
    for name in names {
        let plain = |repo: &str| plain_host(name) && !name.chars().all(|c| c == '.') && split_repo(repo).is_some();
        match github.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).filter(|(_, repo)| plain(repo)) {
            Some((n, repo)) => found.push((n.clone(), repo.clone())),
            None => unknown.push(name.to_string()),
        }
    }
    (found, unknown)
}

#[cfg(test)]
mod tests;
