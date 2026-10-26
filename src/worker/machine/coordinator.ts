// What crosses issues (docs/state-machine.md, "Stores"): the slot ledger and its queue, the
// blocker index and the open-PR index. It is reached only through events, and answers
// with events, each taking the ID of the event it answers with its own kind. Today it lives
// in the same Durable Object as every issue's store and is applied in the step that sent
// its event, so a grant is written with the decision that takes it and no two decisions
// count the same free slot.

import { DEFAULT_SLOTS, type CoordinatorEvent, type Inbound, type Outgoing } from "./types.ts";

// An issue's grant: reserved for a session's request, or held by its work (session null), on
// `host`, this machine when absent. `repo`, while its worker has no worktree and wasn't asked
// for on one: the repo it was placed for, by which it moves should its host close. An issue
// holds at most one.
export type Grant = { session: string | null; sinceMs: number; host?: string; repo?: string | null };
// `repo`: the issue's repo as its read gave it. `on`: the host of the issue's worktree, null for
// this machine, absent with none, as a worker never moves host.
export type Place = { issue: string; session: string; priority: number; at: string; repo?: string | null; on?: string | null };

// Where a grant may go, from the fleet (fleet.ts, `roomOf`): this machine's cap, null until a
// config report says it, and the workers the listing saw here; each paired host open to new
// work, in the order paired, with its cap, the workers the listing saw there and the projects
// ready there; the paired hosts that didn't answer; and the project of each of this machine's
// repos.
export type Open = { id: string; cap: number; held: number; ready: string[] };
export type Room = { cap: number | null; held: number; hosts: Open[]; silent: string[]; projects: Record<string, string> };
export const ALONE: Room = { cap: null, held: 0, hosts: [], silent: [], projects: {} };

export type Coordinator = {
  slots: number;
  grants: Record<string, Grant>;
  queue: Place[];
  // For each issue, the blockers its newest read named, and when that read was sent.
  blockers: Record<string, { sentMs: number; ids: string[] }>;
  // For each repo, the issues with a PR open in it.
  prs: Record<string, string[]>;
  // The newest outbox sequence taken from each sending store.
  heard: Record<string, number>;
  // The compare-and-swap's revision, and the answers not yet delivered, as a store's.
  rev: number;
  seq: number;
  outbox: Outgoing[];
};

export function newCoordinator(): Coordinator {
  return { slots: DEFAULT_SLOTS, grants: {}, queue: [], blockers: {}, prs: {}, heard: {}, rev: 0, seq: 0, outbox: [] };
}

// Linear's priority, for the queue: 1 (urgent) first, 4 (low) last, 0 (none) after them.
function rank(priority: number): number {
  return priority === 0 ? 5 : priority;
}

// The queue's order: by priority, then by oldest request.
export function byTurn(a: Place, b: Place): number {
  return rank(a.priority) - rank(b.priority) || (a.at < b.at ? -1 : a.at > b.at ? 1 : 0);
}

// A host's room: its cap less its grants, or the workers the listing saw there if more, so a
// worker the ledger holds elsewhere still takes its room.
function roomOn(c: Coordinator, host: string | null, cap: number, held: number): number {
  const granted = Object.values(c.grants).filter((g) => (g.host ?? null) === host).length;
  return cap - Math.max(granted, held);
}

// Where the place's grant goes, null while nowhere has room. A worker's own host takes it
// while it has room, or, as a host not open to new work, while the fleet has, as the fleet
// wakes it or waits for it. A new worker goes to this machine while it has room, else to the
// first host open with room and the repo's project ready, never here past its cap: a repo
// whose machines could take it waits for one.
export function place(c: Coordinator, room: Room, p: Place): { host: string | null } | null {
  const here = roomOn(c, null, room.cap ?? c.slots, room.held) > 0;
  if (p.on !== undefined) {
    if (p.on === null) return here ? { host: null } : null;
    const own = room.hosts.find((h) => h.id === p.on);
    return (own === undefined ? c.slots - Object.keys(c.grants).length : roomOn(c, own.id, own.cap, own.held)) > 0 ? { host: p.on } : null;
  }
  if (here) return { host: null };
  const open = room.hosts.find((h) => ready(room, h, p.repo) && roomOn(c, h.id, h.cap, h.held) > 0);
  return open === undefined ? null : { host: open.id };
}

// Whether the host has the repo's project ready.
function ready(room: Room, h: Open, repo: string | null | undefined): boolean {
  return repo != null && Object.hasOwn(room.projects, repo) && h.ready.includes(room.projects[repo]);
}

// Applies one event, returning the events it sends to stores.
export function coordinate(c: Coordinator, e: CoordinatorEvent, nowMs: number, room: Room): Inbound[] {
  const sends: Inbound[] = [];
  switch (e.kind) {
    case "slotAsked": {
      const held = c.grants[e.issue];
      if (held?.session === e.session) {
        // Its grant is on its way, or landed: a second could land after the claim cleared it.
        if (e.rerank === true) break;
        sends.push(granted(e.id, e.issue, e.session, held.host, e.id));
        break;
      }
      // A newer request in the session keeps its place's age, and takes the issue's priority,
      // and but for a re-rank its repo and worktree, as asked now. It is placed before the
      // heads are granted, so a queue of issues already holding a grant holds back no one.
      let placed = c.queue.find((p) => p.issue === e.issue && p.session === e.session);
      if (placed === undefined) {
        placed = { issue: e.issue, session: e.session, priority: e.priority, at: e.at };
        c.queue.push(placed);
      }
      placed.priority = e.priority;
      if (e.rerank !== true) {
        placed.repo = e.repo ?? null;
        if (e.on === undefined) delete placed.on;
        else placed.on = e.on;
      }
      grantHeads(c, room, e.id, nowMs, sends, placed);
      if (c.grants[e.issue]?.session !== e.session) sends.push({ kind: "slotWaiting", id: `slotWaiting:${e.id}`, issue: e.issue, session: e.session });
      break;
    }
    case "slotHeld": {
      const held = c.grants[e.issue];
      // With no reservation, it takes a slot even past the cap: an agent found at work. It is
      // on its worktree's host once one is known, and no longer moves; else where it was reserved.
      const host = e.host === undefined ? held?.host : (e.host ?? undefined);
      const repo = e.host === undefined ? held?.repo : undefined;
      c.grants[e.issue] = { session: null, sinceMs: held?.sinceMs ?? nowMs, ...(host === undefined ? {} : { host }), ...(repo === undefined ? {} : { repo }) };
      break;
    }
    case "slotReleased": {
      const held = c.grants[e.issue];
      if (held !== undefined && held.session === e.session) delete c.grants[e.issue];
      if (e.session !== null) c.queue = c.queue.filter((p) => !(p.issue === e.issue && p.session === e.session));
      grantHeads(c, room, e.id, nowMs, sends);
      break;
    }
    case "blockersRead": {
      const held = c.blockers[e.issue];
      if (held === undefined || held.sentMs <= e.sentMs) c.blockers[e.issue] = { sentMs: e.sentMs, ids: [...e.blockers] };
      break;
    }
    case "blockerChanged":
      for (const [issue, entry] of Object.entries(c.blockers)) {
        if (entry.ids.includes(e.issue)) sends.push({ kind: "blockersChanged", id: `blockersChanged:${e.id}`, issue, blocker: e.issue, done: e.done, at: e.at });
      }
      break;
    // An issue's PR in one repo replaces any it had in another.
    case "prRecorded":
      dropPr(c, e.issue);
      if (e.open) c.prs[e.repo] = [...(Object.hasOwn(c.prs, e.repo) ? c.prs[e.repo] : []), e.issue];
      break;
    case "repoPushed":
      for (const issue of Object.hasOwn(c.prs, e.repo) ? c.prs[e.repo] : []) sends.push({ kind: "basePushed", id: `basePushed:${e.id}`, issue, base: e.base });
      break;
  }
  return sends;
}

// Takes the issue out of the open-PR index, in every repo.
export function dropPr(c: Coordinator, issue: string): void {
  for (const [repo, issues] of Object.entries(c.prs)) {
    const open = issues.filter((i) => i !== issue);
    if (open.length > 0) c.prs[repo] = open;
    else delete c.prs[repo];
  }
}

function granted(id: string, issue: string, session: string, host: string | undefined, answers: string): Inbound {
  return { kind: "slotGranted", id: `slotGranted:${id}`, issue, session, answers, ...(host === undefined ? {} : { host }) };
}

// Moves each grant whose host no longer takes its repo's work, its worker not yet there, to where
// the repo has room, should anywhere: `slotMoved` tells its issue, whose start then names the
// new host, as `listen` turns back a start on a host that can't take it and never picks
// another. One on a host that is only silent stays: a start there may yet land. So does one whose
// repo's project isn't known here, which no host shows ready.
function moveClosed(c: Coordinator, room: Room, answers: string, sends: Inbound[]): void {
  for (const [issue, g] of Object.entries(c.grants)) {
    if (g.repo == null || g.host === undefined || !Object.hasOwn(room.projects, g.repo) || room.silent.includes(g.host)) continue;
    if (room.hosts.some((h) => h.id === g.host && ready(room, h, g.repo))) continue;
    const { host: _, ...rest } = g;
    delete c.grants[issue];
    const at = place(c, room, { issue, session: g.session ?? "", priority: 0, at: "", repo: g.repo });
    c.grants[issue] = at === null ? g : { ...rest, ...(at.host === null ? {} : { host: at.host }) };
    if (at !== null) sends.push({ kind: "slotMoved", id: `slotMoved:${answers}:${issue}`, issue, ...(at.host === null ? {} : { host: at.host }) });
  }
}

// Grants each place in turn that has somewhere to go, skipping an issue holding a grant: a
// later place takes only room no place before it could use. `asker`'s grant answers its ask.
function grantHeads(c: Coordinator, room: Room, answers: string, nowMs: number, sends: Inbound[], asker?: Place): void {
  moveClosed(c, room, answers, sends);
  for (const p of [...c.queue].sort(byTurn)) {
    const at = c.grants[p.issue] === undefined ? place(c, room, p) : null;
    if (at === null) continue;
    const host = at.host ?? undefined;
    c.queue = c.queue.filter((q) => q !== p);
    c.grants[p.issue] = { session: p.session, sinceMs: nowMs, ...(host === undefined ? {} : { host }), ...(p.on === undefined ? { repo: p.repo ?? null } : {}) };
    sends.push(granted(p === asker ? answers : `${answers}:${p.issue}`, p.issue, p.session, host, answers));
  }
}

// The reconcile's look for room: grants each place that has somewhere to go, should a
// release's grant have been lost or the fleet have changed.
export function reconcileSlots(c: Coordinator, nowMs: number, room: Room): Inbound[] {
  const sends: Inbound[] = [];
  grantHeads(c, room, `reconcile:${nowMs}`, nowMs, sends);
  return sends;
}
