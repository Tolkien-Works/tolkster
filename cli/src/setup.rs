// A worker set up on its own host (`tolkster worker`), in the terminal `listen` opens for it
// in its worktree, before its agent starts there: one path for every host, this one included.
// `listen` decides, and the line it launches carries what it decided as IDs, names and enums
// (`Worker::command`); the host reads the rest itself, from its own config, Tolkster's records
// and Linear as the agent, and writes the briefs and settings on its own disk. What the setup
// couldn't do is printed in the worker's terminal before the agent starts; what stops it starts
// no agent, and the machine's wait for a launched worker to be seen working catches it.

use std::path::{Path, PathBuf};

use crate::args::{Kind, Parsed, named};
use crate::brief::{self, Beside, Brief, Reconcile, Startup, Taken, plan_from};
use crate::cli::write_file;
use crate::coauthor;
use crate::config::Config;
use crate::error::Error;
use crate::exec::Run;
use crate::git;
use crate::github::{self, CopilotReview, Reader};
use crate::harness;
use crate::harnesses::{self, Harness};
use crate::config::ladder::{Launch, Rung};
use crate::machine::Resume;
use crate::orca::origin_repo;
use crate::plain::{CommandLine, Id, IssueKey, Line, RepoName, Sha, Time};
use crate::prebuilt;
use crate::ports::Linear;
use crate::config::repo_map::Land;
use crate::request::{Issue, triaged_children, unfinished_children};
use crate::review::{self, Round};
use crate::review_pass::{ReviewSize, size_name, size_named};
use crate::state::{Records, WorkerRecord};
use crate::time::now_ms;
use crate::timing;
use crate::voice;

// A worker's worktree name, e.g. `tw-84-tolkster-responds-to-mentions`, cut at a word to at
// most WORKTREE_NAME_MAX characters. Built from `branchName`, never the title, and checked
// again, so only `[a-z0-9-]` reaches a path or a command line; anything unexpected gives the
// bare ID. Nothing keys on the name: workers are found by the worktree's linked issue.
pub const WORKTREE_NAME_MAX: usize = 40;

pub fn worktree_name(issue: &Issue) -> String {
    let id = issue.identifier.to_ascii_lowercase();
    // Linear may put the user's name first, as `sam/…`, and the ID first or last.
    let branch = issue.branch_name.rsplit('/').next().unwrap_or_default();
    let title = branch.strip_suffix(&format!("-{id}")).or_else(|| branch.strip_prefix(&format!("{id}-"))).unwrap_or_default();
    let mut name = id.clone();
    for word in title.split('-').filter(|w| !w.is_empty()) {
        if name.len() + 1 + word.len() > WORKTREE_NAME_MAX {
            break;
        }
        name.push('-');
        name.push_str(word);
    }
    let safe = |name: &str| !name.is_empty() && name.bytes().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-'));
    if safe(&name) { name } else { id }
}

// The later of the issue's creation and its last description edit, from `edits`, the times of
// the history entries that changed it. Linear's timestamps share one format, so the later is
// the greater string.
pub fn baseline(created_at: &str, edits: &[String]) -> String {
    edits.iter().map(String::as_str).chain([created_at]).max().unwrap_or(created_at).to_string()
}

// What the launch line asks of the host.
#[derive(Clone, Debug, PartialEq)]
pub enum How {
    // A new worker on its brief, as `listen` decided from the issue it read; `then` follows
    // the brief's prompt, the line a resume would have typed.
    Start { plan_first: bool, orchestrate: bool, auto_merge: bool, review: ReviewSize, then: Option<Resume> },
    // An exited worker's session continued on the line for why it is resumed.
    Continue(Resume),
    // An exited worker's review round, in a new session on its round brief, or its session
    // continued on the round's line where the host has no briefs for it.
    Round { number: u64, round: Round },
}

// `repo` names the briefs directory, and a resume without it has no hook; `launch` is the
// worker's harness, model and effort, as its record holds them.
#[derive(Clone, Debug, PartialEq)]
pub struct Worker {
    pub session_id: Id,
    pub identifier: IssueKey,
    pub repo: Option<RepoName>,
    pub launch: Option<Launch>,
    // The commit the primary runs, which the host's own `tolkster` is made level with first
    // (prebuilt.rs).
    pub build: Option<Sha>,
    // Its repo's `trace`, as `listen`'s config.json has it: the session it starts in is traced
    // (hook::narrate).
    pub trace: bool,
    // `listen`'s `timing` setting: off, the session isn't timed (docs/timing.md).
    pub timing: bool,
    pub how: How,
}

pub const OPTIONS: [(&str, Kind); 16] = [
    ("issue", Kind::String),
    ("build", Kind::String),
    ("repo", Kind::String),
    ("harness", Kind::String),
    ("model", Kind::String),
    ("effort", Kind::String),
    ("review", Kind::String),
    ("plan-first", Kind::Boolean),
    ("orchestrate", Kind::Boolean),
    ("manual-merge", Kind::Boolean),
    ("then", Kind::String),
    ("continue", Kind::Boolean),
    ("round", Kind::String),
    ("pr", Kind::String),
    ("trace", Kind::Boolean),
    ("no-timing", Kind::Boolean),
];

fn resume_name(why: Resume) -> &'static str {
    match why {
        Resume::Message => "message",
        Resume::ChildrenEnd => "children-end",
        Resume::ChildrenAdded => "children-added",
    }
}

fn resume_named(name: &str) -> Option<Resume> {
    named(&[Resume::Message, Resume::ChildrenEnd, Resume::ChildrenAdded], resume_name, name)
}

fn round_name(round: Round) -> &'static str {
    match round {
        Round::Review { last: false } => "review",
        Round::Review { last: true } => "last",
        Round::Rebase => "rebase",
    }
}

fn round_named(name: &str) -> Option<Round> {
    named(&[Round::Review { last: false }, Round::Review { last: true }, Round::Rebase], round_name, name)
}

impl Worker {
    // One plain line, with no quote in it, so any host's shell, zsh's included, reads it as
    // `listen` wrote it, and no Linear or GitHub text: the host reads that itself.
    pub fn command(&self) -> Result<CommandLine, Error> {
        let mut words = vec!["tolkster worker".to_string(), self.session_id.to_string(), format!("--issue {}", self.identifier)];
        match &self.repo {
            Some(repo) if plain_repo(repo) => words.push(format!("--repo {repo}")),
            // A resume or a round goes without its hook, a start not at all.
            Some(repo) if matches!(self.how, How::Start { .. }) => return Err(Error::Failed(format!("the repo's name can't go on a command line unquoted: {repo}"))),
            _ => {}
        }
        if let Some(launch) = &self.launch {
            harness::check(Some(launch))?;
            words.push(launch.flags());
        }
        if let Some(build) = &self.build {
            words.push(format!("--build {build}"));
        }
        if self.trace {
            words.push("--trace".into());
        }
        if !self.timing {
            words.push("--no-timing".into());
        }
        match &self.how {
            How::Start { plan_first, orchestrate, auto_merge, review, then } => {
                words.push(format!("--review {}", size_name(*review)));
                words.extend([(*plan_first, "--plan-first"), (*orchestrate, "--orchestrate"), (!auto_merge, "--manual-merge")].into_iter().filter(|(on, _)| *on).map(|(_, flag)| flag.to_string()));
                words.extend(then.map(|why| format!("--then {}", resume_name(why))));
            }
            How::Continue(why) => words.push(format!("--continue --then {}", resume_name(*why))),
            How::Round { number, round } => words.push(format!("--round {} --pr {number}", round_name(*round))),
        }
        Ok(CommandLine::new(&Line::check(&words.join(" "))?))
    }

    pub fn parse(parsed: &Parsed) -> Result<Worker, Error> {
        let usage = |what: &str| Error::Usage(format!("worker: {what}"));
        let named = |option: &str| parsed.non_empty(option);
        let (Some(session_id), Some(identifier)) = (parsed.first(), named("issue")) else { return Err(usage("needs <sessionId> and --issue")) };
        // A value the shell split would leave words over: never taken as read.
        if parsed.positionals.len() > 1 {
            return Err(usage("takes one <sessionId>, and plain values"));
        }
        let (session_id, identifier) = (Id::check(session_id)?, IssueKey::check(identifier)?);
        let repo = named("repo").map(RepoName::check).transpose()?.filter(plain_repo);
        if named("repo").is_some() && repo.is_none() {
            return Err(usage("--repo isn't a plain name"));
        }
        let launch = match (named("model"), named("effort")) {
            (Some(model), Some(effort)) => {
                let harness = named("harness").map_or(Some(harnesses::DEFAULT), harnesses::named).ok_or_else(|| usage("--harness isn't one this build launches"))?;
                let launch = Launch::new(harness, model, harness.effort(effort).ok_or_else(|| usage(&format!("--effort isn't one of {}'s", harness.name)))?);
                if !harness.unmet().is_empty() {
                    return Err(usage(&format!("--harness {} can't run unattended safely", harness.name)));
                }
                harness::check(Some(&launch)).map_err(|_| usage("--model isn't a plain name"))?;
                Some(launch)
            }
            (None, None) if named("harness").is_none() => None,
            _ => return Err(usage("--harness, --model and --effort go together, the model a plain name")),
        };
        let build = named("build").map(Sha::check).transpose()?;
        let then = named("then").map(|name| resume_named(name).ok_or_else(|| usage("--then is message, children-end or children-added"))).transpose()?;
        let how = match (named("review"), parsed.flag("continue"), named("round")) {
            (Some(review), false, None) => {
                let review = size_named(review).ok_or_else(|| usage("--review is below, at, above or unsized"))?;
                if repo.is_none() || launch.is_none() {
                    return Err(usage("a start needs --repo, --model and --effort"));
                }
                How::Start { plan_first: parsed.flag("plan-first"), orchestrate: parsed.flag("orchestrate"), auto_merge: !parsed.flag("manual-merge"), review, then }
            }
            (None, true, None) => How::Continue(then.ok_or_else(|| usage("--continue needs --then"))?),
            (None, false, Some(round)) => {
                let number = named("pr").and_then(|n| n.parse::<u64>().ok()).ok_or_else(|| usage("--round needs --pr <n>"))?;
                How::Round { number, round: round_named(round).ok_or_else(|| usage("--round is review, last or rebase"))? }
            }
            _ => return Err(usage("pass one of --review, --continue or --round")),
        };
        Ok(Worker { session_id, identifier, repo, launch, build, trace: parsed.flag("trace"), timing: !parsed.flag("no-timing"), how })
    }
}

// A repo's name as the line carries it, unquoted: letters, digits, `.`, `_` and `-`.
fn plain_repo(repo: &RepoName) -> bool {
    !repo.starts_with('-') && repo.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

// What a host's setup reads and writes through: `dir` is the worktree the terminal opened in,
// and `trust_file` the harness's own file where a folder's trust is kept (harness::trust).
pub struct Host<'h> {
    pub config: &'h Config,
    pub linear: &'h dyn Linear,
    pub records: &'h dyn Records,
    pub github: &'h mut dyn Reader,
    pub run: &'h mut Run<'h>,
    pub dir: &'h str,
    pub trust_file: &'h Path,
}

// The agent's harness and arguments (harness::args), and what failed on the way that the
// worker starts without.
#[derive(Debug, PartialEq)]
pub struct Ready {
    pub harness: &'static Harness,
    pub args: Vec<String>,
    // The worker in its agent's environment (card::launch_env), and the session it traces.
    pub env: Vec<(String, String)>,
    pub problems: Vec<String>,
}

pub fn set_up(host: &mut Host, worker: &Worker) -> Result<Ready, Error> {
    let mut problems = Vec::new();
    let harness = harness::check(worker.launch.as_ref())?;
    let (session_id, identifier) = (&worker.session_id, &worker.identifier);
    let config = host.config;
    let mut env = crate::card::launch_env(&own_worktree(host, identifier, matches!(worker.how, How::Start { .. }).then_some(session_id))?).to_vec();
    let kind = match worker.how {
        How::Start { .. } => timing::Kind::Start,
        How::Continue(_) => timing::Kind::Resume,
        How::Round { round: Round::Rebase, .. } => timing::Kind::Rebase,
        How::Round { .. } => timing::Kind::Round,
    };
    // Without it the worker's hook neither summarises its session nor reports it.
    if worker.timing {
        env.push((timing::LAUNCH_ENV.into(), timing::launch_value(kind, now_ms(), prebuilt::commit().as_ref())));
    }
    if worker.trace {
        env.push((crate::cli::TRACE_ENV.into(), session_id.to_string()));
    }
    let (continued, prompt) = match &worker.how {
        How::Start { plan_first, orchestrate, auto_merge, review, then } => {
            let taken = |launch: &Launch, issue: &crate::request::Issue| Taken {
                plan_first: *plan_first,
                orchestrate: *orchestrate,
                auto_merge: *auto_merge,
                worktree_name: worktree_name(issue),
                rung: (Rung { name: launch.model.clone(), launch: launch.clone(), up_to: None }, Vec::new()),
                scale: issue.scale,
                review: *review,
                plan_from: plan_from(issue.scale, config.review.deep_at),
            };
            let mut prompt = start(host, worker, taken, &mut problems)?;
            if let Some(why) = then {
                prompt = Line::check(&format!("{prompt} {}", resume_line(*why, identifier, session_id)?))?;
            }
            (false, prompt)
        }
        How::Continue(why) => (true, resume_line(*why, identifier, session_id)?),
        How::Round { number, round } => match worker.repo.as_ref().and_then(|repo| fresh_line(&config.briefs_dir, repo, identifier, *number, session_id, *round)) {
            Some(fresh) => (false, fresh),
            None => (true, review::line(*number, identifier, session_id, *round)?),
        },
    };
    // Claude's own files, which a harness without them never reads.
    let hooks = worker.repo.as_ref().filter(|_| harness.settings).and_then(|repo| settings(&config.briefs_dir, repo, &mut problems));
    if let Err(error) = harness.settings.then(|| harness::trust(host.trust_file, host.dir)).transpose() {
        problems.push(format!("trusting the worktree failed, so the agent asks first: {error}"));
    }
    Ok(Ready { harness, args: harness::args(harness, continued, worker.launch.as_ref(), hooks.as_deref(), &prompt), env, problems })
}

// The line a resume continues an exited worker's session on.
fn resume_line(why: Resume, identifier: &IssueKey, session_id: &Id) -> Result<Line, Error> {
    match why {
        Resume::Message => voice::new_message(session_id),
        Resume::ChildrenEnd => voice::children_finished(identifier, session_id),
        Resume::ChildrenAdded => voice::children_added(identifier, session_id),
    }
}

// The line a new session starts an exited worker's round on, from this host's briefs: None
// when they aren't there.
fn fresh_line(briefs_dir: &str, repo: &RepoName, identifier: &IssueKey, number: u64, session_id: &Id, round: Round) -> Option<Line> {
    let task = brief::path(briefs_dir, repo, identifier);
    let brief = brief::path(briefs_dir, repo, &Beside::from(round).name(identifier));
    (task.is_file() && brief.is_file()).then(|| review::fresh(number, identifier, &task, &brief, session_id, round)).and_then(|line| Line::parse(&line))
}

// The settings files beside the repo's briefs, written again where a rebuilt CLI changed them:
// None, so the worker runs without its hook and with the MCP servers, when they couldn't be.
fn settings(briefs_dir: &str, repo: &RepoName, problems: &mut Vec<String>) -> Option<PathBuf> {
    let hooks = harness::hooks_path(briefs_dir, repo);
    match harness::write_settings(&hooks) {
        Ok(()) => Some(hooks),
        Err(error) => {
            problems.push(format!("its hook is off and its MCP servers on: writing its settings failed: {error}"));
            None
        }
    }
}

// The directory the line runs in must be the worktree Tolkster's record holds for the issue's
// worker on this host, and for a start the session's worker, so a line run anywhere else
// trusts, names, pushes, writes and starts nothing. A resume's session may be a later one.
// That record.
fn own_worktree(host: &Host, identifier: &IssueKey, session_id: Option<&Id>) -> Result<WorkerRecord, Error> {
    let workers = host.records.workers()?;
    match crate::card::worker(&workers, host.config.host.as_deref(), host.dir) {
        Some((record, _)) if record.issue.eq_ignore_ascii_case(identifier) && session_id.is_none_or(|id| record.session_id == id.as_str()) => Ok(record.clone()),
        _ => Err(Error::Failed(format!("{} isn't the worktree of {identifier}'s worker", host.dir))),
    }
}

// A new worker's briefs, written once its session and its branch are set up: the prompt it
// starts on. The session must be on the issue the line names.
fn start(host: &mut Host, worker: &Worker, taken: impl Fn(&Launch, &crate::request::Issue) -> Taken, problems: &mut Vec<String>) -> Result<Line, Error> {
    let (session_id, identifier) = (&worker.session_id, &worker.identifier);
    let (Some(repo), Some(launch)) = (worker.repo.as_ref(), worker.launch.as_ref()) else { return Err(Error::Usage("a start needs --repo, --model and --effort".into())) };
    let session = host.linear.session(session_id)?;
    let issue = session.issue.as_ref().filter(|issue| issue.identifier.eq_ignore_ascii_case(identifier)).ok_or_else(|| Error::Failed(format!("session {session_id} isn't on {identifier}")))?;
    // The branch Orca made: the one the start's result names, and so the one its PR's events map by,
    // which it leaves unsaid past 200 characters (`executor::started`). A start without one fails.
    let branch_name = &git::current_branch(host.run, host.dir)?.filter(|branch| branch.len() <= 200).ok_or_else(|| Error::Failed(format!("{} has no plain branch checked out", host.dir)))?;
    let taken = taken(launch, issue);
    let baseline = baseline(&issue.created_at, &Id::parse(&issue.id).ok_or_else(|| Error::Failed("the issue's ID isn't plain".into())).and_then(|id| host.linear.description_edits(&id)).unwrap_or_else(|error| {
        problems.push(format!("{identifier}'s history wasn't read, so its baseline is its creation: {error}"));
        Vec::new()
    }));
    // Should the read fail, the worker starts without the reconcile.
    let reconcile = landed_since(host.run, host.dir, &baseline, problems).unwrap_or_else(|error| {
        problems.push(format!("reading what landed since {baseline} failed: {error}"));
        None
    });
    // Should the read fail, the brief takes Copilot as off, as if the worker's review were the
    // only one. A ruleset flipped later takes effect at the next start.
    let copilot = origin_repo(host.run, Some(host.dir)).and_then(|origin| github::copilot_reviews(&mut *host.github, &origin, brief::BASE)).unwrap_or_else(|error| {
        problems.push(format!("reading {}'s rules failed, so the brief takes Copilot's review as off: {error}", brief::BASE));
        CopilotReview::Off
    });
    // A worker started again after its host was unpaired carries on from what the first one pushed.
    if let Err(error) = git::continue_issue_branch(host.run, host.dir, identifier) {
        problems.push(format!("continuing {identifier}'s pushed branch failed, so the worker starts from main: {error}"));
    }
    let review = host.config.review_for(Some(repo));
    let mut brief = Brief { identifier, session_id, branch_name, repo, taken: &taken, reconcile, copilot, owner_name: &host.config.owner.name, reviewers: &review.reviewers, startup: Startup::Unpushed, land: host.config.repos.entry(repo).map_or(Land::Pr, |entry| entry.land) };
    brief.check()?;
    brief.startup = set_up_branch(host.run, host.dir, &brief, problems);
    // Its commits only don't credit the co-author.
    if let Err(error) = coauthor::mark(host.run, host.dir, session_id, host.config.owner.co_author.as_deref()) {
        problems.push(format!("marking it failed: {error}"));
    }
    let briefs_dir = &host.config.briefs_dir;
    let brief_path = brief::path(briefs_dir, repo, identifier);
    write_file(&brief_path, &brief.render())?;
    for which in Beside::ALL {
        write_file(&brief::path(briefs_dir, repo, &which.name(identifier)), &brief.render_beside(which))?;
    }
    let keys = |identifiers: Vec<String>| identifiers.iter().map(|identifier| IssueKey::check(identifier)).collect::<Result<Vec<_>, _>>();
    let (children, triaged) = if taken.orchestrate { (keys(unfinished_children(issue).into_iter().map(|(identifier, _)| identifier).collect())?, keys(triaged_children(issue))?) } else { (Vec::new(), Vec::new()) };
    brief::prompt(&brief_path, identifier, session_id, Some((&children, &triaged)))
}

// Only SHAs and a count come back, never what the commits say. A failed fetch goes in
// `problems`, and the branch is read as last fetched. None when nothing has landed since.
fn landed_since(run: &mut Run, dir: &str, baseline: &str, problems: &mut Vec<String>) -> Result<Option<Reconcile>, Error> {
    let baseline = Time::check(baseline)?;
    if let Err(error) = git::fetch(run, dir) {
        problems.push(format!("fetching before the reconcile failed: {error}"));
    }
    let to = git::default_tip(run, dir)?;
    let from = git::last_before(run, dir, &baseline, &to)?;
    let commits = git::count_first_parent(run, dir, from.as_ref(), &to)?;
    Ok((commits > 0).then_some(Reconcile { baseline, from, to, commits }))
}

// The branch is pushed as Orca made it, never renamed: its name is what the start's result
// reported. The push is held back where the worker waits for the owner's go before pushing
// (plan first, or a reconcile that may find drift), and for a parent working through its
// sub-issues. The name reaches git as one argument, never through a shell. A failure only
// costs the worker a turn.
fn set_up_branch(run: &mut Run, dir: &str, brief: &Brief, problems: &mut Vec<String>) -> Startup {
    if brief.taken.orchestrate || brief.taken.plan_first || brief.reconcile.is_some() {
        return Startup::Unpushed;
    }
    match git::push_upstream(run, dir, brief.branch_name) {
        Ok(()) => Startup::Pushed,
        Err(_) => {
            problems.push("pushing its branch failed, so the worker does".into());
            Startup::Unpushed
        }
    }
}

#[cfg(test)]
mod tests;
