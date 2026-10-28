// The request machine's transitions and the decision (docs/state-machine.md, "Request
// transitions" and "The decision"). Each function is one event's rows, read in order, the
// first whose condition holds winning. The decision is where the owner check and the
// blocker re-check before a claim stay in code: no agent decides what is dispatched.

import { arm, awayKey, closeConfirmed, command, deadlineOf, disarm, enterRequest, ISSUE, kindOf, notice, REQUEST_OWN, restWork, send, settleSlot, worktreeHost, stop, STOP_AGAIN_MS, stopAgain, stopOwed, type Ctx } from "./ctx.ts";
import { added, chainHolds, childrenEnd, childrenFinished, endOrPickUp, endOwed, fromChildren, pending, PENDING, pickUp, requestChildChanged, requestParentHalted } from "./chain.ts";
import { blocked, priorityOf, readOf, session, taken, unfinished } from "./facts.ts";
import { AGENT_STATES, AT_REST_WORK, ATTRIBUTION_RECHECK_MS, ATTRIBUTION_WAIT_MS, AWAY_AFTER_MS, MINUTE, WARM_MS, type Event, type IssueFacts, type Reader, type RequestState, type Session, type WaitReason } from "./types.ts";

export function sessionOf(ctx: Ctx, id: string): Session {
  return ctx.s.sessions[id];
}

function stateOf(ctx: Ctx, id: string): RequestState {
  return ctx.s.sessions[id].machine!.state;
}

export function delegated(f: IssueFacts): boolean {
  return f.delegate?.value === "owner" || (f.delegate?.value === "chain" && chainHolds(f));
}

export function finished(f: IssueFacts): boolean {
  return f.issueState?.value === "completed" || f.issueState?.value === "cancelled";
}

// Rule 5: a conversation, when the session is a mention's or a thread reply's and the issue
// isn't delegated, is blocked or is finished; otherwise work. A note's thread always is one.
export function isConversation(ctx: Ctx, id: string): boolean {
  const f = ctx.s.facts;
  const kind = kindOf(ctx.s, id);
  return kind === "note" || ((kind === "mention" || kind === "reply") && (!delegated(f) || blocked(f) || finished(f)));
}

export function answered(sess: Session): boolean {
  const { request, answeredThrough } = sess.facts;
  return request !== null && answeredThrough !== null && answeredThrough > request.at;
}

function read(ctx: Ctx, id: string): void {
  command(ctx, id, "readIssue", { for: sessionOf(ctx, id).facts.request?.at ?? null });
}

export function requestAgain(ctx: Ctx, id: string): void {
  enterRequest(ctx, id, "requested");
  read(ctx, id);
}

function turnStarting(ctx: Ctx, id: string): boolean {
  return ctx.s.queue.some((c) => c.machine === id && c.kind === "startTurn");
}

// The machine begins at `requested` with its session's first start or prompt.
function begin(ctx: Ctx, id: string): void {
  sessionOf(ctx, id).machine = { state: "requested", version: 0, deadlines: [], stop: null };
  requestAgain(ctx, id);
}

// The owner's delegation with no request machine to take it, as on a store retired or reset:
// Linear opens no new session on an issue that has one, so a machine begins on the session
// rule 3 asks, the owner's first, from the sessions the store keeps, or else those an issue
// read lists (`read`). A store that never sent a command never began a machine, so its
// delegation's own session is still to come, as is one the read lists that hasn't started
// and is no older than the delegation. A session whose own events have reached the store
// begins from them, the delegation recorded beside them. After a reset (`rebuilt`) no start
// is still to come, and a chain's session, which only a start Tolkster's record let in makes
// trusted, is taken as started while the chain the rebuild took back from those records holds.
export function unheldDelegation(ctx: Ctx, read: boolean, rebuilt = false): void {
  const f = ctx.s.facts;
  if (!delegated(f) || f.delegate === null) return;
  const machines = Object.values(ctx.s.sessions).some((s) => s.machine !== null);
  // The owner's kept session rule 3 asks may have no machine beside one that has: another
  // session's decision would leave it to this one.
  const id = askedSession(ctx) ?? (machines ? null : askedSession(ctx, true));
  if (id === null) {
    if (!read && !machines && ctx.s.commandSeq > 0) command(ctx, ISSUE, "readIssue", {});
    return;
  }
  if (ctx.s.sessions[id]?.machine != null) return;
  const held = f.sessions[id];
  if (held.started !== true && held.createdAt >= f.delegate.at && !rebuilt) return;
  if (rebuilt && held.kind === "chain") held.started = true;
  const sess = session(ctx.s, id);
  const own = sess.facts.request !== null;
  recordDelegation(sess, f.delegate.at);
  if (own) return;
  sess.facts.byOwner = f.delegate.value === "owner";
  begin(ctx, id);
}

// The owner's delegation, read by a rebuild (rebuild.ts), whose session the intake never
// mapped to its issue: a worker begun on it couldn't report, so nothing begins. The session is
// asked once for a reply, which the intake maps and which begins the request as any prompt
// does. A start that lands after all begins it, the ask said for nothing.
export function unheardDelegation(ctx: Ctx): void {
  const f = ctx.s.facts;
  if (f.delegate?.value !== "owner" || !delegated(f)) return;
  if (Object.values(ctx.s.sessions).some((s) => s.machine !== null)) return;
  const id = askedSession(ctx, true);
  if (id === null || f.sessions[id].kind === "chain") return;
  notice(ctx, ISSUE, "unheard", id, `${id}:${f.delegate.at}`);
}

// After a reset, the read of a parent whose chain ran on session `id` (step.ts,
// `rebuildAsked`). Only while the read shows its delegation holding and it isn't cancelled: its
// chain taken back, of the sub-issues it lists and `children` names, each sent it, and its
// work at rest unless a worker has begun, so a take-back or cancel halts them; then that
// session owes the sub-issues' end as its orchestration did, its request at rest unless the
// owner began one, asked at once if every sub-issue has finished. A worker has had a parent,
// so nothing here starts one. A chain's delegation not yet holding stays pending, for its
// chain's forward to read again (chain.ts); any other refusal drops what was held.
export function rebuiltOrchestration(ctx: Ctx, id: string, children: string[]): void {
  const f = ctx.s.facts;
  if (!delegated(f) || f.issueState?.value === "cancelled") {
    const awaitsChain = f.delegate?.value === "chain" && f.issueState?.value !== "cancelled";
    if (f.chained?.run.startsWith(PENDING) === true && !awaitsChain) f.chained = null;
    return;
  }
  if (f.chained === null || f.chained.run.startsWith(PENDING)) {
    const run = `rebuild:${ctx.nowMs}`;
    const asked = children.filter((c) => f.children?.ids.includes(c) === true);
    f.chained = { run, issues: [], asked, answered: [], haltedAt: null };
    for (const child of asked) send(ctx, child, { kind: "chainRecorded", issue: child, role: "forward", parent: ctx.s.issue, run });
  }
  restWork(ctx);
  if (f.sessions[id] === undefined || f.sessions[id].kind === "note") return;
  if (f.sessions[id].kind === "chain") f.sessions[id].started = true;
  const sess = session(ctx.s, id);
  recordDelegation(sess, f.delegate!.at);
  sess.facts.orchestrated = { at: sess.facts.request!.at, known: f.chained?.asked ?? [] };
  // A machine the owner began on it since the reset keeps its request, the end owed beside it.
  if (sess.machine === null) {
    sess.facts.byOwner = f.delegate!.value === "owner";
    sess.machine = { state: "requested", version: 0, deadlines: [], stop: null };
    enterRequest(ctx, id, "done");
  }
  if (stateOf(ctx, id) === "done" && endOwed(ctx, id)) childrenEnd(ctx, id);
}

// An event's rows on the request machine, then what it holds and no longer wants given back.
export function requestStep(ctx: Ctx, id: string, e: Event): void {
  step(ctx, id, e);
  settleSlot(ctx, id);
}

function step(ctx: Ctx, id: string, e: Event): void {
  const sess = sessionOf(ctx, id);
  // A session the store has never heard of has no machine and no facts to change; a slot
  // granted to it is given back.
  if (sess === undefined) {
    if (e.kind === "slotGranted") send(ctx, "coordinator", { kind: "slotReleased", issue: ctx.s.issue, session: id });
    return;
  }
  if (sess.machine === null) {
    if ((e.kind === "sessionStarted" || e.kind === "ownerPrompted") && sess.facts.request !== null) begin(ctx, id);
    // A stop in a session whose start never landed, or that a retired store forgot, is told
    // it stopped, as in any other.
    if (e.kind === "ownerStopped" && sess.facts.request?.kind === "stop") {
      sess.machine = { state: "requested", version: 0, deadlines: [], stop: null };
      stopped(ctx, id);
    }
    return;
  }
  const state = stateOf(ctx, id);
  const f = ctx.s.facts;
  switch (e.kind) {
    case "sessionStarted":
      return; // A redelivery the dedupe missed.
    case "ownerPrompted":
      requestAgain(ctx, id);
      return;
    case "ownerStopped":
      return stopped(ctx, id);
    case "delegateChanged": {
      const mine = e.by === "owner" || e.by === "chain";
      // A `requested` request's decision reads the delegate and state, merged before it runs,
      // whether a webhook or its own read brings them.
      if (state === "requested" || state === "queued") {
        if (taken(e.by) && !isConversationBefore(ctx, id) && state !== "requested") {
          enterRequest(ctx, id, "done");
          notice(ctx, id, f.everDelegated ? "noLongerMine" : "notDelegated", id, `${id}:${e.at}`);
        } else if (mine && state === "requested" && sess.facts.unattributed?.for === sess.facts.request?.at) {
          // Waiting on who delegated (rule 6), now said: decided again by a fresh read.
          requestAgain(ctx, id);
        }
      } else if (state === "done" && mine) {
        // Only as the session's newest request: a stop or prompt after it was decided already.
        if (recordDelegation(sess, e.at)) requestAgain(ctx, id);
      } else if (state === "claimed" && mine && kindOf(ctx.s, id) !== "note") {
        // Linear opens no new session for a delegation on an issue that already has one. Kept
        // for a turn's end, whose newer request is decided again as work (a queued `startTurn`
        // holds this event back, and the turn is set before it is queued); a worker's claim on
        // the session rule 3 asks is decided again at once, as a prompt is.
        if (sess.facts.turn !== null) recordDelegation(sess, e.at);
        else if (askedSession(ctx) === id && recordDelegation(sess, e.at)) requestAgain(ctx, id);
      }
      return;
    }
    case "issueStateChanged":
      if (state === "queued" && e.state !== "open") {
        const kind = kindOf(ctx.s, id);
        const work = kind === "delegation" || kind === "chain" || (kind !== "note" && delegated(f) && !blocked(f));
        if (work) {
          enterRequest(ctx, id, "done");
          notice(ctx, id, e.state === "completed" ? "alreadyDone" : "cancelled", id, `${id}:${e.at}`);
        }
      }
      return;
    case "blockersChanged":
      if (state === "queued" && sess.facts.waiting === "blockers") blockersMoved(ctx, id);
      return;
    // Asked again, so the queue ranks the place by it, its age kept. What the decision's
    // own ask is answered with still claims at once.
    case "priorityChanged":
      if (state === "queued" && sess.facts.waiting === "slot") rerank(ctx, id);
      return;
    case "turnEnded":
      return turnEnded(ctx, id, e.terminal);
    case "turnIdle":
      return turnIdle(ctx, id, e.terminal);
    case "workerRead":
      // The sub-issues' end or added is no activity a reader reads: only its decision claims it.
      // Nor is a wait on blockers or the merge: only the blocker re-check, or the reap, ends it.
      // Nor a worker's read once its work has left the agent states: it was stopped.
      if (e.reader === "worker" && !AGENT_STATES.includes(ctx.s.work?.state ?? "reaped")) return;
      if ((state === "requested" || (state === "queued" && (sess.facts.waiting === "slot" || sess.facts.waiting === "held"))) && reached(sess, e.reader) && !fromChildren(sess)) enterRequest(ctx, id, "claimed");
      return;
    case "workerAnswered":
      if (state === "claimed" && sess.facts.request !== null && e.at > sess.facts.request.at && reached(sess, e.reader)) enterRequest(ctx, id, "done");
      return;
    case "workChanged":
      return workChanged(ctx, id, e);
    case "slotGranted":
      return slotGranted(ctx, id, e.answers, e.host);
    case "slotWaiting":
      if (state === "queued" && sess.facts.waiting === "slot") notice(ctx, id, "busy", id, `${id}:${sess.facts.request!.at}`);
      return;
    case "childChanged":
      return requestChildChanged(ctx, id, state);
    case "parentHalted":
      return requestParentHalted(ctx, id, state);
    case "deadline":
      return e.deadline === "quiet" ? sayAway(ctx, id) : e.deadline === "warm" ? closeTurn(ctx, id) : deadline(ctx, id);
    case "commandResult":
      return; // step.ts calls `result` with the command it ends.
  }
}

// "The request is work", as it stood before a delegation was taken: a mention's or reply's
// session was work only while the issue was delegated.
function isConversationBefore(ctx: Ctx, id: string): boolean {
  const kind = kindOf(ctx.s, id);
  return kind === "note" || ((kind === "mention" || kind === "reply") && !ctx.s.facts.everDelegated);
}

// Whether the delegation is the session's newest request.
function recordDelegation(sess: Session, at: string): boolean {
  if (sess.facts.request !== null && sess.facts.request.at >= at) return false;
  sess.facts.request = { kind: "delegation", at };
  return true;
}

// The reader has this request: its own read reached it.
function reached(sess: Session, reader: Reader): boolean {
  const { request } = sess.facts;
  const read = readOf(sess.facts, reader);
  return request !== null && read !== null && read >= request.at;
}

function stopped(ctx: Ctx, id: string): void {
  const sess = sessionOf(ctx, id);
  if (stateOf(ctx, id) === "stopping") return;
  // A stop clears the sub-issues' end owed and any pick-up, so neither resumes a worker the
  // owner stopped: the session's own, and every session's when no worker runs.
  sess.facts.orchestrated = null;
  const w = ctx.s.work;
  if (w === null || !AGENT_STATES.includes(w.state)) for (const other of Object.values(ctx.s.sessions)) other.facts.orchestrated = null;
  if (sess.facts.turn !== null || turnStarting(ctx, id)) {
    enterRequest(ctx, id, "stopping");
    return stop(ctx, id);
  }
  if (w !== null && AGENT_STATES.includes(w.state) && w.requestedBy?.session === id) {
    enterRequest(ctx, id, "done");
    ctx.apply({ kind: "stopRequested", id: `stopRequested:${id}:${sess.facts.request!.at}`, issue: ctx.s.issue, session: id, at: sess.facts.request!.at });
    return;
  }
  enterRequest(ctx, id, "done");
  notice(ctx, id, "stopped", id, `${id}:${sess.facts.request!.at}`);
}

function blockersMoved(ctx: Ctx, id: string): void {
  const open = unfinished(ctx.s.facts.blockers);
  if (!blocked(ctx.s.facts)) return requestAgain(ctx, id);
  // The key names the set, so the same set says nothing and a different one is told.
  notice(ctx, id, "blocked", id, `${id}:${sessionOf(ctx, id).facts.request!.at}:${open.join(",")}`);
}

// `terminal`, when its close ended the turn: one a newer turn replaced since ends nothing.
function turnEnded(ctx: Ctx, id: string, terminal: string | undefined): void {
  const sess = sessionOf(ctx, id);
  const turn = sess.facts.turn;
  const state = stateOf(ctx, id);
  if (turn === null || (terminal !== undefined && turn.terminal !== terminal)) return;
  // Ended whatever the request's state, so the next decision may start a turn.
  sess.facts.turn = null;
  if (state !== "claimed" && state !== "stopping") return;
  // The turn's end received since the stop owed was sent is the stop held; one received
  // before it may be of a turn the stop was sent again for.
  if (state === "stopping") {
    const owed = stopOwed(ctx.s, id);
    if (owed !== null && ctx.receivedMs > owed.sentMs) closeConfirmed(ctx, id);
    return;
  }
  const answered = sess.facts.answeredThrough !== null && sess.facts.answeredThrough > turn.for;
  if (sess.facts.request!.at > turn.for) {
    // A delegation kept for the turn's end that another session asks (rule 3) leaves the
    // turn's own request, unanswered, to no later turn.
    const asked = askedSession(ctx);
    if (sess.facts.request!.kind === "delegation" && !answered && asked !== null && asked !== id) notice(ctx, id, "stalled", id, `${id}:${turn.for}`, { through: turn.for });
    return requestAgain(ctx, id);
  }
  if (answered) {
    // A note's thread's turn answers only the replies it was shown: one that ran, answered and
    // was never shown the request it stands for (taken for it as the turn its record named,
    // after a late end) leaves that request to the next turn, whose batch holds it. One that
    // never ran (no terminal: nothing in its batch, or no repo) is asked nothing again.
    if (kindOf(ctx.s, id) === "note" && turn.terminal !== null && (sess.facts.turnReadThrough ?? "") < sess.facts.request!.at) return requestAgain(ctx, id);
    return enterRequest(ctx, id, "done");
  }
  enterRequest(ctx, id, "done");
  // `through`: the request the turn was for, whose reply a note's thread's `stalled` answers.
  notice(ctx, id, "stalled", id, `${id}:${sess.facts.request!.at}`, { through: turn.for });
}

// The turn's Claude is done with its input. Having answered the request it was given, the
// turn is warm, and a newer request, held for the answer, is decided again, which hands it
// over. Otherwise it stopped short: it is closed, and its end decides as a turn's end does.
// One from a terminal the turn doesn't run in is another turn's, and ignored; a turn whose
// start's result was lost, with no start pending, takes the terminal it reports.
function turnIdle(ctx: Ctx, id: string, terminal: string): void {
  const sess = sessionOf(ctx, id);
  const turn = sess.facts.turn;
  if (turn !== null && turn.terminal === null && !turnStarting(ctx, id)) turn.terminal = terminal;
  if (turn === null || turn.terminal !== terminal || turn.idleMs !== null || turn.closingMs !== null) return;
  if (sess.facts.answeredThrough === null || sess.facts.answeredThrough <= turn.for) return closeTurn(ctx, id);
  turn.idleMs = ctx.nowMs;
  if (stateOf(ctx, id) === "claimed" && sess.facts.request!.at > turn.for) requestAgain(ctx, id);
}

// Closes the turn without a word: one warm too long (its `warm` deadline), one that stopped
// short, or one a request it can't take waits on. Sent again until its end lands.
function closeTurn(ctx: Ctx, id: string): void {
  const turn = sessionOf(ctx, id).facts.turn;
  if (turn === null) return;
  turn.idleMs = null;
  turn.closingMs = ctx.nowMs;
  command(ctx, id, "stopTurn", { idle: true });
}

// A warm turn is handed the request; the turn is its once the hand-over lands.
function handOver(ctx: Ctx, id: string): void {
  const sess = sessionOf(ctx, id);
  outcome(ctx, id, "claimed");
  sess.facts.turn!.idleMs = null;
  command(ctx, id, "startTurn", { request: sess.facts.request!.at, warm: true });
}

// Arms each turn's `warm` deadline: WARM_MS after its Claude was last idle with its request
// answered, or the close again STOP_AGAIN_MS after it was sent; none for a turn that works.
// Run as every step ends, so a state's entry, which drops its deadlines, doesn't lose it.
export function warm(ctx: Ctx): void {
  for (const id of Object.keys(ctx.s.sessions)) {
    const m = sessionOf(ctx, id).machine;
    if (m === null) continue;
    const turn = sessionOf(ctx, id).facts.turn;
    const due = turn?.closingMs != null ? turn.closingMs + STOP_AGAIN_MS : turn?.idleMs != null ? turn.idleMs + WARM_MS : null;
    const armed = deadlineOf(m, "warm");
    if (due === null) disarm(ctx, id, "warm");
    else if (armed?.dueMs !== due) arm(ctx, id, "warm", due - ctx.nowMs);
  }
}

function workChanged(ctx: Ctx, id: string, e: Extract<Event, { kind: "workChanged" }>): void {
  const sess = sessionOf(ctx, id);
  const state = stateOf(ctx, id);
  const wasAgent = e.from !== null && AGENT_STATES.includes(e.from);
  const isAgent = AGENT_STATES.includes(e.to);
  if (state === "queued") {
    const waiting = sess.facts.waiting;
    if ((waiting === "held" && e.from === "held" && e.to !== "held") || (waiting === "merged" && e.to === "reaped")) return requestAgain(ctx, id);
    if (waiting === "slot" && !wasAgent && isAgent && ctx.s.work?.requestedBy?.session !== id) return requestAgain(ctx, id);
    return;
  }
  // A sub-issue added, or the last finishing, while the worker ran is acted on once it has left.
  if (state === "done" && wasAgent && !isAgent) return endOrPickUp(ctx, id);
  // A turn's claim isn't the worker's.
  if (state !== "claimed" || sess.facts.turn !== null || !wasAgent || isAgent) return;
  const undelivered = sess.facts.undelivered === sess.facts.request?.at;
  if (answered(sess) && sess.facts.orchestrated !== null && childrenFinished(ctx.s.facts)) return childrenEnd(ctx, id);
  if (answered(sess) && added(ctx, id).length > 0) return pickUp(ctx, id);
  if (answered(sess) || e.noticed === id) return enterRequest(ctx, id, "done");
  // Decided against a worker that had already exited, whose exit came after: decided again
  // against the exit, which resumes it. A stall is no such exit.
  if (undelivered && e.to === "exited" && ctx.s.work?.stalled !== true) return requestAgain(ctx, id);
  enterRequest(ctx, id, "done");
  notice(ctx, id, "stalled", id, `${id}:${sess.facts.request!.at}`);
}

function slotGranted(ctx: Ctx, id: string, answers: string, host: string | undefined): void {
  const sess = sessionOf(ctx, id);
  const state = stateOf(ctx, id);
  // Recorded either way: one for a request not waiting for it, or for an older request, is
  // given back after the step.
  sess.facts.grant ??= { sinceMs: ctx.nowMs, ...(host === undefined ? {} : { host }) };
  if (sess.facts.waiting === "slot" && (state === "queued" || state === "requested")) {
    if (state === "requested") return; // The read under way decides, holding it.
    // Granted in the step that asked, and the work still at rest or gone: claimed. Should
    // the work have moved since, the re-check decides.
    const w = ctx.s.work;
    if (sess.facts.asked === answers && (w === null || w.state === "reaped" || AT_REST_WORK.includes(w.state))) return claimWork(ctx, id);
    // Granted later: the re-check before the claim.
    return requestAgain(ctx, id);
  }
}

// A request the owner waits on while its store holds `listen` away: one not yet told so.
function waitsAway(ctx: Ctx, id: string): boolean {
  const sess = sessionOf(ctx, id);
  const state = sess.machine?.state;
  const at = sess.facts.request?.at;
  return ctx.s.away && (state === "requested" || state === "claimed") && at !== undefined && !ctx.s.facts.notices.includes(awayKey(id, at));
}

// Arms each waiting request's `quiet` deadline, AWAY_AFTER_MS on, and drops it from one that
// no longer waits: run as every step ends, so `listen`'s absence, its connection and the
// request's own moves all reach it. A state's entry drops its deadlines, so it begins again.
export function quiet(ctx: Ctx): void {
  for (const id of Object.keys(ctx.s.sessions)) {
    const m = sessionOf(ctx, id).machine;
    if (m === null) continue;
    const armed = deadlineOf(m, "quiet") !== undefined;
    if (waitsAway(ctx, id) && !armed) arm(ctx, id, "quiet", AWAY_AFTER_MS);
    else if (!waitsAway(ctx, id) && armed) disarm(ctx, id, "quiet");
  }
}

// The away message, said by the Durable Object itself, as `listen` is the one missing.
function sayAway(ctx: Ctx, id: string): void {
  if (!waitsAway(ctx, id)) return;
  const key = awayKey(id, sessionOf(ctx, id).facts.request!.at);
  ctx.s.facts.notices.push(key);
  command(ctx, id, "postAway", { session: id, key });
}

function deadline(ctx: Ctx, id: string): void {
  const sess = sessionOf(ctx, id);
  const state = stateOf(ctx, id);
  const rearm = () => arm(ctx, id, "own", REQUEST_OWN[state]!);
  if (stopAgain(ctx, id)) return;
  switch (state) {
    case "requested":
      read(ctx, id);
      return rearm();
    case "queued":
      // No webhook says a relation was removed, so the blockers are read again.
      if (sess.facts.waiting === "blockers") read(ctx, id);
      else if (sess.facts.waiting === "slot") ask(ctx, id, false);
      return rearm();
    case "claimed":
      command(ctx, id, "observe", { session: id });
      return rearm();
    default:
      return;
  }
}

// A command's result for a request machine, in the state it is in when it lands, then what
// it holds and no longer wants given back.
export function requestResult(ctx: Ctx, id: string, kind: string, result: string, params: Record<string, unknown>): void {
  onResult(ctx, id, kind, result, params);
  settleSlot(ctx, id);
}

function onResult(ctx: Ctx, id: string, kind: string, result: string, params: Record<string, unknown>): void {
  const sess = sessionOf(ctx, id);
  if (sess.machine === null) return;
  const state = stateOf(ctx, id);
  const ok = result === "ok";
  // What else a read shows has reached every machine already, as its events (step.ts).
  if (kind === "readIssue" && ok) {
    if (state === "requested") decide(ctx, id, params.for as string | null);
    return;
  }
  if (kind === "startTurn") {
    const held = sess.facts.turn?.terminal ?? null;
    if (ok && sess.facts.turn !== null) sess.facts.turn.terminal = (params.terminal as string | undefined) ?? sess.facts.turn.terminal;
    if (params.warm === true && state !== "stopping" && sess.facts.turn !== null && sess.facts.turn.closingMs === null) {
      // The hand-over landed, or opened a turn in place of one closed; else it may not have
      // been typed, and the turn is closed, whose end decides the request again. One that may
      // have opened a turn after a stop is the stop's, below.
      if (ok) sess.facts.turn.for = params.request as string;
      else closeTurn(ctx, id);
      return;
    }
    if (state === "claimed") {
      if (result === "failed") {
        sess.facts.turn = null;
        enterRequest(ctx, id, "done");
        // `listen` asked for a reply itself when it says so: one message per failure.
        if (params.said !== true) notice(ctx, id, "couldntPass", id, `${id}:${sess.facts.request!.at}`);
      }
      return;
    }
    // One that failed opened nothing: a turn held with no terminal was only its own.
    if (state === "stopping" && result === "failed" && sess.facts.turn?.terminal === null && sess.facts.turn.for === params.request) sess.facts.turn = null;
    // A turn that may have begun after the stop closed the last one (one that ran says its
    // terminal): in `stopping`, the stop again; with no turn held, as once the stop was
    // confirmed, a turn nobody holds, which listen finds by the session. A turn still held is
    // the next decision's (rule 4).
    if (result !== "lost" && !(ok && typeof params.terminal === "string")) return;
    // One handed to the turn already held ran before the stop closed it: the stop covered it.
    if (state === "stopping") return ok && params.terminal === held ? undefined : stop(ctx, id);
    if (sess.facts.turn === null) command(ctx, id, "stopTurn");
  }
}

// The newest delegation's session, or else the newest mention's when every session is a
// mention's: the one session a delegation asks. `any`: whoever began them, for a store that
// trusts none of them.
function askedSession(ctx: Ctx, any = false): string | null {
  // Only the owner's sessions and a chain's while it holds count: Linear keeps the session of
  // a delegation the Worker cleared, and a chain's is Tolkster's only while Tolkster made
  // the issue's current delegation.
  const trusted = Object.entries(ctx.s.facts.sessions).filter(([id, s]) => s.kind !== "note" && (s.kind === "chain" ? chainHolds(ctx.s.facts) : any || ctx.s.sessions[id]?.facts.byOwner === true));
  const all = trusted.sort(([, a], [, b]) => (a.createdAt < b.createdAt ? -1 : a.createdAt > b.createdAt ? 1 : 0));
  const delegations = all.filter(([, s]) => s.kind === "delegation" || s.kind === "chain");
  if (delegations.length > 0) return delegations.at(-1)![0];
  if (all.length > 0 && all.every(([, s]) => s.kind === "mention")) return all.at(-1)![0];
  return null;
}

// What a `requested` request becomes once its issue is read. The first rule that holds wins.
function decide(ctx: Ctx, id: string, readFor: string | null): void {
  const sess = sessionOf(ctx, id);
  const f = ctx.s.facts;
  const request = sess.facts.request!;
  // 1. A read sent for an older request: the newer request's own read decides.
  if (readFor !== request.at) return;
  // 2. Only the owner's requests, and those the owner's stand behind.
  const owners = request.kind === "childrenEnd" || request.kind === "childrenAdded" || sess.facts.byOwner || (kindOf(ctx.s, id) === "chain" && chainHolds(f));
  if (!owners) {
    // A chain's delegation, unattributed yet, may still hold.
    if (kindOf(ctx.s, id) === "chain" && awaitsAttribution(ctx, id)) return;
    return refuse(ctx, id);
  }
  // A pick-up the read finds nothing added for, or that a worker running would take as a
  // message: nothing, unless the sub-issues' end is owed, which no later change may bring.
  // One that runs is picked up again once it leaves.
  if (request.kind === "childrenAdded" && (added(ctx, id).length === 0 || (ctx.s.work !== null && AGENT_STATES.includes(ctx.s.work.state)))) {
    outcome(ctx, id, "done");
    if (endOwed(ctx, id)) childrenEnd(ctx, id);
    return;
  }
  // 3. A delegation asks one session.
  if (request.kind === "delegation") {
    const asked = askedSession(ctx);
    if (asked !== null && asked !== id) return outcome(ctx, id, "done");
  }
  // 4. A turn runs here: decided again when it ends. A warm one is handed an owner's message
  // in the conversation at once, and closed for any other request, which its end decides.
  const turn = sess.facts.turn;
  if (turn !== null) {
    if (turn.idleMs !== null && request.kind === "prompt" && f.delegate?.value !== "unknown" && isConversation(ctx, id) && !toItsWorker(ctx, id)) return handOver(ctx, id);
    if (turn.idleMs !== null) closeTurn(ctx, id);
    return outcome(ctx, id, "claimed");
  }
  // Before rule 5: a mention's session holding a delegation is work, which who delegated decides.
  if (kindOf(ctx.s, id) !== "note" && awaitsAttribution(ctx, id)) return;
  // Rule 6's wait over, one that no history entry changed, on an issue the owner created, is
  // theirs: an unwritten entry has had time to show.
  if (kindOf(ctx.s, id) !== "note" && f.delegate?.created === true && f.delegate.value === "unknown") {
    f.delegate = { value: "owner", at: f.delegate.at, created: true };
    f.everDelegated = true;
  }
  const conversation = isConversation(ctx, id);
  if (!conversation) {
    // 6. Work needs the delegation.
    if (!delegated(f)) {
      refuse(ctx, id);
      notice(ctx, id, f.everDelegated ? "noLongerMine" : "notDelegated", id, `${id}:${request.at}`);
      return;
    }
    // 7. Work on a finished issue.
    if (finished(f)) {
      refuse(ctx, id);
      notice(ctx, id, f.issueState!.value === "cancelled" ? "cancelled" : "alreadyDone", id, `${id}:${request.at}`);
      return;
    }
  }
  // 8. The worker was stopped or halted after this request: superseded, and told so, as one
  // the worker had been given is when it halts.
  const haltedAfter = f.halted !== null && f.halted.at > request.at;
  const w = ctx.s.work;
  if (!conversation) {
    if (haltedAfter) {
      outcome(ctx, id, "done");
      // The halt's own stop said it already in the session that gave the worker its request.
      if (w?.requestedBy?.session !== id) notice(ctx, id, "stopped", id, `${id}:${request.at}`);
      return;
    }
    if (w !== null && w.state !== "reaped") return toWorker(ctx, id);
    return noWorker(ctx, id);
  }
  // 11. A conversation: to the worker, or a turn of its own. A note's thread never goes to
  // the worker: its reply is answered in its thread.
  if (toItsWorker(ctx, id)) return toWorker(ctx, id);
  outcome(ctx, id, "claimed");
  sess.facts.turn = { terminal: null, sinceMs: ctx.nowMs, for: request.at, idleMs: null, closingMs: null };
  command(ctx, id, "startTurn", { request: request.at });
}

// Rule 11: a conversation goes to the issue's worker while it is short of `merged` and wasn't
// halted after the request; a note's thread's never does.
function toItsWorker(ctx: Ctx, id: string): boolean {
  const { work: w, facts: f } = ctx.s;
  const haltedAfter = f.halted !== null && f.halted.at > sessionOf(ctx, id).facts.request!.at;
  return kindOf(ctx.s, id) !== "note" && w !== null && w.state !== "merged" && w.state !== "reaped" && !haltedAfter;
}

// The wait on who delegated (rule 6): while no history entry read says, the request is read
// again until the delegation's event or a read that finds the entry does, up to
// ATTRIBUTION_WAIT_MS from the first such decision on it, and then decided as anyone else's.
// True while it waits.
function awaitsAttribution(ctx: Ctx, id: string): boolean {
  const sess = sessionOf(ctx, id);
  const at = sess.facts.request!.at;
  if (ctx.s.facts.delegate?.value !== "unknown") {
    delete sess.facts.unattributed;
    return false;
  }
  if (sess.facts.unattributed?.for !== at) sess.facts.unattributed = { for: at, sinceMs: ctx.nowMs };
  if (ctx.nowMs - sess.facts.unattributed.sinceMs >= ATTRIBUTION_WAIT_MS) return false;
  arm(ctx, id, "own", ATTRIBUTION_RECHECK_MS);
  return true;
}

function refuse(ctx: Ctx, id: string): void {
  outcome(ctx, id, "done");
}

function outcome(ctx: Ctx, id: string, state: RequestState, waiting: WaitReason = null): void {
  delete sessionOf(ctx, id).facts.unattributed;
  enterRequest(ctx, id, state, waiting);
}

// 9. The issue has a work machine short of `reaped`.
function toWorker(ctx: Ctx, id: string): void {
  const sess = sessionOf(ctx, id);
  switch (ctx.s.work!.state) {
    case "running":
    case "idle":
    case "launching":
      return claimWork(ctx, id);
    case "held":
      outcome(ctx, id, "queued", "held");
      notice(ctx, id, "held", id, `${id}:${sess.facts.request!.at}`);
      return;
    case "merged":
      return outcome(ctx, id, "queued", "merged");
    default:
      return slot(ctx, id);
  }
}

// 10. Work with no worker: the blocker re-check before the claim, the repo, then a slot.
function noWorker(ctx: Ctx, id: string): void {
  const sess = sessionOf(ctx, id);
  const f = ctx.s.facts;
  const open = unfinished(f.blockers);
  if (blocked(f)) {
    outcome(ctx, id, "queued", "blockers");
    notice(ctx, id, "blocked", id, `${id}:${sess.facts.request!.at}:${open.join(",")}`);
    return;
  }
  if (f.repo !== null && f.repo.value === null) {
    refuse(ctx, id);
    notice(ctx, id, "noRepo", id, `${id}:${sess.facts.request!.at}`);
    return;
  }
  slot(ctx, id);
}

function slot(ctx: Ctx, id: string): void {
  if (sessionOf(ctx, id).facts.grant !== null) return claimWork(ctx, id);
  enterRequest(ctx, id, "queued");
  ask(ctx, id, true);
}

export function rerank(ctx: Ctx, id: string): void {
  send(ctx, "coordinator", { kind: "slotAsked", issue: ctx.s.issue, session: id, priority: priorityOf(ctx.s.facts), at: sessionOf(ctx, id).facts.request!.at, rerank: true });
}

function ask(ctx: Ctx, id: string, fromDecision: boolean): void {
  const facts = sessionOf(ctx, id).facts;
  facts.waiting = "slot";
  const on = worktreeHost(ctx);
  const asked = send(ctx, "coordinator", { kind: "slotAsked", issue: ctx.s.issue, session: id, priority: priorityOf(ctx.s.facts), at: facts.request!.at, repo: ctx.s.facts.repo?.value ?? null, ...(on === undefined ? {} : { on }) });
  facts.asked = fromDecision ? asked : null;
}

// Claims the request for the issue's worker. One that starts or resumes work hands its
// reservation to the work, which holds it as it enters the agent states: cleared before
// the claim, so nothing is given back from under the work. Any other claim gives it back
// as it enters `claimed`.
function claimWork(ctx: Ctx, id: string): void {
  const sess = sessionOf(ctx, id);
  const request = sess.facts.request!;
  const w = ctx.s.work;
  const startsOrResumes = w === null || w.state === "reaped" || AT_REST_WORK.includes(w.state);
  const host = sess.facts.grant?.host;
  // A reservation has left the queue, so the request holds no place beside it.
  if (startsOrResumes && sess.facts.grant !== null) {
    sess.facts.grant = null;
    sess.facts.waiting = null;
    sess.facts.asked = null;
  }
  enterRequest(ctx, id, "claimed");
  const orchestrate = startsOrResumes && (pending(ctx.s.facts.children).length > 0 || ctx.s.facts.children?.more === true);
  // The sub-issues it is sent to chain: a start's brief, a delegation's and a pick-up's
  // send it to `issue chain`; any other resume keeps those it was sent before.
  const chains = w === null || w.state === "reaped" || ctx.s.facts.worktree === null || request.kind === "delegation" || request.kind === "childrenAdded";
  if (orchestrate) sess.facts.orchestrated = { at: request.at, known: chains ? pending(ctx.s.facts.children) : (sess.facts.orchestrated?.known ?? []) };
  ctx.apply({ kind: "workRequested", id: `workRequested:${id}:${request.kind}:${request.at}`, issue: ctx.s.issue, session: id, at: request.at, request: request.kind, orchestrate, ...(host === undefined ? {} : { host }) });
}
