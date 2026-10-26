// A build that isn't a development one keeping itself current: a push to `main` names a
// commit, and `listen` swaps in the Worker's build of it for its target (docs/hosts.md,
// "Builds"). It builds and pulls nothing, so it runs in the shared lane whole: one
// download and a rename. A commit CI hasn't built yet is held and asked for again
// (`Tips`). The restart onto it is the drain's (`Update::in_place`).

use std::sync::atomic::Ordering;

use ureq::Agent;

use super::{Pass, Update, own_branch, short};
use crate::config::files;
use crate::error::{Context, Error};
use crate::exec::Run;
use crate::plain::Sha;
use crate::prebuilt;
use crate::trace::{Restart, Unfit, Update as Said};
use crate::trade;

pub struct Updater {
    repo: String,
    own: Sha,
    target: &'static str,
    agent: Agent,
    pub update: Update,
}

// A build is taken whole, never started on a thread: no pass starts one.
pub enum Build {}

impl Updater {
    // Fit when CI stamped this build with its commit and repo, and it is a platform CI builds.
    pub fn open(agent: &Agent, _run: &mut Run) -> Result<Updater, Unfit> {
        let (Some(own), Some(repo)) = (prebuilt::commit(), prebuilt::repo()) else { return Err(Unfit::Unstamped) };
        let target = prebuilt::target().ok_or(Unfit::OtherPlatform)?;
        Ok(Updater { repo, own, target, agent: agent.clone(), update: Update::default() })
    }

    // Nothing to catch up on as it starts: the relay sends each repo's base tip on connecting.
    pub fn catch_up(&self) {}

    pub fn is_own(&self, repo: &str, reference: &str) -> bool {
        self.repo.eq_ignore_ascii_case(repo) && own_branch(reference)
    }

    // Skipped with no commit to ask for. Behind while the Worker has no build of `after` yet,
    // and Unready when it couldn't be reached, so the tip is held and tried again (`Tips`);
    // Skipped on any other refusal or a failed swap, which a retry won't change.
    pub fn pass(&self, run: &mut Run, say: &mut dyn FnMut(Said), after: Option<&Sha>, _start: impl FnOnce(Build)) -> Pass {
        self.take(run, say, after, |commit| files().and_then(|files| trade::build(&self.agent, &files, self.target, commit)))
    }

    // `fetch` asks the Worker for the build of a commit.
    fn take(&self, run: &mut Run, say: &mut dyn FnMut(Said), after: Option<&Sha>, fetch: impl FnOnce(&Sha) -> Result<Option<Vec<u8>>, Error>) -> Pass {
        let Some(after) = after else { return Pass::Skipped };
        if self.update.draining() {
            say(Said::Draining);
            return Pass::Unready;
        }
        if *after == self.own {
            say(Said::At { head: short(after).into() });
            return Pass::Pulled;
        }
        let tarball = match fetch(after) {
            Ok(Some(tarball)) => tarball,
            Ok(None) => {
                say(Said::NotHeld { after: short(after).into() });
                return Pass::Behind;
            }
            Err(error) => {
                let held = matches!(error, Error::Unavailable(_));
                say(Said::NotFetched(error));
                return if held { Pass::Unready } else { Pass::Skipped };
            }
        };
        // A restart now could find the installed path empty, as a Windows swap renames it aside first.
        self.update.placing.store(true, Ordering::SeqCst);
        let swapped = prebuilt::own_exe().context("finding this tolkster").and_then(|exe| prebuilt::swap_in(&exe, &tarball, after, run));
        self.update.placing.store(false, Ordering::SeqCst);
        match swapped {
            Ok(()) => {
                say(Said::Restarting { head: short(after).into() });
                self.update.in_place(Restart::Built { head: short(after).into() });
                Pass::Pulled
            }
            Err(error) => {
                say(Said::NotPlaced(error));
                Pass::Skipped
            }
        }
    }
}

#[cfg(test)]
mod tests;
