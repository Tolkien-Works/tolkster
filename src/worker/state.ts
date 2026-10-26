// Tolkster's own records, kept in the owner's Durable Object (relay.ts) beside its state
// machines (machine/), which hold what each request and worker is doing. These are what
// `tolkster` keeps for itself: per session, the PR feedback a round has read and its
// conversation's turn (SessionRecord); why each worker it started exists (WorkerRecord);
// which sub-issues Tolkster delegated to itself for a parent it works through
// (Delegation); each repo's shared conversations checkout (CheckoutRecord); per issue,
// Tolkster's one context comment on it (ContextComment); and the claim lock, which stops two runs taking the same verdict or context session. Which triage
// session runs is the triage machine's (machine/triage.ts), which
// `tolkster triage` asks here (`triaging`).
//
// Only what Tolkster did is stored: IDs, timestamps, enums and issue identifiers. What
// people do (delegation, blockers, archiving, prompts) is always read from Linear, and
// no issue or comment text, and nothing about people, is ever stored. Every value is
// rebuilt from checked fields before it is stored, so nothing else can slip in.
//
// `tolkster` records them through POST /state (index.ts), which carries the realtime
// secret (cli/src/state.rs); this only keeps the records and the lock.
//
// Times are Linear's ISO format, e.g. 2026-09-26T01:02:03.456Z, and nothing else is
// accepted, so they compare as text.
//
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node. The
// runtime holds other events back while a handler awaits storage (input gates), so each
// operation below is atomic.

import { IDENTIFIER, TIME } from "../shared/linear.ts";
import { check, Invalid, list, object, str, whole } from "./check.ts";
import { type Triage, TRIAGE_KEY, triaging } from "./machine/triage.ts";
import { pruneDue, type Storage } from "./storage.ts";

// A round's read of a PR's review feedback (`tolkster pr feedback --session`): GitHub's
// time of the newest feedback it read, or of the PR's opening when it read none. Feedback
// no newer than it was read (cli/src/review.rs).
export type Read = { pr: number; at: string };

// A conversation's agent session, which each of its turns resumes, and the terminal its
// latest turn runs in (null until that terminal exists), as of `at`: see cli/src/converse.rs.
export type Conversation = { agentSession: string; terminal: string | null; at: string };

export type SessionRecord = {
  // The newest read of each PR's review feedback from this session, newest first.
  reads?: Read[];
  // The session is a conversation, run as turns of this agent session.
  conversation?: Conversation;
};

export type Claim = { name: string; ttlMs: number | null };

// Why a worker exists, recorded by `tolkster listen` as it starts one: the issue it was
// started for, its Orca worktree, the session that started it, and when. Orca's own link
// from worktree to issue stays the one that finds a worker; this adds that Tolkster
// started it. Kept per worktree, until `reap` removes the worktree.
//
// A start is recorded once, after Orca has named the worktree.
//
// `launch` is the harness, model and effort the worker started with (cli/src/config/ladder.rs),
// which its resumes launch it with again: a harness's name, a model's plain name and one of
// the harness's efforts, never text. A launch sent without a harness, by a CLI older than
// this Worker, is Claude's. `rungFrom`
// is where the size that picked it came from: the claim, no estimate at the claim (picked
// again at a later launch), or the worker's own estimate. An enum alone, never the size or
// the issue's text.
// Each harness's effort levels, weakest first, as cli/src/harnesses.rs has them.
export const HARNESSES = { claude: ["low", "medium", "high", "xhigh", "max"] } as const;
export type Harness = keyof typeof HARNESSES;
export type Effort = (typeof HARNESSES)[Harness][number];
export type Launch = { harness: Harness; model: string; effort: Effort };
export type RungFrom = "claim" | "noEstimate" | "worker";
export type WorkerRecord = { issue: string; worktreeId: string; sessionId: string; at: string; launch: Launch; rungFrom: RungFrom };

// A repo's shared conversations checkout: the Orca worktree every conversation turn in the
// repo runs in, by the repo's Orca ID. Written when `tolkster listen` creates it, and again
// should it have to create it anew.
export type CheckoutRecord = { repoId: string; worktreeId: string; at: string };

// Tolkster's one context comment on an issue (docs/context.md), by the issue's and the
// comment's Linear IDs, and when it was last written: written by `tolkster comment
// --closed` just before each create or update, a create with that ID, so a later
// completion updates it in place. A comment deleted since is replaced, and its record
// with it.
export type ContextComment = { issueId: string; commentId: string; at: string };

// A worktree's record as stored: with when it first went missing from a complete listing
// of Orca's, by this clock, while it is.
type StoredWorker = WorkerRecord & { missingMs?: number };

// How long a worktree must be missing from every complete listing of Orca's, by this
// clock, before it is gone and its record forgotten (`markMissing`, `goneWorkers`).
export const MISSING_KEEP_MS = 60 * 60 * 1000;

// A sub-issue Tolkster delegated to itself, recorded by `tolkster issue update --delegate`
// just before it delegates: the sub-issue and its parent, by Linear id, the owner's (or an
// earlier delegation's) session on the parent that asked, and when. The session Linear then
// starts on the sub-issue is Tolkster's own, not the owner's, and this is what lets it be
// trusted: the Worker acknowledges it (index.ts), and `tolkster` still checks the parent,
// fresh from Linear, before acting. Kept per sub-issue for DELEGATION_KEEP_MS.
export type Delegation = { issueId: string; parentIssueId: string; sessionId: string; at: string };

// How long a delegation counts: far longer than a chain of sub-issues takes.
export const DELEGATION_KEEP_MS = 90 * 24 * 60 * 60 * 1000;

// How long an issue counts as one with a Tolkster session after the last session event on
// it, and how often a later event refreshes that, so each event costs a read, not a write.
export const SESSION_ISSUE_KEEP_MS = 90 * 24 * 60 * 60 * 1000;
export const SESSION_ISSUE_REFRESH_MS = 24 * 60 * 60 * 1000;

type StoredDelegation = Delegation & { storedMs: number };

// A change to one session's record. Times only ever move forward.
export type Change = {
  read?: Read;
  conversation?: Conversation;
};

// POST /state's body.
export type Operation =
  | { op: "read"; sessions: string[] }
  | { op: "record"; sessionId: string; change: Change }
  | { op: "take"; claims: Claim[]; token: string }
  | { op: "release"; claims: string[]; token: string }
  | { op: "startWorker"; worker: WorkerRecord }
  | { op: "workers" }
  | { op: "forgetWorker"; worktreeId: string }
  | { op: "delegate"; delegation: Delegation }
  | { op: "delegations"; issues: string[] }
  | { op: "checkouts" }
  | { op: "setCheckout"; checkout: CheckoutRecord }
  | { op: "contextComment"; issueId: string }
  | { op: "setContextComment"; comment: ContextComment }
  // Whether `session` is the running triage session and was given `identifier`.
  | { op: "triaging"; session: string; identifier: string };

// The response, its body as JSON text, which passes through the Durable Object's RPC as is,
// and what the Worker logs of it: the op, and IDs, enums and counts (see summary).
export type Outcome = { status: number; body: string; log: LogFields };
export type LogFields = Record<string, string | number | boolean | null>;

// Claims older than this are removed: what they guarded is long done.
export const CLAIM_KEEP_MS = 30 * 24 * 60 * 60 * 1000;

// Session records read per storage call: Durable Object storage's limit on one get.
const READ_BATCH = 128;

// Reads kept per session, one per PR, newest first.
const MAX_READS = 20;

const SESSION = "session:";
const CLAIM = "claim:";
const WORKER = "worker:";
const DELEGATED = "delegated:";
const CHECKOUT = "checkout:";
const CONTEXT_COMMENT = "context-comment:";
const APP_USER = "app-user";
const SESSION_ISSUE = "session-issue:";
// When claims were last pruned, by this clock.
const PRUNED_AT = "claims-pruned-at";

// `token` names the take, chosen by the caller: taking again with the same token succeeds,
// so a take whose response was lost can be retried, and a release must carry it, so a run
// whose claim went stale and was taken over can't release the newer holder's. `ttlMs` is the
// holder's: null holds for good.
type StoredClaim = { takenAt: number; token: string; ttlMs: number | null };

const TOKEN = /^[A-Za-z0-9_-]{16,64}$/;


export const ID = /^[A-Za-z0-9_-]{1,64}$/;
export const HEAD = /^[0-9a-f]{7,40}$/;
export const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
// A model as a harness's model flag takes it, as cli/src/config/ladder.rs's plain_model has it.
const MODEL = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;
const RUNG_FROM: readonly string[] = ["claim", "noEstimate", "worker"] satisfies RungFrom[];
// The CLI's claim names, e.g. `triage-TW-1`. Their kind is the CLI's own:
// nothing here reads it, so only the characters and the length are checked.
const CLAIM_NAME = /^[A-Za-z0-9_.:@-]{1,208}$/;
// A worktree's ID, as cli/src/plain.rs's split_worktree has it: on a remote host its Orca
// environment's name and `@`, then Orca's own ID, the repo's ID, `::` and the worktree's
// path, with no control characters (C0, DEL or C1).
export const WORKTREE = /^(?:[A-Za-z0-9_.][A-Za-z0-9_.-]{0,63}@)?[A-Za-z0-9_-]{1,64}::[^\u0000-\u001f\u007f-\u009f]{1,400}$/;
// A paired host's ID, as cli/src/plain.rs's plain_host has it.
export const HOST = /^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,63}$/;

export function hostOfWorktree(worktreeId: string): string | undefined {
  return /^([A-Za-z0-9_.][A-Za-z0-9_.-]{0,63})@[A-Za-z0-9_-]{1,64}::/.exec(worktreeId)?.[1];
}

// Whether a listing that `hosts` answered covers a worktree: the workstation's always.
export function heard(worktreeId: string, hosts: string[]): boolean {
  const host = hostOfWorktree(worktreeId);
  return host === undefined || hosts.includes(host);
}

function change(value: unknown): Change {
  const c = object(value, "change");
  let read: Change["read"];
  if (c.read !== undefined && c.read !== null) {
    const r = object(c.read, "read");
    read = { pr: whole(r.pr, "read.pr", 1e9), at: str(r.at, TIME, "read.at") };
  }
  return clean({ read, conversation: conversation(c.conversation) });
}

function conversation(value: unknown): Conversation | undefined {
  if (value === undefined || value === null) return undefined;
  const c = object(value, "conversation");
  const terminal = c.terminal === null || c.terminal === undefined ? null : str(c.terminal, ID, "conversation.terminal");
  return { agentSession: str(c.agentSession, UUID, "conversation.agentSession"), terminal, at: str(c.at, TIME, "conversation.at") };
}

function claimName(value: unknown): string {
  str(value, CLAIM_NAME, "claim name");
  return value as string;
}

function worktree(value: unknown): string {
  str(value, WORKTREE, "worktreeId");
  return value as string;
}

function worker(value: unknown): WorkerRecord {
  const w = object(value, "worker");
  str(w.issue, IDENTIFIER, "worker.issue");
  check(typeof w.rungFrom === "string" && RUNG_FROM.includes(w.rungFrom), "worker.rungFrom");
  return { issue: w.issue as string, worktreeId: worktree(w.worktreeId), sessionId: str(w.sessionId, ID, "worker.sessionId"), at: str(w.at, TIME, "worker.at"), launch: launch(w.launch), rungFrom: w.rungFrom as RungFrom };
}

function launch(value: unknown): Launch {
  const l = object(value, "worker.launch");
  const harness = l.harness ?? "claude";
  check(typeof harness === "string" && Object.hasOwn(HARNESSES, harness), "worker.launch.harness");
  str(l.model, MODEL, "worker.launch.model");
  const efforts: readonly string[] = HARNESSES[harness as Harness];
  check(typeof l.effort === "string" && efforts.includes(l.effort), "worker.launch.effort");
  return { harness: harness as Harness, model: l.model as string, effort: l.effort as Effort };
}

function checkout(value: unknown): CheckoutRecord {
  const c = object(value, "checkout");
  const repoId = str(c.repoId, ID, "checkout.repoId");
  const worktreeId = worktree(c.worktreeId);
  // One of the repo's own worktrees.
  check(worktreeId.startsWith(`${repoId}::`), "checkout.worktreeId");
  return { repoId, worktreeId, at: str(c.at, TIME, "checkout.at") };
}

function contextComment(value: unknown): ContextComment {
  const c = object(value, "comment");
  return { issueId: str(c.issueId, ID, "comment.issueId"), commentId: str(c.commentId, UUID, "comment.commentId"), at: str(c.at, TIME, "comment.at") };
}

function delegation(value: unknown): Delegation {
  const d = object(value, "delegation");
  return { issueId: str(d.issueId, ID, "delegation.issueId"), parentIssueId: str(d.parentIssueId, ID, "delegation.parentIssueId"), sessionId: str(d.sessionId, ID, "delegation.sessionId"), at: str(d.at, TIME, "delegation.at") };
}

// What a reset (storage.ts) keeps of these records, each rebuilt in its current shape: each
// issue's context comment, so a later completion updates it rather than posting another, and
// each delegation Tolkster made, so its sub-issue's session is still trusted and the rebuild
// (rebuild.ts) knows the chain. Undefined for anything else, and for one that shape can't hold.
export function keptAtReset(key: string, value: unknown): unknown {
  if (!key.startsWith(CONTEXT_COMMENT) && !key.startsWith(DELEGATED)) return undefined;
  try {
    if (key.startsWith(DELEGATED)) return { ...delegation(value), storedMs: whole((value as { storedMs?: unknown }).storedMs, "delegation.storedMs", Number.MAX_SAFE_INTEGER) };
    return contextComment(value);
  } catch (error) {
    if (error instanceof Invalid) return undefined;
    throw error;
  }
}

// Drops unset fields and empty lists, so a record stores only what happened.
function clean<T extends Record<string, unknown>>(value: T): T {
  const kept: Record<string, unknown> = {};
  for (const [key, inner] of Object.entries(value)) {
    if (inner === undefined || (Array.isArray(inner) && inner.length === 0)) continue;
    kept[key] = inner;
  }
  return kept as T;
}

// Reads POST /state's body into an operation, or throws Invalid.
export function operation(body: unknown): Operation {
  const o = object(body, "body");
  switch (o.op) {
    case "read":
      return { op: "read", sessions: list(o.sessions, "sessions", 1000).map((s) => str(s, ID, "session id")) };
    case "record":
      return { op: "record", sessionId: str(o.sessionId, ID, "sessionId"), change: change(o.change) };
    case "take": {
      check(Array.isArray(o.claims) && o.claims.length >= 1 && o.claims.length <= 5, "claims");
      const claims = (o.claims as unknown[]).map((value) => {
        const c = object(value, "claim");
        return { name: claimName(c.name), ttlMs: c.ttlMs === null || c.ttlMs === undefined ? null : whole(c.ttlMs, "claim ttlMs", CLAIM_KEEP_MS) };
      });
      str(o.token, TOKEN, "token");
      return { op: "take", claims, token: o.token as string };
    }
    case "release": {
      check(Array.isArray(o.claims) && o.claims.length >= 1 && o.claims.length <= 5, "claims");
      str(o.token, TOKEN, "token");
      return {
        op: "release",
        claims: (o.claims as unknown[]).map(claimName),
        token: o.token as string,
      };
    }
    case "startWorker":
      return { op: "startWorker", worker: worker(o.worker) };
    case "workers":
      return { op: "workers" };
    case "forgetWorker":
      return { op: "forgetWorker", worktreeId: worktree(o.worktreeId) };
    case "delegate":
      return { op: "delegate", delegation: delegation(o.delegation) };
    case "delegations":
      return { op: "delegations", issues: list(o.issues, "issues", 1000).map((i) => str(i, ID, "issue id")) };
    case "checkouts":
      return { op: "checkouts" };
    case "setCheckout":
      return { op: "setCheckout", checkout: checkout(o.checkout) };
    case "contextComment":
      return { op: "contextComment", issueId: str(o.issueId, ID, "issueId") };
    case "setContextComment":
      return { op: "setContextComment", comment: contextComment(o.comment) };
    case "triaging":
      str(o.identifier, IDENTIFIER, "identifier");
      return { op: "triaging", session: str(o.session, ID, "session"), identifier: o.identifier as string };
    default:
      throw new Invalid("unknown op");
  }
}

// Carries out one operation.
export async function apply(storage: Storage, operation: Operation, nowMs: number): Promise<unknown> {
  switch (operation.op) {
    case "read": {
      const records: Record<string, SessionRecord | null> = {};
      for (let i = 0; i < operation.sessions.length; i += READ_BATCH) {
        const batch = operation.sessions.slice(i, i + READ_BATCH);
        const stored = await storage.get<SessionRecord>(batch.map((sessionId) => SESSION + sessionId));
        for (const sessionId of batch) records[sessionId] = stored.get(SESSION + sessionId) ?? null;
      }
      return { records };
    }
    case "record": {
      const key = SESSION + operation.sessionId;
      const record = applyChange((await storage.get<SessionRecord>(key)) ?? {}, operation.change);
      await storage.put(key, record);
      return { record };
    }
    case "take":
      return { taken: await take(storage, operation.claims, operation.token, nowMs) };
    case "release":
      // Only claims this take still holds.
      for (const name of operation.claims) {
        const held = await storage.get<StoredClaim>(CLAIM + name);
        if (held?.token === operation.token) await storage.delete(CLAIM + name);
      }
      return {};
    // A worktree started again at the same path replaces the old one's record.
    case "startWorker":
      await storage.put(WORKER + operation.worker.worktreeId, operation.worker);
      return {};
    case "workers":
      return { workers: [...(await storage.list<StoredWorker>({ prefix: WORKER })).values()].map(({ missingMs: _, ...record }) => record) };
    // Says whether there was a record to delete.
    case "forgetWorker": {
      const had = (await storage.get(WORKER + operation.worktreeId)) !== undefined;
      await storage.delete(WORKER + operation.worktreeId);
      return { forgotten: had };
    }
    // A later delegation of the same sub-issue replaces the earlier one.
    case "delegate":
      await storage.put<StoredDelegation>(DELEGATED + operation.delegation.issueId, { ...operation.delegation, storedMs: nowMs });
      return {};
    case "delegations": {
      const delegations: Record<string, Delegation | null> = {};
      for (const issueId of operation.issues) delegations[issueId] = await delegated(storage, issueId, nowMs);
      return { delegations };
    }
    // Every repo's shared conversations checkout.
    case "checkouts":
      return { checkouts: [...(await storage.list<CheckoutRecord>({ prefix: CHECKOUT })).values()] };
    // A repo's checkout created anew replaces its old record.
    case "setCheckout":
      await storage.put(CHECKOUT + operation.checkout.repoId, operation.checkout);
      return {};
    case "contextComment":
      return { comment: (await storage.get<ContextComment>(CONTEXT_COMMENT + operation.issueId)) ?? null };
    // A replacement overwrites the record of the comment it replaces.
    case "setContextComment":
      await storage.put(CONTEXT_COMMENT + operation.comment.issueId, operation.comment);
      return {};
    // Read from the triage machine, which alone starts and ends a session.
    case "triaging":
      return { holds: triaging(await storage.get<Triage>(TRIAGE_KEY), operation.session, operation.identifier) };
  }
}

// `listed` is every worker's worktree in a complete listing of Orca's (listing.ts). A record
// whose worktree is in it is present; one whose worktree isn't is marked missing from now,
// if it wasn't already, and any later complete listing that has it clears the mark. So a
// record written just after a listing is only marked, then cleared.
export async function markMissing(storage: Storage, listed: string[], hosts: string[], nowMs: number): Promise<void> {
  const present = new Set(listed);
  for (const [key, stored] of await storage.list<StoredWorker>({ prefix: WORKER })) {
    // A host that didn't answer, or is no longer paired, listed nothing of its own.
    if (!heard(stored.worktreeId, hosts)) continue;
    if (present.has(stored.worktreeId)) {
      if (stored.missingMs !== undefined) {
        const { missingMs: _, ...kept } = stored;
        await storage.put<StoredWorker>(key, kept);
      }
    } else if (stored.missingMs === undefined) {
      await storage.put<StoredWorker>(key, { ...stored, missingMs: nowMs });
    }
  }
}

// The worktrees missing from every complete listing for more than MISSING_KEEP_MS by `nowMs`,
// each with the mark it was found by: gone, as a failed reap or a removal by hand leaves them.
export async function goneWorkers(storage: Storage, hosts: string[], nowMs: number): Promise<{ worktreeId: string; missingMs: number }[]> {
  const gone: { worktreeId: string; missingMs: number }[] = [];
  for (const stored of (await storage.list<StoredWorker>({ prefix: WORKER })).values()) {
    if (stored.missingMs === undefined || !heard(stored.worktreeId, hosts)) continue;
    if (nowMs - stored.missingMs > MISSING_KEEP_MS) gone.push({ worktreeId: stored.worktreeId, missingMs: stored.missingMs });
  }
  return gone;
}

// The worktrees on a host the owner has unpaired, which `paired` (a listing's) leaves out: gone
// at once, as removing the host from Orca is how the owner says it is gone for good. Each with
// the mark it was found by, if any. A host that is only silent stays in `paired`.
export async function unpairedWorkers(storage: Storage, paired: string[]): Promise<{ worktreeId: string; missingMs?: number }[]> {
  const gone: { worktreeId: string; missingMs?: number }[] = [];
  for (const stored of (await storage.list<StoredWorker>({ prefix: WORKER })).values()) {
    const host = hostOfWorktree(stored.worktreeId);
    if (host !== undefined && !paired.includes(host)) gone.push({ worktreeId: stored.worktreeId, missingMs: stored.missingMs });
  }
  return gone;
}

// Deletes a gone worktree's record, as `forgetWorker` does, unless a listing since cleared
// its mark or a new start replaced it; whether it did. Nothing awaited but storage between
// the read and the write, so no listing comes between them.
export async function forgetGone(storage: Storage, worktreeId: string, missingMs: number | undefined): Promise<boolean> {
  if ((await storage.get<StoredWorker>(WORKER + worktreeId))?.missingMs !== missingMs) return false;
  await storage.delete(WORKER + worktreeId);
  return true;
}

// What a signed AgentSessionEvent tells the Issue webhook filter (issue.ts, `fromIssue`):
// Tolkster's app user ID, rewritten only when it differs, and that the session's issue has
// a Tolkster session, refreshed at most once per SESSION_ISSUE_REFRESH_MS. IDs and a time only.
export async function recordSessionEvent(storage: Storage, appUserId: string, issueId: string | null, nowMs: number): Promise<void> {
  if ((await storage.get<string>(APP_USER)) !== str(appUserId, ID, "app user id")) await storage.put(APP_USER, appUserId);
  if (issueId === null) return;
  const key = SESSION_ISSUE + str(issueId, ID, "issue id");
  const seenMs = await storage.get<number>(key);
  if (seenMs === undefined || nowMs - seenMs >= SESSION_ISSUE_REFRESH_MS || nowMs < seenMs) await storage.put(key, nowMs);
}

// Tolkster's app user ID, or null before a session event has brought it, and which of
// `issueIds` had a Tolkster session within SESSION_ISSUE_KEEP_MS. An ID of another shape
// is left out, never refused: it only narrows what is queued from a payload.
export async function tolkstersIssues(storage: Storage, issueIds: string[], nowMs: number): Promise<{ appUserId: string | null; withSessions: string[] }> {
  const withSessions: string[] = [];
  for (const issueId of issueIds) {
    if (!ID.test(issueId)) continue;
    const seenMs = await storage.get<number>(SESSION_ISSUE + issueId);
    if (seenMs !== undefined && nowMs - seenMs <= SESSION_ISSUE_KEEP_MS) withSessions.push(issueId);
  }
  return { appUserId: (await storage.get<string>(APP_USER)) ?? null, withSessions };
}

// The delegation Tolkster recorded for the sub-issue `issueId`, or null if there is none
// younger than DELEGATION_KEEP_MS.
export async function delegated(storage: Storage, issueId: string, nowMs: number): Promise<Delegation | null> {
  const stored = await storage.get<StoredDelegation>(DELEGATED + str(issueId, ID, "issue id"));
  if (!stored || nowMs - stored.storedMs > DELEGATION_KEEP_MS) return null;
  const { storedMs: _, ...kept } = stored;
  return kept;
}

// The chains Tolkster's delegations still counted record, by parent: the session of its
// newest delegation, and every sub-issue delegated.
export async function chains(storage: Storage, nowMs: number): Promise<Map<string, { session: string; at: string; children: string[] }>> {
  const byParent = new Map<string, { session: string; at: string; children: string[] }>();
  for (const stored of (await storage.list<StoredDelegation>({ prefix: DELEGATED })).values()) {
    if (nowMs - stored.storedMs > DELEGATION_KEEP_MS) continue;
    const held = byParent.get(stored.parentIssueId);
    if (held === undefined) byParent.set(stored.parentIssueId, { session: stored.sessionId, at: stored.at, children: [stored.issueId] });
    else {
      held.children.push(stored.issueId);
      if (stored.at > held.at) Object.assign(held, { session: stored.sessionId, at: stored.at });
    }
  }
  return byParent;
}

// A record with `change` applied. Times only move forward, so a late or repeated write
// can't take a record back.
export function applyChange(record: SessionRecord, change: Change): SessionRecord {
  const next: SessionRecord = { ...record };
  // A PR's newest read is kept: a late or repeated write for an older one changes nothing.
  const read = change.read;
  if (read && !record.reads?.some((r) => r.pr === read.pr && r.at >= read.at)) {
    const reads = [read, ...(record.reads ?? []).filter((r) => r.pr !== read.pr)];
    next.reads = reads.sort((a, b) => (a.at < b.at ? 1 : a.at > b.at ? -1 : 0)).slice(0, MAX_READS);
  }
  // A later turn's terminal replaces an earlier one's. The agent session is chosen once,
  // for the first turn, so every later turn resumes it.
  const turn = change.conversation;
  if (turn && (!record.conversation || turn.at >= record.conversation.at)) {
    next.conversation = { ...turn, agentSession: record.conversation?.agentSession ?? turn.agentSession };
  }
  return clean(next);
}

// Takes every claim in `claims` under `token`, or none of them. A claim is held by
// whoever took it, for good without a TTL, or for the `ttlMs` it took it with: a claim
// older than that is stale, e.g. the run that took it died, and is taken over, whatever TTL
// the next taker sends. A claim this token already holds counts as taken, keeps the time
// it was first taken, and takes the TTL it is taken with now.
async function take(storage: Storage, claims: Claim[], token: string, nowMs: number): Promise<boolean> {
  await prune(storage, nowMs);
  const held = new Map<string, StoredClaim>();
  for (const { name } of claims) {
    const claim = await storage.get<StoredClaim>(CLAIM + name);
    if (claim?.token === token) held.set(name, claim);
    else if (claim && (claim.ttlMs === null || nowMs - claim.takenAt <= claim.ttlMs)) return false;
  }
  for (const { name, ttlMs } of claims) await storage.put<StoredClaim>(CLAIM + name, { takenAt: held.get(name)?.takenAt ?? nowMs, token, ttlMs });
  return true;
}

// Removes claims older than CLAIM_KEEP_MS, delegations older than DELEGATION_KEEP_MS,
// and session issues older than SESSION_ISSUE_KEEP_MS, at most once per PRUNE_EVERY_MS: a claim kept
// a little longer changes nothing, as what it guarded is long done.
async function prune(storage: Storage, nowMs: number): Promise<void> {
  if (!(await pruneDue(storage, PRUNED_AT, nowMs))) return;
  await storage.put(PRUNED_AT, nowMs);
  for (const [key, claim] of await storage.list<StoredClaim>({ prefix: CLAIM })) {
    if (nowMs - claim.takenAt > CLAIM_KEEP_MS) await storage.delete(key);
  }
  for (const [key, stored] of await storage.list<StoredDelegation>({ prefix: DELEGATED })) {
    if (nowMs - stored.storedMs > DELEGATION_KEEP_MS) await storage.delete(key);
  }
  for (const [key, seenMs] of await storage.list<number>({ prefix: SESSION_ISSUE })) {
    if (nowMs - seenMs > SESSION_ISSUE_KEEP_MS) await storage.delete(key);
  }
}

// Runs a POST /state body: 400 for anything malformed.
export async function handle(storage: Storage, body: unknown, nowMs: number): Promise<Outcome> {
  let parsed: Operation;
  try {
    parsed = operation(body);
  } catch (error) {
    if (error instanceof Invalid) return { status: 400, body: JSON.stringify({ error: error.message }), log: { reason: error.message } };
    throw error;
  }
  const result = await apply(storage, parsed, nowMs);
  return { status: 200, body: JSON.stringify(result), log: summary(parsed, result) };
}

// What the log keeps of an operation and its result. No worktree IDs: they carry the
// workstation's paths.
function summary(operation: Operation, result: unknown): LogFields {
  const r = result as { records?: Record<string, unknown>; taken?: boolean; workers?: unknown[]; forgotten?: boolean; checkouts?: unknown[]; holds?: boolean; comment?: unknown };
  switch (operation.op) {
    case "read":
      return { op: "read", sessions: operation.sessions.length, found: Object.values(r.records ?? {}).filter((v) => v !== null).length };
    case "record":
      return { op: "record", sessionId: operation.sessionId, change: Object.keys(operation.change).sort().join(",") };
    case "take":
      return { op: "take", claims: operation.claims.length, taken: r.taken ?? null };
    case "release":
      return { op: "release", claims: operation.claims.length };
    case "startWorker":
      return { op: "startWorker", issue: operation.worker.issue, sessionId: operation.worker.sessionId };
    case "workers":
      return { op: "workers", workers: r.workers?.length ?? 0 };
    case "forgetWorker":
      return { op: "forgetWorker", forgotten: typeof r.forgotten === "boolean" ? r.forgotten : null };
    case "delegate":
      return { op: "delegate", issueId: operation.delegation.issueId, parentIssueId: operation.delegation.parentIssueId, sessionId: operation.delegation.sessionId };
    case "delegations":
      return { op: "delegations", issues: operation.issues.length };
    case "checkouts":
      return { op: "checkouts", checkouts: r.checkouts?.length ?? 0 };
    case "setCheckout":
      return { op: "setCheckout", repoId: operation.checkout.repoId };
    case "contextComment":
      return { op: "contextComment", issueId: operation.issueId, found: r.comment !== null && r.comment !== undefined };
    case "setContextComment":
      return { op: "setContextComment", issueId: operation.comment.issueId, commentId: operation.comment.commentId };
    case "triaging":
      // Not the session's id, which is what proves a verdict is its own.
      return { op: "triaging", identifier: operation.identifier, holds: r.holds ?? null };
  }
}
