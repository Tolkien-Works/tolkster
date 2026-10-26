use super::*;

const OWN: &str = "s1";

fn argv(args: &str) -> Vec<String> {
    args.split(' ').filter(|a| !a.is_empty()).map(String::from).collect()
}

// The words of the command main dispatches for `argv`, as it hands them to the gate.
fn dispatched(argv: &[String]) -> Option<&'static [&'static str]> {
    crate::commands::find(argv).map(|(spec, _)| spec.words)
}

fn refused(args: &str) -> bool {
    let argv = argv(args);
    matches!(gate(&argv, dispatched(&argv), OWN), Err(Error::Usage(_)))
}

#[test]
fn only_the_tables_commands_run() {
    let allowed: Vec<String> = TABLE.iter().map(|e| format!("{}{}", e.command, e.sub.map(|s| format!(" {s}")).unwrap_or_default())).collect();
    for spec in crate::commands::COMMANDS {
        let command = spec.words.join(" ");
        let is_allowed = allowed.contains(&command);
        // Any refusal is for the command, not for how it was called: run it bare.
        let bare = argv(&command);
        let why = gate(&bare, dispatched(&bare), OWN).err().map(|e| e.to_string()).unwrap_or_default();
        assert_eq!(is_allowed, !why.contains("doesn't run"), "{command}: {why}");
    }
    for other in ["pr resolve", "pr auto-merge", "pr describe", "issue delegate", "whoami --json", "", "--help"] {
        assert!(refused(other), "{other}");
    }
}

#[test]
fn every_option_outside_the_table_is_refused() {
    // Options the commands take outside a conversation, and a few made up.
    let others = ["--while", "--delegate", "--closed TW-2", "--file x", "--head x", "--repo a/b", "--pr 7", "--print", "--dry-run", "--made-up", "--json"];
    for entry in TABLE {
        let head = format!("{}{} {}", entry.command, entry.sub.map(|s| format!(" {s}")).unwrap_or_default(), if entry.session == SessionAt::First { OWN } else { "" });
        let session = if let SessionAt::Flag(name) = entry.session { format!(" --{name} {OWN}") } else { String::new() };
        for option in others {
            let name = option.trim_start_matches("--").split(' ').next().unwrap();
            if entry.options.iter().any(|(n, _)| *n == name) {
                continue;
            }
            assert!(refused(&format!("{head} TW-1{session} {option}")), "{head} {option}");
        }
    }
}

#[test]
fn a_turn_acts_only_in_its_own_session_and_reads_no_file() {
    assert!(!refused("messages s1 --after 2026-09-26T01:00:00.000Z"));
    assert!(refused("messages s2"));
    assert!(!refused("activity s1 response --body-file -"));
    assert!(!refused("activity s1 thought --state reading-issue"));
    assert!(refused("activity s2 response --body-file -"));
    assert!(refused("activity s1 response --body-file C:/Users/sam/.tolkster/credentials.json"));
    assert!(refused("activity s1 thought --state reading-code --while -- pwsh -c x"));
    assert!(refused("activity s1 thought --state reading-code -- pwsh -c x"));
    assert!(!refused("issue show TW-1 --comments --children --attachments --images"));
    assert!(!refused("issue show TW-1 TW-2 --comments"));
    assert!(!refused("start s1"));
    assert!(refused("start s2"));
    assert!(refused("issue show TW-1 --json"));
    assert!(!refused("issue list --team TW --query words --state open --project Tolkster --parent TW-1 --limit 10"));
    assert!(refused("issue list --team TW --json"));
    assert!(!refused("issue update TW-1 --session s1 --title x --description-file -"));
    assert!(refused("issue update TW-1 --session s1 --description-file notes.md"));
    assert!(refused("issue update TW-1 --session s2 --title x"));
    assert!(refused("issue update TW-1 --title x"));
    assert!(refused("issue update TW-2 --session s1 --delegate"));
    assert!(!refused("issue update TW-1 --session s1 --project none"));
    assert!(!refused("issue update TW-1 --session s1 --assignee me"));
    assert!(refused("issue update TW-1 --session s2 --assignee me"));
    assert!(refused("issue update TW-1 --assignee me"));
    assert!(!refused("issue create --session s1 --title x --parent TW-1 --assignee me"));
    assert!(refused("issue create --title x --parent TW-1 --assignee me"));
    assert!(!refused("issue create --session s1 --title x --parent TW-1"));
    assert!(!refused("issue create --session s1 --title x --team TW --project Tolkster"));
    assert!(!refused("issue relate TW-1 --session s1 --blocks TW-2"));
    assert!(!refused("comment TW-2 --session s1 --body-file -"));
    assert!(refused("comment TW-2 --closed TW-1 --body-file -"));
    assert!(!refused("help"));
}

// A reply's turn in the thread of Tolkster's comment `c1` reads its replies and any issue,
// replies in that thread only, and changes issues for that thread only: no session's
// commands, and no delegation.
#[test]
fn a_replys_turn_replies_only_in_its_own_thread() {
    let thread = |args: &str| gate_thread(&argv(args), dispatched(&argv(args)), "c1");
    let refused = |args: &str| matches!(thread(args), Err(Error::Usage(_)));
    assert!(!refused("messages --thread c1 --after 2026-09-26T01:00:00.000Z"));
    assert!(refused("messages --thread c2"));
    assert!(!thread("issue list --team TW --query words").is_err());
    assert!(refused("messages s1"));
    assert!(!refused("comment TW-1 --reply-to c1 --body-file -"));
    assert!(!refused("comment TW-1 --reply-to c1 --none"));
    assert!(refused("comment TW-1 --reply-to c2 --body-file -"));
    assert!(refused("comment TW-1 --reply-to c1 --body-file notes.md"));
    assert!(refused("comment TW-1 --body-file -"));
    assert!(refused("comment TW-1 --reply-to c1 --session s1 --body-file -"));
    assert!(!refused("issue show TW-1 --comments"));
    assert!(!refused("issue update TW-1 --thread c1 --description-file - --state Todo"));
    assert!(!refused("issue create --thread c1 --title x --parent none --team TW --project Tolkster --triage"));
    assert!(!refused("issue relate TW-1 --thread c1 --blocks TW-2"));
    assert!(!refused("issue unrelate TW-1 --thread c1 --related TW-2"));
    assert!(refused("issue update TW-1 --thread c2 --title x"));
    assert!(refused("issue update TW-1 --thread c1 --delegate"));
    assert!(refused("issue update TW-1 --thread c1 --description-file notes.md"));
    assert!(refused("issue create --thread c2 --title x"));
    assert!(refused("issue relate TW-1 --thread c2 --blocks TW-2"));
    for never in [
        "activity c1 response --body-file -",
        "issue update TW-1 --title x",
        "issue update TW-1 --session s1 --title x",
        "issue create --title x",
        "issue create --session s1 --title x",
        "issue relate TW-1 --blocks TW-2",
        "issue chain TW-1 --thread c1",
        "plan c1 --file -",
        "pr create",
    ] {
        assert!(refused(never), "{never}");
    }
    assert!(!refused("help"));
}

// The gate is its own table, but never lets an option through that the command doesn't take.
#[test]
fn each_entry_takes_only_its_commands_options() {
    for entry in TABLE.iter().chain(THREAD_TABLE) {
        let words: Vec<String> = [Some(entry.command), entry.sub].into_iter().flatten().map(String::from).collect();
        let (spec, rest) = crate::commands::find(&words).unwrap();
        assert!(rest.is_empty(), "{words:?}");
        for option in entry.options {
            assert!(spec.options.contains(option), "{words:?} --{}", option.0);
        }
    }
}

// An entry without a subcommand lets through no longer command dispatch would run instead.
#[test]
fn an_entry_never_passes_a_longer_command() {
    let host = [Allowed { command: "host", sub: None, options: &[], session: SessionAt::Nowhere }];
    let checked = |args: &str| check(&host, &argv(args), dispatched(&argv(args)), OWN, "a test", "host", "its own");
    assert!(checked("host x").is_ok());
    assert!(checked("host rm x").is_err());
}

#[test]
fn the_relations_named_by_hand_are_the_ones_relate_takes() {
    let mut takes = relation_names();
    takes.sort();
    for table in [RELATE, THREAD_RELATE, crate::commands::issue::RELATE] {
        let mut named: Vec<&str> = table.iter().map(|(n, _)| *n).filter(|n| !["session", "thread"].contains(n)).collect();
        named.sort();
        assert_eq!(named, takes);
    }
    // `issue create` takes each of them, repeatable.
    for table in [CREATE, THREAD_CREATE, crate::commands::issue::CREATE] {
        for name in &takes {
            assert!(table.iter().any(|(n, kind)| n == name && *kind == Kind::List), "{name}");
        }
    }
}
