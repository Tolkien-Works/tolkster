// `tolkster worker <sessionId> …`: a worker set up on its own host, the line `listen` launches
// in the worker's new terminal (setup.rs), then its agent started there (harness::exec). Not for people.

use ureq::Agent;

use crate::args::parse;
use crate::config::{files, host_credential};
use crate::credential::git_env;
use crate::error::{Context, Error};
use crate::exec::{run_command, run_command_with};
use crate::github::AppReader;
use crate::issue_state::Started;
use crate::orca::origin_repo;
use crate::prebuilt;
use crate::live::LinearApp;
use crate::runtime::settings;
use crate::harness;
use crate::setup::{Host, OPTIONS, Worker, set_up};
use crate::state::State;
use crate::trade;
use crate::update;

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let worker = Worker::parse(&parse(argv, &OPTIONS, true)?)?;
    let files = files()?;
    update::clear_old();
    // First, before any call that spends the Worker's tokens: a new build starts this again.
    let mut problems: Vec<String> = worker.build.as_ref().and_then(|wanted| prebuilt::level(agent, &files, wanted, argv)).into_iter().collect();
    let (config, state) = (settings(&files)?, State::open(agent, &files)?);
    // First, so a host that can't sign its agent in sets up nothing.
    let token = trade::claude(agent, &files)?;
    let dir = std::env::current_dir().context("reading the current folder")?.display().to_string();
    // A host with a credential pushes as the app: git's helper for this worktree's repo, in the
    // environment of the branch's push and of the agent alone, before either.
    let git = if host_credential(&files).is_some() { pushing(&dir, &mut problems) } else { Vec::new() };
    let started = Started::default();
    let linear = LinearApp { agent, state: &state, owner_id: &config.owner.id, started: &started };
    let mut run = |program: &str, args: &[&str], dir: Option<&str>| run_command_with(&git, program, args, dir);
    let ready = set_up(&mut Host { config: &config, linear: &linear, records: &state, github: &mut AppReader::new(agent, &|| {}), run: &mut run, dir: &dir, trust_file: &harness::settings_file()? }, &worker)?;
    // Before the agent's screen, in this terminal's scrollback: never Linear's.
    for problem in problems.iter().chain(&ready.problems) {
        eprintln!("tolkster worker: {problem}");
    }
    let env: Vec<(String, String)> = git.into_iter().chain(ready.env).collect();
    harness::exec(ready.harness, &ready.args, &env, token)
}

// The helper's environment, or none and why: the push then uses what the host has.
fn pushing(dir: &str, problems: &mut Vec<String>) -> Vec<(String, String)> {
    let env = origin_repo(&mut run_command, Some(dir)).ok().and_then(|repo| {
        let exe = std::env::current_exe().ok()?;
        git_env(std::env::var("GIT_CONFIG_COUNT").ok().as_deref(), exe.to_str()?, &repo)
    });
    env.unwrap_or_else(|| {
        problems.push("no git credential helper for this repo, so its pushes use the host's own login".into());
        Vec::new()
    })
}
