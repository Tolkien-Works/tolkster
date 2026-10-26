use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, IsTerminal};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use ureq::Agent;

use crate::args::{Kind, parse};
use crate::config::changes::Reread;
use crate::config::{files, realtime};
use crate::error::{Context, Error};
use crate::exec::{beating, run_command};
use crate::github;
use crate::health;
use crate::listen;
use crate::panel::{self, HostShare, Listing, Slot};
use crate::scheduler::{self, Acted, BASE_AGAIN, Job};
use crate::watch::{Agents, idle_worktrees, states_of, waiting_worktrees};
use crate::orca::{Cli, Host, Orca, agent_handles, agent_worktrees, candidates, open_terminals, answers, origin_repo, paired, terminal_worktrees, wait_exit};
use crate::plain::{IssueKey, Terminal, split_worktree};
use crate::recipe;
use crate::command::carry_out;
use crate::runtime::{Shared, load, settings};
use crate::timing::Timing;
use crate::offline;
use crate::onboard;
use crate::state::{Records, State};
use crate::hosts::{self, Fleet, cap, total};
use crate::machine::config_line;
use crate::sync::lock;
use crate::trace;
use crate::update::{self, Build, Tips, Updater};

const ENROLLED_TRIES: u32 = 5;
const ENROLLED_PAUSE: std::time::Duration = std::time::Duration::from_secs(3);

pub const OPTIONS: &[(&str, Kind)] = &[("print", Kind::Boolean), ("verbose", Kind::Boolean), ("plain", Kind::Boolean)];

mod handlers;
use handlers::{base, closed, missed};

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, OPTIONS, false)?;
    if !parsed.flag("print") && !update::acting() {
        return update::supervise();
    }
    let files = files()?;
    let (worker_url, secret) = realtime(&files)?;
    let url = worker_url.realtime();
    trace::configure(parsed.flag("verbose"), parsed.flag("plain"));
    // The update, for the panel to read.
    let mut shown_update = None;
    // Only an acting `listen` keeps the timing file.
    let timing = if parsed.flag("print") { Timing::default() } else { Timing::at(crate::cli::timing_file()?) };
    let shared = Arc::new(Shared { timing, ..Shared::default() });
    let scheduler = if parsed.flag("print") {
        None
    } else {
        // The admitted hosts are read before a command can run, or the caps and sleeps would
        // find every host unadmitted until the first listing; a Durable Object that never
        // answers stops `listen` with an error, to be started again.
        shared.pacing.start_enrolled(|| State::open(agent, &files).and_then(|state| state.enrolled_hosts()), ENROLLED_TRIES, || std::thread::sleep(ENROLLED_PAUSE))?;
        // Checked now, so a missing setting stops it at once rather than failing each command.
        let config = shared.settings(&files)?;
        let told = config_line(&config);
        trace::enable();
        if let Err(why) = crate::cli::listen_log_file().and_then(|path| trace::keep_on_disk(path).context("keeping listen.log")) {
            trace::event(trace::Disk::Off(why));
        }
        update::clear_old();
        let updater = match Updater::open(agent, &mut run_command) {
            Ok(updater) => Some(updater),
            Err(why) => {
                trace::event(trace::Update::Off(why));
                None
            }
        };
        if let Some(updater) = &updater {
            updater.catch_up();
        }
        recipe::spawn_prepare(&shared);
        let update = updater.as_ref().map(|u| u.update.clone());
        shown_update.clone_from(&update);
        github::hold();
        let (acting, missing, releasing) = (agent.clone(), agent.clone(), agent.clone());
        let preparing = Arc::clone(&shared);
        let working = Arc::clone(&shared);
        let tips = Mutex::new(Tips::default());
        let scheduler = scheduler::Scheduler::spawn(Arc::clone(&shared), move |_, job, beat| match job {
            // A repo's base tip, on connect or a push: `listen`'s for the repo's conversations
            // checkouts, and for Tolkster's own main, its update. The machines read a PR's base
            // from the push themselves. A tip not pulled yet is tried again after BASE_AGAIN, as
            // nothing resends it (`Tips`).
            Job::Base { repo, reference, after } => {
                if updater.as_ref().is_some_and(|u| u.is_own(repo, reference)) {
                    panel::base(after.as_str());
                }
                base(&acting, &working, repo, after, beat);
                match updater.as_ref().filter(|u| u.is_own(repo, reference)) {
                    Some(updater) if lock(&tips).take(after, &mut say, || updater.pass(&mut beating(beat), &mut say, Some(after), |made| build(&releasing, &preparing, made))) => Acted::Held(BASE_AGAIN),
                    _ => Acted::Done,
                }
            }
            Job::Update => {
                if let Some(updater) = &updater {
                    updater.pass(&mut beating(beat), &mut say, None, |made| build(&releasing, &preparing, made));
                }
                Acted::Done
            }
            // What the watcher saw in Orca, which nothing answers.
            Job::Closed { worktree_id } => {
                closed(&acting, &working, worktree_id, beat);
                Acted::Done
            }
            Job::Ended { terminal } => {
                scheduler::terminal_closed(terminal);
                trace::event(trace::Outcome::Reported);
                Acted::Done
            }
            Job::Command(command) => carry_out(&acting, &working, command, beat),
        });
        // The agents' states failing to list keeps the idle and waiting ones as they were last
        // listed, so exits are still seen, and counts as an incomplete listing of the worktrees.
        // Workers' worktrees too, those linked to an issue (orca::candidates), whose agents'
        // terminals are waited on: not every agent's in Orca, each wait being a process held open.
        // The hosts are those paired as each listing begins, and one that doesn't answer keeps
        // what it showed last (`Agents::keeping`).
        // The panel's slots each name the issue a worker's worktree is linked to, kept for a
        // silent host's as its agents are. A listing of the states that failed isn't shown.
        let (mut before, mut workers_before, mut issues_before) = (Agents::default(), HashSet::new(), HashMap::new());
        // Each of this machine's repos' origin, by Orca ID, read once it is listed: by its
        // project the Durable Object finds a host ready for an issue's repo. One not read is
        // tried again at the next listing.
        let mut origins: BTreeMap<String, String> = BTreeMap::new();
        // Their display names, by which the fleet finds a repo's recipe: read once all are known,
        // and again when a repo listed isn't.
        let mut names: BTreeMap<String, String> = BTreeMap::new();
        // Each agent's terminal, by its handle, with its worktree's issue, for the line its exit makes.
        let issues_of: Arc<Mutex<HashMap<String, String>>> = Arc::default();
        let issue_of = Arc::clone(&issues_of);
        let mut reread = Reread::new(&config, Some(worker_url.base().as_str()));
        let (listing, mut config, watching) = (agent.clone(), config, Arc::clone(&shared));
        let agents = move || {
            let began = Instant::now();
            // A machine machines.json marks asleep as this listing begins is silent as it was,
            // never faulted, a restart's first listing's included; one a wake has just marked
            // awake isn't. Each other silent host is asked again after this listing, which the
            // next one reports.
            if let Ok(now) = crate::config::files().and_then(|files| settings(&files)) {
                watching.health.marked_asleep(now.machines.iter().filter(|(_, made)| made.asleep).filter_map(|(name, made)| made.environment.as_deref().map(|host| (name.as_str(), host))), began);
            }
            watching.health.listed(began);
            let mut run = run_command;
            let mut orca = Cli::paired(&mut run, &watching.health).at_once(run_command);
            let terminals = orca.terminals()?;
            let mut states_listed = true;
            let (idle, waiting, workers, others, listed, all) = match orca.agents_listed() {
                Ok((worktrees, listed)) => {
                    let silent = orca.unanswered();
                    let on_silent = |id: &String| split_worktree(id).and_then(|(at, _, _)| at).is_some_and(|at| silent.iter().any(|s| s == at));
                    let kept = std::mem::take(&mut issues_before).into_iter().filter(|(id, _)| on_silent(id));
                    issues_before = kept.chain(candidates(&worktrees).into_iter().filter_map(|w| Some((w.id.clone(), w.linked_issue.clone()?)))).collect();
                    let workers: HashSet<String> = candidates(&worktrees).into_iter().map(|w| w.id.clone()).collect();
                    let others = worktrees.iter().map(|w| w.id.clone()).filter(|id| !workers.contains(id)).collect();
                    let complete = listed.is_some().then(|| worktrees.clone());
                    let (mut git, mut tried) = (run_command, HashSet::new());
                    // A repo's primary checkout first, whose path is surest to be there.
                    let mut local: Vec<_> = worktrees.iter().filter(|w| w.is_local).collect();
                    local.sort_by_key(|w| !w.is_main);
                    for w in local {
                        if !origins.contains_key(&w.repo_id)
                            && tried.insert(w.repo_id.clone())
                            && let Ok(origin) = origin_repo(&mut git, Some(&w.path))
                        {
                            origins.insert(w.repo_id.clone(), origin);
                        }
                    }
                    (idle_worktrees(&worktrees, &terminals), waiting_worktrees(&worktrees, &terminals), workers, others, listed.map(|ids| ids.into_iter().collect()), complete)
                }
                Err(error) => {
                    trace::event(trace::Watch::Listing(error));
                    states_listed = false;
                    (before.idle.clone(), before.waiting.clone(), workers_before.clone(), before.others.clone(), None, None)
                }
            };
            let answering = orca.answering();
            let hosts = answering.iter().map(|h| h.id.clone()).collect();
            let paired = watching.health.withholding(answering.iter().map(|h| h.id.clone()).chain(orca.unanswered()).collect(), Instant::now());
            let mut now = Agents { running: agent_worktrees(&terminals), open: terminal_worktrees(&terminals), terminals: open_terminals(&terminals, None).into_iter().map(String::from).collect(), agents: agent_handles(&terminals, &workers).into_iter().map(String::from).collect(), idle, waiting, listed, others, hosts, paired, fleet: Fleet::default(), config: String::new(), idle_hosts: BTreeMap::new(), dormant: None }.keeping(&before, &orca.unanswered());
            // The caps as config.json has them now, as the fleet reports them, and the hosts
            // holding an admitted credential, which a command reads as last listed here. A read
            // that fails keeps the last. What the owner changed is said once (`Reread`).
            let fresh = crate::config::files().and_then(|files| {
                watching.pacing.read_enrolled(State::open(&listing, &files).and_then(|state| state.enrolled_hosts()));
                let worker = crate::config::worker_url(&files).ok().map(|url| url.base());
                watching.settings(&files).map(|config| (config, worker))
            });
            if let Some(said) = reread.after(fresh.as_ref().map(|(config, worker)| (config, worker.as_deref()))) {
                trace::event(said);
            }
            if let Ok((fresh, _)) = fresh {
                config = fresh;
            }
            let config = &config;
            // A make, wake, sleep or destroy under way leaves its mark or entry as it passes
            // through: none is dormant until it ends.
            now.dormant = all.as_ref().filter(|_| !watching.recipes.making()).map(|worktrees| hosts::dormant(config, &watching.pacing, &answering, worktrees));
            now.idle_hosts = all.map(|worktrees| hosts::idle(config, &watching.pacing, &answering, &worktrees)).unwrap_or_default();
            // Each host's ready projects, read once per listing: a host with room is one with a
            // cap and any of them (`Orca::ready_projects`). One whose read fails is told as
            // silent, never as having none ready, which would move its grants elsewhere.
            let held = |at: Option<&str>| now.running.iter().filter(|id| !now.others.contains(*id) && split_worktree(id).and_then(|(at, _, _)| at) == at).count() as i64;
            let mut silent: Vec<(Host, i64)> = orca.silent().into_iter().map(|h| { let held = held(Some(&h.id)); (h, held) }).collect();
            let mut answered: Vec<(Host, Vec<String>, i64)> = Vec::new();
            for h in answering {
                let held = held(Some(&h.id));
                match orca.ready_projects(&h.id, onboard::reads_config) {
                    Ok(ready) => answered.push((h, ready, held)),
                    Err(_) => silent.push((h, held)),
                }
            }
            if origins.keys().any(|id| !names.contains_key(id))
                && let Ok(repos) = orca.repos(None)
            {
                names = repos.into_iter().map(|r| (r.id, r.name)).collect();
            }
            now.fleet = hosts::fleet(config, &watching.pacing, &origins, &names, held(None), &answered, &silent);
            now.config = config_line(config);
            let (ready, idle): (Vec<_>, Vec<_>) = answered.into_iter().partition(|(h, ready, _)| cap(config, h) > 0 && !ready.is_empty());
            let ready: Vec<(Host, i64)> = ready.into_iter().map(|(h, _, held)| (h, held)).collect();
            let holding: Vec<(Host, i64)> = idle.into_iter().map(|(h, _, held)| (h, held)).chain(silent).collect();
            let total = total(config, held(None), &ready, &holding);
            let shares = hosts::shares(config, held(None), &ready, &holding);
            let silent = orca.unanswered();
            let hosts = shares.into_iter().map(|(host, _)| match host {
                None => HostShare { id: None, name: "local".into(), cap: config.local_slots(), answering: true, refused: false },
                Some(h) => HostShare { id: Some(h.id.clone()), name: h.name.clone(), cap: cap(config, h), answering: !silent.contains(&h.id), refused: watching.health.refused(&h.id, Instant::now()) },
            }).collect();
            if states_listed {
                let slots = states_of(&now).into_iter().filter(|(id, _)| !now.others.contains(id)).map(|(id, state)| Slot { issue: issues_before.get(&id).cloned(), host: split_worktree(&id).and_then(|(at, _, _)| at).map(String::from), worktree: id, state }).collect();
                panel::listed(Listing { at: began, took: began.elapsed(), slots, hosts, cap: total }, &now.terminals);
            }
            *lock(&issues_of) = terminals.iter().filter(|t| t.agent).filter_map(|t| Some((t.handle.clone()?, issues_before.get(t.worktree_id.as_ref()?).filter(|i| IssueKey::parse(i).is_some())?.clone()))).collect();
            workers_before = workers;
            before.clone_from(&now);
            Ok(now)
        };
        let (missed_by, exits) = (Arc::clone(&shared), Arc::clone(&shared));
        let scheduler = scheduler.watching(agents, move |now| missed(&missing, &missed_by, now), move |handle| {
            let issue = lock(&issue_of).get(handle).cloned();
            Terminal::check(handle).and_then(|handle| wait_exit(&mut run_command, &exits.health, &handle, issue))
        }).telling(told);
        shared.health.on_answer(scheduler.lister());
        // A silent host is asked again only here, off the job lanes.
        let asking = Arc::clone(&shared);
        std::thread::spawn(move || loop {
            std::thread::sleep(health::TICK);
            let mut run = run_command;
            let hosts = || paired(&mut run_command).ok().map(|hosts| hosts.into_iter().map(|h| h.id).collect());
            health::ask_again(&asking.health, hosts, |host| answers(&mut run, host), Instant::now);
        });
        // The label a silent host's issues show is made here too, apart from every lane.
        let (labelling, settling) = (agent.clone(), Arc::clone(&shared));
        std::thread::spawn(move || loop {
            std::thread::sleep(health::TICK);
            offline::settle(&settling.health, Instant::now(), &|| load(&labelling, &settling).and_then(|rt| rt.state.workers()), &offline::App(&labelling));
        });
        Some(match update {
            Some(update) => scheduler.updating(update),
            None => scheduler,
        })
    };
    // The panel only where the plain log would be coloured and every stream is a terminal:
    // `--plain`, NO_COLOR, a piped stream and `--print` keep the plain log. Stopped before a restart, so the new build
    // starts in a restored console.
    let screen = (scheduler.is_some() && trace::colour() && io::stdin().is_terminal() && io::stderr().is_terminal()).then(|| panel::start(shown_update, Arc::clone(&shared))).flatten();
    let ended = listen::listen(&url, &secret, &mut io::stdout(), scheduler);
    drop(screen);
    github::revoke_held(agent);
    match ended? {
        Some(restart) => update::restart(&restart),
        None => Ok(0),
    }
}

fn say(said: trace::Update) {
    trace::event(said);
}

// A new build of `listen`, made on a thread of its own so commands go on being carried out.
// The pull may have changed what a recipe's machines are made from; the restart's start
// prepares them instead.
#[cfg(feature = "develop")]
fn build(agent: &Agent, shared: &Arc<Shared>, build: Build) {
    let (agent, shared) = (agent.clone(), Arc::clone(shared));
    std::thread::spawn(move || {
        if !build.make(&mut run_command, &mut say, &mut update::swap, &mut |run, dir, head| update::release(&agent, run, dir, head)) {
            recipe::spawn_prepare(&shared);
        }
    });
}

#[cfg(not(feature = "develop"))]
fn build(_agent: &Agent, _shared: &Arc<Shared>, build: Build) {
    match build {}
}
