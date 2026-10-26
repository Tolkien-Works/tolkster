use super::*;

// Every notice is fixed text from voice.rs or review.rs, never built from Linear or
// GitHub text: only the identifiers, PR number and count a command carries reach it.
#[test]
fn each_notice_is_fixed_text_posted_as_its_kind() {
    for notice in NOTICES {
        let (kind, body) = notice_text(notice, &["TW-84".into()], Some(7), Some(3));
        assert!(!body.is_empty(), "{notice:?}");
        let expected = match notice {
            Notice::Held | Notice::Paused | Notice::HeldAtPrompt | Notice::Stalled | Notice::CouldntPass | Notice::Unheard => ActivityType::Elicitation,
            Notice::NoRepo => ActivityType::Error,
            _ => ActivityType::Response,
        };
        assert_eq!(kind, expected, "{notice:?}");
    }
    assert_eq!(notice_text(Notice::Blocked, &["TW-84".into()], None, None).1, "Waiting on TW-84. I'll start once it's done.");
    assert_eq!(notice_text(Notice::Blocked, &["TW-84".into(), "TW-85".into()], None, None).1, "Waiting on TW-84 and TW-85. I'll start once they're done.");
    assert_eq!(notice_text(Notice::PrClosed, &[], Some(7), None).1, "PR #7 was closed without merging. I've left its branch as it is.");
    assert!(notice_text(Notice::RoundsCap, &[], Some(7), Some(3)).1.starts_with("PR #7 has had its 3 review rounds"));
    assert_eq!(notice_text(Notice::CouldntPass, &[], None, None).1, "I couldn't pass your message on. Reply here and I'll try again.");
}

const IDS: Ids = Ids { viewer: APP, owner: OWNER };

// What the machine's `IssueRead` takes (src/worker/machine/validate.ts): IDs, enums and
// times, each blocker and sub-issue at its own `updatedAt`, never a link or text.
#[test]
fn an_issues_read_is_the_machines_ids_enums_and_times() {
    let mut read = issue();
    read.state_type = StateType::Started;
    read.delegated = Some(Delegated { at: at(1), by: Some(OWNER.into()), again: false });
    read.blockers = vec![member("TW-84", StateType::Completed, 3)];
    read.children = vec![member("TW-85", StateType::Started, 4), member("TW-86", StateType::Triage, 6)];
    read.parent = Some(Parent { id: "p1".into(), identifier: "TW-80".into(), state_type: StateType::Canceled, delegate_id: Some(APP.into()), priority: 0, undelegated_at: None, canceled_at: None, updated_at: Some(at(5)), delegated_by: Some(Some(APP.into())) });
    let sessions = [session(vec![]), Session { id: "s2".into(), ..mentioned(vec![]) }, Session { id: "s3".into(), creator_id: None, on_own_note: true, ..session(vec![]) }, Session { id: "s4".into(), creator_id: Some(APP.into()), ..session(vec![]) }, Session { id: "s5".into(), creator_id: Some("someone".into()), ..session(vec![]) }];
    let facts = issue_read(&read, &at(9), &IDS, Some("tolkster".into()), &sessions, None).unwrap();
    assert_eq!(facts["updatedAt"], at(9));
    assert_eq!(facts["delegate"], "owner");
    assert_eq!(facts["state"], "open");
    assert_eq!(facts["triage"], false);
    assert_eq!(facts["stateChangedAt"], at(9));
    assert_eq!(facts["blockers"], json!([{ "id": "id-5", "done": true, "updatedAt": at(3) }]));
    // Only a sub-issue says whether it is in Triage, and the issue itself.
    assert_eq!(facts["children"], json!([{ "id": "id-5", "done": false, "triage": false, "updatedAt": at(4) }, { "id": "id-5", "done": false, "triage": true, "updatedAt": at(6) }]));
    assert_eq!(facts["parent"], json!({ "id": "p1", "delegated": true, "by": "chain", "state": "cancelled", "priority": 0, "updatedAt": at(5) }));
    let kinds: Vec<&str> = facts["sessions"].as_array().unwrap().iter().map(|s| s["kind"].as_str().unwrap()).collect();
    // Someone else's session, which the Worker refused, isn't sent: it would count toward
    // which session a delegation asks.
    assert_eq!(kinds, ["delegation", "mention", "reply", "chain"]);
    // One with no creator is the chain's only on an issue Tolkster delegated to itself: an
    // automation's on the owner's delegation is left out.
    let anonymous = [Session { id: "s6".into(), creator_id: None, ..session(vec![]) }];
    assert_eq!(issue_read(&read, &at(9), &IDS, None, &anonymous, None).unwrap()["sessions"], json!([]));
    // And on one no entry names that Tolkster's record says its chain delegated.
    let unnamed = Issue { delegated: None, ..read.clone() };
    assert_eq!(issue_read(&unnamed, &at(9), &IDS, None, &anonymous, Some(&at(5))).unwrap()["sessions"][0]["kind"], "chain");
    read.delegated = Some(Delegated { at: at(1), by: Some(APP.into()), again: false });
    assert_eq!(issue_read(&read, &at(9), &IDS, None, &anonymous, None).unwrap()["sessions"][0]["kind"], "chain");
    assert_eq!(facts["repo"], "tolkster");
    assert!(!facts.to_string().contains("https://") && !facts.to_string().contains("TW-8"), "no links or identifiers: {facts}");
}

// Only a delegation to Tolkster has a delegator, and only the owner's and the chain's are
// told apart: an automation's is `other`; one no history entry read made is `aged` when the
// page read is full and took nothing from Tolkster, else `unknown`, not written yet.
#[test]
fn a_delegation_is_the_owners_the_chains_or_another() {
    let by = |delegate: Option<&str>, by: Option<&str>| {
        let read = Issue { delegate_id: delegate.map(String::from), delegated: Some(Delegated { at: at(1), by: by.map(String::from), again: false }), ..issue() };
        issue_read(&read, &at(9), &IDS, None, &[], None).unwrap()["delegate"].clone()
    };
    assert_eq!(by(Some(APP), Some(OWNER)), "owner");
    assert_eq!(by(Some(APP), Some(APP)), "chain");
    assert_eq!(by(Some(APP), Some("someone")), "other");
    assert_eq!(by(Some(APP), None), "other");
    assert_eq!(by(Some(OWNER), Some(OWNER)), Value::Null);
    assert_eq!(by(None, None), Value::Null);
    let unread = |history_full: bool, undelegated_at: Option<String>| {
        let read = Issue { delegate_id: Some(APP.into()), delegated: None, history_full, undelegated_at, ..issue() };
        issue_read(&read, &at(9), &IDS, None, &[], None).unwrap()["delegate"].clone()
    };
    assert_eq!(unread(false, None), "unknown");
    assert_eq!(unread(true, Some(at(5))), "unknown");
    assert_eq!(unread(true, None), "aged");
    // One no entry changed at all, on an issue the owner created delegated: `created`, and
    // `unknown` for anyone else's.
    let created = |creator: &str| {
        let read = Issue { delegate_id: Some(APP.into()), delegated: None, created_delegated_by: Some(creator.into()), ..issue() };
        issue_read(&read, &at(9), &IDS, None, &[], None).unwrap()["delegate"].clone()
    };
    assert_eq!(created(OWNER), "created");
    assert_eq!(created("someone"), "unknown");
    // Tolkster's record that its chain delegated it, at 5, names the chain unless an entry read
    // took it from Tolkster since, or a full page may hide one that did.
    let recorded = |history_full: bool, undelegated_at: Option<String>, creator: Option<&str>| {
        let read = Issue { delegate_id: Some(APP.into()), delegated: None, history_full, undelegated_at, created_delegated_by: creator.map(String::from), ..issue() };
        issue_read(&read, &at(9), &IDS, None, &[], Some(&at(5))).unwrap()["delegate"].clone()
    };
    assert_eq!(recorded(false, None, None), "chain");
    assert_eq!(recorded(false, None, Some(OWNER)), "chain");
    assert_eq!(recorded(true, Some(at(4)), None), "chain");
    assert_eq!(recorded(true, Some(at(6)), None), "unknown");
    assert_eq!(recorded(true, None, None), "aged");
}

// Past the machine's 50, unfinished blockers are kept and finished ones go, with
// `moreBlockers`; a member read without its ID or clock fails the read.
#[test]
fn a_read_keeps_unfinished_blockers_and_needs_each_ones_clock() {
    let mut read = issue();
    read.blockers = (0..60).map(|n| member(&format!("TW-{n}"), if n < 55 { StateType::Completed } else { StateType::Started }, 1)).collect();
    let facts = issue_read(&read, &at(9), &IDS, None, &[], None).unwrap();
    let blockers = facts["blockers"].as_array().unwrap();
    assert_eq!(blockers.len(), 50);
    assert_eq!(blockers.iter().filter(|b| b["done"] == false).count(), 5);
    assert_eq!(facts["moreBlockers"], true);
    read.blockers = vec![Blocker { updated_at: None, ..member("TW-84", StateType::Started, 1) }];
    assert!(issue_read(&read, &at(9), &IDS, None, &[], None).is_err());
}

#[test]
fn a_finished_issues_state_changed_when_it_finished() {
    let read = Issue { state_type: StateType::Completed, completed_at: Some(at(4)), ..issue() };
    let facts = issue_read(&read, &at(9), &IDS, None, &[], None).unwrap();
    assert_eq!((facts["state"].as_str(), facts["stateChangedAt"].as_str()), (Some("completed"), Some(at(4).as_str())));
}

fn a_pr(merge_state: &str) -> Pr {
    Pr {
        number: 7,
        repo: "Tolkien-Works/tolkster".into(),
        head: HEAD_SHA.into(),
        merge_state: merge_state.into(),
        base: "main".into(),
        base_head: BASE_SHA.into(),
        base_at: None,
        bot_author: None,
        checks_green: false,
        auto_merge_off: true,
        auto_merge_on: false,
        reviews: Vec::new(),
        threads: Vec::new(),
        comments: Vec::new(),
    }
}

fn a_head(state: PrState, at: Option<&str>) -> Head {
    Head { number: 7, branch: "tw-1".into(), same_repo: true, state, sha: HEAD_SHA.into(), at: at.and_then(crate::time::instant) }
}

fn open_with(checked: Checked) -> Option<Open> {
    Some(Open { checked, feedback: None, copilot_wont_approve: false, manual_merge: true })
}

// The machine takes four merge states and four check outcomes: anything else GitHub says
// is folded in, a failure winning over a CI that couldn't run.
#[test]
fn a_pr_read_is_the_machines_shape() {
    let open = a_head(PrState::Open, Some("2026-09-26T01:04:00Z"));
    let read = |merge_state: &str, checked: Checked| pr_read(&open, &a_pr(merge_state), open_with(checked), 3).unwrap();
    for (github, machine) in [("BEHIND", "BEHIND"), ("DIRTY", "DIRTY"), ("CLEAN", "CLEAN"), ("BLOCKED", "CLEAN"), ("UNSTABLE", "CLEAN"), ("HAS_HOOKS", "CLEAN"), ("DRAFT", "CLEAN"), ("UNKNOWN", "UNKNOWN"), ("", "UNKNOWN")] {
        assert_eq!(read(github, Checked::default())["mergeState"], machine, "{github}");
    }
    let checks = |failed, unran, pending| read("CLEAN", Checked { failed, unran, pending })["checks"].clone();
    assert_eq!(checks(false, false, false), "passed");
    assert_eq!(checks(true, true, true), "failed");
    assert_eq!(checks(false, true, true), "couldntRun");
    assert_eq!(checks(false, false, true), "pending");
    let r = read("CLEAN", Checked::default());
    assert_eq!((r["at"].as_str(), r["base"].as_str(), &r["autoMergeOff"], &r["autoMerge"], &r["manualMerge"], &r["feedback"]), (Some("2026-09-26T01:04:00.000Z"), Some(BASE_SHA), &json!(true), &json!(false), &json!(true), &Value::Null));
    // Feedback carries the newest reviewer's time beside the newest, or null for Copilot's alone.
    let fed = |reviewer| pr_read(&open, &a_pr("CLEAN"), Some(Open { checked: Checked::default(), feedback: Some((crate::time::instant("2026-09-26T01:03:00Z").unwrap(), reviewer)), copilot_wont_approve: false, manual_merge: false }), 3).unwrap()["feedback"].clone();
    assert_eq!(fed(crate::time::instant("2026-09-26T01:02:00Z")), json!({ "at": "2026-09-26T01:03:00.000Z", "reviewerAt": "2026-09-26T01:02:00.000Z" }));
    assert_eq!(fed(None), json!({ "at": "2026-09-26T01:03:00.000Z", "reviewerAt": null }));
    // A repo's cap over the machine's is read as the machine's.
    assert_eq!(pr_read(&open, &a_pr("CLEAN"), None, 500).unwrap()["maxRounds"], 100);
    // A merged PR's time is its merge, as its lost `prMerged` would carry it, and nothing
    // only an open PR is read for is claimed of it.
    let merged = pr_read(&a_head(PrState::Merged, Some("2026-09-26T01:05:00Z")), &a_pr("UNKNOWN"), None, 3).unwrap();
    assert_eq!((merged["status"].as_str(), merged["at"].as_str(), merged["checks"].as_str()), (Some("merged"), Some("2026-09-26T01:05:00.000Z"), Some("pending")));
    // With no time for its state the read can't be ordered against the PR's events, and
    // with no head or base commit the machine would refuse it: both fail here.
    assert!(pr_read(&a_head(PrState::Closed, None), &a_pr("CLEAN"), None, 3).is_err());
    assert!(pr_read(&open, &Pr { base_head: String::new(), ..a_pr("CLEAN") }, None, 3).is_err());
    assert!(pr_read(&Head { sha: String::new(), ..a_head(PrState::Open, Some("2026-09-26T01:04:00Z")) }, &a_pr("CLEAN"), None, 3).is_err());
}

// The contract fixture's `readIssue` data, which test/machine.test.ts reads with the machine's
// own validator, is what `issue_read` writes for the same issue.
#[test]
fn an_issues_read_is_the_contract_fixtures() {
    let fixture = crate::state::tests::contract()["resultData"]["readIssue"]["read"].clone();
    let id = |n: u32| format!("1b2c3d4e-0000-4000-8000-0000000000{n}");
    let member = |n: u32, state: StateType, minute: u32| Blocker { id: Some(id(n)), ..member(&format!("TW-{n}"), state, minute) };
    let mut read = issue();
    read.priority = 2;
    read.delegated = Some(Delegated { at: at(1), by: Some(OWNER.into()), again: false });
    read.blockers = vec![member(84, StateType::Completed, 3)];
    read.children = vec![member(85, StateType::Started, 4), member(86, StateType::Triage, 6)];
    read.parent = Some(Parent { id: id(80), identifier: "TW-80".into(), state_type: StateType::Canceled, delegate_id: Some(APP.into()), priority: 3, undelegated_at: None, canceled_at: None, updated_at: Some(at(5)), delegated_by: Some(Some(APP.into())) });
    let described = Some(crate::request::Mention { author_id: Some(OWNER.into()), body: String::new(), in_description: true });
    let sessions = [
        Session { id: "5e5e5e5e-0000-4000-8000-000000000001".into(), ..session(vec![]) },
        Session { id: "5e5e5e5e-0000-4000-8000-000000000002".into(), mention: described, ..session(vec![]) },
    ];
    assert_eq!(issue_read(&read, &at(9), &IDS, Some("tolkster".into()), &sessions, None).unwrap(), fixture);
}

// As the Worker's ingress has it: the owner's mention in the issue's description starts a
// delegation's session, not a mention's, and the read says where it began, which only the
// machine's narrowing of it to a mention's reads.
#[test]
fn a_description_mention_is_a_delegations_session_that_says_so() {
    let mention = |in_description| Some(crate::request::Mention { author_id: Some(OWNER.into()), body: String::new(), in_description });
    let described = Session { mention: mention(true), ..session(vec![]) };
    let commented = Session { id: "s2".into(), mention: mention(false), ..session(vec![]) };
    let delegated = Session { id: "s3".into(), ..session(vec![]) };
    let facts = issue_read(&issue(), &at(9), &IDS, None, &[described, commented, delegated], None).unwrap();
    assert_eq!(facts["sessions"][0]["kind"], "delegation");
    assert_eq!(facts["sessions"][0]["inDescription"], true);
    assert_eq!(facts["sessions"][1]["kind"], "mention");
    assert_eq!(facts["sessions"][2]["kind"], "delegation");
    assert!(facts["sessions"][1].get("inDescription").is_none() && facts["sessions"][2].get("inDescription").is_none());
}

// A queued request's wait is said when it first waits and when it changes, not at each re-check.
#[test]
fn a_queued_wait_is_said_again_only_when_it_changes() {
    let said = crate::runtime::QueuedSaid::default();
    assert!(said.changed("TW-9001", "t0 Blockers b-1".into()));
    assert!(!said.changed("TW-9001", "t0 Blockers b-1".into()));
    assert!(said.changed("TW-9001", "t0 Blockers b-1 b-2".into()));
    assert!(said.changed("TW-9001", "t1 Blockers b-1 b-2".into()));
    assert!(said.changed("TW-9002", "t1 Blockers b-1 b-2".into()));
}
