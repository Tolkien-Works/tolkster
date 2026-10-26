use std::collections::VecDeque;
use std::time::Duration;

use super::*;

fn merged() -> Result<crate::github::AutoMerge, Error> {
    Ok(crate::github::AutoMerge { pr: 7, auto_merge: true })
}

fn turned_on(tries: Vec<Result<crate::github::AutoMerge, Error>>) -> (Result<Option<crate::github::AutoMerge>, Error>, usize, Vec<Duration>) {
    let mut tries: VecDeque<_> = tries.into();
    let mut taken = 0;
    let mut slept = Vec::new();
    let done = turn_on(
        &mut || {
            taken += 1;
            tries.pop_front().expect("try past the script")
        },
        &mut |pause| slept.push(pause),
    );
    (done, taken, slept)
}

fn spent_auto_merge(secs: u64) -> Result<crate::github::AutoMerge, Error> {
    Err(Error::RateLimited { message: "spent".into(), wait: Duration::from_secs(secs) })
}

#[test]
fn a_spent_budget_is_waited_out_and_auto_merge_turns_on() {
    let (done, taken, slept) = turned_on(vec![spent_auto_merge(600), merged()]);
    assert_eq!(done.unwrap(), Some(crate::github::AutoMerge { pr: 7, auto_merge: true }));
    assert_eq!((taken, slept), (2, vec![Duration::from_secs(600)]));
}

#[test]
fn auto_merge_that_works_the_first_time_waits_for_nothing() {
    let (done, taken, slept) = turned_on(vec![merged()]);
    assert!(done.unwrap().is_some());
    assert_eq!((taken, slept.len()), (1, 0));
}

#[test]
fn a_budget_still_spent_after_the_wait_is_no_answer_and_tries_no_third_time() {
    let (done, taken, slept) = turned_on(vec![spent_auto_merge(60), spent_auto_merge(60)]);
    assert!(done.unwrap().is_none());
    assert_eq!((taken, slept.len()), (2, 1));
}

#[test]
fn a_refusal_fails_at_once_without_waiting() {
    let (done, taken, slept) = turned_on(vec![Err(Error::Failed("auto-merge is not allowed".into()))]);
    assert!(matches!(done, Err(Error::Failed(_))));
    assert_eq!((taken, slept.len()), (1, 0));
}

#[test]
fn a_refusal_after_the_wait_is_its_error() {
    let (done, taken, slept) = turned_on(vec![spent_auto_merge(5), Err(Error::Failed("is not open: merged".into()))]);
    assert!(matches!(done, Err(Error::Failed(_))));
    assert_eq!((taken, slept.len()), (2, 1));
}

fn unavailable() -> Result<crate::github::AutoMerge, Error> {
    Err(Error::Unavailable("GitHub couldn't answer the GraphQL call: HTTP 504".into()))
}

#[test]
fn an_unavailable_github_is_tried_again_and_auto_merge_turns_on() {
    let (done, taken, slept) = turned_on(vec![unavailable(), merged()]);
    assert!(done.unwrap().is_some());
    assert_eq!((taken, slept), (2, vec![Duration::from_secs(2)]));
}

#[test]
fn an_unavailable_github_every_time_is_no_answer_after_two_retries() {
    let (done, taken, slept) = turned_on(vec![unavailable(), unavailable(), unavailable()]);
    assert!(done.unwrap().is_none());
    assert_eq!((taken, slept), (3, vec![Duration::from_secs(2), Duration::from_secs(5)]));
}

#[test]
fn an_unavailable_github_after_a_spent_budget_is_still_retried() {
    let (done, taken, _) = turned_on(vec![spent_auto_merge(60), unavailable(), merged()]);
    assert!(done.unwrap().is_some());
    assert_eq!(taken, 3);
}

#[test]
fn pr_create_reports_auto_merge_as_pr_auto_merge_would_exit() {
    let asked = std::cell::Cell::new(0);
    let ask = |told: Result<(), Error>| {
        asked.set(asked.get() + 1);
        told
    };
    let on = turned_on(vec![merged()]).0;
    assert_eq!(auto_merge_outcome(on, &mut || ask(Ok(()))), AutoMergeOutcome::On);
    assert_eq!(auto_merge_outcome(Ok(None), &mut || ask(Ok(()))), AutoMergeOutcome::Unavailable);
    assert_eq!(auto_merge_outcome(Err(Error::Failed("auto-merge is not allowed".into())), &mut || ask(Ok(()))), AutoMergeOutcome::Refused);
    assert_eq!(asked.get(), 0, "only a refusal as mergeable already asks the machine");
    // Refused as mergeable already: on once the machine is told, which merges it; refused if
    // it can't be, so the worker never says it merges by itself.
    let clean = || Ok(Some(crate::github::AutoMerge { pr: 7, auto_merge: false }));
    assert_eq!(auto_merge_outcome(clean(), &mut || ask(Ok(()))), AutoMergeOutcome::On);
    assert_eq!(auto_merge_outcome(clean(), &mut || ask(Err(Error::Failed("unreachable".into())))), AutoMergeOutcome::Refused);
    assert_eq!(asked.get(), 2);
    assert_eq!(serde_json::to_value([AutoMergeOutcome::On, AutoMergeOutcome::Refused, AutoMergeOutcome::Unavailable]).unwrap(), json!(["on", "refused", "unavailable"]));
}

#[test]
fn a_review_verdict_is_light_or_deep_and_clean_without_findings() {
    assert_eq!(verdict(Some("deep"), Some("7 findings, 6 fixed")).unwrap(), Some(("Deep review", "7 findings, 6 fixed".to_string())));
    assert_eq!(verdict(Some("light"), None).unwrap(), Some(("Light review", "clean".to_string())));
    assert_eq!(verdict(None, None).unwrap(), None);
    for (review, findings) in [(Some("skip"), None), (Some("Deep"), None), (None, Some("2 findings"))] {
        assert!(matches!(verdict(review, findings), Err(Error::Usage(_))), "{review:?} {findings:?}");
    }
}
