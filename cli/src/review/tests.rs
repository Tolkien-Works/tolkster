use std::path::Path;

use super::*;
use crate::github::{COPILOT, Comment, Review, Thread};
use crate::ports::fake::FakeReader;
use crate::time::{ms, t};
use serde_json::{Value, json};

const REVIEWER: &str = "a-reviewer";
// The GitHub App's bot, which opens the PR and replies on its threads.
const BOT: &str = "tolkster-agent";
const HEAD: &str = "2878d3d76e19f5ab3b99e754850b711b9a6eef77";

fn blessed() -> Vec<String> {
    vec![REVIEWER.into(), COPILOT.into()]
}

fn pr() -> Pr {
    Pr { number: 7, repo: "Tolkien-Works/tolkster".into(), head: HEAD.into(), merge_state: "CLEAN".into(), base: "main".into(), base_head: String::new(), base_at: Some(ms(0)), bot_author: None, checks_green: false, auto_merge_off: false, auto_merge_on: false, reviews: vec![], threads: vec![], comments: vec![] }
}

fn thread(author: &str, minute: u32) -> Thread {
    Thread { resolved: false, outdated: false, author: Some(author.into()), at: Some(ms(minute)) }
}

fn a_review(author: &str, bot: bool, state: &str, has_body: bool, minute: u32) -> Review {
    Review { author: Some(author.into()), bot, state: state.into(), at: Some(ms(minute)), has_body, commit: None }
}


#[test]
fn the_newest_feedback_comes_with_the_newest_reviewers() {
    let with = |threads: Vec<Thread>| newest_feedback_and_reviewers(&Pr { threads, ..pr() }, &blessed());
    assert_eq!(with(vec![]), None);
    assert_eq!(with(vec![thread(COPILOT, 3)]), Some((ms(3), None)));
    assert_eq!(with(vec![thread(REVIEWER, 2), thread(COPILOT, 3)]), Some((ms(3), Some(ms(2)))));
    assert_eq!(with(vec![thread(COPILOT, 2), thread(REVIEWER, 3)]), Some((ms(3), Some(ms(3)))));
    assert_eq!(with(vec![thread(COPILOT, 3), thread(REVIEWER, 3)]), Some((ms(3), Some(ms(3)))));
}

#[test]
fn the_apps_own_replies_never_count() {
    // Even listed as a reviewer, the bot that opened the PR is the worker replying.
    let listed = vec![REVIEWER.to_string(), COPILOT.to_string(), BOT.to_string()];
    let opened = |t: Thread| Pr { bot_author: Some(BOT.into()), threads: vec![t], comments: vec![Comment { author: Some("Tolkster-Agent".into()), at: Some(ms(11)) }], ..pr() };
    assert_eq!(newest_feedback(&opened(thread(BOT, 10)), &listed), None);
    // Copilot's findings on it still count.
    assert_eq!(newest_feedback(&opened(thread(COPILOT, 10)), &listed), Some(ms(10)));
    // On a PR the owner opened, the bot counts only if it is listed.
    let by_owner = |t: Thread| Pr { threads: vec![t], ..pr() };
    assert_eq!(newest_feedback(&by_owner(thread(BOT, 10)), &blessed()), None);
}

#[test]
fn a_bots_review_counts_only_through_its_threads() {
    // Copilot's overview, and its "unable to review" note, both COMMENTED with a body.
    let overview = Pr { reviews: vec![a_review(COPILOT, true, "COMMENTED", true, 10)], ..pr() };
    assert_eq!(newest_feedback(&overview, &blessed()), None);
    let people = |state, has_body| Pr { reviews: vec![a_review(REVIEWER, false, state, has_body, 10)], ..pr() };
    assert_eq!(newest_feedback(&people("CHANGES_REQUESTED", false), &blessed()), Some(ms(10)));
    assert_eq!(newest_feedback(&people("COMMENTED", true), &blessed()), Some(ms(10)));
    assert_eq!(newest_feedback(&people("COMMENTED", false), &blessed()), None);
    assert_eq!(newest_feedback(&people("APPROVED", true), &blessed()), None);
    let stranger = Pr { reviews: vec![a_review("stranger", false, "CHANGES_REQUESTED", true, 10)], ..pr() };
    assert_eq!(newest_feedback(&stranger, &blessed()), None);
}

#[test]
fn copilot_wont_approve_a_head_it_only_commented_on() {
    let at = |author: &str, state: &str, commit: &str, minute| Review { commit: Some(commit.into()), ..a_review(author, true, state, true, minute) };
    let held = Pr { merge_state: "BLOCKED".into(), ..pr() };
    let with = |reviews: Vec<Review>, threads: Vec<Thread>| copilot_wont_approve(&Pr { reviews, threads, ..held.clone() }, Checked::default());
    let commented = at(COPILOT, "COMMENTED", HEAD, 5);
    assert!(with(vec![commented.clone()], vec![]));
    assert!(!with(vec![], vec![]));
    // A review of an earlier head, or Copilot's newest being an approval, leaves nothing to say.
    assert!(!with(vec![at(COPILOT, "COMMENTED", "abc", 5)], vec![]));
    assert!(!with(vec![commented.clone(), at(COPILOT, "APPROVED", HEAD, 6)], vec![]));
    assert!(with(vec![at(COPILOT, "APPROVED", "abc", 4), commented.clone()], vec![]));
    // A person's approval of the head, or a thread still open, means the merge isn't waiting on one.
    assert!(!with(vec![commented.clone(), at(REVIEWER, "APPROVED", HEAD, 6)], vec![]));
    assert!(!with(vec![commented.clone()], vec![thread(COPILOT, 6)]));
    assert!(with(vec![commented.clone()], vec![Thread { resolved: true, ..thread(COPILOT, 6) }, Thread { outdated: true, ..thread(COPILOT, 6) }]));
    assert!(!with(vec![at(REVIEWER, "COMMENTED", HEAD, 5)], vec![]));
    // A merge GitHub doesn't hold, or checks still to pass, aren't waiting on a person.
    let reviews = vec![commented.clone()];
    assert!(!copilot_wont_approve(&Pr { reviews: reviews.clone(), ..pr() }, Checked::default()));
    for checked in [Checked { pending: true, ..Checked::default() }, Checked { failed: true, ..Checked::default() }, Checked { unran: true, ..Checked::default() }] {
        assert!(!copilot_wont_approve(&Pr { reviews: reviews.clone(), ..held.clone() }, checked));
    }
}

#[test]
fn a_blessed_conversation_comment_counts() {
    let commented = |author: &str, minute| Pr { comments: vec![Comment { author: Some(author.into()), at: Some(ms(minute)) }], ..pr() };
    assert_eq!(newest_feedback(&commented(REVIEWER, 10), &blessed()), Some(ms(10)));
    assert_eq!(newest_feedback(&commented("stranger", 10), &blessed()), None);
}

#[test]
fn the_pickup_and_the_line_are_safe_to_type() {
    assert_eq!(picked_up(7, Some("2878d3d"), Round::Review { last: false }), "review feedback, failing checks or a rebase on PR #7 at 2878d3d");
    assert_eq!(picked_up(7, None, Round::Review { last: true }), "review feedback, failing checks or a rebase on PR #7, its last round");
    assert_eq!(picked_up(7, Some("2878d3d"), Round::Rebase), "a rebase on PR #7 at 2878d3d");
    let rebase = line(7, &"TW-1".into(), &"s1".into(), Round::Rebase).unwrap();
    assert_eq!(rebase.as_str(), "PR #7 needs a rebase. Read the file TW-1-rebase.md beside your task brief in full, then rebase it as it says. Report in Linear session s1.");
    for last in [false, true] {
        let l = line(7, &"TW-1".into(), &"s1".into(), Round::Review { last }).unwrap();
        // It names the round brief written beside the work brief.
        assert!(l.starts_with("PR #7 has new review feedback, failing checks or needs a rebase. Read the file TW-1-round.md beside your task brief in full, then address them as it says, reading them with tolkster pr feedback --pr 7 --session s1 and tolkster pr checks --pr 7."), "{l}");
        assert_eq!(l.contains("This is the last review round: fix only blocking findings and file the rest, as the file TW-1-last-round.md beside your task brief says."), last);
        assert!(l.ends_with(" Report in Linear session s1."));
        assert!(!l.contains(['`', '"', '$', '\'', '\n']));
    }
}

#[test]
fn a_round_line_is_built_only_for_a_plain_identifier() {
    // `split_identifier` takes these, since a `u64` parse accepts a `+` and the key's shape is loose.
    for bad in ["TW-+5", "1W-1", "TW-1234567890"] {
        assert!(crate::plain::split_identifier(bad).is_some(), "{bad}");
        assert!(crate::plain::IssueKey::parse(bad).is_none(), "{bad}");
    }
}

#[test]
fn a_last_rounds_fresh_line_says_so() {
    let fresh_round = fresh(7, &"TW-1".into(), Path::new("/b/TW-1.md"), Path::new("/b/TW-1-round.md"), &"s1".into(), Round::Review { last: true });
    assert!(fresh_round.contains(&format!("This is the last review round: {}", last_round("TW-1"))), "{fresh_round}");
    crate::plain::Line::check(&fresh_round).unwrap();
    // A rebase's names its own brief and asks for no feedback read.
    let rebase = fresh(7, &"TW-1".into(), Path::new("/b/TW-1.md"), Path::new("/b/TW-1-rebase.md"), &"s1".into(), Round::Rebase);
    assert!(rebase.starts_with("Read the file /b/TW-1-rebase.md in full. It is your brief for rebasing PR #7,") && !rebase.contains("pr feedback"), "{rebase}");
    crate::plain::Line::check(&rebase).unwrap();
}

// One page of the head's checks, as github::head_checks reads them, with the PR's head at `oid`.
fn checks_page(oid: &str, contexts: Vec<Value>, more: bool) -> Value {
    let rollup = json!({ "contexts": { "nodes": contexts, "pageInfo": { "hasNextPage": more, "endCursor": "p1" } } });
    json!({ "repository": { "pullRequest": { "headRefOid": oid, "headRefName": "tw-1", "state": "OPEN", "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN", "commits": { "nodes": [{ "commit": { "oid": oid, "statusCheckRollup": rollup } }] } } } })
}

// Another app's check run, as the checks query answers with it.
fn run_check(conclusion: &str, minute: u32, required: bool) -> Value {
    json!({ "__typename": "CheckRun", "name": "ci", "status": "COMPLETED", "conclusion": conclusion, "detailsUrl": null, "completedAt": t(minute), "isRequired": required, "checkSuite": { "app": { "slug": "other-app" } } })
}

fn failed_after(marker: Option<i64>, contexts: Vec<Value>) -> bool {
    let mut gh = FakeReader::always(checks_page(HEAD, contexts, false));
    failed_checks(&mut gh, &pr(), marker).unwrap().failed
}

#[test]
fn a_required_check_that_failed_after_the_marker_counts() {
    for conclusion in ["FAILURE", "TIMED_OUT", "CANCELLED", "STARTUP_FAILURE"] {
        assert!(failed_after(Some(ms(5)), vec![run_check(conclusion, 10, true)]), "{conclusion}");
    }
    for conclusion in ["SUCCESS", "NEUTRAL", "SKIPPED", "ACTION_REQUIRED", "STALE"] {
        assert!(!failed_after(Some(ms(5)), vec![run_check(conclusion, 10, true)]), "{conclusion}");
    }
    // Not required, or failed before the worker's last round ended: it saw that one.
    assert!(!failed_after(Some(ms(5)), vec![run_check("FAILURE", 10, false)]));
    // Actions concluded the job without running it (no minutes, or billing): nothing to
    // resume the worker for. One with steps, or another app's, failed for real.
    let never_ran = |conclusion: &str, steps: u64, app: &str, note: Option<&str>| {
        let mut check = run_check(conclusion, 10, true);
        check["steps"] = json!({ "totalCount": steps });
        check["checkSuite"] = json!({ "app": { "slug": app } });
        check["annotations"] = json!({ "nodes": note.map(|m| vec![json!({ "message": m })]).unwrap_or_default() });
        check
    };
    const BILLING: &str = "The job was not started because recent account payments have failed or your spending limit needs to be increased.";
    assert!(!failed_after(Some(ms(5)), vec![never_ran("FAILURE", 0, "github-actions", Some(BILLING))]));
    assert!(!failed_after(Some(ms(5)), vec![never_ran("FAILURE", 0, "github-actions", None)]));
    assert!(!failed_after(Some(ms(5)), vec![never_ran("FAILURE", 3, "github-actions", Some(BILLING))]));
    assert!(failed_after(Some(ms(5)), vec![never_ran("FAILURE", 3, "github-actions", None)]));
    assert!(failed_after(Some(ms(5)), vec![never_ran("FAILURE", 0, "other-app", None)]));
    assert!(failed_after(Some(ms(5)), vec![never_ran("FAILURE", 0, "github-actions", None), run_check("FAILURE", 10, true)]));
    assert!(!failed_after(Some(ms(15)), vec![run_check("FAILURE", 10, true)]));
    // GitHub's times are to the second: a failure later in the marker's second counts.
    assert!(failed_after(Some(ms(10) + 500), vec![run_check("FAILURE", 10, true)]));
    assert!(!failed_after(Some(ms(10) + 1000), vec![run_check("FAILURE", 10, true)]));
    assert!(failed_after(None, vec![run_check("FAILURE", 10, true)]));
    // A conclusion GitHub adds later fails, as it does for `pr checks`.
    assert!(failed_after(None, vec![run_check("A_CONCLUSION_GITHUB_ADDS_LATER", 10, true)]));
    // Still running.
    let mut running = run_check("", 10, true);
    running["status"] = json!("IN_PROGRESS");
    running["conclusion"] = Value::Null;
    running["completedAt"] = Value::Null;
    assert!(!failed_after(None, vec![running]));
    // A commit status that failed or errored.
    let status = |state: &str, required: bool| json!({ "__typename": "StatusContext", "context": "ci/legacy", "state": state, "targetUrl": null, "createdAt": t(10), "isRequired": required });
    assert!(failed_after(Some(ms(5)), vec![status("FAILURE", true)]));
    assert!(failed_after(Some(ms(5)), vec![status("ERROR", true)]));
    assert!(!failed_after(Some(ms(5)), vec![status("ERROR", false)]));
    assert!(!failed_after(Some(ms(5)), vec![status("PENDING", true)]));
}

#[test]
fn a_failed_check_beyond_the_first_page_counts_and_a_moved_head_says_nothing() {
    let mut pages = 0;
    let mut gh = FakeReader::new(|_, variables| {
        pages += 1;
        Ok(if variables["after"].is_null() { checks_page(HEAD, vec![run_check("SUCCESS", 10, true)], true) } else { checks_page(HEAD, vec![run_check("FAILURE", 10, true)], false) })
    });
    assert!(failed_checks(&mut gh, &pr(), Some(ms(5))).unwrap().failed);
    drop(gh);
    assert_eq!(pages, 2);
    let mut moved = FakeReader::always(checks_page("a-newer-commit", vec![run_check("FAILURE", 10, true)], false));
    assert!(!failed_checks(&mut moved, &pr(), Some(ms(5))).unwrap().failed);
    // GitHub has moved the head while the rollup is still the one read, or a push lands
    // between pages after one that failed: the new head's checks decide, not these.
    let mut split = checks_page(HEAD, vec![run_check("FAILURE", 10, true)], false);
    split["repository"]["pullRequest"]["headRefOid"] = json!("a-newer-commit");
    assert!(!failed_checks(&mut FakeReader::always(split), &pr(), Some(ms(5))).unwrap().failed);
    let mut reads = 0;
    let mut pushed = FakeReader::new(|_, _| {
        reads += 1;
        Ok(if reads == 1 { checks_page(HEAD, vec![run_check("FAILURE", 10, true)], true) } else { checks_page("a-newer-commit", vec![], false) })
    });
    assert!(!failed_checks(&mut pushed, &pr(), Some(ms(5))).unwrap().failed);
}

// A check run Actions concluded without a step, or with `steps`, required or not.
fn actions_check(steps: u64, required: bool) -> Value {
    let mut check = run_check("FAILURE", 10, required);
    check["steps"] = json!({ "totalCount": steps });
    check["checkSuite"] = json!({ "app": { "slug": "github-actions" } });
    check["annotations"] = json!({ "nodes": [] });
    check
}


#[test]
fn a_never_ran_check_is_found_without_being_a_failure() {
    let checked = |contexts: Vec<Value>| failed_checks(&mut FakeReader::always(checks_page(HEAD, contexts, false)), &pr(), None).unwrap();
    assert_eq!(checked(vec![actions_check(0, true)]), Checked { failed: false, unran: true, pending: false });
    // A required check that ran and failed is a failure, not a CI that couldn't run.
    assert_eq!(checked(vec![actions_check(3, true)]), Checked { failed: true, unran: false, pending: false });
    // Both at once: the resume wins, as the failure is the worker's to fix.
    assert_eq!(checked(vec![actions_check(0, true), actions_check(3, true)]), Checked { failed: true, unran: true, pending: false });
    assert_eq!(checked(vec![actions_check(0, false)]), Checked::default());
    // One that waits on a person's action can't be fixed by a worker either: CI couldn't run.
    assert_eq!(checked(vec![run_check("ACTION_REQUIRED", 10, true)]), Checked { failed: false, unran: true, pending: false });
    assert_eq!(checked(vec![run_check("ACTION_REQUIRED", 10, false)]), Checked::default());
    // A skipped required job has no steps either, and counts as passing.
    let mut skipped = actions_check(0, true);
    skipped["conclusion"] = json!("SKIPPED");
    assert_eq!(checked(vec![skipped.clone(), actions_check(3, true)]), Checked { failed: true, unran: false, pending: false });
    // One still queued has no conclusion yet, so the checks are pending.
    let mut queued = actions_check(0, true);
    queued["status"] = json!("QUEUED");
    let pending = Checked { pending: true, ..Checked::default() };
    assert_eq!(checked(vec![queued.clone(), skipped]), pending);
    let mut optional = queued;
    optional["isRequired"] = json!(false);
    assert_eq!(checked(vec![optional]), Checked::default());
    // A head that has moved says nothing but that its checks are still to come.
    let mut moved = FakeReader::always(checks_page("a-newer-commit", vec![actions_check(0, true)], false));
    assert_eq!(failed_checks(&mut moved, &pr(), None).unwrap(), pending);
}

#[test]
fn a_green_rollup_is_passed_without_reading_the_checks() {
    let green = Pr { checks_green: true, ..pr() };
    let mut gh = FakeReader::always(checks_page(HEAD, vec![run_check("FAILURE", 10, true)], false));
    assert_eq!(failed_checks(&mut gh, &green, None).unwrap(), Checked::default());
    assert!(gh.queries.is_empty());
}
