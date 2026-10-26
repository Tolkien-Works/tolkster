// The chain and the orchestration of a parent's sub-issues (docs/parent-issues.md, and
// docs/state-machine.md, "Parent issues"): the rows and facts the request machine, the work
// machine and the step take from it, each called from its own `case`.

import { agent, arm, command, disarm, enterRequest, ISSUE, kindOf, send, send as ctxSend, WORK, type Ctx } from "./ctx.ts";
import { related, unfinished, type Merged } from "./facts.ts";
import { delegated, finished, requestAgain, sessionOf } from "./requests.ts";
import { AGENT_STATES, MINUTE, type Event, type Inbound, type IssueFacts, type IssueStateType, type ReadSet, type RequestState, type Session, type Store } from "./types.ts";
import { stopTo } from "./work.ts";

// A chain's delegation stands only while the issue's `chain` names its parent as read, that
// parent is delegated to Tolkster and not cancelled, and no one else has delegated the
// issue since.
export function chainHolds(f: IssueFacts): boolean {
  return f.chain !== null && f.parent !== null && f.chain.parent === f.parent.id && f.parent.delegated && f.parent.state !== "cancelled" && f.delegate?.value === "chain";
}

export function fromChildren(sess: Session): boolean {
  return sess.facts.request?.kind === "childrenEnd" || sess.facts.request?.kind === "childrenAdded";
}

// The sub-issues' end: a request of its own kind, newer than the session's every request,
// read and answer, asked once per orchestration.
export function childrenEnd(ctx: Ctx, id: string): void {
  askAgain(ctx, id, "childrenEnd");
  sessionOf(ctx, id).facts.orchestrated = null;
}

// Only the session's own clocks: another issue's time could make the owner's later stop or
// prompt look stale.
function askAgain(ctx: Ctx, id: string, kind: "childrenEnd" | "childrenAdded"): void {
  const facts = sessionOf(ctx, id).facts;
  const at = [facts.request?.at, facts.readThrough, facts.turnReadThrough, facts.answeredThrough].filter((t): t is string => t !== null && t !== undefined).sort().at(-1)!;
  facts.request = { kind, at };
  requestAgain(ctx, id);
}

// The sub-issues' end when it is owed, or else a pick-up of any sub-issue added.
export function endOrPickUp(ctx: Ctx, id: string): void {
  if (endOwed(ctx, id)) return childrenEnd(ctx, id);
  pickUp(ctx, id);
}

export function endOwed(ctx: Ctx, id: string): boolean {
  const f = ctx.s.facts;
  return sessionOf(ctx, id).facts.orchestrated !== null && childrenFinished(f) && delegated(f) && f.issueState?.value !== "cancelled";
}

// The sub-issues added since the orchestrating worker was last sent to chain: unfinished,
// out of Triage, and neither known to it nor asked or taken by its chain. None unless the
// orchestration is live: its `orchestrated` owed, the issue delegated and not finished, and
// its chain recorded and not halted. A parent with more than 50 never picks up by itself.
export function added(ctx: Ctx, id: string): string[] {
  const f = ctx.s.facts;
  const o = sessionOf(ctx, id).facts.orchestrated;
  const chain = f.chained;
  if (o === null || chain === null || chain.haltedAt !== null || f.children === null || f.children.more || !delegated(f) || finished(f)) return [];
  const known = new Set([...o.known, ...chain.asked, ...chain.issues]);
  return pending(f.children).filter((c) => !known.has(c));
}

// A sub-issue added mid-orchestration brings the worker back to chain it, once it isn't
// running: a request of its own kind, as the sub-issues' end is, which its decision
// re-checks on a fresh read.
export function pickUp(ctx: Ctx, id: string): void {
  const w = ctx.s.work;
  if (w !== null && AGENT_STATES.includes(w.state)) return;
  if (added(ctx, id).length > 0) askAgain(ctx, id, "childrenAdded");
}

export function requestChildChanged(ctx: Ctx, id: string, state: RequestState): void {
  if (state === "done") endOrPickUp(ctx, id);
}

export function requestParentHalted(ctx: Ctx, id: string, state: RequestState): void {
  if ((state === "requested" || state === "queued") && kindOf(ctx.s, id) === "chain") enterRequest(ctx, id, "done");
}

export function workParentHalted(ctx: Ctx, e: Extract<Event, { kind: "parentHalted" }>): void {
  const w = ctx.s.work!;
  const state = w.state;
  const f = ctx.s.facts;
  // `chain` is read as it stood before this halt cleared it (step.ts).
  if (agent(state) && f.chain?.parent === e.parent && kindOf(ctx.s, w.requestedBy?.session ?? "") === "chain" && f.issueState?.value !== "completed") stopTo(ctx, e.at, "parentHalt");
}

// The take-back owed, sent again until it succeeds.
export function takeBackDeadline(ctx: Ctx): void {
  const f = ctx.s.facts;
  if (f.takeBack === null) return;
  command(ctx, WORK, "takeBackChain", { run: f.takeBack.run });
  return arm(ctx, WORK, "takeBack", 2 * MINUTE);
}

// Ends the take-back owed only for the run it was sent for: an older run's result leaves a
// newer one owed.
export function takeBackDone(ctx: Ctx, ok: boolean, run: unknown): void {
  if (!ok || ctx.s.facts.takeBack === null || ctx.s.facts.takeBack.run !== run) return;
  ctx.s.facts.takeBack = null;
  disarm(ctx, WORK, "takeBack");
}

// A parent's chain a reset's rebuild holds for `rebuiltOrchestration` to take back, its run
// naming the orchestrating session, and the read that does.
export const PENDING = "pending:";

export function rebuildRead(s: Store): Record<string, unknown> {
  return { rebuild: true, orchestrated: s.facts.chained!.run.slice(PENDING.length), children: s.facts.chained!.asked };
}

export function chainRecorded(ctx: Ctx, e: Extract<Event, { kind: "chainRecorded" }>): void {
  const f = ctx.s.facts;
  const w = ctx.s.work;
  if (e.role === "ask") {
    // Taken only from the session whose request is orchestrated, while its worker runs, its
    // delegation holds and it isn't cancelled. The sub-issues are those `issue chain` read
    // as the parent's; each takes the chain only if its own parent isn't another.
    const orchestrating = ctx.s.sessions[e.session]?.facts.orchestrated != null;
    if (!orchestrating || w === null || !agent(w.state) || !delegated(f) || f.issueState?.value === "cancelled") return;
    // Nor beside a take-back queued: sent with `chained`'s issues, it would take back what
    // this run delegates. Refused, the run is unanswered and `issue chain` is run again.
    if (ctx.s.queue.some((c) => c.kind === "takeBackChain")) return;
    const children = e.children;
    // Sub-issues an earlier run chained stay in `chained`, so a halt still reaches them.
    f.chained = { run: e.run, issues: f.chained?.issues ?? [], asked: children, answered: [], haltedAt: null };
    // A later chain ends a take-back still owed: it takes back what it delegates again.
    if (f.takeBack !== null) {
      f.takeBack = null;
      disarm(ctx, WORK, "takeBack");
    }
    for (const child of children) send(ctx, child, { kind: "chainRecorded", issue: child, role: "forward", parent: ctx.s.issue, run: e.run });
    return;
  }
  if (e.role === "forward") {
    // A worker still running for a chain whose `chain` is gone was delegated by a parent the
    // issue moved off: the parent it has now leaves it be rather than chain it a second time.
    // A halt stops such a worker as it clears `chain`, so one the parent delegates again takes.
    const ranOn = w !== null && agent(w.state) && f.chain === null && kindOf(ctx.s, w.requestedBy?.session ?? "") === "chain";
    // An issue known to have no parent is no one's sub-issue: a forward sent before it moved
    // off takes nothing.
    const parentless = f.parent === null && f.unparented !== undefined;
    const took = !(f.parent !== null && f.parent.id !== e.parent) && !parentless && !(f.chain !== null && f.chain.parent !== e.parent) && !ranOn;
    if (took) f.chain = { parent: e.parent, run: e.run };
    // A rebuild's (step.ts, `rebuildAsked`) may land after the sub-issue's own rebuilt read
    // found no chain: it reads again, so the chain's delegation still begins, or, for one that
    // is a parent itself (its chain pending), its orchestration. A sub-issue the rebuild never
    // read (`delegate` unknown), one a worker has had, is left to its own events.
    const pendingChain = f.chained?.run.startsWith(PENDING) === true;
    const bare = (w === null || pendingChain) && f.delegate !== null && Object.values(ctx.s.sessions).every((sess) => sess.machine === null) && !ctx.s.queue.some((c) => c.kind === "readIssue");
    if (took && (pendingChain || e.run.startsWith("rebuild:")) && bare) command(ctx, ISSUE, "readIssue", pendingChain ? rebuildRead(ctx.s) : { rebuild: true });
    send(ctx, e.parent, { kind: "chainRecorded", issue: e.parent, role: "answer", child: ctx.s.issue, run: e.run, took });
    return;
  }
  if (f.chained?.run !== e.run || f.chained.answered.includes(e.child)) return;
  f.chained = { ...f.chained, answered: [...f.chained.answered, e.child], issues: e.took && !f.chained.issues.includes(e.child) ? [...f.chained.issues, e.child] : f.chained.issues };
  // A sub-issue that took the chain after the parent was halted is halted too.
  if (e.took && f.chained.haltedAt !== null) send(ctx, e.child, { kind: "parentHalted", issue: e.child, parent: ctx.s.issue, at: f.chained.haltedAt });
}

// A parent taken from Tolkster or cancelled, or halted by its own parent, with `chained`
// not empty: halts each sub-issue it delegated and takes their delegations back, until
// the take-back succeeds, whatever its state.
export function haltChain(ctx: Ctx, at: string): void {
  const f = ctx.s.facts;
  // Halted already, as by a move and a cancel in one event: its sub-issues were told, and
  // any take-back owed.
  if (f.chained === null || f.chained.haltedAt !== null || ctx.s.work === null) return;
  // The run delegates nothing more, and a sub-issue whose answer is still on its way is
  // halted as it lands.
  f.chained = { ...f.chained, haltedAt: at };
  if (f.chained.issues.length === 0) return;
  for (const child of f.chained.issues) send(ctx, child, { kind: "parentHalted", issue: child, parent: ctx.s.issue, at });
  f.takeBack = { run: f.chained.run };
  command(ctx, WORK, "takeBackChain", { run: f.chained.run });
  arm(ctx, WORK, "takeBack", 2 * MINUTE);
}

// A sub-issue's change to its parent, unless `toNow` is false, and to each parent it left,
// which drops it: the one the move names (`from`) and the one the store held (`held`). A
// move off the parent whose chain delegated it ends that chain here, the parent dropping it
// from `chained` as the `childChanged` lands, so neither halts nor takes back the other. A
// chain taken while the parent was unknown names one left too, once the parent is known.
// Unless the owner delegated it, its delegation no longer holds once its chain ends, so the
// chain it made of its own sub-issues halts, as a parent taken from Tolkster's does: nothing
// would pick it up again.
export function toParents(ctx: Ctx, e: { state: IssueStateType; triage?: boolean; from?: string; at: string }, held: string | null, toNow = true): void {
  const s = ctx.s;
  const done = e.state !== "open";
  const now = s.facts.parent?.id ?? null;
  const known = now !== null || s.facts.unparented !== undefined;
  let ended = false;
  for (const left of new Set([e.from ?? null, held, known ? (s.facts.chain?.parent ?? null) : null])) {
    if (left === null || left === now) continue;
    ctxSend(ctx, left, { kind: "childChanged", issue: left, child: s.issue, done, left: true, at: e.at });
    if (s.facts.chain?.parent === left) {
      s.facts.chain = null;
      ended = true;
    }
  }
  if (toNow && now !== null) ctxSend(ctx, now, { kind: "childChanged", issue: now, child: s.issue, done, triage: !done && e.triage === true, at: e.at });
  if (ended && s.facts.delegate?.value !== "owner") {
    const class1 = ctx.class1;
    ctx.class1 = true;
    haltChain(ctx, e.at);
    ctx.class1 = class1;
  }
}

// A halt from its own parent passes on down this issue's chain, once the work machine
// has read the `chain` it clears.
export function passHalt(ctx: Ctx, e: Extract<Inbound, { kind: "parentHalted" }>): void {
  if (ctx.s.facts.chain?.parent === e.parent) {
    ctx.s.facts.chain = null;
    ctx.class1 = true;
    haltChain(ctx, e.at);
    ctx.class1 = false;
  }
}

export function mergeChildChanged(s: Store, e: Extract<Event, { kind: "childChanged" }>, nowMs: number): Merged {
  const f = s.facts;
  if (e.left === true) {
    // Its chain ended as it moved (`toParents`), so even a move this store has
    // newer news of takes it out of a halt and a take-back. One owed already keeps it: the
    // halt ended its chain first, and its delegation is still to be taken back. An answer
    // still on its way, which waits in the queue as this merges at once, counts as given and
    // taken nothing, so neither it nor a move back before it lands chains it again.
    const c = f.chained;
    if (c !== null) {
      const issues = f.takeBack === null ? c.issues.filter((i) => i !== e.child) : c.issues;
      const answered = c.asked.includes(e.child) && !c.answered.includes(e.child) ? [...c.answered, e.child] : c.answered;
      f.chained = { ...c, issues, answered };
    }
    return leftChildren(f.children, e.child, e.at);
  }
  const merged = related(f.children, e.child, e.done, e.at, e.triage === true);
  if (merged === "stale") return "stale";
  // Its sender's `parent` is this issue, so it is a sub-issue though no read lists it yet:
  // only a read sent after it landed says otherwise.
  if (f.children!.ids.includes(e.child)) return merged === "fresh" ? "fresh" : "stale";
  f.children = { ...f.children!, ids: [...f.children!.ids, e.child], added: { ...f.children!.added, [e.child]: nowMs } };
  return "fresh";
}

// A sub-issue moved off: no longer among those it names, and finished at the move, so a read
// sent before the move that lands later, or an older `childChanged`, never counts it again.
function leftChildren(set: ReadSet | null, id: string, at: string): Merged {
  if (related(set, id, true, at) === "stale") return "stale";
  set!.ids = set!.ids.filter((c) => c !== id);
  if (set!.added !== undefined) {
    delete set!.added[id];
    if (Object.keys(set!.added).length === 0) delete set!.added;
  }
  return "fresh";
}

// The unfinished sub-issues that hold up their parent's end: those in Triage, which nobody
// has accepted, don't, even one the chain took and someone moved back there. The chain's
// list is never cleared, so counting such a one would hold the end for good.
export function pending(set: ReadSet | null): string[] {
  return unfinished(set).filter((id) => set?.states[id]?.triage !== true);
}

// Every sub-issue has finished or is in Triage: none pending among those read, and none past
// the first 50, which count as unfinished whatever their state.
export function childrenFinished(f: IssueFacts): boolean {
  return f.children !== null && !f.children.more && pending(f.children).length === 0;
}
