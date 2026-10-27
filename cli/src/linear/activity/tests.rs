use super::*;
use crate::voice;

#[test]
fn activity_types_match_the_typescript_worker() {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../src/shared/linear.ts")).unwrap();
    let types: Vec<&str> = source.lines().filter_map(|l| l.trim().strip_prefix("| { type: \"")?.split('"').next()).collect();
    assert_eq!(types, ActivityType::ALL.map(ActivityType::as_str), "ActivityType differs between cli/src/linear/activity.rs and src/shared/linear.ts");
    for t in ActivityType::ALL {
        assert_eq!(t.as_str().parse(), Ok(t));
    }
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
