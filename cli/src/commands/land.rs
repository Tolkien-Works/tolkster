// `tolkster land`: a worker's change pushed straight onto the default branch, with no PR, in a
// repo whose `land` is `direct` (docs/landing.md). The direct-mode counterpart of `pr create`.

use serde_json::json;
use ureq::Agent;

use crate::args::{self, Kind, parse};
use crate::cli::{logged, nothing_to_do, print_exit, read_body};
use crate::commands::asker::trusted_session;
use crate::config::{Config, files, owner};
use crate::runtime::settings;
use crate::error::Error;
use crate::exec::{Run, run_command};
use crate::git::{self, Push, Rebased};
use crate::issue_state::complete_issue;
use crate::machine;
use crate::orca::{self, origin_repo};
use crate::plain::{BranchName, IssueKey, Sha};
use crate::config::repo_map::Land;
use crate::sessions::manual_merge;
use crate::state::State;
use crate::linear::{add_session_link, with_token};

const S: Kind = Kind::String;
pub const OPTIONS: &[(&str, Kind)] = &[("session", S), ("title", S), ("body-file", S)];

// How many times a push origin refused, as its default branch had moved on, is fetched,
// rebased and made again.
const PUSH_TRIES: u32 = 4;
// Exit 7, as `pr checks` exits on a PR that conflicts with its base.
const CONFLICTS: u8 = 7;

// Every check is made before the branch changes, and the push is the one write that can't be taken
// back: what follows it is said in the JSON when it fails, and a run again pushes
// nothing and makes each of them again.
pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, OPTIONS, false)?;
    let value = |name| parsed.non_empty(name);
    let (Some(session_id), Some(title), Some(body_file)) = (value("session"), value("title"), value("body-file")) else {
        return Err(Error::Usage("land needs --session, --title and --body-file".into()));
    };
    // One line, so the subject always ends with the issue's key, which a run again looks for.
    if title.chars().any(char::is_control) {
        return Err(Error::Usage("--title must be one line".into()));
    }
    let body = args::file_body(body_file, "message", read_body)?;
    let run = &mut run_command;
    let repo = origin_repo(run, None)?;
    let files = files()?;
    let owner = owner(&files)?;
    let config = settings(&files)?;
    let session = with_token(agent, |token| trusted_session(agent, token, session_id, &owner))?;
    let Some(issue) = session.issue else { return Err(Error::Failed(format!("session {session_id} has no issue"))) };
    let key = IssueKey::parse(&issue.identifier).ok_or_else(|| Error::Failed(format!("the session's issue has no plain identifier: {}", issue.identifier)))?;
    let here = here(&config, run, &repo, &key)?;
    let branch = refusal(here, manual_merge(agent, &issue.id)?, git::current_branch(run, ".")?, &key).map_err(Error::Usage)?;
    if git::uncommitted(run, ".")? {
        return Err(Error::Failed("a tracked file has changes: commit them first".into()));
    }
    // A finished issue is never pushed, though one an earlier run landed and completed is
    // still printed.
    let stop = issue.state_type.is_finished().then(|| format!("{} is finished: nothing more lands for it", key.upper()));
    let (sha, pushed) = match land_branch(run, ".", &branch, &key, &format!("{title} ({})", key.upper()), &body, stop.as_deref())? {
        Landing::Nothing => return nothing_to_do("nothing to land: HEAD changes nothing origin's default branch lacks"),
        Landing::Conflicts(paths) => {
            return print_exit(&json!({ "conflicts": paths }), CONFLICTS);
        }
        Landing::Landed { sha, pushed } => (sha, pushed),
    };
    let url = format!("https://github.com/{repo}/commit/{sha}");
    let done = logged(with_token(agent, |token| complete_issue(agent, token, &issue.id)), "the issue wasn't moved to Done").map_or("failed", |moved| if moved { "moved" } else { "unchanged" });
    let label = format!("Commit {}", crate::plain::short_sha(&sha));
    let linked = logged(with_token(agent, |token| add_session_link(agent, token, session_id, &label, &url)), "the commit wasn't linked on the session").is_some();
    // Every branch of the issue's: a worker continued on `<key>-2` was reset onto `<key>`'s.
    let (mut branches, listed) = logged(git::issue_branches(run, ".", &key), "origin's branches of the issue weren't listed").map_or((Vec::new(), false), |branches| (branches, true));
    if !branches.contains(&branch) {
        branches.push(branch);
    }
    let deleted: Vec<bool> = branches.iter().map(|branch| git::delete_remote_branch(run, ".", branch).map_err(|error| eprintln!("{error}")).is_ok()).collect();
    let branch_deleted = listed && deleted.into_iter().all(|deleted| deleted);
    // Last, as the machine's reap of this worktree may follow it, and only unless the move to
    // Done failed, which a reap on another host waits for: a run again makes both.
    let reported = done != "failed" && {
        let report = machine::landed(&issue.id, session_id, &sha).ok_or_else(|| Error::Failed("a landing's report of the wrong shape".into()));
        logged(report.and_then(|report| State::open(agent, &files)?.reported(Some(report))), "the landing wasn't reported").is_some()
    };
    print_exit(&json!({ "sha": sha, "url": url, "pushed": pushed, "done": done, "linked": linked, "branchDeleted": branch_deleted, "reported": reported }), 0)
}

// What `refusal` reads of this checkout in Orca: how its repo lands, and whether it is a
// worker's worktree linked to the issue.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Here {
    land: Land,
    linked: bool,
}

// Orca is asked only when some repo lands direct, and then a checkout it can't place is an
// error.
fn here(config: &Config, run: &mut Run, repo: &str, key: &IssueKey) -> Result<Here, Error> {
    if !config.repos.0.iter().any(|entry| entry.land == Land::Direct) {
        return Ok(Here { land: Land::Pr, linked: false });
    }
    let worktrees = orca::list_worktrees(run)?;
    let top = git::toplevel(run)?;
    let Some(here) = orca::checkout_of(&worktrees, &top, repo) else {
        return Err(Error::Failed(format!("this checkout isn't an Orca worktree of {repo}, so how it lands can't be told")));
    };
    let name = orca::repo_name(&orca::repos(run, None)?, &here.repo_id);
    let land = name.and_then(|name| config.repos.entry(&name)).map_or(Land::Pr, |entry| entry.land);
    let linked = !here.is_main && here.linked_issue.as_deref().is_some_and(|issue| issue.eq_ignore_ascii_case(key));
    Ok(Here { land, linked })
}

// The branch to land, or why `land` won't run: a repo that lands through PRs, an issue left
// for the owner to merge, or a checkout that isn't the issue's own worktree, by Orca's link,
// on the issue's own branch.
fn refusal(here: Here, manual_merge: bool, branch: Option<BranchName>, key: &IssueKey) -> Result<BranchName, String> {
    if here.land != Land::Direct {
        return Err("this repo lands through a PR: its \"land\" in config.json isn't \"direct\"; open one with tolkster pr create".into());
    }
    if manual_merge {
        return Err(format!("{} is left for the owner to merge (manual-merge): open a PR with tolkster pr create", key.upper()));
    }
    match branch {
        Some(branch) if here.linked && git::issue_branch(&branch, key) => Ok(branch),
        _ => Err(format!("land pushes only the branch of {}'s own worktree, and this checkout isn't it", key.upper())),
    }
}

#[derive(Debug, PartialEq)]
enum Landing {
    // `pushed` false when an earlier run had landed it.
    Landed { sha: Sha, pushed: bool },
    Nothing,
    Conflicts(Vec<String>),
}

// HEAD's commits squashed into one, `subject` and `body`, rebased onto origin's default
// branch and pushed onto it fast-forward only, fetched, rebased and pushed again while origin's
// has moved on. A HEAD already on origin's is landed when it is the issue's own squashed
// commit, made on `branch` here, so a run again pushes nothing; any other has nothing to land,
// as has one that changes nothing there. `stop`, when set, is why nothing may be pushed.
fn land_branch(run: &mut Run, dir: &str, branch: &BranchName, key: &IssueKey, subject: &str, body: &str, stop: Option<&str>) -> Result<Landing, Error> {
    git::fetch(run, dir)?;
    let (target, mut tip, head) = (git::default_branch(run, dir)?, git::default_tip(run, dir)?, git::head(run, dir)?);
    if git::on_default(run, dir, &head)? {
        let own = git::subject(run, dir)?.to_ascii_lowercase().ends_with(&format!(" ({})", key.to_ascii_lowercase())) && git::made_on(run, dir, branch, &head)?;
        return Ok(if own { Landing::Landed { sha: head, pushed: false } } else { Landing::Nothing });
    }
    if git::changed_files(run, dir, &tip)?.is_empty() {
        return Ok(Landing::Nothing);
    }
    if let Some(why) = stop {
        return Err(Error::Usage(why.into()));
    }
    git::squash(run, dir, &tip, subject, body)?;
    for attempt in 1..=PUSH_TRIES {
        if let Rebased::Conflicts(paths) = git::rebase(run, dir, &tip)? {
            return Ok(Landing::Conflicts(paths));
        }
        // The rebase dropped the commit: its change is on origin's already.
        if git::head(run, dir)? == tip {
            return Ok(Landing::Nothing);
        }
        match git::push_fast_forward(run, dir, &target)? {
            Push::Pushed => return Ok(Landing::Landed { sha: git::head(run, dir)?, pushed: true }),
            Push::Behind if attempt < PUSH_TRIES => {
                // The other landing's push may still hold the shared refs: one fetch more.
                git::fetch(run, dir).or_else(|_| git::fetch(run, dir))?;
                tip = git::default_tip(run, dir)?;
            }
            Push::Behind => {}
        }
    }
    Err(Error::Failed(format!("origin's {target} kept moving: {PUSH_TRIES} pushes were refused")))
}

#[cfg(test)]
mod tests;
