use serde_json::{Value, json};

use super::*;

fn files(credentials: Value) -> Files {
    Files { config: Value::Null, credentials }
}

// The `listen` host holds its own client secret and Claude login: without a host credential
// nothing is traded, and the Worker isn't asked.
#[test]
fn only_a_host_with_a_credential_trades() {
    let agent = crate::http::agent();
    let listen = files(json!({ "clientId": "id", "clientSecret": "secret", "realtimeSecret": "r" }));
    assert!(linear(&agent, &listen, None).unwrap().is_none());
    assert!(claude(&agent, &listen).unwrap().is_none());
}

#[test]
fn a_refusal_says_why_and_never_echoes_the_body() {
    let token = json!({ "accessToken": "lin_secret", "expiresAt": 1.0 }).to_string();
    assert_eq!(answer("linear", 200, &token).unwrap()["accessToken"], "lin_secret");
    let refused = answer("claude", 401, "Unauthorized").unwrap_err().to_string();
    assert!(refused.contains("refused this host's credential"), "{refused}");
    let missing = answer("claude", 503, r#"{"missing":["CLAUDE_TOKEN"]}"#).unwrap_err().to_string();
    assert!(missing.contains("no CLAUDE_TOKEN set"), "{missing}");
    // GitHub's mint refused, in the Worker's own words.
    let refused = answer("github", 422, r#"{"refused":"the GitHub App's token does not cover other/tolkster"}"#).unwrap_err().to_string();
    assert!(refused.ends_with("does not cover other/tolkster"), "{refused}");
    // A mint that may pass is unavailable, and a spent budget carries GitHub's wait.
    assert!(matches!(answer("github", 502, r#"{"refused":"GitHub answered HTTP 500 for an installation token"}"#), Err(Error::Unavailable(_))));
    assert!(matches!(answer("github", 500, "Internal Server Error"), Err(Error::Unavailable(_))));
    let spent = answer("github", 429, r#"{"refused":"GitHub's budget is spent","retryAfter":30}"#).unwrap_err();
    assert_eq!(spent.rate_limited(), Some(crate::gateway::wait(Some(30))));
    assert!(matches!(answer("github", 503, r#"{"missing":["GITHUB_APP_KEY"]}"#), Err(Error::Failed(_))));
    for (status, body) in [(500, token.as_str()), (200, "lin_secret")] {
        let error = answer("linear", status, body).unwrap_err().to_string();
        assert!(!error.contains("lin_secret"), "{error}");
    }
}
