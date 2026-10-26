// `tolkster members`: the workspace's users, or one team's, with their Linear IDs, read
// with the app token. How setup finds the owner's ID (`owner.id`, `OWNER_USER_ID`)
// without a personal API key. Prints names and IDs only, as JSON.

use serde_json::{Value, json};
use ureq::Agent;

use crate::args::{Kind, parse};
use crate::cli::print_json;
use crate::error::Error;
use crate::sessions::workspace::{self, User};

pub const OPTIONS: &[(&str, Kind)] = &[("team", Kind::String)];

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, OPTIONS, false)?;
    let users = match parsed.non_empty("team").map(str::to_ascii_uppercase) {
        Some(key) => workspace::team_members(agent, &key)?.ok_or_else(|| Error::Failed(format!("no team with key {key}")))?,
        None => workspace::users(agent)?,
    };
    print_json(&render(users))?;
    Ok(0)
}

// The workspace's active users, by name, as `members` prints them.
pub fn workspace(agent: &Agent) -> Result<Vec<User>, Error> {
    Ok(active(workspace::users(agent)?))
}

// The active users, by name.
fn active(users: Vec<User>) -> Vec<User> {
    let mut users: Vec<User> = users.into_iter().filter(|u| u.active).collect();
    users.sort_by(|a, b| a.name.cmp(&b.name));
    users
}

// The active users, by name, as `members` prints them.
pub fn render(users: Vec<User>) -> Vec<Value> {
    active(users).into_iter().map(|u| json!({ "id": u.id, "name": u.name, "displayName": u.display_name, "admin": u.admin, "guest": u.guest })).collect()
}

#[cfg(test)]
mod tests;
