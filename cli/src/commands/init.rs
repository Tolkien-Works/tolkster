// `tolkster init --team <KEY>`: the owner's, as `repo map` is. What `doctor --team` finds
// missing that Tolkster can set up itself, it does: the team's flags, the workspace's
// `tolkster` group with its states, and the team's `repos` entries. The rest it lists as
// steps for the owner, and it ends with doctor's checks (docs/setup.md, "Adding a team or
// repo"). Every answer is settled before anything is written, and `repos` is merged before any
// label is created, so a missing answer or a refused edit writes nothing; what exists is left
// alone, so a second run changes nothing.

use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;

use serde::Serialize;
use serde_json::Value;
use ureq::Agent;

use crate::args::{Kind, parse};
use crate::cli::{config_file, print_json};
use crate::commands::doctor::{self, Check, Verdict, has_flag, the_team};
use crate::exec::run_command;
use crate::sessions::workspace::{DoctorTeam, TeamRead, read_team};
use crate::commands::repo::{Change, all_edited, max_rounds, merge, refuse_own_agent, reviewers};
use crate::config::{self, files, worker_url};
use crate::error::{Context, Error};
use crate::request::{MANUAL_MERGE_LABEL, PLAN_FIRST_LABEL};
use crate::linear::{create_label, install_url, with_token};
use crate::offline::{self, GROUP, Group, HOST_OFFLINE};
use crate::orca::{Repo, find_repo, repos};
use crate::config::repo_map::{Entry, Place, RepoMap};

pub const OPTIONS: &[(&str, Kind)] = &[("team", Kind::String), ("default-repo", Kind::String), ("yes", Kind::Boolean)];

// The states the group holds. Never a flag: a group allows one of its labels per issue.
const STATES: [&str; 1] = [HOST_OFFLINE];

// What init did, then doctor's checks, printed last.
#[derive(Serialize)]
struct Report {
    created: Vec<&'static str>,
    changed: bool,
    steps: Vec<String>,
    ok: bool,
    checks: Vec<Check>,
}

// The labels it created, by name, whether config.json changed, and the steps left to the owner.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Done {
    pub created: Vec<&'static str>,
    pub changed: bool,
    pub steps: Vec<String>,
}

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, OPTIONS, false)?;
    let key = parsed.non_empty("team").ok_or_else(|| Error::Usage("init needs --team <KEY>".into()))?;
    refuse_own_agent()?;
    let data = read_team(agent, key)?;
    let mut done = match the_team(&data) {
        Some(_) => {
            let orca = repos(&mut run_command, None)?;
            let group = offline::group(agent)?;
            let terminal = io::stdin().is_terminal() && io::stderr().is_terminal();
            let mut owner = Owner { default_repo: parsed.non_empty("default-repo").map(String::from), yes: parsed.flag("yes"), terminal };
            let file = config_file()?;
            // Before the labels are made, so a refusal here leaves nothing created unreported.
            config::adopt_schema(&file)?;
            init(key, &data, &orca, group.as_ref(), &mut owner, &mut App(agent), &file)?
        }
        None => Done { steps: vec![install_step(key)], ..Done::default() },
    };
    // What was done is printed even when the closing read fails.
    // Reading the settings can fail naming a local path, so no error is quoted.
    let checks = doctor::checks(agent, Some(key), None).unwrap_or_else(|_| {
        vec![Check { area: "team", subject: key.into(), check: "doctor", verdict: Verdict::Fail, reason: format!("the closing checks couldn't be read: run tolkster doctor --team {key} for them") }]
    });
    done.steps.extend(checks.iter().filter_map(step));
    let ok = doctor::passed(&checks);
    print_json(&Report { created: done.created, changed: done.changed, steps: done.steps, ok, checks })?;
    Ok(if ok { 0 } else { 1 })
}

// The team `key`, as `workspace::read_team` read it, set up: its answers taken first, then its
// `repos` entries merged into `file`, then its labels created.
pub(crate) fn init(key: &str, data: &TeamRead, repos: &[Repo], group: Option<&Group>, ask: &mut dyn Ask, linear: &mut dyn Labels, file: &Path) -> Result<Done, Error> {
    let team = the_team(data).ok_or_else(|| Error::Failed(format!("the app can see no team with the key {key}")))?;
    let config = match fs::read_to_string(file) {
        Ok(text) => serde_json::from_str(&text).context(file.display())?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Value::Null,
        Err(error) => return Err(error).context(file.display()),
    };
    let edits = mapping(config.get("repos"), repos, key, team, ask)?;
    let mut done = Done { changed: !edits.is_empty() && merge(file, |current| all_edited(current, &unset(current, &edits)))?, ..Done::default() };
    labels(data, key, team, group, linear, &mut done);
    Ok(done)
}

// The edits, with a repo's `review.reviewers` or `review.maxRounds` left out where `repos`, as
// the file has it at the write, sets them already: an answer only fills in what was unset when
// it was asked.
fn unset(current: Option<&Value>, edits: &[(String, Change)]) -> Vec<(String, Change)> {
    let Ok(map) = RepoMap::from_config(current) else { return edits.to_vec() };
    let edits = edits.iter().map(|(repo, change)| {
        let sets = |field: fn(&Entry) -> bool| map.entry(repo).is_some_and(field);
        let reviewers = change.reviewers.clone().filter(|_| !sets(|e| e.reviewers().is_some()));
        let max_rounds = change.max_rounds.filter(|_| !sets(|e| e.max_rounds().is_some()));
        (repo.clone(), Change { reviewers, max_rounds, ..change.clone() })
    });
    edits.collect()
}

// The edits that map the team: its default repo, unless an entry lists the team already; without
// one, a repo for each open project that no entry maps and no one Orca repo is named after; then,
// for each repo the team reaches, its `review.reviewers` and `review.maxRounds` where its entry
// sets neither.
fn mapping(current: Option<&Value>, repos: &[Repo], key: &str, team: &DoctorTeam, ask: &mut dyn Ask) -> Result<Vec<(String, Change)>, Error> {
    let map = RepoMap::from_config(current)?;
    let names: Vec<String> = repos.iter().map(|r| r.name.clone()).collect();
    let in_orca = |repo: &str| find_repo(repos, Some(repo)).map(|(_, name)| name).ok_or_else(|| Error::Failed(format!("Orca has no repo, or more than one, with the display name {repo}")));
    let mut edits = Vec::new();
    let default = Place { team: Some(key.into()), ..Place::default() };
    let mut defaulted = map.claims(&default);
    if !defaulted && let Some(repo) = ask.default_repo(key, &names)? {
        edits.push((in_orca(&repo)?, Change { teams: vec![key.into()], ..Change::default() }));
        defaulted = true;
    }
    let mut places = vec![default];
    // A finished project takes no new issues, as doctor's mapping check skips it.
    for project in team.projects.nodes.iter().filter(|p| p.is_open()) {
        let (id, name) = (project.id.clone(), project.name.clone());
        let place = Place { team: Some(key.into()), project_id: Some(id.clone()), project: Some(name.clone()) };
        let unmapped = !defaulted && !map.claims(&Place { team: None, ..place.clone() }) && find_repo(repos, Some(&name)).is_none();
        // By its ID, which a rename keeps and no other team's project has.
        if unmapped && let Some(repo) = ask.project_repo(&name, propose(&name, repos), &names)? {
            edits.push((in_orca(&repo)?, Change { projects: vec![id.clone()], known: vec![(id, name)], ..Change::default() }));
        }
        places.push(place);
    }
    let after = RepoMap::from_config(Some(&all_edited(current, &edits)?).filter(|v| !v.is_null()))?;
    let mut reached: Vec<String> = Vec::new();
    for (_, repo) in places.iter().filter_map(|place| find_repo(repos, after.repo_for(place))) {
        if !reached.iter().any(|r| r.eq_ignore_ascii_case(&repo)) {
            reached.push(repo);
        }
    }
    for repo in reached {
        let sets = |field: fn(&Entry) -> bool| after.entry(&repo).is_some_and(field);
        let reviewers = match sets(|e| e.reviewers().is_some()) {
            true => None,
            false => ask.reviewers(&repo)?,
        };
        let max_rounds = match sets(|e| e.max_rounds().is_some()) {
            true => None,
            false => ask.max_rounds(&repo)?,
        };
        if reviewers.is_some() || max_rounds.is_some() {
            edits.push((repo, Change { reviewers, max_rounds, ..Change::default() }));
        }
    }
    Ok(edits)
}

// The one Orca repo whose name's words are a run of the project's, ignoring case: `Site` for
// `Site redesign`, never `api` for `Capital`.
fn propose<'a>(project: &str, repos: &'a [Repo]) -> Option<&'a str> {
    let words = |name: &str| name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect::<Vec<_>>();
    let project = words(project);
    let matching: Vec<&str> = repos.iter().filter(|r| {
        let repo = words(&r.name);
        !repo.is_empty() && project.windows(repo.len()).any(|run| run == repo.as_slice())
    }).map(|r| r.name.as_str()).collect();
    match matching[..] {
        [one] => Some(one),
        _ => None,
    }
}

// The team's flags it lacks, created on the team, and the workspace's group with each state it
// lacks. A label Linear refuses becomes a step for the owner.
fn labels(data: &TeamRead, key: &str, team: &DoctorTeam, group: Option<&Group>, linear: &mut dyn Labels, done: &mut Done) {
    let team_id = Some(team.id.clone());
    for flag in [PLAN_FIRST_LABEL, MANUAL_MERGE_LABEL] {
        if has_flag(data, key, flag) {
            continue;
        }
        match linear.create(flag, team_id.as_deref(), false, None) {
            Ok(_) => done.created.push(flag),
            Err(_) => done.steps.push(format!("create a {flag} label on the team {key}, or in the workspace")),
        }
    }
    let group_step = || format!("create the workspace label group {GROUP} with the label {}", STATES.join(", "));
    let parent = match group {
        Some(group) => group.id.clone(),
        None => match linear.create(GROUP, None, true, None) {
            Ok(id) => {
                done.created.push(GROUP);
                id
            }
            Err(_) => {
                done.steps.push(group_step());
                return;
            }
        },
    };
    for state in STATES.into_iter().filter(|state| !group.is_some_and(|g| g.has(state))) {
        match linear.create(state, None, false, Some(&parent)) {
            Ok(_) => done.created.push(state),
            Err(_) => done.steps.push(format!("add the label {state} to the workspace label group {GROUP}")),
        }
    }
}

// What a doctor check that doesn't pass leaves to the owner, for those init can't fix.
fn step(check: &Check) -> Option<String> {
    let subject = &check.subject;
    let text = match (check.area, check.check) {
        _ if check.verdict == Verdict::Ok => return None,
        ("team", "review-state") => format!("give the team {subject} a started state with \"review\" in its name, after the one its work starts in"),
        ("repo", "github-app") => format!("install the GitHub App on {subject}"),
        ("repo", "auto-merge") => format!("turn on Allow auto-merge on {subject}"),
        ("repo", "required-check") => format!("require a status check on {subject}'s default branch"),
        ("repo", "instructions") => format!("give {subject} an AGENTS.md or CLAUDE.md with a tolkster-review block"),
        _ => return None,
    };
    Some(text)
}

// Granting the app the team, with the install URL when the client ID and the Worker's address
// are set.
fn install_step(key: &str) -> String {
    let url = files().ok().and_then(|files| Some((files.secret("clientId")?, worker_url(&files).ok()?)));
    match url {
        Some((client_id, worker)) => format!("grant the Linear app the team {key}: open {}", install_url(&client_id, &worker)),
        None => format!("grant the Linear app the team {key}: open the install URL tolkster setup prints"),
    }
}

// What a label is created through, so a test needs no Linear.
pub(crate) trait Labels {
    // The new label's ID, as `linear::create_label` makes it.
    fn create(&mut self, name: &str, team_id: Option<&str>, group: bool, parent_id: Option<&str>) -> Result<String, Error>;
}

struct App<'a>(&'a Agent);

impl Labels for App<'_> {
    fn create(&mut self, name: &str, team_id: Option<&str>, group: bool, parent_id: Option<&str>) -> Result<String, Error> {
        with_token(self.0, |token| create_label(self.0, token, name, team_id, group, parent_id))
    }
}

// The owner's answers. Each is Err(Usage) when it can't be asked and no flag gives it.
pub(crate) trait Ask {
    // The team's default repo, or None for none.
    fn default_repo(&mut self, key: &str, repos: &[String]) -> Result<Option<String>, Error>;
    // The repo for a project of a team without a default that nothing maps, or None for none.
    fn project_repo(&mut self, project: &str, proposal: Option<&str>, repos: &[String]) -> Result<Option<String>, Error>;
    // A repo's own reviewers and cap on review rounds, or None to leave the global one.
    fn reviewers(&mut self, repo: &str) -> Result<Option<Vec<String>>, Error>;
    fn max_rounds(&mut self, repo: &str) -> Result<Option<i64>, Error>;
}

// From the flags, else on a terminal, the questions on stderr so stdout stays JSON. `yes` takes
// each proposal and asks no repo's settings.
pub(crate) struct Owner {
    pub default_repo: Option<String>,
    pub yes: bool,
    pub terminal: bool,
}

impl Ask for Owner {
    fn default_repo(&mut self, key: &str, repos: &[String]) -> Result<Option<String>, Error> {
        match &self.default_repo {
            Some(repo) if repo.trim().eq_ignore_ascii_case("none") => Ok(None),
            Some(repo) => Ok(Some(repo.trim().to_string())),
            None if self.terminal => choose(&format!("The team {key}'s default repo, for an issue whose project reaches none"), repos, None),
            None => Err(Error::Usage("init needs --default-repo <name|none> for the team's default repo, without a terminal to ask on".into())),
        }
    }

    fn project_repo(&mut self, project: &str, proposal: Option<&str>, repos: &[String]) -> Result<Option<String>, Error> {
        match proposal {
            _ if self.yes => Ok(proposal.map(String::from)),
            _ if !self.terminal => Err(Error::Usage("init needs --yes to take its proposed repos for the projects nothing maps, without a terminal to ask on".into())),
            Some(proposal) => choose(&format!("The project {}'s repo, or 0 for none", shown(project)), repos, Some(proposal)),
            None => choose(&format!("The project {}'s repo, or 0 for none", shown(project)), repos, None),
        }
    }

    fn reviewers(&mut self, repo: &str) -> Result<Option<Vec<String>>, Error> {
        match self.terminal && !self.yes {
            true => answer(&format!("Reviewers for {repo}: GitHub logins, comma-separated, or none (Enter for the global setting)"), reviewers),
            false => Ok(None),
        }
    }

    fn max_rounds(&mut self, repo: &str) -> Result<Option<i64>, Error> {
        match self.terminal && !self.yes {
            true => answer(&format!("Review rounds for {repo} before Copilot's reviews go to you (Enter for the global setting)"), max_rounds),
            false => Ok(None),
        }
    }
}

// A Linear name as shown on the owner's terminal: no control characters and no invisible or bidi
// marks, so no escape sequence and no override reorders the question; no longer than a name needs.
pub(crate) fn shown(name: &str) -> String {
    let format = |c: char| matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}' | '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{FFF9}'..='\u{FFFB}' | '\u{E0000}'..='\u{E007F}');
    name.chars().filter(|&c| !c.is_control() && !format(c)).take(80).collect()
}

fn line(prompt: &str) -> Result<String, Error> {
    let mut stderr = io::stderr();
    let _ = write!(stderr, "{prompt}: ");
    let _ = stderr.flush();
    let mut text = String::new();
    match io::stdin().lock().read_line(&mut text).context("reading the answer")? {
        0 => Err(Error::Failed("init got no answer on stdin".into())),
        _ => Ok(text.trim().to_string()),
    }
}

// One of `repos` by its number, 0 for none, or Enter for `proposal`; asked again until valid.
fn choose(question: &str, repos: &[String], proposal: Option<&str>) -> Result<Option<String>, Error> {
    let mut stderr = io::stderr();
    let _ = writeln!(stderr, "{question}:\n  0) none");
    for (n, repo) in repos.iter().enumerate() {
        let _ = writeln!(stderr, "  {}) {repo}", n + 1);
    }
    let prompt = match proposal {
        Some(proposal) => format!("Number (Enter for {proposal})"),
        None => "Number".into(),
    };
    loop {
        let text = line(&prompt)?;
        match (text.as_str(), proposal) {
            ("", Some(proposal)) => return Ok(Some(proposal.into())),
            ("0", _) => return Ok(None),
            (text, _) => {
                if let Some(repo) = text.parse::<usize>().ok().and_then(|n| repos.get(n.wrapping_sub(1))) {
                    return Ok(Some(repo.clone()));
                }
            }
        }
    }
}

// A line read by `parse`, Enter for None; asked again until it parses.
fn answer<T>(prompt: &str, parse: fn(&str) -> Result<T, Error>) -> Result<Option<T>, Error> {
    loop {
        let text = line(prompt)?;
        if text.is_empty() {
            return Ok(None);
        }
        match parse(&text) {
            Ok(value) => return Ok(Some(value)),
            Err(error) => {
                let _ = writeln!(io::stderr(), "{error}");
            }
        }
    }
}

#[cfg(test)]
mod tests;
