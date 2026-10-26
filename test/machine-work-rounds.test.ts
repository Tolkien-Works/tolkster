import assert from "node:assert/strict";
import { test } from "node:test";
import type { Event } from "../src/worker/machine/types.ts";
import { toWire } from "../src/worker/route.ts";
import { apply, check, deadline, given, ev, HOUR, ISSUE, MIN, NOW, result, S, SHA, T0, T1, T2, type Given } from "./machine-fixture.ts";
import { evaluated, evaluatePr, halted, landed, LATER, own, owed, pr, requested, SHA2, updating } from "./machine-work-fixture.ts";

// A PR's reads and the rounds they start, beyond the doc's one case per row.

test("a PR read older than the PR event held is stale, and a newer head is applied as a push", () => {
  check({ row: "", when: "", given: { work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ status: "closed", at: "2026-09-30T23:00:00.000Z" })), expect: { work: "rest", commands: [] } });
  check({ row: "", when: "", given: { work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ head: SHA2, at: T1 })), expect: { work: "rest", commands: [] } });
  // An open read of the PR before a new one, or older than the PR held, decides nothing.
  check({ row: "", when: "", given: { work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ number: 6, mergeState: "DIRTY" })), expect: { work: "rest", commands: [] } });
  check({ row: "", when: "", given: { work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ at: "2026-09-30T23:00:00.000Z", mergeState: "DIRTY" })), expect: { work: "rest", commands: [] } });
  // A read of the PR before a new one is applied to neither.
  check({ row: "", when: "", given: { work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ number: 6, status: "closed", at: T1 })), expect: { work: "rest", commands: [] } });
  // A read of another head from the second of the PR held is a push only if sent after that
  // was received.
  check({ row: "", when: "", given: { work: "running", facts: { pr: pr() }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ head: SHA2, mergeState: "DIRTY" })), expect: { work: "running", commands: ["startRound"] } });
  check({ row: "", when: "", given: { work: "running", facts: { pr: { ...pr(), receivedMs: NOW - MIN } }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ head: SHA2, mergeState: "DIRTY" })), expect: { work: "running", commands: [] } });
});

test("a webhook's push to the open PR that came before the held one is stale by its before SHA", () => {
  const [SHA3, SHA4] = ["c".repeat(40), "d".repeat(40)];
  const order = (status: "open" | "closed", ...pushes: [string, string, string][]) => {
    let s = given({ work: "rest", facts: { pr: { ...pr(status), at: T1 } } });
    for (const [head, before, at] of pushes) s = apply(s, ev("prPushed", { pr: 7, head, before, at })).store;
    return [s.facts.pr?.head, s.facts.pr?.at];
  };
  // The next push, from the held head, wins in the held second or after; with an older time
  // it is a cycle, as a PR's time never goes back.
  assert.deepEqual(order("open", [SHA2, SHA, T1]), [SHA2, T1]);
  assert.deepEqual(order("open", [SHA2, SHA, LATER]), [SHA2, LATER]);
  assert.deepEqual(order("open", [SHA2, SHA, T0]), [SHA, T1]);
  assert.deepEqual(order("open", [SHA3, SHA2, LATER], [SHA, SHA3, LATER], [SHA2, SHA, T0]), [SHA, LATER]);
  // A push to the held head is a replay in its second, and a push back to it after.
  assert.deepEqual(order("open", [SHA2, SHA, T1], [SHA2, SHA3, T1]), [SHA2, T1]);
  assert.deepEqual(order("open", [SHA, SHA3, LATER]), [SHA, LATER]);
  // Two pushes in one second, delivered out of order: the first is what the held one replaced.
  assert.deepEqual(order("open", [SHA4, SHA2, LATER], [SHA2, SHA, LATER]), [SHA4, LATER]);
  // A force-push back, delivered before the push it undoes: the chain runs both ways, so time decides.
  assert.deepEqual(order("open", [SHA, SHA2, LATER], [SHA2, SHA, T1]), [SHA, LATER]);
  // Off the chain, an older time is stale, and the later arrival wins at the held second.
  assert.deepEqual(order("open", [SHA2, SHA3, T0]), [SHA, T1]);
  assert.deepEqual(order("open", [SHA2, SHA3, T1]), [SHA2, T1]);
  // A closed PR's pushes go by time alone, and the held second keeps the close.
  assert.deepEqual(order("closed", [SHA2, SHA, T0]), [SHA, T1]);
  assert.deepEqual(order("closed", [SHA2, SHA, T1]), [SHA, T1]);
});

test("a Copilot comment with nothing open is told once per head, never to a running worker, and a round on feedback wins", () => {
  const read = (work: "running" | "rest", over: object = {}) => check({ row: "", when: "", given: { work, facts: { pr: pr(), notices: work === "rest" ? [`copilotWontApprove:7:${SHA}`] : [] }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ copilotWontApprove: true, ...over })), expect: { work, commands: [] } });
  read("rest");
  read("running");
  check({ row: "", when: "", given: { work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ copilotWontApprove: true, manualMerge: true })), expect: { work: "rest", commands: [] } });
  check({ row: "", when: "", given: { work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ copilotWontApprove: true, feedback: { at: T1, reviewerAt: null } })), expect: { work: "launching", commands: ["startRound"], out: ["slotHeld"], workOwn: 10 * MIN } });
});

test("past the cap a reviewer's feedback resumes the worker though Copilot's is newer, and auto-merge off from the start tells no one", () => {
  const capped = { pr: pr(), rounds: { total: 3, byHead: {}, lastAt: T0, lastBase: null, lastFailed: null } };
  check({ row: "", when: "", given: { work: "exited", facts: capped, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ feedback: { at: LATER, reviewerAt: T1 } })), expect: { work: "launching", commands: ["startRound"], out: ["slotHeld"], workOwn: 10 * MIN } });
  check({ row: "", when: "", given: { work: "exited", facts: { ...capped, rounds: { ...capped.rounds, lastAt: T1 } }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ feedback: { at: LATER, reviewerAt: T1 } })), expect: { work: "exited", commands: ["postNotice:roundsCap"] } });
  check({ row: "", when: "", given: { work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ autoMergeOff: false })), expect: { work: "rest", commands: [] } });
});

test("a failed check at the held head starts one round, never again for that head, and again for a new head that fails", () => {
  const read = (head = SHA) => result(`${ISSUE}.1`, "ok", evaluated({ head, checks: "failed" }));
  const o = apply(given({ work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }), read());
  assert.equal(o.work, "launching");
  assert.deepEqual(o.commands, ["startRound"]);
  assert.deepEqual(landed(o.store), { total: 1, byHead: { [SHA]: 1 }, lastAt: null, lastBase: null, lastFailed: SHA });
  // Another suite failing on the same head, once the round is over.
  const after = { total: 1, byHead: { [SHA]: 1 }, lastAt: null, lastBase: null, lastFailed: SHA };
  check({ row: "", when: "", given: { work: "rest", facts: { pr: pr(), rounds: after }, queue: [evaluatePr] }, event: read(), expect: { work: "rest", commands: [] } });
  // A new head that fails again.
  const pushed = apply(given({ work: "rest", facts: { pr: { ...pr(), head: SHA2 }, rounds: after }, queue: [evaluatePr] }), read(SHA2));
  assert.deepEqual(pushed.commands, ["startRound"]);
  assert.equal(landed(pushed.store).total, 2);
  // At the cap it says so, once per head.
  check({ row: "", when: "", given: { work: "rest", facts: { pr: pr(), rounds: { ...after, total: 3, lastFailed: null } }, queue: [evaluatePr] }, event: read(), expect: { work: "rest", commands: ["postNotice:roundsCap"] } });
  check({ row: "", when: "", given: { work: "rest", facts: { pr: pr(), rounds: { ...after, total: 3, lastFailed: null }, notices: [`roundsCap:7:${SHA}`] }, queue: [evaluatePr] }, event: read(), expect: { work: "rest", commands: [] } });
});

test("a failed check read while the agent runs waits for its turn to end, then starts the round", () => {
  const atWork = apply(given({ work: "running", sess: { answeredThrough: T1 }, facts: { pr: pr() }, queue: [evaluatePr] }), result(`${ISSUE}.1`, "ok", evaluated({ checks: "failed" })));
  assert.deepEqual(atWork.commands, []);
  assert.equal(atWork.store.facts.rounds.total, 0);
  // Idle at its prompt, its turn is over: the PR is read again, and the round goes into its open terminal.
  const idle = apply(atWork.store, ev("agentIdle", {}));
  assert.equal(idle.work, "idle");
  assert.deepEqual(idle.commands, ["evaluatePr"]);
  const told = apply(idle.store, result(idle.store.queue.at(-1)!.id, "ok", evaluated({ checks: "failed" })));
  assert.equal(told.work, "idle");
  assert.deepEqual(told.commands, ["startRound"]);
  assert.equal(landed(told.store).lastFailed, SHA);
  // An agent that exits before going idle is read when it exits.
  const gone = apply(atWork.store, ev("agentExited", {}));
  assert.equal(gone.work, "rest");
  assert.deepEqual(gone.commands, ["evaluatePr"]);
  const round = apply(gone.store, result(gone.store.queue.at(-1)!.id, "ok", evaluated({ checks: "failed" })));
  assert.equal(round.work, "launching");
  assert.deepEqual(round.commands, ["startRound"]);
  assert.equal(landed(round.store).lastFailed, SHA);
  // A round for feedback on a red head, to a worker still running, leaves the failure to its exit.
  const both = apply(given({ work: "running", facts: { pr: pr() }, queue: [evaluatePr] }), result(`${ISSUE}.1`, "ok", evaluated({ checks: "failed", feedback: { at: T1, reviewerAt: null } })));
  assert.deepEqual(both.commands, ["startRound"]);
  assert.equal(landed(both.store).total, 1);
  assert.equal(landed(both.store).lastFailed, null);
  assert.equal(both.store.work!.prEvaluated?.redWhileRunning, true);
});

test("a failed check at an idle worker's open terminal starts one round per head, with no launch", () => {
  const idle = given({ work: "idle", facts: { pr: pr() }, queue: [evaluatePr] });
  const o = apply(idle, result(`${ISSUE}.1`, "ok", evaluated({ checks: "failed" })));
  assert.equal(o.work, "idle");
  assert.deepEqual(o.commands, ["startRound"]);
  assert.deepEqual(o.store.queue.at(-1)!.params.proposal, { head: SHA, lastFailed: SHA });
  assert.deepEqual(landed(o.store), { total: 1, byHead: { [SHA]: 1 }, lastAt: null, lastBase: null, lastFailed: SHA });
  // Not again at that head, and at the cap it says so instead.
  const after = { total: 1, byHead: { [SHA]: 1 }, lastAt: null, lastBase: null, lastFailed: SHA };
  check({ row: "", when: "", given: { work: "idle", facts: { pr: pr(), rounds: after }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ checks: "failed" })), expect: { work: "idle", commands: [] } });
  check({ row: "", when: "", given: { work: "idle", facts: { pr: pr(), rounds: { ...after, total: 3, lastFailed: null } }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ checks: "failed" })), expect: { work: "idle", commands: ["postNotice:roundsCap"] } });
});

test("a lost read of a red seen mid-turn is read again in an idle worker, not left to an exit", () => {
  const atWork = apply(given({ work: "running", sess: { answeredThrough: T1 }, facts: { pr: pr() }, queue: [evaluatePr] }), result(`${ISSUE}.1`, "ok", evaluated({ checks: "failed" })));
  const idle = apply(atWork.store, ev("agentIdle", {}));
  assert.deepEqual(idle.commands, ["evaluatePr"]);
  const lost = apply(idle.store, result(idle.store.queue.at(-1)!.id, "lost"));
  assert.deepEqual(lost.commands, []);
  assert.equal(lost.store.work!.deadlines.some((d) => d.kind === "recheck"), true);
  const again = apply(lost.store, deadline(lost.store, "work", "recheck"));
  assert.deepEqual(again.commands.slice(0, 1), ["evaluatePr"]);
  // Lost again, it is paced by the next recheck; an exit before it reads at once.
  const lostAgain = apply(again.store, result(again.store.queue.at(-1)!.id, "lost"));
  assert.deepEqual(lostAgain.commands, []);
  assert.equal(lostAgain.store.work!.deadlines.some((d) => d.kind === "recheck"), true);
  const gone = apply(lostAgain.store, ev("agentExited", {}));
  assert.deepEqual(gone.commands, ["evaluatePr"]);
});

test("a burst of PR facts landing while evaluatePr is in flight is evaluated once more, after its result", () => {
  let s = given({ work: "rest", facts: { pr: pr() }, queue: [evaluatePr] });
  for (const e of [ev("prPushed", { pr: 7, head: SHA2, before: SHA, at: T1 }), ev("prReviewed", { pr: 7, sha: SHA2, reviewer: true, copilot: false, at: T1 }), ev("checksCompleted", { sha: SHA2, result: "passed", at: LATER })]) {
    const o = apply(s, e);
    assert.deepEqual(o.commands, [], "a fact waits while the read is in flight");
    s = o.store;
  }
  assert.equal(s.waiting.length, 0, "a PR fact moves the facts' version and waits as no event");
  const first = apply(s, result(`${ISSUE}.1`, "ok", evaluated()));
  assert.deepEqual(first.commands, ["evaluatePr"]);
  const again = apply(first.store, result(first.store.queue[0].id, "ok", evaluated({ head: SHA2, at: LATER })));
  assert.deepEqual(again.commands, []);
  assert.equal(again.store.waiting.length, 0);
});

test("reviews landing seconds apart start one round, and what lands during it is left to it", () => {
  const reviewed = (id: string, at: string): Event => ({ ...ev("prReviewed", { pr: 7, sha: SHA, reviewer: true, copilot: false, at }), id });
  const MORE = new Date(Date.parse(LATER) + MIN).toISOString();
  let s = given({ work: "rest", facts: { pr: pr() } });
  let o = apply(s, reviewed("r1", T1));
  assert.deepEqual(o.commands, ["evaluatePr"]);
  o = apply(o.store, result(o.store.queue[0].id, "ok", evaluated({ feedback: { at: T1, reviewerAt: null } })));
  assert.equal(o.work, "launching");
  assert.deepEqual(o.commands, ["startRound"]);
  // The second review lands before the round's line did: it waits, then is read once more.
  o = apply(o.store, reviewed("r2", LATER));
  assert.deepEqual(o.commands, []);
  o = apply(o.store, result(o.store.queue[0].id, "ok"));
  assert.equal(o.work, "running");
  assert.deepEqual(o.commands, ["evaluatePr"]);
  o = apply(o.store, result(o.store.queue[0].id, "ok", evaluated({ feedback: { at: LATER, reviewerAt: null } })));
  assert.deepEqual(o.commands, [], "the round in flight reads it");
  assert.equal(o.store.facts.rounds.total, 1);
  assert.equal(o.store.facts.rounds.lastAt, LATER);
  // A third lands while the worker is still running the round.
  o = apply(o.store, reviewed("r3", MORE));
  assert.deepEqual(o.commands, ["evaluatePr"]);
  o = apply(o.store, result(o.store.queue.at(-1)!.id, "ok", evaluated({ feedback: { at: MORE, reviewerAt: null } })));
  assert.deepEqual(o.commands, []);
  assert.equal(o.store.facts.rounds.total, 1);
  // Once the worker is idle the round is over, and feedback after it starts the next.
  o = apply(o.store, ev("agentIdle", {}));
  assert.equal(o.work, "idle");
  assert.equal(o.store.work!.inRound ?? null, null);
  const NEXT = new Date(Date.parse(MORE) + MIN).toISOString();
  o = apply(o.store, reviewed("r4", NEXT));
  o = apply(o.store, result(o.store.queue.at(-1)!.id, "ok", evaluated({ feedback: { at: NEXT, reviewerAt: null } })));
  assert.deepEqual(o.commands, ["startRound"]);
  assert.equal(landed(o.store).total, 2);
});

test("feedback on a head the round did not begin on, or after its line failed, starts a round of its own", () => {
  const inRound = { rounds: { total: 1, byHead: { [SHA]: 1 }, lastAt: T0, lastBase: null, lastFailed: null } };
  check({ row: "", when: "", given: { work: "running", facts: { pr: pr(), ...inRound }, w: { inRound: SHA }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ feedback: { at: T1, reviewerAt: null } })), expect: { work: "running", commands: [] } });
  check({ row: "", when: "", given: { work: "running", facts: { pr: { ...pr(), head: SHA2 }, ...inRound }, w: { inRound: SHA }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ head: SHA2, feedback: { at: T1, reviewerAt: null } })), expect: { work: "running", commands: ["startRound"] } });
  const failed = given({ work: "running", facts: { pr: pr(), ...inRound }, w: { inRound: SHA }, queue: [{ kind: "startRound", machine: "work" }] });
  assert.equal(apply(failed, result(`${ISSUE}.1`, "failed")).store.work!.inRound ?? null, null);
});

// A running worker sent a rebase for a DIRTY PR; the round is in flight in the store returned.
const rebasing = () => apply(given({ work: "running", facts: { pr: pr() }, queue: [evaluatePr] }), result(`${ISSUE}.1`, "ok", evaluated({ mergeState: "DIRTY" }))).store;

test("a round's line typed into an agent that exits is read again, not counted", () => {
  const sent = rebasing();
  const round = sent.queue.find((c) => c.kind === "startRound")!.id;
  // The exit beside the line, then the result: nothing is marked, and the read waits for the retry's deadline.
  const exited = apply(sent, ev("agentExited", {}));
  const typed = apply(exited.store, result(round, "failed", { typed: true }));
  assert.equal(typed.store.facts.rounds.lastBase, null);
  assert.equal(typed.store.facts.rounds.total, 0);
  assert.deepEqual(typed.commands, []);
  assert.equal(typed.recheck, true);
  const read = apply(typed.store, deadline(typed.store, "work", "recheck"));
  assert.deepEqual(read.commands, ["evaluatePr"]);
  const again = apply(read.store, result(read.store.queue.find((c) => c.kind === "evaluatePr")!.id, "ok", evaluated({ mergeState: "DIRTY" })));
  assert.deepEqual(again.commands, ["startRound"]);
  // The result first: the round waits for a turn, and an exit before one drops it the same way.
  const waiting = apply(sent, result(round, "failed", { typed: true }));
  assert.equal(waiting.store.facts.rounds.total, 0);
  const gone = apply(waiting.store, ev("agentExited", {}));
  assert.equal(gone.store.facts.rounds.total, 0);
  assert.equal(gone.store.work!.unseen, undefined);
  assert.equal(gone.recheck, true);
  // The exit's own deadline comes first and stalls the worker: the read owed still comes.
  const stalled = apply(gone.store, deadline(gone.store, "work"));
  assert.equal(stalled.work, "exited");
  const waits = apply(stalled.store, deadline(stalled.store, "work", "recheck"));
  const notice = waits.store.queue.find((c) => c.kind === "postNotice")!;
  assert.deepEqual(apply(waits.store, result(notice.id, "ok")).commands, ["evaluatePr"]);
  // A halt leaves nothing waiting for a turn.
  assert.equal(apply(waiting.store, ev("stopRequested", { session: S, at: T1 })).store.work!.unseen, undefined);
  // A turn seen begin marks it, a rebase-only round counting nothing.
  const worked = apply(waiting.store, ev("agentWorking", {}));
  assert.deepEqual(worked.store.facts.rounds, { total: 0, byHead: { [SHA]: 1 }, lastAt: null, lastBase: SHA2, lastFailed: null });
});

test("a PR only behind its base with no agent running is updated as the app, counting no round", () => {
  const behind = (over: object = {}) => result(`${ISSUE}.1`, "ok", evaluated({ mergeState: "BEHIND", ...over }));
  const sent = apply(given({ work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }), behind());
  assert.deepEqual(sent.commands, ["updateBranch"]);
  assert.deepEqual(sent.store.queue[0].params, { head: SHA, base: SHA2 });
  // A read while it is in flight finds its base covered.
  check({ row: "", when: "", given: { work: "rest", facts: { pr: pr() }, queue: [updating(), evaluatePr] }, event: result(`${ISSUE}.2`, "ok", evaluated({ mergeState: "BEHIND" })), expect: { work: "rest", commands: [] } });
  // Done, its base is marked and no round counted, so the same base asks nothing again.
  const done = apply(sent.store, result(sent.store.queue[0].id, "ok"));
  assert.deepEqual(done.store.facts.rounds, { total: 0, byHead: {}, lastAt: null, lastBase: SHA2, lastFailed: null });
  check({ row: "", when: "", given: { work: "rest", facts: { pr: pr(), rounds: done.store.facts.rounds }, queue: [evaluatePr] }, event: behind(), expect: { work: "rest", commands: [] } });
  // A head moved under it marks nothing, and its push is read when it lands.
  assert.equal(apply(sent.store, result(sent.store.queue[0].id, "ok", { moved: true })).store.facts.rounds.lastBase, null);
  // Refused with the head held moved on since, no round starts; a failure
  // GitHub didn't refuse wakes no worker, and is read again later.
  check({ row: "", when: "", given: { work: "rest", facts: { pr: { ...pr(), head: SHA2 } }, queue: [updating()] }, event: result(`${ISSUE}.1`, "failed", { refused: true }), expect: { work: "rest", commands: [], recheck: false } });
  check({ row: "", when: "", given: { work: "rest", facts: { pr: pr() }, queue: [updating()] }, event: result(`${ISSUE}.1`, "failed"), expect: { work: "rest", commands: [], recheck: true } });
  // Past the cap the update still goes, and the notices after it are still said.
  const capped = { pr: pr(), rounds: { total: 3, byHead: {}, lastAt: null, lastBase: null, lastFailed: null } };
  check({ row: "", when: "", given: { work: "rest", facts: capped, queue: [evaluatePr] }, event: behind({ autoMergeOff: true }), expect: { work: "rest", commands: ["updateBranch", "postNotice:autoMergeOff"] } });
  // Refused, the round it starts is a rebase alone.
  const refused = apply(given({ work: "rest", facts: { pr: pr() }, queue: [updating()] }), result(`${ISSUE}.1`, "failed", { refused: true }));
  assert.deepEqual(refused.store.queue[0].params, { pr: 7, proposal: { head: SHA, lastBase: SHA2 } });
  assert.equal(toWire(refused.store.queue[0], refused.store, "TW-1").args.rebaseOnly, true);
  // A conflict, or an agent running, is a worker's round; so is a base move with feedback.
  check({ row: "", when: "", given: { work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }, event: behind({ mergeState: "DIRTY" }), expect: { work: "launching", commands: ["startRound"] } });
  check({ row: "", when: "", given: { work: "running", facts: { pr: pr() }, queue: [evaluatePr] }, event: behind(), expect: { work: "running", commands: ["startRound"] } });
  check({ row: "", when: "", given: { work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }, event: behind({ feedback: { at: T1, reviewerAt: null } }), expect: { work: "launching", commands: ["startRound"] } });
});

test("rebase-only rounds count nothing against the cap, and a conflict past it still starts one", () => {
  const dirty = (over: object = {}) => result(`${ISSUE}.1`, "ok", evaluated({ mergeState: "DIRTY", ...over }));
  const capped = { pr: pr(), rounds: { total: 3, byHead: {}, lastAt: null, lastBase: null, lastFailed: null } };
  // A conflict past the cap starts a round, which counts nothing but marks its base and head.
  const sent = apply(given({ work: "rest", facts: capped, queue: [evaluatePr] }), dirty());
  assert.deepEqual(sent.commands, ["startRound"]);
  assert.deepEqual(sent.store.queue[0].params, { pr: 7, proposal: { head: SHA, lastBase: SHA2 } });
  assert.deepEqual(landed(sent.store), { total: 3, byHead: { [SHA]: 1 }, lastAt: null, lastBase: SHA2, lastFailed: null });
  // Past the cap with Copilot's feedback and a failed check, only the rebase is owed to a worker.
  const rebaseOnly = apply(given({ work: "rest", facts: capped, queue: [evaluatePr] }), dirty({ checks: "failed", feedback: { at: T1, reviewerAt: null } }));
  assert.deepEqual(rebaseOnly.store.queue[0].params, { pr: 7, proposal: { head: SHA, lastBase: SHA2 } });
  // Base moves beyond maxRounds, each a conflict, each get a round without spending the cap.
  let store = given({ work: "rest", facts: { pr: pr() }, queue: [evaluatePr] });
  for (const base of [SHA2, "a".repeat(40), "b".repeat(40), "c".repeat(40)]) {
    const round = apply(store, dirty({ base }));
    assert.deepEqual(round.commands, ["startRound"]);
    const done = apply(round.store, result(round.store.queue.find((c) => c.kind === "startRound")!.id, "ok"));
    assert.equal(done.store.facts.rounds.total, 0);
    store = given({ work: "rest", facts: { pr: pr(), rounds: done.store.facts.rounds }, queue: [evaluatePr] });
  }
  // The same base asks nothing again, and a review under the cap still starts its round.
  check({ row: "", when: "", given: { work: "rest", facts: { pr: pr(), rounds: store.facts.rounds }, queue: [evaluatePr] }, event: dirty({ base: "c".repeat(40) }), expect: { work: "rest", commands: [] } });
  check({ row: "", when: "", given: { work: "rest", facts: { pr: pr(), rounds: store.facts.rounds }, queue: [evaluatePr] }, event: dirty({ base: "c".repeat(40), feedback: { at: T1, reviewerAt: null } }), expect: { work: "launching", commands: ["startRound"] } });
});

test("a rebase-only round leaves no feedback to itself, so what lands during it starts a round", () => {
  const started = apply(given({ work: "running", facts: { pr: pr() }, queue: [evaluatePr] }), result(`${ISSUE}.1`, "ok", evaluated({ mergeState: "DIRTY" })));
  assert.deepEqual(started.commands, ["startRound"]);
  assert.equal(started.store.work!.inRound ?? null, null);
  const typed = apply(started.store, result(started.store.queue[0].id, "ok"));
  const read = apply(typed.store, ev("prReviewed", { pr: 7, sha: SHA, reviewer: true, copilot: false, at: T1 }));
  const o = apply(read.store, result(read.store.queue[0].id, "ok", evaluated({ mergeState: "DIRTY", feedback: { at: T1, reviewerAt: null } })));
  assert.deepEqual(o.commands, ["startRound"]);
  assert.equal(o.store.work!.inRound, SHA);
  // One sent while a review round runs on the head leaves that round its cover.
  const reviewing = { rounds: { total: 1, byHead: { [SHA]: 1 }, lastAt: T1, lastBase: null, lastFailed: null } };
  const rebased = apply(given({ work: "running", facts: { pr: pr(), ...reviewing }, w: { inRound: SHA }, queue: [evaluatePr] }), result(`${ISSUE}.1`, "ok", evaluated({ mergeState: "DIRTY" })));
  assert.deepEqual(rebased.commands, ["startRound"]);
  assert.equal(rebased.store.work!.inRound, SHA);
  // Its line failing with nothing typed leaves that cover too.
  assert.equal(apply(rebased.store, result(rebased.store.queue[0].id, "failed")).store.work!.inRound, SHA);
  // A review round failing untyped ends its cover, though a rebase-only one waits behind it.
  const rebaseQueued = { kind: "startRound" as const, machine: "work", params: { pr: 7, proposal: { head: SHA, lastBase: SHA2 } } };
  const both = given({ work: "running", facts: { pr: pr(), ...reviewing }, w: { inRound: SHA }, queue: [{ kind: "startRound", machine: "work", params: { pr: 7, proposal: { head: SHA, lastAt: T1 } } }, rebaseQueued] });
  assert.equal(apply(both, result(`${ISSUE}.1`, "failed")).store.work!.inRound ?? null, null);
});

test("a merge state still UNKNOWN is read again at 10, 20 and 40 s, then left to the next fact", () => {
  let s = given({ work: "rest", facts: { pr: pr(), base: SHA2 }, queue: [evaluatePr] });
  for (const due of [10_000, 20_000, 40_000, null]) {
    const o = apply(s, result(s.queue[0].id, "ok", evaluated({ mergeState: "UNKNOWN" })));
    const recheck = o.store.work!.deadlines.find((d) => d.kind === "recheck");
    assert.equal(recheck === undefined ? null : recheck.dueMs - NOW, due);
    if (recheck === undefined) break;
    const fired = apply(o.store, deadline(o.store, "work", "recheck"));
    assert.deepEqual(fired.commands, ["evaluatePr"]);
    s = fired.store;
  }
  const known = apply(s, result(s.queue[0].id, "ok", evaluated()));
  assert.equal(known.store.work!.unknownRechecks, 0, "a known merge state starts the count again");
});

test("a PR mergeable now is merged at the head read, with auto-merge on or asked for, and never one the owner holds", () => {
  const merges = (over: object, facts: object = {}, work: Given["work"] = "rest", w: Given["w"] = {}) =>
    apply(given({ work, facts: { pr: pr(), ...facts }, w, queue: [evaluatePr] }), result(`${ISSUE}.1`, "ok", evaluated({ mergeable: true, ...over }))).commands.includes("mergePr");
  assert.equal(merges({ autoMerge: true }), true);
  // GitHub refused auto-merge as mergeable already, and the worker asked for the merge.
  const asked = apply(given({ work: "rest", facts: { pr: pr() } }), ev("mergeAsked", { session: S, pr: 7 }));
  assert.deepEqual([asked.store.facts.mergeAsked, asked.commands], [7, ["evaluatePr"]]);
  assert.equal(merges({ autoMerge: false }, { mergeAsked: 7 }), true);
  assert.equal(merges({ autoMerge: false }, { mergeAsked: 6 }), false, "asked for another PR");
  assert.equal(merges({ autoMerge: false }), false, "left off by the worker");
  assert.equal(merges({ autoMerge: false, autoMergeOff: true }, { mergeAsked: 7 }), false, "turned off by the owner");
  assert.equal(merges({ autoMerge: true, manualMerge: true }), false, "manual-merge");
  assert.equal(merges({ autoMerge: true }, { notices: [`roundsCap:7:${SHA}`] }), false, "capped at the head");
  assert.equal(merges({ autoMerge: true, mergeable: false }), false, "GitHub's own not CLEAN");
  assert.equal(merges({ autoMerge: true, checks: "pending" }), false, "checks pending");
  assert.equal(merges({ autoMerge: true }, {}, "idle"), true, "its worker's turn over");
  // Mid-turn its worker may still push: read again once the turn ends.
  const mid = apply(given({ work: "running", facts: { pr: pr() }, queue: [evaluatePr] }), result(`${ISSUE}.1`, "ok", evaluated({ autoMerge: true, mergeable: true })));
  assert.deepEqual([mid.commands, mid.store.work!.prEvaluated.redWhileRunning], [[], true]);
  assert.deepEqual(apply(mid.store, ev("agentIdle", {})).commands, ["evaluatePr"]);
  assert.equal(merges({ autoMerge: true }, {}, "idle", { inRound: SHA }), false, "a round typed and not yet begun");
  // Its round's end reads it again, though the round pushed nothing.
  const typed = apply(given({ work: "idle", facts: { pr: pr() }, w: { inRound: SHA }, queue: [evaluatePr] }), result(`${ISSUE}.1`, "ok", evaluated({ autoMerge: true, mergeable: true })));
  assert.deepEqual([typed.commands, typed.recheck, typed.store.work!.prEvaluated.redWhileRunning], [[], true, true]);
  const working = apply(typed.store, ev("agentWorking", {}));
  assert.deepEqual(apply(working.store, ev("agentIdle", {})).commands, ["evaluatePr"]);
  assert.equal(merges({ autoMerge: true, feedback: { at: T1, reviewerAt: null } }), false, "feedback first");
  // Pinned to the head read, never the one held when it is sent.
  const o = apply(given({ work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }), result(`${ISSUE}.1`, "ok", evaluated({ autoMerge: true, mergeable: true })));
  const merge = o.store.queue.find((c) => c.kind === "mergePr")!;
  const pushed = structuredClone(o.store);
  pushed.facts.pr!.head = SHA2;
  assert.deepEqual(toWire(merge, pushed, "TW-1").args, { repo: "repo", number: 7, head: SHA });
  // Already merged, by GitHub's own auto-merge, or refused: nothing is said, and its merge event
  // reaps or the next PR fact reads it again; a failure of the way is read again in 10 minutes.
  const after = (outcome: "ok" | "failed", data?: object) => apply(o.store, result(merge.id, outcome, data));
  for (const done of [after("ok"), after("failed", { refused: true })]) assert.deepEqual([done.commands, done.recheck], [[], false]);
  assert.deepEqual([after("failed").commands, after("failed").recheck], [[], true]);
  assert.equal(apply(pushed, result(merge.id, "failed")).recheck, false, "a push since is read when it lands");
  // A move that drops the recheck leaves the read owed: the work reads again as it settles.
  const failed = after("failed").store;
  const moved = apply(failed, ev("agentWorking", {}));
  assert.equal(moved.recheck, false);
  assert.deepEqual(apply(moved.store, ev("agentIdle", {})).commands, ["evaluatePr"]);
});

test("a push gives the merge state its rechecks again, as a base push does", () => {
  const o = apply(given({ work: "rest", facts: { pr: pr() }, w: { unknownRechecks: 3 } }), ev("prPushed", { pr: 7, head: SHA2, before: SHA, at: T1 }));
  assert.deepEqual([o.commands, o.store.work!.unknownRechecks], [["evaluatePr"], 0]);
  const recheck = apply(o.store, result(o.store.queue[0].id, "ok", evaluated({ head: SHA2, at: T1, mergeState: "UNKNOWN" })));
  assert.equal(recheck.store.work!.deadlines.find((d) => d.kind === "recheck")?.dueMs, NOW + 10_000);
});

test("a push to the base gives the merge state its rechecks again", () => {
  const o = apply(given({ work: "rest", facts: { pr: pr(), base: SHA }, w: { unknownRechecks: 3 } }), ev("basePushed", { base: SHA2 }));
  assert.deepEqual(o.commands, ["evaluatePr"]);
  assert.equal(o.store.work!.unknownRechecks, 0);
  // Landing while a read is in flight, it still does.
  const busy = apply(given({ work: "rest", facts: { pr: pr(), base: SHA }, w: { unknownRechecks: 3 }, queue: [evaluatePr] }), ev("basePushed", { base: SHA2 }));
  assert.deepEqual(busy.commands, []);
  assert.equal(busy.store.work!.unknownRechecks, 0);
});

test("a settle deadline armed before it was renamed still reads the PR", () => {
  const s = given({ work: "rest", facts: { pr: pr() } });
  s.work!.deadlines.push({ kind: "settle" as "recheck", version: s.work!.version, dueMs: NOW });
  const o = apply(s, { ...deadline(s, "work", "own"), deadline: "settle" } as unknown as Parameters<typeof apply>[1]);
  assert.deepEqual(o.commands, ["evaluatePr"]);
  assert.equal(o.workOwn, null, "its own deadline is left as it was");
});

test("work: a round's line typed but its turn unseen leaves the round in flight; one never typed ends it", () => {
  const round = (data?: object) => apply(given({ work: "running", w: { inRound: SHA }, queue: [{ kind: "startRound", machine: "work" }] }), result(`${ISSUE}.1`, "failed", data)).store.work?.inRound;
  assert.equal(round({ typed: true }), SHA);
  assert.equal(round(), null);
  const reviewing = apply(given({ work: "launching", facts: { pr: pr() }, w: { launch: "round", inRound: SHA }, queue: [{ kind: "startRound", machine: "work" }] }), result(`${ISSUE}.1`, "failed", { typed: true }));
  assert.equal(reviewing.store.work?.inRound, SHA);
});

test("feedback landing in a round's cover is never written onto a rebase-only round waiting for its turn", () => {
  const unseen = [{ head: SHA, lastBase: SHA2 }];
  const read = apply(given({ work: "running", facts: { pr: pr() }, w: { inRound: SHA, unseen, unseenSentMs: NOW }, queue: [evaluatePr] }), result(`${ISSUE}.1`, "ok", evaluated({ feedback: { at: T1, reviewerAt: null } })));
  assert.deepEqual(read.store.work!.unseen, unseen);
  assert.equal(read.store.facts.rounds.lastAt, T1);
});

test("a startRound that fails with nothing typed counts nothing, and the next read starts the round once", () => {
  const red = () => evaluated({ checks: "failed" });
  const round = apply(given({ work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }), result(`${ISSUE}.1`, "ok", red()));
  assert.deepEqual(round.commands, ["startRound"]);
  assert.equal(landed(round.store).lastFailed, SHA);
  const failed = apply(round.store, result(round.store.queue.at(-1)!.id, "failed"));
  // Back at rest, with no read made at once: the retry's deadline paces the next.
  assert.equal(failed.work, "rest");
  assert.deepEqual(failed.commands, []);
  assert.deepEqual(failed.store.facts.rounds, { total: 0, byHead: {}, lastAt: null, lastBase: null, lastFailed: null });
  const again = apply(failed.store, deadline(failed.store, "work"));
  assert.deepEqual(again.commands.slice(0, 1), ["evaluatePr"]);
  const second = apply(again.store, result(again.store.queue.find((c) => c.kind === "evaluatePr")!.id, "ok", red()));
  assert.equal(second.work, "launching");
  assert.deepEqual(second.commands, ["startRound"]);
  // Queued behind the deadline's other reads, it proposes the first round on red.
  assert.deepEqual(second.store.queue.at(-1)!.params.proposal, { head: SHA, lastFailed: SHA });
});

test("a round's markers are committed by a result that may have reached the worker, in a running worker too", () => {
  const rounds = { total: 1, byHead: { [SHA]: 1 }, lastAt: T0, lastBase: SHA, lastFailed: null };
  const read = evaluated({ mergeState: "BEHIND", base: SHA2, feedback: { at: T1, reviewerAt: null } });
  const started = apply(given({ work: "running", facts: { pr: pr(), rounds }, queue: [evaluatePr] }), result(`${ISSUE}.1`, "ok", read));
  assert.deepEqual(started.commands, ["startRound"]);
  // Nothing is marked while the line is in flight.
  assert.deepEqual(started.store.facts.rounds, rounds);
  const marked = { total: 2, byHead: { [SHA]: 2 }, lastAt: T1, lastBase: SHA2, lastFailed: null };
  const id = started.store.queue.at(-1)!.id;
  const failed = apply(started.store, result(id, "failed"));
  assert.deepEqual(failed.store.facts.rounds, rounds);
  assert.equal(failed.store.work!.inRound ?? null, null);
  // A line held or lost may yet land: the round is counted and its markers move.
  for (const [r, data] of [["held", {}], ["lost", {}], ["ok", {}]] as const) {
    assert.deepEqual(apply(started.store, result(id, r, data)).store.facts.rounds, marked, r);
  }
  // Typed, it is counted once a turn is seen begin (the next test).
  assert.deepEqual(apply(started.store, result(id, "failed", { typed: true })).store.facts.rounds, rounds);
  // The worker having exited meanwhile changes nothing of that.
  const exited = structuredClone(started.store);
  exited.work!.state = "exited";
  exited.work!.inRound = null;
  assert.deepEqual(apply(exited, result(id, "failed")).store.facts.rounds, rounds);
  assert.deepEqual(apply(exited, result(id, "ok")).store.facts.rounds, marked);
  // Feedback read while the line is in flight joins the round's proposal: no second round,
  // dropped with it on a failure, counted with it once it lands.
  const during = structuredClone(started.store);
  during.queue.push({ ...during.queue.at(-1)!, id: `${ISSUE}.99`, kind: "evaluatePr", params: {} });
  const joined = apply(during, result(`${ISSUE}.99`, "ok", evaluated({ feedback: { at: LATER, reviewerAt: null } })));
  assert.deepEqual(joined.commands, []);
  assert.deepEqual(joined.store.facts.rounds, rounds);
  assert.deepEqual(apply(joined.store, result(id, "failed")).store.facts.rounds, rounds);
  assert.deepEqual(apply(joined.store, result(id, "ok")).store.facts.rounds, { ...marked, lastAt: LATER });
});

test("a round's line that landed after a stop is counted, and one failing in a held worker ends its cover", () => {
  const read = evaluated({ feedback: { at: T1, reviewerAt: null } });
  const started = apply(given({ work: "running", facts: { pr: pr() }, queue: [evaluatePr] }), result(`${ISSUE}.1`, "ok", read));
  const id = started.store.queue.at(-1)!.id;
  const stopped = structuredClone(started.store);
  stopped.work!.state = "halted";
  stopped.facts.halted = halted;
  assert.equal(apply(stopped, result(id, "ok")).store.facts.rounds.total, 1);
  const held = structuredClone(started.store);
  held.work!.state = "held";
  assert.equal(apply(held, result(id, "failed")).store.work!.inRound ?? null, null);
  // A round lost may have landed: it is counted, and keeps its cover.
  assert.equal(apply(held, result(id, "lost")).store.work!.inRound, SHA);
});

// No worker waits on CI: each push arms a read for an hour on, which a change of state keeps
// and a merge drops, and only that read tells the owner of a head still pending.
test("a head still pending an hour after its push is told once, from its own read, however the work moved meanwhile", () => {
  const pushed = apply(given({ work: "running", facts: { pr: pr() } }), ev("prPushed", { pr: 7, head: SHA2, before: SHA, at: T1 })).store;
  assert.equal(pushed.work!.deadlines.find((d) => d.kind === "pending")?.dueMs, NOW + HOUR);
  const exited = apply(pushed, ev("agentExited", {})).store;
  assert.equal(exited.work!.deadlines.find((d) => d.kind === "pending")?.dueMs, NOW + HOUR);
  const armed = given({ work: "rest", facts: { pr: pr() }, armed: [{ machine: "work", kind: "pending" }] });
  assert.equal(apply(armed, ev("prMerged", { pr: 7, at: T2 })).store.work!.deadlines.some((d) => d.kind === "pending"), false);
  // A worker begun on a PR opened before it arms one.
  assert.equal(apply(given({ work: "reaped", facts: { pr: pr() } }), requested()).store.work!.deadlines.find((d) => d.kind === "pending")?.dueMs, NOW + HOUR);
  // Ended work arms none.
  assert.equal(apply(given({ work: "halted", facts: { pr: pr(), halted } }), ev("prPushed", { pr: 7, head: SHA2, before: SHA, at: T1 })).store.work!.deadlines.some((d) => d.kind === "pending"), false);
  // Fired while a round launches, the read is owed until the work evaluates.
  const launching = given({ work: "launching", w: { launch: "round" }, facts: { pr: pr() }, armed: [{ machine: "work", kind: "pending" }] });
  const owed = apply(launching, deadline(launching, "work", "pending"));
  assert.deepEqual([owed.commands, owed.store.work!.pendingDue], [[], SHA]);
  const read = (w: object, out: "ok" | "failed") => apply(given({ work: "exited", facts: { pr: pr() }, w, queue: [evaluatePr] }), result(`${ISSUE}.1`, out, out === "ok" ? evaluated({ checks: "pending" }) : {}));
  assert.deepEqual(read({ pendingDue: SHA }, "ok").commands, ["postNotice:checksPending"]);
  // Any other read of pending checks, as of a re-run an hour after the push, tells nothing.
  assert.deepEqual(read({}, "ok").commands, []);
  // A failed read keeps the debt, read again at the recheck, never at once.
  const failed = read({ pendingDue: SHA }, "failed");
  assert.deepEqual([failed.commands, failed.store.work!.pendingDue, failed.store.work!.deadlines.some((d) => d.kind === "recheck")], [[], SHA, true]);
  assert.deepEqual(apply(failed.store, deadline(failed.store, "work", "recheck")).commands, ["evaluatePr"]);
  // So does an ok read lagging at an older head; a debt for a head since replaced is dropped.
  const lagging = apply(given({ work: "exited", facts: { pr: pr() }, w: { pendingDue: SHA }, queue: [evaluatePr] }), result(`${ISSUE}.1`, "ok", evaluated({ head: SHA2, checks: "pending", at: "2000-01-01T00:00:00.000Z" })));
  assert.deepEqual([lagging.commands, lagging.store.work!.pendingDue], [[], SHA]);
  const moved = read({ pendingDue: SHA2 }, "failed");
  assert.deepEqual([moved.store.work!.pendingDue, moved.store.work!.deadlines.some((d) => d.kind === "recheck")], [undefined, false]);
});

test("a finishing worker's PR is read neither by an own deadline armed before it opened, nor a recheck, but at its exit", () => {
  const opened = apply(given({ work: "running", facts: { pr: pr("closed"), prVersion: 1 }, armed: own }), ev("prOpened", { pr: 7, head: SHA, repo: "repo", at: T1 })).store;
  const due = apply(opened, deadline(opened, "work", "own"));
  assert.deepEqual(due.commands, ["observe", "readIssue"]);
  let s = due.store;
  for (const c of [...s.queue]) s = apply(s, result(c.id, "failed")).store;
  assert.deepEqual(s.queue.map((c) => c.kind), []);
  // The opening armed a recheck for the hold's end; one that comes before it is armed again for it.
  const rechecked = apply(s, deadline(s, "work", "recheck"));
  assert.deepEqual(rechecked.commands, []);
  assert.equal(rechecked.store.work!.deadlines.find((d) => d.kind === "recheck")?.dueMs, s.work!.finishing! + HOUR);
  // A move between the agent states keeps it, and at the hold's end it reads the PR.
  const paused = apply(rechecked.store, ev("agentIdle", {})).store;
  assert.equal(paused.work!.state, "idle");
  assert.deepEqual(apply({ ...paused, work: { ...paused.work!, finishing: NOW - HOUR } }, deadline(paused, "work", "recheck")).commands, ["evaluatePr"]);
  assert.deepEqual(apply(s, ev("agentExited", {})).commands, ["evaluatePr"]);
  // Given more to do, it isn't finishing: the PR is read once the line is sent.
  assert.equal(apply(s, requested()).store.work!.finishing, undefined);
});

test("a PR fact that lands where the work doesn't evaluate is evaluated on its next entry into a state that does", () => {
  for (const work of ["held"] as const) {
    const landed = apply(given({ work, facts: { pr: pr() } }), ev("prReviewed", { pr: 7, sha: SHA, reviewer: true, copilot: false, at: T1 }));
    assert.deepEqual(landed.commands, [], work);
    const back = apply(landed.store, ev("agentWorking", {}));
    assert.equal(back.work, "running");
    assert.deepEqual(back.commands, ["evaluatePr"], work);
    const read = apply(back.store, result(back.store.queue.at(-1)!.id, "ok", evaluated()));
    assert.deepEqual(read.commands, [], `${work}: read once`);
  }
});

test("a PR read waits behind the queue, and one whose result lands where the work doesn't evaluate leaves its facts unread", () => {
  const busy = apply(given({ work: "rest", facts: { pr: pr() }, queue: [{ kind: "readComment", machine: "work" }] }), ev("prReviewed", { pr: 7, sha: SHA, reviewer: true, copilot: false, at: T1 }));
  assert.deepEqual(busy.commands, [], "the read waits for the comment's");
  const freed = apply(busy.store, result(`${ISSUE}.1`, "failed"));
  assert.deepEqual(freed.commands.filter((c) => c === "evaluatePr"), ["evaluatePr"]);
  // A read sent at version 1 lands after the work moved to held: dropped, its facts still unread.
  const held = apply(given({ work: "held", facts: { pr: pr(), prVersion: 1 }, queue: [{ ...evaluatePr, params: { version: 1 } }] }), result(`${ISSUE}.1`, "ok", evaluated({ feedback: { at: T1, reviewerAt: T1 } })));
  assert.deepEqual(held.store.work!.prEvaluated, { version: 0 });
  const back = apply(held.store, ev("agentWorking", {}));
  assert.deepEqual(back.commands, ["evaluatePr"]);
});

test("a read's auto-merge is kept for the PR it read, and an older CLI's read without it keeps what is held", () => {
  const read = (over: object) => apply(given({ work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }), result(`${ISSUE}.1`, "ok", evaluated(over))).store;
  assert.deepEqual(read({ autoMerge: false }).facts.autoMerge, { pr: 7, on: false });
  assert.equal(read({}).facts.autoMerge, null);
});
