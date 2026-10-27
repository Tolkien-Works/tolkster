// `tolkster reap`: removes the worktrees whose PR has merged, whose issue was cancelled a day ago or completed with no PR (see ../reap.rs). The machine's `reapWorktree` also reaps a landed worker's.
// `tolkster reap-here`: one worktree reaped on its own host, on the rule another host sent
// (reap::Step), in a terminal there. Not for people.

use ureq::Agent;

use crate::args::{Kind, parse};
use crate::cli::print;
use crate::config::{files, owner};
use crate::error::Error;
use crate::exec::run_command;
use crate::github::{AppReader, Branches};
use crate::health::Health;
use crate::orca::{Cli, Orca};
use crate::plain::IssueKey;
use crate::reap::{KEPT, STEP_OPTIONS, Step, read_prs, reap, reap_here};
use crate::sessions::tolksters_issues;
use crate::state::{Records, State};

pub const OPTIONS: &[(&str, Kind)] = &[("dry-run", Kind::Boolean)];

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, OPTIONS, false)?;
    let files = files()?;
    let state = State::open(agent, &files)?;
    let owner_id = owner(&files)?;
    let mut run = run_command;
    let health = Health::default();
    let mut orca = Cli::paired(&mut run, &health).at_once(run_command);
    let worktrees = orca.worktrees()?;
    let (mut gh, mut branches) = (AppReader::new(agent, &|| {}), Branches::default());
    let tolksters = |identifiers: &[IssueKey]| tolksters_issues(agent, identifiers, &owner_id);
    // Each worktree removed has its worker record forgotten.
    let reaped = reap(&mut orca, &mut read_prs(&mut gh, &mut branches), &mut |id| state.forget_worker(id), &worktrees, tolksters, parsed.flag("dry-run"))?;
    for line in reaped.lines {
        print(&line.to_string());
    }
    Ok(0)
}

// Exits 0 once it removed the worktree, KEPT where it kept it or found none, and 1 on an error.
pub fn run_here(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let step = Step::parse(&parse(argv, &STEP_OPTIONS, false)?)?;
    let owner_id = owner(&files()?)?;
    let mut run = run_command;
    let (mut gh, mut branches) = (AppReader::new(agent, &|| {}), Branches::default());
    let health = Health::default();
    let tolksters = |identifiers: &[IssueKey]| tolksters_issues(agent, identifiers, &owner_id);
    let (lines, removed) = reap_here(&mut Cli::new(&mut run, &health).at_once(run_command), &step, &mut read_prs(&mut gh, &mut branches), tolksters)?;
    for line in lines {
        print(&line.to_string());
    }
    Ok(if removed { 0 } else { KEPT as u8 })
}
