// `tolkster host add <environment> --destination <path>`, on the `listen` host: makes a paired
// Orca server a worker host (docs/hosts.md) by onboard.rs's steps. `--rejoin` joins it again
// whatever its `tolkster` says, for a host whose settings this build changed.

use serde_json::json;
use ureq::Agent;

use crate::args::{Kind, parse};
use crate::cli::print_exit;
use crate::config::files;
use crate::error::Error;
use crate::exec::run_command;
use crate::onboard::{Step, passed, set_up};
use crate::orca::paired;
use crate::plain::{plain_destination, plain_host};

pub const OPTIONS: &[(&str, Kind)] = &[("destination", Kind::String), ("recipe", Kind::String), ("repo", Kind::String), ("rejoin", Kind::Boolean)];

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, OPTIONS, true)?;
    let [environment] = parsed.positionals.as_slice() else { return Err(Error::Usage("host add needs one paired Orca environment, by name or ID, or with --recipe the new host's name".into())) };
    if let Some(recipe) = parsed.non_empty("recipe") {
        if parsed.non_empty("destination").is_some() {
            return Err(Error::Usage("host add --recipe takes no --destination: the recipe's machine is set up in its projectRoot".into()));
        }
        return super::recipe::add(agent, environment, recipe, parsed.non_empty("repo"));
    }
    let destination = parsed.non_empty("destination").filter(|d| plain_destination(d)).ok_or_else(|| Error::Usage("host add needs --destination, the absolute folder on the host the repos are cloned into: letters, digits and / \\ : . _ - only".into()))?;
    let files = files()?;
    let mut run = run_command;
    let hosts = paired(&mut run)?;
    let host = hosts.iter().find(|h| &h.name == environment || &h.id == environment).ok_or_else(|| Error::Usage(format!("no paired Orca environment is named {environment}: pair it in Orca first")))?;
    if !plain_host(&host.name) {
        return Err(Error::Usage("the environment's name must be letters, digits, '_', '.' and '-': rename it in Orca".into()));
    }
    if host.name == crate::config::LOCAL {
        return Err(Error::Usage(format!("\"{}\" is this machine's name in slots: rename the environment in Orca", crate::config::LOCAL)));
    }
    let steps = set_up(agent, &files, &mut run, host, destination, None, parsed.flag("rejoin"))?;
    report(&host.name, steps)
}

pub(super) fn report(host: &str, steps: Vec<Step>) -> Result<u8, Error> {
    let ok = passed(&steps);
    print_exit(&json!({ "host": host, "ok": ok, "steps": steps }), if ok { 0 } else { 1 })
}

#[cfg(test)]
mod tests;
