use super::*;

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

#[test]
fn malformed_options_are_usage_errors() {
    let options = [("body", Kind::String), ("ephemeral", Kind::Boolean)];
    let message = |args: &[&str], positionals| match parse(&argv(args), &options, positionals) {
        Err(Error::Usage(message)) => message,
        other => panic!("{args:?} gave {other:?}"),
    };
    assert_eq!(message(&["--nope"], false), "unknown option --nope");
    assert_eq!(message(&["-abc"], false), "unknown option -a");
    assert_eq!(message(&["--nope=3"], true), "unknown option --nope; for an argument that starts with '-', put it after --, as in -- --nope");
    assert_eq!(message(&["--body"], false), "--body needs a value");
    assert_eq!(message(&["--body", "--"], false), "--body needs a value; for one that starts with '-', write --body=<value>");
    assert_eq!(message(&["--ephemeral=1"], false), "--ephemeral takes no value");
    assert_eq!(message(&["pos"], false), "unexpected argument pos: this command takes none");
}

#[test]
fn well_formed_options_parse() {
    let options = [("body", Kind::String), ("ephemeral", Kind::Boolean)];
    let parsed = parse(&argv(&["a", "--body=-x", "-", "--body", "-", "--ephemeral", "--", "--body"]), &options, true).unwrap();
    assert_eq!(parsed.string("body"), Some("-"));
    assert!(parsed.flag("ephemeral"));
    assert_eq!(parsed.positionals, argv(&["a", "-", "--body"]));
}

#[test]
fn an_empty_value_or_first_positional_counts_as_missing() {
    let options = [("body", Kind::String)];
    let parsed = parse(&argv(&["", "b", "--body="]), &options, true).unwrap();
    assert_eq!((parsed.first(), parsed.non_empty("body"), parsed.non_empty("nope")), (None, None, None));
    let parsed = parse(&argv(&["a", "--body", "x"]), &options, true).unwrap();
    assert_eq!((parsed.first(), parsed.non_empty("body")), (Some("a"), Some("x")));
}

#[test]
fn a_list_option_keeps_every_value() {
    let options = [("reviewer", Kind::List), ("owner", Kind::String)];
    let parsed = parse(&argv(&["--reviewer", "a-reviewer", "--owner", "a", "--reviewer=copilot-pull-request-reviewer", "--owner", "b"]), &options, false).unwrap();
    assert_eq!(parsed.list("reviewer"), argv(&["a-reviewer", "copilot-pull-request-reviewer"]));
    assert_eq!(parsed.string("owner"), Some("b"));
    assert!(parse(&argv(&[]), &options, false).unwrap().list("reviewer").is_empty());
}

fn read_x(source: &str) -> Result<String, Error> {
    Ok(format!("read from {source}"))
}

#[test]
fn a_blank_value_counts_as_missing() {
    let parsed = parse(&argv(&["--body", "  	", "--title", " x "]), &[("body", Kind::String), ("title", Kind::String)], false).unwrap();
    assert_eq!((parsed.non_empty("body"), parsed.non_empty("title")), (None, Some(" x ")));
}

#[test]
fn a_body_is_one_of_body_and_body_file_and_not_blank() {
    let options = [("body", Kind::String), ("body-file", Kind::String)];
    let body = |args: &[&str]| body(&parse(&argv(args), &options, false).unwrap(), "comment", read_x);
    assert_eq!(body(&["--body", "hi"]).unwrap(), "hi");
    assert_eq!(body(&["--body-file", "-"]).unwrap(), "read from -");
    for case in [&[][..], &["--body", " "], &["--body-file", ""], &["--body", "x", "--body-file", "f"]] {
        let outcome = body(case);
        assert!(matches!(outcome, Err(Error::Usage(_))), "{case:?} gave {outcome:?}");
    }
}

#[test]
fn a_blank_body_file_is_a_usage_error_and_a_failed_read_is_not() {
    assert!(matches!(file_body("f", "reply", |_| Ok(" 
".into())), Err(Error::Usage(_))));
    assert_eq!(file_body("f", "reply", |_| Ok("x".into())).unwrap(), "x");
    assert!(matches!(file_body("f", "reply", |_| Err(Error::Failed("no such file".into()))), Err(Error::Failed(_))));
}
