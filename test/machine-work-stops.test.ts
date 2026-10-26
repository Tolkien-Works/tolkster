import assert from "node:assert/strict";
import { test } from "node:test";
import { receive } from "../src/worker/machine/step.ts";
import { apply, check, deadline, given, ev, HOUR, ISSUE, MIN, NOW, result, S, T1, type Case } from "./machine-fixture.ts";
import { cancelledIssue, evaluated, evaluatePr, halted, own, owed, pr } from "./machine-work-fixture.ts";

// The stop owed, and what a stop's results and exits do, beyond the doc's one case per row.

// What `merged` does with a stop still owed, beyond the doc's one case per row.
const owedInMerged: Case[] = [
  { row: "merged agentWorking", when: "Its stop isn't confirmed", given: { work: "merged", facts: { halted }, w: { stop: owed } }, event: ev("agentWorking", {}), expect: { work: "merged", commands: ["stopWorker"], workOwn: 2 * MIN } },
  { row: "merged commandResult", when: "`stopWorker` failed", given: { work: "merged", facts: { halted }, w: { stop: owed }, queue: [{ kind: "stopWorker", machine: "work" }], armed: [{ machine: "work", kind: "own", dueMs: NOW + 2 * MIN }] }, event: result(`${ISSUE}.1`, "failed"), expect: { work: "merged", commands: [], workOwn: 2 * MIN } },
  { row: "launching commandResult", when: "`startRound` failed, its agent without a handle", given: { work: "launching", facts: { pr: pr() }, w: { launch: "round", requestedBy: { session: S, at: T1 } }, queue: [{ kind: "startRound", machine: "work" }] }, event: result(`${ISSUE}.1`, "failed", { unnamed: true }), expect: { work: "rest", workOwn: 10 * MIN, commands: ["postNotice:roundUnnamed"] } },
];
for (const c of owedInMerged) test(`work: ${c.row}, ${c.when}`, () => check(c));

test("a stop or round against an agent with no handle is told once, however often it goes again", () => {
  const unnamed = { unnamed: true };
  const stopped = given({ work: "merged", facts: { halted }, w: { stop: owed, requestedBy: { session: S, at: T1 } }, queue: [{ kind: "stopWorker", machine: "work" }], armed: [{ machine: "work", kind: "own", dueMs: NOW + 2 * MIN }] });
  const first = apply(stopped, result(`${ISSUE}.1`, "failed", unnamed));
  assert.deepEqual(first.commands, ["postNotice:stopUnnamed"]);
  const posted = apply(first.store, result(first.store.queue.find((c) => c.kind === "postNotice")!.id, "ok"));
  const again = apply(posted.store, ev("agentWorking", {}));
  assert.deepEqual(again.commands, ["stopWorker"]);
  assert.deepEqual(apply(again.store, result(again.store.queue.find((c) => c.kind === "stopWorker")!.id, "failed", unnamed)).commands, []);
  const round = () => given({ work: "launching", facts: { pr: pr() }, w: { launch: "round", requestedBy: { session: S, at: T1 } }, queue: [{ kind: "startRound", machine: "work" }] });
  const told = apply(round(), result(`${ISSUE}.1`, "failed", unnamed));
  assert.deepEqual(told.commands, ["postNotice:roundUnnamed"]);
  const retried = given({ work: "launching", facts: { pr: pr(), notices: told.store.facts.notices }, w: { launch: "round", requestedBy: { session: S, at: T1 } }, queue: [{ kind: "startRound", machine: "work" }] });
  assert.deepEqual(apply(retried, result(`${ISSUE}.1`, "failed", unnamed)).commands, []);
});

// The stop's other results and exits, beyond the doc's one case per row.
const resume = { kind: "resumeWorker" as const, machine: "work" };
const stopWorker = { kind: "stopWorker" as const, machine: "work", class1: true };
const stops: Case[] = [
  { row: "halted commandResult", when: "a start that succeeds while the stop is owed sends it again", given: { work: "halted", facts: { halted }, w: { stop: { ...owed, command: `${ISSUE}.2` } }, queue: [resume, stopWorker] }, event: result(`${ISSUE}.1`, "ok"), expect: { work: "halted", commands: ["stopWorker"], workOwn: 2 * MIN } },
  { row: "halted commandResult", when: "the stop succeeded while a start sent before it is in flight", given: { work: "halted", facts: { halted }, w: { stop: { ...owed, command: `${ISSUE}.2` } }, queue: [resume, stopWorker] }, event: result(`${ISSUE}.2`, "ok"), expect: { work: "halted", commands: [], workOwn: 24 * HOUR } },
  { row: "halted commandResult", when: "a start that failed after the stop was confirmed launched nothing", given: { work: "halted", w: { cancelledMs: NOW }, facts: { halted }, queue: [resume] }, event: result(`${ISSUE}.1`, "failed"), expect: { work: "halted", commands: [] } },
  { row: "halted commandResult", when: "an older stop's success confirms nothing", given: { work: "halted", facts: { halted }, w: { stop: { ...owed, command: `${ISSUE}.2` } }, queue: [stopWorker, stopWorker] }, event: result(`${ISSUE}.1`, "ok"), expect: { work: "halted", commands: [] } },
  { row: "halted commandResult", when: "the stop failed", given: { work: "halted", facts: { halted }, w: { stop: owed }, queue: [{ kind: "stopWorker", machine: "work" }], armed: [{ machine: "work", kind: "own", dueMs: NOW + 2 * MIN }] }, event: result(`${ISSUE}.1`, "failed"), expect: { work: "halted", commands: [], workOwn: 2 * MIN } },
  { row: "merged agentExited", when: "an exit waits behind a command in flight", given: { work: "merged", facts: { halted }, w: { stop: { ...owed, command: `${ISSUE}.2` } }, queue: [resume] }, event: ev("agentExited", {}), expect: { work: "merged", commands: [] } },
];
for (const c of stops) test(`work: ${c.row}, ${c.when}`, () => check(c));

test("a resume whose ok lands in stopped after the stop was confirmed sends the stop again", () => {
  const s = given({ work: "halted", facts: { halted }, w: { stop: { ...owed, command: `${ISSUE}.2` } }, queue: [resume, stopWorker] });
  const confirmed = apply(s, result(`${ISSUE}.2`, "ok"));
  assert.equal(confirmed.store.work?.stop, null);
  const late = apply(confirmed.store, result(`${ISSUE}.1`, "ok"));
  assert.deepEqual(late.commands, ["stopWorker"]);
  assert.equal(late.store.work?.stop?.command, late.store.queue.at(-1)!.id);
});

test("an exit received before a resent stop doesn't confirm it, though it waited until after", () => {
  // The old agent's exit waits behind a resume sent before the stop; the resume then succeeds,
  // and the stop goes again.
  const s = given({ work: "halted", facts: { halted }, w: { stop: { command: `${ISSUE}.0`, sentMs: NOW - MIN, since: NOW - MIN } }, armed: own, queue: [resume] });
  const waited = receive(s, ev("agentExited", {}), NOW).store;
  assert.equal(waited.waiting.length, 1);
  const resumed = receive(waited, result(`${ISSUE}.1`, "ok"), NOW + MIN);
  assert.deepEqual(resumed.send.map((c) => c.kind), ["stopWorker"]);
  // That stop fails and the queue empties: the exit runs, and the stop is still owed.
  const failed = receive(resumed.store, result(`${ISSUE}.2`, "failed"), NOW + 2 * MIN).store;
  assert.equal(failed.waiting.length, 0);
  assert.equal(failed.work?.state, "halted");
  assert.equal(failed.work?.stop?.sentMs, NOW + MIN);
});

test("the timer's resend of a stop keeps when it was sent, so an exit that waited still confirms it", () => {
  const s = given({ work: "halted", facts: { halted }, w: { stop: owed }, armed: own, queue: [{ kind: "stopWorker", machine: "work" }] });
  const waited = apply(s, ev("agentExited", {}));
  const again = apply(waited.store, deadline(waited.store, "work"));
  assert.deepEqual(again.commands, ["stopWorker"]);
  assert.equal(again.store.work?.stop?.sentMs, owed.sentMs);
  const second = apply(again.store, result(`${ISSUE}.2`, "failed"));
  const first = apply(second.store, result(`${ISSUE}.1`, "failed"));
  assert.equal(first.store.work?.stop, null);
});

test("a merge with a stop owed sends it again, keeping when it was sent", () => {
  const o = apply(given({ work: "halted", facts: { pr: pr(), halted }, w: { stop: owed } }), ev("prMerged", { pr: 7, at: T1 }));
  assert.equal(o.store.work?.stop?.sentMs, owed.sentMs);
});

test("an exit received after the stop was sent confirms it, however long it waited", () => {
  const s = given({ work: "halted", facts: { halted }, w: { stop: { command: `${ISSUE}.0`, sentMs: NOW - MIN, since: NOW - MIN } }, armed: own, queue: [resume] });
  const waited = apply(s, ev("agentExited", {}));
  const failed = apply(waited.store, result(`${ISSUE}.1`, "failed"));
  assert.equal(failed.store.work?.stop, null);
});

test("a round sent before the stop that failed untyped isn't counted, and sends no stop", () => {
  const started = apply(given({ work: "running", facts: { pr: pr() }, queue: [evaluatePr] }), result(`${ISSUE}.1`, "ok", evaluated({ feedback: { at: T1, reviewerAt: null } })));
  const round = started.store.queue.at(-1)!;
  const stopped = structuredClone(started.store);
  stopped.work!.state = "halted";
  stopped.facts.halted = halted;
  stopped.work!.stop = { command: `${ISSUE}.0`, sentMs: NOW - MIN, since: NOW - MIN };
  const o = apply(stopped, result(round.id, "failed"));
  assert.equal(o.store.facts.rounds.total, 0);
  assert.deepEqual(o.commands, []);
});

test("a cancel reopened with its stop unconfirmed tries the stop again in 2 minutes", () => {
  const s = given({ work: "halted", w: { stop: owed, cancelledMs: NOW - HOUR }, facts: { halted: { ...halted, by: "cancel" }, issueState: cancelledIssue }, armed: [{ machine: "work", kind: "own", dueMs: NOW + MIN }] });
  const o = apply(s, ev("issueStateChanged", { state: "open", parent: null, at: T1 }));
  assert.deepEqual([o.work, o.store.work?.stop, o.store.work?.deadlines], ["halted", owed, s.work?.deadlines]);
  assert.deepEqual(apply(o.store, deadline(o.store, "work")).commands, ["stopWorker"]);
});
