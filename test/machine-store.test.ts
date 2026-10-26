import assert from "node:assert/strict";
import { test } from "node:test";
import { ALONE, coordinate, newCoordinator } from "../src/worker/machine/coordinator.ts";
import { expire, nextDue, receive } from "../src/worker/machine/step.ts";
import { acknowledgedAway, chained, Conflict, connectAsked, connected, contextAsked, deliver, holds, inFlight, loadCoordinator, loadStore, machineTimer, rebuildAsked, reconcile, repoPushed, fleetReported, settle, staged, terminalClosed, TURN_TERMINAL, waitsCleared } from "../src/worker/machine/store.ts";
import { AGENT_STATES, ATTRIBUTION_WAIT_MS, COMMANDS, COORDINATOR_EVENTS, NOTICES, REQUEST_STATES, WORK_STATES, type Command, type Event, type Store } from "../src/worker/machine/types.ts";
import { TRIAGE_STATES } from "../src/worker/machine/triage.ts";
import { parseEvent } from "../src/worker/machine/validate.ts";
import { Invalid } from "../src/worker/check.ts";
import type { Storage } from "../src/worker/storage.ts";
import contract from "./contract.json" with { type: "json" };
import { apply, B, B2, ev, given, HOUR, ISSUE, MIN, NOW, read, readIssue, result, S, S2, SHA, T0, T1, T2 } from "./machine-fixture.ts";
import { chainSessions } from "../src/worker/rebuild.ts";
import { MemoryStorage } from "./storage.ts";

const ISSUE2 = "12345678-1234-4234-8234-123456789abc";
const SESSION2 = "87654321-4321-4321-8321-cba987654321";

test("the machines' names are the contract's", () => {
  const m = contract.machine;
  assert.deepEqual([...REQUEST_STATES], m.requestStates);
  assert.deepEqual([...WORK_STATES], m.workStates);
  assert.deepEqual([...TRIAGE_STATES], m.triageStates);
  assert.deepEqual([...COMMANDS, "postAway"], m.commands);
  assert.deepEqual([...COORDINATOR_EVENTS], m.coordinator);
  assert.deepEqual([...NOTICES], m.notices);
  assert.deepEqual(AGENT_STATES, ["launching", "running", "idle", "held"]);
});

// ---- The store ----

function sink() {
  const sent: Command[] = [];
  return { sent, sender: (c: Command[]) => sent.push(...c) };
}

const start = (issue: string, session: string, at = T0): Event => ({ kind: "sessionStarted", id: `start-${session}`, issue, session, sessionKind: "delegation", byOwner: true, at });
const readOk = (issue: string, session: string, id: string, over: object = {}): Event => ({ kind: "commandResult", id, issue, command: id, result: "ok", data: read({ sessions: [{ id: session, kind: "delegation", createdAt: T0 }], ...over }) as never });

test("a delegation reads its issue, takes a slot and starts the worker, through the coordinator in the same step", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await deliver(storage, start(ISSUE, S), NOW, sender);
  assert.deepEqual(sent.map((c) => c.kind), ["readIssue"]);
  await deliver(storage, readOk(ISSUE, S, sent[0].id), NOW, sender);
  assert.deepEqual(sent.map((c) => c.kind), ["readIssue", "startWorker"]);
  const s = await loadStore(storage, ISSUE);
  assert.equal(s.sessions[S].machine?.state, "claimed");
  assert.equal(s.work?.state, "launching");
  assert.deepEqual(s.outbox, []);
  // Here, and while its worker has no worktree, movable by the repo it was placed for.
  assert.deepEqual((await loadCoordinator(storage)).grants[ISSUE], { session: null, sinceMs: NOW, repo: "repo" });
});

// The read comes back with no history entry for the delegation yet: who delegated is
// `unknown` at the same clock as the Issue webhook's `delegateChanged`. Either order ends in
// the same claim, and nothing says the issue isn't delegated.
const delegatedAt = ev("delegateChanged", { by: "owner", at: T1 });
const unattributed = { delegate: "unknown", updatedAt: T1 };

test("a delegation's session, then a read that can't say who delegated, then its webhook: claimed", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await deliver(storage, start(ISSUE, S), NOW, sender);
  await deliver(storage, readOk(ISSUE, S, sent[0].id, unattributed), NOW, sender);
  assert.deepEqual(sent.map((c) => c.kind), ["readIssue"]);
  assert.equal((await loadStore(storage, ISSUE)).sessions[S].machine?.state, "requested");
  await deliver(storage, delegatedAt, NOW + 1000, sender);
  assert.deepEqual((await loadStore(storage, ISSUE)).facts.delegate, { value: "owner", at: T1 });
  assert.deepEqual(sent.map((c) => c.kind), ["readIssue", "readIssue"]);
  // Linear's history still lags: the read keeps who the webhook said.
  await deliver(storage, readOk(ISSUE, S, sent[1].id, unattributed), NOW + 2000, sender);
  assert.deepEqual(sent.map((c) => c.kind), ["readIssue", "readIssue", "startWorker"]);
  assert.equal((await loadStore(storage, ISSUE)).sessions[S].machine?.state, "claimed");
});

test("a delegation's webhook, then its session, then a read that can't say who delegated: claimed", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await deliver(storage, delegatedAt, NOW, sender);
  await deliver(storage, start(ISSUE, S), NOW, sender);
  await deliver(storage, readOk(ISSUE, S, sent[0].id, unattributed), NOW, sender);
  assert.deepEqual(sent.map((c) => c.kind), ["readIssue", "startWorker"]);
  const s = await loadStore(storage, ISSUE);
  assert.equal(s.sessions[S].machine?.state, "claimed");
  assert.deepEqual(s.facts.delegate, { value: "owner", at: T1 });
});

test("a request whose delegation no read or event attributes is read again, then refused as not delegated", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await deliver(storage, start(ISSUE, S), NOW, sender);
  await deliver(storage, readOk(ISSUE, S, sent[0].id, unattributed), NOW, sender);
  let now = NOW;
  for (let i = 0; i < 10 && (await loadStore(storage, ISSUE)).sessions[S].machine?.state === "requested"; i++) {
    now += 30 * 1000;
    await reconcile(storage, now, sender, true);
    const asked = sent.at(-1)!;
    if (asked.kind === "readIssue") await deliver(storage, readOk(ISSUE, S, asked.id, unattributed), now, sender);
  }
  assert.ok(now - NOW >= ATTRIBUTION_WAIT_MS);
  assert.ok(sent.filter((c) => c.kind === "readIssue").length >= 4, "read again while it waited");
  assert.equal((await loadStore(storage, ISSUE)).sessions[S].machine?.state, "done");
  assert.deepEqual(sent.filter((c) => c.kind !== "readIssue").map((c) => `${c.kind}:${c.params.notice}`), ["postNotice:notDelegated"]);
});

test("an intake event is staged as writes for the intake's own put, and settled after it lands", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  const step = await staged(storage, start(ISSUE, S), NOW);
  // Nothing is written or sent until the caller's put lands.
  assert.equal(await storage.get(`machine:issue:${ISSUE}`), undefined);
  assert.equal(sent.length, 0);
  assert.deepEqual(Object.keys(step.writes).sort(), ["machine:due", `machine:issue:${ISSUE}`]);
  await storage.put(step.writes);
  await settle(storage, step.issue, step.send, NOW, sender);
  assert.deepEqual(sent.map((c) => c.kind), ["readIssue"]);
  // The read in flight holds the issue, and is sent again to a new connection.
  assert.ok(await holds(storage, ISSUE));
  assert.ok(!(await holds(storage, ISSUE2)));
  assert.deepEqual((await inFlight(storage)).map((c) => c.id), [sent[0].id]);
  assert.throws(() => parseEvent({ kind: "basePushed", id: "x", issue: ISSUE, base: "a".repeat(40) }), Invalid);
});

test("the Worker's away acknowledgement marks its request, so the alarm says it no more", async () => {
  const storage = new MemoryStorage();
  const { sender } = sink();
  await deliver(storage, start(ISSUE, S), NOW, sender);
  await acknowledgedAway(storage, ISSUE, S, T1, NOW, sender);
  assert.deepEqual((await loadStore(storage, ISSUE)).facts.notices, []);
  await acknowledgedAway(storage, ISSUE, S, T0, NOW, sender);
  assert.deepEqual((await loadStore(storage, ISSUE)).facts.notices, [`away:${S}:${T0}`]);
});

test("a completion's context is the issue's own command: once per completion, holding no machine", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await contextAsked(storage, ISSUE, T1, NOW, sender);
  assert.deepEqual(sent.map((c) => [c.kind, c.machine, c.version, c.params]), [["startContext", "issue", 0, { at: T1 }]]);
  // A redelivery while it is queued queues nothing more.
  await contextAsked(storage, ISSUE, T1, NOW, sender);
  assert.equal((await loadStore(storage, ISSUE)).queue.length, 1);
  await deliver(storage, result(sent[0].id, "ok"), NOW, sender);
  const after = await loadStore(storage, ISSUE);
  assert.deepEqual([after.queue, after.work, after.sessions], [[], null, {}]);
  // Beside a machine, it waits its turn in the queue and moves none of its versions.
  sent.length = 0;
  await deliver(storage, start(ISSUE, S), NOW, sender);
  const version = (await loadStore(storage, ISSUE)).sessions[S].machine?.version;
  await contextAsked(storage, ISSUE, T2, NOW, sender);
  assert.deepEqual(sent.map((c) => c.kind), ["readIssue"]);
  assert.equal((await loadStore(storage, ISSUE)).sessions[S].machine?.version, version);
  await deliver(storage, readOk(ISSUE, S, sent[0].id), NOW, sender);
  assert.ok(sent.some((c) => c.kind === "startContext"));
});

test("a mention's session lands, then its away mark, before its comment's read comes back, as the Worker orders them", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await deliver(storage, start(ISSUE, S), NOW, sender);
  await deliver(storage, readOk(ISSUE, S, sent[0].id), NOW, sender);
  const worker = sent.find((c) => c.kind === "startWorker");
  assert.ok(worker);
  await deliver(storage, result(worker.id, "ok", { worktree: { id: "repo::C:/w/tw-1", branch: "tw-1" } }), NOW, sender);
  sent.length = 0;
  // The Worker starts the session's ingest first, then posts the away message and marks it
  // once the event is in; the Comment webhook's event and its read follow in their own time.
  const mention: Event = { kind: "sessionStarted", id: "start-mention", issue: ISSUE, session: S2, sessionKind: "mention", byOwner: true, comment: B, at: T1 };
  await deliver(storage, mention, NOW, sender);
  await acknowledgedAway(storage, ISSUE, S2, T1, NOW, sender);
  assert.ok((await loadStore(storage, ISSUE)).facts.notices.includes(`away:${S2}:${T1}`));
  await deliver(storage, { kind: "ownerCommented", id: "comment-1", issue: ISSUE, comment: B, at: T1 }, NOW, sender);
  // Events wait while an issue read is out, and a command waits for the one before it: the
  // session's request is told to the worker, then the comment's read goes out.
  await deliver(storage, readOk(ISSUE, S2, sent.find((c) => c.kind === "readIssue")!.id), NOW, sender);
  await deliver(storage, result(sent.find((c) => c.kind === "deliverMessage")!.id, "ok"), NOW, sender);
  const reading = sent.find((c) => c.kind === "readComment");
  assert.ok(reading);
  sent.length = 0;
  await deliver(storage, result(reading.id, "ok", { comment: { kind: "plain", newestOwnerAt: T1, state: "open" } }), NOW, sender);
  assert.ok(!sent.some((c) => c.kind === "deliverMessage"));
  assert.equal((await loadStore(storage, ISSUE)).facts.sessions[S2].comment, B);
});

test("a worker launched on its brief and never seen working is closed and stalls, never counted paused", async () => {
  const launch = async (worked: boolean) => {
    const storage = new MemoryStorage();
    const { sent, sender } = sink();
    await deliver(storage, start(ISSUE, S), NOW, sender);
    await deliver(storage, readOk(ISSUE, S, sent[0].id), NOW, sender);
    const startWorker = sent.find((c) => c.kind === "startWorker")!;
    // Its hook may report the first turn before the start's result lands.
    if (worked) await deliver(storage, { ...ev("agentWorking", {}), id: "working-1" }, NOW + 1000, sender);
    const data = { worktree: { id: "repo::C:/w/tw-1", branch: "tw-1" }, madeWorktree: true, launched: true };
    await deliver(storage, { kind: "commandResult", id: startWorker.id, issue: ISSUE, command: startWorker.id, result: "ok", data }, NOW + 2000, sender);
    return { storage, sent, sender };
  };
  const { storage, sent, sender } = await launch(false);
  assert.equal((await loadStore(storage, ISSUE)).work?.firstTurn, NOW);
  // Orca's listing calls the agent idle before it took its prompt: not a pause.
  await deliver(storage, { ...ev("agentIdle", {}), id: "idle-1" }, NOW + MIN, sender);
  assert.equal((await loadStore(storage, ISSUE)).work?.state, "running");
  await reconcile(storage, NOW + 6 * MIN, sender, true);
  const stalled = await loadStore(storage, ISSUE);
  assert.equal(stalled.work?.state, "exited");
  // The close goes first; the notice waits its turn in the queue.
  assert.deepEqual(stalled.queue.map((c) => [c.kind, c.params.notice]), [["closeAgent", undefined], ["postNotice", "stalled"]]);
  // Seen working, before or after the result: an idle agent then has paused.
  for (const worked of [true, false]) {
    const run = await launch(worked);
    if (!worked) await deliver(run.storage, { ...ev("agentWorking", {}), id: "working-2" }, NOW + MIN, run.sender);
    assert.equal((await loadStore(run.storage, ISSUE)).work?.firstTurn ?? null, null);
    await deliver(run.storage, { ...ev("agentIdle", {}), id: "idle-2" }, NOW + 2 * MIN, run.sender);
    assert.equal((await loadStore(run.storage, ISSUE)).work?.state, "idle");
  }
});

test("a reply to a worker whose first turn never began starts it again on its brief, never resumes it", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await deliver(storage, start(ISSUE, S), NOW, sender);
  await deliver(storage, readOk(ISSUE, S, sent[0].id), NOW, sender);
  const first = sent.find((c) => c.kind === "startWorker")!;
  const data = { worktree: { id: "repo::C:/w/tw-1", branch: "tw-1" }, madeWorktree: true, launched: true };
  await deliver(storage, result(first.id, "ok", data), NOW, sender);
  assert.equal((await loadStore(storage, ISSUE)).work?.neverBegan, true);
  await reconcile(storage, NOW + 6 * MIN, sender, true);
  for (const c of (await loadStore(storage, ISSUE)).queue) await deliver(storage, result(c.id, "ok"), NOW + 7 * MIN, sender);
  const stalled = await loadStore(storage, ISSUE);
  assert.deepEqual([stalled.work?.state, stalled.work?.neverBegan, stalled.facts.worktree?.id], ["exited", true, "repo::C:/w/tw-1"]);
  sent.length = 0;
  await deliver(storage, { kind: "ownerPrompted", id: "reply-1", issue: ISSUE, session: S, byOwner: true, at: T1 }, NOW + 8 * MIN, sender);
  const reading = sent.find((c) => c.kind === "readIssue");
  if (reading) await deliver(storage, readOk(ISSUE, S, reading.id), NOW + 8 * MIN, sender);
  assert.ok(!sent.some((c) => c.kind === "resumeWorker"));
  const again = sent.find((c) => c.kind === "startWorker");
  assert.ok(again);
  assert.deepEqual([again.params.session, again.params.worktree], [S, "repo::C:/w/tw-1"]);
  // Launched on the brief and still not seen working, it is watched as the first start was.
  await deliver(storage, result(again.id, "ok", { ...data, madeWorktree: false }), NOW + 9 * MIN, sender);
  const relaunched = await loadStore(storage, ISSUE);
  assert.deepEqual([relaunched.work?.state, relaunched.work?.neverBegan], ["running", true]);
  await deliver(storage, { ...ev("agentWorking", {}), id: "working-1" }, NOW + 10 * MIN, sender);
  assert.equal((await loadStore(storage, ISSUE)).work?.neverBegan, false);
});

test("a reply after a start that was lost names the worktree it finds, and relaunches there on the brief", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await deliver(storage, start(ISSUE, S), NOW, sender);
  await deliver(storage, readOk(ISSUE, S, sent[0].id), NOW, sender);
  await deliver(storage, result(sent.find((c) => c.kind === "startWorker")!.id, "lost"), NOW, sender);
  for (const c of (await loadStore(storage, ISSUE)).queue) await deliver(storage, result(c.id, "ok"), NOW + MIN, sender);
  const lost = await loadStore(storage, ISSUE);
  assert.deepEqual([lost.work?.state, lost.work?.neverBegan, lost.facts.worktree], ["exited", true, null]);
  sent.length = 0;
  await deliver(storage, { kind: "ownerPrompted", id: "reply-1", issue: ISSUE, session: S, byOwner: true, at: T1 }, NOW + 2 * MIN, sender);
  const reading = sent.find((c) => c.kind === "readIssue");
  if (reading) await deliver(storage, readOk(ISSUE, S, reading.id), NOW + 2 * MIN, sender);
  const unnamed = sent.find((c) => c.kind === "startWorker")!;
  assert.equal(unnamed.params.worktree, null);
  const found = { worktree: { id: "repo::C:/w/tw-1", branch: "tw-1" }, madeWorktree: false };
  await deliver(storage, result(unnamed.id, "ok", { ...found, launched: false }), NOW + 3 * MIN, sender);
  const named = sent.filter((c) => c.kind === "startWorker").at(-1)!;
  assert.deepEqual([named.id !== unnamed.id, named.params.worktree, named.params.session], [true, "repo::C:/w/tw-1", S]);
  await deliver(storage, result(named.id, "ok", { ...found, launched: true }), NOW + 4 * MIN, sender);
  const relaunched = await loadStore(storage, ISSUE);
  assert.deepEqual([relaunched.work?.state, relaunched.work?.neverBegan, relaunched.work?.firstTurn != null], ["running", true, true]);
});

test("two issues can't both take the last slot: the second waits, then re-checks before its claim", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await fleetReported(storage, { settings: { cap: 1, slots: {}, repos: {}, machines: {} } }, NOW, sender);
  await deliver(storage, start(ISSUE, S), NOW, sender);
  await deliver(storage, start(ISSUE2, SESSION2), NOW, sender);
  await deliver(storage, readOk(ISSUE, S, sent[0].id), NOW, sender);
  await deliver(storage, readOk(ISSUE2, SESSION2, sent[1].id), NOW, sender);
  assert.equal((await loadStore(storage, ISSUE)).work?.state, "launching");
  const waiting = await loadStore(storage, ISSUE2);
  assert.equal(waiting.sessions[SESSION2].machine?.state, "queued");
  assert.ok(waiting.queue.some((c) => c.kind === "postNotice" && c.params.notice === "busy"));
  // The first worker's start fails, giving its slot back: the second is granted it and
  // reads its issue again before claiming.
  const startWorker = sent.find((c) => c.kind === "startWorker" && c.id.startsWith(ISSUE))!;
  await deliver(storage, { kind: "commandResult", id: startWorker.id, issue: ISSUE, command: startWorker.id, result: "failed", data: { madeWorktree: false } }, NOW, sender);
  const busy = sent.find((c) => c.kind === "postNotice" && c.id.startsWith(ISSUE2))!;
  await deliver(storage, { kind: "commandResult", id: busy.id, issue: ISSUE2, command: busy.id, result: "ok" }, NOW, sender);
  const regranted = await loadStore(storage, ISSUE2);
  assert.equal(regranted.sessions[SESSION2].machine?.state, "requested");
  assert.notEqual(regranted.sessions[SESSION2].facts.grant, null);
  const reread = sent.filter((c) => c.kind === "readIssue" && c.id.startsWith(ISSUE2)).at(-1)!;
  await deliver(storage, readOk(ISSUE2, SESSION2, reread.id), NOW, sender);
  assert.equal((await loadStore(storage, ISSUE2)).work?.state, "launching");
});

test("a finished blocker reaches the issue it blocks through the index", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await deliver(storage, start(ISSUE, S), NOW, sender);
  await deliver(storage, readOk(ISSUE, S, sent[0].id, { blockers: [{ id: ISSUE2, done: false, updatedAt: T0 }] }), NOW, sender);
  assert.equal((await loadStore(storage, ISSUE)).sessions[S].machine?.state, "queued");
  await deliver(storage, { kind: "issueStateChanged", id: "done", issue: ISSUE2, state: "completed", parent: null, at: T1 }, NOW, sender);
  // It waits behind the blocked notice in flight, its fact taken at once.
  const waiting = await loadStore(storage, ISSUE);
  assert.deepEqual(waiting.waiting.map((w) => w.event.kind), ["blockersChanged"]);
  assert.equal(waiting.facts.blockers?.states[ISSUE2].done, true);
  const notice = sent.find((c) => c.kind === "postNotice")!;
  await deliver(storage, { kind: "commandResult", id: notice.id, issue: ISSUE, command: notice.id, result: "ok" }, NOW, sender);
  assert.equal((await loadStore(storage, ISSUE)).sessions[S].machine?.state, "requested");
});

test("after a reset, the owner's delegation asks for a reply, which queues its work on its blocker, which starts once the blocker finishes", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await rebuildAsked(storage, ISSUE, null, NOW, sender);
  assert.deepEqual(sent.map((c) => [c.kind, c.machine]), [["readIssue", "issue"]]);
  const blocked = { blockers: [{ id: ISSUE2, done: false, updatedAt: T0 }] };
  // The intake never mapped its session, so a worker begun on it couldn't report: nothing begins.
  await deliver(storage, readOk(ISSUE, S, sent[0].id, blocked), NOW, sender);
  assert.deepEqual(sent.map((c) => [c.kind, c.machine, c.params.notice ?? null, c.params.session ?? null]), [["readIssue", "issue", null, null], ["postNotice", "issue", "unheard", S]]);
  assert.equal((await loadStore(storage, ISSUE)).sessions[S]?.machine ?? null, null);
  await deliver(storage, { kind: "commandResult", id: sent[1].id, issue: ISSUE, command: sent[1].id, result: "ok" }, NOW, sender);
  // Asked again, by a rebuild that failed part way: said once.
  await rebuildAsked(storage, ISSUE, null, NOW, sender);
  await deliver(storage, readOk(ISSUE, S, sent.at(-1)!.id, blocked), NOW, sender);
  assert.equal(sent.filter((c) => c.kind === "postNotice").length, 1);
  // The owner's reply begins the request, which reads for its decision.
  await deliver(storage, { kind: "ownerPrompted", id: "reply", issue: ISSUE, session: S, byOwner: true, at: T1 }, NOW, sender);
  const decision = sent.at(-1)!;
  assert.deepEqual([decision.kind, decision.machine], ["readIssue", S]);
  await deliver(storage, readOk(ISSUE, S, decision.id, blocked), NOW, sender);
  assert.equal((await loadStore(storage, ISSUE)).sessions[S].machine?.state, "queued");
  const notice = sent.filter((c) => c.kind === "postNotice").at(-1)!;
  await deliver(storage, { kind: "commandResult", id: notice.id, issue: ISSUE, command: notice.id, result: "ok" }, NOW, sender);
  await deliver(storage, { kind: "issueStateChanged", id: "done", issue: ISSUE2, state: "completed", parent: null, at: T1 }, NOW, sender);
  const reread = sent.filter((c) => c.kind === "readIssue").at(-1)!;
  assert.equal(reread.machine, S);
  await deliver(storage, readOk(ISSUE, S, reread.id, { blockers: [{ id: ISSUE2, done: true, updatedAt: T1 }] }), NOW, sender);
  assert.equal((await loadStore(storage, ISSUE)).work?.state, "launching");
  assert.equal(sent.at(-1)?.kind, "startWorker");
});

test("at a connect, a delegated issue never commanded asks for a reply once", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await connectAsked(storage, ISSUE, NOW, sender);
  assert.deepEqual(sent.map((c) => [c.kind, c.machine]), [["readIssue", "issue"]]);
  await deliver(storage, readOk(ISSUE, S, sent[0].id), NOW, sender);
  assert.deepEqual(sent.map((c) => [c.kind, c.params.notice ?? null]), [["readIssue", null], ["postNotice", "unheard"]]);
  // A post that fails goes once more.
  await deliver(storage, { kind: "commandResult", id: sent[1].id, issue: ISSUE, command: sent[1].id, result: "failed" }, NOW, sender);
  assert.deepEqual([sent.at(-1)?.kind, sent.at(-1)?.params.again], ["postNotice", true]);
  // Asked again at the next connect, and an issue whose delegation landed: nothing more.
  const before = sent.length;
  await connectAsked(storage, ISSUE, NOW, sender);
  await deliver(storage, start(ISSUE2, SESSION2), NOW, sender);
  const started = sent.length;
  await connectAsked(storage, ISSUE2, NOW, sender);
  assert.equal(sent.length, started);
  assert.equal(sent.slice(before).filter((c) => c.issue === ISSUE).length, 0);
  // A session made seconds ago is asked too, and its start, landing late, still begins it.
  const fresh = new MemoryStorage();
  const late = sink();
  await connectAsked(fresh, ISSUE, NOW, late.sender);
  const young = { sessions: [{ id: S, kind: "delegation", createdAt: new Date(NOW - 10_000).toISOString() }] };
  await deliver(fresh, readOk(ISSUE, S, late.sent[0].id, young), NOW, late.sender);
  assert.equal(late.sent[1].params.notice, "unheard");
  await deliver(fresh, { kind: "commandResult", id: late.sent[1].id, issue: ISSUE, command: late.sent[1].id, result: "ok" }, NOW, late.sender);
  await deliver(fresh, start(ISSUE, S, new Date(NOW - 10_000).toISOString()), NOW, late.sender);
  assert.deepEqual([late.sent.at(-1)?.kind, late.sent.at(-1)?.machine], ["readIssue", S]);
});

test("a rebuild asks nothing of a store whose delegation someone else made", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await rebuildAsked(storage, ISSUE, null, NOW, sender);
  await deliver(storage, readOk(ISSUE, S, sent[0].id, { delegate: "other" }), NOW, sender);
  assert.equal(sent.length, 1);
  assert.equal((await loadStore(storage, ISSUE)).sessions[S]?.machine ?? null, null);
});

// A parent whose orchestrator chained ISSUE2, on session S, before the reset.
const orchestrated = { session: S, children: [ISSUE2] };
const parentRead = (id: string, done: boolean) => ({ kind: "commandResult", id, issue: ISSUE, command: id, result: "ok", data: read({ children: [{ id: ISSUE2, done, triage: false, updatedAt: T0 }] }) as never }) as Event;
// The chain's delegation, which only the rebuilt chain lets the sub-issue's read trust.
const parentFact = { id: ISSUE, delegated: true, by: "owner", state: "open", priority: 0, updatedAt: T0 };
const childRead = (id: string) => ({ kind: "commandResult", id, issue: ISSUE2, command: id, result: "ok", data: read({ delegate: "chain", parent: parentFact, sessions: [{ id: SESSION2, kind: "chain", createdAt: T0 }] }) as never }) as Event;

test("after a reset, a parent whose chain runs, and its chained sub-issue, are rebuilt in either order; the sub-issue's finish asks the parent again with the sub-issues' end", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await rebuildAsked(storage, ISSUE, orchestrated, NOW, sender);
  await rebuildAsked(storage, ISSUE2, null, NOW, sender);
  // The sub-issue's read lands first: no chain holds yet, so nothing begins, and its session is
  // none to map.
  await deliver(storage, childRead(sent[1].id), NOW, sender);
  assert.equal((await loadStore(storage, ISSUE2)).sessions[SESSION2]?.machine ?? null, null);
  assert.deepEqual(chainSessions(await loadStore(storage, ISSUE2)), []);
  // The parent's read takes its chain back, which reads the sub-issue again.
  await deliver(storage, parentRead(sent[0].id, false), NOW, sender);
  const parent = await loadStore(storage, ISSUE);
  assert.equal(parent.sessions[S].machine?.state, "done");
  assert.deepEqual(parent.sessions[S].facts.orchestrated?.known, [ISSUE2]);
  assert.deepEqual(parent.facts.chained?.issues, [ISSUE2]);
  // At rest, so a take-back or cancel halts the chain.
  assert.equal(parent.work?.state, "reaped");
  assert.equal((await loadStore(storage, ISSUE2)).facts.chain?.parent, ISSUE);
  const reread = sent.at(-1)!;
  assert.deepEqual([reread.kind, reread.issue, reread.machine], ["readIssue", ISSUE2, "issue"]);
  await deliver(storage, childRead(reread.id), NOW, sender);
  // Begun on the session the reset unmapped, which the relay maps again so its worker can report.
  assert.deepEqual(chainSessions(await loadStore(storage, ISSUE2)), [SESSION2]);
  const childAsk = sent.at(-1)!;
  assert.deepEqual([childAsk.kind, childAsk.machine], ["readIssue", SESSION2]);
  await deliver(storage, childRead(childAsk.id), NOW, sender);
  assert.equal((await loadStore(storage, ISSUE2)).work?.state, "launching");
  await deliver(storage, { kind: "issueStateChanged", id: "done", issue: ISSUE2, state: "completed", parent: ISSUE, at: T2 }, NOW, sender);
  const asked = (await loadStore(storage, ISSUE)).sessions[S];
  assert.equal(asked.machine?.state, "requested");
  assert.equal(asked.facts.request?.kind, "childrenEnd");
});

test("after a reset, a parent whose sub-issues have all finished asks again with the sub-issues' end at the rebuild", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await rebuildAsked(storage, ISSUE, orchestrated, NOW, sender);
  await deliver(storage, parentRead(sent[0].id, true), NOW, sender);
  const asked = (await loadStore(storage, ISSUE)).sessions[S];
  assert.equal(asked.machine?.state, "requested");
  assert.equal(asked.facts.request?.kind, "childrenEnd");
  assert.ok(sent.some((c) => c.kind === "readIssue" && c.issue === ISSUE && c.machine === S));
});

test("a rebuild takes no chain back for a parent its owner no longer stands behind", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await rebuildAsked(storage, ISSUE, orchestrated, NOW, sender);
  await deliver(storage, { kind: "commandResult", id: sent[0].id, issue: ISSUE, command: sent[0].id, result: "ok", data: read({ delegate: "other", children: [{ id: ISSUE2, done: false, triage: false, updatedAt: T0 }] }) as never }, NOW, sender);
  // Never sent, and dropped: no sub-issue takes it.
  assert.equal((await loadStore(storage, ISSUE)).facts.chained, null);
  assert.equal((await loadStore(storage, ISSUE2)).facts.chain, null);
});

test("after a reset, a chained sub-issue that is a parent itself, read before its parent's chain lands, takes its own chain back and starts no worker", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await rebuildAsked(storage, ISSUE, orchestrated, NOW, sender);
  await rebuildAsked(storage, ISSUE2, { session: SESSION2, children: [B] }, NOW, sender);
  const middleRead = (id: string) => ({ kind: "commandResult", id, issue: ISSUE2, command: id, result: "ok", data: read({ delegate: "chain", parent: parentFact, children: [{ id: B, done: false, triage: false, updatedAt: T0 }], sessions: [{ id: SESSION2, kind: "chain", createdAt: T0 }] }) as never }) as Event;
  await deliver(storage, middleRead(sent[1].id), NOW, sender);
  assert.equal((await loadStore(storage, B)).facts.chain, null);
  await deliver(storage, parentRead(sent[0].id, false), NOW, sender);
  const reread = sent.at(-1)!;
  assert.deepEqual([reread.kind, reread.issue, reread.params.orchestrated], ["readIssue", ISSUE2, SESSION2]);
  await deliver(storage, middleRead(reread.id), NOW, sender);
  const middle = await loadStore(storage, ISSUE2);
  assert.equal(middle.sessions[SESSION2].machine?.state, "done");
  assert.deepEqual(chainSessions(middle), [SESSION2]);
  assert.deepEqual(middle.facts.chained?.issues, [B]);
  assert.equal(middle.work?.state, "reaped");
  assert.equal((await loadStore(storage, B)).facts.chain?.parent, ISSUE2);
  assert.ok(!sent.some((c) => c.kind === "startWorker"));
});

test("a step writes each record it touches once, in one put, and hands listen its commands once that has landed", async () => {
  const puts: string[][] = [];
  let failAt = 0;
  const storage = new MemoryStorage();
  const counting: Storage = Object.assign(Object.create(storage), {
    put: async (entries: Record<string, unknown>) => {
      if (puts.push(Object.keys(entries)) === failAt) throw new Error("write failed");
      return storage.put(entries);
    },
  });
  const { sent, sender } = sink();
  await deliver(counting, start(ISSUE, S), NOW, sender);
  puts.length = 0;
  // The read's result asks the coordinator for a slot, whose grant starts the worker: the
  // event's own write, then the drain's, with each record once.
  failAt = 2;
  await assert.rejects(deliver(counting, readOk(ISSUE, S, sent[0].id), NOW, sender), /write failed/);
  assert.deepEqual(puts, [[`machine:issue:${ISSUE}`, "machine:due"], [`machine:issue:${ISSUE}`, "machine:coordinator", "machine:due"]]);
  // The drain's write failed, so nothing it sent reached listen, and the outbox is still owed.
  assert.deepEqual(sent.map((c) => c.kind), ["readIssue"]);
  assert.ok((await loadStore(storage, ISSUE)).outbox.some((o) => o.event.kind === "slotAsked"));
  failAt = 0;
  await reconcile(counting, NOW, sender, true);
  assert.deepEqual(sent.map((c) => c.kind), ["readIssue", "startWorker"]);
  assert.equal((await loadStore(storage, ISSUE)).work?.state, "launching");
});

test("what a result teaches lands in its step's own put, or not at all", async () => {
  const puts: string[][] = [];
  let fail = false;
  const storage = new MemoryStorage();
  const counting: Storage = Object.assign(Object.create(storage), {
    put: async (entries: Record<string, unknown>) => {
      puts.push(Object.keys(entries));
      if (fail) throw new Error("write failed");
      return storage.put(entries);
    },
  });
  const { sent, sender } = sink();
  await deliver(counting, start(ISSUE, S), NOW, sender);
  puts.length = 0;
  fail = true;
  const also = async () => ({ "intake:branch:x": ISSUE });
  await assert.rejects(deliver(counting, readOk(ISSUE, S, sent[0].id), NOW, sender, undefined, also), /write failed/);
  assert.deepEqual(puts, [["intake:branch:x", `machine:issue:${ISSUE}`, "machine:due"]]);
  assert.equal(await storage.get("intake:branch:x"), undefined);
  fail = false;
  await deliver(counting, readOk(ISSUE, S, sent[0].id), NOW, sender, undefined, also);
  assert.equal(await storage.get("intake:branch:x"), ISSUE);
});

test("a push fanned out to more stores than one put takes is delivered in several, each within storage's 128 entries", async () => {
  const storage = new MemoryStorage();
  const issues = Array.from({ length: 300 }, (_, i) => `00000000-0000-4000-8000-${String(i).padStart(12, "0")}`);
  const c = newCoordinator();
  c.prs = { "o/r": issues };
  await storage.put("machine:coordinator", c);
  const puts: number[] = [];
  const counting: Storage = Object.assign(Object.create(storage), {
    put: async (entries: Record<string, unknown>) => {
      puts.push(Object.keys(entries).length);
      return storage.put(entries);
    },
  });
  const { sender } = sink();
  await repoPushed(counting, "11111111-1111-4111-8111-111111111111", "o/r", SHA, NOW, sender);
  assert.ok(puts.length > 2 && puts.every((n) => n <= 128), `puts of ${puts.join(", ")} entries`);
  assert.deepEqual((await loadCoordinator(storage)).outbox, []);
  assert.equal((await loadStore(storage, issues[299])).facts.base, SHA);
});

test("the compare-and-swap refuses a write over one it didn't read, and runs the step again on what won", async () => {
  const storage = new MemoryStorage();
  const { sender } = sink();
  await deliver(storage, start(ISSUE, S), NOW, sender);
  const key = `machine:issue:${ISSUE}`;
  // Another write lands between this step's read of the store and its write, once: before
  // the write's own read of the revisions it checks.
  const bump = async () => {
    const s = (await storage.get<Store>(key))!;
    await storage.put(key, { ...s, rev: s.rev + 1 });
  };
  let checks = 0;
  const racing: Storage = Object.assign(Object.create(storage), {
    get: async (k: string | string[]) => {
      if (Array.isArray(k) && k.includes(key) && ++checks === 1) await bump();
      return storage.get(k as string[]);
    },
  });
  await deliver(racing, { kind: "ownerPrompted", id: "p", issue: ISSUE, session: S, byOwner: true, at: T1 }, NOW, sender);
  const s = await loadStore(storage, ISSUE);
  assert.equal(s.sessions[S].facts.request?.at, T1);
  // Behind the read in flight, the prompt waits, once.
  assert.deepEqual(s.waiting.map((w) => w.event.id), ["p"]);
  // A store that keeps moving under every write is refused after its retries.
  const always: Storage = Object.assign(Object.create(storage), {
    get: async (k: string | string[]) => {
      if (Array.isArray(k) && k.includes(key)) await bump();
      return storage.get(k as string[]);
    },
  });
  await assert.rejects(deliver(always, { kind: "ownerPrompted", id: "q", issue: ISSUE, session: S, byOwner: true, at: T2 }, NOW, sender), Conflict);
});

test("an event between stores delivered twice applies once", () => {
  const s = given({ req: "done", sess: { orchestrated: { at: T0, known: [] } }, facts: { children: { sentMs: NOW - HOUR, ids: [B], states: { [B]: { done: false, at: T0 } }, more: false } } });
  const e: Event = { kind: "childChanged", id: "childChanged:x:1", issue: ISSUE, child: B, done: true, at: T1 };
  const once = receive(s, e, NOW, { store: B, seq: 1 });
  const twice = receive(once.store, e, NOW, { store: B, seq: 1 });
  assert.equal(twice.store, once.store);
});

// ---- The reconcile ----

test("a command with no result is sent again with its ID, three times in all, then ended as lost", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await deliver(storage, start(ISSUE, S), NOW, sender);
  await deliver(storage, readOk(ISSUE, S, sent[0].id), NOW, sender);
  const startWorker = sent.at(-1)!;
  assert.equal(startWorker.kind, "startWorker");
  await reconcile(storage, NOW + 10 * MIN, sender, true);
  await reconcile(storage, NOW + 20 * MIN, sender, true);
  assert.deepEqual(sent.filter((c) => c.id === startWorker.id).map((c) => c.sends), [1, 2, 3]);
  await reconcile(storage, NOW + 30 * MIN, sender, true);
  const s = await loadStore(storage, ISSUE);
  assert.equal(s.work?.state, "exited");
  assert.equal(s.sessions[S].machine?.state, "done");
});

test("a command waiting on its worker's machine to wake spends no send until WAKE_WAIT_MS has passed", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await deliver(storage, start(ISSUE, S), NOW, sender);
  await deliver(storage, readOk(ISSUE, S, sent[0].id), NOW, sender);
  const command = sent.at(-1)!;
  const sends = () => sent.filter((c) => c.id === command.id).map((c) => c.sends);
  for (const m of [0, 10, 20, 30]) {
    await deliver(storage, result(command.id, "retryable", { waking: true }), NOW + m * MIN, sender);
    await reconcile(storage, NOW + (m + 10) * MIN, sender, true);
  }
  assert.deepEqual(sends(), [1, 1, 1, 1, 2]);
});

// docs/state-machine.md, Commands: a waiting command goes again the moment its cause clears,
// spending no send, and only then; with every clearing lost, its timeout still sends it.
test("a command waiting on a host or the lock is sent again as soon as that clears, and at its timeout without it", async () => {
  for (const [waitsOn, clears, others] of [
    [{ host: "mini" }, { hosts: ["mac", "mini"] }, [{ hosts: ["mac"] }, { lock: true }]],
    [{ lock: true }, { lock: true }, [{ hosts: ["mini"] }]],
    [{ host: "mini" }, { all: true }, []],
  ] as const) {
    const storage = new MemoryStorage();
    const { sent, sender } = sink();
    await deliver(storage, start(ISSUE, S), NOW, sender);
    await deliver(storage, readOk(ISSUE, S, sent[0].id), NOW, sender);
    const command = sent.at(-1)!;
    const sends = () => sent.filter((c) => c.id === command.id).map((c) => [c.sends, c.sentMs]);
    await deliver(storage, result(command.id, "retryable", { waitsOn }), NOW + MIN, sender);
    for (const other of others) await waitsCleared(storage, other, NOW + 2 * MIN, sender);
    assert.deepEqual(sends(), [[1, NOW]], JSON.stringify(waitsOn));
    await waitsCleared(storage, clears, NOW + 3 * MIN, sender);
    assert.deepEqual(sends(), [[1, NOW], [1, NOW + 3 * MIN]], JSON.stringify(clears));
    // Sent, it waits on nothing until it is answered again.
    await waitsCleared(storage, clears, NOW + 4 * MIN, sender);
    assert.equal(sends().length, 2);
    // A clearing lost: its timeout, begun again at the resend, sends it, as one of its sends.
    await deliver(storage, result(command.id, "retryable", { waitsOn }), NOW + 5 * MIN, sender);
    await reconcile(storage, NOW + 13 * MIN, sender, true);
    assert.deepEqual(sends().at(-1), [2, NOW + 13 * MIN]);
  }
});

// A command that waits again on what keeps clearing, as on a host that answers while the command
// fails otherwise, spends a send on each clearing after the first, and so still ends.
test("a command waiting again after its clearing's send spends its sends, and ends lost", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await deliver(storage, start(ISSUE, S), NOW, sender);
  await deliver(storage, readOk(ISSUE, S, sent[0].id), NOW, sender);
  const command = sent.at(-1)!;
  for (let m = 1; m <= 5; m++) {
    await deliver(storage, result(command.id, "retryable", { waitsOn: { host: "mini" } }), NOW + m * MIN, sender);
    await waitsCleared(storage, { hosts: ["mini"] }, NOW + m * MIN, sender);
  }
  assert.deepEqual(sent.filter((c) => c.id === command.id).map((c) => c.sends), [1, 1, 2, 3]);
  await reconcile(storage, NOW + 20 * MIN, sender, true);
  assert.equal((await loadStore(storage, ISSUE)).work?.state, "exited");
});

// Alarms every minute while nothing listens, with each `postAway` the relay posts answered as
// it would be: the minutes it was posted at.
async function awayFor(storage: MemoryStorage, sent: Command[], sender: (c: Command[]) => void, minutes: number): Promise<number[]> {
  const posted: number[] = [];
  for (let m = 1; m <= minutes; m++) {
    const before = sent.length;
    await reconcile(storage, NOW + m * MIN, sender, false);
    for (const c of sent.slice(before).filter((c) => c.kind === "postAway")) {
      posted.push(m);
      await deliver(storage, result(c.id, "ok"), NOW + m * MIN, sender);
    }
  }
  return posted;
}

test("while nothing listens, a request's command waits, and the away message is posted once, 15 minutes after", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await deliver(storage, start(ISSUE, S), NOW, sender);
  // The read's timeout, at 2 minutes, finds `listen` gone: from then on the read waits, never
  // resent or lost, and the away message follows 15 minutes on, once.
  assert.deepEqual(await awayFor(storage, sent, sender, 60), [17]);
  const s = await loadStore(storage, ISSUE);
  assert.deepEqual(sent.filter((c) => c.kind === "readIssue").map((c) => c.sends), [1]);
  assert.deepEqual([s.away, s.sessions[S].machine?.state, s.facts.notices], [true, "requested", [`away:${S}:${T0}`]]);
  // A store whose only wait is `listen` is kept for the connection, which gives the read its
  // timeout again and sends it again.
  assert.deepEqual([await storage.get("machine:due"), await storage.get("machine:held")], [{}, [ISSUE]]);
  const again = await connected(storage, NOW + 2 * HOUR, () => assert.fail("a store refused the connection"));
  assert.deepEqual(again.map((c) => [c.kind, c.sends]), [["readIssue", 2]]);
  const back = await loadStore(storage, ISSUE);
  assert.deepEqual([back.away, back.queue[0].sentMs], [false, NOW + 2 * HOUR]);
  assert.deepEqual([await storage.get("machine:due"), await storage.get("machine:held")], [{ [ISSUE]: NOW + 2 * HOUR + 2 * MIN }, []]);
});

test("the Worker's away acknowledgement is the request's away message: the alarm doesn't say it again", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await deliver(storage, start(ISSUE, S), NOW, sender);
  await acknowledgedAway(storage, ISSUE, S, T0, NOW, sender);
  assert.deepEqual(await awayFor(storage, sent, sender, 60), []);
  // A newer request is told once more.
  await deliver(storage, { kind: "ownerPrompted", id: "again", issue: ISSUE, session: S, byOwner: true, at: T1 }, NOW + 61 * MIN, sender);
  const s = await loadStore(storage, ISSUE);
  const quiet = s.sessions[S].machine?.deadlines.find((d) => d.kind === "quiet");
  assert.equal(quiet?.dueMs, NOW + 76 * MIN);
});

test("a connection gives a held command its timeout again but no fresh sends, and leaves a store it never held running", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await deliver(storage, start(ISSUE, S), NOW, sender);
  await deliver(storage, readOk(ISSUE, S, sent[0].id), NOW, sender);
  const worker = sent.at(-1)!;
  // Sent twice while listening, then held away at its second timeout.
  await reconcile(storage, NOW + 10 * MIN, sender, true);
  await reconcile(storage, NOW + 20 * MIN, sender, false);
  assert.equal((await loadStore(storage, ISSUE)).away, true);
  // Something listens again without a reconnect: the alarm is due at once, and tells it.
  assert.equal(await machineTimer(storage, sender, () => true).next(), 0);
  assert.notEqual(await machineTimer(storage, sender, () => false).next(), 0);
  sent.length = 0;
  await reconcile(storage, NOW + HOUR, sender, true);
  // Sent again as its third send, then `lost` at its timeout.
  assert.deepEqual(sent.map((c) => [c.id, c.sends]), [[worker.id, 3]]);
  await reconcile(storage, NOW + HOUR + 10 * MIN, sender, true);
  assert.equal((await loadStore(storage, ISSUE)).work?.state, "exited");
  // A command that has had its sends while held, as one that crashes `listen` each time, ends
  // `lost` as `listen` connects.
  const spent = given({ req: "claimed", away: true, queue: [{ kind: "observe", machine: S, sends: 3 }] });
  const back = receive(spent, { kind: "listenConnected", id: "c", issue: ISSUE }, NOW);
  assert.deepEqual([back.store.away, back.store.queue.length, back.send], [false, 0, []]);
  // A connection to a store it never held leaves its command's timeout as it was.
  const other = new MemoryStorage();
  await deliver(other, start(ISSUE, S), NOW, sender);
  const before = await loadStore(other, ISSUE);
  await connected(other, NOW + MIN, () => assert.fail("refused"));
  assert.deepEqual((await loadStore(other, ISSUE)).queue, before.queue);
});

test("a superseded command runs its timeouts on to lost while listen is away", () => {
  const s = given({ req: "claimed", away: true, queue: [{ kind: "observe", machine: S, superseded: true, sentMs: NOW - 10 * MIN, sends: 3 }] });
  const lost = expire(s, NOW);
  assert.deepEqual(lost.store.queue, []);
  assert.equal(lost.store.away, false);
});

test("a failed away message goes once more, its key kept", async () => {
  const storage = new MemoryStorage();
  const { sent, sender } = sink();
  await deliver(storage, start(ISSUE, S), NOW, sender);
  for (let m = 1; m <= 17; m++) await reconcile(storage, NOW + m * MIN, sender, false);
  const post = sent.find((c) => c.kind === "postAway")!;
  assert.deepEqual(post.params, { session: S, key: `away:${S}:${T0}` });
  await deliver(storage, result(post.id, "failed"), NOW + 17 * MIN, sender);
  const s = await loadStore(storage, ISSUE);
  assert.deepEqual(s.facts.notices, [`away:${S}:${T0}`]);
  assert.deepEqual(s.queue.filter((c) => c.kind === "postAway").map((c) => c.params), [{ session: S, key: `away:${S}:${T0}`, again: true }]);
});

// ---- Shapes ----

test("an event's IDs, times and enums are checked before any store sees it", () => {
  assert.deepEqual(parseEvent({ ...start(ISSUE, S), text: "an issue's words" }), start(ISSUE, S));
  assert.throws(() => parseEvent(start("TW-1", S)), Invalid);
  assert.throws(() => parseEvent({ ...start(ISSUE, S), at: "yesterday" }), Invalid);
  assert.throws(() => parseEvent({ kind: "ownerPrompted", id: "x", issue: ISSUE, byOwner: true, at: T0 }), Invalid, "a missing session");
  assert.throws(() => parseEvent({ kind: "prPushed", id: "x", issue: ISSUE, pr: 1, head: "not a sha", at: T0 }), Invalid);
  // A push names the head it replaced, so it can be ordered by its chain.
  assert.deepEqual(parseEvent({ kind: "prPushed", id: "x", issue: ISSUE, pr: 1, head: "abcdef1", before: "abcdef2", at: T0 }), { kind: "prPushed", id: "x", issue: ISSUE, pr: 1, head: "abcdef1", before: "abcdef2", at: T0 });
  for (const before of [undefined, null, "not a sha"]) assert.throws(() => parseEvent({ kind: "prPushed", id: "x", issue: ISSUE, pr: 1, head: "abcdef1", before, at: T0 }), Invalid);
  assert.throws(() => parseEvent({ kind: "commandResult", id: "x", issue: ISSUE, command: "rm -rf", result: "ok" }), Invalid);
  assert.throws(() => parseEvent({ kind: "somethingElse", id: "x", issue: ISSUE }), Invalid);
  // What only a store, the coordinator or the alarm makes is never taken from outside.
  for (const kind of ["workRequested", "stopRequested", "workChanged", "slotGranted", "slotWaiting", "deadline", "blockersChanged", "childChanged", "parentHalted", "basePushed"]) {
    assert.throws(() => parseEvent({ kind, id: "x", issue: ISSUE, session: S, at: T0 }), Invalid, kind);
  }
  assert.throws(() => parseEvent({ kind: "chainRecorded", id: "x", issue: ISSUE, role: "forward", parent: S, run: "r1" }), Invalid);
});

test("a second session on the issue keeps to its own machine", () => {
  const o = apply(given({ req: "claimed", other: "done" }), ev("ownerPrompted", { session: S2, byOwner: true, at: T1 }));
  assert.equal(o.req, "claimed");
  assert.equal(o.store.sessions[S2].machine?.state, "requested");
});

// ---- Found in review ----

test("a cancel a request's read finds reaches the work machine too, which stops its worker", () => {
  const s = given({ req: "requested", work: "running", other: "claimed", queue: [readIssue] });
  const o = apply(s, result(`${ISSUE}.1`, "ok", read({ state: "cancelled" })));
  assert.equal(o.work, "halted");
  assert.ok(o.commands.includes("stopWorker"));
});

test("an event for a session the store never heard of changes nothing, and doesn't wedge the queue", () => {
  const s = given({ req: "claimed", queue: [readIssue] });
  const waited = receive(s, ev("turnEnded", { session: SESSION2 }), NOW).store;
  const done = receive(waited, result(`${ISSUE}.1`, "ok", read()), NOW);
  assert.equal(done.store.waiting.length, 0);
});

test("a deadline left waiting is dropped if its state was left meanwhile", () => {
  const s = given({ work: "exited", queue: [{ kind: "readIssue", machine: "work" }], armed: [{ machine: "work", kind: "own" }] });
  let r = receive(s, { kind: "deadline", id: "d", issue: ISSUE, machine: "work", version: 1, deadline: "own" }, NOW).store;
  assert.equal(r.waiting.length, 1);
  r = receive(r, ev("issueStateChanged", { state: "cancelled", parent: null, at: T1 }), NOW).store;
  const done = receive(r, result(`${ISSUE}.1`, "ok", read({ state: "cancelled" })), NOW);
  assert.equal(done.store.work?.state, "halted");
  assert.ok(!done.store.queue.some((c) => c.kind === "reapWorktree"), "the stale exited deadline didn't run");
});

test("a grant answering the ask after the work moved goes to the re-check, keeping the reservation", () => {
  const s = given({ req: "queued", work: "merged", sess: { waiting: "slot", asked: "slotAsked:x:1" } });
  const o = apply(s, ev("slotGranted", { session: S, answers: "slotAsked:x:1" }));
  assert.deepEqual([o.req, o.commands], ["requested", ["readIssue"]]);
  assert.notEqual(o.store.sessions[S].facts.grant, null);
});

test("a free slot isn't held back by a queue of issues that already hold one", () => {
  const c = newCoordinator();
  c.grants.a = { session: null, sinceMs: NOW };
  c.queue = [{ issue: "a", session: "1", priority: 0, at: T0 }];
  const sent = coordinate(c, { kind: "slotAsked", id: "b1", issue: "b", session: "2", priority: 0, at: T1 }, NOW, ALONE);
  assert.deepEqual(sent.map((e) => e.kind), ["slotGranted"]);
});

test("a repo named like an object's own property is only a key", () => {
  const c = newCoordinator();
  assert.doesNotThrow(() => coordinate(c, { kind: "prRecorded", id: "1", issue: "a", repo: "constructor", open: true }, NOW, ALONE));
  assert.deepEqual(coordinate(c, { kind: "repoPushed", id: "2", repo: "toString", base: "abcdef1" }, NOW, ALONE), []);
});

test("a late prOpened for an older PR doesn't replace the newer one", () => {
  const s = given({ work: "rest", facts: { pr: { number: 6, head: "abcdef1", repo: "repo", status: "open", at: T2, receivedMs: 0 } } });
  const o = apply(s, ev("prOpened", { pr: 5, head: "abcdef2", repo: "repo", at: T0 }));
  assert.equal(o.store.facts.pr?.number, 6);
});

test("an older run's take-back leaves a newer one owed", () => {
  const s = given({ work: "running", facts: { takeBack: { run: "r2" } }, queue: [{ kind: "takeBackChain", machine: "work", params: { run: "r1" } }] });
  const o = apply(s, result(`${ISSUE}.1`, "ok"));
  assert.deepEqual(o.store.facts.takeBack, { run: "r2" });
});

test("the reconcile keeps each command's sends when an earlier one is lost in the same pass", () => {
  const s = given({ work: "running", queue: [{ kind: "observe", machine: "work", sends: 3, sentMs: NOW - 3 * MIN }, { kind: "postNotice", machine: "work", class1: true, sends: 1, sentMs: NOW - 3 * MIN, params: { key: "k" } }] });
  const r = expire(s, NOW);
  assert.equal(r.store.queue.find((c) => c.kind === "postNotice")?.sends, 2);
});

test("a sub-issue whose own parent is another refuses the chain; one that takes it after a halt is halted", () => {
  const s = given({ work: "running", sess: { orchestrated: { at: T0, known: [] } } });
  const asked = apply(s, { kind: "chainRecorded", id: "c", issue: ISSUE, role: "ask", session: S, run: "r1", children: [B] });
  assert.deepEqual(asked.store.outbox.map((o) => o.to), [B]);
  const other = given({ facts: { parent: { id: ISSUE2, at: T0, delegated: true, by: "owner", state: "open", parentAt: T0 } } });
  const refused = receive(other, { kind: "chainRecorded", id: "f", issue: ISSUE, role: "forward", parent: B, run: "r1" }, NOW);
  assert.equal(refused.store.facts.chain, null);
  // The parent is undelegated before the answer lands: the run delegates nothing, and the
  // late taker is halted.
  const halted = receive(asked.store, ev("delegateChanged", { by: null, at: T1 }), NOW).store;
  assert.equal(halted.facts.chained?.haltedAt, T1);
  assert.ok(halted.queue.some((c) => c.kind === "stopWorker"));
  // It lands at once, beside the stop, and halts the taker.
  const late = receive(halted, { kind: "chainRecorded", id: "a", issue: ISSUE, role: "answer", child: B, run: "r1", took: true }, NOW).store;
  assert.deepEqual(late.waiting, []);
  assert.deepEqual(late.outbox.at(-1)?.event, { kind: "parentHalted", id: late.outbox.at(-1)?.id, issue: B, parent: ISSUE, at: T1 });
});

// A store for `issue` under ISSUE, as `given` makes ISSUE's.
const under = (issue: string, g: Parameters<typeof given>[0] = {}): Store => {
  const s = given({ ...g, facts: { parent: { id: ISSUE, at: T0, delegated: true, by: "owner", state: "open", parentAt: T0 }, ...g.facts } });
  s.issue = issue;
  s.queue = s.queue.map((c) => ({ ...c, id: c.id.replace(ISSUE, issue), issue }));
  return s;
};

test("a chain asked while the parent resumes and a sibling's command is in flight is answered with every sub-issue that took it", async () => {
  const storage = new MemoryStorage();
  const { sender } = sink();
  // The parent resumed for a sub-issue added, B2, with its resume still in flight; B, chained
  // by the first run, has its own command in flight.
  const first = { run: "r-1", issues: [B], asked: [B], answered: [B], haltedAt: null };
  await storage.put("machine:issue:" + ISSUE, given({ work: "launching", w: { launch: "resume" }, sess: { orchestrated: { at: T0, known: [B, B2] } }, facts: { chained: first }, queue: [{ kind: "resumeWorker", machine: "work" }] }));
  await storage.put("machine:issue:" + B, under(B, { work: "running", facts: { chain: { parent: ISSUE, run: "r-1" } }, queue: [{ kind: "observe", machine: "work" }] }));
  await storage.put("machine:issue:" + B2, under(B2));
  await deliver(storage, { kind: "chainRecorded", id: "ask", issue: ISSUE, role: "ask", session: S, run: "r-2", children: [B, B2] }, NOW, sender);
  assert.deepEqual(await chained(storage, ISSUE, "r-2"), [B, B2]);
  assert.deepEqual((await loadStore(storage, B2)).facts.chain, { parent: ISSUE, run: "r-2" });
  assert.equal((await loadStore(storage, B)).queue.length, 1);
  // A run the parent's store never took is unanswered, never a chain every sub-issue refused.
  assert.equal(await chained(storage, ISSUE, "r-3"), null);
});

test("a start's result the store drops reaches `also` with the live worker's worktree, not its own", async () => {
  const storage = new MemoryStorage();
  const { sender } = sink();
  await storage.put("machine:issue:" + ISSUE, given({ work: "running" }));
  const seen: (string | undefined)[] = [];
  const late = result(`${ISSUE}.99`, "ok", { worktree: { id: "repo::C:/w/old", branch: "old" } });
  await deliver(storage, late, NOW, sender, undefined, async (after) => {
    seen.push(after.facts.worktree?.id);
    return {};
  });
  assert.deepEqual(seen, ["repo::C:/w/tw-1"]);
});

test("a chain asked beside a take-back still queued is refused, so the take-back can't undo it", () => {
  const s = given({ work: "running", sess: { orchestrated: { at: T0, known: [] } }, facts: { takeBack: { run: "r-1" }, chained: { run: "r-1", issues: [B], asked: [B], answered: [B], haltedAt: T0 } }, queue: [{ kind: "takeBackChain", machine: "work", params: { run: "r-1" } }] });
  const o = receive(s, { kind: "chainRecorded", id: "ask", issue: ISSUE, role: "ask", session: S, run: "r-2", children: [B] }, NOW).store;
  assert.equal(o.facts.chained?.run, "r-1");
  assert.deepEqual(o.facts.takeBack, { run: "r-1" });
  assert.deepEqual(o.outbox, []);
  // Its take-back sent, owed or not, refuses it just the same; once it ends, the chain is taken.
  assert.equal(receive({ ...s, facts: { ...s.facts, takeBack: null } }, { kind: "chainRecorded", id: "ask", issue: ISSUE, role: "ask", session: S, run: "r-2", children: [B] }, NOW).store.facts.chained?.run, "r-1");
  const done = receive(s, result(s.queue[0].id, "ok"), NOW).store;
  const taken = receive(done, { kind: "chainRecorded", id: "ask", issue: ISSUE, role: "ask", session: S, run: "r-2", children: [B] }, NOW).store;
  assert.equal(taken.facts.chained?.run, "r-2");
  assert.equal(taken.facts.takeBack, null);
});

test("a turn whose start was lost holds its claim until observe settles it", () => {
  const o = apply(given({ req: "claimed", kind: "mention", sess: { turn: { terminal: null, sinceMs: NOW, for: T0, idleMs: null, closingMs: null } }, queue: [{ kind: "startTurn", machine: S }] }), result(`${ISSUE}.1`, "lost"));
  assert.deepEqual([o.req, o.commands], ["claimed", []]);
});

test("a machine leaving the states that hold a stop drops the stop owed, and its stops with it", () => {
  const s = given({ work: "halted", req: "done", w: { stop: { command: `${ISSUE}.1`, sentMs: NOW - MIN, since: NOW - MIN } }, facts: { halted: { at: T0, by: "stop" } }, queue: [{ kind: "stopWorker", machine: "work", version: 2 }, { kind: "stopWorker", machine: "work", version: 2, sentMs: null }] });
  const o = apply(s, ev("workRequested", { session: S, at: T1, request: "prompt", orchestrate: false }));
  assert.equal(o.store.work?.state, "launching");
  assert.equal(o.store.work?.stop, null);
  // The one in flight runs on to its result, never sent again; the one not yet sent is gone.
  assert.deepEqual(o.store.queue.filter((c) => c.kind === "stopWorker").map((c) => c.superseded), [true]);
});

test("a new work machine carries its version on, and a stop the old one owed is moot", () => {
  const s = given({ work: "reaped", w: { version: 40, stop: { command: `${ISSUE}.0`, sentMs: NOW - MIN, since: NOW - MIN } }, facts: { worktree: null, halted: { at: T0, by: "stop" } } });
  const o = apply(s, ev("workRequested", { session: S, at: T1, request: "prompt", orchestrate: false }));
  assert.ok((o.store.work?.version ?? 0) > 40);
  assert.equal(o.store.work?.stop, null);
});

test("with listen away, a stop owed is sent again only once it connects, and its deadline is only armed again", () => {
  const s = given({ work: "halted", req: "done", w: { stop: { command: `${ISSUE}.1`, sentMs: NOW - MIN, since: NOW - MIN }, deadlines: [{ kind: "own", version: 2, dueMs: NOW }] }, facts: { halted: { at: T0, by: "stop" } }, queue: [{ kind: "stopWorker", machine: "work", version: 2 }] });
  const gone = receive(s, { kind: "listenGone", id: "g", issue: ISSUE }, NOW).store;
  assert.equal(gone.away, true);
  const fired = receive(gone, { kind: "deadline", id: "d", issue: ISSUE, machine: "work", version: 2, deadline: "own" }, NOW);
  assert.deepEqual([fired.send, fired.store.queue.length], [[], 1]);
  assert.deepEqual(fired.store.work?.deadlines.map((d) => [d.kind, d.dueMs]), [["own", NOW + 2 * MIN]]);
  // Nothing is due for the stop in flight but its own deadline.
  assert.equal(nextDue(fired.store, NOW), NOW + 2 * MIN);
});

test("a comment's line that landed covers the owner's comments up to the time it carried", () => {
  const s = given({ work: "running", queue: [{ kind: "deliverMessage", machine: "work", params: { comment: true, through: T1 } }] });
  assert.equal(apply(s, result(`${ISSUE}.1`, "ok")).store.facts.commentTold, T1);
  assert.equal(apply(s, result(`${ISSUE}.1`, "held")).store.facts.commentTold, null);
  assert.deepEqual(parseEvent({ kind: "ownerCommented", id: "d1", issue: ISSUE, comment: B, at: T1 }), { kind: "ownerCommented", id: "d1", issue: ISSUE, comment: B, at: T1 });
});

test("a mention whose session arrives after its comment's read is told only as the session's request", () => {
  const reading = given({ work: "running", queue: [{ kind: "readComment", machine: "work", params: { comment: B, at: T1 } }] });
  const plain = result(`${ISSUE}.1`, "ok", { comment: { kind: "plain", newestOwnerAt: T1, state: "open" } });
  assert.deepEqual(apply(reading, plain).commands, ["deliverMessage"]);
  // Linear made the session after `listen` read the comment, and its event lands before the
  // read's result: the session's comment is kept at once, though its transition waits.
  const mention = ev("sessionStarted", { session: S2, sessionKind: "mention", byOwner: true, comment: B, at: T1 });
  const landed = apply(reading, mention);
  assert.equal(landed.store.facts.sessions[S2].comment, B);
  const read = apply(landed.store, plain);
  assert.ok(!read.commands.includes("deliverMessage"));
  assert.ok(read.commands.includes("readIssue"));
  // Its event lands after the comment's line: the session's request is all it adds.
  const told = apply(given({ work: "running", facts: { commentTold: T1 } }), mention);
  assert.ok(!told.commands.includes("deliverMessage"));
  assert.equal(told.store.facts.commentTold, T1);
  assert.equal(told.store.facts.sessions[S2].comment, B);
  // A mention in a reply to a note of Tolkster's opens no turn in the note's thread too.
  const fresh = new Date(NOW - MIN).toISOString();
  const replying = given({ work: "running", queue: [{ kind: "readComment", machine: "work", params: { comment: B, at: fresh } }] });
  const noteReply = result(`${ISSUE}.1`, "ok", { comment: { kind: "noteReply", newestOwnerAt: fresh, state: "open", root: B2 } });
  assert.notEqual(apply(replying, noteReply).store.sessions[B2], undefined);
  assert.equal(apply(apply(replying, mention).store, noteReply).store.sessions[B2], undefined);
  // Only an ID, and only a comment's.
  assert.deepEqual(parseEvent({ kind: "sessionStarted", id: "d1", issue: ISSUE, session: S2, sessionKind: "mention", byOwner: true, comment: B, at: T1 }).kind, "sessionStarted");
  assert.throws(() => parseEvent({ kind: "sessionStarted", id: "d1", issue: ISSUE, session: S2, sessionKind: "mention", byOwner: true, comment: "text", at: T1 }), Invalid);
});

// ---- A closed terminal ----

const running = (terminal: string) => given({ req: "claimed", sess: { turn: { terminal, sinceMs: NOW - MIN, for: T0, idleMs: null, closingMs: null } } });
const stored = async (store: Store, ...terminals: string[]) => {
  const storage = new MemoryStorage();
  await storage.put(`machine:issue:${ISSUE}`, store);
  for (const terminal of terminals) await storage.put(TURN_TERMINAL + terminal, ISSUE);
  return storage;
};

test("a closed terminal ends the turn that holds it, with nothing but the Durable Object's own record, as after a restart of listen", async () => {
  const storage = await stored(running("term_1"), "term_1");
  await terminalClosed(storage, "term_1", "1700:7", NOW, () => {});
  const s = await loadStore(storage, ISSUE);
  assert.equal(s.sessions[S].machine?.state, "done");
  assert.equal(s.sessions[S].facts.turn, null);
  assert.equal(await storage.get(TURN_TERMINAL + "term_1"), undefined);
});

test("an older turn's terminal closing after a newer turn opened ends nothing, and is forgotten", async () => {
  const storage = await stored(running("term_2"), "term_1", "term_2");
  await terminalClosed(storage, "term_1", "1700:8", NOW, () => {});
  assert.equal((await loadStore(storage, ISSUE)).sessions[S].machine?.state, "claimed");
  assert.equal(await storage.get(TURN_TERMINAL + "term_1"), undefined);
  assert.equal(await storage.get(TURN_TERMINAL + "term_2"), ISSUE);
});

test("a terminal that ran no turn, a worker's, ends nothing and writes nothing", async () => {
  const storage = await stored(running("term_1"), "term_1");
  const before = structuredClone([...storage.data]);
  await terminalClosed(storage, "term_9", "1700:9", NOW, () => {});
  assert.deepEqual([...storage.data], before);
});
