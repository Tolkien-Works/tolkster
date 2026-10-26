use super::*;
use serde_json::json;

#[test]
fn a_rejected_token_is_retried_until_a_new_one_is_rejected() {
    let none: [String; 0] = [];
    let one = ["held".to_string()];
    let two = ["held".to_string(), "from-file".to_string()];
    // The first rejection, of whichever token.
    assert!(retries(&none, false));
    assert!(retries(&none, true));
    // token.json's, rejected too: mint.
    assert!(retries(&one, false));
    // A new token rejected: Linear means it.
    assert!(!retries(&one, true));
    assert!(!retries(&two, true));
    assert!(!retries(&two, false));
}

#[test]
fn a_token_within_a_day_of_expiry_is_not_used() {
    let now = 1_790_000_000_000.0;
    let token = |expires_at| AppToken { access_token: "t".into(), expires_at };
    assert!(!is_usable(None, now));
    assert!(is_usable(Some(&token(now + DAY_MS + 1.0)), now));
    assert!(!is_usable(Some(&token(now + DAY_MS)), now));
}

#[test]
fn the_token_cache_round_trips() {
    let written = r#"{"accessToken":"lin_oauth_x","expiresAt":1792592000123}"#;
    let token = AppToken::from_json(written).unwrap();
    assert_eq!(token, AppToken { access_token: "lin_oauth_x".into(), expires_at: 1_792_592_000_123.0 });
    assert_eq!(AppToken::from_json(&token.to_json()), Some(token));
    assert_eq!(AppToken::from_json("{\"accessToken\":"), None);
    assert_eq!(AppToken::from_json("null"), None);
}

#[test]
fn the_token_grant_decodes_and_one_without_its_lifetime_fails_without_the_token() {
    let granted: Granted = decode(json!({ "access_token": "lin_oauth_secret", "token_type": "Bearer", "expires_in": 2591999, "scope": SCOPES }), "Linear's answer to POST /oauth/token").unwrap();
    assert_eq!((granted.access_token.as_str(), granted.expires_in), ("lin_oauth_secret", 2_591_999.0));
    let error = decode::<Granted>(json!({ "access_token": "lin_oauth_secret" }), "Linear's answer to POST /oauth/token").unwrap_err().to_string();
    assert_eq!(error, "Linear's answer to POST /oauth/token didn't decode: it had no `expires_in`");
    let refused: Refused = serde_json::from_str(r#"{ "error": "invalid_client", "error_description": "Client authentication failed" }"#).unwrap();
    assert_eq!(refused.error_description.as_deref(), Some("Client authentication failed"));
}
