use super::*;
use crate::json::decode;

#[test]
fn priorities_take_linears_numbers_or_names() {
    for (text, expected) in [("0", 0), ("None", 0), ("urgent", 1), ("2", 2), ("medium", 3), ("normal", 3), (" low ", 4)] {
        assert_eq!(priority(text).unwrap(), expected, "{text}");
    }
    for text in ["5", "", "highest", "-1"] {
        assert!(matches!(priority(text), Err(Error::Usage(_))), "{text}");
    }
}

#[test]
fn an_assignee_is_me_none_or_a_user_id() {
    let id = "7c1d9e20-4b3a-4f58-9a61-0d2e8f3b5c4c";
    assert_eq!(assignee("me", "owner-1").unwrap(), json!("owner-1"));
    assert_eq!(assignee(" ME ", "owner-1").unwrap(), json!("owner-1"));
    assert_eq!(assignee("None", "owner-1").unwrap(), Value::Null);
    assert_eq!(assignee(&id.to_ascii_uppercase(), "owner-1").unwrap(), json!(id));
    for text in ["", "chad", "7c1d9e20", "7c1d9e20-4b3a-4f58-9a61-0d2e8f3b5c4", "7c1d9e20-4b3a-4f58-9a61-0d2e8f3b5c4g"] {
        assert!(matches!(assignee(text, "owner-1"), Err(Error::Usage(_))), "{text}");
    }
}

#[test]
fn an_estimate_is_whole_points_or_none() {
    assert_eq!(estimate("3").unwrap(), json!(3));
    assert_eq!(estimate("none").unwrap(), Value::Null);
    for text in ["1.5", "-2", "x", ""] {
        assert!(matches!(estimate(text), Err(Error::Usage(_))), "{text}");
    }
}

#[test]
fn an_estimate_is_written_only_on_the_teams_scale() {
    let team = |kind: &str| -> Estimation { decode(json!({ "id": "t1", "issueEstimationType": kind, "issueEstimationExtended": false, "issueEstimationAllowZero": false }), "Linear's answer to Target").unwrap() };
    assert!(check_estimate(&team("fibonacci"), &json!(5)).is_ok());
    assert!(matches!(check_estimate(&team("fibonacci"), &json!(4)), Err(Error::Usage(_))));
    assert!(matches!(check_estimate(&team("exponential"), &json!(5)), Err(Error::Usage(_))));
    assert!(matches!(check_estimate(&team("notUsed"), &json!(1)), Err(Error::Usage(_))));
    // Clearing it always may.
    assert!(check_estimate(&team("notUsed"), &Value::Null).is_ok());
    assert!(matches!(check_estimate(&team("cubits"), &json!(1)), Err(Error::Failed(_))));
}

#[test]
fn blocked_by_is_the_other_issue_blocking_this_one() {
    assert_eq!(Relation::Blocks.input("a", "b"), json!({ "issueId": "a", "relatedIssueId": "b", "type": "blocks" }));
    assert_eq!(Relation::BlockedBy.input("a", "b"), json!({ "issueId": "b", "relatedIssueId": "a", "type": "blocks" }));
    assert_eq!(Relation::Related.input("a", "b"), json!({ "issueId": "a", "relatedIssueId": "b", "type": "related" }));
    assert_eq!(Relation::DuplicateOf.input("a", "b"), json!({ "issueId": "a", "relatedIssueId": "b", "type": "duplicate" }));
}

#[test]
fn relations_are_found_by_direction_and_type() {
    let issue: Relations<Identified> = decode(json!({
        "id": "i1", "identifier": "TW-1",
        "relations": { "nodes": [
            { "id": "r1", "type": "blocks", "relatedIssue": { "identifier": "TW-2" } },
            { "id": "r2", "type": "related", "relatedIssue": { "identifier": "TW-2" } },
            { "id": "r3", "type": "duplicate", "relatedIssue": { "identifier": "TW-3" } },
        ] },
        "inverseRelations": { "nodes": [
            { "id": "r4", "type": "blocks", "issue": { "identifier": "TW-2" } },
            { "id": "r5", "type": "related", "issue": { "identifier": "TW-2" } },
        ] },
    }), "Linear's answer to Pair").unwrap();
    assert_eq!(Relation::Blocks.find(&issue, "TW-2"), ["r1"]);
    assert_eq!(Relation::BlockedBy.find(&issue, "TW-2"), ["r4"]);
    assert_eq!(Relation::Related.find(&issue, "TW-2"), ["r2", "r5"]);
    assert_eq!(Relation::DuplicateOf.find(&issue, "TW-3"), ["r3"]);
    assert!(Relation::DuplicateOf.find(&issue, "TW-2").is_empty());
    assert!(Relation::Blocks.find(&decode(json!({ "id": "i1" }), "Linear's answer to Pair").unwrap(), "TW-2").is_empty());
}

#[test]
fn states_and_labels_are_picked_by_name_ignoring_case() {
    let states: Vec<WorkflowState> = decode(json!([{ "id": "s1", "name": "In Progress", "type": "started", "position": 1 }, { "id": "s2", "name": "Done", "type": "completed", "position": 2.5 }]), "Linear's answer to States").unwrap();
    assert_eq!(pick_state(&states, "in progress").unwrap(), "s1");
    assert!(pick_state(&states, "Doing").is_err());

    let projects: Vec<TeamProject> = decode(json!([
        { "id": "p1", "name": "Tolkster", "completedAt": null, "canceledAt": null },
        { "id": "p2", "name": "Tolkster", "completedAt": "2026-09-01T00:00:00.000Z", "canceledAt": null },
        { "id": "p3", "name": "Old", "completedAt": null, "canceledAt": "2026-09-01T00:00:00.000Z" },
    ]), "Linear's answer to Projects").unwrap();
    assert_eq!(pick_project(&projects, "tolkster").unwrap(), json!("p1"));
    assert_eq!(pick_project(&projects, " None ").unwrap(), Value::Null);
    assert!(matches!(pick_project(&projects, "Nobody's"), Err(Error::Failed(_))));
    assert!(matches!(pick_project(&projects, "old"), Err(Error::Failed(_))));

    let labels: Vec<Label> = decode(json!([
        { "id": "l1", "name": "Bug", "team": null },
        { "id": "l2", "name": "Docs", "team": { "id": "t1" } },
        { "id": "l3", "name": "Docs", "team": { "id": "t2" } },
        { "id": "l4", "name": "UX", "team": { "id": "t1" } },
        { "id": "l5", "name": "ux", "team": null },
    ]), "Linear's answer to Labels").unwrap();
    assert_eq!(pick_label(&labels, "t1", "bug").unwrap(), "l1");
    // Another team's label of the same name doesn't count.
    assert_eq!(pick_label(&labels, "t1", "Docs").unwrap(), "l2");
    assert_eq!(pick_label(&labels, "t2", "Docs").unwrap(), "l3");
    assert!(pick_label(&labels, "t3", "Docs").is_err());
    assert!(matches!(pick_label(&labels, "t1", "UX"), Err(Error::Failed(m)) if m.contains("several")));
}
