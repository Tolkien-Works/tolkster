// Which host's Orca a call goes to, and how a worktree's ID or a terminal's handle names it.

use serde::Deserialize;
use serde_json::Value;

use super::{orca_json, Answer, Run};
use crate::error::Error;
use crate::json::decode;
use crate::plain::{plain_host, split_terminal, Terminal as Handle, WorktreeId};

// A paired Orca server: its environment's ID, which every call routed to it names, and its
// name, which `slots` keys.
#[derive(Clone, Debug, PartialEq)]
pub struct Host {
    pub id: String,
    pub name: String,
}

// `orca host list`'s answer: this machine, and each SSH host and paired server it reaches.
#[derive(Debug, Deserialize)]
struct Hosts {
    hosts: Vec<HostRow>,
}

#[derive(Debug, Deserialize)]
struct HostRow {
    kind: String,
    id: String,
    // A host with none is named by its ID.
    #[serde(default)]
    name: Option<String>,
}

// The paired servers, from this machine's pairing store, with no network call. An SSH host
// takes no `--environment`, and an ID that isn't plain would reach a command line: neither
// is a host.
pub fn paired(run: &mut Run) -> Result<Vec<Host>, Error> {
    hosts_of(orca_json(run, &HOSTS)?)
}

pub(super) const HOSTS: [&str; 3] = ["host", "list", "--json"];

pub(super) fn hosts_of(listed: Value) -> Result<Vec<Host>, Error> {
    let listed: Answer<Hosts> = decode(listed, "Orca's answer to host list")?;
    // A list with no host at all (Orca lists this machine's) is no list: a host missing from a
    // list releases its workers.
    if listed.result.hosts.is_empty() {
        return Err(Error::Failed("orca host list named no hosts".into()));
    }
    Ok(listed
        .result
        .hosts
        .into_iter()
        .filter(|h| h.kind == "environment" && plain_host(&h.id))
        .map(|h| Host { name: h.name.filter(|name| !name.is_empty()).unwrap_or_else(|| h.id.clone()), id: h.id })
        .collect())
}

// Where a new worktree is made: in a repo of this machine's Orca by its ID, or on a paired host
// by the repo's project ID (`github:<owner>/<name>`), which means the same on every host where
// a repo's ID doesn't.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place<'a> {
    Here { repo_id: &'a str },
    On { host: &'a str, project: &'a str },
}

pub(super) fn by_id(id: &str) -> String {
    format!("id:{id}")
}

// `args`, ending in `--json`, sent to `host`'s Orca, as the environment it is paired as: as
// they are for this machine's.
pub(super) fn at<'a>(host: Option<&'a str>, args: &[&'a str]) -> Vec<&'a str> {
    let (json, rest) = args.split_last().expect("an Orca call ends in --json");
    let mut routed = rest.to_vec();
    if let Some(host) = host {
        routed.extend_from_slice(&["--environment", host]);
    }
    routed.push(json);
    routed
}

// What `host`'s Orca named `id` (a worktree's ID or a terminal's handle) as Tolkster names
// it: behind `<host>@` on a remote host (plain::WorktreeId::hosted, plain::split_terminal).
pub(super) fn hosted(host: Option<&str>, id: &str) -> String {
    host.map_or_else(|| id.to_string(), |host| format!("{host}@{id}"))
}

// A worktree's host, and the selector its own host's Orca knows it by.
pub(super) fn worktree_at(worktree_id: &WorktreeId) -> (Option<&str>, String) {
    (worktree_id.host(), by_id(&worktree_id.own()))
}

// A handle goes back only to the host whose Orca made it.
pub(super) fn terminal_at(handle: &Handle) -> (Option<&str>, &str) {
    split_terminal(handle.as_str()).unwrap_or((None, handle.as_str()))
}
