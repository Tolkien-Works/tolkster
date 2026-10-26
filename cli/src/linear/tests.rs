use super::*;
use crate::voice;

#[test]
fn activity_types_match_the_typescript_worker() {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../src/shared/linear.ts")).unwrap();
    let types: Vec<&str> = source.lines().filter_map(|l| l.trim().strip_prefix("| { type: \"")?.split('"').next()).collect();
    assert_eq!(types, ActivityType::ALL.map(ActivityType::as_str), "ActivityType differs between cli/src/linear.rs and src/shared/linear.ts");
    for t in ActivityType::ALL {
        assert_eq!(t.as_str().parse(), Ok(t));
    }
}

#[test]
fn scopes_match_the_typescript_worker() {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../src/shared/linear.ts")).unwrap();
    let declaration = source.lines().find_map(|l| l.trim().strip_prefix("export const SCOPES = ")).expect("SCOPES in src/shared/linear.ts");
    assert_eq!(declaration, format!("\"{SCOPES}\";"), "SCOPES differs between cli/src/linear.rs and src/shared/linear.ts");
}

#[test]
fn webhook_types_match_the_workers() {
    let mut types = WEBHOOK_TYPES.to_vec();
    types.sort_unstable();
    assert_eq!(serde_json::to_value(types).unwrap(), crate::state::tests::contract()["linearWebhookTypes"]);
}

#[test]
fn the_manifest_names_the_workers_callback_and_webhook() {
    let worker = WorkerUrl::parse("https://tolkster.example.workers.dev").unwrap();
    let manifest = manifest("Owl", &worker);
    assert_eq!(manifest["oauth"]["redirect_uris"], json!([worker.oauth_callback()]));
    assert_eq!(manifest["webhook"]["url"], worker.linear_webhook());
    assert_eq!(manifest["webhook"]["resourceTypes"], json!(WEBHOOK_TYPES));
    // The Worker mints its tokens with this grant.
    assert!(manifest["oauth"]["grant_types"].as_array().unwrap().contains(&json!("client_credentials")));
    let url = create_url("Owl", &worker);
    let (_, query) = url.split_once("?manifest=").unwrap();
    assert!(query.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_./:%".contains(&b)), "{query}");
}

#[test]
fn only_linear_saying_so_rejects_the_token() {
    let reply = |status, body: Value| answer(status, &body.to_string(), &Limits::default(), 0);
    let auth = json!({ "errors": [{ "message": "Authentication required", "extensions": { "type": "authentication error" } }] });
    assert!(matches!(reply(400, auth), Err(Error::Unauthorized(_))));
    let auth_code = json!({ "errors": [{ "message": "Authentication required", "extensions": { "code": "AUTHENTICATION_ERROR" } }] });
    assert!(matches!(reply(400, auth_code), Err(Error::Unauthorized(_))));
    assert!(matches!(answer(401, "Unauthorized", &Limits::default(), 0), Err(Error::Unauthorized(_))));
    assert!(matches!(answer(400, "Bad request", &Limits::default(), 0), Err(Error::Failed(m)) if m == "HTTP 400: Bad request"));
    assert!(matches!(reply(400, json!({ "errors": [] })), Err(Error::Failed(m)) if m == "HTTP 400: {\"errors\":[]}"));
    assert!(matches!(answer(403, "Forbidden", &Limits::default(), 0), Err(Error::Failed(_))));
    assert!(matches!(answer(502, "", &Limits::default(), 0), Err(Error::Failed(m)) if m == "HTTP 502: "));
}

#[test]
fn a_spent_budget_is_told_apart_with_its_wait() {
    let limits = Limits { requests_remaining: Some(0), requests_reset: Some(1_000_090_500), ..Limits::default() };
    let spent = json!({ "errors": [{ "message": "Rate limit exceeded", "extensions": { "code": "RATELIMITED" } }] }).to_string();
    let wait = |result: Result<Value, Error>| result.unwrap_err().rate_limited();
    assert_eq!(wait(answer(400, &spent, &limits, 1_000_000_000)), Some(std::time::Duration::from_secs(91)));
    assert_eq!(wait(answer(400, &spent, &Limits::default(), 0)), Some(std::time::Duration::from_secs(60)));
    assert_eq!(wait(answer(429, "slow down", &limits, 1_000_000_000)), Some(std::time::Duration::from_secs(91)));
    let other = json!({ "errors": [{ "message": "nope", "extensions": { "code": "FORBIDDEN" } }] }).to_string();
    assert_eq!(wait(answer(400, &other, &limits, 0)), None);
    // An error of another shape beside it hides neither a spent budget nor a rejected token.
    let mixed = json!({ "errors": [{ "message": 7, "extensions": null }, { "extensions": { "code": "RATELIMITED", "type": [] } }] }).to_string();
    assert_eq!(wait(answer(400, &mixed, &limits, 1_000_000_000)), Some(std::time::Duration::from_secs(91)));
    let mixed = json!({ "errors": [{ "extensions": { "code": 1 } }, { "extensions": { "type": "authentication error" } }] }).to_string();
    assert!(matches!(answer(400, &mixed, &limits, 0), Err(Error::Unauthorized(_))));
    let mixed = json!({ "errors": [null, "x", { "extensions": { "code": "RATELIMITED" } }] }).to_string();
    assert_eq!(wait(answer(400, &mixed, &limits, 1_000_000_000)), Some(std::time::Duration::from_secs(91)));
    // An error of no shape at all still fails an answer that has data.
    let partial = json!({ "data": { "issue": { "id": "i" } }, "errors": [null] }).to_string();
    assert!(answer(200, &partial, &limits, 0).is_err());
}

#[test]
fn a_failure_is_told_by_linear_first_error() {
    let reply = |status, body: Value| answer(status, &body.to_string(), &Limits::default(), 0);
    let invalid = json!({ "errors": [
        { "message": "Entity not found: Issue", "extensions": { "type": "invalid input", "userPresentableMessage": "Could not find issue" } },
        { "message": "second" },
    ] });
    assert!(matches!(reply(400, invalid), Err(Error::Failed(m)) if m == "Could not find issue"));
    let partial = json!({ "data": { "issue": null }, "errors": [{ "message": "Entity not found: Issue" }] });
    assert!(matches!(reply(200, partial), Err(Error::Failed(m)) if m == "Entity not found: Issue"));
    assert!(matches!(reply(200, json!({ "data": null })), Err(Error::Failed(m)) if m == "HTTP 200: {\"data\":null}"));
}

#[test]
fn data_is_returned_when_there_are_no_errors() {
    let reply = |status, body: Value| answer(status, &body.to_string(), &Limits::default(), 0);
    assert_eq!(reply(200, json!({ "data": { "viewer": { "id": "u1" } } })).unwrap(), json!({ "viewer": { "id": "u1" } }));
    assert_eq!(reply(200, json!({ "data": { "a": 1 }, "errors": [] })).unwrap(), json!({ "a": 1 }));
}

#[test]
fn a_plan_is_sent_whole_as_agent_session_update_takes_it() {
    let input = PlanInput {
        agent_session_id: "s1".into(),
        steps: vec![PlanStep { content: "Read and plan".into(), status: "completed", until: None }, PlanStep { content: "Write the code".into(), status: "inProgress", until: Some("commit") }],
    };
    assert_eq!(
        input.to_json(),
        json!({ "id": "s1", "input": { "plan": [
            { "content": "Read and plan", "status": "completed" },
            { "content": "Write the code", "status": "inProgress" },
        ] } })
    );
}

// Linear offers `select` options as buttons on an elicitation; with none, no signal is sent.
#[test]
fn an_elicitations_options_are_sent_as_a_select_signal() {
    let plain = ActivityInput::text("s1", ActivityType::Elicitation, "Reply 'go'.", false);
    assert_eq!(plain.to_json(), json!({ "agentSessionId": "s1", "content": { "type": "elicitation", "body": "Reply 'go'." } }));
    let input = ActivityInput { options: vec![&voice::GO, &voice::SKIP], ..plain };
    assert_eq!(
        input.to_json(),
        json!({
            "agentSessionId": "s1",
            "content": { "type": "elicitation", "body": "Reply 'go'." },
            "signal": "select",
            "signalMetadata": { "options": [{ "label": "Go", "value": "go" }, { "label": "Skip it", "value": "skip it" }] },
        })
    );
}

#[test]
fn a_slow_call_is_named_by_its_operation() {
    assert_eq!(operation("query Recent($after: String) { viewer { id } }"), "Recent");
    assert_eq!(operation("\n  mutation Reply($input: CommentCreateInput!) { x }"), "Reply");
    assert_eq!(operation("query Viewer { viewer { id name } }"), "Viewer");
    // An anonymous query has no name to give, and nothing else of it is logged.
    assert_eq!(operation("{ viewer { id } }"), "unnamed");
    assert_eq!(operation("query { viewer { id } }"), "unnamed");
}

#[test]
fn activities_sent_together_are_created_in_their_order() {
    let thought = ActivityInput::text("s1", ActivityType::Thought, "first", false);
    let action = ActivityInput::action("s1", "Ran", "cargo test");
    let (names, query, variables) = activities_request(&[&thought, &action]);
    assert_eq!(names, ["a0", "a1"]);
    assert_eq!(query, "mutation AgentActivitiesCreate($a0: AgentActivityCreateInput!, $a1: AgentActivityCreateInput!) { a0: agentActivityCreate(input: $a0) { success } a1: agentActivityCreate(input: $a1) { success } }");
    assert_eq!(variables["a0"], thought.to_json());
    assert_eq!(variables["a1"], action.to_json());
}

// A subcommand reads Linear through `sessions` and writes through a named mutation here,
// so each call has one home (AGENTS.md): none holds GraphQL or calls Linear itself.
#[test]
fn no_subcommand_holds_graphql() {
    let mut dirs = vec![std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src/commands"))];
    let mut read = 0;
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let source = std::fs::read_to_string(&path).unwrap();
                for call in ["\"query ", "\"mutation ", "graphql(", "mutate("] {
                    assert!(!source.contains(call), "{} has {call}", path.display());
                }
                // Each path into `linear`, a `use` over several lines included, up to its `;`.
                let paths = source.split("linear::").skip(1).map(|rest| rest.split(';').next().unwrap_or(rest));
                for word in paths.flat_map(|p| p.split(|c: char| !c.is_alphanumeric() && c != '_')) {
                    assert!(!["read", "graphql", "mutate"].contains(&word), "{} reaches linear::{word}", path.display());
                }
                read += 1;
            }
        }
    }
    assert!(read > 40, "{read} files");
}


#[test]
fn a_mutations_payload_decodes_into_what_it_returns() {
    let reply: Reply = decode(json!({ "id": "c1", "createdAt": "2026-10-23T00:00:00.000Z" }), "Linear's answer to Comment").unwrap();
    assert_eq!(reply, Reply { id: "c1".into(), at: "2026-10-23T00:00:00.000Z".into() });
    let created: CreatedIssue = decode(json!({ "id": "i1", "identifier": "TW-9", "url": "https://linear.app/w/issue/TW-9" }), "Linear's answer to Create").unwrap();
    assert_eq!((created.id.as_str(), created.identifier.as_str()), ("i1", "TW-9"));
    let success: Success = decode(json!({ "success": false, "issue": null }), "Linear's answer to Create").unwrap();
    assert!(!success.success);
}

#[test]
fn a_release_pipeline_is_created_continuous_on_its_team() {
    assert_eq!(release::pipeline_input("Tolkster", "t1"), json!({ "name": "Tolkster", "type": "continuous", "teamIds": ["t1"] }));
}
