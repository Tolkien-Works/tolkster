use std::collections::HashSet;

use super::*;

#[test]
fn the_first_listing_counts_a_checkout_with_nothing_open_as_closed() {
    let checkout = |worktree_id: &str| CheckoutRecord { repo_id: "r1".into(), worktree_id: worktree_id.into(), at: "2026-09-29T01:00:00.000Z".into() };
    let set = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<HashSet<String>>();
    let now = Agents { open: set(&["k1"]), terminals: set(&["term_1"]), ..Agents::default() };
    assert_eq!(missed_closed(&[checkout("k1"), checkout("k2")], &now), ["k2"]);
    assert!(missed_closed(&[], &now).is_empty());
}
