use serde_json::json;

use super::*;

// A team's estimate fields as Linear answers them.
fn team(kind: &str, extended: bool, zero: bool) -> Estimation {
    crate::json::decode(json!({ "id": "t1", "issueEstimationType": kind, "issueEstimationExtended": extended, "issueEstimationAllowZero": zero }), "Linear's answer to Team").unwrap()
}

fn scale(kind: &str, extended: bool, zero: bool) -> Scale {
    Scale::of(&team(kind, extended, zero)).unwrap()
}

#[test]
fn each_estimate_type_has_its_own_values() {
    assert_eq!(scale("exponential", false, false).values(), vec![1, 2, 4, 8, 16]);
    assert_eq!(scale("exponential", true, false).values(), vec![1, 2, 4, 8, 16, 32, 64]);
    assert_eq!(scale("fibonacci", false, false).values(), vec![1, 2, 3, 5, 8]);
    assert_eq!(scale("fibonacci", true, true).values(), vec![0, 1, 2, 3, 5, 8, 13, 21]);
    assert_eq!(scale("linear", false, true).values(), vec![0, 1, 2, 3, 4, 5]);
    assert_eq!(scale("linear", true, false).values(), vec![1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(scale("tShirt", false, false).values(), vec![1, 2, 3, 5, 8]);
    assert_eq!(scale("tShirt", true, false).values(), vec![1, 2, 3, 5, 8, 13, 21]);
    // Not used: nothing, zero or not.
    assert_eq!(scale("notUsed", true, true).values(), Vec::<u32>::new());
    // Unknown: no scale at all; unread, no team.
    assert_eq!(Scale::of(&team("cubits", false, false)), None);
    let unread = crate::json::decode::<Estimation>(json!({ "id": "t1" }), "Linear's answer to Team").unwrap_err().to_string();
    assert_eq!(unread, "Linear's answer to Team didn't decode: it had no `issueEstimationType`");
}

#[test]
fn a_size_is_the_position_on_the_teams_scale() {
    let exponential = scale("exponential", false, false);
    assert_eq!(exponential.size(1.0), Some(Size::XS));
    assert_eq!(exponential.size(4.0), Some(Size::M));
    assert_eq!(exponential.size(8.0), Some(Size::L));
    assert_eq!(exponential.size(16.0), Some(Size(5)));
    assert_eq!(exponential.size(64.0), Some(Size(7)));
    assert_eq!(exponential.size(0.0), Some(Size(0)));
    // Off the scale, from before a change of scale: the value below it.
    assert_eq!(exponential.size(5.0), Some(Size::M));
    let linear = scale("linear", false, false);
    assert_eq!(linear.size(4.0), Some(Size::L));
    assert_eq!(linear.size(5.0), Some(Size(5)));
    // An extended value keeps its size when the extension is off.
    assert_eq!(linear.size(7.0), Some(Size(7)));
    let t_shirt = scale("tShirt", false, false);
    assert_eq!(t_shirt.size(5.0), Some(Size::L));
    // Not used: no size, whatever the issue still carries.
    assert_eq!(scale("notUsed", false, false).size(5.0), None);
}

#[test]
fn only_a_value_on_the_scale_may_be_written() {
    let fibonacci = scale("fibonacci", false, false);
    assert!(fibonacci.check(5).is_ok());
    assert!(fibonacci.check(4).is_err());
    assert!(fibonacci.check(0).is_err());
    assert!(fibonacci.check(13).is_err());
    assert!(scale("fibonacci", true, true).check(0).is_ok());
    assert!(scale("fibonacci", true, true).check(13).is_ok());
    assert!(scale("exponential", false, false).check(3).is_err());
    assert!(scale("exponential", false, false).check(16).is_ok());
    assert!(scale("linear", false, false).check(4).is_ok());
    assert!(scale("tShirt", false, false).check(4).is_err());
    assert!(scale("notUsed", false, false).check(1).is_err());
}

#[test]
fn a_brief_gives_the_teams_values() {
    assert_eq!(scale("tShirt", false, false).describe().unwrap(), "1 (XS), 2 (S), 3 (M), 5 (L) or 8 (XL)");
    assert_eq!(scale("tShirt", true, true).describe().unwrap(), "0 (zero), 1 (XS), 2 (S), 3 (M), 5 (L), 8 (XL), 13 (XXL) or 21 (XXXL)");
    assert_eq!(scale("exponential", false, false).describe().unwrap(), "1, 2, 4, 8 or 16 points");
    assert_eq!(scale("linear", false, true).describe().unwrap(), "0, 1, 2, 3, 4 or 5 points");
    assert_eq!(scale("notUsed", false, false).describe(), None);
}

#[test]
fn sizes_go_by_name() {
    assert_eq!(Size::parse("xl"), Some(Size(5)));
    assert_eq!(Size::parse(" M "), Some(Size::M));
    assert_eq!(Size::parse("zero"), Some(Size(0)));
    assert_eq!(Size::parse("huge"), None);
    assert_eq!(serde_json::to_value(Size::L).unwrap(), json!("L"));
}
