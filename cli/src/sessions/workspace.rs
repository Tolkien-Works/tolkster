// What subcommands read of the workspace: the app user, the people, the teams and the release
// pipelines.

use serde::Deserialize;
use serde_json::{Value, json};
use ureq::Agent;

use crate::error::Error;
use crate::estimates::{Estimation, with_team_fields};
use crate::issue_state::WorkflowState;
use crate::issues::{Label, TeamProject};
use crate::linear::{Nodes, Ref, TeamKey, read_as, read_with};
use crate::plain::Sha;

#[derive(Debug, Deserialize)]
pub struct Viewer {
    pub id: String,
    pub name: String,
}

#[track_caller]
pub fn viewer(agent: &Agent) -> Result<Viewer, Error> {
    #[derive(Deserialize)]
    struct Read {
        viewer: Viewer,
    }
    Ok(read_as::<Read>(agent, "query Viewer { viewer { id name } }", json!({}))?.viewer)
}

// The app user's name, and whether it can see a team.
#[track_caller]
pub fn installed(agent: &Agent) -> Result<(String, bool), Error> {
    #[derive(Deserialize)]
    struct Named {
        name: String,
    }
    #[derive(Deserialize)]
    struct Read {
        viewer: Named,
        teams: Nodes<Ref>,
    }
    let read: Read = read_as(agent, "query Installed { viewer { name } teams(first: 1) { nodes { id } } }", json!({}))?;
    Ok((read.viewer.name, !read.teams.nodes.is_empty()))
}

const USER_FIELDS: &str = "id name displayName active admin guest";

// A person in the workspace, as `tolkster members` prints them.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub active: bool,
    pub admin: bool,
    pub guest: bool,
}

#[track_caller]
pub fn users(agent: &Agent) -> Result<Vec<User>, Error> {
    #[derive(Deserialize)]
    struct Read {
        users: Nodes<User>,
    }
    Ok(read_as::<Read>(agent, &format!("query Users {{ users(first: 250) {{ nodes {{ {USER_FIELDS} }} }} }}"), json!({}))?.users.nodes)
}

// The members of the team `key`, None when the app can see no such team.
#[track_caller]
pub fn team_members(agent: &Agent, key: &str) -> Result<Option<Vec<User>>, Error> {
    #[derive(Deserialize)]
    struct Team {
        members: Nodes<User>,
    }
    #[derive(Deserialize)]
    struct Read {
        teams: Nodes<Team>,
    }
    let query = format!("query Members($key: String!) {{ teams(first: 1, filter: {{ key: {{ eq: $key }} }}) {{ nodes {{ members(first: 250) {{ nodes {{ {USER_FIELDS} }} }} }} }} }}");
    Ok(read_as::<Read>(agent, &query, json!({ "key": key }))?.teams.nodes.into_iter().next().map(|team| team.members.nodes))
}

// The team by its key, ignoring case, if the app can see it, with what doctor's checks read:
// its Triage setting and estimate scale, its states' IDs, names, types and positions, and
// its projects' IDs and names, and the workspace's labels that have either name Tolkster
// looks for. Names are compared, never printed.
const TEAM: &str = r#"query Doctor($key: String!) {
  teams(first: 1, filter: { key: { eqIgnoreCase: $key } }) { nodes {
    id key triageEnabled TEAM_FIELDS
    states(first: 250) { nodes { id name type position } }
    projects(first: 250) { nodes { id name completedAt canceledAt } pageInfo { hasNextPage } }
  } }
  issueLabels(first: 250, filter: { or: [{ name: { eqIgnoreCase: "plan-first" } }, { name: { eqIgnoreCase: "manual-merge" } }] }) { nodes { name team { key } } }
}"#;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamRead {
    pub teams: Nodes<DoctorTeam>,
    pub issue_labels: Nodes<FlagLabel>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorTeam {
    pub id: String,
    pub triage_enabled: bool,
    #[serde(flatten)]
    pub estimation: Estimation,
    pub states: Nodes<WorkflowState>,
    pub projects: Nodes<TeamProject>,
}

// A workspace's or team's label, by name, with the team it is on, if any.
#[derive(Debug, Deserialize)]
pub struct FlagLabel {
    pub name: String,
    pub team: Option<TeamKey>,
}

#[track_caller]
pub fn read_team(agent: &Agent, key: &str) -> Result<TeamRead, Error> {
    read_as(agent, &with_team_fields(TEAM), json!({ "key": key }))
}

// The teams `filter` names, or every team the app can see, each with its projects.
const TEAMS: &str = r#"query RepoMap($filter: TeamFilter) {
  teams(first: 50, filter: $filter) { nodes { key projects(first: 100) { nodes { id name } } } }
}"#;

#[derive(Debug, Deserialize)]
pub struct KeyedTeam {
    pub key: String,
    pub projects: Nodes<crate::linear::Project>,
}

#[track_caller]
pub fn teams_with_projects(agent: &Agent, filter: Value) -> Result<Vec<KeyedTeam>, Error> {
    #[derive(Deserialize)]
    struct Read {
        teams: Nodes<KeyedTeam>,
    }
    Ok(read_as::<Read>(agent, TEAMS, json!({ "filter": filter }))?.teams.nodes)
}

// A team a new issue is filed in, with its estimate scale (estimates::TEAM_FIELDS).
#[derive(Clone, Debug, Deserialize)]
pub struct FilingTeam {
    pub id: String,
    #[serde(flatten)]
    pub estimation: Estimation,
}

const TEAM_BY_KEY: &str = "query Team($key: String!) { teams(filter: { key: { eqIgnoreCase: $key } }) { nodes { id TEAM_FIELDS } } }";

// The team with the key `key`, if the app can see it.
#[track_caller]
pub fn team_by_key(agent: &Agent, token: &str, key: &str) -> Result<Option<FilingTeam>, Error> {
    #[derive(Deserialize)]
    struct Read {
        teams: Nodes<FilingTeam>,
    }
    let query = with_team_fields(TEAM_BY_KEY);
    let read: Read = read_with(agent, token, &query, json!({ "key": key }))?;
    Ok(read.teams.nodes.into_iter().next())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamStates {
    pub triage_enabled: bool,
    pub states: Nodes<WorkflowState>,
}

const STATES: &str = "query States($id: String!) { team(id: $id) { triageEnabled states(first: 250) { nodes { id name type position } } } }";

#[track_caller]
pub fn team_states(agent: &Agent, token: &str, team_id: &str) -> Result<TeamStates, Error> {
    #[derive(Deserialize)]
    struct Read {
        team: TeamStates,
    }
    Ok(read_with::<Read>(agent, token, STATES, json!({ "id": team_id }))?.team)
}

const LABELS: &str = "query Labels($filter: IssueLabelFilter!) { issueLabels(first: 250, filter: $filter) { nodes { id name team { id } } } }";

#[track_caller]
pub fn labels(agent: &Agent, token: &str, filter: Value) -> Result<Vec<Label>, Error> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Read {
        issue_labels: Nodes<Label>,
    }
    Ok(read_with::<Read>(agent, token, LABELS, json!({ "filter": filter }))?.issue_labels.nodes)
}

const PROJECTS: &str = "query Projects($id: String!, $name: String!) { team(id: $id) { projects(first: 50, filter: { name: { eqIgnoreCase: $name } }) { nodes { id name completedAt canceledAt } } } }";

// The team's projects named `name`, ignoring case.
#[track_caller]
pub fn team_projects(agent: &Agent, token: &str, team_id: &str, name: &str) -> Result<Vec<TeamProject>, Error> {
    #[derive(Deserialize)]
    struct Team {
        projects: Nodes<TeamProject>,
    }
    #[derive(Deserialize)]
    struct Read {
        team: Team,
    }
    Ok(read_with::<Read>(agent, token, PROJECTS, json!({ "id": team_id, "name": name }))?.team.projects.nodes)
}

#[derive(Debug, Deserialize)]
pub struct Pipeline {
    pub id: String,
    pub name: String,
}

// The workspace's release pipelines.
#[track_caller]
pub fn release_pipelines(agent: &Agent) -> Result<Vec<Pipeline>, Error> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Read {
        release_pipelines: Nodes<Pipeline>,
    }
    Ok(read_as::<Read>(agent, "query ReleasePipelines { releasePipelines(first: 100) { nodes { id name } } }", json!({}))?.release_pipelines.nodes)
}

// What a release sync starts from: the commit of the pipeline's newest release, None when it
// has none, or none with a full SHA; and the workspace's team keys, upper-cased.
#[track_caller]
#[cfg_attr(not(feature = "develop"), allow(dead_code))]
pub fn release_base(agent: &Agent, pipeline: &str) -> Result<(Option<Sha>, Vec<String>), Error> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Release {
        commit_sha: Option<String>,
    }
    #[derive(Deserialize)]
    struct Releases {
        releases: Nodes<Release>,
    }
    #[derive(Deserialize)]
    struct Keyed {
        key: String,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Read {
        release_pipeline: Releases,
        teams: Nodes<Keyed>,
    }
    let query = "query ReleaseBase($id: String!) { releasePipeline(id: $id) { releases(first: 1, orderBy: createdAt) { nodes { commitSha } } } teams(first: 250) { nodes { key } } }";
    let read: Read = read_as(agent, query, json!({ "id": pipeline }))?;
    let last = read.release_pipeline.releases.nodes.into_iter().next().and_then(|release| release.commit_sha).and_then(|sha| Sha::parse(&sha.to_ascii_lowercase()));
    Ok((last, read.teams.nodes.into_iter().map(|team| team.key.to_ascii_uppercase()).collect()))
}

#[cfg(test)]
mod tests;
