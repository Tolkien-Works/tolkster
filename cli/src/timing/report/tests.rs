use serde_json::json;

use super::*;

// Issue A plans first, waits 1000s on the owner, resumes, opens its PR, takes a round and merges,
// then is started again; B takes one session and hasn't merged; C started before the window; D
// is an XS.
const FIXTURE: &str = include_str!("fixture.jsonl");

fn window() -> Vec<Run> {
    runs(FIXTURE, instant("2026-10-15T00:00:00Z").unwrap(), instant("2026-10-23T00:00:00Z").unwrap())
}

#[test]
fn each_issue_counts_its_first_run_in_the_window_once() {
    let all = window();
    assert_eq!(all.len(), 3);
    let a = &all[0];
    assert_eq!((a.size, a.rung.as_deref(), a.sessions.len()), (Some(Size::M), Some("Opus 5.5"), 3));
    assert_eq!(a.merged, instant("2026-10-20T01:40:00Z"));
    assert!(runs(FIXTURE, instant("2026-10-15T00:00:00Z").unwrap(), instant("2026-10-20T00:00:00Z").unwrap()).is_empty());
}

#[test]
fn the_report_gives_per_issue_medians_with_the_owners_wait_apart() {
    let groups = report(&window(), Group::Size);
    assert_eq!(groups.iter().map(|g| g["group"].clone()).collect::<Vec<_>>(), [json!("XS"), json!("M")]);
    let m = &groups[1];
    assert_eq!(m["n"], json!(2));
    // A's start to PR is 2203s, 1000 of them waiting on the owner: 1203; B's is 701.
    assert_eq!(m["elapsed"], json!({ "startToPr": 952.0, "prToMerge": 3797.0, "startToMerge": 5000.0 }));
    assert_eq!(m["phases"], json!({ "orienting": 227.0, "implementing": 475.0, "reviewAndGate": 250.0, "deepReviewShare": 0.667 }));
    assert_eq!((m["active"].clone(), m["ownerWait"].clone()), (json!(1000.0), json!(500.0)));
    assert_eq!((m["shares"]["model"].clone(), m["shares"]["check"].clone()), (json!(0.438), json!(0.275)));
    // A's rerun, with its 9 check runs and 99 tool turns, counts for nothing.
    assert_eq!((m["toolTurns"].clone(), m["checkRuns"].clone()), (json!(40.0), json!(4.0)));
    let xs = &groups[0];
    assert_eq!(xs["elapsed"], json!({ "startToPr": 301.0, "prToMerge": 100.0, "startToMerge": 401.0 }));
    assert_eq!((xs["phases"]["implementing"].clone(), xs["ownerWait"].clone()), (Value::Null, json!(0.0)));
    let models = report(&window(), Group::Model);
    assert_eq!(models.iter().map(|g| (g["group"].clone(), g["n"].clone())).collect::<Vec<_>>(), [(json!("Opus 5.5"), json!(2)), (json!("Sonnet 5.5"), json!(1))]);
}

#[test]
fn checks_split_by_kind_side_phase_outcome_and_edit() {
    let m = &checks(&window(), Group::Size)[1];
    assert_eq!(m["checkRuns"], json!(4.0));
    assert_eq!(m["byKind"], json!({ "cargoTest": 1.5, "clippy": 1.0, "cargoCheck": 0.0, "nodeTest": 0.5, "simulator": 0.0, "typecheck": 0.0, "gate": 1.0 }));
    assert_eq!(m["by"], json!({ "main": 3.5, "subAgent": 0.5 }));
    // A: three before its gate, the gate and a sub-agent's clippy in review, the round's after.
    assert_eq!(m["byPhase"], json!({ "beforeReview": 2.0, "review": 1.5, "after": 0.5 }));
    assert_eq!(m["byOutcome"], json!({ "failed": 0.5, "passed": 3.0, "unknown": 0.5 }));
    // A's second cargo test follows its failed first.
    assert_eq!(m["byEdit"], json!({ "noEdit": 2.0, "afterEdit": 1.5, "fixAfterFailure": 0.5 }));
    assert_eq!((m["minutes"]["total"].clone(), m["minutes"]["gate"].clone(), m["minutes"]["nodeTest"].clone()), (json!(5.7), json!(3.5), json!(0.2)));
}

// A's lines with its first session told twice under one launch, as a cleared context does, a
// round between its plan and the owner's reply, and a line whose build isn't a plain one.
#[test]
fn a_twice_told_launch_a_round_before_a_resume_and_a_bent_line() {
    let lines: Vec<serde_json::Value> = FIXTURE.lines().filter_map(|l| serde_json::from_str(l).ok()).filter(|l: &serde_json::Value| l["issueId"] == "issue-a").collect();
    let mut told = lines[1].clone();
    told["session"] = json!("s-again");
    told["key"] = json!("k-again");
    let mut round = lines[3].clone();
    round["launched"] = json!("2026-10-20T00:10:00.000Z");
    round["at"] = json!("2026-10-20T00:10:01.000Z");
    let mut bent = lines[2].clone();
    bent["build"] = json!("\u{1b}[31m");
    let text = |extra: &[&serde_json::Value]| lines.iter().chain(extra.iter().copied()).map(|l| format!("{l}\n")).collect::<String>();
    let since = instant("2026-10-15T00:00:00Z").unwrap();
    let until = instant("2026-10-23T00:00:00Z").unwrap();
    let a = &report(&runs(&text(&[&told]), since, until), Group::Size)[0];
    assert_eq!((a["elapsed"]["startToPr"].clone(), a["checkRuns"].clone()), (json!(1203.0), json!(6.0)));
    // The round ends at 00:13:21, so the owner waited from then, not from the plan's end.
    let a = &report(&runs(&text(&[&round]), since, until), Group::Size)[0];
    assert_eq!(a["ownerWait"], json!(501.0));
    let mut fixed = lines.clone();
    fixed[2] = bent;
    let bent_text: String = fixed.iter().map(|l| format!("{l}\n")).collect();
    assert_eq!(runs(&bent_text, since, until)[0].sessions.len(), 2);
}

// A's worker launches ten minutes after its start line, as a rebuild first makes it: the run is
// still its own, from its line: A's start to PR is 1803s, B's 701.
#[test]
fn a_launch_long_after_its_start_line_stays_in_its_run() {
    let lines: Vec<serde_json::Value> = FIXTURE.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    let text: String = lines.into_iter().map(|mut l| {
        if l["key"] == "k1" {
            l["at"] = json!("2026-10-19T23:50:00.000Z");
        }
        format!("{l}\n")
    }).collect();
    let all = runs(&text, instant("2026-10-15T00:00:00Z").unwrap(), instant("2026-10-23T00:00:00Z").unwrap());
    assert_eq!(all[0].sessions.len(), 3);
    assert_eq!(report(&all, Group::Size)[1]["elapsed"]["startToPr"], json!(1252.0));
}
