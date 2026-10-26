// What a store keeps of records that only the current head, request or pause can use
// (docs/state-machine.md, "Stores"). It runs as every step ends (`finish`, step.ts), on the
// store the step is about to write.

import type { Store } from "./types.ts";

// Notices keyed by the PR and its head: `<number>:<head>`.
const HEAD_NOTICES = ["roundsCap", "prClosed", "autoMergeOff", "ciCouldntRun", "checksPending", "copilotWontApprove", "roundUnnamed"];
// Notices keyed by when the worker went idle or held, in milliseconds, then perhaps a session.
const PAUSE_NOTICES = ["paused", "heldAtPrompt"];
// A session's notices are keyed `<session>:<its request's time>`, then perhaps more.
const REQUEST_KEY = /^([^:]+):(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z)(?::|$)/;

// Drops the per-head facts of every head but the PR's current one, and the notices of a
// request, head or pause the store has moved past. None of it is read to decide anything:
// a fact only tells a newer event from a replay, and a notice only that it was said. A key
// whose kind or shape isn't known here is kept, so a notice is never said twice for being
// misread. A key dropped can't come back: a request, pause or push is newer than the one
// before it, and a head returning says its notice once more.
export function pruneStore(s: Store): void {
  const f = s.facts;
  if (f.pr !== null) {
    const head = f.pr.head;
    f.checks = onlyHead(f.checks, head);
    f.feedback = onlyHead(f.feedback, head);
    f.rounds.byHead = onlyHead(f.rounds.byHead, head);
  }
  f.notices = f.notices.filter((full) => !superseded(s, full));
}

function onlyHead<T>(held: Record<string, T>, head: string): Record<string, T> {
  return Object.hasOwn(held, head) ? { [head]: held[head] } : {};
}

function superseded(s: Store, full: string): boolean {
  const split = full.indexOf(":");
  const kind = full.slice(0, split);
  const key = full.slice(split + 1);
  if (HEAD_NOTICES.includes(kind)) return s.facts.pr !== null && key !== `${s.facts.pr.number}:${s.facts.pr.head}`;
  if (PAUSE_NOTICES.includes(kind)) {
    const since = s.work?.pausedSinceMs ?? null;
    return since !== null && Number(key.split(":")[0]) < since;
  }
  const match = REQUEST_KEY.exec(key);
  if (match === null) return false;
  const [, session, at] = match;
  const request = s.sessions[session]?.facts.request?.at ?? null;
  if (request === null || at >= request) return false;
  // The worker's notice for the request it was last given still stands, whatever its session
  // has been asked since.
  const by = s.work?.requestedBy;
  return !(by?.session === session && by.at === at);
}
