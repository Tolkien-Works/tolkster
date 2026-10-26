use super::*;
use crate::linear::PlanStep;

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

fn no_file(source: &str) -> Result<String, Error> {
    panic!("unexpected read of {source}")
}

fn plan_from(json: &str) -> Result<PlanInput, Error> {
    let json = json.to_string();
    parse_plan(&argv(&["s1", "--file", "-"]), move |source| {
        assert_eq!(source, "-");
        Ok(json)
    })
}

fn step(content: &str, status: &'static str) -> PlanStep {
    PlanStep { content: content.into(), status, until: None }
}

#[test]
fn a_plan_is_read_from_a_json_array_of_steps() {
    let input = plan_from(r#"[{"content": "Read and plan", "status": "completed"}, {"content": " Write the code ", "status": "inProgress"}, {"content": "PR open, checks green", "status": "pending"}]"#).unwrap();
    assert_eq!(
        input,
        PlanInput { agent_session_id: "s1".into(), steps: vec![step("Read and plan", "completed"), step("Write the code", "inProgress"), step("PR open, checks green", "pending")] }
    );
}

#[test]
fn a_loosely_spelt_status_is_sent_as_linear_spells_it() {
    for (given, sent) in [
        ("pending", "pending"),
        ("todo", "pending"),
        ("inProgress", "inProgress"),
        ("in_progress", "inProgress"),
        ("In Progress", "inProgress"),
        ("in-progress", "inProgress"),
        ("completed", "completed"),
        ("done", "completed"),
        ("Complete", "completed"),
        ("canceled", "canceled"),
        ("cancelled", "canceled"),
    ] {
        let input = plan_from(&format!(r#"[{{"content": "x", "status": "{given}"}}]"#)).unwrap();
        assert_eq!(input.steps, vec![step("x", sent)], "{given}");
    }
}

#[test]
fn a_step_says_what_ends_it_by_a_known_event_only() {
    let input = plan_from(r#"[{"content": "a", "status": "inProgress", "until": "commit"}, {"content": "b", "status": "pending", "until": "review"}, {"content": "c", "status": "pending", "until": "pr"}]"#).unwrap();
    assert_eq!(input.steps.iter().map(|s| s.until).collect::<Vec<_>>(), [Some("commit"), Some("review"), Some("pr")]);
    for until in [r#""gate""#, r#""Commit""#, r#""""#, "3", "null"] {
        let case = format!(r#"[{{"content": "a", "status": "pending", "until": {until}}}]"#);
        assert!(matches!(plan_from(&case), Err(Error::Usage(_))), "{case}");
    }
}

#[test]
fn a_plan_linear_would_reject_or_misread_is_a_usage_error() {
    let cases = [
        "",
        "not json",
        r#"{"content": "x", "status": "pending"}"#,
        "[]",
        r#"["Read and plan"]"#,
        r#"[{"content": "x"}]"#,
        r#"[{"status": "pending"}]"#,
        r#"[{"content": "   ", "status": "pending"}]"#,
        r#"[{"content": 3, "status": "pending"}]"#,
        r#"[{"content": "x", "status": "started"}]"#,
        r#"[{"content": "x", "status": "pending", "text": "y"}]"#,
        r#"[{"content": "a", "status": "inProgress"}, {"content": "b", "status": "in_progress"}]"#,
    ];
    for case in cases {
        assert!(matches!(plan_from(case), Err(Error::Usage(_))), "{case}");
    }
}

#[test]
fn a_plan_needs_a_session_and_a_file() {
    for case in [&["s1"][..], &["--file", "p.json"], &["s1", "--file", ""], &["s1", "s2", "--file", "p.json"]] {
        assert!(matches!(parse_plan(&argv(case), no_file), Err(Error::Usage(_))), "{}", case.join(" "));
    }
}
