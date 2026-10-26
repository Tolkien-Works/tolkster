use super::*;

fn limits(headers: &[(&str, &str)]) -> Limits {
    Limits::read(|name| headers.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string()))
}

#[test]
fn a_call_is_logged_from_the_headers_it_got() {
    let all = limits(&[
        ("x-complexity", "7"),
        ("x-ratelimit-requests-remaining", "4990"),
        ("x-ratelimit-complexity-remaining", "2999000"),
        ("x-ratelimit-requests-reset", "1790000000000"),
        ("x-ratelimit-complexity-reset", "1790000060000"),
    ]);
    assert_eq!(
        line("show.Show", &all).map(|cost| cost.to_string()).as_deref(),
        Some("linear: show.Show complexity=7 requests_remaining=4990 complexity_remaining=2999000 requests_reset=2026-09-21T14:13:20.000Z complexity_reset=2026-09-21T14:14:20.000Z")
    );
    assert_eq!(line("show.Show", &limits(&[("x-complexity", "7"), ("x-ratelimit-requests-remaining", "x")])).map(|cost| cost.to_string()).as_deref(), Some("linear: show.Show complexity=7"));
    assert_eq!(line("show.Show", &Limits::default()), None);
}

#[test]
fn a_reset_that_is_no_time_is_left_out() {
    assert_eq!(line("a.B", &limits(&[("x-complexity", "1"), ("x-ratelimit-requests-reset", "99999999999999999")])).map(|cost| cost.to_string()).as_deref(), Some("linear: a.B complexity=1"));
}

#[test]
fn the_wait_is_until_the_spent_budget_resets() {
    let now = 1_790_000_000_000;
    let spent = limits(&[("x-ratelimit-requests-remaining", "0"), ("x-ratelimit-requests-reset", "1790000090500"), ("x-ratelimit-complexity-reset", "1790000600000")]);
    assert_eq!(spent.wait(now), Duration::from_secs(91));
    let complexity = limits(&[("x-ratelimit-requests-remaining", "10"), ("x-ratelimit-complexity-remaining", "0"), ("x-ratelimit-requests-reset", "1790000090500"), ("x-ratelimit-complexity-reset", "1790000300000")]);
    assert_eq!(complexity.wait(now), Duration::from_secs(301));
    let neither = limits(&[("x-ratelimit-requests-reset", "1790000090500"), ("x-ratelimit-complexity-reset", "1790000300000")]);
    assert_eq!(neither.wait(now), Duration::from_secs(301));
}

#[test]
fn the_wait_is_bounded_and_a_minute_when_unsaid() {
    let now = 1_790_000_000_000;
    assert_eq!(Limits::default().wait(now), Duration::from_secs(60));
    assert_eq!(limits(&[("x-ratelimit-requests-remaining", "0"), ("x-ratelimit-requests-reset", "1789999990000")]).wait(now), Duration::from_secs(5));
    assert_eq!(limits(&[("x-ratelimit-requests-remaining", "0"), ("x-ratelimit-requests-reset", "1799999999999")]).wait(now), Duration::from_secs(3600));
}
