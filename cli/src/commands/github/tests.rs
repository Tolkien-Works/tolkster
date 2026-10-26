use serde_json::json;

use super::*;

#[test]
fn only_githubs_return_with_this_runs_state_and_a_plain_code_is_exchanged() {
    assert_eq!(route("/", "s1"), Route::Page);
    assert_eq!(route("/created?code=abc123&state=s1", "s1"), Route::Created("abc123".into()));
    assert_eq!(route("/created?state=s1&code=abc123", "s1"), Route::Created("abc123".into()));
    assert_eq!(route("/created?code=abc123&state=s2", "s1"), Route::Refused);
    assert_eq!(route("/created?code=abc123", "s1"), Route::Refused);
    assert_eq!(route("/created?code=../../user&state=s1", "s1"), Route::Refused);
    assert_eq!(route("/created?code=a%2Fb&state=s1", "s1"), Route::Refused);
    assert_eq!(route("/favicon.ico", "s1"), Route::NotFound);
}

#[test]
fn a_request_is_read_with_the_host_it_names() {
    let parsed = parse_request("GET /created?code=a HTTP/1.1\r\nUser-Agent: x\r\nhost: evil.example:51000\r\n\r\n");
    assert_eq!(parsed, Some(("/created?code=a".into(), "evil.example:51000".into())));
    assert_eq!(parse_request("GET / HTTP/1.1\r\n\r\n"), None);
    assert_eq!(parse_request("POST / HTTP/1.1\r\nHost: 127.0.0.1:9\r\n\r\n"), None);
}

#[test]
fn the_page_posts_the_manifest_escaped_and_only_on_its_first_load() {
    let manifest = manifest("Owl \"<x>\"", "https://w.example", "https://w.example/webhooks/github", "http://127.0.0.1:9/created");
    let page_once = page(&manifest, &new_app_url(Some("acme"), "s1"), true);
    assert!(page_once.contains(r#"action="https://github.com/organizations/acme/settings/apps/new?state=s1""#));
    assert!(!page_once.contains("<x>"));
    assert!(page_once.contains("submit()"));
    assert!(!page(&manifest, "https://github.com/settings/apps/new?state=s1", false).contains("submit()"));
}

#[test]
fn an_app_under_another_account_than_org_is_refused() {
    let app = || Created { id: 1, slug: "owl".into(), owner: "Acme".into(), key: "k".into(), webhook_secret: "w".into() };
    assert!(under(app(), Some("acme")).is_ok());
    assert!(under(app(), None).is_ok());
    assert!(under(app(), Some("other")).is_err());
}

#[test]
fn the_installation_is_read_from_the_workers_record_and_an_older_worker_is_said() {
    let read = |record: serde_json::Value| -> AppRecord { crate::json::decode(record, "the Worker's answer to githubApp").unwrap() };
    let record = read(json!({ "events": ["pull_request"], "slug": "owl", "installations": [7], "installed": false }));
    assert_eq!(installations(&record), Ok(vec![7]));
    assert_eq!(installed(&record), Ok(false));
    assert_eq!(record_slug(&record), Ok("owl".into()));
    let older = read(json!({ "events": [], "slug": "owl" }));
    assert!(installations(&older).is_err() && installed(&older).is_err());
}
