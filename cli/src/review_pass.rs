// The pre-push review's pass, decided in code rather than read off the brief's prose by the
// worker (docs/pre-push-review.md, "Which pass"): `tolkster pr review-pass`. It takes the
// claim's size and Copilot's review, which the work brief renders, the repo's review list,
// and the branch's diff. The list is read from the default branch, never the branch under
// review, so a branch can't loosen its own review.

use std::collections::HashMap;

use serde::Serialize;

use crate::args::named;
use crate::error::Error;
use crate::estimates::Size;
use crate::github::CopilotReview;
use crate::plain::RepoPath;
use crate::voice::{DEEPER, DEEPER_LINES, DEEPER_LOCK, DEEPER_LOCK_IN, DEEPER_TRUST, DEEPER_TRUST_FILE, SIZED_NOW};

// The issue's size against `review.deepAt`. Read at the start and kept in `brief::Taken`, so
// an estimate set after it never lightens that worker's review, unless the start had none:
// then `tolkster pr review-pass` reads the one its worker set (`sized`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ReviewSize {
    Below,
    At,
    Above,
    Unsized,
}

pub fn size_against(size: Option<Size>, threshold: Size) -> ReviewSize {
    match size {
        None => ReviewSize::Unsized,
        Some(size) if size < threshold => ReviewSize::Below,
        Some(size) if size == threshold => ReviewSize::At,
        Some(_) => ReviewSize::Above,
    }
}

// The info string of the fenced block in AGENTS.md or CLAUDE.md that holds the list.
pub const FENCE: &str = "tolkster-review";
// Where the list is looked for, in order: the first file that has the block.
pub const FILES: [&str; 2] = ["AGENTS.md", "CLAUDE.md"];

// The light pass's line trigger. Without Copilot it is lower: a small issue with a larger
// diff was likely sized wrong, and the light pass would be its only review.
const NEW_LINES: usize = 500;
const NEW_LINES_ALONE: usize = 200;
// How many reviewers the deep pass starts at once, and fewer without Copilot at
// `review.deepAt`. Below it, a deep pass on a trigger starts one.
const PASSES: u32 = 3;
const FEWER_PASSES: u32 = 2;

// Words that take or add a lock, or set what a session may run, matched whole after an
// identifier is split into its words, so `blocked` or `clock` never match.
const LOCK_WORDS: [&str; 13] = ["lock", "locks", "locked", "locking", "unlock", "unlocks", "unlocked", "unlocking", "relock", "mutex", "mutexes", "permission", "permissions"];
// Pairs of words that do the same: what a session may run, and a Durable Object's input
// lock (`blockConcurrencyWhile`).
const LOCK_PAIRS: [(&str, &str); 3] = [("allowed", "tools"), ("disallowed", "tools"), ("block", "concurrency")];

// The repo's review list. Silent, it names nothing: no change is skipped, and only the
// size and new-line triggers apply.
#[derive(Debug, Default, PartialEq)]
pub struct Settings {
    // The paths CI skips, and the exceptions among them it still runs on.
    pub skip: Vec<String>,
    pub keep: Vec<String>,
    // The files where ordering and trust live.
    pub trust: Vec<String>,
    // Test files, whose locks are no trigger.
    pub tests: Vec<String>,
    // What a comment line starts with, whose words are no trigger.
    pub comments: Vec<String>,
    // The word an import line starts with, which a trust file's trigger doesn't count.
    pub imports: Vec<String>,
}

// The first `tolkster-review` block in `text` whose fence starts its line, None when there
// is none. An unknown key is
// an error, never ignored: a misspelt `trust` would quietly lighten the review.
pub fn settings(text: &str) -> Result<Option<Settings>, Error> {
    let mut lines = text.lines();
    if !lines.any(|line| line.strip_prefix("```").is_some_and(|info| info.trim() == FENCE)) {
        return Ok(None);
    }
    let mut settings = Settings::default();
    for line in lines.take_while(|line| !line.starts_with("```")).map(str::trim).filter(|line| !line.is_empty()) {
        let (key, value) = line.split_once(char::is_whitespace).map(|(key, value)| (key, value.trim())).unwrap_or((line, ""));
        let list = match key {
            "skip" => &mut settings.skip,
            "keep" => &mut settings.keep,
            "trust" => &mut settings.trust,
            "tests" => &mut settings.tests,
            "comment" => &mut settings.comments,
            "import" => &mut settings.imports,
            _ => return Err(Error::Failed(format!("the {FENCE} block has an unknown key: {key}"))),
        };
        if value.is_empty() || value.contains(char::is_whitespace) {
            return Err(Error::Failed(format!("the {FENCE} block's {key} takes one value")));
        }
        list.push(value.to_string());
    }
    Ok(Some(settings))
}

// A path pattern as the list writes one, as in a git pathspec: `dir/` covers everything
// under it, `*` stands for any run of characters, `/` included, and anything else is one
// path from the repo's root.
pub fn covers(pattern: &str, path: &str) -> bool {
    let pattern = if pattern.ends_with('/') { format!("{pattern}*") } else { pattern.to_string() };
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if parts.len() == 1 {
        return path == first;
    }
    if path.len() < first.len() + last.len() || !path.starts_with(first) || !path.ends_with(last) {
        return false;
    }
    let mut rest = &path[first.len()..path.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

fn any_covers(patterns: &[String], path: &str) -> bool {
    patterns.iter().any(|pattern| covers(pattern, path))
}

// A diff's changed lines, from `git diff -U0`, each with its file.
#[derive(Debug, Default, PartialEq)]
pub struct Diff {
    pub added: Vec<(String, String)>,
    pub removed: Vec<(String, String)>,
}

pub fn diff(text: &str) -> Diff {
    let mut diff = Diff::default();
    let (mut old, mut file, mut in_hunk) = (None, None, false);
    for line in text.lines() {
        if line.starts_with("diff --git ") {
            (old, file, in_hunk) = (None, None, false);
        } else if !in_hunk {
            if let Some(path) = line.strip_prefix("--- ") {
                old = path.trim_end_matches('\t').trim_matches('"').strip_prefix("a/").map(str::to_string);
            } else if let Some(path) = line.strip_prefix("+++ ") {
                file = path.trim_end_matches('\t').trim_matches('"').strip_prefix("b/").map(str::to_string);
            } else if line.starts_with("@@") {
                in_hunk = true;
            }
        } else if let Some(added) = line.strip_prefix('+') {
            if let Some(file) = &file {
                diff.added.push((file.clone(), added.to_string()));
            }
        } else if let Some(removed) = line.strip_prefix('-')
            && let Some(old) = &old
        {
            diff.removed.push((old.clone(), removed.to_string()));
        }
    }
    diff
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Pass {
    Skip,
    Light,
    Deep,
}

// Why the pass is what it is. The last three are the triggers that take a light pass deep.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    NothingChanged,
    CiSkipsEveryFile,
    // The issue is above `review.deepAt` and Copilot reviews.
    LargeIssue,
    // Copilot doesn't review, and the issue is at `review.deepAt`, above it or unsized.
    OnlyReview,
    SmallChange,
    TrustFile,
    NewLock,
    NewLines,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Decision {
    pub pass: Pass,
    // None to skip, one for the light pass, and for the deep pass how many reviewers it
    // starts at once.
    pub passes: u32,
    pub reason: Reason,
    // Whether a trigger took it deeper than the issue's size calls for, and which.
    pub escalated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger: Option<Reason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(rename = "newLines")]
    pub new_lines: usize,
    // The size an issue that had none at the claim has now, which the review went by. None on a
    // skip, which no size decides.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sized: Option<Size>,
}

// The size the review goes by: the claim's, or, when the claim had none, the issue's as
// read `now`, the estimate its worker set, and that size. An estimate at the claim is never
// read again, so one set later can't lighten the review; `now` is None when it wasn't read.
pub fn sized(claim: ReviewSize, now: Option<Size>, threshold: Size) -> (ReviewSize, Option<Size>) {
    match (claim, now) {
        (ReviewSize::Unsized, Some(size)) => (size_against(Some(size), threshold), Some(size)),
        _ => (claim, None),
    }
}

// What the claim's size and Copilot's review call for before the diff is read: the pass,
// and for the light pass, its line trigger and the reviewers of a deep pass it triggers.
enum Planned {
    Deep(Reason, u32),
    Light { lines: usize, passes: u32 },
}

fn planned(size: ReviewSize, copilot: CopilotReview) -> Planned {
    let triggered = if size == ReviewSize::Below { 1 } else { PASSES };
    match (copilot.opening(), size) {
        (true, ReviewSize::Above) => Planned::Deep(Reason::LargeIssue, PASSES),
        (true, _) => Planned::Light { lines: NEW_LINES, passes: triggered },
        (false, ReviewSize::Below) => Planned::Light { lines: NEW_LINES_ALONE, passes: triggered },
        (false, ReviewSize::At) => Planned::Deep(Reason::OnlyReview, FEWER_PASSES),
        (false, _) => Planned::Deep(Reason::OnlyReview, PASSES),
    }
}

// `names` are the changed files, both sides of a rename.
#[cfg(test)]
pub fn decide(size: ReviewSize, copilot: CopilotReview, settings: &Settings, names: &[String], diff: &Diff) -> Decision {
    decide_with_list(size, copilot, settings, names, diff, None)
}

// As `decide`, with `list_changed` the file whose review list the branch changes, which
// triggers as a trust file does.
pub fn decide_with_list(size: ReviewSize, copilot: CopilotReview, settings: &Settings, names: &[String], diff: &Diff, list_changed: Option<&str>) -> Decision {
    let skipped = |path: &str| any_covers(&settings.skip, path);
    let new = unmatched(settings, &diff.added, &diff.removed);
    let decision = |pass, passes, reason, file: Option<&str>| {
        let escalated = matches!(reason, Reason::TrustFile | Reason::NewLock | Reason::NewLines);
        Decision { pass, passes, reason, escalated, trigger: escalated.then_some(reason), file: file.map(str::to_string), new_lines: new.len(), sized: None }
    };
    if names.is_empty() {
        return decision(Pass::Skip, 0, Reason::NothingChanged, None);
    }
    if names.iter().all(|path| skipped(path) && !any_covers(&settings.keep, path)) {
        return decision(Pass::Skip, 0, Reason::CiSkipsEveryFile, None);
    }
    let (lines, passes) = match planned(size, copilot) {
        Planned::Deep(reason, passes) => return decision(Pass::Deep, passes, reason, None),
        Planned::Light { lines, passes } => (lines, passes),
    };
    if let Some(file) = list_changed {
        return decision(Pass::Deep, passes, Reason::TrustFile, Some(file));
    }
    let code = || new.iter().filter(|(_, line)| !comment(settings, line));
    // A trust file's deleted line is a behaviour change there too: a guard removed.
    let gone = unmatched(settings, &diff.removed, &diff.added);
    let trusted = code().chain(gone.iter().filter(|(_, line)| !comment(settings, line))).filter(|(_, line)| !import(settings, line)).find(|(file, _)| any_covers(&settings.trust, file));
    if let Some((file, _)) = trusted {
        return decision(Pass::Deep, passes, Reason::TrustFile, Some(file));
    }
    if let Some((file, _)) = code().find(|(file, line)| !any_covers(&settings.tests, file) && locks(line)) {
        return decision(Pass::Deep, passes, Reason::NewLock, Some(file));
    }
    if new.len() > lines {
        return decision(Pass::Deep, passes, Reason::NewLines, None);
    }
    decision(Pass::Light, 1, Reason::SmallChange, None)
}

// The lines of `lines` that are new, or with the sides swapped gone: in a file CI doesn't
// skip, not blank, and not moved, which is a line matching one on the other side, in a
// file CI doesn't skip either, indentation aside. A trust file's line matches only one in
// the same file, so a line moved into or out of it still counts.
fn unmatched<'a>(settings: &Settings, lines: &'a [(String, String)], other: &[(String, String)]) -> Vec<(&'a str, &'a str)> {
    let code = |file: &str| !any_covers(&settings.skip, file);
    let group = |file: &'a str| if any_covers(&settings.trust, file) { file } else { "" };
    let mut pool: HashMap<(&str, &str), usize> = HashMap::new();
    for (file, line) in other.iter().filter(|(file, _)| code(file)) {
        let group = if any_covers(&settings.trust, file) { file.as_str() } else { "" };
        *pool.entry((group, line.trim())).or_default() += 1;
    }
    let mut new = Vec::new();
    for (file, line) in lines.iter().filter(|(file, _)| code(file)) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match pool.get_mut(&(group(file), trimmed)).filter(|count| **count > 0) {
            Some(count) => *count -= 1,
            None => new.push((file.as_str(), trimmed)),
        }
    }
    new
}

fn comment(settings: &Settings, line: &str) -> bool {
    settings.comments.iter().any(|prefix| line.starts_with(prefix.as_str()))
}

// An import line is one statement starting with the list's word and then a space, so `use`
// never matches `used` and `use a; run();` isn't one. Only a trust file's trigger leaves
// these out: a move between modules rewrites them. Where an import points is no trigger.
fn import(settings: &Settings, line: &str) -> bool {
    let one = !line.trim_end_matches(';').contains(';');
    one && settings.imports.iter().any(|word| line.strip_prefix(word.as_str()).is_some_and(|rest| rest.starts_with(char::is_whitespace)))
}

// Whether a line names a lock, a mutex, a permission or allowed tools, as whole words.
pub fn locks(line: &str) -> bool {
    let words = words(line);
    words.iter().any(|word| LOCK_WORDS.contains(&word.as_str())) || words.windows(2).any(|pair| LOCK_PAIRS.contains(&(pair[0].as_str(), pair[1].as_str())))
}

// A line's words, lower-cased: split at anything but a letter or digit, `_` and `-`
// included, where a capital follows a lower-case letter or digit, and before the last
// capital of a run followed by a lower-case letter (`RWLock` is `rw`, `lock`).
fn words(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut words = Vec::new();
    let mut word = String::new();
    for (at, &c) in chars.iter().enumerate() {
        let before = at.checked_sub(1).map(|at| chars[at]);
        let after = chars.get(at + 1);
        let capital = c.is_ascii_uppercase()
            && (before.is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                || (before.is_some_and(|b| b.is_ascii_uppercase()) && after.is_some_and(|a| a.is_ascii_lowercase())));
        if (!c.is_ascii_alphanumeric() || capital) && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        if c.is_ascii_alphanumeric() {
            word.push(c.to_ascii_lowercase());
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

impl Decision {
    // The durable thought `--session` posts when a trigger escalated the review: a settled
    // message built from the trigger, and a path only when it is plain. None otherwise.
    pub fn note(&self) -> Option<String> {
        let path = self.file.as_deref().and_then(RepoPath::parse);
        let path = path.as_deref();
        let why = match (self.trigger?, path) {
            (Reason::TrustFile, Some(path)) => DEEPER_TRUST_FILE.replace("<path>", path),
            (Reason::TrustFile, None) => DEEPER_TRUST.to_string(),
            (Reason::NewLock, Some(path)) => DEEPER_LOCK_IN.replace("<path>", path),
            (Reason::NewLock, None) => DEEPER_LOCK.to_string(),
            (Reason::NewLines, _) => DEEPER_LINES.replace("<count>", &self.new_lines.to_string()),
            _ => return None,
        };
        Some(format!("{DEEPER}{why}"))
    }

    // The durable thought `--session` posts when a review goes by the size the issue has now.
    pub fn sized_note(&self) -> Option<String> {
        self.sized.filter(|_| self.pass != Pass::Skip).map(|size| SIZED_NOW.replace("<size>", size.name()))
    }
}

// The claim's size and Copilot's review as the work brief renders them for the command.
pub fn size_name(size: ReviewSize) -> &'static str {
    match size {
        ReviewSize::Below => "below",
        ReviewSize::At => "at",
        ReviewSize::Above => "above",
        ReviewSize::Unsized => "unsized",
    }
}

pub fn copilot_name(copilot: CopilotReview) -> &'static str {
    match copilot {
        CopilotReview::Off => "off",
        CopilotReview::OnOpening => "opening",
        CopilotReview::OnEachPush => "each-push",
    }
}

pub fn size_named(name: &str) -> Option<ReviewSize> {
    named(&[ReviewSize::Below, ReviewSize::At, ReviewSize::Above, ReviewSize::Unsized], size_name, name)
}

pub fn copilot_named(name: &str) -> Option<CopilotReview> {
    named(&[CopilotReview::Off, CopilotReview::OnOpening, CopilotReview::OnEachPush], copilot_name, name)
}

#[cfg(test)]
mod tests;
