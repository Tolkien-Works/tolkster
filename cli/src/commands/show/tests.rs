use super::*;

fn render(issue: &Value, comments: bool, children: bool, attachments: bool) -> Value {
    super::render(issue, comments, children, attachments).unwrap()
}

fn issue(identifier: &str, sort_order: f64) -> Value {
    json!({
        "id": format!("id-{identifier}"), "identifier": identifier, "title": format!("Title of {identifier}"), "url": format!("https://linear.app/x/issue/{identifier}"),
        "description": "What to do.", "priority": 2, "priorityLabel": "High", "estimate": 3, "dueDate": null, "branchName": format!("do-it-{}", identifier.to_lowercase()),
        "sortOrder": sort_order, "createdAt": "2026-09-01T00:00:00.000Z", "updatedAt": "2026-09-02T00:00:00.000Z", "completedAt": null, "canceledAt": null,
        "state": { "name": "In Progress", "type": "started" }, "team": { "key": "TW", "name": "Tolkien Works" }, "project": { "name": "Tolkster" },
        "assignee": { "id": "u1", "displayName": "sam" }, "delegate": { "id": "app", "name": "Tolkster" },
        "labels": { "nodes": [{ "name": "plan-first" }] }, "parent": null,
        "relations": { "nodes": [{ "type": "blocks", "relatedIssue": { "identifier": "TW-9", "url": "u9", "state": { "type": "backlog" } } }] },
        "inverseRelations": { "nodes": [
            { "type": "blocks", "issue": { "identifier": "TW-8", "url": "u8", "state": { "type": "completed" } } },
            { "type": "related", "issue": { "identifier": "TW-7", "url": "u7", "state": { "type": "started" } } },
        ] },
    })
}

#[test]
fn an_issue_is_printed_with_its_relations_named_as_relate_takes_them() {
    let out = render(&issue("TW-1", 1.0), false, false, false);
    assert_eq!(out.as_object().unwrap().keys().collect::<Vec<_>>(), ["issue"]);
    let issue = &out["issue"];
    assert_eq!(issue["identifier"], "TW-1");
    assert_eq!(issue["state"], json!({ "name": "In Progress", "type": "started" }));
    assert_eq!(issue["project"], "Tolkster");
    assert_eq!(issue["assignee"], json!({ "id": "u1", "displayName": "sam" }));
    assert_eq!(issue["delegate"], json!({ "id": "app", "name": "Tolkster" }));
    assert_eq!(issue["labels"], json!(["plan-first"]));
    assert_eq!(issue["parent"], Value::Null);
    assert_eq!(
        issue["relations"],
        json!([
            { "type": "blocks", "identifier": "TW-9", "url": "u9", "state": "backlog" },
            { "type": "blocked-by", "identifier": "TW-8", "url": "u8", "state": "completed" },
            { "type": "related", "identifier": "TW-7", "url": "u7", "state": "started" },
        ])
    );
    assert!(issue.get("sortOrder").is_none());
}

#[test]
fn nulls_stay_null_and_a_parent_is_named() {
    let mut bare = issue("TW-2", 1.0);
    bare["assignee"] = Value::Null;
    bare["delegate"] = Value::Null;
    bare["project"] = Value::Null;
    bare["description"] = Value::Null;
    bare["parent"] = json!({ "identifier": "TW-1", "title": "Parent", "url": "u1" });
    let issue = render(&bare, false, false, false)["issue"].clone();
    for field in ["assignee", "delegate", "project", "description", "dueDate"] {
        assert_eq!(issue[field], Value::Null, "{field}");
    }
    assert_eq!(issue["parent"], json!({ "identifier": "TW-1", "title": "Parent", "url": "u1" }));
}

#[test]
fn comments_come_oldest_first_with_their_authors_and_replies() {
    let mut with = issue("TW-1", 1.0);
    with["comments"] = json!({ "nodes": [
        { "id": "c2", "body": "Reply", "createdAt": "2026-09-03T00:00:00.000Z", "updatedAt": "2026-09-03T00:00:00.000Z", "parent": { "id": "c1" }, "user": { "id": "u1", "displayName": "sam" } },
        { "id": "c1", "body": "First", "createdAt": "2026-09-02T00:00:00.000Z", "updatedAt": "2026-09-02T00:00:00.000Z", "parent": null, "user": null },
    ] });
    let out = render(&with, true, false, false);
    assert_eq!(out["comments"].as_array().unwrap().iter().map(|c| c["id"].as_str().unwrap()).collect::<Vec<_>>(), ["c1", "c2"]);
    assert_eq!(out["comments"][0]["user"], Value::Null);
    assert_eq!(out["comments"][0]["parentId"], Value::Null);
    assert_eq!(out["comments"][1]["parentId"], "c1");
    assert_eq!(out["comments"][1]["user"], json!({ "id": "u1", "displayName": "sam" }));
    assert!(out.get("children").is_none() && out.get("attachments").is_none());
    // Asked for and empty: an empty list, not nothing.
    assert_eq!(render(&issue("TW-1", 1.0), true, true, true)["comments"], json!([]));
}

#[test]
fn sub_issues_keep_the_parents_order_and_carry_their_links() {
    let mut parent = issue("TW-1", 1.0);
    let mut second = issue("TW-3", 2.0);
    second["attachments"] = json!({ "nodes": [{ "title": "PR #3", "url": "https://github.com/x/y/pull/3" }] });
    let mut first = issue("TW-2", 1.5);
    first["attachments"] = json!({ "nodes": [] });
    // The same sort order falls back to the number.
    let mut third = issue("TW-4", 2.0);
    third["attachments"] = Value::Null;
    parent["children"] = json!({ "nodes": [second, third, first] });
    parent["attachments"] = json!({ "nodes": [{ "title": "Design", "url": "https://example.test/design" }] });
    let out = render(&parent, false, true, true);
    let children = out["children"].as_array().unwrap();
    assert_eq!(children.iter().map(|c| c["identifier"].as_str().unwrap()).collect::<Vec<_>>(), ["TW-2", "TW-3", "TW-4"]);
    assert_eq!(children[0]["attachments"], json!([]));
    assert_eq!(children[1]["attachments"], json!([{ "title": "PR #3", "url": "https://github.com/x/y/pull/3" }]));
    assert_eq!(children[2]["attachments"], json!([]));
    assert_eq!(children[1]["relations"][0]["type"], "blocks");
    assert_eq!(out["attachments"], json!([{ "title": "Design", "url": "https://example.test/design" }]));
}

#[test]
fn reads_at_once_come_back_in_order_past_one_batch() {
    let done = at_once((0..AT_ONCE * 2 + 1).map(|n| move || n));
    assert_eq!(done, (0..AT_ONCE * 2 + 1).collect::<Vec<_>>());
}
