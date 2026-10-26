import assert from "node:assert/strict";
import { test } from "node:test";
import { parseEvent } from "../src/worker/machine/validate.ts";
import { newSession, newStore, newWork } from "../src/worker/machine/ctx.ts";
import type { Command, Store } from "../src/worker/machine/types.ts";
import { parseReport, type Report } from "../src/worker/machine.ts";
import { fromReport, stateType, toEvent, toWire } from "../src/worker/route.ts";
import type { Routed } from "../src/worker/intake.ts";
import contract from "./contract.json" with { type: "json" };

const ISSUE = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const S = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const B = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
const DELIVERY = "11111111-2222-4333-8444-555555555555";
const AT = "2026-10-01T00:00:00.000Z";
const SHA = "0123456789abcdef0123456789abcdef01234567";
const BEFORE = "89abcdef0123456789abcdef0123456789abcdef";
const WT = "repo-1::D:/worktrees/tw-1";

test("Linear's state types are the machine's three", () => {
  assert.deepEqual(["triage", "backlog", "unstarted", "started", "completed", "canceled", "duplicate"].map(stateType), ["open", "open", "open", "open", "completed", "cancelled", "cancelled"]);
});

test("each intake event becomes one the machine takes", () => {
  const session = { deliveryId: DELIVERY, issueId: ISSUE, sessionId: S, at: AT, commentId: null };
  const events: Routed[] = [
    { ...session, kind: "sessionStarted", trust: "owner", sessionKind: "mention", commentId: B },
    { ...session, kind: "sessionStarted", trust: "reply", sessionKind: "mention" },
    { ...session, kind: "sessionStarted", trust: "chain", sessionKind: "delegation" },
    { ...session, kind: "ownerPrompted", trust: "owner", sessionKind: "delegation" },
    { ...session, kind: "ownerStopped", trust: "chain", sessionKind: "delegation" },
    { deliveryId: DELIVERY, issueId: ISSUE, at: AT, issue: { state: "canceled", priority: 2, parentId: B }, kind: "delegateChanged", delegated: true, by: "owner" },
    { deliveryId: DELIVERY, issueId: ISSUE, at: AT, issue: { state: "canceled", priority: 2, parentId: B }, kind: "delegateChanged", delegated: false, by: null },
    { deliveryId: DELIVERY, issueId: ISSUE, at: AT, issue: { state: "canceled", priority: 2, parentId: null }, kind: "issueStateChanged", from: null },
    { deliveryId: DELIVERY, issueId: ISSUE, kind: "prOpened", repo: "o/r", number: 7, branch: "b", head: SHA, at: AT },
    { deliveryId: DELIVERY, issueId: ISSUE, kind: "prPushed", repo: "o/r", number: 7, branch: "b", head: SHA, before: BEFORE, at: AT },
    { deliveryId: DELIVERY, issueId: ISSUE, kind: "prMerged", repo: "o/r", number: 7, branch: "b", head: SHA, at: AT },
    { deliveryId: DELIVERY, issueId: ISSUE, kind: "prReviewed", repo: "o/r", number: 7, branch: "b", head: SHA, sha: SHA, reviewerId: 9, copilot: true, at: AT },
    { deliveryId: DELIVERY, issueId: ISSUE, kind: "checksCompleted", repo: "o/r", branches: null, sha: SHA, source: "status:00000000", result: "couldntRun", at: AT },
    { deliveryId: DELIVERY, issueId: ISSUE, kind: "ownerCommented", commentId: S, at: AT },
  ];
  const made = events.map((e) => parseEvent(toEvent(e)));
  assert.deepEqual(made.map((e) => e.kind), events.map((e) => e.kind));
  for (const e of made) assert.equal(e.issue, ISSUE);
  const started = made.slice(0, 3) as { sessionKind: string; byOwner: boolean }[];
  assert.deepEqual(started.map((e) => [e.sessionKind, e.byOwner]), [["mention", true], ["reply", true], ["chain", false]]);
  // A mention's names the comment it began on; one without a comment names none.
  assert.deepEqual(made.slice(0, 2).map((e) => (e as { comment?: string }).comment), [B, undefined]);
  assert.equal((made[4] as { byOwner: boolean }).byOwner, true);
  assert.deepEqual(made.slice(5, 7).map((e) => (e as { by: unknown }).by), ["owner", null]);
  assert.deepEqual(made[7], { id: DELIVERY, issue: ISSUE, kind: "issueStateChanged", state: "cancelled", triage: false, parent: null, at: AT });
  // Triage is open, and says so.
  const triaged = toEvent({ deliveryId: DELIVERY, issueId: ISSUE, at: AT, issue: { state: "triage", priority: 2, parentId: B }, kind: "issueStateChanged", from: null });
  assert.deepEqual(triaged, { id: DELIVERY, issue: ISSUE, kind: "issueStateChanged", state: "open", triage: true, parent: B, at: AT });
  // A push carries the head it came from, which orders pushes to one PR.
  assert.equal((made[9] as { before: string }).before, BEFORE);
  // Every review is a reviewer's until `evaluatePr` reads the repo's own.
  assert.equal((made[11] as { reviewer: boolean }).reviewer, true);
});

function command(kind: Command["kind"], params: Record<string, unknown> = {}, machine = "work"): Command {
  return { id: `${ISSUE}.1`, issue: ISSUE, kind, machine, version: 4, params, class1: kind === "stopWorker", firstSentMs: 1, sentMs: 1, sends: 1, superseded: false };
}

function held(): Store {
  const s = newStore(ISSUE);
  s.facts.worktree = { id: WT, branch: "b", version: 1 };
  s.facts.pr = { number: 7, head: SHA, repo: "o/r", status: "open", at: AT, receivedMs: 1 };
  s.facts.blockers = { sentMs: 1, ids: [B], states: { [B]: { done: false, at: AT } }, more: false };
  s.facts.chained = { run: "r-1", issues: [B], asked: [B], answered: [B], haltedAt: null };
  s.facts.rounds.total = 3;
  s.work = { ...newWork("running", 4), requestedBy: { session: S, at: AT } };
  return s;
}

// A turn, a session's or a note's thread's, reads after the newest request a reader was shown,
// not after its last answer: one written while a turn ran, before it answered, is the next turn's.
test("a turn reads after the newest request a reader was shown", () => {
  const s = held();
  s.sessions[S] = newSession();
  s.sessions[S].facts.answeredThrough = "2026-09-26T01:05:00.000Z";
  s.sessions[S].facts.turnReadThrough = AT;
  const wire = (c: Command) => toWire(c, s, "TW-1").args;
  assert.deepEqual(wire(command("startTurn", { request: AT, thread: true }, S)), { sessionId: S, thread: true, request: AT, after: AT });
  assert.deepEqual(wire(command("startTurn", { request: AT }, S)), { sessionId: S, thread: false, request: AT, after: AT });
  // A worker that answered a session's later requests has shown them: a session's turn reads
  // after them, a thread's never.
  s.sessions[S].facts.readThrough = "2026-10-01T00:30:00.000Z";
  assert.equal(wire(command("startTurn", { request: AT }, S)).after, "2026-10-01T00:30:00.000Z");
  assert.equal(wire(command("startTurn", { request: AT, thread: true }, S)).after, AT);
  assert.deepEqual(wire(command("stopTurn", { thread: true }, S)), { sessionId: S, thread: true });
});

// A queued request's commands say what it waits for, so `listen`'s log says why the issue
// waits: its blockers' IDs for a wait on them, and nothing once it isn't queued.
test("a queued request's commands carry what it waits for", () => {
  const s = held();
  s.sessions[S] = newSession();
  const sess = s.sessions[S];
  sess.facts.request = { kind: "delegation", at: AT };
  const wire = () => toWire(command("readIssue", {}, S), s, "TW-1").args;
  assert.deepEqual(wire(), {});
  sess.machine = { state: "queued", version: 1, deadlines: [], stop: null };
  assert.deepEqual(wire(), {});
  sess.facts.waiting = "blockers";
  assert.deepEqual(wire(), { queued: { waiting: "blockers", at: AT, blockers: [B] } });
  sess.facts.waiting = "slot";
  assert.deepEqual(wire(), { queued: { waiting: "slot", at: AT } });
  sess.machine.state = "claimed";
  assert.deepEqual(wire(), {});
  // The shape the CLI reads (cli/src/machine.rs, `queued`), pinned by the contract's c-13.
  const pinned = contract.commands.find((c) => c.id === "c-13")!.args.queued!;
  sess.machine.state = "queued";
  sess.facts.waiting = "blockers";
  sess.facts.request = { kind: "delegation", at: pinned.at };
  s.facts.blockers = { sentMs: 1, ids: pinned.blockers!, states: Object.fromEntries(pinned.blockers!.map((id) => [id, { done: false, at: AT }])), more: false };
  assert.deepEqual(wire().queued, pinned);
});

// A cancelled reap says when the store holds no PR, which it may have missed; one it holds,
// closed, is no guard.
test("a cancelled reap names a store with no PR", () => {
  const s = held();
  const rule = () => toWire(command("reapWorktree", { rule: "cancelled" }), s, "TW-1").args.rule;
  s.facts.pr = { ...s.facts.pr!, status: "closed" };
  assert.deepEqual(rule(), { kind: "cancelled" });
  s.facts.pr = null;
  assert.deepEqual(rule(), { kind: "cancelled", noPr: true });
});

test("a round's number counts the rounds committed and each queued up to it, not one dropped before it", () => {
  const s = held();
  const first = { ...command("startRound", { pr: 7, proposal: { head: SHA } }), id: `${ISSUE}.1` };
  const second = { ...command("startRound", { pr: 7, proposal: { head: SHA, lastFailed: SHA } }), id: `${ISSUE}.2` };
  s.queue = [first, second];
  assert.equal(toWire(first, s, "TW-1").args.round, 4);
  assert.equal(toWire(second, s, "TW-1").args.round, 5);
  s.queue = [second];
  assert.equal(toWire(second, s, "TW-1").args.round, 4);
  // Sent as copies of the store's command and from a store loaded again, as the relay does.
  s.queue = [first, second];
  assert.equal(toWire({ ...second }, structuredClone(s), "TW-1").args.round, 5);
  // A rebase-only round queued ahead is counted by no one.
  const rebase = { ...command("startRound", { pr: 7, proposal: { head: SHA, lastBase: SHA } }), id: `${ISSUE}.3` };
  s.queue = [rebase, second];
  assert.equal(toWire(second, s, "TW-1").args.round, 4);
});

test("an update carries the head it was decided on, never a later one", () => {
  const s = held();
  const update = command("updateBranch", { head: SHA, base: SHA });
  s.facts.pr = { ...s.facts.pr!, head: "f".repeat(40) };
  assert.deepEqual(toWire(update, s, "TW-1").args, { repo: "o/r", number: 7, head: SHA });
  assert.deepEqual(toWire(update, newStore(ISSUE), "TW-1").args, {});
});

test("a command goes on the wire with its args read from its store as it is now", () => {
  const s = held();
  const wire = (c: Command) => toWire(c, s, "TW-1");
  assert.deepEqual(wire(command("startWorker", { session: S, orchestrate: true, worktree: null })), { type: "command", id: `${ISSUE}.1`, kind: "startWorker", issueId: ISSUE, identifier: "TW-1", version: 4, urgent: false, args: { sessionId: S, orchestrate: true } });
  assert.deepEqual(wire(command("startWorker", { session: S, orchestrate: false, worktree: "repo::C:/w/tw-1" })).args, { sessionId: S, orchestrate: false, worktreeId: "repo::C:/w/tw-1" });
  assert.deepEqual(wire(command("stopWorker")).args, { worktreeId: WT, sessionId: S });
  assert.equal(wire(command("stopWorker")).urgent, true);
  assert.deepEqual(wire(command("deliverMessage", { session: S })).args, { worktreeId: WT, sessionId: S });
  assert.deepEqual(wire(command("deliverMessage", { comment: true, through: AT })).args, { worktreeId: WT, sessionId: S, comment: true });
  assert.deepEqual(wire(command("evaluatePr")).args, { repo: "o/r", number: 7 });
  assert.deepEqual(wire(command("startTurn", { request: AT, thread: true }, S)).args, { sessionId: S, thread: true, request: AT, after: null });
  assert.deepEqual(wire(command("startRound", { pr: 7 })).args, { repo: "o/r", number: 7, head: SHA, round: 3, sessionId: S, rebaseOnly: false });
  assert.equal(wire(command("startRound", { pr: 7, proposal: { head: SHA, lastBase: SHA } })).args.rebaseOnly, true);
  assert.equal(wire(command("startRound", { pr: 7, proposal: { head: SHA, lastBase: SHA, lastFailed: SHA } })).args.rebaseOnly, false);
  assert.deepEqual(wire(command("observe")).args, { worktreeIds: [WT], sessionId: null, thread: false });
  assert.deepEqual(wire(command("postNotice", { notice: "blocked", session: S, key: "k" }, S)).args, { sessionId: S, thread: false, notice: "blocked", blockers: [B], pr: 7 });
  assert.deepEqual(wire(command("postNotice", { notice: "roundsCap", session: S, key: "k" })).args, { sessionId: S, thread: false, notice: "roundsCap", pr: 7, rounds: 3 });
  assert.deepEqual(wire(command("takeBackChain", { run: "r-1" })).args, { children: [B] });
  // A PR's command with no PR held goes with nothing, for `listen` to end it failed.
  assert.deepEqual(toWire(command("evaluatePr"), newStore(ISSUE), "TW-1").args, {});
});

test("a report becomes its event on the issue it names, or none", async () => {
  const lookups = { sessionIssue: async (id: string) => (id === S ? ISSUE : null), worktreeIssue: async (id: string) => (id === WT ? ISSUE : null), connection: 1700 };
  const report = (r: object) => parseReport(r) as Report;
  const event = async (r: object) => {
    const made = await fromReport(report(r), lookups);
    return made === null ? null : parseEvent(made);
  };
  assert.deepEqual(await event({ report: "commandResult", id: `${ISSUE}.3`, issueId: ISSUE, version: 2, result: "ok", detail: { reaped: true } }), { id: `result:${ISSUE}.3`, issue: ISSUE, kind: "commandResult", command: `${ISSUE}.3`, result: "ok", data: { reaped: true } });
  assert.deepEqual(await event({ report: "agentIdle", worktreeId: WT, sequence: 4 }), { id: "1700:4", issue: ISSUE, kind: "agentIdle" });
  assert.equal(await event({ report: "agentIdle", worktreeId: "repo-1::D:/other", sequence: 4 }), null);
  // An agent's own hook's report is named by its kind and time, not by a connection.
  assert.deepEqual(await event({ report: "agentWaiting", worktreeId: WT, at: AT, elapsedMs: 0 }), { id: `hook:agentWaiting:${AT}`, issue: ISSUE, kind: "agentWaiting" });
  assert.equal(await event({ report: "agentExited", worktreeId: "repo-1::D:/other", at: AT, elapsedMs: 0 }), null);
  assert.deepEqual(await event({ report: "turnEnded", sessionId: S, sequence: 5 }), { id: "1700:5", issue: ISSUE, kind: "turnEnded", session: S });
  assert.deepEqual(await event({ report: "turnIdle", sessionId: S, terminal: "term_1", at: AT }), { id: `idle:${S}:${AT}`, issue: ISSUE, kind: "turnIdle", session: S, terminal: "term_1", at: AT });
  assert.deepEqual(await event({ report: "workerRead", sessionId: S, activityId: "a-1", at: AT, reader: "worker" }), { id: `${S}:a-1`, issue: ISSUE, kind: "workerRead", session: S, reader: "worker", through: AT });
  assert.deepEqual(await event({ report: "workerAnswered", sessionId: S, activityId: "a-2", at: AT, reader: "turn" }), { id: "a-2", issue: ISSUE, kind: "workerAnswered", session: S, reader: "turn", at: AT });
  assert.equal(await event({ report: "workerRead", sessionId: "s-unknown", activityId: "a-1", at: AT, reader: "worker" }), null);
  assert.deepEqual(await event({ report: "chainRecorded", issueId: ISSUE, sessionId: S, run: "r-1", children: [B] }), { id: `${S}:r-1`, issue: ISSUE, kind: "chainRecorded", role: "ask", session: S, run: "r-1", children: [B] });
  // A landing names its own issue: one its session isn't on goes nowhere.
  const head = "2878d3d000000000000000000000000000000000";
  assert.deepEqual(await event({ report: "landed", issueId: ISSUE, sessionId: S, head }), { id: `${S}:${head}`, issue: ISSUE, kind: "landed", session: S, head });
  assert.equal(await event({ report: "landed", issueId: B, sessionId: S, head }), null);
  assert.equal(await fromReport(report({ report: "config", cap: 4, slots: {}, repos: {}, machines: {} }), lookups), null);
  // A report without who read is refused at the wire.
  assert.equal(parseReport({ report: "workerRead", sessionId: S, activityId: "a-1", at: AT }), null);
});
