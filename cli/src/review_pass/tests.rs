use super::*;

const LIST: &str = "Some prose.

```tolkster-review
skip *.md
skip design/
keep AGENTS.md
trust src/claim.rs
tests */tests.rs
comment //
import use
```
";

fn list() -> Settings {
    settings(LIST).unwrap().unwrap()
}

fn names(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|path| path.to_string()).collect()
}

// A diff adding `added` lines to `file`, and removing `removed` from another.
fn change(file: &str, added: &[&str], removed: &[&str]) -> Diff {
    Diff { added: added.iter().map(|line| (file.to_string(), line.to_string())).collect(), removed: removed.iter().map(|line| ("src/old.rs".to_string(), line.to_string())).collect() }
}

fn lines(count: usize) -> Vec<String> {
    (0..count).map(|n| format!("let value_{n} = {n};")).collect()
}

#[test]
fn the_list_comes_from_its_fence_and_refuses_an_unknown_key() {
    let list = list();
    assert_eq!(list.skip, ["*.md", "design/"]);
    assert_eq!((list.keep.as_slice(), list.trust.as_slice(), list.tests.as_slice(), list.comments.as_slice(), list.imports.as_slice()), (&["AGENTS.md".to_string()][..], &["src/claim.rs".to_string()][..], &["*/tests.rs".to_string()][..], &["//".to_string()][..], &["use".to_string()][..]));
    assert_eq!(settings("No block here.\n```rust\nskip x\n```\n").unwrap(), None);
    assert_eq!(settings("- an example:\n  ```tolkster-review\n  trust x\n  ```\n").unwrap(), None);
    assert!(settings("```tolkster-review\ntrusts src/claim.rs\n```\n").is_err());
    assert!(settings("```tolkster-review\ntrust\n```\n").is_err());
    assert!(settings("```tolkster-review\ntrust a b\n```\n").is_err());
}

#[test]
fn patterns_cover_as_a_pathspec_does() {
    assert!(covers("*.md", "docs/cli.md") && covers("*.md", "README.md"));
    assert!(covers("design/", "design/a/b.png") && !covers("design/", "src/design/x.rs"));
    assert!(covers("*/tests.rs", "cli/src/claim/tests.rs") && !covers("*/tests.rs", "cli/src/tests.rs.bak"));
    assert!(covers("AGENTS.md", "AGENTS.md") && !covers("AGENTS.md", "docs/AGENTS.md"));
    assert!(covers("test/", "test/contract.json") && !covers("test/", "cli/test.rs"));
    assert!(covers("src/*/state.ts", "src/worker/state.ts") && !covers("src/*/state.ts", "src/state.ts"));
}

#[test]
fn the_diff_keeps_each_added_line_with_its_file() {
    let text = "diff --git a/src/a.rs b/src/a.rs
index 1..2 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1 +1,2 @@
-old
+new
+++ still content
diff --git a/gone.rs b/gone.rs
deleted file mode 100644
--- a/gone.rs
+++ /dev/null
@@ -1 +0,0 @@
--- a removed line
";
    let diff = diff(text);
    assert_eq!(diff.added, [("src/a.rs".to_string(), "new".to_string()), ("src/a.rs".to_string(), "++ still content".to_string())]);
    let spaced = diff_of_spaced();
    assert_eq!(spaced.added, [("src/a b.rs".to_string(), "x".to_string())]);
    assert_eq!(diff.removed, [("src/a.rs".to_string(), "old".to_string()), ("gone.rs".to_string(), "-- a removed line".to_string())]);
}

// Git ends a name holding a space with a tab.
fn diff_of_spaced() -> Diff {
    diff("diff --git a/src/a b.rs b/src/a b.rs\n--- a/src/a b.rs\t\n+++ b/src/a b.rs\t\n@@ -0,0 +1 @@\n+x\n")
}

#[test]
fn locks_are_whole_words() {
    for line in ["let _held = actions.lock();", "static ACTIONS_LOCK: Mutex<()>", "struct RwLock", "type Guard = RWLock;", "self.locked.store(true)", ".arg(\"--disallowedTools\")", "this.ctx.blockConcurrencyWhile(async () => {})", "unlocking", "\"allowedTools\": []", "allowed_tools", "check the permissions"] {
        assert!(locks(line), "{line}");
    }
    for line in ["blocked_issues", "if blocking {", "let clock = now();", "blocks.push(x)", "Locksmith", "HTMLParser"] {
        assert!(!locks(line), "{line}");
    }
}

#[test]
fn the_size_and_copilot_decide_before_the_diff() {
    let diff = change("src/a.rs", &["let a = 1;"], &[]);
    let pass = |size, copilot| {
        let decided = decide(size, copilot, &list(), &names(&["src/a.rs"]), &diff);
        (decided.pass, decided.passes, decided.reason)
    };
    for copilot in [CopilotReview::OnEachPush, CopilotReview::OnOpening] {
        assert_eq!(pass(ReviewSize::Above, copilot), (Pass::Deep, 3, Reason::LargeIssue));
        for size in [ReviewSize::Below, ReviewSize::At, ReviewSize::Unsized] {
            assert_eq!(pass(size, copilot), (Pass::Light, 1, Reason::SmallChange));
        }
    }
    assert_eq!(pass(ReviewSize::Below, CopilotReview::Off), (Pass::Light, 1, Reason::SmallChange));
    assert_eq!(pass(ReviewSize::At, CopilotReview::Off), (Pass::Deep, 2, Reason::OnlyReview));
    assert_eq!(pass(ReviewSize::Above, CopilotReview::Off), (Pass::Deep, 3, Reason::OnlyReview));
    assert_eq!(pass(ReviewSize::Unsized, CopilotReview::Off), (Pass::Deep, 3, Reason::OnlyReview));
    assert!(!decide(ReviewSize::At, CopilotReview::Off, &list(), &names(&["src/a.rs"]), &diff).escalated);
}

// Skipped when CI skips every changed file, both sides of a rename; never when the list
// is silent, and never for a file CI keeps.
#[test]
fn a_change_ci_skips_skips_the_review() {
    let skip = |settings: &Settings, paths: &[&str]| decide(ReviewSize::Above, CopilotReview::Off, settings, &names(paths), &Diff::default()).reason;
    assert_eq!(skip(&list(), &["docs/a.md", "design/x.png"]), Reason::CiSkipsEveryFile);
    assert_eq!(skip(&list(), &["docs/a.md", "src/a.rs"]), Reason::OnlyReview);
    assert_eq!(skip(&list(), &["docs/a.md", "AGENTS.md"]), Reason::OnlyReview);
    assert_eq!(skip(&Settings::default(), &["docs/a.md"]), Reason::OnlyReview);
    assert_eq!(skip(&list(), &[]), Reason::NothingChanged);
}

// The triggers count new lines alone: not moved ones, blank ones, comments, or a lock in
// a test file. Below `deepReviewEstimate` a triggered deep pass runs once.
#[test]
fn a_trigger_escalates_the_light_pass_on_new_lines_alone() {
    let light = |paths: &[&str], diff: &Diff| decide(ReviewSize::Below, CopilotReview::OnEachPush, &list(), &names(paths), diff);

    let trust = light(&["src/claim.rs"], &change("src/claim.rs", &["if held { return; }"], &[]));
    assert_eq!((trust.pass, trust.passes, trust.escalated, trust.trigger, trust.file.as_deref()), (Pass::Deep, 1, true, Some(Reason::TrustFile), Some("src/claim.rs")));
    // Moved, re-indented, or a comment, in a trust file: no trigger.
    let moved = Diff { removed: vec![("src/claim.rs".to_string(), "if held { return; }".to_string())], ..change("src/claim.rs", &["        if held { return; }", "// Why it holds.", ""], &[]) };
    assert_eq!(light(&["src/claim.rs"], &moved).reason, Reason::SmallChange);

    // Import lines alone in a trust file, new or deleted, no trigger; one more line, or a
    // word merely starting like one, is. An import is no trust trigger alone: one naming a
    // lock still is a lock trigger.
    let imports = change("src/claim.rs", &["use crate::state;", "    use std::sync::Arc;"], &[]);
    assert_eq!(light(&["src/claim.rs"], &imports).reason, Reason::SmallChange);
    let dropped = Diff { removed: vec![("src/claim.rs".to_string(), "use crate::plain;".to_string())], ..Diff::default() };
    assert_eq!(light(&["src/claim.rs"], &dropped).reason, Reason::SmallChange);
    let mixed = change("src/claim.rs", &["use crate::state;", "if held { return; }"], &[]);
    assert_eq!(light(&["src/claim.rs"], &mixed).trigger, Some(Reason::TrustFile));
    assert_eq!(light(&["src/claim.rs"], &change("src/claim.rs", &["used = true;"], &[])).trigger, Some(Reason::TrustFile));
    assert_eq!(light(&["src/claim.rs"], &change("src/claim.rs", &["use crate::state; release(held);"], &[])).trigger, Some(Reason::TrustFile));
    assert_eq!(light(&["src/claim.rs"], &change("src/claim.rs", &["use std::sync::Mutex;"], &[])).trigger, Some(Reason::NewLock));
    let unlisted = decide(ReviewSize::Below, CopilotReview::OnEachPush, &Settings { imports: vec![], ..list() }, &names(&["src/claim.rs"]), &imports);
    assert_eq!(unlisted.trigger, Some(Reason::TrustFile));

    let lock = light(&["src/runtime.rs"], &change("src/runtime.rs", &["let _held = ACTIONS.lock();"], &[]));
    assert_eq!((lock.trigger, lock.file.as_deref()), (Some(Reason::NewLock), Some("src/runtime.rs")));
    for (file, line) in [("src/runtime/tests.rs", "let _held = ACTIONS.lock();"), ("src/runtime.rs", "// Takes the lock first."), ("src/runtime.rs", "if blocked_issues.is_empty() {")] {
        assert_eq!(light(&[file], &change(file, &[line], &[])).reason, Reason::SmallChange, "{line}");
    }
    // A line deleted from a trust file is a change there too, unless it moved; a line
    // removed from a file CI skips moves nothing.
    let deleted = Diff { removed: vec![("src/claim.rs".to_string(), "recheck(held)?;".to_string())], ..Diff::default() };
    assert_eq!(light(&["src/claim.rs"], &deleted).trigger, Some(Reason::TrustFile));
    let moved_within = Diff { added: vec![("src/claim.rs".to_string(), "    recheck(held)?;".to_string())], ..deleted };
    assert_eq!(light(&["src/claim.rs"], &moved_within).reason, Reason::SmallChange);
    let from_docs = Diff { added: vec![("src/claim.rs".to_string(), "Ok(())".to_string())], removed: vec![("docs/a.md".to_string(), "Ok(())".to_string())] };
    assert_eq!(light(&["src/claim.rs", "docs/a.md"], &from_docs).trigger, Some(Reason::TrustFile));
    // A line moved into or out of a trust file still counts; so does a change to the list.
    let into = Diff { added: vec![("src/claim.rs".to_string(), "return Ok(());".to_string())], removed: vec![("src/b.rs".to_string(), "return Ok(());".to_string())] };
    assert_eq!(light(&["src/claim.rs", "src/b.rs"], &into).trigger, Some(Reason::TrustFile));
    let out = Diff { added: vec![("src/b.rs".to_string(), "recheck(held)?;".to_string())], removed: vec![("src/claim.rs".to_string(), "recheck(held)?;".to_string())] };
    assert_eq!(light(&["src/claim.rs", "src/b.rs"], &out).trigger, Some(Reason::TrustFile));
    let listed = decide_with_list(ReviewSize::Below, CopilotReview::OnEachPush, &list(), &names(&["AGENTS.md"]), &Diff::default(), Some("AGENTS.md"));
    assert_eq!((listed.pass, listed.trigger, listed.file.as_deref()), (Pass::Deep, Some(Reason::TrustFile), Some("AGENTS.md")));
    let moved_lock = change("src/runtime.rs", &["let _held = ACTIONS.lock();"], &["let _held = ACTIONS.lock();"]);
    assert_eq!(light(&["src/runtime.rs", "src/old.rs"], &moved_lock).reason, Reason::SmallChange);

    let added = lines(501);
    let added: Vec<&str> = added.iter().map(String::as_str).collect();
    let many = light(&["src/a.rs"], &change("src/a.rs", &added, &[]));
    assert_eq!((many.trigger, many.new_lines, many.file), (Some(Reason::NewLines), 501, None));
    // The same lines moved from elsewhere, or in a file CI skips, count for nothing.
    assert_eq!(light(&["src/a.rs", "src/b.rs"], &change("src/a.rs", &added, &added)).reason, Reason::SmallChange);
    assert_eq!(light(&["docs/a.md", "src/a.rs"], &change("docs/a.md", &added, &[])).reason, Reason::SmallChange);
    // At 500, no trigger; without Copilot the line trigger is 200.
    assert_eq!(light(&["src/a.rs"], &change("src/a.rs", &added[..500], &[])).reason, Reason::SmallChange);
    let alone = decide(ReviewSize::Below, CopilotReview::Off, &list(), &names(&["src/a.rs"]), &change("src/a.rs", &added[..201], &[]));
    assert_eq!((alone.pass, alone.passes, alone.trigger), (Pass::Deep, 1, Some(Reason::NewLines)));
    // At or unsized with Copilot on, a triggered deep pass keeps the full cap.
    let at = decide(ReviewSize::At, CopilotReview::OnOpening, &list(), &names(&["src/a.rs"]), &change("src/a.rs", &added, &[]));
    assert_eq!((at.pass, at.passes), (Pass::Deep, 3));
}

// The note is settled text: the trigger's words, a plain path or a count, and nothing when
// nothing escalated.
#[test]
fn only_an_escalation_posts_a_note() {
    let decided = |file: &str, line: &str| decide(ReviewSize::Below, CopilotReview::OnEachPush, &list(), &names(&[file]), &change(file, &[line], &[]));
    assert_eq!(decided("src/runtime.rs", "let _held = ACTIONS.lock();").note().unwrap(), "Reviewing this one deeply: it adds a lock or a permission in src/runtime.rs.");
    assert_eq!(decided("src/claim.rs", "return;").note().unwrap(), "Reviewing this one deeply: it changes src/claim.rs, where ordering and trust live.");
    assert_eq!(decided("src/we ird.rs", "let m = Mutex::new(());").note().unwrap(), "Reviewing this one deeply: it adds a lock or a permission.");
    assert_eq!(decided("src/a.rs", "let a = 1;").note(), None);
    let added = lines(501);
    let added: Vec<&str> = added.iter().map(String::as_str).collect();
    let many = decide(ReviewSize::Below, CopilotReview::OnEachPush, &list(), &names(&["src/a.rs"]), &change("src/a.rs", &added, &[]));
    assert_eq!(many.note().unwrap(), "Reviewing this one deeply: it adds 501 new lines of code.");
    assert_eq!(decide(ReviewSize::At, CopilotReview::Off, &list(), &names(&["src/a.rs"]), &Diff::default()).note(), None);
}

#[test]
fn the_brief_names_round_trip() {
    for size in [ReviewSize::Below, ReviewSize::At, ReviewSize::Above, ReviewSize::Unsized] {
        assert_eq!(size_named(size_name(size)), Some(size));
    }
    for copilot in [CopilotReview::Off, CopilotReview::OnOpening, CopilotReview::OnEachPush] {
        assert_eq!(copilot_named(copilot_name(copilot)), Some(copilot));
    }
    assert_eq!((size_named("small"), copilot_named("on")), (None, None));
}

// The JSON a worker reads: kebab-case enums, and the trigger and file only when escalated.
#[test]
fn the_decision_prints_as_the_brief_reads_it() {
    let lock = decide(ReviewSize::Below, CopilotReview::Off, &list(), &names(&["src/r.rs"]), &change("src/r.rs", &["lock();"], &[]));
    let json = serde_json::to_value(&lock).unwrap();
    assert_eq!(json, serde_json::json!({ "pass": "deep", "passes": 1, "reason": "new-lock", "escalated": true, "trigger": "new-lock", "file": "src/r.rs", "newLines": 1 }));
    let skip = serde_json::to_value(decide(ReviewSize::Below, CopilotReview::Off, &list(), &names(&["a.md"]), &Diff::default())).unwrap();
    assert_eq!(skip, serde_json::json!({ "pass": "skip", "passes": 0, "reason": "ci-skips-every-file", "escalated": false, "newLines": 0 }));
}

// A claim with no estimate goes by the size its worker set; one with an estimate keeps it,
// whatever the issue says by the push. The triggers escalate either way.
#[test]
fn an_unsized_claim_goes_by_the_workers_size() {
    let (s, m) = (Size(2), Size(3));
    assert_eq!(sized(ReviewSize::Unsized, Some(s), m), (ReviewSize::Below, Some(s)));
    assert_eq!(sized(ReviewSize::Unsized, None, m), (ReviewSize::Unsized, None));
    assert_eq!(sized(ReviewSize::At, Some(s), m), (ReviewSize::At, None));
    let by = |file: &str, line: &str| {
        let (size, sized) = sized(ReviewSize::Unsized, Some(s), m);
        Decision { sized, ..decide(size, CopilotReview::Off, &list(), &names(&[file]), &change(file, &[line], &[])) }
    };
    let small = by("src/a.rs", "let a = 1;");
    assert_eq!((small.pass, small.passes, small.reason), (Pass::Light, 1, Reason::SmallChange));
    assert_eq!(small.sized_note().unwrap(), "It had no estimate when I started, so I'm reviewing it as the S it is now.");
    assert_eq!(serde_json::to_value(&small).unwrap()["sized"], "S");
    let trusted = by("src/claim.rs", "return;");
    assert_eq!((trusted.pass, trusted.trigger), (Pass::Deep, Some(Reason::TrustFile)));
    let skipped = Decision { sized: Some(s), ..decide(ReviewSize::Below, CopilotReview::Off, &list(), &names(&["a.md"]), &Diff::default()) };
    assert_eq!(skipped.sized_note(), None);
    let at_claim = decide(ReviewSize::At, CopilotReview::Off, &list(), &names(&["src/a.rs"]), &change("src/a.rs", &["let a = 1;"], &[]));
    assert_eq!((at_claim.pass, at_claim.passes, at_claim.sized_note()), (Pass::Deep, 2, None));
}
