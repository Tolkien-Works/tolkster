use super::*;

fn state(id: &str, name: &str, state_type: &str, position: f64) -> WorkflowState {
    WorkflowState { id: id.into(), name: name.into(), state_type: state_type.into(), position }
}

fn started() -> Vec<WorkflowState> {
    vec![state("doing", "In Progress", "started", 1.0), state("review", "In Review", "started", 2.0), state("qa", "QA", "started", 3.0)]
}

#[test]
fn work_starts_or_resumes_in_the_first_started_state() {
    let started = started();
    let target = |current: WorkflowState| start_target(&current, &started).unwrap().map(|s| s.id.clone());
    assert_eq!(target(state("todo", "Todo", "unstarted", 0.0)).as_deref(), Some("doing"));
    assert_eq!(target(state("backlog", "Backlog", "backlog", 0.0)).as_deref(), Some("doing"));
    // A review resume brings it back from review.
    assert_eq!(target(started[1].clone()).as_deref(), Some("doing"));
    assert_eq!(target(started[0].clone()), None);
    assert_eq!(target(state("done", "Done", "completed", 0.0)), None);
    assert_eq!(target(state("gone", "Canceled", "canceled", 0.0)), None);
    assert!(start_target(&state("todo", "Todo", "unstarted", 0.0), &[]).is_err());
}

#[test]
fn a_landing_completes_the_issue_but_never_moves_a_finished_one() {
    let completed = [state("done", "Done", "completed", 1.0), state("shipped", "Shipped", "completed", 2.0)];
    let target = |current: WorkflowState| done_target(&current, &completed).unwrap().map(|s| s.id.clone());
    assert_eq!(target(started()[1].clone()).as_deref(), Some("done"));
    assert_eq!(target(state("todo", "Todo", "unstarted", 0.0)).as_deref(), Some("done"));
    for finished in [completed[1].clone(), state("gone", "Canceled", "canceled", 0.0), state("dup", "Duplicate", "duplicate", 0.0)] {
        assert_eq!(target(finished), None);
    }
    assert!(done_target(&started()[0], &[]).is_err());
}

#[test]
fn the_teams_started_states_come_in_position_order() {
    let team = json!({ "states": { "nodes": [{ "id": "qa", "name": "QA", "type": "started", "position": 3.0 }, { "id": "doing", "name": "In Progress", "type": "started", "position": 1 }] } });
    let ids: Vec<String> = in_order(crate::json::decode::<States>(team, "Linear's answer to TeamStarted").unwrap().states.nodes).into_iter().map(|s| s.id).collect();
    assert_eq!(ids, ["doing", "qa"]);
}

#[test]
fn an_open_pr_moves_the_issue_forward_to_review_only() {
    let started = started();
    let target = |current: WorkflowState| review_target(&current, &started).map(|s| s.map(|s| s.id.clone()));
    assert_eq!(target(started[0].clone()), Ok(Some("review".into())));
    assert_eq!(target(state("todo", "Todo", "unstarted", 0.0)), Ok(Some("review".into())));
    assert_eq!(target(started[1].clone()), Ok(None));
    assert_eq!(target(started[2].clone()), Ok(None));
    assert_eq!(target(state("done", "Done", "completed", 0.0)), Ok(None));
    // Named by the team, found ignoring case.
    let custom = vec![state("doing", "Doing", "started", 1.0), state("r", "Code REVIEW", "started", 2.0)];
    assert_eq!(review_target(&custom[0], &custom).map(|s| s.map(|s| s.id.as_str())), Ok(Some("r")));
    assert_eq!(review_target(&custom[0], &custom[..1]), Err(Review::NoReviewState));
}

#[test]
fn each_verdict_moves_the_issue_to_its_own_state_or_nowhere() {
    let states = [state("triage", "Triage", "triage", 0.0), state("backlog", "Backlog", "backlog", 1.0), state("todo", "Todo", "unstarted", 2.0), state("canceled", "Canceled", "canceled", 5.0), state("dup", "Duplicate", "canceled", 6.0)];
    let target = |verdict: Verdict| verdict_target(&states, &verdict).unwrap().map(|s| s.id.as_str());
    assert_eq!(target(Verdict::Accept { estimate: Some(1), priority: 4, relations: vec![] }), Some("backlog"));
    assert_eq!(target(Verdict::Fixed { by: "abc1234".into() }), Some("canceled"));
    assert_eq!(target(Verdict::Duplicate { of: "TW-9".into() }), Some("dup"));
    assert_eq!(target(Verdict::Decline), None);
    // Without a Duplicate state, the first cancelled one; without any, an error.
    assert_eq!(verdict_target(&states[..4], &Verdict::Duplicate { of: "TW-9".into() }).unwrap().map(|s| s.id.as_str()), Some("canceled"));
    assert!(verdict_target(&states[..3], &Verdict::Duplicate { of: "TW-9".into() }).is_err());
    assert!(verdict_target(&states[..3], &Verdict::Fixed { by: "abc1234".into() }).is_err());
    assert!(verdict_target(&[states[0].clone(), states[2].clone()], &Verdict::Accept { estimate: Some(1), priority: 4, relations: vec![] }).is_err());
}

#[test]
fn a_follow_up_goes_to_triage_or_else_the_first_backlog_state() {
    let with_triage = [state("b2", "Whatever it is called", "backlog", 2.0), state("t", "Whatever it is called", "triage", 5.0), state("b1", "Whatever it is called", "backlog", 1.0), state("u", "Whatever it is called", "unstarted", 0.0)];
    let mut sorted = with_triage.to_vec();
    sorted.sort_by(|a, b| a.position.total_cmp(&b.position));
    let id = |states: &[WorkflowState], on: bool| filing_target(states, on).map(|s| s.id.clone());
    assert_eq!(id(&sorted, true).unwrap(), "t");
    // Triage off, whatever states Linear still lists.
    assert_eq!(id(&sorted, false).unwrap(), "b1");
    assert_eq!(id(&sorted[..2], true).unwrap(), "b1");
    // A state named Triage is no triage state.
    let named = [state("n", "Triage", "unstarted", 0.0), state("b1", "Backlog", "backlog", 1.0)];
    assert_eq!(id(&named, true).unwrap(), "b1");
    assert!(id(&named[..1], true).is_err());
}

#[test]
fn the_review_state_is_the_first_started_one_named_for_review() {
    let started = started();
    assert_eq!(review_state(&started).map(|s| s.id.as_str()), Some("review"));
    assert_eq!(review_state(&started[..1]), None);
}

#[test]
fn an_issue_read_without_its_team_is_not_started() {
    let issue = Issue { team_id: None, ..crate::request::tests::issue() };
    assert!(matches!(start_issue(&crate::http::agent(), "token", &Started::default(), &issue), Err(Error::Failed(_))));
}
