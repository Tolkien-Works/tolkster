// The fake Orca (ports.rs) and the World it shows, shared by every lane on it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::error::{Error, Unsent};
use crate::exec::{Output, Run};
use crate::orca::{Host, Listing, Pane, Place, Proof, ReadsConfig, Repo, Status, Terminal, Typed, Worktree, waits_in};
use crate::runtime::Shared;
use crate::plain::{CommandLine, Line, Terminal as Handle, orca_worktree, split_terminal, split_worktree};
use crate::sessions::Tolksters;

// The project every repo is on a host in `ready_on`.
pub const FAKE_PROJECT: &str = "github:acme/app";

// What a line typed into a working agent comes to, as `Orca::send` answers after its
// own one resend.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Send {
    #[default]
    Begins,
    Unproven,
    Fails,
    // The agent exits as the send reaches it: Orca refuses with `no-agent`, typing nothing.
    Exits,
    // Orca refuses with `no-agent`, though another agent is listed.
    Refuses,
}

// What Orca shows, shared by every `Orca` on it, as commands in two lanes share one Orca.
#[derive(Default)]
pub struct World {
    // What the `listen` its commands run in keeps, from one command to the next.
    pub shared: Arc<Shared>,
    pub repos: Vec<Repo>,
    // Each with its repo's origin, which `agents` leaves out, as Orca's listing does.
    pub worktrees: Vec<Worktree>,
    pub terminals: Vec<Terminal>,
    pub send: Send,
    // An agent's terminal fails to open.
    pub launch_fails: bool,
    // Each line typed: its terminal and the line.
    pub typed: Vec<(String, String)>,
    // Each agent launched: its worktree and command.
    pub launched: Vec<(String, String)>,
    pub closed: Vec<String>,
    pub removed: Vec<String>,
    // Each terminal opened that runs no agent: its worktree, title and command.
    pub opened: Vec<(String, Option<String>, String)>,
    // Each card moved: its worktree and column. Setting one fails while `status_fails`.
    pub statuses: Vec<(String, Status)>,
    pub status_fails: bool,
    // Each worktree created: its repo, name and options.
    pub created: Vec<(String, String, Vec<String>)>,
    // The worktrees' listing is cut off, so it says no worktree is gone.
    pub incomplete: bool,
    // The other hosts this machine reaches, and the issues that are Tolkster's as a step run on
    // one reads them (`Orca::run_to_exit`).
    pub hosts: Vec<String>,
    // The hosts that don't answer: their worktrees and terminals are left out of listings. Those
    // in `silent_after_step` join them once a step has run there.
    pub unanswered: Vec<String>,
    pub silent_after_step: Vec<String>,
    // The hosts where every repo is set up and ready.
    pub ready_on: Vec<String>,
    // A recipe machine is being made, the refusal a make or wake meets, the repos one was asked
    // for, the machines woken, those put to sleep and those destroyed (`Machines`).
    pub making: bool,
    pub declined: Option<Option<crate::trace::Declined>>,
    pub made_for: Vec<String>,
    pub woken: Vec<String>,
    pub asleep: Vec<String>,
    pub destroyed: Vec<String>,
    pub tolksters_there: Tolksters,
    // Every branch's PRs, as a step on another host reads them from GitHub.
    pub prs_there: Vec<crate::github::BranchPr>,
    // Git, by the path of each checkout that has it, and the default branch's tip on origin.
    pub git: HashMap<String, Git>,
    pub tip: String,
    // What a first-parent `git log` prints in any checkout that has git.
    pub log: String,
    // Each git command asked for, modelled or not: `[<dir>] git <args>`.
    pub ran: Vec<String>,
    // Each listing asked of Orca, one per spawn: `worktree ps`, `terminal list` or `repo list`,
    // or one host's (`AT`), whose host `listed_at` names, None for this machine.
    pub spawns: Vec<&'static str>,
    pub listed_at: Vec<Option<String>>,
    made: u32,
}

// A checkout as git has it.
#[derive(Clone, Debug, Default)]
pub struct Git {
    pub head: String,
    // None when HEAD is detached.
    pub branch: Option<String>,
    // What `git status --porcelain --ignored` prints.
    pub status: String,
    // How many commits on HEAD or the branch are on no remote.
    pub unpushed: u32,
    // How many commits origin's copy of its branch holds that origin's default branch doesn't.
    pub pushed_work: u32,
    // Its local branches.
    pub branches: Vec<String>,
}

impl World {
    // Git's answer, in `dir`, to `args`: None when it has no checkout there or doesn't model
    // the command. Err is the code a command failed with.
    fn git(&mut self, args: &[&str], dir: Option<&str>) -> Option<Result<String, i32>> {
        let (tip, log) = (self.tip.clone(), self.log.clone());
        let git = self.git.get_mut(dir?)?;
        if args.contains(&"fetch") {
            return Some(Ok(String::new()));
        }
        Some(match args {
            ["rev-parse", "--verify", "--quiet", "origin/HEAD^{commit}"] => Ok(tip),
            ["rev-parse", "--abbrev-ref", "HEAD"] => Ok(git.branch.clone().unwrap_or_else(|| "HEAD".into())),
            ["rev-parse", "--verify", "--quiet", "HEAD^{commit}"] => Ok(git.head.clone()),
            ["switch", "--quiet", "--detach", "--discard-changes", to] => {
                (git.head, git.branch) = ((*to).into(), None);
                Ok(String::new())
            }
            ["clean", ..] => Ok(String::new()),
            // A commit is on the tip only as the tip itself, as a landing's is.
            ["merge-base", "--is-ancestor", commit, onto] => if commit == onto { Ok(String::new()) } else { Err(1) },
            ["log", "--first-parent", ..] => Ok(log.clone()),
            ["status", ..] => Ok(git.status.clone()),
            // A branch it names that isn't there fails, as git does.
            ["rev-list", "--count", "--ignore-missing", _, "--not", "origin/HEAD"] => Ok(git.pushed_work.to_string()),
            ["rev-list", "--count", .., reference, "--not", "--remotes"] if !git.branches.iter().any(|b| reference.strip_prefix("refs/heads/") == Some(b.as_str())) => Err(128),
            ["rev-list", "--count", ..] => Ok(git.unpushed.to_string()),
            ["show-ref", "--verify", "--quiet", reference] => match git.branches.iter().any(|b| reference.strip_prefix("refs/heads/") == Some(b.as_str())) {
                true => Ok(String::new()),
                false => Err(1),
            },
            // `-d` keeps a branch holding commits on no remote, which stands here for commits
            // merged nowhere.
            ["branch", "--quiet", "-d", _] if git.unpushed > 0 => Err(1),
            ["branch", "--quiet", "-d", branch] | ["branch", "-D", branch] => {
                git.branches.retain(|b| b != branch);
                Ok(String::new())
            }
            _ => return None,
        })
    }
}

impl World {
    // The repo `Tolkster` (`r1`, at `acme/tolkster`) with its primary checkout.
    pub fn tolkster() -> Rc<RefCell<World>> {
        let primary = Worktree { id: "r1::/src/tolkster".into(), repo_id: "r1".into(), path: "/src/tolkster".into(), branch: Some("main".into()), linked_issue: None, is_main: true, is_local: true, agents: Vec::new(), origin: Some("acme/tolkster".into()) };
        Rc::new(RefCell::new(World { repos: vec![Repo { id: "r1".into(), name: "Tolkster".into() }], worktrees: vec![primary], ..World::default() }))
    }

    // Two repos, `tolkster` (`r1`) and `other` (`r2`), each with its primary checkout, the
    // first's listed as the main worktree only when `primary`, and a worker's worktree of the
    // first: what a session in a repo's primary checkout looks for (brief::Checkouts).
    pub fn two_repos(primary: bool) -> Rc<RefCell<World>> {
        let tree = |repo_id: &str, path: &str, main: bool| Worktree { id: format!("{repo_id}::{path}"), repo_id: repo_id.into(), path: path.into(), branch: Some("main".into()), linked_issue: None, is_main: main, is_local: true, agents: Vec::new(), origin: None };
        let repos = vec![Repo { id: "r1".into(), name: "tolkster".into() }, Repo { id: "r2".into(), name: "other".into() }];
        let worktrees = vec![tree("r1", "/wt/tw-3", false), tree("r1", "/src/tolkster", primary), tree("r2", "/src/other", true)];
        Rc::new(RefCell::new(World { repos, worktrees, ..World::default() }))
    }

    // Each terminal opened that runs no agent, as `orca terminal create` is asked for it.
    pub fn opened_as_asked(&self) -> Vec<String> {
        self.opened.iter().map(|(worktree_id, title, command)| format!("--worktree id:{worktree_id}{} --command {command}", title.as_ref().map(|t| format!(" --title {t}")).unwrap_or_default())).collect()
    }

    // The agents' terminals in a worktree.
    pub fn agents_in(&self, worktree_id: &str) -> Vec<String> {
        self.terminals.iter().filter(|t| t.agent && t.worktree_id.as_deref() == Some(worktree_id)).filter_map(|t| t.handle.clone()).collect()
    }

    // The agent in a worktree turns `working`, `waiting` at a prompt or `done`.
    pub fn set_agents(&mut self, worktree_id: &str, state: &str) {
        for worktree in self.worktrees.iter_mut().filter(|w| w.id == worktree_id) {
            worktree.agents.iter_mut().for_each(|pane| pane.state = state.into());
        }
    }
}

pub const PS: &str = "worktree ps";
pub const TERMINALS: &str = "terminal list";
pub const REPOS: &str = "repo list";
pub const AT: &str = "one host's listing";

type Shell<'h> = Box<dyn FnMut(&str, &[&str], Option<&str>) -> Result<Output, Error> + 'h>;
pub struct Orca<'h> {
    pub world: Rc<RefCell<World>>,
    // Run once, from inside the next agent's launch, once its terminal is open, or from
    // inside the next worktree's creation, once it is listed: a command of another lane
    // carried out while this one is in flight.
    pub after_launch: Option<Box<dyn FnOnce() + 'h>>,
    pub after_create: Option<Box<dyn FnOnce() + 'h>>,
    // Git, and whatever else reaches the shell, fails, as on a machine without either.
    shell: Shell<'h>,
    // The host whose own Orca this is, None for this machine's, which lists every host's: it
    // lists that host's worktrees and terminals alone, by the IDs its Orca gives them.
    host: Option<String>,
}

impl<'h> Orca<'h> {
    pub fn on(world: &Rc<RefCell<World>>) -> Orca<'h> {
        let git = Rc::clone(world);
        let shell: Shell<'h> = Box::new(move |program, args, dir| {
            let mut world = git.borrow_mut();
            if program == "git" {
                world.ran.push(format!("{}git {}", dir.map(|d| format!("[{d}] ")).unwrap_or_default(), args.join(" ")));
            }
            let answer = if program == "git" { world.git(args, dir) } else { None };
            // What isn't modelled fails as git does where it can't look (128), never as a
            // ref that isn't there (1).
            Ok(match answer {
                Some(Ok(stdout)) => Output::ok(stdout),
                Some(Err(code)) => Output::failed(code, ""),
                None => Output::failed(128, format!("{program} {} isn't there", args.first().unwrap_or(&""))),
            })
        });
        Orca { world: Rc::clone(world), after_launch: None, after_create: None, shell, host: None }
    }

    // A worktree as this Orca lists it: None for another host's.
    fn view(&self, worktree: &Worktree) -> Option<Worktree> {
        let Some(host) = self.host.as_deref() else { return Some(worktree.clone()) };
        (worktree.host() == Some(host)).then(|| Worktree { id: orca_worktree(&worktree.id).unwrap_or_default(), is_local: true, ..worktree.clone() })
    }

    fn view_terminal(&self, terminal: &Terminal) -> Option<Terminal> {
        let Some(host) = self.host.as_deref() else { return Some(terminal.clone()) };
        let on = |id: &str| split_worktree(id).is_some_and(|(at, _, _)| at == Some(host));
        terminal.worktree_id.as_deref().is_some_and(on).then(|| Terminal {
            handle: terminal.handle.as_deref().and_then(split_terminal).map(|(_, own)| own.to_string()),
            worktree_id: terminal.worktree_id.as_deref().and_then(orca_worktree),
            ..terminal.clone()
        })
    }

    // The world's ID of a worktree this Orca names `id`.
    fn world_id(&self, id: &str) -> String {
        self.host.as_ref().map_or_else(|| id.to_string(), |host| format!("{host}@{id}"))
    }

    fn listing(&self) -> Vec<Worktree> {
        let world = self.world.borrow();
        world.worktrees.iter().filter(|w| w.host().is_none_or(|host| !world.unanswered.iter().any(|h| h == host))).filter_map(|w| self.view(w)).collect()
    }

    // On `World::two_repos`, where no terminal opens.
    pub fn refusing() -> Orca<'h> {
        let orca = Orca::on(&World::two_repos(true));
        orca.world.borrow_mut().launch_fails = true;
        orca
    }

    fn worktree(&self, worktree_id: &str) -> Result<(), Error> {
        let worktree_id = self.world_id(worktree_id);
        match self.world.borrow().worktrees.iter().any(|w| w.id == worktree_id) {
            true => Ok(()),
            false => Err(Error::Orca(format!("orca: no worktree {worktree_id}"))),
        }
    }
}

impl crate::orca::Orca for Orca<'_> {
    fn agents(&mut self) -> Result<Vec<Worktree>, Error> {
        self.agents_listed().map(|(worktrees, _)| worktrees)
    }
    fn agents_listed(&mut self) -> Result<(Vec<Worktree>, Option<Vec<String>>), Error> {
        let listing = self.listing();
        let mut world = self.world.borrow_mut();
        world.spawns.push(PS);
        Ok((listing.iter().map(|w| Worktree { origin: None, ..w.clone() }).collect(), (!world.incomplete).then(|| listing.iter().map(|w| w.id.clone()).collect())))
    }
    fn listed(&mut self) -> Result<Option<Vec<String>>, Error> {
        let (_, listed) = self.agents_listed()?;
        Ok(listed.filter(|_| self.world.borrow().unanswered.is_empty()))
    }
    fn worktrees(&mut self) -> Result<Vec<Worktree>, Error> {
        let listing = self.listing();
        self.world.borrow_mut().spawns.push(PS);
        Ok(listing)
    }
    fn terminals(&mut self) -> Result<Vec<Terminal>, Error> {
        let world = self.world.borrow();
        let silent = |t: &Terminal| t.handle.as_deref().and_then(split_terminal).and_then(|(host, _)| host).is_some_and(|host| world.unanswered.iter().any(|h| h == host));
        let terminals = world.terminals.iter().filter(|t| !silent(t)).filter_map(|t| self.view_terminal(t)).collect();
        drop(world);
        self.world.borrow_mut().spawns.push(TERMINALS);
        Ok(terminals)
    }
    // A host's own Orca lists only itself.
    fn listing_on(&mut self, host: Option<&str>) -> Result<Listing, Error> {
        let host = self.host.clone().or_else(|| host.map(String::from));
        let world = self.world.borrow();
        if let Some(host) = host.as_ref().filter(|_| self.host.is_none()) {
            if !world.hosts.contains(host) {
                return Err(Error::Later(format!("{host} is not a paired host")));
            }
            // One that answered this run and then fell silent fails as Orca does.
            if world.unanswered.contains(host) && world.silent_after_step.contains(host) {
                return Err(Error::Orca(format!("orca worktree ps failed: {host} is unavailable")));
            }
            if world.unanswered.contains(host) {
                return Err(Error::silent(host));
            }
        }
        let on = |id: &str| split_worktree(id).and_then(|(at, _, _)| at) == host.as_deref();
        let worktrees: Vec<Worktree> = world.worktrees.iter().filter(|w| on(&w.id)).filter_map(|w| self.view(w)).map(|w| Worktree { origin: None, ..w }).collect();
        let terminals = world.terminals.iter().filter(|t| t.worktree_id.as_deref().is_some_and(on)).filter_map(|t| self.view_terminal(t)).collect();
        let listed = (!world.incomplete).then(|| worktrees.iter().map(|w| w.id.clone()).collect());
        drop(world);
        let mut world = self.world.borrow_mut();
        world.spawns.push(AT);
        world.listed_at.push(host);
        Ok(Listing { worktrees, listed, terminals })
    }
    fn origins(&mut self, worktrees: &mut [Worktree]) {
        let world = self.world.borrow();
        for worktree in worktrees.iter_mut() {
            let id = self.world_id(&worktree.id);
            worktree.origin = world.worktrees.iter().find(|w| w.id == id).and_then(|w| w.origin.clone());
        }
    }
    fn repos(&mut self, _host: Option<&str>) -> Result<Vec<Repo>, Error> {
        let mut world = self.world.borrow_mut();
        world.spawns.push(REPOS);
        Ok(world.repos.clone())
    }
    fn answering(&self) -> Vec<Host> {
        let world = self.world.borrow();
        world.hosts.iter().filter(|h| !world.unanswered.contains(h)).map(|h| Host { id: h.clone(), name: h.clone() }).collect()
    }
    fn silent(&self) -> Vec<Host> {
        let world = self.world.borrow();
        world.hosts.iter().filter(|h| world.unanswered.contains(h)).map(|h| Host { id: h.clone(), name: h.clone() }).collect()
    }
    fn ready(&mut self, host: &str, _project: Option<&str>, _reads_config: ReadsConfig) -> Result<bool, Error> {
        Ok(self.world.borrow().ready_on.iter().any(|h| h == host))
    }
    fn ready_projects(&mut self, host: &str, _reads_config: ReadsConfig) -> Result<Vec<String>, Error> {
        Ok(if self.world.borrow().ready_on.iter().any(|h| h == host) { vec![FAKE_PROJECT.into()] } else { Vec::new() })
    }
    fn send(&mut self, worktree_id: &str, handle: &Handle, line: &Line, proof: Proof) -> Result<Typed, Error> {
        let mut world = self.world.borrow_mut();
        let waiting = world.worktrees.iter().find(|w| w.id == worktree_id).is_some_and(|w| waits_in(w, &world.terminals));
        if !world.terminals.iter().any(|t| t.handle.as_deref() == Some(handle)) {
            return Err(Error::Orca(format!("orca terminal send failed: no terminal {handle}")));
        }
        if world.send == Send::Fails {
            return Err(Error::Orca("orca terminal send failed".into()));
        }
        if matches!(world.send, Send::Exits | Send::Refuses) {
            if world.send == Send::Exits {
                world.terminals.retain(|t| t.worktree_id.as_deref() != Some(worktree_id));
                world.worktrees.iter_mut().filter(|w| w.id == worktree_id).for_each(|w| w.agents.clear());
            }
            return Err(Error::Unsent(Unsent::NoAgent, "orca terminal send failed: input refused: no-agent".into()));
        }
        // A permission prompt stops the line before the agent takes it.
        if waiting {
            return Ok(Typed::Waiting);
        }
        world.typed.push((handle.as_str().into(), line.as_str().into()));
        // An unproven receipt stops at `input_accepted`, which is all a message needs.
        Ok(if world.send == Send::Unproven && proof == Proof::Turn { Typed::Unproven } else { Typed::Delivered })
    }
    fn type_line(&mut self, handle: &Handle, line: &Line) -> Result<(), Error> {
        let mut world = self.world.borrow_mut();
        if !world.terminals.iter().any(|t| t.handle.as_deref() == Some(handle)) || world.send == Send::Fails {
            return Err(Error::Orca(format!("orca terminal send failed: no terminal {handle}")));
        }
        world.typed.push((handle.as_str().into(), line.as_str().into()));
        Ok(())
    }
    fn start_agent(&mut self, worktree_id: &str, command: &CommandLine) -> Result<String, Error> {
        self.worktree(worktree_id)?;
        if self.world.borrow().launch_fails {
            return Err(Error::Orca("orca terminal create failed".into()));
        }
        let handle = {
            let mut world = self.world.borrow_mut();
            world.made += 1;
            let (handle, pane) = (format!("term_{}", world.made), format!("tab{}:leaf{}", world.made, world.made));
            world.terminals.push(Terminal { handle: Some(handle.clone()), worktree_id: Some(worktree_id.into()), agent: true, pane: Some(pane.clone()) });
            for worktree in world.worktrees.iter_mut().filter(|w| w.id == worktree_id) {
                worktree.agents.push(Pane { key: pane.clone(), state: "working".into(), since_ms: Some(0) });
            }
            world.launched.push((worktree_id.into(), command.as_str().into()));
            handle
        };
        if let Some(meanwhile) = self.after_launch.take() {
            meanwhile();
        }
        Ok(handle)
    }
    fn open(&mut self, worktree_id: &str, title: Option<&str>, command: &CommandLine) -> Result<String, Error> {
        self.worktree(worktree_id)?;
        let mut world = self.world.borrow_mut();
        if world.launch_fails {
            return Err(Error::Orca("orca terminal create failed".into()));
        }
        world.made += 1;
        let handle = format!("term_{}", world.made);
        world.terminals.push(Terminal { handle: Some(handle.clone()), worktree_id: Some(worktree_id.into()), agent: false, pane: None });
        world.opened.push((worktree_id.into(), title.map(String::from), command.as_str().into()));
        Ok(handle)
    }
    // A step run on its worktree's host, its line read as that host's shell and `main` would,
    // on that host's Orca over this world: an Orca error for a host this machine doesn't reach.
    fn run_to_exit(&mut self, worktree_id: &str, command: &CommandLine) -> Result<Option<i32>, Error> {
        use crate::reap::{KEPT, STEP_OPTIONS, Step, reap_here};
        let host = split_worktree(worktree_id).and_then(|(host, _, _)| host).map(String::from);
        let (hosts, tolksters, prs) = (self.world.borrow().hosts.clone(), self.world.borrow().tolksters_there.clone(), self.world.borrow().prs_there.clone());
        let Some(host) = host.filter(|host| hosts.contains(host)) else { return Err(Error::Orca(format!("orca: {worktree_id} is on no host this machine reaches"))) };
        self.world.borrow_mut().opened.push((worktree_id.into(), None, command.as_str().into()));
        let line = command.as_str().strip_suffix("; exit $LASTEXITCODE").expect("a step ends its shell");
        let words: Vec<String> = line.strip_prefix("tolkster reap-here ").expect("a reap step").split(' ').map(String::from).collect();
        let step = Step::parse(&crate::args::parse(&words, &STEP_OPTIONS, false)?)?;
        let mut there = Orca { host: Some(host.clone()), ..Orca::on(&self.world) };
        let code = match reap_here(&mut there, &step, &mut |_, _| Ok(prs.clone()), |_| Ok(tolksters.clone())) {
            Ok((_, true)) => 0,
            Ok((_, false)) => KEPT,
            Err(_) => 1,
        };
        let mut world = self.world.borrow_mut();
        if world.silent_after_step.contains(&host) {
            world.unanswered.push(host);
        }
        Ok(Some(code))
    }
    // A terminal already gone is an error, as Orca's stale handle is.
    fn close(&mut self, handle: &Handle) -> Result<(), Error> {
        let mut world = self.world.borrow_mut();
        let Some(at) = world.terminals.iter().position(|t| t.handle.as_deref() == Some(handle)) else {
            return Err(Error::Orca(format!("orca terminal close failed: terminal_handle_stale {handle}")));
        };
        let closed = world.terminals.remove(at);
        for worktree in world.worktrees.iter_mut().filter(|w| w.id.as_str() == closed.worktree_id.as_deref().unwrap_or_default()) {
            worktree.agents.retain(|pane| Some(&pane.key) != closed.pane.as_ref());
        }
        world.closed.push(handle.as_str().into());
        Ok(())
    }
    fn close_all(&mut self, worktree_id: &str) -> Result<(), Error> {
        self.worktree(worktree_id)?;
        let handles: Vec<Handle> = self.world.borrow().terminals.iter().filter(|t| t.worktree_id.as_deref() == Some(worktree_id)).filter_map(|t| t.handle.as_deref().and_then(Handle::parse)).collect();
        handles.iter().try_for_each(|handle| self.close(handle))?;
        // Orca closes them by their worktree, so one listed with no plain handle goes too.
        self.world.borrow_mut().terminals.retain(|t| t.worktree_id.as_deref() != Some(worktree_id));
        Ok(())
    }
    // On another host, in its repo `hosted`, made by the project's ID, which `created` holds.
    fn create_worktree(&mut self, place: Place, name: &str, options: &[&str]) -> Result<(String, String), Error> {
        let linked = options.iter().position(|o| *o == "--linear-issue").and_then(|at| options.get(at + 1)).map(|i| i.to_ascii_uppercase());
        let path = format!("/wt/{name}");
        let (id, repo_id, made_by) = match place {
            Place::Here { repo_id } => (format!("{repo_id}::{path}"), repo_id, repo_id),
            Place::On { host, project } => (format!("{host}@hosted::{path}"), "hosted", project),
        };
        {
            let mut world = self.world.borrow_mut();
            let origin = match place {
                Place::Here { repo_id } => world.worktrees.iter().find(|w| w.repo_id == repo_id).and_then(|w| w.origin.clone()),
                Place::On { project, .. } => project.strip_prefix("github:").map(String::from),
            };
            world.created.push((made_by.into(), name.into(), options.iter().map(|o| o.to_string()).collect()));
            world.worktrees.push(Worktree { id: id.clone(), repo_id: repo_id.into(), path: path.clone(), branch: Some(name.into()), linked_issue: linked, is_main: false, is_local: matches!(place, Place::Here { .. }), agents: Vec::new(), origin });
        }
        if let Some(meanwhile) = self.after_create.take() {
            meanwhile();
        }
        Ok((id, path))
    }
    fn remove_worktree(&mut self, worktree_id: &str) -> Result<(), Error> {
        self.worktree(worktree_id)?;
        let worktree_id = self.world_id(worktree_id);
        let mut world = self.world.borrow_mut();
        world.worktrees.retain(|w| w.id != worktree_id);
        world.terminals.retain(|t| t.worktree_id.as_ref() != Some(&worktree_id));
        world.removed.push(worktree_id);
        Ok(())
    }
    fn name_worktree(&mut self, worktree_id: &str, _display_name: &str) -> Result<(), Error> {
        self.worktree(worktree_id)
    }
    fn set_status(&mut self, worktree_id: &str, status: Status) -> Result<(), Error> {
        self.worktree(worktree_id)?;
        let worktree_id = self.world_id(worktree_id);
        let mut world = self.world.borrow_mut();
        if world.status_fails {
            return Err(Error::Orca("orca: the board is unavailable".into()));
        }
        world.statuses.push((worktree_id, status));
        Ok(())
    }
    fn shell(&mut self) -> &mut Run<'_> {
        &mut *self.shell
    }
}

// An Orca `worktree ps` row for a worker's worktree at `path`, linked to `issue`.
pub fn row(issue: Option<&str>, path: &str) -> Value {
    json!({
        "worktreeId": format!("repo::{path}"), "repoId": "repo", "hostId": "local", "path": path,
        "branch": format!("refs/heads/{}", path.rsplit('/').next().unwrap()),
        "linkedLinearIssue": issue, "isMainWorktree": false, "agents": [],
    })
}

// The row of the repo's primary checkout.
pub fn primary() -> Value {
    json!({ "worktreeId": "repo::D:/main", "repoId": "repo", "hostId": "local", "path": "D:/main", "branch": "refs/heads/main", "linkedLinearIssue": null, "isMainWorktree": true, "agents": [] })
}

// What `orca worktree ps` prints for `rows`.
pub fn ps(rows: Value) -> String {
    json!({ "ok": true, "result": { "worktrees": rows } }).to_string()
}
