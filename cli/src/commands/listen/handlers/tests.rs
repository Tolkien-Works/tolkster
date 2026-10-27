use std::collections::HashSet;

use super::*;

#[test]
fn the_first_listing_counts_a_checkout_with_nothing_open_as_closed() {
    let checkout = |worktree_id: &str| CheckoutRecord { repo_id: "r1".into(), worktree_id: worktree_id.into(), at: "2026-09-29T01:00:00.000Z".into() };
    let now = Agents { open: HashSet::from(["r1::/k1".into()]), terminals: HashSet::from(["term_1".into()]), ..Agents::default() };
    assert_eq!(missed_closed(&[checkout("r1::/k1"), checkout("r1::/k2")], &now), ["r1::/k2"]);
    assert!(missed_closed(&[], &now).is_empty());
}
