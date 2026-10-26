use super::*;

// A failure names the program, its subcommand and its stderr, and nothing it printed on
// stdout, which may carry anything.
#[test]
fn a_failed_command_fails_with_its_stderr() {
    let mut run = |_: &str, _: &[&str], _: Option<&str>| Ok(Output::failed(1, "fatal: no
"));
    assert_eq!(call(&mut run, "git", &["status", "--porcelain"], Some("/wt")).unwrap_err().to_string(), "git status --porcelain failed: fatal: no");
    let mut quiet = |_: &str, _: &[&str], _: Option<&str>| Ok(Output { ok: false, code: Some(1), stdout: r#"{ "error": { "code": "x" } }"#.into(), stderr: String::new() });
    assert_eq!(call(&mut quiet, "git", &["status"], None).unwrap_err().to_string(), "git status failed");
}

#[test]
fn orca_calls_get_longer_slow_cutoffs_than_others() {
    assert_eq!(slow_cutoff("orca", &["worktree", "create", "--json"]), Duration::from_secs(10));
    assert_eq!(slow_cutoff("orca", &["worktree", "ps", "--json"]), Duration::from_secs(5));
    assert_eq!(slow_cutoff("git", &["rev-parse", "HEAD"]), trace::SLOW);
    assert_eq!(slow_cutoff("git", &["worktree", "create"]), trace::SLOW);
}

#[test]
fn a_slow_command_is_named_by_its_program_and_subcommand() {
    assert_eq!(command_site("orca", &["worktree", "ps", "--json"]), "orca worktree ps");
    assert_eq!(command_site("git", &["rev-parse", "HEAD"]), "git rev-parse");
    // A path, a branch, an ID or a flag never reaches the line.
    assert_eq!(command_site("git", &["-C", "D:/w/tw-1", "status"]), "git");
    assert_eq!(command_site("git", &["push", "origin", "task-tw-1"]), "git push origin");
    assert_eq!(command_site("orca", &["terminal", "read", "--terminal", "t-1"]), "orca terminal read");
    assert_eq!(command_site("git", &["branch", "-D", "x"]), "git branch");
    assert_eq!(command_site("git", &["-c", "http.lowSpeedLimit=1000", "-c", "http.lowSpeedTime=60", "fetch", "--quiet", "origin"]), "git fetch");
    assert_eq!(command_site("git", &["-c"]), "git");
}
