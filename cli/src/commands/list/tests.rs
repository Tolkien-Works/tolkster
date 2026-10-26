use super::*;

#[test]
fn no_state_means_open_and_a_name_is_matched_whatever_its_case() {
    let open = json!({ "state": { "type": { "nin": ["completed", "canceled", "duplicate"] } } });
    assert_eq!(filter(None, None, None, None).unwrap(), json!({ "and": [open.clone()] }));
    assert_eq!(filter(None, None, Some("Open"), None).unwrap(), json!({ "and": [open] }));
    assert_eq!(filter(None, None, Some("In Review"), None).unwrap(), json!({ "and": [{ "state": { "name": { "eqIgnoreCase": "In Review" } } }] }));
}

#[test]
fn every_option_given_narrows_the_filter() {
    let all = filter(Some("tw"), Some("Tolkster"), None, Some("tw-587")).unwrap();
    assert_eq!(
        all,
        json!({ "and": [
            { "team": { "key": { "eq": "TW" } } },
            { "project": { "name": { "eqIgnoreCase": "Tolkster" } } },
            { "state": { "type": { "nin": ["completed", "canceled", "duplicate"] } } },
            { "parent": { "and": [{ "team": { "key": { "eq": "TW" } } }, { "number": { "eq": 587 } }] } },
        ] })
    );
    assert!(matches!(filter(None, None, None, Some("587")), Err(Error::Usage(_))));
}

#[test]
fn an_issue_is_printed_by_its_title_and_state_alone() {
    let data = json!({ "searchIssues": {
        "nodes": [{
            "identifier": "TW-5", "title": "A title", "priority": 3, "estimate": null, "updatedAt": "2026-10-03T00:00:00.000Z",
            "state": { "name": "Backlog", "type": "backlog" }, "parent": { "identifier": "TW-1" }, "description": "never printed",
        }, {
            "identifier": "TW-6", "title": "Another", "priority": 0, "estimate": 2, "updatedAt": "2026-10-02T00:00:00.000Z",
            "state": { "name": "Todo", "type": "unstarted" }, "parent": null,
        }],
        "pageInfo": { "hasNextPage": true },
    } });
    let listed = |data: Value| render(&crate::json::decode(data, "Linear's answer to List").unwrap());
    let out = listed(data["searchIssues"].clone());
    assert_eq!(out["more"], true);
    assert_eq!(
        out["issues"][0],
        json!({ "identifier": "TW-5", "title": "A title", "state": { "name": "Backlog", "type": "backlog" }, "priority": 3, "estimate": null, "parent": "TW-1", "updatedAt": "2026-10-03T00:00:00.000Z" })
    );
    assert!(out["issues"][1]["parent"].is_null());
    assert_eq!(listed(json!({ "nodes": [] })), json!({ "issues": [], "more": false }));
}
