// What an event from outside the Durable Object may carry into the machines: the Worker's
// Linear and GitHub events, and listen's results, observations and workers' reports. Each
// is rebuilt from its checked fields alone (Linear's IDs and times, GitHub's SHAs and PR
// numbers, and every enum), so nothing else reaches a stored fact, a command's parameters
// or a store's key, and an event that fails is refused whole, with Invalid. The events a
// store, the coordinator or the alarm make (deadlines, the coordinator's fan-outs and
// answers, the parent's and sub-issues' events, a chain's forward and answer) are never
// taken from outside.

import { IDENTIFIER, TIME } from "../../shared/linear.ts";
import { bool, check, Invalid, list, object, oneOf, str, whole } from "../check.ts";
import { BRANCH, REPO, SHA } from "../github.ts";
import { HEAD, HOST, ID, UUID, WORKTREE } from "../state.ts";
import { MAX_ASKED, type TriageData, type TriageEvent } from "./triage.ts";
import { CHECKS, COMMENT_KINDS, ISSUE_STATES, MERGE_STATES, PR_STATUSES, READERS, RESULTS, SESSION_KINDS, type DelegatedBy, type Event, type IssueRead, type PrRead, type ResultData, type WaitsOn } from "./types.ts";

// A repo as a read gives it, by the name listen's settings have for it (an Orca repo's
// display name): no control characters, and short. A PR's repo is GitHub's `owner/name`
// instead (REPO).
const REPO_NAME = /^[^\u0000-\u001f\u007f-\u009f]{1,200}$/;
// A command's ID: its issue, then its store's command counter.
const COMMAND = /^[0-9a-f-]{36}\.\d{1,12}$/;
// The triage machine's command's ID (triage.ts).
const TRIAGE_COMMAND = /^triage\.\d{1,12}$/;
// Orca's handle, behind `<host>@` on a remote host (cli/src/plain.rs, split_terminal).
export const TERMINAL = /^(?:(?!-)[A-Za-z0-9_.-]{1,64}@)?(?!-)[A-Za-z0-9_:-]{1,128}$/;
// A delivery ID: Linear's, GitHub's, or listen's for an observation or a report.
const EVENT_ID = /^[A-Za-z0-9_.:@-]{1,128}$/;

const BY: readonly DelegatedBy[] = ["owner", "chain", "other", null];
// Only a read says `unknown`, `aged` or `created`.
const READ_BY: readonly DelegatedBy[] = [...BY, "unknown", "aged", "created"];
// A parent is never kept by an `aged` read: only its issue's is.
const PARENT_BY: readonly DelegatedBy[] = [...BY, "unknown"];

export function parseEvent(value: unknown): Event {
  const e = object(value, "event");
  const id = str(e.id, EVENT_ID, "id");
  const issue = str(e.issue, UUID, "issue");
  const base = { id, issue };
  const at = () => str(e.at, TIME, "at");
  const session = () => str(e.session, UUID, "session");
  const pr = () => whole(e.pr, "pr");
  switch (e.kind) {
    case "sessionStarted":
      return { ...base, kind: e.kind, session: session(), sessionKind: oneOf(e.sessionKind, SESSION_KINDS, "sessionKind"), byOwner: bool(e.byOwner, "byOwner"), ...(e.comment === undefined ? {} : { comment: str(e.comment, UUID, "comment") }), at: at() };
    case "ownerPrompted":
    case "ownerStopped":
      return { ...base, kind: e.kind, session: session(), byOwner: bool(e.byOwner, "byOwner"), at: at() };
    case "delegateChanged":
      return { ...base, kind: e.kind, by: oneOf(e.by, BY, "by"), at: at() };
    case "issueStateChanged":
      return { ...base, kind: e.kind, state: oneOf(e.state, ISSUE_STATES, "state"), triage: flag(e.triage, "triage"), parent: e.parent === null ? null : str(e.parent, UUID, "parent"), ...(e.from === undefined ? {} : { from: str(e.from, UUID, "from") }), at: at() };
    case "priorityChanged":
      return { ...base, kind: e.kind, priority: whole(e.priority, "priority", 4), at: at() };
    case "prOpened":
      return { ...base, kind: e.kind, pr: pr(), head: str(e.head, HEAD, "head"), repo: str(e.repo, REPO, "repo"), at: at() };
    case "prPushed":
      return { ...base, kind: e.kind, pr: pr(), head: str(e.head, HEAD, "head"), before: str(e.before, HEAD, "before"), at: at() };
    case "prReviewed":
      return { ...base, kind: e.kind, pr: pr(), sha: e.sha === null ? null : str(e.sha, HEAD, "sha"), reviewer: bool(e.reviewer, "reviewer"), copilot: bool(e.copilot, "copilot"), at: at() };
    case "ownerCommented":
      return { ...base, kind: e.kind, comment: str(e.comment, UUID, "comment"), at: at() };
    case "checksCompleted":
      return { ...base, kind: e.kind, sha: str(e.sha, HEAD, "sha"), result: oneOf(e.result, CHECKS, "result"), at: at() };
    case "prMerged":
      return { ...base, kind: e.kind, pr: pr(), ...(e.head === undefined ? {} : { head: str(e.head, HEAD, "head") }), at: at() };
    case "prClosed":
    case "autoMergeDisabled":
      return { ...base, kind: e.kind, pr: pr(), at: at() };
    case "landed":
      return { ...base, kind: e.kind, session: session(), head: str(e.head, SHA, "head") };
    case "mergeAsked":
      return { ...base, kind: e.kind, session: session(), pr: pr() };
    case "agentWorking":
    case "agentBackground":
    case "agentIdle":
    case "agentWaiting":
    case "agentExited":
    case "worktreeGone":
      return { ...base, kind: e.kind };
    case "turnEnded":
      return { ...base, kind: e.kind, session: session(), ...(e.terminal === undefined ? {} : { terminal: str(e.terminal, TERMINAL, "terminal") }) };
    case "turnIdle":
      return { ...base, kind: e.kind, session: session(), terminal: str(e.terminal, TERMINAL, "terminal"), at: at() };
    case "workerRead":
      return { ...base, kind: e.kind, session: session(), reader: oneOf(e.reader, READERS, "reader"), through: str(e.through, TIME, "through") };
    case "workerAnswered":
      return { ...base, kind: e.kind, session: session(), reader: oneOf(e.reader, READERS, "reader"), at: at() };
    case "commandResult":
      return { ...base, kind: e.kind, command: str(e.command, COMMAND, "command"), result: oneOf(e.result, [...RESULTS, "lost"] as const, "result"), data: e.data === undefined ? undefined : parseData(e.data) };
    case "chainRecorded":
      // Only `tolkster issue chain`'s ask comes from outside; its forward and answer pass
      // between stores.
      check(e.role === "ask", "role");
      return { ...base, kind: e.kind, role: "ask", session: session(), run: str(e.run, ID, "run"), children: list(e.children, "children", 50).map((c) => str(c, UUID, "child")) };
    default:
      throw new Invalid("invalid kind");
  }
}

// An event for the triage machine from outside: the Worker's create in Triage, listen's
// session's end and its commands' results.
export function parseTriageEvent(value: unknown): TriageEvent {
  const e = object(value, "event");
  const id = str(e.id, EVENT_ID, "id");
  switch (e.kind) {
    case "triageAsked":
      return { id, kind: e.kind, issue: str(e.issue, UUID, "issue") };
    case "triageEnded":
      return { id, kind: e.kind, terminal: str(e.terminal, TERMINAL, "terminal") };
    case "commandResult":
      return { id, kind: e.kind, command: str(e.command, TRIAGE_COMMAND, "command"), result: oneOf(e.result, RESULTS, "result"), ...(e.data === undefined ? {} : { data: parseTriageData(e.data) }) };
    default:
      throw new Invalid("invalid kind");
  }
}

function parseTriageData(value: unknown): TriageData {
  const d = object(value, "data");
  const out: TriageData = {};
  if (d.session !== undefined) {
    const s = object(d.session, "session");
    out.session = { id: str(s.id, ID, "session.id"), terminal: str(s.terminal, TERMINAL, "session.terminal"), issues: list(s.issues, "session.issues", 50).map((i) => str(i, IDENTIFIER, "session.issues")) };
  }
  if (d.waiting !== undefined) out.waiting = list(d.waiting, "waiting", MAX_ASKED).map((i) => str(i, UUID, "waiting"));
  if (d.open !== undefined) out.open = bool(d.open, "open");
  return out;
}

function parseData(value: unknown): ResultData {
  const d = object(value, "data");
  const out: ResultData = {};
  if (d.read !== undefined) out.read = parseRead(d.read);
  if (d.worktree !== undefined) {
    if (d.worktree === null) out.worktree = null;
    else {
      const w = object(d.worktree, "worktree");
      out.worktree = { id: str(w.id, WORKTREE, "worktree.id"), branch: str(w.branch, BRANCH, "worktree.branch") };
    }
  }
  if (d.madeWorktree !== undefined) out.madeWorktree = bool(d.madeWorktree, "madeWorktree");
  if (d.launched !== undefined) out.launched = bool(d.launched, "launched");
  if (d.typed !== undefined) out.typed = bool(d.typed, "typed");
  if (d.unnamed !== undefined) out.unnamed = bool(d.unnamed, "unnamed");
  if (d.waking !== undefined) out.waking = bool(d.waking, "waking");
  if (d.waitsOn !== undefined) out.waitsOn = parseWaitsOn(d.waitsOn);
  if (d.said !== undefined) out.said = bool(d.said, "said");
  if (d.terminal !== undefined) out.terminal = str(d.terminal, TERMINAL, "terminal");
  if (d.reaped !== undefined) out.reaped = bool(d.reaped, "reaped");
  if (d.moved !== undefined) out.moved = bool(d.moved, "moved");
  if (d.refused !== undefined) out.refused = bool(d.refused, "refused");
  if (d.pr !== undefined) out.pr = parsePr(d.pr);
  if (d.comment !== undefined) {
    const c = object(d.comment, "comment");
    out.comment = {
      kind: oneOf(c.kind, COMMENT_KINDS, "comment.kind"),
      newestOwnerAt: c.newestOwnerAt === null ? null : str(c.newestOwnerAt, TIME, "comment.newestOwnerAt"),
      state: oneOf(c.state, ISSUE_STATES, "comment.state"),
      ...(c.root === undefined ? {} : { root: str(c.root, UUID, "comment.root") }),
    };
  }
  return out;
}

export function parseWaitsOn(value: unknown): WaitsOn {
  const w = object(value, "waitsOn");
  return w.lock === true ? { lock: true } : { host: str(w.host, HOST, "waitsOn.host") };
}

function members(value: unknown, what: string): { id: string; done: boolean; updatedAt: string }[] {
  return list(value, what, 50).map((v) => {
    const m = object(v, what);
    return { id: str(m.id, UUID, `${what}.id`), done: bool(m.done, `${what}.done`), updatedAt: str(m.updatedAt, TIME, `${what}.updatedAt`) };
  });
}

// A sub-issue says whether it is in Triage too.
function children(value: unknown): { id: string; done: boolean; triage: boolean; updatedAt: string }[] {
  const raw = list(value, "read.children", 50);
  return members(raw, "read.children").map((m, i) => ({ ...m, triage: flag(object(raw[i], "read.children").triage, "read.children.triage") }));
}

// Absent, as from a `listen` or an event queued before it was carried, is false.
function flag(value: unknown, what: string): boolean {
  return value === undefined ? false : bool(value, what);
}

function parseRead(value: unknown): IssueRead {
  const r = object(value, "read");
  let parent: IssueRead["parent"] = null;
  if (r.parent !== null) {
    const p = object(r.parent, "read.parent");
    parent = { id: str(p.id, UUID, "read.parent.id"), delegated: bool(p.delegated, "read.parent.delegated"), by: oneOf(p.by, PARENT_BY, "read.parent.by"), state: oneOf(p.state, ISSUE_STATES, "read.parent.state"), priority: whole(p.priority, "read.parent.priority", 4), updatedAt: str(p.updatedAt, TIME, "read.parent.updatedAt") };
  }
  return {
    updatedAt: str(r.updatedAt, TIME, "read.updatedAt"),
    delegate: oneOf(r.delegate, READ_BY, "read.delegate"),
    state: oneOf(r.state, ISSUE_STATES, "read.state"),
    triage: flag(r.triage, "read.triage"),
    stateChangedAt: str(r.stateChangedAt, TIME, "read.stateChangedAt"),
    priority: whole(r.priority, "read.priority", 4),
    blockers: members(r.blockers, "read.blockers"),
    moreBlockers: bool(r.moreBlockers, "read.moreBlockers"),
    parent,
    children: children(r.children),
    moreChildren: bool(r.moreChildren, "read.moreChildren"),
    repo: r.repo === null ? null : str(r.repo, REPO_NAME, "read.repo"),
    sessions: list(r.sessions, "read.sessions", 250).map((v) => {
      const s = object(v, "read.sessions");
      return {
        id: str(s.id, UUID, "read.sessions.id"),
        kind: oneOf(s.kind, SESSION_KINDS, "read.sessions.kind"),
        createdAt: str(s.createdAt, TIME, "read.sessions.createdAt"),
        ...(s.inDescription === undefined ? {} : { inDescription: bool(s.inDescription, "read.sessions.inDescription") }),
      };
    }),
  };
}

function parsePr(value: unknown): PrRead {
  const p = object(value, "pr");
  let feedback: PrRead["feedback"] = null;
  if (p.feedback !== null) {
    const f = object(p.feedback, "pr.feedback");
    feedback = { at: str(f.at, TIME, "pr.feedback.at"), reviewerAt: f.reviewerAt === null ? null : str(f.reviewerAt, TIME, "pr.feedback.reviewerAt") };
  }
  return {
    number: whole(p.number, "pr.number"),
    status: oneOf(p.status, PR_STATUSES, "pr.status"),
    at: str(p.at, TIME, "pr.at"),
    head: str(p.head, HEAD, "pr.head"),
    base: str(p.base, HEAD, "pr.base"),
    mergeState: oneOf(p.mergeState, MERGE_STATES, "pr.mergeState"),
    autoMergeOff: bool(p.autoMergeOff, "pr.autoMergeOff"),
    ...(p.autoMerge === undefined ? {} : { autoMerge: bool(p.autoMerge, "pr.autoMerge") }),
    ...(p.mergeable === undefined ? {} : { mergeable: bool(p.mergeable, "pr.mergeable") }),
    manualMerge: bool(p.manualMerge, "pr.manualMerge"),
    checks: oneOf(p.checks, [...CHECKS, "pending"] as const, "pr.checks"),
    feedback,
    copilotWontApprove: bool(p.copilotWontApprove, "pr.copilotWontApprove"),
    maxRounds: whole(p.maxRounds, "pr.maxRounds", 100),
  };
}
