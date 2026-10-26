// `tolkster hosts`: this machine and each paired host, whether it answers now (every paired
// host is asked afresh, so a silent one costs Orca's timeout), its cap, by the hosts holding a
// admitted credential now, and the issues of the workers Tolkster's state holds on it (hosts.rs).
// Read-only.

use ureq::Agent;

use crate::args::parse;
use crate::cli::print_json;
use crate::config::{Config, files};
use crate::error::Error;
use crate::exec::run_command;
use crate::hosts::report;
use crate::health::Health;
use crate::orca::{Cli, Orca};
use crate::runtime::settings;
use crate::state::{Records, State};

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    parse(argv, &[], false)?;
    let files = files()?;
    let state = State::open(agent, &files)?;
    let config = Config { enrolled: state.enrolled_hosts()?, ..settings(&files)? };
    let records = state.workers()?;
    let mut run = run_command;
    let health = Health::default();
    let mut orca = Cli::paired(&mut run, &health).at_once(run_command);
    orca.listed()?;
    print_json(&report(&config, &orca.answering(), &orca.silent(), &records))?;
    Ok(0)
}
