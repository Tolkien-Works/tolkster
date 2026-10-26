use super::*;

fn parsed(args: &str) -> Parsed {
    let argv: Vec<String> = args.split(' ').filter(|a| !a.is_empty()).map(String::from).collect();
    let options = [("verdict", Kind::String), ("estimate", Kind::String), ("priority", Kind::String), ("blocked-by", Kind::List), ("related", Kind::List), ("by", Kind::String), ("of", Kind::String)];
    parse(&argv, &options, true).unwrap()
}

#[test]
fn each_verdict_takes_its_own_options_and_nothing_else() {
    let accept = parse_verdict(&parsed("--verdict accept --estimate 3 --priority high --blocked-by TW-2 --related tw-3 --related TW-4 --related TW-3")).unwrap();
    assert_eq!(accept, Verdict::Accept { estimate: Some(3), priority: 2, relations: vec![(Relation::BlockedBy, "TW-2".into()), (Relation::Related, "TW-3".into()), (Relation::Related, "TW-4".into())] });
    assert_eq!(parse_verdict(&parsed("--verdict fixed --by ABC1234")).unwrap(), Verdict::Fixed { by: "abc1234".into() });
    assert_eq!(parse_verdict(&parsed("--verdict duplicate --of tw-9")).unwrap(), Verdict::Duplicate { of: "TW-9".into() });
    assert_eq!(parse_verdict(&parsed("--verdict decline")).unwrap(), Verdict::Decline);
    // The estimate is checked against the issue's team's scale once the issue is read
    // (`accepted_estimate`), so none, or zero, parses.
    assert_eq!(parse_verdict(&parsed("--verdict accept --priority low")).unwrap(), Verdict::Accept { estimate: None, priority: 4, relations: vec![] });
    assert_eq!(parse_verdict(&parsed("--verdict accept --estimate 0 --priority low")).unwrap(), Verdict::Accept { estimate: Some(0), priority: 4, relations: vec![] });
    for bad in [
        "",
        "--verdict maybe",
        "--verdict accept",
        "--verdict accept --estimate 3",
        "--verdict accept --estimate none --priority high",
        "--verdict accept --estimate 1.5 --priority high",
        "--verdict accept --estimate 3 --priority none",
        "--verdict accept --estimate 3 --priority high --by abc1234",
        "--verdict fixed",
        "--verdict fixed --by abc1234 --of TW-9",
        "--verdict fixed --by abc1234 --related TW-9",
        "--verdict duplicate",
        "--verdict duplicate --of TW-9 --estimate 3",
        "--verdict decline --by abc1234",
        "--verdict decline --blocked-by TW-2",
    ] {
        assert!(matches!(parse_verdict(&parsed(bad)), Err(Error::Usage(_))), "{bad}");
    }
}

#[test]
fn an_accepts_estimate_suits_the_issues_team() {
    let team = |kind: &str, zero: bool| -> Estimation { crate::json::decode(json!({ "id": "t1", "issueEstimationType": kind, "issueEstimationExtended": false, "issueEstimationAllowZero": zero }), "Linear's answer to Triaged").unwrap() };
    assert!(accepted_estimate(&team("tShirt", false), Some(5)).is_ok());
    assert!(accepted_estimate(&team("exponential", false), Some(16)).is_ok());
    assert!(accepted_estimate(&team("linear", true), Some(0)).is_ok());
    // Off the scale, or missing on a team that estimates: usage errors.
    for (team, estimate) in [(team("exponential", false), Some(5)), (team("linear", false), Some(0)), (team("fibonacci", false), None), (team("notUsed", false), Some(1))] {
        assert!(matches!(accepted_estimate(&team, estimate), Err(Error::Usage(_))), "{team:?} {estimate:?}");
    }
    // A team that doesn't estimate takes none.
    assert!(accepted_estimate(&team("notUsed", false), None).is_ok());
    // A scale of a kind Linear doesn't document refuses, but not as a usage error.
    assert!(matches!(accepted_estimate(&team("cubits", false), None), Err(Error::Failed(_))));
}
