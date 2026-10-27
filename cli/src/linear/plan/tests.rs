use super::*;

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
