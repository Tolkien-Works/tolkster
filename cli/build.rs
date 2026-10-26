// Stamps the commit this `tolkster` is built from into it (`prebuilt::commit`): CI sets
// TOLKSTER_COMMIT, and a release build, as is the `local` profile a development `listen`'s
// self-update makes, reads its checkout's HEAD, but only when an input of the binary changed, so a merge
// that changes none leaves the stamp, the bytes and `listen` alone. A debug build is
// unstamped, so a commit doesn't rebuild the crate for every test run. CI also sets
// TOLKSTER_REPO, the repo whose pushes this build follows (`prebuilt::repo`).

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=TOLKSTER_COMMIT");
    println!("cargo:rerun-if-env-changed=TOLKSTER_REPO");
    if let Some(repo) = std::env::var("TOLKSTER_REPO").ok().filter(|r| !r.is_empty()) {
        println!("cargo:rustc-env=TOLKSTER_REPO={repo}");
    }
    let git = |args: &[&str]| Command::new("git").args(args).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let mut commit = std::env::var("TOLKSTER_COMMIT").ok().filter(|c| !c.is_empty());
    if commit.is_none() && std::env::var("PROFILE").is_ok_and(|p| p == "release") {
        // The stamp follows the binary's inputs, not the branch (`voice.json` is compiled in
        // through `include_str!`); the build script's paths are relative to this directory.
        for input in ["src", "briefs", "hooks.json", "Cargo.toml", "build.rs", "../src/shared/voice.json"] {
            println!("cargo:rerun-if-changed={input}");
        }
        commit = git(&["rev-parse", "HEAD"]);
    }
    if let Some(commit) = commit {
        println!("cargo:rustc-env=TOLKSTER_COMMIT={commit}");
    }
}
