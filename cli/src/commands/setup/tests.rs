use serde_json::json;

use super::*;

fn sha(c: char) -> Sha {
    Sha::parse(&c.to_string().repeat(40)).unwrap()
}

#[test]
fn the_worker_deploys_from_a_checkout_at_the_builds_own_commit() {
    let (built, other) = (sha('a'), sha('b'));
    assert_eq!(source(Some(&built), Some(&built), Some("o/r"), false), Source::Here);
    assert_eq!(source(Some(&built), Some(&other), Some("o/r"), false), Source::Clone { repo: "o/r".into(), commit: built.clone() });
    assert_eq!(source(Some(&built), None, Some("o/r"), false), Source::Clone { repo: "o/r".into(), commit: built.clone() });
    // A build with no commit is a debug build, deployed from its checkout alone.
    assert_eq!(source(None, Some(&other), Some("o/r"), false), Source::Here);
    assert_eq!(source(None, None, Some("o/r"), false), Source::Nowhere);
    assert_eq!(source(Some(&built), Some(&other), None, false), Source::Nowhere);
    // A develop build's checkout moves on past its commit, and listen keeps both on main.
    assert_eq!(source(Some(&built), Some(&other), None, true), Source::Here);
    assert_eq!(source(Some(&built), None, None, true), Source::Nowhere);
}

#[test]
fn an_agent_name_is_one_linear_takes() {
    assert_eq!(agent_name("  Tolkster ").unwrap(), "Tolkster");
    for bad in ["", "T", &"x".repeat(81), "Linear Bot", "my linear", "see https://x.y", "a\u{1b}[2Jb"] {
        assert!(agent_name(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn only_the_secrets_the_worker_lacks_are_set() {
    let listed = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    let all = listed(&["REALTIME_SECRET", "LINEAR_CLIENT_ID", "LINEAR_CLIENT_SECRET", "LINEAR_WEBHOOK_SECRET", "CLAUDE_TOKEN"]);
    let held = json!({ "clientId": "id", "clientSecret": "secret", "realtimeSecret": "r" });
    assert_eq!(wanted(&all, &held), vec![]);

    let names = |wanted: Vec<(&'static str, Origin)>| wanted.into_iter().map(|(name, _)| name).collect::<Vec<_>>();
    assert_eq!(names(wanted(&[], &held)), ["REALTIME_SECRET", "LINEAR_CLIENT_ID", "LINEAR_CLIENT_SECRET", "LINEAR_WEBHOOK_SECRET", "CLAUDE_TOKEN"]);
    assert_eq!(wanted(&[], &held)[0], ("REALTIME_SECRET", Origin::Credentials("realtimeSecret")));
    // The client's own pair is typed into credentials.json first, never asked for here.
    assert_eq!(names(wanted(&listed(&["REALTIME_SECRET"]), &json!({ "realtimeSecret": "r" }))), ["LINEAR_WEBHOOK_SECRET", "CLAUDE_TOKEN"]);
}

#[test]
fn a_realtime_secret_the_cli_lacks_is_made_afresh() {
    // Set on the Worker but not kept here: its value can't be read back.
    assert_eq!(wanted(&["REALTIME_SECRET".into()], &json!({})), vec![
        ("REALTIME_SECRET", Origin::Made),
        ("LINEAR_WEBHOOK_SECRET", Origin::Typed("The Linear app's webhook signing secret", false)),
        ("CLAUDE_TOKEN", Origin::Typed("A Claude token for worker hosts, from claude setup-token (Enter to skip)", true)),
    ]);
    assert_eq!(wanted(&[], &json!({ "realtimeSecret": "" }))[0], ("REALTIME_SECRET", Origin::Made));
}

#[test]
fn the_owner_proposed_is_the_one_admin() {
    let user = |admin: &bool| crate::json::decode(json!({ "id": "u", "name": "Sam", "displayName": "sam", "active": true, "admin": admin, "guest": false }), "Linear's answer to Users").unwrap();
    let people = |admins: &[bool]| admins.iter().map(user).collect::<Vec<_>>();
    assert_eq!(proposal(&people(&[false, true, false])), Some(1));
    assert_eq!(proposal(&people(&[true, true])), None);
    assert_eq!(proposal(&people(&[false])), None);
}

#[test]
fn node_major_reads_nodes_version() {
    assert_eq!(node_major("v24.1.0\n"), Some(24));
    assert_eq!(node_major("v22.12.0"), Some(22));
    assert_eq!(node_major("24.1.0"), None);
}

#[test]
fn accounts_are_named_with_their_ids() {
    let whoami = json!({ "loggedIn": true, "accounts": [{ "id": "a1", "name": "Works" }, { "id": "a2" }] });
    assert_eq!(accounts(&whoami), ["Works (a1)"]);
    assert_eq!(accounts(&json!({ "loggedIn": true })), Vec::<String>::new());
}
