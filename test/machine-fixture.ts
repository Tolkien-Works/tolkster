// Builds an issue's store in a given state, applies one event, and reports what the
// transition did, for the transition tables' tests (machine-requests.test.ts and
// machine-work*.test.ts).

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { newSession, newStore, newWork } from "../src/worker/machine/ctx.ts";
import { receive } from "../src/worker/machine/step.ts";
import type { Command, DeadlineKind, Event, IssueFacts, IssueRead, RequestState, SessionFacts, SessionKind, StopOwed, Store, WorkMachine, WorkState } from "../src/worker/machine/types.ts";

export const NOW = 1_800_000_000_000;
export const MIN = 60_000;
export const HOUR = 60 * MIN;
export const ISSUE = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
export const S = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
export const S2 = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
export const B = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
export const B2 = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee";
export const P = "ffffffff-ffff-4fff-8fff-ffffffffffff";
export const T0 = "2026-10-01T00:00:00.000Z";
export const T1 = "2026-10-01T01:00:00.000Z";
export const T2 = "2026-10-01T02:00:00.000Z";
export const SHA = "0123456789abcdef0123456789abcdef01234567";

export type Given = {
  req?: RequestState;
  work?: WorkState;
  sess?: Partial<SessionFacts>;
  facts?: Partial<IssueFacts>;
  w?: Partial<WorkMachine>;
  // The request machine's stop owed.
  reqStop?: StopOwed;
  // Commands already queued, in flight unless `sentMs` is null.
  queue?: Partial<Command>[];
  // Deadlines armed at the machine's version: which kinds, on which machine.
  armed?: { machine: "work" | "request"; kind: DeadlineKind; dueMs?: number }[];
  // The store found `listen` gone with a command in flight.
  away?: true;
  // Another session on the issue, in a state.
  other?: RequestState;
  // Session S's kind, as its `sessionStarted` said it.
  kind?: SessionKind;
};

export function given(g: Given = {}): Store {
  const s = newStore(ISSUE);
  s.away = g.away === true;
  s.facts = {
    ...s.facts,
    delegate: { value: "owner", at: T0 },
    everDelegated: true,
    issueState: { value: "open", at: T0, changedAt: T0 },
    priority: { value: 0, at: T0 },
    repo: { value: "repo", at: T0 },
    blockers: { more: false, sentMs: NOW - HOUR, ids: [], states: {} },
    children: { sentMs: NOW - HOUR, ids: [], states: {}, more: false },
    sessions: { [S]: { kind: g.kind ?? "delegation", createdAt: T0 } },
    worktree: { id: "repo::C:/w/tw-1", branch: "tw-1", version: 1 },
    ...g.facts,
  };
  if (g.req !== undefined || g.sess !== undefined) {
    const sess = newSession();
    sess.facts = { ...sess.facts, byOwner: true, request: { kind: "delegation", at: T0 }, ...g.sess };
    sess.machine = g.req === undefined ? null : { state: g.req, version: 1, deadlines: [], stop: g.reqStop ?? null };
    s.sessions[S] = sess;
    // Its `sessionStarted` has been merged, unless the test's sessions leave it out.
    if (s.facts.sessions[S] !== undefined) s.facts.sessions[S] = { ...s.facts.sessions[S], started: true };
  }
  if (g.other !== undefined) {
    const sess = newSession();
    sess.facts = { ...sess.facts, byOwner: true, request: { kind: "start", at: T0 } };
    s.facts.sessions[S2] ??= { kind: "mention", createdAt: T0, started: true };
    sess.machine = { state: g.other, version: 1, deadlines: [], stop: null };
    s.sessions[S2] = sess;
  }
  if (g.work !== undefined) {
    // `launching` is a start's unless `w` names another launch.
    s.work = { ...newWork(g.work, 1), launch: g.work === "launching" ? "start" : null, requestedBy: { session: S, at: T0 }, pausedSinceMs: NOW - HOUR, ...g.w };
  }
  for (const a of g.armed ?? []) {
    const m = a.machine === "work" ? s.work! : s.sessions[S].machine!;
    m.deadlines.push({ kind: a.kind, version: m.version, dueMs: a.dueMs ?? NOW });
  }
  for (const q of g.queue ?? []) {
    s.commandSeq += 1;
    s.queue.push({ id: `${ISSUE}.${s.commandSeq}`, issue: ISSUE, kind: "readIssue", machine: S, version: 1, params: {}, class1: false, firstSentMs: NOW - MIN, sentMs: NOW - MIN, sends: 1, superseded: false, ...q });
  }
  return s;
}

export type Outcome = {
  store: Store;
  // The request machine's (session S's) and the work machine's state after.
  req: RequestState | undefined;
  work: WorkState | undefined;
  // New commands, a notice as `postNotice:<kind>`.
  commands: string[];
  // New events leaving the store, by kind.
  out: string[];
  // The machine's own deadline, as time from now, or null when none is set.
  reqOwn: number | null;
  workOwn: number | null;
  recheck: boolean;
  // The request machine's `quiet` and `warm` deadlines, as time from now, or null when none is set.
  quiet: number | null;
  warm: number | null;
};

export function apply(store: Store, e: Event): Outcome {
  const known = new Set(store.queue.map((c) => c.id));
  const sentOut = store.outbox.length;
  const { store: s } = receive(store, e, NOW);
  const commands = s.queue.filter((c) => !known.has(c.id)).map((c) => (c.kind === "postNotice" ? `postNotice:${c.params.notice}` : c.kind));
  const own = (m: { deadlines: { kind: string; dueMs: number }[] } | null | undefined, kind = "own") => {
    const d = m?.deadlines.find((x) => x.kind === kind);
    return d === undefined ? null : d.dueMs - NOW;
  };
  return {
    store: s,
    req: s.sessions[S]?.machine?.state,
    work: s.work?.state,
    commands,
    out: s.outbox.slice(sentOut).map((o) => o.event.kind),
    reqOwn: own(s.sessions[S]?.machine),
    workOwn: own(s.work),
    recheck: s.work?.deadlines.some((d) => d.kind === "recheck") ?? false,
    quiet: own(s.sessions[S]?.machine, "quiet"),
    warm: own(s.sessions[S]?.machine, "warm"),
  };
}

// An event on the issue, its required fields filled in.
// The fields of an event of `kind`, over each of its shapes.
type Fields<E> = E extends unknown ? Omit<E, "kind" | "id" | "issue"> : never;

export function ev<K extends Event["kind"]>(kind: K, fields: Fields<Extract<Event, { kind: K }>> | Record<string, never> = {}): Event {
  return { kind, id: `${kind}-delivery`, issue: ISSUE, ...fields } as Event;
}

export function result(command: string, outcome: "ok" | "failed" | "held" | "gone" | "lost" | "retryable", data?: object): Event {
  return { kind: "commandResult", id: command, issue: ISSUE, command, result: outcome, data } as Event;
}

// A work machine's readIssue result landing on the store, the read first sent at `sentMs`.
export function landRead(store: Store, read: IssueRead, sentMs = NOW - MIN): Outcome {
  const s = structuredClone(store);
  s.commandSeq += 1;
  const id = `${s.issue}.${s.commandSeq}`;
  s.queue.push({ id, issue: s.issue, kind: "readIssue", machine: "work", version: s.work?.version ?? 1, params: {}, class1: false, firstSentMs: sentMs, sentMs, sends: 1, superseded: false });
  return apply(s, result(id, "ok", { read }));
}

export function deadline(s: Store, machine: "work" | "request", kind: DeadlineKind = "own"): Event {
  const m = machine === "work" ? s.work! : s.sessions[S].machine!;
  const d = m.deadlines.find((x) => x.kind === kind);
  return { kind: "deadline", id: "deadline", issue: ISSUE, machine: machine === "work" ? "work" : S, version: d?.version ?? m.version, deadline: kind };
}

export type Expect = Partial<Pick<Outcome, "req" | "work" | "commands" | "out" | "reqOwn" | "workOwn" | "recheck" | "quiet" | "warm">>;
export type Case = { row: string; when: string; given: Given; event: Event | ((s: Store) => Event); expect: Expect };

export function rowsOf(heading: string): { row: string; when: string }[] {
  const doc = readFileSync(new URL("../docs/state-machine.md", import.meta.url), "utf8").replace(/\r\n/g, "\n");
  const start = doc.indexOf(`\n${heading}\n`);
  const rest = doc.slice(start + heading.length + 2);
  const body = rest.slice(0, rest.search(/^## /m));
  const first = (cell: string) => cell.match(/`([^`]+)`/)?.[1] ?? "";
  return body.split("\n").filter((l) => l.startsWith("|")).slice(2).map((l) => {
    const cells = l.split("|").slice(1, -1).map((c) => c.trim());
    return { row: `${first(cells[0])} ${first(cells[1])}`, when: cells[2] };
  });
}

export function check(c: Case): void {
  const store = given(c.given);
  const o = apply(store, typeof c.event === "function" ? c.event(store) : c.event);
  for (const [key, want] of Object.entries(c.expect)) assert.deepEqual(o[key as keyof Outcome], want, `${c.row} (${c.when}): ${key}`);
}

// The read a `readIssue` result carries, with `over` for its fields, and the command.
export const read = (over: object = {}) => ({
  read: { updatedAt: T1, delegate: "owner", state: "open", stateChangedAt: T0, priority: 0, blockers: [], moreBlockers: false, parent: null, children: [], moreChildren: false, repo: "repo", sessions: [{ id: S, kind: "delegation", createdAt: T0 }], ...over },
});
export const readIssue = { kind: "readIssue" as const, machine: S, params: { for: T0 } };
