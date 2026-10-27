use super::*;

#[test]
fn ids_are_alphanumerics_and_dashes() {
    assert!(plain_id("0a1b2c3d-0000-4000-8000-000000000001") && plain_id("TW-7"));
    for bad in ["", "s 1", "s;rm", "a/b", "s_1", "-x", "--all", &"a".repeat(65)] {
        assert!(!plain_id(bad), "{bad}");
    }
}

#[test]
fn identifiers_are_a_team_key_and_a_number() {
    for ok in ["TW-192", "tw-7", "A1-123456789"] {
        assert!(plain_identifier(ok), "{ok}");
    }
    for bad in ["", "TW", "TW-", "-1", "1W-1", "TW-+5", "TW-1-2", "TW-1234567890", "TW-1; rm", "T W-1", "ABCDEFGHIJKLMNOPQ-1"] {
        assert!(!plain_identifier(bad), "{bad}");
    }
}

#[test]
fn branches_are_refs_git_takes_as_names_not_options() {
    for ok in ["main", "sam/tw-7-add-a-thing", "the-small-simplifications-tw-225", "v1.2"] {
        assert!(plain_branch(ok), "{ok}");
    }
    for bad in ["", "-x", "/x", "a..b", "a b", "a;b", "a\\b", "a~1", "a^"] {
        assert!(!plain_branch(bad), "{bad}");
    }
}

#[test]
fn a_commit_is_seven_to_forty_hex_digits() {
    assert!(plain_commit("abc1234") && plain_commit(&"a".repeat(40)));
    for bad in ["", "abc123", "HEAD", "abc1234..def5678", "--all", &"a".repeat(41)] {
        assert!(!plain_commit(bad), "{bad}");
    }
}

#[test]
fn a_commit_is_forty_hex_digits() {
    assert!(plain_sha("0123456789abcdef0123456789ABCDEF01234567"));
    for bad in ["", "abc", "0123456789abcdef0123456789abcdef0123456", "0123456789abcdef0123456789abcdef012345678", "g123456789abcdef0123456789abcdef01234567", "-x23456789abcdef0123456789abcdef01234567"] {
        assert!(!plain_sha(bad), "{bad}");
    }
}

#[test]
fn times_are_iso_as_linear_writes_them() {
    assert!(plain_time("2026-09-28T10:52:59.568Z") && plain_time("2026-09-28"));
    for bad in ["", "--all", "yesterday", "2026-09-28T10:52:59.568Z;rm", "t 1", &"1".repeat(41)] {
        assert!(!plain_time(bad), "{bad}");
        assert!(Time::check(bad).is_err(), "{bad}");
    }
    assert!(Time::check("2026-09-26T01:00:00.000Z").is_ok());
}

#[test]
fn a_path_is_plain_only_as_git_prints_a_simple_one() {
    for path in ["cli/src/runtime.rs", "AGENTS.md", "src/worker/state.ts"] {
        assert!(plain_path(path), "{path}");
    }
    for path in ["", "-rf", "/etc/passwd", "a/../b", "a b.rs", "a`b", &"a".repeat(201)] {
        assert!(!plain_path(path), "{path}");
    }
}

#[test]
fn a_repo_name_is_one_plain_part_of_a_path() {
    for ok in ["tolkster", "transfer-collect (C&C)"] {
        assert!(RepoName::check(ok).is_ok(), "{ok}");
    }
    for bad in ["", " ", "..", "a/b", "a\\b", "C:x", "a\"b", "a“b", "a”b", "a„b", "$(x)", "a`b", "a!b", "a\nb"] {
        assert!(RepoName::check(bad).is_err(), "{bad}");
    }
}

#[test]
fn a_terminal_line_holds_nothing_a_shell_ends_or_expands_a_string_at() {
    assert!(Line::check("claude --model x Read the file /a/b.md in full.").is_ok());
    for bad in ["a\"b", "a“b", "a”b", "a„b", "a`b", "a$b", "a!b", "a\nb", "C:\\a\\"] {
        assert!(Line::check(bad).is_err(), "{bad}");
    }
}

#[test]
fn identifiers_split_into_team_key_and_number() {
    assert_eq!(split_identifier("TW-82"), Some(("TW", 82)));
    assert_eq!(split_identifier("ozga-64"), Some(("ozga", 64)));
    assert_eq!(split_identifier("TW-+5"), Some(("TW", 5)));
    for bad in ["TW", "TW-", "-82", "TW-x", "T W-1"] {
        assert_eq!(split_identifier(bad), None, "{bad}");
    }
}

// The contract with the Durable Object (src/worker/state.ts, WORKTREE), which
// test/state.test.ts checks against the same cases.
#[test]
fn a_worktree_id_splits_into_its_host_repo_and_path_as_the_worker_takes_it() {
    let contract = crate::state::tests::contract();
    for case in contract["worktreeIds"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        assert_eq!(split_worktree(id), Some((case["host"].as_str(), case["repo"].as_str().unwrap(), case["path"].as_str().unwrap())), "{id}");
    }
    for id in contract["notWorktreeIds"].as_array().unwrap().iter().map(|i| i.as_str().unwrap()) {
        assert_eq!(split_worktree(id), None, "{id}");
    }
}

// The contract with the Durable Object (src/worker/machine/validate.ts, TERMINAL), which
// test/state.test.ts checks against the same cases.
#[test]
fn a_terminal_handle_splits_into_its_host_and_orca_s_own_handle_as_the_worker_takes_it() {
    let contract = crate::state::tests::contract();
    for case in contract["terminals"].as_array().unwrap() {
        let handle = case["handle"].as_str().unwrap();
        assert_eq!(split_terminal(handle), Some((case["host"].as_str(), case["orca"].as_str().unwrap())), "{handle}");
    }
    for handle in contract["notTerminals"].as_array().unwrap().iter().map(|i| i.as_str().unwrap()) {
        assert_eq!(split_terminal(handle), None, "{handle}");
    }
}

#[test]
fn a_host_s_own_orca_knows_its_worktree_without_the_host() {
    let remote = WorktreeId::from("mac@r1::/Users/sam/a::b@c");
    assert_eq!((remote.host(), remote.own().as_str(), remote.path()), (Some("mac"), "r1::/Users/sam/a::b@c", "/Users/sam/a::b@c"));
    assert_eq!(WorktreeId::hosted(Some("mac"), &remote.own()), Some(remote));
    let here = WorktreeId::from("r1::D:/wt/tw-1");
    assert_eq!((here.host(), here.own()), (None, here.clone()));
    assert_eq!(WorktreeId::hosted(Some("bad host"), &here), None);
    // Decoded where it arrives, a value of another shape is refused without being echoed.
    assert_eq!(serde_json::from_str::<WorktreeId>("\"mac@r1::/wt\"").ok(), Some("mac@r1::/wt".into()));
    let refused = serde_json::from_str::<WorktreeId>("\"/Users/sam/wt\"").unwrap_err().to_string();
    assert!(refused.contains("isn't plain") && !refused.contains("sam"), "{refused}");
}

// What a host's shell line can't carry goes as hex, which decodes only to what it came from.
#[test]
fn a_value_goes_to_a_host_as_hex_and_back() {
    assert!(CommandLine::template("x {}", &[&hex("Ada \"L\" <a@b>")]).is_ok());
    assert_eq!(unhex(&hex("Ada Ünï <a@b>")).as_deref(), Some("Ada Ünï <a@b>"));
    assert!(unhex("abc").is_none() && unhex("zz").is_none());
}

#[test]
fn a_destination_is_an_absolute_plain_folder() {
    for good in ["/srv", "/home/me/src", "C:\\src", "D:/work/hosts"] {
        assert!(plain_destination(good), "{good}");
    }
    for bad in ["src", "~/src", "/srv/../etc", "/my src", "/srv;rm", "C:", "-/x", "/x$HOME"] {
        assert!(!plain_destination(bad), "{bad}");
    }
}

#[test]
fn a_repo_is_owner_and_name() {
    assert_eq!(split_repo("Tolkien-Works/tolkster"), Some(("Tolkien-Works", "tolkster")));
    assert_eq!(split_repo("o/my_repo.rs"), Some(("o", "my_repo.rs")));
    assert_eq!(split_repo("o/.github"), Some(("o", ".github")));
    for bad in ["tolkster", "/tolkster", "o/", "o/n/x", "o/..", "o/n?x=1", "o/n#x", "o/n x", "../n", "-o/n", "o_x/n", "o.x/n"] {
        assert_eq!(split_repo(bad), None, "{bad}");
    }
}

#[test]
fn a_file_is_named_only_inside_the_worktree_and_in_a_plain_shape() {
    assert_eq!(relative("D:\\wt\\tw-1\\cli\\src\\claim.rs", "D:/wt/tw-1").as_deref(), Some("cli/src/claim.rs"));
    assert_eq!(relative("d:/WT/tw-1/docs/cli.md", "D:\\wt\\tw-1\\").as_deref(), Some("docs/cli.md"));
    assert_eq!(relative("/d/wt/tw-1/docs/cli.md", "D:/wt/tw-1").as_deref(), Some("docs/cli.md"));
    // In the case the tool gave it.
    assert_eq!(relative("D:\\WT\\tw-1\\AGENTS.md", "d:/wt/tw-1").as_deref(), Some("AGENTS.md"));
    assert_eq!(relative("D:/wt/tw-1/a.rs", ""), None);
    // Outside it, beside it, or not plain.
    assert_eq!(relative("C:\\Users\\someone\\.claude\\briefs\\TW-1.md", "D:/wt/tw-1"), None);
    assert_eq!(relative("D:/wt/tw-10/a.rs", "D:/wt/tw-1"), None);
    assert_eq!(relative("D:/wt/tw-1", "D:/wt/tw-1"), None);
    assert_eq!(relative("D:/wt/tw-1/docs/a file.md", "D:/wt/tw-1"), None);
    assert_eq!(relative("D:/wt/tw-1/../other/a.rs", "D:/wt/tw-1"), None);
}

#[test]
fn a_working_directory_is_matched_to_its_worktree() {
    assert!(inside("D:\\wt\\tw-1", "D:/wt/tw-1"));
    assert!(inside("D:\\wt\\tw-1\\cli", "D:/wt/tw-1/"));
    assert!(inside("/d/wt/tw-1", "D:/wt/tw-1"));
    assert!(!inside("D:\\wt\\tw-10", "D:/wt/tw-1"));
    assert!(!inside("D:\\wt", "D:/wt/tw-1"));
    assert!(!inside("D:\\wt\\tw-1", ""));
}
