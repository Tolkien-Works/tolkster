// What a transition writes with: its machine's version, its deadlines, its commands, its
// notices and the events it sends. A transition is pure: it changes only the store it is
// given (step.ts's copy) and records what leaves it there, in the queue and the outbox.

import {
  AGENT_STATES, GRANT_UNUSED_MS, HOUR, MINUTE,
  type CommandKind, type CoordinatorEvent, type Deadline, type DeadlineKind, type Inbound, type Machine,
  type IssueFacts, type Launch, type MachineRef, type NoticeKind, type RequestState, type Session, type SessionKind, type StopOwed, type Store, type Unsent, type WaitReason, type WorkMachine, type WorkState,
} from "./types.ts";
import { newCommand } from "./queue.ts";
import { hostOfWorktree } from "../state.ts";

export type Ctx = {
  s: Store;
  nowMs: number;
  // The transition running is a class 1 event's: its commands are sent beside the one in
  // flight.
  class1: boolean;
  // Machines whose version this transition has moved already: a transition moves it once.
  moved: Set<MachineRef>;
  // When the event running was received: what it waited behind may be newer, so a turn's
  // end received before the stop owed was sent doesn't confirm it.
  receivedMs: number;
  // Applies an event in this step, before anything waiting (step.ts).
  apply: (event: Inbound) => void;
};

export const WORK: MachineRef = "work";
// A command the issue sends whatever its machines are, which none holds: it has no version
// to move, no stop owed and no transition on its result. No session's ID has this shape.
export const ISSUE: MachineRef = "issue";

// The session's kind, once its own `sessionStarted` has said it: null before.
export function kindOf(s: Store, id: string): SessionKind | null {
  const held = s.facts.sessions[id];
  return held?.started === true ? held.kind : null;
}

export function machineOf(s: Store, ref: MachineRef): Machine<RequestState> | WorkMachine | null {
  return ref === WORK ? s.work : (s.sessions[ref]?.machine ?? null);
}

// The work machine's helpers that its two modules share (work.ts, rounds.ts).
export function agent(state: WorkState): boolean {
  return AGENT_STATES.includes(state);
}

export function openPr(ctx: Ctx): boolean {
  return ctx.s.facts.pr?.status === "open";
}

// The session that last gave the worker a request, where its notices go.
export function noticeSession(ctx: Ctx): string | null {
  return ctx.s.work?.requestedBy?.session ?? null;
}

export function workNotice(ctx: Ctx, kind: Parameters<typeof notice>[2], key: string): boolean {
  return notice(ctx, WORK, kind, noticeSession(ctx), key);
}

// Launches the worker's command: `launch` names it for `launching`'s rows.
export function launch(ctx: Ctx, kind: Launch): void {
  ctx.s.work!.launch = kind;
  enterWork(ctx, "launching");
}

// Moves the machine's version, once per transition: every write does.
export function write(ctx: Ctx, ref: MachineRef): number {
  const m = machineOf(ctx.s, ref);
  if (m === null) throw new Error(`no machine ${ref}`);
  if (!ctx.moved.has(ref)) {
    m.version += 1;
    ctx.moved.add(ref);
  }
  return m.version;
}

export function arm(ctx: Ctx, ref: MachineRef, kind: DeadlineKind, afterMs: number): void {
  const version = write(ctx, ref);
  const m = machineOf(ctx.s, ref)!;
  m.deadlines = [...m.deadlines.filter((d) => d.kind !== kind), { kind, version, dueMs: ctx.nowMs + afterMs }];
}

export function disarm(ctx: Ctx, ref: MachineRef, kind: DeadlineKind): void {
  const m = machineOf(ctx.s, ref)!;
  if (!m.deadlines.some((d) => d.kind === kind)) return;
  write(ctx, ref);
  m.deadlines = m.deadlines.filter((d) => d.kind !== kind);
}

export function deadlineOf(m: { deadlines: Deadline[] }, kind: DeadlineKind): Deadline | undefined {
  return m.deadlines.find((d) => d.kind === kind);
}

// A note's thread's commands that `listen` carries out in the thread, keyed by its root.
const THREADED: readonly CommandKind[] = ["startTurn", "stopTurn", "observe", "postNotice"];

// Queues a command for the machine, carrying its version after this transition, and returns
// its ID. A note's thread's turn and notices carry `thread`, so `listen` acts in the thread.
export function command(ctx: Ctx, ref: MachineRef, kind: CommandKind, params: Record<string, unknown> = {}): string {
  if (ref !== WORK && kindOf(ctx.s, ref) === "note" && THREADED.includes(kind)) params = { ...params, thread: true };
  const version = ref === ISSUE ? 0 : write(ctx, ref);
  ctx.s.commandSeq += 1;
  const id = `${ctx.s.issue}.${ctx.s.commandSeq}`;
  ctx.s.queue.push(newCommand(id, ctx.s.issue, kind, ref, version, params, ctx.class1));
  return id;
}

// A stop owed (docs/state-machine.md, "Commands"): each machine's stop, and the states that
// hold it. Leaving those states drops the stop owed with its stops: a newer request, a reap,
// or the stop's own end.
const STOPS = {
  work: { stop: "stopWorker", holds: ["halted", "merged"] },
  request: { stop: "stopTurn", holds: ["stopping"] },
} as const satisfies Record<string, { stop: CommandKind; holds: readonly string[] }>;

// A stop not confirmed is sent again this often.
export const STOP_AGAIN_MS = 2 * MINUTE;

function stopsOf(ref: MachineRef) {
  return ref === WORK ? STOPS.work : STOPS.request;
}

export function isStop(ref: MachineRef, kind: CommandKind): boolean {
  return stopsOf(ref).stop === kind;
}

// The machine is in a state that holds a stop.
export function holdsStop(s: Store, ref: MachineRef): boolean {
  const state = machineOf(s, ref)?.state;
  return state !== undefined && (stopsOf(ref).holds as readonly string[]).includes(state);
}

// The stop owed, while the machine's state holds it.
export function stopOwed(s: Store, ref: MachineRef): StopOwed | null {
  return holdsStop(s, ref) ? (machineOf(s, ref)?.stop ?? null) : null;
}

// Sends the machine's stop and records it as the one owed, sending it again in 2 minutes
// unless confirmed by then (`stopAgain`); listen finds the agent or turn itself. `again`, a
// resend with nothing begun since, keeps when the stop was sent, so an exit since still
// confirms it; any other send is for an agent or turn that may have begun since.
export function stop(ctx: Ctx, ref: MachineRef, again = false): void {
  const id = command(ctx, ref, stopsOf(ref).stop);
  const m = machineOf(ctx.s, ref)!;
  m.stop = { command: id, sentMs: again && m.stop !== null ? m.stop.sentMs : ctx.nowMs, since: m.stop?.since ?? ctx.nowMs };
  arm(ctx, ref, "own", STOP_AGAIN_MS);
}

// A machine's own deadline with a stop owed: the stop again. With `listen` away the stop in
// flight waits for the connection, which is sent it, so no other is queued beside it: the
// deadline is only armed again. True when it ran.
export function stopAgain(ctx: Ctx, ref: MachineRef): boolean {
  if (stopOwed(ctx.s, ref) === null) return false;
  if (ctx.s.away) arm(ctx, ref, "own", STOP_AGAIN_MS);
  else stop(ctx, ref, true);
  return true;
}

// The stop owed held: the newest stop's own success, or the agent or turn seen closed since it
// was sent. `merged` goes on to the reap its merge held back, `halted` rests, and a request in
// `stopping` is done, and says "Stopped." once for its request, however many closes it took.
export function closeConfirmed(ctx: Ctx, ref: MachineRef): void {
  const m = machineOf(ctx.s, ref);
  if (m === null || stopOwed(ctx.s, ref) === null) return;
  write(ctx, ref);
  dropStops(ctx, ref);
  if (ref !== WORK) {
    const facts = ctx.s.sessions[ref].facts;
    facts.turn = null;
    enterRequest(ctx, ref, "done");
    notice(ctx, ref, "stopped", ref, `${ref}:${facts.request!.at}`);
    return;
  }
  if (ctx.s.work!.state === "merged") reapMerged(ctx);
  arm(ctx, WORK, "own", restAfterStop(ctx));
}

// The work merged: its PR, or its landing on the default branch with none.
export function merged(f: IssueFacts): boolean {
  return f.pr?.status === "merged" || f.landed !== undefined;
}

// Merged work's reap, on its landing's rule or its PR's.
export function reapMerged(ctx: Ctx): void {
  command(ctx, WORK, "reapWorktree", { rule: ctx.s.facts.landed === undefined ? "merged" : "landed" });
}

// The rest a close leads to: the state's own length, or that long after the cancel of an
// issue still cancelled.
function restAfterStop(ctx: Ctx): number {
  const w = ctx.s.work!;
  const rest = ownOf(w, ctx.s.facts)!;
  if (w.state === "halted" && ctx.s.facts.issueState?.value === "cancelled" && w.cancelledMs !== null) return Math.max(0, w.cancelledMs + rest - ctx.nowMs);
  return rest;
}

// A machine moved to a state that holds no stop drops the one owed.
function release(ctx: Ctx, ref: MachineRef, state: string): void {
  const m = machineOf(ctx.s, ref);
  if (m?.stop == null || (stopsOf(ref).holds as readonly string[]).includes(state)) return;
  dropStops(ctx, ref);
}

// Clears the stop owed: its stops not yet sent go, and one in flight is never sent again, so
// none closes what a newer request opens.
function dropStops(ctx: Ctx, ref: MachineRef): void {
  machineOf(ctx.s, ref)!.stop = null;
  ctx.s.queue = ctx.s.queue.filter((c) => !(c.machine === ref && isStop(ref, c.kind) && c.sentMs === null));
  for (const c of ctx.s.queue) if (c.machine === ref && isStop(ref, c.kind)) c.superseded = true;
}

// Posts a notice once per key: a key already recorded isn't sent again, and a failed
// post clears it (step.ts), so the next event that calls for it says it.
export function notice(ctx: Ctx, ref: MachineRef, kind: NoticeKind, session: string | null, key: string, extra: Record<string, unknown> = {}): boolean {
  const full = `${kind}:${key}`;
  if (ctx.s.facts.notices.includes(full)) return false;
  ctx.s.facts.notices.push(full);
  command(ctx, ref, "postNotice", { ...extra, notice: kind, session, key: full });
  return true;
}

// The away message's key in `notices`, once per request: the Worker's acknowledgement and the
// `quiet` deadline's `postAway` both record it.
export function awayKey(session: string, at: string): string {
  return `away:${session}:${at}`;
}

// An event leaving the store, written with the transition that sends it and delivered
// after that write (store.ts). Its ID is its kind, this store and the outbox sequence, so
// each send is a new event and only a redelivery is dropped.
export function send(ctx: Ctx, to: "coordinator" | string, event: Unsent<Inbound> | Unsent<CoordinatorEvent>): string {
  ctx.s.seq += 1;
  const id = `${event.kind}:${ctx.s.issue}:${ctx.s.seq}`;
  ctx.s.outbox.push({ id, seq: ctx.s.seq, to, event: { ...event, id } as Inbound | CoordinatorEvent });
  return id;
}

// Each state's own deadline length, the one a transition into it arms and a re-arm in it repeats.
export const REQUEST_OWN: Record<RequestState, number | null> = {
  requested: 15 * MINUTE, queued: 5 * MINUTE, claimed: HOUR, done: null, stopping: 2 * MINUTE,
};

// Moves a request to a new state, setting that state's own deadline, and dropping its
// recheck (a request has none) and any it armed in the state it left. `waiting` is what a
// `queued` request waits for besides a slot; a newer request keeps its place in the queue,
// and what else it waited for, it no longer does. What it held that the new state doesn't
// want it gives back (`settleSlot`), as the reason that overwrites `waiting` may hide it.
export function enterRequest(ctx: Ctx, session: string, state: RequestState, waiting: WaitReason = null): void {
  const sess = ctx.s.sessions[session];
  const m = sess.machine!;
  const held = holds(sess);
  write(ctx, session);
  m.state = state;
  // The mark is a claim's: once it is left, it is spent or stale.
  if (state !== "claimed") delete sess.facts.undelivered;
  release(ctx, session, state);
  const own = REQUEST_OWN[state];
  m.deadlines = [];
  if (own !== null) arm(ctx, session, "own", own);
  sess.facts.waiting = waiting ?? (sess.facts.waiting === "slot" && (state === "queued" || state === "requested") ? "slot" : null);
  if (held && !wantsSlot(ctx, sess)) giveBack(ctx, session);
}

// A slot reserved for the request (`grant`), or its place in the queue.
function holds(sess: Session): boolean {
  return sess.facts.grant !== null || sess.facts.waiting === "slot";
}

// What a request may hold: a reservation or a place only while `queued` or `requested` and
// waiting for a slot, and a reservation only until it has gone unused GRANT_UNUSED_MS, as
// when `readIssue` keeps failing. Its next decision asks again.
function wantsSlot(ctx: Ctx, sess: Session): boolean {
  const { grant, waiting } = sess.facts;
  const state = sess.machine?.state;
  if (waiting !== "slot" || (state !== "queued" && state !== "requested")) return false;
  return grant === null || ctx.nowMs - grant.sinceMs < GRANT_UNUSED_MS;
}

// Gives back what the request holds and its state no longer wants: run after each of its
// steps, so no transition gives a slot back itself.
export function settleSlot(ctx: Ctx, session: string): void {
  const sess = ctx.s.sessions[session];
  if (sess?.machine != null && holds(sess) && !wantsSlot(ctx, sess)) giveBack(ctx, session);
}

// Gives back the request's reservation and its place in the queue, clearing its `grant`
// in the same write.
function giveBack(ctx: Ctx, session: string): void {
  const facts = ctx.s.sessions[session].facts;
  facts.grant = null;
  if (facts.waiting === "slot") facts.waiting = null;
  facts.asked = null;
  send(ctx, "coordinator", { kind: "slotReleased", issue: ctx.s.issue, session });
}

// How long a launched agent has to be seen working on its prompt.
export const FIRST_TURN_MS = 5 * MINUTE;

const WORK_OWN: Record<Exclude<WorkState, "launching" | "rest">, number | null> = {
  running: HOUR, idle: 10.5 * MINUTE, held: 10 * MINUTE, exited: 10 * MINUTE, halted: 24 * HOUR, merged: 10 * MINUTE, reaped: null,
};

// Each state's own deadline length, null for none: what `enterWork` arms unless a call
// passes its own, as the stop owed (2 minutes) and a launch's first turn do. A start's
// `lost` ends it; `rest` waits less on an open PR.
export function ownOf(w: WorkMachine, f: IssueFacts): number | null {
  if (w.state === "launching") return w.launch === "start" ? null : 10 * MINUTE;
  if (w.state === "rest") return f.pr?.status === "open" ? 6 * HOUR : 24 * HOUR;
  return WORK_OWN[w.state];
}

// Moves the work to a new state: its own deadline (`own`, null for none, or the state's
// length), the slot it holds or gives back, `workChanged` to the issue's request machines,
// and `noticed`, the session a notice from this transition went to. `launching` takes its
// `launch` as set by the caller.
export function enterWork(ctx: Ctx, state: WorkState, own?: number | null, noticed: string | null = null): void {
  const w = ctx.s.work!;
  const from = w.state;
  write(ctx, WORK);
  w.state = state;
  release(ctx, WORK, state);
  if (state !== "launching") w.launch = null;
  if (state !== "exited") w.stalled = false;
  if (state !== "running" && w.firstTurn != null) w.firstTurn = null;
  if (state !== "running" && state !== "held" && w.launch !== "round") w.inRound = null;
  // Rounds typed to an agent that is no longer one never had their turn.
  if (!AGENT_STATES.includes(state)) {
    w.unseen = undefined;
    w.unseenSentMs = undefined;
    w.finishing = undefined;
  }
  // A deadline armed in the state left is dropped; a take-back owed and a head's checks'
  // deadline survive, and a finishing worker's recheck, which reads at the hold's end, is armed
  // again in the agent state entered.
  const recheck = w.finishing === undefined ? undefined : w.deadlines.find((d) => d.kind === "recheck");
  w.deadlines = w.deadlines.filter(survives);
  if (recheck !== undefined) arm(ctx, WORK, "recheck", Math.max(0, recheck.dueMs - ctx.nowMs));
  let length = own === undefined ? ownOf(w, ctx.s.facts) : own;
  // Staying in `running` keeps a launch's first turn watched as closely.
  if (state === "running" && w.firstTurn != null) length = Math.min(length ?? FIRST_TURN_MS, FIRST_TURN_MS);
  if (length !== null) arm(ctx, WORK, "own", length);
  if ((state === "idle" || state === "held") && from !== "idle" && from !== "held") w.pausedSinceMs = ctx.nowMs;
  const wasAgent = AGENT_STATES.includes(from);
  const isAgent = AGENT_STATES.includes(state);
  if (!wasAgent && isAgent) slotHeld(ctx);
  if (wasAgent && !isAgent) send(ctx, "coordinator", { kind: "slotReleased", issue: ctx.s.issue, session: null });
  if (from !== state) changed(ctx, from, noticed);
}

// How long a head's checks may stay pending after its push before the owner is told: CI takes
// minutes, and a check pending longer likelier waits on a person. No worker waits on them.
export const PENDING_MS = HOUR;

function survives(d: Deadline): boolean {
  return d.kind === "takeBack" || d.kind === "pending";
}

// Begins the work machine at `launching` a start, holding a slot. Its version carries on from the
// machine before it, so no command or deadline of that one matches this one's, and no stop
// is owed: it is `reaped` or none, which hold none. The PR facts held so far count as
// evaluated: the worker reads its PR from its brief, and only those since are evaluated. A
// landing was the last worker's merge, never this one's.
export function beginWork(ctx: Ctx): void {
  delete ctx.s.facts.landed;
  ctx.s.work = { ...newWork("launching", ctx.s.work?.version ?? 0, ctx.s.work?.deadlines.filter(survives) ?? []), launch: "start", prEvaluated: { version: ctx.s.facts.prVersion } };
  write(ctx, WORK);
  // A PR opened before any worker had none armed.
  if (ctx.s.facts.pr?.status === "open" && deadlineOf(ctx.s.work, "pending") === undefined) arm(ctx, WORK, "pending", PENDING_MS);
  slotHeld(ctx);
  changed(ctx, null, null);
}

// The host of the issue's worktree, null for this machine, undefined while it has none.
export function worktreeHost(ctx: Ctx): string | null | undefined {
  const worktree = ctx.s.facts.worktree;
  return worktree === null ? undefined : (hostOfWorktree(worktree.id) ?? null);
}

// The work holds its slot, on its worktree's host once one is known.
export function slotHeld(ctx: Ctx): void {
  const host = worktreeHost(ctx);
  send(ctx, "coordinator", { kind: "slotHeld", issue: ctx.s.issue, ...(host === undefined ? {} : { host }) });
}

// An issue with no work machine gets one at `reaped`, where a worker's absence rests, for a
// comment's read to go through. Nothing else sees it change: a worker begins from it as from none.
export function restWork(ctx: Ctx): void {
  ctx.s.work ??= newWork("reaped");
}

function changed(ctx: Ctx, from: WorkState | null, noticed: string | null): void {
  const w = ctx.s.work!;
  ctx.apply({ kind: "workChanged", id: `workChanged:${ctx.s.issue}:${w.version}`, issue: ctx.s.issue, from, to: w.state, version: w.version, noticed });
}

export function newWork(state: WorkState, version = 0, deadlines: Deadline[] = []): WorkMachine {
  return { state, version, deadlines, stop: null, launch: null, stalled: false, requestedBy: null, prEvaluated: { version: 0 }, unknownRechecks: 0, pausedSinceMs: null, cancelledMs: null };
}

export function newSession(): Session {
  return {
    facts: { request: null, byOwner: false, orchestrated: null, grant: null, waiting: null, asked: null, readThrough: null, turnReadThrough: null, answeredThrough: null, turn: null },
    machine: null,
  };
}

// A settled issue's store once retired: its counters, so a store made again for the issue
// goes on from them (its commands' IDs and its events' sequence numbers must not start over,
// or a post's activity ID, a result, or a receiver's `heard` would take the new ones for the
// old), and each session's kind and start, which no later event carries, and whether the owner
// began it, which a delegation on the session rule 3 asks reads.
export function retired(s: Store): Store {
  const kept = newStore(s.issue);
  kept.facts.sessions = s.facts.sessions;
  for (const [id, sess] of Object.entries(s.sessions)) {
    kept.sessions[id] = newSession();
    kept.sessions[id].facts.byOwner = sess.facts.byOwner;
  }
  return { ...kept, rev: s.rev, seq: s.seq, commandSeq: s.commandSeq };
}

export function newStore(issue: string): Store {
  return {
    issue, rev: 0, sessions: {}, work: null, queue: [], waiting: [], seq: 0, outbox: [], heard: {}, commandSeq: 0, away: false, settledMs: null,
    facts: {
      delegate: null, everDelegated: false, issueState: null, priority: null, blockers: null, parent: null, children: null,
      chain: null, takeBack: null, chained: null, sessions: {}, repo: null, worktree: null, agent: null, halted: null,
      pr: null, checks: {}, mergeState: null, autoMerge: null, feedback: {}, rounds: { total: 0, byHead: {}, lastAt: null, lastBase: null, lastFailed: null },
      base: null, prVersion: 0, notices: [], commentTold: null,
    },
  };
}
