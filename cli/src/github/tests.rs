use super::*;
use super::checks::{CHECKS_LIGHT, CHECKS_QUERY};
use super::pr::{branch_prs_query, pr_query};
use crate::ports::fake::FakeReader;

#[test]
fn a_remotes_url_names_its_github_repo() {
    for url in ["https://github.com/Tolkien-Works/tolkster.git", "https://github.com/Tolkien-Works/tolkster", "git@github.com:Tolkien-Works/tolkster.git", "ssh://git@github.com/Tolkien-Works/tolkster.git\n"] {
        assert_eq!(remote_repo(url).as_deref(), Some("Tolkien-Works/tolkster"), "{url}");
    }
    for url in ["", "https://gitlab.com/a/b.git", "https://github.com/a", "https://github.com/a/b/c", "D:/source/tolkster"] {
        assert_eq!(remote_repo(url), None, "{url}");
    }
}

#[test]
fn a_branch_with_an_open_pr_gives_that_pr() {
    assert!(already_open(&json!({ "message": "Validation Failed", "errors": [{ "message": "A pull request already exists for o:b." }] })));
    assert!(!already_open(&json!({ "message": "Validation Failed", "errors": [{ "message": "No commits between main and b" }] })));
    let created = || decode::<RestPr>(json!({ "number": 9, "html_url": "https://github.com/o/n/pull/9", "state": "open", "title": "never read" }), "GitHub's answer to POST /repos/{repo}/pulls");
    assert_eq!(opened(created().unwrap(), true), Opened { number: 9, url: "https://github.com/o/n/pull/9".into(), existing: true });
    assert_eq!(serde_json::to_value(opened(created().unwrap(), false)).unwrap(), json!({ "number": 9, "url": "https://github.com/o/n/pull/9", "existing": false }));
    let unlinked = decode::<RestPr>(json!({ "number": 9 }), "GitHub's answer to POST /repos/{repo}/pulls").unwrap_err().to_string();
    assert_eq!(unlinked, "GitHub's answer to POST /repos/{repo}/pulls didn't decode: it had no `html_url`");
}

#[test]
fn a_repos_variables_are_its_owner_and_name() {
    assert_eq!(repo_variables("o/n", json!({ "number": 7 })).unwrap(), json!({ "owner": "o", "name": "n", "number": 7 }));
    assert!(repo_variables("o/n/x", json!({})).is_err());
}

#[test]
fn auto_merge_is_turned_on_only_for_an_open_pr_without_it() {
    let pr = PrRef { repo: "Tolkien-Works/tolkster", pr: 7 };
    let read = |answer: Value| -> PrRead { decode(answer, "GitHub's answer to the PR query").unwrap() };
    let with = |state: &str, auto: Value| read(json!({ "repository": { "pullRequest": { "id": "PR_x", "state": state, "autoMergeRequest": auto } } }));
    assert_eq!(needs_auto_merge(&with("OPEN", Value::Null), &pr).unwrap(), Some("PR_x".into()));
    assert_eq!(needs_auto_merge(&with("OPEN", json!({ "enabledAt": "2026-09-28T00:00:00Z" })), &pr).unwrap(), None);
    assert_eq!(needs_auto_merge(&with("MERGED", Value::Null), &pr).unwrap_err().to_string(), "Tolkien-Works/tolkster#7 is not open: merged");
    for missing in [json!({ "repository": { "pullRequest": null } }), json!({ "repository": null })] {
        assert_eq!(needs_auto_merge(&read(missing), &pr).unwrap_err().to_string(), "Tolkien-Works/tolkster#7 is not a pull request");
    }
    // A description is replaced only on an open PR, by the same read.
    assert_eq!(an_open_pr(&with("OPEN", Value::Null), &pr).unwrap(), ("PR_x".into(), false));
    assert_eq!(an_open_pr(&with("OPEN", json!({ "enabledAt": "2026-09-28T00:00:00Z" })), &pr).unwrap(), ("PR_x".into(), true));
    assert_eq!(an_open_pr(&with("CLOSED", Value::Null), &pr).unwrap_err().to_string(), "Tolkien-Works/tolkster#7 is not open: closed");
    // A refusal is the mergeable one only while the PR, open without auto-merge, reads `CLEAN`:
    // blocked, unstable or on is any other.
    let merging_in = |allowed: bool, merge: &str, auto: Value| {
        let answer = json!({ "repository": { "autoMergeAllowed": allowed, "pullRequest": { "id": "PR_x", "state": "OPEN", "mergeStateStatus": merge, "autoMergeRequest": auto } } });
        clean(&read(answer), &pr).unwrap()
    };
    let merging = |merge: &str, auto: Value| merging_in(true, merge, auto);
    assert!(merging("CLEAN", Value::Null));
    // A repo that doesn't allow auto-merge refuses it for that: its PRs are the owner's to merge.
    assert!(!merging_in(false, "CLEAN", Value::Null));
    assert!(!merging("BLOCKED", Value::Null) && !merging("UNSTABLE", Value::Null) && !merging("UNKNOWN", Value::Null));
    assert!(!merging("CLEAN", json!({ "enabledAt": "2026-09-28T00:00:00Z" })));
    assert!(clean(&with("MERGED", Value::Null), &pr).is_err());
}

#[test]
fn what_the_pr_commands_print() {
    assert_eq!(serde_json::to_value(AutoMerge { pr: 7, auto_merge: true }).unwrap(), json!({ "pr": 7, "autoMerge": true }));
    assert_eq!(serde_json::to_value(Described { pr: 7, described: true }).unwrap(), json!({ "pr": 7, "described": true }));
}

#[test]
fn a_thread_must_be_on_the_named_pr() {
    let thread = ThreadRef { repo: "Tolkien-Works/tolkster", pr: 7, thread: "PRRT_x" };
    let read = |answer: Value| -> ThreadRead { decode(answer, "GitHub's answer to the thread query").unwrap() };
    let on = |number: u64, repo: &str, resolved: bool| read(json!({ "node": { "isResolved": resolved, "pullRequest": { "number": number, "repository": { "nameWithOwner": repo } } } }));
    assert!(!check_thread(on(7, "Tolkien-Works/tolkster", false), &thread).unwrap());
    assert!(check_thread(on(7, "tolkien-works/Tolkster", true), &thread).unwrap());
    // A node of another type answers with none of the fragment's fields.
    for other in [on(8, "Tolkien-Works/tolkster", false), on(7, "Tolkien-Works/other", false), read(json!({ "node": null })), read(json!({ "node": {} }))] {
        assert_eq!(check_thread(other, &thread).unwrap_err().to_string(), "PRRT_x is not a review thread on Tolkien-Works/tolkster#7");
    }
}

#[test]
fn a_mutations_answer_says_whether_it_wrote() {
    assert!(turned_on(json!({ "enablePullRequestAutoMerge": { "pullRequest": { "autoMergeRequest": { "enabledAt": "2026-09-28T00:00:00Z" } } } })).unwrap());
    assert!(!turned_on(json!({ "enablePullRequestAutoMerge": { "pullRequest": { "autoMergeRequest": null } } })).unwrap());
    assert!(!turned_on(json!({ "enablePullRequestAutoMerge": null })).unwrap());
    assert!(now_resolved(json!({ "resolveReviewThread": { "thread": { "isResolved": true } } })).unwrap());
    assert!(!now_resolved(json!({ "resolveReviewThread": { "thread": { "isResolved": false } } })).unwrap());
    assert_eq!(now_resolved(json!({ "resolveReviewThread": { "thread": {} } })).unwrap_err().to_string(), "GitHub's answer to the resolve mutation didn't decode: it had no `isResolved`");
}

#[test]
fn a_reply_is_read() {
    let data = json!({ "addPullRequestReviewThreadReply": { "comment": { "id": "PRRC_1", "url": "https://github.com/o/n/pull/7#discussion_r1" } } });
    assert_eq!(replied(data).unwrap(), Replied { id: "PRRC_1".into(), url: "https://github.com/o/n/pull/7#discussion_r1".into() });
    assert!(replied(json!({ "addPullRequestReviewThreadReply": { "comment": null } })).is_err());
}

#[test]
fn a_node_id_is_only_what_github_writes() {
    assert!(is_node_id("PRRT_kwDOPx-1ab5Hc9w="));
    let long = "a".repeat(201);
    for bad in ["", "PRRT x", "PRRT\"x", "a/b", long.as_str()] {
        assert!(!is_node_id(bad), "{bad}");
    }
}

// The ruleset's Copilot rule is read by its type and its "review on push" alone
// (TW-240, TW-242): the rule with the parameter on reviews each push; the rule with it
// off, or missing, reviews the opening only; no rule is off. Rules from several
// rulesets add up: any that reviews pushes means pushes are reviewed. A response that
// isn't a list of rules fails, so the caller errs towards the deeper review.
#[test]
fn copilot_reviews_when_the_base_has_its_rule() {
    let rule = |kind: &str| json!({ "type": kind, "ruleset_source_type": "Repository", "ruleset_source": "Tolkien-Works/tolkster", "ruleset_id": 1 });
    let copilot = |on_push: Value| json!({ "type": "copilot_code_review", "parameters": { "review_on_push": on_push, "review_draft_pull_requests": false }, "ruleset_source_type": "Repository", "ruleset_source": "Tolkien-Works/tolkster", "ruleset_id": 1 });
    let mut gh = FakeReader::always(json!([rule("deletion"), rule("pull_request"), copilot(json!(true))]));
    assert_eq!(copilot_reviews(&mut gh, "Tolkien-Works/tolkster", "main").unwrap(), CopilotReview::OnEachPush);
    assert_eq!(gh.calls, ["get Tolkien-Works/tolkster rules/branches/main"]);
    let reads = |rules: Value| copilot_reviews(&mut FakeReader::always(rules), "o/n", "main").unwrap();
    assert_eq!(reads(json!([rule("deletion"), copilot(json!(false))])), CopilotReview::OnOpening);
    assert_eq!(reads(json!([copilot(json!(null))])), CopilotReview::OnOpening);
    assert!(copilot_reviews(&mut FakeReader::always(json!([copilot(json!("true"))])), "o/n", "main").is_err());
    assert_eq!(reads(json!([rule("copilot_code_review")])), CopilotReview::OnOpening);
    assert_eq!(reads(json!([copilot(json!(false)), copilot(json!(true))])), CopilotReview::OnEachPush);
    assert_eq!(reads(json!([rule("deletion"), rule("pull_request")])), CopilotReview::Off);
    assert_eq!(reads(json!([])), CopilotReview::Off);
    assert!(copilot_reviews(&mut FakeReader::always(json!({ "message": "Not Found" })), "o/n", "main").is_err());
    assert!(copilot_reviews(&mut FakeReader::new(|_, _| Err(Error::Failed("GitHub refused reading rules/branches/main: HTTP 403".into()))), "o/n", "main").is_err());
    assert!(CopilotReview::OnEachPush.opening() && CopilotReview::OnOpening.opening() && !CopilotReview::Off.opening());
}

#[test]
fn every_query_sent_takes_its_cost() {
    for query in [PR_QUERY.into(), THREAD_QUERY.into(), READINESS.into(), CHECKS_QUERY.into(), CHECKS_LIGHT.into(), branch_prs_query(), pr_query()] {
        let asked = cost::with_rate_limit(&query);
        assert!(asked.ends_with(" rateLimit { cost remaining resetAt } }"), "{asked}");
        assert_eq!(asked.matches('{').count(), asked.matches('}').count());
    }
}

#[test]
fn the_events_are_the_workers() {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../src/worker/github.ts")).unwrap();
    let declaration = source.lines().find_map(|l| l.trim().strip_prefix("export const GITHUB_EVENTS: readonly string[] = ")).expect("GITHUB_EVENTS in src/worker/github.ts");
    assert_eq!(declaration, format!("{};", serde_json::to_string(&GITHUB_EVENTS).unwrap().replace(',', ", ")), "GITHUB_EVENTS differs between cli/src/github.rs and src/worker/github.ts");
}
