use std::collections::BTreeMap;

use super::*;
use crate::config::Machine;
use crate::config::ladder::Launch;
use crate::orca::Pane;
use crate::ports::fake;
use crate::config::repo_map::{Entry, Land, RecipeMachines, RepoMap};
use crate::state::RungFrom;
use crate::time::iso;

const NOW: i64 = 1_800_000_000_000;

// Worker `n`'s worktree on `host` (None: this machine), its agent running or not, with its
// record from `ago_ms` before NOW.
fn worker(host: Option<&str>, n: u32, agent: bool, ago_ms: i64) -> (Worktree, WorkerRecord) {
    let id = format!("{}r1::/wt/tw-{n}", host.map(|h| format!("{h}@")).unwrap_or_default());
    let agents = if agent { vec![Pane { key: "t:l".into(), state: "working".into(), since_ms: None }] } else { Vec::new() };
    let worktree = Worktree { id: id.clone(), repo_id: "r1".into(), path: format!("/wt/tw-{n}"), branch: None, linked_issue: Some(format!("TW-{n}")), is_main: false, is_local: host.is_none(), agents, origin: None };
    let record = WorkerRecord { issue: format!("TW-{n}"), worktree_id: id, session_id: format!("s-{n}"), at: iso(NOW - ago_ms), launch: Launch::new(crate::harnesses::DEFAULT, "claude-opus-5-5", "medium"), rung_from: RungFrom::Claim };
    (worktree, record)
}

fn running_on(host: Option<&str>, n: u32) -> (Worktree, WorkerRecord) {
    worker(host, n, true, 60 * 60 * 1000)
}

fn config(local: i64, caps: &[(&str, i64)]) -> Config {
    let mut config = fake::config("/briefs");
    config.slots = caps.iter().map(|(name, cap)| (name.to_string(), *cap)).chain([(crate::config::LOCAL.to_string(), local)]).collect();
    // By ID: the fake Orca's are the hosts' names, these tests' own `id-<name>`.
    config.enrolled = ["mac", "pi", "linux"].iter().flat_map(|name| [name.to_string(), format!("id-{name}")]).collect();
    config
}

fn machine(sleeps: bool, asleep: bool) -> Machine {
    Machine { recipe: "cloud".into(), repo: "Tolkster".into(), made: true, sleeps, asleep, environment: None, revision: None }
}

#[test]
fn a_host_s_cap_is_its_slots_entry_and_none_without_one() {
    let config = config(6, &[("mac", 2), ("pi", 0)]);
    let host = |name: &str| Host { id: format!("id-{name}"), name: name.into() };
    assert_eq!([cap(&config, &host("mac")), cap(&config, &host("pi")), cap(&config, &host("linux"))], [2, 0, 0]);
}

// A recipe machine's cap is its repo's `slotsEach`, by its machines.json record, whatever
// `slots` says: 0 while it is asleep, or once the repo no longer opts in.
#[test]
fn a_recipe_machine_s_cap_is_its_repo_s_slots_each() {
    let mut config = Config { repos: opted_in(2), ..config(6, &[("mac", 5)]) };
    config.machines.insert("mac".into(), machine(true, false));
    let mac = Host { id: "id-mac".into(), name: "mac".into() };
    assert_eq!(cap(&config, &mac), 3);
    config.machines.insert("mac".into(), machine(true, true));
    assert_eq!(cap(&config, &mac), 0);
    config.machines.insert("mac".into(), machine(true, false));
    config.repos = RepoMap::default();
    assert_eq!(cap(&config, &mac), 0);
}

// The admitted credential, by ID, never the name: a host whose ID holds none takes nothing,
// though another's name matches.
#[test]
fn a_host_without_an_admitted_credential_has_no_cap() {
    let mut config = config(6, &[("mac", 2)]);
    config.enrolled = vec!["mac".into()];
    assert_eq!(cap(&config, &Host { id: "id-mac".into(), name: "mac".into() }), 0);
    assert_eq!(cap(&config, &Host { id: "mac".into(), name: "mac".into() }), 2);
}

// A read of the enrolled hosts that fails keeps the last one read. A host admitted here, as a
// machine `listen` just made, takes the next start's worker before a listing reads it.
#[test]
fn a_failed_read_keeps_the_hosts_last_enrolled() {
    let pacing = Pacing::default();
    pacing.read_enrolled(Ok(vec!["e1".into()]));
    pacing.read_enrolled(Err(Error::Failed("down".into())));
    assert_eq!(pacing.enrolled(), ["e1"]);
    pacing.read_enrolled(Ok(Vec::new()));
    assert!(pacing.enrolled().is_empty());
    pacing.admitted("new");
    pacing.admitted("new");
    assert_eq!(pacing.enrolled(), ["new"]);
    let config = Config { enrolled: pacing.enrolled(), ..config(1, &[("new", 1)]) };
    assert_eq!(cap(&config, &Host { id: "new".into(), name: "new".into() }), 1);
    // The first read retries, then stops rather than leave the list empty; once it answers, the
    // hosts are there for the first command, before any listing.
    let starting = Pacing::default();
    let mut answers = vec![Err(Error::Failed("down".into())), Err(Error::Failed("down".into()))];
    let pauses = std::cell::Cell::new(0);
    assert!(starting.start_enrolled(|| answers.pop().unwrap_or_else(|| Ok(vec!["new".into()])), 2, || pauses.set(pauses.get() + 1)).is_err());
    assert!(starting.enrolled().is_empty());
    assert!(starting.start_enrolled(|| answers.pop().unwrap_or_else(|| Ok(vec!["new".into()])), 2, || pauses.set(pauses.get() + 1)).is_ok());
    assert_eq!(pauses.get(), 1);
    let started = Config { enrolled: starting.enrolled(), ..config };
    assert_eq!(cap(&started, &Host { id: "new".into(), name: "new".into() }), 1);
}

// The slots are each host's cap, or the workers it holds when more: this machine's, those of the
// hosts that answered with a repo set up, and for one that didn't, the workers it held, whose
// slots stay held. A host over its cap uses up only its own. At most MAX_SLOTS.
#[test]
fn the_slots_are_each_host_s_cap_or_the_workers_it_holds() {
    let config = config(6, &[("mac", 2), ("pi", 0), ("linux", 3)]);
    let host = |name: &str| Host { id: format!("id-{name}"), name: name.into() };
    let ready = |held: [i64; 3]| [(host("mac"), held[0]), (host("pi"), held[1]), (host("linux"), held[2])];
    assert_eq!(total(&config, 0, &ready([0, 0, 0]), &[]), 11);
    assert_eq!(total(&config, 0, &[], &[]), 6);
    assert_eq!(total(&config, 0, &[], &[(host("linux"), 4), (host("mac"), 1)]), 11);
    assert_eq!(total(&self::config(99, &[("linux", 3)]), 0, &[(host("linux"), 0)], &[]), MAX_SLOTS);
    // A host under its cap adds its cap, one over it the workers it holds, this machine included.
    assert_eq!(total(&config, 0, &ready([1, 0, 2]), &[]), 11);
    assert_eq!(total(&config, 0, &ready([4, 0, 0]), &[]), 13);
    assert_eq!(total(&config, 8, &ready([0, 0, 0]), &[]), 13);
}

// The coordinator holds a grant per worker, so one over its cap, or on a host that takes no work,
// must not eat the slots of the rest: this machine's free slot stays free.
#[test]
fn a_host_over_its_cap_leaves_the_others_their_slots() {
    let mut config = config(1, &[("agent37", 0), ("mac", 2)]);
    let host = |name: &str| Host { id: format!("id-{name}"), name: name.into() };
    // Holding 4 with cap 0, silent or with no repo set up: this machine's 1 plus the 4 held.
    assert_eq!(total(&config, 0, &[], &[(host("agent37"), 4)]), 5);
    // Ready with cap 2 running 4: 1 + 4.
    assert_eq!(total(&config, 0, &[(host("mac"), 4)], &[]), 5);
    // Not admitted, asleep: cap 0, and the workers it holds still count.
    config.enrolled.retain(|id| id != "id-mac");
    assert_eq!(total(&config, 0, &[], &[(host("mac"), 3)]), 4);
    config.enrolled.push("id-mac".into());
    config.machines.insert("mac".into(), machine(true, true));
    assert_eq!(total(&config, 0, &[], &[(host("mac"), 1)]), 2);
    let holding = [(host("mac"), 1)];
    let shares = shares(&config, 0, &[], &holding);
    assert_eq!(shares.iter().map(|(h, share)| (h.map(|h| h.name.as_str()), *share)).collect::<Vec<_>>(), [(None, 1), (Some("mac"), 1)]);
}

// A case's workers here, its hosts that answered with their ready projects and workers, and
// its silent hosts with theirs.
type Answered = Vec<(Host, Vec<String>, i64)>;
type Case = (i64, Answered, Vec<(Host, i64)>);

// The Durable Object derives its slots from a listing's fleet and the config report's cap
// (src/worker/machine/fleet.ts, `slotTotal`): test/contract.json's `fleet` holds each case as
// `fleet` writes it with what `total` gives, which test/fleet.test.ts derives the same. This
// machine alone over its cap, a paired host, an asleep machine still holding a worker, and a
// host over its cap beside one with no repo ready.
#[test]
fn the_contract_fleet_cases_sum_as_total_does() {
    let pacing = Pacing::default();
    let mut config = config(2, &[("mac", 2), ("pi", 3)]);
    config.machines.insert("tolkster-1".into(), Machine { environment: Some("id-tolkster-1".into()), ..machine(true, true) });
    config.enrolled.push("id-tolkster-1".into());
    let host = |name: &str| Host { id: format!("id-{name}"), name: name.into() };
    let ready = || vec![crate::ports::fake::FAKE_PROJECT.to_string()];
    let cases: [Case; 4] = [
        (3, Vec::new(), Vec::new()),
        (0, vec![(host("mac"), ready(), 1)], Vec::new()),
        (1, Vec::new(), vec![(host("tolkster-1"), 1)]),
        (0, vec![(host("mac"), ready(), 4), (host("pi"), Vec::new(), 1)], Vec::new()),
    ];
    let fixture = crate::state::tests::contract()["fleet"].clone();
    for (i, (local_held, answering, silent)) in cases.into_iter().enumerate() {
        let fleet = fleet(&config, &pacing, &BTreeMap::new(), &BTreeMap::new(), local_held, &answering, &silent);
        let (ready, holding): (Vec<_>, Vec<_>) = answering.into_iter().partition(|(h, ready, _)| cap(&config, h) > 0 && !ready.is_empty());
        let ready: Vec<(Host, i64)> = ready.into_iter().map(|(h, _, held)| (h, held)).collect();
        let holding: Vec<(Host, i64)> = holding.into_iter().map(|(h, _, held)| (h, held)).chain(silent).collect();
        let case = serde_json::json!({ "cap": config.local_slots(), "fleet": fleet, "total": total(&config, local_held, &ready, &holding) });
        assert_eq!(case, fixture[i], "case {i}");
    }
    assert_eq!(fixture.as_array().map(Vec::len), Some(4));
}

// A host a sleep or destroy has stopped work on is told to the Durable Object at cap 0.
#[test]
fn a_napping_host_is_listed_with_no_cap() {
    let pacing = Pacing::default();
    let mut config = config(2, &[("nap-cap", 3)]);
    config.enrolled.push("nap-cap".into());
    let host = Host { id: "nap-cap".into(), name: "nap-cap".into() };
    pacing.nap("nap-cap");
    let napped = fleet(&config, &pacing, &BTreeMap::new(), &BTreeMap::new(), 0, &[(host.clone(), Vec::new(), 0)], &[]).hosts[0].cap;
    pacing.placing_again("nap-cap");
    assert_eq!((napped, fleet(&config, &pacing, &BTreeMap::new(), &BTreeMap::new(), 0, &[(host, Vec::new(), 0)], &[]).hosts[0].cap), (0, 3));
}

// A host that answers and one that doesn't are each listed with their cap and the workers held
// on them: the silent one's slots stay held, and a worker on no paired host shows nowhere.
#[test]
fn the_report_lists_each_host_with_its_cap_and_workers() {
    let mut config = config(6, &[("pi", 0), ("mac", 2)]);
    config.machines.insert("mac".into(), machine(true, false));
    config.repos = opted_in(2);
    let host = |name: &str| Host { id: format!("id-{name}"), name: name.into() };
    let records: Vec<WorkerRecord> = [running_on(None, 1), running_on(Some("id-mac"), 2), running_on(Some("id-pi"), 3), running_on(Some("id-gone"), 4)].into_iter().map(|(_, record)| record).collect();
    let row = |name: Option<&str>, answers, cap, workers: &[&str], recipe: Option<&str>| Row { name: name.map(String::from), answers, cap, workers: workers.iter().map(|w| w.to_string()).collect(), recipe: recipe.map(String::from) };
    let want = [row(None, true, 6, &["TW-1"], None), row(Some("mac"), true, 3, &["TW-2"], Some("cloud")), row(Some("pi"), false, 0, &["TW-3"], None)];
    assert_eq!(report(&config, &[host("mac")], &[host("pi")], &records), want);
}

// Tolkster's repos entry opting in to `machines` recipe machines of 3 slots each.
pub fn opted_in(machines: i64) -> RepoMap {
    RepoMap(vec![Entry { repo: "Tolkster".into(), projects: Vec::new(), teams: Vec::new(), review: None, recipe: Some(RecipeMachines { id: "cloud".into(), machines, slots_each: 3, idle_minutes: 30, destroy_after: 600 }), land: Land::Pr, trace: false }])
}

// Each of this machine's repos whose `repos` entry has a recipe is named to the fleet by that
// entry, by its Orca ID, so a request's repo finds its recipe; one with no project known, no
// display name read or no recipe isn't.
#[test]
fn the_fleet_names_each_repo_with_a_recipe() {
    let pacing = Pacing::default();
    let config = Config { repos: opted_in(2), ..config(1, &[]) };
    let origins: BTreeMap<String, String> = [("r1", "acme/tolkster"), ("r2", "acme/other"), ("r3", "acme/unnamed")].into_iter().map(|(id, origin)| (id.into(), origin.into())).collect();
    let names: BTreeMap<String, String> = [("r1", "tolkster"), ("r2", "other"), ("r4", "Tolkster")].into_iter().map(|(id, name)| (id.into(), name.into())).collect();
    let fleet = fleet(&config, &pacing, &origins, &names, 0, &[], &[]);
    assert_eq!(fleet.recipes, BTreeMap::from([("r1".to_string(), "Tolkster".to_string())]));
    assert_eq!(serde_json::to_value(&fleet).unwrap()["recipes"], serde_json::json!({ "r1": "Tolkster" }));
    assert!(serde_json::to_value(super::fleet(&config, &pacing, &BTreeMap::new(), &BTreeMap::new(), 0, &[], &[])).unwrap().get("recipes").is_none());
}

// A machine is idle while it is awake, sleeps, is enrolled, answers and no agent works on it: a
// worker done or waiting at a prompt, or exited, keeps it so, and a sleep's check also counts a
// worker being set up there. A napping one is placed on by no start.
#[test]
fn an_idle_machine_has_no_agent_working_and_a_napping_one_takes_no_worker() {
    let pacing = Pacing::default();
    let mut config = Config { repos: opted_in(2), ..config(1, &[]) };
    config.enrolled = vec!["id-nap-1".into(), "id-nap-2".into()];
    config.machines.insert("nap-1".into(), machine(true, false));
    config.machines.insert("nap-2".into(), machine(false, false));
    let answering = [Host { id: "id-nap-1".into(), name: "nap-1".into() }, Host { id: "id-nap-2".into(), name: "nap-2".into() }];
    let (worktree, record) = running_on(Some("id-nap-1"), 1);
    let checkout = Worktree { id: "id-nap-1@r::/home/node/tolkster".into(), is_main: true, agents: Vec::new(), ..worktree.clone() };
    assert_eq!(idle(&config, &pacing, &answering, std::slice::from_ref(&checkout)), BTreeMap::from([("id-nap-1".to_string(), 30)]));
    assert!(idle(&config, &pacing, &answering, &[checkout.clone(), worktree.clone()]).is_empty());
    for state in ["done", "waiting", "blocked"] {
        let resting = Worktree { agents: vec![Pane { key: "t:l".into(), state: state.into(), since_ms: None }], ..worktree.clone() };
        assert_eq!(idle(&config, &pacing, &answering, &[checkout.clone(), resting]).len(), 1, "{state}");
    }
    let exited = Worktree { agents: Vec::new(), ..worktree.clone() };
    assert!(rests(&[checkout.clone(), exited.clone()], std::slice::from_ref(&record), "id-nap-1", NOW));
    let (_, setting_up) = worker(Some("id-nap-1"), 1, false, 60 * 1000);
    assert!(!rests(&[checkout, exited], &[setting_up], "id-nap-1", NOW));
    config.machines.insert("nap-1".into(), machine(true, true));
    assert!(idle(&config, &pacing, &answering, &[]).is_empty());
    config.machines.insert("nap-1".into(), machine(true, false));
    config.enrolled.clear();
    assert!(idle(&config, &pacing, &answering, &[]).is_empty());
    config.enrolled = vec!["id-nap-1".into()];
    pacing.nap("id-nap-1");
    assert!(idle(&config, &pacing, &answering, &[]).is_empty());
    pacing.placing_again("id-nap-1");
    assert_eq!(idle(&config, &pacing, &answering, &[]).len(), 1);
}

// A machine's destroy clock runs while it is asleep, a mark, or awake and holding nothing but
// unadmitted or with no `suspend`, with its repo's `destroyAfter`, or the default where its repo
// dropped its recipe. One admitted that sleeps, holding a worktree, napping or silent has none.
#[test]
fn a_machine_is_dormant_asleep_as_a_mark_or_empty_where_it_never_sleeps() {
    let pacing = Pacing::default();
    let mut config = Config { repos: opted_in(6), ..config(1, &[]) };
    config.enrolled = vec!["id-end-1".into(), "id-end-2".into()];
    config.machines.insert("end-1".into(), machine(true, false));
    config.machines.insert("end-2".into(), machine(false, false));
    config.machines.insert("end-3".into(), machine(true, false));
    config.machines.insert("end-4".into(), machine(true, true));
    config.machines.insert("end-5".into(), Machine { made: false, ..machine(false, false) });
    config.machines.insert("end-6".into(), machine(false, false));
    config.machines.insert("gone-1".into(), Machine { repo: "Gone".into(), ..machine(true, true) });
    let answering: Vec<Host> = ["end-1", "end-2", "end-3"].iter().map(|name| Host { id: format!("id-{name}"), name: name.to_string() }).collect();
    let (worktree, _) = running_on(Some("id-end-2"), 1);
    let checkout = Worktree { id: "id-end-2@r::/home/node/tolkster".into(), is_main: true, agents: Vec::new(), ..worktree.clone() };
    let names = |worktrees: &[Worktree]| dormant(&config, &pacing, &answering, worktrees).into_keys().collect::<Vec<_>>();
    assert_eq!(names(std::slice::from_ref(&checkout)), ["end-2", "end-3", "end-4", "end-5", "gone-1"]);
    assert_eq!(names(&[checkout, worktree]), ["end-3", "end-4", "end-5", "gone-1"]);
    pacing.nap("id-end-3");
    assert_eq!(names(&[]), ["end-2", "end-4", "end-5", "gone-1"]);
    pacing.placing_again("id-end-3");
    let minutes = dormant(&config, &pacing, &answering, &[]);
    // A mark goes after an hour, under a longer `destroyAfter`.
    assert_eq!((minutes["end-4"], minutes["gone-1"], minutes["end-5"]), (600, DEFAULT_DESTROY_AFTER, 60));
}
