// The CLI's HTTP client and the encoding its requests need, for Linear, GitHub and
// Tolkster's state alike. What a status means is each caller's to say.

use std::time::Duration;

use ureq::http::{Method, Request, Response};
use ureq::config::RedirectAuthHeaders;
use ureq::{Agent, Body};

// Each request, from connecting to the last byte of the body. ureq sets no timeouts of
// its own, so without this a stalled connection would hang a worker's terminal forever.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

// A call that fails on the way, or with a 5xx, is sent again after these pauses, by a caller
// whose call is safe to repeat: the state's ops are, as records only move forward and a take
// or release carries its token.
pub const RETRIES: [Duration; 2] = [Duration::from_millis(500), Duration::from_secs(2)];

pub fn agent() -> Agent {
    // Status codes are handled here, as fetch leaves them to the caller.
    Agent::config_builder().http_status_as_error(false).timeout_global(Some(REQUEST_TIMEOUT)).build().into()
}

// For a file fetched with a token: https only, and a redirect, which may leave the host
// the token is for, never carries it.
pub fn file_agent() -> Agent {
    Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(REQUEST_TIMEOUT))
        .https_only(true)
        .max_redirects(3)
        .redirect_auth_headers(RedirectAuthHeaders::Never)
        .build()
        .into()
}

// A value as a URL's path or query takes it: everything but letters, digits and `-_./:` escaped.
pub fn url_part(value: &str) -> String {
    value.bytes().map(|b| if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/' | b':') { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

// One request with a bearer token, and a JSON body when there is one: `headers` first,
// then the content type and the token. The response as it came, whatever its status.
pub fn send_json(agent: &Agent, method: Method, url: &str, bearer: &str, headers: &[(&str, &str)], body: Option<&str>) -> Result<Response<Body>, ureq::Error> {
    let mut request = headers.iter().fold(Request::builder().method(method).uri(url), |r, (k, v)| r.header(*k, *v));
    if body.is_some() {
        request = request.header("Content-Type", "application/json");
    }
    let request = request.header("Authorization", format!("Bearer {bearer}"));
    match body {
        Some(body) => agent.run(request.body(body)?),
        None => agent.run(request.body(())?),
    }
}

// A JSON POST: the status and the body.
pub fn post_json(agent: &Agent, url: &str, bearer: &str, headers: &[(&str, &str)], body: &str) -> Result<(u16, String), ureq::Error> {
    let mut response = send_json(agent, Method::POST, url, bearer, headers, Some(body))?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string()?;
    Ok((status, text))
}

// A request with no credential and no body: the status and the body.
pub fn call_public(agent: &Agent, method: Method, url: &str, headers: &[(&str, &str)]) -> Result<(u16, String), ureq::Error> {
    let request = headers.iter().fold(Request::builder().method(method).uri(url), |r, (k, v)| r.header(*k, *v));
    let mut response = agent.run(request.body(())?)?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string()?;
    Ok((status, text))
}

// A GET with a bearer token: the status and the body's bytes, at most `limit` of them.
pub fn get_bytes(agent: &Agent, url: &str, bearer: &str, limit: u64) -> Result<(u16, Vec<u8>), ureq::Error> {
    let mut response = send_json(agent, Method::GET, url, bearer, &[], None)?;
    let status = response.status().as_u16();
    let bytes = response.body_mut().with_config().limit(limit).read_to_vec()?;
    Ok((status, bytes))
}

// A response header's value, when it has one that is text.
pub fn header(response: &Response<Body>, name: &str) -> Option<String> {
    response.headers().get(name).and_then(|v| v.to_str().ok()).map(String::from)
}

pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

#[cfg(test)]
mod tests;
