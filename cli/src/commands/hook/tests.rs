use super::*;
use crate::time::iso;

const SESSION: &str = "11111111-aaaa";
const ROOT: &str = "D:/wt/tw-1";

fn made(id: &str, tool: &str, input: serde_json::Value) -> Call {
    parse_call(&serde_json::json!({ "hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": input, "tool_use_id": id, "cwd": ROOT }).to_string()).unwrap()
}

// A transcript of the calls (id, tool, input), each with its result in but the last's.
fn transcript(calls: &[(&str, &str, serde_json::Value)]) -> String {
    let mut lines = Vec::new();
    for (i, (id, tool, input)) in calls.iter().enumerate() {
        let at = 1_000 * (i as i64 + 1);
        lines.push(serde_json::json!({ "type": "assistant", "timestamp": iso(at), "message": { "content": [{ "type": "tool_use", "id": id, "name": tool, "input": input }] } }).to_string());
        if i + 1 < calls.len() {
            lines.push(serde_json::json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": id, "content": "x", "is_error": false }] }, "toolUseResult": { "stdout": "" } }).to_string());
        }
    }
    lines.join("\n")
}

// Whether the last call of `calls` repeats the one before it, with the hook's line for it.
fn repeats_after(calls: &[(&str, &str, serde_json::Value)]) -> bool {
    let tail = transcript(calls);
    let (id, tool, input) = calls.last().unwrap();
    let this = made(id, tool, input.clone());
    let at = whereabouts(&tail, true, &this, SESSION);
    let (doing, head) = (doing(&this).unwrap(), voice::head(&at).unwrap());
    let phrase = phrase(&doing, ROOT).unwrap();
    repeats(&this, (&phrase, &head), (&tail, true), SESSION, ROOT)
}

fn bash(command: &str) -> serde_json::Value {
    serde_json::json!({ "command": command })
}

#[test]
fn a_call_that_would_post_what_the_call_before_posted_posts_nothing() {
    let report = ("a", "Bash", bash(&format!("tolkster activity {SESSION} thought --body-file -")));
    let log = |id: &'static str| (id, "Bash", bash("git log --oneline -5"));
    // The first of a run posts, and so does the first call after the worker's own post.
    assert!(!repeats_after(&[report.clone(), log("g1")]));
    assert!(repeats_after(&[report.clone(), log("g1"), log("g2")]));
    assert!(repeats_after(&[report.clone(), log("g1"), log("g2"), log("g3")]));
    // A different kind, or a different file, says something new.
    let read = |id: &'static str, file: &str| (id, "Read", serde_json::json!({ "file_path": format!("{ROOT}/{file}") }));
    assert!(!repeats_after(&[report.clone(), read("r1", "a.rs"), ("e1", "Edit", serde_json::json!({ "file_path": format!("{ROOT}/a.rs") }))]));
    assert!(!repeats_after(&[report.clone(), read("r1", "a.rs"), read("r2", "b.rs")]));
    assert!(repeats_after(&[report.clone(), read("r1", "a.rs"), read("r2", "a.rs")]));
    // A call whose post differs in its head, or that the hook never posts for, is no repeat.
    let log2 = log("g2");
    let tail = transcript(&[report.clone(), log("g1"), log2.clone()]);
    let this = made("g2", "Bash", log2.2);
    assert!(!repeats(&this, ("Looking at git history", "elsewhere"), (&tail, true), SESSION, ROOT));
    assert!(!repeats_after(&[report.clone(), ("t", "TodoWrite", serde_json::json!({})), log("g1")]));
    // A paired call's tick and outcome, or a commit's `Committed`, replace what came before.
    assert!(!repeats_after(&[report.clone(), ("c", "Bash", bash("git commit -m x")), ("c2", "Bash", bash("git commit -m y"))]));
    assert!(!repeats_after(&[report.clone(), ("t", "Bash", bash("cargo test")), ("t2", "Bash", bash("cargo test"))]));
}

#[test]
fn a_call_made_before_the_one_before_it_finished_posts() {
    // Calls made in one message are all written before the first runs, and its hook then
    // posted nothing: the worker had moved on.
    let report = ("a", "Bash", bash(&format!("tolkster activity {SESSION} thought --body-file -")));
    let mut tail = transcript(&[report, ("g1", "Bash", bash("git log")), ("g2", "Bash", bash("git log"))]);
    let this = made("g2", "Bash", bash("git log"));
    let result = tail.lines().position(|l| l.contains("\"tool_result\"") && l.contains("\"g1\"")).unwrap();
    tail = tail.lines().enumerate().filter(|(i, _)| *i != result).map(|(_, l)| l).collect::<Vec<_>>().join("\n");
    let at = whereabouts(&tail, true, &this, SESSION);
    let (doing, head) = (doing(&this).unwrap(), voice::head(&at).unwrap());
    assert!(!repeats(&this, (&phrase(&doing, ROOT).unwrap(), &head), (&tail, true), SESSION, ROOT));
}
