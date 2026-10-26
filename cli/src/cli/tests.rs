use super::*;

// A module's unit tests live in a file of their own, so reading the module never
// loads them (TW-252, and AGENTS.md).
#[test]
fn no_module_keeps_its_tests_inline() {
    // Split, so this file doesn't match itself.
    const INLINE: [&str; 2] = ["mod tests", " {"];
    fn walk(dir: &Path, inline: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, inline);
            } else if path.extension().is_some_and(|e| e == "rs") && std::fs::read_to_string(&path).unwrap().contains(&INLINE.concat()) {
                inline.push(path.display().to_string());
            }
        }
    }
    let mut inline = Vec::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut inline);
    assert!(inline.is_empty(), "tests inline in {inline:?}");
}

// The one writer of the settings files keeps every key it isn't given, unknown ones included,
// removes one given as null, and leaves an unchanged file unwritten.
#[test]
fn merging_settings_keeps_every_other_key() {
    let dir = std::env::temp_dir().join(format!("tolkster-merge-{}", std::process::id()));
    let file = dir.join("config.json");
    let _ = fs::remove_dir_all(&dir);
    let set = |entries: Value| merge_json_file(&file, true, |_| entries.as_object().unwrap().clone()).unwrap();
    assert!(set(serde_json::json!({ "hosts": ["wsl"], "someday": 1 })));
    assert!(set(serde_json::json!({ "hosts": ["wsl", "mac"], "someday": null })));
    let written = fs::read(&file).unwrap();
    assert!(!set(serde_json::json!({ "hosts": ["wsl", "mac"] })));
    assert_eq!(fs::read(&file).unwrap(), written);
    assert_eq!(serde_json::from_slice::<Value>(&written).unwrap(), serde_json::json!({ "hosts": ["wsl", "mac"] }));
    fs::write(&file, "[]").unwrap();
    assert!(merge_json_file(&file, false, |_| serde_json::Map::new()).is_err());
    let _ = fs::remove_dir_all(&dir);
}
