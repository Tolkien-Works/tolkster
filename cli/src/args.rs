// Argument parsing by hand, strict: a malformed option is a usage error, exit 2.

use std::collections::HashMap;

use crate::error::Error;

#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    String,
    Boolean,
    // A string option that may be repeated; every value is kept, in order.
    List,
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub strings: HashMap<String, String>,
    pub flags: HashMap<String, bool>,
    pub lists: HashMap<String, Vec<String>>,
    pub positionals: Vec<String>,
}

impl Parsed {
    pub fn string(&self, name: &str) -> Option<&str> {
        self.strings.get(name).map(String::as_str)
    }

    pub fn flag(&self, name: &str) -> bool {
        self.flags.get(name).copied().unwrap_or(false)
    }

    pub fn list(&self, name: &str) -> &[String] {
        self.lists.get(name).map(Vec::as_slice).unwrap_or_default()
    }

    // A blank value counts as not given.
    pub fn non_empty(&self, name: &str) -> Option<&str> {
        self.string(name).filter(|v| !js_trim(v).is_empty())
    }

    pub fn first(&self) -> Option<&str> {
        self.positionals.first().map(String::as_str).filter(|s| !s.is_empty())
    }
}

// Options are long only; no command defines a short one. A repeated option keeps its
// last value, unless it is a list, and `--` ends the options.
pub fn parse(argv: &[String], options: &[(&str, Kind)], allow_positionals: bool) -> Result<Parsed, Error> {
    let kind_of = |name: &str| options.iter().find(|(n, _)| *n == name).map(|(_, k)| *k);
    let unknown = |option: &str| {
        let hint = if allow_positionals { format!("; for an argument that starts with '-', put it after --, as in -- {option}") } else { String::new() };
        Error::Usage(format!("unknown option {option}{hint}"))
    };

    let mut parsed = Parsed::default();
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        i += 1;
        if arg == "--" {
            positional(&mut parsed, &argv[i..], allow_positionals)?;
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (long, None),
            };
            match kind_of(name) {
                None => return Err(unknown(&format!("--{name}"))),
                Some(Kind::Boolean) => {
                    if inline.is_some() {
                        return Err(Error::Usage(format!("--{name} takes no value")));
                    }
                    parsed.flags.insert(name.to_string(), true);
                }
                Some(kind @ (Kind::String | Kind::List)) => {
                    let value = match inline {
                        Some(value) => value,
                        None => {
                            let Some(next) = argv.get(i) else {
                                return Err(Error::Usage(format!("--{name} needs a value")));
                            };
                            i += 1;
                            if next.len() > 1 && next.starts_with('-') {
                                return Err(Error::Usage(format!("--{name} needs a value; for one that starts with '-', write --{name}=<value>")));
                            }
                            next.clone()
                        }
                    };
                    if kind == Kind::List {
                        parsed.lists.entry(name.to_string()).or_default().push(value);
                    } else {
                        parsed.strings.insert(name.to_string(), value);
                    }
                }
            }
        } else if arg.len() > 1 && arg.starts_with('-') {
            let first = arg[1..].chars().next().unwrap_or_default();
            return Err(unknown(&format!("-{first}")));
        } else {
            positional(&mut parsed, std::slice::from_ref(arg), allow_positionals)?;
        }
    }
    Ok(parsed)
}

fn positional(parsed: &mut Parsed, args: &[String], allowed: bool) -> Result<(), Error> {
    if let (false, Some(first)) = (allowed, args.first()) {
        return Err(Error::Usage(format!("unexpected argument {first}: this command takes none")));
    }
    parsed.positionals.extend_from_slice(args);
    Ok(())
}

// The one of `all` whose `name` is `text`.
pub fn named<T: Copy>(all: &[T], name: fn(T) -> &'static str, text: &str) -> Option<T> {
    all.iter().copied().find(|one| name(*one) == text)
}

// A blank body file is refused: an empty file is more likely a mistake than a wish to
// clear the text.
pub fn file_body(source: &str, what: &str, read: impl FnOnce(&str) -> Result<String, Error>) -> Result<String, Error> {
    if js_trim(source).is_empty() {
        return Err(Error::Usage(format!("the {what}'s file needs a path, or - for stdin")));
    }
    let body = read(source)?;
    if js_trim(&body).is_empty() {
        return Err(Error::Usage(format!("the {what} is empty")));
    }
    Ok(body)
}

pub fn body(parsed: &Parsed, what: &str, read: impl FnOnce(&str) -> Result<String, Error>) -> Result<String, Error> {
    match (parsed.string("body"), parsed.string("body-file")) {
        (Some(_), Some(_)) => Err(Error::Usage("pass --body or --body-file, not both".into())),
        (Some(body), None) if js_trim(body).is_empty() => Err(Error::Usage(format!("the {what} is empty"))),
        (Some(body), None) => Ok(body.into()),
        (None, Some(source)) => file_body(source, what, read),
        (None, None) => Err(Error::Usage(format!("a {what} needs --body or --body-file"))),
    }
}

// JavaScript's String.prototype.trim: its whitespace set differs from Rust's.
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(|c: char| {
        matches!(c, '\t' | '\n' | '\u{B}' | '\u{C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}')
    })
}

#[cfg(test)]
mod tests;
