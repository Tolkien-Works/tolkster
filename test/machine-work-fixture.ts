import type { Store } from "../src/worker/machine/types.ts";
import { apply, ev, HOUR, ISSUE, MIN, NOW, result, S, SHA, T0, T1, type Given } from "./machine-fixture.ts";

// What the work machine's tests share: the transition table's (machine-work.test.ts) and
// the topics' beside it (machine-work-*.test.ts).

// A reply under a week old at `NOW`.
export const fresh = new Date(NOW - HOUR).toISOString();
export const SHA2 = "fedcba9876543210fedcba9876543210fedcba98";
export const LATER = new Date(Date.parse(T1) + MIN).toISOString();
export const pr = (status: "open" | "merged" | "closed" = "open") => ({ number: 7, head: SHA, repo: "repo", status, at: T0, receivedMs: NOW - HOUR });
export const requested = (request: "prompt" | "delegation" = "prompt") => ev("workRequested", { session: S, at: T1, request, orchestrate: false });
export const halted = { at: T1, by: "stop" as const };
export const cancelledIssue = { value: "cancelled" as const, at: T0, changedAt: T0 };
// The stop owed, sent a minute ago as the store's first command.
export const owed = { command: `${ISSUE}.1`, sentMs: NOW - MIN, since: NOW - MIN };
export const own: Given["armed"] = [{ machine: "work", kind: "own" }];
export const evaluated = (over: object = {}) => ({ pr: { number: 7, status: "open", at: T0, head: SHA, mergeState: "CLEAN", base: SHA2, autoMergeOff: false, manualMerge: false, checks: "passed", feedback: null, copilotWontApprove: false, maxRounds: 3, ...over } });
export const evaluatePr = { kind: "evaluatePr" as const, machine: "work", params: { version: 0 } };
// The branch update a read of `evaluated`'s base sends for the held head.
export const updating = (over: object = {}) => ({ kind: "updateBranch" as const, machine: "work", params: { head: SHA, base: SHA2, ...over } });
// `rounds` once the round the store has in flight lands: a round is counted by its result.
export const landed = (s: Store) => apply(s, result(s.queue.find((c) => c.kind === "startRound")!.id, "ok")).store.facts.rounds;
export const readOf = (over: object = {}) => ({ read: { updatedAt: T1, delegate: "owner", state: "open", stateChangedAt: T0, priority: 0, blockers: [], moreBlockers: false, parent: null, children: [], moreChildren: false, repo: "repo", sessions: [], ...over } });
