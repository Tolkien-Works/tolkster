use std::path::Path;

use serde_json::{Value, json};

use super::*;
use crate::harnesses::CLAUDE;
use crate::harnesses::tests::{BARE, STUB};
use crate::plain::Line;

#[test]
fn a_worker_is_launched_with_its_hooks_and_its_prompt_last() {
    let hooks = Path::new("C:/Users/some one/.claude/tolkster-briefs/hooks.json");
    let mcp = Path::new("C:/Users/some one/.claude/tolkster-briefs").join("mcp.json").display().to_string();
    let launch = crate::config::ladder::Launch::new(crate::harnesses::DEFAULT, "claude-sonnet-5-5", "high");
    let prompt: Line = "Read the file".into();
    // No MCP servers but the empty file's, and `--settings` after it, so the prompt that
    // follows isn't taken for another MCP file. No shell reads them, so a path may hold a space.
    assert_eq!(args(&CLAUDE, true, Some(&launch), Some(hooks), &prompt), ["--continue", "--model", "claude-sonnet-5-5", "--effort", "high", "--strict-mcp-config", "--mcp-config", mcp.as_str(), "--settings", "C:/Users/some one/.claude/tolkster-briefs/hooks.json", "Read the file"]);
    assert_eq!(args(&CLAUDE, false, None, None, &prompt), ["Read the file"]);
    // Another harness's flags, and no settings it doesn't read.
    let stub = Launch::new(&STUB, "stub-large", "deep");
    assert_eq!(args(&STUB, true, Some(&stub), Some(hooks), &prompt), ["--resume-last", "--no-trust-dialog", "--no-mcp", "-m", "stub-large", "--depth", "deep", "Read the file"]);
    assert_eq!(continued(Some(&stub)).unwrap().as_str(), "stub-agent --resume-last --no-trust-dialog --no-mcp -m stub-large --depth deep");
    // An effort another harness's, or a harness this build doesn't launch, launches nothing.
    assert!(continued(Some(&Launch { effort: "high".into(), ..stub.clone() })).is_err());
    assert!(continued(Some(&Launch { harness: "codex".into(), ..stub })).is_err());
    // A line a terminal's shell reads is plain, its model a plain name, never extra flags.
    assert_eq!(command(None).unwrap().as_str(), "claude");
    assert_eq!(continued(Some(&launch)).unwrap().as_str(), "claude --continue --model claude-sonnet-5-5 --effort high");
    assert!(continued(Some(&crate::config::ladder::Launch { model: "opus --dangerously-skip-permissions".into(), ..launch })).is_err());
    assert_eq!(serde_json::from_str::<serde_json::Value>(NO_MCP).unwrap(), serde_json::json!({ "mcpServers": {} }));
    // Before a shell call or a sub-agent, the call waits on the start's post, so the post
    // lands before its outcome. Before every tool call, in the background, the line or the
    // ticker, which outlives the longest call it ticks.
    let settings: serde_json::Value = serde_json::from_str(HOOKS).unwrap();
    let hook = |event: &str, n: usize| {
        let entry = &settings["hooks"][event][n];
        (entry["matcher"].as_str(), entry["hooks"][0]["command"].as_str(), entry["hooks"][0]["async"].as_bool())
    };
    assert_eq!(hook("PreToolUse", 0), (Some("Bash|PowerShell|Agent|Task"), Some("tolkster hook start"), None));
    assert_eq!(hook("PreToolUse", 1), (Some("*"), Some("tolkster hook tool"), Some(true)));
    assert!(settings["hooks"]["PreToolUse"][1]["hooks"][0]["timeout"].as_u64().unwrap() > crate::hook::TICK_FOR.as_secs());
    // After each call, in the background, a commit's `Committed` action, the call's outcome or
    // a traced session's action: after one that exits non-zero too (`git commit && git push`
    // with the push failing, a failed test run).
    for event in ["PostToolUse", "PostToolUseFailure"] {
        assert_eq!(hook(event, 0), (Some("*"), Some("tolkster hook after"), Some(true)), "{event}");
    }
    // The state hooks run in turn, never in the background, so one agent's reports land in
    // the order they happened.
    for event in ["UserPromptSubmit", "Stop", "Notification", "SessionEnd"] {
        let hook = &settings["hooks"][event][0]["hooks"][0];
        assert_eq!(hook["command"], "tolkster hook state", "{event}");
        assert!(hook.get("async").is_none(), "{event}");
    }
    assert_eq!(settings["hooks"]["Notification"][0]["matcher"], "permission_prompt|elicitation_dialog");
    assert_eq!(hooks_path("/b/<repo>-briefs", "tolkster"), Path::new("/b/tolkster-briefs").join("hooks.json"));
}

// A harness without a check on its host, or without Claude's settings or flags of its own for
// the trust dialog and MCP servers, can't run unattended safely, so nothing launches it.
#[test]
fn a_harness_that_misses_an_unattended_requirement_is_refused() {
    assert!(CLAUDE.unmet().is_empty() && STUB.unmet().is_empty());
    assert_eq!(BARE.unmet().len(), 2);
    assert_eq!(Harness { host: None, ..STUB }.unmet().len(), 1);
    assert_eq!(Harness { unattended: &[], ..STUB }.unmet().len(), 1);
    assert!(Harness { unattended: &[], settings: true, ..STUB }.unmet().is_empty());
    let bare = Launch { harness: "bare".into(), model: "m".into(), effort: "deep".into() };
    let error = check(Some(&bare)).unwrap_err().to_string();
    assert!(error.contains("can't run unattended safely"), "{error}");
    assert!(continued(Some(&bare)).is_err());
}

#[test]
fn a_workers_settings_are_both_files_this_builds() {
    let dir = std::env::temp_dir().join(format!("tolkster-settings-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let hooks = dir.join("hooks.json");
    let read = |path: &Path| std::fs::read_to_string(path).unwrap();
    write_settings(&hooks).unwrap();
    assert_eq!((read(&hooks), read(&mcp_path(&hooks))), (HOOKS.to_string(), NO_MCP.to_string()));
    // A file an older build wrote is rewritten, and no part file is left beside it.
    std::fs::write(mcp_path(&hooks), "{}").unwrap();
    write_settings(&hooks).unwrap();
    assert_eq!(read(&mcp_path(&hooks)), NO_MCP);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
    std::fs::remove_dir_all(&dir).unwrap();
}

// A worktree Claude hasn't seen is trusted before it starts, by its path with `/`, keeping
// everything else in Claude's file; one in a trusted folder, or trusted already, is left as
// it is.
#[test]
fn an_untrusted_worktree_is_trusted_once_and_a_trusted_one_left_alone() {
    let root = std::env::temp_dir().join(format!("tolkster-trust-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join(".claude.json");
    let read = || serde_json::from_str::<Value>(&std::fs::read_to_string(&file).unwrap()).unwrap();
    // No file yet: Claude's first.
    assert!(trust(&file, "C:\\work\\tw-1").unwrap());
    let key = if cfg!(windows) { "C:/work/tw-1" } else { "C:\\work\\tw-1" };
    assert_eq!(read()["projects"][key]["hasTrustDialogAccepted"], json!(true));
    std::fs::write(&file, json!({ "numStartups": 3, "projects": { "/Users/x/src": { "hasTrustDialogAccepted": true }, "/Users/x/wt/tw-2": { "allowedTools": ["Bash"] } } }).to_string()).unwrap();
    for trusted in ["/Users/x/src", "/Users/x/src/tw-1/"] {
        assert!(!trust(&file, trusted).unwrap(), "{trusted}");
    }
    assert!(trust(&file, "/Users/x/wt/tw-2").unwrap());
    let settings = read();
    assert_eq!(settings["projects"]["/Users/x/wt/tw-2"], json!({ "allowedTools": ["Bash"], "hasTrustDialogAccepted": true }));
    assert_eq!((settings["numStartups"].clone(), settings["projects"].as_object().unwrap().len()), (json!(3), 2));
    assert!(!trust(&file, "/Users/x/wt/tw-2").unwrap());
    // A file that isn't Claude's object is never overwritten.
    std::fs::write(&file, "[]").unwrap();
    assert!(trust(&file, "/Users/x/wt/tw-3").is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "[]");
    std::fs::remove_dir_all(&root).unwrap();
}
