use std::path::{Path, PathBuf};

use super::*;
use crate::exec::Output;

fn sh(dir: &Path, args: &[&str]) -> String {
    let output = run_command("git", args, Some(dir.to_str().unwrap())).unwrap();
    assert!(output.ok, "git {}: {}", args.join(" "), output.stderr);
    output.stdout.trim().to_string()
}

fn commit(dir: &Path, file: &str, text: &str, message: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    sh(dir, &["add", file]);
    sh(dir, &["commit", "-q", "-m", message]);
}

// A bare origin whose `main` has one commit, a worker's clone on `tolkster/tw-1`, and another
// clone of `main`, standing in for every other landing.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new(test: &str) -> Scratch {
        let root = std::env::temp_dir().join(format!("tolkster-land-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let scratch = Scratch { root };
        sh(&scratch.root, &["init", "-q", "--bare", "-b", "main", "origin.git"]);
        for clone in ["other", "worker"] {
            sh(&scratch.root, &["clone", "-q", "origin.git", clone]);
            for (key, value) in [("user.name", "Tester"), ("user.email", "t@example.com"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
                sh(&scratch.dir(clone), &["config", key, value]);
            }
            if clone == "other" {
                commit(&scratch.dir(clone), "a.txt", "one\n", "init");
                sh(&scratch.dir(clone), &["push", "-q", "origin", "main"]);
            }
        }
        sh(&scratch.dir("worker"), &["fetch", "-q", "origin"]);
        sh(&scratch.dir("worker"), &["remote", "set-head", "origin", "main"]);
        sh(&scratch.dir("worker"), &["switch", "-q", "-c", "tolkster/tw-1", "origin/main"]);
        scratch
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    fn worker(&self) -> String {
        self.dir("worker").to_str().unwrap().to_string()
    }

    // `main` as origin has it, newest first: each commit's subject.
    fn main(&self) -> Vec<String> {
        sh(&self.dir("origin.git"), &["log", "--format=%s", "main"]).lines().map(String::from).collect()
    }

    // Another landing on `main`, made from the other clone.
    fn land_other(&self, file: &str, text: &str) {
        let other = self.dir("other");
        sh(&other, &["pull", "-q", "--ff-only", "origin", "main"]);
        commit(&other, file, text, &format!("Other {file}"));
        sh(&other, &["push", "-q", "origin", "main"]);
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn key() -> IssueKey {
    IssueKey::parse("TW-1").unwrap()
}

fn branch() -> BranchName {
    BranchName::parse("tolkster/tw-1").unwrap()
}

fn land(scratch: &Scratch, pushes: &mut u32, before_push: &mut dyn FnMut()) -> Landing {
    land_on(scratch, &branch(), None, pushes, before_push).unwrap()
}

// Real git, with each push counted and `before_push` run before the first.
fn land_on(scratch: &Scratch, branch: &BranchName, stop: Option<&str>, pushes: &mut u32, before_push: &mut dyn FnMut()) -> Result<Landing, Error> {
    let mut run = |program: &str, args: &[&str], dir: Option<&str>| -> Result<Output, Error> {
        if args.contains(&"push") {
            if *pushes == 0 {
                before_push();
            }
            *pushes += 1;
        }
        run_command(program, args, dir)
    };
    land_branch(&mut run, &scratch.worker(), branch, &key(), "Add b (TW-1)", "# Why\n\nBecause.", stop)
}

#[test]
fn a_branch_lands_as_one_commit_and_a_run_again_pushes_nothing() {
    let scratch = Scratch::new("lands");
    let worker = scratch.dir("worker");
    commit(&worker, "b.txt", "b\n", "wip 1");
    commit(&worker, "c.txt", "c\n", "wip 2");
    let mut pushes = 0;
    let Landing::Landed { sha, pushed: true } = land(&scratch, &mut pushes, &mut || {}) else { panic!("not landed") };
    assert_eq!(scratch.main(), ["Add b (TW-1)", "init"]);
    assert_eq!(sh(&scratch.dir("origin.git"), &["rev-parse", "main"]), sha.to_string());
    // The body as written, a markdown heading's `#` included.
    assert_eq!(sh(&worker, &["log", "-1", "--format=%b"]), "# Why\n\nBecause.");
    assert_eq!(pushes, 1);
    // A finished issue still prints its landing.
    let again = land_on(&scratch, &branch(), Some("finished"), &mut pushes, &mut || {}).unwrap();
    assert_eq!(again, Landing::Landed { sha, pushed: false });
    assert_eq!(pushes, 1);
    // A new branch of the issue, made at that landing, didn't land it.
    sh(&worker, &["switch", "-q", "-c", "tolkster/tw-1-2", "origin/main"]);
    assert_eq!(land_on(&scratch, &BranchName::parse("tolkster/tw-1-2").unwrap(), None, &mut pushes, &mut || {}).unwrap(), Landing::Nothing);
}

#[test]
fn a_finished_issue_pushes_nothing() {
    let scratch = Scratch::new("finished");
    commit(&scratch.dir("worker"), "b.txt", "b\n", "wip");
    let mut pushes = 0;
    assert!(matches!(land_on(&scratch, &branch(), Some("finished"), &mut pushes, &mut || {}), Err(Error::Usage(_))));
    assert_eq!((pushes, scratch.main()), (0, vec!["init".to_string()]));
}

#[test]
fn a_change_already_on_main_has_nothing_to_land() {
    // The same change landed meanwhile, so the rebase drops it.
    let scratch = Scratch::new("same");
    commit(&scratch.dir("worker"), "b.txt", "b\n", "wip");
    let mut pushes = 0;
    assert_eq!(land(&scratch, &mut pushes, &mut || scratch.land_other("b.txt", "b\n")), Landing::Nothing);
    // Commits that cancel out.
    let scratch = Scratch::new("undone");
    let worker = scratch.dir("worker");
    commit(&worker, "b.txt", "b\n", "wip");
    sh(&worker, &["rm", "-q", "b.txt"]);
    sh(&worker, &["commit", "-q", "-m", "undo"]);
    assert_eq!(land(&scratch, &mut 0, &mut || {}), Landing::Nothing);
}

#[test]
fn origins_copy_of_the_branch_is_deleted_and_one_already_gone_is_fine() {
    let scratch = Scratch::new("delete");
    let worker = scratch.dir("worker");
    for other in ["tolkster/tw-1-2", "tolkster/tw-12"] {
        sh(&worker, &["push", "-q", "origin", &format!("HEAD:refs/heads/{other}")]);
    }
    sh(&worker, &["push", "-q", "origin", "tolkster/tw-1"]);
    sh(&worker, &["fetch", "-q", "origin"]);
    let mut found = git::issue_branches(&mut run_command, &scratch.worker(), &key()).unwrap();
    found.sort();
    assert_eq!(found, [BranchName::parse("tolkster/tw-1").unwrap(), BranchName::parse("tolkster/tw-1-2").unwrap()]);
    for _ in 0..2 {
        git::delete_remote_branch(&mut run_command, &scratch.worker(), &branch()).unwrap();
    }
    assert_eq!(sh(&scratch.dir("origin.git"), &["branch", "--list", "tolkster/tw-1"]), "");
}

#[test]
fn a_push_refused_as_main_moved_on_is_rebased_and_made_again() {
    let scratch = Scratch::new("retry");
    commit(&scratch.dir("worker"), "b.txt", "b\n", "wip");
    let mut pushes = 0;
    let Landing::Landed { sha, pushed: true } = land(&scratch, &mut pushes, &mut || scratch.land_other("d.txt", "d\n")) else { panic!("not landed") };
    assert_eq!(pushes, 2);
    assert_eq!(scratch.main(), ["Add b (TW-1)", "Other d.txt", "init"]);
    assert_eq!(sh(&scratch.dir("origin.git"), &["rev-parse", "main"]), sha.to_string());
    // The rebase made it here, so a run again finds it landed.
    assert_eq!(land(&scratch, &mut pushes, &mut || {}), Landing::Landed { sha, pushed: false });
}

#[test]
fn a_conflict_names_its_paths_and_pushes_nothing() {
    let scratch = Scratch::new("conflict");
    commit(&scratch.dir("worker"), "a.txt", "mine\n", "wip");
    scratch.land_other("a.txt", "theirs\n");
    let mut pushes = 0;
    assert_eq!(land(&scratch, &mut pushes, &mut || {}), Landing::Conflicts(vec!["a.txt".into()]));
    assert_eq!(pushes, 0);
    assert_eq!(scratch.main(), ["Other a.txt", "init"]);
    // The rebase is aborted: the branch is left squashed, with no rebase under way.
    assert_eq!(sh(&scratch.dir("worker"), &["status", "--porcelain"]), "");
    assert_eq!(sh(&scratch.dir("worker"), &["log", "-1", "--format=%s"]), "Add b (TW-1)");
}

#[test]
fn a_branch_with_no_commit_of_its_own_has_nothing_to_land() {
    let scratch = Scratch::new("nothing");
    let mut pushes = 0;
    assert_eq!(land(&scratch, &mut pushes, &mut || {}), Landing::Nothing);
    assert_eq!(pushes, 0);
}

#[test]
fn land_runs_only_in_a_direct_repo_in_the_issues_own_worktree_of_an_issue_not_merged_by_hand() {
    let refused = |land, linked, manual, name: Option<&str>| refusal(Here { land, linked }, manual, name.and_then(BranchName::parse), &key()).is_err();
    assert!(!refused(Land::Direct, true, false, Some("tolkster/tw-1")));
    assert!(!refused(Land::Direct, true, false, Some("tolkster/TW-1-2")));
    assert!(refused(Land::Pr, true, false, Some("tolkster/tw-1")));
    assert!(refused(Land::Direct, true, true, Some("tolkster/tw-1")));
    // A checkout Orca doesn't link to the issue, or the primary one, on a branch named for it.
    assert!(refused(Land::Direct, false, false, Some("tolkster/tw-1")));
    for other in [Some("tolkster/tw-12"), Some("tolkster/tw-1-x"), Some("main"), None] {
        assert!(refused(Land::Direct, true, false, other), "{other:?}");
    }
}
