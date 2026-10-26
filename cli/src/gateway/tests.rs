use super::*;

const NOW: u64 = 1_000_000;

// GitHub's GraphQL budget at `remaining` of 5000, resetting 10 minutes from NOW.
fn graphql_at(remaining: u64) -> Budgets {
    let mut budgets = Budgets::default();
    budgets.note(Api::GitHub, "graphql", Some(5000), Some(remaining), Some(NOW + 600_000), NOW);
    budgets
}

fn held_for(admitted: Result<(), Error>) -> Option<u64> {
    admitted.err().map(|error| error.rate_limited().expect("held as a spent budget is").as_secs())
}

#[test]
fn a_low_budget_lets_urgent_calls_through_and_holds_normal_reads_until_its_reset() {
    let budgets = graphql_at(999);
    assert!(budgets.admit(Api::GitHub, &["graphql"], true, Lane::Urgent, NOW).is_ok());
    assert_eq!(held_for(budgets.admit(Api::GitHub, &["graphql"], true, Lane::Normal, NOW)), Some(601));
    // A write in the normal lane goes: a job held half done would be held mid-change.
    assert!(budgets.admit(Api::GitHub, &["graphql"], false, Lane::Normal, NOW).is_ok());
    // Another budget, or another API's, says nothing of this call.
    assert!(budgets.admit(Api::GitHub, &["core"], true, Lane::Normal, NOW).is_ok());
    assert!(budgets.admit(Api::Linear, &["graphql"], true, Lane::Normal, NOW).is_ok());
    // At 20% it isn't low.
    assert!(graphql_at(1000).admit(Api::GitHub, &["graphql"], true, Lane::Normal, NOW).is_ok());
    // Past its reset, the next window's first call goes.
    assert!(budgets.admit(Api::GitHub, &["graphql"], true, Lane::Normal, NOW + 600_000).is_ok());
}

#[test]
fn a_spent_budget_holds_every_call_until_its_reset_without_making_it() {
    let budgets = graphql_at(0);
    for (read, lane) in [(true, Lane::Urgent), (false, Lane::Urgent), (true, Lane::Normal), (false, Lane::Normal)] {
        let held = budgets.admit(Api::GitHub, &["graphql"], read, lane, NOW).unwrap_err();
        assert_eq!(held.to_string(), "GitHub's rate limit is spent: it resets in 601 s");
    }
    assert!(budgets.admit(Api::GitHub, &["graphql"], true, Lane::Normal, NOW + 600_001).is_ok());
}

#[test]
fn a_call_spending_two_budgets_waits_for_the_later_reset_of_those_spent() {
    let mut budgets = Budgets::default();
    budgets.note(Api::Linear, "requests", Some(5000), Some(0), Some(NOW + 60_000), NOW);
    budgets.note(Api::Linear, "complexity", Some(3_000_000), Some(0), Some(NOW + 120_000), NOW);
    assert_eq!(held_for(budgets.admit(Api::Linear, &["requests", "complexity"], false, Lane::Urgent, NOW)), Some(121));
    let mut budgets = Budgets::default();
    budgets.note(Api::Linear, "requests", Some(5000), Some(10), Some(NOW + 60_000), NOW);
    budgets.note(Api::Linear, "complexity", Some(3_000_000), Some(2_000_000), Some(NOW + 120_000), NOW);
    let held = budgets.admit(Api::Linear, &["requests", "complexity"], true, Lane::Normal, NOW).unwrap_err();
    assert_eq!(held.to_string(), "Linear's requests budget is low, kept for urgent work: it resets in 61 s");
}

#[test]
fn a_late_response_never_raises_a_budget_within_its_window() {
    let mut budgets = graphql_at(900);
    // Sent before the one noted, answered after it: more left, by an older count.
    budgets.note(Api::GitHub, "graphql", Some(5000), Some(4000), Some(NOW + 600_000), NOW);
    assert!(budgets.admit(Api::GitHub, &["graphql"], true, Lane::Normal, NOW).is_err());
    // An earlier window's count is stale whatever it says.
    budgets.note(Api::GitHub, "graphql", Some(5000), Some(5000), Some(NOW + 1000), NOW);
    assert!(budgets.admit(Api::GitHub, &["graphql"], true, Lane::Normal, NOW).is_err());
    // A later window's replaces it.
    budgets.note(Api::GitHub, "graphql", Some(5000), Some(4999), Some(NOW + 4_200_000), NOW);
    assert!(budgets.admit(Api::GitHub, &["graphql"], true, Lane::Normal, NOW).is_ok());
    // A response with no count says nothing.
    budgets.note(Api::GitHub, "graphql", Some(5000), None, Some(NOW + 4_200_000), NOW);
    assert!(budgets.admit(Api::GitHub, &["graphql"], true, Lane::Normal, NOW).is_ok());
}

#[test]
fn a_spent_answer_holds_its_budgets_for_the_wait_it_gave_and_no_longer() {
    // A secondary limit: plenty left in a window 40 minutes long, and a minute to wait.
    let mut budgets = Budgets::default();
    budgets.note(Api::GitHub, "core", Some(5000), Some(4000), Some(NOW + 2_400_000), NOW);
    budgets.spent(Api::GitHub, &["core"], Duration::from_secs(60), NOW);
    assert_eq!(held_for(budgets.admit(Api::GitHub, &["core"], false, Lane::Urgent, NOW)), Some(61));
    // A response to a call admitted before it, landing late, doesn't lift the hold.
    budgets.note(Api::GitHub, "core", Some(5000), Some(3999), Some(NOW + 3_000_000), NOW);
    assert!(budgets.admit(Api::GitHub, &["core"], false, Lane::Urgent, NOW + 30_000).is_err());
    assert!(budgets.admit(Api::GitHub, &["core"], false, Lane::Urgent, NOW + 60_000).is_ok());
    // Linear's requests spent for 5 minutes hold no call for complexity's later reset.
    let mut budgets = Budgets::default();
    budgets.note(Api::Linear, "requests", Some(5000), Some(0), Some(NOW + 300_000), NOW);
    budgets.note(Api::Linear, "complexity", Some(3_000_000), Some(2_000_000), Some(NOW + 2_400_000), NOW);
    budgets.spent(Api::Linear, &["requests", "complexity"], Duration::from_secs(301), NOW);
    assert_eq!(held_for(budgets.admit(Api::Linear, &["requests", "complexity"], false, Lane::Urgent, NOW)), Some(302));
    assert!(budgets.admit(Api::Linear, &["requests", "complexity"], false, Lane::Urgent, NOW + 301_000).is_ok());
    // A budget never counted before.
    let mut budgets = Budgets::default();
    budgets.spent(Api::Linear, &["requests", "complexity"], Duration::from_secs(30), NOW);
    assert_eq!(held_for(budgets.admit(Api::Linear, &["requests", "complexity"], true, Lane::Urgent, NOW)), Some(31));
    // A count of none left holds until its window's reset, whatever the wait.
    let mut budgets = graphql_at(0);
    budgets.spent(Api::GitHub, &["graphql"], Duration::from_secs(5), NOW);
    assert_eq!(held_for(budgets.admit(Api::GitHub, &["graphql"], true, Lane::Urgent, NOW)), Some(601));
}

#[test]
fn no_count_is_believed_for_more_than_an_hour_after_it_was_said() {
    let mut budgets = Budgets::default();
    budgets.note(Api::GitHub, "core", Some(5000), Some(0), Some(4_102_444_800_000), NOW);
    assert_eq!(held_for(budgets.admit(Api::GitHub, &["core"], false, Lane::Urgent, NOW)), Some(3600));
    // A later count for the same window doesn't start the hour again.
    budgets.note(Api::GitHub, "core", Some(5000), Some(0), Some(4_102_444_800_000), NOW + 600_000);
    assert!(budgets.admit(Api::GitHub, &["core"], false, Lane::Urgent, NOW + 3_000_000).is_err());
    assert!(budgets.admit(Api::GitHub, &["core"], false, Lane::Urgent, NOW + 3_600_000).is_ok());
    // Then a count for an earlier window by its reset replaces it, and is believed.
    budgets.note(Api::GitHub, "core", Some(5000), Some(999), Some(NOW + 4_000_000), NOW + 3_600_000);
    assert!(budgets.admit(Api::GitHub, &["core"], true, Lane::Normal, NOW + 3_600_000).is_err());
    assert!(budgets.admit(Api::GitHub, &["core"], true, Lane::Urgent, NOW + 3_600_000).is_ok());
}

#[test]
fn every_wait_is_kept_between_5_s_and_an_hour() {
    assert_eq!(wait(None), Duration::from_secs(60));
    assert_eq!(wait(Some(0)), Duration::from_secs(5));
    assert_eq!(wait(Some(120)), Duration::from_secs(120));
    assert_eq!(wait(Some(u64::MAX)), Duration::from_secs(3600));
}

#[test]
fn a_lane_holds_for_what_runs_in_it_and_is_given_back_after_even_a_panic() {
    assert_eq!(lane(), Lane::Urgent);
    assert_eq!(in_lane(Lane::Normal, lane), Lane::Normal);
    assert_eq!(lane(), Lane::Urgent);
    let inner = in_lane(Lane::Normal, || (in_lane(Lane::Urgent, lane), lane()));
    assert_eq!(inner, (Lane::Urgent, Lane::Normal));
    assert!(std::panic::catch_unwind(|| in_lane(Lane::Normal, || panic!("acting"))).is_err());
    assert_eq!(lane(), Lane::Urgent);
    // A thread started inside a lane starts urgent.
    assert_eq!(in_lane(Lane::Normal, || std::thread::spawn(lane).join().unwrap()), Lane::Urgent);
}

#[test]
fn a_failure_is_spent_transient_or_refused() {
    let spent = Error::RateLimited { message: "spent".into(), wait: Duration::from_secs(9) };
    assert_eq!(Failure::of(&spent), Failure::Spent(Duration::from_secs(9)));
    assert_eq!(Failure::of(&Error::Failed("x".into())), Failure::Transient);
    assert_eq!(Failure::of(&Error::Unauthorized("x".into())), Failure::Transient);
    assert_eq!(Failure::of(&Error::Unavailable("x".into())), Failure::Transient);
    assert_eq!(Failure::of(&Error::Usage("x".into())), Failure::Refused);
}

#[test]
fn a_low_budget_is_logged_once_a_window_in_numbers() {
    let mut logged = Vec::new();
    let line = |logged: &mut Vec<_>, api, name, remaining, reset_ms, now_ms| low_line(api, name, Some(5000), Some(remaining), Some(reset_ms), now_ms, logged);
    let low = |api, budget: &str, remaining, resets_in| Some(Call::BudgetLow { api, budget: budget.into(), remaining, limit: 5000, resets_in: Duration::from_secs(resets_in) });
    assert_eq!(line(&mut logged, Api::GitHub, "graphql", 999, 1_600_000, 1_000_000), low("github", "graphql", 999, 600));
    assert_eq!(line(&mut logged, Api::GitHub, "graphql", 998, 1_600_000, 1_001_000), None);
    // Once per budget per window: again only after its reset moves.
    assert!(line(&mut logged, Api::GitHub, "graphql", 10, 5_200_000, 5_000_000).is_some());
    // Each budget, and each API's, is its own.
    assert!(line(&mut logged, Api::GitHub, "core", 10, 5_200_000, 5_000_000).is_some());
    assert_eq!(line(&mut logged, Api::Linear, "complexity", 10, 5_200_000, 5_000_000), low("linear", "complexity", 10, 200));
    // One at 20% or more isn't low, nor is one with a count missing, and wild numbers don't overflow.
    assert_eq!(line(&mut logged, Api::Linear, "requests", 1000, 9_000_000, 5_000_000), None);
    assert_eq!(low_line(Api::Linear, "requests", None, Some(0), Some(9_000_000), 5_000_000, &mut logged), None);
    assert_eq!(low_line(Api::Linear, "requests", Some(u64::MAX), Some(u64::MAX), Some(9_000_000), 5_000_000, &mut logged), None);
}

#[test]
fn the_panel_shows_each_apis_scarcest_budget_a_spent_one_first_and_none_past_its_reset() {
    let mut budgets = graphql_at(4000);
    budgets.note(Api::GitHub, "core", Some(5000), Some(1000), Some(NOW + 120_000), NOW);
    budgets.note(Api::Linear, "requests", Some(5000), Some(4500), Some(NOW + 60_000), NOW);
    let shown = budgets.shown(NOW);
    assert_eq!(shown, vec![Shown { api: "GitHub", limit: Some(5000), remaining: 1000, resets: Duration::from_secs(120), spent: false }, Shown { api: "Linear", limit: Some(5000), remaining: 4500, resets: Duration::from_secs(60), spent: false }]);
    // A spent answer outranks a lower count, and its hold is its reset.
    budgets.spent(Api::Linear, &["complexity"], Duration::from_secs(30), NOW);
    assert_eq!(budgets.shown(NOW)[1], Shown { api: "Linear", limit: None, remaining: 0, resets: Duration::from_secs(30), spent: true });
    // Two spent, it resets with the later of them, as a call waits for it.
    budgets.spent(Api::Linear, &["requests"], Duration::from_secs(90), NOW);
    assert_eq!(budgets.shown(NOW)[1].resets, Duration::from_secs(90));
    // Past every reset and hold, nothing is said.
    assert!(budgets.shown(NOW + 600_000).is_empty());
}
