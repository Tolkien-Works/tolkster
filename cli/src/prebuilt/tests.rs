use std::path::PathBuf;

use super::*;
use crate::exec::Output;

fn sha() -> Sha {
    Sha::parse(&"c3".repeat(20)).unwrap()
}

// A folder of its own, with a stand-in for the installed binary and a tarball holding a
// `tolkster` the way CI packs one.
fn folder(test: &str) -> (PathBuf, PathBuf, Vec<u8>) {
    let root = std::env::temp_dir().join(format!("tolkster-prebuilt-{}-{test}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let (bin, src) = (root.join("bin"), root.join("src"));
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&src).unwrap();
    let name = if cfg!(windows) { "tolkster.exe" } else { "tolkster" };
    fs::write(src.join(name), "new build").unwrap();
    fs::write(bin.join(name), "old build").unwrap();
    let archive = root.join("build.tar.gz");
    assert!(run_command("tar", &["-czf", &archive.display().to_string(), "-C", &src.display().to_string(), name], None).unwrap().ok);
    (bin.join(name), root.clone(), fs::read(archive).unwrap())
}

// Real `tar`, and a binary that says what `version` is told it says.
fn running(version: String) -> impl FnMut(&str, &[&str], Option<&str>) -> Result<Output, Error> {
    move |program, args, dir| {
        if program == "tar" {
            return run_command(program, args, dir);
        }
        assert_eq!(args, ["version"]);
        Ok(Output::ok(version.clone()))
    }
}

// The swap is a rename over the binary, after the build says it is the commit asked for: a
// build that says another, or isn't a tarball, leaves the binary as it was, and nothing is
// left beside it but the renamed-aside exe a Windows swap leaves for `clear_old`.
#[test]
fn a_build_replaces_the_binary_only_when_it_is_the_commit_asked_for() {
    let (exe, root, tarball) = folder("swap");
    let bin = exe.parent().unwrap();
    let left = || fs::read_dir(bin).unwrap().count();
    swap_in(&exe, &tarball, &sha(), &mut running(format!("0.1.0 ({})", "b2".repeat(20)))).unwrap_err();
    assert_eq!((fs::read_to_string(&exe).unwrap(), left()), ("old build".into(), 1));
    swap_in(&exe, b"not a tarball", &sha(), &mut running(String::new())).unwrap_err();
    assert_eq!((fs::read_to_string(&exe).unwrap(), left()), ("old build".into(), 1));
    swap_in(&exe, &tarball, &sha(), &mut running(format!("0.1.0 ({})", sha()))).unwrap();
    assert_eq!((fs::read_to_string(&exe).unwrap(), left()), ("new build".into(), if cfg!(windows) { 2 } else { 1 }));
    let _ = fs::remove_dir_all(root);
}

// The listen host builds its own: without a credential nothing is asked of the Worker, which
// the agent here could not reach.
#[test]
fn a_host_without_a_credential_is_never_swapped() {
    let files = Files { config: serde_json::Value::Null, credentials: serde_json::json!({ "realtimeSecret": "r" }) };
    assert!(level(&crate::http::agent(), &files, &sha(), &[]).is_none());
}

// The names a build is asked for by are those CI gives.
#[test]
fn the_targets_are_those_of_the_contract() {
    let mut named = TARGETS.to_vec();
    named.sort_unstable();
    assert_eq!(serde_json::to_value(named).unwrap(), crate::state::tests::contract()["buildTargets"]);
    assert!(target().is_none_or(|own| TARGETS.contains(&own)));
}
