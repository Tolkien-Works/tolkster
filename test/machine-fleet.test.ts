import assert from "node:assert/strict";
import { test } from "node:test";
import { newCoordinator, type Coordinator, type Place } from "../src/worker/machine/coordinator.ts";
import { COOL_DOWN_MS, DESTROY_RETRY_MS, expireFleet, fleetData, fleetDue, NAP_RETRY_MS, newRun, reconcileFleet, REASONS, REVIVE_EVERY_MS, roomOf, SETTLE_MS, stepFleet, UNHEARD_GRACE_MS, UPDATE_RETRY_MS, type Fleet, type FleetEvent, type Heard, type Made, type Run } from "../src/worker/machine/fleet.ts";
import { fleetEvent, fleetReconciled, fleetReported, inFlight, keptAtReset, loadRun, machineTimer, reconcile } from "../src/worker/machine/store.ts";
import { COMMAND_TIMEOUT_MS, SENDS, type Command } from "../src/worker/machine/types.ts";
import { parseReport } from "../src/worker/machine.ts";
import { fleetWire } from "../src/worker/route.ts";
import contract from "./contract.json" with { type: "json" };
import { MIN, NOW, T0 } from "./machine-fixture.ts";
import { MemoryStorage } from "./storage.ts";

const RECIPE = { id: "agent37", machines: 2, slotsEach: 1, idleMinutes: 30, destroyAfter: 1440, revision: null };
const awake: Made = { repo: "tolkster", made: true, asleep: false, environment: null, revision: null };
const asleep: Made = { ...awake, asleep: true, environment: "h-1" };
const host = (id: string, name: string, more: Partial<Heard> = {}): Heard => ({ id, name, cap: 1, admitted: true, answering: true, ready: ["github:acme/tolkster"], held: 0, ...more });

// This machine with a cap of one, the repo `r1` (Orca's ID) set up here and its recipe as
// `tolkster`, and the machines and hosts given.
function fleet(machines: Record<string, Made> = {}, hosts: Heard[] = []): Fleet {
  return { settings: { cap: 1, slots: {}, repos: { tolkster: RECIPE }, machines }, hosts: { held: 0, hosts, projects: { r1: "github:acme/tolkster" }, recipes: { r1: "tolkster" } } };
}

const waits = (issue: string, more: Partial<Place> = {}): Place => ({ issue, session: issue, priority: 2, at: T0, repo: "r1", ...more });

// The ledger with here's one slot held, so a new worker has nowhere to go but a machine.
function full(queue: Place[] = [waits("q")]): Coordinator {
  const c = newCoordinator();
  c.grants.held = { session: null, sinceMs: NOW };
  c.queue = queue;
  return c;
}

function run(r: Run, f: Fleet, c: Coordinator, at: number, admitted = ["h-1", "h-2"]) {
  return reconcileFleet(r, { fleet: f, c, room: roomOf(f, admitted, []), admitted }, at);
}

const asked = (send: Command[]) => send.map((c) => [c.kind, c.params.name ?? c.params.host ?? c.params.repo]);
const step = (r: Run, e: FleetEvent, at: number) => stepFleet(r, e, at).run;
const ok = (r: Run, data: Record<string, unknown> = {}, at = NOW) => step(r, { kind: "commandResult", command: r.command!.id, result: "ok", data }, at);

// TW-883: with no host paired and here full, the repo's first machine is made, and while it is
// made the request waits on it, no second make asked.
test("a request with nowhere to go makes its repo's first machine, once", () => {
  const { run: made, send } = run(newRun(), fleet(), full(), NOW);
  assert.deepEqual(asked(send), [["makeHost", "tolkster"]]);
  assert.equal(send[0].id, "fleet.1");
  const began = ok(made, { began: true });
  const again = run(began, fleet(), full(), NOW + MIN);
  assert.deepEqual(again.send, []);
  assert.deepEqual(again.run.why, { tolkster: "running" });
  // A listing that began before the make did ends nothing, however late it comes; one begun
  // SETTLE_MS after it, with `listen` making nothing, ends it, and a repo still short of a
  // machine asks again.
  assert.notEqual(step(began, { kind: "listed", idle: {}, dormant: {}, elapsedMs: 60_000 }, NOW + 60_000).running, null);
  assert.equal(step(began, { kind: "listed", idle: {}, dormant: null, elapsedMs: 0 }, NOW + SETTLE_MS).running?.repo, "tolkster");
  const ended = step(began, { kind: "listed", idle: {}, dormant: {}, elapsedMs: 0 }, NOW + SETTLE_MS);
  assert.deepEqual(asked(run(ended, fleet(), full(), NOW + SETTLE_MS).send), [["makeHost", "tolkster"]]);
});

// TW-884: a request looked at while here had room isn't left there: the reconcile after each
// event asks for its machine once here fills.
test("a request seen while here had room gets a machine once here fills", () => {
  const room = newCoordinator();
  room.queue = [waits("q")];
  const early = run(newRun(), fleet(), room, NOW);
  assert.deepEqual(early.send, []);
  assert.deepEqual(early.run.why, { tolkster: "hereHasRoom" });
  assert.deepEqual(asked(run(early.run, fleet(), full(), NOW + MIN).send), [["makeHost", "tolkster"]]);
});

// Wake beats make, and with every machine of the repo made and awake it waits.
test("an asleep machine of the repo is woken rather than another made, and with all made it waits", () => {
  assert.deepEqual(asked(run(newRun(), fleet({ "tolkster-1": asleep }), full(), NOW).send), [["wakeHost", "tolkster-1"]]);
  // One whose credential the Durable Object doesn't hold isn't woken: a second is made.
  assert.deepEqual(asked(run(newRun(), fleet({ "tolkster-1": asleep }), full(), NOW, []).send), [["makeHost", "tolkster"]]);
  const all = fleet({ "tolkster-1": awake, "tolkster-2": awake }, [host("h-1", "tolkster-1", { held: 1 }), host("h-2", "tolkster-2", { held: 1 })]);
  const none = run(newRun(), all, full(), NOW);
  assert.deepEqual(none.send, []);
  assert.deepEqual(none.run.why, { tolkster: "allMade" });
  // An awake machine of the repo silent under the grace is room to come: no second is made.
  // One not paired at all is no room: a machine is made.
  assert.deepEqual(asked(run(newRun(), fleet({ "tolkster-1": awake }), full(), NOW).send), [["makeHost", "tolkster"]]);
  const coming = fleet({ "tolkster-1": awake }, [host("h-1", "tolkster-1", { answering: false })]);
  const toCome = run(newRun(), coming, full(), NOW);
  assert.deepEqual([toCome.send, toCome.run.why], [[], { tolkster: "hostComing" }]);
  // Nor is one asleep woken meanwhile; silent past the grace, it is no longer counted on.
  const both = fleet({ "tolkster-1": awake, "tolkster-2": { ...asleep, environment: "h-2" } }, [host("h-1", "tolkster-1", { answering: false })]);
  assert.deepEqual(run(toCome.run, both, full(), NOW + MIN).send, []);
  assert.deepEqual(asked(run(toCome.run, both, full(), NOW + UNHEARD_GRACE_MS).send), [["wakeHost", "tolkster-2"]]);
  // A repo with no recipe says so, by its Orca ID.
  assert.deepEqual(run(newRun(), all, full([waits("q", { repo: "r9" })]), NOW).run.why, { r9: "noRecipe" });
});

// A make that failed, or that `listen` didn't begin, holds its repo back for the cool-down.
test("a make not begun cools its repo down before the next", () => {
  const first = run(newRun(), fleet(), full(), NOW).run;
  for (const end of [(r: Run) => ok(r, { began: false }), (r: Run) => step(r, { kind: "commandResult", command: r.command!.id, result: "failed" }, NOW)]) {
    const cooling = run(end(first), fleet(), full(), NOW + MIN);
    assert.deepEqual(cooling.send, []);
    assert.deepEqual(cooling.run.why, { tolkster: "coolingDown" });
    assert.equal(fleetDue(cooling.run), NOW + COOL_DOWN_MS);
    assert.deepEqual(asked(run(cooling.run, fleet(), full(), NOW + COOL_DOWN_MS).send), [["makeHost", "tolkster"]]);
  }
});

// TW-881: a resume pinned to an asleep machine's worktree wakes it, as does a grant on it or a
// worker's command waiting on it.
test("a request, a grant or a command needing an asleep machine wakes it", () => {
  const pinned = full([waits("p", { on: "h-1" })]);
  assert.deepEqual(asked(run(newRun(), fleet({ "tolkster-1": asleep }), pinned, NOW).send), [["wakeHost", "tolkster-1"]]);
  const granted = newCoordinator();
  granted.grants.g = { session: null, sinceMs: NOW, host: "h-1" };
  assert.deepEqual(asked(run(newRun(), fleet({ "tolkster-1": asleep }), granted, NOW).send), [["wakeHost", "tolkster-1"]]);
  const wanted = step(newRun(), { kind: "wanted", host: "h-1" }, NOW);
  assert.deepEqual(asked(run(wanted, fleet({ "tolkster-1": asleep }), newCoordinator(), NOW).send), [["wakeHost", "tolkster-1"]]);
});

// TW-882: a machine woken for a command isn't put to sleep while the command waits on it, nor
// while a grant names it; once neither does, its idle minutes put it to sleep.
test("an idle host is slept only once nothing holds it awake", () => {
  const f = fleet({ "tolkster-1": awake }, [host("h-1", "tolkster-1")]);
  const idle = step(newRun(), { kind: "listed", idle: { "h-1": 30 }, dormant: {}, elapsedMs: 0 }, NOW);
  assert.equal(fleetDue(idle), NOW + 30 * MIN);
  assert.deepEqual(run(idle, f, newCoordinator(), NOW + 30 * MIN - 1).send, []);
  const granted = newCoordinator();
  granted.grants.g = { session: null, sinceMs: NOW, host: "h-1" };
  const held = run(idle, f, granted, NOW + 30 * MIN);
  assert.deepEqual(held.send, []);
  assert.equal(fleetDue(held.run), NOW + 30 * MIN + NAP_RETRY_MS);
  const silent = fleet({ "tolkster-1": awake }, [host("h-1", "tolkster-1", { answering: false })]);
  const waiting = step(idle, { kind: "wanted", host: "h-1" }, NOW + 20 * MIN);
  assert.deepEqual(run(waiting, silent, newCoordinator(), NOW + 30 * MIN).send, []);
  // Heard again, the command's wait ends and the sleep goes.
  const slept = run(idle, f, newCoordinator(), NOW + 30 * MIN);
  assert.deepEqual(asked(slept.send), [["suspendHost", "h-1"]]);
  // Asleep or kept, its clock ends; failed, it goes again NAP_RETRY_MS on.
  assert.deepEqual(ok(slept.run).idle, {});
  const failed = step(slept.run, { kind: "commandResult", command: "fleet.1", result: "failed" }, NOW + 31 * MIN);
  assert.equal(fleetDue(failed), NOW + 31 * MIN + NAP_RETRY_MS);
});

// TW-879: a machine marked awake whose host is silent past the grace is woken once, and again
// only after REVIVE_EVERY_MS.
test("a machine marked awake but unheard is woken once", () => {
  const f = fleet({ "tolkster-1": awake }, [host("h-1", "tolkster-1", { answering: false })]);
  const seen = run(newRun(), f, newCoordinator(), NOW);
  assert.deepEqual(seen.send, []);
  assert.equal(fleetDue(seen.run), NOW + UNHEARD_GRACE_MS);
  const woken = run(seen.run, f, newCoordinator(), NOW + UNHEARD_GRACE_MS);
  assert.deepEqual(asked(woken.send), [["wakeHost", "tolkster-1"]]);
  const kept = ok(woken.run, { began: false }, NOW + UNHEARD_GRACE_MS);
  assert.deepEqual(run(kept, f, newCoordinator(), NOW + UNHEARD_GRACE_MS + COOL_DOWN_MS).send, []);
  assert.deepEqual(asked(run(kept, f, newCoordinator(), NOW + UNHEARD_GRACE_MS + REVIVE_EVERY_MS).send), [["wakeHost", "tolkster-1"]]);
  // Kept, its repo isn't cooled: a request of it may still have a machine made.
  assert.deepEqual(kept.cooling, {});
  // Heard, it is no longer unheard.
  const heard = fleet({ "tolkster-1": awake }, [host("h-1", "tolkster-1")]);
  assert.deepEqual(run(seen.run, heard, newCoordinator(), NOW + MIN).run.unheard, {});
});

// A dormant machine is destroyed its minutes on, unless a grant names its host; one command at a
// time, so nothing else is asked while it is in flight.
test("a dormant machine is destroyed once due and free, one command at a time", () => {
  const f = fleet({ "tolkster-1": asleep });
  const dormant = step(newRun(), { kind: "listed", idle: {}, dormant: { "tolkster-1": 1440 }, elapsedMs: 0 }, NOW);
  const granted = newCoordinator();
  granted.grants.g = { session: null, sinceMs: NOW, host: "h-1" };
  const sent = run(dormant, f, newCoordinator(), NOW + 1440 * MIN);
  assert.deepEqual(asked(sent.send), [["destroyHost", "tolkster-1"]]);
  assert.deepEqual(run(sent.run, f, full(), NOW + 1441 * MIN).send, []);
  assert.deepEqual(run(sent.run, f, full(), NOW + 1441 * MIN).run.why, { tolkster: "running" });
  const failed = step(sent.run, { kind: "commandResult", command: "fleet.1", result: "lost" }, NOW + 1441 * MIN);
  assert.equal(fleetDue(failed), NOW + 1441 * MIN + DESTROY_RETRY_MS);
  // Held by a grant: the wake goes first instead.
  assert.deepEqual(asked(run(dormant, f, granted, NOW + 1440 * MIN).send), [["wakeHost", "tolkster-1"]]);
});

// The command in flight times out like any, and one waiting on the lock goes again as it clears.
test("the fleet's command is resent at its timeout, lost after its sends, and sent early as its wait clears", () => {
  let r = run(newRun(), fleet(), full(), NOW).run;
  assert.equal(fleetDue(r), NOW + COMMAND_TIMEOUT_MS);
  for (let sends = 1; sends < SENDS; sends++) r = expireFleet(r, fleetDue(r)!).run;
  r = expireFleet(r, fleetDue(r)!).run;
  assert.equal(r.command, null);
  assert.ok(Object.hasOwn(r.cooling, "tolkster"));
  const waiting = step(run(newRun(), fleet(), full(), NOW).run, { kind: "commandResult", command: "fleet.1", result: "retryable", data: { waitsOn: { lock: true } } }, NOW);
  assert.deepEqual(stepFleet(waiting, { kind: "cleared", clears: { hosts: ["h-1"] } }, NOW + MIN).send, []);
  assert.deepEqual(asked(stepFleet(waiting, { kind: "cleared", clears: { lock: true } }, NOW + MIN).send), [["makeHost", "tolkster"]]);
  // A complete listing of a `listen` making nothing frees the lock too.
  assert.deepEqual(asked(stepFleet(waiting, { kind: "listed", idle: {}, dormant: {}, elapsedMs: 0 }, NOW + MIN).send), [["makeHost", "tolkster"]]);
  assert.deepEqual(stepFleet(waiting, { kind: "listed", idle: {}, dormant: null, elapsedMs: 0 }, NOW + MIN).send, []);
  // A result for a command it doesn't hold, an old nap's, changes nothing.
  assert.equal(step(waiting, { kind: "commandResult", command: "naps.4", result: "ok" }, NOW), waiting);
});

// TW-878: a rollout reaches each machine while it sleeps, one at a time, after any machine a
// request needs and any sleep or destroy due, and never one something holds awake.
test("an asleep machine behind its recipe's revision is updated, once until it says the new one", () => {
  const rolled = (machines: Record<string, Made>, revision: string | null = "5") => {
    const f = fleet(machines);
    f.settings!.repos.tolkster = { ...RECIPE, revision };
    return f;
  };
  const behind = { ...asleep, revision: "4" };
  const idle = newCoordinator();
  assert.deepEqual(asked(run(newRun(), rolled({ "tolkster-1": behind }), idle, NOW).send), [["updateHost", "tolkster-1"]]);
  // One with no revision said is behind too; one on it, awake, or before any `prepare` said one, isn't.
  assert.deepEqual(asked(run(newRun(), rolled({ "tolkster-1": asleep }), idle, NOW).send), [["updateHost", "tolkster-1"]]);
  for (const f of [rolled({ "tolkster-1": { ...asleep, revision: "5" } }), rolled({ "tolkster-1": { ...awake, revision: "4" } }), rolled({ "tolkster-1": behind }, null)]) {
    assert.deepEqual(run(newRun(), f, idle, NOW).send, []);
  }
  // One whose credential the Durable Object doesn't hold isn't; a host due to sleep goes first.
  assert.deepEqual(run(newRun(), rolled({ "tolkster-1": behind }), idle, NOW, []).send, []);
  const napping = step(newRun(), { kind: "listed", idle: { "h-2": 30 }, dormant: {}, elapsedMs: 0 }, NOW);
  assert.deepEqual(asked(run(napping, rolled({ "tolkster-1": behind }), idle, NOW + 30 * MIN).send), [["suspendHost", "h-2"]]);
  // A request pinned to it wakes it on the old image instead; a waiting command holds it too.
  const pinned = newCoordinator();
  pinned.queue = [waits("q", { on: "h-1" })];
  assert.deepEqual(asked(run(newRun(), rolled({ "tolkster-1": behind }), pinned, NOW).send), [["wakeHost", "tolkster-1"]]);
  const wanted = step(newRun(), { kind: "wanted", host: "h-1" }, NOW);
  assert.deepEqual(run(wanted, rolled({ "tolkster-1": behind }), idle, NOW, []).send, []);
  // Done or failed, it isn't asked again until UPDATE_RETRY_MS, unless the config report says
  // the new revision first; the next behind goes then.
  const two = rolled({ "tolkster-1": behind, "tolkster-2": { ...behind, environment: "h-2" } });
  for (const result of ["ok", "failed"] as const) {
    const asking = run(newRun(), two, idle, NOW).run;
    const ended = step(asking, { kind: "commandResult", command: asking.command!.id, result }, NOW);
    assert.deepEqual(asked(run(ended, two, idle, NOW).send), [["updateHost", "tolkster-2"]]);
    const one = rolled({ "tolkster-1": behind });
    const held = run(ended, one, idle, NOW);
    assert.deepEqual(held.send, []);
    assert.equal(fleetDue(held.run), NOW + UPDATE_RETRY_MS);
    assert.deepEqual(asked(run(held.run, one, idle, NOW + UPDATE_RETRY_MS).send), [["updateHost", "tolkster-1"]]);
    assert.deepEqual(run(held.run, rolled({ "tolkster-1": { ...behind, revision: "5" } }), idle, NOW).run.updated, {});
  }
});

test("the fleet's wires are the fixture's, and its reasons the CLI's", () => {
  for (const [kind, params] of [["suspendHost", { host: "mac" }], ["destroyHost", { name: "tolkster-1" }], ["makeHost", { repo: "tolkster" }], ["wakeHost", { name: "tolkster-1", repo: "tolkster" }], ["updateHost", { name: "tolkster-1" }]] as const) {
    const wire = contract.commands.find((c) => c.kind === kind)!;
    const command: Command = { id: wire.id, issue: "fleet", kind, machine: "fleet", version: wire.version, params, class1: false, firstSentMs: NOW, sentMs: NOW, sends: 1, superseded: false };
    assert.deepEqual(fleetWire(command), wire);
  }
  assert.deepEqual([...REASONS], contract.machine.fleetReasons);
  assert.deepEqual(fleetData(contract.fleetBegan, undefined), contract.fleetBegan);
  assert.deepEqual(fleetData(contract.fleetCoolDown, undefined), contract.fleetCoolDown);
});

// The machine's record survives in storage: reconciled on a config report and on the alarm, its
// command is sent to a new connection, and a reset keeps only its counters.
test("the store reconciles the fleet and keeps its command in flight", async () => {
  const storage = new MemoryStorage<unknown>();
  const sent: Command[] = [];
  const f = fleet({ "tolkster-1": awake }, [host("h-1", "tolkster-1")]);
  await fleetReported(storage, f, NOW, () => {});
  await fleetEvent(storage, { kind: "listed", idle: { "h-1": 30 }, dormant: {}, elapsedMs: 0 }, NOW, (c) => sent.push(...c));
  await fleetReconciled(storage, NOW, (c) => sent.push(...c));
  assert.equal(await machineTimer(storage, () => {}, () => true).next(), NOW + 30 * MIN);
  await reconcile(storage, NOW + 30 * MIN, (c) => sent.push(...c), true);
  assert.deepEqual(asked(sent), [["suspendHost", "h-1"]]);
  assert.deepEqual((await inFlight(storage)).map((c) => c.id), ["fleet.1"]);
  const held = await loadRun(storage);
  assert.deepEqual(keptAtReset("machine:fleet:run", held), { ...newRun(), rev: held.rev, commandSeq: 1 });
  assert.equal(keptAtReset("machine:naps", { rev: 1, commandSeq: 2 }), undefined);
  await fleetEvent(storage, { kind: "commandResult", command: "fleet.1", result: "ok" }, NOW + 31 * MIN, () => {});
  assert.deepEqual((await loadRun(storage)).idle, {});
});

test("a listing's idle hosts are answering ones', and its dormant machines plain names", () => {
  const listing = { report: "listing", at: "2026-09-26T01:06:00.000Z", elapsedMs: 0, agents: {}, listed: [], hosts: ["mac", "pi"], paired: ["mac", "pi"], idle: {} };
  const parsed = (more: object) => {
    const report = parseReport({ ...listing, ...more });
    return report?.report === "listing" ? [report.idle, report.dormant] : null;
  };
  assert.deepEqual(parsed({ idle: { mac: 30, pi: 0, gone: 30 } })?.[0], { mac: 30 });
  assert.deepEqual(parsed({ idle: [1] })?.[0], {});
  assert.deepEqual(parsed({ dormant: { "tolkster-1": 1440, "-x": 30, y: 0 } })?.[1], { "tolkster-1": 1440 });
  assert.equal(parsed({ dormant: [1] })?.[1], null);
});
