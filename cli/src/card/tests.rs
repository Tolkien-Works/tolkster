use super::*;
use crate::config::ladder::Launch;

fn record(issue: &str, worktree_id: &str, launch: Option<Launch>) -> WorkerRecord {
    let launch = launch.unwrap_or(Launch::new(crate::harnesses::DEFAULT, "claude-opus-5-5", "medium"));
    WorkerRecord { issue: issue.into(), worktree_id: worktree_id.into(), session_id: "s1".into(), at: "2026-10-04T00:00:00.000Z".into(), launch, rung_from: crate::state::RungFrom::Claim }
}

#[test]
fn the_worker_is_the_innermost_worktree_holding_the_directory() {
    let workers = [record("TW-1", "r1::D:/wt/tw-1", None), record("TW-2", "r1::D:/wt/tw-1/nested", None), record("TW-3", "r1::D:/wt/tw-3", None)];
    assert_eq!(worker(&workers, None, r"d:\wt\tw-1\nested\cli").map(|(w, root)| (w.issue.as_str(), root)), Some(("TW-2", "D:/wt/tw-1/nested")));
    assert_eq!(worker(&workers, None, "D:/wt/tw-1").map(|(w, _)| w.issue.as_str()), Some("TW-1"));
    // The primary checkout, or a worktree whose name only starts with one's, is no worker's.
    assert!(worker(&workers, None, "D:/src/tolkster").is_none() && worker(&workers, None, "D:/wt/tw-10").is_none());
    // A record that isn't an issue's is never one.
    assert!(worker(&[record("not an issue", "r1::D:/wt/x", None)], None, "D:/wt/x").is_none());
}

// Two hosts' worktrees at the same path are each their own host's: this machine's has no
// host in its ID.
#[test]
fn a_host_finds_only_its_own_workers() {
    let workers = [record("TW-1", "r1::/srv/wt/tw-1", None), record("TW-2", "mac@9f0c::/srv/wt/tw-1", None), record("TW-3", "mac@9f0c::/Users/sam/wt/tw-3", None)];
    let on = |host, dir| worker(&workers, host, dir).map(|(w, _)| w.issue.as_str());
    assert_eq!((on(None, "/srv/wt/tw-1"), on(Some("mac"), "/srv/wt/tw-1"), on(Some("mac"), "/Users/sam/wt/tw-3/cli")), (Some("TW-1"), Some("TW-2"), Some("TW-3")));
    assert_eq!((on(None, "/Users/sam/wt/tw-3"), on(Some("studio"), "/srv/wt/tw-1")), (None, None));
}

// A worker's commands and hooks run for the worker their launch names; with either half
// missing or out of shape, for none, as in a conversation's turn or the owner's checkout.
#[test]
fn a_command_runs_for_the_worker_its_launch_names() {
    let launch = |record: &WorkerRecord| launch_env(record).map(|(_, value)| Some(value));
    let [worktree, issue] = launch(&record("TW-1", "mac@9f0c::/srv/wt/tw-1", None));
    assert_eq!(launched_of(worktree.clone(), issue.clone()), Some(Launched { worktree_id: "mac@9f0c::/srv/wt/tw-1".into(), root: "/srv/wt/tw-1".into(), issue: "TW-1".into() }));
    assert_eq!((launched_of(None, issue.clone()), launched_of(worktree.clone(), None)), (None, None));
    assert_eq!((launched_of(Some("not a worktree".into()), issue), launched_of(worktree, Some("not an issue".into()))), (None, None));
}

#[test]
fn a_pr_checkpoint_says_how_to_resume_as_the_worker_was_launched() {
    let launch = Launch::new(crate::harnesses::DEFAULT, "claude-opus-5-5", "medium");
    let worker = record("TW-1", "r1::D:/wt/tw-1", Some(launch));
    assert_eq!(pr(&worker, 7, "auto-merge on"), "PR #7, auto-merge on. Resume with: claude --continue --model claude-opus-5-5 --effort medium");
    // A launch that can't go on a command line leaves the resume out, never the checkpoint.
    let flagged = Launch::new(crate::harnesses::DEFAULT, "opus --dangerously-skip-permissions", "high");
    assert_eq!(pr(&record("TW-1", "r1::D:/wt/tw-1", Some(flagged)), 7, "merged"), "PR #7, merged.");
}


#[test]
fn an_action_leaves_a_prs_checkpoint_on_the_card() {
    assert!(holds_pr(Some("PR #7, auto-merge on. Resume with: claude --continue")));
    assert!(!holds_pr(Some("Committed: Add the flag")) && !holds_pr(None));
}

// The card's column is set on the host the worker's worktree is on, and a failing Orca moves
// nothing and panics nothing.
#[test]
fn a_card_moves_to_the_column_of_its_status_and_a_failure_is_only_said() {
    let worker = launched_of(Some("r1::/srv/wt/tw-1".into()), Some("TW-1".into())).unwrap();
    let mut calls = Vec::new();
    moved(&mut |program, args, _| { calls.push(format!("{program} {}", args.join(" "))); Ok(crate::exec::Output::ok("{}")) }, &worker, Status::InReview);
    assert_eq!(calls, ["orca worktree set --worktree id:r1::/srv/wt/tw-1 --workspace-status in-review --json"]);
    moved(&mut |_, _, _| Err(crate::error::Error::Failed("down".into())), &worker, Status::InReview);
}
