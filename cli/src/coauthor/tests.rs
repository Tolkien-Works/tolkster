use super::*;
use std::path::PathBuf;
use std::process::Command;
use crate::exec::run_command;

const CO_AUTHOR: &str = "Tolkster <333968625+tolkster-agent[bot]@users.noreply.github.com>";
const TRAILER: &str = "Co-authored-by: Tolkster <333968625+tolkster-agent[bot]@users.noreply.github.com>";

#[test]
fn a_co_author_must_be_a_name_and_an_email_on_one_line() {
    assert!(hook_command(CO_AUTHOR).unwrap().contains(TRAILER));
    for bad in ["Tolkster", "<a@b>", "Tolkster <a@b", "Tolk'ster <a@b>", "Tolkster <a@b>\nX: y", "Tolkster <a<b>", "Tolkster <>"] {
        assert!(matches!(hook_command(bad), Err(Error::Usage(_))), "{bad}");
    }
}

// Real git, in a scratch repo of each test's own, so libtest runs them alongside each other:
// the rules that matter are git's behaviour.
struct Scratch {
    root: PathBuf,
    main: PathBuf,
}

impl Scratch {
    // A repo, with a worktree of each name beside its primary checkout. Its own
    // prepare-commit-msg, through core.hooksPath as a repo may have it, leaves the message
    // alone and notes in the worktree's git dir that it ran.
    fn new(test: &str, worktrees: &[&str]) -> Scratch {
        let root = std::env::temp_dir().join(format!("tolkster-coauthor-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let main = root.join("main");
        std::fs::create_dir_all(&main).unwrap();
        let scratch = Scratch { root, main };
        sh(&scratch.main, &["init", "-q", "-b", "main"]);
        // Before the repo's hook is set: a worktree needs a commit to branch from, not the hook's run.
        commit(&scratch.main, "init");
        let hooks = scratch.root.join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(hooks.join("prepare-commit-msg"), "#!/bin/sh\necho ran > \"$(git rev-parse --git-dir)/repo-hook-ran\"\n").unwrap();
        #[cfg(unix)]
        std::fs::set_permissions(hooks.join("prepare-commit-msg"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        sh(&scratch.main, &["config", "core.hooksPath", hooks.to_str().unwrap()]);
        for name in worktrees {
            sh(&scratch.main, &["worktree", "add", "-q", "-b", name, scratch.dir(name).to_str().unwrap()]);
        }
        scratch
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn sh(dir: &Path, args: &[&str]) -> String {
    let output = run_command("git", args, Some(dir.to_str().unwrap())).unwrap();
    assert!(output.ok, "git {}: {}", args.join(" "), output.stderr);
    output.stdout
}

fn message(dir: &Path) -> String {
    sh(dir, &["log", "-1", "--format=%B"])
}

fn git_dir(dir: &Path) -> PathBuf {
    PathBuf::from(sh(dir, &["rev-parse", "--path-format=absolute", "--git-dir"]).trim())
}

// Whether the repo's own hook ran since this was last asked.
fn repo_hook_ran(dir: &Path) -> bool {
    std::fs::remove_file(git_dir(dir).join("repo-hook-ran")).is_ok()
}

fn commit(dir: &Path, text: &str) {
    sh(dir, &["-c", "user.name=T", "-c", "user.email=t@example.com", "commit", "-q", "--allow-empty", "-m", text]);
}

// False when this git doesn't run hooks from config, and the test that asked is skipped.
fn mark_co_author(dir: &Path, session_id: &str) -> bool {
    match mark(&mut run_command, dir.to_str().unwrap(), session_id, Some(CO_AUTHOR)) {
        Err(Error::Failed(m)) if m.contains("update Git") => {
            eprintln!("skipped: this git doesn't run hooks from config");
            false
        }
        other => {
            other.unwrap();
            true
        }
    }
}

#[test]
fn the_primary_checkout_is_never_marked() {
    let scratch = Scratch::new("primary", &[]);
    let primary = mark(&mut run_command, scratch.main.to_str().unwrap(), "session-1", Some(CO_AUTHOR));
    assert!(matches!(primary, Err(Error::Failed(ref m)) if m.contains("primary checkout")), "{primary:?}");
}

#[test]
fn a_mark_is_read_back_from_its_worktree_alone_and_marking_again_changes_nothing() {
    let scratch = Scratch::new("read-back", &["marked", "unmarked"]);
    let marked = scratch.dir("marked");
    if !mark_co_author(&marked, "session-1") {
        return;
    }
    // Marking again changes nothing.
    assert!(mark_co_author(&marked, "session-1"));
    assert_eq!(std::fs::read_to_string(git_dir(&marked).join(MARKER)).unwrap(), "session-1\n");
    // The mark is read back from the worktree and from a directory inside it, and from
    // nowhere else: not the primary checkout, another worktree or a directory outside a repo.
    let inside = marked.join("src");
    std::fs::create_dir_all(&inside).unwrap();
    let run: &mut Run = &mut run_command;
    assert!(super::marked(run, Some(marked.to_str().unwrap())));
    assert!(super::marked(run, Some(inside.to_str().unwrap())));
    for other in [&scratch.main, &scratch.dir("unmarked"), &scratch.root] {
        assert!(!super::marked(run, Some(other.to_str().unwrap())), "{}", other.display());
    }
}

#[test]
fn a_marked_worktrees_commit_credits_the_co_author_once_and_the_repos_own_hook_still_runs() {
    let scratch = Scratch::new("credits", &["marked"]);
    let marked = scratch.dir("marked");
    if !mark_co_author(&marked, "session-1") {
        return;
    }
    commit(&marked, "Work\n\nCo-Authored-By: Claude <noreply@anthropic.com>");
    let text = message(&marked);
    assert_eq!(text.matches(TRAILER).count(), 1, "{text}");
    assert!(text.contains("Co-Authored-By: Claude <noreply@anthropic.com>"), "{text}");
    assert!(repo_hook_ran(&marked));
    // An amend keeps one trailer, not two.
    sh(&marked, &["-c", "user.name=T", "-c", "user.email=t@example.com", "commit", "-q", "--allow-empty", "--amend", "--no-edit"]);
    assert_eq!(message(&marked).matches(TRAILER).count(), 1, "{}", message(&marked));
}

#[test]
fn an_editor_closed_untouched_still_aborts_and_one_that_wrote_a_message_is_credited() {
    let scratch = Scratch::new("editor", &["marked"]);
    let marked = scratch.dir("marked");
    if !mark_co_author(&marked, "session-1") {
        return;
    }
    // An editor closed untouched still aborts the commit, verbose or not; one that
    // wrote a message gets the trailer.
    // GIT_EDITOR, not core.editor: the environment's GIT_EDITOR would win.
    let with_editor = |editor: &str, extra: &[&str]| {
        let args = [&["-c", "user.name=T", "-c", "user.email=t@example.com", "commit", "-q", "--allow-empty"][..], extra].concat();
        Command::new("git").args(args).current_dir(&marked).env("GIT_EDITOR", editor).output().unwrap()
    };
    let head = sh(&marked, &["rev-parse", "HEAD"]);
    for verbose in [&[][..], &["-v"][..]] {
        let untouched = with_editor("true", verbose);
        assert!(!untouched.status.success(), "an untouched editor committed: {}", String::from_utf8_lossy(&untouched.stdout));
    }
    assert_eq!(sh(&marked, &["rev-parse", "HEAD"]), head);
    let edited = with_editor(r#"f() { printf 'Edited\n' > "$1"; }; f"#, &[]);
    assert!(edited.status.success(), "{}", String::from_utf8_lossy(&edited.stderr));
    let text = message(&marked);
    assert!(text.starts_with("Edited\n") && text.matches(TRAILER).count() == 1, "{text}");
}

#[test]
fn beside_a_marked_worktree_the_primary_checkout_and_an_unmarked_one_credit_no_one() {
    let scratch = Scratch::new("unmarked", &["marked", "unmarked"]);
    if !mark_co_author(&scratch.dir("marked"), "session-1") {
        return;
    }
    for dir in [&scratch.main, &scratch.dir("unmarked")] {
        commit(dir, "Not Tolkster's");
        let text = message(dir);
        assert!(!text.contains("Co-authored-by"), "{text}");
        assert!(repo_hook_ran(dir));
    }
}

#[test]
fn marking_without_a_co_author_removes_the_hook_and_still_writes_the_marker() {
    let scratch = Scratch::new("unset", &["marked"]);
    let marked = scratch.dir("marked");
    if !mark_co_author(&marked, "session-1") {
        return;
    }
    // Marking without a co-author removes the hook: the next commit has no trailer, and
    // the marker is still written. A repo with no hook marks the same way.
    let run: &mut Run = &mut run_command;
    mark(run, marked.to_str().unwrap(), "session-2", None).unwrap();
    assert_eq!(std::fs::read_to_string(git_dir(&marked).join(MARKER)).unwrap(), "session-2\n");
    assert!(!run("git", &["config", "--local", "--get-regexp", r"^hook\.tolkster-co-author\."], Some(scratch.main.to_str().unwrap())).unwrap().ok);
    commit(&marked, "After unsetting");
    let text = message(&marked);
    assert!(!text.contains("Co-authored-by"), "{text}");
    assert!(repo_hook_ran(&marked));
    mark(run, marked.to_str().unwrap(), "session-3", None).unwrap();
}
