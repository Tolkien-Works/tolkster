// The work machine's transitions (docs/state-machine.md, "Work transitions"), one per issue.
// Each function is one event's rows, read in order, the first whose condition holds
// winning. "The agent states" are the four that take a slot: entering them from outside
// holds one, and leaving them gives it back (ctx.ts's enterWork).

import { agent, arm, beginWork, disarm, PENDING_MS, closeConfirmed, command, enterWork, FIRST_TURN_MS, launch, merged, notice, noticeSession, openPr, ownOf, reapMerged, restWork, slotHeld, stop, stopAgain, stopOwed as owedOf, workNotice, write, WORK, type Ctx } from "./ctx.ts";
import { branchUpdated, commitUnseen, evaluated, proposalOf, rebaseOnly, rebaseOnlyRound, ROUND_RETRY_MS, roundLanded } from "./rounds.ts";
import { chainRecorded, takeBackDeadline, takeBackDone, workParentHalted } from "./chain.ts";
import { mergeEvent, taken } from "./facts.ts";
import { answered } from "./requests.ts";
import {
  AGENT_STATES, AT_REST_WORK, EVALUATES_PR, HOUR, MINUTE, WORK_ENDED,
  type CommandResult, type DeadlineKind, type Event, type HaltedBy, type ResultData, type Store, type WorkState,
} from "./types.ts";

// How long a finishing worker's PR is left unread for its exit (`finishing`). Counted from the
// PR's opening, never from a deadline armed before it, which could end it at once.
const FINISH_MS = HOUR;

// Armed by each opening and push of the PR while the work hasn't ended, which keeps it across
// its moves (ctx.ts's enterWork): no worker waits on CI, so this reads a head that never
// settles. Only its own read tells the owner, so checks re-run on an old head aren't counted
// from its push.
export function armPending(ctx: Ctx): void {
  if (!WORK_ENDED.includes(ctx.s.work!.state)) arm(ctx, WORK, "pending", PENDING_MS);
}

function finishing(ctx: Ctx): boolean {
  const since = ctx.s.work?.finishing;
  return since !== undefined && ctx.nowMs - since < FINISH_MS;
}

// The pause's notice, said in the session that last gave the worker a request and in each
// other whose claim waits on the worker unanswered: a request handed on before it was
// answered is answered only when the worker goes on, so it is told the pause too. Whether
// the requester was told is what it returns.
function pauseNotice(ctx: Ctx, kind: "paused" | "heldAtPrompt"): boolean {
  const since = `${ctx.s.work!.pausedSinceMs}`;
  for (const [id, sess] of Object.entries(ctx.s.sessions)) {
    if (id !== noticeSession(ctx) && sess.machine?.state === "claimed" && sess.facts.turn === null && !answered(sess)) notice(ctx, WORK, kind, id, `${since}:${id}`);
  }
  return workNotice(ctx, kind, since);
}

function answeredLast(ctx: Ctx): boolean {
  const by = ctx.s.work?.requestedBy;
  if (!by) return false;
  const through = ctx.s.sessions[by.session]?.facts.answeredThrough ?? null;
  return through !== null && through > by.at;
}

// Stalls the worker: at rest in `exited`, marked `stalled`, its notice posted unless it is
// `said` elsewhere. The mark is the work's own, never the notice's key, which a request's own
// `stalled` for the same request shares. It is set first, so the work's change finds it.
function stall(ctx: Ctx, said = false): void {
  const w = ctx.s.work!;
  w.stalled = true;
  if (!said) workNotice(ctx, "stalled", stallKey(ctx));
  // A read owed at its time (a round's retry) outlives the move, which clears the deadlines.
  const recheck = w.deadlines.find((d) => d.kind === "recheck");
  enterWork(ctx, "exited", 24 * HOUR, noticeSession(ctx));
  if (recheck !== undefined) arm(ctx, WORK, "recheck", Math.max(0, recheck.dueMs - ctx.nowMs));
}

function stallKey(ctx: Ctx): string {
  return `${noticeSession(ctx)}:${ctx.s.work?.requestedBy?.at}`;
}

// A stop is owed until its close is confirmed.
function stopOwed(ctx: Ctx): boolean {
  return owedOf(ctx.s, WORK) !== null;
}

// The agent was seen closed since the stop owed was sent: an exit received before it may be
// of an agent the stop was sent again for, and one a later status replaced is no close.
function closedSince(ctx: Ctx): boolean {
  const a = ctx.s.facts.agent;
  const owed = owedOf(ctx.s, WORK);
  return owed !== null && a?.status === "exited" && a.sinceMs > owed.sentMs;
}

function own(ctx: Ctx, ms: number): void {
  arm(ctx, WORK, "own", ms);
}

// Reads the PR, as of the PR facts' version now, which its result marks read only when it
// lands in an evaluating state: those that land while it is read are newer, and read once
// more after its result, so a burst is read once more, never once each.
function evaluate(ctx: Ctx): void {
  command(ctx, WORK, "evaluatePr", { version: ctx.s.facts.prVersion });
}

// The one check, ending each step once the queue is empty and nothing waits (step.ts): the PR
// is read when the work is in an evaluating state, the PR facts are newer than its last read,
// or that read found a failed check or a PR to merge mid-turn, or its merge failed, and the agent's turn is over, unless a recheck
// already paces the retry of a read that failed; or the head's `pending` read is owed, as
// paced.
export function evaluateIfNewer(ctx: Ctx): void {
  const w = ctx.s.work;
  if (w === null || ctx.s.queue.length > 0 || !EVALUATES_PR.includes(w.state) || finishing(ctx) || !openPr(ctx)) return;
  const last = w.prEvaluated;
  const paced = w.deadlines.some((d) => d.kind === "recheck");
  const red = last.redWhileRunning === true && w.state !== "running" && !paced;
  if (ctx.s.facts.prVersion > last.version || red || (w.pendingDue === ctx.s.facts.pr!.head && !paced)) evaluate(ctx);
}

// Halts the worker: the state, then the stop.
export function stopTo(ctx: Ctx, at: string, by: HaltedBy): void {
  if (by === "cancel") ctx.s.work!.cancelledMs = ctx.nowMs;
  // A halt clears the sub-issues' end owed in every session, before the work's change
  // reaches them.
  for (const sess of Object.values(ctx.s.sessions)) sess.facts.orchestrated = null;
  enterWork(ctx, "halted");
  ctx.s.facts.halted = { at, by };
  stop(ctx, WORK);
}

export function workStep(ctx: Ctx, e: Event): void {
  if (e.kind === "ownerCommented") restWork(ctx);
  const w = ctx.s.work;
  if (e.kind === "workRequested") return requested(ctx, e);
  if (e.kind === "chainRecorded") return chainRecorded(ctx, e);
  if (w === null) return;
  const state = w.state;
  const f = ctx.s.facts;
  switch (e.kind) {
    case "stopRequested":
      if (agent(state)) stopTo(ctx, e.at, "stop");
      return;
    case "delegateChanged":
      if (agent(state) && taken(e.by)) stopTo(ctx, e.at, "undelegation");
      return;
    case "parentHalted":
      return workParentHalted(ctx, e);
    case "issueStateChanged":
      return issueState(ctx, e.state);
    case "prOpened":
      armPending(ctx);
      // At rest, the PR's own deadline, and a read: a newer PR's facts merged as stale.
      if (state === "rest") {
        own(ctx, ownOf(w, f)!);
        evaluate(ctx);
      } else if (state === "exited" && w.stalled) enterWork(ctx, "rest");
      // The worker opened it, and finishes: what comes before its exit is read after it.
      else if (state === "running" || state === "idle" || state === "held") {
        w.finishing = ctx.nowMs;
        arm(ctx, WORK, "recheck", FINISH_MS);
      }
      return;
    case "ownerCommented":
      // Every comment is read: a worker at its terminal is told of a plain one, one not
      // running reads the comments with its issue when it next runs, and a reply to a note of
      // Tolkster's opens its thread's request, which `commentTold` must never hide.
      command(ctx, WORK, "readComment", { comment: e.comment, at: e.at });
      return;
    // A landing on the default branch is the work's merge, with no PR.
    case "prMerged":
    case "landed":
      disarm(ctx, WORK, "pending");
      if (AT_REST_WORK.includes(state) && stopOwed(ctx)) {
        // The reap waits for the stop's confirmation. No agent began, so an exit since the
        // stop was sent still confirms it.
        enterWork(ctx, "merged");
        stop(ctx, WORK, true);
      } else if (AT_REST_WORK.includes(state)) {
        enterWork(ctx, "merged");
        reapMerged(ctx);
      } else if (state === "idle" || state === "held") enterWork(ctx, "merged");
      return;
    case "prClosed":
      disarm(ctx, WORK, "pending");
      if (state === "rest") own(ctx, ownOf(w, f)!);
      if (!WORK_ENDED.includes(state)) workNotice(ctx, "prClosed", `${f.pr!.number}:${f.pr!.head}`);
      return;
    // A turn ended to wait on a call in the background is still the worker's work: no `idle`,
    // so no paused notice while its gate runs.
    case "agentBackground":
    case "agentWorking":
      // An observation from before the newest typed line was sent, waiting behind it, is a turn
      // that began before it: it took nothing.
      if (w.unseen !== undefined && (f.agent?.workedMs ?? 0) >= (w.unseenSentMs ?? 0)) commitUnseen(ctx);
      if (w.firstTurn != null) w.firstTurn = null;
      if (w.neverBegan) w.neverBegan = false;
      if (state === "idle" || state === "held") {
        enterWork(ctx, "running");
      } else if (state === "exited" || state === "rest") {
        // An agent continued by hand is its worker again, taking a slot.
        enterWork(ctx, "running");
      } else if (stopOwed(ctx)) stop(ctx, WORK);
      // Its prompt shows idle to the running deadline's `observe`: an hour from here is past
      // the listing's hold (listing.ts, BACKGROUND_HOLD_MS).
      else if (state === "running" && e.kind === "agentBackground") own(ctx, HOUR);
      return commentOwed(ctx);
    case "agentIdle":
      // Idle before a launch's first turn is an agent that hasn't taken its prompt yet.
      if (state === "running" && w.firstTurn != null) return;
      if (state === "running" || state === "held") enterWork(ctx, "idle");
      return;
    case "agentWaiting":
      if (state === "running" || state === "idle") enterWork(ctx, "held");
      return;
    case "agentExited":
      return exited(ctx);
    case "worktreeGone":
      if (state !== "reaped") {
        f.worktree = null;
        enterWork(ctx, "reaped");
      }
      return;
    case "workerAnswered":
      // The worker's own answer to a request since its start is a turn it took: the brief was read.
      if (e.reader === "worker" && w.neverBegan && w.requestedBy !== null && e.at > w.requestedBy.at) w.neverBegan = false;
      if (state === "exited" && answeredLast(ctx)) enterWork(ctx, "rest");
      return;
    case "deadline":
      return deadline(ctx, e.deadline);
    default:
      return;
  }
}

// The owner's reply to a note of Tolkster's older than this isn't answered.
const NOTE_REPLY_FRESH_MS = 7 * 24 * HOUR;

// A reply to a note of Tolkster's, or the owner's comment rooting one, which `readComment`
// found to be the owner's: its thread's request machine begins with it, or takes it as a later prompt, at the reply's `createdAt`.
// A later reply is decided again once the thread's turn ends (rule 4).
function noteReplied(ctx: Ctx, root: string | undefined, at: unknown): void {
  if (root === undefined || typeof at !== "string" || Date.parse(at) < ctx.nowMs - NOTE_REPLY_FRESH_MS) return;
  const base = { id: `noteReplied:${root}:${at}`, issue: ctx.s.issue, session: root, at, byOwner: true };
  const started = ctx.s.facts.sessions[root]?.started === true;
  const event: Event = started ? { ...base, kind: "ownerPrompted" } : { ...base, kind: "sessionStarted", sessionKind: "note" };
  if (mergeEvent(ctx.s, event, ctx.nowMs) === "fresh") ctx.apply(event);
}

function requested(ctx: Ctx, e: Extract<Event, { kind: "workRequested" }>): void {
  const w = ctx.s.work;
  const by = { session: e.session, at: e.at };
  const worktree = ctx.s.facts.worktree?.id ?? null;
  const start = () => void command(ctx, WORK, "startWorker", { session: e.session, orchestrate: e.orchestrate, worktree, host: worktree === null ? (e.host ?? null) : null });
  if (w === null || w.state === "reaped") {
    beginWork(ctx);
    ctx.s.work!.requestedBy = by;
    // A fresh worker has read nothing of the session, whatever an earlier worker of it did.
    const sess = ctx.s.sessions[e.session];
    if (sess !== undefined) sess.facts.readThrough = null;
    return start();
  }
  // A landing was the last request's merge: a worker given more to do may push again.
  if (w.state === "running" || w.state === "idle" || w.state === "launching") delete ctx.s.facts.landed;
  if (w.state === "running" || w.state === "idle") {
    // A worker given more to do isn't finishing.
    w.requestedBy = by;
    w.finishing = undefined;
    enterWork(ctx, "running");
    command(ctx, WORK, "deliverMessage", { session: e.session, at: e.at });
    return;
  }
  if (AT_REST_WORK.includes(w.state)) {
    // A newer request resumes the worker: leaving `halted` drops the stop it owed (ctx.ts,
    // `release`), so it can't close the resumed agent.
    w.requestedBy = by;
    // A worker whose start never began has no turn to continue: it starts again, on its brief.
    if (ctx.s.facts.worktree === null || w.neverBegan) {
      launch(ctx, "start");
      return start();
    }
    launch(ctx, "resume");
    command(ctx, WORK, "resumeWorker", { session: e.session, childrenEnd: e.request === "childrenEnd", childrenAdded: e.request === "childrenAdded" });
    return;
  }
  // launching: the worker reads every unanswered request when its command lands. held,
  // merged: the decision queued the request and saw this state.
  if (w.state === "launching") w.requestedBy = by;
}

// A comment whose line found no agent is told once an agent works again, as the resume or a
// hand-continued agent: the line is typed in a running worker only, so a launch still under
// way keeps it owed.
function commentOwed(ctx: Ctx): void {
  const f = ctx.s.facts;
  const owed = f.commentOwed;
  const state = ctx.s.work!.state;
  if (owed === undefined || (state !== "running" && state !== "idle")) return;
  delete f.commentOwed;
  command(ctx, WORK, "deliverMessage", { comment: true, through: owed });
}

function issueState(ctx: Ctx, to: "open" | "completed" | "cancelled"): void {
  const state = ctx.s.work!.state;
  if (to === "cancelled") {
    if (agent(state)) return stopTo(ctx, ctx.s.facts.issueState!.at, "cancel");
    if (AT_REST_WORK.includes(state)) {
      ctx.s.work!.cancelledMs = ctx.nowMs;
      // A stop still owed is tried again in 2 minutes, as `halted`'s own deadline says.
      enterWork(ctx, "halted", stopOwed(ctx) ? 2 * MINUTE : undefined);
    }
    return;
  }
  // A reopened issue's `halted` reads its state when its deadline comes.
  if (to === "completed" && AT_REST_WORK.includes(state) && ctx.s.facts.pr === null) command(ctx, WORK, "reapWorktree", { rule: "completed" });
}

// The agent's exit drops the rounds whose lines it never took: nothing of them is counted, and
// the PR is read again once the retry's deadline comes, as for a line that typed nothing.
function exited(ctx: Ctx): void {
  const w = ctx.s.work!;
  const dropped = w.unseen !== undefined;
  leaveAgent(ctx);
  if (dropped && EVALUATES_PR.includes(w.state)) arm(ctx, WORK, "recheck", ROUND_RETRY_MS);
}

function leaveAgent(ctx: Ctx): void {
  const w = ctx.s.work!;
  const f = ctx.s.facts;
  switch (w.state) {
    case "running":
    case "idle":
    case "held":
      if (merged(f)) {
        enterWork(ctx, "merged");
        reapMerged(ctx);
      } else if (f.issueState?.value === "completed" && f.pr === null) {
        enterWork(ctx, "rest");
        command(ctx, WORK, "reapWorktree", { rule: "completed" });
      } else if (answeredLast(ctx)) enterWork(ctx, "rest");
      else enterWork(ctx, "exited");
      return;
    case "merged":
    case "halted":
      // The agent's exit since the stop was sent is the stop held, and `merged`'s reap follows it.
      if (stopOwed(ctx)) return closedSince(ctx) ? closeConfirmed(ctx, WORK) : undefined;
      if (w.state !== "merged") return;
      reapMerged(ctx);
      return own(ctx, ownOf(w, f)!);
    default:
      return;
  }
}

function deadline(ctx: Ctx, kind: DeadlineKind): void {
  const w = ctx.s.work!;
  const f = ctx.s.facts;
  if (kind === "own" && stopAgain(ctx, WORK)) return;
  if (kind === "takeBack") return takeBackDeadline(ctx);
  // Owed, and read when the work next evaluates (`evaluateIfNewer`).
  if (kind === "pending") {
    if (openPr(ctx)) w.pendingDue = f.pr!.head;
    return;
  }
  // `recheck`, or a kind no longer armed: only `own` runs the state's rows.
  if (kind !== "own") {
    // A read owed while the worker finishes is made when it stops being so.
    if (finishing(ctx)) arm(ctx, WORK, "recheck", w.finishing! + FINISH_MS - ctx.nowMs);
    else if (EVALUATES_PR.includes(w.state)) evaluate(ctx);
    return;
  }
  switch (w.state) {
    case "running":
      if (w.firstTurn != null) {
        // Never seen working since its launch: it didn't take its prompt. With its PR merged
        // it goes as an idle one does; otherwise it is closed, and stalls as a start that
        // failed after making its worktree does.
        if (merged(f)) return enterWork(ctx, "merged");
        command(ctx, WORK, "closeAgent", {});
        return stall(ctx);
      }
      command(ctx, WORK, "observe", {});
      command(ctx, WORK, "readIssue", {});
      return own(ctx, HOUR);
    case "idle":
    case "held": {
      if (merged(f)) return enterWork(ctx, "merged");
      const told = pauseNotice(ctx, w.state === "idle" ? "paused" : "heldAtPrompt");
      if (!told) {
        command(ctx, WORK, "observe", {});
        command(ctx, WORK, "readIssue", {});
      }
      return own(ctx, HOUR);
    }
    case "exited": {
      if (!w.stalled) {
        // A request decided again after its line typed nothing says what became of it itself,
        // as does one now `done`.
        const by = w.requestedBy;
        const sess = by === null ? undefined : ctx.s.sessions[by.session];
        const state = sess?.machine?.state;
        const deciding = (state === "requested" || state === "queued" || state === "done") && sess!.facts.request?.at === by!.at;
        return stall(ctx, deciding);
      }
      if (openPr(ctx)) evaluate(ctx);
      command(ctx, WORK, "readIssue", {});
      return own(ctx, 24 * HOUR);
    }
    case "rest":
      if (openPr(ctx)) evaluate(ctx);
      command(ctx, WORK, "readIssue", {});
      return own(ctx, ownOf(w, f)!);
    case "halted":
      if (f.issueState?.value !== "cancelled") {
        if (openPr(ctx)) evaluate(ctx);
        command(ctx, WORK, "readIssue", {});
      } else if (!openPr(ctx)) command(ctx, WORK, "reapWorktree", { rule: "cancelled" });
      // A lost close or merge would keep it here for good.
      else evaluate(ctx);
      return own(ctx, 24 * HOUR);
    case "launching":
      // A start sets none: its `lost` ends it.
      if (w.launch === "resume") {
        enterWork(ctx, "rest", undefined, noticeSession(ctx));
        workNotice(ctx, "couldntPass", `${noticeSession(ctx)}:${w.requestedBy?.at}`);
      } else if (w.launch === "round") enterWork(ctx, "rest");
      return;
    case "merged": {
      const a = f.agent;
      if ((a?.status === "idle" || a?.status === "waiting") && ctx.nowMs - a.sinceMs >= 10 * MINUTE) command(ctx, WORK, "closeAgent", {});
      else if (a === null || a.status === "exited" || a.status === "gone") reapMerged(ctx);
      return own(ctx, 10 * MINUTE);
    }
    default:
      return; // reaped sets none; an older one is dropped.
  }
}

// Whether `listen` asked, in the session the notice would go to, about the failure: a newer
// request that arrived meanwhile, in another session, still gets the notice.
function saidIn(ctx: Ctx, data: ResultData, params: Record<string, unknown>): boolean {
  return data.said === true && params.session === noticeSession(ctx);
}

// A command's result for the work machine, in the state it is in when it lands. Its stop's
// never reaches it (step.ts).
export function workResult(ctx: Ctx, kind: string, result: CommandResult, data: ResultData, params: Record<string, unknown>, sentMs: number): void {
  const w = ctx.s.work;
  if (w === null) return;
  const f = ctx.s.facts;
  const state = w.state;
  const ok = result === "ok";
  // A round whose line may have reached the worker is counted, whatever state its worker is
  // in by now, a stop included, but a typed one only once a turn is seen begin; one that
  // failed with nothing typed is not, and its trigger stays unread.
  if (kind === "startRound" && (result !== "failed" || data.typed === true)) roundLanded(ctx, result, params, sentMs);
  // A start that may have launched an agent after the stop closed the last one: the stop again,
  // whether or not one is still owed.
  if (STARTS.includes(kind) && (ok || result === "lost") && (state === "halted" || state === "merged")) stop(ctx, WORK);
  const launching = state === "launching" ? w.launch : null;
  switch (kind) {
    case "startWorker":
      if (launching !== "start") return;
      if (ok) {
        if (data.worktree) {
          // The agent fact is its worktree's: another worktree's agent never stands for this one's.
          if (f.worktree !== null && f.worktree.id !== data.worktree.id) f.agent = null;
          f.worktree = { ...data.worktree, version: w.version };
        }
        // A start that never began, `lost` or failed before its worktree was known, finds it
        // with no agent: sent again, naming it, which listen relaunches in alone. Once.
        if (w.neverBegan && data.launched !== true && params.worktree == null && f.worktree !== null) {
          command(ctx, WORK, "startWorker", { session: params.session, orchestrate: params.orchestrate === true, worktree: f.worktree.id, host: null });
          return;
        }
        // A start that named the worktree and launched nothing was refused there: no relaunch
        // will come, so the marker goes.
        const unbegun = w.neverBegan === true && data.launched !== true && params.worktree == null;
        launched(ctx, data, sentMs);
        w.neverBegan = w.firstTurn != null || unbegun;
        return;
      }
      if (result === "lost" || data.madeWorktree === true) {
        // An agent seen working since the start was sent did take its brief.
        const worked = f.agent?.workedMs;
        w.neverBegan = worked === undefined || worked < sentMs;
        // A start `listen` said had failed, asking for a reply, gets no second message.
        return stall(ctx, saidIn(ctx, data, params));
      }
      f.worktree = null;
      f.agent = null;
      enterWork(ctx, "reaped", undefined, noticeSession(ctx));
      if (!saidIn(ctx, data, params)) workNotice(ctx, "stalled", stallKey(ctx));
      return;
    case "resumeWorker":
    case "startRound":
      // A parent's worker started again in a new worktree, its own gone: the new one is the
      // issue's while the resume is in flight or a stop meanwhile holds it, so the stop finds
      // its agent. Any other state has moved on from the resume.
      if (ok && kind === "resumeWorker" && data.worktree && f.worktree?.id !== data.worktree.id && (launching === "resume" || state === "halted" || state === "merged")) {
        f.agent = null;
        f.worktree = { ...data.worktree, version: w.version };
        // Its slot is held on the old worktree's host: it moves to the new one's.
        if (AGENT_STATES.includes(state)) slotHeld(ctx);
      }
      if ((launching === "resume" && kind === "resumeWorker") || (launching === "round" && kind === "startRound")) {
        if (ok) return launched(ctx, data, sentMs);
        if (launching === "resume") {
          enterWork(ctx, "rest", undefined, noticeSession(ctx));
          if (!saidIn(ctx, data, params)) workNotice(ctx, "couldntPass", `${noticeSession(ctx)}:${w.requestedBy?.at}`);
          return;
        }
        // A line typed into the live worker may yet land: the round stays in flight.
        if (data.typed === true) return enterWork(ctx, "running");
        if (result === "failed") {
          if (data.unnamed === true && f.pr !== null) workNotice(ctx, "roundUnnamed", `${f.pr.number}:${f.pr.head}`);
          // Nothing was typed: the next read is paced by the retry's deadline, not made at
          // once, or a cause that persists (Orca down) would start the round again with no
          // pause.
          return enterWork(ctx, "rest", ROUND_RETRY_MS);
        }
        enterWork(ctx, "rest");
        return evaluate(ctx);
      }
      if ((state === "running" || state === "idle") && result === "held") enterWork(ctx, "held");
      else if (kind === "startRound" && result === "failed" && data.typed !== true && w.inRound != null && !rebaseOnlyRound(params) && !w.unseen?.some((u) => !rebaseOnly(u)) && !ctx.s.queue.some((c) => c.machine === WORK && c.kind === "startRound" && proposalOf(c) !== null && !rebaseOnlyRound(c.params))) {
        // No line reached the worker, nor waits to, so no round is under way to leave feedback
        // to; a rebase-only one's failure leaves the cover of the round it was sent beside.
        write(ctx, WORK);
        w.inRound = null;
      }
      return;
    case "readComment": {
      const c = data.comment;
      // A read that failed is made once more: a reply to a note of Tolkster's is lost without it.
      if (!ok && params.again !== true && typeof params.comment === "string" && typeof params.at === "string") command(ctx, WORK, "readComment", { comment: params.comment, at: params.at, again: true });
      if (!ok || c === undefined) return;
      // Read before Linear had the session its mention began: the session's request tells it,
      // and a mention in a reply to a note opens no turn of its own in the note's thread.
      if (typeof params.comment === "string" && Object.values(f.sessions).some((sess) => sess.comment === params.comment)) return;
      if (c.kind === "noteReply") return noteReplied(ctx, c.root, params.at);
      // The owner's comment on Tolkster's own issue is a live worker's, told of it as of a
      // plain one, or read when it next reads the issue; otherwise it opens its own thread's
      // request. Never both.
      if (c.kind === "noteRoot" && !AGENT_STATES.includes(state)) return noteReplied(ctx, c.root, params.at);
      if (state !== "running" && state !== "idle") return;
      const at = c.newestOwnerAt;
      if ((c.kind === "plain" || c.kind === "noteRoot") && c.state === "open" && at !== null && (f.commentTold === null || at > f.commentTold)) command(ctx, WORK, "deliverMessage", { comment: true, through: at });
      return;
    }
    case "deliverMessage": {
      // A comment's line that found no agent is owed to the worker the exit's resume brings,
      // whatever state the exit has left this one in.
      if (result === "gone" && params.comment === true && typeof params.through === "string" && (f.commentTold === null || params.through > f.commentTold) && (f.commentOwed === undefined || params.through > f.commentOwed)) {
        f.commentOwed = params.through;
        // An agent seen working since the line was sent is one the exit didn't end: told now.
        if ((f.agent?.workedMs ?? 0) >= sentMs) commentOwed(ctx);
      }
      if (state !== "running" && state !== "idle") return;
      // A comment's line that landed covers every owner comment up to the time it carried;
      // one held or failed is told again by the next comment.
      if (ok && typeof params.through === "string" && (f.commentTold === null || params.through > f.commentTold)) f.commentTold = params.through;
      if (f.commentOwed !== undefined && f.commentTold !== null && f.commentOwed <= f.commentTold) delete f.commentOwed;
      if (result === "held") return enterWork(ctx, "held");
      if (ok) return;
      // A request's line that typed nothing, unlike one `lost`, never reached the
      // worker: an exit the observe finds decides it again (requests.ts).
      const sess = typeof params.session === "string" ? ctx.s.sessions[params.session] : undefined;
      if ((result === "failed" || result === "gone") && data.typed !== true && sess !== undefined && typeof params.at === "string") sess.facts.undelivered = params.at;
      command(ctx, WORK, "observe", {});
      return;
    }
    case "evaluatePr": {
      // A result dropped outside the evaluating states, or while the worker finishes, leaves
      // its facts unread. One that
      // failed or was lost marks them read all the same, or a cause that persists would read
      // again at once, its deadline the backstop; a failed check left to the turn's end is kept
      // only while the agent is mid-turn.
      if (!EVALUATES_PR.includes(state) || finishing(ctx)) return;
      // The mark never goes back: a read sent before a new worker's start counted its facts
      // read, or before the version was carried, advances nothing.
      const held = w.prEvaluated.version;
      const version = typeof params.version === "number" ? Math.max(params.version, held) : held;
      const red = !ok && w.prEvaluated.redWhileRunning === true && (state === "running" || state === "idle");
      w.prEvaluated = { version, ...(red ? { redWhileRunning: true } : {}) };
      // An idle agent has no exit or deadline of its own to read it again: the mark stays, and
      // the recheck paces the retry (`evaluateIfNewer`), while an exit reads it at once.
      if (red && state === "idle") arm(ctx, WORK, "recheck", ROUND_RETRY_MS);
      // The owed `pending` read is this one if it read the head owed; one that didn't stays owed
      // while that head is held, its retry paced by the recheck.
      const due = w.pendingDue;
      const paid = ok && data.pr?.head === due;
      if (paid || due !== f.pr?.head) w.pendingDue = undefined;
      else arm(ctx, WORK, "recheck", ROUND_RETRY_MS);
      if (ok && data.pr) evaluated(ctx, data.pr, paid ? due : undefined);
      return;
    }
    case "reapWorktree":
      if (ok && data.reaped === true && (state === "merged" || AT_REST_WORK.includes(state))) {
        f.worktree = null;
        f.agent = null;
        enterWork(ctx, "reaped");
      }
      return;
    case "takeBackChain":
      return takeBackDone(ctx, ok, params.run);
    case "updateBranch":
      return branchUpdated(ctx, result, data, params);
    case "mergePr":
      // Refused, by GitHub or the owner, waits for the next PR fact; a failure of the way, or a
      // loss, is read again in 10 minutes while the PR and its head still hold, or, the recheck
      // dropped by a move, once the work next evaluates with no turn under way (the mark).
      if ((result === "failed" && data.refused !== true) || result === "lost") {
        if (!EVALUATES_PR.includes(state) || !openPr(ctx) || f.pr!.head !== params.head) return;
        w.prEvaluated = { ...w.prEvaluated, redWhileRunning: true };
        arm(ctx, WORK, "recheck", ROUND_RETRY_MS);
      }
      return;
    default:
      return;
  }
}

const STARTS = ["startWorker", "resumeWorker", "startRound", "deliverMessage"];

// Running, after a start, resume or round. One that launched an agent waits for it to be
// seen working: any time since the command was first sent counts, as a resume's hook may
// report before the result lands. A start's own hook can't, its worktree unknown until
// then, so Orca's listing shows it; a start with no worktree named is never watched.
function launched(ctx: Ctx, data: ResultData, sentMs: number): void {
  const worked = ctx.s.facts.agent?.workedMs;
  if (data.launched !== true || data.worktree === null || (worked !== undefined && worked >= sentMs)) {
    enterWork(ctx, "running");
    return commentOwed(ctx);
  }
  enterWork(ctx, "running", FIRST_TURN_MS);
  ctx.s.work!.firstTurn = sentMs;
}

// The coordinator moved the issue's slot, its host having closed before the worker reached it:
// a reservation, and a start not yet on a worktree, take the new host, this machine when absent.
export function slotMoved(s: Store, host: string | undefined): void {
  for (const sess of Object.values(s.sessions)) {
    if (sess.facts.grant !== null) sess.facts.grant = { sinceMs: sess.facts.grant.sinceMs, ...(host === undefined ? {} : { host }) };
  }
  for (const c of s.queue) if (c.kind === "startWorker" && c.params.worktree == null) c.params.host = host ?? null;
}
