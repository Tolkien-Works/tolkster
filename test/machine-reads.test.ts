import assert from "node:assert/strict";
import { test } from "node:test";
import { newStore } from "../src/worker/machine/ctx.ts";
import type { Event, IssueRead, PrRead, Store } from "../src/worker/machine/types.ts";
import { apply, B, ev, given, HOUR, ISSUE, landRead, MIN, NOW, result, S, S2, SHA, T0, T1, type Given, type Outcome } from "./machine-fixture.ts";

// A read lands as the events it shows: a lost webhook a read repairs ends where the webhook,
// followed by the same read, would have.

const issueRead = (over: Partial<IssueRead> = {}): IssueRead => ({ updatedAt: T0, delegate: "owner", state: "open", triage: false, stateChangedAt: T0, priority: 0, blockers: [], moreBlockers: false, parent: null, children: [], moreChildren: false, repo: "repo", sessions: [], ...over });
const SHA2 = "b".repeat(40);
const pr = { number: 7, head: SHA, repo: "repo", status: "open" as const, at: T0, receivedMs: NOW - HOUR };
const prRead = (over: Partial<PrRead>): PrRead => ({ number: 7, status: "open", at: T0, head: SHA, mergeState: "CLEAN", base: SHA2, autoMergeOff: false, manualMerge: false, checks: "passed", feedback: null, copilotWontApprove: false, maxRounds: 3, ...over });

// The work machine's evaluatePr result landing, the read first sent a minute ago.
function landPr(store: Store, read: PrRead): Outcome {
  const s = structuredClone(store);
  s.commandSeq += 1;
  const id = `${s.issue}.${s.commandSeq}`;
  s.queue.push({ id, issue: s.issue, kind: "evaluatePr", machine: "work", version: s.work!.version, params: { version: s.facts.prVersion ?? 0 }, class1: false, firstSentMs: NOW - MIN, sentMs: NOW - MIN, sends: 1, superseded: false });
  return apply(s, result(id, "ok", { pr: read }));
}

// The store but its command counter, in the commands' IDs and a stop owed's: the read's own
// command takes another place in the count when the webhook's transition queued commands
// before it.
function held(store: Store): Store {
  const s = structuredClone(store);
  s.commandSeq = 0;
  for (const c of s.queue) c.id = "";
  for (const m of [s.work, ...Object.values(s.sessions).map((x) => x.machine)]) if (m?.stop) m.stop.command = "";
  return s;
}

function repaired(g: Given, webhook: Event, lands: (s: Store) => Outcome): Store {
  const lost = lands(given(g)).store;
  const delivered = lands(apply(given(g), webhook).store).store;
  assert.deepEqual(held(lost), held(delivered));
  return lost;
}

test("a read repairs a lost delegateChanged as the webhook would have", () => {
  const s = repaired({ work: "running", req: "claimed" }, ev("delegateChanged", { by: "other", at: T1 }), (s) => landRead(s, issueRead({ updatedAt: T1, delegate: "other" })));
  assert.equal(s.facts.delegate?.value, "other");
  assert.notEqual(s.facts.halted, null);
});

test("a read repairs a lost issueStateChanged as the webhook would have", () => {
  const s = repaired({ work: "running", req: "done" }, ev("issueStateChanged", { state: "cancelled", triage: false, parent: null, at: T1 }), (s) => landRead(s, issueRead({ updatedAt: T1, state: "cancelled" })));
  assert.equal(s.facts.issueState?.value, "cancelled");
  assert.equal(s.work?.state, "halted");
});

test("a read repairs a lost blockersChanged as the webhook would have", () => {
  const blockers = { more: false, sentMs: NOW - HOUR, ids: [B], states: { [B]: { done: false, at: T0 } } };
  const s = repaired({ req: "queued", sess: { waiting: "blockers" }, facts: { blockers } }, ev("blockersChanged", { blocker: B, done: true, at: T1 }), (s) => landRead(s, issueRead({ blockers: [{ id: B, done: true, updatedAt: T1 }] })));
  assert.equal(s.sessions[Object.keys(s.sessions)[0]].machine?.state, "requested");
  assert.ok(s.queue.some((c) => c.kind === "readIssue"));
});

test("a read repairs a lost prMerged or prClosed as the webhook would have", () => {
  const merged = repaired({ work: "rest", facts: { pr } }, ev("prMerged", { pr: 7, at: T1 }), (s) => landPr(s, prRead({ status: "merged", at: T1 })));
  assert.deepEqual([merged.facts.pr?.status, merged.work?.state], ["merged", "merged"]);
  // The merge it shows names the PR's head, as the webhook's does, past a push that was lost.
  const moved = landPr(given({ work: "rest", facts: { pr } }), prRead({ status: "merged", head: "f".repeat(40), at: T1 })).store;
  assert.deepEqual([moved.facts.pr?.status, moved.facts.pr?.head], ["merged", "f".repeat(40)]);
  const closed = repaired({ work: "rest", facts: { pr } }, ev("prClosed", { pr: 7, at: T1 }), (s) => landPr(s, prRead({ status: "closed", at: T1 })));
  assert.equal(closed.facts.pr?.status, "closed");
  assert.ok(closed.facts.notices.some((n) => n.startsWith("prClosed:")));
});

test("a read that drops an unfinished blocker from the set, which no webhook carries, still reaches the blocked request", () => {
  const blockers = { more: false, sentMs: NOW - HOUR, ids: [B], states: { [B]: { done: false, at: T0 } } };
  const o = landRead(given({ req: "queued", sess: { waiting: "blockers" }, facts: { blockers } }), issueRead());
  assert.deepEqual([o.req, o.commands, o.out], ["requested", ["readIssue"], ["blockersRead"]]);
});

test("a mention whose read is first to show the issue finished is decided as a conversation, not refused by its rows", () => {
  const s = given({ req: "requested", kind: "mention", sess: { request: { kind: "start", at: T0 } }, queue: [{ kind: "readIssue", machine: S, params: { for: T0 } }] });
  const o = apply(s, result(s.queue[0].id, "ok", { read: issueRead({ updatedAt: T1, state: "completed", sessions: [{ id: S, kind: "mention", createdAt: T0 }] }) }));
  assert.deepEqual([o.req, o.commands], ["claimed", ["startTurn"]]);
});

test("an owner's delegation on a store that keeps no session reads the issue, and begins on the session rule 3 asks", () => {
  // Reset: its counters kept. A store that never sent a command waits for the session's own start.
  const reset = { ...newStore(ISSUE), rev: 5, commandSeq: 7 };
  assert.deepEqual(apply(newStore(ISSUE), ev("delegateChanged", { by: "owner", at: T1 })).commands, []);
  assert.deepEqual(apply(reset, ev("delegateChanged", { by: "other", at: T1 })).commands, []);
  const asked = apply(reset, ev("delegateChanged", { by: "owner", at: T1 }));
  assert.deepEqual(asked.commands, ["readIssue"]);
  const read = asked.store.queue[0];
  assert.equal(read.machine, "issue");
  // A read that fails goes once more, and no further.
  const again = apply(asked.store, result(read.id, "failed"));
  assert.deepEqual(again.commands, ["readIssue"]);
  assert.deepEqual(apply(again.store, result(again.store.queue[0].id, "failed")).commands, []);
  const sessions = [{ id: S, kind: "delegation" as const, createdAt: T0 }, { id: S2, kind: "mention" as const, createdAt: T1 }];
  const o = apply(asked.store, result(read.id, "ok", { read: issueRead({ updatedAt: T1, sessions }) }));
  assert.equal(o.store.sessions[S2]?.machine ?? null, null);
  assert.deepEqual(o.store.sessions[S].facts.request, { kind: "delegation", at: T1 });
  // Decided by a read of its own, sent for the delegation.
  assert.equal(o.req, "requested");
  assert.deepEqual(o.store.queue.map((c) => [c.kind, c.machine, c.params.for]), [["readIssue", S, T1]]);
  // A session the read lists that hasn't started, no older than the delegation, is the delegation's own still to come.
  const own = apply(asked.store, result(read.id, "ok", { read: issueRead({ updatedAt: T1, sessions: [{ id: S, kind: "delegation", createdAt: T1 }] }) }));
  assert.equal(own.store.sessions[S]?.machine ?? null, null);
});
