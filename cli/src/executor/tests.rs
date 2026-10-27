use super::*;
use crate::review_pass::ReviewSize;
use crate::estimates::{Kind as ScaleKind, Scale, Size};
use crate::request::tests::{at, issue};


#[test]
fn only_a_manual_merge_label_leaves_auto_merge_off() {
    let labelled = |labels: &[&str]| Issue { labels: labels.iter().map(|l| l.to_string()).collect(), ..issue() };
    assert!(auto_merges(&labelled(&[])));
    assert!(auto_merges(&labelled(&["Feature", "manual-merge-ish"])));
    assert!(!auto_merges(&labelled(&["Feature", "Manual-Merge"])));
}


#[test]
fn a_rung_picked_with_no_estimate_is_picked_again_once_the_issue_has_one() {
    let ladder = Ladder::from_config(None).unwrap();
    let default = ladder.choose(None, &[]).0.launch.clone();
    let small = ladder.choose(Some(Size::S), &[]).0.launch.clone();
    assert_ne!(default, small);
    let record = |rung_from| WorkerRecord { issue: "TW-1".into(), worktree_id: "r::D:/wt/tw-0".into(), session_id: "s1".into(), at: at(0), launch: default.clone(), rung_from };
    let sized = Issue { estimate: Some(2.0), ..issue() };
    let (launch, rerecord) = relaunch(Some(&record(RungFrom::NoEstimate)), &WorktreeId::from("r::D:/wt/tw-1"), &sized, &ladder);
    assert_eq!(launch, Some(small.clone()));
    let rerecord = rerecord.unwrap();
    assert_eq!((rerecord.worktree_id, rerecord.launch, rerecord.rung_from, rerecord.session_id), ("r::D:/wt/tw-1".into(), small, RungFrom::Worker, "s1".to_string()));
    // Still no estimate, one at the start, or the worker's already taken: the launch stands,
    // and nothing is recorded.
    assert_eq!(relaunch(Some(&record(RungFrom::NoEstimate)), &WorktreeId::from("r::w"), &issue(), &ladder), (Some(default.clone()), None));
    for rung_from in [RungFrom::Claim, RungFrom::Worker] {
        assert_eq!(relaunch(Some(&record(rung_from)), &WorktreeId::from("r::w"), &sized, &ladder), (Some(default.clone()), None));
    }
    assert_eq!(relaunch(None, &WorktreeId::from("r::w"), &sized, &ladder), (None, None));
    // A rung on another harness can't continue the session: the launch stands.
    let stub = serde_json::json!({ "rungs": [
        { "name": "Stub", "harness": "stub", "model": "stub-large", "effort": "deep", "upTo": "S" },
        { "name": "Opus", "model": "claude-opus-5-5", "effort": "medium" },
    ], "default": 2 });
    let ladder = Ladder::from_config(Some(&stub)).unwrap();
    let default = ladder.choose(None, &[]).0.launch.clone();
    let record = WorkerRecord { launch: default.clone(), ..record(RungFrom::NoEstimate) };
    assert_eq!(relaunch(Some(&record), &WorktreeId::from("r::w"), &sized, &ladder), (Some(default), None));
}

// The pre-push review's size is the issue's against `deepReviewEstimate`, and unsized
// whenever the start can't place it on a scale (TW-299).
#[test]
fn the_review_size_is_the_issues_against_the_deep_review_estimate() {
    let sized = |estimate: Option<f64>, scale: Option<Scale>| Issue { estimate, scale, ..issue() };
    let t_shirt = issue().scale;
    assert_eq!(review_size(&sized(Some(1.0), t_shirt), Size::M), ReviewSize::Below);
    assert_eq!(review_size(&sized(Some(2.0), t_shirt), Size::M), ReviewSize::Below);
    assert_eq!(review_size(&sized(Some(3.0), t_shirt), Size::M), ReviewSize::At);
    assert_eq!(review_size(&sized(Some(5.0), t_shirt), Size::M), ReviewSize::Above);
    assert_eq!(review_size(&sized(Some(5.0), t_shirt), Size::L), ReviewSize::At);
    assert_eq!(review_size(&sized(None, t_shirt), Size::M), ReviewSize::Unsized);
    let not_used = Some(Scale { kind: ScaleKind::NotUsed, extended: false, zero: false });
    assert_eq!(review_size(&sized(Some(1.0), not_used), Size::M), ReviewSize::Unsized);
    assert_eq!(review_size(&sized(Some(1.0), None), Size::M), ReviewSize::Unsized);
}

#[test]
fn only_labelled_work_plans_first_when_its_worker_starts() {
    let sized = |estimate: Option<f64>, labels: &[&str]| Issue { estimate, labels: labels.iter().map(|l| l.to_string()).collect(), ..issue() };
    assert!(!plans_first(&sized(None, &[])));
    assert!(!plans_first(&sized(Some(3.0), &["Feature"])));
    // However big, an unlabelled issue starts work.
    assert!(!plans_first(&sized(Some(8.0), &[])));
    assert!(!plans_first(&sized(Some(21.0), &[])));
    // The label counts whatever the estimate, ignoring case.
    assert!(plans_first(&sized(None, &["Plan-First"])));
    assert!(plans_first(&sized(Some(1.0), &["plan-first"])));
    assert!(!plans_first(&sized(None, &["plan-first-ish"])));
}
