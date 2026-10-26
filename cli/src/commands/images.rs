// `tolkster issue show --images`: the files people uploaded into an issue's text, saved
// where the worker can open them. Only URLs found in the issue's own description and
// comments are fetched, never one given on the command line, and only from Linear's
// upload host, which alone gets the token (`linear::download`). A file is named from a
// hash of its URL and its type, never from Linear's text: an upload's URL always holds the
// same bytes, so two runs on one issue, or one that the text changed between, agree on
// every path.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;
use serde_json::{Value, json};
use ureq::Agent;

use crate::commands::show::at_once;
use crate::error::Error;
use crate::json::decode_ref;
use crate::linear::{Nodes, UPLOADS, download, is_upload, is_url_char};
use crate::plain::split_identifier;

// At most this many files per issue, each at most this large.
const MAX_IMAGES: usize = 20;
const MAX_BYTES: u64 = 10 * 1024 * 1024;

// The image types a worker can open, and the extension each is saved with.
const TYPES: &[(&str, &str)] = &[("image/png", "png"), ("image/jpeg", "jpg"), ("image/gif", "gif"), ("image/webp", "webp")];

// The upload URLs in `texts`, in order, each once.
pub fn find(texts: &[&str]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for text in texts {
        for (at, _) in text.match_indices(UPLOADS) {
            let path = &text[at + UPLOADS.len()..];
            let url = &text[at..at + UPLOADS.len() + path.find(|c| !is_url_char(c)).unwrap_or(path.len())];
            // Prose's full stop or question mark after a URL is not part of it.
            let url = url.trim_end_matches(['.', '?', '&', '=']);
            if is_upload(url) && !found.iter().any(|f| f == url) {
                found.push(url.to_string());
            }
        }
    }
    found
}

// What the uploads are found in: the issue's description and, when read, its comments.
#[derive(Deserialize)]
struct Texts {
    identifier: String,
    description: Option<String>,
    comments: Option<Nodes<Body>>,
}

#[derive(Deserialize)]
struct Body {
    body: String,
}

// The description's and, when read, the comments' upload URLs, each as `{url, path}` once
// saved or `{url, error}` in its place. A failure is fixed text, so none prints Linear's
// answer, and doesn't fail the rest. `issue` is as `issue show` read it.
pub fn fetch(agent: &Agent, issue: &Value) -> Result<Vec<Value>, Error> {
    let issue: Texts = decode_ref(issue, "Linear's answer to Show")?;
    let mut texts: Vec<&str> = vec![issue.description.as_deref().unwrap_or_default()];
    texts.extend(issue.comments.iter().flat_map(|c| &c.nodes).map(|c| c.body.as_str()));
    let urls = find(&texts);
    if split_identifier(&issue.identifier).is_none() {
        return Ok(urls.into_iter().map(|url| json!({ "url": url, "error": "not fetched: no issue identifier" })).collect());
    }
    let dir = std::env::temp_dir().join("tolkster-images").join(&issue.identifier);
    let saved = at_once(urls.iter().take(MAX_IMAGES).map(|url| {
        let dir = &dir;
        move || match save(agent, url, dir) {
            Ok(path) => json!({ "url": url, "path": path.to_string_lossy() }),
            Err(error) => json!({ "url": url, "error": error }),
        }
    }));
    let over = urls.iter().skip(MAX_IMAGES).map(|url| json!({ "url": url, "error": format!("not fetched: over {MAX_IMAGES} per issue") }));
    Ok(saved.into_iter().chain(over).collect())
}

fn save(agent: &Agent, url: &str, dir: &Path) -> Result<PathBuf, &'static str> {
    let (mime, bytes) = download(agent, url, MAX_BYTES).map_err(|error| {
        eprintln!("{url}: {error}");
        "not fetched: see stderr"
    })?;
    let ext = extension(&mime).ok_or("not fetched: not an image")?;
    let path = dir.join(format!("{}.{ext}", name(url)));
    write(dir, &path, &bytes).map_err(|error| {
        eprintln!("{}: {error}", path.display());
        "not saved: see stderr"
    })?;
    Ok(path)
}

// The first 16 hex digits of the URL's SHA-256.
fn name(url: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, url.as_bytes()).as_ref()[..8].iter().map(|b| format!("{b:02x}")).collect()
}

// Per process, as a part file's name holds the process's ID.
static PARTS: AtomicU64 = AtomicU64::new(0);

// Written to a new file beside it and renamed over, so a reader never sees half a file. A
// new file, never an existing one, so a link planted in a shared temp dir is never written
// through; the rename replaces the link, not its target.
fn write(dir: &Path, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let partial = path.with_extension(format!("{}.{}.part", std::process::id(), PARTS.fetch_add(1, Ordering::Relaxed)));
    let written = OpenOptions::new().write(true).create_new(true).open(&partial).and_then(|mut file| file.write_all(bytes)).and_then(|_| fs::rename(&partial, path));
    if written.is_err() {
        let _ = fs::remove_file(&partial);
    }
    written
}

fn extension(mime: &str) -> Option<&'static str> {
    TYPES.iter().find(|(t, _)| mime.eq_ignore_ascii_case(t)).map(|(_, ext)| *ext)
}

#[cfg(test)]
mod tests;
