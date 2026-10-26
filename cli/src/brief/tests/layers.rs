use super::*;

// The CLI's top-level modules, lowest layer first. A module imports only from its own
// layer or lower (docs/cli.md), so a type or helper lives in the lowest module that needs it.
const LAYERS: [&str; 10] = [
    "error plain sync time json http args estimates cli",
    "exec trace health gateway",
    "config credential harnesses",
    "git orca github linear trade wrangler cloudflare state post",
    "machine request issues sessions issue_state voice hosts offline allowlist timing",
    "runtime ports",
    "harness hook card brief review review_pass coauthor prebuilt update onboard watch",
    "executor reap converse context triage setup recipe panel",
    "live command scheduler listen",
    "commands",
];

// Today's imports that point up a layer, each `from -> to` once. The list only shrinks: a
// line whose edge is gone fails, so a fix deletes its line.
const UPWARD: &[&str] = &[
];

#[test]
fn each_module_imports_only_from_its_layer_or_lower() {
    let layer = |module: &str| LAYERS.iter().position(|names| names.split(' ').any(|name| name == module));
    let mut modules = std::collections::BTreeSet::new();
    let mut edges = std::collections::BTreeSet::new();
    for (at, text) in super::repo_rules::production_files() {
        if at == Path::new("main.rs") || at.starts_with("ports/fake") || at == Path::new("ports/fake.rs") {
            continue;
        }
        let first = at.components().next().unwrap().as_os_str().to_string_lossy().into_owned();
        let from = first.strip_suffix(".rs").unwrap_or(&first).to_string();
        for to in crate_references(&text) {
            if to != from {
                edges.insert((from.clone(), to));
            }
        }
        modules.insert(from);
    }
    for module in &modules {
        assert!(layer(module).is_some(), "no layer names the module {module}");
    }
    let named: Vec<&str> = LAYERS.iter().flat_map(|names| names.split(' ')).collect();
    for name in &named {
        assert!(modules.contains(*name), "a layer names {name}, which isn't a module");
        assert_eq!(named.iter().filter(|other| *other == name).count(), 1, "more than one layer names {name}");
    }
    assert!(edges.len() >= 400, "{}", edges.len());
    let mut upward = Vec::new();
    for (from, to) in &edges {
        let below = layer(to).unwrap_or_else(|| panic!("{from} names crate::{to}, which no layer names"));
        if below > layer(from).unwrap() {
            upward.push(format!("{from} -> {to}"));
        }
    }
    for edge in &upward {
        assert!(UPWARD.contains(&edge.as_str()), "{edge} imports from a higher layer");
    }
    for listed in UPWARD {
        assert!(upward.iter().any(|edge| edge == listed), "{listed} is listed, but no longer imports up: delete its line");
    }
    let level: Vec<(&str, &str)> = edges.iter().filter(|(from, to)| layer(from) == layer(to)).map(|(from, to)| (from.as_str(), to.as_str())).collect();
    let cycles = cycles(&level);
    assert!(cycles.is_empty(), "modules in one layer import each other: {}", cycles.join("; "));
}

// Each set of modules that reach one another through `edges`, as "a <-> b <-> c".
fn cycles(edges: &[(&str, &str)]) -> Vec<String> {
    let reach = |start: &str| {
        let mut seen = std::collections::BTreeSet::new();
        let mut next = vec![start];
        while let Some(at) = next.pop() {
            for (_, to) in edges.iter().filter(|(from, _)| *from == at) {
                if seen.insert(*to) {
                    next.push(to);
                }
            }
        }
        seen
    };
    let modules: std::collections::BTreeSet<&str> = edges.iter().map(|(from, _)| *from).collect();
    let mut found = std::collections::BTreeSet::new();
    for module in &modules {
        let ahead = reach(module);
        let set: Vec<&str> = ahead.iter().copied().filter(|other| reach(other).contains(module)).collect();
        if !set.is_empty() {
            found.insert(set.join(" <-> "));
        }
    }
    found.into_iter().collect()
}

#[test]
fn a_cycle_names_its_modules() {
    let edges = [("reap", "executor"), ("executor", "reap"), ("hook", "trace"), ("a", "b"), ("b", "c"), ("c", "a")];
    assert_eq!(cycles(&edges), ["a <-> b <-> c", "executor <-> reap"]);
    assert!(cycles(&[("hook", "trace"), ("trace", "time")]).is_empty());
}

// The top-level module of each `crate::` path in a file's code, a `use crate::{…}` group's
// each, outside comments.
fn crate_references(text: &str) -> Vec<String> {
    // A line's code ends at its `//`, unless a string may hold it, as a URL does.
    let code: String = text.lines().map(|line| line.split_once("//").filter(|(code, _)| !code.contains('"')).map_or(line, |(code, _)| code)).collect::<Vec<_>>().join("\n");
    let ident = |rest: &str| rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect::<String>();
    let mut names = Vec::new();
    for piece in code.split("crate::").skip(1) {
        let Some(group) = piece.strip_prefix('{') else {
            names.push(ident(piece));
            continue;
        };
        let (mut depth, mut next) = (0, true);
        for (i, c) in group.char_indices() {
            match c {
                '{' => depth += 1,
                '}' if depth == 0 => break,
                '}' => depth -= 1,
                ',' if depth == 0 => next = true,
                c if c.is_whitespace() => {}
                _ if next && depth == 0 => {
                    names.push(ident(&group[i..]));
                    next = false;
                }
                _ => {}
            }
        }
    }
    names.retain(|name| !name.is_empty());
    names
}

#[test]
fn a_crate_reference_maps_to_its_top_level_module() {
    let text = "use crate::linear::Issue;\n// crate::orca\nuse crate::{git, // why\n    http::{self, Error},\n    plain::Name};\nlet x = crate::time::now();";
    assert_eq!(crate_references(text), ["linear", "git", "http", "plain", "time"]);
}
