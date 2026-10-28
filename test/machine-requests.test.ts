import assert from "node:assert/strict";
import { test } from "node:test";
import { retired } from "../src/worker/machine/ctx.ts";
import { childrenFinished, pending } from "../src/worker/machine/chain.ts";
import { mergeReadOnly } from "../src/worker/machine/facts.ts";
import type { IssueRead } from "../src/worker/machine/types.ts";
import type { Event, Store } from "../src/worker/machine/types.ts";
import { stateType, toWire } from "../src/worker/route.ts";
import { apply, B, B2, check, deadline, ev, given, HOUR, ISSUE, landRead, MIN, NOW, P, result, rowsOf, S, S2, T0, T1, T2, type Case } from "./machine-fixture.ts";

const C3 = "abababab-abab-4bab-8bab-abababababab";

// docs/state-machine.md's request transitions, one case per row, in the doc's order: each
// builds a store where the row's condition holds, applies its event and checks its new
// state, commands and deadline. The first test fails when a row is added, moved or
// removed without its case.

const readIssue = { kind: "readIssue" as const, machine: S, params: { for: T0 } };
const read = (over: object = {}) => ({
  read: { updatedAt: T1, delegate: "owner", state: "open", stateChangedAt: T0, priority: 0, blockers: [], moreBlockers: false, parent: null, children: [], moreChildren: false, repo: "repo", sessions: [{ id: S, kind: "delegation", createdAt: T0 }], ...over },
});
const turn = { terminal: "term_1", sinceMs: NOW - MIN, for: T0, idleMs: null, closingMs: null };
// A warm turn handed the request at T1.
const handOver = { kind: "startTurn" as const, machine: S, params: { request: T1, warm: true } };
// The stop owed, sent a minute ago as the store's second command.
const owed = { command: `${ISSUE}.2`, sentMs: NOW - MIN, since: NOW - MIN };
// A parent mid-orchestration: its worker sent to chain B, which its chain took, and B2 added
// since, which none of them knows.
const orchestrating = { orchestrated: { at: T0, known: [B] }, answeredThrough: T1 };
const chainTook = { run: "r-1", issues: [B], asked: [B], answered: [B], haltedAt: null };
const withAdded = { children: { sentMs: NOW - HOUR, ids: [B, B2], states: { [B]: { done: false, at: T0 }, [B2]: { done: false, at: T1 } }, more: false }, chained: chainTook };

export const cases: Case[] = [
  { row: "requested sessionStarted", when: "A redelivery", given: { req: "requested" }, event: ev("sessionStarted", { session: S, sessionKind: "delegation", byOwner: true, at: T0 }), expect: { req: "requested", commands: [] } },
  { row: "requested ownerPrompted", when: "Not newer", given: { req: "requested" }, event: ev("ownerPrompted", { session: S, byOwner: true, at: T0 }), expect: { req: "requested", commands: [] } },
  { row: "requested ownerPrompted", when: "Newer", given: { req: "claimed" }, event: ev("ownerPrompted", { session: S, byOwner: true, at: T1 }), expect: { req: "requested", commands: ["readIssue"], reqOwn: 15 * MIN } },
  { row: "requested ownerStopped", when: "Not newer", given: { req: "requested" }, event: ev("ownerStopped", { session: S, byOwner: true, at: T0 }), expect: { req: "requested", commands: [] } },
  { row: "stopping ownerStopped", when: "A second stop", given: { req: "stopping", sess: { turn } }, event: ev("ownerStopped", { session: S, byOwner: true, at: T1 }), expect: { req: "stopping", commands: [] } },
  { row: "requested ownerStopped", when: "A turn runs", given: { req: "claimed", sess: { turn } }, event: ev("ownerStopped", { session: S, byOwner: true, at: T1 }), expect: { req: "stopping", commands: ["stopTurn"], reqOwn: 2 * MIN } },
  { row: "requested ownerStopped", when: "The issue's worker runs", given: { req: "claimed", work: "running" }, event: ev("ownerStopped", { session: S, byOwner: true, at: T1 }), expect: { req: "done", work: "halted", commands: ["stopWorker"], reqOwn: null } },
  { row: "requested ownerStopped", when: "Otherwise", given: { req: "done" }, event: ev("ownerStopped", { session: S, byOwner: true, at: T1 }), expect: { req: "done", commands: ["postNotice:stopped"], reqOwn: null } },
  { row: "queued delegateChanged", when: "Taken from Tolkster", given: { req: "queued" }, event: ev("delegateChanged", { by: null, at: T1 }), expect: { req: "done", commands: ["postNotice:noLongerMine"], reqOwn: null } },
  { row: "requested delegateChanged", when: "Delegated to Tolkster by the owner or a chain", given: { req: "requested", sess: { unattributed: { for: T0, sinceMs: NOW } }, facts: { delegate: { value: "unknown", at: T1 } } }, event: ev("delegateChanged", { by: "owner", at: T1 }), expect: { req: "requested", commands: ["readIssue"], reqOwn: 15 * MIN } },
  { row: "requested delegateChanged", when: "Otherwise", given: { req: "requested" }, event: ev("delegateChanged", { by: null, at: T1 }), expect: { req: "requested", commands: [] } },
  { row: "done delegateChanged", when: "Delegated to Tolkster", given: { req: "done", facts: { delegate: { value: null, at: T0 } } }, event: ev("delegateChanged", { by: "owner", at: T1 }), expect: { req: "requested", commands: ["readIssue"], reqOwn: 15 * MIN } },
  { row: "done delegateChanged", when: "Otherwise", given: { req: "done" }, event: ev("delegateChanged", { by: null, at: T1 }), expect: { req: "done", commands: [] } },
  { row: "claimed delegateChanged", when: "Delegated to Tolkster, newer, mid-turn", given: { req: "claimed", kind: "mention", sess: { turn }, facts: { delegate: { value: null, at: T0 } } }, event: ev("delegateChanged", { by: "owner", at: T1 }), expect: { req: "claimed", commands: [] } },
  { row: "claimed delegateChanged", when: "Delegated to Tolkster, newer, a worker's", given: { req: "claimed", work: "running", facts: { delegate: { value: "owner", at: T0 } } }, event: ev("delegateChanged", { by: "owner", at: T1 }), expect: { req: "requested", commands: ["readIssue"], reqOwn: 15 * MIN } },
  { row: "claimed delegateChanged", when: "The work machine halts", given: { req: "claimed" }, event: ev("delegateChanged", { by: null, at: T1 }), expect: { req: "claimed", commands: [] } },
  { row: "queued issueStateChanged", when: "Completed or cancelled", given: { req: "queued", sess: { waiting: "blockers" } }, event: ev("issueStateChanged", { state: "completed", parent: null, at: T1 }), expect: { req: "done", commands: ["postNotice:alreadyDone"], reqOwn: null } },
  { row: "requested issueStateChanged", when: "Otherwise", given: { req: "requested" }, event: ev("issueStateChanged", { state: "completed", parent: null, at: T1 }), expect: { req: "requested", commands: [] } },
  { row: "claimed issueStateChanged", when: "The work machine acts", given: { req: "claimed" }, event: ev("issueStateChanged", { state: "completed", parent: null, at: T1 }), expect: { req: "claimed", commands: [] } },
  { row: "queued blockersChanged", when: "Waiting on blockers, every", given: { req: "queued", sess: { waiting: "blockers" }, facts: { blockers: { more: false, sentMs: NOW - HOUR, ids: [B], states: { [B]: { done: false, at: T0 } } } } }, event: ev("blockersChanged", { blocker: B, done: true, at: T1 }), expect: { req: "requested", commands: ["readIssue"], reqOwn: 15 * MIN } },
  { row: "queued blockersChanged", when: "Waiting on blockers, a different", given: { req: "queued", sess: { waiting: "blockers" }, facts: { blockers: { more: false, sentMs: NOW - HOUR, ids: [B, B2], states: { [B]: { done: false, at: T0 }, [B2]: { done: false, at: T0 } } }, notices: [`blocked:${S}:${T0}:${[B, B2].sort().join(",")}`] } }, event: ev("blockersChanged", { blocker: B, done: true, at: T1 }), expect: { req: "queued", commands: ["postNotice:blocked"] } },
  { row: "queued blockersChanged", when: "Waiting on blockers, the same", given: { req: "queued", sess: { waiting: "blockers" }, facts: { blockers: { more: false, sentMs: NOW - HOUR, ids: [B, B2], states: { [B]: { done: false, at: T0 }, [B2]: { done: false, at: T0 } } }, notices: [`blocked:${S}:${T0}:${[B, B2].sort().join(",")}`] } }, event: ev("blockersChanged", { blocker: B, done: false, at: T1 }), expect: { req: "queued", commands: [] } },
  { row: "requested blockersChanged", when: "Otherwise: blockers", given: { req: "queued", facts: { blockers: { more: false, sentMs: NOW - HOUR, ids: [B], states: { [B]: { done: false, at: T0 } } } } }, event: ev("blockersChanged", { blocker: B, done: true, at: T1 }), expect: { req: "queued", commands: [] } },
  { row: "requested turnEnded", when: "From a terminal a newer turn replaced", given: { req: "claimed", sess: { turn: { ...turn, terminal: "term_2" } } }, event: ev("turnEnded", { session: S, terminal: "term_1" }), expect: { req: "claimed", commands: [] } },
  { row: "claimed turnEnded", when: "A kept delegation another session asks waits", given: { req: "claimed", kind: "mention", sess: { turn: { ...turn, terminal: null }, request: { kind: "delegation", at: T1 } }, other: "done", facts: { sessions: { [S]: { kind: "mention", createdAt: T0 }, [S2]: { kind: "delegation", createdAt: T0, started: true } } } }, event: ev("turnEnded", { session: S }), expect: { req: "requested", commands: ["postNotice:stalled", "readIssue"], reqOwn: 15 * MIN } },
  { row: "claimed turnEnded", when: "A request newer", given: { req: "claimed", sess: { turn, request: { kind: "prompt", at: T1 } } }, event: ev("turnEnded", { session: S }), expect: { req: "requested", commands: ["readIssue"], reqOwn: 15 * MIN } },
  { row: "claimed turnEnded", when: "A note's thread's turn that ran", given: { req: "claimed", kind: "note", sess: { turn, answeredThrough: T1 } }, event: ev("turnEnded", { session: S }), expect: { req: "requested", commands: ["readIssue"], reqOwn: 15 * MIN } },
  { row: "claimed turnEnded", when: "The turn answered", given: { req: "claimed", sess: { turn, answeredThrough: T1 } }, event: ev("turnEnded", { session: S }), expect: { req: "done", commands: [] } },
  { row: "claimed turnEnded", when: "The turn ended without", given: { req: "claimed", sess: { turn } }, event: ev("turnEnded", { session: S }), expect: { req: "done", commands: ["postNotice:stalled"] } },
  { row: "stopping turnEnded", when: "The turn being stopped", given: { req: "stopping", sess: { turn }, reqStop: owed }, event: ev("turnEnded", { session: S }), expect: { req: "done", commands: ["postNotice:stopped"] } },
  { row: "stopping turnEnded", when: "Received before", given: { req: "stopping", sess: { turn }, reqStop: { ...owed, sentMs: NOW + MIN } }, event: ev("turnEnded", { session: S }), expect: { req: "stopping", commands: [] } },
  { row: "requested turnEnded", when: "No turn holds", given: { req: "requested" }, event: ev("turnEnded", { session: S }), expect: { req: "requested", commands: [] } },
  { row: "claimed turnIdle", when: "A working turn answered the request it was given, and a newer", given: { req: "claimed", sess: { turn, answeredThrough: T1, request: { kind: "prompt", at: T2 } } }, event: ev("turnIdle", { session: S, terminal: "term_1", at: T2 }), expect: { req: "requested", commands: ["readIssue"], warm: 5 * MIN } },
  { row: "requested turnIdle", when: "A working turn answered the request it was given: warm", given: { req: "done", sess: { turn, answeredThrough: T1 } }, event: ev("turnIdle", { session: S, terminal: "term_1", at: T1 }), expect: { req: "done", commands: [], warm: 5 * MIN } },
  { row: "requested turnIdle", when: "A working turn stopped short", given: { req: "claimed", sess: { turn } }, event: ev("turnIdle", { session: S, terminal: "term_1", at: T1 }), expect: { req: "claimed", commands: ["stopTurn"], warm: 2 * MIN } },
  { row: "requested turnIdle", when: "Otherwise", given: { req: "done", sess: { turn: { ...turn, idleMs: NOW - MIN }, answeredThrough: T1 } }, event: ev("turnIdle", { session: S, terminal: "term_1", at: T1 }), expect: { req: "done", commands: [], warm: 4 * MIN } },
  { row: "queued workerRead", when: "Waiting on blockers", given: { req: "queued", sess: { waiting: "blockers" } }, event: ev("workerRead", { session: S, reader: "worker", through: T0 }), expect: { req: "queued", commands: [] } },
  { row: "requested workerRead", when: "By the issue's worker", given: { req: "queued", sess: { waiting: "held" }, work: "held" }, event: ev("workerRead", { session: S, reader: "worker", through: T0 }), expect: { req: "claimed", reqOwn: HOUR } },
  { row: "requested workerRead", when: "Otherwise", given: { req: "requested", sess: { request: { kind: "prompt", at: T1 } } }, event: ev("workerRead", { session: S, reader: "worker", through: T0 }), expect: { req: "requested", commands: [] } },
  { row: "claimed workerRead", when: "The read is a fact", given: { req: "done" }, event: ev("workerRead", { session: S, reader: "worker", through: T0 }), expect: { req: "done", commands: [] } },
  { row: "claimed workerAnswered", when: "After the request", given: { req: "claimed", sess: { readThrough: T0 } }, event: ev("workerAnswered", { session: S, reader: "worker", at: T1 }), expect: { req: "done", commands: [] } },
  { row: "claimed workerAnswered", when: "Otherwise", given: { req: "claimed" }, event: ev("workerAnswered", { session: S, reader: "worker", at: T1 }), expect: { req: "claimed", commands: [] } },
  { row: "requested workerAnswered", when: "An answer handles", given: { req: "queued", sess: { readThrough: T0 } }, event: ev("workerAnswered", { session: S, reader: "worker", at: T1 }), expect: { req: "queued", commands: [] } },
  { row: "queued workChanged", when: "Its issue's worker left", given: { req: "queued", sess: { waiting: "held" }, work: "running" }, event: ev("workChanged", { from: "held", to: "running", version: 2, noticed: null }), expect: { req: "requested", commands: ["readIssue"] } },
  { row: "queued workChanged", when: "Waiting for a slot", given: { req: "queued", sess: { waiting: "slot" }, work: "launching", w: { requestedBy: { session: S2, at: T0 } } }, event: ev("workChanged", { from: "exited", to: "launching", version: 2, noticed: null }), expect: { req: "requested", commands: ["readIssue"] } },
  { row: "queued workChanged", when: "Otherwise", given: { req: "queued", sess: { waiting: "held" }, work: "idle" }, event: ev("workChanged", { from: "running", to: "idle", version: 2, noticed: null }), expect: { req: "queued", commands: [] } },
  { row: "claimed workChanged", when: "The work left the slot-taking states, this session's newest request is answered, its", given: { req: "claimed", sess: { answeredThrough: T1, orchestrated: { at: T0, known: [] } }, work: "rest", facts: { children: { sentMs: NOW - HOUR, ids: [B], states: { [B]: { done: true, at: T1 } }, more: false } } }, event: ev("workChanged", { from: "running", to: "rest", version: 2, noticed: null }), expect: { req: "requested", commands: ["readIssue"] } },
  { row: "claimed workChanged", when: "The work left the slot-taking states, this session's request is answered, and a sub-issue was added", given: { req: "claimed", sess: orchestrating, work: "rest", facts: withAdded }, event: ev("workChanged", { from: "running", to: "rest", version: 2, noticed: null }), expect: { req: "requested", commands: ["readIssue"] } },
  { row: "claimed workChanged", when: "The work left the slot-taking states, and", given: { req: "claimed", sess: { answeredThrough: T1 }, work: "rest" }, event: ev("workChanged", { from: "running", to: "rest", version: 2, noticed: null }), expect: { req: "done", commands: [] } },
  { row: "claimed workChanged", when: "The work left them unanswered, and its own notice", given: { req: "claimed", work: "exited" }, event: ev("workChanged", { from: "launching", to: "exited", version: 2, noticed: S }), expect: { req: "done", commands: [] } },
  { row: "claimed workChanged", when: "The work left them for `exited` unanswered, after this request's line typed nothing", given: { req: "claimed", sess: { undelivered: T0 }, work: "exited" }, event: ev("workChanged", { from: "running", to: "exited", version: 2, noticed: null }), expect: { req: "requested", commands: ["readIssue"] } },
  { row: "claimed workChanged", when: "The work left them unanswered", given: { req: "claimed", work: "exited" }, event: ev("workChanged", { from: "running", to: "exited", version: 2, noticed: null }), expect: { req: "done", commands: ["postNotice:stalled"] } },
  { row: "claimed workChanged", when: "Otherwise", given: { req: "claimed", work: "idle" }, event: ev("workChanged", { from: "running", to: "idle", version: 2, noticed: null }), expect: { req: "claimed", commands: [] } },
  { row: "done workChanged", when: "The work left the slot-taking states, and the sub-issues' end is owed, or else a sub-issue was added", given: { req: "done", sess: orchestrating, work: "rest", facts: withAdded }, event: ev("workChanged", { from: "running", to: "rest", version: 2, noticed: null }), expect: { req: "requested", commands: ["readIssue"] } },
  { row: "requested workChanged", when: "Otherwise", given: { req: "requested", work: "exited" }, event: ev("workChanged", { from: "running", to: "exited", version: 2, noticed: null }), expect: { req: "requested", commands: [] } },
  { row: "queued slotGranted", when: "For this request, waiting for a slot, granted in the step", given: { req: "queued", sess: { waiting: "slot", asked: "slotAsked:x:1" } }, event: ev("slotGranted", { session: S, answers: "slotAsked:x:1" }), expect: { req: "claimed", work: "launching", commands: ["startWorker"], out: ["slotHeld"], reqOwn: HOUR } },
  { row: "queued slotGranted", when: "For this request, waiting for a slot, granted later", given: { req: "queued", sess: { waiting: "slot" } }, event: ev("slotGranted", { session: S, answers: "slotReleased:y:3" }), expect: { req: "requested", commands: ["readIssue"], reqOwn: 15 * MIN } },
  { row: "requested slotGranted", when: "For this request", given: { req: "requested", sess: { waiting: "slot" } }, event: ev("slotGranted", { session: S, answers: "slotReleased:y:3" }), expect: { req: "requested", commands: [], out: [] } },
  { row: "requested slotGranted", when: "Otherwise", given: { req: "done" }, event: ev("slotGranted", { session: S, answers: "slotReleased:y:3" }), expect: { req: "done", out: ["slotReleased"] } },
  { row: "queued slotWaiting", when: "For this request", given: { req: "queued", sess: { waiting: "slot" } }, event: ev("slotWaiting", { session: S }), expect: { req: "queued", commands: ["postNotice:busy"] } },
  { row: "requested slotWaiting", when: "Otherwise", given: { req: "done" }, event: ev("slotWaiting", { session: S }), expect: { req: "done", commands: [] } },
  { row: "queued priorityChanged", when: "Waiting for a slot", given: { req: "queued", sess: { waiting: "slot" } }, event: ev("priorityChanged", { priority: 1, at: T1 }), expect: { req: "queued", commands: [], out: ["slotAsked"] } },
  { row: "requested priorityChanged", when: "Otherwise", given: { req: "queued", sess: { waiting: "blockers" } }, event: ev("priorityChanged", { priority: 1, at: T1 }), expect: { req: "queued", commands: [], out: [] } },
  { row: "done childChanged", when: "Every sub-issue", given: { req: "done", sess: { orchestrated: { at: T0, known: [] }, answeredThrough: T1 }, facts: { children: { sentMs: NOW - HOUR, ids: [B], states: { [B]: { done: false, at: T0 } }, more: false } } }, event: ev("childChanged", { child: B, done: true, at: T1 }), expect: { req: "requested", commands: ["readIssue"] } },
  { row: "done childChanged", when: "A sub-issue was added", given: { req: "done", sess: orchestrating, work: "rest", facts: { children: { sentMs: NOW - HOUR, ids: [B], states: { [B]: { done: false, at: T0 } }, more: false }, chained: chainTook } }, event: ev("childChanged", { child: B2, done: false, triage: false, at: T1 }), expect: { req: "requested", commands: ["readIssue"] } },
  { row: "requested childChanged", when: "Otherwise", given: { req: "done", facts: { children: { sentMs: NOW - HOUR, ids: [B], states: { [B]: { done: false, at: T0 } }, more: false } } }, event: ev("childChanged", { child: B, done: true, at: T1 }), expect: { req: "done", commands: [] } },
  { row: "requested parentHalted", when: "The session is a chain's", given: { req: "requested", kind: "chain", sess: { byOwner: false } }, event: ev("parentHalted", { parent: P, at: T1 }), expect: { req: "done", commands: [] } },
  { row: "requested parentHalted", when: "Otherwise", given: { req: "requested" }, event: ev("parentHalted", { parent: P, at: T1 }), expect: { req: "requested", commands: [] } },
  { row: "requested deadline", when: "`quiet`", given: { req: "claimed", away: true, queue: [readIssue], armed: [{ machine: "request", kind: "quiet" }] }, event: (s) => deadline(s, "request", "quiet"), expect: { req: "claimed", commands: ["postAway"], quiet: null } },
  { row: "requested deadline", when: "`warm`", given: { req: "done", sess: { turn: { ...turn, idleMs: NOW - 5 * MIN }, answeredThrough: T1 }, armed: [{ machine: "request", kind: "warm", dueMs: NOW }] }, event: (s) => deadline(s, "request", "warm"), expect: { req: "done", commands: ["stopTurn"], warm: 2 * MIN } },
  { row: "requested deadline", when: "The read had no result, and a slot", given: { req: "requested", sess: { grant: { sinceMs: NOW - 31 * MIN }, waiting: "slot" }, armed: [{ machine: "request", kind: "own" }] }, event: (s) => deadline(s, "request"), expect: { req: "requested", commands: ["readIssue"], out: ["slotReleased"], reqOwn: 15 * MIN } },
  { row: "requested deadline", when: "The read had no result", given: { req: "requested", armed: [{ machine: "request", kind: "own" }] }, event: (s) => deadline(s, "request"), expect: { req: "requested", commands: ["readIssue"], out: [], reqOwn: 15 * MIN } },
  { row: "queued deadline", when: "Waiting on blockers", given: { req: "queued", sess: { waiting: "blockers" }, armed: [{ machine: "request", kind: "own" }] }, event: (s) => deadline(s, "request"), expect: { req: "queued", commands: ["readIssue"], reqOwn: 5 * MIN } },
  { row: "queued deadline", when: "Waiting for a slot", given: { req: "queued", sess: { waiting: "slot" }, armed: [{ machine: "request", kind: "own" }] }, event: (s) => deadline(s, "request"), expect: { req: "queued", out: ["slotAsked"], reqOwn: 5 * MIN } },
  { row: "queued deadline", when: "Otherwise", given: { req: "queued", sess: { waiting: "held" }, armed: [{ machine: "request", kind: "own" }] }, event: (s) => deadline(s, "request"), expect: { req: "queued", commands: [], out: [], reqOwn: 5 * MIN } },
  { row: "claimed deadline", when: "The worker's or turn's", given: { req: "claimed", armed: [{ machine: "request", kind: "own" }] }, event: (s) => deadline(s, "request"), expect: { req: "claimed", commands: ["observe"], reqOwn: HOUR } },
  { row: "stopping deadline", when: "The stop is owed", given: { req: "stopping", sess: { turn }, reqStop: owed, armed: [{ machine: "request", kind: "own" }] }, event: (s) => deadline(s, "request"), expect: { req: "stopping", commands: ["stopTurn"], reqOwn: 2 * MIN } },
  { row: "done deadline", when: "At rest", given: { req: "done" }, event: (s) => deadline(s, "request"), expect: { req: "done", commands: [] } },
  { row: "requested commandResult", when: "`readIssue` succeeded", given: { req: "requested", queue: [readIssue] }, event: result(`${ISSUE}.1`, "ok", read()), expect: { req: "queued", out: ["slotAsked"], reqOwn: 5 * MIN } },
  { row: "queued commandResult", when: "`readIssue` succeeded", given: { req: "queued", sess: { waiting: "blockers" }, queue: [readIssue], facts: { blockers: { more: false, sentMs: NOW - HOUR, ids: [B], states: { [B]: { done: false, at: T0 } } } } }, event: result(`${ISSUE}.1`, "ok", read({ blockers: [{ id: B, done: true, updatedAt: T2 }] })), expect: { req: "requested", commands: ["readIssue"] } },
  { row: "requested commandResult", when: "A warm turn's `startTurn` succeeded", given: { req: "claimed", sess: { turn, request: { kind: "prompt", at: T1 } }, queue: [handOver] }, event: result(`${ISSUE}.1`, "ok", { terminal: "term_1" }), expect: { req: "claimed", commands: [], warm: null } },
  { row: "requested commandResult", when: "A warm turn's `startTurn` failed or was `lost`", given: { req: "claimed", sess: { turn, request: { kind: "prompt", at: T1 } }, queue: [handOver] }, event: result(`${ISSUE}.1`, "lost"), expect: { req: "claimed", commands: ["stopTurn"], warm: 2 * MIN } },
  { row: "claimed commandResult", when: "`startTurn` failed", given: { req: "claimed", kind: "mention", sess: { turn }, queue: [{ kind: "startTurn", machine: S }] }, event: result(`${ISSUE}.1`, "failed"), expect: { req: "done", commands: ["postNotice:couldntPass"] } },
  { row: "stopping commandResult", when: "The stop owed succeeded", given: { req: "stopping", sess: { turn }, reqStop: owed, queue: [{ kind: "startTurn", machine: S }, { kind: "stopTurn", machine: S }] }, event: result(`${ISSUE}.2`, "ok"), expect: { req: "done", commands: ["postNotice:stopped"] } },
  { row: "stopping commandResult", when: "`startTurn` opened a turn", given: { req: "stopping", sess: { turn }, reqStop: owed, queue: [{ kind: "startTurn", machine: S }, { kind: "stopTurn", machine: S }] }, event: result(`${ISSUE}.1`, "ok", { terminal: "term_2" }), expect: { req: "stopping", commands: ["stopTurn"], reqOwn: 2 * MIN } },
  { row: "requested commandResult", when: "`startTurn` opened a turn or was `lost`, with no turn held", given: { req: "done", sess: { turn: null }, queue: [{ kind: "startTurn", machine: S }] }, event: result(`${ISSUE}.1`, "ok", { terminal: "term_2" }), expect: { req: "done", commands: ["stopTurn"] } },
  { row: "requested commandResult", when: "Otherwise", given: { req: "requested", queue: [readIssue] }, event: result(`${ISSUE}.1`, "failed"), expect: { req: "requested", commands: [] } },
  { row: "requested listenGone", when: "A command in flight", given: { req: "requested", queue: [readIssue] }, event: ev("listenGone"), expect: { req: "requested", commands: [], quiet: 15 * MIN } },
  { row: "requested listenGone", when: "Otherwise", given: { req: "requested" }, event: ev("listenGone"), expect: { req: "requested", commands: [], quiet: null } },
  { row: "requested listenConnected", when: "—", given: { req: "claimed", away: true, queue: [readIssue], armed: [{ machine: "request", kind: "quiet", dueMs: NOW + MIN }] }, event: ev("listenConnected"), expect: { req: "claimed", commands: [], quiet: null } },
];

// The stop's other results, beyond the doc's one case per row.
const stops: Case[] = [
  { row: "stopping commandResult", when: "a lost startTurn may have opened its turn", given: { req: "stopping", sess: { turn }, reqStop: owed, queue: [{ kind: "startTurn", machine: S }, { kind: "stopTurn", machine: S }] }, event: result(`${ISSUE}.1`, "lost"), expect: { req: "stopping", commands: ["stopTurn"], reqOwn: 2 * MIN } },
  { row: "stopping commandResult", when: "a startTurn that opened nothing sends no stop", given: { req: "stopping", sess: { turn }, reqStop: owed, queue: [{ kind: "startTurn", machine: S }, { kind: "stopTurn", machine: S }] }, event: result(`${ISSUE}.1`, "ok"), expect: { req: "stopping", commands: [] } },
  { row: "stopping commandResult", when: "an older stop's success confirms nothing", given: { req: "stopping", sess: { turn }, reqStop: owed, queue: [{ kind: "stopTurn", machine: S }, { kind: "stopTurn", machine: S }] }, event: result(`${ISSUE}.1`, "ok"), expect: { req: "stopping", commands: [] } },
  { row: "stopping commandResult", when: "a startTurn that failed opened nothing", given: { req: "stopping", sess: { turn }, reqStop: owed, queue: [{ kind: "startTurn", machine: S }, { kind: "stopTurn", machine: S }] }, event: result(`${ISSUE}.1`, "failed"), expect: { req: "stopping", commands: [] } },
];
for (const c of stops) test(`request: ${c.row}, ${c.when}`, () => check(c));

test("request: claimed commandResult, `startTurn` failed and `listen` said so: no notice", () => check({ row: "claimed commandResult", when: "`startTurn` failed, said", given: { req: "claimed", kind: "mention", sess: { turn }, queue: [{ kind: "startTurn", machine: S }] }, event: result(`${ISSUE}.1`, "failed", { said: true }), expect: { req: "done", commands: [] } }));

test("request: a queued request on an issue that moves to a duplicate state ends with the cancelled notice", () => check({ row: "queued issueStateChanged", when: "Completed or cancelled", given: { req: "queued", sess: { waiting: "blockers" } }, event: ev("issueStateChanged", { state: stateType("duplicate"), parent: null, at: T1 }), expect: { req: "done", commands: ["postNotice:cancelled"], reqOwn: null } }));

test("every request transition in the doc has its case, in order", () => {
  const rows = rowsOf("## Request transitions");
  assert.equal(cases.length, rows.length, "one case per row");
  rows.forEach((r, i) => {
    assert.equal(cases[i].row, r.row, `row ${i + 1}`);
    assert.ok(r.when.startsWith(cases[i].when), `row ${i + 1}: "${r.when}" starts "${cases[i].when}"`);
  });
});

for (const c of cases) test(`request: ${c.row}, ${c.when}`, () => check(c));

test("an owner's delegation begins the kept session rule 3 asks, though another session's machine took it", () => {
  // A retired store's owner's delegation session S, older; a mention S2 since begun and answered.
  const s = given({ sess: { request: null }, other: "done", facts: { delegate: null, sessions: { [S]: { kind: "delegation", createdAt: T0, started: true }, [S2]: { kind: "mention", createdAt: T1, started: true } } } });
  const o = apply(s, ev("delegateChanged", { by: "owner", at: T2 }));
  assert.equal(o.req, "requested");
  assert.deepEqual(o.store.sessions[S].facts.request, { kind: "delegation", at: T2 });
});

test("a stop in a session with no machine, its start lost or retired, is told it stopped", () => {
  const o = apply(given({ sess: { request: null } }), ev("ownerStopped", { session: S, byOwner: true, at: T1 }));
  assert.equal(o.req, "done");
  assert.deepEqual(o.commands, ["postNotice:stopped"]);
});

test("a session's first start begins its machine at requested, asking for the issue", () => {
  const o = apply(given({ sess: { request: null }, facts: { sessions: {} } }), ev("sessionStarted", { session: S, sessionKind: "delegation", byOwner: true, at: T1 }));
  assert.equal(o.req, "requested");
  assert.deepEqual(o.commands, ["readIssue"]);
});

test("a second session waits for its own read; the newest delegation's session is the one asked", () => {
  const s = given({ req: "requested", other: "requested", facts: { sessions: { [S]: { kind: "delegation", createdAt: T0, started: true }, [S2]: { kind: "delegation", createdAt: T1, started: true } } }, queue: [readIssue] });
  const o = apply(s, result(`${ISSUE}.1`, "ok", read({ sessions: [{ id: S, kind: "delegation", createdAt: T0 }, { id: S2, kind: "delegation", createdAt: T1 }] })));
  assert.equal(o.req, "done");
});

// A creator-less session on an issue the owner delegated again is a chain's only to the
// Worker's ingress; the machine counts it toward the asked session while Tolkster made the
// current delegation.
test("a chain's session takes a delegation's ask only while its chain holds", () => {
  const parent = { id: P, delegated: true, by: "owner" as const, state: "open" as const, priority: 0, updatedAt: T1 };
  const sessions = { [S]: { kind: "delegation" as const, createdAt: T0, started: true as const }, [S2]: { kind: "chain" as const, createdAt: T1, started: true as const } };
  const at = (delegate: "owner" | "chain") => apply(given({ req: "requested", facts: { sessions, chain: { parent: P, run: "r1" }, delegate: { value: delegate, at: T1 } }, queue: [readIssue] }), result(`${ISSUE}.1`, "ok", read({ delegate, parent })));
  assert.notEqual(at("owner").req, "done");
  assert.equal(at("chain").req, "done");
});

// A mention in a new issue's description reaches the Worker as a delegation's session; only a
// read says where it began (docs/state-machine.md, "The owner's messages").
const undelegated = { delegate: { value: null, at: T0 }, everDelegated: false };
const described = { id: S, kind: "delegation" as const, createdAt: T0, inDescription: true };

test("a description mention on an undelegated issue is answered as a mention, not refused as a delegation", () => {
  const s = given({ req: "requested", facts: undelegated, queue: [readIssue] });
  const o = apply(s, result(`${ISSUE}.1`, "ok", read({ delegate: null, sessions: [described] })));
  assert.equal(o.req, "claimed");
  assert.deepEqual(o.commands, ["startTurn"]);
  assert.deepEqual(o.store.facts.sessions[S], { kind: "mention", createdAt: T0, started: true });
  assert.equal(o.store.sessions[S].facts.request?.kind, "start");
  const plain = apply(given({ req: "requested", facts: undelegated, queue: [readIssue] }), result(`${ISSUE}.1`, "ok", read({ delegate: null })));
  assert.deepEqual([plain.req, plain.commands], ["done", ["postNotice:notDelegated"]]);
});

// A prompt written while a session's turn runs, before it answers, is after what that turn was
// shown but before its answer: the next turn's messages start after the former.
test("a session's prompt written mid-turn, before its answer, is in the next turn's messages", () => {
  const issue = read({ delegate: null, sessions: [{ id: S, kind: "mention", createdAt: T0 }] });
  const land = (o: { store: Store }) => apply(o.store, result(o.store.queue.findLast((c) => c.kind === "readIssue")!.id, "ok", issue));
  const s = given({ req: "claimed", kind: "mention", facts: undelegated, sess: { turn, request: { kind: "start", at: T0 }, turnReadThrough: T0 } });
  const prompted = land(apply(s, ev("ownerPrompted", { session: S, byOwner: true, at: T1 })));
  assert.equal(prompted.req, "claimed");
  const answered = apply(prompted.store, ev("workerAnswered", { session: S, reader: "turn", at: T2 }));
  const next = land(apply(answered.store, ev("turnEnded", { session: S })));
  const start = next.store.queue.find((c) => c.kind === "startTurn");
  assert.ok(start !== undefined, next.commands.join());
  assert.equal(toWire(start, next.store, "TW-1").args.after, T0);
});

test("a read narrows a delegation's session to a mention's and never widens one", () => {
  const readOf = (sessions: IssueRead["sessions"]): IssueRead => ({ updatedAt: T1, delegate: null, state: "open", triage: false, stateChangedAt: T0, priority: 0, blockers: [], moreBlockers: false, parent: null, children: [], moreChildren: false, repo: "repo", sessions });
  const merged = (kind: "delegation" | "mention", entry: object, request: { kind: "start" | "delegation"; at: string } = { kind: "delegation", at: T0 }) => {
    const s = given({ req: "requested", kind, sess: { request } });
    mergeReadOnly(s, readOf([{ id: S, kind: "delegation", createdAt: T0, ...entry }]), NOW);
    return s;
  };
  assert.equal(merged("delegation", { inDescription: true }).facts.sessions[S].kind, "mention");
  assert.equal(merged("mention", {}).facts.sessions[S].kind, "mention");
  assert.equal(merged("delegation", {}).facts.sessions[S].kind, "delegation");
  // A request a later delegation made is that delegation's, not the start's.
  assert.equal(merged("delegation", { inDescription: true }).sessions[S].facts.request?.kind, "start");
  assert.equal(merged("delegation", { inDescription: true }, { kind: "delegation", at: T1 }).sessions[S].facts.request?.kind, "delegation");
  // Narrowed, it stays so when its own event arrives after the read, and in what a settled issue keeps.
  const early = given({ facts: { sessions: {}, ...undelegated } });
  mergeReadOnly(early, readOf([described]), NOW);
  assert.equal(early.facts.sessions[S].kind, "mention");
  const o = apply(early, ev("sessionStarted", { session: S, sessionKind: "delegation", byOwner: true, at: T0 }));
  assert.equal(o.store.facts.sessions[S].kind, "mention");
  assert.equal(o.store.sessions[S].facts.request?.kind, "start");
  assert.equal(retired(o.store).facts.sessions[S].kind, "mention");
});

test("a read with more than 50 blockers counts the issue as blocked, though none read is unfinished", () => {
  const o = apply(given({ req: "requested", queue: [readIssue] }), result(`${ISSUE}.1`, "ok", read({ blockers: [{ id: B, done: true, updatedAt: T1 }], moreBlockers: true })));
  assert.deepEqual([o.req, o.store.sessions[S].facts.waiting], ["queued", "blockers"]);
});

test("a request waiting on blockers waits for nothing once they finish, and one waiting for anything else ignores them", () => {
  const blockers = { more: false, sentMs: NOW - HOUR, ids: [B], states: { [B]: { done: false, at: T0 } } };
  const freed = apply(given({ req: "queued", sess: { waiting: "blockers" }, facts: { blockers } }), ev("blockersChanged", { blocker: B, done: true, at: T1 }));
  assert.deepEqual([freed.req, freed.store.sessions[S].facts.waiting], ["requested", null]);
  const slot = apply(given({ req: "queued", sess: { waiting: "slot" }, facts: { blockers } }), ev("blockersChanged", { blocker: B, done: true, at: T1 }));
  assert.deepEqual([slot.req, slot.commands], ["queued", []]);
});

test("a sub-issue in Triage doesn't hold up its parent's end, but one past the first 50 does", () => {
  const children = (more: boolean) => ({ sentMs: NOW - HOUR, ids: [B, B2], states: { [B]: { done: false, at: T0 }, [B2]: { done: false, triage: true, at: T0 } }, more });
  const parent = (more: boolean) => given({ req: "done", sess: { orchestrated: { at: T0, known: [] }, answeredThrough: T1 }, facts: { children: children(more) } });
  const ended = apply(parent(false), ev("childChanged", { child: B, done: true, at: T1 }));
  assert.equal(ended.req, "requested");
  assert.deepEqual(ended.commands, ["readIssue"]);
  assert.equal(apply(parent(true), ev("childChanged", { child: B, done: true, at: T1 })).req, "done");
  // One the chain took and someone moved back to Triage doesn't hold it either.
  const took = given({ req: "done", sess: { orchestrated: { at: T0, known: [] }, answeredThrough: T1 }, facts: { children: children(false), chained: { run: "r-1", issues: [B2], asked: [], answered: [], haltedAt: null } } });
  assert.equal(apply(took, ev("childChanged", { child: B, done: true, at: T1 })).req, "requested");
  // Accepted out of Triage, it holds the end up again.
  const accepted = apply(parent(false), ev("childChanged", { child: B2, done: false, triage: false, at: T1 }));
  assert.equal(accepted.req, "done");
  assert.deepEqual(pending(accepted.store.facts.children), [B, B2]);
});

test("a read's sub-issue in Triage is left out of the parent's end, and leaving Triage changes it", () => {
  const s = given({ req: "done" });
  const member = (id: string, done: boolean, triage: boolean, updatedAt: string) => ({ id, done, triage, updatedAt });
  const first: IssueRead = { updatedAt: T1, delegate: "owner", state: "open", triage: false, stateChangedAt: T0, priority: 0, blockers: [], moreBlockers: false, parent: null, children: [member(B, true, false, T1), member(B2, false, true, T1)], moreChildren: false, repo: "repo", sessions: [] };
  const read = landRead(s, first, NOW - MIN);
  assert.ok(childrenFinished(read.store.facts));
  const accepted = landRead(read.store, { ...first, updatedAt: T2, children: [member(B, true, false, T1), member(B2, false, false, T2)] }, NOW);
  assert.ok(!childrenFinished(accepted.store.facts));
});

test("a sub-issue tells its parent whether it is in Triage, and only while it is open", () => {
  const child = (triage: boolean) => given({ req: "done", facts: { issueState: { value: "open", at: T0, changedAt: T0, triage }, parent: { id: P, at: T0, delegated: true, by: "owner", state: "open", parentAt: T0 } } });
  const sent = (state: "open" | "completed", triage: boolean, was = !triage) => {
    const o = apply(child(was), ev("issueStateChanged", { state, triage, parent: P, at: T1 }));
    const e = o.store.outbox.map((m) => m.event).find((x) => x.kind === "childChanged");
    return e?.kind === "childChanged" ? { child: e.child, done: e.done, triage: e.triage } : null;
  };
  assert.deepEqual(sent("open", true), { child: ISSUE, done: false, triage: true });
  assert.deepEqual(sent("open", false), { child: ISSUE, done: false, triage: false });
  assert.deepEqual(sent("completed", true), { child: ISSUE, done: true, triage: false });
  // A move between states the machines see as one tells it nothing.
  assert.equal(sent("open", false, false), null);
});

// A note's thread's turn that answered is done with its request once it was shown it, or when
// no turn ran (nothing in its batch, or no repo): asking again would find the same.
test("a note's thread's turn that answered and was shown its request, or never ran, handles it", () => {
  const shown = apply(given({ req: "claimed", kind: "note", sess: { turn, answeredThrough: T1, turnReadThrough: T0 } }), ev("turnEnded", { session: S }));
  assert.deepEqual([shown.req, shown.commands], ["done", []]);
  const none = apply(given({ req: "claimed", kind: "note", sess: { turn: { ...turn, terminal: null }, answeredThrough: T1 } }), ev("turnEnded", { session: S }));
  assert.deepEqual([none.req, none.commands], ["done", []]);
});

// A turn's end that lands once its request has left `claimed` (a reply moved it to
// `requested`, or its answer to `done`) still forgets the turn, so the next decision starts
// one rather than wait for the deadline's `observe`.
test("a turn that ends outside claimed is forgotten, so the next request starts a turn", () => {
  for (const req of ["requested", "done"] as const) {
    const o = apply(given({ req, sess: { turn } }), ev("turnEnded", { session: S }));
    assert.equal(o.req, req);
    assert.equal(o.store.sessions[S].facts.turn, null, req);
  }
});

// Linear opens no new session for it, and sends it only on a change: the owner's delegation
// over the owner's, its take-away late, is the worker's session's request again, as the
// seeds found it, and a read's only moves the clock.
test("the owner's newer delegation on a worker's claim is decided again as its request, and a read's is not", () => {
  const claim = { req: "claimed", work: "running", sess: { answeredThrough: T1 }, facts: { delegate: { value: "owner", at: T0 } } } as const;
  const again = apply(given(claim), ev("delegateChanged", { by: "owner", at: T2 }));
  assert.deepEqual([again.req, again.commands, again.store.sessions[S].facts.request], ["requested", ["readIssue"], { kind: "delegation", at: T2 }]);
  const late = apply(again.store, ev("delegateChanged", { by: null, at: T1 }));
  assert.deepEqual([late.req, late.commands], ["requested", []]);
  const viaRead = apply(given({ ...claim, queue: [{ kind: "readIssue", machine: "issue" }] }), result(`${ISSUE}.1`, "ok", read({ updatedAt: T2 })));
  assert.deepEqual([viaRead.req, viaRead.commands], ["claimed", []]);
  // A create's late webhook at the clock of the `created` delegation rule 6 took as the owner's.
  const created = apply(given({ ...claim, facts: { delegate: { value: "owner", at: T0, created: true } } }), ev("delegateChanged", { by: "owner", at: T0 }));
  assert.deepEqual([created.req, created.commands], ["claimed", []]);
});

test("a claimed turn whose start was lost and never opened is told at its deadline's observe, which reports it ended", () => {
  const opening = { ...turn, terminal: null };
  const lost = apply(given({ req: "claimed", kind: "mention", sess: { turn: opening }, queue: [{ kind: "startTurn", machine: S }] }), result(`${ISSUE}.1`, "lost"));
  assert.deepEqual([lost.req, lost.commands, lost.store.sessions[S].facts.turn?.terminal], ["claimed", [], null]);
  const held = given({ req: "claimed", kind: "mention", sess: { turn: opening }, armed: [{ machine: "request", kind: "own" }] });
  assert.deepEqual(apply(held, deadline(held, "request")).commands, ["observe"]);
  const ended = apply(held, ev("turnEnded", { session: S }));
  assert.deepEqual([ended.req, ended.commands], ["done", ["postNotice:stalled"]]);
});

test("a startTurn that lands ok or lost after its stop was confirmed stops the turn it may have opened, says \"Stopped.\" no more, and a held, failed or dropped one opened nothing", () => {
  // Still unsent when the stop comes, it is dropped with the turn held for it.
  const unsent = apply(given({ req: "claimed", sess: { turn: { ...turn, terminal: null } }, queue: [{ kind: "readIssue", machine: "issue" }, { kind: "startTurn", machine: S, params: { request: turn.for }, firstSentMs: null, sentMs: null, sends: 0 }] }), ev("ownerStopped", { session: S, byOwner: true, at: T1 }));
  assert.deepEqual([unsent.req, unsent.commands, unsent.store.sessions[S].facts.turn], ["stopping", ["stopTurn"], null]);
  // An older request's start, dropped or failed, leaves a newer request's turn alone.
  const newer = { ...turn, terminal: null, for: T1 };
  const older = { kind: "startTurn", machine: S, params: { request: T0 } } as const;
  const dropped = apply(given({ req: "claimed", sess: { turn: newer }, queue: [{ kind: "readIssue", machine: "issue" }, { ...older, firstSentMs: null, sentMs: null, sends: 0 }] }), ev("ownerStopped", { session: S, byOwner: true, at: T2 }));
  assert.equal(dropped.store.sessions[S].facts.turn?.for, T1);
  const failedOlder = apply(given({ req: "stopping", sess: { turn: newer }, reqStop: owed, queue: [older, { kind: "stopTurn", machine: S }] }), result(`${ISSUE}.1`, "failed"));
  assert.equal(failedOlder.store.sessions[S].facts.turn?.for, T1);
  const s = given({ req: "stopping", kind: "note", sess: { turn: { ...turn, terminal: null } }, reqStop: owed, queue: [{ kind: "startTurn", machine: S, params: { request: turn.for } }, { kind: "stopTurn", machine: S }] });
  // Failed before the stop is confirmed, the turn held for it is dropped, and the stop still ends it.
  const failed = apply(s, result(`${ISSUE}.1`, "failed"));
  assert.deepEqual([failed.req, failed.store.sessions[S].facts.turn], ["stopping", null]);
  assert.equal(apply(failed.store, result(`${ISSUE}.2`, "ok")).req, "done");
  const confirmed = apply(s, result(`${ISSUE}.2`, "ok"));
  assert.deepEqual([confirmed.req, confirmed.commands], ["done", ["postNotice:stopped"]]);
  const late = apply(confirmed.store, result(`${ISSUE}.1`, "ok", { terminal: "term_2" }));
  assert.deepEqual(late.commands, ["stopTurn"]);
  // The late turn's close, and its end, say nothing more.
  const closed = apply(late.store, result(late.store.queue.find((c) => c.kind === "stopTurn")!.id, "ok"));
  assert.deepEqual([closed.commands, apply(closed.store, ev("turnEnded", { session: S, terminal: "term_2" })).commands], [[], []]);
  assert.deepEqual(apply(confirmed.store, result(`${ISSUE}.1`, "lost")).commands, ["stopTurn"]);
  // One that opened nothing, as a note's thread with nothing to answer, says no terminal.
  assert.deepEqual(apply(confirmed.store, result(`${ISSUE}.1`, "ok")).commands, []);
  assert.equal(apply(confirmed.store, result(`${ISSUE}.1`, "held")).commands.length, 0);
});

test("a startTurn that lands in a request still holding its turn records the terminal and stops nothing", () => {
  const o = apply(given({ req: "requested", sess: { turn: { ...turn, terminal: null } }, queue: [{ kind: "startTurn", machine: S }] }), result(`${ISSUE}.1`, "ok", { terminal: "term_2" }));
  assert.deepEqual(o.commands, []);
  assert.equal(o.store.sessions[S].facts.turn?.terminal, "term_2");
});

// A parent mid-orchestration whose worker has chained B and exited, and a sub-issue's change on it.
const parent = (over: { facts?: object; sess?: object; work?: "rest" | "running" } = {}) =>
  given({ req: "done", work: over.work ?? "rest", sess: { ...orchestrating, ...over.sess }, facts: { children: { sentMs: NOW - HOUR, ids: [B], states: { [B]: { done: false, at: T0 } }, more: false }, chained: chainTook, ...over.facts } });
const added = (child: string, at: string, triage = false) => ev("childChanged", { child, done: false, triage, at });
const member = (id: string, done: boolean, updatedAt: string, triage = false) => ({ id, done, triage, updatedAt });
// The pick-up's read lands: the parent's sub-issues as Linear has them, and the slot its
// decision asks for granted in that step, as the coordinator grants a free one.
const readLands = (s: Store, children: ReturnType<typeof member>[]) => {
  const c = s.queue.find((q) => q.kind === "readIssue" && q.machine === S)!;
  const o = apply(s, result(c.id, "ok", read({ children })));
  const asked = o.store.sessions[S].facts.asked;
  return o.req === "queued" && asked !== null ? apply(o.store, ev("slotGranted", { session: S, answers: asked })) : o;
};
const resumes = (o: { store: Store }) => o.store.queue.filter((c) => c.kind === "resumeWorker");

// Linear opens no new session for a delegation on an issue that has one, so the turn's end is
// the only thing left to start the work.
test("a delegation while a conversation turn runs is decided as work once the turn ends", () => {
  const during = apply(given({ req: "claimed", kind: "mention", sess: { turn }, facts: { delegate: { value: null, at: T0 } } }), ev("delegateChanged", { by: "owner", at: T1 }));
  assert.deepEqual(during.store.sessions[S].facts.request, { kind: "delegation", at: T1 });
  const ended = apply(during.store, ev("turnEnded", { session: S }));
  assert.deepEqual([ended.req, ended.commands], ["requested", ["readIssue"]]);
  const c = ended.store.queue.find((q) => q.kind === "readIssue" && q.machine === S)!;
  const o = apply(ended.store, result(c.id, "ok", read({ updatedAt: T1, sessions: [{ id: S, kind: "mention", createdAt: T0 }] })));
  const asked = o.store.sessions[S].facts.asked!;
  const granted = apply(o.store, ev("slotGranted", { session: S, answers: asked }));
  assert.equal(granted.req, "claimed");
  assert.deepEqual([granted.work, granted.commands], ["launching", ["startWorker"]]);
  // A note's thread is answered in its thread: the delegation's own session decides it.
  const note = apply(given({ req: "claimed", kind: "note", sess: { turn }, facts: { delegate: { value: null, at: T0 } } }), ev("delegateChanged", { by: "owner", at: T1 }));
  assert.notEqual(note.store.sessions[S].facts.request?.at, T1);
});

test("a sub-issue added to a parent mid-orchestration resumes its worker once, to chain it", () => {
  const asked = apply(parent(), added(B2, T2));
  assert.equal(asked.req, "requested");
  // On the session's own clocks, never the sub-issue's, so the owner's later stop still counts.
  assert.deepEqual(asked.store.sessions[S].facts.request, { kind: "childrenAdded", at: T1 });
  const o = readLands(asked.store, [member(B, false, T0), member(B2, false, T2)]);
  assert.equal(o.req, "claimed");
  assert.equal(o.work, "launching");
  assert.deepEqual(resumes(o).map((c) => c.params), [{ session: S, childrenEnd: false, childrenAdded: true }]);
  assert.deepEqual(o.store.sessions[S].facts.orchestrated, { at: T1, known: [B, B2] });
  // Its `issue chain` is taken from the session, as the orchestration's, once the resume lands.
  const resumed = apply(o.store, result(resumes(o)[0].id, "ok"));
  const chained = apply(resumed.store, { kind: "chainRecorded", id: "chain-2", issue: ISSUE, role: "ask", session: S, run: "r-2", children: [B2] });
  assert.deepEqual(chained.store.facts.chained?.asked, [B2]);
  assert.ok(chained.out.includes("chainRecorded"));
  // Known now, it brings nothing back again.
  const again = apply(chained.store, ev("workChanged", { from: "launching", to: "rest", version: 3, noticed: null }));
  assert.ok(!again.commands.includes("readIssue"));
});

test("sub-issues added together resume the parent once", () => {
  const first = apply(parent(), added(B2, T1));
  const second = apply(first.store, added(C3, T2));
  assert.equal(second.req, "requested");
  assert.deepEqual(second.commands, []);
  const o = readLands(second.store, [member(B, false, T0), member(B2, false, T1), member(C3, false, T2)]);
  assert.equal(resumes(o).length, 1);
  assert.deepEqual(o.store.sessions[S].facts.orchestrated?.known, [C3, B, B2]);
});

// A sub-issue's store, B2's, under P as read, and what it sends to other stores.
const sub = (g: Parameters<typeof given>[0] = {}) => {
  const s = given({ ...g, facts: { parent: { id: P, at: T0, delegated: true, by: "owner", state: "open", parentAt: T0 }, ...g.facts } });
  s.issue = B2;
  return s;
};
const moved = (parent: string | null, from: string | null, at: string, state: "open" | "completed" = "open") => ev("issueStateChanged", { state, triage: false, parent, ...(from === null ? {} : { from }), at });
const sent = (before: Store, after: Store) => after.outbox.slice(before.outbox.length).map(({ event: { id: _, ...e } }) => e);

test("a sub-issue moved under a parent tells it and the parent it left, moving none of its own machines", () => {
  // Finished already: a change of state would refuse the request, with a notice.
  const s = sub({ req: "requested", facts: { issueState: { value: "completed", at: T0, changedAt: T0 } } });
  const o = apply(s, moved(ISSUE, P, T1, "completed"));
  assert.deepEqual(sent(s, o.store), [
    { kind: "childChanged", issue: P, child: B2, done: true, left: true, at: T1 },
    { kind: "childChanged", issue: ISSUE, child: B2, done: true, triage: false, at: T1 },
  ]);
  assert.equal(o.req, "requested");
  assert.deepEqual(o.commands, []);
  assert.equal(o.store.facts.parent?.id, ISSUE);
  // Its replay, or an older move, only tells the parent it left again, whose own clock drops it.
  assert.deepEqual(apply(o.store, moved(ISSUE, P, T1, "completed")).out, ["childChanged"]);
  const older = apply(o.store, moved(C3, P, T0, "completed"));
  assert.deepEqual(sent(o.store, older.store), [{ kind: "childChanged", issue: P, child: B2, done: true, left: true, at: T0 }]);
  assert.equal(older.store.facts.parent?.id, ISSUE);
});

test("a sub-issue created or moved under a parent mid-orchestration is picked up", () => {
  for (const s of [sub({ facts: { parent: null } }), sub()]) {
    const o = apply(s, moved(ISSUE, s.facts.parent?.id ?? null, T2));
    const [child] = sent(s, o.store).filter((e) => "issue" in e && e.issue === ISSUE);
    assert.deepEqual(child, { kind: "childChanged", issue: ISSUE, child: B2, done: false, triage: false, at: T2 });
    const picked = apply(parent(), { ...child, id: "moved" } as Event);
    assert.equal(picked.req, "requested");
    assert.deepEqual(picked.commands, ["readIssue"]);
  }
});

test("a sub-issue moved off its parent no longer reaches it, and the parent no longer counts it", () => {
  const s = sub();
  const o = apply(s, moved(null, P, T1));
  assert.deepEqual(sent(s, o.store), [{ kind: "childChanged", issue: P, child: B2, done: false, left: true, at: T1 }]);
  assert.equal(o.store.facts.parent, null);
  const done = apply(o.store, moved(null, null, T2, "completed"));
  assert.deepEqual(sent(o.store, done.store).map((e) => e.kind), ["blockerChanged"]);
  // The parent's only sub-issue left: none holds up its end, which it brings.
  const left = apply(parent(), ev("childChanged", { child: B, done: false, left: true, at: T1 }));
  assert.deepEqual(left.store.facts.children?.ids, []);
  assert.ok(childrenFinished(left.store.facts));
  assert.equal(left.req, "requested");
  // An older change of it, or a read sent before the move that lands after, counts it no more.
  assert.deepEqual(apply(left.store, added(B, T0)).store.facts.children?.ids, []);
  const f = landRead(left.store, { updatedAt: T1, delegate: "owner", state: "open", triage: false, stateChangedAt: T0, priority: 0, blockers: [], moreBlockers: false, parent: null, children: [member(B, false, T0)], moreChildren: false, repo: "repo", sessions: [] }, NOW - 2 * MIN);
  assert.deepEqual(pending(f.store.facts.children), []);
});

test("a chained sub-issue moved off its parent while its worker runs leaves that chain, and the new parent chains it no second time", () => {
  const chained = { work: "running" as const, kind: "chain" as const, sess: { byOwner: false }, facts: { chain: { parent: P, run: "r-1" }, delegate: { value: "chain" as const, at: T0 } } };
  const s = sub(chained);
  const o = apply(s, moved(ISSUE, P, T1));
  assert.equal(o.store.facts.chain, null);
  assert.deepEqual([o.work, o.commands], ["running", []]);
  const [left] = sent(s, o.store);
  assert.deepEqual(left, { kind: "childChanged", issue: P, child: B2, done: false, left: true, at: T1 });
  // The old parent drops it from its chain, so its halt neither stops it nor takes it back.
  const old = parent({ work: "running", facts: { children: { sentMs: NOW - HOUR, ids: [B, B2], states: { [B]: { done: false, at: T0 }, [B2]: { done: false, at: T0 } }, more: false }, chained: { ...chainTook, issues: [B, B2], asked: [B, B2], answered: [B, B2] } } });
  const dropped = apply(old, { ...left, id: "left" } as Event);
  assert.deepEqual(dropped.store.facts.chained?.issues, [B]);
  const halted = apply(dropped.store, ev("delegateChanged", { by: null, at: T2 }));
  assert.deepEqual(sent(dropped.store, halted.store).filter((e) => e.kind === "parentHalted").map((e) => e.issue), [B]);
  // One that left only, after a halt sent before the move lands, keeps its worker.
  assert.deepEqual(apply(o.store, ev("parentHalted", { parent: P, at: T2 })).work, "running");
  // The parent it has now asks it into its chain: it refuses, keeping the worker it has.
  const forward = apply(o.store, ev("chainRecorded", { role: "forward", parent: ISSUE, run: "r-2" }));
  assert.equal(forward.store.facts.chain, null);
  assert.deepEqual(sent(o.store, forward.store), [{ kind: "chainRecorded", issue: ISSUE, role: "answer", child: B2, run: "r-2", took: false }]);
  // Once halted, a chain that delegates it again takes it.
  const stopped = apply(sub(chained), ev("parentHalted", { parent: P, at: T1 }));
  assert.equal(stopped.work, "halted");
  const stop = stopped.store.queue.find((c) => c.kind === "stopWorker")!;
  const again = apply(apply(stopped.store, result(stop.id, "ok")).store, ev("chainRecorded", { role: "forward", parent: P, run: "r-2" }));
  assert.equal(again.store.facts.chain?.parent, P);
});

test("a move ends the chain however it races the chain's own events", () => {
  const two = { children: { sentMs: NOW - HOUR, ids: [B, B2], states: { [B]: { done: false, at: T0 }, [B2]: { done: false, at: T0 } }, more: false } };
  // An answer that waited behind the move's `childChanged` adds nothing.
  const asking = parent({ work: "running", facts: { ...two, chained: { run: "r-2", issues: [B], asked: [B, B2], answered: [B], haltedAt: null } } });
  const left = apply(asking, ev("childChanged", { child: B2, done: false, left: true, at: T1 }));
  const late = apply(left.store, ev("chainRecorded", { role: "answer", child: B2, run: "r-2", took: true }));
  assert.deepEqual(late.store.facts.chained?.issues, [B]);
  // Nor one that lands after it moved back.
  const back = apply(left.store, ev("childChanged", { child: B2, done: false, triage: false, at: T2 }));
  assert.deepEqual(apply(back.store, ev("chainRecorded", { role: "answer", child: B2, run: "r-2", took: true })).store.facts.chained?.issues, [B]);
  // A take-back owed keeps the one it halted, whose delegation it is still to take back.
  const owed = parent({ work: "running", facts: { ...two, takeBack: { run: "r-1" }, chained: { ...chainTook, issues: [B, B2], haltedAt: T0 } } });
  assert.deepEqual(apply(owed, ev("childChanged", { child: B2, done: false, left: true, at: T1 })).store.facts.chained?.issues, [B, B2]);
  // A forward sent before it moved off to no parent takes nothing.
  const off = apply(sub(), moved(null, P, T1)).store;
  const forward = apply(off, ev("chainRecorded", { role: "forward", parent: P, run: "r-2" }));
  assert.equal(forward.store.facts.chain, null);
  // A chain taken while its parent was unknown ends, telling that parent, once a move shows another.
  const unknown = sub({ facts: { parent: null, chain: { parent: P, run: "r-1" } } });
  const shown = apply(unknown, moved(ISSUE, null, T1));
  assert.equal(shown.store.facts.chain, null);
  assert.deepEqual(sent(unknown, shown.store)[0], { kind: "childChanged", issue: P, child: B2, done: false, left: true, at: T1 });
});

test("a sub-issue tells the parent it held whenever it learns of a move, and an older read gives it none back", () => {
  const issueRead = (parent: string | null, updatedAt: string): IssueRead => ({ updatedAt, delegate: "owner", state: "open", triage: false, stateChangedAt: T0, priority: 0, blockers: [], moreBlockers: false, parent: parent === null ? null : { id: parent, delegated: true, by: "owner", state: "open", priority: 0, updatedAt: T0 }, children: [], moreChildren: false, repo: "repo", sessions: [] });
  // A webhook with no parent, though it names none it left.
  const s = sub();
  const o = apply(s, moved(null, null, T1));
  assert.deepEqual(sent(s, o.store), [{ kind: "childChanged", issue: P, child: B2, done: false, left: true, at: T1 }]);
  // A read sent before the move, landing after it, gives it no parent back.
  const late = landRead(o.store, issueRead(P, T0));
  assert.deepEqual(sent(o.store, late.store), []);
  assert.equal(late.store.facts.parent, null);
  // A webhook naming the parent it has moves its clock, so an older read moves it nowhere.
  const named = apply(sub(), moved(P, null, T2));
  const older = landRead(named.store, issueRead(ISSUE, T1));
  assert.deepEqual(sent(named.store, older.store), []);
  assert.equal(older.store.facts.parent?.id, P);
  // Nor does a read older than a newer read naming the same parent.
  const twice = landRead(sub(), issueRead(P, T2));
  const again = landRead(twice.store, issueRead(ISSUE, T1));
  assert.deepEqual(sent(twice.store, again.store), []);
  assert.equal(again.store.facts.parent?.id, P);
  // A read that shows a move, its webhook lost: the parent it held drops it, and the new one counts it.
  const read = sub({ queue: [{ kind: "readIssue", machine: "work", params: {} }] });
  const landed = apply(read, result(read.queue[0].id, "ok", { read: issueRead(ISSUE, T1) }));
  assert.deepEqual(sent(read, landed.store), [
    { kind: "childChanged", issue: P, child: B2, done: false, left: true, at: T1 },
    { kind: "childChanged", issue: ISSUE, child: B2, done: false, triage: false, at: T1 },
  ]);
});

test("a sub-issue added in Triage resumes nothing until it leaves Triage", () => {
  const triaged = apply(parent(), added(B2, T1, true));
  assert.deepEqual([triaged.req, triaged.commands], ["done", []]);
  const accepted = apply(triaged.store, added(B2, T2));
  assert.deepEqual([accepted.req, accepted.commands], ["requested", ["readIssue"]]);
});

test("a sub-issue added to a halted, undelegated or stopped parent resumes nothing", () => {
  for (const over of [
    { facts: { chained: { ...chainTook, haltedAt: T1 } } },
    { facts: { delegate: { value: null, at: T1 } } },
    { facts: { issueState: { value: "cancelled" as const, at: T1, changedAt: T1 } } },
    { sess: { orchestrated: null } },
    { facts: { chained: null } },
  ]) {
    const o = apply(parent(over), added(B2, T2));
    assert.deepEqual([o.req, o.commands], ["done", []], JSON.stringify(over));
  }
});

test("a sub-issue added while the parent's worker runs waits for it to leave", () => {
  const running = apply(parent({ work: "running" }), added(B2, T2));
  assert.deepEqual([running.req, running.commands], ["done", []]);
  running.store.work!.state = "rest";
  const left = apply(running.store, ev("workChanged", { from: "running", to: "rest", version: 2, noticed: null }));
  assert.deepEqual([left.req, left.commands], ["requested", ["readIssue"]]);
});

test("a pick-up whose read finds nothing added, or a worker running, does nothing", () => {
  const asked = apply(parent(), added(B2, T2));
  // Finished before the read: nothing to chain.
  const done = readLands(structuredClone(asked.store), [member(B, false, T0), member(B2, true, "2026-10-01T03:00:00.000Z")]);
  assert.deepEqual([done.req, resumes(done).length], ["done", 0]);
  // A worker running, resumed for another request meanwhile: picked up once it leaves.
  const busy = structuredClone(asked.store);
  busy.work!.state = "running";
  const o = readLands(busy, [member(B, false, T0), member(B2, false, T2)]);
  assert.deepEqual([o.req, o.store.queue.filter((c) => c.kind === "deliverMessage").length], ["done", 0]);
});

test("a pick-up that finds every sub-issue finished brings the parent's end instead", () => {
  const asked = apply(parent(), added(B2, T2));
  // B finished, and B2 was cancelled, while the read was in flight.
  const o = readLands(asked.store, [member(B, true, T2), member(B2, true, "2026-10-01T03:00:00.000Z")]);
  assert.deepEqual([o.req, o.commands], ["requested", ["readIssue"]]);
  assert.equal(o.store.sessions[S].facts.request?.kind, "childrenEnd");
  assert.equal(o.store.sessions[S].facts.orchestrated, null);
});

test("an owner's stop clears the pick-up of every session while no worker runs", () => {
  const s = parent();
  const other = structuredClone(s.sessions[S]);
  other.facts = { ...other.facts, request: { kind: "start", at: T0 }, orchestrated: null };
  s.sessions[S2] = other;
  s.facts.sessions[S2] = { kind: "mention", createdAt: T0, started: true };
  const stopped = apply(s, ev("ownerStopped", { session: S2, byOwner: true, at: T1 }));
  assert.equal(stopped.store.sessions[S].facts.orchestrated, null);
  const o = apply(stopped.store, added(B2, T2));
  assert.deepEqual([o.req, o.commands], ["done", []]);
  // With the worker running for its own session, a stop elsewhere leaves its orchestration.
  const running = parent({ work: "running" });
  running.sessions[S2] = other;
  assert.notEqual(apply(running, ev("ownerStopped", { session: S2, byOwner: true, at: T1 })).store.sessions[S].facts.orchestrated, null);
});

test("a late read report never claims a pick-up its decision hasn't", () => {
  const asked = apply(parent(), added(B2, T2));
  const o = apply(asked.store, ev("workerRead", { session: S, reader: "worker", through: T1 }));
  assert.equal(o.req, "requested");
});

test("a read while the work is merged leaves the request waiting, and the reap then decides it again", () => {
  const read = apply(given({ req: "queued", sess: { waiting: "merged" }, work: "merged" }), ev("workerRead", { session: S, reader: "worker", through: T0 }));
  assert.equal(read.req, "queued");
  const reaped = apply(read.store, ev("workChanged", { from: "merged", to: "reaped", version: 2, noticed: null }));
  assert.deepEqual([reaped.req, reaped.commands], ["requested", ["readIssue"]]);
});

// The stopped worker shown idle again, as the seeds found it: its read of the delegation
// after the stop claims nothing, which would wait on an answer no worker owes.
test("a worker's read while its work is halted or exited leaves the request requested", () => {
  for (const work of ["halted", "exited"] as const) {
    const read = apply(given({ req: "requested", work }), ev("workerRead", { session: S, reader: "worker", through: T1 }));
    assert.deepEqual([read.req, read.commands], ["requested", []], work);
  }
});

test("each reader's read mark is its own, so a turn's read leaves a fresh worker's unread", () => {
  const turned = apply(given({ req: "requested" }), ev("workerRead", { session: S, reader: "turn", through: T1 }));
  assert.equal(turned.store.sessions[S].facts.readThrough, null);
  assert.equal(turned.store.sessions[S].facts.turnReadThrough, T1);
  const worked = apply(turned.store, ev("workerRead", { session: S, reader: "worker", through: T0 }));
  assert.equal(worked.store.sessions[S].facts.readThrough, T0);
  assert.equal(worked.store.sessions[S].facts.turnReadThrough, T1);
  // Only forward, per reader.
  const late = apply(worked.store, ev("workerRead", { session: S, reader: "turn", through: T0 }));
  assert.equal(late.store.sessions[S].facts.turnReadThrough, T1);
});

test("an answer handles a request only once its own reader's read reached it", () => {
  const answered = (reader: "worker" | "turn", sess: object) => apply(given({ req: "claimed", sess }), ev("workerAnswered", { session: S, reader, at: T1 })).req;
  assert.equal(answered("worker", { turnReadThrough: T0 }), "claimed");
  assert.equal(answered("turn", { readThrough: T0 }), "claimed");
  assert.equal(answered("worker", { readThrough: T0 }), "done");
  assert.equal(answered("turn", { turnReadThrough: T0 }), "done");
});

test("a read sent before a sub-issue's change landed doesn't drop it from the parent, but drops what it removed", () => {
  const s = parent({ work: "running", facts: { children: { sentMs: NOW - HOUR, ids: [B, C3], states: { [B]: { done: false, at: T0 }, [C3]: { done: false, at: T0 } }, more: false } } });
  // A read for another session, sent before B2's change landed.
  s.commandSeq += 1;
  s.queue.push({ id: `${ISSUE}.${s.commandSeq}`, issue: ISSUE, kind: "readIssue", machine: S, version: 1, params: { for: T0 }, class1: false, firstSentMs: NOW - MIN, sentMs: NOW - MIN, sends: 1, superseded: false });
  const landed = apply(s, added(B2, T2));
  assert.deepEqual(landed.store.facts.children?.ids, [B, C3, B2]);
  const late = apply(landed.store, result(`${ISSUE}.${s.commandSeq}`, "ok", read({ children: [member(B, false, T0)] })));
  assert.deepEqual(late.store.facts.children?.ids, [B, B2]);
});

test("each way a request ends gives back the slot reserved for it, and only a start hands it to the work", () => {
  type Way = { way: string; given?: Parameters<typeof given>[0]; event: Event | ((s: Store) => Event); req: string };
  const decided = (over: object = {}) => result(`${ISSUE}.1`, "ok", read(over));
  const ways: Way[] = [
    { way: "the delegation taken", given: { req: "queued" }, event: ev("delegateChanged", { by: null, at: T1 }), req: "done" },
    { way: "the issue finished", given: { req: "queued" }, event: ev("issueStateChanged", { state: "completed", triage: false, parent: null, at: T1 }), req: "done" },
    { way: "its parent halted", given: { kind: "chain", sess: { byOwner: false } }, event: ev("parentHalted", { parent: P, at: T1 }), req: "done" },
    { way: "the owner's stop", event: ev("ownerStopped", { session: S, byOwner: true, at: T1 }), req: "done" },
    { way: "the worker's read", given: { work: "held" }, event: ev("workerRead", { session: S, reader: "worker", through: T0 }), req: "claimed" },
    { way: "a decision: not delegated", event: decided({ delegate: null }), req: "done" },
    { way: "a decision: blocked", event: decided({ blockers: [{ id: B, done: false, updatedAt: T1 }] }), req: "queued" },
    { way: "a decision: the worker waits at a prompt", given: { work: "held" }, event: decided(), req: "queued" },
    { way: "a decision: the worktree merged", given: { work: "merged" }, event: decided(), req: "queued" },
    { way: "a decision: the worker runs", given: { work: "running" }, event: decided(), req: "claimed" },
    { way: "a decision: a conversation's turn", given: { kind: "note" }, event: decided(), req: "claimed" },
    { way: "unused 30 minutes", given: { sess: { grant: { sinceMs: NOW - 31 * MIN } }, armed: [{ machine: "request", kind: "own" }] }, event: (s) => deadline(s, "request"), req: "requested" },
  ];
  for (const w of ways) {
    // A read in flight holds back every other event, so only a decision has one.
    const reads = typeof w.event !== "function" && w.event.kind === "commandResult" ? [readIssue] : [];
    const s = given({ req: "requested", queue: reads, ...w.given, sess: { grant: { sinceMs: NOW }, waiting: "slot", ...w.given?.sess } });
    const o = apply(s, typeof w.event === "function" ? w.event(s) : w.event);
    assert.equal(o.req, w.req, w.way);
    assert.deepEqual(o.out.filter((k) => k === "slotReleased"), ["slotReleased"], w.way);
    assert.equal(o.store.sessions[S].facts.grant, null, w.way);
    assert.notEqual(o.store.sessions[S].facts.waiting, "slot", w.way);
  }
  // A grant for a request waiting on its worker is given back, and it still waits for it.
  const late = apply(given({ req: "queued", sess: { waiting: "held" } }), ev("slotGranted", { session: S, answers: "slotReleased:y:3" }));
  assert.deepEqual(late.out, ["slotReleased"]);
  assert.equal(late.store.sessions[S].facts.waiting, "held");
  // A lapse is given back by whatever step comes first after it, not only the deadline.
  const lapsed = apply(given({ req: "requested", sess: { grant: { sinceMs: NOW - 31 * MIN }, waiting: "slot" } }), ev("slotWaiting", { session: S }));
  assert.deepEqual(lapsed.out, ["slotReleased"]);
  assert.equal(lapsed.store.sessions[S].facts.grant, null);
  // A start takes the reservation with it: the work holds it, and nothing is given back.
  const started = apply(given({ req: "requested", queue: [readIssue], sess: { grant: { sinceMs: NOW }, waiting: "slot" } }), decided());
  assert.equal(started.req, "claimed");
  assert.equal(started.work, "launching");
  assert.deepEqual(started.out, ["slotHeld"]);
  assert.equal(started.store.sessions[S].facts.grant, null);
});

// A reply in a conversation goes to its warm turn at once, the turn's once the hand-over
// lands; a delegation closes the warm turn instead, and is decided again at its end.
test("a warm turn is handed a reply at once, and closed for a delegation", () => {
  const issue = (delegate: string | null) => read({ delegate, sessions: [{ id: S, kind: "mention", createdAt: T0 }] });
  const land = (o: { store: Store }, delegate: string | null) => apply(o.store, result(o.store.queue.findLast((c) => c.kind === "readIssue")!.id, "ok", issue(delegate)));
  const warm = given({ req: "done", kind: "mention", facts: undelegated, sess: { turn: { ...turn, idleMs: NOW - MIN }, request: { kind: "start", at: T0 }, answeredThrough: T1, turnReadThrough: T0 } });
  const prompted = land(apply(warm, ev("ownerPrompted", { session: S, byOwner: true, at: T2 })), null);
  assert.deepEqual([prompted.req, prompted.commands, prompted.warm], ["claimed", ["startTurn"], null]);
  const start = prompted.store.queue.find((c) => c.kind === "startTurn")!;
  assert.deepEqual(toWire(start, prompted.store, "TW-1").args, { sessionId: S, thread: false, request: T2, after: T0 });
  const handed = apply(prompted.store, result(start.id, "ok", { terminal: "term_1" }));
  assert.equal(handed.store.sessions[S].facts.turn?.for, T2);
  const delegated = land(apply(warm, ev("delegateChanged", { by: "owner", at: T2 })), "owner");
  assert.deepEqual([delegated.req, delegated.commands], ["claimed", ["stopTurn"]]);
  const stop = delegated.store.queue.find((c) => c.kind === "stopTurn")!;
  assert.deepEqual(toWire(stop, delegated.store, "TW-1").args, { sessionId: S, thread: false, idle: true });
  const ended = apply(apply(delegated.store, result(stop.id, "ok")).store, ev("turnEnded", { session: S }));
  assert.deepEqual([ended.req, ended.commands, ended.warm], ["requested", ["readIssue"], null]);
});

// An idle or an end from a terminal the turn no longer runs in is another turn's; a warm
// hand-over's result in `stopping` gets the stop again, as any start that may have opened a turn.
test("a warm turn's idles and ends count only from its own terminal, and a stop outranks its hand-over", () => {
  const working = given({ req: "done", sess: { turn: { ...turn, terminal: "term_2" }, answeredThrough: T1 } });
  assert.equal(apply(working, ev("turnIdle", { session: S, terminal: "term_1", at: T1 })).store.sessions[S].facts.turn?.idleMs, null);
  const replaced = apply(given({ req: "claimed", sess: { turn: { ...turn, terminal: "term_2" } } }), ev("turnEnded", { session: S, terminal: "term_1" }));
  assert.deepEqual([replaced.req, replaced.commands, replaced.store.sessions[S].facts.turn?.terminal], ["claimed", [], "term_2"]);
  const stopping = apply(given({ req: "stopping", sess: { turn, request: { kind: "prompt", at: T1 } }, reqStop: owed, queue: [handOver, { kind: "stopTurn", machine: S }] }), result(`${ISSUE}.1`, "ok", { terminal: "term_2" }));
  assert.deepEqual([stopping.req, stopping.commands], ["stopping", ["stopTurn"]]);
  // Typed into the turn the stop then closed: the stop covered it, and isn't sent again.
  const covered = apply(given({ req: "stopping", sess: { turn, request: { kind: "prompt", at: T1 } }, reqStop: owed, queue: [handOver, { kind: "stopTurn", machine: S }] }), result(`${ISSUE}.1`, "ok", { terminal: "term_1" }));
  assert.deepEqual([covered.req, covered.commands], ["stopping", []]);
  // A turn whose start's result was lost takes the terminal its idle names.
  const adopted = apply(given({ req: "done", sess: { turn: { ...turn, terminal: null }, answeredThrough: T1 } }), ev("turnIdle", { session: S, terminal: "term_3", at: T1 }));
  assert.deepEqual([adopted.store.sessions[S].facts.turn?.terminal, adopted.warm], ["term_3", 5 * MIN]);
});

// A note's thread's turn warms as a session's: a reply is handed to it at once, with the
// newest reply it was shown; one that lands while its idle close is in flight waits for the
// close's end, and then opens a turn of its own.
test("a note's thread's warm turn is handed a reply, and one landing as it closes gets a new turn", () => {
  const land = (o: { store: Store }) => apply(o.store, result(o.store.queue.findLast((c) => c.kind === "readIssue")!.id, "ok", read({ delegate: null })));
  const answered = apply(given({ req: "done", kind: "note", facts: undelegated, sess: { turn, request: { kind: "prompt", at: T0 }, answeredThrough: T1, turnReadThrough: T0 } }), ev("turnIdle", { session: S, terminal: "term_1", at: T1 }));
  assert.equal(answered.warm, 5 * MIN);
  const prompted = land(apply(answered.store, ev("ownerPrompted", { session: S, byOwner: true, at: T2 })));
  assert.deepEqual([prompted.req, prompted.commands, prompted.warm], ["claimed", ["startTurn"], null]);
  const start = prompted.store.queue.find((c) => c.kind === "startTurn")!;
  assert.deepEqual([start.params, toWire(start, prompted.store, "TW-1").args], [{ request: T2, warm: true, thread: true }, { sessionId: S, thread: true, request: T2, after: T0 }]);
  assert.equal(apply(prompted.store, result(start.id, "ok", { terminal: "term_1" })).store.sessions[S].facts.turn?.for, T2);
  const closing = apply(answered.store, deadline(answered.store, "request", "warm"));
  assert.deepEqual(closing.commands, ["stopTurn"]);
  const stop = closing.store.queue.find((c) => c.kind === "stopTurn")!;
  const held = land(apply(apply(closing.store, result(stop.id, "ok")).store, ev("ownerPrompted", { session: S, byOwner: true, at: T2 })));
  assert.deepEqual([held.req, held.commands], ["claimed", []]);
  const ended = land(apply(held.store, ev("turnEnded", { session: S, terminal: "term_1" })));
  const cold = ended.store.queue.findLast((c) => c.kind === "startTurn")!;
  assert.deepEqual([ended.req, cold.params], ["claimed", { request: T2, thread: true }]);
});
