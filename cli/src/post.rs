// The one way Tolkster posts: `send`, and `post_once` for a post that a command run again
// must not repeat, by an activity ID made from what it is for. A reply in a note's thread
// goes through `reply_once`, by a comment ID made the same way, and any other comment
// under a claim through `comment_once`.

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use ureq::Agent;

use crate::error::Error;
use crate::linear::{ActivityInput, Nodes, Ref, Reply, create_activities, create_activity, create_comment, read_as, with_token};
use crate::state::{Store, derived_uuid, holding};
use crate::trace::{self, Work};

pub fn send(agent: &Agent, input: &ActivityInput) -> Result<String, Error> {
    with_token(agent, |token| create_activity(agent, token, input))
}

// Posts each of `inputs`, in order, one request for them all.
pub fn send_all(agent: &Agent, inputs: &[&ActivityInput]) -> Result<(), Error> {
    match inputs {
        [] => Ok(()),
        [one] => send(agent, one).map(drop),
        _ => with_token(agent, |token| create_activities(agent, token, inputs)),
    }
}

// Posts `input` with an activity ID made from `key`, so Linear refuses a second post of it,
// and a refused post whose activity is there was posted by a run before this one.
pub fn post_once(agent: &Agent, key: &str, input: ActivityInput) -> Result<(), Error> {
    let id = derived_uuid(key);
    match send(agent, &ActivityInput { id: Some(id.clone()), ..input }) {
        Ok(_) => Ok(()),
        Err(error) if posted(agent, &id) => {
            trace::event(Work::PostedAlready { key: key.into(), error });
            Ok(())
        }
        Err(error) => Err(error),
    }
}

// Which of the posts `post_once` would make from `keys` Linear holds already, by one read,
// so a run of them tries only the rest. None when the read fails: then each is tried.
pub fn posted_of(agent: &Agent, keys: &[String]) -> Option<Vec<bool>> {
    let ids: Vec<String> = keys.iter().map(|key| derived_uuid(key)).collect();
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Answer {
        agent_activities: Nodes<Ref>,
    }
    let query = "query Posted($ids: [ID!]!) { agentActivities(first: 250, filter: { id: { in: $ids } }) { nodes { id } } }";
    let answer: Answer = read_as(agent, query, json!({ "ids": ids })).ok()?;
    Some(ids.iter().map(|id| answer.agent_activities.nodes.iter().any(|held| held.id == *id)).collect())
}

fn posted(agent: &Agent, id: &str) -> bool {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Answer {
        agent_activity: Option<Ref>,
    }
    let query = "query Posted($id: String!) { agentActivity(id: $id) { id } }";
    read_as::<Answer>(agent, query, json!({ "id": id })).is_ok_and(|answer| answer.agent_activity.is_some_and(|a| a.id == id))
}

// Posts `body` once as Tolkster's comment on issue `issue_id`, in the thread of its comment
// `parent` when there is one, under `claims`, held for good: a triage verdict.
// None when one was already taken, and nothing was posted. Should the post fail, the claims
// are given back.
pub fn comment_once(agent: &Agent, store: &dyn Store, issue_id: &str, parent: Option<&str>, body: &str, claims: &[String], token: &str) -> Result<Option<Reply>, Error> {
    let claims: Vec<(String, Option<Duration>)> = claims.iter().map(|claim| (claim.clone(), None)).collect();
    holding(store, &claims, token, || with_token(agent, |access| create_comment(agent, access, issue_id, parent, body, None)))
}

// Posts `body` once as Tolkster's reply in the thread of its comment `root` on issue
// `issue_id`, with a comment ID made from `key`, so Linear refuses a second post of it, and a
// refused post whose comment is there was posted before. None when it was, and nothing was
// posted now.
pub fn reply_once(agent: &Agent, issue_id: &str, root: &str, body: &str, key: &str) -> Result<Option<Reply>, Error> {
    let id = derived_uuid(key);
    match with_token(agent, |access| create_comment(agent, access, issue_id, Some(root), body, Some(&id))) {
        Ok(reply) => Ok(Some(reply)),
        Err(error) if commented(agent, &id) => {
            trace::event(Work::RepliedAlready { key: key.into(), error });
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn commented(agent: &Agent, id: &str) -> bool {
    #[derive(Deserialize)]
    struct Answer {
        comment: Option<Ref>,
    }
    let query = "query Commented($id: String!) { comment(id: $id) { id } }";
    read_as::<Answer>(agent, query, json!({ "id": id })).is_ok_and(|answer| answer.comment.is_some_and(|c| c.id == id))
}

// The key of the one answer in the thread of `root` to the request at `request`: the turn's
// answer, or else its `stalled` or `noRepo` word, whichever is posted first.
pub fn answer_key(root: &str, request: &str) -> String {
    format!("thread {root} answer {request}")
}
