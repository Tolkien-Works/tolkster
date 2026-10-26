use serde_json::Value;

use super::*;

#[test]
fn a_turn_runs_in_auto_mode_and_resumes_only_a_saved_session() {
    let args = args("u1", true);
    // No MCP servers, and a flag after the config, which takes every word up to the next.
    assert_eq!(&args[..8], ["--strict-mcp-config", "--mcp-config", "{\"mcpServers\":{}}", "-p", "--resume", "u1", "--permission-mode", "auto"]);
    assert_eq!(super::args("u1", false)[4], "--session-id");
    // A turn stays warm: its input and output are stream-json, after the rest.
    assert_eq!(&args[8..], ["--input-format", "stream-json", "--output-format", "stream-json", "--verbose"]);
    // No tool list: what a turn may run is auto mode's and the brief's to say.
    assert!(!args.iter().any(|a| a.contains("Tools")), "{args:?}");
    let path = transcript(Path::new("C:/Users/sam/.claude"), Path::new("D:\\wt\\tolkster-conversations"), "u1");
    assert_eq!(path, Path::new("C:/Users/sam/.claude/projects/D--wt-tolkster-conversations/u1.jsonl"));
}

#[test]
fn a_session_keeps_its_recorded_id_and_a_thread_takes_only_a_uuid() {
    assert_eq!(session_id(Some("c1")), "c1");
    assert!(takes_id(&session_id(None)));
    assert!(takes_id("9f0c1d2e-3a4b-4c5d-8e6f-7a8b9c0d1e2f"));
    assert!(!takes_id("9f0c1d2e-3a4b-4c5d-8e6f-7a8b9c0d1e2"));
    assert!(!takes_id("9f0c1d2e-3a4b-4c5d-8e6f_7a8b9c0d1e2f"));
}

#[test]
fn a_message_is_one_line_of_user_json() {
    let mut written = Vec::new();
    message(&mut written, "two\nlines").unwrap();
    let text = String::from_utf8(written).unwrap();
    assert_eq!(text.lines().count(), 1);
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), json!({ "type": "user", "message": { "role": "user", "content": "two\nlines" } }));
}

// The idle signal is one `Done` per `result`, and nothing else counts as one.
#[test]
fn the_output_is_heard_as_text_and_one_done_per_result() {
    let output = [
        r#"{"type":"system","subtype":"init","session_id":"u1","tools":["Bash"]}"#,
        r#"{"type":"assistant","message":{"id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"Hoo."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}],"stop_reason":null},"parent_tool_use_id":null,"session_id":"u1"}"#,
        "not json",
        r#"{"type":"user","message":{"content":[{"type":"text","text":"{\"type\":\"result\"}"}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"duration_ms":1200,"num_turns":1,"result":"Hoo.","session_id":"u1"}"#,
        r#"{"type":"result","subtype":"error_during_execution","is_error":true,"session_id":"u1"}"#,
    ]
    .join("\n");
    let mut heard = Vec::new();
    hear(output.as_bytes(), |h| heard.push(h));
    assert_eq!(heard, [Heard::Text("Hoo.".into()), Heard::Done, Heard::Done]);
}
