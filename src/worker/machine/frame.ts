// What the coordinator and the stores it names hold, as `listen`'s panel shows it: the slot
// ledger, the queue in its order, each held issue's deadline and PR, the parked work, the
// in-flight parent chains, the triage machine, each paired host's cap as the fleet last heard it, and why
// the fleet's last reconcile asked for no machine for each repo with a request waiting. Level state, as a base tip is (realtime.ts,
// `publish`): sent on connect and on each change, never queued or acknowledged, and read by
// the panel alone, never by what `listen` carries out. IDs, enums, numbers, times and host names;
// test/contract.json's `frame` holds one, which cli/src/panel/frame.rs reads the same.
//
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node.

import { identifierOf } from "../intake.ts";
import { hostOfWorktree } from "../state.ts";
import type { Storage } from "../storage.ts";
import { childrenFinished } from "./chain.ts";
import { byTurn, type Coordinator } from "./coordinator.ts";
import { type Hosts, loadFleet, type Reason } from "./fleet.ts";
import { loadRun, loadStore, loadTriage, shown } from "./store.ts";
import type { Triage, TriageState } from "./triage.ts";
import type { Checks, DeadlineKind, MergeState, Store, WorkState } from "./types.ts";

// `capped`: its review rounds are spent at its head. `autoMerge`: GitHub's, null until a read
// of the PR shows it.
export type PrView = { number: number; checks: Checks | "pending"; mergeState: MergeState | null; rounds: number; capped: boolean; autoMerge: boolean | null };
// `host`: the paired host its worktree is on, as its start's result named it; null on this
// machine or before a start has named one. `background`: its agent's hook says it waits on a
// call in the background. `anomaly`: the grant is held for an issue the store
// has seen completed or cancelled.
export type SlotView = { issue: string; identifier: string | null; reserved: boolean; sinceMs: number; host: string | null; work: WorkState | null; background: boolean; deadline: { kind: DeadlineKind; dueMs: number } | null; pr: PrView | null; anomaly: boolean };
// `at`: the request's, on Linear's clock. `blockers`: the identifiers of those unfinished
// that the Durable Object knows, for a wait on blockers.
export type QueuedView = { issue: string; identifier: string | null; waiting: "slot" | "held" | "merged" | "blockers"; blockers: string[]; at: string };
export type ParkedView = { issue: string; identifier: string | null; work: WorkState; stalled: boolean; pr: PrView | null };
export type ChainView = { issue: string; identifier: string | null; run: string; issues: number; answered: number };
export type HostView = { id: string; name: string; cap: number; answering: boolean };
export type Frame = {
  type: "frame";
  cap: number;
  hosts: HostView[];
  slots: SlotView[];
  queue: QueuedView[];
  parked: ParkedView[];
  chains: ChainView[];
  // `dueMs`: its deadline, the retry's while `retrying`.
  triage: { state: TriageState; dueMs: number | null; failures: number };
  why: Record<string, Reason>;
};

// Work that holds no slot and isn't at rest for good: an `exited` or `rest` worker with its
// PR open, a `halted` one, and a `merged` one awaiting its reap.
function parked(s: Store): boolean {
  const state = s.work?.state;
  return state === "halted" || state === "merged" || ((state === "exited" || state === "rest") && s.facts.pr?.status === "open");
}

function prView(s: Store): PrView | null {
  const pr = s.facts.pr;
  if (pr === null || pr.status !== "open") return null;
  return { number: pr.number, checks: s.facts.checks[pr.head]?.value ?? "pending", mergeState: s.facts.mergeState?.value ?? null, rounds: s.facts.rounds.total, capped: s.facts.notices.includes(`roundsCap:${pr.number}:${pr.head}`), autoMerge: s.facts.autoMerge?.pr === pr.number ? s.facts.autoMerge.on : null };
}

function hostOf(s: Store | undefined): string | null {
  const worktree = s?.facts.worktree;
  return worktree ? (hostOfWorktree(worktree.id) ?? null) : null;
}

function deadline(s: Store): SlotView["deadline"] {
  return (s.work?.deadlines ?? []).reduce<SlotView["deadline"]>((soonest, d) => (soonest === null || d.dueMs < soonest.dueMs ? { kind: d.kind, dueMs: d.dueMs } : soonest), null);
}

// Built from records already loaded, so a test needs no storage.
export function build(c: Coordinator, t: Triage, stores: Map<string, Store>, identifiers: Map<string, string>, heard: Hosts | null = null, why: Record<string, Reason> = {}): Frame {
  const name = (issue: string) => identifiers.get(issue) ?? null;
  const slots = Object.entries(c.grants).map(([issue, grant]): SlotView => {
    const s = stores.get(issue);
    return { issue, identifier: name(issue), reserved: grant.session !== null, sinceMs: grant.sinceMs, host: hostOf(s), work: s?.work?.state ?? null, background: s?.facts.agent?.status === "background", deadline: s ? deadline(s) : null, pr: s ? prView(s) : null, anomaly: s?.facts.issueState?.value === "completed" || s?.facts.issueState?.value === "cancelled" };
  });
  slots.sort((a, b) => a.sinceMs - b.sinceMs || (a.issue < b.issue ? -1 : 1));
  const queue: QueuedView[] = [...c.queue].sort(byTurn).map((p) => ({ issue: p.issue, identifier: name(p.issue), waiting: "slot", blockers: [], at: p.at }));
  const others: QueuedView[] = [];
  const rows: ParkedView[] = [];
  const chains: ChainView[] = [];
  for (const [issue, s] of stores) {
    for (const session of Object.values(s.sessions)) {
      const waiting = session.facts.waiting;
      if (session.machine?.state !== "queued" || waiting === "slot" || waiting === null || session.facts.request === null) continue;
      const blockers = waiting === "blockers" ? (s.facts.blockers?.ids ?? []).filter((id) => s.facts.blockers?.states[id]?.done !== true).flatMap((id) => identifiers.get(id) ?? []) : [];
      others.push({ issue, identifier: name(issue), waiting, blockers, at: session.facts.request.at });
    }
    if (c.grants[issue] === undefined && parked(s)) rows.push({ issue, identifier: name(issue), work: s.work!.state, stalled: s.work!.stalled, pr: prView(s) });
    const chained = s.facts.chained;
    const ended = s.facts.issueState?.value === "completed" || s.facts.issueState?.value === "cancelled" || childrenFinished(s.facts);
    if (chained !== null && chained.haltedAt === null && !ended) chains.push({ issue, identifier: name(issue), run: chained.run, issues: chained.asked.length, answered: chained.answered.length });
  }
  others.sort((a, b) => (a.at < b.at ? -1 : a.at > b.at ? 1 : 0));
  const hosts = (heard?.hosts ?? []).map((h): HostView => ({ id: h.id, name: h.name, cap: h.cap, answering: h.answering }));
  return { type: "frame", cap: c.slots, hosts, slots, queue: [...queue, ...others], parked: rows, chains, triage: { state: t.state, dueMs: t.deadline?.dueMs ?? null, failures: t.failures }, why };
}

// The frame as the Durable Object holds it now, around the coordinator `c` read from it.
export async function frameOf(storage: Storage, c: Coordinator): Promise<Frame> {
  const stores = new Map<string, Store>();
  for (const issue of await shown(storage, c)) stores.set(issue, await loadStore(storage, issue));
  const identifiers = new Map<string, string>();
  const named = new Set([...stores.keys(), ...[...stores.values()].flatMap((s) => s.facts.blockers?.ids ?? [])]);
  for (const issue of named) {
    const identifier = await identifierOf(storage, issue);
    if (identifier !== null) identifiers.set(issue, identifier);
  }
  return build(c, await loadTriage(storage), stores, identifiers, (await loadFleet(storage)).hosts, (await loadRun(storage)).why);
}
