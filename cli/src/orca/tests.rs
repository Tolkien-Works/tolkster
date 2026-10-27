use super::*;
use std::time::Instant;
use serde_json::json;
use crate::error::Unsent;
use crate::health;
use crate::onboard::reads_config;
use crate::ports::fake::{primary, row};

// A fake shell that records every command and answers Orca's with `answer`.
fn shell<'a>(calls: &'a mut Vec<String>, answer: &'a dyn Fn(&[&str]) -> Value) -> impl FnMut(&str, &[&str], Option<&str>) -> Result<Output, Error> + 'a {
    move |program, args, dir| {
        calls.push(format!("{}{program} {}", dir.map(|d| format!("[{d}] ")).unwrap_or_default(), args.join(" ")));
        Ok(Output::ok(answer(args).to_string()))
    }
}

#[test]
fn the_origin_is_read_once_per_repo_and_carried_on_every_worktree() {
    let mut other_main = primary();
    other_main["worktreeId"] = json!("other::D:/other");
    other_main["repoId"] = json!("other");
    other_main["path"] = json!("D:/other");
    let mut remote_only = row(Some("TW-3"), "/home/x/tw-3");
    remote_only["repoId"] = json!("remote");
    remote_only["hostId"] = json!("ssh-box");
    let mut broken = row(Some("TW-4"), "D:/wt/tw-4");
    broken["repoId"] = json!("broken");
    let ps = json!({ "result": { "worktrees": [row(Some("TW-1"), "D:/wt/tw-1"), primary(), row(Some("TW-2"), "D:/wt/tw-2"), other_main, remote_only, broken] } });
    let answer = |args: &[&str]| match args[..2] {
        ["worktree", "ps"] => ps.clone(),
        _ => json!({}),
    };
    let mut calls = Vec::new();
    let worktrees = {
        let mut run = |program: &str, args: &[&str], dir: Option<&str>| {
            calls.push(format!("{}{program} {}", dir.map(|d| format!("[{d}] ")).unwrap_or_default(), args.join(" ")));
            let stdout = match (program, dir) {
                ("orca", _) => answer(args).to_string(),
                ("git", Some("D:/main")) => "https://github.com/Tolkien-Works/tolkster.git\n".into(),
                ("git", Some("D:/other")) => "git@github.com:Tolkien-Works/other.git\n".into(),
                _ => "not a remote\n".into(),
            };
            Ok(Output::ok(stdout))
        };
        list_worktrees(&mut run).unwrap()
    };
    let origins: Vec<(String, Option<String>)> = worktrees.iter().map(|w| (w.id.clone(), w.origin.clone())).collect();
    assert_eq!(
        origins,
        [
            ("repo::D:/wt/tw-1".to_string(), Some("Tolkien-Works/tolkster".to_string())),
            ("repo::D:/main".into(), Some("Tolkien-Works/tolkster".into())),
            ("repo::D:/wt/tw-2".into(), Some("Tolkien-Works/tolkster".into())),
            ("other::D:/other".into(), Some("Tolkien-Works/other".into())),
            // A remote that isn't GitHub's.
            ("repo::D:/wt/tw-4".into(), None),
        ]
    );
    // One git read per repo with a local worktree, in its primary checkout.
    let reads: Vec<&String> = calls.iter().filter(|c| c.contains("git remote")).collect();
    assert_eq!(reads, ["[D:/main] git remote get-url origin", "[D:/other] git remote get-url origin", "[D:/wt/tw-4] git remote get-url origin"]);
    assert_eq!(worktrees[0].origin().unwrap(), "Tolkien-Works/tolkster");
    assert_eq!(worktrees[4].origin().unwrap_err().to_string(), "the origin of D:/wt/tw-4 is not a GitHub repo, or couldn't be read");
}

#[test]
fn the_repo_a_project_names_is_found_ignoring_case_and_never_among_several() {
    let repos = parse_repos(json!({ "result": { "repos": [{ "id": "r1", "displayName": "Tolkster", "path": "D:/src/tolkster" }, { "id": "r2", "displayName": "Plan Portal", "path": "D:/src/portal" }] } })).unwrap();
    assert_eq!(find_repo(&repos, Some("tolkster")), Some(("r1".into(), "Tolkster".into())));
    assert_eq!(find_repo(&repos, Some("plan portal")), Some(("r2".into(), "Plan Portal".into())));
    assert_eq!(find_repo(&repos, Some("other")), None);
    assert_eq!(find_repo(&repos, None), None);
    let twice = parse_repos(json!({ "result": { "repos": [{ "id": "r1", "displayName": "tolkster", "path": "D:/a" }, { "id": "r2", "displayName": "Tolkster", "path": "D:/b" }] } })).unwrap();
    assert_eq!(find_repo(&twice, Some("Tolkster")), None);
}

// A repo list as Orca prints it: a repo whose remote Orca can't read has no origin.
#[test]
fn a_recorded_repo_list_names_each_repo_and_its_github_origin() {
    let listed = json!({ "ok": true, "result": { "repos": [
        { "id": "r1", "kind": "git", "displayName": "Tolkster", "path": "D:/src/tolkster", "addedAt": 1, "badgeColor": "#fff", "gitRemoteIdentity": { "canonicalKey": "github.com/Tolkien-Works/tolkster", "remoteName": "origin", "remoteUrl": "https://github.com/Tolkien-Works/tolkster.git" } },
        { "id": "r2", "kind": "git", "displayName": "Scratch", "path": "D:/src/scratch", "gitRemoteIdentity": null },
    ] } });
    let answer = move |_: &[&str]| listed.clone();
    let mut calls = Vec::new();
    let mut run = shell(&mut calls, &answer);
    assert_eq!(github_repos(&mut run).unwrap(), [("Tolkster".to_string(), "Tolkien-Works/tolkster".to_string())]);
    assert_eq!(repo_folders(&mut run).unwrap(), [("Tolkster".to_string(), "D:/src/tolkster".to_string()), ("Scratch".into(), "D:/src/scratch".into())]);
}

// Orca's checks pass a recipe only when its report says so, bare or in Orca's envelope.
#[test]
fn a_recipe_passes_only_on_a_report_that_says_so() {
    for (said, passes) in [(json!({ "ok": true }), true), (json!({ "ok": true, "result": { "ok": true, "checks": [] } }), true), (json!({ "ok": true, "result": { "ok": false } }), false), (json!({ "result": {} }), false)] {
        let answer = move |_: &[&str]| said.clone();
        assert_eq!(recipe_passes(&mut shell(&mut Vec::new(), &answer), "vm", "D:/src/app"), passes);
    }
}

// Orca's receipt for a text-plus-Enter prompt, at the stages it reached.
fn receipt(stages: &[&str], observation: &str) -> Value {
    json!({ "ok": true, "result": { "send": { "accepted": true, "handle": "t1", "prompt": { "requestId": "req-1", "stages": stages, "provider": "claude", "observation": observation } } } })
}

// An agent is launched on its command as it is, which starts it on its host (setup.rs):
// nothing typed into the new terminal, nor waited on.
#[test]
fn an_agent_is_launched_on_its_command_and_nothing_is_typed() {
    let answer = |_: &[&str]| json!({ "result": { "terminal": { "handle": "term_new" } } });
    let mut calls = Vec::new();
    let line = CommandLine::new(&"tolkster worker s1 --issue TW-1 --continue --then message".into());
    assert_eq!(Cli::alone(&mut shell(&mut calls, &answer)).start_agent("r1::/wt/tw-1", &line).unwrap(), "term_new");
    assert_eq!(calls, ["orca terminal create --worktree id:r1::/wt/tw-1 --command tolkster worker s1 --issue TW-1 --continue --then message --json"]);
}


#[test]
fn a_typed_line_counts_by_its_proof_and_a_launch_line_must_be_plain() {
    for (said, turn, input) in [
        (receipt(&["input_accepted", "turn_started"], "observed"), Typed::Delivered, Typed::Delivered),
        (receipt(&["input_accepted"], "timeout"), Typed::Unproven, Typed::Delivered),
        (receipt(&["input_accepted"], "permission"), Typed::Waiting, Typed::Waiting),
        (json!({ "ok": true, "result": { "send": { "accepted": true } } }), Typed::Unproven, Typed::Unproven),
        (json!({ "ok": true, "result": {} }), Typed::Unproven, Typed::Unproven),
        (json!({ "ok": true, "result": { "send": { "prompt": { "observation": "timeout" } } } }), Typed::Unproven, Typed::Unproven),
        (json!("not a receipt"), Typed::Unproven, Typed::Unproven),
    ] {
        for (proof, expected) in [(Proof::Turn, turn), (Proof::Input, input)] {
            let answer = |_: &[&str]| said.clone();
            let mut calls = Vec::new();
            assert_eq!(Cli::alone(&mut shell(&mut calls, &answer)).send("r1::/wt/tw-1", &"t1".into(), &"New message".into(), proof).unwrap(), expected);
            assert_eq!(calls.len(), 1, "never typed twice: {calls:?}");
            // Only a send that needs the turn waits for it.
            assert_eq!(calls[0].contains("--wait-submit 30"), proof == Proof::Turn, "{calls:?}");
        }
    }
    // A quoted prompt is never a `Line`, so never reaches a launch.
    assert!(Line::parse("Say \"hi\"").is_none());
}

#[test]
fn a_worktree_is_created_with_its_options_and_a_terminal_with_its_title() {
    let answer = |args: &[&str]| match args[..2] {
        ["worktree", "create"] => json!({ "result": { "worktree": { "id": "r1::/wt/tw-1", "path": "/wt/tw-1" } } }),
        ["terminal", "create"] => json!({ "result": { "terminal": { "handle": "term_new" } } }),
        _ => json!({ "ok": true }),
    };
    let mut calls = Vec::new();
    {
        let mut run = shell(&mut calls, &answer);
        let mut orca = Cli::alone(&mut run);
        assert_eq!(orca.create_worktree(Place::Here { repo_id: "r1" }, "tw-1", &["--no-parent", "--setup", "skip"]).unwrap(), ("r1::/wt/tw-1".to_string(), "/wt/tw-1".to_string()));
        assert_eq!(orca.open("r1::/wt/tw-1", Some("TW-1 context"), &CommandLine::new(&"claude".into()).quoted(&"x".into())).unwrap(), "term_new");
        assert_eq!(orca.open("r1::/wt/tw-1", None, &CommandLine::new(&"claude --continue".into())).unwrap(), "term_new");
        orca.remove_worktree("r1::/wt/tw-1").unwrap();
        orca.name_worktree("r1::/wt/tw-1", "tw-1").unwrap();
        orca.close(&"term_new".into()).unwrap();
        orca.close_all("r1::/wt/tw-1").unwrap();
    }
    assert_eq!(
        calls,
        [
            "orca worktree create --repo id:r1 --name tw-1 --no-parent --setup skip --json",
            "orca terminal create --worktree id:r1::/wt/tw-1 --title TW-1 context --command claude \"x\" --json",
            "orca terminal create --worktree id:r1::/wt/tw-1 --command claude --continue --json",
            "orca worktree rm --worktree id:r1::/wt/tw-1 --force --json",
            "orca worktree set --worktree id:r1::/wt/tw-1 --display-name tw-1 --json",
            "orca terminal close --terminal term_new --json",
            "orca terminal close --worktree id:r1::/wt/tw-1 --all --json",
        ]
    );
}

#[test]
fn a_failed_command_fails_with_its_stderr() {
    let mut run = |program: &str, args: &[&str], _: Option<&str>| Ok(Output::failed(1, format!("{program} {}: no\n", args.len())));
    let error = call(&mut run, &["terminal", "close", "--terminal", "t1"]).unwrap_err();
    assert_eq!(error.to_string(), "orca terminal close failed: orca 4: no");
}

#[test]
fn a_failed_orca_call_names_orcas_own_error_on_one_line() {
    let failed = |stdout: Value| move |_: &str, _: &[&str], _: Option<&str>| Ok(Output { ok: false, code: Some(1), stdout: stdout.to_string(), stderr: String::new() });
    let args = ["terminal", "send", "--terminal", "t1", "--json"];
    let stale = json!({ "ok": false, "error": { "code": "terminal_handle_stale", "message": "terminal_handle_stale Re-issue
it." } });
    assert_eq!(call(&mut failed(stale), &args).unwrap_err().to_string(), "orca terminal send failed: terminal_handle_stale Re-issue it.");
    let coded = json!({ "ok": false, "error": { "code": "runtime_unavailable", "message": "Orca could not verify." } });
    assert_eq!(call(&mut failed(coded), &args).unwrap_err().to_string(), "orca terminal send failed: runtime_unavailable: Orca could not verify.");
    let refused = json!({ "ok": true, "result": { "send": { "accepted": false, "refusedReason": "no-agent" } } });
    assert_eq!(call(&mut failed(refused), &args).unwrap_err().to_string(), "orca terminal send failed: input refused: no-agent");
    // An error that is there, even null, is never taken for a refusal, which may be sent again.
    let null_error = json!({ "ok": false, "error": null, "result": { "send": { "accepted": false, "refusedReason": "no-agent" } } });
    assert_eq!(call(&mut failed(null_error), &args).unwrap_err().to_string(), "orca terminal send failed");
    // Windows prints a crashpad line on every call: Orca's own error outranks it.
    let crashpad = "[1:ERROR:registration_protocol_win.cc:108] CreateFile: The system cannot find the file specified. (0x2)";
    let down = json!({ "ok": false, "error": { "code": "remote_runtime_unavailable", "message": "Could not connect." } });
    let mut noisy = |_: &str, _: &[&str], _: Option<&str>| Ok(Output { ok: false, code: Some(1), stdout: down.to_string(), stderr: crashpad.to_string() });
    assert_eq!(call(&mut noisy, &args).unwrap_err().to_string(), "orca terminal send failed: remote_runtime_unavailable: Could not connect.");
}

// A read that finds Orca absent waits on it, as it opened nothing; a write, which may have, and
// a read Orca answered with any other failure, fail as Orca's.
#[test]
fn a_read_that_finds_orca_absent_waits_on_it_but_a_write_fails() {
    use crate::error::{Context, Waits};
    let said = |code: &'static str| move |_: &str, _: &[&str], _: Option<&str>| Ok(Output { ok: false, code: Some(1), stdout: json!({ "ok": false, "error": { "code": code, "message": "down" } }).to_string(), stderr: String::new() });
    let mut missing = |_: &str, _: &[&str], _: Option<&str>| Err(std::io::Error::new(std::io::ErrorKind::NotFound, "program not found")).context("orca");
    let mut denied = |_: &str, _: &[&str], _: Option<&str>| Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied")).context("orca");
    assert!(matches!(repos(&mut said("runtime_unavailable"), None), Err(Error::Waits(Waits::Orca, _))));
    assert!(matches!(repos(&mut missing, None), Err(Error::Waits(Waits::Orca, _))));
    assert!(matches!(repos(&mut said("remote_runtime_unavailable"), Some("mac")), Err(Error::Orca(_))));
    assert!(matches!(repos(&mut said("invalid_argument"), None), Err(Error::Orca(_))));
    assert!(matches!(repos(&mut denied, None), Err(Error::Orca(_))));
    let send = ["terminal", "send", "--terminal", "t1", "--json"];
    assert!(matches!(call(&mut said("runtime_unavailable"), &send), Err(Error::Orca(_))));
    assert!(matches!(call(&mut missing, &send), Err(Error::Orca(_))));
}

// A command's listing that finds Orca absent waits on it, from a listing's thread too, until
// the command's first Orca write: sent again after it, the command would repeat that write.
#[test]
fn a_commands_listing_waits_on_an_absent_orca_until_its_first_write() {
    use crate::error::Waits;
    fn ask(_: &str, args: &[&str], _: Option<&str>) -> Result<Output, Error> {
        let absent = json!({ "ok": false, "error": { "code": "runtime_unavailable", "message": "down" } });
        Ok(match args[..2] {
            ["worktree", "ps"] => Output::ok(json!({ "result": { "truncated": false, "worktrees": [] } }).to_string()),
            _ => Output { ok: false, code: Some(1), stdout: absent.to_string(), stderr: String::new() },
        })
    }
    let waits = |result: Result<Listing, Error>| matches!(result, Err(Error::Waits(Waits::Orca, _)));
    let mut run = ask;
    assert!(matches!(Cli::paired(&mut run, Box::leak(Box::default())).waiting().terminals(), Err(Error::Waits(Waits::Orca, _))));
    let mut run = ask;
    assert!(waits(Cli::alone(&mut run).at_once(ask).waiting().listing_on(None)));
    let mut run = ask;
    assert!(matches!(Cli::alone(&mut run).at_once(ask).listing_on(None), Err(Error::Orca(_))));
    let mut run = ask;
    let mut orca = Cli::alone(&mut run).waiting();
    assert!(waits(orca.listing_on(None)));
    assert!(orca.close_all("repo::/wt/tw-1").is_err());
    assert!(matches!(orca.listing_on(None), Err(Error::Orca(_))));
}

// A `Cli` that retries a refused send at once, not seconds apart.
fn quick<'r>(run: &'r mut Run<'r>) -> Cli<'r> {
    Cli { refused_pause: Duration::ZERO, ..Cli::alone(run) }
}

// A fake Orca whose first `failures` `terminal send`s fail printing `first`, and whose
// worktree's agent waits at a prompt when `waiting`.
fn failing_send<'a>(calls: &'a mut Vec<String>, first: Value, waiting: bool, failures: usize) -> impl FnMut(&str, &[&str], Option<&str>) -> Result<Output, Error> + 'a {
    let mut failed = 0;
    move |program, args, _| {
        calls.push(format!("{program} {}", args.join(" ")));
        let ok = |value: Value| Ok(Output::ok(value.to_string()));
        match args[..2] {
            ["terminal", "send"] if failed < failures => {
                failed += 1;
                Ok(Output { ok: false, code: Some(1), stdout: first.to_string(), stderr: String::new() })
            }
            ["terminal", "list"] => ok(json!({ "result": { "terminals": [{ "handle": "t1", "worktreeId": "r1::/wt/tw-1", "agentIdentity": "claude", "tabId": "tab", "leafId": "leaf" }] } })),
            ["worktree", "ps"] => ok(json!({ "result": { "worktrees": [{ "worktreeId": "r1::/wt/tw-1", "repoId": "r1", "path": "/wt/tw-1", "isMainWorktree": false, "agents": [{ "paneKey": "tab:leaf", "state": if waiting { "waiting" } else { "working" } }] }] } })),
            ["terminal", "send"] => ok(receipt(&["input_accepted", "turn_started"], "observed")),
            _ => ok(json!({ "ok": true })),
        }
    }
}

#[test]
fn a_failed_send_is_tried_again_only_as_orcas_own_answer_allows() {
    let send = "orca terminal send --terminal t1 --enter --text New message --wait-submit 30 --json";
    let sends = |calls: &[String]| calls.iter().filter(|c| c.starts_with("orca terminal send")).cloned().collect::<Vec<_>>();
    // With a request ID: re-issued with it, which replays the first send rather than typing again.
    let replay = json!({ "ok": false, "error": { "code": "runtime_unavailable", "message": "lost", "data": { "orchestrationRequestId": "req-1" } } });
    let mut calls = Vec::new();
    assert_eq!(quick(&mut failing_send(&mut calls, replay, false, 1)).send("r1::/wt/tw-1", &"t1".into(), &"New message".into(), Proof::Turn).unwrap(), Typed::Delivered);
    assert_eq!(calls, [send, "orca terminal send --terminal t1 --enter --text New message --wait-submit 30 --retry-request req-1 --json"]);
    // Refused before anything was written: sent again once no agent is read to wait at a prompt.
    let refused = json!({ "ok": true, "result": { "send": { "accepted": false } } });
    let mut calls = Vec::new();
    assert_eq!(quick(&mut failing_send(&mut calls, refused.clone(), false, 1)).send("r1::/wt/tw-1", &"t1".into(), &"New message".into(), Proof::Turn).unwrap(), Typed::Delivered);
    assert_eq!(calls.len(), 4, "{calls:?}");
    assert_eq!(sends(&calls), [send, send]);
    // A refusal that outlasts the first retry is waited out, and gives up after REFUSED_RETRIES.
    let mut calls = Vec::new();
    assert_eq!(quick(&mut failing_send(&mut calls, refused.clone(), false, 3)).send("r1::/wt/tw-1", &"t1".into(), &"New message".into(), Proof::Turn).unwrap(), Typed::Delivered);
    assert_eq!(sends(&calls), [send; 4]);
    let mut calls = Vec::new();
    let error = quick(&mut failing_send(&mut calls, refused.clone(), false, 99)).send("r1::/wt/tw-1", &"t1".into(), &"New message".into(), Proof::Turn).unwrap_err();
    assert_eq!(error.to_string(), "orca terminal send failed: input refused");
    assert_eq!(sends(&calls).len(), 1 + REFUSED_RETRIES as usize);
    // ...but never typed into an agent that now waits at a prompt.
    let mut calls = Vec::new();
    let error = quick(&mut failing_send(&mut calls, refused, true, 1)).send("r1::/wt/tw-1", &"t1".into(), &"New message".into(), Proof::Turn).unwrap_err();
    assert_eq!(error.to_string(), "orca terminal send failed: input refused");
    assert_eq!(sends(&calls), [send]);
    // A refusal waiting can't clear is not waited on.
    let mut calls = Vec::new();
    let no_agent = json!({ "ok": true, "result": { "send": { "accepted": false, "refusedReason": "no-agent" } } });
    let error = quick(&mut failing_send(&mut calls, no_agent, false, 1)).send("r1::/wt/tw-1", &"t1".into(), &"New message".into(), Proof::Turn).unwrap_err();
    assert_eq!((error, calls.as_slice()), (Error::Unsent(Unsent::NoAgent, "orca terminal send failed: input refused: no-agent".into()), &[send.to_string()][..]));
    // ...as is a handle Orca no longer knows; any other failure is Orca's, which may have typed.
    let mut calls = Vec::new();
    let stale = json!({ "ok": false, "error": { "code": "terminal_handle_stale", "message": "gone" } });
    let error = quick(&mut failing_send(&mut calls, stale, false, 1)).send("r1::/wt/tw-1", &"t1".into(), &"New message".into(), Proof::Turn).unwrap_err();
    assert!(matches!(error, Error::Unsent(Unsent::TerminalGone, _)), "{error}");
    let mut calls = Vec::new();
    let unsure = json!({ "ok": false, "error": { "code": "terminal_handle_stale", "message": "gone", "data": { "retrySafe": false } } });
    assert!(matches!(quick(&mut failing_send(&mut calls, unsure, false, 1)).send("r1::/wt/tw-1", &"t1".into(), &"New message".into(), Proof::Turn), Err(Error::Orca(_))));
    let mut calls = Vec::new();
    let lost = json!({ "ok": false, "error": { "code": "runtime_error", "message": "lost" } });
    assert!(matches!(quick(&mut failing_send(&mut calls, lost, false, 1)).send("r1::/wt/tw-1", &"t1".into(), &"New message".into(), Proof::Turn), Err(Error::Orca(_))));
    // Delivery Orca can't prove, or a failure that printed no JSON: never sent again.
    let unknown = json!({ "ok": false, "error": { "code": "runtime_error", "message": "maybe", "data": { "deliveryOutcome": "unknown", "retrySafe": false } } });
    for first in [unknown, Value::Null, json!({ "ok": false, "error": { "code": "runtime_error", "message": "lost" } })] {
        let mut calls = Vec::new();
        assert!(quick(&mut failing_send(&mut calls, first, false, 1)).send("r1::/wt/tw-1", &"t1".into(), &"New message".into(), Proof::Turn).is_err());
        assert_eq!(calls, [send]);
    }
}

// A field the call relies on that Orca's answer lacks fails it, naming the call; a terminal
// made whatever Orca printed fails as Orca's, and an empty handle, which would name the
// active terminal, names none.
#[test]
fn an_answer_lacking_a_field_fails_naming_the_call() {
    let answer = |args: &[&str]| match args[..2] {
        ["worktree", "create"] => json!({ "ok": true, "result": { "worktree": { "id": "r1::/wt/tw-1" } } }),
        ["terminal", "create"] => json!({ "ok": true, "result": { "terminal": { "handle": "" } } }),
        _ => json!({ "ok": true, "result": {} }),
    };
    let mut calls = Vec::new();
    let mut run = shell(&mut calls, &answer);
    let health = Health::default();
    let mut orca = Cli::new(&mut run, &health);
    let failed = |result: Result<(String, String), Error>| result.unwrap_err().to_string();
    assert_eq!(failed(orca.create_worktree(Place::Here { repo_id: "r1" }, "tw-1", &[])), "Orca's answer to worktree create didn't decode: it had no `path`");
    assert!(matches!(orca.open("r1::/wt/tw-1", None, &CommandLine::new(&"claude".into())), Err(Error::Orca(m)) if m == "Orca's answer to terminal create named no terminal"));
    assert_eq!(orca.repos(None).unwrap_err().to_string(), "Orca's answer to repo list didn't decode: it had no `repos`");
}

#[test]
fn a_wait_for_exit_tells_an_exit_from_a_timeout_and_an_error() {
    let answer = |ok: bool, said: Value| move |_: &str, _: &[&str], _: Option<&str>| Ok(Output { ok, code: Some(i32::from(!ok)), stdout: said.to_string(), stderr: String::new() });
    let exited = json!({"ok": true, "result": {"wait": {"satisfied": true, "status": "exited", "exitCode": 3}}});
    assert!(wait_exit(&mut answer(true, exited.clone()), &Health::default(), &"term_1".into(), None).unwrap());
    assert_eq!(exit_code(&mut answer(true, exited), &"term_1".into()).unwrap(), Waited::Exited(Some(3)));
    // A wait that ran with an answer of another shape proves no exit.
    assert_eq!(exit_code(&mut answer(true, json!({"ok": true, "result": {}})), &"term_1".into()).unwrap(), Waited::Open);
    let timeout = json!({"ok": false, "error": {"code": "timeout", "message": "timeout"}});
    assert!(!wait_exit(&mut answer(false, timeout), &Health::default(), &"term_1".into(), None).unwrap());
    // Gone before the wait began.
    let stale = json!({"ok": false, "error": {"code": "terminal_handle_stale", "message": "terminal_handle_stale"}});
    assert!(wait_exit(&mut answer(false, stale.clone()), &Health::default(), &"term_1".into(), None).unwrap());
    assert_eq!(exit_code(&mut answer(false, stale), &"term_1".into()).unwrap(), Waited::Stale);
    // An error field of another shape hides not its code.
    let odd = json!({"ok": false, "error": {"code": "timeout", "message": 7, "data": "x"}});
    assert_eq!(exit_code(&mut answer(false, odd), &"term_1".into()).unwrap(), Waited::Open);
    let odd = json!({"ok": false, "error": {"code": "terminal_handle_stale", "data": {"retrySafe": "false"}}});
    assert_eq!(exit_code(&mut answer(false, odd), &"term_1".into()).unwrap(), Waited::Stale);
    let refused = json!({"ok": false, "error": {"code": "runtime_unavailable", "message": "runtime_unavailable"}});
    assert!(wait_exit(&mut answer(false, refused), &Health::default(), &"term_1".into(), None).is_err());
}

// A line is typed again only into an agent just listed as not waiting: with no listing to
// tell, the agent counts as waiting and the first send's error is the answer.
#[test]
fn a_refused_send_is_not_typed_again_when_the_listing_fails() {
    let mut calls: Vec<String> = Vec::new();
    let mut run = |program: &str, args: &[&str], _: Option<&str>| {
        calls.push(format!("{program} {}", args.join(" ")));
        let refused = json!({ "ok": true, "result": { "send": { "accepted": false } } });
        match args[..2] {
            ["terminal", "send"] => Ok(Output { ok: false, code: Some(1), stdout: refused.to_string(), stderr: String::new() }),
            _ => Err(Error::Failed("orca is gone".into())),
        }
    };
    let error = quick(&mut run).send("r1::/wt/tw-1", &"t1".into(), &"New message".into(), Proof::Turn).unwrap_err();
    assert_eq!(error.to_string(), "orca terminal send failed: input refused");
    assert_eq!(calls.iter().filter(|c| c.starts_with("orca terminal send")).count(), 1, "{calls:?}");
}

#[test]
fn a_cards_comment_is_read_where_orca_shows_it_and_a_dash_never_reaches_it() {
    let shown = |comment: Value| json!({ "ok": true, "result": { "worktree": { "id": "r1::D:/wt/tw-1", "comment": comment } } });
    let (mut calls, said) = (Vec::new(), shown(json!("PR #7, auto-merge on.")));
    let answer = |_: &[&str]| said.clone();
    assert_eq!(comment(&mut shell(&mut calls, &answer), "r1::D:/wt/tw-1").unwrap().as_deref(), Some("PR #7, auto-merge on."));
    assert_eq!(calls, ["orca worktree show --worktree id:r1::D:/wt/tw-1 --json"]);
    let empty = shown(json!(""));
    let answer = |_: &[&str]| empty.clone();
    assert_eq!(comment(&mut shell(&mut Vec::new(), &answer), "r1::D:/wt/tw-1").unwrap(), None);
    let mut calls = Vec::new();
    let answer = |_: &[&str]| json!({ "ok": true });
    assert!(set_comment(&mut shell(&mut calls, &answer), "r1::D:/wt/tw-1", "--force").is_err() && calls.is_empty());
    set_comment(&mut shell(&mut calls, &answer), "r1::D:/wt/tw-1", "Committed: x").unwrap();
    assert_eq!(calls, ["orca worktree set --worktree id:r1::D:/wt/tw-1 --comment Committed: x --json"]);
    calls.clear();
    set_status(&mut shell(&mut calls, &answer), "mac@r1::/wt/tw-1", Status::InProgress).unwrap();
    assert_eq!(calls, ["orca worktree set --worktree id:r1::/wt/tw-1 --workspace-status in-progress --environment mac --json"]);
}

// Two hosts: this machine's Orca, and `mac`'s, reached as the environment it is paired as.
fn two_hosts<'a>(calls: &'a mut Vec<String>, mac_fails: bool) -> impl FnMut(&str, &[&str], Option<&str>) -> Result<Output, Error> + 'a {
    move |program, args, dir| {
        calls.push(format!("{}{program} {}", dir.map(|d| format!("[{d}] ")).unwrap_or_default(), args.join(" ")));
        let mac = args.windows(2).any(|pair| pair == ["--environment", "mac"] || pair == ["--host", "runtime:mac"]);
        if program == "git" {
            return Ok(Output::ok("https://github.com/Tolkien-Works/tolkster.git\n"));
        }
        if mac && mac_fails {
            return Ok(Output::failed(1, "runtime_unavailable"));
        }
        // A routed reply calls each of its rows `local`, and omits the hosts it doesn't cover.
        let mut remote = row(Some("TW-2"), "/Users/x/tw-2");
        remote["worktreeId"] = json!("r9::/Users/x/tw-2");
        remote["repoId"] = json!("r9");
        let said = match (&args[..2], mac) {
            (["worktree", "ps"], false) => json!({ "result": { "truncated": false, "hostScope": { "omittedHostIds": ["runtime:mac"] }, "worktrees": [row(Some("TW-1"), "D:/wt/tw-1"), primary()] } }),
            (["worktree", "ps"], true) => json!({ "result": { "truncated": false, "hostScope": { "omittedHostIds": ["runtime:pc"] }, "worktrees": [remote] } }),
            (["terminal", "list"], false) => json!({ "result": { "terminals": [{ "handle": "term_1", "worktreeId": "repo::D:/wt/tw-1", "agentIdentity": "claude" }] } }),
            (["terminal", "list"], true) => json!({ "result": { "terminals": [{ "handle": "term_1", "worktreeId": "r9::/Users/x/tw-2", "agentIdentity": "claude" }] } }),
            (["host", "list"], false) => json!({ "result": { "hosts": [
                { "kind": "local", "name": "this machine", "id": "local" },
                { "kind": "environment", "name": "Mac", "id": "mac" },
                { "kind": "ssh", "name": "box", "id": "ssh-box" },
                { "kind": "environment", "name": "odd", "id": "-odd" },
            ] } }),
            (["project", "setups"], true) => json!({ "result": { "setups": [
                { "projectId": "github:acme/app", "setupState": "ready", "path": "/Users/x/app", "kind": "git" },
                { "projectId": "github:acme/web", "setupState": "cloning", "path": "/Users/x/web", "kind": "git" },
            ] } }),
            (["repo", "list"], true) => json!({ "result": { "repos": [{ "id": "r9", "displayName": "app", "path": "/Users/x/app", "gitRemoteIdentity": { "canonicalKey": "github.com/Acme/app" } }] } }),
            (["worktree", "create"], true) => json!({ "result": { "worktree": { "id": "r9::/Users/x/tw-3", "path": "/Users/x/tw-3" } } }),
            (["host", "name"], true) => json!({ "result": { "platform": "darwin" } }),
            (["terminal", "create"], _) => json!({ "result": { "terminal": { "handle": "term_2" } } }),
            (["terminal", "send"], _) => receipt(&["turn_started"], ""),
            (["terminal", "wait"], _) => json!({ "result": { "wait": { "satisfied": true, "exitCode": 3 } } }),
            _ => json!({ "ok": true }),
        };
        Ok(Output::ok(said.to_string()))
    }
}

#[test]
fn every_host_is_listed_and_merged_each_row_with_the_host_and_origin_of_the_call_that_listed_it() {
    let mut calls = Vec::new();
    let (worktrees, listed, terminals) = {
        let mut run = two_hosts(&mut calls, false);
        let mut orca = Cli::with_hosts(&mut run, &["mac"]);
        let worktrees = orca.worktrees().unwrap();
        (worktrees, orca.listed().unwrap(), orca.terminals().unwrap())
    };
    let named: Vec<(&str, bool, Option<&str>)> = worktrees.iter().map(|w| (w.id.as_str(), w.is_local, w.origin.as_deref())).collect();
    assert_eq!(
        named,
        [
            ("repo::D:/wt/tw-1", true, Some("Tolkien-Works/tolkster")),
            ("repo::D:/main", true, Some("Tolkien-Works/tolkster")),
            ("mac@r9::/Users/x/tw-2", false, Some("Acme/app")),
        ]
    );
    assert_eq!(worktrees[2].host(), Some("mac"));
    // Each host's call covers its own rows, whatever others it omits.
    assert_eq!(listed, Some(vec!["repo::D:/wt/tw-1".into(), "repo::D:/main".into(), "mac@r9::/Users/x/tw-2".into()]));
    // Two hosts may hand out the same handle: each goes back to its own.
    assert_eq!(agent_terminal(&terminals, "repo::D:/wt/tw-1").as_deref(), Some("term_1"));
    assert_eq!(agent_terminal(&terminals, "mac@r9::/Users/x/tw-2").as_deref(), Some("mac@term_1"));
    assert_eq!(terminals.len(), 2);
    assert_eq!(candidates(&worktrees).len(), 2);
    assert!(calls.contains(&"orca worktree ps --limit 1000 --environment mac --json".to_string()), "{calls:?}");
    assert!(calls.contains(&"orca repo list --environment mac --json".to_string()), "{calls:?}");
    // Git reads only this machine's checkouts.
    assert!(calls.iter().filter(|c| c.contains("git ")).all(|c| c.starts_with("[D:/main]")), "{calls:?}");
}

#[test]
fn the_hosts_are_the_paired_environments_with_plain_ids() {
    let mut calls = Vec::new();
    let mut run = two_hosts(&mut calls, false);
    let mut orca = Cli::paired(&mut run, Box::leak(Box::default()));
    orca.agents().unwrap();
    assert_eq!(orca.answering(), [Host { id: "mac".into(), name: "Mac".into() }]);
}

// A listing is made only from a list of hosts that was read: a failed `orca host list`, or a
// reply with no list, fails it, so nothing is taken as unpaired.
#[test]
fn a_host_list_that_fails_or_names_no_hosts_fails_the_listing() {
    for reply in [None, Some(json!({ "result": {} })), Some(json!({ "result": { "hosts": [] } }))] {
        let mut run = |_: &str, _: &[&str], _: Option<&str>| Ok(match &reply {
            None => Output::failed(1, "runtime_unavailable"),
            Some(said) => Output::ok(said.to_string()),
        });
        let mut orca = Cli::paired(&mut run, Box::leak(Box::default()));
        assert!(orca.agents_listed().is_err());
        assert!(orca.terminals().is_err());
    }
}

#[test]
fn a_project_id_is_the_repo_in_lower_case() {
    assert_eq!(project_id("Tolkien-Works/Tolkster"), "github:tolkien-works/tolkster");
}

// `two_hosts`, but a terminal's wait ends with `exit`.
fn exiting<'a>(calls: &'a mut Vec<String>, exit: i64) -> impl FnMut(&str, &[&str], Option<&str>) -> Result<Output, Error> + 'a {
    let mut inner = two_hosts(calls, false);
    move |program, args, dir| match args[..2] {
        ["terminal", "wait"] => Ok(Output::ok(json!({ "result": { "wait": { "satisfied": true, "exitCode": exit } } }).to_string())),
        _ => inner(program, args, dir),
    }
}

#[test]
fn a_repo_is_ready_on_a_host_only_once_set_up_there() {
    let mut calls = Vec::new();
    {
        let mut run = exiting(&mut calls, 0);
        let mut orca = Cli::with_hosts(&mut run, &["mac"]);
        assert!(orca.ready("mac", Some("github:acme/app"), reads_config).unwrap());
        assert!(!orca.ready("mac", Some("github:acme/web"), reads_config).unwrap());
        assert!(!orca.ready("mac", Some("github:acme/api"), reads_config).unwrap());
        assert!(orca.ready("mac", None, reads_config).unwrap());
        assert_eq!(orca.ready_projects("mac", reads_config).unwrap(), ["github:acme/app"]);
    }
    assert_eq!(calls[0], "orca project setups --project github:acme/app --host runtime:mac --json");
    // The host's `tolkster config` ran once, in the folder of the setup there, and its verdict was kept.
    let created: Vec<_> = calls.iter().filter(|c| c.contains("terminal create")).collect();
    assert_eq!(created.len(), 1, "{calls:?}");
    assert!(created[0].contains("--worktree path:/Users/x/app") && created[0].contains("tolkster\" config"), "{created:?}");
}

// A host whose `tolkster` can't read its config.json takes no work, and says so until a later
// check finds it rejoined.
#[test]
fn a_host_whose_tolkster_cannot_read_its_config_is_not_ready() {
    let health = Health::default();
    let mut calls = Vec::new();
    {
        let mut run = exiting(&mut calls, 2);
        let mut orca = Cli::sharing(&mut run, &["mac"], &health);
        assert!(!orca.ready("mac", Some("github:acme/app"), reads_config).unwrap());
        assert!(!orca.ready("mac", None, reads_config).unwrap());
        assert!(orca.ready_projects("mac", reads_config).unwrap().is_empty());
        assert!(health.refused("mac", Instant::now()));
    }
    assert_eq!(calls.iter().filter(|c| c.contains("terminal create")).count(), 1, "{calls:?}");
}

// One that doesn't answer is left out, and what it holds is unknown: no listing is complete.
#[test]
fn a_host_that_does_not_answer_is_left_out_of_every_listing() {
    let mut calls = Vec::new();
    let mut run = two_hosts(&mut calls, true);
    let mut orca = Cli::with_hosts(&mut run, &["mac"]);
    let (worktrees, listed) = orca.agents_listed().unwrap();
    assert!(worktrees.iter().all(|w| w.host().is_none()) && listed.is_some_and(|ids| ids.len() == 2));
    assert_eq!(orca.listed().unwrap(), None);
    assert_eq!(orca.terminals().unwrap().len(), 1);
    assert_eq!((orca.unanswered(), orca.answering()), (vec!["mac".to_string()], Vec::new()));
    // Nor is a worktree on a host no longer paired heard: neither is known gone.
    assert!(orca.heard("repo::D:/wt/tw-1") && !orca.heard("mac@r9::/Users/x/tw-2") && !orca.heard("pi@r9::/Users/x/tw-2"));
}

// A host that didn't answer is asked by no later command or listing until `listen`'s
// background ask hears it, and counts as silent meanwhile: a call aimed at it fails at once.
#[test]
fn a_silent_host_is_asked_by_nothing_until_it_answers_again() {
    let health = Health::default();
    let mut calls = Vec::new();
    {
        let mut run = two_hosts(&mut calls, true);
        Cli::sharing(&mut run, &["mac"], &health).agents().unwrap();
    }
    assert!(health.silent("mac"));
    calls.clear();
    {
        let mut run = two_hosts(&mut calls, false);
        let mut orca = Cli::sharing(&mut run, &["mac"], &health);
        assert_eq!(orca.agents().unwrap().len(), 2);
        assert_eq!(orca.listed().unwrap(), None);
        assert_eq!(orca.terminals().unwrap().len(), 1);
        assert_eq!((orca.unanswered(), orca.answering()), (vec!["mac".to_string()], Vec::new()));
        let mac = "mac@r9::/Users/x/tw-2";
        let later = |result: Result<(), Error>| assert!(matches!(result, Err(Error::Waits(crate::error::Waits::Host(ref host), _)) if host == "mac"), "{result:?}");
        later(orca.send(mac, &"mac@term_1".into(), &"New message".into(), Proof::Turn).map(|_| ()));
        later(orca.type_line(&"mac@term_1".into(), &"go".into()));
        later(orca.open(mac, None, &CommandLine::new(&"claude".into())).map(|_| ()));
        later(orca.close(&"mac@term_1".into()));
        later(orca.close_all(mac));
        later(orca.remove_worktree(mac));
        later(orca.create_worktree(Place::On { host: "mac", project: "github:acme/app" }, "tw-3", &[]).map(|_| ()));
        later(orca.ready("mac", None, reads_config).map(|_| ()));
        later(orca.repos(Some("mac")).map(|_| ()));
    }
    later_wait(wait_exit(&mut two_hosts(&mut calls, false), &health, &"mac@term_1".into(), None));
    assert!(calls.iter().all(|c| !c.contains("mac")), "{calls:?}");
    // Its first answer puts it back in rotation.
    health::ask_again(&health, || Some(vec!["mac".into()]), |_| true, || Instant::now() + health::AGAIN);
    let mut run = two_hosts(&mut calls, false);
    assert_eq!(Cli::sharing(&mut run, &["mac"], &health).agents().unwrap().len(), 3);
}

// The background ask hears a host only on both listings whose failure makes it silent.
#[test]
fn a_host_answers_only_when_both_its_listings_do() {
    let mut calls = Vec::new();
    assert!(answers(&mut two_hosts(&mut calls, false), "mac"));
    assert!(!answers(&mut two_hosts(&mut calls, true), "mac"));
    let mut listings = two_hosts(&mut calls, false);
    let mut run = move |program: &str, args: &[&str], dir: Option<&str>| match args.first() {
        Some(&"terminal") => Ok(Output::failed(1, "runtime_unavailable")),
        _ => listings(program, args, dir),
    };
    assert!(!answers(&mut run, "mac"));
}

fn later_wait(result: Result<bool, Error>) {
    assert!(matches!(result, Err(Error::Waits(crate::error::Waits::Host(ref host), _)) if host == "mac"), "{result:?}");
}

// A host that answered and then fails fails the call, as this machine's does: a command never
// sees a host's worktrees and not its terminals.
#[test]
fn a_host_that_answered_and_then_fails_fails_the_call() {
    let mut calls = Vec::new();
    let mut listings = two_hosts(&mut calls, false);
    let mut run = move |program: &str, args: &[&str], dir: Option<&str>| match args.first() {
        Some(&"terminal") if args.contains(&"mac") => Ok(Output::failed(1, "runtime_unavailable")),
        _ => listings(program, args, dir),
    };
    let mut orca = Cli::with_hosts(&mut run, &["mac"]);
    assert_eq!(orca.agents().unwrap().len(), 3);
    assert!(orca.terminals().is_err());
}

// An ID or handle may name a host from the Durable Object's records: one this machine doesn't
// ask is never reached.
#[test]
fn a_host_this_machine_does_not_ask_is_never_reached() {
    let mut calls = Vec::new();
    {
        let mut run = two_hosts(&mut calls, false);
        let mut orca = Cli::alone(&mut run);
        assert!(orca.close(&"mac@term_2".into()).is_err());
        assert!(orca.remove_worktree("mac@r9::/Users/x/tw-3").is_err());
        assert!(orca.create_worktree(Place::On { host: "mac", project: "github:acme/app" }, "tw-3", &[]).is_err());
        assert!(orca.ready("mac", Some("github:acme/app"), reads_config).is_err());
    }
    assert!(calls.is_empty(), "{calls:?}");
}

#[test]
fn each_call_reaches_the_host_its_worktree_or_handle_names() {
    let mut calls = Vec::new();
    {
        let mut run = two_hosts(&mut calls, false);
        let mut orca = Cli::with_hosts(&mut run, &["mac"]);
        let mac = "mac@r9::/Users/x/tw-3";
        assert_eq!(orca.create_worktree(Place::On { host: "mac", project: "github:acme/app" }, "tw-3", &["--no-parent"]).unwrap(), (mac.to_string(), "/Users/x/tw-3".to_string()));
        let handle: Handle = orca.open(mac, None, &CommandLine::new(&"claude".into())).unwrap().as_str().into();
        assert_eq!(handle, "mac@term_2");
        assert_eq!(orca.send(mac, &handle, &"New message".into(), Proof::Turn).unwrap(), Typed::Delivered);
        orca.type_line(&handle, &"go".into()).unwrap();
        orca.close(&handle).unwrap();
        orca.close_all(mac).unwrap();
        orca.name_worktree(mac, "tw-3").unwrap();
        orca.remove_worktree(mac).unwrap();
        assert_eq!(orca.run_to_exit(mac, &CommandLine::new(&"tolkster reap-here".into()).exiting()).unwrap(), Some(3));
        assert_eq!(orca.repos(Some("mac")).unwrap()[0].id, "r9");
        // This machine's calls are as they are with one host.
        orca.close(&"term_1".into()).unwrap();
    }
    assert!(wait_exit(&mut two_hosts(&mut calls, false), &Health::default(), &"mac@term_2".into(), None).unwrap());
    set_comment(&mut two_hosts(&mut calls, false), "mac@r9::/Users/x/tw-3", "Committed: x").unwrap();
    assert_eq!(
        calls,
        [
            "orca worktree create --project github:acme/app --host runtime:mac --name tw-3 --no-parent --json",
            "orca terminal create --worktree id:r9::/Users/x/tw-3 --command claude --environment mac --json",
            "orca terminal send --terminal term_2 --enter --text New message --wait-submit 30 --environment mac --json",
            "orca terminal send --terminal term_2 --enter --text go --environment mac --json",
            "orca terminal close --terminal term_2 --environment mac --json",
            "orca terminal close --worktree id:r9::/Users/x/tw-3 --all --environment mac --json",
            "orca worktree set --worktree id:r9::/Users/x/tw-3 --display-name tw-3 --environment mac --json",
            "orca worktree rm --worktree id:r9::/Users/x/tw-3 --force --environment mac --json",
            "orca terminal create --worktree id:r9::/Users/x/tw-3 --command tolkster reap-here; exit $LASTEXITCODE --environment mac --json",
            "orca terminal wait --terminal term_2 --for exit --timeout-ms 300000 --environment mac --json",
            "orca terminal close --terminal term_2 --environment mac --json",
            "orca repo list --environment mac --json",
            "orca terminal close --terminal term_1 --json",
            "orca terminal wait --terminal term_2 --for exit --timeout-ms 300000 --environment mac --json",
            "orca worktree set --worktree id:r9::/Users/x/tw-3 --comment Committed: x --environment mac --json",
        ]
    );
}

// A worktree's host is listed there alone, each time asked: no other host is reached, and one
// unpaired or silent fails at once.
#[test]
fn a_listing_at_a_worktree_reads_its_host_alone_each_time() {
    let mut calls = Vec::new();
    {
        let mut run = two_hosts(&mut calls, false);
        let mut orca = Cli::with_hosts(&mut run, &["mac"]);
        let listing = orca.listing_at("mac@r9::/Users/x/tw-2").unwrap();
        assert_eq!(listing.worktree("mac@r9::/Users/x/tw-2").map(|w| w.host()), Some(Some("mac")));
        assert_eq!((listing.terminals.len(), listing.lacks("mac@r9::/Users/x/tw-2"), listing.lacks("mac@r9::/Users/x/tw-3")), (1, false, true));
        orca.listing_at("mac@r9::/Users/x/tw-2").unwrap();
        assert_eq!(orca.listing_at("repo::D:/wt/tw-1").unwrap().worktrees.len(), 2);
        assert!(orca.listing_at("pi@r9::/x").is_err());
    }
    assert_eq!(
        calls,
        [
            "orca worktree ps --limit 1000 --environment mac --json",
            "orca terminal list --limit 1000 --environment mac --json",
            "orca worktree ps --limit 1000 --environment mac --json",
            "orca terminal list --limit 1000 --environment mac --json",
            "orca worktree ps --limit 1000 --json",
            "orca terminal list --limit 1000 --json",
        ]
    );
    let mut run = two_hosts(&mut calls, true);
    let mut orca = Cli::with_hosts(&mut run, &["mac"]);
    later(orca.listing_at("mac@r9::/Users/x/tw-2").map(|_| ()));
    later(orca.listing_at("mac@r9::/Users/x/tw-2").map(|_| ()));
}

fn later(result: Result<(), Error>) {
    assert!(matches!(result, Err(Error::Waits(crate::error::Waits::Host(ref host), _)) if host == "mac"), "{result:?}");
}

// Each host's two listings on a thread of their own, merged in the hosts' order: one that
// fails is silent as when asked in turn.
#[test]
fn a_listing_asks_every_host_at_once() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static ASKING: AtomicUsize = AtomicUsize::new(0);
    static MOST: AtomicUsize = AtomicUsize::new(0);
    fn ask(_: &str, args: &[&str], _: Option<&str>) -> Result<Output, Error> {
        MOST.fetch_max(ASKING.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(100));
        ASKING.fetch_sub(1, Ordering::SeqCst);
        let host = args.windows(2).find(|pair| pair[0] == "--environment").map_or("here", |pair| pair[1]);
        if host == "off" {
            return Ok(Output::failed(1, "runtime_unavailable"));
        }
        Ok(Output::ok(match args[..2] {
            ["worktree", "ps"] => json!({ "result": { "worktrees": [row(Some(&format!("TW-{}", host.len())), &format!("/{host}/wt"))] } }),
            _ => json!({ "result": { "terminals": [{ "handle": "t1", "worktreeId": format!("repo::/{host}/wt"), "agentIdentity": "claude" }] } }),
        }.to_string()))
    }
    // One call of each listing goes through `run`, whose beat shows the job at work, though no
    // call is this machine's.
    let mut beats = 0;
    {
        let mut run = |program: &str, args: &[&str], dir: Option<&str>| {
            beats += 1;
            ask(program, args, dir)
        };
        let mut orca = Cli::with_hosts(&mut run, &["mac", "off", "pi"]).at_once(ask);
        let ids: Vec<String> = orca.agents().unwrap().into_iter().map(|w| w.id).collect();
        assert_eq!(ids, ["repo::/here/wt", "mac@repo::/mac/wt", "pi@repo::/pi/wt"]);
        assert_eq!(orca.terminals().unwrap().len(), 3);
        assert_eq!(orca.unanswered(), ["off"]);
        assert_eq!(orca.listing_on(Some("pi")).unwrap().worktrees.len(), 1);
    }
    assert_eq!(beats, 3);
    assert!(MOST.load(Ordering::SeqCst) >= 2, "{}", MOST.load(Ordering::SeqCst));
}
