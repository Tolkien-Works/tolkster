use std::path::PathBuf;

use serde_json::{Value, json};

use super::transcript::{POLL, TAIL_BYTES, TICK_EVERY};
use super::*;
use crate::machine::Observed;
use crate::time::iso;
use crate::voice::At;

fn call(tool: &str, input: Value) -> Call {
    parse_call(&json!({ "hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": input, "tool_use_id": "toolu_1", "cwd": "D:\\wt\\tw-1" }).to_string()).unwrap()
}

fn kind(tool: &str, input: Value) -> Option<&'static str> {
    doing(&call(tool, input)).map(|d| d.kind)
}

fn shell(command: &str) -> Option<&'static str> {
    kind("Bash", json!({ "command": command }))
}

#[test]
fn a_tool_calls_input_is_read_for_what_the_hook_needs() {
    let input = json!({
        "session_id": "abc", "transcript_path": "C:\\t\\abc.jsonl", "cwd": "D:\\wt\\tw-1", "hook_event_name": "PreToolUse",
        "tool_name": "Edit", "tool_input": { "file_path": "D:\\wt\\tw-1\\cli\\src\\claim.rs", "old_string": "a", "new_string": "b" }, "tool_use_id": "toolu_9"
    });
    let call = parse_call(&input.to_string()).unwrap();
    assert_eq!((call.tool, call.id.as_str(), call.cwd.as_str()), (Tool::Edit, "toolu_9", "D:\\wt\\tw-1"));
    assert_eq!(call.transcript, Some(PathBuf::from("C:\\t\\abc.jsonl")));
    assert!(!call.sub_agent);
    // Only a shell call's command is read, so another tool's can't pass for a `tolkster` run.
    assert_eq!(self::call("mcp__x__run", json!({ "command": "tolkster activity s1 response" })).command, None);
    assert_eq!(self::call("Bash", json!({ "command": "ls" })).command.as_deref(), Some("ls"));
    let other = tool_entry(1_000, "t1", "mcp__x__run", json!({ "command": "tolkster activity s1 response" }));
    assert_eq!(crate::harness::events::tool_calls(&other)[0].command, None);
    // A sub-agent's call, which the worker's transcript may not hold, posts nothing.
    let mut sub = input.clone();
    sub["agent_id"] = json!("a1");
    let sub = parse_call(&sub.to_string()).unwrap();
    assert!(sub.sub_agent && doing(&sub).is_none());
    // Not a tool call's input, one without a field every tool call's has, or not JSON: the
    // hook does nothing rather than fail the agent's call.
    assert_eq!(parse_call(&json!({ "hook_event_name": "Stop", "cwd": "D:/wt" }).to_string()), None);
    for field in ["hook_event_name", "cwd", "tool_use_id"] {
        let mut lacking = input.clone();
        lacking.as_object_mut().unwrap().remove(field);
        assert_eq!(parse_call(&lacking.to_string()), None, "{field}");
    }
    assert_eq!(parse_call("not json"), None);
}

// A tail as Claude Code writes it: a prompt's text, the worker's thinking, text and call, the
// call's result, a background call's notice in both its entries, and lines of other types.
#[test]
fn a_recorded_transcript_tail_reads_its_calls_text_and_ends() {
    let tail = [
        json!({ "type": "file-history-snapshot", "messageId": "m0", "snapshot": {} }),
        json!({ "type": "user", "uuid": "u0", "timestamp": "2026-10-23T01:00:00.000Z", "isSidechain": false, "message": { "role": "user", "content": "Read the brief" } }),
        json!({ "type": "assistant", "uuid": "u1", "timestamp": "2026-10-23T01:00:01.000Z", "isSidechain": false, "message": { "id": "msg_1", "role": "assistant", "model": "claude", "content": [{ "type": "thinking", "thinking": "", "signature": "s" }, { "type": "text", "text": "Run the tests." }, { "type": "tool_use", "id": "t1", "name": "Bash", "input": { "command": "cargo test", "run_in_background": true } }] } }),
        json!({ "type": "user", "uuid": "u2", "timestamp": "2026-10-23T01:00:02.000Z", "message": { "role": "user", "content": [{ "tool_use_id": "t1", "type": "tool_result", "content": "Command running in background", "is_error": false }] }, "toolUseResult": { "stdout": "", "stderr": "", "interrupted": false, "isImage": false, "backgroundTaskId": "b1" } }),
        json!({ "type": "assistant", "uuid": "u3", "timestamp": "2026-10-23T01:00:03.000Z", "message": { "role": "assistant", "content": [{ "type": "tool_use", "id": "t2", "name": "Read", "input": { "file_path": "D:/wt/a.rs" } }] } }),
        json!({ "type": "user", "uuid": "u4", "timestamp": "2026-10-23T01:00:04.000Z", "message": { "role": "user", "content": [{ "tool_use_id": "t2", "type": "tool_result", "content": "<tool_use_error>File does not exist.</tool_use_error>", "is_error": true }] }, "toolUseResult": "Error: File does not exist." }),
        json!({ "type": "queue-operation", "operation": "enqueue", "timestamp": "2026-10-23T01:00:05.000Z", "sessionId": "s1", "content": "<task-notification>\n<task-id>b1</task-id>\n<tool-use-id>t1</tool-use-id>\n<status>completed</status>\n</task-notification>" }),
        json!({ "type": "attachment", "attachment": { "type": "queued_command", "prompt": "<task-notification>\n<tool-use-id>t3</tool-use-id>\n<status>failed</status>\n</task-notification>" } }),
        json!({ "type": "summary", "summary": "Tests", "leafUuid": "u4" }),
    ]
    .map(|line| line.to_string())
    .join("\n");
    let calls = crate::harness::events::tool_calls(&tail);
    let read: Vec<_> = calls.iter().map(|c| (c.id.as_str(), c.tool, c.command.as_deref(), c.file.as_deref())).collect();
    assert_eq!(read, [("t1", Tool::Shell, Some("cargo test"), None), ("t2", Tool::Read, None, Some("D:/wt/a.rs"))]);
    assert_eq!(calls[0].at, crate::time::instant("2026-10-23T01:00:01.000Z").unwrap());
    assert_eq!(crate::harness::events::narration(&tail, "t1"), ["Run the tests."]);
    let ends = crate::harness::events::ends(&tail);
    assert!(matches!(ends.get("t1"), Some(crate::harness::events::End::Notice(Some(false)))));
    assert!(matches!(ends.get("t2"), Some(crate::harness::events::End::Returned(true))));
    assert!(matches!(ends.get("t3"), Some(crate::harness::events::End::Notice(Some(true)))));
}

#[test]
fn a_state_hooks_input_says_the_agents_state_only_when_it_is_sure() {
    let state = |input: Value| parse_state(&input.to_string(), 0, |_| None);
    let base = |event: &str| json!({ "session_id": "abc", "transcript_path": "C:/t/abc.jsonl", "cwd": "D:/wt/tw-1", "hook_event_name": event });
    let with = |event: &str, field: &str, value: &str| {
        let mut input = base(event);
        input[field] = json!(value);
        input
    };
    let cwd = || "D:/wt/tw-1".to_string();
    assert_eq!(state(base("UserPromptSubmit")), Some((Observed::AgentWorking, cwd())));
    assert_eq!(state(base("Stop")), Some((Observed::AgentIdle, cwd())));
    for waiting in ["permission_prompt", "elicitation_dialog"] {
        assert_eq!(state(with("Notification", "notification_type", waiting)), Some((Observed::AgentWaiting, cwd())));
    }
    for exit in ["prompt_input_exit", "logout", "other"] {
        assert_eq!(state(with("SessionEnd", "reason", exit)), Some((Observed::AgentExited, cwd())));
    }
    // The agent goes on after `/clear` or `/resume`, an idle reminder isn't a question, and an
    // input that doesn't say which is left to the listing.
    for unsure in [with("SessionEnd", "reason", "clear"), with("SessionEnd", "reason", "resume"), base("SessionEnd"), with("Notification", "notification_type", "idle_prompt"), base("Notification"), base("SubagentStop"), base("PreToolUse")] {
        assert_eq!(state(unsure.clone()), None, "{unsure}");
    }
    let mut no_cwd = base("Stop");
    no_cwd.as_object_mut().unwrap().remove("cwd");
    assert_eq!(state(no_cwd), None);
    assert_eq!(parse_state("not json", 0, |_| None), None);
}

#[test]
fn a_tool_maps_to_a_fixed_kind_and_names_only_its_file() {
    assert_eq!(doing(&call("Edit", json!({ "file_path": "D:\\wt\\tw-1\\a.rs" }))), Some(Doing { kind: "editing", file: Some("D:\\wt\\tw-1\\a.rs".into()) }));
    assert_eq!(doing(&call("NotebookEdit", json!({ "notebook_path": "D:/wt/tw-1/n.ipynb" }))).unwrap().file.as_deref(), Some("D:/wt/tw-1/n.ipynb"));
    assert_eq!(doing(&call("Read", json!({ "file_path": "D:/wt/tw-1/docs/cli.md" }))).unwrap().kind, "reading");
    assert_eq!(kind("Grep", json!({ "pattern": "fn claim" })), Some("searching"));
    assert_eq!(kind("Agent", json!({ "prompt": "Review the diff" })), Some("delegating"));
    assert_eq!(kind("WebFetch", json!({ "url": "https://example.com" })), Some("looking-up"));
    // A tool with no phrase posts nothing.
    for tool in ["TodoWrite", "ToolSearch", "Skill", "mcp__linear__save_issue"] {
        assert_eq!(kind(tool, json!({})), None, "{tool}");
    }
}

#[test]
fn a_command_maps_to_a_kind_by_its_programs_and_is_never_echoed() {
    assert_eq!(shell("cd /d/wt/tw-1 && cargo test --locked --manifest-path cli/Cargo.toml 2>&1 | grep -E 'FAILED'"), Some("testing"));
    assert_eq!(shell("cargo clippy --locked --manifest-path cli/Cargo.toml"), Some("building"));
    assert_eq!(shell("npm test"), Some("testing"));
    assert_eq!(shell("npm run typecheck"), Some("building"));
    assert_eq!(shell("npx wrangler deploy --dry-run --outdir dist"), Some("building"));
    assert_eq!(shell("node --test test/hook.test.ts"), Some("testing"));
    assert_eq!(shell("git add -A && git commit -q -m \"Wire it in\""), Some("committing"));
    assert_eq!(shell("git commit -q -m \"Wire it in\""), Some("committing"));
    assert_eq!(shell("git push -u origin tw-1"), Some("pushing"));
    assert_eq!(shell("git fetch origin && git rebase origin/main"), Some("rebasing"));
    assert_eq!(shell("git log --oneline -5"), Some("history"));
    assert_eq!(shell("grep -n fn cli/src/hook.rs"), Some("searching"));
    assert_eq!(shell("sed -n 1,40p cli/src/hook.rs"), Some("reading"));
    assert_eq!(kind("PowerShell", json!({ "command": "Get-Content cli/src/hook.rs -TotalCount 40" })), Some("reading"));
    assert_eq!(shell("python script.py"), Some("working"));
    assert_eq!(shell("python script.py | grep x"), Some("working"));
    assert_eq!(shell("make all"), Some("command"));
    assert_eq!(shell("cat a.txt | grep x"), Some("reading"));
    assert_eq!(shell("C:\\tools\\Cargo.EXE test"), Some("testing"));
    // The worker's own reporting, and Orca's plumbing, post nothing, wherever they come in
    // the command; so does a command that only waits or only sets up.
    assert_eq!(shell("tolkster activity s1 thought --state reading-code"), None);
    assert_eq!(shell("cd /d/wt/tw-1 && git status; orca terminal close --terminal t1 --json"), None);
    assert_eq!(shell("cat body.md | tolkster activity s1 response --body-file -"), None);
    assert_eq!(shell("sleep 5"), None);
    assert_eq!(shell("cd /d/wt/tw-1"), None);
    assert_eq!(kind("Bash", json!({})), None);
}

#[test]
fn a_command_that_writes_a_file_is_an_edit_by_its_text_alone() {
    for command in [
        "sed -i 's/a/b/' cli/src/hook.rs",
        "sed -i.bak -e 's/a/b/' a.rs",
        "sed -Ei 's/a/b/' a.rs",
        "perl -pi -e 's/a/b/' a.rs",
        "perl -0pi -e 's/a/b/' a.rs",
        "perl -i.bak -e 's/a/b/' a.rs",
        "perl -wpi.orig -e 's/a/b/' a.rs",
        "sed -i.bak2 's/a/b/' a.rs",
        "sed --in-place=.bak 's/a/b/' a.rs",
        "bash -c \"echo x > a.rs\"",
        "powershell.exe -Command \"echo x > a.rs\"",
        "pwsh -NoProfile -Command \"echo x > a.rs\"",
        "jq -r '@csv' a.json > out.csv",
        "@\"\nx\n\"@ > a.md",
        "@'\nx\n'@ > a.md",
        "python -c \"json.dump(x, open('a.json', 'w'))\"",
        "python -c \"import pathlib; pathlib.Path('a').open('w')\"",
        "echo hi > notes.md",
        "cat >> docs/cli.md <<'EOF'\nmore\nEOF",
        "cat <<'EOF' > cli/a.rs\nfn a() {}\nEOF",
        "printf x | tee cli/a.rs",
        "cd /d/wt/tw-1 && cargo fmt --check 2> out.txt; ls",
        "Set-Content -Path a.rs -Value x",
        "Get-Content a.rs | Out-File b.rs",
        "pwsh -Command \"Add-Content a.rs x\"",
        "python - <<'EOF'\nimport pathlib\npathlib.Path('a.rs').write_text('x')\nEOF",
        "python -c \"open('a.rs', 'w').write('x')\"",
        "python -c \"open('a.rs', mode='w').write('x')\"",
        "node -e \"require('fs').writeFileSync('a.rs', 'x')\"",
    ] {
        assert_eq!(shell(command), Some("editing"), "{command}");
    }
    // A test or a build that redirects its output is still that; scratch space, the null
    // device, a quoted `>` and a cmdlet writing to the temp directory are no edit; and a
    // script that writes nothing to a file, or whose text is on disk, is only work.
    assert_eq!(shell("cargo test > out.txt 2>&1"), Some("testing"));
    assert_eq!(shell("git commit -m 'a > b'"), Some("committing"));
    for command in [
        "ls > /dev/null 2>&1",
        "ls > $null",
        "cargo fmt > /tmp/out.txt",
        "cat a | tee $env:TEMP/x.txt",
        "echo hi > C:\\Users\\u\\AppData\\Local\\Temp\\x.txt",
        "Set-Content $env:TEMP\\body.md x",
        "Get-Content a | Out-File /tmp/b",
        "grep -- '->' a.rs",
        "grep '>' a.rs",
        "python -c \"print(1 > 2)\"",
        "perl script.pl -input x",
        "python -c \"import webbrowser; webbrowser.open('www.example.com')\"",
        "jq 'select(.n > 3)' a.json",
        "sed -n 1,5p a.rs",
    ] {
        assert_ne!(shell(command), Some("editing"), "{command}");
    }
    for command in [
        "python tools/gate.py",
        "node scripts/gate.ts",
        "pwsh -File build.ps1",
        "python - <<'EOF'\nprint(1)\nEOF",
        "python -c \"print(open('a.rs').read())\"",
        "python -c \"print(open('words.txt').read())\"",
        "python -c \"import os; os.popen('wc')\"",
        "python -c \"Image.open('white.png')\"",
        "node - <<'EOF'\nprocess.stdout.write('x')\nEOF",
    ] {
        assert_eq!(shell(command), Some("working"), "{command}");
    }
    // A heredoc's body is text, not commands: a line of it that starts with `git` or `find`
    // names nothing, and is no push, commit or tolkster run either.
    assert_eq!(shell("python - <<'EOF'\nprint(1)\nfind x\ngit push\nEOF"), Some("working"));
    assert_eq!(shell("cat <<'EOF'\ngit push origin main\nEOF"), Some("reading"));
    assert_eq!(shell("cat <<'EOF' > a.md\ntolkster activity s1 thought\nEOF"), Some("editing"));
    assert_eq!(shell("@'\ncargo test\n'@ | python -"), Some("working"));
    assert!(!commits(&call("Bash", json!({ "command": "cat <<'EOF'\ngit commit -m x\nEOF" }))));
    assert!(commits(&call("Bash", json!({ "command": "git commit -F - <<'EOF'\nMessage\nEOF" }))));
    // What follows a body is code again, and a body nothing ends stays code.
    assert_eq!(shell("cat <<'EOF'\nx\nEOF\ncargo test"), Some("testing"));
    assert_eq!(shell("cat <<'EOF'\ncargo test"), Some("testing"));
    // An edit by script is the phase of changing the code.
    let write = shell_entry(1_000, "a", "python - <<'EOF'\nopen('a.rs', 'w').write('x')\nEOF");
    assert_eq!(whereabouts(&write, true, &call("Bash", json!({ "command": "ls" })), SESSION), At::Phase(CHANGING));
}

// A transcript line for an assistant entry holding the tool call `id`, made at `ms`.
fn entry(ms: i64, id: &str) -> String {
    json!({ "type": "assistant", "timestamp": iso(ms), "message": { "content": [{ "type": "tool_use", "id": id, "name": "Bash", "input": {} }] } }).to_string()
}

// A tool call's transcript line, made at `ms`.
fn tool_entry(ms: i64, id: &str, tool: &str, input: Value) -> String {
    json!({ "type": "assistant", "timestamp": iso(ms), "message": { "content": [{ "type": "tool_use", "id": id, "name": tool, "input": input }] } }).to_string()
}

fn plan_entry(id: &str, plan: &str) -> String {
    shell_entry(1_000, id, &format!("tolkster plan 11111111-aaaa --file - <<'EOF'\n{plan}\nEOF"))
}

const SESSION: &str = "11111111-aaaa";

const PLAN: &str = r#"[{"content": "Read the issue", "status": "completed"}, {"content": "Add the CLI   command\nand its tests", "status": "inProgress"}, {"content": "PR open", "status": "pending"}]"#;

#[test]
fn a_plan_in_the_tail_names_its_step_in_progress() {
    let edit = call("Edit", json!({ "file_path": "D:/wt/tw-1/cli/src/hook.rs" }));
    let at = |calls: &[String]| whereabouts(&calls.join("\n"), true, &edit, SESSION);
    let step = |content: &str| At::Step(content.into());
    let added = step("Add the CLI command and its tests");
    assert_eq!(at(&[plan_entry("p", PLAN), entry(2_000, "a")]), added);
    assert_eq!(crate::voice::head(&added).as_deref(), Some("Add the CLI command and its tests"));
    // A PowerShell here-string's plan, or a heredoc piped in, reads the same.
    assert_eq!(at(&[shell_entry(1_000, "p", &format!("@'\n{PLAN}\n'@ | tolkster plan 11111111-aaaa --file -"))]), added);
    assert_eq!(at(&[shell_entry(1_000, "p", &format!("cat <<'EOF' | tolkster plan 11111111-aaaa --file -\n{PLAN}\nEOF"))]), added);
    // A later plan wins.
    let later = r#"[{"content": "Read the issue", "status": "completed"}, {"content": "Add the CLI command and its tests", "status": "completed"}, {"content": "PR open", "status": "inProgress"}]"#;
    assert_eq!(at(&[plan_entry("p", PLAN), plan_entry("q", later)]), step("PR open"));
    // Only a plan before the call counts, when the tail holds it.
    assert_eq!(at(&[plan_entry("p", PLAN), entry(2_000, "toolu_1"), plan_entry("q", later)]), added);
    // A commit that returned moves a step that ends on one.
    let until = PLAN.replace(r#""status": "inProgress""#, r#""status": "inProgress", "until": "commit""#);
    let result = json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "c", "content": "x", "is_error": false }] } }).to_string();
    assert_eq!(at(&[plan_entry("p", &until), shell_entry(2_000, "c", "git commit -m x"), result]), step("PR open"));
    // The step is plain text on one line, and no longer than a line should be.
    let marked = r#"[{"content": "Fix **[the](http://x)** `a_b` ~~c~~", "status": "inProgress"}]"#;
    assert_eq!(at(&[plan_entry("p", marked)]), step("Fix the (http://x) a b c"));
    let long = format!(r#"[{{"content": "{}", "status": "inProgress"}}]"#, "word ".repeat(40));
    let At::Step(content) = at(&[plan_entry("p", &long)]) else { panic!("no step") };
    assert!(content.chars().count() <= 100 && content.ends_with('…'), "{content}");
}

#[test]
fn without_a_plan_of_its_session_piped_in_the_line_names_the_phase() {
    let edit = call("Edit", json!({ "file_path": "D:/wt/tw-1/a.rs" }));
    let read = call("Read", json!({ "file_path": "D:/wt/tw-1/a.rs" }));
    let phase_of = |calls: &[String], whole: bool, this: &Call| match whereabouts(&calls.join("\n"), whole, this, SESSION) {
        At::Phase(phase) => phase,
        step => panic!("{step:?}"),
    };
    let phase = |calls: &[String], this: &Call| phase_of(calls, true, this);
    let sh = |id: &str, command: &str| shell_entry(1_000, id, command);
    let edited = tool_entry(1_000, "e", "Edit", json!({ "file_path": "D:/wt/tw-1/a.rs" }));
    // No plan, one sent from a file, even with its JSON in the command, one whose JSON doesn't
    // parse, one for another session, or a plan in another heredoc's body.
    for plan in [
        String::new(),
        "tolkster plan 11111111-aaaa --file C:/t/plan.json".into(),
        format!("echo '{PLAN}' > /tmp/p.json && tolkster plan 11111111-aaaa --file /tmp/p.json"),
        "tolkster plan 11111111-aaaa --file - <<'EOF'\n[{\"content\": \"x\", \"status\": \"inProgress\"\nEOF".into(),
        format!("tolkster plan 22222222-bbbb --file - <<'EOF'\n{PLAN}\nEOF"),
        format!("cat > /tmp/notes.md <<'EOF'\ntolkster plan 11111111-aaaa --file - <<'X'\n{PLAN}\nX\nEOF"),
    ] {
        assert_eq!(phase(&[sh("p", &plan)], &read), UNDERSTANDING, "{plan}");
    }
    // A newest plan with no step in progress, even after one that had one.
    let done = r#"[{"content": "Read", "status": "completed"}]"#;
    assert_eq!(phase(&[plan_entry("p", PLAN), plan_entry("q", done)], &read), UNDERSTANDING);
    // Before the first edit, a push of the empty branch or a test run is still understanding,
    // unless the tail is cut and may have lost the edits.
    assert_eq!(phase(&[sh("a", "git push -u origin tw-1"), sh("b", "cargo test")], &read), UNDERSTANDING);
    assert_eq!(phase_of(&[sh("a", "cargo test"), sh("b", "git push")], false, &read), SHIPPING);
    // The call itself, not yet in the tail, counts.
    assert_eq!(phase(&[sh("a", "git push -u origin tw-1")], &edit), CHANGING);
    assert_eq!(phase(&[edited.clone(), sh("a", "grep -n x a.rs")], &read), CHANGING);
    assert_eq!(phase(&[sh("c", "git commit -m x")], &read), CHANGING);
    for check in ["cargo test --locked", "npm run typecheck", "node --test test/hook.test.ts", "pwsh -NoProfile -Command \"npm test\"","pwsh -c \"cargo test\""] {
        assert_eq!(phase(&[edited.clone(), sh("a", check)], &read), CHECKING, "{check}");
    }
    assert_eq!(phase(&[edited.clone(), sh("a", "cargo test"), sh("b", "git commit -m x")], &read), CHECKING);
    for ship in ["git push -u origin tw-1", "cat body.md | tolkster pr create --head tw-1 --base main --body-file -"] {
        assert_eq!(phase(&[edited.clone(), sh("a", "cargo test"), sh("b", ship)], &read), SHIPPING, "{ship}");
    }
    // A fix after a push is changing the code again.
    assert_eq!(phase(&[edited.clone(), sh("a", "git push")], &edit), CHANGING);
    for name in [UNDERSTANDING, CHANGING, CHECKING, SHIPPING] {
        assert!(crate::voice::head(&At::Phase(name)).is_some(), "{name} has no phrase in src/shared/voice.json");
    }
}

#[test]
fn a_transcript_that_cant_be_read_names_no_session() {
    // The first line of a cut tail is skipped, as are lines in a format it doesn't know, so
    // with no post of the worker's read from it the hook stays quiet.
    let cut = format!("{{\"type\": \"assist\n{}\n{{\"type\":\"assistant\",\"timestamp\":\"yesterday\"}}", entry(1_000, "a"));
    assert_eq!(last_report(&cut), None);
    assert_eq!(last_report("garbage\n{}"), None);
}

#[test]
fn a_question_or_an_error_silences_the_hook_and_a_response_does_not() {
    let after = |command: &str| may_post_after(&last_report(&shell_entry(1_000, "a", command)).unwrap());
    assert!(!after("cat q.md | tolkster activity 11111111-aaaa elicitation --body-file -"));
    assert!(!after("tolkster activity 11111111-aaaa error --body-file -"));
    // A response, early or final, leaves room: the final one is followed only by the closing
    // command, which the hook never posts for.
    assert!(after("tolkster activity 11111111-aaaa response --body-file -"));
    for kind in ["thought --state reading-code", "action --action Committed", "plan"] {
        assert!(after(&format!("tolkster activity 11111111-aaaa {kind}")), "{kind}");
    }
    assert!(after("tolkster plan 11111111-aaaa --file p.json"));
    // A question written another way still silences it, with no session to post in.
    assert!(!after("tolkster activity $s elicitation --body-file -"));
    assert!(!after("tolkster activity 11111111-aaaa --body-file r.md elicitation"));
    assert_eq!(last_report(&shell_entry(1_000, "a", "tolkster activity $s response")).map(|r| (r.session, r.kind)), Some((String::new(), Some("response".into()))));
    // A heredoc's body is never the run: the command before it is.
    let heredoc = "tolkster activity 11111111-aaaa elicitation --body-file - <<'EOF'
tolkster activity 22222222-bbbb thought
EOF";
    assert!(!after(heredoc));
    // Only the newest post counts: a thought after a question opens it again.
    let tail = [
        shell_entry(1_000, "a", "tolkster activity 11111111-aaaa elicitation --body-file -"),
        shell_entry(2_000, "b", "tolkster activity 11111111-aaaa thought --state reading-code"),
    ]
    .join("\n");
    assert!(may_post_after(&last_report(&tail).unwrap()));
}

#[test]
fn every_kind_the_hook_names_has_a_phrase() {
    let inputs = [
        ("Edit", json!({})), ("Read", json!({})), ("Grep", json!({})), ("Agent", json!({})), ("WebSearch", json!({})),
    ];
    let commands = ["cargo test", "cargo build", "git commit", "git push", "git rebase", "git log", "ls", "cat x", "python x", "sed -i s/a/b/ x", "make"];
    let kinds = inputs.into_iter().filter_map(|(tool, input)| kind(tool, input)).chain(commands.iter().filter_map(|c| shell(c)));
    for kind in kinds {
        assert!(crate::voice::tool(kind, None).is_some(), "{kind} has no phrase in src/shared/voice.json");
    }
}

// A shell call's transcript line, made at `ms`.
fn shell_entry(ms: i64, id: &str, command: &str) -> String {
    json!({ "type": "assistant", "timestamp": iso(ms), "message": { "content": [{ "type": "tool_use", "id": id, "name": "Bash", "input": { "command": command } }] } }).to_string()
}

#[test]
fn a_call_since_the_hook_started_means_the_worker_has_moved_on() {
    let tail = [entry(1_000, "a"), entry(5_000, "b")].join("\n");
    assert!(moved_on(&tail, "a", 2_000));
    // Its own call, or only calls before it started, aren't.
    assert!(!moved_on(&tail, "b", 2_000));
    assert!(!moved_on(&tail, "c", 5_000));
    // Calls made in one message share a time, and are all written before the first runs:
    // one after it in the transcript has moved on, whatever its time.
    let together = [entry(1_000, "a"), entry(1_000, "b")].join("\n");
    assert!(moved_on(&together, "a", 2_000));
    assert!(!moved_on(&together, "b", 2_000));
}

#[test]
fn the_session_is_the_one_the_worker_last_posted_in() {
    let tail = [
        shell_entry(1_000, "a", "tolkster activity 11111111-aaaa thought --state reading-code"),
        shell_entry(2_000, "b", "cat plan.json | tolkster plan 22222222-bbbb --file -"),
        shell_entry(3_000, "c", "cargo test"),
    ]
    .join("\n");
    assert_eq!(last_report(&tail), Some(Report { session: "22222222-bbbb".into(), kind: None }));
    // Only a plain ID is taken; with none, the run still counts, with no session.
    assert_eq!(last_report(&shell_entry(1_000, "a", "tolkster activity -x thought")).map(|r| r.session), Some(String::new()));
    assert_eq!(last_report(&shell_entry(1_000, "a", "tolkster messages 11111111-aaaa")), None);
    // Only a run of tolkster names it, not a mention in another program's arguments.
    assert_eq!(last_report(&shell_entry(1_000, "a", "echo tolkster activity 11111111-aaaa")), None);
    assert_eq!(last_report(&shell_entry(1_000, "a", "cd /d/wt && tolkster activity 11111111-aaaa thought")).map(|r| r.session).as_deref(), Some("11111111-aaaa"));
    assert_eq!(last_report(""), None);
}

// A worker's run of big reads pushes its newest post out of the first tail (TW-651's commit
// went unannounced so): the tail grows until it holds one, the transcript's start or the cap.
#[test]
fn the_tail_reads_back_until_it_holds_the_workers_newest_post() {
    let post = shell_entry(1_000, "a", "tolkster activity 11111111-aaaa thought --body-file -");
    let filler = format!("{}\n", "x".repeat(1024));
    let text = format!("{}\n{post}\n{}{}", filler.repeat(1000), filler.repeat(600), entry(2_000, "b"));
    let len = text.len() as u64;
    let posted = |tail: &str| last_report(tail).is_some();
    let mut reads = Vec::new();
    let (tail, whole) = tail_back(len, |from| {
        reads.push(from);
        Some(text[from as usize..].to_string())
    }, posted)
    .unwrap();
    assert_eq!(last_report(&tail).map(|r| r.session).as_deref(), Some("11111111-aaaa"));
    assert!(!whole);
    assert_eq!(reads, [len - TAIL_BYTES, len - 2 * TAIL_BYTES, len - 4 * TAIL_BYTES]);
    // With no post anywhere, it stops at the start, which is the whole transcript.
    let (_, whole) = tail_back(len, |_| Some(String::new()), posted).unwrap();
    assert!(whole);
    // A read that fails reads nothing.
    assert_eq!(tail_back(len, |_| None, posted), None);
    // Nor past the cap.
    let mut reads = 0;
    tail_back(10 * MOST_BYTES, |_| {
        reads += 1;
        Some(String::new())
    }, posted);
    assert_eq!(reads, 7);
}

// The hook posts the `Committed` action after a shell call of the worker's that commits
// (TW-622), whatever else the command runs.
#[test]
fn a_call_commits_when_it_runs_git_commit() {
    let shell_call = |tool: &str, command: &str| call(tool, json!({ "command": command }));
    for yes in ["git commit -m x", "git add -A && git commit -F C:/t/m.txt", "cd D:/wt; git -C D:/wt commit -q -m y", "git -c core.hooksPath=x commit -m y", "git commit -m x && git push && tolkster pr create --head b"] {
        assert!(commits(&shell_call("Bash", yes)), "{yes}");
    }
    assert!(commits(&shell_call("PowerShell", "git commit -m x")));
    for no in ["git status", "git log -1", "echo git commit", "git rebase --continue"] {
        assert!(!commits(&shell_call("Bash", no)), "{no}");
    }
    assert!(!commits(&call("Read", json!({ "command": "git commit" }))));
    // A sub-agent's commit isn't the worker's to report.
    assert!(!commits(&Call { sub_agent: true, ..shell_call("Bash", "git commit -m x") }));
}

fn moved(sha: char, at: i64, why: &str) -> Move {
    Move { sha: Sha::parse(&sha.to_string().repeat(40)).unwrap(), at, why: why.to_string() }
}

#[test]
fn a_calls_commit_is_the_oldest_since_it_was_made() {
    let made = 1_700_000_000_500;
    let sha = |c: char| Sha::parse(&c.to_string().repeat(40)).unwrap();
    assert_eq!(made_at(&entry(made, "a"), "a"), Some(made));
    assert_eq!(made_at(&entry(made, "a"), "b"), None);
    assert_eq!(committed(&[moved('a', 1_700_000_000, "commit: Add the flag (TW-1)")], made), Some((sha('a'), "Add the flag (TW-1)".into())));
    assert_eq!(committed(&[moved('a', 1_700_000_003, "commit (amend): Add the flag")], made), Some((sha('a'), "Add the flag".into())));
    // A commit made since, in the next call, isn't this call's: the oldest since it is.
    let since = [moved('b', 1_700_000_009, "commit: Second"), moved('a', 1_700_000_001, "commit: First"), moved('0', 1_699_999_000, "commit: Before")];
    assert_eq!(committed(&since, made), Some((sha('a'), "First".into())));
    // A rebase since leaves the commit before it.
    let rebased = [moved('c', 1_700_000_009, "rebase (finish): returning to refs/heads/x"), moved('b', 1_700_000_008, "rebase (pick): First"), moved('a', 1_700_000_001, "commit: First")];
    assert_eq!(committed(&rebased, made), Some((sha('a'), "First".into())));
    // HEAD last moved before the call, so its commit failed; or a merge, a reset or a
    // checkout moved it.
    assert_eq!(committed(&[moved('a', 1_699_999_999, "commit: Add the flag")], made), None);
    for why in ["commit (merge): Merge branch 'main'", "reset: moving to HEAD~1", "checkout: moving from a to b", "commit: ", "commit"] {
        assert_eq!(committed(&[moved('a', 1_700_000_001, why)], made), None, "{why}");
    }
    assert_eq!(committed(&[], made), None);
}

#[test]
fn a_commits_session_is_the_one_reported_in_when_it_was_made() {
    let tail = [
        shell_entry(1_000, "a", "tolkster activity 11111111-aaaa thought --body-file -"),
        shell_entry(2_000, "b", "git commit -m x"),
        shell_entry(3_000, "c", "tolkster activity 22222222-bbbb response --body-file -"),
        shell_entry(4_000, "d", "tolkster pr create --head x"),
    ]
    .join("\n");
    assert_eq!(report_through(&tail, "b"), Some(Report { session: "11111111-aaaa".into(), kind: Some("thought".into()) }));
    assert_eq!(report_through(&tail, "z"), None);
    // A `tolkster pr` run since the commit may be setting the card.
    assert!(pr_since(&tail, "b") && !pr_since(&tail, "d"));
    assert!(pr_since(&tail, "z"));
}

#[test]
fn tests_builds_pushes_rebases_and_sub_agents_pair() {
    let pair = |command: &str| pairing(&call("Bash", json!({ "command": command })));
    for (command, kind) in [
        ("cargo test --locked 2>&1 | grep -E 'test result'", "testing"),
        ("npm run typecheck", "building"),
        ("git push -u origin tw-1", "pushing"),
        ("git fetch origin main && git rebase origin/main", "rebasing"),
        ("git fetch origin && git rebase origin/main && git push --force-with-lease", "pushing"),
    ] {
        assert_eq!(pair(command), Some(kind), "{command}");
    }
    // A commit's `Committed` action stands for it, a push in the same call included, and the
    // worker's own reporting and any other command post no action.
    for command in ["git commit -m x && git push", "cargo test && git commit -m x", "tolkster activity s1 thought --state running-checks; cargo test", "tolkster pr checks --pr 4", "orca terminal read", "git status", "ls"] {
        assert_eq!(pair(command), None, "{command}");
    }
    assert_eq!(pairing(&call("Agent", json!({ "prompt": "review" }))), Some("delegating"));
    assert_eq!(pairing(&call("Task", json!({}))), Some("delegating"));
    assert_eq!(pairing(&call("Read", json!({ "file_path": "a.rs" }))), None);
    assert_eq!(pairing(&Call { sub_agent: true, ..call("Bash", json!({ "command": "cargo test" })) }), None);
    // Each has its action in voice.json.
    for kind in ["testing", "building", "pushing", "rebasing", "delegating"] {
        assert!(crate::voice::action(kind).is_some(), "{kind}");
    }
}

#[test]
fn the_hook_after_a_call_reads_whether_it_failed_or_went_to_the_background() {
    let input = |event: &str, response: Value| {
        let input = json!({ "transcript_path": "C:\\t\\abc.jsonl", "cwd": "D:\\wt\\tw-1", "hook_event_name": event, "tool_name": "Bash", "tool_input": { "command": "cargo test" }, "tool_use_id": "toolu_9", "tool_response": response });
        let call = parse_call(&input.to_string()).unwrap();
        (call.failed, call.launched)
    };
    assert_eq!(input("PostToolUse", json!({ "stdout": "", "interrupted": false })), (false, false));
    assert_eq!(input("PostToolUseFailure", json!({})), (true, false));
    assert_eq!(input("PostToolUse", json!({ "stdout": "", "backgroundTaskId": "b1" })), (false, true));
    assert_eq!(input("PostToolUse", json!({ "isAsync": true, "status": "async_launched" })), (false, true));
}

// A call's result in the transcript: failed or not, or a start in the background.
fn result_entry(id: &str, failed: bool, launched: bool) -> String {
    let result = if launched { json!({ "backgroundTaskId": "b1" }) } else { json!({ "stdout": "" }) };
    json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": id, "content": "x", "is_error": failed }] }, "toolUseResult": result }).to_string()
}

// The notice Claude Code queues once a call in the background ends.
fn notice_entry(id: &str, status: &str) -> String {
    let notice = format!("<task-notification>\n<task-id>b1</task-id>\n<tool-use-id>{id}</tool-use-id>\n<status>{status}</status>\n<summary>x</summary>\n</task-notification>");
    json!({ "type": "queue-operation", "operation": "enqueue", "content": notice }).to_string()
}

#[test]
fn a_call_runs_until_its_result_or_for_one_in_the_background_its_notice() {
    let run = |lines: &[String]| run_of(&lines.join("\n"), "t");
    let called = shell_entry(1_000, "t", "cargo test");
    assert_eq!(run(&[]), Run::Gone);
    assert_eq!(run(std::slice::from_ref(&called)), Run::Running);
    assert_eq!(run(&[called.clone(), result_entry("t", true, false)]), Run::Returned);
    assert_eq!(run(&[called.clone(), result_entry("t", false, true)]), Run::Running);
    assert_eq!(run(&[called.clone(), result_entry("t", false, true), notice_entry("t", "completed")]), Run::Finished(Some(false)));
    assert_eq!(run(&[called.clone(), result_entry("t", false, true), notice_entry("t", "failed")]), Run::Finished(Some(true)));
    assert_eq!(run(&[called.clone(), result_entry("t", false, true), notice_entry("t", "killed")]), Run::Finished(None));
    // Another call's notice, or a notice quoted in a result, isn't this one's.
    assert_eq!(run(&[called.clone(), result_entry("t", false, true), notice_entry("u", "completed")]), Run::Running);
    let quoted = json!({ "type": "user", "message": { "content": [{ "type": "tool_result", "tool_use_id": "u", "content": notice_entry("t", "completed") }] } }).to_string();
    assert_eq!(run(&[called.clone(), result_entry("t", false, true), quoted]), Run::Running);
    let relayed = json!({ "type": "queue-operation", "content": format!("<task-notification>
<tool-use-id>u</tool-use-id>
<status>completed</status>
<result>{}</result>
</task-notification>", notice_entry("t", "failed")) }).to_string();
    assert_eq!(run(&[called, result_entry("t", false, true), relayed]), Run::Running);
}

// A turn that ends to wait on a call it sent to the background is still its work; once every
// such call's notice is in, its call made before the window, or with no transcript to read, it
// is idle.
#[test]
fn a_stop_with_a_call_in_the_background_and_no_notice_is_in_the_background() {
    let stop = json!({ "transcript_path": "C:/t/abc.jsonl", "cwd": "D:/wt/tw-1", "hook_event_name": "Stop" }).to_string();
    let since = |lines: &[String], since: i64| parse_state(&stop, since, |path| (path == std::path::Path::new("C:/t/abc.jsonl")).then(|| lines.join("\n"))).map(|(observed, _)| observed);
    let state = |lines: &[String]| since(lines, 1_000);
    let (gate, check) = (shell_entry(1_000, "t", "node scripts/gate.ts"), shell_entry(2_000, "u", "cargo test"));
    let launched = [gate.clone(), result_entry("t", false, true), check.clone(), result_entry("u", false, false)];
    assert_eq!(state(&launched), Some(Observed::AgentBackground));
    for ended in ["completed", "failed", "killed"] {
        assert_eq!(state(&[launched.as_slice(), &[notice_entry("t", ended)]].concat()), Some(Observed::AgentIdle), "{ended}");
    }
    assert_eq!(since(&launched, 1_001), Some(Observed::AgentIdle));
    assert!(reaches(&launched.join("\n"), 1_001) && !reaches(&launched.join("\n"), 1_000));
    assert_eq!(state(&[gate, result_entry("t", false, false)]), Some(Observed::AgentIdle));
    assert_eq!(parse_state(&stop, 0, |_| None).map(|(observed, _)| observed), Some(Observed::AgentIdle));
}

#[test]
fn only_an_end_whose_one_program_is_the_check_says_how_it_went() {
    let shell = |command: &str| known(Tool::Shell, Some(command));
    for yes in ["cargo test --locked", "cd cli && cargo test hook", "npm run typecheck", "cargo test 2>&1", "git rebase origin/main", "git push --force-with-lease"] {
        assert!(shell(yes), "{yes}");
    }
    // The shell exits as its last program did: a filter's, an echo's or another check's.
    for no in ["cargo test 2>&1 | grep -E '^test result'", "npm run typecheck 2>&1 | tail -n 3", "cargo test; echo done", "tolkster pr checks --pr 4", "pwsh -c \"npm test\"", "cargo test; cargo build", "cargo test -p a; cargo test -p b", "cargo test && cargo clippy", "tolkster pr checks --pr 4; cargo test"] {
        assert!(!shell(no), "{no}");
    }
    assert!(known(Tool::SubAgent, None) && !known(Tool::Read, None));
}

#[test]
fn the_ticker_ticks_until_the_calls_end_is_in_the_tail() {
    // Runs the ticker over the tails it reads in turn, one per POLL, the last one again.
    let ticks = |tails: Vec<String>| {
        let (mut reads, mut said) = (0, Vec::new());
        tick("t", || { reads += 1; tails.get(reads - 1).or(tails.last()).cloned() }, |_| {}, |_, tick| { said.push(tick); true });
        (reads, said)
    };
    let every = (TICK_EVERY.as_secs() / POLL.as_secs()) as usize;
    let running = shell_entry(1_000, "t", "cargo test");
    // A run of two ticks, then its result: the hook after the call posts the outcome.
    let mut tails = vec![running.clone(); 2 * every + 1];
    tails.push([running.clone(), result_entry("t", false, false)].join("\n"));
    assert_eq!(ticks(tails), (2 * every + 2, vec![Tick::Minutes(3), Tick::Minutes(6)]));
    // One in the background ticks on past its result, and says how it ended.
    let launched = [running.clone(), result_entry("t", false, true)].join("\n");
    let mut tails = vec![launched.clone(); every];
    tails.push(format!("{launched}\n{}", notice_entry("t", "failed")));
    assert_eq!(ticks(tails).1, vec![Tick::Minutes(3), Tick::Ended(true)]);
    // A call not written yet is waited for, but one still missing at the first tick ends it.
    assert_eq!(ticks(vec![String::new()]), (every, vec![]));
    // A transcript it can't read ends it, and a run that never ends stops at TICK_FOR.
    let (mut reads, mut said) = (0, 0);
    tick("t", || { reads += 1; (reads < 3).then(|| running.clone()) }, |_| {}, |_, _| { said += 1; true });
    assert_eq!((reads, said), (3, 0));
    // A tick that answers false, as for a worker that's gone, ends it.
    let (mut reads, mut said) = (0, 0);
    tick("t", || { reads += 1; Some(running.clone()) }, |_| {}, |_, _| { said += 1; false });
    assert_eq!((reads, said), (every, 1));
    let (reads, said) = ticks(vec![running]);
    assert_eq!((reads as u64, said.len() as u64), (TICK_FOR.as_secs() / POLL.as_secs(), TICK_FOR.as_secs() / TICK_EVERY.as_secs()));
}
