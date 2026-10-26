use super::*;

#[test]
fn a_session_listed_through_its_issue_or_from_the_newest_decodes_either_way() {
    let through = json!({ "appUser": { "id": "app" }, "id": "s1", "status": "active", "updatedAt": "2026-10-23T01:00:00.000Z", "creator": { "id": "owner" } });
    let listed: Listed = decode(through, "Linear's answer to IssueSessions").unwrap();
    assert_eq!((listed.app_user_id(), listed.issue.is_none(), listed.creator.as_ref().map(|c| c.id.as_str())), (Some("app"), true, Some("owner")));
    let newest = json!({ "id": "s2", "status": "complete", "updatedAt": "2026-10-23T02:00:00.000Z", "creator": null, "issue": { "id": "i1" } });
    let listed: Listed = decode(newest, "Linear's answer to Sessions").unwrap();
    assert_eq!((listed.app_user_id(), listed.issue.as_ref().map(|i| i.id.as_str()), listed.status.as_str()), (None, Some("i1"), "complete"));
}

#[test]
fn a_pair_reads_the_first_issues_relations_and_the_second_by_id() {
    let answer = json!({
        "a": { "id": "i1", "identifier": "TW-1", "relations": { "nodes": [{ "id": "r1", "type": "blocks", "relatedIssue": { "identifier": "TW-2" } }] }, "inverseRelations": { "nodes": [] } },
        "b": { "id": "i2", "identifier": "TW-2" },
    });
    let pair: Pair = decode(answer, "Linear's answer to Pair").unwrap();
    assert_eq!(crate::linear::Relation::Blocks.find(&pair.a.relations, &pair.b.identifier), ["r1"]);
    let error = decode::<Pair>(json!({ "a": { "identifier": "TW-1" }, "b": null }), "Linear's answer to Pair").unwrap_err().to_string();
    assert_eq!(error, "Linear's answer to Pair didn't decode: it had no `id`");
}
