use super::*;
use crate::error::Waits;
use crate::hosts::tests::opted_in;
use crate::trace::Declined;
use crate::state::WorkerRecord;

const ON_MAC: &str = "mac@hosted::/wt/tw-1";

fn record(issue: &str, worktree_id: &str) -> WorkerRecord {
    WorkerRecord { issue: issue.into(), worktree_id: worktree_id.into(), session_id: "s0".into(), at: at(0), launch: crate::config::ladder::Launch::new(crate::harnesses::DEFAULT, "claude-opus-5-5", "medium"), rung_from: crate::state::RungFrom::Claim }
}

// This machine full, with TW-9's worker running here, and `mac` paired.
fn full_here(world: &Rc<RefCell<World>>, records: &fake::Records) -> Config {
    let mut w = world.borrow_mut();
    let agents = vec![Pane { key: "t:l".into(), state: "working".into(), since_ms: None }];
    w.worktrees.push(Worktree { id: "r1::/wt/tw-9".into(), repo_id: "r1".into(), path: "/wt/tw-9".into(), branch: None, linked_issue: Some("TW-9".into()), is_main: false, is_local: true, agents, origin: Some("acme/tolkster".into()) });
    w.hosts = vec!["mac".into()];
    records.workers.borrow_mut().insert("r1::/wt/tw-9".into(), record("TW-9", "r1::/wt/tw-9"));
    Config { slots: [("local".into(), 1), ("mac".into(), 6)].into(), enrolled: vec!["mac".into()], ..fake::config(&briefs("hosts")) }
}

fn start_on(host: &str) -> Command {
    order("c-start", Order::StartWorker { session_id: "s1".into(), orchestrate: false, worktree_id: None, host: Some(host.into()) })
}

// A new worker goes where the Durable Object granted its slot: a host, with the repo ready there
// by the repo's project, or this machine, however many `listen` counts here.
#[test]
fn a_start_goes_to_the_host_its_slot_was_granted_on() {
    let (linear, records, world) = fakes();
    let config = full_here(&world, &records);
    world.borrow_mut().ready_on = vec!["mac".into()];
    let started = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start_on("mac")).unwrap();
    assert_eq!((started.outcome, &started.detail["worktree"]["id"]), (Outcome::Ok, &json!(ON_MAC)));
    assert_eq!(world.borrow().created[0].0, "github:acme/tolkster");
    let (linear, records, world) = fakes();
    let config = full_here(&world, &records);
    let started = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start()).unwrap();
    assert_eq!((started.outcome, &started.detail["worktree"]["id"]), (Outcome::Ok, &json!("r1::/wt/tw-1")));
}

// A host granted that has since been revoked, by `host rm` or a sleep, read afresh, that doesn't
// answer, or that hasn't the repo ready turns the start back, sent again later: `listen` picks
// no other, and nothing is made, moved or said first.
#[test]
fn a_host_granted_that_can_t_take_the_worker_turns_the_start_back_unchanged() {
    for why in ["revoked", "silent", "unready"] {
        let (linear, records, world) = fakes();
        let config = full_here(&world, &records);
        {
            let mut w = world.borrow_mut();
            w.hosts.push("pi".into());
            w.ready_on = if why == "unready" { vec!["pi".into()] } else { vec!["mac".into(), "pi".into()] };
            if why == "silent" {
                w.unanswered = vec!["mac".into()];
            }
        }
        if why == "revoked" {
            records.revoked.borrow_mut().push("mac".into());
        }
        let started = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start_on("mac"));
        assert_eq!(Finished::of(&started.unwrap_err()).outcome, Outcome::Retryable, "{why}");
        assert!(world.borrow().created.is_empty() && linear.started.borrow().is_empty(), "{why}");
        assert!(["thought", "action", "response", "elicitation"].iter().all(|kind| linear.posted(kind).is_empty()), "{why}");
    }
}

// A start granted on a host marks it acted on, so a sleep's check finds it busy; one granted on a
// host a sleep has checked waits for it, making nothing.
#[test]
fn a_start_granted_on_a_host_orders_against_its_sleep() {
    let (linear, records, world) = fakes();
    let shared = Arc::clone(&world.borrow().shared);
    let config = Config { slots: [("local".into(), 1), ("mbx".into(), 2)].into(), enrolled: vec!["mbx".into()], ..fake::config(&briefs("hosts-acted")) };
    {
        let mut w = world.borrow_mut();
        w.hosts = vec!["mbx".into()];
        w.ready_on = vec!["mbx".into()];
    }
    shared.pacing.nap("mbx");
    let waits = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start_on("mbx")).unwrap();
    shared.pacing.placing_again("mbx");
    assert_eq!((waits.outcome, &waits.detail["waking"], &waits.detail["waitsOn"]), (Outcome::Retryable, &json!(true), &json!({ "lock": true })));
    assert!(world.borrow().created.is_empty());
    assert_eq!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start_on("mbx")).unwrap().outcome, Outcome::Ok);
    assert!(shared.pacing.acted_lately("mbx", crate::time::now_ms()));
}

// A start on a host that never began is the repo's by its origin, as the host's repo has an
// ID of its own: it is launched again there, making no worktree.
#[test]
fn a_start_that_never_began_on_a_host_is_launched_again_there() {
    let (linear, records, world) = fakes();
    let config = full_here(&world, &records);
    world.borrow_mut().ready_on = vec!["mac".into()];
    carried(&config, &linear, &records, &mut fake::Orca::on(&world), &start_on("mac")).unwrap();
    fake::Orca::on(&world).close_all(ON_MAC).unwrap();
    let named = order("c-start-2", Order::StartWorker { session_id: "s1".into(), orchestrate: false, worktree_id: Some(ON_MAC.into()), host: None });
    // Its origin not read is no answer either way: it is tried again.
    let origin = world.borrow_mut().worktrees.iter_mut().find(|w| w.id == ON_MAC).unwrap().origin.take();
    let unread = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &named);
    assert_eq!(Finished::of(&unread.unwrap_err()).outcome, Outcome::Retryable);
    world.borrow_mut().worktrees.iter_mut().find(|w| w.id == ON_MAC).unwrap().origin = origin;
    let again = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &named).unwrap();
    assert_eq!((&again.detail["madeWorktree"], &again.detail["launched"]), (&json!(false), &json!(true)));
    assert_eq!((world.borrow().created.len(), world.borrow().launched.len()), (1, 2));
}

// A host that doesn't answer, or is no longer paired, may hold the issue's worktree: nothing
// is made or started again in its place, though the start names none, as one resent after its
// result was lost doesn't, and a stop or a resume decides nothing.
#[test]
fn a_worktree_on_a_host_not_heard_is_never_taken_as_gone() {
    for paired in [true, false] {
        let (linear, records, world) = fakes();
        let config = fake::config(&briefs("hosts-silent"));
        {
            let mut w = world.borrow_mut();
            w.hosts = if paired { vec!["mac".into()] } else { Vec::new() };
            w.unanswered = w.hosts.clone();
        }
        records.workers.borrow_mut().insert(ON_MAC.into(), record("TW-1", ON_MAC));
        let later = |command: &Command| Finished::of(&carried(&config, &linear, &records, &mut fake::Orca::on(&world), command).unwrap_err()).outcome;
        let named = order("c-start", Order::StartWorker { session_id: "s1".into(), orchestrate: false, worktree_id: Some(ON_MAC.into()), host: None });
        assert_eq!(later(&start()), Outcome::Retryable, "paired: {paired}");
        assert_eq!(later(&named), Outcome::Retryable, "paired: {paired}");
        assert_eq!(later(&stop("c-stop", Some(ON_MAC))), Outcome::Retryable, "paired: {paired}");
        assert_eq!(later(&order("c-resume", Order::ResumeWorker { session_id: "s1".into(), why: Resume::Message })), Outcome::Retryable, "paired: {paired}");
        assert!(world.borrow().created.is_empty() && world.borrow().launched.is_empty());
    }
}

// A command for a worker on an asleep recipe machine, by the issue's record or the worktree it
// names, waits on its host without spending a send, which the fleet wakes, as does one on a
// machine a sleep has checked; one on a machine awake but silent waits on nothing. None wakes it.
#[test]
fn a_worker_s_command_on_an_asleep_machine_waits_for_it() {
    let (linear, records, world) = fakes();
    let shared = Arc::clone(&world.borrow().shared);
    const ON_MINI: &str = "mini@hosted::/wt/tw-1";
    let mut config = fake::config(&briefs("hosts-asleep"));
    {
        let mut w = world.borrow_mut();
        w.hosts = vec!["mini".into()];
        w.unanswered = w.hosts.clone();
    }
    records.workers.borrow_mut().insert(ON_MINI.into(), record("TW-1", ON_MINI));
    let made = |asleep: bool| crate::config::Machine { recipe: "cloud".into(), repo: "Tolkster".into(), made: true, sleeps: true, asleep, environment: asleep.then(|| "mini".into()), revision: None };
    config.machines.insert("mini".into(), made(true));
    let deliver = order("c-deliver", Order::DeliverMessage { worktree_id: ON_MINI.into(), session_id: None, comment: true });
    let resume = order("c-resume", Order::ResumeWorker { session_id: "s1".into(), why: Resume::Message });
    let reap = order("c-reap", Order::ReapWorktree { worktree_id: ON_MINI.into(), rule: crate::machine::ReapRule::Cancelled { no_pr: true } });
    for command in [&deliver, &resume, &stop("c-stop", Some(ON_MINI)), &start(), &reap] {
        let waits = carried(&config, &linear, &records, &mut fake::Orca::on(&world), command).unwrap();
        assert_eq!((waits.outcome, &waits.detail["waking"], &waits.detail["waitsOn"]), (Outcome::Retryable, &json!(true), &json!({ "host": "mini" })), "{}", command.id);
    }
    assert!(world.borrow().woken.is_empty());
    // A command naming a worktree acts on its host alone, whatever the issue's records say.
    let here = order("c-reap-here", Order::ReapWorktree { worktree_id: "r1::/wt/tw-1".into(), rule: crate::machine::ReapRule::Cancelled { no_pr: true } });
    let reaped = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &here);
    assert!(reaped.is_err() || reaped.unwrap().detail.get("waking").is_none());
    assert!(world.borrow().woken.is_empty());
    config.machines.insert("mini".into(), made(false));
    assert!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &resume).is_err());
    shared.pacing.nap("mini");
    let waits = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &resume).unwrap();
    shared.pacing.placing_again("mini");
    assert_eq!(waits.detail["waking"], json!(true));
    assert!(world.borrow().woken.is_empty());
}

// A suspend puts to sleep only a recipe machine that answers, enrolled and awake, with no agent
// working on it, as `listen` reads it with placement stopped: a worker being set up there
// keeps it, and one at rest doesn't.
#[test]
fn a_suspend_puts_to_sleep_only_a_resting_enrolled_recipe_machine() {
    const ON_MBP: &str = "mbp@hosted::/wt/tw-1";
    let (linear, records, world) = fakes();
    let shared = Arc::clone(&world.borrow().shared);
    let mut config = full_here(&world, &records);
    world.borrow_mut().hosts = vec!["mbp".into()];
    let suspend = order("naps.1", Order::SuspendHost { host: "mbp".into() });
    let asleep = |config: &Config| carried(config, &linear, &records, &mut fake::Orca::on(&world), &suspend).unwrap().detail["asleep"].clone();
    assert_eq!(asleep(&config), json!(false));
    let made = |asleep| crate::config::Machine { recipe: "cloud".into(), repo: "Tolkster".into(), made: true, sleeps: true, asleep, environment: None, revision: None };
    config.machines.insert("mbp".into(), made(true));
    assert_eq!(asleep(&config), json!(false));
    config.machines.insert("mbp".into(), made(false));
    let setting_up = WorkerRecord { at: crate::time::iso(crate::time::now_ms()), ..record("TW-1", ON_MBP) };
    records.workers.borrow_mut().insert(ON_MBP.into(), setting_up);
    assert_eq!(asleep(&config), json!(false));
    records.workers.borrow_mut().insert(ON_MBP.into(), record("TW-1", ON_MBP));
    // Revoked since the listing, as by `host rm`: left to it.
    records.revoked.borrow_mut().push("mbp".into());
    assert_eq!(asleep(&config), json!(false));
    records.revoked.borrow_mut().clear();
    assert_eq!(asleep(&config), json!(true));
    assert_eq!(world.borrow().asleep, ["mbp"]);
    // A worker's command keeps its machine awake while it acts, and for a while after.
    let host = ["mbp-acting".to_string()];
    let acting = shared.pacing.act_on(&host);
    assert!(shared.pacing.acted_lately(&host[0], crate::time::now_ms() + 6 * 60 * 1000));
    drop(acting);
    assert!(shared.pacing.acted_lately(&host[0], crate::time::now_ms()));
    assert!(!shared.pacing.acted_lately(&host[0], crate::time::now_ms() + 6 * 60 * 1000));
}

// A destroy goes ahead only for a machine `listen` reads as dormant now, with no make, wake or
// sleep running, and one awake that sleeps only while unadmitted afresh; it destroys the machine
// only once nothing is left on it, a worker being set up there included.
#[test]
fn a_destroy_takes_only_a_dormant_machine_that_holds_nothing() {
    const ON_OLD: &str = "old@hosted::/wt/tw-1";
    let (linear, records, world) = fakes();
    let mut config = fake::config(&briefs("hosts-destroy"));
    world.borrow_mut().hosts = vec!["old".into()];
    let destroy = order("naps.2", Order::DestroyHost { name: "old".into() });
    let destroyed = |config: &Config| carried(config, &linear, &records, &mut fake::Orca::on(&world), &destroy).map(|done| done.detail["destroyed"].clone());
    let made = |sleeps, asleep| crate::config::Machine { recipe: "cloud".into(), repo: "Tolkster".into(), made: true, sleeps, asleep, environment: None, revision: None };
    // No such machine, or one awake and admitted that sleeps: not dormant.
    assert_eq!(destroyed(&config).unwrap(), json!(false));
    config.machines.insert("old".into(), made(true, false));
    config.enrolled.push("old".into());
    assert_eq!(destroyed(&config).unwrap(), json!(false));
    // Unadmitted by the listing, but admitted afresh.
    config.enrolled.clear();
    assert_eq!(destroyed(&config).unwrap(), json!(false));
    config.machines.insert("old".into(), made(true, true));
    world.borrow_mut().unanswered = vec!["old".into()];
    world.borrow_mut().making = true;
    assert!(destroyed(&config).is_err());
    world.borrow_mut().making = false;
    world.borrow_mut().unanswered = vec!["old".into()];
    let setting_up = WorkerRecord { at: crate::time::iso(crate::time::now_ms()), ..record("TW-1", ON_OLD) };
    records.workers.borrow_mut().insert(ON_OLD.into(), setting_up);
    assert_eq!(destroyed(&config).unwrap(), json!(false));
    records.workers.borrow_mut().insert(ON_OLD.into(), record("TW-1", ON_OLD));
    world.borrow_mut().unanswered = vec!["old".into()];
    assert_eq!(destroyed(&config).unwrap(), json!(true));
    assert_eq!(world.borrow().destroyed, ["old"]);
}

// An awake machine a start acts on, granted there, is kept by a destroy: the start may not have
// written its record yet. One nothing acts on goes.
#[test]
fn a_destroy_keeps_a_machine_a_start_acts_on() {
    let (linear, records, world) = fakes();
    let shared = Arc::clone(&world.borrow().shared);
    let mut config = fake::config(&briefs("hosts-destroy-acting"));
    world.borrow_mut().hosts = vec!["dx-1".into(), "dx-2".into()];
    for name in ["dx-1", "dx-2"] {
        config.machines.insert(name.into(), crate::config::Machine { recipe: "cloud".into(), repo: "Tolkster".into(), made: true, sleeps: false, asleep: false, environment: None, revision: None });
    }
    let destroyed = |name: &str| carried(&config, &linear, &records, &mut fake::Orca::on(&world), &order("naps.3", Order::DestroyHost { name: name.into() })).unwrap().detail["destroyed"].clone();
    let acting = shared.pacing.act_on(&["dx-1".to_string()]);
    assert_eq!(destroyed("dx-1"), json!(false));
    drop(acting);
    assert_eq!(destroyed("dx-2"), json!(true));
    assert_eq!(world.borrow().destroyed, ["dx-2"]);
}

// `makeHost` begins a machine for a repo whose recipe opts in, none for another; one refused
// while another make, wake or sleep runs waits on the lock, and one in a cool-down isn't begun.
#[test]
fn a_make_begins_only_for_a_repo_with_a_recipe_and_waits_on_the_lock() {
    let (linear, records, world) = fakes();
    let config = Config { repos: opted_in(2), ..full_here(&world, &records) };
    let make = |repo: &str| order("fleet.1", Order::MakeHost { repo: repo.into() });
    let made = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &make("tolkster")).unwrap();
    let contract = crate::state::tests::contract();
    assert_eq!((made.outcome, json!(made.detail)), (Outcome::Ok, contract["fleetBegan"].clone()));
    assert_eq!(world.borrow().made_for, ["Tolkster"]);
    let other = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &make("other")).unwrap();
    assert_eq!(other.detail["began"], json!(false));
    world.borrow_mut().declined = Some(Some(Declined::Running));
    assert!(matches!(carried(&config, &linear, &records, &mut fake::Orca::on(&world), &make("tolkster")), Err(Error::Waits(Waits::Lock, _))));
    world.borrow_mut().declined = Some(Some(Declined::CoolDown));
    let cooling = carried(&config, &linear, &records, &mut fake::Orca::on(&world), &make("tolkster")).unwrap();
    assert_eq!(json!(cooling.detail), contract["fleetCoolDown"]);
    assert_eq!(world.borrow().made_for, ["Tolkster"]);
}

// `wakeHost` wakes a machine that sleeps, asleep or marked awake while Orca doesn't hear it,
// only while its credential is admitted when asked: one a destroy revoked would wake to cap 0.
#[test]
fn a_wake_wakes_an_admitted_machine_asleep_or_unheard() {
    let (linear, records, world) = fakes();
    let mut config = full_here(&world, &records);
    let made = |asleep: bool| crate::config::Machine { recipe: "cloud".into(), repo: "Tolkster".into(), made: true, sleeps: true, asleep, environment: asleep.then(|| "mini".into()), revision: None };
    {
        let mut w = world.borrow_mut();
        w.hosts.push("mini".into());
        w.unanswered = vec!["mini".into()];
    }
    let wake = order("fleet.2", Order::WakeHost { name: "mini".into() });
    let began = |config: &Config| carried(config, &linear, &records, &mut fake::Orca::on(&world), &wake).unwrap().detail["began"].clone();
    config.machines.insert("mini".into(), made(true));
    records.revoked.borrow_mut().push("mini".into());
    assert_eq!(began(&config), json!(false));
    records.revoked.borrow_mut().clear();
    assert_eq!(began(&config), json!(true));
    config.machines.insert("mini".into(), made(false));
    assert_eq!(began(&config), json!(true));
    assert_eq!(world.borrow().woken, ["mini", "mini"]);
    config.machines.remove("mini");
    assert_eq!(began(&config), json!(false));
}

// An update goes only to a machine that sleeps, asleep as machines.json has it, and waits while
// a make, wake or sleep runs, so it never restarts a machine an agent could be at work on.
#[test]
fn an_update_goes_only_to_an_asleep_machine() {
    let (linear, records, world) = fakes();
    let mut config = full_here(&world, &records);
    let made = |asleep: bool| crate::config::Machine { recipe: "cloud".into(), repo: "Tolkster".into(), made: true, sleeps: true, asleep, environment: asleep.then(|| "mini".into()), revision: Some("4".into()) };
    let update = order("fleet.4", Order::UpdateHost { name: "mini".into() });
    let updated = |config: &Config| carried(config, &linear, &records, &mut fake::Orca::on(&world), &update).map(|done| done.detail["updated"].clone());
    config.machines.insert("mini".into(), made(false));
    assert_eq!(updated(&config).unwrap(), json!(false));
    config.machines.insert("mini".into(), made(true));
    world.borrow_mut().making = true;
    assert!(matches!(updated(&config), Err(Error::Waits(Waits::Lock, _))));
    world.borrow_mut().making = false;
    assert_eq!(updated(&config).unwrap(), json!(true));
    assert_eq!(world.borrow().updated, ["mini"]);
}
