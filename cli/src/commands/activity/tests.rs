use super::*;

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

fn no_file(source: &str) -> Result<String, Error> {
    panic!("unexpected read of {source}")
}

fn text(kind: &str, body: &str) -> ActivityContent {
    ActivityContent::Text { kind: kind.parse().unwrap(), body: body.into() }
}

#[test]
fn a_thought_takes_its_body_from_body() {
    let input = parse_activity(&argv(&["s1", "thought", "--body", "Reading the issue"]), no_file).unwrap();
    assert_eq!(
        input,
        ActivityInput { agent_session_id: "s1".into(), content: text("thought", "Reading the issue"), ephemeral: false, id: None, options: vec![] }
    );
}

#[test]
fn a_response_can_take_its_body_from_a_file_or_stdin() {
    let input = parse_activity(&argv(&["s1", "response", "--body-file", "-"]), |source| Ok(format!("read from {source}"))).unwrap();
    assert_eq!(input.content, text("response", "read from -"));
}

#[test]
fn an_action_carries_verb_parameter_and_optional_result() {
    let input = parse_activity(&argv(&["s1", "action", "--action", "Opened PR", "--parameter", "#3", "--result", "CI green"]), no_file).unwrap();
    assert_eq!(
        input.content,
        ActivityContent::Action { action: "Opened PR".into(), parameter: "#3".into(), result: Some("CI green".into()) }
    );
    let input = parse_activity(&argv(&["s1", "action", "--action", "Running", "--parameter", "npm test"]), no_file).unwrap();
    assert_eq!(input.content, ActivityContent::Action { action: "Running".into(), parameter: "npm test".into(), result: None });
}

#[test]
fn an_unknown_type_lists_the_types() {
    let Err(Error::Usage(message)) = parse_activity(&argv(&["s1", "thougth", "--body", "x"]), no_file) else { panic!("accepted") };
    assert_eq!(message, "type must be one of: thought, elicitation, response, error, action");
}

#[test]
fn ephemeral_is_allowed_on_thoughts_and_actions_only() {
    assert!(parse_activity(&argv(&["s1", "thought", "--body", "x", "--ephemeral"]), no_file).unwrap().ephemeral);
    assert!(matches!(parse_activity(&argv(&["s1", "response", "--body", "x", "--ephemeral"]), no_file), Err(Error::Usage(_))));
}

#[test]
fn a_state_posts_one_of_its_phrases_as_an_ephemeral_thought() {
    let input = parse_activity(&argv(&["s1", "thought", "--state", "reading-issue"]), no_file).unwrap();
    let ActivityContent::Text { kind, body } = &input.content else { panic!("{input:?}") };
    assert_eq!(*kind, ActivityType::Thought);
    assert!(voice::phrases("reading-issue").unwrap().contains(body), "{body}");
    assert!(input.ephemeral);
}

#[test]
fn a_state_is_refused_where_it_doesnt_fit() {
    let cases: &[&[&str]] = &[
        &["s1", "thought", "--state", "no-such-state"],
        &["s1", "response", "--state", "reading-issue"],
        &["s1", "action", "--state", "reading-issue"],
        &["s1", "thought", "--state", "reading-issue", "--body", "x"],
        &["s1", "thought", "extra", "--state", "reading-issue"],
        &["s1", "thought", "--state", "reading-issue", "--while"],
    ];
    for case in cases {
        assert!(matches!(parse_activity(&argv(case), |_| Ok("x".into())), Err(Error::Usage(_))), "{}", case.join(" "));
    }
}

#[test]
fn bad_input_is_a_usage_error_not_a_request_linear_would_reject() {
    let cases: &[&[&str]] = &[
        &[],
        &["s1"],
        &["s1", "prompt", "--body", "x"],
        &["s1", "thought"],
        &["s1", "thought", "--body", "   "],
        &["s1", "thought", "--body", "x", "--body-file", "f"],
        &["s1", "action", "--action", "Running"],
    ];
    for case in cases {
        let result = parse_activity(&argv(case), |_| Ok("x".into()));
        assert!(matches!(result, Err(Error::Usage(_))), "{}", case.join(" "));
    }
}

#[test]
fn an_elicitation_offers_fixed_options_by_name() {
    let input = parse_activity(&argv(&["s1", "elicitation", "--body", "x", "--option", "go", "--option", "skip"]), no_file).unwrap();
    assert_eq!(input.options, vec![&voice::GO, &voice::SKIP]);
    let usage = |args: &[&str]| match parse_activity(&argv(args), no_file) {
        Err(Error::Usage(message)) => message,
        other => panic!("{other:?}"),
    };
    assert_eq!(usage(&["s1", "elicitation", "--body", "x", "--option", "Go"]), "option must be one of: go, approve, skip, retry");
    assert_eq!(usage(&["s1", "elicitation", "--body", "x", "--option", "go", "--option", "approve"]), "option approve sends the same reply as another one given");
    for args in [&["s1", "response", "--body", "x", "--option", "go"][..], &["s1", "action", "--action", "a", "--parameter", "p", "--option", "go"], &["s1", "thought", "--state", "reading-issue", "--option", "go"]] {
        assert_eq!(usage(args), "--option applies only to elicitation");
    }
}
