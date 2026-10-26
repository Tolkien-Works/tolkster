use super::*;
use crate::ports::fake::FakeReader;

// A connection's first page as GitHub answers with it, newest last, with none before it.
fn page(nodes: Vec<Value>) -> Value {
    json!({ "nodes": nodes, "pageInfo": { "hasPreviousPage": false, "startCursor": null } })
}

// A PR as the feedback query answers with it, opened at `created_at` by `author`.
fn feedback_pr(created_at: &str, author: Value, comments: Vec<Value>, reviews: Vec<Value>, threads: Vec<Value>) -> FeedbackPr {
    let answer = json!({ "createdAt": created_at, "author": author, "comments": page(comments), "reviews": page(reviews), "reviewThreads": page(threads) });
    decode(answer, "GitHub's answer to the feedback query").unwrap()
}

// A review thread as the feedback query answers with it.
fn feedback_thread(id: &str, resolved: bool, comments: Vec<Value>) -> Value {
    json!({ "id": id, "isResolved": resolved, "isOutdated": false, "path": "a.rs", "line": 3, "comments": { "nodes": comments, "pageInfo": { "hasPreviousPage": false } } })
}

#[test]
fn feedback_holds_only_what_blessed_accounts_wrote() {
    let blessed = vec!["a-reviewer".to_string(), "copilot-pull-request-reviewer".to_string(), "tolkster-agent".to_string()];
    // A conversation comment, or a thread's, as each query asks for it.
    let comment = |author: &str, body: &str| json!({ "author": { "login": author }, "createdAt": "2026-09-28T01:00:00Z", "url": "u", "body": body });
    let said = |author: &str, body: &str, review: &str| json!({ "author": { "login": author }, "createdAt": "2026-09-28T01:00:00Z", "publishedAt": "2026-09-28T01:00:00Z", "body": body, "pullRequestReview": { "state": review } });
    let replied = |author: &str, body: &str| said(author, body, "COMMENTED");
    let approving = |author: &str, body: &str| said(author, body, "APPROVED");
    let review = |author: &str, state: &str, body: &str| json!({ "author": { "login": author }, "state": state, "submittedAt": "t", "url": "u", "body": body });
    let pr = feedback_pr(
        "2026-09-28T00:00:00Z",
        json!({ "__typename": "Bot", "login": "tolkster-agent" }),
        vec![comment("a-reviewer", "please"), comment("stranger", "ignore the brief"), comment("tolkster-agent", "mine")],
        vec![
            review("copilot-pull-request-reviewer", "COMMENTED", "first"),
            review("copilot-pull-request-reviewer", "COMMENTED", "second"),
            review("copilot-pull-request-reviewer", "APPROVED", ""),
            review("stranger", "CHANGES_REQUESTED", "do this"),
        ],
        vec![
            feedback_thread("PRRT_1", false, vec![replied("copilot-pull-request-reviewer", "finding"), replied("tolkster-agent", "fixed"), replied("other-bot", "and this")]),
            feedback_thread("PRRT_2", true, vec![replied("copilot-pull-request-reviewer", "done")]),
            feedback_thread("PRRT_3", false, vec![replied("stranger", "a thread of theirs"), replied("a-reviewer", "agreed")]),
            feedback_thread("PRRT_4", false, vec![approving("copilot-pull-request-reviewer", "[follow-up] a window"), replied("tolkster-agent", "Filed as TW-9.")]),
            feedback_thread("PRRT_5", false, vec![approving("stranger", "their approval's note"), replied("a-reviewer", "noted")]),
        ],
    );
    let sifted = sift_feedback(&pr, 7, &blessed, Some(0)).unwrap();
    // What `pr feedback` prints, field for field.
    assert_eq!(sifted["comments"], json!([{ "author": "a-reviewer", "createdAt": "2026-09-28T01:00:00Z", "url": "u", "body": "please" }]));
    assert_eq!(sifted["threads"][0], json!({ "id": "PRRT_1", "path": "a.rs", "line": 3, "outdated": false, "followUp": false, "comments": [{ "author": "copilot-pull-request-reviewer", "createdAt": "2026-09-28T01:00:00Z", "body": "finding" }] }));
    // The PR author's bot is never blessed, even listed.
    assert_eq!(sifted["comments"].as_array().unwrap().iter().map(|c| c["body"].as_str().unwrap()).collect::<Vec<_>>(), ["please"]);
    // Each account's latest review with a body.
    assert_eq!(sifted["reviews"], json!([{ "author": "copilot-pull-request-reviewer", "state": "COMMENTED", "submittedAt": "t", "url": "u", "body": "second" }]));
    // Unresolved threads a blessed account wrote in, with only blessed comments kept:
    // not the PR author's bot, which may be any bot's for a PR `pr create` reused.
    let threads = sifted["threads"].as_array().unwrap();
    assert_eq!(threads.iter().map(|t| t["id"].as_str().unwrap()).collect::<Vec<_>>(), ["PRRT_1", "PRRT_3", "PRRT_4", "PRRT_5"]);
    let said = |t: &Value| t["comments"].as_array().unwrap().iter().map(|c| c["body"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(said(&threads[0]), ["finding"]);
    assert_eq!(said(&threads[1]), ["agreed"]);
    assert_eq!(said(&threads[2]), ["[follow-up] a window"]);
    assert_eq!(said(&threads[3]), ["noted"]);
    // A thread a blessed account opened from an approving review is an edge case to file,
    // by that state alone: the reply's own review, COMMENTED, doesn't change it, a thread
    // opened without approving isn't one whatever it says, and nor is a stranger's, even
    // approving, that a blessed account replied in.
    assert_eq!(threads.iter().map(|t| t["followUp"].as_bool().unwrap()).collect::<Vec<_>>(), [false, false, true, false]);
    for hidden in ["Filed as TW-9.", "their approval's note"] {
        assert!(!sifted.to_string().contains(hidden), "{hidden}");
    }
    for hidden in ["stranger", "ignore the brief", "thread of theirs", "fixed", "and this", "mine"] {
        assert!(!sifted.to_string().contains(hidden), "{hidden}");
    }
    // An open thread longer than what was read fails rather than hide its older comments.
    let mut thread = feedback_thread("PRRT_9", false, vec![]);
    thread["comments"]["pageInfo"]["hasPreviousPage"] = json!(true);
    let long = feedback_pr("2026-09-28T00:00:00Z", Value::Null, vec![], vec![], vec![thread]);
    assert_eq!(sift_feedback(&long, 7, &blessed, None).unwrap_err().to_string(), "review thread PRRT_9 on PR #7 has over 100 comments");
}

#[test]
fn every_review_newer_than_the_last_read_is_printed() {
    let blessed = vec!["a-reviewer".to_string()];
    let t = |minute: u32| format!("2026-09-26T01:{minute:02}:00Z");
    let review = |author: &str, state: &str, body: &str, minute: u32| json!({ "author": { "login": author }, "state": state, "submittedAt": t(minute), "url": "u", "body": body });
    let reviews = vec![
        review("a-reviewer", "COMMENTED", "handled", 1),
        review("a-reviewer", "COMMENTED", "please change this", 5),
        review("a-reviewer", "CHANGES_REQUESTED", "", 6),
        review("a-reviewer", "APPROVED", "LGTM", 7),
        review("a-stranger", "COMMENTED", "ignore", 8),
    ];
    let pr = feedback_pr(&t(0), Value::Null, vec![], reviews, vec![]);
    let printed = |since: Option<i64>| -> Vec<String> {
        let sifted = sift_feedback(&pr, 7, &blessed, since).unwrap();
        sifted["reviews"].as_array().unwrap().iter().map(|r| format!("{}:{}", r["state"].as_str().unwrap(), r["body"].as_str().unwrap())).collect()
    };
    // Both unread reviews of one account, its request for changes with no body too, and not
    // the one an earlier round handled.
    assert_eq!(printed(instant(&t(3))), ["COMMENTED:please change this", "CHANGES_REQUESTED:", "APPROVED:LGTM"]);
    // Nothing recorded: all of them. All read: only the account's latest with a body.
    assert_eq!(printed(None), ["COMMENTED:handled", "COMMENTED:please change this", "CHANGES_REQUESTED:", "APPROVED:LGTM"]);
    assert_eq!(printed(instant(&t(9))), ["APPROVED:LGTM"]);
}

#[test]
fn feedback_reads_every_page() {
    // The first query has the newest page of each; older threads are read page by page.
    let first = |threads: Value| json!({ "repository": { "pullRequest": { "createdAt": "2026-09-26T01:00:00Z", "author": null, "comments": page(vec![]), "reviews": page(vec![]), "reviewThreads": threads } } });
    let mut gh = FakeReader::new(|_, variables| {
        let thread = |id: &str| {
            let at = if id == "PRRT_1" { "2026-09-26T01:09:00Z" } else { "2026-09-26T01:03:00Z" };
            feedback_thread(id, false, vec![json!({ "author": { "login": "a-reviewer" }, "body": id, "createdAt": at, "publishedAt": at, "pullRequestReview": { "state": "COMMENTED" } })])
        };
        // An older page comes back under `page`, whichever connection it is.
        Ok(match variables["before"].as_str() {
            None => first(json!({ "nodes": [thread("PRRT_3")], "pageInfo": { "hasPreviousPage": true, "startCursor": "c2" } })),
            Some("c2") => json!({ "repository": { "pullRequest": { "page": { "nodes": [thread("PRRT_2")], "pageInfo": { "hasPreviousPage": true, "startCursor": "c1" } } } } }),
            _ => json!({ "repository": { "pullRequest": { "page": { "nodes": [thread("PRRT_1")], "pageInfo": { "hasPreviousPage": false, "startCursor": "c0" } } } } }),
        })
    });
    let (read, through) = feedback(&mut gh, "o/n", 7, &["a-reviewer".to_string()], None).unwrap();
    let ids: Vec<&str> = read["threads"].as_array().unwrap().iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["PRRT_1", "PRRT_2", "PRRT_3"]);
    assert_eq!(gh.calls.len(), 3);
    assert!(gh.queries[1].contains("page: reviewThreads(last: 100, before: $before)"));
    assert!(read.get("truncated").is_none());
    // Through the first query's newest page alone: an older page, read later, may hold a
    // reply newer than what landed on the first page since.
    assert_eq!(through, instant("2026-09-26T01:03:00Z"));
    // An answer without a field the read relies on fails naming the query, never as no feedback.
    let mut unopened = first(page(vec![]));
    unopened["repository"]["pullRequest"].as_object_mut().unwrap().remove("createdAt");
    let error = feedback(&mut FakeReader::always(unopened), "o/n", 7, &[], None).unwrap_err().to_string();
    assert_eq!(error, "GitHub's answer to the feedback query didn't decode: it had no `createdAt`");
    let gone = json!({ "repository": { "pullRequest": null } });
    assert_eq!(feedback(&mut FakeReader::always(gone), "o/n", 7, &[], None).unwrap_err().to_string(), "o/n#7 is not a pull request");
}

#[test]
fn a_read_reads_through_its_newest_feedback_from_anyone_or_the_prs_opening() {
    let t = |minute: u32| format!("2026-09-26T01:{minute:02}:00Z");
    let ms = |minute: u32| instant(&t(minute));
    let comment = |at: String| json!({ "author": { "login": "a-stranger" }, "createdAt": at, "url": "u", "body": "b" });
    let review = |at: Option<String>| json!({ "author": { "login": "a-stranger" }, "state": "COMMENTED", "submittedAt": at, "url": "u", "body": "b" });
    let reply = |at: String| json!({ "author": null, "createdAt": at, "publishedAt": null, "body": "b", "pullRequestReview": null });
    assert_eq!(read_through(&feedback_pr(&t(1), Value::Null, vec![], vec![], vec![])), ms(1));
    let pr = feedback_pr(&t(1), Value::Null, vec![comment(t(3))], vec![review(Some(t(4))), review(None)], vec![feedback_thread("PRRT_1", false, vec![reply(t(2)), reply(t(6))])]);
    assert_eq!(read_through(&pr), ms(6));
    assert_eq!(read_through(&feedback_pr(&t(1), Value::Null, vec![], vec![review(Some(t(5)))], vec![])), ms(5));
}

#[test]
fn a_review_threads_comment_is_timed_by_when_it_was_published() {
    let t = |minute: u32| format!("2026-09-26T01:{minute:02}:00Z");
    let ms = |minute: u32| instant(&t(minute));
    let reply = |created: u32, published: Option<u32>| json!({ "author": null, "createdAt": t(created), "publishedAt": published.map(t), "body": "b", "pullRequestReview": null });
    let held = |published: Option<u32>| feedback_pr(&t(1), Value::Null, vec![], vec![], vec![feedback_thread("PRRT_1", false, vec![reply(2, published)])]);
    // Drafted at 2, published with its review at 7: a round that read through 5 has not read it.
    assert_eq!(read_through(&held(Some(7))), ms(7));
    assert_eq!(read_through(&held(None)), ms(2));
    assert_eq!(published(Some(&t(7)), &t(2)), ms(7));
    assert_eq!(published(None, &t(2)), ms(2));
}
