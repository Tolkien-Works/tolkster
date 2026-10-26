use serde_json::json;

use super::*;
use crate::estimates::{Kind, Scale};

fn labels(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

// The rung, counted from 1, `choose` picks for an estimate on the T-shirt scale, and
// whether it logged anything.
fn pick(ladder: &Ladder, estimate: Option<f64>, names: &[&str]) -> (usize, bool) {
    let t_shirt = Scale { kind: Kind::TShirt, extended: false, zero: true };
    let (rung, notes) = ladder.choose(estimate.and_then(|p| t_shirt.size(p)), &labels(names));
    (ladder.rungs.iter().position(|r| r == rung).unwrap() + 1, !notes.is_empty())
}

#[test]
fn the_estimate_picks_the_rung_and_no_estimate_the_default() {
    let ladder = Ladder::default();
    assert_eq!(pick(&ladder, Some(0.0), &[]), (1, false));
    assert_eq!(pick(&ladder, Some(1.0), &[]), (1, false));
    assert_eq!(pick(&ladder, Some(2.0), &[]), (2, false));
    assert_eq!(pick(&ladder, Some(3.0), &[]), (3, false));
    assert_eq!(pick(&ladder, Some(5.0), &[]), (4, false));
    assert_eq!(pick(&ladder, Some(8.0), &[]), (4, false));
    assert_eq!(pick(&ladder, None, &[]), (3, false));
    // Priority, and labels that aren't the ladder's, play no part.
    assert_eq!(pick(&ladder, Some(1.0), &["Feature", "plan-first", "urgent"]), (1, false));
}

#[test]
fn labels_move_within_the_ladder_to_the_nearest_matching_rung() {
    let ladder = Ladder::default();
    // A big issue, held to Sonnet: its strongest rung.
    assert_eq!(pick(&ladder, Some(8.0), &["model:sonnet"]), (2, false));
    assert_eq!(pick(&ladder, Some(8.0), &["Model: Sonnet 5.5"]), (2, false));
    assert_eq!(pick(&ladder, Some(8.0), &["model:claude-sonnet-5-5"]), (2, false));
    // A small one, raised to Opus: its weakest.
    assert_eq!(pick(&ladder, Some(1.0), &["model:opus"]), (3, false));
    // Effort alone, nearest the estimate's rung, the stronger on a tie.
    assert_eq!(pick(&ladder, Some(1.0), &["effort:high"]), (2, false));
    assert_eq!(pick(&ladder, Some(8.0), &["effort:medium"]), (3, false));
    assert_eq!(pick(&ladder, Some(3.0), &["effort:high"]), (4, false));
    // Both.
    assert_eq!(pick(&ladder, Some(8.0), &["model:sonnet", "effort:medium"]), (1, false));
}

#[test]
fn a_label_off_the_ladder_gives_the_default_and_is_logged_without_its_text() {
    let ladder = Ladder::default();
    for off in [&["model:gpt-9"][..], &["effort:max"], &["effort:hard"], &["model:sonnet", "model:gpt-9"]] {
        assert_eq!(pick(&ladder, Some(1.0), off), (3, true), "{off:?}");
    }
    // Each on the ladder, but no rung has both.
    let single = Ladder { rungs: vec![Ladder::default().rungs[0].clone(), Ladder::default().rungs[3].clone()], default: 2 };
    assert_eq!(pick(&single, Some(1.0), &["model:sonnet", "effort:high"]), (2, true));
    let (_, notes) = ladder.choose(Some(Size::XS), &labels(&["model:$(rm -rf)"]));
    assert_eq!(notes, [RungNote::Unmatched("model")]);
}

#[test]
fn an_agent_label_other_than_claude_is_logged_and_ignored() {
    let ladder = Ladder::default();
    assert_eq!(pick(&ladder, Some(2.0), &["agent:codex"]), (2, true));
    assert_eq!(pick(&ladder, Some(2.0), &["agent:claude"]), (2, false));
}

#[test]
fn the_ladder_is_read_from_config_and_checked() {
    assert_eq!(Ladder::from_config(None).unwrap(), Ladder::default());
    let two = json!({ "rungs": [
        { "name": "Haiku", "model": "claude-haiku-4-5-20251001", "effort": "low", "upTo": "S" },
        { "name": "Opus", "model": "opus", "effort": "HIGH" },
    ], "default": 2 });
    let ladder = Ladder::from_config(Some(&two)).unwrap();
    assert_eq!(ladder.default, 2);
    assert_eq!(ladder.rungs[1].launch, Launch::new(crate::harnesses::DEFAULT, "opus", "high"));
    assert_eq!(ladder.rungs[1].up_to, None);
    assert_eq!(pick(&ladder, Some(3.0), &[]), (2, false));
    let written_as_floats = json!({ "rungs": [
        { "name": "Haiku", "model": "claude-haiku-4-5-20251001", "effort": "low", "upTo": "s" },
        { "name": "Opus", "model": "opus", "effort": "high" },
    ], "default": 2.0 });
    assert_eq!(Ladder::from_config(Some(&written_as_floats)).unwrap(), ladder);
    let rung = |fields: Value| json!({ "rungs": [fields], "default": 1 });
    for bad in [
        json!({ "rungs": [], "default": 1 }),
        json!({ "rungs": two["rungs"].clone() }),
        json!({ "rungs": two["rungs"].clone(), "default": 3 }),
        json!({ "rungs": two["rungs"].clone(), "default": 0 }),
        rung(json!({ "name": "A", "model": "--dangerously-skip-permissions", "effort": "low" })),
        rung(json!({ "name": "A", "model": "opus $(x)", "effort": "low" })),
        rung(json!({ "name": "A", "model": "opus\"", "effort": "low" })),
        rung(json!({ "name": "A", "model": "opus", "effort": "extreme" })),
        rung(json!({ "name": "A`b", "model": "opus", "effort": "low" })),
        rung(json!({ "model": "opus", "effort": "low" })),
        rung(json!({ "name": "A", "model": "opus", "effort": "low", "upTo": -1 })),
        // Never points: a size means the same on every team's scale.
        rung(json!({ "name": "A", "model": "opus", "effort": "low", "upTo": 2 })),
        rung(json!({ "name": "A", "model": "opus", "effort": "low", "upTo": "S", "upto": "M" })),
        // Out of order, or open before the last: a later rung could never be reached.
        json!({ "rungs": [{ "name": "A", "model": "sonnet", "effort": "low", "upTo": "L" }, { "name": "B", "model": "opus", "effort": "low", "upTo": "S" }], "default": 1 }),
        json!({ "rungs": [{ "name": "A", "model": "sonnet", "effort": "low", "upTo": "S" }, { "name": "B", "model": "opus", "effort": "low", "upTo": "s" }], "default": 1 }),
        json!({ "rungs": two["rungs"].clone(), "default": 1, "fallback": 2 }),
        json!({ "rungs": [{ "name": "A", "model": "sonnet", "effort": "low" }, { "name": "B", "model": "opus", "effort": "low" }], "default": 1 }),
    ] {
        assert!(Ladder::from_config(Some(&bad)).is_err(), "{bad}");
    }
}

#[test]
fn a_launch_is_plain_flags_and_a_rung_says_what_it_runs_as() {
    let rung = &Ladder::default().rungs[1];
    assert_eq!(rung.launch.flags(), "--model claude-sonnet-5-5 --effort high");
    assert_eq!(rung.running_as(), "Sonnet 5.5, high effort");
    assert_eq!(serde_json::to_value(&rung.launch).unwrap(), json!({ "harness": "claude", "model": "claude-sonnet-5-5", "effort": "high" }));
    // A record written before launches had a harness is the default's.
    assert_eq!(serde_json::from_value::<Launch>(json!({ "model": "claude-sonnet-5-5", "effort": "high" })).unwrap(), rung.launch);
    let stub = Launch::new(&crate::harnesses::tests::STUB, "stub-large", "deep");
    assert_eq!(stub.flags(), "--harness stub --model stub-large --effort deep");
}

// A rung on another harness names its model and effort as that harness takes them, and an
// `agent:` label keeps to the rungs on the harness it names.
#[test]
fn a_rung_names_its_harness_and_an_agent_label_keeps_to_it() {
    let two = json!({ "rungs": [
        { "name": "Sonnet", "model": "sonnet", "effort": "high", "upTo": "S" },
        { "name": "Stub", "harness": "stub", "model": "stub-large", "effort": "Deep", "upTo": "M" },
        { "name": "Opus", "model": "opus", "effort": "high" },
    ], "default": 3 });
    let ladder = Ladder::from_config(Some(&two)).unwrap();
    assert_eq!(ladder.rungs[1].launch, Launch::new(&crate::harnesses::tests::STUB, "stub-large", "deep"));
    assert_eq!(ladder.rungs[0].launch.harness, "claude");
    // A harness that can't run unattended safely names no rung.
    let bare = json!({ "rungs": [{ "name": "Bare", "harness": "bare", "model": "m", "effort": "deep" }] });
    assert!(Ladder::from_config(Some(&bare)).unwrap_err().to_string().contains("can't run unattended safely"));
    assert_eq!(pick(&ladder, Some(1.0), &["agent:stub"]), (2, false));
    assert_eq!(pick(&ladder, Some(8.0), &["Agent: Claude"]), (3, false));
    assert_eq!(pick(&ladder, Some(8.0), &["agent:stub", "effort:deep"]), (2, false));
    // No rung on both: the default.
    assert_eq!(pick(&ladder, Some(1.0), &["agent:stub", "model:sonnet"]), (3, true));
    // A harness this build doesn't launch, or an effort another harness's, is refused.
    for bad in [json!({ "name": "A", "harness": "codex", "model": "gpt", "effort": "high" }), json!({ "name": "A", "harness": "stub", "model": "stub", "effort": "high" }), json!({ "name": "A", "model": "opus", "effort": "deep" })] {
        assert!(Ladder::from_config(Some(&json!({ "rungs": [bad], "default": 1 }))).is_err(), "{bad}");
    }
}

#[test]
fn up_to_is_a_size_by_name() {
    let sizes = json!({ "rungs": [
        { "name": "Sonnet", "model": "sonnet", "effort": "medium", "upTo": "s" },
        { "name": "Opus", "model": "opus", "effort": "medium", "upTo": "L" },
        { "name": "Opus", "model": "opus", "effort": "high" },
    ], "default": 2 });
    let ladder = Ladder::from_config(Some(&sizes)).unwrap();
    assert_eq!(ladder.rungs.iter().map(|r| r.up_to).collect::<Vec<_>>(), vec![Some(Size::S), Some(Size::L), None]);
    // The same size picks the same rung on every team's scale: L is 8 points on an
    // exponential one, 4 on a linear one.
    let on = |kind: Kind, points: f64| ladder.choose(Scale { kind, extended: false, zero: false }.size(points), &[]).0.launch.effort.as_str();
    assert_eq!((on(Kind::Exponential, 8.0), on(Kind::Exponential, 16.0)), ("medium", "high"));
    assert_eq!((on(Kind::Linear, 4.0), on(Kind::Linear, 5.0)), ("medium", "high"));
    assert_eq!(ladder.choose(None, &[]).0, &ladder.rungs[1]);
    // Nothing is above XXXL, so a rung below the last can't take it.
    let unreachable = json!({ "rungs": [
        { "name": "A", "model": "sonnet", "effort": "low", "upTo": "XXXL" },
        { "name": "B", "model": "opus", "effort": "high" },
    ], "default": 1 });
    assert!(Ladder::from_config(Some(&unreachable)).is_err());
    assert!(Ladder::from_config(Some(&json!({ "rungs": [{ "name": "A", "model": "opus", "effort": "low", "upTo": "huge" }], "default": 1 }))).is_err());
}
