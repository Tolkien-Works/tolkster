use serde_json::json;

use super::*;
use crate::harness::events::moments;
use crate::time::instant;

const T0: &str = "2026-10-24T01:00:00.000Z";

fn at(seconds: i64) -> String {
    iso(instant(T0).unwrap() + seconds * 1000)
}

fn prompt(s: i64) -> String {
    json!({ "type": "user", "timestamp": at(s), "message": { "content": "Do the task in D:/wt/secret/brief.md" } }).to_string()
}

// A reply `id` at `s`, calling `tool` with `input` as `call`, or with text alone.
fn reply(s: i64, id: &str, call: Option<(&str, &str, serde_json::Value)>) -> String {
    let parts = match call {
        Some((call, tool, input)) => json!([{ "type": "tool_use", "id": call, "name": tool, "input": input }]),
        None => json!([{ "type": "text", "text": "secret prose" }]),
    };
    json!({ "type": "assistant", "timestamp": at(s), "message": { "id": id, "content": parts } }).to_string()
}

fn result(s: i64, call: &str, failed: bool, launched: bool) -> String {
    let returned = if launched { json!({ "backgroundTaskId": "b1", "stdout": "OUTPUT-TEXT" }) } else { json!({ "stdout": "OUTPUT-TEXT" }) };
    json!({ "type": "user", "timestamp": at(s), "message": { "content": [{ "type": "tool_result", "tool_use_id": call, "is_error": failed, "content": "OUTPUT-TEXT at D:/wt/secret" }] }, "toolUseResult": returned }).to_string()
}

fn notice(s: i64, call: &str, status: &str) -> String {
    json!({ "type": "queue-operation", "operation": "enqueue", "timestamp": at(s), "content": format!("<task-notification> <task-id>x</task-id> <tool-use-id>{call}</tool-use-id> <status>{status}</status> <summary>OUTPUT-TEXT</summary> </task-notification>") }).to_string()
}

fn shell(command: &str) -> serde_json::Value {
    json!({ "command": command })
}

// A worker's run: it reads, edits, tests, commits and ends a turn, then after ten minutes
// waiting runs the gate in the background, reviews with a sub-agent, and opens its PR.
fn session() -> Vec<String> {
    vec![
        prompt(0),
        reply(2, "m1", Some(("r1", "Read", json!({ "file_path": "D:/wt/secret/src/lib.rs" })))),
        result(3, "r1", false, false),
        reply(10, "m2", Some(("e1", "Edit", json!({ "file_path": "D:/wt/secret/src/lib.rs", "old_string": "a", "new_string": "b" })))),
        result(11, "e1", false, false),
        reply(15, "m3", Some(("c1", "Bash", shell("cargo test --locked --manifest-path cli/Cargo.toml secretword")))),
        result(75, "c1", false, false),
        reply(80, "m4", Some(("g1", "Bash", shell("git commit -m 'secret subject' && git push")))),
        result(82, "g1", false, false),
        reply(85, "m5", None),
        prompt(685),
        reply(690, "m6", Some(("b1", "Bash", shell("node scripts/gate.ts")))),
        result(691, "b1", false, true),
        reply(692, "m7", Some(("rp", "Bash", shell("tolkster pr review-pass --size above --session s-1")))),
        result(695, "rp", false, false),
        reply(700, "m8", Some(("a1", "Agent", json!({ "description": "secret review", "prompt": "secret" })))),
        json!({ "type": "user", "timestamp": at(701), "message": { "content": [{ "type": "tool_result", "tool_use_id": "a1", "content": "OUTPUT-TEXT" }] }, "toolUseResult": { "isAsync": true, "status": "async_launched" } }).to_string(),
        notice(900, "a1", "completed"),
        notice(960, "b1", "failed"),
        reply(965, "m9", Some(("p1", "Bash", shell("tolkster pr create --head secret-branch --body-file - --session s-1")))),
        result(970, "p1", false, false),
    ]
}

fn sub_agent() -> Vec<String> {
    vec![reply(710, "s1m", Some(("s1", "Bash", shell("cargo clippy --locked --manifest-path cli/Cargo.toml -- -D warnings")))), result(740, "s1", false, false)]
}

fn summarised(main: &[String], subs: &[Vec<String>]) -> Summary {
    let launched = instant(T0).unwrap() - 1000;
    summary("0a1b2c3d-0000-4000-8000-000000000001", (Kind::Start, launched, None), moments(&main.join("\n")), subs.iter().map(|lines| moments(&lines.join("\n"))).collect()).unwrap()
}

#[test]
fn a_session_is_summarised_from_its_transcripts_moments() {
    let summary = summarised(&session(), &[sub_agent()]);
    assert_eq!((summary.at.as_str(), summary.wall, summary.active, summary.model), (T0, 970, 370, 37));
    assert_eq!(summary.tools, Tools { read: 1, edit: 1, check: 330, git: 2, bookkeeping: 8, agent: 200, other: 0 });
    assert_eq!((summary.api_turns, summary.tool_turns), (9, 8));
    assert_eq!(summary.offsets, Offsets { first_edit: Some(10), first_commit: Some(80), first_gate: Some(690), review_start: Some(692), review_end: Some(900), pr: Some(965) });
    assert_eq!(
        summary.checks,
        [
            CheckRun { kind: CheckKind::CargoTest, failed: Some(false), edited: true, seconds: 60, at: 15, by: By::Main },
            CheckRun { kind: CheckKind::Gate, failed: Some(true), edited: false, seconds: 270, at: 690, by: By::Main },
            CheckRun { kind: CheckKind::Clippy, failed: Some(false), edited: false, seconds: 30, at: 710, by: By::SubAgent },
        ]
    );
}

#[test]
fn the_summary_carries_no_command_text_path_or_output() {
    let text = serde_json::to_string(&summarised(&session(), &[sub_agent()])).unwrap();
    for leaked in ["secret", "OUTPUT", "D:/", "wt/", "Cargo.toml", "scripts", "gate.ts", "--", "s-1", "lib.rs"] {
        assert!(!text.contains(leaked), "{leaked} in {text}");
    }
}

#[test]
fn only_what_came_after_the_launch_counts() {
    let mut earlier = vec![prompt(-600), reply(-590, "m0", Some(("c0", "Bash", shell("cargo test")))), result(-500, "c0", true, false)];
    earlier.extend(session());
    let summary = summarised(&earlier, &[]);
    assert_eq!((summary.at.as_str(), summary.wall, summary.api_turns), (T0, 970, 9));
    assert_eq!(summary.checks.len(), 2);
    assert!(summary.checks[0].edited);
}

#[test]
fn a_call_running_across_a_gap_keeps_it_active() {
    let lines = [prompt(0), reply(1, "m1", Some(("c1", "Bash", shell("cargo test")))), result(601, "c1", false, false), reply(603, "m2", None)];
    let summary = summarised(&lines.map(String::from), &[]);
    assert_eq!((summary.wall, summary.active, summary.tools.check), (603, 603, 600));
}

#[test]
fn a_check_whose_end_another_program_decides_has_no_outcome() {
    let lines = [prompt(0), reply(1, "m1", Some(("c1", "Bash", shell("cargo test 2>&1 | grep -E 'test result'")))), result(9, "c1", false, false)];
    assert_eq!(summarised(&lines.map(String::from), &[]).checks[0].failed, None);
}

#[test]
fn nothing_since_the_launch_is_no_summary() {
    let launched = instant(T0).unwrap() + 10_000;
    assert!(summary("s", (Kind::Round, launched, None), moments(&prompt(0)), Vec::new()).is_none());
}

#[test]
fn check_runs_are_told_apart_by_kind() {
    for (command, check) in [
        ("node scripts/gate.ts --all", CheckKind::Gate),
        ("node --test test/machine-sim.test.ts 2>&1 | grep x", CheckKind::Simulator),
        ("node --test test/realtime.test.ts", CheckKind::NodeTest),
        ("cargo test --locked --manifest-path cli/Cargo.toml hook", CheckKind::CargoTest),
        ("cargo clippy --all-targets -- -D warnings", CheckKind::Clippy),
        ("cd cli && cargo check", CheckKind::CargoCheck),
        ("npm run typecheck 2>&1 | tail -n 3", CheckKind::Typecheck),
        ("npm test", CheckKind::NodeTest),
    ] {
        assert_eq!(check_of(command).map(|(c, _)| c), Some(check), "{command}");
    }
    assert_eq!(check_of("git status"), None);
    assert_eq!(check_of("cargo test; cargo clippy"), Some((CheckKind::CargoTest, false)));
}

