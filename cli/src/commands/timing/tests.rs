use super::*;

#[test]
fn the_table_has_a_column_per_group_and_a_dash_for_none() {
    let groups = [json!({ "group": "XS", "n": 1, "active": 3725.0, "phases": { "deepReviewShare": null } }), json!({ "group": "Opus 5.5", "n": 12, "active": 59.6, "phases": { "deepReviewShare": 0.25 } })];
    let rows: &[Row] = &[("issues", "/n", Unit::Count), ("active", "/active", Unit::Seconds), ("deep review", "/phases/deepReviewShare", Unit::Share)];
    assert_eq!(table(&groups, rows), ["                XS  Opus 5.5", "issues           1        12", "active       1h02m     1m00s", "deep review      -       25%"].join("\n"));
}
