use super::*;
use serde_json::json;

fn limits(used: Option<u64>, remaining: Option<u64>, resource: Option<&str>) -> Limits {
    Limits { used, remaining, resource: resource.map(String::from), ..Limits::default() }
}

#[test]
fn a_query_asks_for_its_cost_and_a_mutation_does_not() {
    assert_eq!(with_rate_limit("query($id: ID!) { node(id: $id) { id } }
"), "query($id: ID!) { node(id: $id) { id } rateLimit { cost remaining resetAt } }");
    let mutation = "mutation($id: ID!) { resolveReviewThread(input: { threadId: $id }) { thread { id } } }";
    assert_eq!(with_rate_limit(mutation), mutation);
}

#[test]
fn a_query_is_logged_with_its_own_cost() {
    let body = json!({ "data": { "rateLimit": { "cost": 3, "remaining": 4990, "resetAt": "2026-10-01T01:00:00Z" } } });
    assert_eq!(line("head_checks", &limits(Some(10), Some(4990), Some("graphql")), &body).map(|cost| cost.to_string()).as_deref(), Some("github: head_checks graphql cost=3 remaining=4990 resetAt=2026-10-01T01:00:00Z"));
    assert_eq!(line("head_checks", &Limits::default(), &body).map(|cost| cost.to_string()).as_deref(), Some("github: head_checks graphql cost=3 remaining=4990 resetAt=2026-10-01T01:00:00Z"));
}

#[test]
fn a_reset_that_is_not_a_time_is_left_out() {
    let body = json!({ "data": { "rateLimit": { "cost": 1, "remaining": 5, "resetAt": "now\nforged line" } } });
    assert_eq!(line("checks", &Limits::default(), &body).map(|cost| cost.to_string()).as_deref(), Some("github: checks graphql cost=1 remaining=5"));
}

#[test]
fn any_other_call_is_logged_from_the_headers() {
    assert_eq!(line("copilot_reviews", &limits(Some(12), Some(4988), Some("core")), &Value::Null).map(|cost| cost.to_string()).as_deref(), Some("github: copilot_reviews core used=12 remaining=4988"));
    assert_eq!(line("reply", &limits(None, Some(7), None), &json!({ "data": {} })).map(|cost| cost.to_string()).as_deref(), Some("github: reply api remaining=7"));
    assert_eq!(line("reply", &Limits::default(), &Value::Null), None);
}

#[test]
fn a_site_is_named_until_dropped() {
    assert_eq!(current(), "unnamed");
    {
        let _outer = site("feedback");
        assert_eq!(current(), "feedback");
        {
            let _inner = site("readiness");
            assert_eq!(current(), "readiness");
        }
        assert_eq!(current(), "feedback");
    }
    assert_eq!(current(), "unnamed");
}
