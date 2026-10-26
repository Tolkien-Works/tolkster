use super::*;
use crate::request::tests::{APP, issue, session};

// Each recorded answer is decoded into its query's shape first, as Linear's is.
fn decoded<T: serde::de::DeserializeOwned>(data: &Value) -> T {
    crate::json::decode(data.clone(), "Linear's answer").unwrap()
}

fn refuse_delegation(session: &Session, data: &Value) -> Option<String> {
    super::refuse_delegation(session, &decoded(data))
}

fn filing_state<'a>(asker: &Asker, has_parent: bool, data: &Value, state: Option<StateChoice<'a>>) -> Result<Option<StateChoice<'a>>, Error> {
    super::filing_state(asker, has_parent, Some(&decoded(data)), state)
}

fn refuse_relating_filed_work(asker: &Asker, has_parent: bool, data: &Value, relating: bool) -> Result<(), Error> {
    super::refuse_relating_filed_work(asker, has_parent, Some(&decoded(data)), relating)
}

fn in_my_work(data: &Value) -> bool {
    super::in_my_work(&decoded(data))
}

fn plan_chain(session: &Session, asked: &str, data: &Value, blocks: &[(String, String)]) -> Result<Chain, String> {
    super::plan_chain(session, asked, &decoded(data), blocks)
}

// `read` as `Others` answers it, by alias.
fn resolved_relations(read: &Value, wanted: &[(&'static str, Relation, String)]) -> Result<Vec<(&'static str, Relation, String, String)>, Error> {
    let found: std::collections::HashMap<String, Option<IssueRef>> = decoded(read);
    super::resolved_relations(&(0..wanted.len()).map(|i| found.get(&format!("o{i}")).cloned().flatten()).collect::<Vec<_>>(), wanted)
}

fn target(parent: Option<&str>, state: &str) -> Value {
    json!({ "viewer": { "id": APP }, "issue": { "id": "i2", "identifier": "TW-2", "team": { "id": "t", "key": "TW", "issueEstimationType": "fibonacci", "issueEstimationExtended": false, "issueEstimationAllowZero": false }, "parent": parent.map(|id| json!({ "id": id })), "state": { "type": state }, "delegate": null } })
}

#[test]
fn only_an_unfinished_sub_issue_of_a_delegated_issue_is_delegated() {
    let parent = session(vec![]);
    assert_eq!(refuse_delegation(&parent, &target(Some("i1"), "backlog")), None);
    // Not a sub-issue of the session's issue, or no parent at all.
    for other in [Some("i9"), None] {
        assert!(refuse_delegation(&parent, &target(other, "backlog")).unwrap().contains("isn't a sub-issue of TW-1"));
    }
    assert!(refuse_delegation(&parent, &target(Some("i1"), "completed")).unwrap().contains("has finished"));
    assert!(refuse_delegation(&parent, &target(Some("i1"), "triage")).unwrap().contains("is in Triage"));
    // The session's issue must be delegated to Tolkster itself.
    let undelegated = crate::request::Session { issue: Some(crate::request::Issue { delegate_id: Some("someone".into()), ..issue() }), ..session(vec![]) };
    assert!(refuse_delegation(&undelegated, &target(Some("i1"), "backlog")).unwrap().contains("isn't delegated to me"));
    let orphan = crate::request::Session { issue: None, ..session(vec![]) };
    assert!(refuse_delegation(&orphan, &target(Some("i1"), "backlog")).is_some());
    // A sub-issue delegated to someone else is left with them; one already Tolkster's is fine.
    let mut elsewhere = target(Some("i1"), "backlog");
    elsewhere["issue"]["delegate"] = json!({ "id": "someone" });
    assert!(refuse_delegation(&parent, &elsewhere).unwrap().contains("delegated to someone else"));
    elsewhere["issue"]["delegate"] = json!({ "id": APP });
    assert_eq!(refuse_delegation(&parent, &elsewhere), None);
    // A cancelled parent's chain has ended.
    let cancelled = crate::request::Session { issue: Some(crate::request::Issue { state_type: StateType::Canceled, ..issue() }), ..session(vec![]) };
    assert!(refuse_delegation(&cancelled, &target(Some("i1"), "backlog")).unwrap().contains("was cancelled"));
}

#[test]
fn a_new_issue_without_a_session_needs_a_parent_or_a_team() {
    assert!(matches!(check_create_context(&Asker::Owner, None, None), Err(Error::Usage(_))));
    assert!(check_create_context(&Asker::Owner, Some("TW-1"), None).is_ok());
    assert!(check_create_context(&Asker::Owner, None, Some("TW")).is_ok());
    // A session's issue can supply the team, so the read decides.
    assert!(check_create_context(&Asker::Session("s1".into()), None, None).is_ok());
}

#[test]
fn a_replys_turn_changes_only_its_threads_issue() {
    assert!(only_its_issue(Some("i1"), "i1", "TW-1").is_ok());
    assert!(matches!(only_its_issue(Some("i2"), "i1", "TW-1"), Err(Error::Usage(_))));
    // A session's or the owner's change names any issue.
    assert!(only_its_issue(None, "i1", "TW-1").is_ok());
}

#[test]
fn a_thread_files_a_sub_issue_of_a_delegated_issue_in_triage() {
    let thread = Asker::Thread("c1".into());
    let parent = |delegate: Option<&str>| json!({ "viewer": { "id": APP }, "issue": { "id": "i1", "team": { "id": "t", "key": "TW", "issueEstimationType": "fibonacci", "issueEstimationExtended": false, "issueEstimationAllowZero": false }, "project": null, "delegate": delegate.map(|id| json!({ "id": id })) } });
    let delegated = parent(Some(APP));
    for state in [None, Some(StateChoice::Triage)] {
        assert_eq!(filing_state(&thread, true, &delegated, state).unwrap(), Some(StateChoice::OnlyTriage));
    }
    assert!(matches!(filing_state(&thread, true, &delegated, Some(StateChoice::Named("Todo"))), Err(Error::Failed(_))));
    // No parent, a parent delegated to no one or someone else, or a session asking: as asked.
    let todo = Some(StateChoice::Named("Todo"));
    assert_eq!(filing_state(&thread, false, &delegated, todo).unwrap(), todo);
    assert_eq!(filing_state(&thread, true, &parent(None), todo).unwrap(), todo);
    assert_eq!(filing_state(&thread, true, &parent(Some("someone")), todo).unwrap(), todo);
    assert_eq!(filing_state(&Asker::Session("s1".into()), true, &delegated, todo).unwrap(), todo);
}

#[test]
fn a_thread_leaves_tolksters_work_and_what_blocks_it_alone() {
    let mine = json!({ "delegate": { "id": APP } });
    // No sub-issue and no relation read, unless one is given.
    let work = |mut issue: Value| {
        for connection in ["children", "blocking"] {
            if issue.get(connection).is_none() {
                issue[connection] = json!({ "nodes": [] });
            }
        }
        json!({ "viewer": { "id": APP }, "issue": issue })
    };
    assert!(in_my_work(&work(json!({ "delegate": { "id": APP } }))));
    assert!(in_my_work(&work(json!({ "parent": mine }))));
    // It blocks an issue delegated to Tolkster, or a sub-issue of one; or more relations than read.
    assert!(in_my_work(&work(json!({ "blocking": { "nodes": [{ "type": "blocks", "relatedIssue": mine }] } }))));
    assert!(in_my_work(&work(json!({ "blocking": { "nodes": [{ "type": "blocks", "relatedIssue": { "parent": mine } }] } }))));
    assert!(in_my_work(&work(json!({ "blocking": { "nodes": [], "pageInfo": { "hasNextPage": true } } }))));
    // A parent of one, which a team's auto-close cancels with it, or with more sub-issues than read.
    assert!(in_my_work(&work(json!({ "children": { "nodes": [mine] } }))));
    assert!(in_my_work(&work(json!({ "children": { "nodes": [], "pageInfo": { "hasNextPage": true } } }))));
    // Someone else's, a relation that blocks nothing, or no such issue: outside it.
    assert!(!in_my_work(&work(json!({ "delegate": { "id": "someone" }, "parent": { "delegate": null } }))));
    assert!(!in_my_work(&work(json!({ "blocking": { "nodes": [{ "type": "related", "relatedIssue": mine }], "pageInfo": { "hasNextPage": false } } }))));
    assert!(!in_my_work(&json!({ "viewer": { "id": APP }, "issue": null })));
}

// A sub-issue of TW-1 as `issue chain` reads it, blocked by `blockers` as (identifier, state).
fn child(n: u32, state: &str, blockers: &[(&str, &str)]) -> Value {
    let nodes: Vec<Value> = blockers.iter().map(|(i, s)| json!({ "id": format!("r-{n}-{i}"), "type": "blocks", "issue": { "identifier": i, "state": { "type": s } } })).collect();
    json!({ "id": format!("i{n}"), "identifier": format!("TW-{n}"), "state": { "type": state }, "inverseRelations": { "nodes": nodes, "pageInfo": { "hasNextPage": false } } })
}

fn parent(children: Vec<Value>) -> Value {
    json!({ "viewer": { "id": APP }, "issue": { "id": "i1", "identifier": "TW-1", "children": { "nodes": children, "pageInfo": { "hasNextPage": false } } } })
}

fn blocks(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(s, b)| (s.to_string(), b.to_string())).collect()
}

fn delegated(chain: &Chain) -> Vec<&str> {
    chain.delegate.iter().map(|(_, i)| i.as_str()).collect()
}

#[test]
fn a_chain_adds_the_missing_blocks_and_delegates_the_unblocked_first() {
    // TW-4 waits on TW-2 already; TW-5 is done; TW-3 is to wait on TW-2 as well.
    let data = parent(vec![child(4, "backlog", &[("TW-2", "started")]), child(2, "backlog", &[]), child(3, "backlog", &[]), child(5, "completed", &[])]);
    let chain = plan_chain(&session(vec![]), "TW-1", &data, &blocks(&[("TW-3", "TW-2"), ("TW-4", "TW-2"), ("TW-3", "TW-2")])).unwrap();
    assert_eq!(chain.blocks, [(("i3".to_string(), "TW-3".to_string()), ("i2".to_string(), "TW-2".to_string()))]);
    assert_eq!(delegated(&chain), ["TW-2", "TW-3", "TW-4"]);
    // No blocks: every unfinished sub-issue starts together, and one blocked from
    // outside the parent goes last.
    let outside = parent(vec![child(3, "backlog", &[("TW-9", "started"), ("TW-8", "completed")]), child(2, "backlog", &[])]);
    assert_eq!(delegated(&plan_chain(&session(vec![]), "TW-1", &outside, &[]).unwrap()), ["TW-2", "TW-3"]);
}

#[test]
fn a_chain_leaves_a_sub_issue_delegated_elsewhere_alone() {
    let mut elsewhere = child(3, "backlog", &[]);
    elsewhere["delegate"] = json!({ "id": "someone" });
    let mut ours = child(4, "backlog", &[]);
    ours["delegate"] = json!({ "id": APP });
    let mut finished = child(5, "completed", &[]);
    finished["delegate"] = json!({ "id": "someone" });
    let data = parent(vec![child(2, "backlog", &[]), elsewhere, ours, finished]);
    let chain = plan_chain(&session(vec![]), "TW-1", &data, &[]).unwrap();
    assert_eq!(delegated(&chain), ["TW-2", "TW-4"]);
    assert_eq!(chain.skipped, ["TW-3"]);
    // A block on a skipped sub-issue is still added: only the delegation is left alone.
    let chain = plan_chain(&session(vec![]), "TW-1", &data, &blocks(&[("TW-2", "TW-3")])).unwrap();
    assert_eq!(chain.blocks.len(), 1);
    assert_eq!(delegated(&chain), ["TW-4", "TW-2"]);
    // Nothing left to delegate but the skipped one.
    let alone = parent(vec![data["issue"]["children"]["nodes"][1].clone()]);
    let chain = plan_chain(&session(vec![]), "TW-1", &alone, &[]).unwrap();
    assert!(chain.delegate.is_empty());
    assert_eq!(chain.skipped, ["TW-3"]);
}

#[test]
fn a_chain_leaves_alone_a_sub_issue_already_blocked_by_one_in_triage() {
    // TW-3 waits on TW-2 in Triage, TW-4 on TW-3, TW-5 on nothing; TW-6 waits on a started TW-5.
    let data = parent(vec![child(2, "triage", &[]), child(3, "backlog", &[("TW-2", "triage")]), child(4, "backlog", &[("TW-3", "backlog")]), child(5, "backlog", &[]), child(6, "backlog", &[("TW-5", "started")])]);
    let chain = plan_chain(&session(vec![]), "TW-1", &data, &[]).unwrap();
    assert_eq!(delegated(&chain), ["TW-5", "TW-6"]);
    assert_eq!(chain.skipped, ["TW-2", "TW-3", "TW-4"]);
    // An issue outside the sub-issues, in Triage, holds the same.
    let outside = parent(vec![child(3, "backlog", &[("TW-9", "triage")])]);
    let chain = plan_chain(&session(vec![]), "TW-1", &outside, &[]).unwrap();
    assert!(chain.delegate.is_empty());
    assert_eq!(chain.skipped, ["TW-3"]);
}

#[test]
fn a_chain_leaves_a_sub_issue_in_triage_alone() {
    let data = parent(vec![child(2, "backlog", &[]), child(3, "triage", &[]), child(4, "triage", &[]), child(5, "completed", &[])]);
    let chain = plan_chain(&session(vec![]), "TW-1", &data, &[]).unwrap();
    assert_eq!(delegated(&chain), ["TW-2"]);
    assert_eq!(chain.skipped, ["TW-3", "TW-4"]);
    // A block either way on one in Triage is refused: it would hold the end until someone accepts it.
    assert!(plan_chain(&session(vec![]), "TW-1", &data, &blocks(&[("TW-2", "TW-3")])).unwrap_err().contains("TW-3 is in Triage"));
    assert!(plan_chain(&session(vec![]), "TW-1", &data, &blocks(&[("TW-3", "TW-2")])).unwrap_err().contains("TW-3 is in Triage"));
    // Nothing left to delegate but those in Triage.
    let alone = parent(vec![child(3, "triage", &[])]);
    let chain = plan_chain(&session(vec![]), "TW-1", &alone, &[]).unwrap();
    assert!(chain.delegate.is_empty());
    assert_eq!(chain.skipped, ["TW-3"]);
}

#[test]
fn a_cancelled_parent_delegates_nothing() {
    let data = parent(vec![child(2, "backlog", &[])]);
    let cancelled = crate::request::Session { issue: Some(crate::request::Issue { state_type: StateType::Canceled, ..issue() }), ..session(vec![]) };
    assert!(plan_chain(&cancelled, "TW-1", &data, &[]).unwrap_err().contains("was cancelled"));
}

#[test]
fn a_chain_refuses_before_it_writes_anything() {
    let data = parent(vec![child(2, "backlog", &[("TW-3", "started")]), child(3, "backlog", &[]), child(4, "completed", &[])]);
    let refused = |pairs: &[(&str, &str)]| plan_chain(&session(vec![]), "TW-1", &data, &blocks(pairs)).unwrap_err();
    assert!(refused(&[("TW-9", "TW-2")]).contains("TW-9 isn't a sub-issue of TW-1"));
    assert!(refused(&[("TW-2", "TW-9")]).contains("TW-9 isn't a sub-issue of TW-1"));
    assert!(refused(&[("TW-2", "TW-4")]).contains("TW-4 has finished"));
    assert!(refused(&[("TW-2", "TW-2")]).contains("can't wait for itself"));
    // TW-2 already waits on TW-3, so TW-3 can't wait on TW-2.
    assert!(refused(&[("TW-3", "TW-2")]).contains("loop through TW-2, TW-3"));
    // A loop already among the sub-issues is refused even with no block to add.
    let looped = parent(vec![child(2, "backlog", &[("TW-3", "started")]), child(3, "backlog", &[("TW-2", "started")])]);
    assert!(plan_chain(&session(vec![]), "TW-1", &looped, &[]).unwrap_err().contains("already wait on each other"));
    // Only from the session's own issue, delegated to Tolkster.
    let other = json!({ "viewer": { "id": APP }, "issue": { "id": "i7", "identifier": "TW-7", "children": { "nodes": [], "pageInfo": { "hasNextPage": false } } } });
    assert!(plan_chain(&session(vec![]), "TW-7", &other, &[]).unwrap_err().contains("TW-7 isn't the session's issue"));
    let undelegated = crate::request::Session { issue: Some(crate::request::Issue { delegate_id: Some("someone".into()), ..issue() }), ..session(vec![]) };
    assert!(plan_chain(&undelegated, "TW-1", &data, &[]).unwrap_err().contains("isn't delegated to me"));
    // Past what one read holds, it can't check a loop, so it refuses.
    let mut paged = data.clone();
    paged["issue"]["children"]["pageInfo"]["hasNextPage"] = json!(true);
    assert!(plan_chain(&session(vec![]), "TW-1", &paged, &[]).unwrap_err().contains("more sub-issues"));
    let mut paged = data.clone();
    paged["issue"]["children"]["nodes"][0]["inverseRelations"]["pageInfo"]["hasNextPage"] = json!(true);
    assert!(plan_chain(&session(vec![]), "TW-1", &paged, &[]).unwrap_err().contains("TW-2 has more relations"));
}

#[test]
fn a_block_names_two_plain_identifiers() {
    assert_eq!(parse_blocks(&["tw-3:TW-2".into()]).unwrap(), blocks(&[("TW-3", "TW-2")]));
    for bad in ["TW-3", "TW-3:", "TW-3:TW-2:TW-1", "TW-3;TW-2", "TW-3:$x"] {
        assert!(matches!(parse_blocks(&[bad.into()]), Err(Error::Usage(_))), "{bad}");
    }
}

// TW-1 as a middle issue: a sub-issue of TW-0, which Tolkster's chain delegated, on the
// session Linear opened for that delegation, with the root as `root` makes it.
fn middle(root: impl FnOnce(&mut crate::request::Parent)) -> crate::request::Session {
    let mut parent = crate::request::Parent { id: "i0".into(), identifier: "TW-0".into(), state_type: StateType::Started, delegate_id: Some(crate::request::tests::APP.into()), priority: 0, undelegated_at: None, canceled_at: None, updated_at: None, delegated_by: None };
    root(&mut parent);
    let delegation = Delegation { issue_id: "i1".into(), parent_issue_id: "i0".into(), session_id: "s1".into(), at: crate::request::tests::at(0) };
    crate::request::Session { creator_id: None, issue: Some(crate::request::Issue { parent: Some(parent), ..issue() }), delegation: Some(delegation), ..session(vec![]) }
}

#[test]
fn a_middle_issues_chain_holds_on_its_chained_session_only_while_the_root_does() {
    let owner = crate::request::tests::OWNER;
    let data = parent(vec![child(2, "backlog", &[]), child(3, "backlog", &[])]);
    let chained = middle(|_| {});
    assert!(chained.is_owners(owner));
    assert_eq!(delegated(&plan_chain(&chained, "TW-1", &data, &[]).unwrap()), ["TW-2", "TW-3"]);
    // The root taken back, or cancelled, ends the middle issue's trust, so `trusted_session`
    // refuses its `issue chain` before the plan.
    for ended in [middle(|p| p.delegate_id = None), middle(|p| p.delegate_id = Some("someone".into())), middle(|p| p.state_type = StateType::Canceled)] {
        assert!(!ended.is_owners(owner));
    }
    // A record for another parent, as after the middle issue moved, trusts nothing.
    assert!(!middle(|p| p.id = "i9".into()).is_owners(owner));
}

fn create_args(args: &[&str]) -> Result<Vec<(&'static str, Relation, String)>, Error> {
    let argv: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    wanted_relations(&parse(&argv, CREATE, false).unwrap())
}

#[test]
fn a_create_takes_each_relation_repeated_and_mixed() {
    let wanted = create_args(&["--title", "x", "--related", "TW-1", "--blocked-by", "TW-3", "--related", "TW-2", "--related", "tw-1", "--blocks", "TW-4"]).unwrap();
    let names: Vec<_> = wanted.iter().map(|(name, _, other)| (*name, other.as_str())).collect();
    // The table's order, each pair once.
    assert_eq!(names, [("blocks", "TW-4"), ("blocked-by", "TW-3"), ("related", "TW-1"), ("related", "TW-2")]);
    assert!(create_args(&["--title", "x"]).unwrap().is_empty());
    assert!(matches!(create_args(&["--title", "x", "--blocks", " "]), Err(Error::Usage(_))));
}

#[test]
fn an_unknown_issue_files_nothing() {
    let wanted = create_args(&["--title", "x", "--related", "TW-1", "--blocks", "TW-9"]).unwrap();
    let read = json!({ "o0": { "id": "i9", "identifier": "TW-9" }, "o1": null });
    assert!(matches!(resolved_relations(&read, &wanted), Err(Error::Failed(_))));
    let twice = create_args(&["--title", "x", "--related", "TW-1", "--blocks", "tw-1"]).unwrap();
    let read = json!({ "o0": { "id": "i1", "identifier": "TW-1" }, "o1": { "id": "i1", "identifier": "TW-1" } });
    assert!(matches!(resolved_relations(&read, &twice), Err(Error::Usage(_))));
    let read = json!({ "o0": { "id": "i9", "identifier": "TW-9" }, "o1": { "id": "i1", "identifier": "TW-1" } });
    let resolved = resolved_relations(&read, &wanted).unwrap();
    assert_eq!(resolved.iter().map(|(_, _, id, identifier)| (id.as_str(), identifier.as_str())).collect::<Vec<_>>(), [("i9", "TW-9"), ("i1", "TW-1")]);
}

#[test]
fn a_failed_relation_stops_the_rest_and_a_retry_resumes_after_the_made() {
    let relation = |id: &str| ("related", Relation::Related, id.to_string(), id.to_string());
    let relations = [relation("a"), relation("b"), relation("c")];
    let (mut made, mut tried) = (0, Vec::new());
    let result = relate_all(&relations, &mut made, |_, id| {
        tried.push(id.to_string());
        if id == "b" { Err(Error::Failed("no".into())) } else { Ok(()) }
    });
    assert!(result.is_err());
    assert_eq!((made, tried.as_slice()), (1, ["a".to_string(), "b".to_string()].as_slice()));
    // The same run over again, a rejected token's retry, relates a second time nothing already made.
    let mut tried = Vec::new();
    relate_all(&relations, &mut made, |_, id| {
        tried.push(id.to_string());
        Ok(())
    })
    .unwrap();
    assert_eq!((made, tried.as_slice()), (3, ["b".to_string(), "c".to_string()].as_slice()));
}

#[test]
fn a_thread_relates_nothing_it_files_into_tolksters_work() {
    let follows = |delegate: Value| json!({ "viewer": { "id": APP }, "issue": { "id": "i1", "team": { "id": "t", "key": "TW", "issueEstimationType": "fibonacci", "issueEstimationExtended": false, "issueEstimationAllowZero": false }, "project": null, "delegate": delegate } });
    let thread = Asker::Thread("c1".into());
    let mine = follows(json!({ "id": APP }));
    assert!(matches!(refuse_relating_filed_work(&thread, true, &mine, true), Err(Error::Failed(_))));
    // Nothing to relate, no parent, a parent delegated to no one or someone else, or a session asking.
    assert!(refuse_relating_filed_work(&thread, true, &mine, false).is_ok());
    assert!(refuse_relating_filed_work(&thread, false, &mine, true).is_ok());
    assert!(refuse_relating_filed_work(&thread, true, &follows(Value::Null), true).is_ok());
    assert!(refuse_relating_filed_work(&thread, true, &follows(json!({ "id": "someone" })), true).is_ok());
    assert!(refuse_relating_filed_work(&Asker::Session("s1".into()), true, &mine, true).is_ok());
}
