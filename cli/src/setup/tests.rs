use std::path::PathBuf;

use super::*;
use crate::exec::run_command;
use crate::plain::WorktreeId;
use crate::ports::fake::{self, FakeReader};
use crate::request::{Issue, Session};
use crate::request::tests::{at, issue, session};
use crate::state::{RungFrom, WorkerRecord};

fn launch() -> Launch {
    Launch::new(crate::harnesses::DEFAULT, "claude-sonnet-5-5", "high")
}

fn worker(how: How) -> Worker {
    Worker { session_id: "s1".into(), identifier: "TW-1".into(), repo: Some("tolkster".into()), launch: Some(launch()), build: None, trace: false, timing: true, how }
}

fn start(plan_first: bool, orchestrate: bool) -> How {
    How::Start { plan_first, orchestrate, auto_merge: true, review: ReviewSize::Above, then: None }
}

fn parsed(line: &CommandLine) -> Result<Worker, Error> {
    let words: Vec<String> = line.as_str().split(' ').skip(2).map(String::from).collect();
    Worker::parse(&crate::args::parse(&words, &OPTIONS, true)?)
}

// The line `listen` launches is plain, with no quote for any shell to balance, and the host
// reads back from it exactly what was decided.
#[test]
fn the_launch_line_is_plain_and_read_back_as_it_was_written() {
    let then = How::Start { plan_first: true, orchestrate: false, auto_merge: false, review: ReviewSize::Unsized, then: Some(Resume::ChildrenAdded) };
    let unrecorded = Worker { launch: None, repo: None, ..worker(How::Continue(Resume::Message)) };
    let built = Worker { build: Sha::parse(&"a1".repeat(20)), ..worker(How::Continue(Resume::Message)) };
    for worker in [built, worker(start(false, true)), worker(then), worker(How::Continue(Resume::ChildrenEnd)), unrecorded, worker(How::Round { number: 7, round: Round::Review { last: true } }), worker(How::Round { number: 7, round: Round::Rebase })] {
        let line = worker.command().unwrap();
        assert!(line.as_str().starts_with("tolkster worker s1 --issue TW-1") && !line.as_str().contains(['"', '\'']), "{line:?}");
        assert_eq!(parsed(&line).unwrap(), worker);
    }
    let line = worker(start(true, false)).command().unwrap();
    assert_eq!(line.as_str(), "tolkster worker s1 --issue TW-1 --repo tolkster --model claude-sonnet-5-5 --effort high --review above --plan-first");
    assert!(Worker { build: Sha::parse(&"a1".repeat(20)), ..worker(How::Continue(Resume::Message)) }.command().unwrap().as_str().ends_with(&format!("--build {} --continue --then message", "a1".repeat(20))));
    // A build that isn't a full commit is refused, not read.
    assert!(Worker::parse(&crate::args::parse(&["s1".to_string(), "--issue".into(), "TW-1".into(), "--build".into(), "main".into(), "--continue".into(), "--then".into(), "message".into()], &OPTIONS, true).unwrap()).is_err());
    // A model that isn't a plain name, as a record could only have if tampered with, launches nothing.
    assert!(Worker { launch: Some(Launch { model: "opus --dangerously-skip-permissions".into(), ..launch() }), ..worker(start(false, false)) }.command().is_err());
    // A start needs what it renders the brief with, and one line asks for one thing.
    let refused = |words: &[&str]| Worker::parse(&crate::args::parse(&words.iter().map(|w| w.to_string()).collect::<Vec<_>>(), &OPTIONS, true).unwrap()).is_err();
    assert!(refused(&["s1", "--issue", "TW-1", "--review", "above"]));
    assert!(refused(&["s1", "--issue", "TW-1", "--continue"]));
    assert!(refused(&["s1", "--issue", "TW-1", "--continue", "--then", "message", "--round", "review", "--pr", "7"]));
    assert!(refused(&["s1", "--issue", "TW-1", "--model", "opus", "--continue", "--then", "message"]));
    assert!(refused(&["s1", "--repo", "tolkster", "--continue", "--then", "message"]));
    // A repo's name the shell would split, or read as more, never reaches the line: a start
    // isn't launched, and a resume goes without its hook. A split value is refused, never
    // half read.
    for odd in ["Tolkster App", "a;b", "it's", "-x"] {
        assert!(Worker { repo: Some(odd.into()), ..worker(start(false, false)) }.command().is_err(), "{odd}");
        let resumed = Worker { repo: Some(odd.into()), ..worker(How::Continue(Resume::Message)) }.command().unwrap();
        assert!(!resumed.as_str().contains("--repo"), "{odd}");
    }
    assert!(refused(&["s1", "--issue", "TW-1", "--repo", "Tolkster", "App", "--continue", "--then", "message"]));
    assert!(refused(&["s1", "--issue", "TW-1", "--repo", "a;b", "--continue", "--then", "message"]));
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = run_command("git", args, Some(dir.to_str().unwrap())).unwrap();
    assert!(output.ok, "git {args:?}: {}", output.stderr);
    output.stdout.trim().to_string()
}

// A checkout with a local origin, and a worktree of it on the branch another host's Orca
// would make, with that host's prefix.
fn repo(test: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("tolkster-setup-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let (origin, primary, worktree) = (root.join("origin.git"), root.join("primary"), root.join("tw-1"));
    let path = |p: &Path| p.to_str().unwrap().to_string();
    git(&root, &["init", "--quiet", "--bare", &path(&origin)]);
    git(&root, &["init", "--quiet", &path(&primary)]);
    git(&primary, &["remote", "add", "origin", &path(&origin)]);
    git(&primary, &["-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "--quiet", "--allow-empty", "-m", "first"]);
    git(&primary, &["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    git(&primary, &["fetch", "--quiet", "origin"]);
    git(&primary, &["remote", "set-head", "origin", "main"]);
    git(&primary, &["worktree", "add", "--quiet", "-b", "someone/tw-1", &path(&worktree)]);
    (root, worktree)
}

// TW-1's session `s1`, its branch `tw-1-start`, and Tolkster's record of its worker's worktree
// at `recorded`.
fn linear(created_at: &str, recorded: &Path) -> fake::Linear {
    let on = Issue { branch_name: "tw-1-start".into(), created_at: created_at.into(), ..issue() };
    let linear = fake::Linear::with(vec![Session { issue: Some(on), ..session(Vec::new()) }]);
    let id = WorktreeId::from(format!("r1::{}", recorded.display().to_string().replace('\\', "/")).as_str());
    let record = WorkerRecord { issue: "TW-1".into(), worktree_id: id.clone(), session_id: "s1".into(), at: created_at.into(), launch: launch(), rung_from: RungFrom::Claim };
    linear.records.workers.borrow_mut().insert(id, record);
    linear
}

fn set_up_in(root: &Path, dir: &Path, linear: &fake::Linear, worker: &Worker) -> Result<Ready, Error> {
    let config = fake::config(&root.join("<repo>-briefs").display().to_string());
    let mut github = FakeReader::new(|_, _| Err(Error::Failed("GitHub isn't there".into())));
    let mut run = run_command;
    let records = std::rc::Rc::clone(&linear.records);
    let mut host = Host { config: &config, linear, records: &*records, github: &mut github, run: &mut run, dir: &dir.display().to_string(), trust_file: &root.join(".claude.json") };
    set_up(&mut host, worker)
}

const LATER: &str = "2090-01-01T00:00:00.000Z";

// Each startup case on the worker's own host, the branch kept as its Orca made it, prefix
// included, whatever Linear's branch name: pushed with its upstream, or held for the owner's go
// (plan first, a reconcile, or a parent working through its sub-issues). Its briefs and
// settings are on that host's disk, its worktree is trusted, and `claude` starts on its brief.
#[test]
fn a_worker_sets_itself_up_for_each_startup_case() {
    let cases = [
        ("pushed", LATER, start(false, false), true, "is already checked out here and pushed"),
        ("plan-first", LATER, start(true, false), false, "`, isn't pushed."),
        ("reconcile", "2000-01-01T00:00:00.000Z", start(false, false), false, "## Reconcile first"),
        ("orchestrate", LATER, start(false, true), false, "`, isn't pushed."),
    ];
    for (test, created_at, how, pushed, said) in cases {
        let (root, dir) = repo(test);
        let ready = set_up_in(&root, &dir, &linear(created_at, &dir), &worker(how)).unwrap();
        assert_eq!(git(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]), "someone/tw-1", "{test}");
        let upstream = run_command("git", &["rev-parse", "--abbrev-ref", "@{upstream}"], Some(dir.to_str().unwrap())).unwrap();
        assert_eq!(upstream.ok, pushed, "{test}: {:?}", ready.problems);
        if pushed {
            assert_eq!(upstream.stdout.trim(), "origin/someone/tw-1", "{test}");
        }
        let briefs = root.join("tolkster-briefs");
        let brief = std::fs::read_to_string(briefs.join("TW-1.md")).unwrap();
        assert!(brief.contains(said) && brief.contains("`someone/tw-1`") && !brief.contains("tw-1-start"), "{test}");
        for name in ["TW-1-round", "TW-1-rebase", "TW-1-deep", "TW-1-reviewer", "TW-1-last-round", "hooks", "mcp"] {
            assert!(briefs.join(name).with_extension(if name.starts_with("TW") { "md" } else { "json" }).is_file(), "{test}: {name}");
        }
        let hooks = briefs.join("hooks.json").display().to_string();
        assert_eq!(&ready.args[..4], ["--model", "claude-sonnet-5-5", "--effort", "high"], "{test}");
        assert!(ready.args.contains(&hooks) && ready.args.last().unwrap().starts_with(&format!("Read the file {}", briefs.join("TW-1.md").display())), "{test}: {:?}", ready.args);
        // Copilot's rule couldn't be read from a local origin: the worker starts all the same.
        assert!(ready.problems.iter().any(|p| p.contains("Copilot")), "{test}: {:?}", ready.problems);
        assert!(!crate::harness::trust(&root.join(".claude.json"), &dir.display().to_string()).unwrap(), "{test}: trusted already");
        std::fs::remove_dir_all(&root).ok();
    }
}

// A line run in a worktree that isn't the session's worker's, or for a session on another
// issue, names, pushes and writes nothing.
#[test]
fn a_setup_acts_only_in_its_own_workers_worktree() {
    let (root, dir) = repo("refused");
    let elsewhere = root.join("primary");
    for (linear, worker) in [
        (linear(LATER, &elsewhere), worker(start(false, false))),
        (linear(LATER, &dir), Worker { identifier: "TW-2".into(), ..worker(start(false, false)) }),
    ] {
        assert!(set_up_in(&root, &dir, &linear, &worker).is_err());
        assert_eq!(git(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]), "someone/tw-1");
        assert!(!root.join("tolkster-briefs").exists() && !root.join(".claude.json").exists());
    }
    std::fs::remove_dir_all(&root).ok();
}

// A resume continues the worker's session on its line; a round starts a new session on its
// round brief where this host has it, and otherwise continues on the round's line. Neither
// reads Linear or git, and neither runs outside its issue's worker's worktree, whatever its
// session: a resume's may be a later one.
#[test]
fn a_resume_and_a_round_relaunch_on_this_hosts_briefs() {
    let root = std::env::temp_dir().join(format!("tolkster-setup-resume-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let unrecorded = fake::Linear::default();
    assert!(set_up_in(&root, &root, &unrecorded, &worker(How::Continue(Resume::Message))).is_err());
    assert!(!root.join(".claude.json").exists());
    let linear = linear(LATER, &root);
    linear.records.workers.borrow_mut().values_mut().for_each(|record| record.session_id = "s0".into());
    let resumed = set_up_in(&root, &root, &linear, &worker(How::Continue(Resume::Message))).unwrap();
    assert_eq!((&resumed.args[0], resumed.args.last().unwrap()), (&"--continue".to_string(), &voice::new_message(&"s1".into()).unwrap().to_string()));
    let round = worker(How::Round { number: 7, round: Round::Review { last: false } });
    let unbriefed = set_up_in(&root, &root, &linear, &round).unwrap();
    assert_eq!((&unbriefed.args[0], unbriefed.args.last().unwrap()), (&"--continue".to_string(), &review::line(7, &"TW-1".into(), &"s1".into(), Round::Review { last: false }).unwrap().to_string()));
    for name in ["TW-1", "TW-1-round"] {
        write_file(&root.join("tolkster-briefs").join(format!("{name}.md")), "brief").unwrap();
    }
    let fresh = set_up_in(&root, &root, &linear, &round).unwrap();
    assert!(fresh.args[0] != "--continue" && fresh.args.last().unwrap().starts_with(&format!("Read the file {}", root.join("tolkster-briefs").join("TW-1-round.md").display())), "{:?}", fresh.args);
    // Without its repo it has no settings to point at, and runs without the hook.
    let unhooked = set_up_in(&root, &root, &linear, &Worker { repo: None, ..worker(How::Continue(Resume::Message)) }).unwrap();
    assert!(!unhooked.args.iter().any(|a| a == "--settings"));
    assert!(resumed.problems.is_empty(), "{:?}", resumed.problems);
    std::fs::remove_dir_all(&root).ok();
}

// An issue started again after its first worker's host was unpaired carries on from what that
// worker pushed to origin, on whatever branch Orca gave it. A branch of another issue and a
// checkout with a commit of its own are left alone.
#[test]
fn a_worker_started_again_carries_on_from_the_branch_pushed_to_origin() {
    let (root, dir) = repo("carry-on");
    let origin = root.join("origin.git").display().to_string();
    let sha = |branch: &str| git(&dir, &["rev-parse", branch]);
    let commit = |branch: &str, message: &str| {
        git(&dir, &["checkout", "--quiet", "-b", branch, "origin/main"]);
        git(&dir, &["-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "--quiet", "--allow-empty", "-m", message]);
        git(&dir, &["push", "--quiet", &origin, &format!("{branch}:{branch}")]);
        let tip = sha("HEAD");
        git(&dir, &["checkout", "--quiet", "someone/tw-1"]);
        tip
    };
    commit("dead/tw-10", "another issue");
    let pushed = commit("dead/tw-1", "the first worker's work");
    git(&dir, &["fetch", "--quiet", "origin"]);
    let key = IssueKey::check("TW-1").unwrap();
    let mut run = run_command;
    let dir_str = dir.display().to_string();
    assert_eq!(git::continue_issue_branch(&mut run, &dir_str, &key).unwrap().as_deref(), Some("dead/tw-1"));
    assert_eq!(sha("HEAD"), pushed);
    assert_eq!(git(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]), "someone/tw-1");
    // A checkout with a commit of its own stays as it is.
    git(&dir, &["reset", "--quiet", "--hard", "origin/main"]);
    git(&dir, &["-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "--quiet", "--allow-empty", "-m", "own"]);
    let own = sha("HEAD");
    assert_eq!(git::continue_issue_branch(&mut run, &dir_str, &key).unwrap(), None);
    assert_eq!(sha("HEAD"), own);
    std::fs::remove_dir_all(&root).ok();
}

// A worker on another harness carries it on its line, and its host launches that harness's
// program with its flags, without Claude's settings or trust.
#[test]
fn a_worker_on_another_harness_is_launched_on_it() {
    let stub = Launch::new(&crate::harnesses::tests::STUB, "stub-large", "deep");
    let resume = Worker { launch: Some(stub.clone()), ..worker(How::Continue(Resume::Message)) };
    let line = resume.command().unwrap();
    assert!(line.as_str().contains(" --harness stub --model stub-large --effort deep "), "{line:?}");
    assert_eq!(parsed(&line).unwrap(), resume);
    let root = std::env::temp_dir().join(format!("tolkster-setup-stub-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let linear = linear(LATER, &root);
    linear.records.workers.borrow_mut().values_mut().for_each(|record| record.launch = stub.clone());
    let ready = set_up_in(&root, &root, &linear, &resume).unwrap();
    assert_eq!(ready.harness, &crate::harnesses::tests::STUB);
    assert_eq!(&ready.args[..7], ["--resume-last", "--no-trust-dialog", "--no-mcp", "-m", "stub-large", "--depth", "deep"]);
    assert_eq!(ready.args.len(), 8, "{:?}", ready.args);
    // Its agent learns its worker from the launch, as the record holds it: its hooks read no records.
    let id = linear.records.workers.borrow().keys().next().cloned().unwrap();
    assert_eq!(ready.env[..2], [(crate::cli::WORKTREE_ENV.to_string(), id.to_string()), (crate::cli::ISSUE_ENV.to_string(), "TW-1".to_string())]);
    // And how it was launched, for its session's timing.
    assert_eq!((ready.env.len(), ready.env[2].0.as_str()), (3, crate::timing::LAUNCH_ENV));
    assert!(crate::timing::launch_of(&ready.env[2].1).is_some_and(|(kind, _, _)| kind == crate::timing::Kind::Resume));
    // A traced worker's line says so, whatever its host's config, and its environment names
    // the session it traces; one that isn't, as above, names none.
    let traced = Worker { trace: true, ..resume.clone() };
    let line = traced.command().unwrap();
    assert!(line.as_str().contains(" --trace "), "{line:?}");
    assert_eq!(parsed(&line).unwrap(), traced);
    let ready = set_up_in(&root, &root, &linear, &traced).unwrap();
    assert_eq!(ready.env.last(), Some(&(crate::cli::TRACE_ENV.to_string(), "s1".to_string())));
    // Timing off, as `listen`'s config has it: the line says so, and the agent's environment
    // has no launch, so its hook neither summarises its session nor reports it.
    let untimed = Worker { timing: false, ..resume.clone() };
    let line = untimed.command().unwrap();
    assert!(line.as_str().contains(" --no-timing"), "{line:?}");
    assert_eq!(parsed(&line).unwrap(), untimed);
    assert!(!resume.command().unwrap().as_str().contains("timing"));
    let ready = set_up_in(&root, &root, &linear, &untimed).unwrap();
    assert!(ready.env.iter().all(|(name, _)| name != crate::timing::LAUNCH_ENV), "{:?}", ready.env);
    assert!(!root.join(".claude.json").exists() && !root.join("tolkster-briefs").exists());
    // A harness this build doesn't launch, or an effort that isn't the harness's, is refused.
    let refused = |words: &[&str]| Worker::parse(&crate::args::parse(&words.iter().map(|w| w.to_string()).collect::<Vec<_>>(), &OPTIONS, true).unwrap()).is_err();
    assert!(refused(&["s1", "--issue", "TW-1", "--harness", "codex", "--model", "gpt", "--effort", "high", "--continue", "--then", "message"]));
    assert!(refused(&["s1", "--issue", "TW-1", "--harness", "stub", "--model", "stub", "--effort", "high", "--continue", "--then", "message"]));
    assert!(refused(&["s1", "--issue", "TW-1", "--harness", "stub", "--continue", "--then", "message"]));
    assert!(refused(&["s1", "--issue", "TW-1", "--harness", "bare", "--model", "m", "--effort", "deep", "--continue", "--then", "message"]));
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_worktree_is_named_after_the_issue_from_linears_branch_name() {
    let named = |identifier: &str, branch_name: &str| worktree_name(&Issue { identifier: identifier.into(), branch_name: branch_name.into(), ..issue() });
    assert_eq!(named("TW-84", "tolkster-responds-to-mentions-and-replies-tw-84"), "tw-84-tolkster-responds-to-mentions-and");
    // Cut at a word, never inside one, and never past the limit.
    assert_eq!(named("TW-119", "name-worker-worktrees-after-the-issue-as-well-as-its-id-eg-tw-119"), "tw-119-name-worker-worktrees-after-the");
    assert!(named("TW-119", "name-worker-worktrees-after-the-issue-as-well-as-its-id-eg-tw-119").len() <= WORKTREE_NAME_MAX);
    assert_eq!(named("TW-1", "a-verylongwordthatcannotfitinthenameatallreally-tw-1"), "tw-1-a");
    // Linear's other layouts: the ID first, or the user's name before it.
    assert_eq!(named("TW-84", "tw-84-tolkster-responds"), "tw-84-tolkster-responds");
    assert_eq!(named("TW-84", "sam/tw-84-tolkster-responds"), "tw-84-tolkster-responds");
    // Doubled hyphens don't make empty words.
    assert_eq!(named("TW-84", "fix--it-tw-84"), "tw-84-fix-it");
    // No title, another issue's branch, or anything outside [a-z0-9-]: the bare ID.
    assert_eq!(named("TW-1", "tw-1"), "tw-1");
    assert_eq!(named("TW-1", ""), "tw-1");
    assert_eq!(named("TW-1", "something-else-tw-2"), "tw-1");
    assert_eq!(named("TW-1", "rm -rf-tw-1"), "tw-1");
    assert_eq!(named("TW-1", "fix-\"$(x)\"-tw-1"), "tw-1");
    assert_eq!(named("TW-1", "Fix-It-tw-1"), "tw-1");
    assert_eq!(named("TW-1", "caf\u{e9}-tw-1"), "tw-1");
}

#[test]
fn the_baseline_is_the_later_of_creation_and_the_last_description_edit() {
    assert_eq!(baseline(&at(0), &[]), at(0));
    assert_eq!(baseline(&at(0), &[at(5), at(9), at(2)]), at(9));
    // An edit can't be earlier than the creation, but the creation wins if one seems so.
    assert_eq!(baseline(&at(3), &[at(1)]), at(3));
}
