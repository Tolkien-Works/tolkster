use super::*;

#[test]
fn linear_and_github_times_compare_as_instants() {
    assert_eq!(instant("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(instant("2026-09-26T01:02:03.456Z"), Some(instant("2026-09-26T01:02:03Z").unwrap() + 456));
    assert_eq!(instant("2026-09-26T11:00:00+10:00"), instant("2026-09-26T01:00:00Z"));
    assert_eq!(instant("2000-03-01T00:00:00Z").unwrap() - instant("2000-02-28T00:00:00Z").unwrap(), 2 * 86_400_000);
    assert_eq!(instant("2026-09-28T01:00:00.5Z"), Some(instant("2026-09-28T01:00:00.000Z").unwrap() + 500));
    assert_eq!(instant("2026-09-26T23:59:60Z"), Some(instant("2026-09-26T23:59:59Z").unwrap() + 1000));
    for bad in [
        "",
        "2026-09-26",
        "2026-13-01T00:00:00Z",
        "2026-09-26T01:00:00",
        "2026-09-28 01:00:00.000Z",
        "2026-09-28T24:00:00Z",
        "2026-09-28T01:60:00Z",
        "2026-09-28T01:00:61Z",
        "2026-09-28T01:00:00.123junkZ",
        "2026-09-28T01:00.5Z",
        "2026-09-28T01:00:00+01:99",
        "2026-09-28T01:00:00+24:00",
        "2026-09-28T01:00:00+",
    ] {
        assert_eq!(instant(bad), None, "{bad}");
    }
}

// The contract with the Worker: `iso` writes the one format its TIME accepts
// (src/shared/linear.ts), which test/linear.test.ts checks against the same fixture,
// and none of the near misses the Worker refuses.
#[test]
fn the_cli_writes_the_one_format_the_worker_accepts() {
    let contract = crate::state::tests::contract();
    for case in contract["times"].as_array().unwrap() {
        let (ms, text) = (case["ms"].as_i64().unwrap(), case["iso"].as_str().unwrap());
        assert_eq!(iso(ms), text);
        assert_eq!(instant(text), Some(ms), "{text}");
    }
    for text in contract["notTimes"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()) {
        assert_ne!(instant(text).map(iso).as_deref(), Some(text), "{text} is what the CLI writes, and the Worker refuses it");
    }
}

#[test]
fn linears_times_read_back_as_iso_writes_them() {
    for ms in [0, 1_727_312_523_456, 1_790_000_000_000, 951_782_400_000, 4_102_444_799_999] {
        assert_eq!(instant(&iso(ms)), Some(ms), "{ms}");
    }
    assert_eq!(iso(1_727_312_523_456), "2024-09-26T01:02:03.456Z");
}

