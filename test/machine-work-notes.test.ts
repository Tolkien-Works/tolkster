import assert from "node:assert/strict";
import { test } from "node:test";
import { toWire } from "../src/worker/route.ts";
import { B, apply, deadline, given, ev, HOUR, ISSUE, MIN, NOW, P, result, S, T0 } from "./machine-fixture.ts";
import { fresh, pr, readOf } from "./machine-work-fixture.ts";

// A note's thread, whole: the reply is read, its request machine begins, and its decision
// opens a turn in the thread whatever the issue or its worker is doing.
function noteDecided(over: Parameters<typeof given>[0], read: object = {}, kind = "noteReply"): ReturnType<typeof apply> {
  const sent = apply(given({ work: "reaped", ...over, queue: [{ kind: "readComment", machine: "work", params: { comment: B, at: fresh } }] }), result(`${ISSUE}.1`, "ok", { comment: { kind, newestOwnerAt: fresh, state: "open", root: S } }));
  return apply(sent.store, result(sent.store.queue.find((c) => c.kind === "readIssue")!.id, "ok", readOf(read)));
}

test("work: a note's reply opens its thread's turn with no worker told, whatever the issue's state", () => {
  for (const [name, over, read] of [
    ["no worker, undelegated", {}, { delegate: null }],
    ["a running worker", { work: "running" as const }, {}],
    ["a finished issue", { work: "rest" as const, facts: { pr: pr() } }, { state: "completed" }],
    ["a blocked issue", {}, { blockers: [{ id: P, done: false, updatedAt: T0 }] }],
  ] as const) {
    const o = noteDecided(over, read);
    assert.equal(o.req, "claimed", name);
    assert.deepEqual(o.commands, ["startTurn"], name);
    assert.equal(o.store.queue.find((c) => c.kind === "startTurn")!.params.thread, true, name);
    assert.ok(!o.out.includes("slotHeld"), name);
  }
});

test("work: the owner's comment on Tolkster's issue opens its thread's turn at rest, and a running worker's line instead", () => {
  for (const [name, over] of [["no worker", {}], ["at rest", { work: "rest" as const }]] as const) {
    const o = noteDecided(over, {}, "noteRoot");
    assert.equal(o.req, "claimed", name);
    assert.deepEqual(o.commands, ["startTurn"], name);
  }
  const told = apply(given({ work: "running", queue: [{ kind: "readComment", machine: "work", params: { comment: B, at: fresh } }] }), result(`${ISSUE}.1`, "ok", { comment: { kind: "noteRoot", newestOwnerAt: fresh, state: "open", root: S } }));
  assert.equal(told.req, undefined);
  assert.deepEqual(told.commands, ["deliverMessage"]);
  const held = apply(given({ work: "held", queue: [{ kind: "readComment", machine: "work", params: { comment: B, at: fresh } }] }), result(`${ISSUE}.1`, "ok", { comment: { kind: "noteRoot", newestOwnerAt: fresh, state: "open", root: S } }));
  assert.equal(held.req, undefined);
});

test("work: a note's turn that marks its request read with no reply is done with no notice; one with neither stalls", () => {
  const decided = noteDecided({}, {}, "noteRoot");
  const started = apply(decided.store, result(decided.store.queue.find((c) => c.kind === "startTurn")!.id, "ok", { terminal: "term_1" }));
  const shown = apply(started.store, ev("workerRead", { session: S, reader: "turn", through: fresh }));
  const marked = apply(shown.store, ev("workerAnswered", { session: S, reader: "turn", at: new Date(NOW).toISOString() }));
  const ended = apply(marked.store, ev("turnEnded", { session: S }));
  assert.equal(ended.req, "done");
  assert.ok(!ended.store.queue.some((c) => c.kind === "postNotice"));
  const silent = apply(shown.store, ev("turnEnded", { session: S }));
  assert.equal(silent.req, "done");
  assert.equal(silent.store.queue.find((c) => c.kind === "postNotice")!.params.notice, "stalled");
});

test("work: a note's thread's turn and its notices carry thread, so listen acts in the thread", () => {
  const decided = noteDecided({});
  const started = apply(decided.store, result(decided.store.queue.find((c) => c.kind === "startTurn")!.id, "ok"));
  const ended = apply(started.store, ev("turnEnded", { session: S }));
  assert.equal(ended.req, "done");
  const notice = ended.store.queue.find((c) => c.kind === "postNotice")!;
  assert.equal(notice.params.notice, "stalled");
  assert.equal(notice.params.thread, true);
  // The request its turn was for, whose reply's answer the notice is.
  assert.equal(notice.params.through, fresh);
});

test("work: a note's thread's turn keeps the terminal its startTurn returned, and a reply meanwhile is the next turn's", () => {
  const decided = noteDecided({});
  const started = apply(decided.store, result(decided.store.queue.find((c) => c.kind === "startTurn")!.id, "ok", { terminal: "term_1" }));
  assert.equal(started.store.sessions[S].facts.turn!.terminal, "term_1");
  const looked = apply(started.store, deadline(started.store, "request"));
  assert.deepEqual(looked.commands, ["observe"]);
  const seen = apply(looked.store, result(looked.store.queue.find((c) => c.kind === "observe")!.id, "ok"));
  // A reply while it runs is decided again when it ends, as a session's is.
  const later = new Date(NOW - 30_000).toISOString();
  const prompted = apply(seen.store, ev("ownerPrompted", { session: S, byOwner: true, at: later }));
  const kept = apply(prompted.store, result(prompted.store.queue.find((c) => c.kind === "readIssue")!.id, "ok", readOf()));
  assert.equal(kept.req, "claimed");
  assert.deepEqual(kept.commands, []);
  // It answers after the reply came: the reply is still the next turn's, from what the turn was shown.
  const shown = apply(kept.store, ev("workerRead", { session: S, reader: "turn", through: fresh }));
  const answered = apply(shown.store, ev("workerAnswered", { session: S, reader: "turn", at: new Date(NOW).toISOString() }));
  const ended = apply(answered.store, ev("turnEnded", { session: S }));
  assert.equal(ended.req, "requested");
  const next = apply(ended.store, result(ended.store.queue.find((c) => c.kind === "readIssue")!.id, "ok", readOf()));
  const start = next.store.queue.find((c) => c.kind === "startTurn")!;
  assert.deepEqual([start.params.request, toWire(start, next.store, "TW-1").args.after], [later, fresh]);
});

test("work: every owner comment is read, so a reply covered by commentTold still reaches its thread", () => {
  const o = apply(given({ work: "running", facts: { commentTold: fresh } }), ev("ownerCommented", { comment: B, at: fresh }));
  assert.deepEqual(o.commands, ["readComment"]);
});

test("work: an issue with no work machine gets one at reaped for a comment's read", () => {
  const o = apply(given(), ev("ownerCommented", { comment: B, at: fresh }));
  assert.equal(o.work, "reaped");
  assert.deepEqual(o.commands, ["readComment"]);
  assert.deepEqual(o.out, []);
});

test("work: a reply to a note a week old or more is not answered", () => {
  const stale = new Date(NOW - 8 * 24 * HOUR).toISOString();
  const o = apply(given({ queue: [{ kind: "readComment", machine: "work", params: { comment: B, at: stale } }] }), result(`${ISSUE}.1`, "ok", { comment: { kind: "noteReply", newestOwnerAt: stale, state: "open", root: S } }));
  assert.equal(o.req, undefined);
});

test("work: a later reply while its thread's turn runs is decided again when the turn ends", () => {
  const first = noteDecided({});
  const again = new Date(NOW - 60_000).toISOString();
  const store = structuredClone(first.store);
  store.queue = [{ id: `${ISSUE}.9`, issue: ISSUE, kind: "readComment", machine: "work", version: 1, params: { comment: B, at: again }, class1: false, firstSentMs: NOW - MIN, sentMs: NOW - MIN, sends: 1, superseded: false }];
  const prompted = apply(store, result(`${ISSUE}.9`, "ok", { comment: { kind: "noteReply", newestOwnerAt: again, state: "open", root: S } }));
  assert.equal(prompted.req, "requested");
  assert.deepEqual(prompted.commands, ["readIssue"]);
  const decided = apply(prompted.store, result(prompted.store.queue.find((c) => c.kind === "readIssue")!.id, "ok", readOf()));
  assert.equal(decided.req, "claimed");
  assert.deepEqual(decided.commands, []);
});

test("work: a second failed read of a comment is not made again", () => {
  const o = apply(given({ work: "running", queue: [{ kind: "readComment", machine: "work", params: { comment: B, at: fresh, again: true } }] }), result(`${ISSUE}.1`, "failed"));
  assert.deepEqual(o.commands, []);
});

test("work: a read of a comment waiting its turn survives a class 1 event that moves the work", () => {
  const store = given({ work: "running" });
  const o = apply(store, ev("ownerCommented", { comment: B, at: fresh }));
  const queued = structuredClone(o.store);
  queued.queue.forEach((c) => (c.sentMs = null));
  const stopped = apply(queued, ev("delegateChanged", { by: null, at: new Date(NOW).toISOString() }));
  assert.equal(stopped.work, "halted");
  assert.ok(stopped.store.queue.some((c) => c.kind === "readComment"));
});
