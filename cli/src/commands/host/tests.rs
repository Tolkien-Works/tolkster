use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::JoinHandle;

use super::*;
use crate::http::agent;
use crate::state::State;

fn url() -> WorkerUrl {
    WorkerUrl::parse("https://tolkster.example.workers.dev/").unwrap()
}

// The settings are the Worker's, but `worker` is where the code was traded, and a credential
// not issued to the host the answer names is refused rather than written.
#[test]
fn a_join_writes_what_the_worker_answered_with_its_own_address() {
    let answer = json!({ "host": "wsl", "credential": format!("wsl:{}", "A".repeat(43)), "config": { "owner": { "id": "o" }, "slots": { "wsl": 2 }, "worker": "https://elsewhere" } }).to_string();
    let (host, credential, config) = joined(200, &answer, &url()).unwrap();
    assert_eq!((host.as_str(), credential.starts_with("wsl:")), ("wsl", true));
    assert_eq!(Value::Object(config), json!({ "owner": { "id": "o" }, "slots": { "wsl": 2 }, "worker": "https://tolkster.example.workers.dev", "$schema": "./config.schema.json" }));
    for bad in [json!({ "host": "wsl", "credential": format!("mac:{}", "A".repeat(43)), "config": {} }), json!({ "host": "wsl", "credential": "wsl:x", "config": [] }), json!({ "host": "-x", "credential": "-x:y", "config": {} })] {
        assert!(joined(200, &bad.to_string(), &url()).is_err(), "{bad}");
    }
}

// A used, expired or unknown code says to run `host add` again, and never echoes the answer.
#[test]
fn a_refused_code_writes_nothing_and_says_what_to_do() {
    let error = joined(404, "Unknown code", &url()).unwrap_err().to_string();
    assert!(error.contains("host add again"), "{error}");
    assert!(!joined(500, "secret-ish", &url()).unwrap_err().to_string().contains("secret-ish"));
}

// A Worker's state on a local port that answers one request with `answer`, and returns the
// `op` body it received (null when the test connects and closes without asking).
fn serving(answer: Value) -> (WorkerUrl, JoinHandle<Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = WorkerUrl::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut received = String::new();
        let mut buffer = [0u8; 1024];
        loop {
            if let Some((head, body)) = received.split_once("\r\n\r\n") {
                let length = head.to_ascii_lowercase().lines().find_map(|l| l.strip_prefix("content-length: ")?.trim().parse::<usize>().ok()).unwrap_or(0);
                if body.len() >= length {
                    break;
                }
            }
            let n = stream.read(&mut buffer).unwrap();
            if n == 0 {
                return Value::Null;
            }
            received.push_str(&String::from_utf8_lossy(&buffer[..n]));
        }
        let text = answer.to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len()).unwrap();
        serde_json::from_str(received.split_once("\r\n\r\n").unwrap().1).unwrap()
    });
    (url, server)
}

// The Worker scopes a host to workers whose worktree ID starts `<environment id>@`, so its
// credential is minted and revoked for the environment's ID, never its name.
#[test]
fn a_host_is_enrolled_by_its_environment_id() {
    let (url, server) = serving(json!({ "code": "c0de" }));
    let agent = agent();
    let state = State::new(&agent, &url, "secret".into());
    let host = Host { id: "env-1".into(), name: "wsl".into() };
    assert_eq!(crate::onboard::enrol(&state, &host, &json!({})).unwrap(), "c0de");
    let body = server.join().unwrap();
    assert_eq!((body["op"].as_str(), body["host"].as_str()), (Some("enrolHost"), Some("env-1")));
}

#[test]
fn rm_revokes_the_id_orca_pairs_under_the_name_else_the_argument() {
    let paired = [Host { id: "env-1".into(), name: "wsl".into() }, Host { id: "env-2".into(), name: "mac".into() }];
    assert_eq!(credential_id(&paired, "wsl"), "env-1");
    assert_eq!(credential_id(&paired, "gone"), "gone");
    assert_eq!(credential_id(&paired, "env-2"), "env-2");
}

// A failed recipe run revokes the environment it paired as, by ID, and none when it never paired.
#[test]
fn a_failed_recipe_revokes_the_paired_environment_s_id() {
    let (url, server) = serving(json!({ "held": true }));
    let agent = agent();
    let state = State::new(&agent, &url, "secret".into());
    crate::recipe::revoke(&state, &Some("env-1".into()));
    let body = server.join().unwrap();
    assert_eq!((body["op"].as_str(), body["host"].as_str()), (Some("revokeHost"), Some("env-1")));
    // Nothing paired: nothing is asked of the Worker.
    let (url, server) = serving(json!({}));
    crate::recipe::revoke(&State::new(&agent, &url, "secret".into()), &None);
    TcpStream::connect(url.base().trim_start_matches("http://")).unwrap();
    assert_eq!(server.join().unwrap(), Value::Null);
}
