import assert from "node:assert/strict";
import { test } from "node:test";
import { hostScope } from "../src/worker/scope.ts";
import { BY, CHECKS, forwardTiming, KINDS, MOST_CHECKS, parseTiming } from "../src/worker/timing.ts";
import contract from "./contract.json" with { type: "json" };
import { MemoryStorage } from "./storage.ts";

// The contract with the CLI (cli/src/timing.rs): its report reads as test/contract.json holds
// it, and goes on to `listen` as the forward, without the worktree.
const { report, forward } = contract.timing;

const sent = (): { messages: unknown[]; send(message: string): void } => ({ messages: [], send(message: string) { this.messages.push(JSON.parse(message)); } });

test("a session's timing goes on to listen as the contract's forward, and is kept nowhere", async () => {
  const storage = new MemoryStorage();
  await storage.put(`worktree:${report.worktreeId}`, forward.issueId);
  const before = [...storage.data.keys()];
  const to = sent();
  const outcome = await forwardTiming(storage, to, report);
  assert.equal(outcome.status, 200);
  assert.deepEqual(to.messages, [forward]);
  assert.deepEqual([...storage.data.keys()], before);
  assert.ok(!JSON.stringify(outcome.log).includes("D:/"));
});

test("the kinds, checks and cap are the contract's, which the CLI holds to too", () => {
  assert.deepEqual([KINDS, CHECKS, BY, MOST_CHECKS], [contract.timing.kinds, contract.timing.checks, contract.timing.by, contract.timing.mostChecks]);
});

test("only the checked fields go on", async () => {
  const storage = new MemoryStorage();
  await storage.put(`worktree:${report.worktreeId}`, forward.issueId);
  const to = sent();
  await forwardTiming(storage, to, { ...report, extra: "D:/wt", timing: { ...report.timing, path: "D:/wt", tools: { ...report.timing.tools, text: 1 } } });
  assert.deepEqual(to.messages, [forward]);
});

test("a timing of the wrong shape is refused", () => {
  const bent = (timing: Record<string, unknown>) => ({ ...report, timing: { ...report.timing, ...timing } });
  for (const body of [
    bent({ session: "has space" }),
    bent({ session: "has_underscore" }),
    bent({ session: "-leading" }),
    bent({ kind: "later" }),
    bent({ build: "abc" }),
    bent({ wall: -1 }),
    bent({ at: "yesterday" }),
    bent({ tools: { ...report.timing.tools, git: "2" } }),
    bent({ offsets: { ...report.timing.offsets, pr: "soon" } }),
    bent({ checks: [{ ...report.timing.checks[0], kind: "lint" }] }),
    bent({ checks: Array(1001).fill(report.timing.checks[0]) }),
    { ...report, worktreeId: "no host or repo" },
    { ...report, op: "record" },
  ]) {
    assert.equal(parseTiming(body), null, JSON.stringify(body).slice(0, 200));
  }
  assert.deepEqual(parseTiming(report), { worktreeId: report.worktreeId, timing: report.timing });
});

test("with no connection to take it the report is answered 503, and one whose worktree isn't indexed is dropped", async () => {
  const storage = new MemoryStorage();
  const unindexed = sent();
  assert.equal((await forwardTiming(storage, unindexed, report)).status, 200);
  assert.deepEqual(unindexed.messages, []);
  await storage.put(`worktree:${report.worktreeId}`, forward.issueId);
  assert.equal((await forwardTiming(storage, null, report)).status, 503);
  const dead = { send: () => { throw new Error("closed"); } };
  assert.equal((await forwardTiming(storage, dead, report)).status, 503);
  assert.equal((await forwardTiming(storage, sent(), { ...report, timing: {} })).status, 400);
});

test("a host reports only its own workers' timing", async () => {
  const storage = new MemoryStorage();
  assert.equal(await hostScope(storage, "wsl", { ...report, worktreeId: "wsl@repo::/w/a" }, 1), null);
  for (const worktreeId of ["mac@repo::/w/b", "repo::C:/w/c"]) assert.equal((await hostScope(storage, "wsl", { ...report, worktreeId }, 1))?.status, 403, worktreeId);
});
