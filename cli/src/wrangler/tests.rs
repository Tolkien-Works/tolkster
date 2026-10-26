use super::*;

#[test]
fn workers_dev_is_the_address_deploy_printed() {
    let printed = "Uploaded tolkster (4.1 sec)\nDeployed tolkster triggers (0.9 sec)\n  https://tolkster.example.workers.dev\nCurrent Version ID: 1234\n";
    assert_eq!(workers_dev(printed).as_deref(), Some("https://tolkster.example.workers.dev"));
    assert_eq!(workers_dev("see https://developers.cloudflare.com/workers/, then https://a.b.workers.dev."), Some("https://a.b.workers.dev".into()));
    assert_eq!(workers_dev("Deployed tolkster triggers\n  example.com/*\n"), None);
}

#[test]
fn only_cloudflares_not_found_is_a_worker_absent() {
    assert!(absent("X [ERROR] A request to the Cloudflare API (/accounts/a/workers/scripts/tolkster/secrets) failed.\n  This Worker does not exist on your account. [code: 10007]\n"));
    assert!(!absent("X [ERROR] A request to the Cloudflare API failed.\n  Internal server error [code: 10013]\n"));
    assert!(!absent("fetch failed"));
}

#[test]
fn names_are_the_secrets_listed_and_nothing_else() {
    assert_eq!(names(r#"[{"name":"REALTIME_SECRET","type":"secret_text"},{"name":"OWNER_USER_ID","type":"secret_text"}]"#), Some(vec!["REALTIME_SECRET".into(), "OWNER_USER_ID".into()]));
    assert_eq!(names("[]"), Some(vec![]));
    assert_eq!(names(r#"[{"type":"secret_text"}]"#), None);
    assert_eq!(names("Not logged in"), None);
}
