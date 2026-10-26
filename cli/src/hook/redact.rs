// The trace's redactor (hook::narrate): what a worker wrote and what its calls printed goes to
// Linear only through `Scrub::text`, which is the trust boundary of a traced session
// (docs/voice.md). The worktree's root becomes a relative path, the home directory `~`, any
// other absolute path its last part, this machine's names `<host>`, and every secret it is
// given (credentials.json's, the cached token's, the environment's) and every known secret
// shape `<secret>`, before a cap.

use serde_json::Value;

pub struct Scrub {
    root: String,
    home: String,
    hosts: Vec<String>,
    // Longest first, so a secret that holds another goes whole.
    secrets: Vec<String>,
}

// The shortest credential value scrubbed by its value: anything shorter, a flag or a count, is
// left to the shapes.
const SECRET_LEAST: usize = 8;

impl Scrub {
    // `credentials` as credentials.json holds them: every string in it, at any depth, is a
    // secret, and a `:`-joined one's longer parts too, as a host credential is an ID and a key.
    pub fn new(root: &str, home: &str, hosts: &[String], credentials: &Value) -> Scrub {
        let mut secrets = Vec::new();
        strings(credentials, &mut secrets);
        let parts: Vec<String> = secrets.iter().flat_map(|s| s.split(':')).filter(|p| p.len() >= 16).map(String::from).collect();
        secrets.extend(parts);
        secrets.retain(|s| s.len() >= SECRET_LEAST);
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        secrets.dedup();
        let hosts = hosts.iter().map(|h| h.trim().to_string()).filter(|h| h.len() >= 3).collect();
        Scrub { root: root.trim_end_matches(['/', '\\']).to_string(), home: home.trim_end_matches(['/', '\\']).to_string(), hosts, secrets }
    }

    // `text` scrubbed, then cut to `most` characters.
    pub fn text(&self, text: &str, most: usize) -> String {
        let mut text = text.to_string();
        for secret in &self.secrets {
            text = text.replace(secret.as_str(), "<secret>");
        }
        text = pem(&text);
        for (path, with) in [(&self.root, "."), (&self.home, "~")] {
            for form in forms(path) {
                text = replace_path(&text, &form, with);
            }
        }
        for host in &self.hosts {
            text = replace_word(&text, host, "<host>");
        }
        cap(&shapes(&absolute(&text)), most)
    }
}

fn strings(value: &Value, into: &mut Vec<String>) {
    match value {
        Value::String(s) => into.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|v| strings(v, into)),
        Value::Object(map) => map.values().for_each(|v| strings(v, into)),
        _ => {}
    }
}

// The first `most` characters of `text`, with `…` for what was cut.
pub fn cap(text: &str, most: usize) -> String {
    match text.char_indices().nth(most) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

// A path as a shell may print it: as given, with either slash, and on Windows Git Bash's
// `/d/source` for `D:\source`. Empty for a path too short to tell from text.
fn forms(path: &str) -> Vec<String> {
    if path.len() < 3 {
        return Vec::new();
    }
    let (forward, back) = (path.replace('\\', "/"), path.replace('/', "\\"));
    let mut forms = vec![forward.clone(), back];
    if let [drive, b':', b'/', ..] = forward.as_bytes() {
        forms.push(format!("/{}{}", (*drive as char).to_ascii_lowercase(), &forward[2..]));
    }
    forms.dedup();
    forms
}

// Each occurrence of `path`, ignoring ASCII case as Windows does, that ends at a separator or
// the path's end, replaced by `with`: `root/cli/x.rs` becomes `cli/x.rs` for `.`, and `~/x`
// for `~`.
fn replace_path(text: &str, path: &str, with: &str) -> String {
    let (lower, needle) = (text.to_ascii_lowercase(), path.to_ascii_lowercase());
    let (mut out, mut at) = (String::new(), 0);
    while let Some(found) = lower[at..].find(&needle).map(|i| at + i) {
        let end = found + needle.len();
        let next = text[end..].chars().next();
        if !matches!(next, None | Some('/' | '\\' | '"' | '\'' | ' ' | '\n' | '\t' | ')' | ',' | ';' | ':' | '`')) {
            out.push_str(&text[at..end]);
            at = end;
            continue;
        }
        out.push_str(&text[at..found]);
        match (with, next) {
            (".", Some('/' | '\\')) => at = end + 1,
            _ => {
                out.push_str(with);
                at = end;
            }
        }
    }
    out.push_str(&text[at..]);
    out
}

// Each whole-word occurrence of `word`, ignoring ASCII case.
fn replace_word(text: &str, word: &str, with: &str) -> String {
    let (lower, needle) = (text.to_ascii_lowercase(), word.to_ascii_lowercase());
    let joined = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    let (mut out, mut at) = (String::new(), 0);
    while let Some(found) = lower[at..].find(&needle).map(|i| at + i) {
        let end = found + needle.len();
        out.push_str(&text[at..found]);
        let whole = !joined(text[..found].chars().next_back()) && !joined(text[end..].chars().next());
        out.push_str(if whole { with } else { &text[found..end] });
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

// A PEM block, from its BEGIN line to its END line, or to the text's end when cut; an END
// whose BEGIN was cut off takes all before it.
fn pem(text: &str) -> String {
    let (mut out, mut rest) = (String::new(), text);
    loop {
        let (begin, end) = (rest.find("-----BEGIN "), rest.find("-----END "));
        let from = match (begin, end) {
            (Some(begin), end) if end.is_none_or(|end| begin < end) => {
                out.push_str(&rest[..begin]);
                begin
            }
            (_, Some(end)) => end,
            _ => break,
        };
        out.push_str("<secret>");
        let block = &rest[from..];
        rest = match block.find("-----END ") {
            Some(end) => block[end + 9..].find("-----").map_or("", |close| &block[end + 9 + close + 5..]),
            None => "",
        };
    }
    out.push_str(rest);
    out
}

// Characters that end a path in text.
fn ends_path(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`' | ')' | ']' | '>' | '<' | ',' | ';' | '|' | '*')
}

// Each absolute path left, replaced by `…/` and its last part: a drive's, `C:\x\y` or `C:/x`,
// a share's, `\\server\x`, and a Unix one of two parts or more, `/etc/x`, at a word's start,
// so a URL's `//host` or a relative `a/b` is not one.
fn absolute(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let before = i.checked_sub(1).map(|j| chars[j]);
        let starts = before.is_none_or(|c| ends_path(c) || matches!(c, '=' | '(' | '[' | '{'));
        let drive = starts && chars[i].is_ascii_alphabetic() && chars.get(i + 1) == Some(&':') && matches!(chars.get(i + 2), Some('\\' | '/'));
        let share = starts && chars[i] == '\\' && chars.get(i + 1) == Some(&'\\');
        let unix = starts && chars[i] == '/' && chars.get(i + 1).is_some_and(|c| !ends_path(*c) && *c != '/');
        if drive || share || unix {
            let end = (i..chars.len()).find(|&j| ends_path(chars[j])).unwrap_or(chars.len());
            let path: String = chars[i..end].iter().collect();
            let trimmed = path.trim_end_matches(['.', ':']);
            let parts: Vec<&str> = trimmed.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
            if !unix || parts.len() >= 2 {
                out.push('…');
                if let Some(last) = parts.last().filter(|_| parts.len() > 1) {
                    out.push('/');
                    out.push_str(last);
                }
                out.push_str(&path[trimmed.len()..]);
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

// Prefixes of known token shapes.
const PREFIXES: &[&str] = &["lin_api_", "lin_oauth_", "ghp_", "gho_", "ghs_", "ghu_", "ghr_", "github_pat_", "glpat-", "npm_", "sk-", "xoxb-", "xoxp-", "xoxa-", "xoxs-", "AKIA"];

// Names a secret follows after `=` or `:`, `token: <token>`, or as a flag after a space,
// `--token <token>`, each also as a name's last part, `CLOUDFLARE_API_TOKEN=` or `x-api-key:`;
// and the schemes it follows after a space, `Bearer <token>`.
const KEYS: &[&str] = &["token", "secret", "password", "passwd", "key", "apikey", "authorization", "credential", "credentials", "bearer", "basic"];
const SCHEMES: &[&str] = &["bearer", "basic"];

fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+' | '/' | '=')
}

// Whether a name is a secret's: a key word, or one as its last part after `_`, `-` or a
// capital, `accessToken`.
pub fn key(word: &str) -> bool {
    let word = word.trim_start_matches('-');
    let camel = word.char_indices().rev().find(|(at, c)| *at > 0 && c.is_ascii_uppercase()).map_or("", |(at, _)| &word[at..]);
    let word = word.to_ascii_lowercase();
    let last = word.rsplit(['_', '-']).next().unwrap_or_default();
    let camel = camel.to_ascii_lowercase();
    KEYS.contains(&word.as_str()) || (last != word && KEYS.contains(&last)) || KEYS.contains(&camel.as_str())
}

// Each run of token characters that has a known shape, or follows a key, replaced by
// `<secret>`.
fn shapes(text: &str) -> String {
    let mut out = String::new();
    let (mut keyed, mut named) = (false, false);
    let mut rest = text;
    while let Some(start) = rest.find(token_char) {
        out.push_str(&rest[..start]);
        let from = &rest[start..];
        let whole = &from[..from.find(|c: char| !token_char(c)).unwrap_or(from.len())];
        // A sentence's full stop or a path's last slash ends the run, not the token.
        let run = whole.trim_end_matches(['.', '/']);
        let after = &from[whole.len()..];
        let gap = format!("{}{}", &whole[run.len()..], &after[..after.find(token_char).unwrap_or(after.len())]);
        match run.split_once('=') {
            _ if run.is_empty() => {}
            _ if keyed => out.push_str("<secret>"),
            // `TOKEN =value`.
            Some(("", value)) if named && !value.is_empty() => out.push_str("=<secret>"),
            Some((name, value)) if !name.is_empty() && !value.is_empty() && (key(name) || secret_shaped(value)) => {
                out.push_str(name);
                out.push_str("=<secret>");
            }
            _ if secret_shaped(run) => out.push_str("<secret>"),
            _ => out.push_str(run),
        }
        let scheme = SCHEMES.iter().any(|s| run.eq_ignore_ascii_case(s));
        // `NAME = 'value'`: a lone `=` after a key passes the key on.
        keyed = !run.is_empty() && match gap.trim_matches([' ', '"', '\'']) {
            _ if run == "=" => named,
            // `password="v"` and `TOKEN= v`: the run takes the `=`, and the value is the next.
            _ if run.ends_with('=') => key(run.trim_end_matches('=')),
            "=" | ":" => key(run),
            "" => gap.starts_with(' ') && (scheme || (run.starts_with('-') && key(run))),
            _ => false,
        };
        named = key(run) && gap.trim().is_empty();
        out.push_str(&whole[run.len()..]);
        rest = after;
    }
    out.push_str(rest);
    out
}

fn secret_shaped(run: &str) -> bool {
    if PREFIXES.iter().any(|p| run.strip_prefix(p).is_some_and(|rest| rest.len() >= 12 && !rest.contains('/'))) {
        return true;
    }
    if run.starts_with("eyJ") && run.matches('.').count() == 2 && run.len() >= 40 {
        return true;
    }
    // A long part of letters and digits mixed, as a key's is: a word, a path's part or a
    // commit's SHA is not, nor a UUID's parts.
    run.split(['-', '_', '.', '/']).any(|part| {
        let hex = part.chars().all(|c| c.is_ascii_hexdigit());
        let letters = part.chars().any(|c| c.is_ascii_alphabetic());
        let digits = part.chars().any(|c| c.is_ascii_digit());
        part.len() >= 24 && letters && digits && !(hex && part.len() == 40)
    })
}

#[cfg(test)]
mod tests;
