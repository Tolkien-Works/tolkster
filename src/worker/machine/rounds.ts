// A PR's reads and the rounds they start (docs/state-machine.md, "Work transitions"): what a
// read of the PR finds already covered, the round, branch update or merge it calls for, and
// what each result leaves marked.

import { arm, command, launch, openPr, workNotice, write, WORK, agent, type Ctx } from "./ctx.ts";
import { delegated, finished } from "./requests.ts";
import {
  EVALUATES_PR, MINUTE, RECHECK_MS, UNKNOWN_RECHECKS,
  type CommandResult, type IssueFacts, type PrRead, type Proposal, type ResultData,
} from "./types.ts";

// How long after a `startRound` that typed nothing the PR is read again.
export const ROUND_RETRY_MS = 10 * MINUTE;

// A round's proposal stays in the queue with its command and isn't sent.
export function proposalOf(c: { kind: string; params: Record<string, unknown> }): Proposal | null {
  return c.kind === "startRound" && typeof c.params.proposal === "object" && c.params.proposal !== null ? (c.params.proposal as Proposal) : null;
}

// `rounds` with each round still in the queue counted and its markers moved: what a read of
// the PR finds already covered.
function covered(ctx: Ctx): IssueFacts["rounds"] {
  let r = ctx.s.facts.rounds;
  for (const p of ctx.s.work?.unseen ?? []) r = marked(r, p);
  for (const c of ctx.s.queue) {
    const p = c.machine === WORK ? proposalOf(c) : null;
    if (p !== null) r = marked(r, p);
    const base = c.machine === WORK ? updating(c) : null;
    if (base !== null) r = { ...r, lastBase: base };
  }
  return r;
}

// A rebase-only round marks its base and head but is not counted against the cap.
function marked(r: IssueFacts["rounds"], p: Proposal): IssueFacts["rounds"] {
  return {
    total: rebaseOnly(p) ? r.total : r.total + 1,
    byHead: { ...r.byHead, [p.head]: (r.byHead[p.head] ?? 0) + 1 },
    lastAt: p.lastAt !== undefined && (r.lastAt === null || p.lastAt > r.lastAt) ? p.lastAt : r.lastAt,
    lastBase: p.lastBase ?? r.lastBase,
    lastFailed: p.lastFailed ?? r.lastFailed,
  };
}

function roundCommitted(ctx: Ctx, p: Proposal): void {
  write(ctx, WORK);
  ctx.s.facts.rounds = marked(ctx.s.facts.rounds, p);
}

// A round's result that says its line may have reached the worker. A line typed with no turn
// seen begin since it was sent is not yet taken: it waits (`unseen`) for the agent to be seen
// working, or to exit, which an exit still waiting behind this result does next. With no agent
// state left it never was, and its trigger is read again.
export function roundLanded(ctx: Ctx, result: CommandResult, params: Record<string, unknown>, sentMs: number): void {
  const p = proposalOf({ kind: "startRound", params });
  if (p === null) return;
  const w = ctx.s.work!;
  const worked = ctx.s.facts.agent?.workedMs;
  if (result !== "failed" || (worked !== undefined && worked >= sentMs)) return roundCommitted(ctx, p);
  write(ctx, WORK);
  if (!agent(w.state)) {
    if (EVALUATES_PR.includes(w.state)) arm(ctx, WORK, "recheck", ROUND_RETRY_MS);
    return;
  }
  w.unseen = [...(w.unseen ?? []), p];
  w.unseenSentMs = Math.max(w.unseenSentMs ?? 0, sentMs);
}

export function commitUnseen(ctx: Ctx): void {
  const w = ctx.s.work!;
  write(ctx, WORK);
  for (const p of w.unseen!) ctx.s.facts.rounds = marked(ctx.s.facts.rounds, p);
  w.unseen = undefined;
  w.unseenSentMs = undefined;
}

// The PR as evaluatePr read it, for a worker running or at rest with its PR.
export function evaluated(ctx: Ctx, p: PrRead, due?: string): void {
  const w = ctx.s.work!;
  const f = ctx.s.facts;
  const running = w.state === "running" || w.state === "idle";
  // Only a read of the PR held, open in the read and as held, at the head held and no older
  // than what is held, decides a round or notice: a merge, close or push it showed was
  // applied already, so a read of another head is one older than the push held, or from its
  // second and sent before it was received.
  if (p.status !== "open" || f.pr === null || f.pr.number !== p.number || f.pr.status !== "open" || f.pr.head !== p.head || p.at < f.pr.at) return;
  f.mergeState = { value: p.mergeState, base: p.base };
  if (p.autoMerge !== undefined) f.autoMerge = { pr: p.number, on: p.autoMerge };
  if (p.mergeState !== "UNKNOWN") w.unknownRechecks = 0;
  // The issue no longer wants work: taken back or finished.
  if (!delegated(f) || finished(f)) return;
  // The owed read's verdict comes first: nothing below may take its one chance.
  if (p.checks === "pending" && due === p.head) workNotice(ctx, "checksPending", `${f.pr.number}:${p.head}`);
  // A round under way on this head reads every feedback itself, so what landed since it began
  // is its to read, not another round's: `lastAt` moves up to it, and nothing is counted.
  // While its line is still in flight that is its proposal's, which a failure drops with it.
  const newer = (at: string | null | undefined, r: IssueFacts["rounds"]) => typeof at === "string" && (r.lastAt === null || at > r.lastAt);
  if (w.inRound === p.head && newer(p.feedback?.at, covered(ctx))) {
    write(ctx, WORK);
    const flight = ctx.s.queue.find((c) => c.machine === WORK && proposalOf(c)?.head === p.head && !rebaseOnlyRound(c.params));
    const unseen = w.unseen?.findLast((u) => u.head === p.head && !rebaseOnly(u));
    if (flight !== undefined) flight.params = { ...flight.params, proposal: { ...proposalOf(flight)!, lastAt: p.feedback!.at } };
    else if (unseen !== undefined) unseen.lastAt = p.feedback!.at;
    else f.rounds = { ...f.rounds, lastAt: p.feedback!.at };
  }
  const r = covered(ctx);
  const since = newer(p.feedback?.at, r);
  const rebase = (p.mergeState === "DIRTY" || p.mergeState === "BEHIND") && p.base !== r.lastBase;
  // A failed check brings the worker back once per head, however many suites report it. An
  // agent mid-turn may be fixing it, so the failure is left unmarked
  // and read again once the turn ends, idle or exited, even past a round for feedback: a line
  // held or lost there must not hide it. An idle agent's turn is over: it is told in its
  // open terminal, as feedback is.
  const red = p.checks === "failed" && r.lastFailed !== p.head;
  const mid = w.state === "running";
  if (red && mid) w.prEvaluated = { ...w.prEvaluated, redWhileRunning: true };
  const failed = red && !mid;
  const under = r.total < p.maxRounds;
  // Past the cap only a reviewer's other than Copilot resumes the worker, even when Copilot
  // wrote after it.
  const reviewerAtCap = newer(p.feedback?.reviewerAt, r);
  // A PR only behind its base, with no agent running, has no conflict to resolve: GitHub
  // brings it up to date as the app, leased on the head read, with no worker and no round.
  // One it refuses goes to a worker as a round would (`branchUpdated`). The notices below
  // still apply.
  const updating = rebase && !since && !failed && p.mergeState === "BEHIND" && !running;
  if (updating) command(ctx, WORK, "updateBranch", { head: p.head, base: p.base });
  // A base moved past the cap still resumes the worker, with a round that counts nothing: what
  // else is owed stays for the notice below.
  const resumes = under || reviewerAtCap;
  const rebasing = rebase && !updating;
  if (((since || failed) && resumes) || rebasing) {
    write(ctx, WORK);
    const proposal: Proposal = { head: p.head, ...(since && resumes ? { lastAt: p.feedback!.at } : {}), ...(rebase ? { lastBase: p.base } : {}), ...(failed && resumes ? { lastFailed: p.head } : {}) };
    startRound(ctx, running, proposal);
    return;
  }
  if ((since || failed) && !under) {
    workNotice(ctx, "roundsCap", `${f.pr?.number}:${p.head}`);
    return;
  }
  if (!running && p.autoMergeOff && !p.manualMerge) {
    workNotice(ctx, "autoMergeOff", `${f.pr?.number}:${p.head}`);
    return;
  }
  if (p.checks === "couldntRun") {
    workNotice(ctx, "ciCouldntRun", `${f.pr?.number}:${p.head}`);
    return;
  }
  // Nothing wakes the worker for a review that only commented, and an approval the PR waits
  // for won't come: say so once the worker is done with the head.
  if (!running && p.copilotWontApprove && !p.manualMerge) workNotice(ctx, "copilotWontApprove", `${f.pr?.number}:${p.head}`);
  // A worker mid-turn may still push: the merge is read again once its turn ends, idle or exited.
  // A round typed into an idle worker and not yet begun is its turn too, read again as paced.
  if (mergeWanted(f, p)) {
    if (!mid && w.inRound !== p.head) command(ctx, WORK, "mergePr", { head: p.head });
    else w.prEvaluated = { ...w.prEvaluated, redWhileRunning: true };
    if (!mid && w.inRound === p.head) arm(ctx, WORK, "recheck", ROUND_RETRY_MS);
  }
  // GitHub computes mergeability lazily and tells nothing when it has, so it is read again,
  // each time twice as late, a bounded number of times.
  const tries = w.unknownRechecks;
  if (p.mergeState === "UNKNOWN" && tries < UNKNOWN_RECHECKS) {
    w.unknownRechecks = tries + 1;
    arm(ctx, WORK, "recheck", RECHECK_MS * 2 ** tries);
  }
}

// GitHub's auto-merge fires only as a requirement changes, so never on a PR whose requirements
// were met before it was asked: Tolkster merges one GitHub would merge now, with its checks
// passed, auto-merge on or refused as mergeable already, and nothing left to the owner (their
// auto-merge off, `manual-merge`, its rounds capped at this head).
function mergeWanted(f: IssueFacts, p: PrRead): boolean {
  const asked = p.autoMerge === true || f.mergeAsked === p.number;
  return asked && p.mergeable === true && p.checks === "passed" && !p.autoMergeOff && !p.manualMerge && !f.notices.includes(`roundsCap:${p.number}:${p.head}`);
}

// A rebase-only round reads no feedback, so it takes no cover of its own, and leaves a round
// still in flight on the head the cover it has.
function startRound(ctx: Ctx, running: boolean, proposal: Proposal): void {
  if (!rebaseOnly(proposal)) ctx.s.work!.inRound = proposal.head;
  if (!running) launch(ctx, "round");
  command(ctx, WORK, "startRound", { pr: ctx.s.facts.pr?.number ?? null, proposal });
}

export function rebaseOnly(p: Proposal): boolean {
  return p.lastBase !== undefined && p.lastAt === undefined && p.lastFailed === undefined;
}

export function rebaseOnlyRound(params: Record<string, unknown>): boolean {
  const p = proposalOf({ kind: "startRound", params });
  return p !== null && rebaseOnly(p);
}

// The base a branch update in the queue was sent for, which it covers as a round's proposal
// does, counting no round.
function updating(c: { kind: string; params: Record<string, unknown> }): string | null {
  return c.kind === "updateBranch" && typeof c.params.base === "string" ? c.params.base : null;
}

// A branch update done, or with nothing left to do, marks its base, and a head moved under it
// is read when its push lands. One GitHub refused at the head it was sent for, as for a
// conflict, starts the worker's round while the PR, its head and the work still hold; one
// that failed otherwise, or was lost, is read again in 10 minutes, or by the state's own
// deadline first, so no blip wakes a worker.
export function branchUpdated(ctx: Ctx, result: CommandResult, data: ResultData, params: Record<string, unknown>): void {
  const w = ctx.s.work!;
  const f = ctx.s.facts;
  const { head, base } = params;
  if (typeof head !== "string" || typeof base !== "string") return;
  if (result === "ok") {
    if (data.moved === true) return;
    write(ctx, WORK);
    f.rounds = { ...f.rounds, lastBase: base };
    return;
  }
  const atRest = w.state === "exited" || w.state === "rest";
  if (!atRest || !openPr(ctx) || f.pr!.head !== head || !delegated(f) || finished(f)) return;
  if (result === "failed" && data.refused === true) {
    write(ctx, WORK);
    startRound(ctx, false, { head, lastBase: base });
    return;
  }
  if (result === "failed" || result === "lost") arm(ctx, WORK, "recheck", ROUND_RETRY_MS);
}
