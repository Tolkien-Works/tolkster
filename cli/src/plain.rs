// The plain shapes of the values that reach a terminal line, a command, a claim's name, a
// path or a GraphQL variable, whatever Linear, GitHub or the relay sent them as. Each is a
// type only this module builds, and only from a value of its shape: holding one is the check,
// so the functions that type, run or name with one take it, never a `&str`.

use std::fmt;
use std::ops::Deref;

use serde::Serialize;

use crate::error::Error;

macro_rules! plain {
    ($(#[$doc:meta])* $name:ident, $shape:ident, $refused:literal) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: &str) -> Option<Self> {
                $shape(value).then(|| Self(value.to_string()))
            }

            pub fn check(value: &str) -> Result<Self, Error> {
                Self::parse(value).ok_or_else(|| Error::Usage(format!(concat!($refused, ": {}"), value)))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Deref for $name {
            type Target = str;
            fn deref(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<$name> for String {
            fn eq(&self, other: &$name) -> bool {
                *self == other.0
            }
        }

        impl PartialEq<$name> for str {
            fn eq(&self, other: &$name) -> bool {
                self == other.0
            }
        }

        impl PartialEq<String> for $name {
            fn eq(&self, other: &String) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<$name> for &str {
            fn eq(&self, other: &$name) -> bool {
                *self == other.0
            }
        }

        impl std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        // Tests write their values as literals; outside them a value is only parsed.
        #[cfg(test)]
        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::check(value).unwrap()
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> String {
                value.0
            }
        }
    };
}

plain!(
    // A Linear, relay or GitHub node ID.
    Id, plain_id,
    "the ID has characters a command can't take"
);
plain!(
    // An issue's identifier, `TW-192`.
    IssueKey, plain_identifier,
    "the identifier has characters a brief can't take"
);
plain!(
    // A branch's name, as git takes one as a name, not an option.
    BranchName, plain_branch,
    "the branch name has characters a brief can't take"
);
plain!(
    // A path relative to a repo's root, as git prints a simple one.
    RepoPath, plain_path,
    "the path isn't a plain one"
);
plain!(
    // A commit's full SHA.
    Sha, plain_sha,
    "the commit isn't a full SHA"
);
plain!(
    // A commit, its SHA full or abbreviated.
    Commit, plain_commit,
    "the commit isn't plain hex"
);
plain!(
    // An ISO time, as Linear writes one.
    Time, plain_time,
    "the time isn't a plain ISO time"
);
plain!(
    // A repo's name, one plain part of a path a brief and a command line can take.
    RepoName, plain_repo,
    "the repo name has characters a brief can't take"
);
plain!(
    // A command's ID from the relay, as a claim's name keeps it (src/worker/machine.ts,
    // COMMAND_ID).
    CommandId, plain_command_id,
    "the command's ID isn't plain"
);
plain!(
    // An Orca terminal's handle.
    Terminal, plain_terminal,
    "the terminal's handle isn't plain"
);
plain!(
    // A line typed into an agent's terminal, or a command a terminal starts with.
    Line, plain_line,
    "a line for an agent's terminal has characters it can't take (a quote, `, $, !, a trailing \\ or a control character): check \"ladder\" in config.json and the home folder's path"
);

impl Id {
    // Two numbers in hex, as a token is made: always plain.
    pub fn hex(high: u64, low: u64) -> Id {
        Id(format!("{high:016x}{low:016x}"))
    }
}

impl IssueKey {
    // Linear matches a key without case; a claim's name and a comparison don't.
    pub fn upper(&self) -> IssueKey {
        IssueKey(self.0.to_ascii_uppercase())
    }

    // What the triage machine's or the fleet's commands carry for their identifier
    // (machine::TRIAGE, machine::FLEET), which no issue's is, as it has no number: it names their lane and their
    // log lines, never an issue.
    pub fn machine(name: &'static str) -> IssueKey {
        IssueKey(name.to_string())
    }
}

impl From<Sha> for Commit {
    fn from(sha: Sha) -> Commit {
        Commit(sha.0)
    }
}

// The command a terminal starts with: a plain line, then quoted lines and flags from code, so
// PowerShell and a POSIX shell read each quoted part as one string that ends where it should.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandLine(String);

impl CommandLine {
    pub fn new(base: &Line) -> CommandLine {
        CommandLine(base.0.clone())
    }

    pub fn quoted(mut self, line: &Line) -> CommandLine {
        self.0.push_str(&format!(" \"{}\"", line.0));
        self
    }

    // A flag from code, never from input, given only when `on`.
    pub fn flag(mut self, on: bool, flag: &'static str) -> CommandLine {
        if on {
            self.0.push(' ');
            self.0.push_str(flag);
        }
        self
    }

    // The shell ends with the command's exit code, for the wait on its terminal to read: in
    // PowerShell that is `$LASTEXITCODE`, and in a POSIX shell, where it is unset, `exit`
    // alone gives the last command's.
    pub fn exiting(mut self) -> CommandLine {
        self.0.push_str("; exit $LASTEXITCODE");
        self
    }

    // A command from code for a host's own shell (onboard.rs): `template`, fixed
    // text that may use the shell's syntax, with each `{}` filled in turn by a fill no shell
    // reads as syntax, quoted or not, nor as an option.
    pub fn template(template: &'static str, fills: &[&str]) -> Result<CommandLine, Error> {
        if let Some(fill) = fills.iter().find(|fill| !plain_fill(fill)) {
            return Err(Error::Usage(format!("a line for a host's shell takes only letters, digits and . _ : / \\ -, not a leading dash: {fill}")));
        }
        let mut parts = template.split("{}");
        let mut text = parts.next().unwrap_or_default().to_string();
        for (part, fill) in parts.zip(fills.iter().copied().chain(std::iter::repeat(""))) {
            text.push_str(fill);
            text.push_str(part);
        }
        Ok(CommandLine(text))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CommandLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// Never starts with a dash: an option.
fn plain_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && !id.starts_with('-') && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn plain_command_id(id: &str) -> bool {
    (1..=200).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || "_.:@-".contains(c))
}

fn plain_sha(sha: &str) -> bool {
    sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit())
}

fn plain_commit(commit: &str) -> bool {
    (7..=40).contains(&commit.len()) && commit.chars().all(|c| c.is_ascii_hexdigit())
}

// Never starts with a dash (an option) or a slash.
fn plain_branch(branch: &str) -> bool {
    !branch.is_empty() && !branch.starts_with(['-', '/']) && !branch.contains("..") && branch.chars().all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c))
}

fn plain_path(path: &str) -> bool {
    path.len() <= 200 && plain_branch(path)
}

// The one parse of `KEY-123`, loose on purpose: a key of letters and digits, then a number a `u64`
// takes (so `TW-+5` and `1W-1` pass), split at the last dash. What a plain identifier is stays
// `plain_identifier`'s.
pub fn split_identifier(identifier: &str) -> Option<(&str, u64)> {
    let (key, number) = identifier.rsplit_once('-')?;
    let number = number.parse().ok()?;
    (!key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric())).then_some((key, number))
}

fn plain_identifier(identifier: &str) -> bool {
    let Some((key, number)) = identifier.split_once('-') else { return false };
    key.len() <= 16 && key.starts_with(|c: char| c.is_ascii_alphabetic()) && key.chars().all(|c| c.is_ascii_alphanumeric()) && (1..=9).contains(&number.len()) && number.chars().all(|c| c.is_ascii_digit())
}

// A worktree's ID as Tolkster names it, split into its host, repo ID and path: Orca's own
// `<repoId>::<path>`, behind `<host>@` on a remote host, the Orca environment it is paired
// as, since a remote host's ID means nothing to this machine's Orca. Without a host it is
// this machine's. Repo IDs are minted per host, and a path may be POSIX or Windows. The
// Durable Object takes the same shape (src/worker/state.ts, WORKTREE), so a path's length
// counts UTF-16 units, as JavaScript does.
pub fn split_worktree(id: &str) -> Option<(Option<&str>, &str, &str)> {
    let (head, path) = id.split_once("::")?;
    let (host, repo) = match head.split_once('@') {
        Some((host, repo)) if plain_host(host) => (Some(host), repo),
        Some(_) => return None,
        None => (None, head),
    };
    let repo_shape = (1..=64).contains(&repo.len()) && repo.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    (repo_shape && (1..=400).contains(&path.encode_utf16().count()) && !path.chars().any(char::is_control)).then_some((host, repo, path))
}

// The ID the worktree's own host's Orca knows it by: `id` without its host.
pub fn orca_worktree(id: &str) -> Option<String> {
    split_worktree(id).map(|(_, repo, path)| format!("{repo}::{path}"))
}

// An Orca environment's name. Never starts with a dash: an option.
pub fn plain_host(host: &str) -> bool {
    (1..=64).contains(&host.len()) && !host.starts_with('-') && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

// An enrolment code (src/worker/hosts.ts, CODE), which `host add` types into a host's shell.
pub fn plain_code(code: &str) -> bool {
    code.len() == 64 && code.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

// An absolute folder on a POSIX or Windows host, of characters a shell line takes as they are.
pub fn plain_destination(path: &str) -> bool {
    let absolute = path.starts_with('/') || path.get(1..3).is_some_and(|s| s == ":\\" || s == ":/") && path.starts_with(|c: char| c.is_ascii_alphabetic());
    absolute && path.len() <= 200 && !path.contains("..") && path.chars().all(|c| c.is_ascii_alphanumeric() || "/\\:._-".contains(c))
}

// An Orca terminal's handle, as the Durable Object takes it (src/worker/machine/validate.ts,
// TERMINAL), split into its host and the handle that host's Orca made: behind `<host>@` on a
// remote host, as a worktree's ID is (`split_worktree`), since a handle means something only
// to the Orca that made it, and two hosts' may be the same.
pub fn split_terminal(handle: &str) -> Option<(Option<&str>, &str)> {
    let (host, own) = match handle.split_once('@') {
        Some((host, own)) if plain_host(host) => (Some(host), own),
        Some(_) => return None,
        None => (None, handle),
    };
    ((1..=128).contains(&own.len()) && !own.starts_with('-') && own.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-'))).then_some((host, own))
}

fn plain_terminal(handle: &str) -> bool {
    split_terminal(handle).is_some()
}

// Never starts with a dash: an option.
fn plain_time(time: &str) -> bool {
    !time.is_empty() && time.len() <= 40 && !time.starts_with('-') && time.chars().all(|c| c.is_ascii_digit() || matches!(c, '-' | ':' | '.' | 'T' | 'Z'))
}

// `owner/name`, each part only what GitHub allows in one, since both go into a URL path
// (and never a shell's): an owner is alphanumerics and hyphens, not starting with one; a
// name adds `_` and `.`, and is never `.` or `..`.
pub fn split_repo(repo: &str) -> Option<(&str, &str)> {
    let (owner, name) = repo.split_once('/')?;
    let owner_ok = !owner.is_empty() && !owner.starts_with('-') && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let name_ok = !name.is_empty() && name != "." && name != ".." && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    (owner_ok && name_ok).then_some((owner, name))
}

fn plain_repo(repo: &str) -> bool {
    let path_char = |c: char| !c.is_control() && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '“' | '”' | '„' | '<' | '>' | '|' | '`' | '$' | '!');
    !repo.trim().is_empty() && repo.chars().all(path_char) && !repo.contains("..")
}

// A value a host's shell line can't carry as it is, as the hex a fill takes: the `tolkster` on
// the host decodes it, so no shell reads the value.
pub fn hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(hex: &str) -> Option<String> {
    let bytes: Option<Vec<u8>> = (0..hex.len()).step_by(2).map(|i| hex.get(i..i + 2).and_then(|pair| u8::from_str_radix(pair, 16).ok())).collect();
    bytes.and_then(|bytes| String::from_utf8(bytes).ok())
}

fn plain_fill(fill: &str) -> bool {
    !fill.is_empty() && !fill.starts_with('-') && fill.chars().all(|c| c.is_ascii_alphanumeric() || "._:/\\-".contains(c))
}

// Passes through a double-quoted argument as it is in PowerShell, bash or zsh, whichever the
// host's terminal runs: PowerShell ends the string at “ ” „ as at ", an interactive bash or zsh
// expands history at !, and a POSIX shell's closing quote after a trailing \ is escaped.
fn plain_line(line: &str) -> bool {
    !line.ends_with('\\')&& !line.chars().any(|c| c.is_control() || matches!(c, '"' | '“' | '”' | '„' | '`' | '$' | '!'))
}

// `file` as a path inside the worktree at `root`, in the case the tool gave it, when it is
// one and plain (RepoPath). Paths compare with `/` for `\` and without case, as
// Windows does.
pub fn relative(file: &str, root: &str) -> Option<String> {
    let (file, root) = (slashed(file), normal(root));
    let root = format!("{}/", root.trim_end_matches('/'));
    // ASCII case only, so the lowered path's bytes line up with the file's.
    if root == "/" || !file.to_ascii_lowercase().starts_with(&root) {
        return None;
    }
    RepoPath::parse(&file[root.len()..]).map(String::from)
}

// Whether `cwd` is the worktree at `root` or inside it.
pub fn inside(cwd: &str, root: &str) -> bool {
    let (cwd, root) = (normal(cwd), normal(root));
    let root = root.trim_end_matches('/');
    !root.is_empty() && (cwd.trim_end_matches('/') == root || cwd.starts_with(&format!("{root}/")))
}

fn normal(path: &str) -> String {
    slashed(path).to_ascii_lowercase()
}

// With `/` for `\`, and a Git Bash path, `/d/source`, read as `d:/source`.
fn slashed(path: &str) -> String {
    let path = path.replace('\\', "/");
    let mut chars = path.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some('/'), Some(drive), Some('/')) if drive.is_ascii_alphabetic() => format!("{drive}:/{}", &path[3..]),
        _ => path,
    }
}

#[cfg(test)]
mod tests;
