// `tolkster doctor`: read-only checks, each `ok`, `warn` or `fail`, for docs/setup.md,
// "Adding a team or repo". A reason is written here from names Tolkster already holds
// (a team's key, a project's ID, an Orca repo's display name, a GitHub repo and its
// default branch): never a state's, label's or project's name, or any other Linear text,
// and never a secret.

use serde::Serialize;
use serde_json::json;
use ureq::Agent;

use crate::args::{Kind, parse};
use crate::cli::print_exit;
use crate::config::{files, host_credential};
use crate::exec::{Run, run_command};
use crate::request::{MANUAL_MERGE_LABEL, PLAN_FIRST_LABEL};
use crate::error::Error;
use crate::estimates::Scale;
use crate::github::{AppReader, GITHUB_EVENTS, Reader, readiness};
use crate::issue_state::{WorkflowState, filing_target, in_order, review_state};
use crate::linear::{Nodes, StateType};
use crate::issues::TeamProject;
use crate::sessions::workspace::{DoctorTeam, TeamRead, read_team};
use crate::offline::{self, Group, HOST_OFFLINE};
use crate::orca::{Repo, Worktree, find_repo, list_worktrees, primary_checkout, repos};
use crate::config::repo_map::{Place, RepoMap};
use crate::trade;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Ok,
    Warn,
    Fail,
}

// One check: what it is about (`area` "team", "mapping", "repo", "orca", "app", "worker", "harness" or "workspace", and
// `subject`, a team's key, a project's ID, an Orca repo's display name, a harness's name or, for the app, a
// webhook event), which check it is, its verdict and why.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Check {
    pub area: &'static str,
    pub subject: String,
    pub check: &'static str,
    pub verdict: Verdict,
    pub reason: String,
}

pub const OPTIONS: &[(&str, Kind)] = &[("team", Kind::String), ("repo", Kind::String)];

fn check(area: &'static str, subject: &str, check: &'static str, verdict: Verdict, reason: impl Into<String>) -> Check {
    Check { area, subject: subject.into(), check, verdict, reason: reason.into() }
}

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, OPTIONS, false)?;
    let (team, repo) = (parsed.non_empty("team"), parsed.non_empty("repo"));
    if team.is_none() && repo.is_none() {
        return Err(Error::Usage("doctor needs --team or --repo".into()));
    }
    let checks = checks(agent, team, repo)?;
    let ok = passed(&checks);
    print_exit(&json!({ "ok": ok, "checks": checks }), if ok { 0 } else { 1 })
}

// Every check `doctor` runs, with the settings as they are now.
pub fn checks(agent: &Agent, team: Option<&str>, repo: Option<&str>) -> Result<Vec<Check>, Error> {
    let files = files()?;
    let map = RepoMap::from_config(files.get("/repos"))?;
    let mut gh = AppReader::new(agent, &|| {});
    let mut checks = doctor(team, repo, &map, &|key| read_team(agent, key), &mut run_command, &mut gh)?;
    checks.push(secrets_check(trade::missing(agent, &files), host_credential(&files).is_some()));
    checks.push(group_check(offline::group(agent)));
    checks.extend(harness_checks(&crate::harnesses::KNOWN));
    Ok(checks)
}

pub fn passed(checks: &[Check]) -> bool {
    checks.iter().all(|c| c.verdict != Verdict::Fail)
}

// Every check for `team` and `repo`, reading Linear with `read_team`, Orca and git with
// `run`, and GitHub with `gh`. Err only when Linear can't be read: Orca or GitHub failing
// is a check that fails.
pub fn doctor(team: Option<&str>, repo: Option<&str>, map: &RepoMap, read_team: &dyn Fn(&str) -> Result<TeamRead, Error>, run: &mut Run, gh: &mut dyn Reader) -> Result<Vec<Check>, Error> {
    let mut checks = Vec::new();
    let data = team.map(read_team).transpose()?;
    let team = team.zip(data.as_ref());
    if let Some((key, data)) = team {
        checks.extend(team_checks(key, data));
    }
    let orca = repos(run, None).and_then(|repos| Ok((repos, list_worktrees(run)?)));
    let (repos, worktrees) = match orca {
        Ok(orca) => orca,
        Err(error) => {
            checks.push(check("orca", "orca", "listing", Verdict::Fail, format!("Orca's repos and worktrees couldn't be listed: {error}")));
            return Ok(checks);
        }
    };
    let mut reached = Vec::new();
    if let Some((key, found)) = team.and_then(|(key, data)| Some((key, the_team(data)?))) {
        let (mapping, repos) = mapping_checks(map, &repos, key, &found.projects);
        checks.extend(mapping);
        reached = repos;
    }
    let names = match repo {
        Some(repo) => vec![repo.to_string()],
        None => reached,
    };
    let any = !names.is_empty();
    for name in names {
        checks.extend(repo_checks(&name, &repos, &worktrees, gh));
    }
    if any {
        checks.extend(app_checks(gh));
    }
    Ok(checks)
}

// Whether the Worker holds every secret it hands out (docs/hosts.md), by name. One missing,
// or a failed ask, as a refused credential's is, fails on a worker host, `worker_host`, which
// can start no worker then; on the `listen` host, which holds its own Linear and Claude
// credentials, only a GitHub App secret's fails, since every host's GitHub tokens are minted
// with them.
pub fn secrets_check(missing: Result<Vec<String>, Error>, worker_host: bool) -> Check {
    let secrets = |verdict, reason: String| check("worker", "worker", "host-secrets", verdict, reason);
    let Some(why) = trade::lacking(&missing) else { return secrets(Verdict::Ok, "the Worker holds every secret it hands out".into()) };
    let fails = worker_host || missing.is_ok_and(|names| names.iter().any(|n| n.starts_with("GITHUB_")));
    secrets(if fails { Verdict::Fail } else { Verdict::Warn }, why)
}

// Whether each harness this build launches meets the unattended-safety requirements
// (docs/harness.md): one that doesn't is refused at launch, so no worker of it starts.
pub fn harness_checks(known: &[&crate::harnesses::Harness]) -> Vec<Check> {
    known
        .iter()
        .map(|harness| match harness.unmet().as_slice() {
            [] => check("harness", harness.name, "unattended", Verdict::Ok, "it blocks risky actions with no person, is trusted and starts no MCP servers"),
            unmet => check("harness", harness.name, "unattended", Verdict::Fail, format!("it can't run unattended safely, so none of its workers start: {}", unmet.join("; "))),
        })
        .collect()
}

// Whether the workspace has the label group of Tolkster's states with the label of a silent
// host. One check for the workspace, not one per team. Without it nothing is labelled and
// nothing else changes, so a miss only warns.
pub fn group_check(group: Result<Option<Group>, Error>) -> Check {
    let labels = |verdict, reason: &str| check("workspace", "workspace", "tolkster-label-group", verdict, reason);
    match group {
        Ok(Some(group)) if group.has(HOST_OFFLINE) => labels(Verdict::Ok, "the tolkster label group has the host offline label"),
        Ok(_) => labels(Verdict::Warn, "no workspace label group tolkster with a host offline label, so an issue isn't labelled while its worker's host doesn't answer"),
        Err(_) => labels(Verdict::Warn, "the workspace's labels couldn't be read, so whether the tolkster label group exists is unknown"),
    }
}

// Each webhook event the Worker takes, and whether the GitHub App is subscribed to it: one
// it isn't sends nothing, so what it would have told never reaches the machine.
pub fn app_checks(gh: &mut dyn Reader) -> Vec<Check> {
    let app = match gh.app() {
        Ok(app) => app,
        // Reading credentials.json can fail naming a local path, so no error is quoted here.
        Err(_) => return vec![check("app", "github-app", "events", Verdict::Fail, "the GitHub App's own record couldn't be read through the Worker: check the worker/host-secrets check, and that GitHub is reachable")],
    };
    GITHUB_EVENTS
        .iter()
        .map(|&event| match app.events.iter().any(|e| e == event) {
            true => check("app", event, "event", Verdict::Ok, "the GitHub App is subscribed to it"),
            false => check("app", event, "event", Verdict::Fail, "the GitHub App isn't subscribed to it, so GitHub never sends it"),
        })
        .collect()
}

pub fn the_team(data: &TeamRead) -> Option<&DoctorTeam> {
    data.teams.nodes.first()
}

pub fn team_checks(key: &str, data: &TeamRead) -> Vec<Check> {
    let team_check = |name, verdict, reason: &str| check("team", key, name, verdict, reason);
    let Some(team) = the_team(data) else {
        return vec![team_check("access", Verdict::Fail, "the app isn't granted a team with this key, or none has it")];
    };
    let mut checks = vec![team_check("access", Verdict::Ok, "the app is granted the team")];
    let states = in_order(team.states.nodes.clone());
    let started: Vec<WorkflowState> = states.iter().filter(|s| s.state_type == StateType::Started).cloned().collect();
    checks.push(match started.len() {
        0 => team_check("started-state", Verdict::Fail, "no started state, so a worker's issue can't move when its work starts"),
        _ => team_check("started-state", Verdict::Ok, "a worker's issue moves to its first started state"),
    });
    // The review state must come after the first started state, where work starts.
    checks.push(match review_state(&started) {
        Some(review) if started.first().is_some_and(|first| first.id == review.id) => team_check("review-state", Verdict::Fail, "the first started state has \"review\" in its name, so an issue shows as in review as soon as its work starts"),
        Some(_) => team_check("review-state", Verdict::Ok, "an issue moves to the first started state with \"review\" in its name once its PR is open"),
        None => team_check("review-state", Verdict::Fail, "no started state has \"review\" in its name, so an issue stays where it is once its PR is open"),
    });
    let triage_on = team.triage_enabled;
    checks.push(match filing_target(&states, triage_on) {
        Ok(state) if state.state_type == StateType::Triage => team_check("triage-state", Verdict::Ok, "Triage is on: follow-ups are filed in its triage state"),
        Ok(_) => team_check("triage-state", Verdict::Ok, "Triage is off: follow-ups are filed in its first backlog state"),
        Err(_) => team_check("triage-state", Verdict::Fail, "neither a triage nor a backlog state, so follow-ups can't be filed"),
    });
    checks.push(match Scale::of(&team.estimation) {
        Some(scale) => match scale.describe() {
            Some(values) => team_check("estimates", Verdict::Ok, &format!("sized on {values}")),
            None => team_check("estimates", Verdict::Ok, "the team doesn't use estimates, so nothing is sized"),
        },
        None => team_check("estimates", Verdict::Warn, "the team's estimate type couldn't be read"),
    });
    // Without manual-merge there is no way to keep a PR from auto-merging by hand, so it fails.
    for (label, name, missing, without) in [(PLAN_FIRST_LABEL, "plan-first-label", Verdict::Warn, "an issue can't be marked to plan first"), (MANUAL_MERGE_LABEL, "manual-merge-label", Verdict::Fail, "an issue can't be marked to merge by hand")] {
        checks.push(match has_flag(data, key, label) {
            true => team_check(name, Verdict::Ok, &format!("the {label} label exists")),
            false => team_check(name, missing, &format!("no {label} label in the workspace or the team, so {without}")),
        });
    }
    checks
}

// Whether the workspace, or the team `key`, has the owner's flag `label`, from TEAM's read.
pub fn has_flag(data: &TeamRead, key: &str, label: &str) -> bool {
    data.issue_labels.nodes.iter().any(|l| l.name.eq_ignore_ascii_case(label) && l.team.as_ref().is_none_or(|t| t.key.eq_ignore_ascii_case(key)))
}

// The repo each of the team `key`'s open `projects`, and its team default, reach, with the
// display names of the repos reached, in order, once each.
pub fn mapping_checks(map: &RepoMap, repos: &[Repo], key: &str, projects: &Nodes<TeamProject>) -> (Vec<Check>, Vec<String>) {
    let mut checks = Vec::new();
    let mut reached: Vec<String> = Vec::new();
    let mut verdict = |place: Place, name: &'static str, subject: &str, what: &str, unmapped: &str| {
        let reason = match find_repo(repos, map.repo_for(&place)) {
            Some((_, repo)) => {
                if !reached.iter().any(|r| r.eq_ignore_ascii_case(&repo)) {
                    reached.push(repo.clone());
                }
                (Verdict::Ok, format!("{what} reaches the repo {repo}"))
            }
            // By its project's own entry (or two entries naming it by ID and by name), or
            // else by the team's default, whose own check says so too.
            None if map.claims(&Place { team: None, ..place.clone() }) => (Verdict::Fail, format!("{what} is mapped by \"repos\" to a repo Orca doesn't have or has twice, or by two entries, so its issues reach none")),
            None if map.claims(&place) => (Verdict::Fail, format!("{what} falls to the team's default repo, which Orca doesn't have or has twice, so its issues reach none")),
            None => (Verdict::Warn, format!("{what} reaches no repo: {unmapped}")),
        };
        checks.push(check("mapping", subject, name, reason.0, reason.1));
    };
    // A completed or cancelled project takes no new issues (issues::pick_project), so an old
    // entry for one is left out as noise; an issue still open in one resolves through its
    // mapping all the same.
    for project in projects.nodes.iter().filter(|p| p.is_open()) {
        let place = Place { team: Some(key.into()), project_id: Some(project.id.clone()), project: Some(project.name.clone()) };
        verdict(place, "project", &project.id, "the project", "no entry of \"repos\" maps it, and no one Orca repo is named after it");
    }
    let default = Place { team: Some(key.into()), ..Place::default() };
    verdict(default, "team-default", key, "an issue without a mapped project", "no entry of \"repos\" lists the team");
    if projects.page_info.has_next_page {
        checks.push(check("mapping", key, "projects", Verdict::Warn, "the team has more than 250 projects; only the first 250 were checked"));
    }
    (checks, reached)
}

// The Orca repo with the display name `name`, from `repos` and `worktrees`, and its GitHub
// repo as the app sees it. Each check needs the one before it, so the first that fails
// ends them.
pub fn repo_checks(name: &str, repos: &[Repo], worktrees: &[Worktree], gh: &mut dyn Reader) -> Vec<Check> {
    let repo_check = |which: &'static str, verdict, reason: String| check("repo", name, which, verdict, reason);
    let fail = |which: &'static str, reason: &str| repo_check(which, Verdict::Fail, reason.into());
    let mut checks = Vec::new();
    // The failed check, as the last.
    let mut chain = || -> Result<(), Check> {
        let (id, _) = find_repo(repos, Some(name)).ok_or_else(|| fail("orca", "no Orca repo has this display name, or more than one does"))?;
        checks.push(repo_check("orca", Verdict::Ok, "registered in Orca".into()));
        let primary = primary_checkout(worktrees, &id).ok_or_else(|| fail("checkout", "no primary checkout on this workstation"))?;
        checks.push(repo_check("checkout", Verdict::Ok, "its primary checkout is on this workstation".into()));
        let origin = primary.origin.as_deref().ok_or_else(|| fail("origin", "the primary checkout's origin isn't a GitHub repo, or couldn't be read"))?;
        checks.push(repo_check("origin", Verdict::Ok, format!("origin is {origin} on GitHub")));
        let read = readiness(gh, origin).map_err(|error| fail("github-app", &format!("{origin} couldn't be read as the GitHub App: {error}")))?;
        checks.push(repo_check("github-app", Verdict::Ok, format!("the GitHub App is installed on {origin}")));
        let branch = read.default_branch.ok_or_else(|| fail("default-branch", "the repo has no default branch"))?;
        checks.push(repo_check("default-branch", Verdict::Ok, format!("the default branch is {branch}")));
        checks.push(match read.auto_merge {
            true => repo_check("auto-merge", Verdict::Ok, "the repo allows auto-merge".into()),
            false => repo_check("auto-merge", Verdict::Fail, "the repo doesn't allow auto-merge, so a worker's PR can't merge by itself".into()),
        });
        checks.push(match read.required_check {
            Ok(true) => repo_check("required-check", Verdict::Ok, format!("{branch} requires a status check")),
            Ok(false) => repo_check("required-check", Verdict::Warn, format!("{branch} requires no status check, so GitHub may refuse a worker's auto-merge, and a PR that merges doesn't wait for CI")),
            Err(error) => repo_check("required-check", Verdict::Warn, format!("whether {branch} requires a status check couldn't be read: {error}")),
        });
        checks.push(match read.instructions {
            true => repo_check("instructions", Verdict::Ok, "the repo has an AGENTS.md or a CLAUDE.md".into()),
            false => repo_check("instructions", Verdict::Warn, "no AGENTS.md or CLAUDE.md at the repo's root, so workers get no repo rules".into()),
        });
        Ok(())
    };
    if let Err(failed) = chain() {
        checks.push(failed);
    }
    checks
}

#[cfg(test)]
mod tests;
