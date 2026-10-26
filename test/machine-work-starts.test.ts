import assert from "node:assert/strict";
import { test } from "node:test";
import { apply, check, deadline, given, ev, HOUR, ISSUE, NOW, result, S, T0, T1, T2, type Case } from "./machine-fixture.ts";
import { halted, own, requested } from "./machine-work-fixture.ts";

// A launch's failure `listen` told the session itself (`said`) gets no notice, beyond the
// doc's one case per row; the next request starts it again.
const by = { session: S, at: T1 };
const said: Case[] = [
  { row: "launching commandResult", when: "`startWorker` failed after making the worktree, said", given: { work: "launching", w: { requestedBy: by }, queue: [{ kind: "startWorker", machine: "work", params: { session: S } }] }, event: result(`${ISSUE}.1`, "failed", { madeWorktree: true, said: true }), expect: { work: "exited", commands: [], workOwn: 24 * HOUR } },
  { row: "launching commandResult", when: "`startWorker` failed, said", given: { work: "launching", facts: { worktree: null }, w: { requestedBy: by }, queue: [{ kind: "startWorker", machine: "work", params: { session: S } }] }, event: result(`${ISSUE}.1`, "failed", { madeWorktree: false, said: true }), expect: { work: "reaped", commands: [], workOwn: null } },
  { row: "launching commandResult", when: "`resumeWorker` failed, said", given: { work: "launching", w: { launch: "resume", requestedBy: by }, queue: [{ kind: "resumeWorker", machine: "work", params: { session: S } }] }, event: result(`${ISSUE}.1`, "failed", { said: true }), expect: { work: "rest", commands: [], workOwn: 24 * HOUR } },
  { row: "launching commandResult", when: "`resumeWorker` failed, said in a session older than the newest request", given: { work: "launching", w: { launch: "resume", requestedBy: { session: "s-newer", at: T2 } }, queue: [{ kind: "resumeWorker", machine: "work", params: { session: S } }] }, event: result(`${ISSUE}.1`, "failed", { said: true }), expect: { work: "rest", commands: ["postNotice:couldntPass"], workOwn: 24 * HOUR } },
];
for (const c of said) test(`work: ${c.row}, ${c.when}`, () => check(c));

test("a start that failed and said so is started again by the reply, its worktree left or removed", () => {
  for (const madeWorktree of [true, false]) {
    const failed = apply(given({ work: "launching", facts: { worktree: null }, w: { requestedBy: by }, queue: [{ kind: "startWorker", machine: "work", params: { session: S } }] }), result(`${ISSUE}.1`, "failed", { madeWorktree, said: true }));
    assert.deepEqual([failed.store.work?.state, failed.commands], [madeWorktree ? "exited" : "reaped", []]);
    assert.deepEqual(apply(failed.store, requested()).commands, ["startWorker"]);
    // Its stall is recorded, so `exited`'s deadline reads the issue rather than saying it again.
    if (madeWorktree) assert.deepEqual(apply(failed.store, deadline(failed.store, "work")).commands, ["readIssue"]);
  }
});

test("a start lost or failed after its agent was seen working began, and only an answer since its request says so", () => {
  const start = { kind: "startWorker" as const, machine: "work" };
  for (const [workedMs, never] of [[NOW, false], [NOW - HOUR, true]] as const) {
    const s = given({ work: "launching", facts: { agent: { status: "working", sinceMs: workedMs, workedMs } }, queue: [start] });
    assert.equal(apply(s, result(`${ISSUE}.1`, "lost")).store.work?.neverBegan, never);
  }
  for (const [at, never] of [[T0, true], [T2, false]] as const) {
    const s = given({ work: "exited", sess: {}, w: { neverBegan: true, requestedBy: { session: S, at: T1 } } });
    assert.equal(apply(s, ev("workerAnswered", { session: S, reader: "worker", at })).store.work?.neverBegan, never);
  }
});

test("a start that named its worktree and launched nothing there drops the marker: no relaunch will come", () => {
  const named = { kind: "startWorker" as const, machine: "work", params: { session: S, orchestrate: false, worktree: "repo::C:/w/tw-1" } };
  const s = given({ work: "launching", w: { neverBegan: true }, queue: [named] });
  const o = apply(s, result(`${ISSUE}.1`, "ok", { worktree: { id: "repo::C:/w/tw-1", branch: "tw-1" }, madeWorktree: false, launched: false }));
  assert.deepEqual([o.store.work?.state, o.store.work?.neverBegan, o.commands], ["running", false, []]);
});

test("a new worker starts with none of the session read, an earlier worker's included", () => {
  const fresh = (sess: object) => apply(given({ work: "reaped", facts: { worktree: null }, sess }), requested()).store.sessions[S].facts;
  assert.equal(fresh({ readThrough: T0, turnReadThrough: T0 }).readThrough, null);
  // A turn's own mark is the turn's.
  assert.equal(fresh({ readThrough: T0, turnReadThrough: T0 }).turnReadThrough, T0);
});

test("the agent fact is its worktree's: a start in another worktree, or a reap, leaves none seen", () => {
  const seen = { agent: { status: "working" as const, sinceMs: NOW - HOUR } };
  const start = (worktree: string) => result(`${ISSUE}.1`, "ok", { worktree: { id: worktree, branch: "tw-1" } });
  const starting = given({ work: "launching", facts: seen, queue: [{ kind: "startWorker", machine: "work" }] });
  assert.equal(apply(starting, start("repo::C:/w/tw-1-b")).store.facts.agent, null);
  assert.equal(apply(starting, start("repo::C:/w/tw-1")).store.facts.agent?.status, "working");
  const merged = given({ work: "merged", facts: seen, queue: [{ kind: "reapWorktree", machine: "work" }] });
  assert.equal(apply(merged, result(`${ISSUE}.1`, "ok", { reaped: true })).store.facts.agent, null);
  // A parent's resume that started a new orchestrator in a new worktree makes that the issue's.
  const launching = given({ work: "launching", w: { launch: "resume" }, facts: seen, queue: [{ kind: "resumeWorker", machine: "work", params: { session: S } }] });
  const resumed = apply(launching, result(`${ISSUE}.1`, "ok", { worktree: { id: "repo::C:/w/tw-1-b", branch: "tw-1" }, launched: true })).store.facts;
  assert.deepEqual([resumed.worktree?.id, resumed.agent], ["repo::C:/w/tw-1-b", null]);
  // Also when a stop landed first: its next stop must name the worktree that runs.
  const stopped = given({ work: "halted", facts: { halted, worktree: { id: "repo::C:/w/tw-1", branch: "tw-1", version: 1 } }, queue: [{ kind: "resumeWorker", machine: "work", version: 1 }] });
  assert.equal(apply(stopped, result(`${ISSUE}.1`, "ok", { worktree: { id: "repo::C:/w/tw-1-b", branch: "tw-1" }, launched: true })).store.facts.worktree?.id, "repo::C:/w/tw-1-b");
});

test("a request's line that failed typing nothing is marked, and one typed or lost is not", () => {
  const line = (out: "failed" | "lost", data?: object) => {
    const s = given({ req: "claimed", work: "running", queue: [{ kind: "deliverMessage", machine: "work", params: { session: S, at: T0 } }] });
    return apply(s, result(`${ISSUE}.1`, out, data)).store.sessions[S].facts.undelivered;
  };
  assert.equal(line("failed"), T0);
  assert.equal(line("failed", { typed: true }), undefined);
  assert.equal(line("lost"), undefined);
});

test("a request decided again tells its own outcome: the exit's deadline and a failed resume add no stalled", () => {
  const exited = given({ req: "queued", sess: { waiting: "slot" }, work: "exited", armed: own });
  const deciding = apply(exited, deadline(exited, "work"));
  assert.deepEqual([deciding.work, deciding.commands], ["exited", []]);
  const done = given({ req: "done", work: "exited", armed: own });
  assert.deepEqual(apply(done, deadline(done, "work")).commands, []);
  const resume = apply(given({ req: "claimed", work: "launching", w: { launch: "resume" }, queue: [{ kind: "resumeWorker", machine: "work", params: { session: S } }] }), result(`${ISSUE}.1`, "failed"));
  assert.deepEqual([resume.req, resume.commands], ["done", ["postNotice:couldntPass"]]);
});

test("a stall left to its request marks the work, not the notices: the request's own stalled still posts after a resume", () => {
  const exited = given({ req: "queued", sess: { waiting: "slot" }, work: "exited", armed: own });
  const deciding = apply(exited, deadline(exited, "work")).store;
  assert.deepEqual([deciding.work?.stalled, deciding.facts.notices], [true, []]);
  const resumed = apply(given({ req: "claimed", work: "running", facts: { notices: deciding.facts.notices } }), ev("agentExited", {}));
  assert.deepEqual([resumed.req, resumed.commands], ["done", ["postNotice:stalled"]]);
  assert.equal(resumed.store.work?.stalled, false);
});
