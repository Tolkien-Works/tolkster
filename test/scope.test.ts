import assert from "node:assert/strict";
import { test } from "node:test";
import { newStore } from "../src/worker/machine/ctx.ts";
import { hostScope } from "../src/worker/scope.ts";
import { MemoryStorage } from "./storage.ts";

const AT = "2026-10-21T00:00:00.000Z";
const TOKEN = "a".repeat(16);
const UUID = "0b9d7b5e-4f8a-4a3c-8c1e-5d2f6a7b8c9d";
const launch = { model: "opus", effort: "high" };

// Three issues, each with a session and a worker: one on host `wsl`, one on `mac` and one on
// the workstation, whose worktree ID has no host.
const WORKTREES = { wsl: "wsl@repo::/w/a", mac: "mac@repo::/w/b", local: "repo::C:/w/c" };

async function seeded(): Promise<MemoryStorage> {
  const storage = new MemoryStorage();
  for (const [name, worktreeId] of Object.entries(WORKTREES)) {
    const store = newStore(`i-${name}`);
    store.facts.worktree = { id: worktreeId, branch: name, version: 1 };
    await storage.put(`machine:issue:i-${name}`, store);
    await storage.put(`intake:session:s-${name}`, `i-${name}`);
    await storage.put(`worker:${worktreeId}`, { issue: "TW-1", worktreeId, sessionId: `s-${name}`, at: AT, launch, rungFrom: "claim" });
  }
  // A session the Durable Object holds no issue for.
  await storage.put("intake:session:s-orphan", "i-gone");
  return storage;
}

const call = async (storage: MemoryStorage, body: unknown) => hostScope(storage, "wsl", body, 1);

test("a host's credential passes its own workers' reports, reads and records", async () => {
  const storage = await seeded();
  const own = [
    { op: "report", report: "agentWorking", worktreeId: WORKTREES.wsl, at: AT, elapsedMs: 0 },
    { op: "report", report: "workerRead", sessionId: "s-wsl", activityId: "a1", at: AT, reader: "worker" },
    { op: "report", report: "workerAnswered", sessionId: "s-wsl", activityId: "a1", at: AT, reader: "worker" },
    { op: "report", report: "chainRecorded", issueId: "i-wsl", sessionId: "s-wsl", run: "r1", children: ["c1"] },
    { op: "report", report: "landed", issueId: "i-wsl", sessionId: "s-wsl", head: "0123456789abcdef0123456789abcdef01234567" },
    { op: "session", sessionId: "s-wsl" },
    { op: "record", sessionId: "s-wsl", change: { read: { pr: 1, at: AT } } },
    { op: "delegate", delegation: { issueId: "c1", parentIssueId: "i-wsl", sessionId: "s-wsl", at: AT } },
  ];
  for (const body of own) assert.equal(await call(storage, body), null, JSON.stringify(body));
});

test("a host's credential is refused every other host's, the workstation's and the owner's", async () => {
  const storage = await seeded();
  const refused = [
    ...["mac", "local"].flatMap((other) => [
      { op: "report", report: "agentIdle", worktreeId: WORKTREES[other as keyof typeof WORKTREES], at: AT, elapsedMs: 0 },
      { op: "report", report: "workerRead", sessionId: `s-${other}`, activityId: "a1", at: AT, reader: "worker" },
      { op: "report", report: "workerAnswered", sessionId: `s-${other}`, activityId: "a1", at: AT, reader: "worker" },
      { op: "report", report: "chainRecorded", issueId: `i-${other}`, sessionId: `s-${other}`, run: "r1", children: ["c1"] },
      { op: "report", report: "landed", issueId: `i-${other}`, sessionId: `s-${other}`, head: "0123456789abcdef0123456789abcdef01234567" },
        { op: "record", sessionId: `s-${other}`, change: {} },
      { op: "delegate", delegation: { issueId: "c1", parentIssueId: `i-${other}`, sessionId: `s-${other}`, at: AT } },
      { op: "startWorker", worker: { issue: "TW-1", worktreeId: WORKTREES[other as keyof typeof WORKTREES], sessionId: `s-${other}`, at: AT, launch, rungFrom: "claim" } },
      { op: "forgetWorker", worktreeId: WORKTREES[other as keyof typeof WORKTREES] },
    ]),
    // A session no issue is held for, and a turn's report on the host's own.
    { op: "record", sessionId: "s-wsl", change: { conversation: { agentSession: UUID, terminal: "term_1", at: AT } } },
    { op: "delegate", delegation: { issueId: "c1", parentIssueId: "i-wsl", sessionId: "s-mac", at: AT } },
    { op: "report", report: "chainRecorded", issueId: "i-wsl", sessionId: "s-mac", run: "r1", children: ["c1"] },
    { op: "report", report: "landed", issueId: "i-wsl", sessionId: "s-mac", head: "0123456789abcdef0123456789abcdef01234567" },
    { op: "report", report: "workerRead", sessionId: "s-wsl", activityId: "a1", at: AT, reader: "turn" },
    { op: "report", report: "turnIdle", sessionId: "s-wsl", terminal: "term_1", at: AT },
    // What only `listen` reports, on its socket.
    { op: "report", report: "config", cap: 3, slots: {}, repos: {}, machines: {} },
    { op: "report", report: "worktreeGone", worktreeId: WORKTREES.wsl, sequence: 1 },
    { op: "report", report: "commandResult", id: "c1", issueId: "i-wsl", version: 1, result: "ok", detail: {} },
    { op: "take", claims: [{ name: "comment-i-wsl", ttlMs: null }], token: TOKEN },
    { op: "release", claims: ["comment-i-wsl"], token: TOKEN },
    { op: "checkouts" },
    { op: "setCheckout", checkout: { repoId: "repo", worktreeId: "repo::/w", at: AT } },
    { op: "contextComment", issueId: "i-wsl" },
    { op: "setContextComment", comment: { issueId: "i-wsl", commentId: UUID, at: AT } },
    { op: "triaging", session: "s-wsl", identifier: "TW-1" },
    { op: "startWorker", worker: { issue: "TW-1", worktreeId: WORKTREES.wsl, sessionId: "s-wsl", at: AT, launch, rungFrom: "claim" } },
    { op: "forgetWorker", worktreeId: WORKTREES.wsl },
    // An op no host has been given.
    { op: "later" },
  ];
  for (const body of refused) {
    const outcome = await call(storage, body);
    // A body that doesn't parse as an op is left to `handle`, which answers it 400.
    assert.equal(outcome?.status, body.op === "later" ? undefined : 403, JSON.stringify(body));
  }
  // Nothing the refusals tried reached the records.
  assert.ok(await storage.get(`worker:${WORKTREES.mac}`) && await storage.get(`worker:${WORKTREES.local}`) && await storage.get(`worker:${WORKTREES.wsl}`));
});

test("a host reads only its own sessions and delegations, the rest as none", async () => {
  const storage = await seeded();
  await storage.put("delegated:i-wsl", { issueId: "i-wsl", parentIssueId: "p", sessionId: "s-wsl", at: AT, storedMs: 1 });
  await storage.put("delegated:i-mac", { issueId: "i-mac", parentIssueId: "p", sessionId: "s-mac", at: AT, storedMs: 1 });
  await storage.put("session:s-wsl", { reads: [] });
  await storage.put("session:s-mac", { reads: [] });
  const read = JSON.parse((await call(storage, { op: "read", sessions: ["s-wsl", "s-mac", "s-orphan"] }))!.body) as { records: Record<string, unknown> };
  assert.deepEqual(Object.entries(read.records).map(([id, record]) => [id, record !== null]), [["s-wsl", true], ["s-mac", false], ["s-orphan", false]]);
  const delegations = JSON.parse((await call(storage, { op: "delegations", issues: ["i-wsl", "i-mac", "i-local"] }))!.body) as { delegations: Record<string, unknown> };
  assert.deepEqual(Object.entries(delegations.delegations).map(([id, d]) => [id, d !== null]), [["i-wsl", true], ["i-mac", false], ["i-local", false]]);
  assert.deepEqual(JSON.parse((await call(storage, { op: "read", sessions: ["s-mac"] }))!.body), { records: { "s-mac": null } });
});

test("a worker whose start the store dropped is still its host's, by the worktree index", async () => {
  const storage = await seeded();
  const store = newStore("i-lost");
  await storage.put("machine:issue:i-lost", store);
  await storage.put("intake:session:s-lost", "i-lost");
  const body = { op: "session", sessionId: "s-lost" };
  assert.equal((await call(storage, body))?.status, 200);
  await storage.put(`worktree:${WORKTREES.wsl}x`, "i-lost");
  assert.equal(await call(storage, body), null);
  assert.equal((await hostScope(storage, "mac", body, 1))?.status, 200);
});

test("a host reads only its own worker records", async () => {
  const storage = await seeded();
  const outcome = await call(storage, { op: "workers" });
  assert.equal(outcome?.status, 200);
  const { workers } = JSON.parse(outcome!.body) as { workers: { worktreeId: string }[] };
  assert.deepEqual(workers.map((w) => w.worktreeId), [WORKTREES.wsl]);
  // A host whose name is another's prefix reads none of it.
  assert.deepEqual(JSON.parse((await hostScope(storage, "ws", { op: "workers" }, 1))!.body), { workers: [] });
  // A session that isn't its own reads as one nothing is recorded for.
  for (const [who, sessionId] of [["ws", "s-wsl"], ["wsl", "s-mac"], ["wsl", "s-local"], ["wsl", "s-orphan"]]) {
    assert.deepEqual(JSON.parse((await hostScope(storage, who, { op: "session", sessionId }, 1))!.body), { readThrough: null, turnReadThrough: null, answeredThrough: null }, sessionId);
  }
});
