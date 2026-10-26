use serde_json::{Value, json};

use super::*;
use crate::git::Move;
use crate::hook::commits_since;
use crate::plain::Sha;
use crate::time::iso;

const SESSION: &str = "11111111-aaaa";

fn tool(ms: i64, id: &str, name: &str, input: Value) -> String {
    json!({ "type": "assistant", "timestamp": iso(ms), "message": { "content": [{ "type": "tool_use", "id": id, "name": name, "input": input }] } }).to_string()
}

fn shell(ms: i64, id: &str, command: &str) -> String {
    tool(ms, id, "Bash", json!({ "command": command }))
}

fn ended(id: &str, failed: bool) -> String {
    json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": id, "content": "x", "is_error": failed }] }, "toolUseResult": { "stdout": "" } }).to_string()
}

// A shell call and its result.
fn ran(ms: i64, id: &str, command: &str, failed: bool) -> String {
    format!("{}\n{}", shell(ms, id, command), ended(id, failed))
}

fn plan_command(plan: &str) -> String {
    format!("tolkster plan {SESSION} --file - <<'EOF'\n{plan}\nEOF")
}

// A plan sent at `ms` that returned unfailed.
fn planned(ms: i64, id: &str, plan: &str) -> String {
    ran(ms, id, &plan_command(plan), false)
}

// The session's first post, at 1s.
fn opened() -> String {
    ran(1_000, "t", &format!("tolkster activity {SESSION} thought --body-file - <<'EOF'\nThe task.\nEOF"), false)
}

fn call(tool: &str, input: Value) -> Call {
    crate::hook::parse_call(&json!({ "hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": input, "tool_use_id": "now", "cwd": "D:/wt/tw-1" }).to_string()).unwrap()
}

fn bash(command: &str) -> Call {
    call("Bash", json!({ "command": command }))
}

fn join(lines: &[String]) -> String {
    lines.join("\n")
}

// The edge's lines, once the call is one.
fn at_edge(tail: &str, call: &Call) -> Vec<Line> {
    edge_at(tail, call).lines
}

fn edge_at(tail: &str, call: &Call) -> Edge {
    match edge(tail, call) {
        Some(session) => lines(tail, call, &session, None),
        None => Edge::default(),
    }
}

fn step(content: &str, span: Option<(i64, i64)>, outcomes: Vec<(&'static str, bool)>) -> Line {
    Line { done: Done::Step(content.into()), since_ms: span.map(|s| s.0), until_ms: span.map(|s| s.1), outcomes }
}

fn phase(phase: &'static str, since_ms: i64, until_ms: Option<i64>, outcomes: Vec<(&'static str, bool)>) -> Line {
    Line { done: Done::Phase(phase), since_ms: Some(since_ms), until_ms, outcomes }
}

const FIRST: &str = r#"[{"content": "Read the issue", "status": "completed"}, {"content": "Add the **flag**", "status": "inProgress"}, {"content": "PR open", "status": "pending"}]"#;
const ADDED: &str = r#"[{"content": "Read the issue", "status": "completed"}, {"content": "Add the **flag**", "status": "completed"}, {"content": "PR open", "status": "inProgress"}]"#;
const ALL: &str = r#"[{"content": "Read the issue", "status": "completed"}, {"content": "Add the **flag**", "status": "completed"}, {"content": "PR open", "status": "completed"}]"#;

#[test]
fn the_call_after_a_plan_that_completes_a_step_puts_in_every_step_completed() {
    let worked = [opened(), planned(2_000, "p", FIRST), ran(3_000, "a", "cargo test", true), ran(4_000, "b", "cargo test", false), ran(5_000, "c", "cargo build", true)];
    let next = bash("ls");
    let added = join(&[join(&worked), planned(6_000, "q", ADDED)]);
    let flag = step("Add the flag", Some((2_000, 6_000)), vec![("testing", false), ("building", true)]);
    assert_eq!(at_edge(&added, &next), [step("Read the issue", None, Vec::new()), flag]);
    // Each edge puts in every step so far, so one a failed post missed goes in at the next.
    let all = join(&[added.clone(), ran(7_000, "d", "git push", false), planned(8_000, "r", ALL)]);
    assert_eq!(at_edge(&all, &next).iter().map(Line::key).collect::<Vec<_>>(), ["Read the issue", "Add the flag", "PR open"]);
    assert_eq!(at_edge(&all, &next)[2], step("PR open", Some((6_000, 8_000)), vec![("pushing", false)]));
    // A plan sent again with nothing newly completed, or the call after the next, is no edge.
    assert_eq!(at_edge(&join(&[added.clone(), planned(7_000, "r", ADDED)]), &next), []);
    assert_eq!(at_edge(&join(&[added.clone(), shell(7_000, "x", "ls")]), &next), []);
    // Calls `hook start` doesn't run before, such as an edit, leave the edge to the next it does.
    let edited = join(&[added.clone(), tool(7_000, "e", "Edit", json!({})), tool(7_500, "f", "Read", json!({}))]);
    assert_eq!(at_edge(&edited, &next).len(), 2);
    // The first plan's completed steps go in, even sent before the session's thought.
    assert_eq!(at_edge(&join(&[opened(), planned(2_000, "p", FIRST)]), &next), [step("Read the issue", None, Vec::new())]);
    assert_eq!(at_edge(&planned(2_000, "p", FIRST), &next), [step("Read the issue", None, Vec::new())]);
}

const UNTIL: &str = r#"[{"content": "Read", "status": "completed"}, {"content": "Add the flag", "status": "inProgress", "until": "commit"}, {"content": "Gate and review", "status": "pending", "until": "review"}, {"content": "PR open", "status": "pending", "until": "pr"}]"#;

// The statuses of the plan an edge posts, None when it posts none.
fn moved(edge: &Edge) -> Option<Vec<&'static str>> {
    edge.plan.as_ref().map(|plan| plan.iter().map(|s| s.status).collect())
}

#[test]
fn each_until_event_moves_the_plan_on_and_the_next_edge_posts_it() {
    let next = bash("ls");
    let sent = join(&[opened(), planned(2_000, "p", UNTIL), ran(3_000, "a", "cargo test", false)]);
    // The worker's plan as sent is Linear's already.
    assert_eq!(moved(&edge_at(&sent, &next)), None);
    let committed = join(&[sent.clone(), ran(4_000, "c", "git commit -m x && git push", false)]);
    let edge = edge_at(&committed, &next);
    assert_eq!(moved(&edge), Some(vec!["completed", "completed", "inProgress", "pending"]));
    assert_eq!(edge.lines.iter().map(Line::key).collect::<Vec<_>>(), ["Read", "Add the flag"]);
    assert_eq!(edge.lines[1].since_ms, Some(2_000));
    // Posted once: the call after the next is no edge.
    let quiet = join(&[committed.clone(), ran(5_000, "x", "ls", false)]);
    assert_eq!(edge_at(&quiet, &next), Edge::default());
    // A fix commit during the review step moves nothing.
    let fixed = join(&[quiet.clone(), ran(6_000, "f", "git commit -m fix", false)]);
    assert_eq!(edge_at(&fixed, &next), Edge::default());
    // `pr create` posts the review's verdict, then opens the PR: both steps end.
    let opened_pr = join(&[fixed.clone(), ran(7_000, "r", &format!("tolkster pr create --head b --base main --title t --body-file - --session {SESSION} --review light"), false)]);
    let edge = edge_at(&opened_pr, &next);
    assert_eq!(moved(&edge), Some(vec!["completed"; 4]));
    assert_eq!(edge.lines.iter().map(Line::key).collect::<Vec<_>>(), ["Read", "Add the flag", "Gate and review", "PR open"]);
    // The review step ran from the commit's next call to the PR's opening, the last call.
    assert_eq!((edge.lines[2].since_ms, edge.lines[2].until_ms), (Some(5_000), None));
    // A commit that failed moves nothing.
    let failed = join(&[sent.clone(), ran(4_000, "c", "git commit -m x", true)]);
    assert_eq!(moved(&edge_at(&failed, &next)), None);
}

#[test]
fn the_response_finishes_the_plan_once_its_pr_is_open_and_nothing_moves_it_after() {
    let committed = join(&[opened(), planned(2_000, "p", UNTIL), ran(4_000, "c", "git commit -m x", false)]);
    let response = bash(&format!("tolkster activity {SESSION} response --body-file -"));
    // Before the PR, as an answer mid-task or a stop short of it, the response moves nothing,
    // but puts in what an earlier edge may have lost.
    let early = edge_at(&committed, &response);
    assert_eq!(moved(&early), Some(vec!["completed", "completed", "inProgress", "pending"]));
    assert_eq!(early.lines.iter().map(Line::key).collect::<Vec<_>>(), ["Read", "Add the flag"]);
    let opened_pr = join(&[committed.clone(), ran(7_000, "r", &format!("tolkster pr create --title t --session {SESSION}"), false)]);
    assert_eq!(moved(&edge_at(&opened_pr, &response)), Some(vec!["completed"; 4]));
    // A plan with no PR step is finished: the step in progress completed, the rest canceled.
    let plain = r#"[{"content": "Answer", "status": "inProgress"}, {"content": "Later", "status": "pending"}]"#;
    let finished = edge_at(&join(&[opened(), planned(2_000, "p", plain)]), &response);
    assert_eq!(moved(&finished), Some(vec!["completed", "canceled"]));
    assert_eq!(finished.lines.iter().map(Line::key).collect::<Vec<_>>(), ["Answer"]);
    // A question doesn't finish it.
    let question = bash(&format!("tolkster activity {SESSION} elicitation --body-file -"));
    assert_eq!(moved(&edge_at(&join(&[opened(), planned(2_000, "p", plain), ran(3_000, "x", "ls", false)]), &question)), None);
    // After the final response, the call that closes the worker is no edge.
    let answered = join(&[opened_pr.clone(), ran(8_000, "e", &format!("tolkster activity {SESSION} response --body-file -"), false)]);
    assert_eq!(edge(&answered, &bash("ls")), None);
    // A commit after an answer mid-task is an edge as before.
    let mid = join(&[committed.clone(), ran(5_000, "e", &format!("tolkster activity {SESSION} response --body-file -"), false), ran(6_000, "f", "git commit -m more", false)]);
    assert_eq!(edge(&mid, &bash("ls")).as_deref(), Some(SESSION));
}

#[test]
fn a_commit_counts_by_the_reflog_and_the_pr_by_its_session() {
    let next = bash("ls");
    let sent = join(&[opened(), planned(2_000, "p", UNTIL), ran(3_000, "a", "cargo test", false)]);
    let commit = |at: i64| Move { sha: Sha::parse(&"a".repeat(40)).unwrap(), at, why: "commit: x".into() };
    let with = |tail: &str, moves: &[Move]| match edge(tail, &next) {
        Some(session) => moved(&lines(tail, &next, &session, Some(moves))),
        None => None,
    };
    // A push that failed after the commit still counts it; `|| true` with no commit doesn't.
    let pushed = join(&[sent.clone(), ran(4_000, "c", "git commit -m x && git push", true)]);
    assert_eq!(with(&pushed, &[commit(4)]), Some(vec!["completed", "completed", "inProgress", "pending"]));
    let nothing = join(&[sent.clone(), ran(4_000, "c", "git commit -m x || true", false)]);
    assert_eq!(with(&nothing, &[commit(1)]), None);
    // A commit goes to the newest commit call before it: after one a hook failed, the next
    // one's commit is that call's own, and so news at its edge.
    let retried = join(&[sent.clone(), ran(4_000, "c", "git commit -m x", true), ran(5_000, "d", "git add -A && git commit -m x", false)]);
    assert_eq!(with(&retried, &[commit(6)]), Some(vec!["completed", "completed", "inProgress", "pending"]));
    // Two commit calls in one message whose commits land in one second count one each.
    let two = r#"[{"content": "A", "status": "inProgress", "until": "commit"}, {"content": "B", "status": "pending", "until": "commit"}, {"content": "PR open", "status": "pending", "until": "pr"}]"#;
    let together = join(&[opened(), planned(2_000, "p", two), ran(4_000, "c", "git commit -m a", false), ran(4_005, "d", "git commit -m b", false)]);
    assert_eq!(with(&together, &[commit(5), commit(5)]), Some(vec!["completed", "completed", "inProgress"]));
    // Another session's PR moves nothing; this one's ends every open step up to its own.
    let other = join(&[sent.clone(), ran(4_000, "r", "tolkster pr create --title t --session 22222222-bbbb", false)]);
    assert_eq!(moved(&edge_at(&other, &next)), None);
    let ours = join(&[sent.clone(), ran(4_000, "r", &format!("tolkster pr create --title t --session {SESSION}"), false)]);
    assert_eq!(moved(&edge_at(&ours, &next)), Some(vec!["completed"; 4]));
}

#[test]
fn a_plan_sent_again_keeps_what_the_hook_completed() {
    let next = bash("ls");
    let committed = join(&[opened(), planned(2_000, "p", UNTIL), ran(4_000, "c", "git commit -m x", false), ran(5_000, "x", "ls", false)]);
    // Sent again with a new step, and the step the commit ended still in progress.
    let more = r#"[{"content": "Read", "status": "completed"}, {"content": "Add the flag", "status": "inProgress", "until": "commit"}, {"content": "Fix the review", "status": "pending", "until": "commit"}, {"content": "Gate and review", "status": "pending", "until": "review"}, {"content": "PR open", "status": "pending", "until": "pr"}]"#;
    let resent = join(&[committed.clone(), planned(6_000, "q", more)]);
    assert_eq!(moved(&edge_at(&resent, &next)), Some(vec!["completed", "completed", "inProgress", "pending", "pending"]));
}

#[test]
fn a_plan_that_didnt_return_unfailed_or_isnt_the_sessions_open_one_puts_in_nothing() {
    let worked = join(&[opened(), planned(2_000, "p", FIRST), ran(3_000, "x", "ls", false)]);
    let next = bash("ls");
    // Denied or failed, still running, or sent from a file: what auto mode refused never shows.
    for sent in [ran(6_000, "q", &plan_command(ADDED), true), shell(6_000, "q", &plan_command(ADDED)), ran(6_000, "q", &format!("tolkster plan {SESSION} --file p.json"), false)] {
        assert_eq!(at_edge(&join(&[worked.clone(), sent.clone()]), &next), [], "{sent}");
    }
    // Another session's plan, or one sent after a response or a question.
    let other = ran(6_000, "q", &format!("tolkster plan 22222222-bbbb --file - <<'EOF'\n{ADDED}\nEOF"), false);
    assert_eq!(lines(&join(&[worked.clone(), other]), &next, SESSION, None).lines, []);
    // Another session's response doesn't close this one.
    let ended_other = ran(500, "o", "tolkster activity 22222222-bbbb response --body-file -", false);
    assert_eq!(at_edge(&join(&[ended_other, planned(2_000, "p", FIRST)]), &next), [step("Read the issue", None, Vec::new())]);
    // A question whose session isn't plain still closes it.
    let unplain = ran(5_000, "e", "tolkster activity $S elicitation --body-file -", false);
    assert_eq!(edge(&join(&[worked.clone(), unplain, planned(6_000, "q", ADDED)]), &next), None);
    let asked = ran(5_000, "e", &format!("tolkster activity {SESSION} elicitation --body-file -"), false);
    assert_eq!(edge(&join(&[worked.clone(), asked, planned(6_000, "q", ADDED)]), &next), None);
    // A response, an answer mid-task, doesn't: the plan sent after it goes in.
    let answered = ran(5_000, "e", &format!("tolkster activity {SESSION} response --body-file -"), false);
    assert_eq!(edge(&join(&[worked.clone(), answered, planned(6_000, "q", ADDED)]), &next).as_deref(), Some(SESSION));
}

#[test]
fn without_a_plan_each_phase_passed_goes_in_at_the_next_shell_call_or_sub_agent() {
    let test = bash("cargo test");
    let read = join(&[opened(), tool(2_000, "r", "Read", json!({ "file_path": "a.rs" }))]);
    // A check or a push before anything changed is still understanding.
    assert_eq!(at_edge(&read, &test), []);
    let edited = join(&[read.clone(), tool(3_000, "e", "Edit", json!({ "file_path": "a.rs" }))]);
    assert_eq!(at_edge(&edited, &bash("ls")), [phase(UNDERSTANDING, 1_000, Some(3_000), Vec::new())]);
    assert_eq!(at_edge(&edited, &test), [phase(UNDERSTANDING, 1_000, Some(3_000), Vec::new()), phase(CHANGING, 3_000, None, Vec::new())]);
    // Forward only: a fix after a red test opens no line, and the checks' line holds both runs.
    let checked = join(&[edited.clone(), ran(4_000, "a", "cargo test", true), tool(5_000, "f", "Edit", json!({})), ran(6_000, "b", "cargo test", false)]);
    assert_eq!(at_edge(&checked, &test), []);
    let pushed = at_edge(&checked, &bash("git push"));
    assert_eq!(pushed.last(), Some(&phase(CHECKING, 4_000, None, vec![("testing", false)])));
    // A response after the change closes the phase it is in; one before it closes nothing.
    let shipped = join(&[checked.clone(), ran(7_000, "c", "git commit -m x && git push", false)]);
    let response = bash(&format!("tolkster activity {SESSION} response --body-file -"));
    assert_eq!(at_edge(&shipped, &response).last(), Some(&phase(SHIPPING, 7_000, None, Vec::new())));
    assert_eq!(at_edge(&read, &response), []);
    // A plan of the session's, before or in the call, leaves it to the steps.
    assert_eq!(at_edge(&join(&[edited.clone(), planned(3_500, "p", FIRST), ran(3_550, "x", "ls", false), tool(3_600, "g", "Edit", json!({}))]), &test), []);
    assert_eq!(lines(&edited, &bash(&plan_command(FIRST)), SESSION, None).lines, []);
    // Calls before the session's first post, as a review round's are, don't count.
    let round = join(&[tool(500, "e", "Edit", json!({})), ran(600, "a", "cargo test", false), opened(), tool(2_000, "f", "Edit", json!({}))]);
    assert_eq!(at_edge(&round, &test), [phase(UNDERSTANDING, 1_000, Some(2_000), Vec::new()), phase(CHANGING, 2_000, None, Vec::new())]);
}

#[test]
fn a_cut_tail_is_read_back_to_the_sessions_first_post() {
    // Only one of another session's before it says the tail holds the session's start.
    assert!(holds_start(&join(&[ran(500, "o", "tolkster activity 22222222-bbbb thought", false), opened()]), SESSION));
    assert!(!holds_start(&opened(), SESSION));
    assert!(!holds_start(&ran(500, "o", "tolkster activity 22222222-bbbb thought", false), SESSION));
    // A cut tail that starts after a check still says a change since may be an edge.
    let cut = join(&[ran(1_000, "v", &format!("tolkster activity {SESSION} action --action x"), false), ran(2_000, "a", "cargo test", false)]);
    assert_eq!(edge(&join(&[cut.clone(), ran(3_000, "p", "git push", false)]), &bash(&format!("tolkster activity {SESSION} response --body-file -"))).as_deref(), Some(SESSION));
}

#[test]
fn a_line_says_its_commits_and_checks_in_a_few_words_and_lists_them() {
    let line = step("Add the flag", Some((2_000, 6_000)), vec![("testing", false), ("building", true)]);
    let commits = ["Add the **flag** (TW-1)".to_string(), "Test it (TW-1)".to_string()];
    let content = |input: ActivityInput| match input.content {
        ActivityContent::Action { action, parameter, result } => (action, parameter, result, input.ephemeral),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        content(activity(SESSION, &line, &commits).unwrap()),
        ("Add the flag".into(), "2 commits · tests passed · build failed".into(), Some("- Add the flag (TW-1)\n- Test it (TW-1)\n- tests passed\n- build failed".into()), false)
    );
    let bare = phase(UNDERSTANDING, 1_000, None, Vec::new());
    assert_eq!(content(activity(SESSION, &bare, &commits[1..]).unwrap()), ("Understanding the task".into(), "1 commit".into(), Some("- Test it (TW-1)".into()), false));
    assert_eq!(content(activity(SESSION, &bare, &[]).unwrap()).1, "done");
}

#[test]
fn a_lines_commits_are_those_since_it_began_oldest_first() {
    let moved = |c: char, at: i64, why: &str| Move { sha: Sha::parse(&c.to_string().repeat(40)).unwrap(), at, why: why.into() };
    let moves = [
        moved('e', 1_700_000_009, "commit (amend): Second, reworded"),
        moved('d', 1_700_000_008, "rebase (pick): First"),
        moved('c', 1_700_000_006, "commit: Fix"),
        moved('b', 1_700_000_005, "commit: Fix"),
        moved('a', 1_700_000_001, "commit: First"),
        moved('0', 1_699_999_000, "commit: Before"),
    ];
    // Two commits alike are two; an amend replaces the one before it.
    assert_eq!(commits_since(&moves, 1_700_000_000_500, None), ["First", "Fix", "Second, reworded"]);
    assert_eq!(commits_since(&moves, 1_700_000_010_000, None), Vec::<String>::new());
    // Nor those after its end.
    assert_eq!(commits_since(&moves, 1_700_000_000_500, Some(1_700_000_005_900)), ["First", "Fix"]);
}
