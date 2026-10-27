// Reading a shell command: its programs and what they do, from their names and write markers
// alone, never echoed.

use super::{Call, Tool};
use crate::timing::CheckKind;

// Programs that only set up the ones that matter, and say nothing.
const SETUP: &[&str] = &["cd", "set-location", "echo", "export", "set", "env", "true", "time"];

// The kinds a command can be, most telling first: `git add && git commit` is a commit, and
// `cargo test | grep` the tests.
const TELLING: &[&str] = &["testing", "building", "pushing", "committing", "rebasing", "editing", "history", "working", "command", "reading", "searching"];

// A shell command's kind, from its programs' names and its write markers alone: it is never
// echoed. None when any part of it runs `tolkster` or `orca`, or it only sets up or sleeps.
pub(super) fn command_kind(command: &str) -> Option<&'static str> {
    let segments = segments(command);
    if segments.iter().any(|words| matches!(program(words).as_str(), "tolkster" | "orca")) {
        return None;
    }
    let mut kinds: Vec<&str> = segments.iter().filter_map(|words| segment_kind(words)).collect();
    if script_writes(command) {
        kinds.push("editing");
    }
    TELLING.iter().copied().find(|kind| kinds.contains(kind))
}

// A command's parts, each the words of one program's run, lowercased and unquoted. A
// heredoc's or here-string's body is text for a program, not a command: it is left out.
pub(super) fn segments(command: &str) -> Vec<Vec<String>> {
    mask_quoted(&code(command))
        .split(['\n', ';', '|', '&', '(', ')', '{', '}'])
        .map(|segment| segment.split_whitespace().map(|word| word.trim_matches(['"', '\'']).to_ascii_lowercase()).filter(|word| !word.is_empty()).collect::<Vec<_>>())
        .filter(|words| !words.is_empty())
        .collect()
}

// The command with each body taken out. A heredoc's runs from the line after
// its `<<` to its delimiter's line, which goes too; a PowerShell here-string's, opened by a
// line ending in `@'` or `@"`, to the line that starts with its closing `'@` or `"@`, which
// stays, as a pipe may follow it. A body no line ends belongs to no one, and stays code.
fn code(command: &str) -> String {
    let (mut code, mut rest) = (String::new(), command);
    while !rest.is_empty() {
        let (line, after) = rest.split_once('\n').unwrap_or((rest, ""));
        code.push_str(line);
        code.push('\n');
        rest = after;
        let opener = line.trim_end();
        let here_string = [("@'", "'@"), ("@\"", "\"@")].into_iter().find(|(open, _)| opener.ends_with(open)).map(|(_, close)| close);
        let taken = match (here_string, line.split_once("<<")) {
            (Some(close), _) => ends_at(rest, |l| l.starts_with(close)).map(|at| (&rest[..at], &rest[at..])),
            (None, Some((_, marker))) => heredoc(marker, rest).map(|body| (body, rest[body.len()..].split_once('\n').map_or("", |(_, left)| left))),
            (None, None) => None,
        };
        if let Some((_, left)) = taken {
            rest = left;
        }
    }
    code
}

// `>` inside quotes, as a word of `segments` has it, so `edits` doesn't read it as a redirect.
const QUOTED_GT: char = '\u{1}';

// Whether the text before a quote ends a shell's `-c` or `-Command`: what follows is code, not
// an argument.
fn script_follows(before: &str) -> bool {
    let before = before.to_ascii_lowercase();
    let words: Vec<&str> = before.rsplit(['\n', ';', '|', '&']).next().unwrap_or_default().split_whitespace().collect();
    let shell = words.first().is_some_and(|w| matches!(w.rsplit(['/', '\\']).next().unwrap_or_default().trim_end_matches(".exe"), "bash" | "sh" | "zsh" | "pwsh" | "powershell" | "cmd"));
    shell && words.last().is_some_and(|w| *w == "/c" || (w.starts_with('-') && (w.ends_with('c') || w.ends_with("command"))))
}

// A here-string's `@'` ends its line and its `'@` starts one: neither is a quote.
fn mask_quoted(code: &str) -> String {
    let chars: Vec<char> = code.chars().collect();
    let (mut out, mut quote, mut mask) = (String::new(), None, false);
    for (i, &c) in chars.iter().enumerate() {
        let line_end = |at: usize| chars.get(at).is_none_or(|&next| next == '\n' || next == '\r');
        let here_string = matches!(c, '\'' | '"') && ((i > 0 && chars[i - 1] == '@' && line_end(i + 1)) || (chars.get(i + 1) == Some(&'@') && (i == 0 || chars[i - 1] == '\n')));
        match (c, quote) {
            ('\n', _) => quote = None,
            ('\'' | '"', None) if !here_string => (quote, mask) = (Some(c), !script_follows(&out)),
            (_, Some(open)) if c == open => quote = None,
            _ => {}
        }
        out.push(if c == '>' && quote.is_some() && mask { QUOTED_GT } else { c });
    }
    out
}

// Where the first line of `text` that `ends` begins.
fn ends_at(text: &str, ends: impl Fn(&str) -> bool) -> Option<usize> {
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        if ends(line) {
            return Some(at);
        }
        at += line.len();
    }
    None
}

// A program's name, without its directory or `.exe`.
pub(super) fn program(words: &[String]) -> String {
    let name = words[0].rsplit(['/', '\\']).next().unwrap_or_default();
    name.strip_suffix(".exe").unwrap_or(name).to_string()
}

// Programs that run a script of their own, whose text a program name doesn't tell.
fn runner(program: &str) -> bool {
    program.starts_with("python") || matches!(program, "py" | "node" | "deno" | "bun" | "ruby" | "pwsh" | "powershell" | "bash" | "sh")
}

// What a script writes a file with: Python's and Node's own calls.
const WRITES: &[&str] = &["write_text(", "write_bytes(", ".write(", "writefile(", "writefilesync("];

// Whether the command runs a script (`runner`) whose text, in the command or a body of it
// (`python - <<'EOF'`, `-c`), writes a file.
pub(super) fn script_writes(command: &str) -> bool {
    if !segments(command).iter().any(|words| runner(&program(words))) {
        return false;
    }
    // A write to the console is no edit.
    let text = command.to_ascii_lowercase().replace("stdout.write(", "").replace("stderr.write(", "");
    WRITES.iter().any(|marker| text.contains(marker)) || text.match_indices("open(").any(|(at, _)| opens_for_writing(&text[..at], &text[at + 5..]))
}

// Whether an `open(` call, `after` its parenthesis, takes a mode that starts with `w`: its second
// argument, `mode=`, or a method's only one (`Path.open('w')`). `before` is the text ahead of it, which a longer name (`popen`) ends in.
fn opens_for_writing(before: &str, after: &str) -> bool {
    let method = before.ends_with('.');
    if before.ends_with(|c: char| c.is_alphanumeric() || c == '_') && !before.ends_with("fd") {
        return false;
    }
    let call = after.split(')').next().unwrap_or_default();
    let keyword = call.split_once("mode").and_then(|(_, rest)| rest.trim_start().strip_prefix('='));
    let mode = keyword.or_else(|| call.split_once(',').map(|(_, mode)| mode));
    if let Some(mode) = mode {
        return mode.trim_start().strip_prefix(['\'', '"']).is_some_and(|mode| mode.starts_with('w'));
    }
    // A method's only argument is a mode only when it is one: `'w'`, `"wb"`, `'w+'`.
    let only = call.trim();
    method && only.starts_with(['\'', '"']) && only.len() > 2 && only.trim_matches(['\'', '"']).starts_with('w') && only.trim_matches(['\'', '"']).chars().all(|c| "wxbt+".contains(c))
}

// Whether one run edits a file by its own words: `sed -i`, `perl -i`, `tee` or a redirect
// into a path outside the temp directory, or a PowerShell cmdlet that writes one.
fn edits(words: &[String]) -> bool {
    let program = program(words);
    // A short-option cluster that holds `i`, its backup suffix after it: `-i.bak`, `-ni`, `-Ei`;
    // what comes before it is only the letters a program's options are.
    let cluster = |letters: &dyn Fn(char) -> bool| words.iter().skip(1).any(|w| w.strip_prefix('-').and_then(|f| f.split_once('i')).is_some_and(|(before, after)| before.chars().all(letters) && (after == "e" || after.chars().next().is_none_or(|c| !c.is_alphabetic()))));
    let target = |w: &str| !w.is_empty() && !scratch(w);
    let cmdlet = |w: &str| matches!(w, "set-content" | "out-file" | "add-content");
    // A cmdlet writes where it is told: a path in the temp directory among its words is no edit.
    let writes = || !words.iter().skip(1).any(|w| scratch(w));
    match program.as_str() {
        "sed" if cluster(&|c| c.is_ascii_alphabetic()) || words.iter().any(|w| w.starts_with("--in-place")) => return true,
        "perl" if cluster(&|c| "0123456789plnaw".contains(c)) => return true,
        "tee" if words.iter().skip(1).any(|w| !w.starts_with('-') && target(w)) => return true,
        "pwsh" | "powershell" if words.iter().any(|w| cmdlet(w)) && writes() => return true,
        p if cmdlet(p) && writes() => return true,
        _ => {}
    }
    // `2>` and `>>`, but not `->` or a `>` in the middle of a word.
    words.iter().enumerate().any(|(i, w)| {
        let Some(after) = w.trim_start_matches(|c: char| c.is_ascii_digit()).strip_prefix('>') else { return false };
        let path = after.trim_start_matches('>');
        target(if path.is_empty() { words.get(i + 1).map_or("", String::as_str) } else { path })
    })
}

// Whether `path` (lowercased) is in the temp directory or is the null device.
fn scratch(path: &str) -> bool {
    let path = path.replace('\\', "/");
    matches!(path.as_str(), "nul" | "$null") || ["/dev/", "/tmp/", "$env:temp", "$env:tmp", "$tmp", "$temp", "%temp%", "%tmp%"].iter().any(|p| path.starts_with(p)) || path.contains("/temp/")
}

pub(super) fn segment_kind(words: &[String]) -> Option<&'static str> {
    let kind = program_kind(words);
    if matches!(kind, Some("testing" | "building" | "pushing" | "committing" | "rebasing")) || !edits(words) {
        return kind;
    }
    Some("editing")
}

fn program_kind(words: &[String]) -> Option<&'static str> {
    let arg = |n: usize| words.get(n).map(String::as_str).unwrap_or_default();
    let has = |word: &str| words.iter().any(|w| w == word);
    let program = program(words);
    if SETUP.contains(&program.as_str()) || program.starts_with('$') {
        return None;
    }
    Some(match program.as_str() {
        "sleep" | "start-sleep" => return None,
        "cargo" if arg(1) == "test" => "testing",
        "cargo" if matches!(arg(1), "check" | "build" | "clippy") => "building",
        "npm" | "pnpm" | "yarn" if has("test") => "testing",
        "npm" | "pnpm" | "yarn" if has("typecheck") || has("build") || has("lint") => "building",
        "npx" if has("wrangler") || has("tsc") => "building",
        "node" if has("--test") => "testing",
        "tsc" => "building",
        "git" => match words.iter().skip(1).find(|w| !w.starts_with('-') && !w.contains(['/', '\\', ':', '='])).map(String::as_str) {
            Some("commit") => "committing",
            Some("push") => "pushing",
            Some("fetch" | "rebase" | "pull" | "merge") => "rebasing",
            _ => "history",
        },
        "grep" | "rg" | "find" | "ls" | "select-string" | "get-childitem" => "searching",
        "cat" | "sed" | "head" | "tail" | "less" | "get-content" => "reading",
        p if runner(p) => "working",
        _ => "command",
    })
}

// Whether a shell call of the worker's own runs `git commit`, for the `Committed` action the
// hook posts after it. A sub-agent's call, which the worker's transcript may not
// hold, never does.
pub fn commits(call: &Call) -> bool {
    shell_runs(call, &["committing"])
}

// The events a shell command's runs make that may end a plan step (UNTILS in linear/plan.rs),
// in order: `commit` for `git commit`, and `review` then `pr` for `tolkster pr create` in
// `session`, which posts the review's verdict, when there was a review, before it opens the PR.
pub(super) fn events(command: &str, session: &str) -> Vec<&'static str> {
    let mut events = Vec::new();
    for words in segments(command) {
        if program(&words) == "tolkster" {
            let named = |w: &str| w.eq_ignore_ascii_case(session);
            let in_session = words.windows(2).any(|w| w[0] == "--session" && named(&w[1])) || words.iter().any(|w| w.strip_prefix("--session=").is_some_and(named));
            if words.get(1).is_some_and(|w| w == "pr") && words.get(2).is_some_and(|w| w == "create") && in_session {
                events.extend(["review", "pr"]);
            }
        } else if segment_kind(&words) == Some("committing") {
            events.push("commit");
        }
    }
    events
}

fn shell_runs(call: &Call, kinds: &[&str]) -> bool {
    let Some(command) = call.command.as_deref().filter(|_| !call.sub_agent && call.tool == Tool::Shell) else { return false };
    runs(command, kinds)
}

pub(super) fn runs(command: &str, kinds: &[&str]) -> bool {
    segments(command).iter().any(|words| segment_kind(words).is_some_and(|kind| kinds.contains(&kind)))
}

// A call that posts an action as it starts and its outcome once it ends, and ticks while it
// runs: a key of voice.json's `actions`. None for any other: a sub-agent's, one that commits,
// whose `Committed` action stands for it, or one that runs `orca` or `tolkster`, as the
// worker's own reporting does.
pub fn pairing(call: &Call) -> Option<&'static str> {
    if call.sub_agent {
        return None;
    }
    pair_of(call.tool, call.command.as_deref())
}

// The kinds of shell command that pair.
const PAIRED: &[&str] = &["testing", "building", "pushing", "rebasing"];

pub(super) fn pair_of(tool: Tool, command: Option<&str>) -> Option<&'static str> {
    match tool {
        Tool::SubAgent => return Some("delegating"),
        Tool::Shell => {}
        _ => return None,
    }
    let segments = segments(command?);
    let kinds: Vec<&str> = segments.iter().filter_map(|words| segment_kind(words)).collect();
    if kinds.contains(&"committing") {
        return None;
    }
    if segments.iter().any(|words| matches!(program(words).as_str(), "tolkster" | "orca")) {
        return None;
    }
    TELLING.iter().copied().find(|kind| kinds.contains(kind)).filter(|kind| PAIRED.contains(kind))
}

// Whether a call's end says how its check went: a sub-agent's does, and a shell command's
// only when the paired program is the one program it runs, setup aside, and runs last, as
// the shell exits as its last program did: `cargo test | grep` exits as `grep` does, and
// `cargo test; cargo build` as the build.
pub fn known(tool: Tool, command: Option<&str>) -> bool {
    match tool {
        Tool::SubAgent => true,
        Tool::Shell => {
            // `2>&1` splits off a lone `1`.
            let segments: Vec<Vec<String>> = segments(command.unwrap_or_default()).into_iter().filter(|words| !words[0].chars().all(|c| c.is_ascii_digit())).collect();
            let run: Vec<&Vec<String>> = segments.iter().filter(|words| segment_kind(words).is_some()).collect();
            let kind = pair_of(tool, command);
            run.len() == 1 && segments.last() == Some(run[0]) && segment_kind(run[0]) == kind
        }
        _ => false,
    }
}

// The check a shell command runs, the most telling first, and whether its end says how the
// check went: the check is the one program it runs, setup aside, and runs last, as `known`
// says of a paired call. None for a command that runs none.
pub(super) fn check_of(command: &str) -> Option<(CheckKind, bool)> {
    let segments: Vec<Vec<String>> = segments(command).into_iter().filter(|words| !words[0].chars().all(|c| c.is_ascii_digit())).collect();
    let checks: Vec<Option<CheckKind>> = segments.iter().map(|words| segment_check(words)).collect();
    let check = [CheckKind::Gate, CheckKind::Simulator, CheckKind::CargoTest, CheckKind::Clippy, CheckKind::CargoCheck, CheckKind::NodeTest, CheckKind::Typecheck].into_iter().find(|c| checks.contains(&Some(*c)))?;
    let run: Vec<&Vec<String>> = segments.iter().filter(|words| segment_kind(words).is_some()).collect();
    Some((check, run.len() == 1 && checks.last() == Some(&Some(check))))
}

fn segment_check(words: &[String]) -> Option<CheckKind> {
    let has = |word: &str| words.iter().any(|w| w == word);
    let names = |part: &str| words.iter().any(|w| w.contains(part));
    Some(match (program(words).as_str(), words.get(1).map(String::as_str)) {
        ("node", _) if names("gate.ts") => CheckKind::Gate,
        ("node", _) if has("--test") && names("machine-sim") => CheckKind::Simulator,
        ("node", _) if has("--test") => CheckKind::NodeTest,
        ("cargo", Some("test")) => CheckKind::CargoTest,
        ("cargo", Some("clippy")) => CheckKind::Clippy,
        ("cargo", Some("check")) => CheckKind::CargoCheck,
        ("npm" | "pnpm" | "yarn", _) if has("typecheck") => CheckKind::Typecheck,
        ("npm" | "pnpm" | "yarn", _) if has("test") => CheckKind::NodeTest,
        ("tsc", _) => CheckKind::Typecheck,
        ("npx", _) if has("tsc") => CheckKind::Typecheck,
        _ => return None,
    })
}

// Whether a shell call of the worker's own closes a terminal, as a worker's last does its own:
// no stop follows it.
pub fn closes_terminal(call: &Call) -> bool {
    !call.sub_agent && call.tool == Tool::Shell && call.command.as_deref().is_some_and(|c| runs_words(c, "orca", &["terminal", "close"]))
}

// Whether a shell command runs `program` with `words` after it: `tolkster pr create`.
pub(super) fn runs_words(command: &str, program_name: &str, words: &[&str]) -> bool {
    segments(command).iter().any(|run| program(run) == program_name && run.len() > words.len() && run[1..=words.len()].iter().zip(words).all(|(a, b)| a == b))
}

// A heredoc's body in `rest`, up to the line of its delimiter, the first word `after` the
// `<<`, unquoted. None when no line ends it, so a delimiter misread never takes the
// command's lines after the body.
pub(super) fn heredoc<'a>(after: &str, rest: &'a str) -> Option<&'a str> {
    let delimiter = after.trim_start_matches('-').split_whitespace().next()?.trim_matches(['"', '\'']);
    let mut at = 0;
    for line in rest.split_inclusive('\n') {
        if !delimiter.is_empty() && line.trim() == delimiter {
            return Some(&rest[..at]);
        }
        at += line.len();
    }
    None
}
