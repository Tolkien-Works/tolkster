import type { Routed } from "./intake.ts";
import type { Command as Wire, Report } from "./machine.ts";
import { WORK } from "./machine/ctx.ts";
import { unfinished } from "./machine/facts.ts";
import { FLEET } from "./machine/fleet.ts";
import { TRIAGE } from "./machine/triage.ts";
import { proposalOf, rebaseOnly, rebaseOnlyRound } from "./machine/rounds.ts";
import type { Command, IssueStateType, Store } from "./machine/types.ts";

// The joins between the intake, the machines and `listen` (docs/state-machine.md, Events
// and Commands): an intake's event as the machine's, a machine's command as the wire's, and
// a report from `listen` or a worker as the machine's event. Each carries only what the
// intake or the store already checked, IDs, times, SHAs and enums, and each wire command's
// args are read from its store as it is sent, so a command queued before its worktree or PR
// was recorded names them once they are.
//
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node.

// Linear's state types as the machine's: an issue in triage, the backlog, unstarted or
// started is open; a duplicate has ended, as a cancelled one has.
export function stateType(linear: string): IssueStateType {
  return linear === "completed" ? "completed" : linear === "canceled" || linear === "duplicate" ? "cancelled" : "open";
}

// The intake's event as the machine's. A review counts as a reviewer's here: which of them
// are the repo's `reviewers` is `evaluatePr`'s to read, so another's costs one read and
// moves no round.
export function toEvent(e: Routed): Record<string, unknown> {
  const base = { id: e.deliveryId, issue: e.issueId };
  switch (e.kind) {
    case "sessionStarted": {
      const sessionKind = e.trust === "owner" ? e.sessionKind : e.trust;
      return { ...base, kind: e.kind, session: e.sessionId, sessionKind, byOwner: e.trust !== "chain", ...(e.commentId === null ? {} : { comment: e.commentId }), at: e.at };
    }
    case "ownerPrompted":
    case "ownerStopped":
      // `decide` passes only the owner's prompts, whoever started the session.
      return { ...base, kind: e.kind, session: e.sessionId, byOwner: true, at: e.at };
    case "delegateChanged":
      return { ...base, kind: e.kind, by: e.delegated ? e.by : null, at: e.at };
    case "issueStateChanged":
      return { ...base, kind: e.kind, state: stateType(e.issue.state), triage: e.issue.state === "triage", parent: e.issue.parentId, ...(e.from === null ? {} : { from: e.from }), at: e.at };
    case "priorityChanged":
      return { ...base, kind: e.kind, priority: e.issue.priority, at: e.at };
    case "prOpened":
      return { ...base, kind: e.kind, pr: e.number, head: e.head, repo: e.repo, at: e.at };
    case "prPushed":
      return { ...base, kind: e.kind, pr: e.number, head: e.head, before: e.before, at: e.at };
    case "prMerged":
      return { ...base, kind: e.kind, pr: e.number, head: e.head, at: e.at };
    case "prClosed":
    case "autoMergeDisabled":
      return { ...base, kind: e.kind, pr: e.number, at: e.at };
    case "prReviewed":
      return { ...base, kind: e.kind, pr: e.number, sha: e.sha, reviewer: true, copilot: e.copilot, at: e.at };
    case "checksCompleted":
      return { ...base, kind: e.kind, sha: e.sha, result: e.result, at: e.at };
    case "ownerCommented":
      return { ...base, kind: e.kind, comment: e.commentId, at: e.at };
  }
}

// The machine's command on the wire, with its args read from its store as it is now. A
// command whose args can't be read (a PR's, with no PR held) goes with what there is, and
// `listen` ends it `failed`.
export function toWire(c: Command, s: Store, identifier: string): Wire {
  return { type: "command", id: c.id, kind: c.kind, issueId: c.issue, identifier, version: c.version, urgent: c.class1, args: { ...args(c, s), ...queued(c, s) } };
}

// What a queued request waits for, beside any command sent for its session, so `listen`'s log
// says why the issue waits (docs/realtime.md): the machine's own `waiting`, the unfinished
// blockers' IDs for a wait on them, and the request's time, which tells a new request's wait
// from the last one's.
function queued(c: Command, s: Store): Record<string, unknown> {
  const sess = s.sessions[c.machine];
  const waiting = sess?.facts.waiting;
  if (sess?.machine?.state !== "queued" || !waiting || sess.facts.request === null) return {};
  return { queued: { waiting, at: sess.facts.request.at, ...(waiting === "blockers" ? { blockers: unfinished(s.facts.blockers) } : {}) } };
}

// The triage machine's command (machine/triage.ts), whose args are its own: it has no store.
export function triageWire(c: Command): Wire {
  const args = c.kind === "startTriage" ? { issueIds: c.params.issueIds } : { sessionId: c.params.session, terminal: c.params.terminal };
  return { type: "command", id: c.id, kind: c.kind, issueId: TRIAGE, identifier: TRIAGE, version: c.version, urgent: false, args };
}

// The fleet's command (machine/fleet.ts): the repo to make a machine for, the machine to wake
// or destroy, by its name, or the host to put to sleep, by its ID.
export function fleetWire(c: Command): Wire {
  const args = c.kind === "makeHost" ? { repo: c.params.repo } : c.kind === "suspendHost" ? { host: c.params.host } : { name: c.params.name };
  return { type: "command", id: c.id, kind: c.kind, issueId: FLEET, identifier: FLEET, version: c.version, urgent: false, args };
}

// The rule a reap was decided on. A merged one names the PR held now, which must be the
// one merged, and a landed one a landing held: with none, `rule` is absent and `listen` ends
// the command `failed`. A landing names no commit: listen's guards read git (cli/src/reap.rs).
function reapRule(rule: unknown, f: Store["facts"]): Record<string, unknown> | undefined {
  const pr = f.pr;
  if (rule === "merged") return pr?.status === "merged" ? { kind: "merged", number: pr.number, head: pr.head } : undefined;
  if (rule === "landed") return f.landed === undefined ? undefined : { kind: rule };
  // `noPr`: the store holds none, and may have missed one, so listen guards what it would lose.
  if (rule === "cancelled") return pr === null ? { kind: rule, noPr: true } : { kind: rule };
  return rule === "completed" ? { kind: rule } : undefined;
}

function args(c: Command, s: Store): Record<string, unknown> {
  const p = c.params;
  const f = s.facts;
  const thread = p.thread === true;
  const worktreeId = f.worktree?.id ?? null;
  // Where the worker's own notices and its stop are said: the session that last gave it a request.
  const requestedBy = s.work?.requestedBy?.session ?? null;
  const pr = f.pr === null ? {} : { repo: f.pr.repo, number: f.pr.number };
  switch (c.kind) {
    case "readIssue":
    case "startContext":
      return {};
    case "startWorker":
      return { sessionId: p.session, orchestrate: p.orchestrate === true, ...(typeof p.worktree === "string" ? { worktreeId: p.worktree } : {}), ...(typeof p.host === "string" ? { host: p.host } : {}) };
    case "resumeWorker":
      return { sessionId: p.session, childrenEnd: p.childrenEnd === true, childrenAdded: p.childrenAdded === true };
    case "deliverMessage":
      return p.comment === true ? { worktreeId, sessionId: requestedBy, comment: true } : { worktreeId, sessionId: p.session };
    case "readComment":
      return { commentId: p.comment };
    case "startRound": {
      // The round's number: those counted, then each round queued up to this one, so one
      // dropped before it isn't counted (a command queued before proposals was counted when
      // queued). The head it is for, the session it reports in, and whether a rebase is all
      // it is for, which has a brief of its own.
      const at = s.queue.findIndex((q) => q.id === c.id);
      const ahead = (at === -1 ? s.queue : s.queue.slice(0, at)).filter((q) => q.machine === WORK && proposalOf(q) !== null && !rebaseOnlyRound(q.params)).length;
      const proposal = proposalOf(c);
      return f.pr === null ? {} : { ...pr, head: f.pr.head, round: f.rounds.total + ahead + (proposal !== null ? 1 : 0), sessionId: requestedBy, rebaseOnly: proposal !== null && rebaseOnly(proposal) };
    }
    case "evaluatePr":
      return pr;
    case "updateBranch":
    case "mergePr":
      // The head it was decided on, never the one held now: GitHub's lease on it refuses an
      // update or merge over a push the machine hasn't seen.
      return f.pr === null ? {} : { ...pr, head: p.head };
    case "stopWorker":
      return { worktreeId, sessionId: requestedBy };
    case "startTurn": {
      // A turn reads what the owner wrote after the newest request a reader was shown, never
      // after an answer: one written while a turn ran, before it answered, is the next turn's.
      // A session's worker may have answered some (rule 11), so its reads count too; a note's
      // thread has none, and its batch stops at `request`.
      const facts = s.sessions[c.machine]?.facts;
      const shown = [facts?.turnReadThrough, thread ? null : facts?.readThrough].filter((t): t is string => t !== null && t !== undefined).sort().at(-1);
      return { sessionId: c.machine, thread, request: p.request ?? null, after: shown ?? null };
    }
    case "stopTurn":
      return { sessionId: c.machine, thread, ...(p.idle === true ? { idle: true } : {}) };
    case "observe":
      return { worktreeIds: worktreeId === null ? [] : [worktreeId], sessionId: p.session ?? null, thread };
    case "closeAgent":
      return { worktreeId };
    case "reapWorktree":
      return { worktreeId, rule: reapRule(p.rule, f) };
    case "postNotice":
      return {
        sessionId: p.session ?? null,
        thread,
        notice: p.notice,
        ...(p.notice === "blocked" ? { blockers: unfinished(f.blockers) } : {}),
        ...(f.pr === null ? {} : { pr: f.pr.number }),
        ...(p.notice === "roundsCap" ? { rounds: f.rounds.total } : {}),
        ...(typeof p.through === "string" ? { through: p.through } : {}),
      };
    case "takeBackChain":
      return { children: f.chained?.issues ?? [] };
    default:
      return {};
  }
}

// What a report is delivered as needs, beyond the report: the issue a session or a note's
// thread is on, the issue a worktree was started for, and the connection it came on.
export type Lookups = {
  sessionIssue: (sessionId: string) => Promise<string | null>;
  worktreeIssue: (worktreeId: string) => Promise<string | null>;
  // When the connection a socket report came on was accepted: with its sequence, the
  // observation's delivery ID.
  connection: number;
};

// A report as the machine's event, or null when it names no issue the Durable Object
// knows, which is logged and dropped. `chainRecorded` comes in as the parent's ask.
export async function fromReport(report: Report, at: Lookups): Promise<Record<string, unknown> | null> {
  switch (report.report) {
    case "commandResult":
      return { id: `result:${report.id}`, issue: report.issueId, kind: "commandResult", command: report.id, result: report.result, data: report.detail };
    case "turnEnded": {
      const issue = await at.sessionIssue(report.sessionId);
      return issue === null ? null : { id: `${at.connection}:${report.sequence}`, issue, kind: "turnEnded", session: report.sessionId };
    }
    case "turnIdle": {
      const issue = await at.sessionIssue(report.sessionId);
      return issue === null ? null : { id: `idle:${report.sessionId}:${report.at}`, issue, kind: "turnIdle", session: report.sessionId, terminal: report.terminal, at: report.at };
    }
    case "workerRead":
    case "workerAnswered": {
      const issue = await at.sessionIssue(report.sessionId);
      if (issue === null) return null;
      const base = { issue, kind: report.report, session: report.sessionId, reader: report.reader };
      return report.report === "workerRead" ? { ...base, id: `${report.sessionId}:${report.activityId}`, through: report.at } : { ...base, id: report.activityId, at: report.at };
    }
    case "chainRecorded":
      return { id: `${report.sessionId}:${report.run}`, issue: report.issueId, kind: "chainRecorded", role: "ask", session: report.sessionId, run: report.run, children: report.children };
    // Of the issue it names, only from a session on it, as a merge asked is.
    case "landed": {
      const issue = await at.sessionIssue(report.sessionId);
      return issue === report.issueId ? { id: `${report.sessionId}:${report.head}`, issue, kind: "landed", session: report.sessionId, head: report.head } : null;
    }
    case "mergeAsked": {
      const issue = await at.sessionIssue(report.sessionId);
      return issue === report.issueId ? { id: `${report.sessionId}:merge:${report.pr}`, issue, kind: "mergeAsked", session: report.sessionId, pr: report.pr } : null;
    }
    case "config":
      // Settings, not an issue's event: the fleet takes them (machine/store.ts, `fleetReported`).
      return null;
    case "terminalClosed":
      // No issue's by itself: the relay finds what ran in it (relay.ts).
      return null;
    case "listing":
      // Of every worktree the Durable Object holds (listing.ts).
      return null;
    default: {
      const issue = await at.worktreeIssue(report.worktreeId);
      if (issue === null) return null;
      return { id: "sequence" in report ? `${at.connection}:${report.sequence}` : `hook:${report.report}:${report.at}`, issue, kind: report.report };
    }
  }
}
