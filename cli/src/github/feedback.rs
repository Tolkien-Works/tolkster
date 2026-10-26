// What `tolkster pr feedback` reads of a PR (`feedback`): its conversation comments, reviews
// and review threads, sifted to what blessed accounts wrote.

use serde::Deserialize;
use serde_json::{Value, json};

use super::cost::site;
use super::{Author, Newest, OnePr, Reader, blessed, bot_author, connection, has_body, login, published, read_older, repo_variables};
use crate::error::Error;
use crate::json::decode;
use crate::time::instant;

// What `pr feedback` reads of a PR: its conversation comments, reviews and review
// threads, each thread with its newest 100 comments.
const FEEDBACK: [(&str, &str); 3] = [
    ("comments", "author { login } createdAt url body"),
    ("reviews", "author { login } state submittedAt url body"),
    ("reviewThreads", "id isResolved isOutdated path line comments(last: 100) { nodes { author { login } createdAt publishedAt body pullRequestReview { state } } pageInfo { hasPreviousPage } }"),
];

// The PR as `feedback` reads it: when it opened, its author and FEEDBACK's connections.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeedbackPr {
    created_at: String,
    author: Option<Author>,
    comments: Newest<FeedbackComment>,
    reviews: Newest<FeedbackReview>,
    review_threads: Newest<FeedbackThread>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeedbackComment {
    author: Option<Author>,
    created_at: String,
    url: String,
    body: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeedbackReview {
    author: Option<Author>,
    state: String,
    // Null while the review is pending.
    submitted_at: Option<String>,
    url: String,
    body: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeedbackThread {
    id: String,
    is_resolved: bool,
    is_outdated: bool,
    path: String,
    // Null once the line is gone from the diff.
    line: Option<u64>,
    comments: Newest<ThreadComment>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadComment {
    author: Option<Author>,
    created_at: String,
    published_at: Option<String>,
    body: String,
    pull_request_review: Option<ReviewState>,
}

#[derive(Debug, Deserialize)]
struct ReviewState {
    state: String,
}

// What `tolkster pr feedback` prints: the PR's feedback from blessed accounts (config.json's
// `review.reviewers`) and nobody else, so no one else's text reaches the worker: the conversation
// comments; the reviews, each account's latest with a body and every review of any account
// newer than `since` (None: all of them); and the unresolved review threads, with their IDs
// for `pr reply` and `pr resolve`, keeping only blessed comments. A thread is a `followUp`
// when the review that opened it approved the PR, read from the review's state, never the
// text (cli/briefs/round.md). Every page is read, and a PR with more fails: feedback that
// woke the worker is never left out, so the round isn't taken as handled. With it, the time
// the read reads through (`read_through`).
pub fn feedback(gh: &mut dyn Reader, repo: &str, number: u64, listed: &[String], since: Option<i64>) -> Result<(Value, Option<i64>), Error> {
    let _site = site("feedback");
    let first: Vec<String> = FEEDBACK.iter().map(|(name, fields)| connection(name, fields, false)).collect();
    let query = format!("query($owner: String!, $name: String!, $number: Int!) {{ repository(owner: $owner, name: $name) {{ pullRequest(number: $number) {{ createdAt author {{ __typename login }} {} }} }} }}", first.join(" "));
    let data = gh.graphql(repo, &query, repo_variables(repo, json!({ "number": number }))?)?;
    let read: OnePr<FeedbackPr> = decode(data, "GitHub's answer to the feedback query")?;
    let mut pr = read.pr().ok_or_else(|| Error::Failed(format!("{repo}#{number} is not a pull request")))?;
    // Through the first query alone: an older page read later can hold a reply newer than
    // one that landed on the first page after it was read.
    let through = read_through(&pr);
    let [comments, reviews, threads] = FEEDBACK;
    read_older(gh, repo, number, comments, &mut pr.comments)?;
    read_older(gh, repo, number, reviews, &mut pr.reviews)?;
    read_older(gh, repo, number, threads, &mut pr.review_threads)?;
    Ok((sift_feedback(&pr, number, listed, since)?, through))
}

// GitHub's time of the newest feedback in a read of it, from anyone, or of the PR's
// opening when it has none: what a round that read it has read (review::Since). GitHub's
// times, never the workstation's clock, which could run ahead and hide a later review.
fn read_through(pr: &FeedbackPr) -> Option<i64> {
    let comments = pr.comments.nodes.iter().filter_map(|c| instant(&c.created_at));
    let reviews = pr.reviews.nodes.iter().filter_map(|r| r.submitted_at.as_deref().and_then(instant));
    let threads = pr.review_threads.nodes.iter().flat_map(|t| &t.comments.nodes).filter_map(|c| published(c.published_at.as_deref(), &c.created_at));
    comments.chain(reviews).chain(threads).chain(instant(&pr.created_at)).max()
}

fn sift_feedback(pr: &FeedbackPr, number: u64, listed: &[String], since: Option<i64>) -> Result<Value, Error> {
    let bot = bot_author(&pr.author);
    let is = |a: Option<&str>, b: Option<&str>| a.zip(b).is_some_and(|(a, b)| a.eq_ignore_ascii_case(b));
    let blessed_login = |author: &Option<Author>| blessed(login(author), bot, listed);

    let comments: Vec<Value> = pr
        .comments
        .nodes
        .iter()
        .filter(|c| blessed_login(&c.author))
        .map(|c| json!({ "author": login(&c.author), "createdAt": c.created_at, "url": c.url, "body": c.body }))
        .collect();

    // A review that counts as feedback (`review::feedback_from`) and is newer than what the
    // read records past (`read_through`) is printed, whatever its account's later reviews.
    // Older ones only as each account's latest with a body, as the worker has read the rest.
    let unread = |r: &FeedbackReview| since.is_none_or(|read| r.submitted_at.as_deref().and_then(instant).is_some_and(|at| at > read));
    let counted: Vec<&FeedbackReview> = pr.reviews.nodes.iter().filter(|r| blessed_login(&r.author) && (has_body(&r.body) || (unread(r) && r.state == "CHANGES_REQUESTED"))).collect();
    let reviews: Vec<Value> = counted
        .iter()
        .enumerate()
        .filter(|(i, r)| unread(r) || !counted[i + 1..].iter().any(|later| has_body(&later.body) && is(login(&later.author), login(&r.author))))
        .map(|(_, r)| json!({ "author": login(&r.author), "state": r.state, "submittedAt": r.submitted_at, "url": r.url, "body": r.body }))
        .collect();
    let open: Vec<&FeedbackThread> = pr.review_threads.nodes.iter().filter(|t| !t.is_resolved).collect();
    // A thread's newest 100 comments are read. One with more would hide the older ones.
    if let Some(long) = open.iter().find(|t| t.comments.page_info.has_previous_page) {
        return Err(Error::Failed(format!("review thread {} on PR #{number} has over 100 comments", long.id)));
    }
    let threads: Vec<Value> = open
        .iter()
        // Any blessed comment keeps a thread, outdated ones included; review.rs wakes the worker
        // only for one that isn't outdated and whose latest comment is blessed.
        .filter(|t| t.comments.nodes.iter().any(|c| blessed_login(&c.author)))
        .map(|t| {
            let kept = t.comments.nodes.iter().filter(|c| blessed_login(&c.author));
            let comments: Vec<Value> = kept.map(|c| json!({ "author": login(&c.author), "createdAt": c.created_at, "body": c.body })).collect();
            // The thread's first comment is the review's own; a reply's is its own review,
            // COMMENTED. Only a blessed account's approval counts: a stranger's thread that a
            // blessed account replied in was opened by nobody the worker files for.
            let opened = t.comments.nodes.first().filter(|c| blessed_login(&c.author));
            let follow_up = opened.and_then(|c| c.pull_request_review.as_ref()).is_some_and(|r| r.state == "APPROVED");
            json!({ "id": t.id, "path": t.path, "line": t.line, "outdated": t.is_outdated, "followUp": follow_up, "comments": comments })
        })
        .collect();

    Ok(json!({ "pr": number, "comments": comments, "reviews": reviews, "threads": threads }))
}

#[cfg(test)]
mod tests;
