// The machines' persistence in the Durable Object's storage: one record per issue, its
// store, and one for the coordinator. Each write is a compare-and-swap on the record's
// revision, which every move of a machine's version moves too, so a replay or a race
// applies once: one that lost re-reads and runs again on what won.
//
// An event a store sends is written in the same write as the transition that sends it,
// then delivered, and dropped from the outbox in the write that lands its delivery, which
// lands the whole drain at once; one store's events reach another in the order sent, and a
// redelivery after a failure between the two is dropped by its outbox sequence. Today every store and the coordinator live in the owner's one
// Durable Object, which runs one event at a time, so the coordinator's events and their
// answers are applied in the step that sent them.
//
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node.

import type { Timer } from "../alarm.ts";
import { coordinate, dropPr, newCoordinator, reconcileSlots, type Coordinator, type Room } from "./coordinator.ts";
import { newStore, retired } from "./ctx.ts";
import { LOCAL } from "./queue.ts";
import { awaySaid, cleared, contextAsked as askContext, dueDeadlines, expire, markSettled, nextDue, rebuildAsked as askRebuild, connectAsked as askConnect, receive, retirable, type Received } from "./step.ts";
import { expireFleet, FLEET_KEY, fleetDue, loadFleet, napping, newRun, reconcileFleet, roomOf, RUN_KEY, slotTotal, stepFleet, type Fleet, type FleetEvent, type FleetStepped, type Run } from "./fleet.ts";
import { expireTriage, newTriage, stepTriage, TRIAGE_KEY as TRIAGE, triageDue, type Triage, type TriageEvent, type TriageStepped } from "./triage.ts";
import { type Clears, type Command, type CoordinatorEvent, type Inbound, type Outgoing, type Store } from "./types.ts";
import { Invalid, object, WEBHOOK_DELIVERY, whole } from "../check.ts";
import { REPO } from "../github.ts";
import { HEAD } from "../state.ts";
import { admittedHosts } from "../hosts.ts";
import { parseEvent } from "./validate.ts";
import type { Storage } from "../storage.ts";

const STORE = "machine:issue:";
const COORDINATOR = "machine:coordinator";
// Each issue's next due time, so the alarm reads only the stores that need it.
const DUE = "machine:due";
// The issues whose store holds `listen` away, so the connection, or an alarm that finds it
// listening, tells each, whatever it is due on.
const HELD = "machine:held";
// The issue a worktree was started for, by worktree ID, written as a start's result is
// learned (relay.ts): an observation is delivered to it.
export const WORKTREE = "worktree:";
// The newest an agent's own hook reported, by worktree, as `worktreeId` is kept: `at`, its
// host's clock's time, which orders two hooks of one worktree, and `seen`, the Durable
// Object's, which orders a hook against a listing.
export const HOOK_AT = "hookAt:";
export type HookTime = { at: string; seen: number };

export class Conflict extends Error {}

// Hands commands to `listen` (TW-456's relay).
export type Sender = (commands: Command[]) => void;

// A record the machines keep: its key, what it is before its first write, and, for a store,
// what each change of it does to it and the issue whose due time, and whether it holds
// `listen` away, are written beside it in the same put, and the keys the Durable Object
// indexes it by, which go in the same write as the step that stops the record holding them.
type Kind<T extends { rev: number }> = {
  key: string;
  fresh: () => T;
  changed?: (next: T, nowMs: number) => void;
  indexed?: (held: T) => string[];
  due?: { issue: string; at: (next: T, nowMs: number) => number | null; away: (next: T) => boolean };
};

function storeKind(issue: string): Kind<Store> {
  // The store and when it is next due, in one write, so a failure can't leave its
  // deadlines unscheduled.
  return { key: STORE + issue, fresh: () => newStore(issue), changed: markSettled, indexed: worktreeKeys, due: { issue, at: nextDue, away: (s) => s.away } };
}

// A store's worktree is indexed by the issue it was started for and by its newest hook, both
// kept while the store's `worktree` fact names it (and after a lost start's, which leaves the
// fact null, since a launched agent's hooks still route by them).
function worktreeKeys(s: Store): string[] {
  return s.facts.worktree === null ? [] : [WORKTREE + s.facts.worktree.id, HOOK_AT + s.facts.worktree.id];
}

const COORDINATOR_KIND: Kind<Coordinator> = { key: COORDINATOR, fresh: newCoordinator };

// What a reset (storage.ts) keeps of the machines: each issue store's, the triage and fleet
// machines' counters, which never go back, so a command ID, and the post Linear keys by it,
// is never taken for an older one's. Undefined for anything else. A counter it can't read
// fails the reset rather than start over.
export function keptAtReset(key: string, value: unknown): unknown {
  const held = key.startsWith(STORE) || key === TRIAGE || key === RUN_KEY ? object(value, key) : null;
  const counter = (name: string) => whole(held![name], `${key} ${name}`, Number.MAX_SAFE_INTEGER);
  if (key.startsWith(STORE)) return { ...newStore(key.slice(STORE.length)), rev: counter("rev"), seq: counter("seq"), commandSeq: counter("commandSeq") };
  if (key === RUN_KEY) return { ...newRun(), rev: counter("rev"), commandSeq: counter("commandSeq") };
  if (key !== TRIAGE) return undefined;
  return { ...newTriage(), rev: counter("rev"), commandSeq: counter("commandSeq") };
}

// The requests a reset (storage.ts) loses of an issue's store, each by its session and its
// request's kind and state. An unstarted delegation is rebuilt (rebuild.ts); a claimed or
// stopping one is left, its worker maybe still running, and only these lines show it.
// Nothing for a finished request or a value that isn't a store.
export function lostAtReset(key: string, value: unknown): { issue: string; session: string; kind: string; state: string }[] {
  if (!key.startsWith(STORE)) return [];
  const sessions = (value as Partial<Store> | null)?.sessions ?? {};
  return Object.entries(sessions).flatMap(([session, held]) => {
    const kind = held?.facts?.request?.kind;
    const state = held?.machine?.state;
    return kind === undefined || state === undefined || state === "done" ? [] : [{ issue: key.slice(STORE.length), session, kind, state }];
  });
}

const TRIAGE_KIND: Kind<Triage> = { key: TRIAGE, fresh: newTriage };
const RUN_KIND: Kind<Run> = { key: RUN_KEY, fresh: newRun };

async function load<T extends { rev: number }>(storage: Storage, kind: Kind<T>): Promise<T> {
  return (await storage.get<T>(kind.key)) ?? kind.fresh();
}

export async function loadStore(storage: Storage, issue: string): Promise<Store> {
  return load(storage, storeKind(issue));
}

export async function loadCoordinator(storage: Storage): Promise<Coordinator> {
  return load(storage, COORDINATOR_KIND);
}

export async function loadTriage(storage: Storage): Promise<Triage> {
  return load(storage, TRIAGE_KIND);
}

export async function loadRun(storage: Storage): Promise<Run> {
  return load(storage, RUN_KIND);
}

// Where the coordinator may grant a slot, as the fleet, the hosts admitted and the fleet's
// command hold it now: a host with a suspend or a destroy in flight takes no new work.
export async function loadRoom(storage: Storage): Promise<Room> {
  const fleet = await loadFleet(storage);
  return roomOf(fleet, await admittedHosts(storage), napping(await loadRun(storage), fleet));
}

async function loadDue(storage: Storage): Promise<Record<string, number>> {
  return (await storage.get<Record<string, number>>(DUE)) ?? {};
}

async function loadHeld(storage: Storage): Promise<string[]> {
  return (await storage.get<string[]>(HELD)) ?? [];
}

// Every issue the alarm or a connection may need: those due and those holding `listen` away.
async function known(storage: Storage): Promise<string[]> {
  return [...new Set([...Object.keys(await loadDue(storage)), ...(await loadHeld(storage))])];
}

// Every issue whose store may show in `listen`'s frame (frame.ts): those `known`, and each
// the coordinator holds a grant, a place, blockers or an open PR for.
export async function shown(storage: Storage, c: Coordinator): Promise<string[]> {
  return [...new Set([...Object.keys(c.grants), ...c.queue.map((p) => p.issue), ...Object.keys(c.blockers), ...Object.values(c.prs).flat(), ...(await known(storage))])];
}

// The most records one batch holds before its drain stops, so its put stays within
// storage's 128 entries with the due index and what the last delivery touched.
const BATCH_RECORDS = 100;

// The records one step touches, each read once and changed in memory, then written together
// in one put, each at most once: the one compare-and-swap, against the revision each was
// read at, so a record that moved since throws `Conflict` and nothing is written. The due
// index goes in the same put, only when a store's due time moved.
class Batch {
  private held = new Map<string, { kind: Kind<{ rev: number }>; read: number; first: { rev: number }; value: { rev: number }; changed: boolean }>();
  private readonly storage: Storage;
  private roomRead: Room | null = null;
  readonly nowMs: number;

  constructor(storage: Storage, nowMs: number) {
    this.storage = storage;
    this.nowMs = nowMs;
  }

  async load<T extends { rev: number }>(kind: Kind<T>): Promise<T> {
    const held = this.held.get(kind.key);
    if (held !== undefined) return held.value as T;
    const value = await load(this.storage, kind);
    this.held.set(kind.key, { kind: kind as unknown as Kind<{ rev: number }>, read: value.rev, first: value, value, changed: false });
    return value;
  }

  // Where the coordinator may grant a slot, read once per batch: nothing in a batch changes it.
  async room(): Promise<Room> {
    this.roomRead ??= await loadRoom(this.storage);
    return this.roomRead;
  }

  // Applies `step` to the record as this batch holds it; what it returns replaces it, unless
  // that is the record as held.
  async change<T extends { rev: number }, R>(kind: Kind<T>, step: (before: T) => [T, R]): Promise<R> {
    const before = await this.load(kind);
    const [next, out] = step(before);
    if (next !== before) {
      kind.changed?.(next, this.nowMs);
      this.held.set(kind.key, { ...this.held.get(kind.key)!, value: next, changed: true });
    }
    return out;
  }

  // Whether a drain stops here, leaving the rest of its outboxes to the next batch.
  get full(): boolean {
    return this.held.size >= BATCH_RECORDS;
  }

  async update(issue: string, step: (s: Store) => Received): Promise<Command[]> {
    return this.change(storeKind(issue), (s) => {
      const { store, send } = step(s);
      return [store, send];
    });
  }

  // Applies `step` to a copy of the coordinator and queues the answers it returns; one that
  // returns null changes nothing, and the result says whether anything changed.
  async updateCoordinator(step: (c: Coordinator) => Inbound[] | null): Promise<boolean> {
    return this.change(COORDINATOR_KIND, (c) => {
      const next = structuredClone(c);
      const answers = step(next);
      if (answers === null) return [c, false];
      queueAnswers(next, answers);
      return [next, true];
    });
  }

  // The writes for every record changed. Their revisions are read in one `get` and checked
  // with no I/O before the caller's put, so no other write can land between the check and it.
  async writes(): Promise<Record<string, unknown>> {
    const changed = [...this.held.values()].filter((h) => h.changed);
    if (changed.length === 0) return {};
    const keys = changed.map((h) => h.kind.key);
    if (changed.some((h) => h.kind.due !== undefined)) keys.push(DUE, HELD);
    const got = new Map<string, unknown>();
    // A `get` takes at most 128 keys.
    for (let i = 0; i < keys.length; i += 128) {
      for (const [k, v] of await this.storage.get<unknown>(keys.slice(i, i + 128))) got.set(k, v);
    }
    const out: Record<string, unknown> = {};
    const due = (got.get(DUE) as Record<string, number> | undefined) ?? {};
    const held = (got.get(HELD) as string[] | undefined) ?? [];
    let dueMoved = false;
    let heldMoved = false;
    for (const h of changed) {
      if (((got.get(h.kind.key) as { rev: number } | undefined)?.rev ?? 0) !== h.read) throw new Conflict(`${h.kind.key} moved`);
      if (h.kind.due !== undefined) {
        const { issue, at, away } = h.kind.due;
        const next = at(h.value, this.nowMs);
        if ((due[issue] ?? null) !== next) {
          dueMoved = true;
          if (next === null) delete due[issue];
          else due[issue] = next;
        }
        if (away(h.value) !== held.includes(issue)) {
          heldMoved = true;
          if (away(h.value)) held.push(issue);
          else held.splice(held.indexOf(issue), 1);
        }
      }
      out[h.kind.key] = { ...h.value, rev: h.read + 1 };
    }
    if (dueMoved) out[DUE] = due;
    if (heldMoved) out[HELD] = held;
    return out;
  }

  // The index keys the records no longer hold, as they stood when read against as they are
  // now, so a record that moved away and back drops none.
  drops(): string[] {
    const dropped: string[] = [];
    for (const h of this.held.values()) {
      if (!h.changed || h.kind.indexed === undefined) continue;
      const now = new Set(h.kind.indexed(h.value));
      for (const key of h.kind.indexed(h.first)) if (!now.has(key)) dropped.push(key);
    }
    return dropped;
  }

  // `also`: writes outside the records, none at a record's key, landing in the same put.
  // The index keys the records dropped are deleted in the same write: issued together, so
  // storage lands both or neither.
  async commit(also: Record<string, unknown> = {}): Promise<void> {
    const writes = { ...also, ...(await this.writes()) };
    const dropped = this.drops();
    await Promise.all([
      Object.keys(writes).length > 0 ? this.storage.put(writes) : undefined,
      dropped.length > 0 ? this.storage.delete(dropped) : undefined,
    ]);
  }
}

// Today every machine lives in the owner's one Durable Object, whose input gate lets no
// other event in between a step's read and its put, so `Conflict` never fires there. The
// revision and this one retry loop stay so the machines can be split across Durable
// Objects, and the simulator's `race` runs two steps inside each other to hold them to it.
const RETRIES = 3;

// Runs `work` on a batch and writes what it changed, running it again from storage should
// another write have come between. Nothing `work` does reaches outside the batch, so a run
// that lost repeats nothing: its commands go to listen only from what the commit returns.
async function batched<R>(storage: Storage, nowMs: number, work: (b: Batch) => Promise<R>, also?: () => Promise<Record<string, unknown>>): Promise<R> {
  for (let attempt = 1; ; attempt++) {
    const b = new Batch(storage, nowMs);
    try {
      const out = await work(b);
      await b.commit(await also?.());
      return out;
    } catch (error) {
      if (!(error instanceof Conflict) || attempt >= RETRIES) throw error;
    }
  }
}

// Reads the record, applies `step` and writes back what it returns, unless that is the
// record as read.
async function change<T extends { rev: number }, R>(storage: Storage, kind: Kind<T>, nowMs: number, step: (before: T) => [T, R]): Promise<R> {
  return batched(storage, nowMs, (b) => b.change(kind, step));
}

async function update(storage: Storage, issue: string, nowMs: number, step: (s: Store) => Received): Promise<Command[]> {
  return batched(storage, nowMs, (b) => b.update(issue, step));
}

// An event for the triage machine (triage.ts): an issue created in Triage, a session's end
// or a result of its command. Hands listen the command it sends.
export async function triageEvent(storage: Storage, e: TriageEvent, nowMs: number, sender: Sender): Promise<void> {
  sender(await updateTriage(storage, nowMs, (t) => stepTriage(t, e, nowMs)));
}

// A terminal a `startTurn` opened, to the issue whose turn it ran, written with its result
// (relay.ts, `learned`) and deleted once its close has been looked at.
export const TURN_TERMINAL = "turn:";

// A terminal `listen` saw close ends what the Durable Object holds as running in it: the triage
// session, and the turn that holds it as its newest (`turn.terminal`), never an older turn's of
// the same session, whose terminal a newer turn replaced. A terminal that ran neither, a
// worker's, ends nothing. Its index key goes once the close has been looked at, and stays when
// the delivery failed, as `listen` sends no report again.
export async function terminalClosed(storage: Storage, terminal: string, id: string, nowMs: number, sender: Sender): Promise<void> {
  await triageEvent(storage, { kind: "triageEnded", id, terminal }, nowMs, sender);
  const issue = await storage.get<string>(TURN_TERMINAL + terminal);
  if (issue === undefined) return;
  const session = Object.entries((await loadStore(storage, issue)).sessions).find(([, s]) => s.facts.turn?.terminal === terminal)?.[0];
  if (session !== undefined) await deliver(storage, { id, issue, kind: "turnEnded", session, terminal }, nowMs, sender);
  await storage.delete(TURN_TERMINAL + terminal);
}

// An event for the fleet's machine (fleet.ts): a complete listing's idle and dormant machines,
// a result of its command, a worker's command waiting on a host, or a wait cleared. Hands listen
// the commands it sends.
export async function fleetEvent(storage: Storage, e: FleetEvent, nowMs: number, sender: Sender): Promise<void> {
  sender(await updateRun(storage, nowMs, (r) => stepFleet(r, e, nowMs)));
}

// The fleet's reconcile (fleet.ts, `reconcileFleet`) over what the Durable Object holds now, run
// after each event that may change the fleet or the ledger.
export async function fleetReconciled(storage: Storage, nowMs: number, sender: Sender): Promise<void> {
  const [fleet, c, admitted, room] = [await loadFleet(storage), await loadCoordinator(storage), await admittedHosts(storage), await loadRoom(storage)];
  sender(await updateRun(storage, nowMs, (r) => reconcileFleet(r, { fleet, c, room, admitted }, nowMs)));
}

async function updateRun(storage: Storage, nowMs: number, step: (r: Run) => FleetStepped): Promise<Command[]> {
  return change(storage, RUN_KIND, nowMs, (held) => {
    const { run, send } = step(held);
    return [run, send];
  });
}

async function updateTriage(storage: Storage, nowMs: number, step: (t: Triage) => TriageStepped): Promise<Command[]> {
  return change(storage, TRIAGE_KIND, nowMs, (t) => {
    const { triage, send } = step(t);
    return [triage, send];
  });
}

// Writes beside a step's records, read from storage alone and the issue's store as the step
// left it, so nothing comes between them and the put; read again when a conflict runs the
// step again.
export type Also = (after: Store) => Promise<Record<string, unknown>>;

// Delivers an event from outside the Durable Object (listen's results, observations and
// workers' reports) to its issue's store, then everything it sends on, and hands listen
// the commands to send. What `also` returns lands in the event's own write, and `landed`
// runs once that has landed, before anything that may still fail.
export async function deliver(storage: Storage, body: unknown, nowMs: number, sender: Sender, landed?: () => Promise<void>, also?: Also): Promise<void> {
  const event = parseEvent(body);
  let after: Store | undefined;
  const commands = await batched(
    storage,
    nowMs,
    (b) =>
      b.update(event.issue, (s) => {
        const received = receive(s, event, nowMs);
        after = received.store;
        return received;
      }),
    also && (() => also(after!)),
  );
  await landed?.();
  await settle(storage, event.issue, commands, nowMs, sender);
}

// An event from the intake as writes for the intake's own put, so its delivery's dedupe and
// the transition it runs land together or not at all. The commands it sends are in the
// store's queue, so once the put lands they go out through `settle`, and a send that is
// lost is sent again at their timeout.
export async function staged(storage: Storage, body: unknown, nowMs: number): Promise<{ issue: string; writes: Record<string, unknown>; send: Command[] }> {
  const event = parseEvent(body);
  const b = new Batch(storage, nowMs);
  const send = await b.update(event.issue, (s) => receive(s, event, nowMs));
  const writes = await b.writes();
  // The intake's put can't delete, and no event it carries drops a worktree (only a result,
  // an observation of one, or a reap does, through `deliver`).
  if (b.drops().length > 0) throw new Error("an intake event dropped a worktree");
  return { issue: event.issue, writes, send };
}

// After an event's write has landed: hands listen its commands and delivers what the store
// sent on.
export async function settle(storage: Storage, issue: string, send: Command[], nowMs: number, sender: Sender): Promise<void> {
  sender(send);
  await deliverOutbox(storage, issue, nowMs, sender);
}

// Delivers a store's outbox, and everything that sends on, in one batch, and hands listen
// the commands once it has landed. A batch that fills up is followed by another, which
// delivers what it left of this outbox and the coordinator's; what it left elsewhere is in
// a store due now, for the alarm.
async function deliverOutbox(storage: Storage, from: string, nowMs: number, sender: Sender): Promise<void> {
  for (let first = true, full = true; full; first = false) {
    sender(
      await batched(storage, nowMs, async (b) => {
        const send: Command[] = [];
        await flush(b, from, send, new Set());
        if (!first && from !== "coordinator") await flush(b, "coordinator", send, new Set());
        full = b.full;
        return send;
      }),
    );
  }
}

// The Worker's acknowledgement was the away message for the session's request at `at`
// (step.ts, `awaySaid`).
export async function acknowledgedAway(storage: Storage, issue: string, session: string, at: string, nowMs: number, sender: Sender): Promise<void> {
  sender(await update(storage, issue, nowMs, (s) => awaySaid(s, session, at, nowMs)));
}

// The issue completed, at `at`, for its context (step.ts, `contextAsked`).
export async function contextAsked(storage: Storage, issue: string, at: string, nowMs: number, sender: Sender): Promise<void> {
  sender(await update(storage, issue, nowMs, (s) => askContext(s, at, nowMs)));
}

// An issue delegated to Tolkster, after a storage reset, with the chain Tolkster's records
// name it the parent of (step.ts, `rebuildAsked`).
export async function rebuildAsked(storage: Storage, issue: string, chain: { session: string; children: string[] } | null, nowMs: number, sender: Sender): Promise<void> {
  await settle(storage, issue, await update(storage, issue, nowMs, (s) => askRebuild(s, nowMs, chain)), nowMs, sender);
}

// An issue delegated to Tolkster, at `listen`'s connect (step.ts, `connectAsked`).
export async function connectAsked(storage: Storage, issue: string, nowMs: number, sender: Sender): Promise<void> {
  await settle(storage, issue, await update(storage, issue, nowMs, (s) => askConnect(s, nowMs)), nowMs, sender);
}

// Whether a parent's chain holds on the issue (intake.ts, `Chained`).
export async function chainHolds(storage: Storage, issue: string): Promise<boolean> {
  return (await loadStore(storage, issue)).facts.chain !== null;
}

// Whether the issue's store still holds it, so the intake keeps its maps: a machine, which
// only the store's retirement (`retired`, ctx.ts) drops, or an open PR, which a reopen after
// that retirement brings back with no machine, and whose events still need the branch.
export async function holds(storage: Storage, issue: string): Promise<boolean> {
  const s = await loadStore(storage, issue);
  return s.work !== null || Object.values(s.sessions).some((sess) => sess.machine !== null) || s.facts.pr?.status === "open";
}

// The issues with a PR open in the repo, by the coordinator's index, each with that PR's
// number and head as its store holds them, which `prShows` repairs from GitHub's reads.
// The index moves only once a store's `prRecorded` lands, after the store's own put, so a
// store due now, which an undelivered outbox makes it, is read too: a delivery that failed
// leaves it there for the alarm, and a status or comment meanwhile still reaches its issue.
export async function openPrs(storage: Storage, repo: string, nowMs: number): Promise<{ issueId: string; number: number; head: string }[]> {
  const c = await loadCoordinator(storage);
  const indexed = Object.hasOwn(c.prs, repo) ? c.prs[repo] : [];
  const due = Object.entries(await loadDue(storage)).flatMap(([issue, at]) => (at <= nowMs ? [issue] : []));
  const open: { issueId: string; number: number; head: string }[] = [];
  for (const issue of new Set([...indexed, ...due])) {
    const pr = (await loadStore(storage, issue)).facts.pr;
    if (pr?.status === "open" && pr.repo === repo) open.push({ issueId: issue, number: pr.number, head: pr.head });
  }
  return open;
}

// The commands in flight on every issue with one, for a connection that has just come: what
// was sent to none, or to a connection now gone, goes again at once with the same ID, which
// `listen` carries out once. The Durable Object's own posts aren't `listen`'s.
export async function inFlight(storage: Storage): Promise<Command[]> {
  const out: Command[] = [];
  for (const issue of await known(storage)) {
    const s = await loadStore(storage, issue);
    for (const c of s.queue) if (c.sentMs !== null && !c.superseded && !LOCAL.includes(c.kind)) out.push(c);
  }
  const triage = (await loadTriage(storage)).command;
  if (triage !== null && triage.sentMs !== null) out.push(triage);
  const fleet = (await loadRun(storage)).command;
  if (fleet !== null && fleet.sentMs !== null) out.push(fleet);
  return out;
}

// `listen` has connected: every machine is told (`listenConnected`), and one that held it
// away begins its commands' timeouts again; the connection is sent what is in flight and
// what the machines send now.
export async function connected(storage: Storage, nowMs: number, failed: (issue: string, error: unknown) => void): Promise<Command[]> {
  const sent: Command[] = [];
  const id = `connected:${nowMs}`;
  // One machine whose write fails stops none of the others, nor the connection's sending: it
  // still holds `listen` away, which makes the alarm due and tell it (`machineTimer`).
  const told = async (issue: string, step: () => Promise<Command[]>) => {
    try {
      sent.push(...(await step()));
    } catch (error) {
      failed(issue, error);
    }
  };
  await told("triage", () => updateTriage(storage, nowMs, (t) => stepTriage(t, { kind: "listenConnected", id }, nowMs)));
  for (const issue of await known(storage)) {
    await told(issue, () => update(storage, issue, nowMs, (s) => receive(s, { kind: "listenConnected", id, issue }, nowMs)));
  }
  await told("fleet", async () => {
    const send: Command[] = [];
    await fleetReconciled(storage, nowMs, (c) => send.push(...c));
    return send;
  });
  return [...(await inFlight(storage)).filter((c) => !sent.some((s) => s.id === c.id)), ...sent];
}

// Delivers a store's outbox in order, each event dropped from it once it has landed in the
// batch, and collects the commands the stores it reaches send.
async function flush(b: Batch, from: string, send: Command[], flushing: Set<string>): Promise<void> {
  // A store whose flush runs further up this step delivers in that loop, in order.
  if (flushing.has(from)) return;
  flushing.add(from);
  try {
    for (;;) {
      const outbox = from === "coordinator" ? (await b.load(COORDINATOR_KIND)).outbox : (await b.load(storeKind(from))).outbox;
      const next = outbox[0];
      if (next === undefined || b.full) return;
      await land(b, from, next, send, flushing);
      if (from === "coordinator") await b.change(COORDINATOR_KIND, (c) => [{ ...c, outbox: c.outbox.filter((o) => o.seq > next.seq) }, null]);
      else await b.update(from, (s) => ({ store: { ...s, outbox: s.outbox.filter((o) => o.seq > next.seq) }, send: [] }));
    }
  } finally {
    flushing.delete(from);
  }
}

async function land(b: Batch, from: string, out: Outgoing, send: Command[], flushing: Set<string>): Promise<void> {
  if (out.to === "coordinator") {
    const room = await b.room();
    const heard = await b.updateCoordinator((c) => {
      if ((c.heard[from] ?? 0) >= out.seq) return null;
      c.heard[from] = out.seq;
      return coordinate(c, out.event as CoordinatorEvent, b.nowMs, room);
    });
    if (!heard) return;
    return flush(b, "coordinator", send, flushing);
  }
  const event = out.event as Inbound;
  send.push(...(await b.update(out.to, (s) => receive(s, event, b.nowMs, { store: from, seq: out.seq }))));
  await flush(b, out.to, send, flushing);
}

function queueAnswers(c: Coordinator, answers: Inbound[]): void {
  for (const event of answers) {
    c.seq += 1;
    c.outbox.push({ id: event.id, seq: c.seq, to: event.issue, event });
  }
}

// A push to a repo's default branch, from GitHub, mapped to the repo at ingress: the
// coordinator sends `basePushed` to each issue with a PR open in it.
export async function repoPushed(storage: Storage, delivery: string, repo: string, base: string, nowMs: number, sender: Sender): Promise<void> {
  if (typeof repo !== "string" || !REPO.test(repo) || typeof base !== "string" || !HEAD.test(base) || typeof delivery !== "string" || !WEBHOOK_DELIVERY.test(delivery)) throw new Invalid("invalid push");
  const room = await loadRoom(storage);
  await coordinatorStep(storage, nowMs, sender, (c) => coordinate(c, { kind: "repoPushed", id: delivery, repo, base }, nowMs, room));
}

// What `listen` said of its settings or hosts, kept in the fleet record, the coordinator's
// slots set to the total the fleet gives (fleet.ts, `slotTotal`), and each place granted that
// now has somewhere to go: on each config report, as `listen` connects or its settings change,
// and on a listing when the total moves or a place is granted.
export async function fleetReported(storage: Storage, said: Partial<Fleet>, nowMs: number, sender: Sender): Promise<void> {
  const fleet = { ...(await loadFleet(storage)), ...said };
  await storage.put(FLEET_KEY, fleet);
  const slots = slotTotal(fleet);
  if (slots === null) return;
  const room = await loadRoom(storage);
  await coordinatorStep(storage, nowMs, sender, (c) => {
    const moved = c.slots !== slots;
    c.slots = slots;
    const answers = reconcileSlots(c, nowMs, room);
    return said.settings === undefined && !moved && answers.length === 0 ? null : answers;
  });
}

// What waits on `clears` is sent again at once (docs/state-machine.md, Commands): each issue's
// command in flight that waits on it, and the fleet's. A command waiting is in flight, so its
// issue is in the due index; a store with none waiting isn't written. A write that fails leaves
// the rest to their timeouts. `only`: those issues alone, and not the fleet's.
export async function waitsCleared(storage: Storage, clears: Clears, nowMs: number, sender: Sender, only?: string[]): Promise<void> {
  for (const issue of only ?? Object.keys(await loadDue(storage))) sender(await update(storage, issue, nowMs, (s) => cleared(s, clears, nowMs)));
  if (only === undefined) await fleetEvent(storage, { kind: "cleared", clears }, nowMs, sender);
}

// A step of the coordinator, written with its answers in its outbox before they are
// delivered, so a delivery that fails leaves them for the alarm: a push's has no other.
async function coordinatorStep(storage: Storage, nowMs: number, sender: Sender, step: (c: Coordinator) => Inbound[] | null): Promise<void> {
  await batched(storage, nowMs, (b) => b.updateCoordinator(step));
  await deliverOutbox(storage, "coordinator", nowMs, sender);
}

// The issues a chain's run took, once every forward has been answered, and none once the
// parent was halted; null, unanswered, when the parent refused the run or a forward hasn't
// been answered.
export async function chained(storage: Storage, parent: string, run: string): Promise<string[] | null> {
  const chain = (await loadStore(storage, parent)).facts.chained;
  if (chain?.run !== run || chain.asked.some((c) => !chain.answered.includes(c))) return null;
  return chain.haltedAt !== null ? [] : chain.issues.filter((c) => chain.asked.includes(c));
}

// The alarm's timer: fires each deadline due as an event, and reconciles over the
// Durable Object's own tables, never Linear's: `listen` found gone is told to each machine
// with a command in flight, a command with no result past its timeout is sent again or
// ended as `lost`, an outbox not yet delivered is delivered, and a free slot goes to the
// queue's head.
export function machineTimer(storage: Storage, sender: Sender, listening: () => boolean): Timer {
  return {
    async next() {
      const due = Object.values(await loadDue(storage));
      const c = await storage.get<Coordinator>(COORDINATOR);
      if (c !== undefined && c.outbox.length > 0) due.push(0);
      const t = await loadTriage(storage);
      const triage = triageDue(t);
      if (triage !== null) due.push(triage);
      const fleet = fleetDue(await loadRun(storage));
      if (fleet !== null) due.push(fleet);
      // A machine still holding `listen` away while something listens is due now: the
      // connection's telling failed, or the alarm found `listen` gone while its socket lived on.
      if (((await loadHeld(storage)).length > 0 || t.away) && listening()) due.push(0);
      return due.length === 0 ? null : Math.min(...due);
    },
    async run(nowMs) {
      await reconcile(storage, nowMs, sender, listening());
    },
  };
}

// `listens` is read once, for the whole turn. A machine still holding `listen` away while it
// listens is told it has connected, should the connection's own telling have failed.
export async function reconcile(storage: Storage, nowMs: number, sender: Sender, listens: boolean): Promise<void> {
  const presence = listens ? "listenConnected" : "listenGone";
  const id = `${presence}:${nowMs}`;
  let failed: unknown = null;
  // One step that fails stops none of the others; the first error is thrown after, so the
  // runtime retries the alarm.
  try {
    await deliverOutbox(storage, "coordinator", nowMs, sender);
    // Slots freed with no answer to send change nothing.
    const room = await loadRoom(storage);
    await coordinatorStep(storage, nowMs, sender, (c) => {
      const answers = reconcileSlots(c, nowMs, room);
      return answers.length > 0 ? answers : null;
    });
  } catch (error) {
    failed = error;
  }
  try {
    const held = await loadTriage(storage);
    if (listens === held.away) sender(await updateTriage(storage, nowMs, (t) => stepTriage(t, { kind: presence, id }, nowMs)));
    const triage = triageDue(await loadTriage(storage));
    if (triage !== null && triage <= nowMs) sender(await updateTriage(storage, nowMs, (t) => expireTriage(t, nowMs)));
  } catch (error) {
    failed ??= error;
  }
  const due = await loadDue(storage);
  const held = await loadHeld(storage);
  for (const issue of await known(storage)) {
    if (!(listens && held.includes(issue)) && !(Object.hasOwn(due, issue) && due[issue] <= nowMs)) continue;
    try {
      // The store's presence first, so no timeout of a command `listen` was gone for runs out,
      // then its timeouts, then each deadline due on the store as they left it, in one write
      // of their own, so a delivery to another store that fails holds none of them up.
      sender(
        await batched(storage, nowMs, async (b) => {
          const send: Command[] = [];
          if (listens === ((await b.load(storeKind(issue))).away)) send.push(...(await b.update(issue, (s) => receive(s, { kind: presence, id, issue }, nowMs))));
          send.push(...(await b.update(issue, (s) => expire(s, nowMs))));
          for (const event of dueDeadlines(await b.load(storeKind(issue)), nowMs)) {
            send.push(...(await b.update(issue, (s) => receive(s, event, nowMs))));
          }
          return send;
        }),
      );
      await deliverOutbox(storage, issue, nowMs, sender);
      await batched(storage, nowMs, (b) => retire(b, issue));
    } catch (error) {
      failed ??= error;
    }
  }
  try {
    const fleet = fleetDue(await loadRun(storage));
    if (fleet !== null && fleet <= nowMs) sender(await updateRun(storage, nowMs, (r) => expireFleet(r, nowMs)));
    await fleetReconciled(storage, nowMs, sender);
  } catch (error) {
    failed ??= error;
  }
  if (failed !== null) throw failed;
}

// Shrinks a settled issue's store to its counters once it has been settled for
// RETIRE_AFTER_MS, and drops what the coordinator keeps of it: its blockers entry, any
// open-PR entry, and the sequence it heard from the store, which the store's own counter now
// outruns. All in one write with the due index's, each record checked at its revision.
async function retire(b: Batch, issue: string): Promise<void> {
  if (!retirable(await b.load(storeKind(issue)), b.nowMs)) return;
  await b.change(storeKind(issue), (s) => [retired(s), null]);
  await b.change(COORDINATOR_KIND, (c) => {
    const next = structuredClone(c);
    delete next.blockers[issue];
    delete next.heard[issue];
    dropPr(next, issue);
    return [next, null];
  });
}
