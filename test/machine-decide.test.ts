import assert from "node:assert/strict";
import { test } from "node:test";
import { ALONE, coordinate, newCoordinator, reconcileSlots, type Room } from "../src/worker/machine/coordinator.ts";
import { newStore } from "../src/worker/machine/ctx.ts";
import { newCommand } from "../src/worker/machine/queue.ts";
import { slotMoved } from "../src/worker/machine/work.ts";
import { receive } from "../src/worker/machine/step.ts";
import { repoPushed } from "../src/worker/machine/store.ts";
import type { Event } from "../src/worker/machine/types.ts";
import { apply, B, ev, given, HOUR, ISSUE, NOW, P, read, readIssue, result, S, S2, SHA, T0, T1, T2 } from "./machine-fixture.ts";
import { mergeEvent } from "../src/worker/machine/facts.ts";

// ---- The decision ----

function decide(g: Parameters<typeof given>[0], over: object = {}) {
  return apply(given({ req: "requested", queue: [readIssue], ...g }), result(`${ISSUE}.1`, "ok", read(over)));
}

test("rule 1: a read sent for an older request decides nothing", () => {
  const o = decide({ sess: { request: { kind: "prompt", at: T1 } } });
  assert.equal(o.req, "requested");
  assert.deepEqual(o.commands, []);
});

test("rule 2: a request that isn't the owner's is refused, saying nothing", () => {
  const o = decide({ sess: { byOwner: false } });
  assert.equal(o.req, "done");
  assert.deepEqual(o.commands, []);
});

test("rule 2: a chain's delegation stands only while the parent, as read, is delegated", () => {
  const P = "99999999-9999-4999-8999-999999999999";
  const chain = { kind: "chain" as const, sess: { byOwner: false }, facts: { chain: { parent: P, run: "r1" } } };
  const parent = (delegated: boolean) => ({ delegate: "chain", parent: { id: P, delegated, by: "owner", state: "open", priority: 0, updatedAt: T1 } });
  assert.equal(decide(chain, parent(true)).req, "queued");
  assert.equal(decide(chain, parent(false)).req, "done");
});

test("rules 6 and 7: work needs the delegation and an unfinished issue", () => {
  const never = decide({ facts: { everDelegated: false, delegate: null } }, { delegate: null });
  assert.deepEqual([never.req, never.commands], ["done", ["postNotice:notDelegated"]]);
  const taken = decide({}, { delegate: null });
  assert.deepEqual([taken.req, taken.commands], ["done", ["postNotice:noLongerMine"]]);
  const done = decide({}, { state: "cancelled" });
  assert.deepEqual([done.req, done.commands], ["done", ["postNotice:cancelled"]]);
});

test("rule 8: a stop after the request supersedes it", () => {
  const o = decide({ work: "halted", facts: { halted: { at: T1, by: "stop" } } });
  assert.equal(o.req, "done");
  // The halt's own stop said "Stopped." in the session that gave the worker its request.
  assert.deepEqual(o.commands, []);
  assert.deepEqual(decide({ work: "halted", w: { requestedBy: { session: S2, at: T0 } }, facts: { halted: { at: T1, by: "stop" } } }).commands, ["postNotice:stopped"]);
});

test("rule 9: the request goes to the issue's worker by its state", () => {
  assert.deepEqual(decide({ work: "running" }).commands, ["deliverMessage"]);
  const held = decide({ work: "held" });
  assert.deepEqual([held.req, held.commands], ["queued", ["postNotice:held"]]);
  assert.equal(decide({ work: "merged" }).req, "queued");
  const atRest = decide({ work: "rest", sess: { grant: { sinceMs: NOW } } });
  assert.deepEqual([atRest.req, atRest.work, atRest.commands], ["claimed", "launching", ["resumeWorker"]]);
});

test("rule 10: the blocker re-check and the repo come before a slot", () => {
  const blocked = decide({}, { blockers: [{ id: B, done: false, updatedAt: T1 }] });
  assert.deepEqual([blocked.req, blocked.store.sessions[S].facts.waiting, blocked.commands], ["queued", "blockers", ["postNotice:blocked"]]);
  const noRepo = decide({}, { repo: null });
  assert.deepEqual([noRepo.req, noRepo.commands], ["done", ["postNotice:noRepo"]]);
  const granted = decide({ sess: { grant: { sinceMs: NOW } } });
  assert.deepEqual([granted.req, granted.work, granted.commands], ["claimed", "launching", ["startWorker"]]);
});

test("rule 11: a mention on an issue not delegated is a conversation, in a turn of its own", () => {
  const o = decide({ kind: "mention", sess: { request: { kind: "start", at: T0 } } }, { delegate: null, sessions: [{ id: S, kind: "mention", createdAt: T0 }] });
  assert.deepEqual([o.req, o.commands], ["claimed", ["startTurn"]]);
  assert.equal(o.store.sessions[S].facts.turn?.for, T0);
});

test("work on an issue with unfinished sub-issues starts an orchestration, recorded on the session", () => {
  const o = decide({ sess: { grant: { sinceMs: NOW }, waiting: "slot" } }, { children: [{ id: B, done: false, updatedAt: T1 }] });
  assert.equal(o.store.queue.find((c) => c.kind === "startWorker")?.params.orchestrate, true);
  assert.deepEqual(o.store.sessions[S].facts.orchestrated, { at: T0, known: [B] });
});

test("rule 6: a delegation no history entry attributes yet waits, a chain's and a mention's included", () => {
  const P = "99999999-9999-4999-8999-999999999999";
  const owner = decide({}, { delegate: "unknown" });
  assert.deepEqual([owner.req, owner.commands], ["requested", []]);
  assert.equal(owner.reqOwn, 30 * 1000);
  const chain = decide({ kind: "chain", sess: { byOwner: false }, facts: { chain: { parent: P, run: "r1" } } }, { delegate: "unknown", parent: { id: P, delegated: true, by: "owner", state: "open", priority: 0, updatedAt: T1 } });
  assert.deepEqual([chain.req, chain.commands], ["requested", []]);
  const mention = decide({ kind: "mention", sess: {} }, { delegate: "unknown" });
  assert.deepEqual([mention.req, mention.commands], ["requested", []]);
});

test("a newer read that can't say who delegated replaces a known delegator", () => {
  const o = decide({ facts: { delegate: { value: "owner", at: T0 } } }, { delegate: "unknown", updatedAt: T1 });
  assert.deepEqual(o.store.facts.delegate, { value: "unknown", at: T1 });
  assert.equal(o.req, "requested");
  // One aged out of the history read keeps it, and the work goes on.
  const aged = decide({ facts: { delegate: { value: "owner", at: T0 } } }, { delegate: "aged", updatedAt: T1 });
  assert.deepEqual(aged.store.facts.delegate, { value: "owner", at: T0 });
  assert.equal(aged.req, "queued");
  const none = decide({ facts: { delegate: null } }, { delegate: "aged", updatedAt: T1 });
  assert.deepEqual([none.store.facts.delegate, none.req], [{ value: "unknown", at: T1 }, "requested"]);
});

test("an issue the owner created delegated, which no entry changed, is theirs only once the wait is over", () => {
  const read = { delegate: "created" as const, updatedAt: T1 };
  const waits = decide({ facts: { delegate: { value: "owner", at: T0 } } }, read);
  assert.deepEqual([waits.store.facts.delegate, waits.req, waits.reqOwn], [{ value: "unknown", at: T1, created: true }, "requested", 30 * 1000]);
  const over = decide({ sess: { unattributed: { for: T0, sinceMs: NOW - HOUR } } }, read);
  assert.deepEqual([over.req, over.store.facts.delegate], ["queued", { value: "owner", at: T1, created: true }]);
  // A later `created` read keeps the owner it was taken for; an `unknown` one doesn't.
  const again = (delegate: string) => decide({ facts: { delegate: { value: "owner", at: T1, created: true } } }, { delegate, updatedAt: T2 }).store.facts.delegate;
  assert.deepEqual([again("created"), again("unknown")], [{ value: "owner", at: T1, created: true }, { value: "unknown", at: T2 }]);
  // At its own clock, any other event corrects it; and a `created` read over someone else's
  // delegation, or a take-away, held is refused after the wait as `unknown` is.
  const s = given({ facts: { delegate: { value: "owner", at: T1, created: true } } });
  assert.equal(mergeEvent(s, ev("delegateChanged", { by: "other", at: T1 }), NOW), "fresh");
  for (const held of ["other", null] as const) {
    const o = decide({ facts: { delegate: { value: held, at: T0 }, everDelegated: true } }, read);
    assert.deepEqual([o.store.facts.delegate, o.req], [{ value: held, at: T0 }, "done"]);
    // Read again at the same clock, as rule 6's wait would, it stays refused.
    const s2 = given({ req: "requested", queue: [{ ...readIssue, id: `${ISSUE}.2` }], facts: o.store.facts, sess: { unattributed: { for: T0, sinceMs: NOW - HOUR } } });
    assert.deepEqual(apply(s2, result(`${ISSUE}.2`, "ok", read)).store.facts.delegate, { value: held, at: T0 });
  }
  // A mention's session is judged as work once it is the owner's.
  const mention = decide({ kind: "mention", sess: { unattributed: { for: T0, sinceMs: NOW - HOUR } } }, { ...read, sessions: [{ id: S, kind: "mention", createdAt: T0 }] });
  assert.equal(mention.req, "queued");
  // Without it, the wait refuses it as anyone else's.
  const unknown = decide({ sess: { unattributed: { for: T0, sinceMs: NOW - HOUR } } }, { ...read, delegate: "unknown" });
  assert.equal(unknown.req, "done");
});

// ---- Facts ----

test("a fact whose clock isn't newer is dropped, and an event with nothing newer runs no transition", () => {
  const s = given({ req: "queued" });
  const stale = apply(s, ev("delegateChanged", { by: null, at: T0 }));
  assert.equal(stale.req, "queued");
  assert.deepEqual(stale.commands, []);
});

test("a stale read can't put back a blocker its webhook finished", () => {
  const s = given({ req: "queued", sess: { waiting: "blockers" }, facts: { blockers: { more: false, sentMs: NOW - HOUR, ids: [B], states: { [B]: { done: true, at: T2 } } } }, queue: [{ ...readIssue, firstSentMs: NOW - 2 * HOUR }] });
  const o = apply(s, result(`${ISSUE}.1`, "ok", read({ blockers: [{ id: B, done: false, updatedAt: T1 }] })));
  assert.equal(o.store.facts.blockers?.states[B].done, true);
});

test("at one GitHub second, a PR's later kind of event wins, and a higher-numbered PR over another", () => {
  const SHA2 = "b".repeat(40);
  const merge = (...events: Event[]) => {
    const s = given();
    return events.map((e): string => mergeEvent(s, e, NOW)).concat(s.facts.pr === null ? [] : [`${s.facts.pr.number}:${s.facts.pr.status}:${s.facts.pr.head}`]);
  };
  const opened = (pr: number, at = T1) => ev("prOpened", { pr, head: SHA, repo: "o/r", at });
  const pushed = (at = T1) => ev("prPushed", { pr: 7, head: SHA2, before: SHA, at });
  // A merged PR stays merged: a close, a push or a reopen after it, in its second or later.
  assert.deepEqual(merge(opened(7, T0), ev("prMerged", { pr: 7, at: T1 }), ev("prClosed", { pr: 7, at: T1 }), ev("prClosed", { pr: 7, at: T2 }), pushed(T2), opened(7, T2)), ["fresh", "fresh", "stale", "stale", "stale", "stale", `7:merged:${SHA}`]);
  // A merge outranks a close in its second; a push and a reopen are outranked by a close.
  assert.deepEqual(merge(opened(7, T0), ev("prClosed", { pr: 7, at: T1 }), ev("prMerged", { pr: 7, at: T1 })), ["fresh", "fresh", "fresh", `7:merged:${SHA}`]);
  assert.deepEqual(merge(opened(7, T0), ev("prClosed", { pr: 7, at: T1 }), pushed(), opened(7)), ["fresh", "fresh", "stale", "stale", `7:closed:${SHA}`]);
  // A push outranks an open, in either order.
  assert.deepEqual(merge(opened(7), pushed()), ["fresh", "fresh", `7:open:${SHA2}`]);
  assert.deepEqual(merge(pushed(), opened(7)), ["stale", "fresh", `7:open:${SHA}`]);
  // The newer PR, by its number, in either order; another repo's is no newer.
  assert.deepEqual(merge(ev("prClosed", { pr: 7, at: T1 }), opened(7, T0), ev("prClosed", { pr: 7, at: T1 }), opened(8)), ["stale", "fresh", "fresh", "fresh", `8:open:${SHA}`]);
  assert.deepEqual(merge(opened(8), opened(7)), ["fresh", "stale", `8:open:${SHA}`]);
  assert.deepEqual(merge(opened(7), ev("prOpened", { pr: 8, head: SHA, repo: "o/other", at: T1 })), ["fresh", "stale", `7:open:${SHA}`]);
  // A merge names the head it merged, which a lost push left behind; one stored before it
  // carried the head keeps the held one.
  assert.deepEqual(merge(opened(7, T0), ev("prMerged", { pr: 7, head: SHA2, at: T1 })), ["fresh", "fresh", `7:merged:${SHA2}`]);
  // A merged PR keeps no other repo's PR of its number out.
  assert.deepEqual(merge(opened(7, T0), ev("prMerged", { pr: 7, at: T0 }), ev("prOpened", { pr: 7, head: SHA2, repo: "o/other", at: T1 })), ["fresh", "fresh", "fresh", `7:open:${SHA2}`]);
});

test("feedback or a check older than the head's held one is stale; one in its second reads the PR again", () => {
  const s = given();
  const reviewed = (copilot: boolean, at: string) => ev("prReviewed", { pr: 7, sha: SHA, reviewer: true, copilot, at });
  const checked = (result: "passed" | "failed", at: string) => ev("checksCompleted", { sha: SHA, result, at });
  // Copilot's newer review arrives first; a reviewer's older one after it was read by the
  // `evaluatePr` Copilot's asked for, while one in its second may have come after that read.
  assert.deepEqual([reviewed(true, T2), reviewed(false, T1), reviewed(false, T2)].map((e) => mergeEvent(s, e, NOW)), ["fresh", "stale", "fresh"]);
  assert.deepEqual(s.facts.feedback[SHA], { at: T2, copilot: false });
  assert.deepEqual([checked("passed", T2), checked("failed", T1), checked("failed", T2)].map((e) => mergeEvent(s, e, NOW)), ["fresh", "stale", "fresh"]);
});

test("a change of state the machines can't see, under the same parent, keeps its clock and tells no one", () => {
  const s = given();
  const changed = (state: "open" | "completed", at: string, parent: string | null = null) => ev("issueStateChanged", { state, parent, at });
  assert.equal(mergeEvent(s, changed("open", T1), NOW), "stale");
  assert.equal(s.facts.issueState?.at, T1);
  assert.equal(mergeEvent(s, changed("open", T1), NOW), "stale");
  assert.equal(mergeEvent(s, changed("open", T2, P), NOW), "moved");
  assert.equal(mergeEvent(s, changed("completed", "2026-10-01T03:00:00.000Z", P), NOW), "fresh");
});

// ---- Precedence and the command queue ----

test("with a command in flight, an observation waits; it applies once the queue empties", () => {
  const s = given({ work: "running", queue: [{ kind: "observe", machine: "work" }] });
  const waited = apply(s, ev("agentIdle", {}));
  assert.equal(waited.work, "running");
  assert.equal(waited.store.waiting.length, 1);
  const done = apply(waited.store, result(`${ISSUE}.1`, "ok"));
  assert.equal(done.work, "idle");
  assert.equal(done.store.waiting.length, 0);
});

test("waiting events apply by class: the owner's request before an observation, before a deadline", () => {
  let s = given({ req: "claimed", work: "running", queue: [{ kind: "observe", machine: "work" }], armed: [{ machine: "work", kind: "own" }] });
  const order: string[] = [];
  s = receive(s, { kind: "deadline", id: "d", issue: ISSUE, machine: "work", version: 1, deadline: "own" }, NOW).store;
  s = receive(s, ev("agentWaiting", {}), NOW).store;
  s = receive(s, ev("ownerPrompted", { session: S, byOwner: true, at: T1 }), NOW).store;
  for (const w of s.waiting) order.push(`${w.cls}:${w.event.kind}`);
  assert.deepEqual(order, ["5:deadline", "4:agentWaiting", "2:ownerPrompted"]);
  const after = receive(s, result(`${ISSUE}.1`, "ok"), NOW).store;
  // The prompt applied first and queued its read, so the rest wait behind it.
  assert.equal(after.sessions[S].machine?.state, "requested");
  assert.deepEqual(after.waiting.map((w) => w.event.kind), ["deadline", "agentWaiting"]);
});

test("a class 1 stop runs beside the command in flight, drops what was queued and never resends what it overtook", () => {
  const s = given({
    req: "claimed",
    work: "running",
    queue: [{ kind: "deliverMessage", machine: "work" }, { kind: "observe", machine: "work", sentMs: null, firstSentMs: null, sends: 0 }, { kind: "postNotice", machine: "work", sentMs: null, firstSentMs: null, sends: 0, params: { key: "k" } }],
  });
  const r = receive(s, ev("ownerStopped", { session: S, byOwner: true, at: T1 }), NOW);
  assert.equal(r.store.work?.state, "halted");
  assert.deepEqual(r.store.queue.map((c) => c.kind), ["deliverMessage", "postNotice", "stopWorker"]);
  assert.equal(r.store.queue[0].superseded, true);
  assert.deepEqual(r.send.map((c) => c.kind), ["stopWorker"]);
  // The overtaken message lands after the stop: the worker is stopped again.
  const late = receive(r.store, result(`${ISSUE}.1`, "ok"), NOW);
  assert.ok(late.store.queue.filter((c) => c.kind === "stopWorker").length === 2);
});

test("one command is in flight per issue: the next is sent after the first's result", () => {
  const s = given({ work: "running", armed: [{ machine: "work", kind: "own" }] });
  const r = receive(s, { kind: "deadline", id: "d", issue: ISSUE, machine: "work", version: 1, deadline: "own" }, NOW);
  assert.deepEqual(r.send.map((c) => c.kind), ["observe"]);
  const next = receive(r.store, result(r.send[0].id, "ok"), NOW);
  assert.deepEqual(next.send.map((c) => c.kind), ["readIssue"]);
});

test("a deadline armed again, or whose state was left, is dropped", () => {
  const s = given({ work: "running", armed: [{ machine: "work", kind: "own" }] });
  const stale = receive(s, { kind: "deadline", id: "d", issue: ISSUE, machine: "work", version: 0, deadline: "own" }, NOW);
  assert.equal(stale.store, s);
  const left = receive(s, ev("agentIdle", {}), NOW).store;
  const old = receive(left, { kind: "deadline", id: "d", issue: ISSUE, machine: "work", version: 1, deadline: "own" }, NOW);
  assert.equal(old.store, left);
});

test("a result for a command already ended is dropped", () => {
  const s = given({ work: "running" });
  const r = receive(s, result(`${ISSUE}.9`, "ok"), NOW);
  assert.deepEqual(r.store.work, s.work);
});

test("a failed notice goes once more with its key kept, and failing again clears it, so the next event that calls for it says it", () => {
  const s = given({ req: "done", facts: { notices: ["stopped:k"] }, queue: [{ kind: "postNotice", machine: S, params: { notice: "halted", key: "stopped:k" } }] });
  const first = receive(s, result(`${ISSUE}.1`, "failed"), NOW);
  assert.deepEqual(first.store.facts.notices, ["stopped:k"]);
  assert.deepEqual(first.store.queue.map((c) => c.params), [{ notice: "halted", key: "stopped:k", again: true }]);
  const second = receive(first.store, result(first.store.queue[0].id, "lost"), NOW);
  assert.deepEqual(second.store.facts.notices, []);
  assert.deepEqual(second.store.queue, []);
});

// ---- The coordinator ----

test("the last slot goes to one ask: the other waits in the queue, and gets it once given back", () => {
  const c = newCoordinator();
  c.slots = 1;
  const first = coordinate(c, { kind: "slotAsked", id: "a1", issue: "a", session: "1", priority: 0, at: T0 }, NOW, ALONE);
  const second = coordinate(c, { kind: "slotAsked", id: "b1", issue: "b", session: "2", priority: 0, at: T1 }, NOW, ALONE);
  assert.deepEqual(first.map((e) => e.kind), ["slotGranted"]);
  assert.deepEqual(second.map((e) => e.kind), ["slotWaiting"]);
  coordinate(c, { kind: "slotHeld", id: "a2", issue: "a" }, NOW, ALONE);
  const freed = coordinate(c, { kind: "slotReleased", id: "a3", issue: "a", session: null }, NOW, ALONE);
  assert.deepEqual(freed.map((e) => [e.kind, e.issue]), [["slotGranted", "b"]]);
});

test("a waiting request asked again takes its issue's new priority and keeps its age", () => {
  const c = newCoordinator();
  c.slots = 1;
  c.grants.a = { session: null, sinceMs: NOW };
  coordinate(c, { kind: "slotAsked", id: "b1", issue: "b", session: "2", priority: 2, at: T0 }, NOW, ALONE);
  coordinate(c, { kind: "slotAsked", id: "c1", issue: "c", session: "3", priority: 3, at: T1 }, NOW, ALONE);
  // Raised from Medium to Urgent: ahead of the High request waiting longer.
  assert.deepEqual(coordinate(c, { kind: "slotAsked", id: "c2", issue: "c", session: "3", priority: 1, at: T2 }, NOW, ALONE).map((e) => e.kind), ["slotWaiting"]);
  assert.deepEqual(c.queue.find((p) => p.issue === "c"), { issue: "c", session: "3", priority: 1, at: T1, repo: null });
  const freed = coordinate(c, { kind: "slotReleased", id: "a2", issue: "a", session: null }, NOW, ALONE);
  assert.deepEqual(freed.map((e) => [e.kind, e.issue]), [["slotGranted", "c"]]);
  // A re-rank its grant is on its way to gets no second one.
  assert.deepEqual(coordinate(c, { kind: "slotAsked", id: "c3", issue: "c", session: "3", priority: 2, at: T1, rerank: true }, NOW, ALONE), []);
});

test("a priority change asks again for a request waiting for a slot, by its own priority, else its parent's", () => {
  const parent = { id: P, at: T0, delegated: false, by: null, state: "open" as const, parentAt: null };
  const asked = (o: ReturnType<typeof apply>) => o.store.outbox.slice(o.store.outbox.length - o.out.length).filter((x) => x.event.kind === "slotAsked").map((x) => (x.event as { priority: number }).priority);
  const queued = given({ req: "queued", sess: { waiting: "slot" }, facts: { parent } });
  const fromParent = apply(queued, ev("priorityChanged", { priority: 1, parent: P, at: T1 }));
  assert.deepEqual(asked(fromParent), [1]);
  // Another parent's, or an older one, changes nothing.
  assert.deepEqual(apply(fromParent.store, ev("priorityChanged", { priority: 4, parent: B, at: T2 })).out, []);
  assert.deepEqual(apply(fromParent.store, ev("priorityChanged", { priority: 4, parent: P, at: T0 })).out, []);
  // A re-rank leaves what answers the decision's own ask to claim at once.
  assert.equal(fromParent.store.sessions[S].facts.asked, queued.sessions[S].facts.asked);
  // Its own wins over its parent's, and the same priority again is nothing.
  const own = apply(fromParent.store, ev("priorityChanged", { priority: 3, at: T1 }));
  assert.deepEqual(asked(own), [3]);
  assert.deepEqual(apply(own.store, ev("priorityChanged", { priority: 3, at: T2 })).out, []);
});

test("a webhook's move to another parent while a request waits for a slot reads the issue, for its new parent's priority", () => {
  const parent = { id: P, at: T0, delegated: false, by: null, state: "open" as const, parentAt: null, priority: { value: 1, at: T0 } };
  const moved = apply(given({ req: "queued", sess: { waiting: "slot" }, facts: { parent } }), ev("issueStateChanged", { state: "open", parent: B, from: P, at: T1 }));
  assert.deepEqual(moved.commands, ["readIssue"]);
  // Off its parent, ranked again at once by none; with a priority of its own, nothing.
  const off = apply(given({ req: "queued", sess: { waiting: "slot" }, facts: { parent } }), ev("issueStateChanged", { state: "open", parent: null, from: P, at: T1 }));
  assert.deepEqual([off.commands, off.store.outbox.filter((x) => x.event.kind === "slotAsked").map((x) => (x.event as { priority: number }).priority)], [[], [0]]);
  assert.deepEqual(apply(given({ req: "queued", sess: { waiting: "slot" }, facts: { parent, priority: { value: 2, at: T0 } } }), ev("issueStateChanged", { state: "open", parent: B, from: P, at: T1 })).commands, []);
  // A read that first shows it off its parent ranks it again too.
  const offRead = apply(given({ req: "queued", sess: { waiting: "slot" }, facts: { parent } }), { ...ev("issueStateChanged", { state: "open", parent: null, at: T1 }), id: "read" } as Event);
  assert.ok(offRead.store.outbox.some((x) => x.event.kind === "slotAsked"));
  assert.deepEqual(apply(given({ req: "queued", sess: { waiting: "blockers" }, facts: { parent } }), ev("issueStateChanged", { state: "open", parent: B, from: P, at: T1 })).commands, []);
});

test("a parent's own priority change reaches the sub-issues it has read", () => {
  const s = given({ req: "done", facts: { children: { sentMs: NOW - HOUR, ids: [B], states: { [B]: { done: false, at: T0 } }, more: false } } });
  const o = apply(s, ev("priorityChanged", { priority: 1, at: T1 }));
  assert.deepEqual(o.store.outbox.map((x) => [x.to, x.event]), [[B, { kind: "priorityChanged", issue: B, priority: 1, parent: ISSUE, at: T1, id: o.store.outbox[0].id }]]);
  // A parent's priority forwarded to it goes no further.
  assert.deepEqual(apply(given({ req: "done", facts: { parent: { id: P, at: T0, delegated: false, by: null, state: "open", parentAt: null }, children: s.facts.children } }), ev("priorityChanged", { priority: 1, parent: P, at: T1 })).out, []);
});

test("an agent found at work takes a slot even past the cap", () => {
  const c = newCoordinator();
  c.slots = 0;
  coordinate(c, { kind: "slotHeld", id: "a", issue: "a" }, NOW, ALONE);
  assert.deepEqual(c.grants.a, { session: null, sinceMs: NOW });
});

// Each answer's kind, and a grant's host.
const where = (e: Event) => [e.kind, e.kind === "slotGranted" ? e.host : undefined];

// This machine with room for one, and a paired host with room for one, ready for app's repo.
const PAIRED: Room = { cap: 1, held: 0, hosts: [{ id: "mac", cap: 1, held: 0, ready: ["github:acme/app"] }], silent: [], projects: { app: "github:acme/app", web: "github:acme/web" } };

test("a start in flight here fills it: the next goes to a host with room and its repo ready, or waits, never here", () => {
  const c = newCoordinator();
  c.slots = 2;
  const ask = (issue: string, repo: string, room: Room) => coordinate(c, { kind: "slotAsked", id: `${issue}1`, issue, session: issue, priority: 0, at: T0, repo }, NOW, room);
  assert.deepEqual(ask("a", "app", PAIRED).map(where), [["slotGranted", undefined]]);
  // The host isn't ready for web's repo, and here is full: it waits, heading the queue.
  assert.deepEqual(ask("b", "web", PAIRED).map((e) => e.kind), ["slotWaiting"]);
  assert.deepEqual(ask("d", "app", PAIRED).map(where), [["slotGranted", "mac"]]);
  assert.equal(c.grants.d.host, "mac");
  // With no host paired, a repo's worker waits for a machine rather than going here.
  const alone: Room = { ...PAIRED, hosts: [] };
  assert.deepEqual(ask("e", "app", alone).map((e) => e.kind), ["slotWaiting"]);
  // Here's start ends: its room goes to the head, here.
  assert.deepEqual(coordinate(c, { kind: "slotReleased", id: "a2", issue: "a", session: "a" }, NOW, PAIRED).map((e) => [e.issue, ...where(e)]), [["b", "slotGranted", undefined]]);
});

test("a worker found on a host counts there, and a worker's own host takes its resume, or the fleet's room when not open", () => {
  const c = newCoordinator();
  c.slots = 2;
  coordinate(c, { kind: "slotHeld", id: "a1", issue: "a", host: "mac" }, NOW, PAIRED);
  assert.deepEqual(c.grants.a, { session: null, sinceMs: NOW, host: "mac" });
  // Its worktree on mac, which is full: it waits, though here has room.
  const resume = (room: Room) => coordinate(c, { kind: "slotAsked", id: "b1", issue: "b", session: "b", priority: 0, at: T0, repo: "app", on: "mac" }, NOW, room);
  assert.deepEqual(resume(PAIRED).map((e) => e.kind), ["slotWaiting"]);
  // mac silent: the fleet's total has room.
  assert.deepEqual(resume({ ...PAIRED, hosts: [] }).map(where), [["slotGranted", "mac"]]);
  // One whose worktree is here takes here's room.
  assert.deepEqual(coordinate(c, { kind: "slotAsked", id: "d1", issue: "d", session: "d", priority: 0, at: T0, repo: "app", on: null }, NOW, PAIRED).map(where), [["slotGranted", undefined]]);
  // A hold with no worktree keeps where it was reserved; one with a worktree moves there.
  coordinate(c, { kind: "slotHeld", id: "b2", issue: "b" }, NOW, PAIRED);
  assert.equal(c.grants.b.host, "mac");
  coordinate(c, { kind: "slotHeld", id: "b3", issue: "b", host: null }, NOW, PAIRED);
  assert.equal(c.grants.b.host, undefined);
});

test("the blocker index fans a finished blocker out, and an older read doesn't replace a newer one", () => {
  const c = newCoordinator();
  coordinate(c, { kind: "blockersRead", id: "1", issue: "a", sentMs: 20, blockers: [B] }, NOW, ALONE);
  coordinate(c, { kind: "blockersRead", id: "2", issue: "a", sentMs: 10, blockers: [] }, NOW, ALONE);
  const sent = coordinate(c, { kind: "blockerChanged", id: "3", issue: B, done: true, at: T1 }, NOW, ALONE);
  assert.deepEqual(sent.map((e) => [e.kind, e.issue]), [["blockersChanged", "a"]]);
});

test("a push to a repo's default branch reaches each issue with a PR open in it", () => {
  const c = newCoordinator();
  coordinate(c, { kind: "prRecorded", id: "1", issue: "a", repo: "r", open: true }, NOW, ALONE);
  coordinate(c, { kind: "prRecorded", id: "2", issue: "b", repo: "r", open: true }, NOW, ALONE);
  coordinate(c, { kind: "prRecorded", id: "3", issue: "b", repo: "r", open: false }, NOW, ALONE);
  assert.deepEqual(coordinate(c, { kind: "repoPushed", id: "4", repo: "r", base: "abcdef1" }, NOW, ALONE).map((e) => e.issue), ["a"]);
  // A PR in another repo takes the issue out of the first's.
  coordinate(c, { kind: "prRecorded", id: "5", issue: "a", repo: "s", open: true }, NOW, ALONE);
  assert.deepEqual(c.prs, { s: ["a"] });
  coordinate(c, { kind: "prRecorded", id: "6", issue: "a", repo: "r", open: false }, NOW, ALONE);
  assert.deepEqual(c.prs, {});
});

test("a grant whose host closes before its worker reaches it moves to where its repo has room, and its start names the new host", () => {
  const c = newCoordinator();
  c.slots = 2;
  coordinate(c, { kind: "slotAsked", id: "h1", issue: "h", session: "h", priority: 0, at: T0, repo: "app" }, NOW, { ...PAIRED, cap: 0 });
  coordinate(c, { kind: "slotAsked", id: "a1", issue: "a", session: "a", priority: 0, at: T0, repo: "app" }, NOW, { ...PAIRED, hosts: [], silent: ["mac"] });
  assert.deepEqual([c.grants.h.host, c.grants.a.host], ["mac", undefined]);
  // A worker found on mac, and one asked for on its worktree there, stay; h, with no worktree, moves here.
  c.grants.w = { session: null, sinceMs: NOW, host: "mac" };
  const moved = reconcileSlots(c, NOW, { ...PAIRED, cap: 3, hosts: [] });
  assert.deepEqual(moved.map((e) => [e.kind, e.issue, e.kind === "slotMoved" ? e.host : null]), [["slotMoved", "h", undefined]]);
  assert.deepEqual([c.grants.h.host, c.grants.w.host], [undefined, "mac"]);
  // With nowhere to go it stays where it was; on a host only silent, where its start may yet
  // land, too.
  c.grants.h = { session: null, sinceMs: NOW, host: "mac", repo: "app" };
  assert.deepEqual(reconcileSlots(c, NOW, { ...PAIRED, cap: 0, hosts: [] }), []);
  assert.deepEqual(reconcileSlots(c, NOW, { ...PAIRED, cap: 3, hosts: [], silent: ["mac"] }), []);
  // mac answering with the repo no longer ready: it moves, here's one free slot not counting it.
  const unready = reconcileSlots(c, NOW, { ...PAIRED, cap: 2, hosts: [{ id: "mac", cap: 1, held: 0, ready: [] }] });
  assert.deepEqual(unready.map((e) => [e.kind, e.issue]), [["slotMoved", "h"]]);
  c.grants.h = { session: null, sinceMs: NOW, host: "mac", repo: "app" };
  // Held on its worktree's host, it no longer moves.
  coordinate(c, { kind: "slotHeld", id: "h2", issue: "h", host: "mac" }, NOW, PAIRED);
  assert.deepEqual(reconcileSlots(c, NOW, { ...PAIRED, cap: 3, hosts: [] }), []);
  // The issue's start not yet on a worktree, and a reservation, take the new host.
  const s = newStore(ISSUE);
  s.queue.push(newCommand("x.1", ISSUE, "startWorker", "work", 1, { session: S, worktree: null, host: "mac" }, false));
  s.queue.push(newCommand("x.2", ISSUE, "startWorker", "work", 1, { session: S, worktree: "r1::/wt/x", host: null }, false));
  slotMoved(s, "pi");
  assert.deepEqual(s.queue.map((q) => q.params.host), ["pi", null]);
});

