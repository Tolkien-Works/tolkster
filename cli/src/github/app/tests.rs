use super::*;

// The Worker mints by these names (src/worker/mint.ts, PURPOSES), as test/contract.json pins.
#[test]
fn the_purposes_are_the_workers() {
    let mut names: Vec<&str> = Purpose::ALL.iter().map(|p| p.name()).collect();
    names.sort_unstable();
    assert_eq!(serde_json::to_value(names).unwrap(), crate::state::tests::contract()["githubPurposes"]);
}

#[test]
fn a_token_is_reused_for_fifty_minutes() {
    assert!(fresh(1_000, 1_000 + TOKEN_LIFE_SECS - 1));
    assert!(!fresh(1_000, 1_000 + TOKEN_LIFE_SECS));
    // A clock that went back still counts the token as fresh.
    assert!(fresh(1_000, 900));
}

#[test]
fn a_refusal_says_why() {
    let body = json!({ "message": "Validation Failed", "errors": [{ "message": "A pull request already exists for o:b." }] });
    assert_eq!(refused("the pull request", 422, &body).to_string(), "GitHub refused the pull request: HTTP 422: Validation Failed; A pull request already exists for o:b.");
    assert_eq!(refused("an installation token", 403, &Value::Null).to_string(), "GitHub refused an installation token: HTTP 403");
    // A detail that isn't an object, as a 422's may be, says nothing; the rest still does.
    let mixed = json!({ "message": "Validation Failed", "errors": ["a bare string", { "resource": "PullRequest", "code": "custom", "message": "No commits between main and b" }] });
    assert_eq!(refused("the pull request", 422, &mixed).to_string(), "GitHub refused the pull request: HTTP 422: Validation Failed; No commits between main and b");
    assert_eq!(refused("reading rules", 404, &json!(["a", "b"])).to_string(), "GitHub refused reading rules: HTTP 404");
    // A field of another shape hides neither the other field nor its fellow errors.
    let odd = json!({ "message": 7, "errors": [{ "message": ["x"] }, { "message": "No commits between main and b" }] });
    assert_eq!(refused("the pull request", 422, &odd).to_string(), "GitHub refused the pull request: HTTP 422: No commits between main and b");
    let odd = json!({ "message": "Validation Failed", "errors": "a bare string" });
    assert_eq!(refused("the pull request", 422, &odd).to_string(), "GitHub refused the pull request: HTTP 422: Validation Failed");
}

#[test]
fn a_server_error_is_unavailable_and_says_nothing_was_refused() {
    for status in [500, 502, 503, 504] {
        let error = refused("the GraphQL call", status, &json!({ "message": "We couldn't respond to your request in time" }));
        assert!(matches!(error, Error::Unavailable(_)), "{status}");
        assert_eq!(error.to_string(), format!("GitHub couldn't answer the GraphQL call: HTTP {status}"));
    }
    for status in [400, 401, 403, 404, 422] {
        assert!(matches!(refused("the GraphQL call", status, &Value::Null), Error::Failed(_)), "{status}");
    }
    assert!(matches!(graphql_data(json!({ "errors": [{ "message": "x" }] })), Err(Error::Failed(_))));
    assert!(matches!(Error::from(ureq::Error::Timeout(ureq::Timeout::Global)), Error::Unavailable(_)));
    assert!(matches!(Error::from(ureq::Error::ConnectionFailed), Error::Unavailable(_)));
    assert!(matches!(Error::from(ureq::Error::BadUri("x".into())), Error::Failed(_)));
}

#[test]
fn graphql_errors_are_read() {
    let refused = graphql_data(json!({ "data": null, "errors": [{ "message": "Resource not accessible by integration" }] }));
    assert_eq!(refused.unwrap_err().to_string(), "GitHub refused the GraphQL call: Resource not accessible by integration");
    assert!(graphql_data(json!({ "data": null })).is_err());
    let data = graphql_data(json!({ "data": { "ok": true } })).unwrap();
    assert_eq!(data, json!({ "ok": true }));
}

fn limits(headers: &[(&str, &str)]) -> Limits {
    Limits::read(|name| headers.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string()))
}

#[test]
fn a_spent_graphql_budget_is_rate_limited_until_its_reset() {
    let body = json!({ "errors": [{ "type": "RATE_LIMITED", "message": "API rate limit already exceeded for installation ID 1." }] });
    let headers = limits(&[("x-ratelimit-limit", "5000"), ("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "1300"), ("x-ratelimit-resource", "graphql")]);
    let error = spent(200, &headers, &body, 1000).expect("rate limited");
    assert_eq!(error.rate_limited(), Some(Duration::from_secs(301)));
    assert_eq!(error.to_string(), "GitHub's rate limit is spent: it resets in 301 s");
    // Another GraphQL error, with budget left, is a refusal, not a spent budget.
    let left = limits(&[("x-ratelimit-remaining", "12")]);
    assert!(spent(200, &left, &json!({ "errors": [{ "type": "FORBIDDEN", "message": "no" }] }), 1000).is_none());
    assert!(spent(200, &left, &json!({ "errors": [{ "message": "API rate limit already exceeded for installation ID 1." }] }), 1000).is_none());
}

#[test]
fn an_untyped_graphql_error_with_none_remaining_is_rate_limited() {
    let body = json!({ "errors": [{ "message": "API rate limit already exceeded for installation ID 1." }] });
    let headers = limits(&[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "1300")]);
    assert_eq!(spent(200, &headers, &body, 1000).and_then(|e| e.rate_limited()), Some(Duration::from_secs(301)));
    // None remaining without an error is a success that used the last point.
    assert!(spent(200, &headers, &json!({ "data": {} }), 1000).is_none());
}

#[test]
fn a_spent_rest_budget_is_rate_limited() {
    let none_left = limits(&[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "1060")]);
    assert_eq!(spent(403, &none_left, &Value::Null, 1000).and_then(|e| e.rate_limited()), Some(Duration::from_secs(61)));
    assert_eq!(spent(429, &none_left, &Value::Null, 1000).and_then(|e| e.rate_limited()), Some(Duration::from_secs(61)));
    // A secondary limit asks to retry after a time, which comes first.
    let retry = limits(&[("retry-after", "30"), ("x-ratelimit-remaining", "12"), ("x-ratelimit-reset", "1060")]);
    assert_eq!(spent(403, &retry, &Value::Null, 1000).and_then(|e| e.rate_limited()), Some(Duration::from_secs(30)));
    // With no time given, a minute.
    assert_eq!(spent(429, &limits(&[("x-ratelimit-remaining", "0")]), &Value::Null, 1000).and_then(|e| e.rate_limited()), Some(Duration::from_secs(60)));
    // A secondary limit may say so only in its message.
    let secondary = json!({ "message": "You have exceeded a secondary rate limit. Please wait a few minutes before you try again." });
    assert_eq!(spent(403, &limits(&[("x-ratelimit-remaining", "12")]), &secondary, 1000).and_then(|e| e.rate_limited()), Some(Duration::from_secs(60)));
    // With some left, the window's reset is not its wait: a minute, however far off it is.
    let some_left = limits(&[("x-ratelimit-remaining", "4000"), ("x-ratelimit-reset", "3400")]);
    assert_eq!(spent(403, &some_left, &secondary, 1000).and_then(|e| e.rate_limited()), Some(Duration::from_secs(60)));
    // GraphQL's RATE_LIMITED with points left, too few for the query, waits for the reset.
    let typed = json!({ "errors": [{ "type": "RATE_LIMITED", "message": "API rate limit exceeded" }] });
    let few_left = limits(&[("x-ratelimit-remaining", "30"), ("x-ratelimit-reset", "1300")]);
    assert_eq!(spent(200, &few_left, &typed, 1000).and_then(|e| e.rate_limited()), Some(Duration::from_secs(301)));
    // A wait is at least 5 s, as with a clock ahead of GitHub's, and at most an hour.
    assert_eq!(spent(403, &none_left, &Value::Null, 2000).and_then(|e| e.rate_limited()), Some(Duration::from_secs(5)));
    assert_eq!(spent(429, &limits(&[("retry-after", "18446744073709551615")]), &Value::Null, 1000).and_then(|e| e.rate_limited()), Some(Duration::from_secs(3600)));
    assert_eq!(spent(403, &limits(&[("x-ratelimit-remaining", "0"), ("x-ratelimit-reset", "18446744073709551615")]), &Value::Null, 0).and_then(|e| e.rate_limited()), Some(Duration::from_secs(3600)));
    // A 403 with budget left is a plain refusal, and a 200 with none left is a success.
    assert!(spent(403, &limits(&[("x-ratelimit-remaining", "12")]), &Value::Null, 1000).is_none());
    assert!(spent(200, &none_left, &Value::Null, 1000).is_none());
}

#[test]
fn a_resource_that_is_not_a_plain_word_never_reaches_a_log_line() {
    assert_eq!(limits(&[("x-ratelimit-resource", "core; evil")]).resource, None);
}

