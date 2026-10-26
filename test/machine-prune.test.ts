import assert from "node:assert/strict";
import { test } from "node:test";
import { SEEN_KEEP_MS } from "../src/worker/intake.ts";
import type { Storage } from "../src/worker/storage.ts";
import { pruneStore } from "../src/worker/machine/prune.ts";
import { newCoordinator } from "../src/worker/machine/coordinator.ts";
import { nextDue, receive, retirable, settled } from "../src/worker/machine/step.ts";
import { deliver, loadCoordinator, loadStore, reconcile } from "../src/worker/machine/store.ts";
import { RETIRE_AFTER_MS, type Command } from "../src/worker/machine/types.ts";
import { ev, given, ISSUE, NOW, S, SHA, T0, T1, T2 } from "./machine-fixture.ts";
import { MemoryStorage } from "./storage.ts";

const OLD = "1111111111111111111111111111111111111111";
const BASE = "2222222222222222222222222222222222222222";
const pr = (head: string) => ({ number: 7, head, repo: "o/r", status: "open" as const, at: T1, receivedMs: NOW });

const sender = () => {};

test("a PR's checks, feedback and rounds are kept for its current head alone", () => {
  const s = given({ work: "rest", facts: { pr: pr(SHA) } });
  s.facts.checks = { [OLD]: { value: "passed", at: T0 }, [SHA]: { value: "failed", at: T1 } };
  s.facts.feedback = { [OLD]: { at: T0, copilot: false }, [SHA]: { at: T1, copilot: true } };
  s.facts.rounds.byHead = { [OLD]: 2, [SHA]: 1 };
  pruneStore(s);
  assert.deepEqual(Object.keys(s.facts.checks), [SHA]);
  assert.deepEqual(Object.keys(s.facts.feedback), [SHA]);
  assert.deepEqual(s.facts.rounds.byHead, { [SHA]: 1 });
  assert.equal(s.facts.rounds.total, 0);
  s.facts.pr = pr(BASE);
  pruneStore(s);
  assert.deepEqual([s.facts.checks, s.facts.feedback, s.facts.rounds.byHead], [{}, {}, {}]);
});

test("with no PR held, nothing per head is dropped", () => {
  const s = given({ work: "running" });
  s.facts.checks = { [OLD]: { value: "passed", at: T0 } };
  pruneStore(s);
  assert.deepEqual(Object.keys(s.facts.checks), [OLD]);
});

test("a notice stays only for its session's current request, the worker's, the PR's head and the pause", () => {
  const s = given({ req: "queued", work: "rest", facts: { pr: pr(SHA) }, sess: { request: { kind: "prompt", at: T2 } }, w: { requestedBy: { session: S, at: T1 }, pausedSinceMs: NOW } });
  s.facts.notices = [
    `busy:${S}:${T0}`, // an earlier request's
    `busy:${S}:${T2}`, // the current one's
    `blocked:${S}:${T0}:TW-1,TW-2`,
    `blocked:${S}:${T2}:TW-1`,
    `stalled:${S}:${T1}`, // the worker's request, though its session has been asked since
    `roundsCap:7:${OLD}`,
    `roundsCap:7:${SHA}`,
    `prClosed:6:${SHA}`,
    `ciCouldntRun:7:${OLD}`,
    `copilotWontApprove:7:${OLD}`,
    `paused:${NOW - 1}`,
    `paused:${NOW}`,
    `heldAtPrompt:${NOW - 1}:${S}`,
    `heldAtPrompt:${NOW}:${S}`,
    "heldAtPrompt:null",
    "unheard:of:kinds",
  ];
  pruneStore(s);
  assert.deepEqual(s.facts.notices, [`busy:${S}:${T2}`, `blocked:${S}:${T2}:TW-1`, `stalled:${S}:${T1}`, `roundsCap:7:${SHA}`, `paused:${NOW}`, `heldAtPrompt:${NOW}:${S}`, "heldAtPrompt:null", "unheard:of:kinds"]);
});

test("a notice a step says is kept by that step, whichever key it took", () => {
  const s = given({ req: "queued", sess: { request: { kind: "prompt", at: T2 } } });
  s.facts.notices = [`noLongerMine:${S}:${T1}`];
  // `e.at` of a delegation change is earlier than the request it refused, here.
  pruneStore(s);
  assert.deepEqual(s.facts.notices, []);
  s.facts.notices = [`noLongerMine:${S}:${T2}`];
  pruneStore(s);
  assert.deepEqual(s.facts.notices, [`noLongerMine:${S}:${T2}`]);
});

test("every step ends by pruning, so a push moves the per-head facts to its head", () => {
  const s = given({ work: "rest", facts: { pr: pr(OLD) } });
  s.facts.checks = { [OLD]: { value: "passed", at: T0 } };
  s.facts.rounds.byHead = { [OLD]: 1 };
  const { store } = receive(s, ev("prPushed", { pr: 7, head: SHA, before: OLD, at: T2 }), NOW);
  assert.equal(store.facts.pr?.head, SHA);
  assert.deepEqual([store.facts.checks, store.facts.rounds.byHead], [{}, {}]);
});

// ---- Retirement ----

const quiet = () => given({ work: "reaped", req: "done" });

test("a store is settled when no machine, command, event or deadline is left", () => {
  assert.ok(settled(quiet()));
  assert.ok(!settled(given({})), "a store with no machine has nothing to retire");
  assert.ok(!settled(given({ work: "reaped", req: "done", facts: { pr: pr(SHA) } })));
  assert.ok(settled(given({ work: "reaped", req: "done", facts: { pr: { ...pr(SHA), status: "merged" } } })));
  assert.ok(!settled(given({ work: "rest", req: "done" })));
  assert.ok(!settled(given({ work: "reaped", req: "claimed" })));
  assert.ok(!settled(given({ work: "reaped", req: "done", queue: [{ sentMs: null, firstSentMs: null }] })));
  assert.ok(!settled(given({ work: "reaped", req: "done", other: "queued" })));
  const owed = quiet();
  owed.work!.deadlines.push({ kind: "takeBack", version: owed.work!.version, dueMs: NOW });
  assert.ok(!settled(owed));
});

test("a settled store is due for its retirement, and an event since puts it off", () => {
  const { store } = receive(quiet(), ev("agentExited"), NOW);
  assert.equal(store.settledMs, NOW);
  assert.equal(nextDue(store, NOW), NOW + RETIRE_AFTER_MS);
  assert.ok(!retirable(store, NOW + RETIRE_AFTER_MS - 1));
  assert.ok(retirable(store, NOW + RETIRE_AFTER_MS));
  const woken = receive(store, ev("ownerPrompted", { session: S, byOwner: true, at: T2 }), NOW + 1000).store;
  assert.equal(woken.settledMs, null);
  assert.ok(!retirable(woken, NOW + RETIRE_AFTER_MS));
});

test("the retirement outlasts the intake's replay window", () => {
  assert.ok(RETIRE_AFTER_MS > SEEN_KEEP_MS);
});

async function settledAt(storage: Storage): Promise<void> {
  await storage.put("machine:issue:" + ISSUE, quiet());
  await deliver(storage, ev("agentExited"), NOW, sender);
}

test("the alarm retires a settled store to its counters, and the coordinator's entries for it", async () => {
  const storage = new MemoryStorage();
  await settledAt(storage);
  const c = newCoordinator();
  c.blockers[ISSUE] = { sentMs: NOW, ids: ["x"] };
  c.heard[ISSUE] = 4;
  c.heard.other = 2;
  c.blockers.other = { sentMs: NOW, ids: ["y"] };
  await storage.put("machine:coordinator", c);
  const before = await loadStore(storage, ISSUE);
  assert.ok(before.work !== null);
  assert.deepEqual(Object.keys((await storage.get<object>("machine:due")) ?? {}), [ISSUE]);

  // Not yet.
  await reconcile(storage, NOW + RETIRE_AFTER_MS - 1, sender, true);
  assert.ok((await loadStore(storage, ISSUE)).work !== null);

  await reconcile(storage, NOW + RETIRE_AFTER_MS, sender, true);
  const after = await loadStore(storage, ISSUE);
  assert.equal(after.work, null);
  assert.deepEqual(Object.keys(after.sessions), [S]);
  assert.deepEqual([after.facts.sessions[S], after.sessions[S].machine, after.sessions[S].facts.request], [{ kind: "delegation", createdAt: T0, started: true }, null, null]);
  assert.equal(after.settledMs, null);
  assert.deepEqual([after.seq, after.commandSeq], [before.seq, before.commandSeq]);
  assert.ok(after.rev > before.rev);
  assert.deepEqual(await storage.get("machine:due"), {});
  const kept = await loadCoordinator(storage);
  assert.deepEqual([kept.blockers, kept.heard], [{ other: { sentMs: NOW, ids: ["y"] } }, { other: 2 }]);
  assert.equal(kept.rev, c.rev + 1);

  // Nothing more is due, and a later alarm leaves it be.
  await reconcile(storage, NOW + 2 * RETIRE_AFTER_MS, sender, true);
  assert.equal((await loadStore(storage, ISSUE)).rev, after.rev);
});

test("a retired issue that stirs again goes on from its counters", async () => {
  const storage = new MemoryStorage();
  await storage.put("machine:issue:" + ISSUE, { ...quiet(), commandSeq: 9, seq: 5 });
  await deliver(storage, ev("agentExited"), NOW, sender);
  await reconcile(storage, NOW + RETIRE_AFTER_MS, sender, true);
  const sent: Command[] = [];
  const later = NOW + RETIRE_AFTER_MS + 1;
  await deliver(storage, { kind: "ownerPrompted", id: "again", issue: ISSUE, session: S, byOwner: true, at: "2026-12-01T00:00:00.000Z" }, later, (c) => sent.push(...c));
  assert.deepEqual(sent.map((c) => c.id), [`${ISSUE}.10`]);
});

test("a store whose last step sent an event is settled once it has been delivered, and retires", async () => {
  const storage = new MemoryStorage();
  await storage.put("machine:issue:" + ISSUE, quiet());
  await deliver(storage, ev("issueStateChanged", { state: "completed", at: T2, parent: null }), NOW, sender);
  const held = await loadStore(storage, ISSUE);
  assert.deepEqual(held.outbox, []);
  assert.equal(held.settledMs, NOW);
  assert.deepEqual(await storage.get("machine:due"), { [ISSUE]: NOW + RETIRE_AFTER_MS });
  await reconcile(storage, NOW + RETIRE_AFTER_MS, sender, true);
  assert.equal((await loadStore(storage, ISSUE)).work, null);
});

test("an open PR or sub-issues still to finish keep a store from settling", () => {
  assert.ok(settled(quiet()));
  assert.ok(!settled(given({ work: "reaped", req: "done", facts: { pr: pr(SHA) } })));
  const parent = given({ work: "reaped", req: "done", sess: { orchestrated: { at: T1, known: [] } } });
  parent.facts.children = { sentMs: NOW, ids: ["c1"], states: { c1: { done: false, at: T1 } }, more: false };
  assert.ok(!settled(parent));
  parent.facts.children.states.c1 = { done: true, at: T2 };
  assert.ok(settled(parent));
  const chained = given({ work: "reaped", req: "done", facts: { chained: { run: "r", issues: ["c1"], asked: [], answered: [], haltedAt: null } } });
  chained.facts.children = { sentMs: NOW, ids: ["c1"], states: { c1: { done: false, at: T1 } }, more: false };
  assert.ok(!settled(chained));
});

