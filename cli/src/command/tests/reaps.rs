use super::*;

// TW-1's worker's worktree, `r1::/wt/tw-1`, on its branch at HEAD_SHA, clean, with the
// branch in the primary checkout too, and its worker record. PR #7 for the branch merged at
// HEAD_SHA, as GitHub answers it.
fn merged_worker(world: &Rc<RefCell<World>>, records: &fake::Records) -> FakeReader<'static> {
    let mut w = world.borrow_mut();
    w.worktrees.push(Worktree { id: WORKTREE.into(), repo_id: "r1".into(), path: "/wt/tw-1".into(), branch: Some("tw-1".into()), linked_issue: Some("TW-1".into()), is_main: false, is_local: true, agents: Vec::new(), origin: Some("acme/tolkster".into()) });
    w.git.insert("/wt/tw-1".into(), fake::Git { head: HEAD_SHA.into(), branch: Some("tw-1".into()), branches: vec!["tw-1".into()], ..fake::Git::default() });
    w.git.insert("/src/tolkster".into(), fake::Git { head: BASE_SHA.into(), branch: Some("main".into()), branches: vec!["main".into(), "tw-1".into()], ..fake::Git::default() });
    let worker = crate::state::WorkerRecord { issue: "i1".into(), worktree_id: WORKTREE.into(), session_id: "s1".into(), at: at(0), launch: crate::config::ladder::Launch::new(crate::harnesses::DEFAULT, "claude-opus-5-5", "medium"), rung_from: crate::state::RungFrom::Claim };
    records.workers.borrow_mut().insert(WORKTREE.into(), worker);
    FakeReader::always(json!({ "repository": { "all": { "nodes": [{ "number": 7, "state": "MERGED", "headRefOid": HEAD_SHA, "isCrossRepository": false }], "pageInfo": { "hasNextPage": false } } } }))
}

fn reap(id: &str, rule: Rule) -> Command {
    Command { kind: Kind::ReapWorktree, ..order(id, Order::ReapWorktree { worktree_id: WORKTREE.into(), rule }) }
}

// PR #7's merge at HEAD_SHA, as the machine names it.
fn merged() -> Rule {
    Rule::Merged { number: 7, head: HEAD_SHA.into() }
}

// A reap on the fakes alone: a merged worker's worktree goes, with its record, and its branch
// in the primary checkout is left to Orca, and a reap sent again finds it gone. One with its agent running,
// or of another issue, stays, and a worktree only an incomplete listing misses isn't gone.
#[test]
fn a_reap_removes_a_merged_workers_worktree_its_record_and_its_branch() {
    let config = fake::config(&briefs("reap"));
    let (linear, records, world) = fakes();
    linear.tolksters.borrow_mut().insert("TW-1".into(), crate::sessions::IssueNow::Open);
    let mut github = merged_worker(&world, &records);
    // Its agent is running: kept.
    world.borrow_mut().terminals.push(crate::orca::Terminal { handle: Some("term_0".into()), worktree_id: Some(WORKTREE.into()), agent: true, pane: Some("tab0:leaf0".into()) });
    let kept = carried_with(&config, &linear, &records, &mut fake::Orca::on(&world), &mut github, &reap("c-reap", merged())).unwrap();
    assert_eq!(kept.detail["reaped"], json!(false));
    assert!(world.borrow().removed.is_empty());
    world.borrow_mut().terminals.clear();
    // Its host's listing, and `remove`'s deliberate re-read of it: no other host is asked.
    let mut reaped = None;
    let spawns = spawned(&world, || reaped = Some(carried_with(&config, &linear, &records, &mut fake::Orca::on(&world), &mut github, &reap("c-reap", merged())).unwrap()));
    assert_eq!(spawns, [fake::AT, fake::AT]);
    assert!(world.borrow().listed_at.iter().all(Option::is_none));
    assert_eq!(reaped.unwrap().detail["reaped"], json!(true));
    {
        let w = world.borrow();
        assert_eq!(w.removed, [WORKTREE]);
        // Orca deletes it, which the fake world doesn't model.
        assert_eq!(w.git["/src/tolkster"].branches, ["main", "tw-1"]);
    }
    assert!(records.workers.borrow().is_empty());
    // Sent again: gone by a complete listing, and by no other.
    let again = carried_with(&config, &linear, &records, &mut fake::Orca::on(&world), &mut github, &reap("c-reap", merged())).unwrap();
    assert_eq!(again.detail["reaped"], json!(true));
    // A listing cut off decides nothing, so the reap is retried, never reported done.
    world.borrow_mut().incomplete = true;
    let unsure = carried_with(&config, &linear, &records, &mut fake::Orca::on(&world), &mut github, &reap("c-reap", merged()));
    assert!(matches!(unsure, Err(Error::Later(_))));
    // Another issue's worktree is refused, and left as it is.
    let (linear, records, world) = fakes();
    let mut github = merged_worker(&world, &records);
    world.borrow_mut().worktrees[1].linked_issue = Some("TW-2".into());
    assert!(carried_with(&config, &linear, &records, &mut fake::Orca::on(&world), &mut github, &reap("c-other", merged())).is_err());
    assert!(world.borrow().removed.is_empty() && !records.workers.borrow().is_empty());
}

// A reap on a rule the machine decided reads no PR: GitHub answers nothing here. Only that
// rule's guards run. A completed issue's worktree goes once clean and pushed, and a cancelled
// one's dirty or not once Linear's cancel is a day old, unless the store holds no PR and its
// branch is on origin with work, as a missed PR's would be: then only clean and pushed. Either is kept once Linear says the
// issue is open again, and a merged PR's is kept at another commit than the PR's head.
#[test]
fn a_reap_runs_only_the_guards_of_the_rule_the_machine_named() {
    let config = fake::config(&briefs("reap"));
    let run = |rule: Rule, now: crate::sessions::IssueNow, prepare: &dyn Fn(&mut World)| {
        let (linear, records, world) = fakes();
        linear.tolksters.borrow_mut().insert("TW-1".into(), now);
        merged_worker(&world, &records);
        prepare(&mut world.borrow_mut());
        let mut nothing = FakeReader::new(|_, _| Err(Error::Failed("GitHub isn't there".into())));
        let finished = carried_with(&config, &linear, &records, &mut fake::Orca::on(&world), &mut nothing, &reap("c-reap", rule)).unwrap();
        assert_eq!(finished.outcome, Outcome::Ok);
        let removed = world.borrow().removed == [WORKTREE];
        assert_eq!(finished.detail["reaped"], json!(removed));
        removed
    };
    let completed = || Rule::Completed;
    let cancelled = || Rule::Cancelled { no_pr: true };
    let done = || crate::sessions::IssueNow::Completed(at(0));
    let cancelled_ago = |hours: i64| crate::sessions::IssueNow::Cancelled(crate::time::iso(crate::time::now_ms() - hours * 3_600_000));
    assert!(run(completed(), done(), &|_| {}));
    assert!(run(cancelled(), cancelled_ago(25), &|_| {}));
    // A cancel the queued reap didn't see: reopened and cancelled again since.
    assert!(!run(cancelled(), cancelled_ago(0), &|_| {}));
    // A cancelled checkout is taken dirty or unpushed while its branch holds no work on
    // origin, but kept when it does, as a PR the store missed would be.
    let dirty = |w: &mut World| w.git.get_mut("/wt/tw-1").unwrap().status = "?? notes.md".into();
    let unpushed = |w: &mut World| w.git.get_mut("/wt/tw-1").unwrap().unpushed = 1;
    let on_origin = |w: &mut World| w.git.get_mut("/wt/tw-1").unwrap().pushed_work = 2;
    assert!(run(cancelled(), cancelled_ago(25), &dirty));
    assert!(run(cancelled(), cancelled_ago(25), &unpushed));
    assert!(run(cancelled(), cancelled_ago(25), &on_origin));
    assert!(!run(cancelled(), cancelled_ago(25), &|w| { on_origin(w); dirty(w) }));
    assert!(!run(cancelled(), cancelled_ago(25), &|w| { on_origin(w); unpushed(w) }));
    assert!(!run(cancelled(), cancelled_ago(25), &|w| { on_origin(w); w.git.get_mut("/wt/tw-1").unwrap().branches.clear() }));
    // A PR the store holds, closed, is no guard: that work was cancelled.
    assert!(run(Rule::Cancelled { no_pr: false }, cancelled_ago(25), &|w| { on_origin(w); dirty(w) }));
    // Linear says it is open again.
    assert!(!run(completed(), crate::sessions::IssueNow::Open, &|_| {}));
    assert!(!run(cancelled(), crate::sessions::IssueNow::Open, &|_| {}));
    // The completed rule's checkout guards: uncommitted work, or commits on no remote.
    assert!(!run(completed(), done(), &|w| w.git.get_mut("/wt/tw-1").unwrap().status = "?? notes.md".into()));
    assert!(!run(completed(), done(), &|w| w.git.get_mut("/wt/tw-1").unwrap().unpushed = 1));
    // The merged rule's: the checkout at the PR's head, and clean.
    assert!(run(merged(), crate::sessions::IssueNow::Open, &|_| {}));
    assert!(!run(Rule::Merged { number: 7, head: BASE_SHA.into() }, crate::sessions::IssueNow::Open, &|_| {}));
    assert!(!run(merged(), crate::sessions::IssueNow::Open, &|w| w.git.get_mut("/wt/tw-1").unwrap().status = "?? notes.md".into()));
}

// A landing's reap reads no PR and names no commit: origin fetched first, the checkout must be
// clean, its HEAD on origin's default branch, and nothing on HEAD or the branch on no remote.
// Then the worktree goes with its branch, which Orca keeps, as no PR merged it.
#[test]
fn a_landed_reap_takes_the_worktree_and_its_branch_once_its_head_is_on_the_default_branch() {
    let config = fake::config(&briefs("reap"));
    let run = |rule: Rule, prepare: &dyn Fn(&mut World)| {
        let (linear, records, world) = fakes();
        linear.tolksters.borrow_mut().insert("TW-1".into(), crate::sessions::IssueNow::Completed(at(0)));
        merged_worker(&world, &records);
        world.borrow_mut().tip = HEAD_SHA.into();
        prepare(&mut world.borrow_mut());
        let mut nothing = FakeReader::new(|_, _| Err(Error::Failed("GitHub isn't there".into())));
        let finished = carried_with(&config, &linear, &records, &mut fake::Orca::on(&world), &mut nothing, &reap("c-reap", rule)).unwrap();
        assert_eq!(finished.outcome, Outcome::Ok);
        assert_eq!(finished.detail["reaped"], json!(world.borrow().removed == [WORKTREE]));
        world
    };
    let world = run(Rule::Landed, &|_| {});
    assert_eq!(world.borrow().removed, [WORKTREE]);
    assert_eq!(world.borrow().git["/src/tolkster"].branches, ["main"]);
    // Fetched before the check, whatever the remote-tracking refs say.
    let ran = &world.borrow().ran;
    let fetched = ran.iter().position(|c| c.ends_with("fetch --quiet origin")).expect("a fetch");
    assert!(fetched < ran.iter().position(|c| c.contains("merge-base")).unwrap(), "{ran:?}");
    for kept in [&(|w: &mut World| w.tip = BASE_SHA.into()) as &dyn Fn(&mut World), &|w| w.git.get_mut("/wt/tw-1").unwrap().status = "?? notes.md".into(), &|w| w.git.get_mut("/wt/tw-1").unwrap().unpushed = 1] {
        assert!(run(Rule::Landed, kept).borrow().removed.is_empty());
    }
}

// `merged_worker`'s worktree moved to the host `mac`, with that host's primary checkout of the
// repo, which holds the branch too, and the worker record moved with it. This machine reaches
// `mac`, and the issue is Tolkster's there as here.
const ON_MAC: &str = "mac@r9::/Users/x/tw-1";
const MAC_PRIMARY: &str = "mac@r9::/Users/x/tolkster";

fn on_mac(world: &Rc<RefCell<World>>, records: &fake::Records, linear: &fake::Linear) {
    let mut w = world.borrow_mut();
    let at = w.worktrees.iter().position(|w| w.id == WORKTREE).unwrap();
    w.worktrees[at] = Worktree { id: ON_MAC.into(), repo_id: "r9".into(), path: "/Users/x/tw-1".into(), is_local: false, ..w.worktrees[at].clone() };
    w.worktrees.push(Worktree { id: MAC_PRIMARY.into(), repo_id: "r9".into(), path: "/Users/x/tolkster".into(), branch: Some("main".into()), linked_issue: None, is_main: true, is_local: false, agents: Vec::new(), origin: Some("acme/tolkster".into()) });
    let checkout = w.git.remove("/wt/tw-1").unwrap();
    w.git.insert("/Users/x/tw-1".into(), checkout);
    let primary = w.git["/src/tolkster"].clone();
    w.git.insert("/Users/x/tolkster".into(), primary);
    w.hosts.push("mac".into());
    w.tolksters_there = linear.tolksters.borrow().clone();
    w.prs_there = vec![crate::github::BranchPr { number: 7, state: crate::github::PrState::Merged, head: HEAD_SHA.into() }];
    let mut workers = records.workers.borrow_mut();
    let record = workers.remove(WORKTREE).unwrap();
    workers.insert(ON_MAC.into(), crate::state::WorkerRecord { worktree_id: ON_MAC.into(), ..record });
}

// Two hosts: a reap runs its rule's guards on the worktree's own host, against that host's
// checkout, in a terminal in that host's primary checkout, and deletes the branch there, never
// in this machine's. A clean, merged worktree on either goes, with its record; a dirty or
// unpushed one on either stays, as does one on a host this machine can't reach.
#[test]
fn a_reap_runs_its_guards_on_the_worktrees_own_host() {
    let config = fake::config(&briefs("reap"));
    let run = |mac: bool, rule: Rule, prepare: &dyn Fn(&mut World, &str)| {
        let (linear, records, world) = fakes();
        linear.tolksters.borrow_mut().insert("TW-1".into(), crate::sessions::IssueNow::Completed(at(0)));
        let mut github = merged_worker(&world, &records);
        let (worktree, path) = if mac { (ON_MAC, "/Users/x/tw-1") } else { (WORKTREE, "/wt/tw-1") };
        if mac {
            on_mac(&world, &records, &linear);
        }
        prepare(&mut world.borrow_mut(), path);
        let reap = Order::ReapWorktree { worktree_id: worktree.into(), rule };
        let finished = carried_with(&config, &linear, &records, &mut fake::Orca::on(&world), &mut github, &Command { kind: Kind::ReapWorktree, ..order("c-reap", reap) });
        let removed = world.borrow().removed == [worktree];
        if let Ok(finished) = &finished {
            assert_eq!(finished.detail["reaped"], json!(removed));
        }
        assert_eq!(records.workers.borrow().is_empty(), removed);
        (finished.is_ok(), removed, world)
    };
    let completed = || Rule::Completed;
    // A completed issue's branch has no PR.
    let no_pr = |w: &mut World| w.prs_there.clear();
    let dirty = |w: &mut World, path: &str| w.git.get_mut(path).unwrap().status = "?? notes.md".into();
    let unpushed = |w: &mut World, path: &str| {
        no_pr(w);
        w.git.get_mut(path).unwrap().unpushed = 1;
    };
    for mac in [false, true] {
        assert!(run(mac, merged(), &|_, _| {}).1, "on mac: {mac}");
        assert!(run(mac, Rule::Landed, &|w, _| w.tip = HEAD_SHA.into()).1, "on mac: {mac}");
        assert!(!run(mac, Rule::Landed, &|w, _| w.tip = BASE_SHA.into()).1, "on mac: {mac}");
        assert!(run(mac, completed(), &|w, _| no_pr(w)).1, "on mac: {mac}");
        assert!(!run(mac, merged(), &dirty).1, "on mac: {mac}");
        assert!(!run(mac, completed(), &unpushed).1, "on mac: {mac}");
    }
    // The step takes no rule on trust from its line: GitHub's PRs and Linear, as mac reads
    // them, must bear it out, and a cancel be a day old.
    let fresh_cancel = |w: &mut World, _: &str| {
        w.tolksters_there.insert("TW-1".into(), crate::sessions::IssueNow::Cancelled(crate::time::iso(crate::time::now_ms())));
    };
    let unmerged = |w: &mut World, _: &str| w.prs_there[0].state = crate::github::PrState::Open;
    assert!(!run(true, merged(), &unmerged).1);
    // A landing's step needs the issue done in Linear and no PR open on its branch, as mac reads them.
    assert!(!run(true, Rule::Landed, &|w, p| { w.tip = HEAD_SHA.into(); unmerged(w, p) }).1);
    assert!(!run(true, Rule::Landed, &|w, _| { w.tip = HEAD_SHA.into(); w.tolksters_there.insert("TW-1".into(), crate::sessions::IssueNow::Open); }).1);
    assert!(!run(true, Rule::Merged { number: 7, head: BASE_SHA.into() }, &|w, _| w.git.get_mut("/Users/x/tw-1").unwrap().head = BASE_SHA.into()).1);
    assert!(!run(true, completed(), &|_, _| {}).1);
    assert!(!run(true, Rule::Cancelled { no_pr: false }, &fresh_cancel).1);
    let day_old = |w: &mut World, _: &str| {
        w.tolksters_there.insert("TW-1".into(), crate::sessions::IssueNow::Cancelled(crate::time::iso(crate::time::now_ms() - 25 * 3_600_000)));
    };
    assert!(run(true, Rule::Cancelled { no_pr: false }, &day_old).1);
    // On mac, the step runs in mac's primary checkout, and the branch goes from there alone.
    let (_, _, world) = run(true, Rule::Cancelled { no_pr: false }, &day_old);
    let w = world.borrow();
    assert_eq!(w.opened, [(MAC_PRIMARY.to_string(), None, "tolkster reap-here --issue TW-1 --branch tw-1 --cancelled; exit $LASTEXITCODE".to_string())]);
    assert_eq!(w.git["/Users/x/tolkster"].branches, ["main"]);
    assert_eq!(w.git["/src/tolkster"].branches, ["main", "tw-1"]);
    drop(w);
    // A host this machine can't reach: the reap fails, and the worktree stays.
    let (ok, removed, _) = run(true, merged(), &|w, _| w.hosts.clear());
    assert!(!ok && !removed);
    // One that is paired but silent leaves its worktree out of the listing: the reap is retried,
    // not reported done with the worktree still there.
    let (linear, records, world) = fakes();
    linear.tolksters.borrow_mut().insert("TW-1".into(), crate::sessions::IssueNow::Open);
    let mut github = merged_worker(&world, &records);
    on_mac(&world, &records, &linear);
    world.borrow_mut().unanswered.push("mac".into());
    let order = Command { kind: Kind::ReapWorktree, ..order("c-reap", Order::ReapWorktree { worktree_id: ON_MAC.into(), rule: merged() }) };
    let silent = carried_with(&config, &linear, &records, &mut fake::Orca::on(&world), &mut github, &order);
    assert!(matches!(silent, Err(Error::Waits(crate::error::Waits::Host(ref host), _)) if host == "mac"), "{silent:?}");
    assert!(world.borrow().removed.is_empty() && !records.workers.borrow().is_empty());
    // One silent by the time its step has run can't show the worktree gone: retried too, and
    // the record kept until a listing shows it.
    world.borrow_mut().unanswered.clear();
    world.borrow_mut().silent_after_step.push("mac".into());
    let unread = carried_with(&config, &linear, &records, &mut fake::Orca::on(&world), &mut github, &order);
    // The fake's read fails as Orca's would, not as a silence: it waits on nothing it names.
    assert!(matches!(unread, Err(Error::Later(_))), "{unread:?}");
    assert!(world.borrow().removed == [ON_MAC] && !records.workers.borrow().is_empty());
}

// `tolkster reap` by hand covers every host: its rule, from the PR it reads, goes to the
// worktree's own host, and an agent there is never closed from here.
#[test]
fn reap_by_hand_reaps_a_worktree_on_another_host() {
    let (linear, records, world) = fakes();
    linear.tolksters.borrow_mut().insert("TW-1".into(), crate::sessions::IssueNow::Open);
    merged_worker(&world, &records);
    on_mac(&world, &records, &linear);
    let reap_all = |dry_run: bool| {
        let worktrees = world.borrow().worktrees.clone();
        let pr = || vec![crate::github::BranchPr { number: 7, state: crate::github::PrState::Merged, head: HEAD_SHA.into() }];
        let tolksters = linear.tolksters.borrow().clone();
        crate::reap::reap(&mut fake::Orca::on(&world), &mut |_, _| Ok(pr()), &mut |id| crate::state::Records::forget_worker(&*records, id), &worktrees, |_| Ok(tolksters.clone()), dry_run).unwrap()
    };
    // An idle agent on mac keeps it.
    world.borrow_mut().terminals.push(crate::orca::Terminal { handle: Some("mac@term_0".into()), worktree_id: Some(ON_MAC.into()), agent: true, pane: Some("tab0:leaf0".into()) });
    assert!(reap_all(false).lines.is_empty());
    world.borrow_mut().terminals.clear();
    assert_eq!(reap_all(true).lines.iter().map(ToString::to_string).collect::<Vec<_>>(), ["TW-1: would ask mac to check /Users/x/tw-1, then remove it and branch tw-1 (PR #7 merged)"]);
    assert!(world.borrow().removed.is_empty());
    let reaped = reap_all(false);
    assert_eq!(reaped.lines.iter().map(ToString::to_string).collect::<Vec<_>>(), ["TW-1: removed /Users/x/tw-1 and branch tw-1 on mac (PR #7 merged)"]);
    assert!(reaped.removed && world.borrow().removed == [ON_MAC] && records.workers.borrow().is_empty());
}

// A worktree found already gone, as one another host's step removed after an earlier reap
// stopped waiting on it, has its worker record forgotten.
#[test]
fn a_reap_that_finds_the_worktree_gone_forgets_its_record() {
    let config = fake::config(&briefs("reap"));
    let (linear, records, world) = fakes();
    let mut github = merged_worker(&world, &records);
    world.borrow_mut().worktrees.retain(|w| w.id != WORKTREE);
    let finished = carried_with(&config, &linear, &records, &mut fake::Orca::on(&world), &mut github, &reap("c-reap", merged())).unwrap();
    assert_eq!(finished.detail["reaped"], json!(true));
    assert!(records.workers.borrow().is_empty());
}
