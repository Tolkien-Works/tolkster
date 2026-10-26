use super::*;
use crate::args::{Kind, parse};

// Every `tolkster` line a brief names is one the CLI parses (commands::COMMANDS, with the
// options each command's `run` parses), so renaming a command or an option a brief names
// fails here. Those in a turn's brief and a thread's also pass its gate (allowlist.rs).

// The words of a shell line: quotes group them and are dropped.
fn words(line: &str) -> Vec<String> {
    let (mut words, mut word, mut quote, mut open) = (Vec::new(), String::new(), None, false);
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => (quote, open) = (Some(c), true),
            (None, c) if c.is_whitespace() => {
                if open || !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
                open = false;
            }
            (None, c) => word.push(c),
        }
    }
    if open || !word.is_empty() {
        words.push(word);
    }
    words
}

// The `tolkster` lines in a brief's code spans, whitespace collapsed, a line a span wraps
// included, and each command a span chains. A span's `[...]` marks what may be left out,
// and `…` that more may follow.
fn lines(text: &str) -> Vec<Vec<String>> {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.split('`')
        .skip(1)
        .step_by(2)
        .flat_map(|span| span.split("; ").flat_map(|command| command.split(" && ")))
        .filter_map(|span| span.strip_prefix("tolkster"))
        .filter(|rest| rest.is_empty() || rest.starts_with(' '))
        .map(|rest| words(rest).into_iter().map(|w| w.trim_matches(['[', ']']).to_string()).filter(|w| !w.is_empty() && w != "…").collect())
        .collect()
}

// A line naming alternatives, `relate|unrelate`, is each of them.
fn alternatives(line: &[String]) -> Vec<Vec<String>> {
    match line.iter().take(2).position(|w| w.contains('|')) {
        Some(at) => line[at].split('|').map(|alt| [&line[..at], &[alt.to_string()], &line[at + 1..]].concat()).collect(),
        None => vec![line.to_vec()],
    }
}

// What a line names, the CLI parses: its command, and its options with a value each. One
// that ends in an option names it alone, as in "with `--estimate`".
fn parses(line: &[String]) -> Result<(), String> {
    if line.is_empty() {
        return Ok(());
    }
    let (spec, rest) = crate::commands::find(line).ok_or("names no command")?;
    let mut rest = rest.to_vec();
    if let Some(last) = rest.last().and_then(|w| w.strip_prefix("--"))
        && spec.options.iter().any(|(name, kind)| *name == last && *kind != Kind::Boolean)
    {
        rest.push("<value>".into());
    }
    parse(&rest, spec.options, spec.arguments).map(|_| ()).map_err(|error| error.to_string())
}

fn rendered() -> Vec<(&'static str, String)> {
    let reviewers = vec!["copilot-pull-request-reviewer".to_string()];
    let mut texts = briefs(&reviewers);
    texts.push(("context", context()));
    texts
}

#[test]
fn every_tolkster_line_in_a_brief_parses() {
    let mut checked = 0;
    for (which, text) in rendered() {
        // An odd backtick would turn the spans after it into prose, and skip their lines.
        assert_eq!(text.matches('`').count() % 2, 0, "{which}");
        for line in lines(&text).iter().flat_map(|line| alternatives(line)) {
            assert!(parses(&line).is_ok(), "`tolkster {}` in {which}: {:?}", line.join(" "), parses(&line));
            checked += 1;
        }
    }
    assert!(checked > 50, "{checked}");
}

#[test]
fn a_turns_tolkster_lines_pass_its_gate() {
    for (which, text, own, thread) in [("turn", turn().render(), SESSION, false), ("thread", thread().render(), COMMENT, true)] {
        let lines: Vec<Vec<String>> = lines(&text).iter().flat_map(|line| alternatives(line)).filter(|line| !line.is_empty()).collect();
        assert!(lines.len() > 5, "{which}");
        for line in lines {
            // A line that names its own session or thread is run as it is; one that leaves it
            // to the prose names a command and options the gate lets through.
            let dispatched = crate::commands::find(&line).map(|(spec, _)| spec.words);
            let passed = if line.iter().any(|w| w == own) {
                if thread { crate::allowlist::gate_thread(&line, dispatched, own) } else { crate::allowlist::gate(&line, dispatched, own) }
            } else if line.len() == 1 && line[0] == "issue" {
                Ok(())
            } else {
                crate::allowlist::runs(&line, dispatched, thread)
            };
            assert!(passed.is_ok(), "`tolkster {}` in {which}: {passed:?}", line.join(" "));
        }
    }
}

// The check itself: a renamed option or command, or one a turn's gate refuses, fails.
#[test]
fn a_line_the_cli_wouldnt_take_fails() {
    let line = |text: &str| lines(&format!("`tolkster {text}`")).remove(0);
    for good in ["hook start", "hook after", "pr checks --pr <n>", "issue update --estimate", "activity s1 thought --state rebasing", "issue show <ID> [--comments]"] {
        assert!(parses(&line(good)).is_ok(), "{good}");
    }
    for bad in ["hook commit", "pr checks --pr <n> --watch", "pr merge --pr <n>", "issues show TW-1", "activity s1 thought --state x --while -- tolkster pr checks --watch", "issue update TW-1 --delegated"] {
        assert!(parses(&line(bad)).is_err(), "{bad}");
    }
    assert_eq!(alternatives(&line("issue relate|unrelate <ID>")).len(), 2);
    let runs = |line: Vec<String>| crate::allowlist::runs(&line, crate::commands::find(&line).map(|(spec, _)| spec.words), false);
    assert!(runs(line("pr create --head x")).is_err());
    assert!(runs(line("comment TW-1 --body-file <path>")).is_err());
    assert_eq!(words("--action \"Plan approved\" --body ''"), ["--action", "Plan approved", "--body", ""]);
}
