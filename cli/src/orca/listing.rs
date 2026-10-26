// What a host's Orca lists, decoded once from its answers: its worktrees with their agents
// (`worktree ps`) and its open terminals (`terminal list`), and what callers find in them.

use std::collections::HashSet;

use serde::Deserialize;
use serde_json::Value;

use super::routing::hosted;
use super::{Answer, same_path};
use crate::error::Error;
use crate::json::decode;
use crate::plain::{split_identifier, split_worktree, Terminal as Handle};

// What one host lists (`Orca::listing_at`): its worktrees with their agents, without their
// origins, every one's ID when its listing is complete (`Ps::complete`), and its terminals.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listing {
    pub worktrees: Vec<Worktree>,
    pub listed: Option<Vec<String>>,
    pub terminals: Vec<Terminal>,
}

impl Listing {
    pub fn worktree(&self, worktree_id: &str) -> Option<&Worktree> {
        self.worktrees.iter().find(|w| w.id == worktree_id)
    }

    // Whether the host's complete listing lacks `worktree_id`.
    pub fn lacks(&self, worktree_id: &str) -> bool {
        self.listed.as_ref().is_some_and(|ids| !ids.iter().any(|id| id == worktree_id))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Worktree {
    pub id: String,
    pub repo_id: String,
    pub path: String,
    pub branch: Option<String>,
    pub linked_issue: Option<String>,
    pub is_main: bool,
    pub is_local: bool,
    pub agents: Vec<Pane>,
    // `owner/name` of the repo's `origin`, where its PRs are: None when it couldn't be read
    // or isn't GitHub's.
    pub origin: Option<String>,
}

impl Worktree {
    // The host it runs on, from its ID: None on this machine.
    pub fn host(&self) -> Option<&str> {
        split_worktree(&self.id).and_then(|(host, _, _)| host)
    }

    pub fn origin(&self) -> Result<&str, Error> {
        self.origin.as_deref().ok_or_else(|| Error::Failed(format!("the origin of {} is not a GitHub repo, or couldn't be read", self.path)))
    }
}

// An agent in a worktree. `key` is `<tabId>:<leafId>` of its terminal; `state` is Orca's
// word for it: `working`, `waiting` (a permission prompt or a question), `blocked` or
// `done` with its turn; `since_ms` is since when, as milliseconds since the epoch.
#[derive(Clone, Debug, PartialEq)]
pub struct Pane {
    pub key: String,
    pub state: String,
    pub since_ms: Option<i64>,
}

// `orca worktree ps`'s answer.
pub type Ps = Answer<PsResult>;

#[derive(Debug, Deserialize)]
pub struct PsResult {
    // A listing that doesn't say is taken as cut off.
    truncated: Option<bool>,
    worktrees: Vec<Row>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    worktree_id: String,
    repo_id: String,
    // `local` on every row of a routed reply too: the host is the call's (`hosted`).
    host_id: Option<String>,
    path: String,
    branch: Option<String>,
    // Null for a worktree linked to no issue.
    linked_linear_issue: Option<String>,
    is_main_worktree: bool,
    agents: Vec<AgentRow>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentRow {
    pane_key: String,
    state: String,
    state_started_at: Option<i64>,
}

impl Ps {
    pub fn read(answer: Value) -> Result<Ps, Error> {
        decode(answer, "Orca's answer to worktree ps")
    }

    // The rows of `host`'s listing, each named with the host whose call listed it, since a
    // routed reply calls every row `local`.
    pub fn worktrees(&self, host: Option<&str>) -> Vec<Worktree> {
        let some = |text: &Option<String>| text.clone().filter(|text| !text.is_empty());
        self.own_rows()
            .map(|row| Worktree {
                id: hosted(host, &row.worktree_id),
                repo_id: row.repo_id.clone(),
                path: row.path.clone(),
                branch: some(&row.branch).map(|b| b.strip_prefix("refs/heads/").unwrap_or(&b).to_string()),
                linked_issue: some(&row.linked_linear_issue).map(|i| i.to_ascii_uppercase()),
                is_main: row.is_main_worktree,
                // A worktree on another host can't be checked with git from here.
                is_local: host.is_none(),
                // One with no key or state names no agent a terminal could match.
                agents: row.agents.iter().filter(|a| !a.pane_key.is_empty() && !a.state.is_empty()).map(|a| Pane { key: a.pane_key.clone(), state: a.state.clone(), since_ms: a.state_started_at }).collect(),
                origin: None,
            })
            .collect()
    }

    // Every worktree on `host`, None when its listing is cut off at its limit: only a complete
    // listing may say a worktree is gone. A call answers for its own host alone, whatever others
    // it names as omitted: their own calls cover them.
    pub fn complete(&self, host: Option<&str>) -> Option<Vec<String>> {
        (self.result.truncated == Some(false)).then(|| self.own_rows().map(|row| hosted(host, &row.worktree_id)).collect())
    }

    // A row another host owns isn't the call's to answer for.
    fn own_rows(&self) -> impl Iterator<Item = &Row> {
        self.result.worktrees.iter().filter(|row| row.host_id.as_deref().is_none_or(|h| h.is_empty() || h == "local"))
    }
}

// An open terminal as Orca lists it. `handle` and `worktree_id` are None when Orca printed
// none; `agent` when its `agentIdentity` names one, which a shell's doesn't; `pane` is
// `<tabId>:<leafId>`, its agent's key, None without both.
#[derive(Clone, Debug, PartialEq)]
pub struct Terminal {
    pub handle: Option<String>,
    pub worktree_id: Option<String>,
    pub agent: bool,
    pub pane: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Terminals {
    terminals: Vec<TerminalRow>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalRow {
    handle: Option<String>,
    worktree_id: Option<String>,
    // Absent or null on a shell's.
    agent_identity: Option<String>,
    // One Orca doesn't call orphaned is one it runs.
    #[serde(default)]
    orphaned: bool,
    tab_id: Option<String>,
    leaf_id: Option<String>,
}

// `orca terminal list`'s answer. An orphaned terminal's process is gone, so it isn't listed.
// Its handle and worktree are named with `host`, whose listing it is; an empty handle stays
// empty, naming no terminal.
pub fn parse_terminals(listed: Value, host: Option<&str>) -> Result<Vec<Terminal>, Error> {
    let listed: Answer<Terminals> = decode(listed, "Orca's answer to terminal list")?;
    let some = |text: Option<String>| text.filter(|text| !text.is_empty());
    Ok(listed
        .result
        .terminals
        .into_iter()
        .filter(|t| !t.orphaned)
        .map(|t| Terminal {
            handle: t.handle.map(|h| if h.is_empty() { h } else { hosted(host, &h) }),
            worktree_id: t.worktree_id.map(|w| hosted(host, &w)),
            agent: t.agent_identity.is_some_and(|a| !a.is_empty()),
            pane: match (some(t.tab_id), some(t.leaf_id)) {
                (Some(tab), Some(leaf)) => Some(format!("{tab}:{leaf}")),
                _ => None,
            },
        })
        .collect())
}

impl Terminal {
    fn in_worktree(&self, worktree_id: &str) -> bool {
        self.worktree_id.as_deref() == Some(worktree_id)
    }

    fn plain_handle(&self) -> Option<Handle> {
        self.handle.as_deref().and_then(Handle::parse)
    }
}

// Whether an agent runs in the worktree, whatever handle Orca lists it with. A shell isn't
// an agent's.
pub fn agent_in(terminals: &[Terminal], worktree_id: &str) -> bool {
    agents(terminals).any(|t| t.in_worktree(worktree_id))
}

// The handle to type to or close the worktree's agent by. None when no agent runs there, or
// when Orca lists it with an empty or unplain handle, which would name the active terminal:
// whether one runs is `agent_in`'s.
pub fn agent_terminal(terminals: &[Terminal], worktree_id: &str) -> Option<Handle> {
    agents(terminals).filter(|t| t.in_worktree(worktree_id)).find_map(Terminal::plain_handle)
}

// A terminal without a tab and a leaf names no pane, so matches no agent.
pub fn agent_panes(terminals: &[Terminal], worktree_id: &str) -> Vec<String> {
    agents(terminals).filter(|t| t.in_worktree(worktree_id)).map(|t| t.pane.clone().unwrap_or_default()).collect()
}

// Whether any agent open in the worktree waits at a prompt, as Orca has it, whatever the
// others are doing: unlike `watch::idle_in`, one working beside it doesn't hide it, as a line
// typed into the worktree's agent terminal could reach the waiting one.
pub fn waits_in(worktree: &Worktree, terminals: &[Terminal]) -> bool {
    agent_panes(terminals, &worktree.id).iter().any(|pane| worktree.agents.iter().any(|a| &a.key == pane && matches!(a.state.as_str(), "waiting" | "blocked")))
}

// The agents' terminals in `worktrees`, by their plain handles: an empty one would name the
// active terminal.
pub fn agent_handles(terminals: &[Terminal], worktrees: &HashSet<String>) -> HashSet<Handle> {
    agents(terminals).filter(|t| t.worktree_id.as_ref().is_some_and(|id| worktrees.contains(id))).filter_map(Terminal::plain_handle).collect()
}

pub fn agent_worktrees(terminals: &[Terminal]) -> HashSet<String> {
    agents(terminals).filter_map(|t| t.worktree_id.clone()).collect()
}

fn agents(terminals: &[Terminal]) -> impl Iterator<Item = &Terminal> {
    terminals.iter().filter(|t| t.agent)
}

pub fn terminal_worktrees(terminals: &[Terminal]) -> HashSet<String> {
    terminals.iter().filter_map(|t| t.worktree_id.clone()).collect()
}

// The open terminals' plain handles, to find one by. Whether any is open in a worktree is
// `terminal_worktrees`', as one may be listed with no handle to name it by.
pub fn open_terminals(terminals: &[Terminal], worktree_id: Option<&str>) -> HashSet<Handle> {
    terminals.iter().filter(|t| worktree_id.is_none_or(|id| t.in_worktree(id))).filter_map(Terminal::plain_handle).collect()
}

// Might be a worker's, on any host: linked to an issue, and not a primary checkout.
pub fn candidates(worktrees: &[Worktree]) -> Vec<&Worktree> {
    worktrees.iter().filter(|w| !w.is_main && w.linked_issue.as_deref().and_then(split_identifier).is_some()).collect()
}

// A primary on another host is no use to git from here.
pub fn primary_checkout<'w>(worktrees: &'w [Worktree], repo_id: &str) -> Option<&'w Worktree> {
    worktrees.iter().find(|w| w.is_main && w.is_local && w.repo_id == repo_id)
}

// The worktree at `top`, a path on this machine, if it is a checkout of `repo`: only this
// machine's worktrees are, and an origin that couldn't be read never matches.
pub fn checkout_of<'w>(worktrees: &'w [Worktree], top: &str, repo: &str) -> Option<&'w Worktree> {
    worktrees.iter().filter(|w| w.is_local).find(|w| same_path(&w.path, top)).filter(|w| w.origin.as_deref().is_some_and(|o| o.eq_ignore_ascii_case(repo)))
}

#[cfg(test)]
mod tests;
