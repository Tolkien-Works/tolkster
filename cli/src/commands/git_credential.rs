// `tolkster git-credential <owner/name> <get|store|erase>`: the helper git runs for a
// worker's pushes (credential.rs). Only `get` answers; there is nothing to remember or forget,
// for every push asks for a token of its own. Not for people.

use std::io::{self, Read, Write};

use ureq::Agent;

use crate::config::files;
use crate::credential::answer;
use crate::error::{Context, Error};
use crate::github::push_token;
use crate::plain::split_repo;

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let [repo, operation] = argv else { return Err(Error::Usage("git-credential needs <owner/name> and <get|store|erase>".into())) };
    let repo = split_repo(repo).ok_or_else(|| Error::Usage("git-credential needs <owner/name> as GitHub spells it".into()))?;
    if operation != "get" {
        return Ok(0);
    }
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).context("reading git's request")?;
    let reply = answer(&input, || push_token(agent, &files()?, repo))?;
    let _ = io::stdout().write_all(reply.as_bytes());
    Ok(0)
}
