use serde_json::{Value, json};

// The activity types Tolkster posts, as `src/shared/linear.ts` has them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ActivityType {
    Thought,
    Elicitation,
    Response,
    Error,
    Action,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Prompt,
    // A thought or an action: work in progress, not an answer.
    Progress,
    Answer,
}

impl ActivityType {
    // Ordered as `tolkster activity`'s usage lists them.
    pub const ALL: [ActivityType; 5] = [ActivityType::Thought, ActivityType::Elicitation, ActivityType::Response, ActivityType::Error, ActivityType::Action];

    pub fn as_str(self) -> &'static str {
        match self {
            ActivityType::Thought => "thought",
            ActivityType::Elicitation => "elicitation",
            ActivityType::Response => "response",
            ActivityType::Error => "error",
            ActivityType::Action => "action",
        }
    }

    pub fn answers(self) -> bool {
        self.kind() == Kind::Answer
    }

    // Linear accepts ephemeral only on thoughts and actions.
    pub fn may_be_ephemeral(self) -> bool {
        matches!(self, ActivityType::Thought | ActivityType::Action)
    }

    pub fn kind(self) -> Kind {
        match self {
            ActivityType::Thought | ActivityType::Action => Kind::Progress,
            ActivityType::Elicitation | ActivityType::Response | ActivityType::Error => Kind::Answer,
        }
    }

    // None for a prompt, or for a type Linear adds later.
    pub fn from_typename(typename: &str) -> Option<ActivityType> {
        match typename {
            "AgentActivityThoughtContent" => Some(ActivityType::Thought),
            "AgentActivityElicitationContent" => Some(ActivityType::Elicitation),
            "AgentActivityResponseContent" => Some(ActivityType::Response),
            "AgentActivityErrorContent" => Some(ActivityType::Error),
            "AgentActivityActionContent" => Some(ActivityType::Action),
            _ => None,
        }
    }
}

impl std::str::FromStr for ActivityType {
    type Err = ();

    fn from_str(s: &str) -> Result<ActivityType, ()> {
        ActivityType::ALL.into_iter().find(|t| t.as_str() == s).ok_or(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ActivityContent {
    // thought, elicitation, response or error
    Text { kind: ActivityType, body: String },
    Action { action: String, parameter: String, result: Option<String> },
}

// An elicitation's select options, which Linear offers as buttons. A pick comes back as the
// owner's prompt, holding its label or its value, so the two read as the same reply, the one
// the brief waits for; typing it does the same. A worker names an option, never writes one.
#[derive(Debug, PartialEq)]
pub struct Choice {
    pub name: &'static str,
    pub label: &'static str,
    pub value: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActivityInput {
    pub agent_session_id: String,
    pub content: ActivityContent,
    pub ephemeral: bool,
    // Chosen beforehand so a writer can find the activity again (state::Posting); None
    // lets Linear choose.
    pub id: Option<String>,
    // An elicitation's select options; none sends no signal.
    pub options: Vec<&'static Choice>,
}

impl ActivityInput {
    pub fn text(session_id: &str, kind: ActivityType, body: &str, ephemeral: bool) -> ActivityInput {
        ActivityInput { agent_session_id: session_id.into(), content: ActivityContent::Text { kind, body: body.into() }, ephemeral, id: None, options: vec![] }
    }

    // Durable, with no result.
    pub fn action(session_id: &str, action: &str, parameter: &str) -> ActivityInput {
        ActivityInput {
            agent_session_id: session_id.into(),
            content: ActivityContent::Action { action: action.into(), parameter: parameter.into(), result: None },
            ephemeral: false,
            id: None,
            options: vec![],
        }
    }

    pub(super) fn to_json(&self) -> Value {
        let content = match &self.content {
            ActivityContent::Text { kind, body } => json!({ "type": kind.as_str(), "body": body }),
            ActivityContent::Action { action, parameter, result } => {
                let mut content = json!({ "type": ActivityType::Action.as_str(), "action": action, "parameter": parameter });
                if let Some(result) = result {
                    content["result"] = json!(result);
                }
                content
            }
        };
        let mut input = json!({ "agentSessionId": self.agent_session_id, "content": content });
        if self.ephemeral {
            input["ephemeral"] = json!(true);
        }
        if let Some(id) = &self.id {
            input["id"] = json!(id);
        }
        if !self.options.is_empty() {
            let options: Vec<Value> = self.options.iter().map(|c| json!({ "label": c.label, "value": c.value })).collect();
            input["signal"] = json!("select");
            input["signalMetadata"] = json!({ "options": options });
        }
        input
    }
}

#[cfg(test)]
mod tests;
