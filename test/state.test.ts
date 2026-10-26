import assert from "node:assert/strict";
import { test } from "node:test";
import {
  applyChange,
  type Change,
  type CheckoutRecord,
  type ContextComment,
  CLAIM_KEEP_MS,
  type Conversation,
  type Delegation,
  DELEGATION_KEEP_MS,
  delegated,
  forgetGone,
  goneWorkers,
  handle,
  HARNESSES,
  keptAtReset,
  type Launch,
  markMissing,
  MISSING_KEEP_MS,
  recordSessionEvent,
  type Read,
  type RungFrom,
  SESSION_ISSUE_KEEP_MS,
  SESSION_ISSUE_REFRESH_MS,
  type SessionRecord,
  tolkstersIssues,
  unpairedWorkers,
  type WorkerRecord,
  WORKTREE,
} from "../src/worker/state.ts";
import { fromIssue } from "../src/worker/events.ts";
import { PRUNE_EVERY_MS, resetOnce, STORAGE_VERSION } from "../src/worker/storage.ts";
import { newStore } from "../src/worker/machine/ctx.ts";
import { keptAtReset as machinesKept, lostAtReset } from "../src/worker/machine/store.ts";
import { newTriage, TRIAGE_KEY } from "../src/worker/machine/triage.ts";
import { TERMINAL } from "../src/worker/machine/validate.ts";
import contract from "./contract.json" with { type: "json" };
import { keys } from "./keys.ts";
import { MemoryStorage } from "./storage.ts";

const at = (minute: number) => `2026-09-26T01:${String(minute).padStart(2, "0")}:00.000Z`;

async function ok(storage: MemoryStorage, body: unknown, nowMs = 0): Promise<Record<string, unknown>> {
  const outcome = await handle(storage, body, nowMs);
  assert.equal(outcome.status, 200, outcome.body);
  return JSON.parse(outcome.body) as Record<string, unknown>;
}

const keysOf = (value: object): string[] => Object.keys(value).sort();

// The contract with the CLI: test/contract.json holds every shape as the CLI writes it
// (cli/src/state.rs reads and writes each back as itself), each with every field the
// types here have, and the Durable Object accepts and keeps each as written.
test("the contract fixture names every field of each shape, and is stored as written", async () => {
  const { change, record } = contract;
  assert.deepEqual(keysOf(change), keys<Change>({ read: true, conversation: true }));
  assert.deepEqual(keysOf(record), keys<SessionRecord>({ reads: true, conversation: true }));
  assert.deepEqual(keysOf(change.conversation), keys<Conversation>({ agentSession: true, terminal: true, at: true }));
  assert.deepEqual(keysOf(change.read), keys<Read>({ pr: true, at: true }));
  assert.deepEqual(keysOf(record.reads[0]), keys<Read>({ pr: true, at: true }));
  // `mode` is only ever read back, never sent by the CLI since.
  assert.deepEqual(contract.harnesses, HARNESSES);
  assert.deepEqual(contract.enums.rungFrom, keys<Record<RungFrom, true>>({ claim: true, noEstimate: true, worker: true }));
  assert.deepEqual(keysOf(contract.worker), keys<WorkerRecord>({ issue: true, worktreeId: true, sessionId: true, at: true, launch: true, rungFrom: true }));
  assert.deepEqual(keysOf(contract.worker.launch), keys<Launch>({ harness: true, model: true, effort: true }));
  assert.deepEqual(keysOf(contract.delegation), keys<Delegation>({ issueId: true, parentIssueId: true, sessionId: true, at: true }));
  assert.deepEqual(keysOf(contract.checkout), keys<CheckoutRecord>({ repoId: true, worktreeId: true, at: true }));
  assert.deepEqual(keysOf(contract.contextComment), keys<ContextComment>({ issueId: true, commentId: true, at: true }));
  assert.deepEqual(keysOf(contract.triaging), ["identifier", "session"]);

  // Stored as written: the change, applied to a session with no record, is the record.
  const storage = new MemoryStorage();
  await ok(storage, { op: "record", sessionId: "s-1", change });
  assert.deepEqual(await ok(storage, { op: "read", sessions: ["s-1"] }), { records: { "s-1": record } });
  await ok(storage, { op: "startWorker", worker: contract.worker });
  assert.deepEqual(await ok(storage, { op: "workers" }), { workers: [contract.worker] });
  await ok(storage, { op: "delegate", delegation: contract.delegation });
  assert.deepEqual(await ok(storage, { op: "delegations", issues: [contract.delegation.issueId] }), { delegations: { [contract.delegation.issueId]: contract.delegation } });
  await ok(storage, { op: "setCheckout", checkout: contract.checkout });
  assert.deepEqual(await ok(storage, { op: "checkouts" }), { checkouts: [contract.checkout] });
  await ok(storage, { op: "setContextComment", comment: contract.contextComment });
  assert.deepEqual(await ok(storage, { op: "contextComment", issueId: contract.contextComment.issueId }), { comment: contract.contextComment });
  assert.deepEqual(await ok(storage, { op: "triaging", ...contract.triaging }), { holds: false });
  storage.data.set(TRIAGE_KEY, { ...newTriage(), state: "running", session: { id: contract.triaging.session, terminal: "term_1", issues: [contract.triaging.identifier] } });
  assert.deepEqual(await ok(storage, { op: "triaging", ...contract.triaging }), { holds: true });
});

// The contract with the CLI (cli/src/plain.rs, split_worktree): a worktree's ID with and
// without its host, a POSIX path included, and the near misses neither side takes.
test("a worktree's ID is taken with or without its host, as the CLI takes it", () => {
  for (const { id } of contract.worktreeIds) assert.ok(WORKTREE.test(id), id);
  for (const id of contract.notWorktreeIds) assert.ok(!WORKTREE.test(id), id);
});

// The contract with the CLI (cli/src/plain.rs, split_terminal): a terminal's handle with and
// without its host.
test("a terminal's handle is taken with or without its host, as the CLI takes it", () => {
  for (const { handle } of contract.terminals) assert.ok(TERMINAL.test(handle), handle);
  for (const handle of contract.notTerminals) assert.ok(!TERMINAL.test(handle), handle);
});

test("a PR's newest read is kept, one per PR, and an older write changes nothing", () => {
  let record: SessionRecord = {};
  const read = (pr: number, minute: number) => {
    record = applyChange(record, { read: { pr, at: at(minute) } });
  };
  read(7, 2);
  read(9, 3);
  read(7, 4);
  assert.deepEqual(record.reads, [{ pr: 7, at: at(4) }, { pr: 9, at: at(3) }]);
  // Repeated, or late: kept as it is.
  read(7, 4);
  read(7, 1);
  assert.deepEqual(record.reads, [{ pr: 7, at: at(4) }, { pr: 9, at: at(3) }]);
});

async function take(storage: MemoryStorage, claims: unknown[], token: string, nowMs: number): Promise<boolean> {
  const body = await ok(storage, { op: "take", claims, token }, nowMs);
  return body.taken as boolean;
}

const RUN_A = "run-a-0123456789abcdef";
const RUN_B = "run-b-0123456789abcdef";

test("a claim is taken once, all together or not at all, and a stale one is taken over", async () => {
  const storage = new MemoryStorage();
  const request = { name: `request-s-1@${at(1)}`, ttlMs: null };
  const start = { name: "start-TW-1", ttlMs: 15 * 60_000 };
  assert.equal(await take(storage, [start, request], RUN_A, 0), true);
  assert.equal(await take(storage, [request], RUN_B, 1000), false);
  // Another request on the same issue: the issue's start is held, so neither is taken.
  const other = { name: `request-s-2@${at(2)}`, ttlMs: null };
  assert.equal(await take(storage, [start, other], RUN_B, 1000), false);
  assert.equal(await take(storage, [other], RUN_B, 1000), true);
  // Released with its token, it can be taken again.
  await ok(storage, { op: "release", claims: [request.name], token: RUN_A });
  assert.equal(await take(storage, [request], RUN_B, 2000), true);
  // Claims older than 30 days are pruned.
  await take(storage, [{ name: "review-TW-1-pr7@none", ttlMs: 600_000 }], RUN_A, CLAIM_KEEP_MS + 20 * 60_000);
  assert.deepEqual(claims(storage), ["claim:review-TW-1-pr7@none"]);
});

test("a stale claim is taken over by its holder's TTL, whatever TTL the taker sends", async () => {
  const storage = new MemoryStorage();
  const answer = { name: "request-s-1@a", ttlMs: 20 * 60_000 };
  const good = { name: "request-s-2@a", ttlMs: null };
  assert.equal(await take(storage, [answer, good], RUN_A, 0), true);
  // A taker for good doesn't keep out a claim its holder took for 20 minutes, nor is
  // one taken for good taken over by a taker with a TTL.
  assert.equal(await take(storage, [{ ...answer, ttlMs: null }], RUN_B, 21 * 60_000), true);
  assert.equal(await take(storage, [{ ...good, ttlMs: 20 * 60_000 }], RUN_B, 21 * 60_000), false);
});

const claims = (storage: MemoryStorage) => [...storage.data.keys()].filter((k) => k.startsWith("claim:")).sort();

test("claims are pruned at most once an hour", async () => {
  const storage = new MemoryStorage();
  assert.equal(await take(storage, [{ name: "start-TW-1", ttlMs: 1000 }], RUN_A, 0), true);
  // Pruned just now: the old claim stays until an hour after that.
  const late = CLAIM_KEEP_MS + 2;
  await take(storage, [{ name: "start-TW-2", ttlMs: 1000 }], RUN_A, 1);
  await take(storage, [{ name: "start-TW-3", ttlMs: 1000 }], RUN_A, PRUNE_EVERY_MS - 1);
  assert.deepEqual(claims(storage), ["claim:start-TW-1", "claim:start-TW-2", "claim:start-TW-3"]);
  // Three takes within the hour pruned once: the claims, the delegations and the session
  // issues listed once each.
  assert.equal(storage.lists, 3);
  await take(storage, [{ name: "start-TW-4", ttlMs: 1000 }], RUN_A, late);
  assert.equal(storage.lists, 6);
  assert.deepEqual(claims(storage), ["claim:start-TW-3", "claim:start-TW-4"]);
});

test("any claim name of safe characters is accepted, and nothing else", async () => {
  const storage = new MemoryStorage();
  assert.equal(await take(storage, [{ name: "notice-s-1@2026-09-26T01:00:00.000Z:busy:0123", ttlMs: 60_000 }], RUN_A, 0), true);
  for (const name of ["", "a b", "x/y", "é", "a".repeat(209)]) {
    const outcome = await handle(storage, { op: "take", claims: [{ name, ttlMs: null }], token: RUN_A }, 0);
    assert.equal(outcome.status, 400, name);
    const released = await handle(storage, { op: "release", claims: [name], token: RUN_A }, 0);
    assert.equal(released.status, 400, name);
  }
});

test("a read of many sessions is batched", async () => {
  const storage = new MemoryStorage();
  const sessions = Array.from({ length: 300 }, (_, i) => `s-${i}`);
  await ok(storage, { op: "record", sessionId: "s-7", change: { read: { pr: 1, at: at(1) } } });
  await ok(storage, { op: "record", sessionId: "s-250", change: { read: { pr: 2, at: at(2) } } });
  storage.gets = 0;
  const { records } = (await ok(storage, { op: "read", sessions })) as { records: Record<string, unknown> };
  assert.equal(storage.gets, 3);
  assert.equal(Object.keys(records).length, 300);
  assert.deepEqual(records["s-7"], { reads: [{ pr: 1, at: at(1) }] });
  assert.deepEqual(records["s-250"], { reads: [{ pr: 2, at: at(2) }] });
  assert.equal(records["s-0"], null);
});

test("a take can be retried, and a stale claim taken over can't be released by the run that lost it", async () => {
  const storage = new MemoryStorage();
  const start = { name: "start-TW-1", ttlMs: 15 * 60_000 };
  assert.equal(await take(storage, [start], RUN_A, 0), true);
  // The response was lost, and the take is sent again: still taken, from the first time.
  assert.equal(await take(storage, [start], RUN_A, 1000), true);
  // Past its TTL, the start is taken over by another run.
  assert.equal(await take(storage, [start], RUN_B, 15 * 60_000 + 1), true);
  // The first run fails late and gives its claim back: the newer holder keeps it.
  await ok(storage, { op: "release", claims: [start.name], token: RUN_A });
  assert.equal(await take(storage, [start], RUN_A, 15 * 60_000 + 2), false);
  await ok(storage, { op: "release", claims: [start.name], token: RUN_B });
  assert.equal(await take(storage, [start], RUN_A, 15 * 60_000 + 3), true);
});

test("only IDs, times, enums and identifiers are stored", async () => {
  const storage = new MemoryStorage();
  // Unknown fields are dropped, not stored.
  await ok(storage, { op: "record", sessionId: "s-1", change: { read: { pr: 7, at: at(1), body: "some review text" }, body: "some issue text" } });
  assert.deepEqual(await ok(storage, { op: "read", sessions: ["s-1"] }), { records: { "s-1": { reads: [{ pr: 7, at: at(1) }] } } });
  const refused = [
    { op: "record", sessionId: "s 1", change: {} },
    { op: "record", sessionId: "s-1", change: { read: { pr: 7, at: "2026-09-26T01:00:00Z" } } },
    { op: "record", sessionId: "s-1", change: { read: { pr: -1, at: at(1) } } },
    { op: "record", sessionId: "s-1", change: { conversation: { agentSession: "not a uuid", terminal: null, at: at(1) } } },
    // What the session records kept before the machines is refused, not stored.
    { op: "dues" },
    { op: "open" },
    { op: "take", claims: [{ name: "anything at all", ttlMs: null }], token: RUN_A },
    { op: "take", claims: [{ name: "start-TW-1", ttlMs: null }] },
    { op: "release", claims: ["start-TW-1"] },
    { op: "release", claims: ["start-TW-1"], token: "not a token" },
    { op: "drop" },
    // An op that no longer exists is refused like any unknown one.
    { op: "migrate", records: {} },
    [],
  ];
  for (const body of refused) assert.equal((await handle(storage, body, 0)).status, 400, JSON.stringify(body));
});

test("a worker's record is kept per worktree until it is forgotten", async () => {
  const storage = new MemoryStorage();
  const worktreeId = "90c17eac-c327-4dda-8e34-d6f5eede5c4e::D:/source/worktrees/tolkster/tw-1";
  const launch = { harness: "claude", model: "claude-sonnet-5-5", effort: "high" };
  const first = { issue: "TW-1", worktreeId, sessionId: "s-1", at: at(1), launch, rungFrom: "claim" };
  const other = { issue: "TW-2", worktreeId: "r1::C:/work trees/tw-2", sessionId: "s-2", at: at(2), launch, rungFrom: "noEstimate" };
  await ok(storage, { op: "startWorker", worker: { ...first, note: "some issue text" } });
  await ok(storage, { op: "startWorker", worker: other });
  assert.deepEqual(await ok(storage, { op: "workers" }), { workers: [first, other] });
  // Started again at the same path: the newer record replaces it.
  const work = { ...first, sessionId: "s-3", at: at(3) };
  await ok(storage, { op: "startWorker", worker: work });
  // Forgetting deletes the record, once.
  assert.deepEqual(await ok(storage, { op: "forgetWorker", worktreeId: other.worktreeId }, 0), { forgotten: true });
  assert.deepEqual(await ok(storage, { op: "forgetWorker", worktreeId: other.worktreeId }, 60_000), { forgotten: false });
  assert.deepEqual(await ok(storage, { op: "forgetWorker", worktreeId: "r1::D:/wt/never" }, 0), { forgotten: false });
  assert.deepEqual(await ok(storage, { op: "workers" }), { workers: [work] });
  assert.equal(storage.data.has(`worker:${other.worktreeId}`), false);
  // A new start at a forgotten worktree's path is a record again.
  await ok(storage, { op: "forgetWorker", worktreeId });
  await ok(storage, { op: "startWorker", worker: first });
  assert.deepEqual(await ok(storage, { op: "workers" }), { workers: [first] });
  // What it was launched with is kept, and nothing else inside it.
  await ok(storage, { op: "startWorker", worker: { ...first, launch: { ...launch, note: "text" } } });
  assert.deepEqual(await ok(storage, { op: "workers" }), { workers: [first] });
  // A launch with no harness, from a CLI older than this, is Claude's.
  await ok(storage, { op: "startWorker", worker: { ...first, launch: { model: launch.model, effort: launch.effort } } });
  assert.deepEqual(await ok(storage, { op: "workers" }), { workers: [first] });
  const refused = [
    { op: "startWorker", worker: { ...work, issue: "Fix the thing" } },
    { op: "startWorker", worker: { ...work, worktreeId: "D:/wt/tw-1" } },
    { op: "startWorker", worker: { ...work, worktreeId: "r1::D:/wt/tw-1\nx" } },
    { op: "startWorker", worker: { ...work, worktreeId: "r1::D:/wt/tw-1\u0085x" } },
    { op: "startWorker", worker: { ...work, worktreeId: undefined } },
    { op: "startWorker", worker: { ...work, worktreeId: null } },
    { op: "startWorker", worker: { ...work, at: "yesterday" } },
    // A launch is a model's plain name and an effort, never flags or text.
    { op: "startWorker", worker: { ...work, launch: { model: "opus --dangerously-skip-permissions", effort: "high" } } },
    { op: "startWorker", worker: { ...work, launch: { model: "-opus", effort: "high" } } },
    { op: "startWorker", worker: { ...work, launch: { model: "opus", effort: "extreme" } } },
    { op: "startWorker", worker: { ...work, launch: { harness: "stub", model: "opus", effort: "high" } } },
    { op: "startWorker", worker: { ...work, launch: { harness: "toString", model: "opus", effort: "high" } } },
    // Where the rung's size came from is an enum, never a size or text.
    { op: "startWorker", worker: { ...work, rungFrom: "S" } },
    { op: "startWorker", worker: { ...work, rungFrom: undefined } },
    { op: "startWorker", worker: { ...work, launch: undefined } },
    { op: "startWorker", worker: { ...work, launch: { model: "opus" } } },
    { op: "startWorker", worker: { ...work, launch: "opus" } },
    { op: "forgetWorker" },
  ];
  for (const body of refused) assert.equal((await handle(storage, body, 0)).status, 400, JSON.stringify(body));
});

test("a conversation keeps the agent session its first turn chose, and its latest turn's terminal", () => {
  const agentSession = "0f8a3c2e-9b1d-4e7a-8c5f-2d6b9e1a4c3f";
  let record = applyChange({}, { conversation: { agentSession, terminal: null, at: at(1) } });
  record = applyChange(record, { conversation: { agentSession, terminal: "term_1", at: at(2) } });
  assert.deepEqual(record.conversation, { agentSession, terminal: "term_1", at: at(2) });
  // A later turn can't choose another session; an older write changes nothing.
  record = applyChange(record, { conversation: { agentSession: "1f8a3c2e-9b1d-4e7a-8c5f-2d6b9e1a4c3f", terminal: "term_2", at: at(3) } });
  record = applyChange(record, { conversation: { agentSession, terminal: "term_0", at: at(0) } });
  assert.deepEqual(record.conversation, { agentSession, terminal: "term_2", at: at(3) });
});

test("a conversation holds only a UUID, a terminal handle and a time", async () => {
  const storage = new MemoryStorage();
  const agentSession = "0f8a3c2e-9b1d-4e7a-8c5f-2d6b9e1a4c3f";
  await ok(storage, { op: "record", sessionId: "s-1", change: { conversation: { agentSession, terminal: "term_ab-12", at: at(1), prompt: "some text" } } });
  assert.deepEqual(await ok(storage, { op: "read", sessions: ["s-1"] }), { records: { "s-1": { conversation: { agentSession, terminal: "term_ab-12", at: at(1) } } } });
  for (const conversation of [
    { agentSession: "not a uuid", terminal: null, at: at(1) },
    { agentSession, terminal: "term 1; rm", at: at(1) },
    { agentSession, terminal: null, at: "now" },
  ]) {
    assert.equal((await handle(storage, { op: "record", sessionId: "s-1", change: { conversation } }, 0)).status, 400, JSON.stringify(conversation));
  }
});

test("each repo's conversations checkout is kept, and replaced when made anew", async () => {
  const storage = new MemoryStorage();
  const checkout = { repoId: "r1", worktreeId: "r1::D:/wt/conversations", at: at(1) };
  await ok(storage, { op: "setCheckout", checkout: { ...checkout, note: "x" } });
  await ok(storage, { op: "setCheckout", checkout: { repoId: "r2", worktreeId: "r2::D:/wt/conversations", at: at(1) } });
  const again = { ...checkout, worktreeId: "r1::D:/wt/conversations-2", at: at(2) };
  await ok(storage, { op: "setCheckout", checkout: again });
  assert.deepEqual(await ok(storage, { op: "checkouts" }), { checkouts: [again, { repoId: "r2", worktreeId: "r2::D:/wt/conversations", at: at(1) }] });
  for (const bad of [{ ...checkout, worktreeId: "r2::D:/wt/x" }, { ...checkout, repoId: "r 1" }, { ...checkout, worktreeId: "D:/wt/x" }]) {
    assert.equal((await handle(storage, { op: "setCheckout", checkout: bad }, 0)).status, 400, JSON.stringify(bad));
  }
});

test("an issue's one context comment is kept by its ID, and a replacement overwrites it", async () => {
  const storage = new MemoryStorage();
  const comment = { issueId: "i-1", commentId: "c0c0c0c0-0000-4000-8000-000000000001", at: at(1) };
  assert.deepEqual(await ok(storage, { op: "contextComment", issueId: "i-1" }), { comment: null });
  await ok(storage, { op: "setContextComment", comment: { ...comment, body: "x" } });
  assert.deepEqual(await ok(storage, { op: "contextComment", issueId: "i-1" }), { comment });
  const replaced = { ...comment, commentId: "c0c0c0c0-0000-4000-8000-000000000002", at: at(2) };
  await ok(storage, { op: "setContextComment", comment: replaced });
  assert.deepEqual(await ok(storage, { op: "contextComment", issueId: "i-1" }), { comment: replaced });
  assert.deepEqual(await ok(storage, { op: "contextComment", issueId: "i-2" }), { comment: null });
  for (const bad of [{ ...comment, commentId: "not a uuid" }, { ...comment, issueId: "i 1" }, { ...comment, at: "now" }]) {
    assert.equal((await handle(storage, { op: "setContextComment", comment: bad }, 0)).status, 400, JSON.stringify(bad));
  }
});

test("a triage session holds an issue only while the triage machine runs it with that issue", async () => {
  const storage = new MemoryStorage();
  const session = "0123456789abcdef0123456789abcdef";
  const holds = async (s: string, identifier: string) => (await ok(storage, { op: "triaging", session: s, identifier })).holds;
  assert.equal(await holds(session, "TW-1"), false);
  const running = { ...newTriage(), state: "running", session: { id: session, terminal: "term_1", issues: ["TW-1"] } };
  storage.data.set(TRIAGE_KEY, running);
  assert.equal(await holds(session, "TW-1"), true);
  assert.equal(await holds(session, "TW-2"), false);
  assert.equal(await holds("another", "TW-1"), false);
  // Once it has ended, the next start holds nothing of it.
  storage.data.set(TRIAGE_KEY, { ...running, state: "starting" });
  assert.equal(await holds(session, "TW-1"), false);
  for (const bad of [{ session: "s 1", identifier: "TW-1" }, { session, identifier: "tw-1" }, { session }]) {
    assert.equal((await handle(storage, { op: "triaging", ...bad }, 0)).status, 400, JSON.stringify(bad));
  }
});

test("a record is gone only after its worktree is missing from complete listings for an hour", async () => {
  const storage = new MemoryStorage();
  const hourMs = 60 * 60_000;
  const record = (n: number) => ({ issue: `TW-${n}`, worktreeId: `r1::D:/wt/tw-${n}`, sessionId: `s-${n}`, at: at(n), launch: { harness: "claude", model: "claude-opus-5-5", effort: "medium" }, rungFrom: "claim" });
  await ok(storage, { op: "startWorker", worker: record(1) });
  await ok(storage, { op: "startWorker", worker: record(2) });
  const listing = (listed: number[], nowMs: number) => markMissing(storage, listed.map((n) => `r1::D:/wt/tw-${n}`), [], nowMs);
  const gone = async (nowMs: number) => (await goneWorkers(storage, [], nowMs)).map((g) => g.worktreeId);
  // TW-2 goes missing, and TW-1 once, when a listing predated it.
  await listing([], 0);
  await listing([1], 60_000);
  // Missing for an hour: TW-2 is gone. TW-1 came back, so its hour starts again.
  assert.deepEqual(await gone(hourMs), []);
  assert.deepEqual(await gone(hourMs + 1), ["r1::D:/wt/tw-2"]);
  // A listing that has it before it is deleted clears it, and the delete finds the mark gone.
  await listing([1, 2], hourMs + 1);
  await forgetGone(storage, "r1::D:/wt/tw-2", 0);
  assert.deepEqual(await ok(storage, { op: "workers" }, hourMs + 1), { workers: [record(1), record(2)] });
  await listing([], hourMs + 2);
  assert.deepEqual(await gone(2 * hourMs + 2), []);
  assert.deepEqual(await gone(2 * hourMs + 3), ["r1::D:/wt/tw-1", "r1::D:/wt/tw-2"]);
  await forgetGone(storage, "r1::D:/wt/tw-1", hourMs + 2);
  await forgetGone(storage, "r1::D:/wt/tw-2", hourMs + 2);
  assert.deepEqual(await gone(3 * hourMs), []);
  assert.deepEqual(await ok(storage, { op: "workers" }, 2 * hourMs + 3), { workers: [] });
  // `listen` no longer reconciles or sets a deadline.
  assert.equal((await handle(storage, { op: "reconcile", listed: [] }, 0)).status, 400);
  assert.equal((await handle(storage, { op: "schedule", deadline: { type: "unlisted", worktreeId: null, afterMs: 0 } }, 0)).status, 400);
});

test("a worker on a host that didn't answer, or isn't paired, is neither marked missing nor gone", async () => {
  const storage = new MemoryStorage();
  const record = { issue: "TW-1", worktreeId: "mac@r9::/Users/x/tw-1", sessionId: "s-1", at: at(1), launch: { harness: "claude", model: "claude-opus-5-5", effort: "medium" }, rungFrom: "claim" };
  await ok(storage, { op: "startWorker", worker: record });
  const gone = async (hosts: string[]) => (await goneWorkers(storage, hosts, 2 * MISSING_KEEP_MS)).map((g) => g.worktreeId);
  await markMissing(storage, [], ["linux"], 0);
  assert.deepEqual(await gone(["mac"]), []);
  // Marked while mac answered, then mac falls silent: not gone until it answers again.
  await markMissing(storage, [], ["mac"], 0);
  assert.deepEqual(await gone(["linux"]), []);
  assert.deepEqual(await gone(["mac"]), [record.worktreeId]);
  // Only a host out of the paired is unpaired, and this machine's never is.
  await ok(storage, { op: "startWorker", worker: { ...record, issue: "TW-2", worktreeId: "r1::D:/wt/tw-2" } });
  assert.deepEqual(await unpairedWorkers(storage, ["mac"]), []);
  assert.deepEqual(await unpairedWorkers(storage, ["linux"]), [{ worktreeId: record.worktreeId, missingMs: 0 }]);
});

test("each outcome says what the log keeps: the op and its counts, never a worktree's path", async () => {
  const storage = new MemoryStorage();
  const log = async (body: unknown, nowMs = 0) => (await handle(storage, body, nowMs)).log;
  assert.deepEqual(await log({ op: "take", claims: [{ name: "start-TW-1", ttlMs: null }], token: RUN_A }), { op: "take", claims: 1, taken: true });
  assert.deepEqual(await log({ op: "take", claims: [{ name: "start-TW-1", ttlMs: null }], token: RUN_B }), { op: "take", claims: 1, taken: false });
  assert.deepEqual(await log({ op: "record", sessionId: "s-1", change: { read: { pr: 7, at: at(1) } } }), { op: "record", sessionId: "s-1", change: "read" });
  assert.deepEqual(await log({ op: "read", sessions: ["s-1", "s-2"] }), { op: "read", sessions: 2, found: 1 });
  const worker = { issue: "TW-1", worktreeId: "r1::C:/work trees/tw-1", sessionId: "s-1", at: at(2), launch: { harness: "claude", model: "claude-opus-5-5", effort: "medium" }, rungFrom: "claim" };
  const started = await log({ op: "startWorker", worker });
  assert.deepEqual(started, { op: "startWorker", issue: "TW-1", sessionId: "s-1" });
  const checkout = { repoId: "r1", worktreeId: "r1::C:/work trees/conversations", at: at(2) };
  const set = await log({ op: "setCheckout", checkout });
  assert.deepEqual(set, { op: "setCheckout", repoId: "r1" });
  assert.deepEqual(await log({ op: "checkouts" }), { op: "checkouts", checkouts: 1 });
  const comment = { issueId: "i-1", commentId: "c0c0c0c0-0000-4000-8000-000000000001", at: at(2) };
  assert.deepEqual(await log({ op: "contextComment", issueId: "i-1" }), { op: "contextComment", issueId: "i-1", found: false });
  assert.deepEqual(await log({ op: "setContextComment", comment }), { op: "setContextComment", issueId: "i-1", commentId: comment.commentId });
  assert.deepEqual(await log({ op: "contextComment", issueId: "i-1" }), { op: "contextComment", issueId: "i-1", found: true });
  assert.deepEqual(await log({ op: "triaging", session: "s-1", identifier: "TW-1" }), { op: "triaging", identifier: "TW-1", holds: false });
  assert.deepEqual(await log({ op: "workers" }), { op: "workers", workers: 1 });
  assert.deepEqual(await log({ op: "forgetWorker", worktreeId: worker.worktreeId }), { op: "forgetWorker", forgotten: true });
  assert.deepEqual(await log({ op: "nope" }), { reason: "unknown op" });
  for (const line of [started, set, await log({ op: "forgetWorker", worktreeId: worker.worktreeId })]) assert.ok(!JSON.stringify(line).includes("work trees"));
});

test("a session event stores the app user ID and its issue, refreshed daily and pruned after 90 days", async () => {
  const storage = new MemoryStorage();
  assert.deepEqual(await tolkstersIssues(storage, ["issue-1"], 0), { appUserId: null, withSessions: [] });
  await recordSessionEvent(storage, "app-1", "issue-1", 1000);
  await recordSessionEvent(storage, "app-1", null, 1000);
  assert.deepEqual(await tolkstersIssues(storage, ["issue-1", "issue-2", "not an id!"], 1000), { appUserId: "app-1", withSessions: ["issue-1"] });
  // A later event within the day writes nothing; one after it refreshes the time.
  await recordSessionEvent(storage, "app-1", "issue-1", 1000 + SESSION_ISSUE_REFRESH_MS - 1);
  assert.equal(storage.data.get("session-issue:issue-1"), 1000);
  await recordSessionEvent(storage, "app-2", "issue-1", 1000 + SESSION_ISSUE_REFRESH_MS);
  const seen = 1000 + SESSION_ISSUE_REFRESH_MS;
  assert.deepEqual(await tolkstersIssues(storage, ["issue-1"], seen + SESSION_ISSUE_KEEP_MS), { appUserId: "app-2", withSessions: ["issue-1"] });
  assert.deepEqual(await tolkstersIssues(storage, ["issue-1"], seen + SESSION_ISSUE_KEEP_MS + 1), { appUserId: "app-2", withSessions: [] });
  await ok(storage, { op: "take", claims: [{ name: "request-x", ttlMs: null }], token: RUN_A }, seen + SESSION_ISSUE_KEEP_MS + 1);
  assert.equal(storage.data.has("session-issue:issue-1"), false);
  // Only IDs are taken.
  await assert.rejects(recordSessionEvent(storage, "not an id!", null, 0));
  await assert.rejects(recordSessionEvent(storage, "app-2", "not an id!", 0));
});

test("an owner's delegation and un-delegation on the Issue webhook count the moment an app user ID is stored", async () => {
  const storage = new MemoryStorage();
  const OWNER = "owner-1";
  const DELIVERY = "0f6a1c2e-1111-4222-8333-444455556666";
  const payload = (data: object, changed: object) => ({ action: "update", actor: { id: OWNER }, data: { id: "0a1b2c3d-0000-4000-8000-000000000001", updatedAt: at(1), state: { type: "started" }, ...data }, updatedFrom: changed }) as never;
  const delegation = payload({ delegateId: "app-1" }, { delegateId: null });
  const taken = payload({ delegateId: null }, { delegateId: "app-1" });
  // Before any session, no app user ID is known and no delegate change is told apart.
  const before = await tolkstersIssues(storage, [], 0);
  assert.deepEqual([fromIssue(delegation, DELIVERY, OWNER, before.appUserId), fromIssue(taken, DELIVERY, OWNER, before.appUserId)], [[], []]);
  await recordSessionEvent(storage, "app-1", null, 0);
  const { appUserId } = await tolkstersIssues(storage, [], 0);
  assert.deepEqual(fromIssue(delegation, DELIVERY, OWNER, appUserId).map((e) => e.kind === "delegateChanged" && [e.delegated, e.by]), [[true, "owner"]]);
  assert.deepEqual(fromIssue(taken, DELIVERY, OWNER, appUserId).map((e) => e.kind === "delegateChanged" && [e.delegated, e.by]), [[false, null]]);
});

test("a delegation Tolkster made is kept per sub-issue, replaced by a later one, and pruned after 90 days", async () => {
  const storage = new MemoryStorage();
  const made = { issueId: "issue-2", parentIssueId: "issue-1", sessionId: "s-1", at: at(1) };
  assert.deepEqual(await ok(storage, { op: "delegate", delegation: made }), {});
  assert.deepEqual(await ok(storage, { op: "delegations", issues: ["issue-2", "issue-3"] }), { delegations: { "issue-2": made, "issue-3": null } });
  const again = { ...made, sessionId: "s-9", at: at(5) };
  await ok(storage, { op: "delegate", delegation: again }, 1000);
  assert.deepEqual(await delegated(storage, "issue-2", 2000), again);
  // Past DELEGATION_KEEP_MS it counts for nothing, and the next prune removes it.
  assert.equal(await delegated(storage, "issue-2", 1000 + DELEGATION_KEEP_MS + 1), null);
  await ok(storage, { op: "take", claims: [{ name: "request-x", ttlMs: null }], token: RUN_A }, 1000 + DELEGATION_KEEP_MS + 1);
  assert.equal(storage.data.has("delegated:issue-2"), false);
  // Only IDs and a time are taken.
  for (const bad of [{ ...made, issueId: "TW 2" }, { ...made, at: "yesterday" }, { ...made, sessionId: undefined }]) {
    assert.equal((await handle(storage, { op: "delegate", delegation: bad }, 0)).status, 400);
  }
});

test("a reset keeps only the machines' counters, the context comments' and the delegations' records, and runs again after a failure", async () => {
  const storage = new MemoryStorage();
  const issue = "0b1c2d3e-4f50-4a61-8b72-9c8d7e6f5a4b";
  const request = (kind: string, state: string) => ({ facts: { request: { kind, at: "t" } }, machine: { state, version: 1, deadlines: [], stop: null } });
  const sessions = { "s-q": request("delegation", "queued"), "s-c": request("prompt", "claimed"), "s-d": request("start", "done") };
  const store = { ...newStore(issue), sessions, rev: 9, seq: 4, commandSeq: 7, facts: { ...newStore(issue).facts, notices: ["busy:x"] } };
  await storage.put({
    "storage-version": STORAGE_VERSION - 1,
    [`machine:issue:${issue}`]: store,
    [TRIAGE_KEY]: { ...newTriage(), rev: 3, commandSeq: 5, asked: [issue] },
    "claim:verdict-TW-1": { takenAt: 0, token: RUN_A, ttlMs: null },
    "worker:r1::D:/wt/tw-1": { issue: "TW-1" },
    // A context comment its current shape can't hold goes.
    "context-comment:i-9": { issueId: "i-9", at: "yesterday" },
    // A delegation is kept in its current shape, so its sub-issue's session is still trusted.
    "delegated:i-2": { ...contract.delegation, storedMs: 5, note: "text" },
    "delegated:i-3": { ...contract.delegation, issueId: "i-3" },
  });
  // A kept context comment is rebuilt in its current shape, without what it doesn't hold.
  await storage.put(`context-comment:${contract.contextComment.issueId}`, { ...contract.contextComment, note: "text" });
  const keep = (key: string, value: unknown) => machinesKept(key, value) ?? keptAtReset(key, value);
  const after = new Map<string, unknown>([
    [`machine:issue:${issue}`, { ...newStore(issue), rev: 9, seq: 4, commandSeq: 7 }],
    [TRIAGE_KEY, { ...newTriage(), rev: 3, commandSeq: 5 }],
    [`context-comment:${contract.contextComment.issueId}`, contract.contextComment],
    ["delegated:i-2", { ...contract.delegation, storedMs: 5 }],
  ]);
  // One that fails before it records the version is run again, and keeps the same.
  // What it loses is logged before anything changes, so the attempt that fails has logged it.
  const lost: unknown[] = [];
  const lose = (key: string, value: unknown) => void lost.push(...lostAtReset(key, value));
  await assert.rejects(resetOnce(storage, keep, async () => {
    throw new Error("alarm");
  }, lose));
  assert.deepEqual(new Map(storage.data), after);
  let alarms = 0;
  assert.equal(await resetOnce(storage, keep, async () => void alarms++, lose), true);
  assert.deepEqual(lost, [{ issue, session: "s-q", kind: "delegation", state: "queued" }, { issue, session: "s-c", kind: "prompt", state: "claimed" }]);
  assert.deepEqual(new Map(storage.data), new Map([...after, ["storage-version", STORAGE_VERSION]]));
  assert.equal(await resetOnce(storage, keep, async () => void alarms++), false);
  assert.equal(alarms, 1);
  // A counter it can't read fails the reset, never starts over.
  storage.data.set("storage-version", STORAGE_VERSION - 1);
  storage.data.set(`machine:issue:${issue}`, { ...store, commandSeq: undefined });
  await assert.rejects(resetOnce(storage, keep, async () => {}));
});
