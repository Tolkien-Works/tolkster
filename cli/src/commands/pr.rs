// `tolkster pr ...`: act on a PR and read it as the GitHub App (github.rs); each fails,
// writing nothing, when the app isn't set up. `review-pass` reads git alone, before
// there is a PR.

use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use ureq::Agent;

use crate::args::{self, Kind, Parsed, parse};
use crate::cli::{home_dir, print_json, read_body};
use crate::time::{instant, iso, now_secs};
use crate::commands::asker::session_issue;
use crate::commands::link::pr_opened;
use crate::config::{self, files, owner};
use crate::review_pass::ReviewSize;
use crate::card;
use crate::error::Error;
use crate::exec::{self, run_command};
use crate::estimates::Size;
use crate::gateway::{self, Api};
use crate::git;
use crate::github::{self, AppReader, Pushed};
use crate::linear::{ActivityInput, ActivityType, with_token};
use crate::machine::merge_asked;
use crate::orca::{self, origin_repo};
use crate::plain::{Commit, Id, Sha};
use crate::post::{post_once, send};
use crate::review_pass;
use crate::sessions;
use crate::state::{Change, ReadRecord, State, Store};

const S: Kind = Kind::String;
pub const CREATE: &[(&str, Kind)] = &[("repo", S), ("head", S), ("base", S), ("title", S), ("body-file", S), ("session", S), ("auto-merge", Kind::Boolean), ("review", S), ("findings", S)];
pub const FEEDBACK: &[(&str, Kind)] = &[("repo", S), ("pr", S), ("session", S)];
pub const CHECKS: &[(&str, Kind)] = &[("repo", S), ("pr", S), ("head", S)];
pub const REPLY: &[(&str, Kind)] = &[("repo", S), ("pr", S), ("thread", S), ("body-file", S)];
pub const RESOLVE: &[(&str, Kind)] = &[("repo", S), ("pr", S), ("thread", S)];
pub const AUTO_MERGE: &[(&str, Kind)] = &[("repo", S), ("pr", S)];
pub const DESCRIBE: &[(&str, Kind)] = &[("repo", S), ("pr", S), ("body-file", S)];
pub const REVIEW_PASS: &[(&str, Kind)] = &[("size", S), ("copilot", S), ("session", S)];

// --repo, or else the current checkout's origin, so a worker never has to look it up.
fn repo(parsed: &Parsed) -> Result<String, Error> {
    match parsed.non_empty("repo") {
        Some(repo) => Ok(repo.to_string()),
        None => origin_repo(&mut run_command, None).map_err(|error| Error::Usage(format!("no --repo, and {error}"))),
    }
}

pub fn create(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, CREATE, false)?;
    let value = |name| parsed.non_empty(name);
    let (Some(head), Some(base), Some(title), Some(body_file)) = (value("head"), value("base"), value("title"), value("body-file")) else {
        return Err(Error::Usage("pr create needs --head, --base, --title and --body-file".into()));
    };
    let verdict = verdict(value("review"), value("findings"))?;
    if verdict.is_some() && value("session").is_none() {
        return Err(Error::Usage("pr create --review needs --session".into()));
    }
    let repo = repo(&parsed)?;
    let body = args::file_body(body_file, "description", read_body)?;
    let app = github::app()?;
    // The session is checked before the PR is opened: the owner's, with an issue.
    let session = match value("session") {
        Some(session_id) => {
            let owner = owner(&files()?)?;
            Some((session_id, with_token(agent, |token| session_issue(agent, token, session_id, &owner))?))
        }
        None => None,
    };
    // The review's verdict goes in before the PR opens, each verdict once in the session, so
    // a retry doesn't post it again. It never stops the PR, as the link below doesn't.
    if let (Some((action, parameter)), Some((session_id, _))) = (verdict, &session) {
        let key = format!("review {} {action} {parameter}", session_id.to_ascii_lowercase());
        if post_once(agent, &key, ActivityInput::action(session_id, action, &parameter)).is_err() {
            eprintln!("the review's verdict wasn't posted in session {session_id}");
        }
    }
    // A branch with an open PR already gives that PR, so a retry still links it and moves
    // the issue below.
    let opened = github::create_pr(agent, &app, &github::NewPr { repo: &repo, head, base, title, body: &body }, now_secs())?;
    let mut printed = json!({ "number": opened.number, "url": opened.url, "existing": opened.existing });
    if let Some((session_id, issue_id)) = &session {
        let (linked, review) = pr_opened(agent, session_id, issue_id, opened.number, &opened.url);
        printed["linked"] = json!(linked);
        printed["review"] = json!(review);
    }
    // After the link, so a spent budget's wait never holds it up. Asked again, it finds the
    // PR and turns it on again, which changes nothing once it is on.
    let merge = parsed.flag("auto-merge").then(|| {
        let on = turn_on(&mut || github::auto_merge(agent, &app, &github::PrRef { repo: &repo, pr: opened.number }, now_secs()), &mut std::thread::sleep);
        auto_merge_outcome(on, &mut || match &session {
            // The machine holds the issue's PR by its number in the checkout's own repo.
            Some(_) if parsed.non_empty("repo").is_some() => Err(Error::Usage("with --repo the merge can't be asked for: the session's issue holds the checkout's own repo's PR".into())),
            Some((session_id, issue_id)) => State::open(agent, &files()?)?.reported(Some(merge_asked(issue_id, session_id, opened.number).ok_or_else(|| Error::Failed("a merge's report of the wrong shape".into()))?)),
            None => Err(Error::Usage("without --session the merge can't be asked for".into())),
        })
    });
    if let Some(merge) = merge {
        printed["autoMerge"] = json!(merge);
    }
    print_json(&printed)?;
    let what = if merge == Some(AutoMergeOutcome::On) { "auto-merge on" } else { "opened" };
    card::checkpoint_pr(agent, opened.number, what);
    Ok(0)
}

// `pr create --review light|deep [--findings <text>]`: the pre-push review's verdict, as the
// durable action it posts, "clean" when it found nothing. None without --review, as after a
// `skip` pass.
fn verdict(review: Option<&str>, findings: Option<&str>) -> Result<Option<(&'static str, String)>, Error> {
    let action = match review {
        None if findings.is_some() => return Err(Error::Usage("--findings goes with --review".into())),
        None => return Ok(None),
        Some("light") => "Light review",
        Some("deep") => "Deep review",
        Some(_) => return Err(Error::Usage("--review must be light or deep".into())),
    };
    Ok(Some((action, findings.unwrap_or("clean").to_string())))
}

// `pr create --auto-merge`'s `autoMerge`, as `pr auto-merge` would exit: `on` for 0,
// `refused` for 1 or 2 (its reason on stderr) and `unavailable` for 8, the budget still
// spent or GitHub still not answering after the retries. A PR refused as mergeable already
// is `on` once the machine is told (`ask`), which then merges it (docs/auto-merge.md).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum AutoMergeOutcome {
    On,
    Refused,
    Unavailable,
}

fn auto_merge_outcome(on: Result<Option<github::AutoMerge>, Error>, ask: &mut dyn FnMut() -> Result<(), Error>) -> AutoMergeOutcome {
    let error = match on.map(|on| on.map(|on| on.auto_merge)) {
        Ok(Some(true)) => return AutoMergeOutcome::On,
        Ok(None) => return AutoMergeOutcome::Unavailable,
        Ok(Some(false)) => match ask() {
            Ok(()) => return AutoMergeOutcome::On,
            Err(error) => Error::Failed(format!("GitHub refused auto-merge as the PR is mergeable already, and Tolkster couldn't be asked to merge it: {error}")),
        },
        Err(error) => error,
    };
    eprintln!("{error}");
    AutoMergeOutcome::Refused
}

// With --session, a round's read: once printed, the time it reads through is recorded on
// the owner's session, so the next round's read prints only what came after. Should that
// record fail, the read still stands: the worst is feedback printed twice.
pub fn feedback(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, FEEDBACK, false)?;
    // A read is recorded by the PR's number alone, so only the checkout's own repo's: a read
    // of another repo's PR of that number can't hide this one's feedback.
    if parsed.non_empty("session").is_some() && parsed.non_empty("repo").is_some() {
        return Err(Error::Usage("--session reads the checkout's own repo: leave out --repo".into()));
    }
    let (repo, pr) = pr_ref(&parsed)?;
    let blessed = blessed(&repo)?;
    if blessed.is_empty() {
        return Err(Error::Usage("no reviewers: add \"reviewers\" to config.json, or to the repo's \"repos\" entry, the GitHub logins whose feedback counts".into()));
    }
    // The session is checked before the read: the owner's, with an issue.
    let session = match parsed.non_empty("session") {
        Some(session_id) => {
            let files = files()?;
            let owner = owner(&files)?;
            with_token(agent, |token| session_issue(agent, token, session_id, &owner))?;
            let state = State::open(agent, &files)?;
            // The session's own newest read: another session's can only make this print more.
            let since = state.read(&[session_id.to_string()])?.remove(session_id).flatten().and_then(|record| record.reads.iter().filter(|r| r.pr == pr).filter_map(|r| instant(&r.at)).max());
            Some((session_id, state, since))
        }
        None => None,
    };
    let since = session.as_ref().and_then(|(_, _, since)| *since);
    let (read, through) = github::feedback(&mut AppReader::new(agent, &|| {}), &repo, pr, &blessed, since)?;
    print_json(&read)?;
    if let (Some((session_id, state, _)), Some(through)) = (session, through) {
        let change = Change { read: Some(ReadRecord { pr, at: iso(through) }), ..Change::default() };
        if let Err(error) = state.record(session_id, &change) {
            eprintln!("couldn't record the read on session {session_id}: {error}");
        }
    }
    Ok(0)
}

// Whose feedback counts on `repo`'s PRs: the reviewers of the Orca repo of the worktree
// this runs in (Config::review_in), which must be a checkout of `repo`. Orca is asked
// only when some repo has its own, and then a checkout it can't place is an error, never
// the global reviewers, which could bless one the repo left out.
fn blessed(repo: &str) -> Result<Vec<String>, Error> {
    let files = files()?;
    let config = config::config(&files, owner(&files)?, &home_dir()?)?;
    // Only a repo's own reviewers need its checkout placed, not its own cap alone.
    if !config.repo_reviewers_differ() {
        return Ok(config.review.reviewers);
    }
    let worktrees = orca::list_worktrees(&mut run_command)?;
    let top = git::toplevel(&mut run_command)?;
    match orca::checkout_of(&worktrees, &top, repo) {
        Some(here) => Ok(config.review_in(&mut || Ok(orca::repo_name(&orca::repos(&mut run_command, None)?, &here.repo_id)))?.reviewers),
        None => Err(Error::Failed(format!("this checkout isn't an Orca worktree of {repo}, so whose reviewers count can't be told"))),
    }
}

// Exits as `gh pr checks` does (github::CheckState::exit_code), plus 4 when CI couldn't
// run, 5 or 6 when the PR merged or closed, and 7 when it conflicts with its base.
// The checks are those of the commit the worker pushed, which `--head` names, else the
// checkout's upstream (git::pushed) when `--repo` doesn't point elsewhere; until GitHub's
// head for the PR is that commit, they are pending. Neither: the PR's head, as GitHub has it.
pub fn checks(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, CHECKS, false)?;
    let (repo, pr) = pr_ref(&parsed)?;
    let pushed = match (parsed.non_empty("head"), parsed.non_empty("repo")) {
        (Some(head), _) => match Sha::parse(head) {
            Some(sha) => Some(Pushed { branch: None, sha: sha.into() }),
            None => return Err(Error::Usage(format!("--head must be a commit's full SHA: {head}"))),
        },
        (None, None) => git::pushed(&mut run_command, ".").ok().map(|(branch, sha)| Pushed { branch: Some(branch), sha: sha.into() }),
        (None, Some(_)) => None,
    };
    let mut gh = AppReader::new(agent, &|| {});
    let seen = github::checks(&mut gh, &repo, pr, pushed.as_ref())?;
    print_json(&seen)?;
    Ok(seen.state.exit_code())
}

pub fn reply(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, REPLY, false)?;
    let Some(body_file) = parsed.non_empty("body-file") else { return Err(Error::Usage("--body-file is required".into())) };
    let (repo, pr, thread) = thread(&parsed)?;
    let body = args::file_body(body_file, "reply", read_body)?;
    let app = github::app()?;
    print_json(&github::reply(agent, &app, &github::ThreadRef { repo: &repo, pr, thread }, &body, now_secs())?)?;
    Ok(0)
}

pub fn resolve(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, RESOLVE, false)?;
    let (repo, pr, thread) = thread(&parsed)?;
    let app = github::app()?;
    print_json(&github::resolve(agent, &app, &github::ThreadRef { repo: &repo, pr, thread }, now_secs())?)?;
    Ok(0)
}

pub fn auto_merge(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, AUTO_MERGE, false)?;
    let (repo, pr) = pr_ref(&parsed)?;
    let app = github::app()?;
    let on = turn_on(&mut || github::auto_merge(agent, &app, &github::PrRef { repo: &repo, pr }, now_secs()), &mut std::thread::sleep)?;
    let Some(on) = on else { return Ok(AUTO_MERGE_SPENT) };
    if !on.auto_merge {
        return Err(Error::Failed(format!("GitHub refused auto-merge on {repo}#{pr} as it is mergeable already")));
    }
    print_json(&on)?;
    Ok(0)
}

// Exit of `pr auto-merge` when the budget is still spent, or GitHub still unavailable, after
// the retries: auto-merge isn't on, and unlike a refusal (exit 1) a later try may turn it on.
// Nothing is printed to stdout.
const AUTO_MERGE_SPENT: u8 = 8;

// Turning auto-merge on is safe to repeat, as it changes nothing when it is on already, so
// a spent budget is waited out once and an unavailable GitHub tried again a couple of times
// (`gateway::repeat_safe_call`).
fn turn_on(
    attempt: &mut dyn FnMut() -> Result<github::AutoMerge, Error>,
    sleep: &mut dyn FnMut(Duration),
) -> Result<Option<github::AutoMerge>, Error> {
    gateway::repeat_safe_call(Api::GitHub, "turning on auto-merge", attempt, sleep)
}

// Replaces the PR's description with the file's text, as the app. For the worker to put
// on its PR what CI couldn't say (the brief's CI-unavailable step): the body is its own.
pub fn describe(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, DESCRIBE, false)?;
    let Some(body_file) = parsed.non_empty("body-file") else { return Err(Error::Usage("--body-file is required".into())) };
    let (repo, pr) = pr_ref(&parsed)?;
    let body = args::file_body(body_file, "description", read_body)?;
    let app = github::app()?;
    print_json(&github::describe(agent, &app, &github::PrRef { repo: &repo, pr }, &body, now_secs())?)?;
    Ok(0)
}

// Prints the decision before posting anything: a failed post is only logged, since the
// worker follows what was printed.
pub fn review_pass(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, REVIEW_PASS, false)?;
    let size = parsed.non_empty("size").and_then(review_pass::size_named);
    let copilot = parsed.non_empty("copilot").and_then(review_pass::copilot_named);
    let (Some(size), Some(copilot)) = (size, copilot) else {
        return Err(Error::Usage("pr review-pass needs --size and --copilot, each one of its values".into()));
    };
    let session = parsed.non_empty("session").map(|session| Id::parse(session).ok_or_else(|| Error::Usage("--session isn't a plain session ID".into()))).transpose()?;
    let run = &mut run_command;
    let tip = git::default_tip(run, ".")?;
    let (settings, file) = list_at(run, &tip.clone().into())?;
    let head = git::head(run, ".")?;
    let (at_head, head_file) = list_at(run, &head.into())?;
    // A branch that changes the list changes every later review, so it gets a deep one.
    let list_changed = (at_head != settings).then(|| file.or(head_file)).flatten();
    let names = git::changed_files(run, ".", &tip)?;
    let diff = review_pass::diff(&git::diff_lines(run, ".", &tip)?);
    let (size, sized) = match (size, &session) {
        (ReviewSize::Unsized, Some(session)) => match deep_review_estimate() {
            Ok(threshold) => review_pass::sized(size, sized_now(agent, session), threshold),
            Err(error) => {
                eprintln!("couldn't read the configuration, so the review goes by the claim's size: {error}");
                (size, None)
            }
        },
        _ => (size, None),
    };
    let decision = review_pass::decide_with_list(size, copilot, &settings.unwrap_or_default(), &names, &diff, list_changed.as_deref());
    let decision = review_pass::Decision { sized: sized.filter(|_| decision.pass != review_pass::Pass::Skip), ..decision };
    print_json(&decision)?;
    let Some(session) = session else { return Ok(0) };
    // The decision is printed: nothing after it fails the command.
    for note in [decision.sized_note(), decision.note()].into_iter().flatten() {
        if let Err(error) = send(agent, &ActivityInput::text(&session, ActivityType::Thought, &note, false)) {
            eprintln!("couldn't post how the review was decided: {error}");
        }
    }
    Ok(0)
}

fn deep_review_estimate() -> Result<Size, Error> {
    let files = files()?;
    Ok(config::config(&files, owner(&files)?, &home_dir()?)?.review.deep_at)
}

// The size of the session's issue as Linear has it now. None when it has none, or the read
// failed: the review then goes by the claim's, the no-estimate row.
fn sized_now(agent: &Agent, session_id: &str) -> Option<Size> {
    match with_token(agent, |token| sessions::read(agent, token, None, session_id, sessions::Read::Owner)) {
        Ok(session) => session.issue.and_then(|issue| issue.size()),
        Err(error) => {
            eprintln!("couldn't read the issue's estimate, so the review goes by the claim's: {error}");
            None
        }
    }
}

// The review list at `commit`, and the file that holds it.
fn list_at(run: &mut exec::Run, commit: &Commit) -> Result<(Option<review_pass::Settings>, Option<String>), Error> {
    for name in git::files_at(run, ".", commit, &review_pass::FILES)? {
        if let Some(settings) = review_pass::settings(&git::show(run, ".", commit, &name)?)? {
            return Ok((Some(settings), Some(name)));
        }
    }
    Ok((None, None))
}

// --pr, given and checked, and the repo, before anything is read. github.rs checks the
// repo is owner/name before anything is sent.
fn pr_ref(parsed: &Parsed) -> Result<(String, u64), Error> {
    let Some(pr) = parsed.non_empty("pr") else { return Err(Error::Usage("--pr is required".into())) };
    let Some(pr) = pr.parse::<u64>().ok().filter(|n| *n > 0) else {
        return Err(Error::Usage(format!("--pr must be the PR's number: {pr}")));
    };
    Ok((repo(parsed)?, pr))
}

// --pr and --thread, each given and checked, and the repo, before anything is read.
fn thread(parsed: &Parsed) -> Result<(String, u64, &str), Error> {
    let Some(thread) = parsed.non_empty("thread") else { return Err(Error::Usage("--thread is required".into())) };
    if !github::is_node_id(thread) {
        return Err(Error::Usage(format!("--thread must be the review thread's node ID: {thread}")));
    }
    let (repo, pr) = pr_ref(parsed)?;
    Ok((repo, pr, thread))
}

#[cfg(test)]
mod tests;
