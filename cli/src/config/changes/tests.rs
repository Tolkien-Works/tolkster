use super::*;
use serde_json::json;
use std::path::Path;

use crate::config::{Files, from_files};

fn read(config: Value) -> Result<Config, Error> {
    from_files(&Files { config, credentials: Value::Null }, "owner-1".into(), Path::new("/home/someone"), None)
}

fn config(config: Value) -> Config {
    read(config).unwrap()
}

fn changed(said: Option<Work>) -> Vec<String> {
    match said {
        Some(Work::ConfigChanged { changes }) => changes,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_change_is_said_once_by_its_key_and_value() {
    let before = config(json!({ "repos": { "tolkster": { "projects": ["P"] } } }));
    let mut reread = Reread::new(&before, Some("https://w.dev"));
    assert_eq!(reread.after(Ok((&before, Some("https://w.dev")))), None);
    let after = config(json!({ "repos": { "tolkster": { "projects": ["P"], "trace": true } }, "slots": { "local": 2 } }));
    assert_eq!(changed(reread.after(Ok((&after, Some("https://w.dev"))))), ["repos.tolkster.trace = true", "slots.local = 2"]);
    assert_eq!(reread.after(Ok((&after, Some("https://w.dev")))), None, "the next listing finds it said");
}

#[test]
fn a_string_is_named_never_shown_and_a_new_repo_names_its_keys() {
    let before = config(json!({}));
    let mut reread = Reread::new(&before, None);
    let after = config(json!({ "owner": { "name": "Someone Secret" }, "repos": { "r": { "trace": true } } }));
    let changes = changed(reread.after(Ok((&after, None))));
    assert!(changes.contains(&"owner.name changed".to_string()), "{changes:?}");
    assert!(changes.contains(&"repos.r added".to_string()), "{changes:?}");
    assert!(changes.iter().all(|line| !line.contains("Secret")), "{changes:?}");
    let gone = config(json!({}));
    assert!(changed(reread.after(Ok((&gone, None)))).contains(&"repos.r removed".to_string()));
}

#[test]
fn a_long_change_names_the_first_few_and_counts_the_rest() {
    let before = config(json!({}));
    let mut reread = Reread::new(&before, None);
    let repos: serde_json::Map<String, Value> = (0..9).map(|n| (format!("r{n}"), json!({ "trace": true }))).collect();
    let changes = changed(reread.after(Ok((&config(json!({ "repos": repos })), None))));
    assert_eq!((changes.len(), changes.last().map(String::as_str)), (NAMED + 1, Some("3 more")));
}

#[test]
fn the_worker_is_said_to_need_a_restart() {
    let settings = config(json!({}));
    let mut reread = Reread::new(&settings, Some("https://a.dev"));
    assert_eq!(changed(reread.after(Ok((&settings, Some("https://b.dev"))))), ["worker changed, read at startup only"]);
}

#[test]
fn a_failed_read_is_said_once_until_it_reads_or_fails_another_way() {
    let settings = config(json!({ "timing": false }));
    let mut reread = Reread::new(&settings, None);
    let bad = read(json!({ "slots": 1 })).unwrap_err();
    assert!(matches!(reread.after(Err(&bad)), Some(Work::ConfigKept(_))));
    assert_eq!(reread.after(Err(&bad)), None);
    let other = read(json!({ "timing": 1 })).unwrap_err();
    assert!(matches!(reread.after(Err(&other)), Some(Work::ConfigKept(_))), "a different failure is a new line");
    // Reading again with nothing changed says nothing, and the same failure later is said again.
    assert_eq!(reread.after(Ok((&settings, None))), None);
    assert!(matches!(reread.after(Err(&bad)), Some(Work::ConfigKept(_))));
}

#[test]
fn a_failed_read_leaves_the_last_config_as_the_one_compared() {
    let before = config(json!({}));
    let mut reread = Reread::new(&before, None);
    let bad = read(json!({ "slots": 1 })).unwrap_err();
    reread.after(Err(&bad));
    let after = config(json!({ "timing": false }));
    assert_eq!(changed(reread.after(Ok((&after, None)))), ["timing = false"]);
}
