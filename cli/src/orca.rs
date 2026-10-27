// Every call to `orca` is made here, and nowhere else. `orca` and `git` both run through
// `Run`, so a caller is tested over a fake shell, without Orca or a network.

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::error::Error;
use crate::exec::{Output, Run};
use crate::git;
use crate::health::Health;
use crate::github::remote_repo;
use crate::json::decode;
use crate::plain::{split_worktree, CommandLine, Line, Terminal as Handle};
use crate::trace;

mod call;
mod listing;
mod live;
mod routing;

pub use call::{call, take_listing_stale, Answer};
use call::{call_said, orca_json, orca_listed, orca_read, unsent, writes, Said};
pub use listing::*;
pub use live::Cli;
pub use routing::{paired, Host, Place};
use routing::{at, by_id, hosted, hosts_of, terminal_at, worktree_at, HOSTS};

// How a caller asks whether a host's own `tolkster` reads its config.json: one that can't takes
// no work. Err where the host can't be asked. The verdict is kept per host, whichever probe
// gave it (`Health::reads`), so every caller passes `onboard::reads_config`.
pub type ReadsConfig = fn(&mut Run, &str) -> Result<bool, Error>;

// How long a `Proof::Turn` line's receipt waits for Orca to see the agent's turn begin.
const SUBMIT_WITHIN_S: &str = "30";
// A refusal Orca gives while its terminal isn't yet writable (it names no reason) clears within
// seconds: a refused send is tried again up to REFUSED_RETRIES times, REFUSED_PAUSE apart.
const REFUSED_RETRIES: u32 = 6;
const REFUSED_PAUSE: Duration = Duration::from_secs(5);
// A wait for a terminal's exit is asked again after this: a wait left behind by a `listen`
// that has ended, which Windows doesn't end with it, lasts no longer.
const EXIT_WITHIN_MS: &str = "300000";

// The host a worktree's ID names, None for this machine.
pub fn host_of(worktree_id: &str) -> Option<&str> {
    split_worktree(worktree_id).and_then(|(host, _, _)| host)
}

pub fn origin_repo(run: &mut Run, path: Option<&str>) -> Result<String, Error> {
    let url = git::origin_url(run, path)?;
    remote_repo(&url).ok_or_else(|| Error::Failed("the checkout's origin is not a GitHub repo".into()))
}

// A Windows path compares with either slash, ignoring case, as Windows does; any other
// exactly, as on a case-sensitive volume.
fn same_path(a: &str, b: &str) -> bool {
    let windows = |p: &str| p.contains('\\') || p.as_bytes().get(1) == Some(&b':');
    let (a, b) = (a.trim(), b.trim());
    if windows(a) || windows(b) {
        let plain = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_ascii_lowercase();
        return plain(a) == plain(b);
    }
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

// This machine's worktrees, with their repos' origins.
pub fn list_worktrees(run: &mut Run) -> Result<Vec<Worktree>, Error> {
    let mut worktrees = Ps::read(ps(run, None)?)?.worktrees(None);
    read_origins(run, &mut worktrees);
    Ok(worktrees)
}

const PS: [&str; 5] = ["worktree", "ps", "--limit", "1000", "--json"];

fn ps(run: &mut Run, host: Option<&str>) -> Result<Value, Error> {
    orca_json(run, &at(host, &PS))
}

const TERMINALS: [&str; 5] = ["terminal", "list", "--limit", "1000", "--json"];

// Whether `host` answers both listings, the calls whose failure makes it silent (`Cli::asked`).
pub fn answers(run: &mut Run, host: &str) -> bool {
    ps(run, Some(host)).is_ok() && orca_json(run, &at(Some(host), &TERMINALS)).is_ok()
}

// The worktrees of one repo share its remotes, so its primary checkout, or else its first
// local worktree, answers for all of them. A failed read leaves their origin None. Git here
// can't reach another host's paths: a worktree there takes its origin from `hosted_origins`.
fn read_origins(run: &mut Run, worktrees: &mut [Worktree]) {
    let mut origins: HashMap<String, Option<String>> = HashMap::new();
    for repo_id in worktrees.iter().filter(|w| w.is_local).map(|w| w.repo_id.clone()).collect::<Vec<_>>() {
        if origins.contains_key(&repo_id) {
            continue;
        }
        let at = primary_checkout(worktrees, &repo_id).or_else(|| worktrees.iter().find(|w| w.is_local && w.repo_id == repo_id));
        let origin = at.and_then(|w| origin_repo(run, Some(w.path.as_str())).ok());
        origins.insert(repo_id, origin);
    }
    for worktree in worktrees.iter_mut().filter(|w| w.is_local) {
        worktree.origin = origins.get(&worktree.repo_id).cloned().flatten();
    }
}

// One `repo list` per other host with a worktree listed, as a host's repo IDs are its own. A
// failed listing leaves its worktrees' origins None.
fn hosted_origins(run: &mut Run, worktrees: &mut [Worktree]) {
    let mut hosts: Vec<String> = worktrees.iter().filter_map(|w| w.host().map(String::from)).collect();
    hosts.sort();
    hosts.dedup();
    for host in hosts {
        let origins = repo_list(run, Some(&host)).map(|listed| listed.origins()).unwrap_or_default();
        for worktree in worktrees.iter_mut().filter(|w| w.host() == Some(host.as_str())) {
            worktree.origin = origins.get(&worktree.repo_id).cloned();
        }
    }
}

// This machine's repos on GitHub, as (display name, `owner/name`): what `host add` sets up
// on a new host under the same names.
pub fn github_repos(run: &mut Run) -> Result<Vec<(String, String)>, Error> {
    let listed = repo_list(run, None)?;
    let origins = listed.origins();
    Ok(listed.repos.into_iter().filter_map(|repo| Some((repo.display_name, origins.get(&repo.id)?.clone()))).collect())
}

// `orca repo list`'s answer.
#[derive(Debug, Deserialize)]
struct RepoList {
    repos: Vec<RepoRow>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepoRow {
    id: String,
    display_name: String,
    path: String,
    // Null for a repo whose remote Orca can't read.
    git_remote_identity: Option<RemoteIdentity>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteIdentity {
    canonical_key: String,
}

fn repo_list(run: &mut Run, host: Option<&str>) -> Result<RepoList, Error> {
    orca_read::<Answer<RepoList>>(run, &at(host, &["repo", "list", "--json"])).map(|answer| answer.result)
}

impl RepoList {
    // Each repo's `owner/name` on GitHub by its ID, from its remote's canonical key
    // (`github.com/<owner>/<name>`): a repo elsewhere isn't listed.
    fn origins(&self) -> HashMap<String, String> {
        self.repos.iter().filter_map(|r| Some((r.id.clone(), remote_repo(&format!("https://{}", r.git_remote_identity.as_ref()?.canonical_key))?))).collect()
    }
}

// A repo Orca knows, by its ID and display name.
#[derive(Clone, Debug, PartialEq)]
pub struct Repo {
    pub id: String,
    pub name: String,
}

// One host's repos: a repo's ID means something only to the host that made it.
pub fn repos(run: &mut Run, host: Option<&str>) -> Result<Vec<Repo>, Error> {
    Ok(listed_repos(repo_list(run, host)?))
}

#[cfg(test)]
pub fn parse_repos(listed: Value) -> Result<Vec<Repo>, Error> {
    decode::<Answer<RepoList>>(listed, "Orca's answer to repo list").map(|answer| listed_repos(answer.result))
}

fn listed_repos(listed: RepoList) -> Vec<Repo> {
    listed.repos.into_iter().map(|r| Repo { id: r.id, name: r.display_name }).collect()
}

// None when no repo has this display name, or several do.
pub fn find_repo(repos: &[Repo], project: Option<&str>) -> Option<(String, String)> {
    let project = project?.trim();
    let matching: Vec<(String, String)> = repos.iter().filter(|r| r.name.trim().eq_ignore_ascii_case(project)).map(|r| (r.id.clone(), r.name.clone())).collect();
    match <[_; 1]>::try_from(matching) {
        Ok([one]) => Some(one),
        Err(_) => None,
    }
}

pub fn repo_name(repos: &[Repo], repo_id: &str) -> Option<String> {
    repos.iter().find(|r| r.id == repo_id).map(|r| r.name.clone())
}

// This machine's repos, as (display name, folder).
pub fn repo_folders(run: &mut Run) -> Result<Vec<(String, String)>, Error> {
    Ok(repo_list(run, None)?.repos.into_iter().map(|r| (r.display_name, r.path)).collect())
}

// `orca vm recipe doctor`'s report, bare or in Orca's envelope.
#[derive(Debug, Deserialize)]
struct Checked {
    ok: bool,
    result: Option<CheckedResult>,
}

#[derive(Debug, Deserialize)]
struct CheckedResult {
    ok: Option<bool>,
}

// Whether Orca's own checks pass the recipe `id` in the repo at `folder`, run nothing. A
// failing check fails the call, and a report of another shape passes nothing.
pub fn recipe_passes(run: &mut Run, id: &str, folder: &str) -> bool {
    let Ok(checked) = orca_read::<Checked>(run, &["vm", "recipe", "doctor", id, "--repo-path", folder, "--json"]) else { return false };
    checked.ok && checked.result.and_then(|result| result.ok) != Some(false)
}

// Pairs this machine's Orca with a server by its pairing code, a secret: Orca's error is
// shown without it, as `failure` shows no argument past the verb.
pub fn pair(run: &mut Run, name: &str, code: &str) -> Result<(), Error> {
    orca_json(run, &["environment", "add", "--name", name, "--pairing-code", code, "--json"]).map(|_| ())
}

pub fn unpair(run: &mut Run, host: &str) -> Result<(), Error> {
    orca_json(run, &["environment", "rm", "--environment", host, "--json"]).map(|_| ())
}

#[derive(Debug, Deserialize)]
struct HostName {
    platform: String,
}

// What a paired host's Orca says it runs on: `linux`, `darwin` or `win32`.
pub fn platform(run: &mut Run, host: &str) -> Result<String, Error> {
    orca_read::<Answer<HostName>>(run, &at(Some(host), &["host", "name", "--json"])).map(|answer| answer.result.platform)
}

// `orca project setups`'s answer: each setup of a project on a host.
#[derive(Debug, Deserialize)]
struct Setups {
    setups: Vec<Setup>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Setup {
    project_id: String,
    path: String,
    kind: String,
    setup_state: String,
}

impl Setup {
    fn ready(&self) -> bool {
        self.setup_state == "ready"
    }
}

// The setups on `host`, by its own Orca: `project`'s alone where one is named.
fn setups(run: &mut Run, host: &str, project: Option<&str>) -> Result<Vec<Setup>, Error> {
    let runtime = format!("runtime:{host}");
    let mut args = vec!["project", "setups"];
    if let Some(project) = project {
        args.extend_from_slice(&["--project", project]);
    }
    args.extend_from_slice(&["--host", &runtime, "--json"]);
    orca_read::<Answer<Setups>>(run, &args).map(|answer| answer.result.setups)
}

// The kind of `host`'s setup at `path` (`git` or `folder`), whatever its project, or None for
// no setup there.
pub fn setup_at(run: &mut Run, host: &str, path: &str) -> Result<Option<String>, Error> {
    Ok(setups(run, host, None)?.into_iter().find(|s| same_path(&s.path, path)).map(|s| s.kind))
}

// The folder of a setup ready on `host`, any project's: Orca opens a terminal on a paired
// host only in a worktree it knows there, so `host add` runs its steps in this one.
pub fn ready_folder(run: &mut Run, host: &str) -> Result<Option<String>, Error> {
    Ok(setups(run, host, None)?.into_iter().find(Setup::ready).map(|s| s.path))
}

// Imports the folder at `path` on `host`, already there, as `project`'s setup of `kind`
// under `display_name`.
pub fn import_folder(run: &mut Run, host: &str, project: &str, path: &str, kind: &str, display_name: &str) -> Result<(), Error> {
    let runtime = format!("runtime:{host}");
    orca_json(run, &["project", "setup-existing-folder", "--project", project, "--host", &runtime, "--path", path, "--kind", kind, "--display-name", display_name, "--json"]).map(|_| ())
}

// `orca terminal create`'s answer.
#[derive(Debug, Deserialize)]
struct Opened {
    terminal: OpenedTerminal,
}

#[derive(Debug, Deserialize)]
struct OpenedTerminal {
    handle: String,
}

// The handle of the terminal Orca's answer `created` names, on `host`: an empty one would
// name the active terminal.
fn opened(created: Value, host: Option<&str>) -> Result<String, Error> {
    let handle = decode::<Answer<Opened>>(created, "Orca's answer to terminal create")?.result.terminal.handle;
    match handle.is_empty() {
        true => Err(Error::Failed("Orca's answer to terminal create named no terminal".into())),
        false => Ok(hosted(host, &handle)),
    }
}

// A terminal opened on `host` in the worktree `worktree` (a selector its Orca takes) on
// `command`, which ends its shell, waited on until it exits, then closed: its exit code, None
// where it still ran at EXIT_WITHIN_MS or Orca names none.
pub fn run_on(run: &mut Run, host: &str, worktree: &str, command: &CommandLine) -> Result<Option<i32>, Error> {
    let created = orca_json(run, &at(Some(host), &["terminal", "create", "--worktree", worktree, "--command", command.as_str(), "--json"]))?;
    let handle = Handle::check(&opened(created, Some(host))?)?;
    exited_then_closed(run, &handle)
}

// The exit code of the terminal `handle` once it exits, None where it still ran at
// EXIT_WITHIN_MS or Orca names none. Its tab stays listed once it has exited, and a stale
// handle is no harm, so it is closed after; one still running, or whose wait failed, is closed
// too, so a retry's step never runs beside it.
fn exited_then_closed(run: &mut Run, handle: &Handle) -> Result<Option<i32>, Error> {
    let code = exit_code(run, handle);
    let _ = close_terminal(run, handle);
    Ok(match code? {
        Waited::Exited(code) => code,
        Waited::Open | Waited::Stale => None,
    })
}

// The ID Orca gives the GitHub repo `owner/name`, which it matches case-sensitively: Orca's
// is lower case, and a remote's canonical key keeps GitHub's case.
pub fn project_id(repo: &str) -> String {
    format!("github:{}", repo.to_ascii_lowercase())
}

// The projects (`project_id`) set up and ready on `host`, by its own Orca, sorted: only those
// of a plain shape, as the Durable Object takes (src/worker/machine/fleet.ts, PROJECT), and at
// most MAX_READY, so a host's other projects can't fail its entry.
const MAX_READY: usize = 100;

pub fn plain_project(project: &str) -> bool {
    (1..=200).contains(&project.len()) && project.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '-'))
}

pub fn ready_projects_on(run: &mut Run, host: &str) -> Result<Vec<String>, Error> {
    let mut projects: Vec<String> = setups(run, host, None)?.into_iter().filter(Setup::ready).map(|s| s.project_id).filter(|p| plain_project(p)).take(MAX_READY).collect();
    projects.sort();
    projects.dedup();
    Ok(projects)
}

// Whether `project` (`project_id`), or with None any project, is set up and ready
// on `host`, by its own Orca.
pub fn ready_on(run: &mut Run, host: &str, project: Option<&str>) -> Result<bool, Error> {
    Ok(setups(run, host, project)?.iter().any(|s| project.is_none_or(|project| s.project_id.eq_ignore_ascii_case(project)) && s.ready()))
}

// The comment on a worktree's card in Orca, which the owner reads there. One with a
// leading dash is refused, so Orca never reads it as an option.
pub fn set_comment(run: &mut Run, worktree_id: &str, comment: &str) -> Result<(), Error> {
    if comment.starts_with('-') {
        return Err(Error::Usage("a worktree's comment can't start with a dash".into()));
    }
    let (host, worktree) = worktree_at(worktree_id);
    call(run, &at(host, &["worktree", "set", "--worktree", &worktree, "--comment", comment, "--json"])).map(|_| ())
}

// The column of a worktree's card on Orca's board, by the ID of one of its default columns:
// fixed words, never text from Linear or GitHub.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    InProgress,
    InReview,
}

impl Status {
    pub fn id(self) -> &'static str {
        match self {
            Status::InProgress => "in-progress",
            Status::InReview => "in-review",
        }
    }
}

pub fn set_status(run: &mut Run, worktree_id: &str, status: Status) -> Result<(), Error> {
    let (host, worktree) = worktree_at(worktree_id);
    call(run, &at(host, &["worktree", "set", "--worktree", &worktree, "--workspace-status", status.id(), "--json"])).map(|_| ())
}

#[derive(Debug, Deserialize)]
struct Shown {
    worktree: ShownWorktree,
}

#[derive(Debug, Deserialize)]
struct ShownWorktree {
    comment: String,
}

// The worktree card's comment as Orca has it now; None when it has none.
pub fn comment(run: &mut Run, worktree_id: &str) -> Result<Option<String>, Error> {
    let (host, worktree) = worktree_at(worktree_id);
    let shown: Answer<Shown> = orca_read(run, &at(host, &["worktree", "show", "--worktree", &worktree, "--json"]))?;
    Ok(Some(shown.result.worktree.comment).filter(|comment| !comment.is_empty()))
}

pub fn close_terminal(run: &mut Run, handle: &Handle) -> Result<(), Error> {
    let (host, handle) = terminal_at(handle);
    call(run, &at(host, &["terminal", "close", "--terminal", handle, "--json"])).map(|_| ())
}

// What a wait on a terminal's exit answered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Waited {
    // Still open at EXIT_WITHIN_MS.
    Open,
    // Exited, with the code its program ended with where Orca names one.
    Exited(Option<i32>),
    // Orca no longer knows the handle, as for one that closed before the wait began.
    Stale,
}

// Blocks until the terminal `handle`, `issue`'s agent's where the listing knows it, exits,
// however it went: true once Orca says it has (`Waited`), false when it is still open. A terminal on another host is waited on there, and
// on a silent one (health.rs) not at all. Orca's answer is no proof the terminal is gone: the
// watcher checks a listing.
pub fn wait_exit(run: &mut Run, health: &Health, handle: &Handle, issue: Option<String>) -> Result<bool, Error> {
    if let (Some(host), _) = terminal_at(handle)
        && health.silent(host)
    {
        return Err(Error::silent(host));
    }
    let said = match exit_code(run, handle)? {
        Waited::Open => return Ok(false),
        Waited::Exited(code) => trace::Closed::Exited(code),
        Waited::Stale => trace::Closed::Stale,
    };
    trace::event(trace::Watch::Waited { issue, handle: handle.to_string(), said });
    Ok(true)
}

// `orca terminal wait`'s answer.
#[derive(Debug, Deserialize)]
struct WaitAnswer {
    wait: Wait,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Wait {
    satisfied: bool,
    // Null where the program ended with no code Orca knows.
    exit_code: Option<i64>,
}

fn exit_code(run: &mut Run, handle: &Handle) -> Result<Waited, Error> {
    let (host, handle) = terminal_at(handle);
    let (waited, said) = call_said(run, &at(host, &["terminal", "wait", "--terminal", handle, "--for", "exit", "--timeout-ms", EXIT_WITHIN_MS, "--json"]));
    match waited {
        Ok(stdout) => {
            // The wait ran: an answer of another shape proves no exit, so the terminal stays open.
            let Ok(Answer { result: WaitAnswer { wait } }) = serde_json::from_str::<Answer<WaitAnswer>>(&stdout) else { return Ok(Waited::Open) };
            let code = wait.exit_code.and_then(|code| i32::try_from(code).ok());
            Ok(if wait.satisfied { Waited::Exited(code) } else { Waited::Open })
        }
        Err(error) => match said.as_deref().and_then(Said::of).as_ref().and_then(Said::code) {
            Some("timeout") => Ok(Waited::Open),
            Some("terminal_handle_stale") => Ok(Waited::Stale),
            _ => Err(error),
        },
    }
}

// What a typed line must show to count as delivered. A round is typed into a worker that
// should start a turn on it, so it needs the turn (`turn_started`). A message is typed into a
// worker usually mid-turn, whose turn it can't start: Orca taking the input (`input_accepted`)
// is delivery, and the agent reads it when its turn ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Proof {
    Turn,
    Input,
}

// What came of a line typed into a running agent, as its `Proof` reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Typed {
    Delivered,
    // The agent waits at a permission prompt, which the line didn't pass.
    Waiting,
    // Accepted, without the proof: it may sit unsent in the agent's input.
    Unproven,
}

// `orca terminal send`'s receipt.
#[derive(Debug, Deserialize)]
struct Receipt {
    send: Sent,
}

#[derive(Debug, Deserialize)]
struct Sent {
    // None where Orca watched no prompt it could follow.
    prompt: Option<Prompt>,
}

#[derive(Debug, Deserialize)]
struct Prompt {
    stages: Vec<String>,
    observation: Option<String>,
}

// A send's receipt: a prompt whose stages reach `turn_started` began a turn, which delivers
// any line; `input_accepted` delivers a `Proof::Input` one. Any other is unproven, unless Orca
// saw a permission prompt stop it. The send exited 0, so the line went in: a receipt of
// another shape, or none, is unproven too, never a failure a caller may take for nothing
// typed.
fn typed(stdout: &str, proof: Proof) -> Typed {
    let Ok(Answer { result: Receipt { send: Sent { prompt: Some(prompt) } } }) = serde_json::from_str::<Answer<Receipt>>(stdout) else { return Typed::Unproven };
    let reached = |stage: &str| prompt.stages.iter().any(|s| s == stage);
    match (reached("turn_started"), prompt.observation.as_deref()) {
        (true, _) => Typed::Delivered,
        (false, Some("permission")) => Typed::Waiting,
        (false, _) if proof == Proof::Input && reached("input_accepted") => Typed::Delivered,
        (false, _) => Typed::Unproven,
    }
}

// What `listen`'s commands and handlers ask of Orca (command.rs, executor.rs,
// reap.rs, converse.rs), as typed facts: `Cli` asks Orca's CLI through a shell, and tests an
// in-memory Orca (ports/fake.rs). The watcher's wait for a terminal's exit (`wait_exit`) is
// the one call outside it. The checks on what a command may act on stay with the caller: none is
// made or skipped here.
//
// A call on a worktree or a terminal goes to the host its ID or handle names, and a listing
// asks every paired host and merges the answers. A host that fails a listing is silent
// (health.rs) for the rest of the run and every later one until it answers again: left out of
// listings, and a call aimed at it fails at once (`Error::Later`). What it, or a host no
// longer paired, holds is unknown, never gone (`heard`), so `listed` is None while one is
// unanswered, and a lookup that misses there decides nothing (`Error::Later`). One that
// answered and then fails fails the call, as this machine's always does: a stop would close
// nothing, a start would make a second worker, and a line could be typed twice.
pub trait Orca {
    // The worktrees with their agents, without their origins, which cost a git read per repo.
    fn agents(&mut self) -> Result<Vec<Worktree>, Error> {
        self.agents_listed().map(|(worktrees, _)| worktrees)
    }
    // The worktrees with their agents, and every worktree's ID when each host's listing is
    // complete (`complete_listing`), from one listing per host.
    fn agents_listed(&mut self) -> Result<(Vec<Worktree>, Option<Vec<String>>), Error>;
    // Every worktree's ID, None when the listing is incomplete.
    fn listed(&mut self) -> Result<Option<Vec<String>>, Error>;
    // Forgets which hosts answered this run's listings, so the next asks each again: one woken
    // since was silent.
    fn ask_again(&mut self) {}
    // The worktrees with their repos' origins.
    fn worktrees(&mut self) -> Result<Vec<Worktree>, Error>;
    fn terminals(&mut self) -> Result<Vec<Terminal>, Error>;
    // `host`, None for this machine, listed now and there alone: a line typed into an agent, a
    // close of terminals a listing named and a removal decide on their worktree's, read after
    // the command's last Orca write before that decision.
    // One unpaired or silent fails at once, as a call aimed at it does.
    fn listing_on(&mut self, host: Option<&str>) -> Result<Listing, Error>;
    // The host a worktree's ID or a terminal's handle names, listed there alone.
    fn listing_at(&mut self, worktree_id: &str) -> Result<Listing, Error> {
        self.listing_on(host_of(worktree_id))
    }
    fn listing_of(&mut self, handle: &Handle) -> Result<Listing, Error> {
        self.listing_on(terminal_at(handle).0)
    }
    // Each worktree's repo's origin, as `worktrees` carries it.
    fn origins(&mut self, worktrees: &mut [Worktree]);
    // The repos of `host`, None for this machine.
    fn repos(&mut self, host: Option<&str>) -> Result<Vec<Repo>, Error>;
    // The paired hosts, in the order paired, but for those that haven't answered: asked by a
    // listing first.
    fn answering(&self) -> Vec<Host>;
    // The paired hosts that haven't answered.
    fn silent(&self) -> Vec<Host>;
    fn unanswered(&self) -> Vec<String> {
        self.silent().into_iter().map(|h| h.id).collect()
    }
    // Whether the host `worktree_id` is on answered this run: this machine always.
    fn heard(&self, worktree_id: &str) -> bool {
        host_of(worktree_id).is_none_or(|host| self.answering().iter().any(|h| h.id == host))
    }
    // Whether `project`, or with None any project, is set up and ready on `host`, whose
    // `tolkster` reads its config.json as `reads_config` asks it.
    fn ready(&mut self, host: &str, project: Option<&str>, reads_config: ReadsConfig) -> Result<bool, Error>;
    // The projects set up and ready on `host`, none where `ready` would say no project is.
    fn ready_projects(&mut self, host: &str, reads_config: ReadsConfig) -> Result<Vec<String>, Error>;
    // One line typed into a running agent, never twice. Only a `Proof::Turn` send waits for
    // the turn to begin.
    fn send(&mut self, worktree_id: &str, handle: &Handle, line: &Line, proof: Proof) -> Result<Typed, Error>;
    // One line typed into a terminal no agent runs in, a warm turn's (`converse`), whose
    // program reads it: Orca's acceptance is all there is to wait on.
    fn type_line(&mut self, handle: &Handle, line: &Line) -> Result<(), Error>;
    // An agent launched in a new terminal on `command`, which starts it (setup.rs): its handle.
    fn start_agent(&mut self, worktree_id: &str, command: &CommandLine) -> Result<String, Error>;
    // A terminal opened in a worktree on `command`: its handle.
    fn open(&mut self, worktree_id: &str, title: Option<&str>, command: &CommandLine) -> Result<String, Error>;
    // A terminal opened in a worktree on `command`, a `tolkster` step that ends its shell, and
    // waited on until it exits, then closed: its exit code, None where it still ran at
    // EXIT_WITHIN_MS or Orca names none.
    fn run_to_exit(&mut self, worktree_id: &str, command: &CommandLine) -> Result<Option<i32>, Error>;
    fn close(&mut self, handle: &Handle) -> Result<(), Error>;
    // Every terminal in a worktree closed.
    fn close_all(&mut self, worktree_id: &str) -> Result<(), Error>;
    // A new worktree's ID and path, at `place`.
    fn create_worktree(&mut self, place: Place, name: &str, options: &[&str]) -> Result<(String, String), Error>;
    fn remove_worktree(&mut self, worktree_id: &str) -> Result<(), Error>;
    fn name_worktree(&mut self, worktree_id: &str, display_name: &str) -> Result<(), Error>;
    // The worktree's card moved to `status`'s column on the board.
    fn set_status(&mut self, worktree_id: &str, status: Status) -> Result<(), Error>;
    // The shell beside Orca, for git alone: every call to `orca` is one of the above.
    fn shell(&mut self) -> &mut Run<'_>;
}


#[cfg(test)]
pub mod tests;
