use super::*;

fn contract() -> Value {
    serde_json::from_str(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../test/contract.json")).unwrap()).unwrap()
}

#[test]
fn a_launch_reads_back_as_written() {
    let build = Sha::parse("2878d3d000000000000000000000000000000000").unwrap();
    for (kind, build) in [(Kind::Start, Some(build)), (Kind::Rebase, None)] {
        assert_eq!(launch_of(&launch_value(kind, 1_729_000_000_000, build.as_ref())), Some((kind, 1_729_000_000_000, build)));
    }
    for text in ["", "start", "start 1", "begin 1 -", "start x -", "start 1 abc", "start 1 - extra"] {
        assert_eq!(launch_of(text), None, "{text}");
    }
}

// The contract with the relay (src/worker/timing.ts): the report is built as test/contract.json
// holds it, and its forward reads as the same summary.
#[test]
fn the_report_and_its_forward_are_the_contracts() {
    let contract = contract();
    let summary: Summary = serde_json::from_value(contract["timing"]["report"]["timing"].clone()).unwrap();
    assert_eq!(report(&"r1::D:/wt/tw-1".into(), &summary), Some(contract["timing"]["report"].clone()));
    assert_eq!(forwarded(&contract["timing"]["forward"]), Some(Ok((Id::parse("0a1b-2c").unwrap(), summary.clone()))));
    let mut bent = contract["timing"]["forward"].clone();
    bent["timing"]["build"] = json!("not a sha");
    assert!(matches!(forwarded(&bent), Some(Err(_))));
    bent["timing"]["build"] = json!(null);
    bent["timing"]["path"] = json!("D:/wt");
    assert!(matches!(forwarded(&bent), Some(Err(_))));
    assert_eq!(forwarded(&contract["baseTip"]), None);
}

// Every kind, check and side, and the cap, are the contract's, which the relay holds to too: a
// variant added here fails the match below until the contract, and so the relay, has it.
#[test]
fn the_kinds_checks_and_cap_are_the_contracts() {
    let named = |value: Value| value.as_str().unwrap().to_string();
    let kinds = [Kind::Start, Kind::Resume, Kind::Round, Kind::Rebase].map(|k| match k {
        Kind::Start | Kind::Resume | Kind::Round | Kind::Rebase => named(json!(k)),
    });
    let checks = [CheckKind::CargoTest, CheckKind::Clippy, CheckKind::CargoCheck, CheckKind::NodeTest, CheckKind::Simulator, CheckKind::Typecheck, CheckKind::Gate].map(|c| match c {
        CheckKind::CargoTest | CheckKind::Clippy | CheckKind::CargoCheck | CheckKind::NodeTest | CheckKind::Simulator | CheckKind::Typecheck | CheckKind::Gate => named(json!(c)),
    });
    let by = [By::Main, By::SubAgent].map(|b| match b {
        By::Main | By::SubAgent => named(json!(b)),
    });
    let contract = &contract()["timing"];
    assert_eq!((json!(kinds), json!(checks), json!(by), json!(MOST_CHECKS)), (contract["kinds"].clone(), contract["checks"].clone(), contract["by"].clone(), contract["mostChecks"].clone()));
    assert!(KINDS.iter().map(|(_, name)| *name).eq(kinds.iter().map(String::as_str)));
}

fn line(key: &str, at: &str, wall: Option<u64>, checks: usize) -> Value {
    let mut line = json!({ "line": "session", "key": key, "at": at });
    if let Some(wall) = wall {
        line["wall"] = json!(wall);
        line["checks"] = json!(vec![json!({}); checks]);
    }
    line
}

const NOW: &str = "2026-10-24T01:00:00.000Z";

#[test]
fn a_line_is_written_once_and_a_session_again_only_fuller() {
    let now = instant(NOW).unwrap();
    let session = |wall, checks| (line("a", NOW, Some(wall), checks), (wall, checks));
    let kept = |text: &str, (line, full): (Value, (u64, usize))| kept_in(text, &line, full, now);
    let first = kept("", session(60, 1)).unwrap();
    assert_eq!(first.lines().count(), 1);
    assert_eq!(kept(&first, session(60, 1)), None);
    assert_eq!(kept(&first, session(30, 5)), None);
    // As long, with a sub-agent's check run the earlier report hadn't seen.
    let more = kept(&first, session(60, 2)).unwrap();
    assert_eq!(more, format!("{}\n", line("a", NOW, Some(60), 2)));
    let fuller = kept(&more, session(90, 0)).unwrap();
    assert_eq!(fuller, format!("{}\n", line("a", NOW, Some(90), 0)));
    let fact = json!({ "line": "merge", "key": "b", "at": NOW });
    let both = kept_in(&fuller, &fact, (0, 0), now).unwrap();
    assert_eq!(both.lines().count(), 2);
    assert_eq!(kept_in(&both, &fact, (0, 0), now), None);
}

#[test]
fn a_line_past_the_window_is_dropped_as_the_file_is_written() {
    let now = instant(NOW).unwrap();
    let old = format!("{}\nnot json\n", line("old", &iso(now - WINDOW_MS - 1), None, 0));
    assert_eq!(kept_in(&old, &line("new", NOW, None, 0), (0, 0), now), Some(format!("{}\n", line("new", NOW, None, 0))));
    assert_eq!(kept_in(&old, &line("late", &iso(now - WINDOW_MS - 1), None, 0), (0, 0), now), Some(String::new()));
    assert_eq!(kept_in("", &line("late", &iso(now - WINDOW_MS - 1), None, 0), (0, 0), now), None);
}

#[test]
fn listen_writes_each_session_start_and_merge_once() {
    let mut summary: Summary = serde_json::from_value(contract()["timing"]["report"]["timing"].clone()).unwrap();
    summary.at = iso(now_ms());
    Timing::default().session(&Id::parse("0a1b-2c").unwrap(), &summary);
    let dir = std::env::temp_dir().join(format!("tolkster-timing-{}", std::process::id()));
    let file = dir.join("timing.jsonl");
    let timing = Timing::at(file.clone());
    let merged_at = now_ms() - 60_000;
    for _ in 0..2 {
        timing.session(&Id::parse("0a1b-2c").unwrap(), &summary);
        timing.started("0a1b-2c", "TW-1", "c-2", Some(Size::M), "opus-high");
        // Heard of late, then read at GitHub's time, which replaces it; heard of again, kept.
        timing.merged("0a1b-2c", "TW-1", Some(7), now_ms(), false);
        timing.merged("0a1b-2c", "TW-1", Some(7), merged_at, true);
        timing.merged("0a1b-2c", "TW-1", Some(7), now_ms(), false);
        timing.merged("0a1b-2d", "TW-2", None, merged_at, false);
    }
    // The same session reported for another issue is that issue's line, not a replacement.
    timing.session(&Id::parse("0a1b-2e").unwrap(), &Summary { wall: summary.wall + 1, ..summary.clone() });
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let kept: Vec<Value> = text.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(kept.iter().map(|l| l["line"].as_str().unwrap()).collect::<Vec<_>>(), ["session", "start", "merge", "merge", "session"]);
    assert_eq!((kept[0]["issueId"].as_str(), kept[0]["wall"].as_u64()), (Some("0a1b-2c"), Some(summary.wall)));
    assert_eq!((kept[1]["size"].as_str(), kept[1]["rung"].as_str()), (Some("M"), Some("opus-high")));
    assert_eq!((kept[2]["pr"].as_u64(), kept[2]["at"].as_str()), (Some(7), Some(iso(merged_at).as_str())));
    assert_eq!((kept[3]["pr"].clone(), kept[4]["issueId"].as_str()), (Value::Null, Some("0a1b-2e")));
}

// The `timing` setting off: a report, a start and a merge write nothing, what is kept stays,
// and turning it on again writes as before.
#[test]
fn nothing_is_written_while_timing_is_off() {
    let mut summary: Summary = serde_json::from_value(contract()["timing"]["report"]["timing"].clone()).unwrap();
    summary.at = iso(now_ms());
    let dir = std::env::temp_dir().join(format!("tolkster-timing-off-{}", std::process::id()));
    let file = dir.join("timing.jsonl");
    let timing = Timing::at(file.clone());
    let issue = Id::parse("0a1b-2c").unwrap();
    timing.session(&issue, &summary);
    timing.allow(false);
    timing.session(&issue, &Summary { wall: summary.wall + 1, ..summary.clone() });
    timing.started("0a1b-2c", "TW-1", "c-2", None, "opus-high");
    timing.merged("0a1b-2c", "TW-1", Some(7), now_ms(), false);
    let kept = std::fs::read_to_string(&file).unwrap();
    assert_eq!(kept.lines().count(), 1);
    assert_eq!(serde_json::from_str::<Value>(kept.lines().next().unwrap()).unwrap()["wall"].as_u64(), Some(summary.wall));
    timing.allow(true);
    timing.started("0a1b-2c", "TW-1", "c-2", None, "opus-high");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(text.lines().count(), 2);
}
