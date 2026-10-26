use super::*;
use crate::scheduler::BUILD_HOLDS;
use crate::trace::STUCK_AFTER;

// The relay's constants that mirror this side's are equal (src/worker/realtime.ts).
#[test]
fn the_relays_constants_match_the_typescript_worker() {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../src/worker/realtime.ts")).unwrap();
    // A constant's value, as `A * B * C`.
    let constant = |name: &str| -> u64 {
        let declaration = source.lines().find_map(|l| l.strip_prefix(&format!("export const {name} = "))).unwrap_or_else(|| panic!("{name} in src/worker/realtime.ts"));
        declaration.trim_end_matches(';').split('*').map(|factor| factor.trim().parse::<u64>().unwrap()).product()
    };
    assert_eq!(constant("LIVE_WITHIN_MS"), DEAD_AFTER.as_millis() as u64, "the relay reads a connection as gone after LIVE_WITHIN_MS; listen gives up after DEAD_AFTER");
    // The machines' own, in minutes (src/worker/machine/types.ts).
    let machine = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../src/worker/machine/types.ts")).unwrap();
    let away: u64 = machine.lines().find_map(|l| l.strip_prefix("export const AWAY_AFTER_MS = ")?.strip_suffix(" * MINUTE;")?.parse().ok()).expect("AWAY_AFTER_MS in src/worker/machine/types.ts");
    assert!(u128::from(away * 60 * 1000) > (STUCK_AFTER + BUILD_HOLDS + LAST_DELAY).as_millis(), "the away message waits out a self-update's drain, restart and reconnect");
    assert_eq!(source.lines().find_map(|l| l.strip_prefix("export const PING = ")), Some(format!("\"{PING}\";").as_str()), "PING differs between cli/src/listen.rs and src/worker/realtime.ts");
}

// Only the relay's superseded code ends `listen` for good; any other close reconnects.
#[test]
fn a_close_with_the_superseded_code_exits_and_every_other_reconnects() {
    use tungstenite::protocol::CloseFrame;
    use tungstenite::protocol::frame::coding::CloseCode;
    assert_eq!(serde_json::from_value::<u16>(crate::state::tests::contract()["supersededClose"].clone()).unwrap(), SUPERSEDED, "src/worker/realtime.ts SUPERSEDED");
    let close = |code: u16| Message::Close(Some(CloseFrame { code: CloseCode::from(code), reason: "x".into() }));
    assert!(superseded(&close(SUPERSEDED)));
    for other in [1000, 1001, 1006, 4001] {
        assert!(!superseded(&close(other)), "{other}");
    }
    assert!(!superseded(&Message::Close(None)) && !superseded(&Message::text("x")));
}

#[test]
fn a_failed_send_after_the_superseding_close_still_exits() {
    use tungstenite::protocol::CloseFrame;
    use tungstenite::protocol::frame::coding::CloseCode;
    let close = |code: u16| Wake::Frame(Ok(Message::Close(Some(CloseFrame { code: CloseCode::from(code), reason: "x".into() }))));
    let (wake, woken) = mpsc::channel();
    wake.send(Wake::Ended).unwrap();
    wake.send(close(SUPERSEDED)).unwrap();
    assert!(matches!(closed_first(End::Retry(Ended::Closed), &woken), End::Superseded));
    wake.send(close(1000)).unwrap();
    assert!(matches!(closed_first(End::Retry(Ended::Closed), &woken), End::Retry(_)));
    wake.send(close(SUPERSEDED)).unwrap();
    assert!(matches!(closed_first(End::Stop(Error::Failed("x".into())), &woken), End::Stop(_)));
}

#[test]
fn reconnects_back_off_to_a_minute_and_reset_after_a_stable_connection() {
    let mut backoff = Backoff::new();
    let quick = Duration::from_secs(1);
    let delays: Vec<u64> = (0..8).map(|_| backoff.delay(quick).as_secs()).collect();
    assert_eq!(delays, [1, 2, 4, 8, 16, 32, 60, 60]);
    assert_eq!(backoff.delay(STABLE).as_secs(), 1);
    assert_eq!(backoff.delay(quick).as_secs(), 2);
}

#[test]
fn a_refused_secret_stops_and_other_refusals_retry() {
    assert!(matches!(refused(401, None), End::Stop(_)));
    assert!(matches!(refused(404, None), End::Stop(_)));
    assert!(matches!(refused(503, None), End::Retry(_)));
    assert!(matches!(refused(502, None), End::Retry(_)));
}

#[test]
fn a_lookup_that_times_out_is_waited_on_again_not_started_again() {
    // A slot of its own: the connection tests share `PENDING` and could take the lookup between the calls.
    let slot: Mutex<Option<Lookup>> = Mutex::new(None);
    let pending = || slot.lock().unwrap().as_ref().map(|(key, _)| key.clone());
    let key = ("localhost".to_string(), 1);
    // Too short to finish, most likely: then the next connection picks it up.
    if resolve_in(&slot, "localhost", 1, Duration::ZERO).is_err() {
        assert_eq!(pending(), Some(key.clone()));
    }
    assert!(!resolve_in(&slot, "localhost", 1, Duration::from_secs(10)).unwrap().is_empty());
    assert_ne!(pending(), Some(key));
}

// A server that accepts the connection and then never answers.
fn silent_server() -> (std::net::TcpListener, u16) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

#[test]
fn a_handshake_that_stalls_gives_up_and_retries() {
    // The WebSocket handshake over plain ws, and the TLS one over wss.
    for scheme in ["ws", "wss"] {
        let (listener, port) = silent_server();
        let started = Instant::now();
        let end = connect(&format!("{scheme}://127.0.0.1:{port}/realtime"), "secret", Duration::from_millis(300), false);
        assert!(matches!(end, Err(End::Retry(_))), "{scheme}");
        assert!(started.elapsed() < Duration::from_secs(10), "{scheme}");
        drop(listener);
    }
}

#[test]
fn a_refused_upgrade_still_maps_through_refused() {
    let (listener, port) = silent_server();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0; 4096];
        let _ = io::Read::read(&mut stream, &mut buffer);
        stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n").unwrap();
    });
    let end = connect(&format!("ws://127.0.0.1:{port}/realtime"), "secret", Duration::from_secs(5), false);
    assert!(matches!(end, Err(End::Stop(_))));
    server.join().unwrap();
}

// The Worker's 503 names the setting it lacks (src/worker/realtime.ts, UNSET_SETTINGS), and
// `listen` names that one, never another.
#[test]
fn a_503_names_the_setting_the_worker_lacks() {
    let pinned: Vec<String> = serde_json::from_value(crate::state::tests::contract()["unsetSettings"].clone()).unwrap();
    assert_eq!(pinned, [Setting::OwnerUserId, Setting::RealtimeSecret].map(|setting| setting.name()), "src/worker/realtime.ts UNSET_SETTINGS");
    let ended = |body: Option<&[u8]>| match refused(503, body) {
        End::Retry(ended) => ended,
        _ => panic!("a 503 retries"),
    };
    assert_eq!(ended(Some(br#"{"missing":["OWNER_USER_ID"]}"#)), Ended::Unset(Setting::OwnerUserId));
    assert_eq!(ended(Some(br#"{"missing":["REALTIME_SECRET"]}"#)), Ended::Unset(Setting::RealtimeSecret));
    // A name outside the set, a body that isn't the shape, or none: an unnamed 503.
    for body in [Some(&br#"{"missing":["LINEAR_CLIENT_SECRET"]}"#[..]), Some(&b"Not configured"[..]), Some(&br#"{"missing":[]}"#[..]), None] {
        assert_eq!(ended(body), Ended::Status(503));
    }
}

// A frame sent with the handshake's response is read by the handshake, so no byte comes to
// the reader's wait for it: it is handed over all the same.
#[test]
fn a_frame_that_came_with_the_handshake_is_handed_over() {
    use std::io::{BufRead, BufReader};
    let (listener, port) = silent_server();
    let (done, finished) = mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let mut stream = listener.accept().unwrap().0;
        let mut key = String::new();
        let mut request = BufReader::new(stream.try_clone().unwrap());
        loop {
            let mut line = String::new();
            assert_ne!(request.read_line(&mut line).unwrap(), 0, "the request ended before its headers did");
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("sec-websocket-key")
            {
                key = value.trim().to_string();
            }
            if line == "\r\n" {
                break;
            }
        }
        let accept = tungstenite::handshake::derive_accept_key(key.as_bytes());
        let mut bytes = format!("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").into_bytes();
        // An unmasked text frame, as a server sends it, in the same write.
        bytes.extend([0x81, 5]);
        bytes.extend(b"hello");
        stream.write_all(&bytes).unwrap();
        let _ = finished.recv();
    });
    let socket = connect(&format!("ws://127.0.0.1:{port}/realtime"), "secret", HANDSHAKE_TIMEOUT, false).unwrap_or_else(|end| panic!("connecting ended with {}", name(&end)));
    let (frames, woken) = mpsc::channel();
    let _reading = Reading::start(socket, frames).unwrap();
    // Well short of READ_TIMEOUT, so no timed-out wait hands it over in its place.
    match woken.recv_timeout(Duration::from_secs(2)) {
        Ok(Wake::Frame(Ok(Message::Text(text)))) => assert_eq!(text.as_str(), "hello"),
        Ok(Wake::Frame(other)) => panic!("the reader handed over {other:?}"),
        Ok(Wake::Ended | Wake::Stopped) => panic!("the reader stopped"),
        Err(error) => panic!("no frame was handed over: {error}"),
    }
    done.send(()).unwrap();
    server.join().unwrap();
}

// What a connection ended on, for a failure to name.
fn name(end: &End) -> String {
    match end {
        End::Retry(why) => format!("a retry: {why:?}"),
        End::Stop(error) => format!("a stop: {error}"),
        End::Restart(_) => "a restart".into(),
        End::Superseded => "superseded".into(),
        End::Quit => "quit".into(),
    }
}

// A result goes out as its command's job ends, while no frame comes to end a read.
#[test]
fn a_result_goes_out_as_its_job_ends_not_on_the_next_read() {
    let (listener, port) = silent_server();
    // The server returns what its read failed on, not unwraps it, so a connection that
    // ended early is named by the `End` asserted below, not by a panic here.
    let server = std::thread::spawn(move || -> tungstenite::Result<Instant> {
        let mut socket = tungstenite::accept(listener.accept().unwrap().0).unwrap();
        let command = serde_json::json!({ "type": "command", "id": "c-1", "kind": "readIssue", "issueId": "i1", "identifier": "TW-1", "version": 1, "urgent": false, "args": {} });
        socket.send(Message::text(command.to_string()))?;
        loop {
            if let Message::Text(text) = socket.read()?
                && text.contains("\"c-1\"")
            {
                return Ok(Instant::now());
            }
        }
    });
    let began = Arc::new(Mutex::new(None));
    let beginning = Arc::clone(&began);
    let mut scheduler = Scheduler::spawn(Arc::default(), move |_, job, _| {
        if let crate::scheduler::Job::Command(command) = job {
            *beginning.lock().unwrap() = Some(Instant::now());
            // Long enough for the loop to be waiting on the connection: were it not, the
            // loop would send the result on its next turn anyway, and the test would pass
            // without proving the wake.
            sleep(Duration::from_millis(300));
            crate::scheduler::answer(command, &crate::machine::Finished::ok(serde_json::json!({})));
        }
    });
    let reading = std::thread::spawn(move || connection(&format!("ws://127.0.0.1:{port}/realtime"), "secret", &mut Vec::new(), Some(&mut scheduler), false));
    let received = server.join().unwrap();
    // The server is done, so the connection ends: of what it saw, or, if it ended first,
    // of its own `End`, which the failure message names.
    let end = reading.join().unwrap();
    assert!(matches!(end, End::Retry(_)), "the connection ended with {}", name(&end));
    let received = received.unwrap_or_else(|error| panic!("the server read failed: {error}; the connection ended with {}", name(&end)));
    // Not a margin on the job's 300 ms: a result left to the loop's next read goes out
    // when READ_TIMEOUT wakes it, about READ_TIMEOUT after the job began, so a result
    // received well before that was sent by the wake. Half of it either side leaves a
    // runner seconds of slack.
    let began = began.lock().unwrap().unwrap();
    let took = received.duration_since(began);
    assert!(took < READ_TIMEOUT / 2, "the result took {took:?} after its job began; READ_TIMEOUT is {READ_TIMEOUT:?}");
}
