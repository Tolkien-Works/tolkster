use super::*;
use crate::json::decode;
use crate::linear::{StateType, answer_to};

#[test]
fn a_triaged_issue_decodes_with_its_teams_id_and_states() {
    let answer = json!({ "viewer": { "id": "app" }, "issue": {
        "id": "i1", "identifier": "TW-231", "createdAt": "2026-09-28T01:00:00.000Z", "archivedAt": null,
        "state": { "id": "s0", "name": "Triage", "type": "triage", "position": 0 }, "project": null, "creator": { "id": "owner" },
        "team": { "id": "t1", "key": "TW", "issueEstimationType": "fibonacci", "issueEstimationExtended": false, "issueEstimationAllowZero": false,
            "states": { "nodes": [{ "id": "s0", "name": "Triage", "type": "triage", "position": 0 }, { "id": "s1", "name": "Backlog", "type": "backlog", "position": 1 }] } },
    } });
    let triaged: Triaged = decode(answer, &answer_to(TRIAGED)).unwrap();
    let read = triaged.issue.unwrap();
    assert_eq!((read.id.as_str(), read.issue.state.kind, read.issue.team.id.as_deref()), ("i1", StateType::Triage, Some("t1")));
    assert_eq!(read.issue.team.states.unwrap().nodes.len(), 2);
    assert!(read.issue.relations.relations.is_none() && read.issue.project.is_none());
}
