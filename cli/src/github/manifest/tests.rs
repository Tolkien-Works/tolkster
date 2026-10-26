use serde_json::json;

use super::*;
use crate::coauthor::hook_command;

#[test]
fn the_manifest_asks_for_every_event_the_worker_takes_and_the_documented_permissions() {
    let manifest = manifest("Owl", "https://w.example", "https://w.example/webhooks/github", "http://127.0.0.1:9/created");
    assert_eq!(manifest["default_events"], json!(GITHUB_EVENTS));
    assert_eq!(manifest["hook_attributes"], json!({ "url": "https://w.example/webhooks/github", "active": true }));
    assert_eq!(manifest["public"], false);
    assert_eq!(
        manifest["default_permissions"],
        json!({ "pull_requests": "write", "contents": "write", "checks": "read", "statuses": "read", "actions": "read", "issues": "read", "metadata": "read" })
    );
}

#[test]
fn a_new_app_is_taken_only_whole_and_with_a_plain_slug() {
    let app = json!({ "id": 5, "slug": "owl-agent", "owner": { "login": "acme" }, "pem": "-----BEGIN RSA PRIVATE KEY-----", "webhook_secret": "w" });
    let Ok(made) = created(app.clone()) else { panic!("whole") };
    assert_eq!((made.id, made.slug.as_str(), made.owner.as_str()), (5, "owl-agent", "acme"));
    for (key, value) in [("slug", json!("Owl/../x")), ("pem", json!("")), ("webhook_secret", json!(null)), ("id", json!("5")), ("owner", json!(null))] {
        let mut app = app.clone();
        app[key] = value;
        assert!(created(app).is_err(), "{key}");
    }
    // Missing, a field is named, never a value: the answer holds the key.
    let mut keyless = app.clone();
    keyless.as_object_mut().unwrap().remove("pem");
    let Err(error) = created(keyless) else { panic!("keyless") };
    assert_eq!(error.to_string(), "GitHub's answer to POST /app-manifests/{code}/conversions didn't decode: it had no `pem`");
}

#[test]
fn the_bot_user_is_read_by_its_id() {
    assert_eq!(user_id(r#"{ "login": "owl-agent[bot]", "id": 42, "type": "Bot" }"#), Ok(42));
    assert_eq!(user_id("not json").unwrap_err().to_string(), "GitHub's answer to GET /users/{slug}[bot] didn't decode: a field had another shape");
}

#[test]
fn a_code_or_slug_that_could_leave_its_url_path_is_refused() {
    assert!(plain_code("0f3a_b-C9"));
    assert!(!plain_code("") && !plain_code("a/b") && !plain_code("a?b") && !plain_code(&"a".repeat(101)));
    assert!(plain_slug("owl-agent2"));
    assert!(!plain_slug("Owl") && !plain_slug("-owl") && !plain_slug("owl[bot]") && !plain_slug(""));
}

#[test]
fn the_co_author_is_the_bots_noreply_address_and_a_hook_takes_it() {
    let line = co_author("Owl", 42, "owl-agent");
    assert_eq!(line, "Owl <42+owl-agent[bot]@users.noreply.github.com>");
    assert!(hook_command(&line).is_ok());
    assert!(credits(&line, 42, "owl-agent"));
    // A slug a deleted app freed is another bot's.
    assert!(!credits(&line, 43, "owl-agent") && !credits(&line, 42, "agent") && !credits("Owl <42+owl-agent-2[bot]@users.noreply.github.com>", 42, "owl-agent"));
}
