use serde_json::Value;

use super::*;
use crate::github::COPILOT;
use crate::ports::fake::FakeReader;
use crate::time::{ms, t};

// A PR as `pr_query` answers with it, every field it asks for: open on `tw-2` of
// Tolkien-Works/tolkster at HEAD, its checks green, with nothing on its review connections.
fn pr_node() -> Value {
    let page = json!({ "nodes": [], "pageInfo": { "hasPreviousPage": false, "startCursor": null } });
    json!({
        "state": "OPEN", "mergedAt": null, "closedAt": null, "updatedAt": "2026-09-26T01:04:00Z",
        "headRefName": "tw-2", "headRepository": { "nameWithOwner": "Tolkien-Works/tolkster" },
        "number": 7, "headRefOid": HEAD, "mergeStateStatus": "CLEAN", "baseRefName": "main",
        "author": { "__typename": "Bot", "login": "tolkster-agent" }, "repository": { "nameWithOwner": "Tolkien-Works/tolkster" },
        "autoMergeRequest": null, "timelineItems": { "nodes": [] },
        "baseRefOid": "b0", "baseRef": { "target": { "oid": "b1", "committedDate": "2026-09-26T01:01:00Z" } },
        "commits": { "nodes": [{ "commit": { "oid": HEAD, "statusCheckRollup": { "state": "SUCCESS" } } }] },
        "reviews": page, "reviewThreads": page, "comments": page,
        "title": "never read",
    })
}

fn node(answer: Value) -> PrNode {
    decode(answer, "GitHub's answer to the PR query").unwrap()
}

#[test]
fn a_branch_is_read_in_one_light_query_that_reap_and_review_both_filter() {
    let data = json!({ "repository": {
        "all": { "nodes": [
            { "number": 9, "state": "OPEN", "headRefOid": "b", "isCrossRepository": false },
            { "number": 7, "state": "MERGED", "headRefOid": "a", "isCrossRepository": false },
            { "number": 5, "state": "CLOSED", "headRefOid": "0", "isCrossRepository": false },
        ], "pageInfo": { "hasNextPage": false } },
    } });
    let parse = |answer: &Value| branch_prs(&mut FakeReader::always(answer.clone()), "o/n", "tw-1");
    let mut gh = FakeReader::always(data.clone());
    let read = branch_prs(&mut gh, "o/n", "tw-1").unwrap();
    assert_eq!(gh.calls, [r#"graphql o/n {"branch":"tw-1"}"#]);
    assert_eq!(
        read,
        [
            BranchPr { number: 9, state: PrState::Open, head: "b".into() },
            BranchPr { number: 7, state: PrState::Merged, head: "a".into() },
            BranchPr { number: 5, state: PrState::Closed, head: "0".into() },
        ]
    );
    // A branch with more PRs than a page is never judged on some of them.
    let mut over = data.clone();
    over["repository"]["all"]["pageInfo"]["hasNextPage"] = json!(true);
    assert_eq!(parse(&over).unwrap_err().to_string(), "branch tw-1 has over 100 pull requests in o/n");
    // A PR without a number, a state or where its head is fails naming the query, and so does
    // a response without the list.
    let mut odd = data.clone();
    odd["repository"]["all"]["nodes"][0] = json!({ "state": "OPEN", "headRefOid": "b", "isCrossRepository": false });
    assert_eq!(parse(&odd).unwrap_err().to_string(), "GitHub's answer to the branch's PRs query didn't decode: it had no `number`");
    odd["repository"]["all"]["nodes"][0] = json!({ "number": 9, "headRefOid": "b", "isCrossRepository": false });
    assert!(parse(&odd).is_err());
    odd["repository"]["all"]["nodes"][0] = json!({ "number": 9, "state": "OPEN", "headRefOid": "b" });
    assert!(parse(&odd).is_err());
    assert!(parse(&json!({ "repository": { "all": {} } })).is_err() && parse(&Value::Null).is_err());
    assert_eq!(parse(&json!({ "repository": null })).unwrap_err().to_string(), "GitHub had no pull requests for o/n");
    // A fork's PRs on the branch's name are left out, and five open ones ahead of the
    // worker's own don't hide it.
    let fork = |number: u64| json!({ "number": number, "state": "OPEN", "headRefOid": "f", "isCrossRepository": true });
    let forks: Vec<u64> = (20..25).collect();
    let mut crowded = data.clone();
    crowded["repository"]["all"]["nodes"] = json!(forks.iter().map(|n| fork(*n)).chain(data["repository"]["all"]["nodes"].as_array().unwrap().iter().cloned()).collect::<Vec<_>>());
    let read = branch_prs(&mut FakeReader::always(crowded), "o/n", "tw-1").unwrap();
    assert_eq!(read.iter().map(|pr| pr.number).collect::<Vec<_>>(), [9, 7, 5]);
    // A run reads each branch once, and reads again only after a failed read.
    let mut reads = 0;
    let mut gh = FakeReader::new(|_, _| {
        reads += 1;
        if reads == 1 { Err(Error::Failed("down".into())) } else { Ok(data.clone()) }
    });
    let mut branches = Branches::new();
    assert!(branches.prs(&mut gh, "o/n", "tw-1").is_err());
    assert_eq!(branches.prs(&mut gh, "o/n", "tw-1").unwrap().len(), 3);
    assert_eq!(branches.prs(&mut gh, "O/N", "tw-1").unwrap().len(), 3);
    assert_eq!(branches.prs(&mut gh, "o/n", "tw-2").unwrap().len(), 3);
    assert_eq!(gh.calls.len(), 3);
}

#[test]
fn a_review_request_is_never_read_as_feedback() {
    // No brief asks Copilot for a review, but the ruleset may request one. Only reviews,
    // threads and comments are read, so the request itself can't wake a worker.
    assert_eq!(REVIEW_CONNECTIONS.map(|(name, _)| name), ["reviews", "reviewThreads", "comments"]);
    for query in [branch_prs_query(), pr_query()] {
        for field in ["reviewRequests", "REVIEW_REQUESTED", "ReviewRequestedEvent"] {
            assert!(!query.contains(field), "{field}");
        }
    }
    // The timeline is read only for how often auto-merge was turned off.
    let query = pr_query();
    assert_eq!(query.matches("timelineItems").count(), 1);
    assert!(query.contains("timelineItems(itemTypes: [AUTO_MERGE_DISABLED_EVENT], last: 1) { nodes { __typename } }"));
    // Every PR on the branch, in one query with nothing of their reviews: that costs a point,
    // not a hundred.
    let query = branch_prs_query();
    assert!(query.contains("all: pullRequests(headRefName: $branch, first: 100,"));
    assert!(query.contains("isCrossRepository"));
    assert_eq!(query.matches("pullRequests(").count(), 1);
    for field in REVIEW_CONNECTIONS.map(|(name, _)| name).into_iter().chain(["timelineItems", "commits", "author"]) {
        assert!(!query.contains(field), "{field}");
    }
}

#[test]
fn a_pr_actions_pr_is_read_once_by_number_and_its_head_stands_in_for_its_branch() {
    let pr = |state: &str| {
        let mut node = pr_node();
        node["state"] = json!(state);
        if state != "OPEN" {
            node["closedAt"] = json!("2026-09-26T01:03:00Z");
        }
        if state == "MERGED" {
            node["mergedAt"] = json!("2026-09-26T01:03:00Z");
        }
        json!({ "repository": { "pullRequest": node } })
    };
    let mut gh = FakeReader::always(pr("MERGED"));
    let (merged, first) = read_pr(&mut gh, "tolkien-works/tolkster", 7).unwrap();
    // One GraphQL query, by number, with what the review resume reads of an open PR.
    assert_eq!(gh.calls, ["graphql tolkien-works/tolkster {\"number\":7}"]);
    let query = pr_query();
    assert!(query.contains("pullRequest(number: $number)") && query.contains(&open_pr_fields()));
    assert_eq!(merged, Head { number: 7, branch: "tw-2".into(), same_repo: true, state: PrState::Merged, sha: HEAD.into(), at: instant("2026-09-26T01:03:00Z") });
    // The first page is the PR's, as a branch read returns an open one.
    let read = first_page_pr(&first);
    assert_eq!((read.number, read.repo.as_str(), read.base.as_str(), read.merge_state.as_str()), (7, "Tolkien-Works/tolkster", "main", "CLEAN"));
    assert_eq!((read.base_head.as_str(), read.base_at, read.bot_author.as_deref()), ("b1", instant("2026-09-26T01:01:00Z"), Some("tolkster-agent")));
    assert!(read.checks_green && !read.auto_merge_on && !read.auto_merge_off);
    // Each state's own time: an open PR's is its last update.
    let open = read_pr(&mut FakeReader::always(pr("OPEN")), "o/n", 7).unwrap().0;
    assert_eq!((open.state, open.at), (PrState::Open, instant("2026-09-26T01:04:00Z")));
    assert_eq!(read_pr(&mut FakeReader::always(pr("CLOSED")), "o/n", 7).unwrap().0.state, PrState::Closed);
    // A deleted fork's PR has no head repo.
    let mut deleted = pr("OPEN");
    deleted["repository"]["pullRequest"]["headRepository"] = Value::Null;
    assert!(!read_pr(&mut FakeReader::always(deleted), "o/n", 7).unwrap().0.same_repo);
    // A base branch gone leaves the base GitHub last recorded.
    let mut gone = pr("OPEN");
    gone["repository"]["pullRequest"]["baseRef"] = Value::Null;
    let read = first_page_pr(&read_pr(&mut FakeReader::always(gone), "o/n", 7).unwrap().1);
    assert_eq!((read.base_head.as_str(), read.base_at), ("b0", None));
    for missing in [json!({ "repository": { "pullRequest": { "headRepository": null } } }), json!({ "repository": { "pullRequest": null } }), Value::Null] {
        assert!(read_pr(&mut FakeReader::always(missing.clone()), "o/n", 7).is_err(), "{missing}");
    }
}

// A PR's head, as review.rs's tests have it.
const HEAD: &str = "2878d3d76e19f5ab3b99e754850b711b9a6eef77";

#[test]
fn only_a_successful_rollup_at_the_head_counts_as_green() {
    let rollup = |oid: &str, state: Value| {
        let mut answer = pr_node();
        answer["commits"]["nodes"][0]["commit"] = json!({ "oid": oid, "statusCheckRollup": state });
        parse_pr(&node(answer)).checks_green
    };
    assert!(rollup(HEAD, json!({ "state": "SUCCESS" })));
    // Failed, errored, waiting or expected: the checks are read in full.
    for state in ["FAILURE", "ERROR", "PENDING", "EXPECTED", "SOMETHING_NEW"] {
        assert!(!rollup(HEAD, json!({ "state": state })), "{state}");
    }
    assert!(!rollup(HEAD, Value::Null));
    assert!(!rollup("an-older-commit", json!({ "state": "SUCCESS" })));
}

#[test]
fn feedback_beyond_the_first_hundred_is_read_too() {
    let mut pr = pr_node();
    pr["reviewThreads"] = json!({ "nodes": [{ "isResolved": true, "isOutdated": false, "comments": { "nodes": [] } }], "pageInfo": { "hasPreviousPage": true, "startCursor": "c2" } });
    let mut gh = FakeReader::new(|_, variables| {
        let (nodes, more) = match variables["before"].as_str() {
            Some("c2") => (json!([{ "isResolved": false, "isOutdated": false, "comments": { "nodes": [{ "author": { "login": COPILOT }, "createdAt": t(10), "publishedAt": null }] } }]), true),
            _ => (json!([]), false),
        };
        Ok(json!({ "repository": { "pullRequest": { "page": { "nodes": nodes, "pageInfo": { "hasPreviousPage": more, "startCursor": "c1" } } } } }))
    });
    let parsed = read_open_pr(&mut gh, "o/n", FirstPage(node(pr))).unwrap();
    assert_eq!(gh.calls, [r#"graphql o/n {"before":"c2","number":7}"#, r#"graphql o/n {"before":"c1","number":7}"#]);
    // Oldest first, with the older page's open Copilot thread.
    assert_eq!(parsed.threads, [
        Thread { resolved: false, outdated: false, author: Some(COPILOT.into()), at: Some(ms(10)) },
        Thread { resolved: true, outdated: false, author: None, at: None },
    ]);
}
