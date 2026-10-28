import assert from "node:assert/strict";
import { test } from "node:test";
import { toWire } from "../src/worker/route.ts";
import { apply, B, check, deadline, given, ev, HOUR, ISSUE, MIN, NOW, P, result, rowsOf, S, SHA, T0, T1, type Case } from "./machine-fixture.ts";
import { cancelledIssue, evaluated, evaluatePr, fresh, halted, own, owed, pr, readOf, requested, SHA2, updating } from "./machine-work-fixture.ts";

// docs/state-machine.md's work transitions, one case per row, in the doc's order, as the
// request transitions' are.

const cases: Case[] = [
  { row: "reaped workRequested", when: "A new worker", given: { work: "reaped", facts: { worktree: null } }, event: requested(), expect: { work: "launching", commands: ["startWorker"], out: ["slotHeld"], workOwn: null } },
  { row: "running workRequested", when: "—", given: { work: "idle" }, event: requested(), expect: { work: "running", commands: ["deliverMessage"], workOwn: HOUR } },
  { row: "exited workRequested", when: "No worktree recorded, or its start never began", given: { work: "exited", facts: { worktree: null } }, event: requested(), expect: { work: "launching", commands: ["startWorker"], out: ["slotHeld"], workOwn: null } },
  { row: "exited workRequested", when: "Otherwise", given: { work: "rest" }, event: requested(), expect: { work: "launching", commands: ["resumeWorker"], out: ["slotHeld"], workOwn: 10 * MIN } },
  { row: "launching workRequested", when: "The worker reads", given: { work: "launching", w: { launch: "resume" } }, event: requested(), expect: { work: "launching", commands: [] } },
  { row: "held workRequested", when: "The decision queues", given: { work: "held" }, event: requested(), expect: { work: "held", commands: [] } },
  { row: "launching stopRequested", when: "—", given: { work: "running" }, event: ev("stopRequested", { session: S, at: T1 }), expect: { work: "halted", commands: ["stopWorker"], out: ["slotReleased"], workOwn: 2 * MIN } },
  { row: "exited stopRequested", when: "No agent runs", given: { work: "exited" }, event: ev("stopRequested", { session: S, at: T1 }), expect: { work: "exited", commands: [] } },
  { row: "launching delegateChanged", when: "Taken from Tolkster", given: { work: "held" }, event: ev("delegateChanged", { by: null, at: T1 }), expect: { work: "halted", commands: ["stopWorker"], workOwn: 2 * MIN } },
  { row: "launching delegateChanged", when: "Otherwise", given: { work: "launching" }, event: ev("delegateChanged", { by: "chain", at: T1 }), expect: { work: "launching", commands: [] } },
  { row: "exited delegateChanged", when: "No agent runs", given: { work: "rest" }, event: ev("delegateChanged", { by: null, at: T1 }), expect: { work: "rest", commands: [] } },
  { row: "launching parentHalted", when: "Its `chain` held", given: { work: "running", kind: "chain", sess: { byOwner: false }, facts: { chain: { parent: P, run: "r1" }, delegate: { value: "chain", at: T0 } } }, event: ev("parentHalted", { parent: P, at: T1 }), expect: { work: "halted", commands: ["stopWorker"], workOwn: 2 * MIN } },
  { row: "launching parentHalted", when: "Otherwise", given: { work: "running", sess: {}, facts: { chain: { parent: P, run: "r1" } } }, event: ev("parentHalted", { parent: P, at: T1 }), expect: { work: "running", commands: [] } },
  { row: "exited parentHalted", when: "No agent runs", given: { work: "exited", facts: { chain: { parent: P, run: "r1" } } }, event: ev("parentHalted", { parent: P, at: T1 }), expect: { work: "exited", commands: [] } },
  { row: "launching chainRecorded", when: "A fact", given: { work: "running" }, event: ev("chainRecorded", { role: "forward", parent: P, run: "r1" }), expect: { work: "running", commands: [], out: ["chainRecorded"] } },
  { row: "launching issueStateChanged", when: "Cancelled", given: { work: "running" }, event: ev("issueStateChanged", { state: "cancelled", parent: null, at: T1 }), expect: { work: "halted", commands: ["stopWorker"], workOwn: 2 * MIN } },
  { row: "exited issueStateChanged", when: "Cancelled:", given: { work: "rest", facts: { pr: pr() } }, event: ev("issueStateChanged", { state: "cancelled", parent: null, at: T1 }), expect: { work: "halted", commands: [], workOwn: 24 * HOUR } },
  { row: "exited issueStateChanged", when: "Completed, with no PR", given: { work: "exited" }, event: ev("issueStateChanged", { state: "completed", parent: null, at: T1 }), expect: { work: "exited", commands: ["reapWorktree"] } },
  { row: "launching issueStateChanged", when: "Otherwise", given: { work: "halted", facts: { halted, issueState: cancelledIssue }, armed: own }, event: ev("issueStateChanged", { state: "open", parent: null, at: T1 }), expect: { work: "halted", commands: [] } },
  { row: "merged issueStateChanged", when: "Already going", given: { work: "merged" }, event: ev("issueStateChanged", { state: "cancelled", parent: null, at: T1 }), expect: { work: "merged", commands: [] } },
  { row: "rest prOpened", when: "Its PR opened, or reopened", given: { work: "rest", facts: { pr: pr("closed") } }, event: ev("prOpened", { pr: 7, head: SHA, repo: "repo", at: T1 }), expect: { work: "rest", commands: ["evaluatePr"], workOwn: 6 * HOUR, out: ["prRecorded"] } },
  { row: "exited prOpened", when: "Stalled", given: { work: "exited", w: { stalled: true } }, event: ev("prOpened", { pr: 7, head: SHA, repo: "repo", at: T1 }), expect: { work: "rest", workOwn: 6 * HOUR, out: ["prRecorded"] } },
  { row: "running prOpened", when: "Opened or reopened while its agent runs", given: { work: "running", facts: { prVersion: 1 } }, event: ev("prOpened", { pr: 7, head: SHA, repo: "repo", at: T1 }), expect: { work: "running", commands: [], out: ["prRecorded"] } },
  { row: "launching prOpened", when: "A fact", given: { work: "exited" }, event: ev("prOpened", { pr: 7, head: SHA, repo: "repo", at: T1 }), expect: { work: "exited", commands: [] } },
  { row: "running prPushed", when: "Its PR is open", given: { work: "rest", facts: { pr: pr() } }, event: ev("prReviewed", { pr: 7, sha: SHA, reviewer: true, copilot: false, at: T1 }), expect: { work: "rest", commands: ["evaluatePr"] } },
  { row: "running prPushed", when: "Otherwise", given: { work: "rest" }, event: ev("checksCompleted", { sha: SHA, result: "passed", at: T1 }), expect: { work: "rest", commands: [] } },
  { row: "launching prPushed", when: "`evaluatePr` is sent", given: { work: "held", facts: { pr: pr() } }, event: ev("prPushed", { pr: 7, head: SHA2, before: SHA, at: T1 }), expect: { work: "held", commands: [] } },
  { row: "halted prPushed", when: "A halted worker", given: { work: "halted", facts: { pr: pr() } }, event: ev("prPushed", { pr: 7, head: SHA2, before: SHA, at: T1 }), expect: { work: "halted", commands: [] } },
  { row: "launching ownerCommented", when: "Each comment is read", given: { work: "held", facts: { commentTold: T1 } }, event: ev("ownerCommented", { comment: B, at: T1 }), expect: { work: "held", commands: ["readComment"] } },
  { row: "halted prMerged", when: "Its stop isn't confirmed", given: { work: "halted", facts: { pr: pr(), halted }, w: { stop: owed } }, event: ev("prMerged", { pr: 7, at: T1 }), expect: { work: "merged", commands: ["stopWorker"], workOwn: 2 * MIN } },
  { row: "exited prMerged", when: "Otherwise", given: { work: "rest", facts: { pr: pr() } }, event: ev("prMerged", { pr: 7, at: T1 }), expect: { work: "merged", commands: ["reapWorktree"], workOwn: 10 * MIN } },
  { row: "idle prMerged", when: "`merged`'s deadline", given: { work: "idle", facts: { pr: pr() } }, event: ev("prMerged", { pr: 7, at: T1 }), expect: { work: "merged", commands: [], out: ["prRecorded", "slotReleased"], workOwn: 10 * MIN } },
  { row: "launching prMerged", when: "A fact", given: { work: "running", facts: { pr: pr() } }, event: ev("prMerged", { pr: 7, at: T1 }), expect: { work: "running", commands: [] } },
  { row: "merged prMerged", when: "A replay", given: { work: "merged", facts: { pr: { ...pr("merged"), at: T1 } } }, event: ev("prMerged", { pr: 7, at: T1 }), expect: { work: "merged", commands: [] } },
  { row: "rest prClosed", when: "—", given: { work: "rest", facts: { pr: pr() } }, event: ev("prClosed", { pr: 7, at: T1 }), expect: { work: "rest", commands: ["postNotice:prClosed"], workOwn: 24 * HOUR } },
  { row: "launching prClosed", when: "—", given: { work: "running", facts: { pr: pr() } }, event: ev("prClosed", { pr: 7, at: T1 }), expect: { work: "running", commands: ["postNotice:prClosed"] } },
  { row: "halted prClosed", when: "Nothing is removed", given: { work: "halted", facts: { pr: pr() } }, event: ev("prClosed", { pr: 7, at: T1 }), expect: { work: "halted", commands: [] } },
  { row: "idle agentWorking", when: "—", given: { work: "held" }, event: ev("agentWorking", {}), expect: { work: "running", workOwn: HOUR } },
  { row: "exited agentWorking", when: "An agent continued by hand", given: { work: "rest" }, event: ev("agentWorking", {}), expect: { work: "running", out: ["slotHeld"], workOwn: HOUR } },
  { row: "halted agentWorking", when: "Its stop isn't confirmed", given: { work: "halted", facts: { halted }, w: { stop: owed } }, event: ev("agentWorking", {}), expect: { work: "halted", commands: ["stopWorker"], workOwn: 2 * MIN } },
  { row: "halted agentWorking", when: "Otherwise", given: { work: "halted", facts: { halted, issueState: cancelledIssue } }, event: ev("agentWorking", {}), expect: { work: "halted", commands: [] } },
  { row: "running agentBackground", when: "A stop in the background", given: { work: "running", w: { firstTurn: NOW - MIN } }, event: ev("agentBackground", {}), expect: { work: "running", commands: [], workOwn: HOUR } },
  { row: "launching agentWorking", when: "Already running", given: { work: "running" }, event: ev("agentWorking", {}), expect: { work: "running", commands: [] } },
  { row: "merged agentWorking", when: "Otherwise: the reap waits", given: { work: "merged" }, event: ev("agentWorking", {}), expect: { work: "merged", commands: [] } },
  { row: "running agentIdle", when: "A launch's first turn not seen", given: { work: "running", w: { firstTurn: NOW - MIN } }, event: ev("agentIdle", {}), expect: { work: "running", commands: [] } },
  { row: "running agentIdle", when: "Otherwise", given: { work: "running" }, event: ev("agentIdle", {}), expect: { work: "idle", workOwn: 10.5 * MIN } },
  { row: "launching agentIdle", when: "Already idle", given: { work: "launching" }, event: ev("agentIdle", {}), expect: { work: "launching", commands: [] } },
  { row: "exited agentIdle", when: "No worker of Tolkster's", given: { work: "exited" }, event: ev("agentIdle", {}), expect: { work: "exited", commands: [] } },
  { row: "running agentWaiting", when: "—", given: { work: "idle" }, event: ev("agentWaiting", {}), expect: { work: "held", workOwn: 10 * MIN } },
  { row: "launching agentWaiting", when: "Already held", given: { work: "launching" }, event: ev("agentWaiting", {}), expect: { work: "launching", commands: [] } },
  { row: "running agentExited", when: "The work merged", given: { work: "running", facts: { pr: pr("merged") } }, event: ev("agentExited", {}), expect: { work: "merged", commands: ["reapWorktree"], workOwn: 10 * MIN } },
  { row: "running agentExited", when: "The issue completed with no PR", given: { work: "idle", facts: { issueState: { value: "completed", at: T0, changedAt: T0 } } }, event: ev("agentExited", {}), expect: { work: "rest", commands: ["reapWorktree"], workOwn: 24 * HOUR } },
  { row: "running agentExited", when: "The session that last gave", given: { work: "running", sess: { answeredThrough: T1 }, facts: { pr: pr() } }, event: ev("agentExited", {}), expect: { work: "rest", workOwn: 6 * HOUR } },
  { row: "running agentExited", when: "Not answered", given: { work: "running" }, event: ev("agentExited", {}), expect: { work: "exited", out: ["slotReleased"], workOwn: 10 * MIN } },
  { row: "merged agentExited", when: "The agent exited since", given: { work: "merged", facts: { halted }, w: { stop: owed } }, event: ev("agentExited", {}), expect: { work: "merged", commands: ["reapWorktree"], workOwn: 10 * MIN } },
  { row: "halted agentExited", when: "The agent exited since", given: { work: "halted", w: { stop: owed, cancelledMs: NOW - HOUR }, facts: { halted, issueState: cancelledIssue } }, event: ev("agentExited", {}), expect: { work: "halted", commands: [], workOwn: 23 * HOUR } },
  { row: "merged agentExited", when: "The exit came before", given: { work: "halted", facts: { halted, agent: { status: "exited", sinceMs: NOW - 2 * MIN } }, w: { stop: owed }, armed: own }, event: ev("agentExited", {}), expect: { work: "halted", commands: [] } },
  { row: "merged agentExited", when: "Otherwise", given: { work: "merged" }, event: ev("agentExited", {}), expect: { work: "merged", commands: ["reapWorktree"], workOwn: 10 * MIN } },
  { row: "halted agentExited", when: "Otherwise: the close", given: { work: "halted", facts: { halted }, armed: own }, event: ev("agentExited", {}), expect: { work: "halted", commands: [] } },
  { row: "launching agentExited", when: "The command's result decides", given: { work: "launching", w: { launch: "resume" } }, event: ev("agentExited", {}), expect: { work: "launching", commands: [] } },
  { row: "exited agentExited", when: "A replay", given: { work: "exited" }, event: ev("agentExited", {}), expect: { work: "exited", commands: [] } },
  { row: "launching worktreeGone", when: "Removed by hand", given: { work: "idle" }, event: ev("worktreeGone", {}), expect: { work: "reaped", out: ["slotReleased"], workOwn: null } },
  { row: "reaped worktreeGone", when: "A replay", given: { work: "reaped" }, event: ev("worktreeGone", {}), expect: { work: "reaped", commands: [] } },
  { row: "exited workerAnswered", when: "A late answer", given: { work: "exited", w: { stalled: true }, sess: {} }, event: ev("workerAnswered", { session: S, reader: "worker", at: T1 }), expect: { work: "rest", workOwn: 24 * HOUR } },
  { row: "launching workerAnswered", when: "A fact", given: { work: "running", sess: {} }, event: ev("workerAnswered", { session: S, reader: "worker", at: T1 }), expect: { work: "running", commands: [] } },
  { row: "launching deadline", when: "`takeBack`, with a take-back owed", given: { work: "rest", facts: { takeBack: { run: "r1" } }, armed: [{ machine: "work", kind: "takeBack" }] }, event: (s) => deadline(s, "work", "takeBack"), expect: { work: "rest", commands: ["takeBackChain"] } },
  { row: "running deadline", when: "`recheck`", given: { work: "rest", facts: { pr: pr() }, armed: [{ machine: "work", kind: "recheck" }] }, event: (s) => deadline(s, "work", "recheck"), expect: { work: "rest", commands: ["evaluatePr"] } },
  { row: "launching deadline", when: "`pending`, its PR open", given: { work: "exited", w: { stalled: true }, facts: { pr: pr() }, armed: [{ machine: "work", kind: "pending" }] }, event: (s) => deadline(s, "work", "pending"), expect: { work: "exited", commands: ["evaluatePr"] } },
  { row: "launching deadline", when: "A start's own: none is set", given: { work: "launching" }, event: (s) => deadline(s, "work"), expect: { work: "launching", commands: [] } },
  { row: "launching deadline", when: "A resume's: no result came", given: { work: "launching", w: { launch: "resume" }, armed: own }, event: (s) => deadline(s, "work"), expect: { work: "rest", commands: ["postNotice:couldntPass"], out: ["slotReleased"], workOwn: 24 * HOUR } },
  { row: "launching deadline", when: "A round's: no result came", given: { work: "launching", w: { launch: "round" }, facts: { pr: pr() }, armed: own }, event: (s) => deadline(s, "work"), expect: { work: "rest", workOwn: 6 * HOUR } },
  { row: "running deadline", when: "Its own, with a launch's first turn never seen and the work merged", given: { work: "running", facts: { pr: pr("merged") }, w: { firstTurn: NOW - 5 * MIN }, armed: own }, event: (s) => deadline(s, "work"), expect: { work: "merged", commands: [], workOwn: 10 * MIN } },
  { row: "running deadline", when: "Its own, with a launch's first turn never seen:", given: { work: "running", w: { firstTurn: NOW - 5 * MIN, requestedBy: { session: S, at: T1 } }, armed: own }, event: (s) => deadline(s, "work"), expect: { work: "exited", commands: ["closeAgent", "postNotice:stalled"], out: ["slotReleased"], workOwn: 24 * HOUR } },
  { row: "running deadline", when: "Its own, otherwise", given: { work: "running", armed: own }, event: (s) => deadline(s, "work"), expect: { work: "running", commands: ["observe", "readIssue"], workOwn: HOUR } },
  { row: "idle deadline", when: "The work merged", given: { work: "held", facts: { pr: pr("merged") }, armed: own }, event: (s) => deadline(s, "work"), expect: { work: "merged", workOwn: 10 * MIN } },
  { row: "idle deadline", when: "Not told yet", given: { work: "idle", armed: own }, event: (s) => deadline(s, "work"), expect: { work: "idle", commands: ["postNotice:paused"], workOwn: HOUR } },
  { row: "held deadline", when: "Not told yet", given: { work: "held", armed: own }, event: (s) => deadline(s, "work"), expect: { work: "held", commands: ["postNotice:heldAtPrompt"], workOwn: HOUR } },
  { row: "idle deadline", when: "Told already", given: { work: "idle", facts: { notices: [`paused:${NOW - HOUR}`] }, armed: own }, event: (s) => deadline(s, "work"), expect: { work: "idle", commands: ["observe", "readIssue"], workOwn: HOUR } },
  { row: "exited deadline", when: "Its own, not stalled yet", given: { work: "exited", armed: own }, event: (s) => deadline(s, "work"), expect: { work: "exited", commands: ["postNotice:stalled"], workOwn: 24 * HOUR } },
  { row: "exited deadline", when: "Its own, stalled", given: { work: "exited", w: { stalled: true }, facts: { pr: pr() }, armed: own }, event: (s) => deadline(s, "work"), expect: { work: "exited", commands: ["evaluatePr", "readIssue"], workOwn: 24 * HOUR } },
  { row: "rest deadline", when: "Its own", given: { work: "rest", facts: { pr: pr() }, armed: own }, event: (s) => deadline(s, "work"), expect: { work: "rest", commands: ["evaluatePr", "readIssue"], workOwn: 6 * HOUR } },
  { row: "halted deadline", when: "Its stop isn't confirmed", given: { work: "halted", facts: { halted, issueState: cancelledIssue }, w: { stop: owed }, armed: own }, event: (s) => deadline(s, "work"), expect: { work: "halted", commands: ["stopWorker"], workOwn: 2 * MIN } },
  { row: "halted deadline", when: "The issue cancelled, no PR open", given: { work: "halted", facts: { halted, issueState: cancelledIssue }, armed: own }, event: (s) => deadline(s, "work"), expect: { work: "halted", commands: ["reapWorktree"], workOwn: 24 * HOUR } },
  { row: "halted deadline", when: "The issue cancelled, a PR open", given: { work: "halted", facts: { halted, issueState: cancelledIssue, pr: pr() }, armed: own }, event: (s) => deadline(s, "work"), expect: { work: "halted", commands: ["evaluatePr"], workOwn: 24 * HOUR } },
  { row: "halted deadline", when: "Otherwise: a lost cancel", given: { work: "halted", facts: { halted, pr: pr() }, armed: own }, event: (s) => deadline(s, "work"), expect: { work: "halted", commands: ["evaluatePr", "readIssue"], workOwn: 24 * HOUR } },
  { row: "merged deadline", when: "Otherwise: an agent idle", given: { work: "merged", facts: { agent: { status: "idle", sinceMs: NOW - 11 * MIN } }, armed: own }, event: (s) => deadline(s, "work"), expect: { work: "merged", commands: ["closeAgent"], workOwn: 10 * MIN } },
  { row: "merged deadline", when: "An agent at work", given: { work: "merged", facts: { agent: { status: "working", sinceMs: NOW - 11 * MIN } }, armed: own }, event: (s) => deadline(s, "work"), expect: { work: "merged", commands: [], workOwn: 10 * MIN } },
  { row: "merged deadline", when: "No agent", given: { work: "merged", armed: own }, event: (s) => deadline(s, "work"), expect: { work: "merged", commands: ["reapWorktree"], workOwn: 10 * MIN } },
  { row: "reaped deadline", when: "Otherwise", given: { work: "reaped" }, event: (s) => deadline(s, "work"), expect: { work: "reaped", commands: [] } },
  { row: "launching commandResult", when: "`startWorker` named no worktree and found one with no agent", given: { work: "launching", facts: { worktree: null }, w: { neverBegan: true }, queue: [{ kind: "startWorker", machine: "work", params: { session: S, orchestrate: false, worktree: null } }] }, event: result(`${ISSUE}.1`, "ok", { worktree: { id: "repo::C:/w/tw-1", branch: "tw-1" }, madeWorktree: false, launched: false }), expect: { work: "launching", commands: ["startWorker"] } },
  { row: "launching commandResult", when: "`startWorker` succeeded", given: { work: "launching", queue: [{ kind: "startWorker", machine: "work" }] }, event: result(`${ISSUE}.1`, "ok", { worktree: { id: "repo::C:/w/tw-1", branch: "tw-1" } }), expect: { work: "running", workOwn: HOUR } },
  { row: "launching commandResult", when: "`startWorker` `lost`", given: { work: "launching", queue: [{ kind: "startWorker", machine: "work" }] }, event: result(`${ISSUE}.1`, "lost"), expect: { work: "exited", commands: ["postNotice:stalled"], workOwn: 24 * HOUR } },
  { row: "launching commandResult", when: "`startWorker` failed after", given: { work: "launching", queue: [{ kind: "startWorker", machine: "work" }] }, event: result(`${ISSUE}.1`, "failed", { madeWorktree: true }), expect: { work: "exited", commands: ["postNotice:stalled"], workOwn: 24 * HOUR } },
  { row: "launching commandResult", when: "`startWorker` failed", given: { work: "launching", facts: { worktree: null }, queue: [{ kind: "startWorker", machine: "work" }] }, event: result(`${ISSUE}.1`, "failed", { madeWorktree: false }), expect: { work: "reaped", commands: ["postNotice:stalled"], workOwn: null } },
  { row: "launching commandResult", when: "A resume's or round's command succeeded", given: { work: "launching", w: { launch: "round" }, queue: [{ kind: "startRound", machine: "work" }] }, event: result(`${ISSUE}.1`, "ok"), expect: { work: "running", workOwn: HOUR } },
  { row: "launching commandResult", when: "`resumeWorker` failed", given: { work: "launching", w: { launch: "resume" }, queue: [{ kind: "resumeWorker", machine: "work" }] }, event: result(`${ISSUE}.1`, "failed"), expect: { work: "rest", commands: ["postNotice:couldntPass"], workOwn: 24 * HOUR } },
  { row: "launching commandResult", when: "`startRound` failed with its line typed", given: { work: "launching", facts: { pr: pr() }, w: { launch: "round", inRound: SHA }, queue: [{ kind: "startRound", machine: "work" }] }, event: result(`${ISSUE}.1`, "failed", { typed: true }), expect: { work: "running", workOwn: HOUR, commands: [] } },
  { row: "launching commandResult", when: "`startRound` failed with no line typed: its proposal is dropped, so nothing is marked or counted", given: { work: "launching", facts: { pr: pr() }, w: { launch: "round" }, queue: [{ kind: "startRound", machine: "work" }] }, event: result(`${ISSUE}.1`, "failed"), expect: { work: "rest", workOwn: 10 * MIN, commands: [] } },
  { row: "launching commandResult", when: "`startRound` lost", given: { work: "launching", facts: { pr: pr() }, w: { launch: "round" }, queue: [{ kind: "startRound", machine: "work" }] }, event: result(`${ISSUE}.1`, "lost"), expect: { work: "rest", workOwn: 6 * HOUR, commands: ["evaluatePr"] } },
  { row: "launching commandResult", when: "`readComment`: a reply to a note", given: { work: "reaped", queue: [{ kind: "readComment", machine: "work", params: { comment: B, at: fresh } }] }, event: result(`${ISSUE}.1`, "ok", { comment: { kind: "noteReply", newestOwnerAt: fresh, state: "open", root: S } }), expect: { req: "requested", work: "reaped", commands: ["readIssue"], reqOwn: 15 * MIN } },
  { row: "launching commandResult", when: "`readComment` failed", given: { work: "running", queue: [{ kind: "readComment", machine: "work", params: { comment: B, at: fresh } }] }, event: result(`${ISSUE}.1`, "failed"), expect: { work: "running", commands: ["readComment"] } },
  { row: "running commandResult", when: "`readComment`: a plain comment", given: { work: "running", queue: [{ kind: "readComment", machine: "work" }] }, event: result(`${ISSUE}.1`, "ok", { comment: { kind: "plain", newestOwnerAt: T1, state: "open" } }), expect: { work: "running", commands: ["deliverMessage"] } },
  { row: "running commandResult", when: "`deliverMessage` of a comment's line succeeded", given: { work: "idle", queue: [{ kind: "deliverMessage", machine: "work", params: { comment: true, through: T1 } }] }, event: result(`${ISSUE}.1`, "ok"), expect: { work: "idle", commands: [] } },
  { row: "running commandResult", when: "`deliverMessage` or `startRound` held", given: { work: "running", queue: [{ kind: "deliverMessage", machine: "work" }] }, event: result(`${ISSUE}.1`, "held"), expect: { work: "held", workOwn: 10 * MIN } },
  { row: "running commandResult", when: "`deliverMessage` failed", given: { work: "idle", queue: [{ kind: "deliverMessage", machine: "work" }] }, event: result(`${ISSUE}.1`, "failed"), expect: { work: "idle", commands: ["observe"] } },
  { row: "running commandResult", when: "`evaluatePr`, and the issue no longer", given: { work: "rest", facts: { pr: pr(), delegate: { value: null, at: T0 } }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ feedback: { at: T1, reviewerAt: T1 } })), expect: { work: "rest", commands: [] } },
  { row: "exited commandResult", when: "`evaluatePr`: `BEHIND` on a base newer", given: { work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ mergeState: "BEHIND" })), expect: { work: "rest", commands: ["updateBranch"] } },
  { row: "exited commandResult", when: "`updateBranch` `refused` at the head", given: { work: "rest", facts: { pr: pr() }, queue: [updating()] }, event: result(`${ISSUE}.1`, "failed", { refused: true }), expect: { work: "launching", commands: ["startRound"], out: ["slotHeld"], workOwn: 10 * MIN } },
  { row: "exited commandResult", when: "`updateBranch` failed otherwise, or lost", given: { work: "exited", facts: { pr: pr() }, queue: [updating()] }, event: result(`${ISSUE}.1`, "lost"), expect: { work: "exited", commands: [], recheck: true } },
  { row: "launching commandResult", when: "`updateBranch` succeeded", given: { work: "exited", facts: { pr: pr() }, queue: [updating()] }, event: result(`${ISSUE}.1`, "ok"), expect: { work: "exited", commands: [] } },
  { row: "exited commandResult", when: "`evaluatePr`: feedback", given: { work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ feedback: { at: T1, reviewerAt: null } })), expect: { work: "launching", commands: ["startRound"], out: ["slotHeld"], workOwn: 10 * MIN } },
  { row: "running commandResult", when: "`evaluatePr`: the same", given: { work: "running", facts: { pr: pr() }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ mergeState: "DIRTY" })), expect: { work: "running", commands: ["startRound"] } },
  { row: "running commandResult", when: "`evaluatePr`: feedback newer than the last round, on the head a round", given: { work: "running", facts: { pr: pr(), rounds: { total: 1, byHead: { [SHA]: 1 }, lastAt: T0, lastBase: null, lastFailed: null } }, w: { inRound: SHA }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ feedback: { at: T1, reviewerAt: null } })), expect: { work: "running", commands: [] } },
  { row: "running commandResult", when: "`evaluatePr`: a failed check", given: { work: "running", facts: { pr: pr() }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ checks: "failed" })), expect: { work: "running", commands: [] } },
  { row: "running commandResult", when: "`evaluatePr`: Copilot's feedback at the cap", given: { work: "exited", facts: { pr: pr(), rounds: { total: 3, byHead: {}, lastAt: T0, lastBase: null, lastFailed: null } }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ feedback: { at: T1, reviewerAt: null } })), expect: { work: "exited", commands: ["postNotice:roundsCap"] } },
  { row: "exited commandResult", when: "`evaluatePr`: auto-merge turned off", given: { work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ autoMergeOff: true })), expect: { work: "rest", commands: ["postNotice:autoMergeOff"] } },
  { row: "running commandResult", when: "`evaluatePr`: CI couldn't run", given: { work: "idle", facts: { pr: pr() }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ checks: "couldntRun" })), expect: { work: "idle", commands: ["postNotice:ciCouldntRun"] } },
  { row: "running commandResult", when: "`evaluatePr`: the owed `pending` read", given: { work: "exited", facts: { pr: pr() }, w: { pendingDue: SHA }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ checks: "pending" })), expect: { work: "exited", commands: ["postNotice:checksPending"] } },
  { row: "exited commandResult", when: "`evaluatePr`: Copilot's newest review only commented on the head", given: { work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ copilotWontApprove: true })), expect: { work: "rest", commands: ["postNotice:copilotWontApprove"] } },
  { row: "idle commandResult", when: "`evaluatePr`: GitHub's own `CLEAN`", given: { work: "rest", facts: { pr: pr() }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ autoMerge: true, mergeable: true })), expect: { work: "rest", commands: ["mergePr"] } },
  { row: "running commandResult", when: "`mergePr` failed, not `refused`", given: { work: "exited", facts: { pr: pr() }, queue: [{ kind: "mergePr", machine: "work", params: { head: SHA } }] }, event: result(`${ISSUE}.1`, "lost"), expect: { work: "exited", commands: [], recheck: true } },
  { row: "running commandResult", when: "`evaluatePr`: merge state `UNKNOWN`", given: { work: "rest", facts: { pr: pr(), base: SHA2 }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ mergeState: "UNKNOWN" })), expect: { work: "rest", commands: [], recheck: true } },
  { row: "halted commandResult", when: "The stop owed succeeded", given: { work: "halted", facts: { halted }, w: { stop: owed }, queue: [{ kind: "stopWorker", machine: "work" }] }, event: result(`${ISSUE}.1`, "ok"), expect: { work: "halted", commands: [], workOwn: 24 * HOUR } },
  { row: "merged commandResult", when: "The stop owed succeeded", given: { work: "merged", facts: { halted }, w: { stop: owed }, queue: [{ kind: "stopWorker", machine: "work" }] }, event: result(`${ISSUE}.1`, "ok"), expect: { work: "merged", commands: ["reapWorktree"], workOwn: 10 * MIN } },
  { row: "halted commandResult", when: "The stop owed failed, its agent without a handle", given: { work: "merged", facts: { halted }, w: { stop: owed, requestedBy: { session: S, at: T1 } }, queue: [{ kind: "stopWorker", machine: "work" }], armed: [{ machine: "work", kind: "own", dueMs: NOW + 2 * MIN }] }, event: result(`${ISSUE}.1`, "failed", { unnamed: true }), expect: { work: "merged", commands: ["postNotice:stopUnnamed"], workOwn: 2 * MIN } },
  { row: "halted commandResult", when: "A start", given: { work: "halted", facts: { halted }, queue: [{ kind: "resumeWorker", machine: "work" }] }, event: result(`${ISSUE}.1`, "ok"), expect: { work: "halted", commands: ["stopWorker"], workOwn: 2 * MIN } },
  { row: "exited commandResult", when: "`reapWorktree` reaped", given: { work: "rest", facts: { pr: pr() }, queue: [{ kind: "reapWorktree", machine: "work" }] }, event: result(`${ISSUE}.1`, "ok", { reaped: true }), expect: { work: "reaped", workOwn: null } },
  { row: "launching commandResult", when: "`takeBackChain` succeeded", given: { work: "running", facts: { takeBack: { run: "r1" } }, armed: [{ machine: "work", kind: "takeBack" }], queue: [{ kind: "takeBackChain", machine: "work", params: { run: "r1" } }] }, event: result(`${ISSUE}.1`, "ok"), expect: { work: "running", commands: [] } },
  { row: "running commandResult", when: "`evaluatePr` shows the PR merged", given: { work: "halted", facts: { halted, issueState: cancelledIssue, pr: pr() }, queue: [evaluatePr] }, event: result(`${ISSUE}.1`, "ok", evaluated({ status: "merged", at: T1 })), expect: { work: "merged", commands: ["reapWorktree"], workOwn: 10 * MIN } },
  { row: "running commandResult", when: "`readIssue`: applied as", given: { work: "running", queue: [{ kind: "readIssue", machine: "work" }] }, event: result(`${ISSUE}.1`, "ok", readOf({ state: "cancelled" })), expect: { work: "halted", commands: ["stopWorker"], workOwn: 2 * MIN } },
  { row: "launching commandResult", when: "Otherwise", given: { work: "running", queue: [{ kind: "readIssue", machine: "work" }] }, event: result(`${ISSUE}.1`, "ok", readOf()), expect: { work: "running", commands: [] } },
];

test("every work transition in the doc has its case, in order", () => {
  const rows = rowsOf("## Work transitions");
  assert.equal(cases.length, rows.length, "one case per row");
  rows.forEach((r, i) => {
    assert.equal(cases[i].row, r.row, `row ${i + 1}`);
    assert.ok(r.when.startsWith(cases[i].when), `row ${i + 1}: "${r.when}" starts "${cases[i].when}"`);
  });
});

for (const c of cases) test(`work: ${c.row}, ${c.when}`, () => check(c));

// A turn ended to wait on a call in the background is working: no `idle`, so no paused deadline.
test("an agent waiting on a call in the background from idle is running", () => {
  check({ row: "idle agentBackground", when: "—", given: { work: "idle" }, event: ev("agentBackground", {}), expect: { work: "running", workOwn: HOUR } });
});

test("a held worker's notice to another claim on it leaves the hour's observe and read to the requester's", () =>
  check({ row: "idle deadline", when: "Told already", given: { work: "held", other: "claimed", facts: { notices: [`heldAtPrompt:${NOW - HOUR}`] }, armed: own }, event: (s) => deadline(s, "work"), expect: { work: "held", commands: ["postNotice:heldAtPrompt", "observe", "readIssue"], workOwn: HOUR } }));

// A landing is the work's merge with no PR: its reap goes on its own rule, the PR held goes from
// the store and the open-PR index, and a late event of that PR, or a later landing, moves nothing.
test("a landing reaps on its own rule and drops the PR held, whose late events are stale", () => {
  const landed = ev("landed", { session: S, head: SHA2 });
  const o = apply(given({ work: "rest", facts: { pr: pr() } }), landed);
  assert.deepEqual([o.work, o.commands, o.out, o.store.facts.pr], ["merged", ["reapWorktree"], ["prRecorded"], null]);
  const reap = o.store.queue.find((c) => c.kind === "reapWorktree")!;
  assert.deepEqual(toWire(reap, o.store, "TW-1").args.rule, { kind: "landed" });
  for (const late of [landed, ev("landed", { session: S, head: SHA }), ev("prMerged", { pr: 7, at: T1 }), ev("prClosed", { pr: 7, at: T1 }), ev("prOpened", { pr: 7, head: SHA, repo: "repo", at: T1 })]) {
    const after = apply(o.store, late);
    assert.deepEqual([after.work, after.commands, after.out, after.store.facts.pr], ["merged", [], [], null], late.kind);
  }
  // A landing after a PR's merge is the merge reaped; one after the reap is a worker's gone since.
  const prMerged = apply(given({ work: "merged", facts: { pr: pr("merged") } }), landed);
  assert.deepEqual([prMerged.store.facts.landed, prMerged.store.facts.pr], [{ head: SHA2 }, null]);
  const gone = apply(given({ work: "reaped", facts: { worktree: null, pr: pr() } }), landed);
  assert.deepEqual([gone.store.facts.landed, gone.store.facts.pr?.number], [undefined, 7]);
});

test("a worker given another request after its landing may push again: the landing goes", () => {
  const landed = apply(given({ work: "running" }), ev("landed", { session: S, head: SHA2 }));
  const asked = apply(landed.store, requested());
  assert.deepEqual([asked.work, asked.commands, asked.store.facts.landed], ["running", ["deliverMessage"], undefined]);
  assert.equal(apply(asked.store, ev("prOpened", { pr: 8, head: SHA, repo: "repo", at: T1 })).store.facts.pr?.number, 8);
  const delivered = apply(asked.store, result(asked.store.queue.find((c) => c.kind === "deliverMessage")!.id, "ok"));
  assert.deepEqual(apply(delivered.store, ev("agentExited", {})).work, "exited");
});

test("a running worker's landing is reaped once it exits, and a new worker's PR counts again", () => {
  const running = apply(given({ work: "running" }), ev("landed", { session: S, head: SHA2 }));
  assert.deepEqual([running.work, running.commands], ["running", []]);
  const exited = apply(running.store, ev("agentExited", {}));
  assert.deepEqual([exited.work, exited.commands], ["merged", ["reapWorktree"]]);
  const reaped = apply(exited.store, result(exited.store.queue.find((c) => c.kind === "reapWorktree")!.id, "ok", { reaped: true }));
  assert.equal(reaped.work, "reaped");
  const started = apply(reaped.store, requested());
  assert.deepEqual([started.work, started.store.facts.landed], ["launching", undefined]);
  assert.equal(apply(started.store, ev("prOpened", { pr: 8, head: SHA, repo: "repo", at: T1 })).store.facts.pr?.number, 8);
});
