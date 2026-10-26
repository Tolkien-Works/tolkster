// What `tolkster listen` (the parent module) does for what isn't one of the machines'
// commands (command.rs): what the watcher saw, a closed terminal and a base tip. Each loads the
// runtime (runtime.rs) fresh.

use std::sync::Arc;

use ureq::Agent;

use crate::converse;
use crate::error::Error;
use crate::exec::{beating, run_command};
use crate::orca::{Cli, Orca};
use crate::plain::Sha;
use crate::runtime::{Shared, load, log};
use crate::state::{CheckoutRecord, Records};
use crate::trace::Refresh;
use crate::watch::{Agents, Missed};

// What the watcher's first listing, `now`, can't see change (watch.rs, `Missed`): each
// recorded conversations checkout with no terminal open counts as closed, safe to repeat.
// An agent that exited while nothing listened is the first listing's, which the Durable
// Object derives each worker's state from. A triage session's end is the triage machine's,
// which reads Orca again at its deadline (`observeTriage`).
pub fn missed(agent: &Agent, shared: &Arc<Shared>, now: &Agents) -> Result<Missed, Error> {
    let rt = load(agent, shared)?;
    Ok(Missed { closed: missed_closed(&rt.state.checkouts()?, now) })
}

// Of `checkouts`, those with no terminal open in `now`.
fn missed_closed(checkouts: &[CheckoutRecord], now: &Agents) -> Vec<String> {
    checkouts.iter().filter(|c| !now.open.contains(&c.worktree_id)).map(|c| c.worktree_id.clone()).collect()
}

// The last terminal open in the worktree `worktree_id` closed. In a repo's conversations
// checkout, a turn's terminal closes once the turn has ended: the checkout is refreshed
// if nothing has opened there since, under the checkouts' lock (converse::refresh_idle).
// Not on the agent's exit, as the turn's terminal outlives its Claude.
pub fn closed(agent: &Agent, shared: &Arc<Shared>, worktree_id: &str, beat: &dyn Fn()) {
    log(closed_once(agent, shared, worktree_id, beat));
}

fn closed_once(agent: &Agent, shared: &Arc<Shared>, worktree_id: &str, beat: &dyn Fn()) -> Result<Vec<Refresh>, Error> {
    let rt = load(agent, shared)?;
    let records: Vec<_> = rt.state.checkouts()?.into_iter().filter(|r| r.worktree_id == worktree_id).collect();
    if records.is_empty() {
        return Ok(Vec::new());
    }
    let mut shell = beating(beat);
    let mut orca = Cli::new(&mut shell, &shared.health).at_once(run_command);
    let worktrees = orca.agents()?;
    Ok(converse::refresh_idle(&shared.checkouts, &mut orca, &records, &worktrees, None))
}

// A push to `repo`'s default branch, at `after`, as the relay sent it on connect or the push:
// the repo's conversations checkouts no turn runs in are brought to it now, so the next turn
// starts without a fetch (converse::refresh_idle). A tip already seen does nothing again.
pub fn base(agent: &Agent, shared: &Arc<Shared>, repo: &str, after: &Sha, beat: &dyn Fn()) {
    if shared.checkouts.base_tip(repo, after) {
        log(base_once(agent, shared, repo, after, beat));
    }
}

fn base_once(agent: &Agent, shared: &Arc<Shared>, repo: &str, after: &Sha, beat: &dyn Fn()) -> Result<Vec<Refresh>, Error> {
    let rt = load(agent, shared)?;
    let records = rt.state.checkouts()?;
    if records.is_empty() {
        return Ok(Vec::new());
    }
    let mut shell = beating(beat);
    let mut orca = Cli::new(&mut shell, &shared.health).at_once(run_command);
    // With their origins, which name the repo.
    let worktrees = orca.worktrees()?;
    Ok(converse::refresh_idle(&shared.checkouts, &mut orca, &converse::checkouts_of(&records, &worktrees, repo), &worktrees, Some(after)))
}

#[cfg(test)]
mod tests;
