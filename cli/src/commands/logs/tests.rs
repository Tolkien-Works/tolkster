use super::*;

const NOW: i64 = 1_800_000_000_000;

#[test]
fn since_takes_an_instant_with_its_zone_or_a_span_back_from_now() {
    assert_eq!(since_ms("2026-10-22T07:30:00Z", NOW), instant("2026-10-22T07:30:00Z"));
    assert_eq!(since_ms("30m", NOW), Some(NOW - 30 * 60_000));
    assert_eq!(since_ms("6h", NOW), Some(NOW - 6 * 60 * 60_000));
    assert_eq!(since_ms("2d", NOW), Some(NOW - 2 * DAY_MS));
    for bad in ["2026-10-22T07:30:00", "0h", "-2h", "h", "2w", "", "99999999999999999d"] {
        assert_eq!(since_ms(bad, NOW), None, "{bad}");
    }
}

#[test]
fn the_query_asks_for_events_matching_any_filter_in_the_timeframe() {
    let body = query(&[Filter { key: "issue", value: "TW-1" }, Filter { key: "issueId", value: "abc" }], 10, 20);
    assert_eq!(body["view"], "events");
    assert_eq!(body["timeframe"], json!({ "from": 10, "to": 20 }));
    assert_eq!(body["parameters"]["filterCombination"], "or");
    assert_eq!(
        body["parameters"]["filters"],
        json!([
            { "key": "issue", "operation": "eq", "type": "string", "value": "TW-1" },
            { "key": "issueId", "operation": "eq", "type": "string", "value": "abc" },
        ])
    );
}

#[test]
fn lines_are_each_event_once_oldest_first_with_scalar_fields_only() {
    let event = |id: &str, at: i64, source: Value| json!({ "$metadata": { "id": id }, "timestamp": at, "source": source });
    let events = vec![
        event("b", 2_000, json!({ "event": "activity_posted", "sessionId": "s1", "durationMs": 40 })),
        event("e", 1_500, json!({ "event": "report_received", "at": "2026-10-01T00:00:00.000Z" })),
        event("a", 1_000, json!({ "event": "request", "status": 200, "nested": { "x": 1 }, "list": [1], "seen": null })),
        event("b", 2_000, json!({ "event": "activity_posted", "sessionId": "s1", "durationMs": 40 })),
        event("c", 3_000, json!("a plain string line")),
        json!({ "$metadata": { "id": "d" }, "source": { "event": "no_time" } }),
    ];
    assert_eq!(
        lines(events),
        [
            json!({ "at": iso(1_000), "event": "request", "status": 200, "seen": null }),
            json!({ "at": iso(1_500), "event": "report_received" }),
            json!({ "at": iso(2_000), "event": "activity_posted", "sessionId": "s1", "durationMs": 40 }),
        ]
    );
}

#[test]
fn an_issues_lines_name_its_other_plain_sessions() {
    let events = [
        json!({ "source": { "event": "request", "sessionId": "s2" } }),
        json!({ "source": { "event": "request", "sessionId": "s1" } }),
        json!({ "source": { "event": "request", "sessionId": "s2" } }),
        json!({ "source": { "event": "request", "sessionId": "not plain!" } }),
        json!({ "source": { "event": "event_received" } }),
    ];
    assert_eq!(sessions_named(&events, Some("s1")).into_iter().collect::<Vec<_>>(), ["s2"]);
}
