use super::*;
use std::cell::RefCell;

// The Durable Object's claims, by name to token, and records, as far as the CLI uses them. `ttls` is the TTL each claim was last taken with; nothing lapses by
// itself, so a test lapses a claim by removing it.
#[derive(Default)]
pub struct Fake {
    pub claims: RefCell<HashMap<String, String>>,
    pub ttls: RefCell<HashMap<String, Option<Duration>>>,
    pub changes: RefCell<Vec<(String, Change)>>,
}

impl Store for Fake {
    fn take(&self, claims: &[(String, Option<Duration>)], token: &str) -> Result<bool, Error> {
        let mut held = self.claims.borrow_mut();
        if claims.iter().any(|(name, _)| held.get(name).is_some_and(|t| t != token)) {
            return Ok(false);
        }
        for (name, ttl) in claims {
            held.insert(name.clone(), token.into());
            self.ttls.borrow_mut().insert(name.clone(), *ttl);
        }
        Ok(true)
    }

    fn release(&self, names: &[String], token: &str) -> Result<(), Error> {
        let mut held = self.claims.borrow_mut();
        for name in names {
            if held.get(name).is_some_and(|t| t == token) {
                held.remove(name);
            }
        }
        Ok(())
    }

    fn record(&self, session_id: &str, change: &Change) -> Result<Record, Error> {
        self.changes.borrow_mut().push((session_id.into(), change.clone()));
        Ok(Record::default())
    }
}





// The contract between this and the Durable Object: test/contract.json holds every
// shape as the Worker stores it, and test/state.test.ts checks the Worker accepts and
// keeps each as written. A field added on either side fails one side's test until
// the fixture, and so the other side, has it.
pub fn contract() -> Value {
    serde_json::from_str(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../test/contract.json")).unwrap()).unwrap()
}

// `value` reads as `full`, which names every field of its type, and writes back as
// itself: so the fixture has no field this can't read, and this no field the fixture
// lacks.
fn pins<T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug>(value: &Value, full: T) {
    assert_eq!(serde_json::from_value::<T>(value.clone()).unwrap(), full);
    assert_eq!(&serde_json::to_value(&full).unwrap(), value);
}

#[test]
fn the_contract_fixture_reads_and_writes_as_this_does() {
    let c = contract();
    let t = |minute: u32| format!("2026-09-26T01:{minute:02}:00.000Z");
    let read = ReadRecord { pr: 7, at: "2026-09-26T01:08:30.000Z".into() };
    let conversation = Conversation { agent_session: "0a1b2c3d-0000-4000-8000-000000000003".into(), terminal: Some("term_1".into()), at: t(11) };
    pins(&c["change"], Change { read: Some(read.clone()), conversation: Some(conversation.clone()) });
    // What the Durable Object stores once that change is applied to a session.
    pins(&c["record"], Record { reads: vec![read], conversation: Some(conversation) });
    // Each enum's every value, as the fixture lists them.
    // Each harness's effort levels, weakest first.
    let harnesses: std::collections::BTreeMap<_, _> = crate::harnesses::KNOWN.iter().map(|harness| (harness.name, harness.efforts)).collect();
    assert_eq!(c["harnesses"], serde_json::to_value(harnesses).unwrap());
    pins(&c["enums"]["rungFrom"], vec![RungFrom::Claim, RungFrom::NoEstimate, RungFrom::Worker]);
    let launch = || Launch::new(crate::harnesses::DEFAULT, "claude-opus-5-5", "medium");
    pins(&c["worker"], WorkerRecord { issue: "TW-1".into(), worktree_id: "r1::D:/wt/tw-1".into(), session_id: "s-1".into(), at: t(0), launch: launch(), rung_from: RungFrom::NoEstimate });
    pins(&c["delegation"], Delegation { issue_id: "i-2".into(), parent_issue_id: "i-1".into(), session_id: "s-1".into(), at: t(0) });
    pins(&c["checkout"], CheckoutRecord { repo_id: "r1".into(), worktree_id: "r1::D:/wt/conversations".into(), at: t(0) });
    pins(&c["contextComment"], ContextComment { issue_id: "i-1".into(), comment_id: "c0c0c0c0-0000-4000-8000-000000000001".into(), at: t(0) });
    let mut triaging = c["triaging"].clone();
    triaging["op"] = "triaging".into();
    assert_eq!(triaging_body("s-1", "TW-1"), triaging);
    // What the session's machine holds of its reader, as the Durable Object answers `session`.
    assert_eq!(serde_json::from_value::<Reading>(c["reading"].clone()).unwrap(), Reading { read_through: Some(t(1)), turn_read_through: Some(t(2)), answered_through: None });
}

#[test]
fn tokens_are_unique_and_as_the_worker_accepts_them() {
    let (a, b) = (new_token(), new_token());
    assert_ne!(a, b);
    assert!(a.len() == 32 && a.chars().all(|c| c.is_ascii_hexdigit()), "{a}");
}

#[test]
fn claim_names_hold_only_what_the_worker_accepts() {
    assert_eq!(claim_name(ClaimKind::Context, "TW-1@2026-09-26T01:00:00.000Z"), "context-TW-1@2026-09-26T01:00:00.000Z");
    assert_eq!(claim_name(ClaimKind::Comment, "TW-1 x/y"), "comment-TW-1_x_y");
}

// A Durable Object whose release fails.
struct Unreleasing(Fake);

impl Store for Unreleasing {
    fn take(&self, claims: &[(String, Option<Duration>)], token: &str) -> Result<bool, Error> {
        self.0.take(claims, token)
    }

    fn release(&self, _: &[String], _: &str) -> Result<(), Error> {
        Err(Error::Failed("the Durable Object".into()))
    }

    fn record(&self, session_id: &str, change: &Change) -> Result<Record, Error> {
        self.0.record(session_id, change)
    }
}

#[test]
fn holding_takes_the_claims_for_the_action_and_gives_them_back_only_when_it_fails() {
    let store = Fake::default();
    let claims = [("a".to_string(), None), ("b".to_string(), Some(Duration::from_secs(60)))];
    // Done: the claims stay held.
    assert_eq!(holding(&store, &claims, "t1", || Ok(7)).unwrap(), Some(7));
    assert_eq!(store.claims.borrow().len(), 2);
    // Held by another run: nothing runs.
    let mut ran = false;
    assert!(holding(&store, &claims, "t2", || { ran = true; Ok(()) }).unwrap().is_none() && !ran);
    // Failed: both claims are given back, and the error is the action's.
    store.claims.borrow_mut().clear();
    let failed = holding(&store, &claims, "t3", || Err::<(), _>(Error::Failed("Linear".into()))).unwrap_err();
    assert_eq!(failed.to_string(), "Linear");
    assert!(store.claims.borrow().is_empty());
    // Failed, and not given back: the error says both.
    let stuck = Unreleasing(Fake::default());
    let failed = holding(&stuck, &claims, "t4", || Err::<(), _>(Error::Failed("Linear".into()))).unwrap_err();
    assert_eq!(failed.to_string(), "Linear; and giving back its claim failed: the Durable Object");
}

#[test]
fn an_unanswered_chain_is_an_error_and_a_refusal_takes_none() {
    let children = vec!["c-1".to_string(), "c-2".to_string()];
    assert_eq!(chain_taken(&json!({ "taken": ["c-2", "other"] }), &children).unwrap(), vec!["c-2".to_string()]);
    assert!(chain_taken(&json!({ "taken": [] }), &children).unwrap().is_empty());
    // Exit 1, not 3: nothing was delegated because nothing was answered.
    let unanswered = chain_taken(&json!({ "taken": null }), &children).unwrap_err();
    assert!(matches!(unanswered, Error::Failed(ref m) if m.starts_with("the chain wasn't answered")));
    assert!(matches!(chain_taken(&json!({}), &children), Err(Error::Failed(_))));
}

// A warm thread turn answers the newest request it was shown, never one older than it
// started on, nor one a malformed read names.
#[test]
fn a_thread_turn_answers_the_newest_request_it_was_shown() {
    let (start, later) = ("2026-10-01T00:00:00.000Z", "2026-10-01T00:05:00.000Z");
    let floor = || Time::parse(start).unwrap();
    assert_eq!(newest_request(floor(), Some(later)).as_str(), later);
    assert_eq!(newest_request(floor(), Some("2026-09-30T00:00:00.000Z")).as_str(), start);
    assert_eq!(newest_request(floor(), None).as_str(), start);
    assert_eq!(newest_request(floor(), Some("soon")).as_str(), start);
}
