// Which Orca repo an issue's work runs in (`repo_for`), from the `repos` setting
// (docs/setup.md, step 6). First match wins: the issue's project by an entry, then its team
// by an entry's team default, then an Orca repo whose display name is the project's name.
// The answer is always a configured or Orca-registered repo: no Linear text names a path.

use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};
use serde_json::Value;

use crate::config::{DEFAULT_DESTROY_AFTER, DEFAULT_IDLE_MINUTES, keys, names, whole_number};
use crate::error::Error;
use crate::json::str;
use crate::trace;

// Each None when the issue has none or the read didn't ask.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Place {
    pub team: Option<String>,
    pub project_id: Option<String>,
    pub project: Option<String>,
}

impl Place {
    // An issue's, from its team's key and its project's ID and name, as a read gives them.
    pub fn of(team: Option<&str>, project: Option<(&str, &str)>) -> Place {
        Place { team: team.map(String::from), project_id: project.map(|(id, _)| id.into()), project: project.map(|(_, name)| name.into()) }
    }
}

// A repo's opt-in to machines made from its Orca recipe: the recipe's id in the repo's
// orca.yaml, how many machines may exist at once (spend), each one's cap in place of a `slots`
// entry, how many minutes one stays idle before it sleeps, and how many more it stays dormant
// before it is destroyed (hosts::dormant).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipeMachines {
    pub id: String,
    pub machines: i64,
    pub slots_each: i64,
    pub idle_minutes: i64,
    pub destroy_after: i64,
}

// A repo's own review settings, each None where it leaves the global one; Config fills them in
// with the effective values, so `tolkster config` prints what each repo gets.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoReview {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_rounds: Option<i64>,
}

// How a worker's change reaches the default branch (docs/landing.md): through a PR, or
// pushed straight onto it by `tolkster land`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Land {
    #[default]
    Pr,
    Direct,
}

// One repo's entry, `repo` its key in `repos`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    #[serde(skip)]
    pub repo: String,
    pub projects: Vec<String>,
    pub teams: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review: Option<RepoReview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipe: Option<RecipeMachines>,
    pub land: Land,
    // Its workers' sessions show their narration and each call (hook::narrate).
    pub trace: bool,
}

impl Entry {
    pub fn reviewers(&self) -> Option<&Vec<String>> {
        self.review.as_ref()?.reviewers.as_ref()
    }

    pub fn max_rounds(&self) -> Option<i64> {
        self.review.as_ref()?.max_rounds
    }
}

// The entries keyed by their repo's display name, as config.json holds them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RepoMap(pub Vec<Entry>);

impl Serialize for RepoMap {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for entry in &self.0 {
            map.serialize_entry(&entry.repo, entry)?;
        }
        map.end()
    }
}

pub(crate) const ENTRY: &[&str] = &["projects", "teams", "review", "recipe", "land", "trace"];
pub(crate) const REVIEW: &[&str] = &["reviewers", "maxRounds"];
pub(crate) const RECIPE: &[&str] = &["id", "machines", "slotsEach", "idleMinutes", "destroyAfter"];

impl RepoMap {
    // Each entry is keyed by an Orca repo's display name, once ignoring case, and names at least
    // one project or team, or sets the repo's `review` or `recipe`; no project or team is claimed
    // twice, ignoring case, even within one entry: which of two entries wins would otherwise be
    // a guess. A project named by its ID in one entry and by its name in another can't be seen
    // here; `repo_for` maps it to no repo.
    pub fn from_config(value: Option<&Value>) -> Result<RepoMap, Error> {
        let Some(value) = value.filter(|v| !v.is_null()) else { return Ok(RepoMap::default()) };
        let bad = |why: &str| Error::Usage(format!("\"repos\" in config.json {why}"));
        let entries = value.as_object().ok_or_else(|| bad("must be an object keyed by each Orca repo's display name"))?;
        let entries = entries.iter().map(|(key, entry)| {
            let repo = key.trim();
            let at = |why: &str| bad(&format!("\"{repo}\": {why}"));
            if repo.is_empty() {
                return Err(bad("has an entry with no repo: each key is an Orca repo's display name"));
            }
            let moved = [("repo", Some("the entry's key")), ("reviewers", Some("review.reviewers")), ("maxRounds", Some("review.maxRounds"))];
            keys(entry, &format!("repos.{repo}"), ENTRY, &moved)?;
            keys(&entry["review"], &format!("repos.{repo}.review"), REVIEW, &[])?;
            keys(&entry["recipe"], &format!("repos.{repo}.recipe"), RECIPE, &[("cap", Some("machines"))])?;
            if !entry.is_object() {
                return Err(at("must be an object"));
            }
            let list = |name: &str| -> Result<Vec<String>, Error> {
                match entry.get(name) {
                    None | Some(Value::Null) => Ok(vec![]),
                    Some(v) => names(v).ok_or_else(|| at(&format!("\"{name}\" must be a list of names"))),
                }
            };
            let (projects, teams) = (list("projects")?, list("teams")?);
            let review = match entry.get("review") {
                None | Some(Value::Null) => None,
                Some(v) => Some(repo_review(v).map_err(|why| at(&why))?).filter(|r| *r != RepoReview::default()),
            };
            let recipe = match entry.get("recipe") {
                None | Some(Value::Null) => None,
                Some(v) => Some(recipe_machines(v).ok_or_else(|| at("\"recipe\" must be {\"id\": the recipe's id, \"machines\" and \"slotsEach\": whole numbers of at least 1, \"idleMinutes\": an optional whole number of at least 1, \"destroyAfter\": an optional one of at least 60}"))?),
            };
            let land = match entry.get("land") {
                None | Some(Value::Null) => None,
                Some(v) => Some(match v.as_str() {
                    Some("pr") => Land::Pr,
                    Some("direct") => Land::Direct,
                    _ => return Err(at("\"land\" must be \"pr\" or \"direct\"")),
                }),
            };
            let trace = match entry.get("trace") {
                None | Some(Value::Null) => None,
                Some(Value::Bool(on)) => Some(*on),
                Some(_) => return Err(at("\"trace\" must be true or false")),
            };
            if projects.is_empty() && teams.is_empty() && review.is_none() && recipe.is_none() && land.is_none() && trace.is_none() {
                return Err(at("must name at least one of \"projects\" or \"teams\", or set \"review\", \"recipe\", \"land\" or \"trace\""));
            }
            Ok(Entry { repo: repo.into(), projects, teams, review, recipe, land: land.unwrap_or_default(), trace: trace.unwrap_or_default() })
        });
        let entries = entries.collect::<Result<Vec<_>, Error>>()?;
        let repos = twice(entries.iter().map(|e| &e.repo)).map(|r| ("names the repo", r));
        let projects = || twice(entries.iter().flat_map(|e| &e.projects)).map(|p| ("claims the project", p));
        let teams = || twice(entries.iter().flat_map(|e| &e.teams)).map(|t| ("claims the team", t));
        if let Some((what, name)) = repos.or_else(projects).or_else(teams) {
            return Err(bad(&format!("{what} \"{name}\" more than once")));
        }
        Ok(RepoMap(entries))
    }

    // The entry for the repo with this display name, ignoring case.
    pub fn entry(&self, repo: &str) -> Option<&Entry> {
        self.0.iter().find(|e| same(&e.repo, repo))
    }

    // The display name the Orca repo has, which a caller looks up in Orca's repos
    // (`orca::find_repo`). A rule that matches decides, even when its repo isn't in Orca or is
    // named twice there: the issue then has no repo rather than one a later rule guesses.
    pub fn repo_for<'a>(&'a self, place: &'a Place) -> Option<&'a str> {
        let by = |rule: &dyn Fn(&Entry) -> bool| match self.0.iter().filter(|e| rule(e)).collect::<Vec<_>>()[..] {
            [] => None,
            [entry] => Some(Some(entry.repo.as_str())),
            _ => Some(None),
        };
        by(&|e| by_project(e, place)).or_else(|| by(&|e| by_team(e, place))).unwrap_or(place.project.as_deref())
    }

    // Whether an entry maps `place`: `repo_for`'s answer is then that entry's, even when it
    // finds no repo, which `tolkster doctor` tells from a place nothing maps.
    pub fn claims(&self, place: &Place) -> bool {
        self.0.iter().any(|e| by_project(e, place) || by_team(e, place))
    }
}

// None unless it is an object of `id`, `machines`, `slotsEach` and optionally `idleMinutes` and
// `destroyAfter`, its keys checked before.
fn recipe_machines(value: &Value) -> Option<RecipeMachines> {
    let fields = value.as_object()?;
    let id = str(value, "/id").map(str::trim).filter(|s| !s.is_empty())?;
    let machines = whole_number(fields.get("machines")?, 1)?;
    let slots_each = whole_number(fields.get("slotsEach")?, 1)?;
    let minutes = |key: &str, default: i64, least: i64| match fields.get(key) {
        None | Some(Value::Null) => Some(default),
        Some(v) => whole_number(v, least),
    };
    // At least an hour: a mark `host add` holds in another process while it makes its machine
    // is dormant here (hosts::dormant).
    let (idle_minutes, destroy_after) = (minutes("idleMinutes", DEFAULT_IDLE_MINUTES, 1)?, minutes("destroyAfter", DEFAULT_DESTROY_AFTER, 60)?);
    Some(RecipeMachines { id: id.into(), machines, slots_each, idle_minutes, destroy_after })
}

// A repo's `review`: `reviewers` and `maxRounds`, each optional.
fn repo_review(value: &Value) -> Result<RepoReview, String> {
    if !value.is_object() {
        return Err("\"review\" must be an object of \"reviewers\" and \"maxRounds\"".into());
    }
    let reviewers = match value.get("reviewers") {
        None | Some(Value::Null) => None,
        Some(v) => Some(names(v).ok_or("\"review.reviewers\" must be a list of GitHub logins")?),
    };
    let max_rounds = match value.get("maxRounds") {
        None | Some(Value::Null) => None,
        Some(v) => Some(whole_number(v, 1).ok_or("\"review.maxRounds\" must be a whole number of at least 1")?),
    };
    Ok(RepoReview { reviewers, max_rounds })
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn by_project(entry: &Entry, place: &Place) -> bool {
    entry.projects.iter().any(|p| [&place.project_id, &place.project].into_iter().flatten().any(|q| same(p, q)))
}

fn by_team(entry: &Entry, place: &Place) -> bool {
    place.team.as_deref().is_some_and(|t| entry.teams.iter().any(|k| same(k, t)))
}

fn twice<'a>(names: impl Iterator<Item = &'a String>) -> Option<String> {
    let mut names: Vec<String> = names.map(|n| n.to_ascii_lowercase()).collect();
    names.sort();
    names.windows(2).find(|pair| pair[0] == pair[1]).map(|pair| pair[0].clone())
}

// Why an issue's repo has no checkout to open a session in (`brief::Checkouts`).
#[derive(Debug, PartialEq, Eq)]
pub enum Missing {
    Repo,
    Primary,
}

impl From<Missing> for trace::Missing {
    fn from(missing: Missing) -> trace::Missing {
        match missing {
            Missing::Repo => trace::Missing::Repo,
            Missing::Primary => trace::Missing::Primary,
        }
    }
}

#[cfg(test)]
mod tests;
