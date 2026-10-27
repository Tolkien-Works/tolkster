// Only IDs, names, settings, and for a reconcile times, SHAs and a count are filled in,
// never Linear or commit text: the worker reads the issue, the owner's messages and the
// commits itself.

use std::path::{Path, PathBuf};

use crate::error::Error;
use crate::estimates::{Scale, Size};
use crate::config::ladder::{Launch, Rung};
use crate::request::Issue;
use crate::github::CopilotReview;
use crate::harness;
use crate::orca::{Orca, Repo, Worktree, find_repo, primary_checkout};
use crate::plain::{BranchName, CommandLine, Id, IssueKey, Line, RepoName, Sha, Time};
use crate::config::repo_map::{Land, Missing, Place, RepoMap};
use crate::review_pass::{ReviewSize, copilot_name, size_against, size_name};

// What a new worker's brief is written from, decided from the issue as read when it starts
// (executor.rs, `start`).
#[derive(Clone, Debug, PartialEq)]
pub struct Taken {
    pub plan_first: bool,
    pub orchestrate: bool,
    pub auto_merge: bool,
    pub worktree_name: String,
    // The rung it runs on (ladder::Ladder::choose), and what the log should say of labels it
    // didn't follow.
    pub rung: (Rung, Vec<crate::trace::RungNote>),
    pub scale: Option<Scale>,
    pub review: ReviewSize,
    // The points from which an unsized issue's worker keeps a plan, once it has set its
    // estimate: the smallest on its scale at or above `review.deepAt` (`plan_from`).
    pub plan_from: Option<u32>,
}

pub fn review_size(issue: &Issue, threshold: Size) -> ReviewSize {
    size_against(issue.size(), threshold)
}

const WORK: &str = include_str!("../briefs/work.md");
const PLAN_FIRST: &str = include_str!("../briefs/plan-first.md");
const RECONCILE: &str = include_str!("../briefs/reconcile.md");
const CONVERSATION: &str = include_str!("../briefs/conversation.md");
const THREAD: &str = include_str!("../briefs/thread.md");
const CHILDREN: &str = include_str!("../briefs/children.md");
// Written beside the work brief: a worker reads them only when it needs them.
const ROUND: &str = include_str!("../briefs/round.md");
const REBASE: &str = include_str!("../briefs/rebase.md");
const DEEP: &str = include_str!("../briefs/deep.md");
// Every deep-pass reviewer reads the same instructions, so an Agent call names only its lens
// and the diff.
const REVIEWER: &str = include_str!("../briefs/reviewer.md");
const LAST_ROUND: &str = include_str!("../briefs/last-round.md");
const REPORT_PROGRESS: (&str, &str) = ("<REPORT PROGRESS>", include_str!("../briefs/report-progress.md"));
pub const RUBRIC: (&str, &str) = ("<RUBRIC>", include_str!("../briefs/rubric.md"));
const CLOSE_TERMINAL: (&str, &str) = ("<CLOSE TERMINAL>", include_str!("../briefs/close-terminal.md"));
// The rules for posting as Tolkster, in every brief that posts: a session in a repo with no
// Tolkster AGENTS.md reads them nowhere else. A review round's worker has the work brief's.
const POSTING: (&str, &str) = ("<POSTING>", include_str!("../briefs/posting.md"));
// The sessions that run in a shared checkout, and the turns among them, carry these.
const SESSION_PARTIALS: [(&str, &str); 5] = [
    ("<SESSION PREAMBLE>", include_str!("../briefs/session-preamble.md")),
    ("<CHECKOUT>", include_str!("../briefs/checkout.md")),
    ("<TURN>", include_str!("../briefs/turn.md")),
    POSTING,
    CLOSE_TERMINAL,
];
pub fn session_brief(template: &str) -> String {
    SESSION_PARTIALS.into_iter().fold(template.to_string(), |text, (placeholder, partial)| text.replace(placeholder, partial.strip_suffix('\n').unwrap_or(partial)))
}
// Plan first and reconcile both hold the work until the owner's go, and resume on it alike.
// The round and rebase briefs share the leased push and the batched rebase. The work brief's
// review and the deep pass's reviewers check against the same list.
const PARTIALS: [(&str, &str); 8] = [
    REPORT_PROGRESS,
    POSTING,
    CLOSE_TERMINAL,
    ("<HOLD>", include_str!("../briefs/hold.md")),
    ("<APPROVED>", include_str!("../briefs/approved.md")),
    RUBRIC,
    ROUND_GIT,
    ("<CHECKS>", include_str!("../briefs/checks.md")),
];
pub const ROUND_GIT: (&str, &str) = ("<ROUND GIT>", include_str!("../briefs/round-git.md"));

// An orchestrating parent has no branch of its own to push: it is `Startup::Unpushed` throughout.
const APPROVED_PUSH: &str = "<PUSH>";
const APPROVED_PUSH_LINE: &str = "run `git push -u origin <branchName>`, ";

// The denial rule, filled for what each brief posts and how it ends. A turn's brief also says
// that a refusal from `tolkster` itself (exit 2) isn't a denial.
const DENIED: &str = include_str!("../briefs/denied.md");
// voice.rs's choices, on the first ask only: a second denial has no retry to offer.
const DENIED_OPTIONS: &str = " with `--option go --option skip`";
fn denied(ask: &str, options: &str, ids: &str, refusal: &str, stop: &str) -> String {
    DENIED.strip_suffix('\n').unwrap_or(DENIED).replace("<refusal>", refusal).replace("<options>", options).replace("<ask>", ask).replace("<ids>", ids).replace("<stop>", stop)
}
const TURN_REFUSAL: &str = " A command `tolkster` itself refuses here (exit 2) isn't a denial: don't ask about it.";

const DENIED_PLACEHOLDER: &str = "<DENIED>";
// A repo that lands `direct` finishes with `tolkster land`, its gate the only check: no CI runs
// on `main` (docs/landing.md). An issue left to the owner's merge opens a PR there too.
const FINISH: &str = "<FINISH>";
const OPEN_PR: &str = include_str!("../briefs/open-pr.md");
const LAND: &str = include_str!("../briefs/land.md");
const GATE: &str = "<GATE>";
const GATE_PR: &str = "The gate is to run every check the repo lists, unless its `AGENTS.md` / `CLAUDE.md` says the gate runs only the checks for the paths a branch changes: then those it names for the paths `git diff --name-only --no-renames origin/main...HEAD` prints, and all of them for a path it names none for.";
const GATE_LAND: &str = "No CI runs after you, so the gate is every check CI would run, whatever the paths: a gate command that leaves some to CI runs with `--all`.";
const VERDICT: &str = "<VERDICT>";
const VERDICT_PR: &str = "give `pr create` its verdict, unless the pass was `skip`: `--review deep --findings \"7 findings, 6 fixed\"` (`light`, and no `--findings` when it found nothing).";
const VERDICT_LAND: &str = "end the commit's body with it, unless the pass was `skip`.";
const MERGE: &str = "<MERGE>";
const AUTO_MERGE: &str = include_str!("../briefs/auto-merge.md");
const MANUAL_MERGE: &str = include_str!("../briefs/manual-merge.md");
// `pr create` turns auto-merge on with the PR where the claim lets it (TW-619).
const AUTO_MERGE_PLACEHOLDER: &str = "<AUTO MERGE>";
const AUTO_MERGE_FLAG: &str = " --auto-merge";

// The size and Copilot's review are read at the claim and rendered into `tolkster pr
// review-pass`'s command, so an estimate set later never lightens the pre-push review,
// unless the claim had none (review_pass::sized). A review round's own review follows
// Copilot alone: none while Copilot reviews each push, the light pass otherwise.
const REVIEW_PASS: &str = "<review pass>";
const ROUND_PUSH: &str = "<ROUND PUSH>";
const ROUND_PUSH_WITH_COPILOT: &str = include_str!("../briefs/round-push-with-copilot.md");
const ROUND_PUSH_LIGHT: &str = include_str!("../briefs/round-push-light.md");
const NO_COPILOT_LEAD: &str = "Copilot doesn't review this PR, so nothing else looks at what you push.";
const OPENING_COPILOT_LEAD: &str = "Copilot reviewed this PR as it opened, but the ruleset doesn't have it review pushes, so nothing else looks at what you push.";

// The base branch the work brief opens the PR against (`--base main` in work.md).
pub const BASE: &str = "main";

// The reconcile, plan-first and sub-issues sections go right before it, in that order.
const AFTER_THE_TASK: &str = "## Before any work";

// A parent the brief orchestrates is sized for its own work alone, never its sub-issues' sum.
const SIZING: &str = "<SIZING>";
const SIZING_USED: &str = include_str!("../briefs/sizing.md");
const SIZING_PARENT: &str = include_str!("../briefs/sizing-parent.md");
const SIZING_NONE: &str = include_str!("../briefs/sizing-none.md");

const STARTUP: &str = "<STARTUP>";

// A plan only where there was planning: plan first, sub-issues, or an issue at or above
// `review.deepAt`, or for an unsized one, at or above it once its worker sizes it. A
// small issue's plan only repeats the hook's phases.
const PLAN: &str = "<PLAN>";
const PLAN_BODY: &str = include_str!("../briefs/plan.md");
const PLAN_KEPT: &str = "- **Keep a plan on the session.**";
const PLAN_IF_SIZED: &str = "- **Keep a plan on the session** only if the estimate you set in step 3 is <points> or more; otherwise post none.";
const PLAN_NONE: &str = "- **Post no plan:** the issue is small, and the hook's line names the phase you're in.";

pub fn plan_from(scale: Option<Scale>, threshold: Size) -> Option<u32> {
    let scale = scale?;
    scale.values().into_iter().find(|&points| scale.size(f64::from(points)).is_some_and(|size| size >= threshold))
}
const STARTUP_DONE: &str = include_str!("../briefs/startup-done.md");
const STARTUP_PUSH: &str = include_str!("../briefs/startup-push.md");

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Startup {
    // The branch Orca made, pushed with its upstream.
    Pushed,
    // Not pushed: held back until the owner's go (plan first, or a reconcile that may find
    // drift) or until a parent's own work, or the push failed.
    Unpushed,
}

pub struct Brief<'a> {
    pub identifier: &'a IssueKey,
    pub session_id: &'a Id,
    pub branch_name: &'a BranchName,
    pub repo: &'a RepoName,
    pub taken: &'a Taken,
    pub reconcile: Option<Reconcile>,
    // Off too when the ruleset couldn't be read, so the brief asks for the review of a PR
    // nothing else reviews.
    pub copilot: CopilotReview,
    pub owner_name: &'a str,
    pub reviewers: &'a [String],
    pub startup: Startup,
    // The repo's `land`, read on the worker's host as `tolkster land` reads it there.
    pub land: Land,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Reconcile {
    pub baseline: Time,
    // The newest commit at or before the baseline, None when there is none.
    pub from: Option<Sha>,
    pub to: Sha,
    pub commits: u32,
}

impl Reconcile {
    // `to` alone when every commit is after the baseline.
    pub fn range(&self) -> String {
        match &self.from {
            Some(from) => format!("{from}..{}", self.to),
            None => self.to.to_string(),
        }
    }
}

impl Brief<'_> {
    // Its names are plain by their types; what's left is the worker's launch.
    pub fn check(&self) -> Result<(), Error> {
        harness::continued(Some(self.launch())).map(|_| ())
    }

    fn launch(&self) -> &Launch {
        &self.taken.rung.0.launch
    }

    pub fn render(&self) -> String {
        let reconcile = self.reconcile.as_ref().map_or(String::new(), |r| {
            let commits = if r.commits == 1 { "1 commit has".to_string() } else { format!("{} commits have", r.commits) };
            format!("{RECONCILE}\n").replace("<baseline>", &r.baseline).replace("<commits>", &commits).replace("<range>", &r.range())
        });
        let plan_first = if self.taken.plan_first { format!("{PLAN_FIRST}\n") } else { String::new() };
        let children = if self.taken.orchestrate { format!("{CHILDREN}\n") } else { String::new() };
        let startup = match self.startup {
            Startup::Pushed => STARTUP_DONE.to_string(),
            Startup::Unpushed => STARTUP_PUSH.to_string(),
        };
        let sizing = match self.taken.scale.and_then(|s| s.describe().zip(s.values().first().copied())) {
            Some((values, smallest)) => {
                let parent = if self.taken.orchestrate { SIZING_PARENT.replace("<smallest estimate>", &smallest.to_string()) } else { String::new() };
                format!("{}{parent}", SIZING_USED.replace("<estimate scale>", &values))
            }
            None => SIZING_NONE.to_string(),
        };
        let work = WORK.replacen(AFTER_THE_TASK, &format!("{reconcile}{plan_first}{children}{AFTER_THE_TASK}"), 1);
        let work = work.replace(SIZING, sizing.strip_suffix('\n').unwrap_or(&sizing));
        let plan = match self.taken.review {
            _ if self.taken.plan_first || self.taken.orchestrate => Some(PLAN_KEPT.to_string()),
            ReviewSize::At | ReviewSize::Above => Some(PLAN_KEPT.to_string()),
            ReviewSize::Unsized => self.taken.plan_from.map(|points| PLAN_IF_SIZED.replace("<points>", &points.to_string())),
            ReviewSize::Below => None,
        };
        let plan = plan.map_or(PLAN_NONE.to_string(), |lead| format!("{lead} {}", PLAN_BODY.trim_end()));
        let work = work.replace(PLAN, &plan);
        self.fill(work.replace(STARTUP, startup.strip_suffix('\n').unwrap_or(&startup)))
    }

    pub fn render_beside(&self, which: Beside) -> String {
        self.fill(match which {
            Beside::Round => ROUND,
            Beside::Rebase => REBASE,
            Beside::Deep => DEEP,
            Beside::Reviewer => REVIEWER,
            Beside::LastRound => LAST_ROUND,
        }.to_string())
    }

    fn fill(&self, text: String) -> String {
        let (finish, gate, verdict) = if self.land == Land::Direct && self.taken.auto_merge { (LAND, GATE_LAND, VERDICT_LAND) } else { (OPEN_PR, GATE_PR, VERDICT_PR) };
        let mut text = text.replace(FINISH, finish.strip_suffix('\n').unwrap_or(finish)).replace(GATE, gate).replace(VERDICT, verdict);
        let (merge, auto_merge_flag) = if self.taken.auto_merge { (AUTO_MERGE, AUTO_MERGE_FLAG) } else { (MANUAL_MERGE, "") };
        let review_pass = format!("--size {} --copilot {}", size_name(self.taken.review), copilot_name(self.copilot));
        let round_push = match self.copilot {
            CopilotReview::OnEachPush => ROUND_PUSH_WITH_COPILOT.to_string(),
            CopilotReview::OnOpening => format!("{OPENING_COPILOT_LEAD} {ROUND_PUSH_LIGHT}"),
            CopilotReview::Off => format!("{NO_COPILOT_LEAD} {ROUND_PUSH_LIGHT}"),
        };
        let denied = denied(
            "an `elicitation`",
            DENIED_OPTIONS,
            "issue, PR or session",
            "",
            "Then print `<identifier> BLOCKED` and stop; if the `elicitation` is denied too, print that line and stop all the same.",
        );
        for (placeholder, partial) in PARTIALS.into_iter().chain([(MERGE, merge), (AUTO_MERGE_PLACEHOLDER, auto_merge_flag), (REVIEW_PASS, review_pass.as_str()), (ROUND_PUSH, round_push.as_str()), (DENIED_PLACEHOLDER, denied.as_str())]) {
            text = text.replace(placeholder, partial.strip_suffix('\n').unwrap_or(partial));
        }
        text.replace(APPROVED_PUSH, if self.taken.orchestrate { "" } else { APPROVED_PUSH_LINE })
            .replace("<identifier>", self.identifier)
            .replace("<sessionId>", self.session_id)
            .replace("<branchName>", self.branch_name)
            .replace("<repo displayName>", self.repo)
            .replace("<OWNER NAME>", self.owner_name)
            .replace("<REVIEWERS>", &reviewers(self.reviewers))
    }
}

pub struct TurnBrief<'a> {
    pub identifier: &'a IssueKey,
    pub session_id: &'a Id,
    pub repo: &'a RepoName,
    pub owner_name: &'a str,
}

impl TurnBrief<'_> {
    pub fn render(&self) -> String {
        let denied = denied(
            "an `elicitation`",
            DENIED_OPTIONS,
            "issue or session",
            TURN_REFUSAL,
            "Name the changes you made in it, and the turn ends there. A later turn that no longer knows what the step was says so in its `response`.",
        );
        session_brief(CONVERSATION)
            .replace(DENIED_PLACEHOLDER, &denied)
            .replace("<answer>", "`response`")
            .replace("<identifier>", self.identifier)
            .replace("<sessionId>", self.session_id)
            .replace("<repo displayName>", self.repo)
            .replace("<OWNER NAME>", self.owner_name)
    }
}

pub struct ThreadBrief<'a> {
    pub identifier: &'a IssueKey,
    pub comment_id: &'a Id,
    pub repo: &'a RepoName,
    pub owner_name: &'a str,
}

impl ThreadBrief<'_> {
    pub fn render(&self) -> String {
        let denied = denied("a reply", "", "issue or comment", TURN_REFUSAL, "The turn ends there.");
        session_brief(THREAD)
            .replace(DENIED_PLACEHOLDER, &denied)
            .replace("<answer>", "reply")
            .replace("<identifier>", self.identifier)
            .replace("<commentId>", self.comment_id)
            .replace("<repo displayName>", self.repo)
            .replace("<OWNER NAME>", self.owner_name)
    }
}

fn reviewers(logins: &[String]) -> String {
    if logins.is_empty() {
        return "no one".into();
    }
    logins.iter().map(|l| format!("`{l}`")).collect::<Vec<_>>().join(" and ")
}

pub fn path(briefs_dir: &str, repo: &str, name: &str) -> PathBuf {
    Path::new(&briefs_dir.replace("<repo>", repo)).join(format!("{name}.md"))
}

// The briefs beside the work brief, named for `path`.
#[derive(Clone, Copy)]
pub enum Beside {
    Round,
    Rebase,
    Deep,
    Reviewer,
    LastRound,
}

impl Beside {
    pub const ALL: [Beside; 5] = [Beside::Round, Beside::Rebase, Beside::Deep, Beside::Reviewer, Beside::LastRound];

    fn suffix(self) -> &'static str {
        match self {
            Beside::Round => "round",
            Beside::Rebase => "rebase",
            Beside::Deep => "deep",
            Beside::Reviewer => "reviewer",
            Beside::LastRound => "last-round",
        }
    }

    pub fn name(self, identifier: &str) -> String {
        format!("{identifier}-{}", self.suffix())
    }
}

// Plain, with no quote in it, so it is typed into a terminal as it is. It has the worker read
// the issue and the request in the turn it reads the brief, not a turn later. A parent working
// through its sub-issues is told on this line which it may chain and delegate: auto mode
// lets through a write the line names.
// `chain` is the sub-issues the worker of `issue` may chain, and those it skips, in Triage.
pub fn prompt(path: &Path, issue: &IssueKey, session: &Id, chain: Option<(&[IssueKey], &[IssueKey])>) -> Result<Line, Error> {
    let mut line = format!("Read the file {} in full and, in the same turn, run tolkster start {session}. The file is your complete task brief. Execute it end to end in this worktree, then report back. Do not merge it yourself; the brief says how the work lands.", path.display());
    if let Some((children, triaged)) = chain
        && let Some((named, many)) = listed(children)
    {
        let noun = if many { "sub-issues" } else { "sub-issue" };
        line.push_str(&format!(" You may chain and delegate the {noun} {named} of {issue} with tolkster issue chain."));
        if let Some((named, many)) = listed(triaged) {
            line.push_str(&format!(" Skip {named}: {}, so nobody has accepted {}.", if many { "they are in Triage" } else { "it is in Triage" }, if many { "them" } else { "it" }));
        }
    }
    Line::check(&line)
}

// "A", "A and B" or "A, B and C", and whether it names more than one.
fn listed(keys: &[IssueKey]) -> Option<(String, bool)> {
    let (last, rest) = keys.split_last()?;
    let rest: Vec<&str> = rest.iter().map(IssueKey::as_str).collect();
    Some((if rest.is_empty() { last.to_string() } else { format!("{} and {last}", rest.join(", ")) }, !rest.is_empty()))
}

// An agent's launch with its first prompt as its argument, in double quotes, so a prompt
// that is a `Line` stays one argument in the shell, and
// one with no leading dash is never read as a flag.
pub fn with_prompt(command: CommandLine, prompt: &Line) -> Result<CommandLine, Error> {
    if prompt.is_empty() || prompt.starts_with('-') {
        return Err(Error::Usage(format!("an agent's first prompt can't be empty or start with a dash: {prompt}")));
    }
    Ok(command.quoted(prompt))
}

// A session in the primary checkout reads the brief and, in the same turn, each of `issues`
// (after `git fetch origin` when `fetch`), which its brief renders from the same IDs, so the
// second turn already decides. Err for a path a terminal line can't take.
pub fn reading_prompt(path: &Path, fetch: bool, issues: &[&IssueKey]) -> Result<Line, Error> {
    let fetched = fetch.then(|| "git fetch origin".to_string());
    let shown = (!issues.is_empty()).then(|| format!("tolkster issue show {} --comments", issues.iter().map(|issue| issue.as_str()).collect::<Vec<_>>().join(" ")));
    let reads: Vec<String> = fetched.into_iter().chain(shown).collect();
    Line::check(&format!("Read the file {} in full and, in the same turn, run {} as one shell call. It is your complete task brief. Execute it end to end, then close your terminal as it says.", path.display(), reads.join("; ")))
}

// Orca's repos and worktrees, where `find` looks for the primary checkout of an issue's repo.
pub struct Checkouts {
    repos: Vec<Repo>,
    worktrees: Vec<Worktree>,
}

pub struct Found {
    pub repo_id: String,
    pub repo: String,
    pub primary: String,
    // The primary checkout's path, where git runs.
    pub path: String,
}

impl Checkouts {
    pub fn list(orca: &mut dyn Orca) -> Result<Checkouts, Error> {
        Ok(Checkouts { repos: orca.repos(None)?, worktrees: orca.worktrees()? })
    }

    pub fn find(&self, map: &RepoMap, place: &Place) -> Result<Found, Missing> {
        let (repo_id, repo) = find_repo(&self.repos, map.repo_for(place)).ok_or(Missing::Repo)?;
        let primary = primary_checkout(&self.worktrees, &repo_id).ok_or(Missing::Primary)?;
        Ok(Found { repo_id, repo, primary: primary.id.clone(), path: primary.path.clone() })
    }
}

// Builds its line before it writes, and writes before it opens, so a failure leaves at most
// a brief that nothing reads. `prompt` is `reading_prompt`.
pub fn open_session(orca: &mut dyn Orca, write: &mut dyn FnMut(&Path, &str) -> Result<(), Error>, primary: &str, title: &str, path: &Path, prompt: &Line, text: &str) -> Result<String, Error> {
    let command = with_prompt(harness::command(None)?, prompt)?;
    write(path, text)?;
    orca.open(primary, Some(title), &command)
}

#[cfg(test)]
mod tests;
