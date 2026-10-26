import assert from "node:assert/strict";
import { test } from "node:test";
import { inFlight, loadTriage, reconcile, triageEvent } from "../src/worker/machine/store.ts";
import {
  expireTriage,
  MAX_ASKED,
  newTriage,
  stepTriage,
  TRIAGE_OBSERVE_MS,
  TRIAGE_QUICK_END_MS,
  TRIAGE_RETRIES,
  TRIAGE_RETRY_FIRST_MS,
  TRIAGE_RETRY_LAST_MS,
  triageDue,
  triaging,
  type Triage,
  type TriageEvent,
  type TriageState,
} from "../src/worker/machine/triage.ts";
import { COMMAND_TIMEOUT_MS, HOUR, SENDS, START_TIMEOUT_MS, type Command } from "../src/worker/machine/types.ts";
import { parseTriageEvent } from "../src/worker/machine/validate.ts";
import { triageWire } from "../src/worker/route.ts";
import { Invalid } from "../src/worker/check.ts";
import contract from "./contract.json" with { type: "json" };
import { MIN, NOW, rowsOf } from "./machine-fixture.ts";
import { MemoryStorage } from "./storage.ts";

const A = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const B = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const SESSION = { id: "0123456789abcdef", terminal: "term_1", issues: ["TW-1", "TW-2"] };

type Given = { state?: TriageState; command?: "startTriage" | "observeTriage"; asked?: string[]; sent?: string[]; session?: boolean; failures?: number; deadline?: boolean; ended?: string; since?: number; lost?: string; away?: true; sends?: number };

function given(g: Given = {}): Triage {
  const t = newTriage();
  t.state = g.state ?? "idle";
  t.version = 5;
  t.asked = g.asked ?? [];
  t.failures = g.failures ?? 0;
  t.ended = g.ended === undefined ? [] : [g.ended];
  t.since = g.since ?? null;
  t.lost = g.lost ?? null;
  t.away = g.away === true;
  if (g.session ?? t.state === "running") t.session = SESSION;
  if (g.deadline ?? (t.state === "running" || t.state === "retrying")) t.deadline = { version: 5, dueMs: NOW };
  if (g.command !== undefined) {
    t.commandSeq = 3;
    const params = g.command === "startTriage" ? { issueIds: g.sent ?? [] } : { session: SESSION.id, terminal: SESSION.terminal };
    t.command = { id: "triage.3", issue: "triage", kind: g.command, machine: "triage", version: 5, params, class1: false, firstSentMs: NOW, sentMs: NOW, sends: g.sends ?? 1, superseded: false };
  }
  return t;
}

const asked = (issue = A): TriageEvent => ({ kind: "triageAsked", id: "d-1", issue });
const ended = (terminal = SESSION.terminal): TriageEvent => ({ kind: "triageEnded", id: "c:1", terminal });
const result = (r: "ok" | "failed" | "retryable" | "held" | "lost", data?: object, command = "triage.3"): TriageEvent => ({ kind: "commandResult", id: `result:${command}`, command, result: r, ...(data === undefined ? {} : { data }) });
const deadline = (version = 5): TriageEvent => ({ kind: "deadline", id: "triage:deadline", version });

type Expect = { state?: TriageState; commands?: string[]; deadlineMs?: number | null; asked?: string[]; unchanged?: true; session?: boolean; ended?: string; sentMs?: number; sends?: number; failures?: number; away?: boolean };
type Case = { row: string; when: string; given: Given; event: TriageEvent; expect: Expect };

const late = (data: object = { session: SESSION }) => result("ok", data, "triage.2");
const connected: TriageEvent = { kind: "listenConnected", id: "connected:1" };
const gone: TriageEvent = { kind: "listenGone", id: "gone:1" };

const cases: Case[] = [
  { row: "idle triageAsked", when: "—", given: {}, event: asked(), expect: { state: "starting", commands: ["startTriage"], asked: [A] } },
  { row: "starting triageAsked", when: "—", given: { state: "starting", command: "startTriage", sent: [A], asked: [A] }, event: asked(B), expect: { state: "starting", commands: [], asked: [A, B] } },
  { row: "running triageEnded", when: "The session held, ended within 30 seconds", given: { state: "running", since: NOW + MIN - 1000, asked: [A] }, event: ended(), expect: { state: "retrying", commands: [], session: false, deadlineMs: NOW + MIN + TRIAGE_RETRY_FIRST_MS, asked: [A] } },
  { row: "running triageEnded", when: "The session held", given: { state: "running", since: NOW + MIN - TRIAGE_QUICK_END_MS, failures: 3 }, event: ended(), expect: { state: "starting", commands: ["startTriage"], session: false, deadlineMs: null, failures: 0 } },
  { row: "starting triageEnded", when: "Before its start's result", given: { state: "starting", command: "startTriage" }, event: ended(), expect: { state: "starting", commands: [], ended: SESSION.terminal } },
  { row: "idle triageEnded", when: "Otherwise", given: { state: "running" }, event: ended("another"), expect: { unchanged: true } },
  { row: "starting listenConnected", when: "A command in flight, held away, that has had its three sends", given: { state: "starting", command: "startTriage", away: true, sends: SENDS }, event: connected, expect: { state: "retrying", commands: [], away: false, deadlineMs: NOW + MIN + TRIAGE_RETRY_FIRST_MS } },
  { row: "starting listenConnected", when: "A command in flight, held away", given: { state: "starting", command: "startTriage", away: true }, event: connected, expect: { state: "starting", commands: ["startTriage"], sentMs: NOW + MIN, sends: 2, away: false } },
  { row: "starting listenConnected", when: "A command in flight otherwise", given: { state: "starting", command: "startTriage" }, event: connected, expect: { unchanged: true } },
  { row: "retrying listenConnected", when: "—", given: { state: "retrying", asked: [A] }, event: connected, expect: { state: "starting", commands: ["startTriage"], deadlineMs: null } },
  { row: "idle listenConnected", when: "An issue asked", given: { asked: [A] }, event: connected, expect: { state: "starting", commands: ["startTriage"] } },
  { row: "idle listenConnected", when: "Otherwise", given: { state: "running" }, event: connected, expect: { unchanged: true } },
  { row: "starting listenGone", when: "A command in flight", given: { state: "starting", command: "startTriage" }, event: gone, expect: { state: "starting", commands: [], away: true } },
  { row: "idle listenGone", when: "Otherwise", given: { state: "retrying" }, event: gone, expect: { unchanged: true } },
  { row: "idle commandResult", when: "Not the command in flight, but the `startTriage` that last failed", given: { state: "retrying", asked: [A], lost: "triage.2" }, event: late(), expect: { state: "running", commands: [], session: true, deadlineMs: NOW + MIN + TRIAGE_OBSERVE_MS, asked: [A] } },
  { row: "idle commandResult", when: "Not the command in flight: stale", given: { state: "starting", command: "startTriage" }, event: result("ok", {}, "triage.2"), expect: { unchanged: true } },
  { row: "starting commandResult", when: "`startTriage` failed, `retryable`, `held` or `lost`, or ok with a session whose end was heard already, the 20th in a row", given: { state: "starting", command: "startTriage", failures: TRIAGE_RETRIES - 1, asked: [A] }, event: result("failed"), expect: { state: "idle", commands: [], deadlineMs: null, asked: [A] } },
  { row: "starting commandResult", when: "`startTriage` failed, `retryable`, `held` or `lost`, or ok with a session whose end was heard already", given: { state: "starting", command: "startTriage", sent: [A], asked: [A], ended: SESSION.terminal }, event: result("ok", { session: SESSION }), expect: { state: "retrying", commands: [], deadlineMs: NOW + MIN + TRIAGE_RETRY_FIRST_MS, asked: [A], session: false } },
  { row: "starting commandResult", when: "`startTriage` ok with a session", given: { state: "starting", command: "startTriage", sent: [A, B], asked: [A, B] }, event: result("ok", { session: SESSION, waiting: [B] }), expect: { state: "running", commands: [], asked: [B], session: true, deadlineMs: NOW + MIN + TRIAGE_OBSERVE_MS } },
  { row: "starting commandResult", when: "`startTriage` ok with none, and an issue was asked during it", given: { state: "starting", command: "startTriage", sent: [A], asked: [A, B] }, event: result("ok", {}), expect: { state: "starting", commands: ["startTriage"], asked: [B] } },
  { row: "starting commandResult", when: "`startTriage` ok with none", given: { state: "starting", command: "startTriage", sent: [A, B], asked: [A, B] }, event: result("ok", { waiting: [B] }), expect: { state: "idle", commands: [], asked: [B] } },
  { row: "running commandResult", when: "`observeTriage` ok, and the terminal is gone", given: { state: "running", command: "observeTriage", deadline: false }, event: result("ok", { open: false }), expect: { state: "starting", commands: ["startTriage"], session: false } },
  { row: "running commandResult", when: "`observeTriage` otherwise", given: { state: "running", command: "observeTriage", deadline: false }, event: result("ok", { open: true }), expect: { state: "running", commands: [], session: true, deadlineMs: NOW + MIN + TRIAGE_OBSERVE_MS } },
  { row: "idle deadline", when: "Not the one armed", given: { state: "running" }, event: deadline(4), expect: { unchanged: true } },
  { row: "running deadline", when: "The one armed", given: { state: "running" }, event: deadline(), expect: { state: "running", commands: ["observeTriage"], deadlineMs: null, session: true } },
  { row: "retrying deadline", when: "The one armed", given: { state: "retrying", asked: [A] }, event: deadline(), expect: { state: "starting", commands: ["startTriage"], deadlineMs: null } },
];

function check(c: Case): void {
  const before = given(c.given);
  // A minute after the command in flight was sent.
  const { triage, send } = stepTriage(before, c.event, NOW + MIN);
  const what = `${c.row} (${c.when})`;
  if (c.expect.unchanged) {
    assert.equal(triage, before, what);
    assert.deepEqual(send, [], what);
    return;
  }
  if (c.expect.state !== undefined) assert.equal(triage.state, c.expect.state, what);
  if (c.expect.commands !== undefined) assert.deepEqual(send.map((s) => s.kind), c.expect.commands, what);
  if (c.expect.deadlineMs !== undefined) assert.equal(triage.deadline?.dueMs ?? null, c.expect.deadlineMs, what);
  if (c.expect.asked !== undefined) assert.deepEqual(triage.asked, c.expect.asked, what);
  if (c.expect.session !== undefined) assert.equal(triage.session !== null, c.expect.session, what);
  if (c.expect.ended !== undefined) assert.deepEqual(triage.ended, [c.expect.ended], what);
  if (c.expect.sentMs !== undefined) assert.equal(triage.command?.sentMs, c.expect.sentMs, what);
  if (c.expect.sends !== undefined) assert.equal(triage.command?.sends, c.expect.sends, what);
  if (c.expect.failures !== undefined) assert.equal(triage.failures, c.expect.failures, what);
  if (c.expect.away !== undefined) assert.equal(triage.away, c.expect.away, what);
  // What it sends is the one command in flight, and every write that sends a new one or arms
  // moves the version; a command sent again moves none.
  if (send.length > 0) {
    assert.deepEqual(triage.command, send[0], what);
    if (send[0].id !== before.command?.id) assert.ok(triage.version > before.version, what);
  }
  if (triage.deadline !== null) assert.equal(triage.deadline.version, triage.version, what);
}

test("every triage transition in the doc has its case, in order", () => {
  const rows = rowsOf("## Triage transitions");
  assert.equal(cases.length, rows.length, "one case per row");
  rows.forEach((r, i) => {
    assert.equal(cases[i].row, r.row, `row ${i + 1}`);
    assert.ok(r.when.startsWith(cases[i].when), `row ${i + 1}: "${r.when}" starts "${cases[i].when}"`);
  });
});

for (const c of cases) test(`triage: ${c.row}, ${c.when}`, () => check(c));

test("a start carries every issue asked, and an ask already held while one is under way changes nothing", () => {
  const { triage, send } = stepTriage(given({ asked: [B] }), asked(A), NOW);
  assert.deepEqual(send[0].params, { issueIds: [B, A] });
  const again = stepTriage(triage, asked(A), NOW);
  assert.equal(again.triage, triage);
  // An idle machine still holding an issue, after its retries ran out, starts for it again.
  assert.deepEqual(stepTriage(given({ asked: [A] }), asked(A), NOW).send.map((c) => c.kind), ["startTriage"]);
});

test("the issues asked keep the newest MAX_ASKED", () => {
  let t = given({ state: "running" });
  for (let i = 0; i <= MAX_ASKED; i++) t = stepTriage(t, asked(`aaaaaaaa-aaaa-4aaa-8aaa-${String(i).padStart(12, "0")}`), NOW).triage;
  assert.equal(t.asked.length, MAX_ASKED);
  assert.ok(!t.asked.includes("aaaaaaaa-aaaa-4aaa-8aaa-000000000000"));
});

test("a failed start waits twice as long each time, to the last", () => {
  let t = given({ state: "starting", command: "startTriage" });
  const waits: number[] = [];
  for (let i = 0; i < 8; i++) {
    t = stepTriage(t, result("failed"), NOW).triage;
    waits.push(t.deadline!.dueMs - NOW);
    t = stepTriage(t, deadline(t.version), NOW).triage;
    t = { ...t, command: { ...t.command!, id: "triage.3" } };
  }
  assert.deepEqual(waits.slice(0, 3), [TRIAGE_RETRY_FIRST_MS, 2 * TRIAGE_RETRY_FIRST_MS, 4 * TRIAGE_RETRY_FIRST_MS]);
  assert.equal(waits.at(-1), TRIAGE_RETRY_LAST_MS);
  // A start that then ends ok counts from nothing again.
  assert.equal(stepTriage(t, result("ok", {}), NOW).triage.failures, 0);
});

test("a command with no result is sent again with its ID, then ended as lost; a deadline due fires", () => {
  let t = given({ state: "starting", command: "startTriage" });
  assert.equal(triageDue(t), NOW + START_TIMEOUT_MS);
  assert.equal(expireTriage(t, NOW + START_TIMEOUT_MS - 1).triage, t);
  let at = NOW;
  for (let sends = 2; sends <= SENDS; sends++) {
    at += START_TIMEOUT_MS;
    const { triage, send } = expireTriage(t, at);
    assert.deepEqual(send.map((c) => [c.id, c.sends]), [["triage.3", sends]]);
    t = triage;
  }
  at += START_TIMEOUT_MS;
  t = expireTriage(t, at).triage;
  assert.equal(t.state, "retrying");
  assert.equal(t.command, null);
  assert.equal(triageDue(t), at + TRIAGE_RETRY_FIRST_MS);
  const fired = expireTriage(t, at + TRIAGE_RETRY_FIRST_MS);
  assert.deepEqual(fired.send.map((c) => c.kind), ["startTriage"]);
  // observeTriage times out sooner.
  const observing = stepTriage(given({ state: "running" }), deadline(), NOW).triage;
  assert.equal(triageDue(observing), NOW + COMMAND_TIMEOUT_MS);
  assert.equal(triageDue(newTriage()), null);
});

test("only the running session holds the issues it was given", () => {
  assert.equal(triaging(given({ state: "running" }), SESSION.id, "TW-1"), true);
  assert.equal(triaging(given({ state: "running" }), SESSION.id, "TW-3"), false);
  assert.equal(triaging(given({ state: "running" }), "another", "TW-1"), false);
  assert.equal(triaging(given({ state: "starting", session: true }), SESSION.id, "TW-1"), false);
  assert.equal(triaging(undefined, SESSION.id, "TW-1"), false);
});

test("the triage machine's events from outside are rebuilt from their checked fields, or refused", () => {
  assert.deepEqual(parseTriageEvent({ id: "d-1", kind: "triageAsked", issue: A, extra: "x" }), { id: "d-1", kind: "triageAsked", issue: A });
  assert.deepEqual(parseTriageEvent({ id: "result:triage.1", kind: "commandResult", command: "triage.1", result: "ok", data: contract.reports.find((r) => r.id === "triage.1")!.detail }), { id: "result:triage.1", kind: "commandResult", command: "triage.1", result: "ok", data: { session: { id: "s-3", terminal: "term_2", issues: ["TW-1", "TW-2"] }, waiting: ["0a1b2c3d-0000-4000-8000-000000000002"] } });
  for (const bad of [
    { id: "d-1", kind: "triageAsked", issue: "TW-1" },
    { id: "d-1", kind: "triageEnded", terminal: "term 1" },
    { id: "d-1", kind: "commandResult", command: `${A}.1`, result: "ok" },
    { id: "d-1", kind: "commandResult", command: "triage.1", result: "lost" },
    { id: "d-1", kind: "commandResult", command: "triage.1", result: "ok", data: { session: { ...SESSION, issues: ["tw-1"] } } },
    { id: "d-1", kind: "deadline", version: 1 },
  ]) {
    assert.throws(() => parseTriageEvent(bad), Invalid, JSON.stringify(bad));
  }
});

test("its commands go on the wire as the contract's", () => {
  const start = stepTriage(given({ asked: [] }), asked("0a1b-2c"), NOW).send[0];
  assert.deepEqual(triageWire({ ...start, id: "triage.1", version: 1 }), contract.commands.find((c) => c.kind === "startTriage"));
  const observe: Command = { ...start, id: "triage.2", kind: "observeTriage", version: 2, params: { session: "s-3", terminal: "term_2" } };
  assert.deepEqual(triageWire(observe), contract.commands.find((c) => c.kind === "observeTriage"));
});

test("the store keeps the machine, sends its command, resends it to a new connection and fires it on the alarm", async () => {
  const storage = new MemoryStorage();
  const sent: Command[] = [];
  const sender = (c: Command[]) => sent.push(...c);
  await triageEvent(storage, asked(), NOW, sender);
  assert.deepEqual(sent.map((c) => c.kind), ["startTriage"]);
  assert.equal((await loadTriage(storage)).rev, 1);
  assert.deepEqual((await inFlight(storage)).map((c) => c.id), ["triage.1"]);
  // A replayed ask while it starts changes nothing, and writes nothing.
  await triageEvent(storage, asked(), NOW, sender);
  assert.equal((await loadTriage(storage)).rev, 1);
  await triageEvent(storage, result("ok", { session: SESSION }, "triage.1"), NOW, sender);
  assert.equal((await loadTriage(storage)).state, "running");
  assert.deepEqual(await inFlight(storage), []);
  // The alarm reads Orca again at the deadline, and the closed terminal starts the next batch.
  sent.length = 0;
  await reconcile(storage, NOW + TRIAGE_OBSERVE_MS, sender, true);
  assert.deepEqual(sent.map((c) => c.kind), ["observeTriage"]);
  await triageEvent(storage, result("ok", { open: false }, sent[0].id), NOW + TRIAGE_OBSERVE_MS + MIN, sender);
  assert.deepEqual(sent.map((c) => c.kind), ["observeTriage", "startTriage"]);
  assert.equal((await loadTriage(storage)).state, "starting");
});

test("a late result is held only from the start that last failed, and only once", () => {
  const failedLost = stepTriage(given({ state: "starting", command: "startTriage" }), result("lost"), NOW).triage;
  assert.equal(failedLost.lost, "triage.3");
  const held = stepTriage(failedLost, result("ok", { session: SESSION }), NOW);
  assert.equal(held.triage.state, "running");
  // A duplicate of an older start's result, or of this one once a later start was sent, holds nothing.
  assert.equal(stepTriage(failedLost, result("ok", { session: SESSION }, "triage.1"), NOW).triage, failedLost);
  const restarted = stepTriage(failedLost, deadline(failedLost.version), NOW).triage;
  const idle = stepTriage(restarted, result("ok", {}, restarted.command!.id), NOW).triage;
  assert.equal(idle.state, "idle");
  assert.equal(stepTriage(idle, result("ok", { session: SESSION }), NOW).triage, idle);
});

test("with nothing listening, no command times out; a deadline still fires", () => {
  const t = stepTriage(given({ state: "starting", command: "startTriage" }), gone, NOW).triage;
  assert.equal(t.away, true);
  assert.equal(triageDue(t), null);
  assert.equal(expireTriage(t, NOW + 10 * START_TIMEOUT_MS).triage, t);
  // Gone with nothing in flight holds nothing, and changes nothing.
  const retrying = given({ state: "retrying" });
  assert.equal(stepTriage(retrying, gone, NOW).triage, retrying);
  assert.equal(triageDue(retrying), NOW);
  assert.deepEqual(expireTriage(retrying, NOW).send.map((c) => c.kind), ["startTriage"]);
  // The connection gives the start its timeouts afresh.
  const back = stepTriage(t, connected, NOW + HOUR).triage;
  assert.deepEqual([back.away, back.command?.sentMs, triageDue(back)], [false, NOW + HOUR, NOW + HOUR + START_TIMEOUT_MS]);
  // A start that ends while away no longer holds `listen` away.
  assert.equal(stepTriage(t, result("failed"), NOW).triage.away, false);
});

test("another terminal closing before a start's result doesn't hide the session's, and only the newest few are kept", () => {
  let t = given({ state: "starting", command: "startTriage", sent: [A], asked: [A] });
  for (const terminal of [SESSION.terminal, "term_other"]) t = stepTriage(t, ended(terminal), NOW).triage;
  assert.deepEqual(t.ended, [SESSION.terminal, "term_other"]);
  const done = stepTriage(t, result("ok", { session: SESSION }), NOW);
  assert.equal(done.triage.state, "retrying");
  for (let i = 0; i < 20; i++) t = stepTriage(t, ended(`term_${i}`), NOW).triage;
  assert.equal(t.ended.length, 16);
});
