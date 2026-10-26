// `tolkster host add <name> --recipe <id>`, on the `listen` host: a worker host on a machine a
// recipe makes (recipe.rs). Running it is the owner's go for `create`, which costs money.

use ureq::Agent;

use super::add::report;
use crate::config::{files, realtime};
use crate::error::Error;
use crate::exec::run_command;
use crate::onboard::{Author, NO_AUTHOR, unstamped};
use crate::orca::{paired, recipe_passes};
use crate::plain::plain_host;
use crate::prebuilt;
use crate::recipe::{find, make_named, recorded_machines};
use crate::runtime::settings;

pub fn add(agent: &Agent, name: &str, id: &str, repo: Option<&str>) -> Result<u8, Error> {
    if !plain_host(name) || name == crate::config::LOCAL {
        return Err(Error::Usage(format!("host add --recipe needs the new host's name: letters, digits, '_', '.' and '-', and not \"{}\"", crate::config::LOCAL)));
    }
    // What would stop the setup is found before anything is paid for.
    let files = files()?;
    realtime(&files)?;
    unstamped(prebuilt::commit())?;
    if Author::read(&mut run_command).is_none() {
        return Err(Error::Usage(NO_AUTHOR.into()));
    }
    if recorded_machines()?.contains_key(name) {
        return Err(Error::Usage(format!("a recipe already made a machine for {name}: tolkster host rm {name} destroys it first")));
    }
    let mut run = run_command;
    if paired(&mut run)?.iter().any(|host| host.name == name) {
        return Err(Error::Usage(format!("Orca already has an environment named {name}: pick another name")));
    }
    let recipe = find(&mut run, id, repo)?;
    // Its cap is the repo's `slotsEach`: without one, a paid machine would take no work.
    if settings(&files)?.repos.entry(&recipe.repo).and_then(|entry| entry.recipe.as_ref()).is_none() {
        return Err(Error::Usage(format!("repos.{} in config.json has no recipe, whose slotsEach would be the machine's cap: add one first", recipe.repo)));
    }
    if !recipe_passes(&mut run, &recipe.id, &recipe.folder) {
        return Err(Error::Usage(format!("Orca's checks fail the recipe {id}: orca vm recipe doctor {id} --repo-path <the repo's folder> says why")));
    }
    match make_named(agent, &files, &mut run, &recipe, name)? {
        Some(steps) => report(name, steps),
        None => Err(Error::Usage(format!("a machines.json entry or Orca environment has the name {name}, ignoring case: pick another name"))),
    }
}
