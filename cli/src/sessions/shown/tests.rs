use super::*;

#[test]
fn the_query_reads_only_what_the_flags_ask_for() {
    let query = |comments, children, attachments| Show::new(comments, children, attachments).0;
    let bare = query(false, false, false);
    assert!(bare.contains("issue(id: $id)") && bare.contains("inverseRelations"));
    assert!(!bare.contains("comments(") && !bare.contains("children(") && !bare.contains("attachments("));
    assert!(query(true, false, false).contains("comments(first: 250)"));
    let with_children = query(false, true, false);
    assert!(with_children.contains("children(first: 250)") && with_children.matches("attachments(first: 50)").count() == 1);
    assert_eq!(query(false, false, true).matches("attachments(first: 50)").count(), 1);
    assert_eq!(query(true, true, true).matches("attachments(first: 50)").count(), 2);
}

#[test]
fn the_issue_is_read_through_the_session_with_its_comments() {
    let query = session_query();
    assert!(query.contains("agentSession(id: $id) { issue {") && query.contains("comments(first: 250)"));
    assert!(!query.contains("children(") && !query.contains("attachments("));
}

#[test]
fn a_search_reads_through_linears_search_and_a_listing_newest_first() {
    let search = list_query(true);
    assert!(search.contains("searchIssues(term: $term, filter: $filter, first: $first)") && search.contains("$term: String!"));
    assert!(!search.contains("orderBy"));
    let listing = list_query(false);
    assert!(listing.contains("issues(filter: $filter, first: $first, orderBy: updatedAt)") && !listing.contains("$term"));
}
