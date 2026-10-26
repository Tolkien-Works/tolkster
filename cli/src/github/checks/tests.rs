use super::*;
use crate::ports::fake::FakeReader;

// A check run as the light checks query answers with it, every field it asks for, by the app
// `slug`: CHECKS_QUERY's steps and annotations only where a test adds them.
fn check_run(status: &str, conclusion: Value, slug: &str) -> Value {
    json!({ "__typename": "CheckRun", "name": "ci", "status": status, "conclusion": conclusion, "detailsUrl": "https://x", "completedAt": "2026-09-26T01:05:00Z", "isRequired": true, "checkSuite": { "app": { "slug": slug } } })
}

fn status_context(state: &str) -> Value {
    json!({ "__typename": "StatusContext", "context": "ci/legacy", "state": state, "targetUrl": null, "createdAt": "2026-09-26T01:05:00Z", "isRequired": true })
}

fn contexts(answer: Value) -> Vec<Context> {
    decode(answer, "GitHub's answer to the checks query").unwrap()
}

fn run_of(answer: Value) -> CheckRun {
    decode(answer, "GitHub's answer to the checks query").unwrap()
}

#[test]
fn checks_come_to_the_worst_of_them() {
    let run = |name: &str, status: &str, conclusion: Value| {
        let mut run = check_run(status, conclusion, "some-app");
        run["name"] = json!(name);
        run
    };
    let read = |answer: Value| read_checks("abc".into(), &contexts(answer), 7);
    let passed = read(json!([run("rust", "COMPLETED", json!("SUCCESS")), run("lint", "COMPLETED", json!("SKIPPED")), status_context("SUCCESS")]));
    assert_eq!(passed.state, CheckState::Pass);
    assert_eq!(passed.checks[0], Check { name: "rust".into(), state: CheckState::Pass, url: Some("https://x".into()) });
    assert_eq!(passed.checks[2], Check { name: "ci/legacy".into(), state: CheckState::Pass, url: None });
    assert_eq!(read(json!([run("rust", "IN_PROGRESS", Value::Null), status_context("SUCCESS")])).state, CheckState::Pending);
    assert_eq!(read(json!([run("rust", "IN_PROGRESS", Value::Null), run("npm", "COMPLETED", json!("FAILURE"))])).state, CheckState::Fail);
    assert_eq!(read(json!([status_context("ERROR")])).state, CheckState::Fail);
    assert_eq!(read(json!([status_context("PENDING")])).state, CheckState::Pending);
    // A commit with no rollup has no checks yet.
    let none = read(json!([]));
    assert_eq!((none.state, none.checks.len()), (CheckState::None, 0));
    assert_eq!([CheckState::Pass, CheckState::Fail, CheckState::Pending, CheckState::None, CheckState::Unavailable, CheckState::Merged, CheckState::Closed, CheckState::Conflicts].map(CheckState::exit_code), [0, 1, 8, 3, 4, 5, 6, 7]);
    assert_eq!(serde_json::to_value(&none).unwrap(), json!({ "pr": 7, "head": "abc", "state": "none", "checks": [] }));
    // A context GitHub's union doesn't have, or one without what it is, fails naming the query.
    let error = decode::<Vec<Context>>(json!([{ "name": "ci" }]), "GitHub's answer to the checks query").unwrap_err().to_string();
    assert_eq!(error, "GitHub's answer to the checks query didn't decode: it had no `__typename`");
}

#[test]
fn a_check_that_never_ran_is_unavailable_not_failed() {
    let read = |answer: Value| read_checks("abc".into(), &contexts(answer), 7);
    let actions = |conclusion: &str, steps: u64, annotations: Vec<&str>| {
        let notes: Vec<Value> = annotations.iter().map(|m| json!({ "message": m })).collect();
        let mut run = check_run("COMPLETED", json!(conclusion), "github-actions");
        run["steps"] = json!({ "totalCount": steps });
        run["annotations"] = json!({ "nodes": notes });
        run
    };
    const BILLING: &str = "The job was not started because recent account payments have failed or your spending limit needs to be increased.";
    // Actions concluded the job without a step: the org's minutes ran out, or it was
    // cancelled before it began. Either way nothing of the code ran.
    for never in [actions("FAILURE", 0, vec![BILLING]), actions("FAILURE", 0, vec![]), actions("CANCELLED", 0, vec![]), actions("ACTION_REQUIRED", 0, vec![])] {
        assert!(never_ran(&run_of(never.clone())), "{never}");
        let read = read(json!([never.clone()]));
        assert_eq!((read.state, read.checks[0].state), (CheckState::Unavailable, CheckState::Unavailable), "{never}");
    }
    // A run that ran but needs a person's action is unavailable, not failed, and a stale one
    // never blocks a merge: neither is a failure the worker can fix.
    for (conclusion, state) in [("ACTION_REQUIRED", CheckState::Unavailable), ("STALE", CheckState::Pass)] {
        let waiting = actions(conclusion, 4, vec![]);
        assert!(!never_ran(&run_of(waiting.clone())), "{conclusion}");
        assert_eq!(read(json!([waiting])).state, state, "{conclusion}");
    }
    // GitHub's annotation alone says so too, should the steps be reported.
    assert!(never_ran(&run_of(actions("FAILURE", 3, vec!["Some other note", BILLING]))));
    // A job that ran and failed, or timed out, has steps and no such annotation; and a
    // startup failure is the workflow file's own fault, steps or none.
    for ran in [actions("FAILURE", 4, vec![]), actions("TIMED_OUT", 4, vec!["The job has exceeded the maximum execution time of 10 minutes"]), actions("FAILURE", 2, vec!["Process completed with exit code 1."]), actions("STARTUP_FAILURE", 0, vec![])] {
        assert!(!never_ran(&run_of(ran.clone())), "{ran}");
        assert_eq!(read(json!([ran.clone()])).state, CheckState::Fail, "{ran}");
    }
    // Another app's check run has no steps to count, and its annotations are its own:
    // its failure is a failure, whatever they say.
    let mut other = actions("FAILURE", 0, vec![BILLING]);
    other["checkSuite"]["app"]["slug"] = json!("some-deployer");
    assert!(!never_ran(&run_of(other.clone())));
    assert_eq!(read(json!([other.clone()])).state, CheckState::Fail);
    // And a run whose steps weren't read counts as run.
    assert!(!never_ran(&run_of(check_run("COMPLETED", json!("FAILURE"), "github-actions"))));
    // A passing conclusion is a pass whatever its steps: a skipped job has none.
    assert_eq!(read(json!([actions("SKIPPED", 0, vec![])])).state, CheckState::Pass);
    // The PR's state: a genuine failure wins over CI that couldn't run, a pending check
    // is waited for first, and only then is the PR's CI unavailable.
    let pending = check_run("IN_PROGRESS", Value::Null, "github-actions");
    let passed = status_context("SUCCESS");
    assert_eq!(read(json!([actions("FAILURE", 0, vec![BILLING]), other])).state, CheckState::Fail);
    assert_eq!(read(json!([actions("FAILURE", 0, vec![BILLING]), pending])).state, CheckState::Pending);
    assert_eq!(read(json!([actions("FAILURE", 0, vec![BILLING]), passed])).state, CheckState::Unavailable);
    assert_eq!(serde_json::to_value(read(json!([actions("FAILURE", 0, vec![BILLING])])).state).unwrap(), json!("unavailable"));
    // The full checks query asks for what `never_ran` reads, and the light one only what
    // `may_not_have_run` does.
    assert!(CHECKS_QUERY.contains("checkSuite { app { slug } } steps(first: 1) { totalCount } annotations(first: 20) { nodes { message } } }"));
    assert!(CHECKS_LIGHT.contains("checkSuite { app { slug } } }") && !CHECKS_LIGHT.contains("steps") && !CHECKS_LIGHT.contains("annotations"));
}

#[test]
fn checks_are_read_in_full_only_for_a_run_that_may_not_have_run() {
    // Pending, passing, another app's failure and a startup failure settle on the light read.
    for (status, conclusion, slug) in [("IN_PROGRESS", Value::Null, "github-actions"), ("COMPLETED", json!("SUCCESS"), "github-actions"), ("COMPLETED", json!("FAILURE"), "deployer"), ("COMPLETED", json!("STARTUP_FAILURE"), "github-actions"), ("COMPLETED", json!("STALE"), "github-actions")] {
        let mut gh = FakeReader::always(checks_page("abc", "abc", vec![check_run(status, conclusion.clone(), slug)], false));
        checks(&mut gh, "o/n", 7, None).unwrap();
        assert_eq!(gh.queries, [CHECKS_LIGHT], "{status} {conclusion} {slug}");
    }
    // An Actions run that failed is read again in full, which finds it never ran.
    let failed = check_run("COMPLETED", json!("FAILURE"), "github-actions");
    let mut never = failed.clone();
    never["steps"] = json!({ "totalCount": 0 });
    never["annotations"] = json!({ "nodes": [] });
    let mut reads = 0;
    let mut gh = FakeReader::new(|_, _| {
        reads += 1;
        Ok(checks_page("abc", "abc", vec![if reads == 1 { failed.clone() } else { never.clone() }], false))
    });
    assert_eq!(checks(&mut gh, "o/n", 7, None).unwrap().state, CheckState::Unavailable);
    assert_eq!(gh.queries, [CHECKS_LIGHT, CHECKS_QUERY]);
}

// `pr checks` and `evaluatePr` read a context with one rule, `CheckState::of`.
#[test]
fn a_check_is_classified_alike_by_pr_checks_and_by_the_head_read() {
    let pr = Pr { number: 7, repo: "o/n".into(), head: "abc".into(), merge_state: "CLEAN".into(), base: "main".into(), base_head: String::new(), base_at: None, bot_author: None, checks_green: false, auto_merge_off: false, auto_merge_on: false, reviews: Vec::new(), threads: Vec::new(), comments: Vec::new() };
    let run = |conclusion: &str| {
        let mut run = check_run("COMPLETED", json!(conclusion), "other-app");
        run["steps"] = json!({ "totalCount": 3 });
        run
    };
    let queued = check_run("QUEUED", Value::Null, "other-app");
    for (context, state) in [
        (run("SUCCESS"), CheckState::Pass),
        (run("STALE"), CheckState::Pass),
        (run("ACTION_REQUIRED"), CheckState::Unavailable),
        (run("FAILURE"), CheckState::Fail),
        (run("A_CONCLUSION_GITHUB_ADDS_LATER"), CheckState::Fail),
        (queued, CheckState::Pending),
        (status_context("SUCCESS"), CheckState::Pass),
        (status_context("EXPECTED"), CheckState::Pending),
        (status_context("A_STATE_GITHUB_ADDS_LATER"), CheckState::Fail),
    ] {
        let mut gh = FakeReader::always(checks_page("abc", "abc", vec![context.clone()], false));
        let head = head_checks(&mut gh, &pr).unwrap().unwrap();
        let read = contexts(json!([context.clone()]));
        assert_eq!((CheckState::of(&read[0]), head[0].state, read_checks("abc".into(), &read, 7).checks[0].state), (state, state, state), "{context}");
        // Required, and when it finished: a run's completion, a status's setting.
        assert_eq!((head[0].required, head[0].at), (true, instant("2026-09-26T01:05:00Z")), "{context}");
    }
}

#[test]
fn head_checks_are_read_in_full_only_for_a_run_that_may_not_have_run() {
    let run = |conclusion: &str| check_run("COMPLETED", json!(conclusion), "github-actions");
    let pr = Pr { number: 7, repo: "o/n".into(), head: "abc".into(), merge_state: "CLEAN".into(), base: "main".into(), base_head: String::new(), base_at: None, bot_author: None, checks_green: false, auto_merge_off: false, auto_merge_on: false, reviews: Vec::new(), threads: Vec::new(), comments: Vec::new() };
    let mut gh = FakeReader::always(checks_page("abc", "abc", vec![run("SUCCESS")], false));
    assert_eq!(head_checks(&mut gh, &pr).unwrap().unwrap().len(), 1);
    assert_eq!(gh.queries, [CHECKS_LIGHT]);
    // A failed Actions run is read again in full, which finds it never ran.
    let mut never = run("FAILURE");
    never["steps"] = json!({ "totalCount": 0 });
    never["annotations"] = json!({ "nodes": [] });
    let mut reads = 0;
    let mut gh = FakeReader::new(|_, _| {
        reads += 1;
        Ok(checks_page("abc", "abc", vec![if reads == 1 { run("FAILURE") } else { never.clone() }], false))
    });
    let checks = head_checks(&mut gh, &pr).unwrap().unwrap();
    assert_eq!(checks[0].state, CheckState::Unavailable);
    assert_eq!(gh.queries, [CHECKS_LIGHT, CHECKS_QUERY]);
    // A head moved between the reads is None, as one moved before the first is.
    let mut reads = 0;
    let mut gh = FakeReader::new(|_, _| {
        reads += 1;
        Ok(checks_page(if reads == 1 { "abc" } else { "def" }, if reads == 1 { "abc" } else { "def" }, vec![run("FAILURE")], false))
    });
    assert!(head_checks(&mut gh, &pr).unwrap().is_none());
}

// A PR as the checks query answers with it, every field it asks for: open, merging cleanly,
// with GitHub's head, the commit whose rollup came back, and that rollup's page of checks.
fn checks_page(head: &str, rolled: &str, nodes: Vec<Value>, more: bool) -> Value {
    let rollup = json!({ "contexts": { "nodes": nodes, "pageInfo": { "hasNextPage": more, "endCursor": "p1" } } });
    json!({ "repository": { "pullRequest": { "headRefOid": head, "headRefName": "tw-1", "state": "OPEN", "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN", "commits": { "nodes": [{ "commit": { "oid": rolled, "statusCheckRollup": rollup } }] } } } })
}

// `page` with GitHub's `mergeable` and `mergeStateStatus` for its PR.
fn merging(mut page: Value, mergeable: &str, status: &str) -> Value {
    page["repository"]["pullRequest"]["mergeable"] = json!(mergeable);
    page["repository"]["pullRequest"]["mergeStateStatus"] = json!(status);
    page
}

fn passing() -> Value {
    check_run("COMPLETED", json!("SUCCESS"), "github-actions")
}

#[test]
fn checks_beyond_the_first_page_count() {
    // 100 passing checks, then a failing one on the second page: the PR fails.
    let run = |conclusion: &str| check_run("COMPLETED", json!(conclusion), "other-app");
    let mut gh = FakeReader::new(|_, variables| Ok(if variables["after"].is_null() { checks_page("abc", "abc", vec![run("SUCCESS"); 100], true) } else { checks_page("abc", "abc", vec![run("FAILURE")], false) }));
    let read = checks(&mut gh, "o/n", 7, None).unwrap();
    assert_eq!((read.state, read.checks.len()), (CheckState::Fail, 101));
    assert_eq!(gh.calls, [r#"graphql o/n {"after":null,"number":7}"#, r#"graphql o/n {"after":"p1","number":7}"#]);
    // With no rollup at all, one read and no checks.
    let mut bare = checks_page("abc", "abc", vec![], false);
    bare["repository"]["pullRequest"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"] = Value::Null;
    let mut bare = FakeReader::always(bare);
    assert_eq!(checks(&mut bare, "o/n", 7, None).unwrap().state, CheckState::None);
    assert_eq!(bare.calls.len(), 1);
}

#[test]
fn the_head_before_the_push_reads_as_pending_and_the_pushed_one_by_its_checks() {
    let pushed = |branch: Option<&str>| Pushed { branch: branch.map(String::from), sha: "new".into() };
    // GitHub still has the head before the push: its checks all passed, and none are shown.
    let mut old = FakeReader::always(checks_page("old", "old", vec![passing()], false));
    let read = checks(&mut old, "o/n", 7, Some(&pushed(None))).unwrap();
    assert_eq!((read.state, read.head.as_str(), read.checks.len()), (CheckState::Pending, "old", 0));
    assert_eq!(read.state.exit_code(), 8);
    // The head has moved but the rollup is still the old commit's, or the other way round.
    for (head, rolled) in [("new", "old"), ("old", "new")] {
        let mut moving = FakeReader::always(checks_page(head, rolled, vec![passing()], false));
        assert_eq!(checks(&mut moving, "o/n", 7, Some(&pushed(None))).unwrap().state, CheckState::Pending, "{head} {rolled}");
        assert_eq!(checks(&mut moving, "o/n", 7, None).unwrap().state, CheckState::Pending, "{head} {rolled}, no push named");
    }
    // The pushed commit's own passing checks are a pass, and its head is the one shown.
    let mut new = FakeReader::always(checks_page("new", "new", vec![passing()], false));
    let read = checks(&mut new, "o/n", 7, Some(&pushed(None))).unwrap();
    assert_eq!((read.state, read.head.as_str(), read.checks.len()), (CheckState::Pass, "new", 1));
    assert_eq!(checks(&mut new, "o/n", 7, Some(&Pushed { branch: None, sha: "NEW".into() })).unwrap().state, CheckState::Pass);
    // A checkout's upstream counts for the PR of its own branch only.
    assert_eq!(checks(&mut old, "o/n", 7, Some(&pushed(Some("tw-1")))).unwrap().state, CheckState::Pending);
    assert_eq!(checks(&mut old, "o/n", 7, Some(&pushed(Some("main")))).unwrap().state, CheckState::Pass);
}

#[test]
fn a_pr_no_longer_open_ends_the_checks_whatever_was_pushed() {
    let pushed = Pushed { branch: Some("tw-1".into()), sha: "new".into() };
    for (state, ended, code) in [("MERGED", CheckState::Merged, 5), ("CLOSED", CheckState::Closed, 6)] {
        let mut page = checks_page("old", "old", vec![passing()], false);
        page["repository"]["pullRequest"]["state"] = json!(state);
        let mut gh = FakeReader::always(page);
        let read = checks(&mut gh, "o/n", 7, Some(&pushed)).unwrap();
        assert_eq!((read.state, read.head.as_str(), read.checks.len(), read.state.exit_code()), (ended, "old", 0, code));
        assert_eq!(gh.calls.len(), 1);
    }
    let open = checks_page("new", "new", vec![passing()], false);
    assert_eq!(checks(&mut FakeReader::always(open), "o/n", 7, Some(&pushed)).unwrap().state, CheckState::Pass);
}

#[test]
fn a_push_between_pages_reads_as_pending() {
    let mut reads = 0;
    let mut gh = FakeReader::new(|_, _| {
        reads += 1;
        Ok(if reads == 1 { checks_page("a", "a", vec![passing(); 100], true) } else { checks_page("b", "b", vec![passing()], false) })
    });
    let read = checks(&mut gh, "o/n", 7, None).unwrap();
    assert_eq!((read.state, read.head.as_str(), read.checks.len()), (CheckState::Pending, "b", 0));
    assert_eq!(gh.calls.len(), 2);
}

#[test]
fn a_pr_that_conflicts_with_its_base_is_that_over_its_checks() {
    let pending = check_run("IN_PROGRESS", Value::Null, "github-actions");
    let failed = check_run("COMPLETED", json!("FAILURE"), "other-app");
    let read = |page: Value| checks(&mut FakeReader::always(page), "o/n", 7, None).unwrap();
    for nodes in [vec![], vec![pending.clone()], vec![failed.clone()]] {
        for (mergeable, status) in [("CONFLICTING", "DIRTY"), ("CONFLICTING", ""), ("CONFLICTING", "UNKNOWN"), ("", "DIRTY")] {
            let conflicting = read(merging(checks_page("abc", "abc", nodes.clone(), false), mergeable, status));
            assert_eq!((conflicting.state.exit_code(), conflicting.conflicts, conflicting.checks.len()), (7, true, nodes.len()), "{mergeable} {status} {nodes:?}");
        }
    }
    assert_eq!(serde_json::to_value(read(merging(checks_page("abc", "abc", vec![], false), "CONFLICTING", "DIRTY"))).unwrap()["conflicts"], json!(true));
    // A PR no longer open is that first, and a head not yet the pushed one is pending.
    let mut merged = merging(checks_page("abc", "abc", vec![], false), "CONFLICTING", "DIRTY");
    merged["repository"]["pullRequest"]["state"] = json!("MERGED");
    assert_eq!(read(merged).state.exit_code(), 5);
    let stale = merging(checks_page("old", "old", vec![], false), "CONFLICTING", "DIRTY");
    assert_eq!(checks(&mut FakeReader::always(stale), "o/n", 7, Some(&Pushed { branch: None, sha: "new".into() })).unwrap().state, CheckState::Pending);
    // Behind its base alone isn't a conflict.
    for nodes in [vec![], vec![passing()], vec![failed]] {
        let behind = read(merging(checks_page("abc", "abc", nodes, false), "MERGEABLE", "BEHIND"));
        assert!(behind.state.exit_code() != 7 && !behind.conflicts, "{behind:?}");
    }
    // No checks while GitHub hasn't worked out whether the PR merges is pending; checks it
    // has come to what they came to.
    for (mergeable, status) in [("UNKNOWN", "UNKNOWN"), ("UNKNOWN", "DIRTY"), ("", "UNKNOWN")] {
        assert_eq!(read(merging(checks_page("abc", "abc", vec![], false), mergeable, status)).state, CheckState::Pending, "{mergeable} {status}");
    }
    assert_eq!(read(merging(checks_page("abc", "abc", vec![passing()], false), "UNKNOWN", "UNKNOWN")).state, CheckState::Pass);
    let none = serde_json::to_value(read(merging(checks_page("abc", "abc", vec![], false), "MERGEABLE", "CLEAN"))).unwrap();
    assert!(none.get("conflicts").is_none(), "{none}");
}
