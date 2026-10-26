use super::*;

// Tolkster runs without gh (TW-197): every GitHub call goes through the app. No brief,
// partial or template in cli/briefs/ names it, and nothing in cli/src/ runs it.
#[test]
fn nothing_uses_gh() {
    let gh = ["g", "h"].concat();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let names = |text: &str| text.split(|c: char| !c.is_ascii_alphanumeric()).any(|word| word == gh);
    let briefs: Vec<PathBuf> = std::fs::read_dir(root.join("briefs")).unwrap().map(|e| e.unwrap().path()).collect();
    assert!(briefs.len() >= 8, "{briefs:?}");
    for brief in briefs {
        assert!(!names(&std::fs::read_to_string(&brief).unwrap()), "{} names it", brief.display());
    }
    let spawned = format!("\"{gh}\"");
    let mut dirs = vec![root.join("src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                assert!(!std::fs::read_to_string(&path).unwrap().contains(&spawned), "{} runs it", path.display());
            }
        }
    }
}

// The command layer sits on the core, never under it: what `listen` runs, recipe machines
// included, lives outside commands/, so no production file but main.rs and those in commands/
// names it, by any path.
// Tests may, to check a brief's lines against the CLI or what a command renders.
#[test]
fn only_main_and_the_commands_name_the_commands() {
    let mut read = 0;
    for (at, text) in production_files() {
        if !at.starts_with("commands") && at != Path::new("main.rs") {
            read += 1;
            assert!(!text.contains("commands::"), "{} names commands::", at.display());
        }
    }
    assert!(read >= 50, "{read}");
}

// A poisoned lock is taken one way, in sync.rs, so no other file names PoisonError.
#[test]
fn only_sync_takes_a_poisoned_lock() {
    for (at, text) in production_files() {
        assert!(at == Path::new("sync.rs") || !text.contains("PoisonError"), "{} names PoisonError", at.display());
    }
}

// What `listen` keeps from command to command lives in its context (runtime::Shared), not a
// static: a static or thread-local that holds state says why it is the process's or a
// thread's, in a comment above it or the group of statics it leads.
#[test]
fn each_static_says_why_it_is_one() {
    let stateful = ["Mutex", "RwLock", "Atomic", "Cell", "Once", "Lazy"];
    let opens = |line: &str| line.starts_with("static ") || line.starts_with("pub static ") || line.starts_with("thread_local!");
    let mut read = 0;
    for (at, text) in production_files() {
        let lines: Vec<&str> = text.lines().map(str::trim_start).collect();
        for (i, line) in lines.iter().enumerate() {
            if opens(line) && !line.starts_with("thread_local!") && stateful.iter().any(|kind| line.contains(kind)) {
                read += 1;
                let above = lines[..i].iter().rev().find(|line| !opens(line));
                assert!(above.is_some_and(|line| line.starts_with("//")), "{}:{} says no reason", at.display(), i + 1);
            }
        }
    }
    assert!(read >= 20, "{read}");
}

// Every production file under src/, by its path there, with its text: no test file.
pub(super) fn production_files() -> Vec<(PathBuf, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut dirs = vec![src.clone()];
    let mut files = Vec::new();
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let at = path.strip_prefix(&src).unwrap().to_path_buf();
            if at.components().any(|part| part.as_os_str() == "tests") {
                continue;
            }
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") && !path.ends_with("tests.rs") {
                files.push((at, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    files
}

// The briefs are shared by every repo (TW-267), so none names this repo's own paths,
// checks or CI: a worker takes those from the repo's AGENTS.md / CLAUDE.md, and this
// repo's AGENTS.md states its CI skip paths.
#[test]
fn the_briefs_carry_no_rules_of_this_repo() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for entry in std::fs::read_dir(root.join("briefs")).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        for own in ["design/", "cli/src/", "cli/briefs/", "src/worker/", "src/shared/", "typecheck", "wrangler", "dry run", "Actions minutes"] {
            assert!(!text.contains(own), "{} names {own}", path.display());
        }
    }
    let agents = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../AGENTS.md")).unwrap();
    assert!(agents.contains("its skip paths are `*.md` and `design/`, except\n`AGENTS.md`, `README.md`, `cli/briefs/` and `docs/`"));
}

// This repo's review list, which `tolkster pr review-pass` reads (TW-372), says what its
// prose does, and names only paths that exist.
#[test]
fn this_repos_review_list_matches_its_prose() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let agents = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../AGENTS.md")).unwrap();
    let list = crate::review_pass::settings(&agents).unwrap().expect("AGENTS.md has no tolkster-review block");
    assert_eq!((list.skip.as_slice(), list.keep.as_slice()), (&["*.md".to_string(), "design/".to_string()][..], &["AGENTS.md".to_string(), "README.md".to_string(), "cli/briefs/".to_string(), "docs/".to_string()][..]));
    assert!(!list.trust.is_empty() && !list.tests.is_empty() && !list.comments.is_empty());
    for path in &list.trust {
        assert!(repo.join(path).exists(), "{path} doesn't exist");
    }
}

// AGENTS.md's `sides` block says which side's checks each path needs (TW-614): every file
// outside cli/ that a Rust test reads, or `include_str!` compiles in, is on the Rust side,
// so a worker whose diff touches it runs `cargo test`. A read found here and not listed
// fails, so the list can't drift; test/sides.test.ts holds the TypeScript side to it.
#[test]
fn every_read_outside_cli_is_on_the_rust_side() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let agents = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../AGENTS.md")).unwrap();
    let block = agents.split("```sides\n").nth(1).expect("AGENTS.md has no sides block").split("```").next().unwrap();
    let mut rust = Vec::new();
    for line in block.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let (side, pattern) = line.split_once(' ').unwrap_or((line, ""));
        assert!(["rust", "ts"].contains(&side) && !pattern.is_empty(), "the sides block's line {line:?}");
        if side == "rust" {
            rust.push(pattern.to_string());
        }
    }
    // A path's segments from the repo's root, after `..` and `.`.
    let resolve = |base: &[String], relative: &str| {
        let mut path = base.to_vec();
        for segment in relative.split('/') {
            match segment {
                ".." => _ = path.pop(),
                "." | "" => {}
                segment => path.push(segment.to_string()),
            }
        }
        path.join("/")
    };
    // Split so this file's own text doesn't match.
    let (included, bytes, across) = (["include_", "str!(\""].concat(), ["include_", "bytes!(\""].concat(), ["\"/", "../"].concat());
    let mut reads = Vec::new();
    let mut dirs = vec![root.join("src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let file: Vec<String> = path.parent().unwrap().strip_prefix(root.parent().unwrap()).unwrap().components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            for (marker, base, prefix) in [(&included, &file, ""), (&bytes, &file, ""), (&across, &vec!["cli".to_string()], "../")] {
                for piece in text.split(marker.as_str()).skip(1) {
                    reads.push(resolve(base, &format!("{prefix}{}", &piece[..piece.find('"').unwrap()])));
                }
            }
        }
    }
    for read in ["AGENTS.md", "src/shared/voice.json", "test/contract.json", "cli/briefs/work.md"] {
        assert!(reads.iter().any(|r| r == read), "the scan missed {read}");
    }
    for read in reads.iter().filter(|read| !read.starts_with("cli/")) {
        assert!(rust.iter().any(|pattern| crate::review_pass::covers(pattern, read)), "a Rust test reads {read}, which AGENTS.md's sides block doesn't put on the Rust side");
    }
}
