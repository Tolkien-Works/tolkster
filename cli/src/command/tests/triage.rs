use super::*;

// A triage start (docs/triage.md) on the fakes: the issues asked, then the queue, oldest
// first, in one session in the repo's primary checkout.

fn in_triage(id: &str, identifier: &str, state: &str, creator: &str, created: u32) -> (Id, crate::ports::Candidate) {
    let node = json!({ "identifier": identifier, "createdAt": at(created), "state": { "type": state }, "team": { "key": "TW", "issueEstimationType": "notUsed", "issueEstimationExtended": false, "issueEstimationAllowZero": false }, "project": { "id": "p1", "name": "Tolkster" }, "creator": { "id": creator } });
    (id.into(), crate::triage::parse_candidate(&crate::json::decode(node, "Linear's answer to TriageQueue").unwrap()).unwrap())
}

fn start_triage(asked: &[&str]) -> Command {
    order("c-triage", Order::StartTriage { issue_ids: asked.iter().map(|id| Id::from(*id)).collect() })
}

#[test]
fn a_triage_start_reads_the_asked_issues_and_their_histories_once_each_beside_the_queue() {
    let config = fake::config(&briefs("triage-start"));
    let (linear, records, world) = fakes();
    // TW-9 is asked about, a newer issue than the queue's, and still goes first.
    *linear.triage_issues.borrow_mut() = vec![in_triage("i2", "TW-2", "triage", APP, 2), in_triage("i1", "TW-1", "triage", OWNER, 1), in_triage("i9", "TW-9", "triage", OWNER, 9)];
    let started = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start_triage(&["i9"])).unwrap();
    assert_eq!(started.outcome, Outcome::Ok);
    assert_eq!(started.detail["session"]["issues"], json!(["TW-9", "TW-1", "TW-2"]));
    assert_eq!(started.detail["waiting"], json!([]));
    assert_eq!(*linear.triage_reads.borrow(), ["queue", "issues", "histories"]);
}

#[test]
fn a_triage_start_leaves_out_what_may_not_be_triaged_and_says_what_still_waits() {
    let mut config = fake::config(&briefs("triage-waiting"));
    config.triage.batch = 1;
    let (linear, records, world) = fakes();
    *linear.triage_issues.borrow_mut() = vec![
        in_triage("i1", "TW-1", "triage", APP, 1),
        in_triage("i2", "TW-2", "triage", OWNER, 2),
        in_triage("i3", "TW-3", "backlog", OWNER, 3),
        in_triage("i4", "TW-4", "triage", "someone", 4),
        in_triage("i5", "TW-5", "triage", OWNER, 5),
        in_triage("i6", "TW-6", "triage", OWNER, 6),
    ];
    // TW-5 was moved into Triage, so it takes no place in the batch of one; i7 is an issue
    // Linear has none of, TW-3 isn't in Triage and TW-4 isn't the owner's or Tolkster's.
    linear.moved.borrow_mut().insert("TW-5".into());
    let started = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start_triage(&["i3", "i4", "i5", "i7", "i2", "i6"])).unwrap();
    assert_eq!(started.detail["session"]["issues"], json!(["TW-2"]));
    // TW-6 is past the batch; TW-1, which wasn't asked about, waits in the queue.
    assert_eq!(started.detail["waiting"], json!(["i6"]));
    // The batch of one reads a history for each candidate it reaches, TW-5 and then TW-2.
    assert_eq!(*linear.triage_reads.borrow(), ["queue", "issues", "histories", "histories"]);
}

#[test]
fn a_triage_start_fails_when_linears_budget_is_spent_for_the_machine_to_send_again() {
    let config = fake::config(&briefs("triage-spent"));
    let (linear, records, world) = fakes();
    *linear.triage_issues.borrow_mut() = vec![in_triage("i1", "TW-1", "triage", APP, 1)];
    linear.spent.set(2);
    let result = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start_triage(&["i1"]));
    assert!(result.is_err_and(|error| error.rate_limited().is_some()));
}

#[test]
fn a_triage_candidate_whose_history_cant_be_read_costs_its_neighbours_nothing() {
    let config = fake::config(&briefs("triage-unreadable"));
    let (linear, records, world) = fakes();
    *linear.triage_issues.borrow_mut() = vec![in_triage("i1", "TW-1", "triage", APP, 1), in_triage("i2", "TW-2", "triage", APP, 2), in_triage("i3", "TW-3", "triage", APP, 3)];
    linear.unreadable.borrow_mut().insert("TW-2".into());
    let started = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start_triage(&[])).unwrap();
    assert_eq!(started.detail["session"]["issues"], json!(["TW-1", "TW-3"]));
}
