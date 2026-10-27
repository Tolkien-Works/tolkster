// Orca's CLI asked through a shell: the paired hosts it reaches, and which of them have answered.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

use super::*;
use crate::error::Waits;
use crate::git;
use crate::health::Health;
use crate::json::decode;
use crate::plain::{CommandLine, Line, Terminal as Handle};
use crate::trace;

// `hosts` are the paired hosts, which every listing asks beside this machine's, and the only
// hosts a call is sent to: None until the first call that needs them reads them, so a
// command that never reaches Orca never fails on it. Each answers or not once for the run's
// listings (`asked`), so one command never sees a host's worktrees listed and its terminals
// not: `health`, which other runs change, decides only for a host no listing of this run has
// asked yet. `waits_from` is the thread's Orca write count as a command's `Cli` was made
// (`waiting`).
pub struct Cli<'r> {
    pub(super) run: &'r mut Run<'r>,
    pub(super) hosts: Option<Vec<Host>>,
    pub(super) health: &'r Health,
    pub(super) answered: HashSet<String>,
    pub(super) unanswered: HashSet<String>,
    pub(super) refused_pause: Duration,
    pub(super) at_once: Option<Ask>,
    pub(super) waits_from: Option<u64>,
}

// The program runner a listing's threads call, which a `Run` borrowing its caller can't be.
pub type Ask = fn(&str, &[&str], Option<&str>) -> Result<Output, Error>;

type Answered = Vec<(Option<String>, Vec<Value>)>;

// A listing call's answer, and whether it found Orca absent (`Cli::failed`).
type Listed = (Result<Value, Error>, bool);

impl<'r> Cli<'r> {
    // This machine's Orca alone.
    pub fn new(run: &'r mut Run<'r>, health: &'r Health) -> Cli<'r> {
        Cli { run, hosts: Some(Vec::new()), health, answered: HashSet::new(), unanswered: HashSet::new(), refused_pause: REFUSED_PAUSE, at_once: None, waits_from: None }
    }

    // A listing asks every host at once, each call on a thread of its own made by `ask`, so it
    // costs the slowest host's, not their sum. Without, the calls are made in turn by `run`.
    pub fn at_once(self, ask: Ask) -> Cli<'r> {
        Cli { at_once: Some(ask), ..self }
    }

    // A command's: a listing that finds Orca absent waits on it, as a read does (`orca_read`),
    // until the command's first Orca write.
    pub fn waiting(self) -> Cli<'r> {
        Cli { waits_from: Some(writes()), ..self }
    }

    // This machine's Orca and every host paired with it as the first call needs them.
    pub fn paired(run: &'r mut Run<'r>, health: &'r Health) -> Cli<'r> {
        Cli { hosts: None, ..Cli::new(run, health) }
    }

    // This machine's Orca alone, with a health of its own, so no test sees another's silent hosts.
    #[cfg(test)]
    pub fn alone(run: &'r mut Run<'r>) -> Cli<'r> {
        Cli::new(run, Box::leak(Box::default()))
    }

    #[cfg(test)]
    pub fn with_hosts(run: &'r mut Run<'r>, hosts: &[&str]) -> Cli<'r> {
        Cli::sharing(run, hosts, Box::leak(Box::default()))
    }

    #[cfg(test)]
    pub fn sharing(run: &'r mut Run<'r>, hosts: &[&str], health: &'r Health) -> Cli<'r> {
        Cli { hosts: Some(hosts.iter().map(|h| Host { id: h.to_string(), name: h.to_string() }).collect()), ..Cli::new(run, health) }
    }

    fn hosts(&mut self) -> Result<&[Host], Error> {
        if self.hosts.is_none() {
            let (listed, absent) = orca_listed(self.run, &HOSTS);
            self.hosts = Some(hosts_of(listed.map_err(|error| self.failed(error, absent))?)?);
        }
        Ok(self.hosts.as_deref().unwrap_or_default())
    }

    fn every_host(&mut self) -> Result<Vec<Option<String>>, Error> {
        Ok(std::iter::once(None).chain(self.hosts()?.iter().map(|h| Some(h.id.clone()))).collect())
    }

    // An ID or handle from the Durable Object may name any plain host: one not paired is
    // refused, never reached, and a silent one isn't waited on.
    fn known<'h>(&mut self, host: Option<&'h str>) -> Result<Option<&'h str>, Error> {
        match host {
            Some(host) if !self.hosts()?.iter().any(|h| h.id == host) => Err(Error::Failed(format!("{host} is not a paired host"))),
            Some(host) if self.is_silent(host) => Err(Error::silent(host)),
            host => Ok(host),
        }
    }

    // Whether the `tolkster` on `host` reads its config.json: one that can't would fail a
    // worker's first call. One that can't be asked or names no exit code counts as refusing,
    // so a slow host is not asked again by every listing.
    fn reads_config(&mut self, host: &str, ask: ReadsConfig) -> bool {
        if let Some(reads) = self.health.reads(host, Instant::now()) {
            return reads;
        }
        let reads = ask(self.run, host).unwrap_or(false);
        self.health.read(host, reads, Instant::now());
        reads
    }

    // What a listing's failed call fails with. One that found Orca absent before its command's
    // first Orca write waits on it; after, the command sent again would repeat that write, a
    // line typed included (`send`'s `waits_now`), so it fails.
    fn failed(&self, error: Error, absent: bool) -> Error {
        match self.waits_from {
            Some(from) if absent && from == writes() => Error::Waits(Waits::Orca, error.to_string()),
            _ => error,
        }
    }

    fn is_silent(&self, host: &str) -> bool {
        self.unanswered.contains(host) || (!self.answered.contains(host) && self.health.silent(host))
    }

    // A listing's call to `host`, None once that host hasn't answered: `early` when it was
    // asked already, else asked now. One that fails is silent for the calls after it, and fails
    // this one if it answered before.
    fn asked(&mut self, host: Option<&str>, args: &[&str], early: Option<Listed>) -> Result<Option<Value>, Error> {
        let answer = |run: &mut Run| early.unwrap_or_else(|| orca_listed(run, &at(host, args)));
        let Some(host) = host else {
            let (answer, absent) = answer(self.run);
            return answer.map(Some).map_err(|error| self.failed(error, absent));
        };
        if self.is_silent(host) {
            self.unanswered.insert(host.to_string());
            return Ok(None);
        }
        let (error, absent) = match answer(self.run) {
            (Ok(answer), _) => {
                if self.answered.insert(host.to_string()) {
                    self.health.heard(host, Instant::now());
                }
                return Ok(Some(answer));
            }
            (Err(error), absent) => (error, absent),
        };
        // An absent Orca is this machine's, never the host's fault: one the command waits on
        // marks no host silent.
        let error = match self.failed(error, absent) {
            Error::Waits(waits, why) => return Err(Error::Waits(waits, why)),
            error => error,
        };
        let name: Option<String> = self.hosts().ok().and_then(|hosts| hosts.iter().find(|h| h.id == host).map(|h| h.name.chars().filter(|c| !c.is_control()).collect()));
        if self.health.stopped(host, name.clone(), Instant::now()) {
            trace::event(trace::Host::NotAnswering { name, id: host.to_string(), error: error.clone() });
        }
        if self.answered.contains(host) {
            return Err(error);
        }
        self.unanswered.insert(host.to_string());
        Ok(None)
    }

    // `calls` asked of each of `hosts` not silent, in turn, or at once by `at_once`: the hosts
    // that answered every one, with their answers in order.
    fn ask_every(&mut self, hosts: Vec<Option<String>>, calls: &[&[&str]]) -> Result<Answered, Error> {
        let mut early = match self.at_once {
            Some(ask) => self.ask_at_once(ask, &hosts, calls),
            None => Vec::new(),
        };
        let mut answered = Vec::new();
        'hosts: for (n, host) in hosts.into_iter().enumerate() {
            let mut early = early.get_mut(n).and_then(Option::take).map(Vec::into_iter);
            let mut answers = Vec::new();
            for args in calls {
                let Some(answer) = self.asked(host.as_deref(), args, early.as_mut().and_then(Iterator::next))? else { continue 'hosts };
                answers.push(answer);
            }
            answered.push((host, answers));
        }
        Ok(answered)
    }

    // Each call to each host not silent now, made at once: the first host's first by `run`,
    // whose beat shows the command at work, every other by `ask` on a thread that acts as the
    // command's job (`trace::acting_as`).
    fn ask_at_once(&mut self, ask: Ask, hosts: &[Option<String>], calls: &[&[&str]]) -> Vec<Option<Vec<Listed>>> {
        let asking: Vec<Option<&str>> = hosts.iter().map(Option::as_deref).filter(|host| host.is_none_or(|host| !self.is_silent(host))).collect();
        let job = trace::job();
        let one = |host: Option<&str>, args: &[&str]| {
            let _acting = job.as_ref().map(|(id, issue, kind)| trace::acting_as(id, issue.as_deref(), kind.clone()));
            let mut ask = ask;
            orca_listed(&mut ask, &at(host, args))
        };
        let gone = || (Err(Error::Orca("an orca listing's thread panicked".into())), false);
        let mut answers = std::thread::scope(|scope| {
            let threads: Vec<Vec<_>> = asking.iter().enumerate().map(|(h, &host)| calls.iter().enumerate().map(|(n, &args)| (h + n > 0).then(|| scope.spawn(move || one(host, args)))).collect()).collect();
            let mut here = asking.first().map(|&host| orca_listed(self.run, &at(host, calls[0])));
            let joined = threads.into_iter().map(|calls| calls.into_iter().map(|call| call.map_or_else(|| here.take().unwrap_or_else(gone), |thread| thread.join().unwrap_or_else(|_| gone()))).collect());
            joined.collect::<Vec<Vec<_>>>().into_iter()
        });
        hosts.iter().map(|host| asking.contains(&host.as_deref()).then(|| answers.next()).flatten()).collect()
    }

    // `host`'s worktrees and terminals, listed there alone. One no longer paired may hold a
    // worktree a record names, so it decides nothing, as a silent one doesn't.
    fn host_listing(&mut self, host: Option<&str>) -> Result<Listing, Error> {
        let host = match self.known(host) {
            Err(Error::Failed(why)) => return Err(Error::Later(why)),
            host => host?,
        };
        let silent = || host.map_or_else(|| Error::Later("this machine didn't answer".into()), Error::silent);
        let (_, answers) = self.ask_every(vec![host.map(String::from)], &[&PS, &TERMINALS])?.pop().ok_or_else(silent)?;
        let [ps, terminals] = <[Value; 2]>::try_from(answers).map_err(|_| silent())?;
        let ps = Ps::read(ps)?;
        Ok(Listing { worktrees: ps.worktrees(host), listed: ps.complete(host), terminals: parse_terminals(terminals, host)? })
    }

    fn worktree_at<'w>(&mut self, worktree_id: &'w WorktreeId) -> Result<(Option<&'w str>, String), Error> {
        let (host, worktree) = worktree_at(worktree_id);
        Ok((self.known(host)?, worktree))
    }

    fn terminal_at<'t>(&mut self, handle: &'t Handle) -> Result<(Option<&'t str>, &'t str), Error> {
        let (host, handle) = terminal_at(handle);
        Ok((self.known(host)?, handle))
    }

    // Whether the agents in a worktree wait at a prompt now, its host read afresh for a send's
    // retry: a caller decides on its own read of the host (`waits_in`).
    fn waits_now(&mut self, worktree_id: &WorktreeId) -> Result<bool, Error> {
        let listing = self.listing_at(worktree_id)?;
        Ok(listing.worktree(worktree_id).is_some_and(|w| waits_in(w, &listing.terminals)))
    }
}

// `orca worktree create`'s answer.
#[derive(Debug, Deserialize)]
struct Made {
    worktree: MadeWorktree,
}

#[derive(Debug, Deserialize)]
struct MadeWorktree {
    id: String,
    path: String,
}

impl Orca for Cli<'_> {
    fn agents_listed(&mut self) -> Result<(Vec<Worktree>, Option<Vec<WorktreeId>>), Error> {
        let (mut worktrees, mut listed) = (Vec::new(), Some(Vec::new()));
        let hosts = self.every_host()?;
        for (host, answers) in self.ask_every(hosts, &[&PS])? {
            let host = host.as_deref();
            let ps = Ps::read(answers.into_iter().next().unwrap_or_default())?;
            worktrees.extend(ps.worktrees(host));
            listed = listed.zip(ps.complete(host)).map(|(mut all, ids)| {
                all.extend(ids);
                all
            });
        }
        Ok((worktrees, listed))
    }
    fn listed(&mut self) -> Result<Option<Vec<WorktreeId>>, Error> {
        let (_, listed) = self.agents_listed()?;
        Ok(listed.filter(|_| self.unanswered.is_empty()))
    }
    fn ask_again(&mut self) {
        self.answered.clear();
        self.unanswered.clear();
    }
    fn worktrees(&mut self) -> Result<Vec<Worktree>, Error> {
        let mut worktrees = self.agents()?;
        self.origins(&mut worktrees);
        Ok(worktrees)
    }
    fn origins(&mut self, worktrees: &mut [Worktree]) {
        read_origins(self.run, worktrees);
        hosted_origins(self.run, worktrees);
    }
    fn terminals(&mut self) -> Result<Vec<Terminal>, Error> {
        let hosts = self.every_host()?;
        let mut terminals = Vec::new();
        for (host, answers) in self.ask_every(hosts, &[&TERMINALS])? {
            terminals.extend(parse_terminals(answers.into_iter().next().unwrap_or_default(), host.as_deref())?);
        }
        Ok(terminals)
    }
    fn listing_on(&mut self, host: Option<&str>) -> Result<Listing, Error> {
        self.host_listing(host)
    }
    fn repos(&mut self, host: Option<&str>) -> Result<Vec<Repo>, Error> {
        let host = self.known(host)?;
        repos(self.run, host)
    }
    fn answering(&self) -> Vec<Host> {
        self.hosts.iter().flatten().filter(|h| !self.is_silent(&h.id)).cloned().collect()
    }
    fn silent(&self) -> Vec<Host> {
        self.hosts.iter().flatten().filter(|h| self.is_silent(&h.id)).cloned().collect()
    }
    fn ready(&mut self, host: &str, project: Option<&str>, reads_config: ReadsConfig) -> Result<bool, Error> {
        self.known(Some(host))?;
        Ok(ready_on(self.run, host, project)? && self.reads_config(host, reads_config))
    }
    fn ready_projects(&mut self, host: &str, reads_config: ReadsConfig) -> Result<Vec<String>, Error> {
        self.known(Some(host))?;
        let projects = ready_projects_on(self.run, host)?;
        Ok(if !projects.is_empty() && self.reads_config(host, reads_config) { projects } else { Vec::new() })
    }
    // One plain line: a line starting with a dash, or with an unbalanced quote, breaks in the
    // shell the terminal runs. A line is never typed twice (docs/dispatcher.md): a failed send
    // is tried again only as Orca's own JSON allows, and one it can't prove delivered, or
    // printed nothing about, is never sent again. A `Proof::Turn` send waits on its receipt for
    // the turn it starts, and a re-issue with Orca's request ID replays that receipt, never the
    // line.
    fn send(&mut self, worktree_id: &WorktreeId, handle: &Handle, line: &Line, proof: Proof) -> Result<Typed, Error> {
        let (host, handle) = self.terminal_at(handle)?;
        let args = match proof {
            Proof::Turn => at(host, &["terminal", "send", "--terminal", handle, "--enter", "--text", line.as_str(), "--wait-submit", SUBMIT_WITHIN_S, "--json"]),
            Proof::Input => at(host, &["terminal", "send", "--terminal", handle, "--enter", "--text", line.as_str(), "--json"]),
        };
        let mut refusals = 0;
        loop {
            let (result, said) = call_said(self.run, &args);
            let error = match result {
                Ok(stdout) => return Ok(typed(&stdout, proof)),
                Err(error) => error,
            };
            // Stdout that isn't Orca's JSON of this shape says nothing, so allows no retry.
            let Some(said) = said.as_deref().and_then(Said::of) else { return Err(error) };
            if said.data().and_then(|data| data.retry_safe) == Some(false) {
                return Err(error);
            }
            if let Some(request) = said.data().and_then(|data| data.orchestration_request_id.as_deref()) {
                return Ok(typed(&call(self.run, &[&args[..args.len() - 1], &["--retry-request", request, "--json"]].concat())?, proof));
            }
            // Orca's verdict on delivery outranks this: only an error it didn't qualify proves
            // nothing was typed, and a caller may act on that (`executor::round`).
            if let Some(why) = unsent(&said) {
                return Err(Error::Unsent(why, error.to_string()));
            }
            // No permission to type doesn't clear on a second try.
            if !said.refused() || said.refused_reason() == Some("permission") {
                return Err(error);
            }
            refusals += 1;
            if refusals > REFUSED_RETRIES {
                return Err(error);
            }
            std::thread::sleep(self.refused_pause);
            // A listing that fails counts as waiting, and the send's own error is the one reported.
            if self.waits_now(worktree_id).unwrap_or(true) {
                return Err(error);
            }
        }
    }
    fn type_line(&mut self, handle: &Handle, line: &Line) -> Result<(), Error> {
        let (host, handle) = self.terminal_at(handle)?;
        call(self.run, &at(host, &["terminal", "send", "--terminal", handle, "--enter", "--text", line.as_str(), "--json"])).map(|_| ())
    }
    // The agent's first prompt goes on its command line (harness::command), never typed: a line typed
    // before the agent can take it is lost, and no wait on the terminal proves it can. That
    // its first turn began is for the machine to see.
    fn start_agent(&mut self, worktree_id: &WorktreeId, command: &CommandLine) -> Result<String, Error> {
        self.open(worktree_id, None, command)
    }
    fn open(&mut self, worktree_id: &WorktreeId, title: Option<&str>, command: &CommandLine) -> Result<String, Error> {
        let (host, selector) = self.worktree_at(worktree_id)?;
        let mut args = vec!["terminal", "create", "--worktree", &selector];
        if let Some(title) = title {
            args.extend_from_slice(&["--title", title]);
        }
        args.extend_from_slice(&["--command", command.as_str(), "--json"]);
        // Made by now, whatever it printed: Orca's error, so a command that answers one isn't
        // tried again into a second terminal (command.rs, `startContext`).
        let created: Value = serde_json::from_str(&call(self.run, &at(host, &args))?).map_err(|error| Error::Orca(format!("orca terminal create printed no JSON: {error}")))?;
        opened(created, host).map_err(|error| Error::Orca(error.to_string()))
    }
    fn run_to_exit(&mut self, worktree_id: &WorktreeId, command: &CommandLine) -> Result<Option<i32>, Error> {
        let handle = Handle::check(&self.open(worktree_id, None, command)?)?;
        exited_then_closed(self.run, &handle)
    }
    fn close(&mut self, handle: &Handle) -> Result<(), Error> {
        self.terminal_at(handle)?;
        close_terminal(self.run, handle)
    }
    fn close_all(&mut self, worktree_id: &WorktreeId) -> Result<(), Error> {
        let (host, worktree) = self.worktree_at(worktree_id)?;
        call(self.run, &at(host, &["terminal", "close", "--worktree", &worktree, "--all", "--json"])).map(|_| ())
    }
    // On a host, by `--host runtime:<id>` rather than `--environment`: Orca makes it on that
    // host's setup of the project.
    fn create_worktree(&mut self, place: Place, name: &str, options: &[&str]) -> Result<(WorktreeId, String), Error> {
        let (host, made_by) = match place {
            Place::Here { repo_id } => (None, ["--repo".to_string(), by_id(repo_id)]),
            Place::On { host, project } => (self.known(Some(host))?, ["--project".to_string(), project.to_string()]),
        };
        let runtime = host.map(|host| format!("runtime:{host}"));
        let mut args = vec!["worktree", "create", made_by[0].as_str(), made_by[1].as_str()];
        if let Some(runtime) = &runtime {
            args.extend_from_slice(&["--host", runtime]);
        }
        args.extend_from_slice(&["--name", name]);
        args.extend_from_slice(options);
        args.push("--json");
        let created: Answer<Made> = {
            let _writing = git::writing();
            decode(orca_json(self.run, &args)?, "Orca's answer to worktree create")?
        };
        let made = created.result.worktree;
        let id = WorktreeId::hosted(host, &made.id).ok_or_else(|| Error::Failed("Orca's answer to worktree create named a worktree ID that isn't plain".into()))?;
        Ok((id, made.path))
    }
    fn remove_worktree(&mut self, worktree_id: &WorktreeId) -> Result<(), Error> {
        let (host, worktree) = self.worktree_at(worktree_id)?;
        let _writing = git::writing();
        call(self.run, &at(host, &["worktree", "rm", "--worktree", &worktree, "--force", "--json"])).map(|_| ())
    }
    fn name_worktree(&mut self, worktree_id: &WorktreeId, display_name: &str) -> Result<(), Error> {
        let (host, worktree) = self.worktree_at(worktree_id)?;
        call(self.run, &at(host, &["worktree", "set", "--worktree", &worktree, "--display-name", display_name, "--json"])).map(|_| ())
    }
    fn set_status(&mut self, worktree_id: &WorktreeId, status: Status) -> Result<(), Error> {
        let (host, worktree) = self.worktree_at(worktree_id)?;
        call(self.run, &at(host, &["worktree", "set", "--worktree", &worktree, "--workspace-status", status.id(), "--json"])).map(|_| ())
    }
    fn shell(&mut self) -> &mut Run<'_> {
        self.run
    }
}
