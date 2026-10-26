use super::*;
use crate::exec::Output;
use crate::plain::Sha;
use crate::ports::fake::{Git, Orca as FakeOrca, World};
use crate::state::tests::Fake;
use serde_json::{Value, json};

const OWNER: &str = "owner-1";
const APP: &str = "app-1";

// An issue as TRIAGE_ISSUE reads it.
fn node(identifier: &str, state: &str, creator: Option<&str>, created_at: &str) -> Value {
    json!({ "identifier": identifier, "createdAt": created_at, "archivedAt": null, "state": { "type": state }, "team": { "key": "TW", "issueEstimationType": "notUsed", "issueEstimationExtended": false, "issueEstimationAllowZero": false }, "project": { "id": "p1", "name": "Tolkster" }, "creator": creator.map(|c| json!({ "id": c })) })
}

fn parse_candidate(issue: &Value) -> Result<Candidate, Error> {
    super::parse_candidate(&crate::json::decode(issue.clone(), "Linear's answer to TriageQueue")?)
}

fn filed_in_triage(history: &Value) -> bool {
    super::filed_in_triage(&crate::json::decode(history.clone(), "Linear's answer to StateHistory").unwrap())
}

fn may_relate(identifier: &str, team_id: &str, other: &Value) -> Result<(), String> {
    super::may_relate(identifier, team_id, &crate::json::decode(other.clone(), "Linear's answer to Other").unwrap())
}

fn candidate(identifier: &str, created_at: &str) -> Candidate {
    parse_candidate(&node(identifier, "triage", Some(APP), created_at)).unwrap()
}

#[test]
fn an_identifier_that_isnt_plain_is_no_candidate() {
    for bad in ["TW-1\"x", "TW-", "1-2", "TW-1 2", "tw"] {
        assert!(parse_candidate(&node(bad, "triage", Some(APP), "2026-09-28T01:00:00.000Z")).is_err(), "{bad}");
    }
}

#[test]
fn only_a_triage_issue_of_the_owners_or_mine_is_triaged() {
    let mine = candidate("tw-231", "2026-09-28T01:00:00.000Z");
    assert_eq!(mine, Candidate { identifier: "TW-231".into(), state_type: StateType::Triage, creator: Some(APP.into()), created_at: Some("2026-09-28T01:00:00.000Z".into()), place: Place { team: Some("TW".into()), project_id: Some("p1".into()), project: Some("Tolkster".into()) }, scale: Scale::of(&crate::json::decode(json!({ "issueEstimationType": "notUsed", "issueEstimationExtended": false, "issueEstimationAllowZero": false }), "a team").unwrap()), archived: false, related: Vec::new() });
    assert!(may_triage(&mine, OWNER, APP).is_ok());
    let mut node3 = node("TW-3", "triage", Some(OWNER), "2026-09-28T01:00:00.000Z");
    node3["archivedAt"] = json!("2026-09-29T01:00:00.000Z");
    let archived = parse_candidate(&node3).unwrap();
    assert_eq!(may_triage(&archived, OWNER, APP), Err(Refusal::Archived));
    let owners = parse_candidate(&node("TW-1", "triage", Some(OWNER), "2026-09-28T01:00:00.000Z")).unwrap();
    assert!(may_triage(&owners, OWNER, APP).is_ok());
    let strangers = parse_candidate(&node("TW-2", "triage", Some("someone"), "2026-09-28T01:00:00.000Z")).unwrap();
    assert_eq!(may_triage(&strangers, OWNER, APP), Err(Refusal::NotOurs));
    let nobodys = parse_candidate(&node("TW-3", "triage", None, "2026-09-28T01:00:00.000Z")).unwrap();
    assert!(may_triage(&nobodys, OWNER, APP).is_err());
    for state in ["backlog", "unstarted", "started", "completed", "canceled", "odd"] {
        let elsewhere = parse_candidate(&node("TW-4", state, Some(APP), "2026-09-28T01:00:00.000Z")).unwrap();
        assert_eq!(may_triage(&elsewhere, OWNER, APP), Err(Refusal::NotInTriage), "{state}");
    }
}

#[test]
fn only_an_issue_its_history_shows_filed_in_triage_is_triaged() {
    let state = |t: Option<&str>| t.map_or(Value::Null, |t| json!({ "type": t }));
    let history = |entries: &[(Option<&str>, Option<&str>)], more: bool| json!({
        "nodes": entries.iter().map(|(from, to)| json!({ "fromState": state(*from), "toState": state(*to) })).collect::<Vec<_>>(),
        "pageInfo": { "hasNextPage": more },
    });
    // Created in Triage, with or without an entry for the creation, and edited since.
    assert!(filed_in_triage(&history(&[], false)));
    assert!(filed_in_triage(&history(&[(None, Some("triage")), (None, None)], false)));
    // Moved from one team's Triage to another's.
    assert!(filed_in_triage(&history(&[(Some("triage"), Some("triage"))], false)));
    // Parked: moved there from another state, however long ago, or there and back.
    assert!(!filed_in_triage(&history(&[(Some("backlog"), Some("triage"))], false)));
    assert!(!filed_in_triage(&history(&[(Some("triage"), Some("started")), (Some("started"), Some("triage"))], false)));
    // More than a page: counted as moved. No page at all fails the read, naming its query.
    assert!(!filed_in_triage(&history(&[], true)));
    let unread = crate::json::decode::<StateHistory>(json!({ "nodes": [] }), "Linear's answer to StateHistory").unwrap_err().to_string();
    assert_eq!(unread, "Linear's answer to StateHistory didn't decode: it had no `pageInfo`");
}

#[test]
fn the_queue_is_what_may_be_triaged_oldest_first() {
    let nodes = [
        node("TW-5", "triage", Some(APP), "2026-09-28T03:00:00.000Z"),
        node("TW-3", "triage", Some("someone"), "2026-09-28T00:00:00.000Z"),
        node("TW-4", "backlog", Some(APP), "2026-09-28T00:00:00.000Z"),
        node("TW-2", "triage", Some(OWNER), "2026-09-28T02:00:00.000Z"),
        node("TW-1", "triage", Some(APP), "2026-09-28T01:00:00.000Z"),
        node("TW-1", "triage", Some(APP), "2026-09-28T01:00:00.000Z"),
        node("TW-0", "triage", Some(OWNER), "2026-09-27T00:00:00.000Z"),
        // An identifier that isn't plain is no candidate.
        node("TW-6\"x", "triage", Some(OWNER), "2026-09-27T00:00:00.000Z"),
    ];
    let read: Vec<Candidate> = nodes.iter().filter_map(|n| parse_candidate(n).ok()).collect();
    let kept: Vec<String> = super::queue(read, OWNER, APP).into_iter().map(|c| c.identifier.into()).collect();
    // TW-0 entered Triage later: its history, read only as a batch reaches it, tells.
    assert_eq!(kept, ["TW-0", "TW-1", "TW-2", "TW-5"]);
}

#[test]
fn a_candidate_is_related_to_the_issues_on_either_side_of_a_related_pair() {
    let mut node = node("TW-231", "triage", Some(APP), "2026-09-28T01:00:00.000Z");
    node["relations"] = json!({ "nodes": [
        { "type": "related", "relatedIssue": { "identifier": "tw-5" } },
        { "type": "blocks", "relatedIssue": { "identifier": "TW-7" } },
        { "type": "related", "relatedIssue": { "identifier": "TW-231" } },
        { "type": "related", "relatedIssue": null },
        { "type": "related", "relatedIssue": { "identifier": "TW-3\"x" } },
    ] });
    node["inverseRelations"] = json!({ "nodes": [
        { "type": "related", "issue": { "identifier": "TW-3" } },
        { "type": "related", "issue": { "identifier": "TW-5" } },
        { "type": "blocks", "issue": { "identifier": "TW-8" } },
    ] });
    assert_eq!(parse_candidate(&node).unwrap().related, ["TW-3", "TW-5"]);
}

#[test]
fn the_brief_says_what_merged_for_each_related_issue() {
    let related = |identifier: &str, related: &[&str]| Candidate { related: related.iter().map(|r| IssueKey::from(*r)).collect(), ..candidate(identifier, "2026-09-28T01:00:00.000Z") };
    let candidates = [related("TW-1", &["TW-3", "TW-4"]), related("TW-2", &[]), related("TW-5", &["TW-6"])];
    let start = Start { candidates: &candidates, batch: 5, owner_name: "Sam", briefs_dir: "/briefs/<repo>", repos: &RepoMap::default() };
    let sha = "c".repeat(40);
    let merges: Vec<(IssueKey, Merge)> = vec![("TW-3".into(), Merge::Found(Sha::parse(&sha).unwrap())), ("TW-4".into(), Merge::Absent)];
    let issues: Vec<IssueKey> = vec!["TW-1".into(), "TW-2".into(), "TW-5".into()];
    let text = start.render(&"tolkster".into(), &issues, &"abc123".into(), &merges);
    assert!(text.contains(&format!("any other issue `issue show` relates):\n   - `TW-1`: `TW-3` merged as `{sha}` (`git show {sha}`); `TW-4` had no squash merge on the default branch when this brief was written.\n   - `TW-2`: no related issue.\n   - `TW-5`: `TW-6`'s merge wasn't looked up: find it with `git log origin/HEAD --grep \"TW-6\"`.\n2. ")), "{text}");
}

#[test]
fn a_verdict_claim_is_one_per_issue() {
    assert_eq!(verdict_claim(&"tw-231".into()), "verdict-TW-231");
}


#[test]
fn a_verdict_names_only_open_issues_of_the_same_team() {
    let other = |identifier: &str, state: &str, team: &str, archived: bool| json!({ "id": "i", "identifier": identifier, "archivedAt": if archived { json!("2026-09-27T00:00:00.000Z") } else { Value::Null }, "state": { "type": state }, "team": { "id": team } });
    assert!(may_relate("TW-1", "t1", &other("TW-2", "backlog", "t1", false)).is_ok());
    assert!(may_relate("TW-1", "t1", &other("TW-2", "started", "t1", false)).is_ok());
    assert!(may_relate("TW-1", "t1", &other("tw-1", "triage", "t1", false)).unwrap_err().contains("itself"));
    assert!(may_relate("TW-1", "t1", &other("OT-2", "backlog", "t2", false)).unwrap_err().contains("same team"));
    for state in ["completed", "canceled"] {
        assert!(may_relate("TW-1", "t1", &other("TW-2", state, "t1", false)).unwrap_err().contains("isn't open"), "{state}");
    }
    assert!(may_relate("TW-1", "t1", &other("TW-2", "backlog", "t1", true)).unwrap_err().contains("isn't open"));
}

#[test]
fn a_fixed_commit_is_a_plain_sha_on_the_default_branch() {
    let mut asked = Vec::new();
    let mut git = |program: &str, args: &[&str], _: Option<&str>| {
        asked.push(format!("{program} {}", args.join(" ")));
        let on_main = args.get(2) == Some(&"abc1234");
        Ok(Output { ok: on_main, code: Some(if on_main { 0 } else { 1 }), stdout: String::new(), stderr: String::new() })
    };
    assert!(merged(&mut git, &"abc1234".into()).is_ok());
    assert!(matches!(merged(&mut git, &"def5678".into()), Err(Error::Failed(_))));
    for bad in ["", "HEAD", "abc", "abc1234..def5678", "--all", &"a".repeat(41)] {
        assert!(Commit::parse(bad).is_none(), "{bad}");
    }
    assert_eq!(asked, ["git merge-base --is-ancestor abc1234 origin/HEAD", "git merge-base --is-ancestor def5678 origin/HEAD"]);
}

#[test]
fn the_brief_names_only_identifiers_and_the_session() {
    let unknown = |identifier: &str, at: &str| Candidate { scale: None, ..candidate(identifier, at) };
    let candidates = [unknown("TW-231", "2026-09-28T01:00:00.000Z"), unknown("TW-233", "2026-09-28T02:00:00.000Z")];
    let start = Start { candidates: &candidates, batch: 5, owner_name: "Sam", briefs_dir: "/briefs/<repo>", repos: &RepoMap::default() };
    let text = start.render(&"tolkster".into(), &["TW-231".into(), "TW-233".into()], &"abc123".into(), &[]);
    assert!(text.contains("`TW-231`, `TW-233`"));
    assert!(text.contains("--session abc123"));
    assert!(text.contains(crate::brief::RUBRIC.1.lines().next().unwrap()));
    for placeholder in ["<issues>", "<session>", "<repo displayName>", "<OWNER NAME>", "<RUBRIC>", "<SESSION PREAMBLE>", "<CHECKOUT>", "<CLOSE TERMINAL>", "<SOURCES>"] {
        assert!(!text.contains(placeholder), "{placeholder}");
    }
    for never in ["git commit", "git push", "tolkster pr", "--delegate", "--add-label"] {
        assert!(!text.contains(never), "{never}");
    }
    // With a scale of a kind Linear doesn't document, the estimate is left to `tolkster triage` to check.
    assert!(text.contains("The estimate is one on the team's scale, which `tolkster triage` lists if it refuses one."));
}

fn started_for(issues: &[&str]) -> TriageNote {
    TriageNote::Started(issues.iter().map(|issue| (*issue).into()).collect())
}

struct Done {
    result: Result<Started, Error>,
    written: Vec<(String, String)>,
}

impl Done {
    fn notes(self) -> Vec<TriageNote> {
        self.result.unwrap().notes
    }
}

fn started(candidates: &[Candidate], batch: usize, orca: &mut FakeOrca, store: &Fake, token: &str) -> Done {
    started_filed(candidates, batch, orca, store, token, &mut |_| Ok(true))
}

fn started_filed(candidates: &[Candidate], batch: usize, orca: &mut FakeOrca, store: &Fake, token: &str, filed: &mut dyn FnMut(&Candidate) -> Result<bool, Error>) -> Done {
    let start = Start { candidates, batch, owner_name: "Sam", briefs_dir: "/briefs/<repo>", repos: &RepoMap::default() };
    let mut written = Vec::new();
    let result = super::start(
        &start,
        filed,
        orca,
        &mut |path, text| {
            written.push((path.display().to_string(), text.to_string()));
            Ok(())
        },
        store,
        &token.into(),
    );
    Done { result, written }
}

fn queue(n: usize) -> Vec<Candidate> {
    (1..=n).map(|i| candidate(&format!("TW-{i}"), &format!("2026-09-28T0{i}:00:00.000Z"))).collect()
}

fn triaged(store: &Fake, identifiers: &[&str]) {
    for identifier in identifiers {
        store.claims.borrow_mut().insert(verdict_claim(&(*identifier).into()), "verdict".into());
    }
}

#[test]
fn a_batch_of_the_oldest_issues_without_a_verdict_starts_one_session_in_the_primary_checkout() {
    let (store, mut orca) = (Fake::default(), FakeOrca::on(&World::two_repos(true)));
    // TW-2 has its verdict from an earlier session.
    triaged(&store, &["TW-2"]);
    let done = started(&queue(4), 2, &mut orca, &store, "t1");
    assert_eq!(done.written.len(), 1);
    assert!(done.written[0].0.ends_with("triage-t1.md"), "{}", done.written[0].0);
    assert!(done.written[0].1.starts_with("# Triage: `TW-1`, `TW-3`"));
    let started = done.result.unwrap();
    assert_eq!(started.notes, vec![TriageNote::TriagedAlready(1), started_for(&["TW-1", "TW-3"])]);
    assert_eq!(started.opened, Some(Opened { session: "t1".into(), terminal: "term_1".into(), issues: vec!["TW-1".into(), "TW-3".into()] }));
    // TW-4 is past the batch, so it waits; TW-2 is settled by its verdict.
    assert_eq!(started.waiting, ["TW-4"]);
    let create = orca.world.borrow().opened_as_asked().into_iter().next().unwrap();
    assert!(create.contains("--worktree id:r1::/src/tolkster --title triage TW-1 TW-3 --command claude \"Read the file "), "{create}");
    // The machine holds the session: nothing is claimed, and each probe gave its claim back.
    assert_eq!(store.claims.borrow().keys().collect::<Vec<_>>(), ["verdict-TW-2"]);
}

#[test]
fn an_issue_moved_into_triage_or_with_an_unread_history_takes_no_place_in_the_batch() {
    let (store, mut orca) = (Fake::default(), FakeOrca::on(&World::two_repos(true)));
    let mut asked = Vec::new();
    let done = started_filed(&queue(4), 2, &mut orca, &store, "t1", &mut |c| {
        asked.push(c.identifier.clone());
        match c.identifier.as_str() {
            "TW-1" => Ok(false),
            "TW-2" => Err(Error::Failed("Linear".into())),
            _ => Ok(true),
        }
    });
    assert_eq!(done.notes(), vec![TriageNote::MovedIn { issue: "TW-1".into() }, TriageNote::History { issue: "TW-2".into(), error: Error::Failed("Linear".into()), rest_wait: false }, started_for(&["TW-3", "TW-4"])]);
    // Asked only of those the batch reached.
    assert_eq!(asked, ["TW-1", "TW-2", "TW-3", "TW-4"]);
}

#[test]
fn a_spent_linear_budget_stops_the_history_reads_and_starts_what_was_taken() {
    let (store, mut orca) = (Fake::default(), FakeOrca::on(&World::two_repos(true)));
    let mut asked = Vec::new();
    let done = started_filed(&queue(4), 3, &mut orca, &store, "t1", &mut |c| {
        asked.push(c.identifier.clone());
        match c.identifier.as_str() {
            "TW-2" => Err(Error::RateLimited { message: "spent".into(), wait: std::time::Duration::from_secs(60) }),
            _ => Ok(true),
        }
    });
    assert_eq!(done.notes(), vec![TriageNote::History { issue: "TW-2".into(), error: Error::RateLimited { message: "spent".into(), wait: std::time::Duration::from_secs(60) }, rest_wait: true }, started_for(&["TW-1"])]);
    assert_eq!(asked, ["TW-1", "TW-2"]);
    // Nothing taken: the budget's error is returned, though another came first, so the
    // start fails and the machine sends it again.
    let (store, mut orca) = (Fake::default(), FakeOrca::on(&World::two_repos(true)));
    let done = started_filed(&queue(2), 3, &mut orca, &store, "t1", &mut |c| match c.identifier.as_str() {
        "TW-1" => Err(Error::Failed("Linear".into())),
        _ => Err(Error::RateLimited { message: "spent".into(), wait: std::time::Duration::from_secs(60) }),
    });
    assert!(done.result.unwrap_err().rate_limited().is_some());
    assert!(store.claims.borrow().is_empty() && orca.world.borrow().opened.is_empty());
}

#[test]
fn an_issue_whose_session_ended_without_its_verdict_is_triaged_again() {
    let (store, mut orca) = (Fake::default(), FakeOrca::on(&World::two_repos(true)));
    assert_eq!(started(&queue(2), 5, &mut orca, &store, "t1").notes(), vec![started_for(&["TW-1", "TW-2"])]);
    // The session posted TW-2's verdict, then ended without TW-1's: the next start takes
    // TW-1 again, and only TW-1.
    triaged(&store, &["TW-2"]);
    let mut again = FakeOrca::on(&World::two_repos(true));
    assert_eq!(started(&queue(2), 5, &mut again, &store, "t2").notes(), vec![TriageNote::TriagedAlready(1), started_for(&["TW-1"])]);
    assert!(!store.claims.borrow().contains_key("verdict-TW-1"), "the probe gave the verdict claim back");
}

#[test]
fn an_issue_with_a_verdict_is_never_triaged_again() {
    let (store, mut orca) = (Fake::default(), FakeOrca::on(&World::two_repos(true)));
    let mut candidates = queue(2);
    candidates[0].place.project = Some("other".into());
    // TW-1 was declined, so it stays in Triage.
    triaged(&store, &["TW-1"]);
    let done = started(&candidates, 5, &mut orca, &store, "t1");
    // It is counted, not taken, and doesn't pick the repo.
    assert_eq!(done.notes(), vec![TriageNote::TriagedAlready(1), started_for(&["TW-2"])]);
    assert!(orca.world.borrow().opened_as_asked().into_iter().any(|c| c.contains("--worktree id:r1::/src/tolkster --title triage TW-2 ")));
    assert_eq!(store.claims.borrow().get("verdict-TW-1").map(String::as_str), Some("verdict"));
    let mut later = FakeOrca::on(&World::two_repos(true));
    assert_eq!(started(&queue(1), 5, &mut later, &store, "t2").notes(), vec![TriageNote::TriagedAlready(1)]);
    assert!(later.world.borrow().opened.is_empty());
}

#[test]
fn a_terminal_that_wont_open_fails_the_start_and_holds_nothing() {
    let store = Fake::default();
    let mut orca = FakeOrca::refusing();
    assert!(started(&queue(2), 5, &mut orca, &store, "t1").result.is_err());
    assert!(store.claims.borrow().is_empty());
    // So the machine's next start opens it.
    let mut orca = FakeOrca::on(&World::two_repos(true));
    assert_eq!(started(&queue(2), 5, &mut orca, &store, "t2").notes(), vec![started_for(&["TW-1", "TW-2"])]);
}

#[test]
fn nothing_starts_without_a_repo_and_its_primary_checkout() {
    let store = Fake::default();
    let mut orca = FakeOrca::on(&World::two_repos(true));
    assert_eq!(started(&[], 5, &mut orca, &store, "t").result.unwrap(), Started { opened: None, waiting: Vec::new(), notes: Vec::new() });
    let elsewhere = [Candidate { place: Place { project: Some("Elsewhere".into()), ..Place::default() }, ..candidate("TW-1", "2026-09-28T01:00:00.000Z") }];
    // No repo settles it; no primary checkout leaves it waiting.
    assert_eq!(started(&elsewhere, 5, &mut orca, &store, "t").result.unwrap(), Started { opened: None, waiting: Vec::new(), notes: vec![TriageNote::Missing { issue: "TW-1".into(), missing: crate::trace::Missing::Repo }] });
    let mut unchecked = FakeOrca::on(&World::two_repos(false));
    assert_eq!(started(&queue(1), 5, &mut unchecked, &store, "t").result.unwrap(), Started { opened: None, waiting: vec!["TW-1".into()], notes: vec![TriageNote::Missing { issue: "TW-1".into(), missing: crate::trace::Missing::Primary }] });
    assert!(store.claims.borrow().is_empty());
    assert!(orca.world.borrow().opened.is_empty());
}

#[test]
fn one_repo_per_session_and_the_rest_wait() {
    let (store, mut orca) = (Fake::default(), FakeOrca::on(&World::two_repos(true)));
    let mut candidates = queue(3);
    candidates[1].place.project = Some("other".into());
    let started = started(&candidates, 5, &mut orca, &store, "t1").result.unwrap();
    assert_eq!(started.notes, vec![started_for(&["TW-1", "TW-3"])]);
    assert_eq!(started.waiting, ["TW-2"]);
}

#[test]
fn a_session_is_given_no_more_issues_than_the_durable_object_holds() {
    let (store, mut orca) = (Fake::default(), FakeOrca::on(&World::two_repos(true)));
    let many: Vec<Candidate> = (1..=MAX_BATCH + 2).map(|i| candidate(&format!("TW-{i}"), &format!("2026-09-28T01:00:{:02}.000Z", i % 60))).collect();
    let started = started(&many, 500, &mut orca, &store, "t1").result.unwrap();
    assert_eq!(started.opened.unwrap().issues.len(), MAX_BATCH);
    assert_eq!(started.waiting.len(), 2);
}

#[test]
fn an_issue_triaged_already_at_the_head_of_one_repo_doesnt_hold_back_another() {
    let (store, mut orca) = (Fake::default(), FakeOrca::on(&World::two_repos(true)));
    let mut candidates = queue(4);
    candidates[1].place.project = Some("other".into());
    candidates[3].place.project = Some("other".into());
    // TW-1, the oldest, has its verdict, as has TW-3, in the repo that isn't chosen.
    triaged(&store, &["TW-1", "TW-3"]);
    let done = started(&candidates, 5, &mut orca, &store, "t1");
    assert_eq!(done.notes(), vec![TriageNote::TriagedAlready(1), started_for(&["TW-2", "TW-4"])]);
    let create = orca.world.borrow().opened_as_asked().into_iter().next().unwrap();
    assert!(create.contains("--worktree id:r2::/src/other --title triage TW-2 TW-4"), "{create}");
}

#[test]
fn the_cap_counts_the_issues_taken_not_those_triaged_already() {
    let (store, mut orca) = (Fake::default(), FakeOrca::on(&World::two_repos(true)));
    triaged(&store, &["TW-1", "TW-2", "TW-3"]);
    assert_eq!(started(&queue(6), 2, &mut orca, &store, "t1").notes(), vec![TriageNote::TriagedAlready(3), started_for(&["TW-4", "TW-5"])]);
}

#[test]
fn the_brief_gives_each_issues_team_scale() {
    let on = |identifier: &str, kind: &str| {
        let mut node = node(identifier, "triage", Some(APP), "2026-09-28T01:00:00.000Z");
        node["team"] = json!({ "key": "TW", "issueEstimationType": kind, "issueEstimationExtended": false, "issueEstimationAllowZero": false });
        parse_candidate(&node).unwrap()
    };
    let render = |candidates: &[Candidate]| {
        let issues: Vec<IssueKey> = candidates.iter().map(|c| c.identifier.clone()).collect();
        Start { candidates, batch: 5, owner_name: "Sam", briefs_dir: "/briefs/<repo>", repos: &RepoMap::default() }.render(&"tolkster".into(), &issues, &"abc123".into(), &[])
    };
    let text = render(&[on("TW-1", "tShirt"), on("TW-2", "tShirt")]);
    assert!(text.contains("The estimate is one of 1 (XS), 2 (S), 3 (M), 5 (L) or 8 (XL)."), "{text}");
    assert!(render(&[on("TW-1", "notUsed")]).contains("The estimate is left out, as the team doesn't use estimates."));
    let mixed = render(&[on("TW-1", "exponential"), on("TW-2", "notUsed")]);
    assert!(mixed.contains("The estimate is for `TW-1`, one of 1, 2, 4, 8 or 16 points; for `TW-2`, left out, as the team doesn't use estimates."), "{mixed}");
}

#[test]
fn the_session_reads_every_issue_and_what_merged_for_those_they_relate_to_in_its_first_turn() {
    let world = World::two_repos(true);
    {
        let mut world = world.borrow_mut();
        world.tip = "a".repeat(40);
        world.log = format!("{}\tAfter TW-30 (#6)\n{}\tThe thing (TW-3) (#5)\n", "b".repeat(40), "c".repeat(40));
        world.git.insert("/src/tolkster".into(), Git::default());
    }
    let mut orca = FakeOrca::on(&world);
    let mut candidates = queue(2);
    candidates[0].related = vec!["TW-3".into()];
    candidates[1].related = vec!["TW-2".into(), "TW-3".into(), "TW-4".into()];
    let done = started(&candidates, 5, &mut orca, &Fake::default(), "t1");
    let brief = done.written[0].1.clone();
    done.result.unwrap();
    // One fetch for all, a lookup each of the distinct related issues, and the brief has the SHA.
    let ran = orca.world.borrow().ran.clone();
    assert_eq!(ran.iter().filter(|c| c.contains("fetch --quiet origin")).count(), 1, "{ran:?}");
    assert_eq!(ran.iter().filter(|c| c.contains("git log --first-parent")).count(), 3, "{ran:?}");
    assert!(brief.contains(&format!("- `TW-1`: `TW-3` merged as `{}`", "c".repeat(40))), "{brief}");
    assert!(brief.contains("`TW-4` had no squash merge on the default branch"), "{brief}");
    // The launch line fetches, then reads the batch and each distinct related issue once, in one call.
    let create = orca.world.borrow().opened_as_asked().into_iter().next().unwrap();
    let reads = ["TW-1", "TW-2", "TW-3", "TW-4"].join(" ");
    assert!(create.contains(&format!("in the same turn, run git fetch origin; tolkster issue show {reads} --comments as one shell call.")), "{create}");
    // With nothing related, the session still fetches, and the checkout is left alone.
    let mut bare = FakeOrca::on(&World::two_repos(true));
    assert_eq!(started(&queue(1), 5, &mut bare, &Fake::default(), "t2").notes(), vec![started_for(&["TW-1"])]);
    assert!(bare.world.borrow().ran.iter().all(|c| !c.contains("fetch")), "{:?}", bare.world.borrow().ran);
    assert!(bare.world.borrow().opened_as_asked()[0].contains("run git fetch origin; tolkster issue show TW-1 --comments as one shell call."));
}
