use super::*;
use crate::exec::Output;

// A fake git that fails every command whose words include `fail`, with a stderr naming a
// credential, and answers the rest with `SHA`.
const SHA: &str = "2878d3d000000000000000000000000000000000";

fn git_failing<'a>(calls: &'a mut Vec<String>, fail: &'a str) -> impl FnMut(&str, &[&str], Option<&str>) -> Result<Output, Error> + 'a {
    move |_, args, _| {
        calls.push(args.join(" "));
        let ok = !args.contains(&fail);
        let stderr = if ok { String::new() } else { "fatal: https://x-access-token:secret@github.com/o/r.git refused".into() };
        Ok(Output { ok, code: Some(if ok { 0 } else { 1 }), stdout: format!("{SHA}\n"), stderr })
    }
}

#[test]
fn a_failed_push_hides_what_git_said() {
    let mut calls = Vec::new();
    let error = push_upstream(&mut git_failing(&mut calls, "push"), "D:/w", &"tw-1".into()).unwrap_err().to_string();
    assert!(!error.contains("secret") && !error.contains("github.com"), "{error}");
    assert_eq!(calls, ["-c http.lowSpeedLimit=1000 -c http.lowSpeedTime=60 push --quiet -u origin tw-1"]);
}

#[test]
fn a_failed_fetch_hides_what_git_said() {
    let mut calls = Vec::new();
    let error = fetch(&mut git_failing(&mut calls, "fetch"), "D:/w").unwrap_err().to_string();
    assert!(!error.contains("secret") && !error.contains("github.com"), "{error}");
}

#[test]
fn the_default_tip_falls_back_to_main() {
    let mut calls = Vec::new();
    assert_eq!(default_tip(&mut git_failing(&mut calls, "origin/HEAD^{commit}"), "D:/w").unwrap(), SHA);
    assert_eq!(calls, ["rev-parse --verify --quiet origin/HEAD^{commit}", "rev-parse --verify --quiet origin/main^{commit}"]);
}

#[test]
fn what_a_checkout_pushed_is_its_branch_and_its_upstreams_commit() {
    let mut calls = Vec::new();
    assert_eq!(pushed(&mut git_failing(&mut calls, "none"), "D:/w").unwrap(), (SHA.to_string(), SHA.into()));
    assert_eq!(calls, ["symbolic-ref --short HEAD", "rev-parse --verify --quiet @{upstream}^{commit}"]);
    // No upstream, or no branch, is an error, never a guess.
    assert!(pushed(&mut git_failing(&mut Vec::new(), "rev-parse"), "D:/w").is_err());
    assert!(pushed(&mut git_failing(&mut Vec::new(), "symbolic-ref"), "D:/w").is_err());
}

#[test]
fn a_detach_discards_changes_and_a_delete_keeps_a_branch_with_commits_of_its_own() {
    let mut calls = Vec::new();
    detach(&mut git_failing(&mut calls, "none"), "D:/w", &SHA.into()).unwrap();
    delete_merged_branch(&mut git_failing(&mut calls, "none"), "D:/w", &"b".into()).unwrap();
    assert_eq!(calls, [format!("switch --quiet --detach --discard-changes {SHA}"), "branch --quiet -d b".into()]);
    assert!(delete_merged_branch(&mut git_failing(&mut Vec::new(), "-d"), "D:/w", &"b".into()).is_err());
}

#[test]
fn what_git_says_is_a_commit_only_when_it_is_one() {
    let mut said = |_: &str, _: &[&str], _: Option<&str>| Ok(Output::ok("--all\n"));
    assert!(default_tip(&mut said, "D:/w").is_err());
    assert!(last_before(&mut said, "D:/w", &"2026-09-28".into(), &SHA.into()).is_err());
}

#[test]
fn a_detached_head_has_no_branch() {
    let mut detached = |_: &str, _: &[&str], _: Option<&str>| Ok(Output::ok("HEAD\n"));
    assert_eq!(current_branch(&mut detached, "D:/w").unwrap(), None);
}

fn says(code: i32, stdout: &str, stderr: &str) -> impl FnMut(&str, &[&str], Option<&str>) -> Result<Output, Error> {
    let (stdout, stderr) = (stdout.to_string(), stderr.to_string());
    move |_, _, _| Ok(Output { ok: code == 0, code: Some(code), stdout: stdout.clone(), stderr: stderr.clone() })
}

#[test]
fn a_status_keeps_the_blank_column_of_its_first_line() {
    assert_eq!(status_porcelain(&mut says(0, " M a.rs\n?? b\n", ""), "D:/w").unwrap(), " M a.rs\n?? b\n");
}

#[test]
fn a_branch_exists_when_git_says_so_and_a_failure_is_not_a_no() {
    let branch: BranchName = "tw-1".into();
    assert!(branch_exists(&mut says(0, "", ""), "D:/w", &branch).unwrap());
    assert!(!branch_exists(&mut says(1, "", ""), "D:/w", &branch).unwrap());
    assert_eq!(branch_exists(&mut says(128, "", "fatal: not a git repository\n"), "D:/w", &branch).unwrap_err().to_string(), "fatal: not a git repository");
}

#[test]
fn unpushed_counts_commits_on_no_remote_and_none_is_not_a_count() {
    let branch: BranchName = "tw-1".into();
    let mut calls = Vec::new();
    let mut run = |_: &str, args: &[&str], _: Option<&str>| {
        calls.push(args.join(" "));
        Ok(Output { ok: true, code: Some(0), stdout: "3\n".into(), stderr: String::new() })
    };
    assert_eq!(unpushed(&mut run, "D:/w", &branch).unwrap(), Some(3));
    assert_eq!(calls, ["rev-list --count HEAD refs/heads/tw-1 --not --remotes"]);
    assert_eq!(unpushed(&mut says(0, "many\n", ""), "D:/w", &branch).unwrap(), None);
    assert!(unpushed(&mut says(128, "", "fatal: bad revision"), "D:/w", &branch).is_err());
}

#[test]
fn a_forced_delete_names_the_branch_and_fails_loudly() {
    let mut calls = Vec::new();
    force_delete_branch(&mut git_failing(&mut calls, "none"), "D:/w", &"b".into()).unwrap();
    assert_eq!(calls, ["branch -D b"]);
    assert!(force_delete_branch(&mut git_failing(&mut Vec::new(), "-D"), "D:/w", &"b".into()).is_err());
}

#[test]
fn the_reads_of_a_checkout_run_where_the_process_does_when_given_no_dir() {
    let mut seen = Vec::new();
    let mut run = |_: &str, args: &[&str], dir: Option<&str>| {
        seen.push((args.join(" "), dir.map(str::to_string)));
        Ok(Output { ok: true, code: Some(0), stdout: "D:/top\n".into(), stderr: String::new() })
    };
    assert_eq!(toplevel(&mut run).unwrap(), "D:/top");
    assert_eq!(origin_url(&mut run, None).unwrap(), "D:/top");
    assert_eq!(origin_url(&mut run, Some("D:/w")).unwrap(), "D:/top");
    on_origin_head(&mut run, &"abc1234".into()).unwrap();
    assert_eq!(seen, [
        ("rev-parse --show-toplevel".to_string(), None),
        ("remote get-url origin".to_string(), None),
        ("remote get-url origin".to_string(), Some("D:/w".to_string())),
        ("merge-base --is-ancestor abc1234 origin/HEAD".to_string(), None),
    ]);
    assert!(on_origin_head(&mut says(1, "", ""), &"abc1234".into()).is_err());
}

#[test]
fn a_hook_is_removed_only_when_set_and_listed_only_by_its_name() {
    let mut calls = Vec::new();
    let mut unset = |_: &str, args: &[&str], _: Option<&str>| {
        calls.push(args.join(" "));
        Ok(Output { ok: false, code: Some(1), stdout: String::new(), stderr: String::new() })
    };
    remove_hook_config(&mut unset, "D:/w", "h").unwrap();
    assert_eq!(calls, [r"config --local --get-regexp ^hook\.h\."]);
    let mut calls = Vec::new();
    let mut set = |_: &str, args: &[&str], _: Option<&str>| {
        calls.push(args.join(" "));
        Ok(Output { ok: true, code: Some(0), stdout: "hook.h.event pre\n".into(), stderr: String::new() })
    };
    remove_hook_config(&mut set, "D:/w", "h").unwrap();
    assert_eq!(calls, [r"config --local --get-regexp ^hook\.h\.", "config --local --remove-section hook.h"]);
    assert!(hook_listed(&mut says(0, "h\nother\n", ""), "D:/w", "pre", "h").unwrap());
    assert!(!hook_listed(&mut says(0, "other\n", ""), "D:/w", "pre", "h").unwrap());
    assert!(!hook_listed(&mut says(1, "h\n", ""), "D:/w", "pre", "h").unwrap());
}


#[test]
fn a_merge_is_the_newest_subject_naming_the_issue_whole_in_its_last_tag() {
    let sha = |c: char| c.to_string().repeat(40);
    let log = format!("{}\tAfter TW-100 (#9)\n{}\tTW-10 and its sequel TW-10x\n{}\tTW-10 reverted, no PR\n{}\tFollow-up to TW-10 (TW-30) (#9)\n{}\tRevert \"The thing (TW-10) (#5)\" (#7)\n{}\tThe thing (TW-10) (#5)\n{}\tOlder (TW-10, TW-11) (#2)\n", sha('a'), sha('b'), sha('c'), sha('g'), sha('h'), sha('d'), sha('e'));
    let mut calls = Vec::new();
    let mut run = |_: &str, args: &[&str], _: Option<&str>| {
        calls.push(args.join(" "));
        Ok(Output::ok(log.clone()))
    };
    let tip = Sha::parse(&sha('f')).unwrap();
    assert_eq!(merge_of(&mut run, "D:/w", &tip, &"TW-10".into()).unwrap(), Sha::parse(&sha('d')));
    assert_eq!(merge_of(&mut run, "D:/w", &tip, &"TW-1".into()).unwrap(), None);
    assert_eq!(calls[0], format!("log --first-parent --fixed-strings --grep=TW-10 --format=%H%x09%s {}", sha('f')));
    assert!(merge_of(&mut says(128, "", "fatal"), "D:/w", &tip, &"TW-10".into()).is_err());
    // A title that begins "Revert" is a merge like another, a revert's own ends in a quote,
    // the tag may name several issues, and a landing's has no PR number after it.
    let only = |line: &str| merge_of(&mut says(0, &format!("{}\t{line}\n", sha('d')), ""), "D:/w", &tip, &"TW-11".into()).unwrap();
    assert_eq!(only("Revert to polling (TW-11) (#5)"), Sha::parse(&sha('d')));
    assert_eq!(only("Older (TW-10, TW-11) (#2)"), Sha::parse(&sha('d')));
    assert_eq!(only("Landed straight on main (TW-11)"), Sha::parse(&sha('d')));
    assert_eq!(only("Fix the panel (#640 follow-up) (TW-11)"), Sha::parse(&sha('d')));
    for refused in ["Revert \"The thing (TW-11) (#5)\"", "Revert \"Landed (TW-11)\"", "Fix (TW-11) (#channel) tail", "X (TW-11) (#5) [skip ci]", "Follow-up to TW-11 (#5)", "Follow-up to TW-11", "Landed (TW-111)"] {
        assert_eq!(only(refused), None, "{refused}");
    }
}

#[test]
fn what_landed_is_each_issue_in_the_last_tags_after_an_ancestor_else_the_tips_own() {
    let (from, to) = (Sha::parse(&"a".repeat(40)).unwrap(), Sha::parse(&"b".repeat(40)).unwrap());
    let log = "Second (tw-12) (#9)\nBoth (TW-11, WEB-3) (#8)\nMentions TW-99 (#7)\nRevert \"Old (TW-5) (#2)\" (#6)\nLanded (TW-11)\nBump (deps)\n";
    let mut calls = Vec::new();
    let mut run = |_: &str, args: &[&str], _: Option<&str>| {
        calls.push(args.join(" "));
        Ok(if args[0] == "merge-base" { Output { code: Some(if calls.len() == 1 { 0 } else { 1 }), ..Output::ok(String::new()) } } else { Output::ok(log) })
    };
    let keys = |keys: Vec<IssueKey>| keys.iter().map(|key| key.as_str().to_string()).collect::<Vec<_>>();
    assert_eq!(keys(landed(&mut run, "D:/w", Some(&from), &to).unwrap()), ["TW-12", "TW-11", "WEB-3"]);
    // `from` is no ancestor (merge-base exits 1): the tip alone; with none, no merge-base at all.
    landed(&mut run, "D:/w", Some(&from), &to).unwrap();
    landed(&mut run, "D:/w", None, &to).unwrap();
    assert_eq!(calls, [
        format!("merge-base --is-ancestor {from} {to}"),
        format!("log --first-parent --format=%s {from}..{to}"),
        format!("merge-base --is-ancestor {from} {to}"),
        format!("log --first-parent --format=%s {to}^!"),
        format!("log --first-parent --format=%s {to}^!"),
    ]);
}

#[test]
fn heads_moves_are_their_commit_time_and_gits_line_for_each() {
    let a = "a".repeat(40);
    let moves = |stdout: &str| last_moves(&mut says(0, stdout, ""), "D:/w", 5).unwrap();
    let said = format!("{a}\tHEAD@{{1791066826}}\tcommit: Add the flag: at last\n{a}\tHEAD@{{yesterday}}\tcommit: x\nnot a line\n");
    assert_eq!(moves(&said), [Move { sha: Sha::parse(&a).unwrap(), at: 1_791_066_826, why: "commit: Add the flag: at last".into() }]);
    assert_eq!(moves(""), []);
    assert!(last_moves(&mut says(128, "", "fatal"), "D:/w", 5).is_err());
}

#[test]
fn merges_fetch_once_and_say_unknown_where_git_could_not_look() {
    let sha = |c: char| c.to_string().repeat(40);
    let log = format!("{}\tThe thing (TW-10) (#5)\n", sha('d'));
    let calls = std::cell::RefCell::new(Vec::new());
    let mut run = |_: &str, args: &[&str], _: Option<&str>| {
        calls.borrow_mut().push(args.join(" "));
        Ok(Output::ok(if args.contains(&"log") { log.clone() } else { format!("{}\n", sha('f')) }))
    };
    let (found, absent): (IssueKey, IssueKey) = ("TW-10".into(), "TW-11".into());
    assert_eq!(merges(&mut run, "D:/w", &[&found, &absent]), [Merge::Found(Sha::parse(&sha('d')).unwrap()), Merge::Absent]);
    assert_eq!(calls.borrow().iter().filter(|c| c.ends_with("fetch --quiet origin")).count(), 1, "{:?}", calls.borrow());
    // None to look for: git isn't asked at all.
    assert_eq!(merges(&mut run, "D:/w", &[]), []);
    assert_eq!(calls.borrow().len(), 4, "{:?}", calls.borrow());
    // A failed fetch, or a failed log, is never read as nothing merged.
    assert_eq!(merges(&mut git_failing(&mut Vec::new(), "fetch"), "D:/w", &[&found, &absent]), [Merge::Unknown, Merge::Unknown]);
    assert_eq!(merges(&mut git_failing(&mut Vec::new(), "log"), "D:/w", &[&found]), [Merge::Unknown]);
}
