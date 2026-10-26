use super::*;

#[test]
fn base64_encodes_as_btoa_does() {
    assert_eq!(base64(b""), "");
    assert_eq!(base64(b"f"), "Zg==");
    assert_eq!(base64(b"fo"), "Zm8=");
    assert_eq!(base64(b"foo"), "Zm9v");
    assert_eq!(base64(b"id:secret"), "aWQ6c2VjcmV0");
}

#[test]
fn a_url_part_escapes_what_would_end_or_split_it() {
    assert_eq!(url_part("Tolkien-Works:tw-1/x y&z"), "Tolkien-Works:tw-1/x%20y%26z");
    // Git allows a `#` in a branch name, which would end a URL path.
    assert_eq!(url_part("main#x"), "main%23x");
}

// Answers one request on a local port with `200 {}`, and returns what it received.
fn serve_once() -> (String, std::thread::JoinHandle<String>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/x", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut received = String::new();
        let mut buffer = [0u8; 1024];
        let body_length = |head: &str| head.lines().find_map(|l| l.strip_prefix("content-length: ")?.trim().parse::<usize>().ok()).unwrap_or(0);
        loop {
            if let Some((head, body)) = received.split_once("\r\n\r\n")
                && body.len() >= body_length(&head.to_ascii_lowercase())
            {
                break;
            }
            let n = stream.read(&mut buffer).unwrap();
            assert!(n > 0, "closed before the request ended: {received}");
            received.push_str(&String::from_utf8_lossy(&buffer[..n]));
        }
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
        received.to_ascii_lowercase()
    });
    (url, server)
}

#[test]
fn a_post_sends_its_headers_then_the_content_type_and_the_token_with_the_body() {
    let (url, server) = serve_once();
    let (status, text) = post_json(&agent(), &url, "secret", &[("x-first", "1")], "{\"a\":1}").unwrap();
    assert_eq!((status, text.as_str()), (200, "{}"));
    let request = server.join().unwrap();
    assert!(request.starts_with("post /x http/1.1"), "{request}");
    let at = |header: &str| request.find(header).unwrap_or_else(|| panic!("{header} in {request}"));
    assert!(at("x-first: 1") < at("content-type: application/json") && at("content-type: application/json") < at("authorization: bearer secret"), "{request}");
    assert!(request.ends_with("{\"a\":1}"), "{request}");
}

#[test]
fn a_request_without_a_body_sends_no_content_type() {
    let (url, server) = serve_once();
    let response = send_json(&agent(), Method::GET, &url, "secret", &[], None).unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let request = server.join().unwrap();
    assert!(request.starts_with("get /x http/1.1") && request.contains("authorization: bearer secret"), "{request}");
    assert!(!request.contains("content-type"), "{request}");
}

