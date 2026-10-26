use super::*;

// The command gets the issue's size and Copilot's review as the claim read them (TW-299).
#[test]
fn the_pre_push_review_follows_the_issues_size_at_the_claim() {
    for copilot in [CopilotReview::Off, CopilotReview::OnOpening, CopilotReview::OnEachPush] {
        for review in [ReviewSize::Below, ReviewSize::At, ReviewSize::Above, ReviewSize::Unsized] {
            let text = Brief { copilot, taken: Box::leak(Box::new(Taken { review, ..taken(false, false, true).clone() })), ..brief(false, &[]) }.render();
            let args = format!("review-pass --size {} --copilot {}\n", size_name(review), copilot_name(copilot));
            assert!(text.contains(&args), "{args}");
        }
    }
}

// The claim keeps the size it read, so an estimate set after it, the worker's own
// included, never changes the brief's review (TW-299).
#[test]
fn an_estimate_set_after_the_claim_leaves_the_review_as_it_was() {
    let mut issue = Issue { estimate: Some(8.0), ..crate::request::tests::issue() };
    let at_claim = Taken { review: review_size(&issue, Size::M), ..taken(false, false, true).clone() };
    let render = |taken: &Taken| Brief { copilot: CopilotReview::Off, taken: Box::leak(Box::new(taken.clone())), ..brief(false, &[]) }.render();
    let before = render(&at_claim);
    issue.estimate = Some(1.0);
    assert_eq!(review_size(&issue, Size::M), ReviewSize::Below);
    assert_eq!(render(&at_claim), before);
    assert!(before.contains("--size above --copilot off"));
}

// A round's push follows Copilot's review (TW-240, TW-242): nothing of the worker's own
// while it reviews each push, and otherwise the light pass over what the round changed,
// led by why. The claim decides which, so no brief tells the worker to read the ruleset.
#[test]
fn a_rounds_push_follows_whether_copilot_reviews_each_push() {
    let flat = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let round = |copilot| flat(&Brief { copilot, ..brief(false, &[]) }.render_round());
    let each = round(CopilotReview::OnEachPush);
    assert!(each.contains(&flat(ROUND_PUSH_WITH_COPILOT)) && !each.contains(&flat(ROUND_PUSH_LIGHT)));
    for (copilot, lead) in [(CopilotReview::Off, NO_COPILOT_LEAD), (CopilotReview::OnOpening, OPENING_COPILOT_LEAD)] {
        let text = round(copilot);
        let light = format!("{lead} {ROUND_PUSH_LIGHT}").replace("<branchName>", "sam/tw-7-add-a-thing");
        assert!(text.contains(&flat(&light)) && !text.contains(&flat(ROUND_PUSH_WITH_COPILOT)));
    }
    for copilot in [CopilotReview::Off, CopilotReview::OnOpening, CopilotReview::OnEachPush] {
        assert!(Brief { copilot, ..brief(false, &[]) }.render().contains(&format!("--base {BASE} ")));
    }
}
