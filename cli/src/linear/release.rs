// Linear's release API: a pipeline, and a release on it per commit `listen` rebuilt itself
// at (docs/realtime.md, "Keeping up with main"). Linear has it in alpha.

use serde::Deserialize;
use serde_json::json;
use ureq::Agent;

use super::{Ref, mutate};
use crate::error::Error;
use crate::plain::{IssueKey, Sha};

#[cfg_attr(not(feature = "develop"), allow(dead_code))]
const SYNC: &str = "mutation ReleaseSync($input: ReleaseSyncInput!) { releaseSync(input: $input) { success } }";

// The release of `commit` on `pipeline`, linking `issues`, each at that commit, its version
// the short SHA.
#[track_caller]
#[cfg_attr(not(feature = "develop"), allow(dead_code))]
pub fn sync_release(agent: &Agent, access_token: &str, pipeline: &str, commit: &Sha, issues: &[IssueKey]) -> Result<(), Error> {
    let references: Vec<_> = issues.iter().map(|key| json!({ "identifier": key.as_str(), "commitSha": commit.as_str() })).collect();
    let input = json!({ "pipelineId": pipeline, "commitSha": commit.as_str(), "version": crate::plain::short_sha(commit.as_str()), "issueReferences": references });
    mutate::<serde_json::Value>(agent, access_token, "releaseSync", SYNC, json!({ "input": input })).map(|_| ())
}

const CREATE_PIPELINE: &str = "mutation ReleasePipelineCreate($input: ReleasePipelineCreateInput!) { releasePipelineCreate(input: $input) { success releasePipeline { id } } }";

// On a team, as Linear lets only a workspace admin make one on none, which the app isn't.
pub(super) fn pipeline_input(name: &str, team_id: &str) -> serde_json::Value {
    json!({ "name": name, "type": "continuous", "teamIds": [team_id] })
}

// A continuous pipeline named `name` on the team `team_id`: its ID.
#[track_caller]
pub fn create_release_pipeline(agent: &Agent, access_token: &str, name: &str, team_id: &str) -> Result<String, Error> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Created {
        release_pipeline: Ref,
    }
    Ok(mutate::<Created>(agent, access_token, "releasePipelineCreate", CREATE_PIPELINE, json!({ "input": pipeline_input(name, team_id) }))?.release_pipeline.id)
}
