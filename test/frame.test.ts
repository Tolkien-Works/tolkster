import assert from "node:assert/strict";
import { test } from "node:test";
import { newCoordinator } from "../src/worker/machine/coordinator.ts";
import { newSession, newStore } from "../src/worker/machine/ctx.ts";
import { build, type Frame, type QueuedView, type SlotView } from "../src/worker/machine/frame.ts";
import { newTriage } from "../src/worker/machine/triage.ts";
import type { Store, WorkMachine } from "../src/worker/machine/types.ts";
import { publish } from "../src/worker/realtime.ts";
import contract from "./contract.json" with { type: "json" };
import { keys } from "./keys.ts";
import { MemoryStorage } from "./storage.ts";

// The contract with the CLI: test/contract.json has a frame, which cli/src/panel/frame.rs reads.
test("the contract fixture's frame has the frame's fields", () => {
  assert.deepEqual(Object.keys(contract.frame).sort(), keys<Frame>({ type: true, cap: true, hosts: true, slots: true, queue: true, parked: true, chains: true, triage: true, why: true }));
  assert.deepEqual(Object.keys(contract.frame.slots[0]).sort(), keys<SlotView>({ issue: true, identifier: true, reserved: true, sinceMs: true, host: true, work: true, background: true, deadline: true, pr: true, anomaly: true }));
  assert.deepEqual(Object.keys(contract.frame.queue[0]).sort(), keys<QueuedView>({ issue: true, identifier: true, waiting: true, blockers: true, at: true }));
});

function work(state: WorkMachine["state"], more: Partial<WorkMachine> = {}): WorkMachine {
  return { state, version: 1, deadlines: [], stop: null, launch: null, stalled: false, requestedBy: null, prEvaluated: { version: 0 }, unknownRechecks: 0, pausedSinceMs: null, cancelledMs: null, ...more };
}

test("the frame shows a reserved slot, the queue in its turn, each wait, parked work and a chain", () => {
  const c = newCoordinator();
  c.grants["i-1"] = { session: null, sinceMs: 100 };
  c.grants["i-2"] = { session: "s-2", sinceMs: 200 };
  c.grants["i-10"] = { session: null, sinceMs: 300 };
  c.queue = [
    { issue: "i-4", session: "s-4", priority: 0, at: "2026-09-26T01:00:00.000Z" },
    { issue: "i-3", session: "s-3", priority: 2, at: "2026-09-26T01:05:00.000Z" },
  ];
  const running: Store = { ...newStore("i-1"), work: work("running", { deadlines: [{ kind: "own", version: 1, dueMs: 900 }, { kind: "recheck", version: 1, dueMs: 500 }] }) };
  running.facts.pr = { number: 7, head: "h", repo: "o/r", status: "open", at: "2026-09-26T01:00:00.000Z", receivedMs: 0 };
  running.facts.checks.h = { value: "failed", at: "2026-09-26T01:00:00.000Z" };
  running.facts.notices.push("roundsCap:7:h");
  running.facts.worktree = { id: "tolkster-1@r1::/srv/tw-1", branch: "b", version: 1 };
  running.facts.agent = { status: "background", sinceMs: 100 };
  // On this machine: no host.
  const finished: Store = { ...newStore("i-10"), work: work("running") };
  finished.facts.worktree = { id: "r1::D:/wt/tw-10", branch: "b", version: 1 };
  finished.facts.issueState = { value: "completed", at: "2026-09-26T01:00:00.000Z", changedAt: "2026-09-26T01:00:00.000Z" };
  const blocked = newStore("i-5");
  blocked.sessions["s-5"] = { ...newSession(), machine: { state: "queued", version: 1, deadlines: [], stop: null } };
  blocked.sessions["s-5"].facts.request = { kind: "start", at: "2026-09-26T01:02:00.000Z" };
  blocked.sessions["s-5"].facts.waiting = "blockers";
  blocked.facts.blockers = { sentMs: 0, ids: ["b-1", "b-2", "b-3"], states: { "b-2": { done: true, at: "x" } }, more: false };
  const exited: Store = { ...newStore("i-6"), work: work("exited", { stalled: true }) };
  exited.facts.pr = { number: 8, head: "k", repo: "o/r", status: "open", at: "2026-09-26T01:00:00.000Z", receivedMs: 0 };
  // At rest with its PR open, and a read showing auto-merge off: the owner's steps hold it.
  const held: Store = { ...newStore("i-11"), work: work("rest") };
  held.facts.pr = { number: 9, head: "m", repo: "o/r", status: "open", at: "2026-09-26T01:00:00.000Z", receivedMs: 0 };
  held.facts.autoMerge = { pr: 9, on: false };
  const done: Store = { ...newStore("i-8"), work: work("rest") };
  const parent = newStore("i-7");
  parent.facts.chained = { run: "run-1", issues: ["i-9"], asked: ["i-9", "i-10"], answered: ["i-9"], haltedAt: null };
  const stores = new Map([running, finished, blocked, exited, held, done, parent].map((s) => [s.issue, s]));
  const frame = build(c, { ...newTriage(), state: "retrying", deadline: { version: 1, dueMs: 50 }, failures: 2 }, stores, new Map([["i-1", "TW-1"], ["b-1", "TW-11"], ["b-2", "TW-12"]]), { held: 0, hosts: [{ id: "tolkster-1", name: "tolkster-1", cap: 3, admitted: true, answering: true, ready: ["github:o/r"], held: 1 }], projects: {} });

  assert.deepEqual(frame.slots, [
    { issue: "i-1", identifier: "TW-1", reserved: false, sinceMs: 100, host: "tolkster-1", work: "running", background: true, deadline: { kind: "recheck", dueMs: 500 }, pr: { number: 7, checks: "failed", mergeState: null, rounds: 0, capped: true, autoMerge: null }, anomaly: false },
    // Reserved before its worker exists: no store's work yet.
    { issue: "i-2", identifier: null, reserved: true, sinceMs: 200, host: null, work: null, background: false, deadline: null, pr: null, anomaly: false },
    // Held for an issue the store has seen completed.
    { issue: "i-10", identifier: null, reserved: false, sinceMs: 300, host: null, work: "running", background: false, deadline: null, pr: null, anomaly: true },
  ]);
  // Priority 2 before none; then the waits that aren't a slot's, naming only known unfinished blockers.
  assert.deepEqual(frame.queue.map((q) => [q.issue, q.waiting, q.blockers]), [["i-3", "slot", []], ["i-4", "slot", []], ["i-5", "blockers", ["TW-11"]]]);
  // A rest with no PR open is at rest for good.
  assert.deepEqual(frame.parked.map((p) => [p.issue, p.work, p.stalled, p.pr?.checks, p.pr?.autoMerge]), [["i-6", "exited", true, "pending", null], ["i-11", "rest", false, "pending", false]]);
  assert.deepEqual(frame.chains, [{ issue: "i-7", identifier: null, run: "run-1", issues: 2, answered: 1 }]);
  assert.deepEqual(frame.triage, { state: "retrying", dueMs: 50, failures: 2 });
  assert.deepEqual(frame.hosts, [{ id: "tolkster-1", name: "tolkster-1", cap: 3, answering: true }]);
});

test("a chain is listed only while its parent is in flight", () => {
  const chained = (issue: string): Store => {
    const s = newStore(issue);
    s.facts.chained = { run: "run-1", issues: ["c-1"], asked: ["c-1"], answered: [], haltedAt: null };
    return s;
  };
  const at = "2026-09-26T01:00:00.000Z";
  const completed = chained("p-done");
  completed.facts.issueState = { value: "completed", at, changedAt: at };
  const cancelled = chained("p-cancelled");
  cancelled.facts.issueState = { value: "cancelled", at, changedAt: at };
  const finished = chained("p-finished");
  finished.facts.children = { sentMs: 0, ids: ["c-1"], states: { "c-1": { done: true, at } }, more: false };
  const open = chained("p-open");
  open.facts.children = { sentMs: 0, ids: ["c-1", "c-2"], states: { "c-1": { done: true, at } }, more: false };
  const stores = new Map([completed, cancelled, finished, open].map((s) => [s.issue, s]));
  assert.deepEqual(build(newCoordinator(), newTriage(), stores, new Map()).chains.map((c) => c.issue), ["p-open"]);
});

class FakeSocket {
  sent: string[] = [];
  send(message: string): void {
    this.sent.push(message);
  }
}

test("a frame is sent when it changes, not when it doesn't, and always to a fresh connection, apart from the waiting head", async () => {
  const storage = new MemoryStorage<unknown>();
  const [held, fresh] = [new FakeSocket(), new FakeSocket()];
  const frame = build(newCoordinator(), newTriage(), new Map(), new Map());
  assert.equal(await publish(storage, [held], frame), 1);
  assert.equal(await publish(storage, [held], frame), 0);
  assert.equal(await publish(storage, [held, fresh], frame, fresh), 1);
  assert.equal(fresh.sent.length, 1);
  assert.equal(await publish(storage, [held, fresh], { ...frame, cap: 2 }), 2);
  assert.equal(held.sent.length, 2);
  assert.equal(await publish(storage, [held], { ...frame, cap: 2 }), 0);
});
