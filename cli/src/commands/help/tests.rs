use super::*;
use crate::error::Error;

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

#[test]
fn a_subcommand_gets_its_own_entry_from_the_help() {
    let block = block(&argv(&["issue", "create", "--title", "x"])).unwrap();
    assert!(block.starts_with("  issue create "));
    for option in ["--parent", "--triage", "--description-file"] {
        assert!(block.contains(option), "{option} missing from {block}");
    }
    assert!(!block.contains("issue update"));
    assert!(!block.contains("issue relate"));
}

#[test]
fn a_command_without_a_known_subcommand_gets_all_its_entries() {
    let issue = block(&argv(&["issue", "--help"])).unwrap();
    for entry in ["issue show", "issue list", "issue update", "issue create", "issue relate|unrelate", "issue chain"] {
        assert!(issue.contains(&format!("  {entry} ")), "{entry} missing");
    }
    let comment = block(&argv(&["comment", "TW-1"])).unwrap();
    assert_eq!(comment.lines().filter(|l| l.starts_with("  comment ")).count(), 3);
    assert!(!comment.contains("triage"));
    assert_eq!(block(&argv(&["nothing"])), None);
    assert_eq!(block(&argv(&["help"])), None);
    for prose in ["Settings:", "it", "the", "docs/cli.md"] {
        assert_eq!(block(&argv(&[prose])), None);
    }
    assert_eq!(block(&argv(&["version", "print"])).unwrap().lines().count(), 1);
    assert_eq!(usage().matches("  issue relate|unrelate ").count(), 1);
}

#[test]
fn each_command_is_one_entry_and_the_longest_name_runs() {
    let names: Vec<String> = COMMANDS.iter().map(|spec| spec.words.join(" ")).collect();
    for name in &names {
        assert_eq!(names.iter().filter(|n| *n == name).count(), 1, "{name}");
    }
    let named = |words: &[&str]| crate::commands::find(&argv(words)).map(|(spec, rest)| (spec.words.join(" "), rest.len()));
    assert_eq!(named(&["host", "rm", "x"]), Some(("host rm".into(), 1)));
    assert_eq!(named(&["host", "frob"]), Some(("host".into(), 1)));
    assert_eq!(named(&["github", "app", "create", "--org", "o"]), Some(("github app create".into(), 2)));
    assert_eq!(named(&["github", "app"]), Some(("github".into(), 1)));
    assert_eq!(named(&["frob"]), None);
}

#[test]
fn help_is_asked_only_among_the_options() {
    assert!(asked(&argv(&["issue", "create", "--help"])));
    assert!(!asked(&argv(&["issue", "show", "--", "--help"])));
    assert!(!asked(&argv(&["--help"])));
}

#[test]
fn a_usage_error_ends_with_the_block_and_help_prints_it() {
    let Err(Error::Usage(message)) = crate::run(&argv(&["issue", "create", "--team", "TW"])) else { panic!("not a usage error") };
    let block = block(&argv(&["issue", "create"])).unwrap();
    assert!(message.starts_with("issue create needs --title\n\n"));
    assert!(message.ends_with(&block));
    assert_eq!(crate::run(&argv(&["issue", "create", "--help"])).unwrap(), 0);
}
