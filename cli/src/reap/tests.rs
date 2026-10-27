use super::*;
use crate::exec::Output;
use crate::ports::fake::{FakeReader, primary, ps, row};
use crate::orca::{Cli, list_worktrees};
use serde_json::{Value, json};

const HEAD: &str = "abc1230000000000000000000000000000000000";
const OTHER: &str = "def4560000000000000000000000000000000000";

// A worker's row whose agent, at pane t1:leaf, has been in `state` for `minutes`, as
// Orca has it from the agent's hooks.
fn agent_row(issue: &str, path: &str, state: &str, minutes: i64) -> Value {
    let mut row = row(Some(issue), path);
    row["agents"] = json!([{ "paneKey": "t1:leaf", "state": state, "stateStartedAt": now_ms() - minutes * 60_000 }]);
    row
}

// The agent's terminal `handle` in a worktree, at pane t1:leaf.
fn agent_terminal_in(worktree_id: &str, handle: &str) -> String {
    json!({ "result": { "terminals": [{ "handle": handle, "worktreeId": worktree_id, "agentIdentity": "claude", "tabId": "t1", "leafId": "leaf" }] } }).to_string()
}

const TW1: &str = "repo::D:/wt/tw-1";

// Reads Orca, then reaps, as `tolkster reap` does, through the CLI's adapter over the fake
// shell: each branch's PRs read from GitHub.
fn reap_all(run: &mut Run, gh: &mut dyn Reader, forget: &mut Forget, tolksters: impl FnMut(&[IssueKey]) -> Result<Tolksters, Error>, dry_run: bool) -> Result<Vec<String>, Error> {
    let worktrees = list_worktrees(run)?;
    let mut asked = 0;
    let mut watched = |p: &str, a: &[&str], d: Option<&str>| {
        asked += usize::from(p == "orca" && a.starts_with(&["worktree", "rm"]));
        run(p, a, d)
    };
    let mut branches = Branches::default();
    let reaped = reap(&mut Cli::alone(&mut watched), &mut read_prs(gh, &mut branches), forget, &worktrees, tolksters, dry_run)?;
    // It says it tried to remove a worktree exactly when it asked Orca to, whether or
    // not Orca managed it, and counts each one it tried.
    assert_eq!(reaped.removed, asked > 0);
    assert_eq!(reaped.tried, asked);
    // It failed exactly when a line says a worktree's turn errored, for what wanted it gone.
    assert_eq!(reaped.failed, reaped.lines.iter().any(|l| matches!(l.what, Reaping::Failed(_))), "{:?}", reaped.lines);
    assert_eq!(reaped.spent, None);
    Ok(reaped.lines.iter().map(ToString::to_string).collect())
}

// GitHub, answering every branch's PRs with `prs`, as github::branch_prs reads them.
fn gh(prs: Value) -> FakeReader<'static> {
    FakeReader::always(prs_answer(prs, false))
}

// The branch's PRs query answered with `prs`, each the branch's own, and `more` after them.
fn prs_answer(prs: Value, more: bool) -> Value {
    let own: Vec<Value> = prs.as_array().unwrap().iter().cloned().map(|mut pr| {
        pr["isCrossRepository"] = json!(false);
        pr
    }).collect();
    json!({ "repository": { "all": { "nodes": own, "pageInfo": { "hasNextPage": more } } } })
}

// What `gh` would answer, as the reap rules read it.
fn read(prs: &Value) -> Vec<BranchPr> {
    crate::github::branch_prs(&mut gh(prs.clone()), "o/n", "b").unwrap()
}

const ORIGIN: &str = "https://github.com/Tolkien-Works/tolkster.git
";

// What `orca terminal list` prints with none open.
const NO_TERMINALS: &str = r#"{ "ok": true, "result": { "terminals": [] } }"#;

// A fake shell: answers from `answer`, and records every command it was asked to run,
// and each read of GitHub (`reads`).
struct Fake {
    calls: Vec<String>,
    reads: Vec<String>,
    ps: String,
    prs: Value,
    head: String,
    status: String,
    branch_exists: bool,
    terminals: String,
    // The worktrees whose worker records were forgotten.
    forgotten: Vec<String>,
    // Where every issue stands now, as Linear says.
    now: IssueNow,
    // What `git rev-list --count` says of the commits at HEAD on no remote.
    unpushed: String,
    // How many more times `git worktree list` still names the removed worktree, as git does
    // for a moment after `orca worktree rm` returns.
    still_listed: usize,
}

impl Fake {
    fn new(ps: String, prs: Value) -> Fake {
        Fake { calls: Vec::new(), reads: Vec::new(), ps, prs, head: format!("{HEAD}\n"), status: String::new(), branch_exists: true, terminals: NO_TERMINALS.into(), forgotten: Vec::new(), now: IssueNow::Open, unpushed: "0\n".into(), still_listed: 0 }
    }

    fn answer(&mut self, program: &str, args: &[&str], dir: Option<&str>) -> Result<Output, Error> {
        self.calls.push(format!("{}{program} {}", dir.map(|d| format!("[{d}] ")).unwrap_or_default(), args.join(" ")));
        let ok = |stdout: &str| Ok(Output::ok(stdout));
        match (program, args.first().copied().unwrap_or("")) {
            ("orca", _) if args[1] == "ps" => ok(&self.ps),
            // A closed terminal is gone from the list.
            ("orca", "terminal") if args[1] == "close" => {
                self.terminals = NO_TERMINALS.into();
                ok("{}")
            }
            ("orca", "terminal") => ok(&self.terminals.clone()),
            ("orca", _) => ok("{}"),
            ("git", "remote") => ok(ORIGIN),
            ("git", "rev-parse") => ok(&self.head.clone()),
            ("git", "status") => ok(&self.status.clone()),
            ("git", "rev-list") => ok(&self.unpushed.clone()),
            ("git", "worktree") => {
                let listed = self.still_listed > 0;
                self.still_listed = self.still_listed.saturating_sub(1);
                ok(&format!("worktree D:/main\nHEAD {HEAD}\nbranch refs/heads/main\n{}", if listed { format!("\nworktree D:/wt/tw-1\nHEAD {HEAD}\nbranch refs/heads/tw-1\n") } else { String::new() }))
            }
            ("git", "show-ref") => Ok(if self.branch_exists { Output::ok("") } else { Output::failed(1, "") }),
            _ => ok(""),
        }
    }

    fn reap(&mut self, tolksters: &[&str], dry_run: bool) -> Vec<String> {
        let tolksters: Tolksters = tolksters.iter().map(|s| (s.to_string(), self.now.clone())).collect();
        let mut forgotten = Vec::new();
        let mut gh = gh(self.prs.clone());
        let lines = {
            let mut run = |p: &str, a: &[&str], d: Option<&str>| self.answer(p, a, d);
            let mut forget = |id: &str| {
                forgotten.push(id.to_string());
                Ok(())
            };
            reap_all(&mut run, &mut gh, &mut forget, |_| Ok(tolksters.clone()), dry_run).unwrap()
        };
        self.reads.extend(gh.calls);
        self.forgotten.extend(forgotten);
        lines
    }

    fn removed(&self) -> bool {
        self.calls.iter().any(|c| c.starts_with("orca worktree rm"))
    }

    fn closed(&self) -> bool {
        self.calls.iter().any(|c| c.starts_with("orca terminal close"))
    }
}

fn theirs(issues: &[&str]) -> Tolksters {
    issues.iter().map(|i| (i.to_string(), IssueNow::Open)).collect()
}

fn merged() -> Value {
    json!([{ "number": 7, "state": "MERGED", "headRefOid": HEAD }])
}

#[test]
fn the_verdict_names_the_newest_merged_pr() {
    let prs = json!([{ "number": 7, "state": "MERGED", "headRefOid": "a" }, { "number": 9, "state": "MERGED", "headRefOid": "b" }]);
    assert_eq!(verdict(&read(&prs)), Verdict::Merged { number: 9, head: "b".into() });
    assert_eq!(verdict(&read(&json!([prs[0]]))), Verdict::Merged { number: 7, head: "a".into() });
    assert_eq!(verdict(&[]), Verdict::Keep);
}

#[test]
fn a_merged_workers_worktree_is_removed_and_its_branch_left_to_orca() {
    let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), merged());
    assert_eq!(fake.reap(&["TW-1"], false), ["TW-1: removed D:/wt/tw-1 and branch tw-1 (PR #7 merged)"]);
    assert_eq!(fake.calls.last().unwrap(), "orca worktree rm --worktree id:repo::D:/wt/tw-1 --force --json");
    // The origin comes with the listing, read once in the primary checkout.
    assert_eq!(fake.calls.iter().filter(|c| c.contains("git remote")).collect::<Vec<_>>(), ["[D:/main] git remote get-url origin"]);
    assert_eq!(fake.reads, [r#"graphql Tolkien-Works/tolkster {"branch":"tw-1"}"#]);
    assert_eq!(fake.forgotten, ["repo::D:/wt/tw-1"]);
    // Git still holds the branch for a moment after the removal, and Orca deletes it itself.
    assert!(!fake.calls.iter().any(|c| c.contains("branch -D") || c.contains("show-ref") || c.contains("worktree list")));
}

#[test]
fn a_running_agent_keeps_a_merged_worktree() {
    // Auto-merge merged the PR while its worker was still finishing.
    let terminal = |agent: &str, orphaned: bool| json!({ "result": { "terminals": [
        { "handle": "term_1", "worktreeId": "repo::D:/wt/tw-1", "agentIdentity": agent, "orphaned": orphaned },
    ] } }).to_string();
    let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), merged());
    fake.terminals = terminal("claude", false);
    assert!(fake.reap(&["TW-1"], false).is_empty());
    assert!(!fake.removed());
    // Nor is GitHub asked about it until the agent has exited.
    assert!(fake.reads.is_empty());
    // An agent resumed after Orca was first read still keeps it: the terminals are read
    // again just before removing.
    let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), merged());
    let resumed = terminal("claude", false);
    let mut lists = 0;
    let mut run = |p: &str, a: &[&str], d: Option<&str>| {
        if p == "orca" && a.first() == Some(&"terminal") {
            lists += 1;
            if lists > 1 {
                return Ok(Output::ok(resumed.clone()));
            }
        }
        fake.answer(p, a, d)
    };
    assert!(reap_all(&mut run, &mut gh(merged()), &mut |_| Ok(()), |_| Ok(theirs(&["TW-1"])), false).unwrap().is_empty());
    assert_eq!(lists, 2);
    assert!(!fake.removed());
    // A shell, or a terminal whose process is gone, isn't an agent.
    for terminals in [terminal("", false), terminal("claude", true)] {
        let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), merged());
        fake.terminals = terminals;
        assert_eq!(fake.reap(&["TW-1"], false), ["TW-1: removed D:/wt/tw-1 and branch tw-1 (PR #7 merged)"]);
    }
}

#[test]
fn an_idle_agent_in_a_merged_prs_worktree_is_closed_and_the_worktree_reaped() {
    // Done with its turn, or waiting at a prompt, for IDLE_FOR_MS: the worker never
    // closed its terminal.
    for state in ["done", "waiting", "blocked"] {
        let mut fake = Fake::new(ps(json!([primary(), agent_row("TW-1", "D:/wt/tw-1", state, 11)])), merged());
        fake.terminals = agent_terminal_in(TW1, "term_1");
        assert_eq!(fake.reap(&["TW-1"], false), ["TW-1: removed D:/wt/tw-1 and branch tw-1 (PR #7 merged; closed its idle agent)"], "{state}");
        let orca: Vec<&str> = fake.calls.iter().filter(|c| c.starts_with("orca")).map(String::as_str).collect();
        // Listed, then its host read again just before closing, closed, read once more before
        // removing, and removed.
        assert_eq!(
            orca,
            [
                "orca worktree ps --limit 1000 --json",
                "orca terminal list --limit 1000 --json",
                "orca worktree ps --limit 1000 --json",
                "orca terminal list --limit 1000 --json",
                "orca terminal close --terminal term_1 --json",
                "orca worktree ps --limit 1000 --json",
                "orca terminal list --limit 1000 --json",
                "orca worktree rm --worktree id:repo::D:/wt/tw-1 --force --json",
            ],
            "{state}"
        );
        // The checkout is checked before anything is closed.
        let at = |what: &str| fake.calls.iter().position(|c| c.contains(what)).unwrap();
        assert!(at("git status") < at("terminal close"), "{state}");
        assert_eq!(fake.forgotten, ["repo::D:/wt/tw-1"]);
    }
}

// An agent Orca lists with an empty handle still runs, so keeps its worktree, idle or not:
// closing it by that handle would close the active terminal instead.
#[test]
fn an_agent_with_an_empty_handle_keeps_a_merged_worktree_and_nothing_is_closed() {
    for state in ["working", "done"] {
        let mut fake = Fake::new(ps(json!([primary(), agent_row("TW-1", "D:/wt/tw-1", state, 60)])), merged());
        fake.terminals = agent_terminal_in(TW1, "");
        assert!(fake.reap(&["TW-1"], false).is_empty(), "{state}");
        assert!(!fake.removed() && !fake.closed(), "{state}");
    }
}

#[test]
fn a_working_or_lately_idle_agent_keeps_a_merged_worktree_and_nothing_is_closed() {
    // Working, idle for less than IDLE_FOR_MS, in a state Orca adds later, or with no
    // state at all: only a positive idle counts.
    let rows = [agent_row("TW-1", "D:/wt/tw-1", "working", 60), agent_row("TW-1", "D:/wt/tw-1", "done", 9), agent_row("TW-1", "D:/wt/tw-1", "compacting", 60), row(Some("TW-1"), "D:/wt/tw-1")];
    for row in rows {
        let mut fake = Fake::new(ps(json!([primary(), row.clone()])), merged());
        fake.terminals = agent_terminal_in(TW1, "term_1");
        assert!(fake.reap(&["TW-1"], false).is_empty(), "{row}");
        assert!(!fake.removed() && !fake.closed(), "{row}");
        // Nor is GitHub asked about it until the agent has exited or sat idle.
        assert!(fake.reads.is_empty(), "{row}");
    }
}

#[test]
fn a_worktree_is_never_reaped_nor_its_idle_agent_closed_unless_its_pr_merged_and_the_checkout_may_go() {
    for agent in [true, false] {
        let fake = |prs: Value| {
            let listed = if agent { agent_row("TW-1", "D:/wt/tw-1", "done", 60) } else { row(Some("TW-1"), "D:/wt/tw-1") };
            let mut fake = Fake::new(ps(json!([primary(), listed])), prs);
            if agent {
                fake.terminals = agent_terminal_in(TW1, "term_1");
            }
            fake
        };
        for prs in [
            json!([]),
            json!([{ "number": 7, "state": "OPEN", "headRefOid": HEAD }]),
            json!([{ "number": 7, "state": "CLOSED", "headRefOid": HEAD }]),
            // Merged once, then reopened on the same branch: the new PR is still in review.
            json!([{ "number": 7, "state": "MERGED", "headRefOid": "old" }, { "number": 9, "state": "OPEN", "headRefOid": HEAD }]),
        ] {
            let mut fake = fake(prs.clone());
            assert!(fake.reap(&["TW-1"], false).is_empty(), "{agent} {prs}");
            assert!(!fake.removed() && !fake.closed(), "{agent} {prs}");
        }
        // Merged, but the checkout would lose work: kept, and any agent left open.
        let mut ahead = fake(merged());
        ahead.head = format!("{OTHER}\n");
        assert_eq!(ahead.reap(&["TW-1"], false), [format!("TW-1: kept D:/wt/tw-1: PR #7 merged, but the checkout is at {OTHER}, not the PR's head")]);
        let mut dirty = fake(merged());
        dirty.status = " M README.md\n".into();
        assert_eq!(dirty.reap(&["TW-1"], false), ["TW-1: kept D:/wt/tw-1: PR #7 merged, but the checkout has uncommitted or ignored files, e.g. README.md"]);
        for fake in [ahead, dirty] {
            assert!(!fake.removed() && !fake.closed(), "{agent}");
        }
    }
    // A cancelled issue's idle agent is left to its halt, however recent the cancel,
    // and even with a merged PR.
    let idle = || ps(json!([primary(), agent_row("TW-1", "D:/wt/tw-1", "done", 60)]));
    let mut cancelled = Fake::new(idle(), json!([]));
    cancelled.terminals = agent_terminal_in(TW1, "term_1");
    cancelled.now = self::cancelled(48);
    assert!(cancelled.reap(&["TW-1"], false).is_empty());
    let mut recent = Fake::new(idle(), merged());
    recent.terminals = agent_terminal_in(TW1, "term_1");
    recent.now = self::cancelled(1);
    assert!(recent.reap(&["TW-1"], false).is_empty());
    for fake in [cancelled, recent] {
        assert!(!fake.removed() && !fake.closed());
    }
}

#[test]
fn an_agent_reached_since_the_listing_is_not_closed() {
    // Orca is read again just before closing: the agent is working again by then, or
    // another agent's terminal has taken its place.
    let idle = || ps(json!([primary(), agent_row("TW-1", "D:/wt/tw-1", "done", 60)]));
    let working = ps(json!([primary(), agent_row("TW-1", "D:/wt/tw-1", "working", 0)]));
    let mut fake = Fake::new(idle(), merged());
    fake.terminals = agent_terminal_in(TW1, "term_1");
    let mut lists = 0;
    let mut run = |p: &str, a: &[&str], d: Option<&str>| {
        if p == "orca" && a.starts_with(&["worktree", "ps"]) {
            lists += 1;
            if lists > 1 {
                return Ok(Output::ok(working.clone()));
            }
        }
        fake.answer(p, a, d)
    };
    assert!(reap_all(&mut run, &mut gh(merged()), &mut |_| Ok(()), |_| Ok(theirs(&["TW-1"])), false).unwrap().is_empty());
    assert_eq!(lists, 2);
    assert!(!fake.removed() && !fake.closed());

    let mut fake = Fake::new(idle(), merged());
    fake.terminals = agent_terminal_in(TW1, "term_1");
    let replaced = agent_terminal_in(TW1, "term_2");
    let mut lists = 0;
    let mut run = |p: &str, a: &[&str], d: Option<&str>| {
        if p == "orca" && a.starts_with(&["terminal", "list"]) {
            lists += 1;
            if lists > 1 {
                return Ok(Output::ok(replaced.clone()));
            }
        }
        fake.answer(p, a, d)
    };
    assert!(reap_all(&mut run, &mut gh(merged()), &mut |_| Ok(()), |_| Ok(theirs(&["TW-1"])), false).unwrap().is_empty());
    assert!(!fake.removed() && !fake.closed());
}

#[test]
fn a_close_that_fails_or_leaves_the_terminal_listed_removes_nothing() {
    let idle = || ps(json!([primary(), agent_row("TW-1", "D:/wt/tw-1", "done", 60)]));
    let mut fake = Fake::new(idle(), merged());
    fake.terminals = agent_terminal_in(TW1, "term_1");
    let mut run = |p: &str, a: &[&str], d: Option<&str>| {
        if p == "orca" && a.starts_with(&["terminal", "close"]) {
            return Ok(Output::failed(1, "no such terminal"));
        }
        fake.answer(p, a, d)
    };
    let lines = reap_all(&mut run, &mut gh(merged()), &mut |_| Ok(()), |_| Ok(theirs(&["TW-1"])), false).unwrap();
    assert_eq!(lines, ["TW-1: error: orca terminal close failed: no such terminal"]);
    assert!(!fake.removed());
    // Closed, but Orca still lists the terminal: the worktree waits for its exit.
    let mut fake = Fake::new(idle(), merged());
    fake.terminals = agent_terminal_in(TW1, "term_1");
    let still = fake.terminals.clone();
    let mut run = |p: &str, a: &[&str], d: Option<&str>| {
        if p == "orca" && a.starts_with(&["terminal", "close"]) {
            fake.calls.push("orca terminal close".into());
            return Ok(Output::ok(still.clone()));
        }
        fake.answer(p, a, d)
    };
    let lines = reap_all(&mut run, &mut gh(merged()), &mut |_| Ok(()), |_| Ok(theirs(&["TW-1"])), false).unwrap();
    assert_eq!(lines, ["TW-1: closed the idle agent in D:/wt/tw-1 (PR #7 merged); its terminal is still listed, so the worktree waits"]);
    assert!(fake.closed() && !fake.removed() && fake.forgotten.is_empty());
    // Closed, then the terminals can't be listed, or Orca refuses the removal: the line
    // still says the agent was closed.
    let mut fake = Fake::new(idle(), merged());
    fake.terminals = agent_terminal_in(TW1, "term_1");
    let mut lists = 0;
    let mut run = |p: &str, a: &[&str], d: Option<&str>| {
        if p == "orca" && a.starts_with(&["terminal", "list"]) {
            lists += 1;
            if lists > 2 {
                return Ok(Output::failed(1, "offline"));
            }
        }
        fake.answer(p, a, d)
    };
    let lines = reap_all(&mut run, &mut gh(merged()), &mut |_| Ok(()), |_| Ok(theirs(&["TW-1"])), false).unwrap();
    assert_eq!(lines, ["TW-1: error: closed the idle agent in D:/wt/tw-1 (PR #7 merged), but orca terminal list failed: offline"]);
    assert!(fake.closed() && !fake.removed());
    let mut fake = Fake::new(idle(), merged());
    fake.terminals = agent_terminal_in(TW1, "term_1");
    let mut run = |p: &str, a: &[&str], d: Option<&str>| {
        if p == "orca" && a.starts_with(&["worktree", "rm"]) {
            return Ok(Output::failed(1, "busy"));
        }
        fake.answer(p, a, d)
    };
    let lines = reap_all(&mut run, &mut gh(merged()), &mut |_| Ok(()), |_| Ok(theirs(&["TW-1"])), false).unwrap();
    assert_eq!(lines, ["TW-1: error: couldn't remove D:/wt/tw-1 (PR #7 merged; closed its idle agent): orca worktree rm failed: busy"]);
}

#[test]
fn a_dry_run_says_it_would_close_the_idle_agent() {
    let mut fake = Fake::new(ps(json!([primary(), agent_row("TW-1", "D:/wt/tw-1", "done", 60)])), merged());
    fake.terminals = agent_terminal_in(TW1, "term_1");
    assert_eq!(fake.reap(&["TW-1"], true), ["TW-1: would close its idle agent and remove D:/wt/tw-1 and branch tw-1 (PR #7 merged)"]);
    assert!(!fake.closed() && !fake.removed() && fake.forgotten.is_empty());
}

#[test]
fn a_branch_with_more_prs_than_a_page_is_never_judged_on_some() {
    let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), merged());
    let mut gh = FakeReader::always(prs_answer(merged(), true));
    let mut run = |p: &str, a: &[&str], d: Option<&str>| fake.answer(p, a, d);
    let lines = reap_all(&mut run, &mut gh, &mut |_| Ok(()), |_| Ok(theirs(&["TW-1"])), false).unwrap();
    assert_eq!(lines, ["TW-1: error: branch tw-1 has over 100 pull requests in Tolkien-Works/tolkster"]);
    assert!(!fake.removed());
}

#[test]
fn worktrees_not_linked_to_a_tolkster_issue_are_never_touched() {
    let rows = json!([primary(), row(Some("OZGA-64"), "D:/wt/ozga-64"), row(None, "D:/wt/mine")]);
    let mut fake = Fake::new(ps(rows), merged());
    assert!(fake.reap(&["TW-1"], false).is_empty());
    assert_eq!(fake.calls, ["orca worktree ps --limit 1000 --json", "[D:/main] git remote get-url origin"]);
    assert!(fake.reads.is_empty());
}

#[test]
fn with_no_linked_worktree_linear_is_not_asked() {
    let mut run = |p: &str, a: &[&str], d: Option<&str>| Fake::new(ps(json!([primary()])), merged()).answer(p, a, d);
    let lines = reap_all(&mut run, &mut gh(merged()), &mut |_| Ok(()), |_| Err(Error::Failed("Linear was asked".into())), false).unwrap();
    assert!(lines.is_empty());
}

#[test]
fn claudes_lock_file_does_not_keep_a_worktree_but_other_files_do() {
    assert!(would_lose("!! .claude/scheduled_tasks.lock
").is_empty());
    assert_eq!(would_lose("!! .claude/scheduled_tasks.lock
!! .claude/settings.local.json
?? .claude/scheduled_tasks.lock
"), [".claude/settings.local.json", ".claude/scheduled_tasks.lock"]);
}

#[test]
fn the_clean_check_pins_untracked_files_against_git_config() {
    let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), merged());
    fake.reap(&["TW-1"], false);
    assert!(fake.calls.iter().any(|c| c == "[D:/wt/tw-1] git status --porcelain --ignored --untracked-files=normal"), "{:?}", fake.calls);
}

#[test]
fn ignored_files_are_kept_but_ignored_directories_are_build_output() {
    assert!(would_lose("!! cli/target/
!! node_modules/
").is_empty());
    assert_eq!(would_lose("!! cli/target/
!! .dev.vars
?? notes.md
"), [".dev.vars", "notes.md"]);

    let mut secrets = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), merged());
    secrets.status = "!! node_modules/
!! .env
".into();
    assert_eq!(secrets.reap(&["TW-1"], false), ["TW-1: kept D:/wt/tw-1: PR #7 merged, but the checkout has uncommitted or ignored files, e.g. .env"]);
    assert!(!secrets.removed());
}

#[test]
fn a_branch_git_cannot_check_is_reported_not_taken_as_gone() {
    let at = cancelled(25);
    let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), json!([]));
    let mut run = |p: &str, a: &[&str], d: Option<&str>| {
        if a.first() == Some(&"show-ref") {
            return Ok(Output::failed(128, "fatal: not a git repository
"));
        }
        fake.answer(p, a, d)
    };
    let lines = reap_all(&mut run, &mut gh(json!([])), &mut |_| Ok(()), |_| Ok(Tolksters::from([("TW-1".to_string(), at.clone())])), false).unwrap();
    assert_eq!(lines, ["TW-1: error: removed D:/wt/tw-1 (issue cancelled), but couldn't check branch tw-1: fatal: not a git repository"]);
}

#[test]
fn a_remote_primary_is_not_used_to_delete_the_branch() {
    let mut remote_primary = primary();
    remote_primary["hostId"] = json!("ssh-box");
    let mut fake = Fake::new(ps(json!([remote_primary, row(Some("TW-1"), "D:/wt/tw-1")])), merged());
    assert_eq!(fake.reap(&["TW-1"], false), ["TW-1: removed D:/wt/tw-1 (PR #7 merged); kept branch tw-1: no local primary checkout"]);
    assert!(!fake.calls.iter().any(|c| c.contains("show-ref") || c.contains("branch -D")));
}

#[test]
fn a_dry_run_removes_nothing() {
    let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), merged());
    assert_eq!(fake.reap(&["TW-1"], true), ["TW-1: would remove D:/wt/tw-1 and branch tw-1 (PR #7 merged)"]);
    assert!(!fake.removed() && fake.forgotten.is_empty());
}

#[test]
fn a_worker_record_that_cant_be_forgotten_is_noted_and_the_rest_goes_on() {
    let at = cancelled(25);
    let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), merged());
    let mut run = |p: &str, a: &[&str], d: Option<&str>| fake.answer(p, a, d);
    let lines = reap_all(&mut run, &mut gh(merged()), &mut |_| Err(Error::Failed("offline".into())), |_| Ok(theirs(&["TW-1"])), false).unwrap();
    assert_eq!(lines, ["TW-1: removed D:/wt/tw-1 and branch tw-1 (PR #7 merged; its worker record stays: offline)"]);
    // The branch can't be deleted either: the line still says the worktree went, and
    // that the record stays.
    let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), json!([]));
    let mut run = |p: &str, a: &[&str], d: Option<&str>| {
        if a.first() == Some(&"branch") {
            return Ok(Output::failed(1, "locked"));
        }
        fake.answer(p, a, d)
    };
    let lines = reap_all(&mut run, &mut gh(json!([])), &mut |_| Err(Error::Failed("offline".into())), |_| Ok(Tolksters::from([("TW-1".to_string(), at.clone())])), false).unwrap();
    assert_eq!(lines, ["TW-1: error: removed D:/wt/tw-1 (issue cancelled; its worker record stays: offline), but couldn't delete branch tw-1: git branch -D failed: locked"]);
}

#[test]
fn a_removal_orca_refuses_is_reported_and_still_counts_as_tried() {
    let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), merged());
    let mut run = |p: &str, a: &[&str], d: Option<&str>| {
        if p == "orca" && a.starts_with(&["worktree", "rm"]) {
            return Ok(Output::failed(1, "busy"));
        }
        fake.answer(p, a, d)
    };
    // reap_all checks the attempt is reported, so the caller lists the worktrees again.
    let lines = reap_all(&mut run, &mut gh(merged()), &mut |_| Ok(()), |_| Ok(theirs(&["TW-1"])), false).unwrap();
    assert_eq!(lines, ["TW-1: error: couldn't remove D:/wt/tw-1 (PR #7 merged): orca worktree rm failed: busy"]);
}

#[test]
fn a_branch_orca_already_deleted_is_not_deleted_again() {
    let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), json!([]));
    fake.now = cancelled(25);
    fake.branch_exists = false;
    assert_eq!(fake.reap(&["TW-1"], false), ["TW-1: removed D:/wt/tw-1 and branch tw-1 (issue cancelled)"]);
    assert!(!fake.calls.iter().any(|c| c.contains("branch -D")));
}

#[test]
fn a_branch_orca_deletes_between_the_check_and_the_delete_still_reports_removed() {
    let at = cancelled(25);
    let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), json!([]));
    let mut checks = 0;
    let mut run = |p: &str, a: &[&str], d: Option<&str>| {
        if a.first() == Some(&"show-ref") {
            checks += 1;
            return Ok(if checks == 1 { Output::ok("") } else { Output::failed(1, "") });
        }
        if a.first() == Some(&"branch") {
            return Ok(Output::failed(1, "branch 'tw-1' not found"));
        }
        fake.answer(p, a, d)
    };
    let lines = reap_all(&mut run, &mut gh(json!([])), &mut |_| Ok(()), |_| Ok(Tolksters::from([("TW-1".to_string(), at.clone())])), false).unwrap();
    assert_eq!(lines, ["TW-1: removed D:/wt/tw-1 and branch tw-1 (issue cancelled)"]);
}

#[test]
fn a_cancelled_reap_deletes_the_branch_only_once_git_has_dropped_the_worktree() {
    let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), json!([]));
    fake.now = cancelled(25);
    fake.still_listed = 3;
    assert_eq!(fake.reap(&["TW-1"], false), ["TW-1: removed D:/wt/tw-1 and branch tw-1 (issue cancelled)"]);
    let listings = fake.calls.iter().filter(|c| c.contains("git worktree list")).count();
    assert_eq!(listings, 4);
    assert_eq!(fake.calls.last().unwrap(), "[D:/main] git branch -D tw-1");
    // A worktree git never lets go of delays the delete by the wait, then git says why.
    let mut stuck = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), json!([]));
    stuck.now = cancelled(25);
    stuck.still_listed = usize::MAX;
    stuck.reap(&["TW-1"], false);
    assert_eq!(stuck.calls.iter().filter(|c| c.contains("git worktree list")).count(), UNLISTED_POLLS as usize);
    assert_eq!(stuck.calls.last().unwrap(), "[D:/main] git branch -D tw-1");
}

#[test]
fn a_worktree_whose_prs_found_the_budget_spent_fails_its_turn_with_the_wait() {
    let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1"), row(Some("TW-2"), "D:/wt/tw-2")])), merged());
    let mut run = |p: &str, a: &[&str], d: Option<&str>| fake.answer(p, a, d);
    let worktrees = list_worktrees(&mut run).unwrap();
    let mut spent = FakeReader::new(|_, _| Err(Error::RateLimited { message: "GitHub's rate limit is spent: it resets in 300 s".into(), wait: Duration::from_secs(300) }));
    let mut branches = Branches::default();
    let reaped = reap(&mut Cli::alone(&mut run), &mut read_prs(&mut spent, &mut branches), &mut |_| Ok(()), &worktrees, |_| Ok(theirs(&["TW-1", "TW-2"])), false).unwrap();
    assert!(reaped.failed && !reaped.removed);
    assert_eq!(reaped.spent, Some(Duration::from_secs(300)));
    // Every worktree was tried: its turn fails, and the others' go on.
    assert_eq!(reaped.lines.len(), 2);
}

#[test]
fn one_worktrees_failure_does_not_stop_the_others() {
    let rows = json!([primary(), row(Some("TW-1"), "D:/wt/tw-1"), row(Some("TW-2"), "D:/wt/tw-2")]);
    let mut fake = Fake::new(ps(rows), merged());
    let mut run = |p: &str, a: &[&str], d: Option<&str>| {
        if a.first() == Some(&"rev-parse") && d == Some("D:/wt/tw-1") {
            return Ok(Output::failed(128, "fatal: not a git repository\n"));
        }
        fake.answer(p, a, d)
    };
    let lines = reap_all(&mut run, &mut gh(merged()), &mut |_| Ok(()), |_| Ok(theirs(&["TW-1", "TW-2"])), false).unwrap();
    assert_eq!(lines, ["TW-1: error: git rev-parse --verify failed: fatal: not a git repository", "TW-2: removed D:/wt/tw-2 and branch tw-2 (PR #7 merged)"]);
}

#[test]
fn a_worktree_whose_origin_cannot_be_read_is_reported_and_the_branch_is_read_once() {
    // Two worktrees of one repo: GitHub is asked once per branch, and a repo whose origin
    // git can't name fails each of its worktrees without asking GitHub.
    let rows = json!([primary(), row(Some("TW-1"), "D:/wt/tw-1"), row(Some("TW-2"), "D:/wt/tw-2")]);
    let mut fake = Fake::new(ps(rows.clone()), merged());
    fake.reap(&["TW-1", "TW-2"], true);
    assert_eq!(fake.reads, [r#"graphql Tolkien-Works/tolkster {"branch":"tw-1"}"#, r#"graphql Tolkien-Works/tolkster {"branch":"tw-2"}"#]);
    let mut fake = Fake::new(ps(rows), merged());
    let mut run = |p: &str, a: &[&str], d: Option<&str>| {
        if a.first() == Some(&"remote") {
            return Ok(Output::failed(2, "error: No such remote 'origin'\n"));
        }
        fake.answer(p, a, d)
    };
    let mut gh = gh(merged());
    let lines = reap_all(&mut run, &mut gh, &mut |_| Ok(()), |_| Ok(theirs(&["TW-1", "TW-2"])), false).unwrap();
    assert_eq!(lines, ["TW-1: error: the origin of D:/wt/tw-1 is not a GitHub repo, or couldn't be read", "TW-2: error: the origin of D:/wt/tw-2 is not a GitHub repo, or couldn't be read"]);
    assert!(gh.calls.is_empty());
    assert!(!fake.removed());
}

// Cancelled `hours` ago.
fn cancelled(hours: i64) -> IssueNow {
    IssueNow::Cancelled(iso(now_ms() - hours * 3_600_000))
}

#[test]
fn a_cancelled_issues_worktree_goes_a_day_later_dirty_or_not() {
    for (status, head) in [("", HEAD), (" M README.md\n?? notes.md\n!! .env\n", OTHER)] {
        let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), json!([]));
        fake.now = cancelled(25);
        fake.status = status.into();
        fake.head = format!("{head}\n");
        assert_eq!(fake.reap(&["TW-1"], false), ["TW-1: removed D:/wt/tw-1 and branch tw-1 (issue cancelled)"]);
        assert_eq!(fake.forgotten, ["repo::D:/wt/tw-1"]);
        assert_eq!(fake.calls.last().unwrap(), "[D:/main] git branch -D tw-1");
        // The checkout isn't checked: the owner cancelled that work.
        assert!(!fake.calls.iter().any(|c| c.contains("git status") || c.contains("rev-parse")));
    }
    // A closed or merged PR doesn't keep it either.
    for prs in [json!([{ "number": 7, "state": "CLOSED", "headRefOid": HEAD }]), json!([{ "number": 7, "state": "MERGED", "headRefOid": "old" }])] {
        let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), prs.clone());
        fake.now = cancelled(25);
        fake.head = format!("{OTHER}\n");
        assert_eq!(fake.reap(&["TW-1"], false), ["TW-1: removed D:/wt/tw-1 and branch tw-1 (issue cancelled)"], "{prs}");
    }
}

#[test]
fn a_cancelled_issues_worktree_is_kept_within_a_day_with_an_open_pr_or_a_running_agent() {
    let rows = || ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")]));
    // Within the day: a mis-click reopened finds its worker to resume.
    let mut recent = Fake::new(rows(), json!([]));
    recent.now = cancelled(23);
    recent.status = " M README.md\n".into();
    assert!(recent.reap(&["TW-1"], false).is_empty());
    // Reopened: no cancel time, and no merged PR.
    let mut reopened = Fake::new(rows(), json!([]));
    assert!(reopened.reap(&["TW-1"], false).is_empty());
    // An open PR: Tolkster never closes one.
    let mut open = Fake::new(rows(), json!([{ "number": 7, "state": "OPEN", "headRefOid": HEAD }]));
    open.now = cancelled(48);
    assert!(open.reap(&["TW-1"], false).is_empty());
    let mut running = Fake::new(rows(), json!([]));
    running.now = cancelled(48);
    running.terminals = json!({ "result": { "terminals": [{ "handle": "term_1", "worktreeId": "repo::D:/wt/tw-1", "agentIdentity": "claude" }] } }).to_string();
    assert!(running.reap(&["TW-1"], false).is_empty());
    for fake in [recent, reopened, open, running] {
        assert!(!fake.removed() && fake.forgotten.is_empty());
    }
}

#[test]
fn an_issue_reopened_after_the_first_read_keeps_its_worktree() {
    // Closed at the first read, reopened (or closed again, later) by the second.
    for first in [cancelled(25), IssueNow::Completed(COMPLETED_AT.into())] {
        let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), json!([]));
        let mut reads = 0;
        let lines = {
            let mut run = |p: &str, a: &[&str], d: Option<&str>| fake.answer(p, a, d);
            let tolksters = |_: &[IssueKey]| {
                reads += 1;
                Ok(Tolksters::from([("TW-1".to_string(), if reads == 1 { first.clone() } else { IssueNow::Open })]))
            };
            reap_all(&mut run, &mut gh(json!([])), &mut |_| Ok(()), tolksters, false).unwrap()
        };
        assert!(lines.is_empty(), "{lines:?}");
        assert_eq!(reads, 2);
        assert!(!fake.removed());
    }
}

#[test]
fn a_dry_run_names_a_cancelled_removal() {
    let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), json!([]));
    fake.now = cancelled(25);
    assert_eq!(fake.reap(&["TW-1"], true), ["TW-1: would remove D:/wt/tw-1 and branch tw-1 (issue cancelled)"]);
    assert!(!fake.removed() && fake.forgotten.is_empty());
}

const COMPLETED_AT: &str = "2026-09-28T01:00:00.000Z";

// A completed issue's worktree, its branch with `prs`, as its agent's exit reaps it.
fn completed(prs: Value) -> Fake {
    let mut fake = Fake::new(ps(json!([primary(), row(Some("TW-1"), "D:/wt/tw-1")])), prs);
    fake.now = IssueNow::Completed(COMPLETED_AT.into());
    fake
}

#[test]
fn a_completed_issues_worktree_with_no_pr_goes_once_clean_and_pushed() {
    let mut fake = completed(json!([]));
    fake.status = "!! target/\n".into();
    assert_eq!(fake.reap(&["TW-1"], false), ["TW-1: removed D:/wt/tw-1 and branch tw-1 (issue completed with no PR)"]);
    assert_eq!(fake.forgotten, ["repo::D:/wt/tw-1"]);
    assert!(fake.calls.iter().any(|c| c == "[D:/wt/tw-1] git rev-list --count HEAD refs/heads/tw-1 --not --remotes"), "{:?}", fake.calls);
    assert_eq!(fake.calls.last().unwrap(), "[D:/main] git branch -D tw-1");
    let mut dry = completed(json!([]));
    assert_eq!(dry.reap(&["TW-1"], true), ["TW-1: would remove D:/wt/tw-1 and branch tw-1 (issue completed with no PR)"]);
    assert!(!dry.removed());
}

#[test]
fn a_completed_issues_worktree_is_kept_with_a_pr_unpushed_or_uncommitted_work_or_an_agent() {
    let mut dirty = completed(json!([]));
    dirty.status = " M README.md\n".into();
    assert_eq!(dirty.reap(&["TW-1"], false), ["TW-1: kept D:/wt/tw-1: issue completed with no PR, but the checkout has uncommitted or ignored files, e.g. README.md"]);
    let mut ahead = completed(json!([]));
    ahead.unpushed = "2\n".into();
    assert_eq!(ahead.reap(&["TW-1"], false), ["TW-1: kept D:/wt/tw-1: issue completed with no PR, but 2 commits on HEAD or the branch are on no remote"]);
    // git couldn't say: kept, never counted as pushed.
    let mut unknown = completed(json!([]));
    unknown.unpushed = String::new();
    assert_eq!(unknown.reap(&["TW-1"], false), ["TW-1: kept D:/wt/tw-1: issue completed with no PR, but git didn't count the commits on no remote"]);
    // Any PR on the branch, open or closed, keeps it from this rule.
    let mut open = completed(json!([{ "number": 7, "state": "OPEN", "headRefOid": HEAD }]));
    assert!(open.reap(&["TW-1"], false).is_empty());
    let mut closed = completed(json!([{ "number": 7, "state": "CLOSED", "headRefOid": HEAD }]));
    assert!(closed.reap(&["TW-1"], false).is_empty());
    // An agent, even one idle long enough for the merged rule to close it, is left alone.
    let mut idle = completed(json!([]));
    idle.ps = ps(json!([primary(), agent_row("TW-1", "D:/wt/tw-1", "done", 30)]));
    idle.terminals = agent_terminal_in(TW1, "term_1");
    assert!(idle.reap(&["TW-1"], false).is_empty());
    // Open, not completed: nothing to do.
    let mut open_issue = completed(json!([]));
    open_issue.now = IssueNow::Open;
    assert!(open_issue.reap(&["TW-1"], false).is_empty());
    for fake in [dirty, ahead, unknown, open, closed, idle, open_issue] {
        assert!(!fake.removed() && !fake.closed() && fake.forgotten.is_empty());
    }
}

// A sweep's cancel or completion sent to another host carries Linear's time on its line,
// checked; the machine's carries none.
#[test]
fn a_steps_line_carries_the_rules_time_only_when_it_has_one() {
    let at = iso(now_ms());
    let step = |rule| Step { issue: "TW-1".into(), branch: "tw-1".into(), rule }.command().map(|line| line.to_string());
    assert_eq!(step(Rule::Cancelled { at: Some(at.clone()), no_pr: true }).unwrap(), format!("tolkster reap-here --issue TW-1 --branch tw-1 --cancelled --at {at} --no-pr; exit $LASTEXITCODE"));
    assert_eq!(step(Rule::Completed { at: None }).unwrap(), "tolkster reap-here --issue TW-1 --branch tw-1 --completed; exit $LASTEXITCODE");
    assert!(step(Rule::Completed { at: Some("yesterday; rm -rf /".into()) }).is_err());
}
