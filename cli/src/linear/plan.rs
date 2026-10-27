use serde_json::{Value, json};

use crate::args::js_trim;
use crate::error::Error;

// A session's plan: the checklist Linear shows on the session. Linear takes the whole
// plan on every update, never one step.
const PLAN_STATUSES: [&str; 4] = ["pending", "inProgress", "completed", "canceled"];

#[derive(Clone, Debug, PartialEq)]
pub struct PlanStep {
    pub content: String,
    pub status: &'static str,
    // What ends the step, for the hook to move the plan on (hook::whereabouts). Never sent:
    // Linear's plan has no such field.
    pub until: Option<&'static str>,
}

// A plan's steps from its JSON, as `tolkster plan` takes them, or the usage error that says
// what is wrong. The hook reads a worker's plan with it too (hook::whereabouts).
pub fn steps(value: &Value) -> Result<Vec<PlanStep>, Error> {
    let Some(items) = value.as_array() else {
        return Err(Error::Usage("the plan must be a JSON array of steps".into()));
    };
    if items.is_empty() {
        return Err(Error::Usage("the plan has no steps".into()));
    }
    let mut steps = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let n = i + 1;
        let Some(fields) = item.as_object() else {
            return Err(Error::Usage(format!("step {n} must be an object with content and status")));
        };
        if let Some(other) = fields.keys().find(|k| !matches!(k.as_str(), "content" | "status" | "until")) {
            return Err(Error::Usage(format!("step {n} has an unknown field '{other}': a step has only content, status and until")));
        }
        let Some(content) = fields.get("content").and_then(Value::as_str).map(js_trim).filter(|c| !c.is_empty()) else {
            return Err(Error::Usage(format!("step {n} needs its content as text")));
        };
        let status = fields.get("status").and_then(Value::as_str).and_then(plan_status);
        let Some(status) = status else {
            return Err(Error::Usage(format!("step {n}'s status must be one of: {}", PLAN_STATUSES.join(", "))));
        };
        let until = match fields.get("until") {
            None => None,
            Some(until) => match until.as_str().and_then(|u| UNTILS.iter().find(|known| **known == u)) {
                Some(until) => Some(*until),
                None => return Err(Error::Usage(format!("step {n}'s until must be one of: {}", UNTILS.join(", ")))),
            },
        };
        steps.push(PlanStep { content: content.into(), status, until });
    }
    if steps.iter().filter(|s| s.status == "inProgress").count() > 1 {
        return Err(Error::Usage("only one step can be inProgress".into()));
    }
    Ok(steps)
}

// The events that end a step, as the hook sees them (hook::whereabouts): a commit, the
// pre-push review's verdict and the PR's opening. Each is a git commit or a `tolkster`
// call, so any repo has them; a repo's gate is its own commands, and the PR opens only once
// it has passed.
const UNTILS: [&str; 3] = ["commit", "review", "pr"];

// A step's status as Linear spells it, ignoring case, spaces, `-` and `_`.
fn plan_status(status: &str) -> Option<&'static str> {
    let key: String = status.chars().filter(|c| !matches!(c, ' ' | '-' | '_')).flat_map(char::to_lowercase).collect();
    match key.as_str() {
        "pending" | "todo" => Some("pending"),
        "inprogress" => Some("inProgress"),
        "completed" | "complete" | "done" => Some("completed"),
        "canceled" | "cancelled" => Some("canceled"),
        _ => None,
    }
}

#[derive(Debug, PartialEq)]
pub struct PlanInput {
    pub agent_session_id: String,
    pub steps: Vec<PlanStep>,
}

impl PlanInput {
    pub fn to_json(&self) -> Value {
        let plan: Vec<Value> = self.steps.iter().map(|s| json!({ "content": s.content, "status": s.status })).collect();
        json!({ "id": self.agent_session_id, "input": { "plan": plan } })
    }
}

#[cfg(test)]
mod tests;
