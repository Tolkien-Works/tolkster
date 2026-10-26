// One event into an issue's store: the pure step, (store, event) → (store, commands to
// send), with the precedence between events that race and the issue's command queue
// (docs/state-machine.md, "Commands" and "Precedence"). Nothing here does I/O; store.ts
// keeps the result with a compare-and-swap and delivers what leaves it.
//
// An event updates its facts at once. A class 1 event's transition runs at once too, even
// with a command in flight, and sends its commands beside it; any other waits until the
// issue has no command queued, and waiting events are then applied one at a time, by class
// and then by arrival. A command result, and the events a transition sends within the
// store (workRequested, stopRequested, workChanged) or the coordinator's answers, are
// applied in the step that sent them, before anything waiting.

import { awayKey, closeConfirmed, command, holdsStop, ISSUE, isStop, machineOf, notice, send as ctxSend, stopOwed, WORK, type Ctx } from "./ctx.ts";
import { childrenFinished, haltChain, passHalt, PENDING, pending as pendingChildren, rebuildRead, toParents } from "./chain.ts";
import { mergeEvent, mergeParent, mergeReadOnly, same, taken, unfinished, type Merged } from "./facts.ts";
import { pruneStore } from "./prune.ts";
import { clearedBy, dueOf, expiry, inFlight, resent, resentCleared, sent, waitsOnListen } from "./queue.ts";
import { quiet, rebuiltOrchestration, requestResult, requestStep, rerank, unheardDelegation, unheldDelegation, warm } from "./requests.ts";
import { RETIRE_AFTER_MS, WAKE_WAIT_MS, type Clears, type Command, type Deadline, type CommandResult, type Event, type Inbound, type IssueRead, type MachineRef, type PrRead, type ResultData, type Store, type Waiting } from "./types.ts";
import { armPending, evaluateIfNewer, slotMoved, workResult, workStep } from "./work.ts";

// Kept when a class 1 event moves their machine, beside a stop its state holds: everything
// else queued is superseded.
const KEPT = ["readIssue", "readComment", "postNotice", "takeBackChain"];

const IMMEDIATE = 0;

export type Received = { store: Store; send: Command[] };

// The class an event's transition takes its turn in. 0 runs at once, in the step that
// sent it; 1 runs at once beside any command in flight; 2 to 5 wait for the queue.
export function classOf(s: Store, e: Inbound): number {
  switch (e.kind) {
    case "ownerStopped":
    case "parentHalted":
      return 1;
    case "delegateChanged":
      return taken(e.by) ? 1 : 2;
    case "issueStateChanged":
      return e.state === "cancelled" ? 1 : 2;
    case "sessionStarted":
    case "ownerPrompted":
    case "blockersChanged":
    case "childChanged":
    case "ownerCommented":
      return 2;
    case "prOpened":
    case "prPushed":
    case "prReviewed":
    case "checksCompleted":
    case "prMerged":
    case "prClosed":
    case "autoMergeDisabled":
    case "mergeAsked":
    case "basePushed":
    case "landed":
      return 3;
    case "agentWorking":
    case "agentBackground":
    case "agentIdle":
    case "agentWaiting":
    case "agentExited":
    case "worktreeGone":
    case "turnEnded":
    case "turnIdle":
    case "workerRead":
    case "workerAnswered":
      return 4;
    case "deadline":
      // A machine's own deadline with a stop owed sends the stop again (ctx.ts, `stopAgain`),
      // and the away message is for a store whose command waits on `listen`.
      return (e.deadline === "own" && stopOwed(s, e.machine) !== null) || e.deadline === "quiet" ? 1 : 5;
    // A chain's ask, forward and answer move no machine, only facts and other stores, so
    // none waits behind a command: `issue chain` is answered in the step its report lands in.
    // Nor does a priority, which only asks the coordinator again.
    case "chainRecorded":
    case "priorityChanged":
    default:
      return IMMEDIATE;
  }
}

function context(s: Store, nowMs: number): Ctx {
  const ctx: Ctx = { s, nowMs, class1: false, moved: new Set(), apply: (event) => transition(ctx, event), receivedMs: nowMs };
  return ctx;
}

// Receives an event: `from` names the store that sent it, and its outbox sequence, for an
// event between stores, whose redelivery is then dropped.
export function receive(store: Store, e: Inbound, nowMs: number, from?: { store: string; seq: number }): Received {
  const s = structuredClone(store);
  if (from !== undefined) {
    if ((s.heard[from.store] ?? 0) >= from.seq) return { store, send: [] };
    s.heard[from.store] = from.seq;
  }
  const ctx = context(s, nowMs);
  let sentAgain: Command[] = [];
  if (e.kind === "commandResult") {
    const index = s.queue.findIndex((c) => c.id === e.command);
    // A result for a command already ended, never sent, or from an older run, is dropped.
    if (index === -1 || s.queue[index].sentMs === null) return { store, send: [] };
    const [cmd] = s.queue.splice(index, 1);
    result(ctx, cmd, e.result, e.data ?? {});
  } else if (e.kind === "deadline") {
    const armed = armedFor(s, e);
    // One whose state was left, that was armed again since, or that waits already, is
    // dropped.
    if (armed === undefined || armed.fired === true) return { store, send: [] };
    armed.fired = true;
    admit(ctx, e, classOf(s, e));
  } else if (e.kind === "slotMoved") {
    slotMoved(s, e.host);
  } else if (e.kind === "listenConnected" || e.kind === "listenGone") {
    const again = presence(s, e.kind === "listenGone", nowMs);
    if (again === null) return { store, send: [] };
    for (const c of again.lost) {
      s.queue = s.queue.filter((q) => q.id !== c.id);
      result(ctx, c, "lost", {});
    }
    // As it now is: a loss may have dropped or superseded a command sent again with it.
    sentAgain = again.resend.filter((c) => s.queue.some((q) => q.id === c.id && !q.superseded));
  } else {
    const held = s.facts.parent?.id ?? null;
    const merged = mergeEvent(s, e, nowMs);
    land(ctx, e, merged, held);
    if (merged === "stale") return finish(s, nowMs);
  }
  drain(ctx);
  const done = finish(s, nowMs);
  return { store: done.store, send: [...sentAgain, ...done.send] };
}

// An event whose facts have merged, a webhook's or one a read shows, on to its transition:
// run now or kept waiting. `held`: the parent the store held before it merged.
function land(ctx: Ctx, e: Event, merged: Merged, held: string | null): void {
  const s = ctx.s;
  if (merged === "stale") {
    // A move older than what the store holds merges nothing, but still tells the parent it
    // left, whose own clock for this issue keeps it from undoing anything newer.
    if (e.kind === "issueStateChanged" && e.from !== undefined) toParents(ctx, e, null, false);
    return;
  }
  const fresh = merged === "fresh";
  // A new base or head is computed again from the start, so it gets its own rechecks, whenever
  // the PR is next read.
  if (fresh && (e.kind === "basePushed" || e.kind === "prPushed") && s.work !== null) s.work.unknownRechecks = 0;
  // A PR fact that asks for `evaluatePr` has no row of its own: it moves the PR facts'
  // version, which the check after the step reads.
  if (EVALUATES.includes(e.kind)) {
    if (fresh) s.facts.prVersion += 1;
    if (fresh && e.kind === "prPushed" && s.work !== null) armPending(ctx);
  } else if (merged === "moved" && e.kind === "issueStateChanged") toParents(ctx, e, held);
  else {
    acrossIssues(ctx, e, held);
    admit(ctx, e, classOf(s, e));
  }
  // A read showing a move off its parent says no priority of that parent's to rank it by.
  if (e.kind === "issueStateChanged" && (e.id !== "read" || s.facts.parent === null) && (s.facts.parent?.id ?? null) !== held) parentMoved(ctx);
}

// A move while a request waits for a slot, on an issue ranked by its parent's priority: off its
// parent, ranked again by none; to another, by a webhook, read for that one's unless a read not
// yet sent will be: one in flight may predate the move.
function parentMoved(ctx: Ctx): void {
  const s = ctx.s;
  const waiting = Object.keys(s.sessions).filter((id) => s.sessions[id].machine?.state === "queued" && s.sessions[id].facts.waiting === "slot");
  if (waiting.length === 0 || (s.facts.priority?.value ?? 0) !== 0) return;
  if (s.facts.parent === null) for (const id of waiting) rerank(ctx, id);
  else if (!s.queue.some((c) => c.machine === ISSUE && c.kind === "readIssue" && c.sentMs === null)) command(ctx, ISSUE, "readIssue", {});
}

// The Worker's acknowledgement of the session's request at `at` was the away message: its
// key is recorded, as the `quiet` deadline's `postAway` records it, so it isn't said again.
// A request not stored (yet, or no longer the session's newest) is left alone.
export function awaySaid(store: Store, session: string, at: string, nowMs: number): Received {
  const key = awayKey(session, at);
  if (store.sessions[session]?.facts.request?.at !== at || store.facts.notices.includes(key)) return { store, send: [] };
  const s = structuredClone(store);
  s.facts.notices.push(key);
  drain(context(s, nowMs));
  return finish(s, nowMs);
}

// The issue completed, at Linear's `at`, and the relay judged it Tolkster's (issue.ts,
// `contextAsked`): its context session, which `listen` decides on from a fresh read and
// guards with its claim, so a redelivery that queues it again starts nothing more. One
// already queued for that completion isn't queued twice.
export function contextAsked(store: Store, at: string, nowMs: number): Received {
  if (store.queue.some((c) => c.kind === "startContext" && c.params.at === at)) return { store, send: [] };
  const s = structuredClone(store);
  command(context(s, nowMs), ISSUE, "startContext", { at });
  return finish(s, nowMs);
}

// After a storage reset, an issue Linear shows delegated to Tolkster (rebuild.ts): the issue's
// own read, on a store with no machine, no work and no read queued, whose result asks the
// owner to reply in their delegation's session (requests.ts, `unheardDelegation`), and begins
// a chain's as for a delegation no machine took (`unheldDelegation`). A parent whose
// chain Tolkster's records name (`chain`: its orchestrating session, and its sub-issues still
// delegated to Tolkster) holds it pending (`PENDING`, chain.ts) and is read for
// `rebuiltOrchestration` until it is taken back, even beside a machine or worker the owner
// began since the reset, so its chain is never lost to it.
export function rebuildAsked(store: Store, nowMs: number, chain: { session: string; children: string[] } | null): Received {
  const reading = store.queue.some((c) => c.kind === "readIssue" && (chain === null || typeof c.params.orchestrated === "string"));
  const held = chain === null ? store.work !== null || Object.values(store.sessions).some((sess) => sess.machine !== null) : store.facts.chained !== null && !store.facts.chained.run.startsWith(PENDING);
  if (held || reading) return { store, send: [] };
  const s = structuredClone(store);
  if (chain !== null) s.facts.chained ??= { run: PENDING + chain.session, issues: [], asked: chain.children, answered: [], haltedAt: null };
  command(context(s, nowMs), ISSUE, "readIssue", chain === null ? { rebuild: true } : rebuildRead(s));
  return finish(s, nowMs);
}

// At `listen`'s connect, an issue Linear shows delegated to Tolkster and not started, on a
// store that has never sent a command (rebuild.ts): its delegation's webhook, or the session
// start it waits on, never landed, and its read asks the owner for a reply (requests.ts,
// `unheardDelegation`). A store that has sent one is left, as one retired keeps no trace of a
// request that ended, and its owner would be asked again.
export function connectAsked(store: Store, nowMs: number): Received {
  if (store.commandSeq > 0) return { store, send: [] };
  const s = structuredClone(store);
  command(context(s, nowMs), ISSUE, "readIssue", { connected: true });
  return finish(s, nowMs);
}

// `listen` gone, while a command waits on it, or connected after the store held it away: the
// commands to send again and those to end `lost`, or null when the store doesn't change.
// With nothing waiting there is nothing to hold. A connection sends each waiting command again
// at once (queue.ts, `resent`): the connection's own sending may have failed, which an alarm
// that finds it connected makes up for. A store that never held `listen` away keeps its
// timeouts running.
function presence(s: Store, gone: boolean, nowMs: number): { resend: Command[]; lost: Command[] } | null {
  if (gone) {
    if (s.away || !s.queue.some(waitsOnListen)) return null;
    s.away = true;
    return { resend: [], lost: [] };
  }
  if (!s.away) return null;
  s.away = false;
  const out: { resend: Command[]; lost: Command[] } = { resend: [], lost: [] };
  s.queue = s.queue.map((c) => {
    if (!waitsOnListen(c)) return c;
    const again = resent(c, nowMs);
    if (again === null) out.lost.push(c);
    else out.resend.push({ ...again });
    return again ?? c;
  });
  return out;
}

// The commands to send, once the step's transitions are done. A store holds `listen` away
// only while a command waits on it.
function finish(s: Store, nowMs: number): Received {
  const send = pump(s, nowMs);
  if (s.away && !s.queue.some(waitsOnListen)) s.away = false;
  pruneStore(s);
  markSettled(s, nowMs);
  return { store: s, send };
}

// The deadline a deadline event names, if it is still the one armed: by its machine, its
// kind and the version that armed it.
function armedFor(s: Store, e: Extract<Event, { kind: "deadline" }>) {
  return machineOf(s, e.machine)?.deadlines.find((d) => d.kind === e.deadline && d.version === e.version);
}

// What any issue sends on, machine or none: a finished or reopened issue to the blocker
// index and its parent, a PR to the open-PR index, and a halt down its chain.
function acrossIssues(ctx: Ctx, e: Event, held: string | null = null): void {
  const s = ctx.s;
  switch (e.kind) {
    case "issueStateChanged": {
      const done = e.state !== "open";
      ctx.class1 = true;
      // A later webhook carries the same `updatedAt`, so the coordinator's fan-out of it
      // is a replay for each blocked issue's facts.
      ctxSend(ctx, "coordinator", { kind: "blockerChanged", issue: s.issue, done, at: e.at });
      toParents(ctx, e, held);
      if (e.state === "cancelled") haltChain(ctx, e.at);
      ctx.class1 = false;
      return;
    }
    case "delegateChanged":
      if (taken(e.by)) {
        ctx.class1 = true;
        haltChain(ctx, e.at);
        ctx.class1 = false;
      }
      return;
    // Its own reaches the sub-issues it has read, which may be queued by it.
    case "priorityChanged":
      if (e.parent === undefined) for (const child of s.facts.children?.ids ?? []) ctxSend(ctx, child, { kind: "priorityChanged", issue: child, priority: e.priority, parent: s.issue, at: e.at });
      return;
    case "prOpened":
    case "prMerged":
    case "prClosed":
      if (s.facts.pr !== null) ctxSend(ctx, "coordinator", { kind: "prRecorded", issue: s.issue, repo: s.facts.pr.repo, open: s.facts.pr.status === "open" });
      return;
    // A landing holds no PR: the one held goes from the store and the open-PR index, so a
    // late event of it is stale.
    case "landed":
      if (s.facts.pr !== null) ctxSend(ctx, "coordinator", { kind: "prRecorded", issue: s.issue, repo: s.facts.pr.repo, open: false });
      s.facts.pr = null;
      return;
    default:
      return;
  }
}

// The PR facts that ask for `evaluatePr`.
const EVALUATES: readonly Inbound["kind"][] = ["prPushed", "prReviewed", "checksCompleted", "autoMergeDisabled", "mergeAsked", "basePushed"];

// Runs the event's transition now, or keeps it waiting for the queue to empty.
function admit(ctx: Ctx, e: Inbound, cls: number): void {
  if (cls === IMMEDIATE || cls === 1 || ctx.s.queue.length === 0) return run(ctx, e, cls);
  ctx.s.waiting.push({ event: e, cls, receivedMs: ctx.nowMs });
}

function run(ctx: Ctx, e: Inbound, cls: number, receivedMs = ctx.nowMs): void {
  ctx.receivedMs = receivedMs;
  if (e.kind === "deadline") {
    // Checked again as it runs: its state may have been left while it waited.
    const armed = armedFor(ctx.s, e);
    if (armed === undefined) return;
    const m = machineOf(ctx.s, e.machine)!;
    m.deadlines = m.deadlines.filter((d) => d !== armed);
  }
  const before = states(ctx.s);
  const queued = new Set(ctx.s.queue.map((c) => c.id));
  ctx.class1 = cls === 1;
  ctx.moved = new Set();
  transition(ctx, e);
  if (cls === 1) supersede(ctx.s, before, queued);
  ctx.class1 = false;
  if (e.kind === "parentHalted") passHalt(ctx, e);
}

function states(s: Store): Map<MachineRef, string> {
  const m = new Map<MachineRef, string>();
  if (s.work) m.set(WORK, s.work.state);
  for (const [id, sess] of Object.entries(s.sessions)) if (sess.machine) m.set(id, sess.machine.state);
  return m;
}

// A class 1 transition's new state supersedes what was queued for each machine it moved:
// those commands are dropped, unless kept, and the one in flight is never sent again,
// unless a take-back or a stop its state holds, its timeouts running on to `lost`. A turn
// held with no terminal goes with the start dropped for it, which can no longer open it.
function supersede(s: Store, before: Map<MachineRef, string>, queued: Set<string>): void {
  const after = states(s);
  const moved = new Set([...after].filter(([ref, state]) => before.get(ref) !== state).map(([ref]) => ref));
  s.queue = s.queue.filter((c) => {
    if (!moved.has(c.machine) || !queued.has(c.id)) return true;
    const owed = c.kind === "takeBackChain" || (isStop(c.machine, c.kind) && holdsStop(s, c.machine));
    if (c.sentMs !== null) {
      if (!owed) c.superseded = true;
      return true;
    }
    const kept = owed || KEPT.includes(c.kind);
    const sess = s.sessions[c.machine];
    if (!kept && c.kind === "startTurn" && sess?.facts.turn?.terminal === null && sess.facts.turn.for === c.params.request) sess.facts.turn = null;
    return kept;
  });
}

// A stop's result, which the machine's rows don't see: only the newest stop's success
// confirms the close. A start's goes on to them, which send the stop again if it may have
// outlived it.
function stopResult(ctx: Ctx, cmd: Command, outcome: CommandResult, data: ResultData): boolean {
  if (!isStop(cmd.machine, cmd.kind)) return false;
  const owed = stopOwed(ctx.s, cmd.machine);
  if (outcome === "ok" && owed?.command === cmd.id) closeConfirmed(ctx, cmd.machine);
  // Told once per stop owed, however often it goes again.
  if (outcome === "failed" && data.unnamed === true && cmd.machine === WORK && owed !== null) notice(ctx, WORK, "stopUnnamed", ctx.s.work?.requestedBy?.session ?? null, `stop:${owed.since}`);
  return true;
}

// An event's rows, on each machine it reaches. A row that turns on a newer fact runs only on
// a fresh merge: a stale event returns before `admit`, and those rows' events merge as fresh
// or stale, never as `none`.
function transition(ctx: Ctx, e: Inbound): void {
  switch (e.kind) {
    case "sessionStarted":
    case "ownerPrompted":
    case "ownerStopped":
    case "turnEnded":
    case "turnIdle":
    case "workerRead":
    case "slotGranted":
    case "slotWaiting":
      return requestStep(ctx, e.session, e);
    case "blockersChanged":
    case "workChanged":
    case "childChanged":
      return eachRequest(ctx, e);
    case "delegateChanged":
      workStep(ctx, e);
      eachRequest(ctx, e);
      // A read's own delegation is its reader's to decide.
      if (e.by === "owner" && e.id !== "read") unheldDelegation(ctx, false);
      return;
    case "issueStateChanged":
    case "parentHalted":
      workStep(ctx, e);
      return eachRequest(ctx, e);
    case "priorityChanged":
      return eachRequest(ctx, e);
    case "workerAnswered":
      if (ctx.s.sessions[e.session]) requestStep(ctx, e.session, e);
      return workStep(ctx, e);
    case "deadline":
      if (e.machine === WORK) return workStep(ctx, e);
      return requestStep(ctx, e.machine, e);
    case "commandResult":
      return;
    default:
      return workStep(ctx, e);
  }
}

function eachRequest(ctx: Ctx, e: Event): void {
  for (const id of Object.keys(ctx.s.sessions)) {
    if (ctx.s.sessions[id].machine === null) continue;
    requestStep(ctx, id, e);
  }
}

// Ends a command with its result, and runs its transition in the state its machine is in
// by now: a class 1 event may have moved it.
function result(ctx: Ctx, cmd: Command, outcome: CommandResult, data: ResultData): void {
  const s = ctx.s;
  // `retryable`: no result yet; it is sent again once what it waits on clears (`cleared`), else
  // at its timeout. One waiting on its worker's machine to wake spends no send while
  // WAKE_WAIT_MS from its first lasts, as a wake takes longer than its sends.
  if (outcome === "retryable") {
    const waking = data.waking === true && ctx.nowMs - (cmd.firstSentMs ?? ctx.nowMs) < WAKE_WAIT_MS;
    s.queue.unshift({ ...cmd, sends: waking ? Math.max(0, cmd.sends - 1) : cmd.sends, waitsOn: data.waitsOn ?? null });
    return;
  }
  if ((cmd.kind === "postNotice" || cmd.kind === "postAway") && outcome !== "ok") {
    // No event may call for it again, and the request it answers would stay unanswered: it goes
    // once more, its key kept. Failing that too clears the key, so a later event says it.
    if (cmd.params.again !== true && (machineOf(s, cmd.machine) !== null || cmd.machine === ISSUE)) command(ctx, cmd.machine, cmd.kind, { ...cmd.params, again: true });
    else s.facts.notices = s.facts.notices.filter((k) => k !== cmd.params.key);
  }
  // A read lands first as the events it shows, as their lost webhooks would have, then as
  // its reader's result.
  const sentMs = cmd.firstSentMs ?? ctx.nowMs;
  let params = cmd.params;
  const version = machineOf(s, cmd.machine)?.version;
  if (cmd.kind === "readIssue" && outcome === "ok" && data.read) {
    readLands(ctx, data.read, sentMs);
    // A request its read's events moved has done what the read shows, or asked again: a read
    // sent since decides.
    if (cmd.machine !== WORK && machineOf(s, cmd.machine)?.version !== version) return;
  }
  if (cmd.kind === "evaluatePr" && outcome === "ok" && data.pr) {
    // A push this read shows is evaluated by this same read: its version counts as read.
    const own = prLands(ctx, data.pr, sentMs);
    if (own > 0 && typeof params.version === "number") params = { ...params, version: params.version + own };
  }
  ctx.moved = new Set();
  if (cmd.kind === "startTurn" && outcome === "ok" && data.terminal !== undefined) cmd.params = { ...cmd.params, terminal: data.terminal };
  if (cmd.kind === "startTurn" && data.said === true) cmd.params = { ...cmd.params, said: true };
  if (cmd.machine === ISSUE) {
    if (cmd.kind !== "readIssue") return;
    if (outcome === "ok" && typeof cmd.params.orchestrated === "string") rebuiltOrchestration(ctx, cmd.params.orchestrated, cmd.params.children as string[]);
    // An owner's delegation a rebuild reads has no session the intake mapped. A connect asks
    // nothing of a chain's; a reset begins it, though the intake never mapped it either.
    else if (outcome === "ok" && (cmd.params.connected === true || (cmd.params.rebuild === true && ctx.s.facts.delegate?.value === "owner"))) unheardDelegation(ctx);
    else if (outcome === "ok") unheldDelegation(ctx, true, cmd.params.rebuild === true);
    // No machine re-reads for it: a read that failed or was lost goes once more.
    else if ((outcome === "failed" || outcome === "lost") && cmd.params.again !== true) command(ctx, ISSUE, "readIssue", { ...cmd.params, again: true });
    return;
  }
  if (stopResult(ctx, cmd, outcome, data)) return;
  if (cmd.machine === WORK) workResult(ctx, cmd.kind, outcome, data, params, sentMs);
  else requestResult(ctx, cmd.machine, cmd.kind, outcome, cmd.params);
}

// A readIssue result: what no event carries merged as read, then the delegation, state and
// parent it shows, and each blocker's and sub-issue's state, as the events their webhooks
// would be. Every fact merges before any transition runs. Which issues block it or are its
// sub-issues is only ever read: a change to them the machines see, with no member's event to
// carry it, lands as one `blockersChanged` or `childChanged` of its own.
function readLands(ctx: Ctx, read: IssueRead, sentMs: number): void {
  const s = ctx.s;
  const f = s.facts;
  const held = f.parent?.id ?? null;
  const ids = [...(f.blockers?.ids ?? [])].sort();
  const blockers = { open: unfinished(f.blockers), more: f.blockers?.more === true };
  const children = pendingChildren(f.children);
  mergeReadOnly(s, read, sentMs);
  const base = { id: "read", issue: s.issue, at: read.updatedAt };
  const events: Event[] = [];
  // An `aged` read keeps a known delegator, and says `unknown` without one. A `created` read
  // keeps the owner it was taken for once waited out, and someone else's delegation or a
  // take-away held, which it can't be: it says `unknown` only over none or the owner's. Neither
  // replaces the chain's while its chain is recorded: a write that left the delegate as it was
  // moves the read's clock all the same. An `unknown` read may have seen it taken since.
  const known = f.delegate !== null && f.delegate.value !== null && f.delegate.value !== "unknown";
  const created = read.delegate === "created";
  const taken = f.delegate?.value === "other" || (f.delegate?.value === null && f.everDelegated);
  const chained = f.delegate?.value === "chain" && f.chain !== null && (read.delegate === "aged" || created);
  const kept = chained || (created ? (f.delegate?.created === true && f.delegate.value === "owner") || taken : read.delegate === "aged" && known);
  if (!kept) events.push({ ...base, kind: "delegateChanged", by: read.delegate === "aged" || created ? "unknown" : read.delegate });
  events.push({ ...base, kind: "issueStateChanged", state: read.state, triage: read.triage === true, parent: read.parent?.id ?? null });
  // After the parent it names has merged, so its priority is kept for that parent.
  events.push({ ...base, kind: "priorityChanged", priority: read.priority });
  if (read.parent !== null) events.push({ ...base, kind: "priorityChanged", priority: read.parent.priority, parent: read.parent.id, at: read.parent.updatedAt });
  // Only the members of the set as merged: a newer read held may have dropped one.
  for (const m of read.blockers) if (f.blockers!.ids.includes(m.id)) events.push({ ...base, kind: "blockersChanged", blocker: m.id, done: m.done, at: m.updatedAt });
  for (const m of read.children) if (f.children!.ids.includes(m.id)) events.push({ ...base, kind: "childChanged", child: m.id, done: m.done, triage: m.triage === true, at: m.updatedAt });
  const merged = events.map((e) => mergeEvent(s, e, ctx.nowMs, sentMs));
  if (created && f.delegate?.value === "unknown" && f.delegate.at === read.updatedAt) f.delegate.created = true;
  mergeParent(s, read);
  const now = [...f.blockers!.ids].sort();
  if (!same(ids, now)) ctxSend(ctx, "coordinator", { kind: "blockersRead", issue: s.issue, sentMs: f.blockers!.sentMs, blockers: f.blockers!.ids });
  events.forEach((e, i) => land(ctx, e, merged[i], held));
  const told = (kind: Event["kind"]) => events.some((e, i) => e.kind === kind && merged[i] !== "stale");
  if (!told("blockersChanged") && (!same(blockers.open, unfinished(f.blockers)) || blockers.more !== (f.blockers!.more === true))) {
    const e: Event = { ...base, kind: "blockersChanged", blocker: "", done: true };
    admit(ctx, e, classOf(s, e));
  }
  if (!told("childChanged") && !same(children, pendingChildren(f.children))) {
    const e: Event = { ...base, kind: "childChanged", child: "", done: true };
    admit(ctx, e, classOf(s, e));
  }
}

// An evaluatePr result's PR, for the PR `pr` holds, as the merge, close or push its lost
// event would be, at its `merged_at`, `closed_at` or `updated_at`. Returns how far the push
// it shows moved the PR facts' version.
function prLands(ctx: Ctx, p: PrRead, sentMs: number): number {
  const s = ctx.s;
  const version = s.facts.prVersion;
  const base = { id: "read", issue: s.issue, pr: p.number, at: p.at };
  const e: Event = p.status === "merged" ? { ...base, kind: "prMerged", head: p.head } : p.status === "closed" ? { ...base, kind: "prClosed" } : { ...base, kind: "prPushed", head: p.head, before: null };
  land(ctx, e, mergeEvent(s, e, ctx.nowMs, sentMs), null);
  return s.facts.prVersion - version;
}

// Applies waiting events once the queue is empty, one at a time, by class and arrival, then,
// with the queue still empty, reads the PR if its facts are newer than its last read: a PR
// read waits behind every command and event, as a PR fact's transition did. Every step ends
// here.
function drain(ctx: Ctx): void {
  const s = ctx.s;
  while (s.queue.length === 0 && s.waiting.length > 0) {
    let next = 0;
    for (let i = 1; i < s.waiting.length; i++) if (s.waiting[i].cls < s.waiting[next].cls) next = i;
    const [w] = s.waiting.splice(next, 1) as Waiting[];
    run(ctx, w.event, w.cls, w.receivedMs);
  }
  evaluateIfNewer(ctx);
  quiet(ctx);
  warm(ctx);
}

// The commands to send now: each class 1 command not yet sent, and the queue's first
// other command while none is in flight.
export function pump(s: Store, nowMs: number): Command[] {
  const out: Command[] = [];
  for (const c of s.queue) if (c.class1 && !inFlight(c)) out.push(sent(c, nowMs));
  if (!s.queue.some((c) => !c.class1 && inFlight(c))) {
    const next = s.queue.find((c) => !c.class1 && !inFlight(c));
    if (next) out.push(sent(next, nowMs));
  }
  return out;
}

// The reconcile for one store: each command past its timeout is sent again or ended as
// `lost` (queue.ts, `expiry`), none while the store holds `listen` away; a superseded one
// counts its timeouts without being sent. Then whatever its loss frees is applied.
export function expire(store: Store, nowMs: number): Received {
  let s = structuredClone(store);
  const send: Command[] = [];
  for (const id of store.queue.map((q) => q.id)) {
    // Looked up in the store as it now is: a loss before it may have replaced it.
    const c = s.queue.find((q) => q.id === id);
    if (c === undefined) continue;
    const next = expiry(c, nowMs, s.away);
    if (next === "wait") continue;
    if (next === "resend") {
      const again = sent(c, nowMs);
      if (!c.superseded) send.push(again);
      continue;
    }
    const lost = receive(s, { kind: "commandResult", id: c.id, issue: s.issue, command: c.id, result: "lost" }, nowMs);
    s = lost.store;
    send.push(...lost.send);
  }
  const ctx = context(s, nowMs);
  drain(ctx);
  send.push(...finish(s, nowMs).send);
  return { store: s, send };
}

// Each command in flight that waits on what `clears` frees is sent again now, its send spent
// only after a clearing's own (queue.ts, `resentCleared`). None while `listen` is away, whose
// connection sends it.
export function cleared(store: Store, clears: Clears, nowMs: number): Received {
  const send = store.away ? [] : store.queue.filter((c) => clearedBy(c, clears)).map((c) => resentCleared(c, nowMs));
  if (send.length === 0) return { store, send };
  const s = structuredClone(store);
  s.queue = s.queue.map((c) => ({ ...c, ...send.find((again) => again.id === c.id) }));
  return { store: s, send };
}

// The deadlines due by `nowMs`, as the events the alarm fires.
export function dueDeadlines(s: Store, nowMs: number): Event[] {
  const due: Event[] = [];
  const add = (machine: MachineRef, m: { deadlines: Deadline[] } | null) => {
    for (const d of m?.deadlines ?? []) {
      if (d.dueMs <= nowMs && d.fired !== true) due.push({ kind: "deadline", id: `deadline:${s.issue}:${machine}:${d.version}:${d.kind}`, issue: s.issue, machine, version: d.version, deadline: d.kind });
    }
  };
  add(WORK, s.work);
  for (const [id, sess] of Object.entries(s.sessions)) add(id, sess.machine);
  return due;
}

// Whether the store has something to drop and nothing left to do: a work machine or a
// request machine (a retired store has none), the work none or `reaped`, every request
// done, no command, event or deadline outstanding, and nothing a later event
// would need its facts for: an open PR, or sub-issues still to finish under a parent that
// ordered them or waits for them.
export function settled(s: Store): boolean {
  const done = (m: { state: string } | null) => m === null || m.state === "done";
  const f = s.facts;
  const parent = f.chained !== null || Object.values(s.sessions).some((sess) => sess.facts.orchestrated !== null);
  return (
    (s.work !== null || Object.values(s.sessions).some((sess) => sess.machine !== null)) &&
    (s.work === null || s.work.state === "reaped") &&
    Object.values(s.sessions).every((sess) => done(sess.machine)) &&
    s.queue.length === 0 && s.waiting.length === 0 && s.outbox.length === 0 &&
    f.pr?.status !== "open" && !(parent && !childrenFinished(f)) &&
    pending(s, 0) === null
  );
}

// Starts or ends the store's settled time, as it stands. Every write of a store does it, so
// an outbox trimmed after a step still starts it.
export function markSettled(s: Store, nowMs: number): void {
  s.settledMs = settled(s) ? (s.settledMs ?? nowMs) : null;
}

// Whether the store has been settled long enough to shrink to its counters (`retired`,
// ctx.ts). It is judged again where it is written, so an event that came since stops it.
export function retirable(s: Store, nowMs: number): boolean {
  return settled(s) && s.settledMs != null && s.settledMs + RETIRE_AFTER_MS <= nowMs;
}

// When the store next needs the alarm: its earliest deadline or command timeout, or now
// while its outbox holds an event not yet delivered; once settled, when it is to retire.
export function nextDue(s: Store, nowMs: number): number | null {
  const first = pending(s, nowMs);
  if (first !== null || s.settledMs == null) return first;
  return s.settledMs + RETIRE_AFTER_MS;
}

function pending(s: Store, nowMs: number): number | null {
  let first: number | null = s.outbox.length > 0 ? nowMs : null;
  const consider = (t: number) => {
    if (first === null || t < first) first = t;
  };
  for (const d of s.work?.deadlines ?? []) if (d.fired !== true) consider(d.dueMs);
  for (const sess of Object.values(s.sessions)) for (const d of sess.machine?.deadlines ?? []) if (d.fired !== true) consider(d.dueMs);
  for (const c of s.queue) {
    const due = dueOf(c, s.away);
    if (due !== null) consider(due);
  }
  return first;
}
