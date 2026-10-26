import type { ConversationComment, Ingress, RepoPushed } from "./events.ts";
import { BRANCH as BRANCH_NAME, REPO } from "./github.ts";
import { describe, info, warn } from "./log.ts";
import { LINEAR_ID } from "../shared/linear.ts";
import { pruneDue, type Storage } from "./storage.ts";

// The Durable Object's intake (docs/state-machine.md, "Events"): each event from events.ts
// is mapped to its issues and deduped by its kind, delivery ID and issue. It holds none of
// an issue's facts: whether an event is newer than what is held is the store's to judge
// (`mergeEvent`, machine/facts.ts). What it returns reaches the machine, except a replay and
// Tolkster's own chain's event on an issue its chain no longer holds.
//
// Correlation:
// - a session → its issue, written at each of its events;
// - a repo and branch → the issue whose worker works on it, written by `recordBranch` at
//   the worker's start, which unmaps the issue's branch before it;
// - a PR → its issue only through its head branch in the PR's own repo, so a fork's, or a
//   branch no worker started, maps to none;
// - a repo → the issues with a PR open in it (`OpenPrs`) whose worker's branch is still
//   recorded there, for a comment on a PR's conversation, by its number, and a commit
//   status, by the PR's head;
// - a Linear comment → its issue.
//
// Each event's writes for an issue, its delivery, its maps and whatever the caller's
// `also` adds (the machine's transition), go in one `put`, so a failure leaves none of
// them, and a redelivery applies it again. GitHub redelivers only on request.
//
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node.

const DAY_MS = 24 * 60 * 60 * 1000;
// Linear redelivers a failed webhook for hours, and GitHub for 3 days on request. A
// delivery is kept by the day it arrived, so for 7 days to 8.
export const SEEN_KEEP_MS = 7 * DAY_MS;
// An issue's maps are looked at again this long after its last event, and retired once its
// store no longer holds it (`Holds`).
export const KEEP_MS = 14 * DAY_MS;
// Keys one prune lists of each kind; a longer backlog is worked off by the events after.
const PRUNE_LIMIT = 128;

// `intake:seen:` and `intake:expiry:` keys begin with their UTC day, so a prune lists only
// the days before its cutoff.
const SEEN = "intake:seen:";
const RECORD = "intake:facts:";
const SESSION = "intake:session:";
const BRANCH = "intake:branch:";
// An issue's identifier, as Linear's webhooks give it, which a command carries for `listen`'s
// lanes and its reads; and the issue a note's thread is on, by its root comment, which stands
// for its session. Both retire with the issue's record.
const IDENTIFIER = "identifier:";
const THREAD = "thread:";
const EXPIRY = "intake:expiry:";
const PRUNED_AT = "intake:pruned-at";

// Whether the issue's store still holds it, so its maps stay past their expiry. It reads
// storage only, as it runs inside the input gate the intake's writes rely on.
export type Holds = (issueId: string) => Promise<boolean>;

// Whether a parent's chain holds on the issue, as its store has it: the parent's forwarded
// `chainRecorded` sets it and `parentHalted` clears it (machine/chain.ts). False, the chain's
// own sessions and delegation make no event.
export type Chained = (issueId: string) => Promise<boolean>;

// The issues with a PR open in the repo, with that PR's number and head as their stores
// hold them. It reads storage only, as `Holds` does.
export type OpenPrs = (repo: string) => Promise<{ issueId: string; number: number; head: string }[]>;

// What the intake keeps of one issue: what its maps need, to be unmapped and retired.
export type IssueRecord = {
  // The worker's branch, as `recordBranch` last mapped it.
  branch?: { repo: string; branch: string };
  // Each session mapped to the issue, by ID.
  sessions?: Record<string, true>;
  // Each note thread mapped to the issue, by its root comment.
  threads?: Record<string, true>;
  // The UTC day its maps are looked at again, the one its `intake:expiry:` key names. A key
  // for any other day is one an event since has superseded.
  expires?: string;
};

export type Routed = Exclude<Ingress, RepoPushed | ConversationComment> & { issueId: string };

// What became of an event on one issue: `accepted` reaches the machine; a `replay` was
// seen before; `refused` is Tolkster's own chain's on an issue its chain no longer holds.
export type Intaken = { event: Routed; outcome: "accepted" | "replay" | "refused" };

// An intaken event's `event_received` line. A session's names it, so `tolkster logs
// --session` finds what became of it (docs/observability.md).
export function received({ event, outcome }: Intaken): Record<string, unknown> {
  return { kind: event.kind, deliveryId: event.deliveryId, issueId: event.issueId, ...("sessionId" in event ? { sessionId: event.sessionId } : {}), outcome };
}

// Writes to add to an accepted event's own. It reads storage only, as `apply`'s put must
// follow its reads with no other I/O between.
export type Also = (accepted: Routed) => Promise<Record<string, unknown>>;

// Maps the event to its issues and applies it to each. One that maps to none is logged by
// its IDs and dropped. A push to a default branch never comes here: the coordinator takes it.
export async function intake(storage: Storage, event: Exclude<Ingress, RepoPushed>, nowMs: number, holds: Holds, openPrs: OpenPrs, also: Also, chained: Chained): Promise<Intaken[]> {
  const routed = await correlate(storage, event, openPrs);
  if (routed.length === 0) info("event_unmapped", { kind: event.kind, deliveryId: event.deliveryId });
  const results: Intaken[] = [];
  for (const one of routed) results.push(await apply(storage, one, nowMs, also, chained));
  // After, and never failing the event: what it prunes is only ever older, and an issue
  // this event touched has been given a later expiry.
  try {
    await prune(storage, nowMs, holds);
  } catch (thrown) {
    warn("intake_prune_failed", { error: describe(thrown) });
  }
  return results;
}

// The worker's branch in its repo, at its start: what its PR's events map by. The issue's
// branch before it, and the issue that held this branch, if another, are unmapped, so a
// superseded branch's events reach no issue.
export async function recordBranch(storage: Storage, issueId: string, repo: string, branch: string, nowMs: number): Promise<void> {
  const writes = await branchWrites(storage, issueId, repo, branch, nowMs);
  if (Object.keys(writes).length > 0) await storage.put(writes);
}

// `recordBranch`'s writes, for the caller's own put. It reads storage only, as `Also` does.
export async function branchWrites(storage: Storage, issueId: string, repo: string, branch: string, nowMs: number): Promise<Record<string, unknown>> {
  if (!LINEAR_ID.test(issueId) || !REPO.test(repo) || !BRANCH_NAME.test(branch)) throw new Error("invalid branch record");
  repo = repo.toLowerCase();
  const key = branchKey(repo, branch);
  const held = await storage.get<string | null>(key);
  const mine = await record(storage, issueId);
  if (held === issueId && mine.branch?.repo === repo && mine.branch.branch === branch) return {};
  const writes: Record<string, unknown> = { [key]: issueId };
  if (mine.branch !== undefined) {
    const old = branchKey(mine.branch.repo, mine.branch.branch);
    if (old !== key && (await storage.get<string | null>(old)) === issueId) writes[old] = null;
  }
  if (held !== undefined && held !== null && held !== issueId) {
    info("branch_remapped", { from: held, to: issueId });
    const theirs = await record(storage, held);
    delete theirs.branch;
    writes[RECORD + held] = theirs;
  }
  mine.branch = { repo, branch };
  arm(mine, issueId, nowMs, writes);
  writes[RECORD + issueId] = mine;
  return writes;
}

// The issue a session was mapped to at its start, while its records last: what a worker's
// report on the session is delivered to.
export async function sessionIssue(storage: Storage, sessionId: string): Promise<string | null> {
  return (await storage.get<string>(SESSION + sessionId)) ?? null;
}

// The issue a note's thread is on, by its root comment, while the issue's records last.
export async function threadIssue(storage: Storage, root: string): Promise<string | null> {
  return (await storage.get<string>(THREAD + root)) ?? null;
}

// The thread's map, added to the caller's `writes` for its own put, on the issue's record as
// they hold it. It reads storage only, as `Also` does.
export async function threadWrites(storage: Storage, issueId: string, root: string, nowMs: number, writes: Record<string, unknown>): Promise<void> {
  const mine = copy((writes[RECORD + issueId] as IssueRecord | undefined) ?? (await storage.get<IssueRecord>(RECORD + issueId)));
  (mine.threads ??= {})[root] = true;
  writes[THREAD + root] = issueId;
  arm(mine, issueId, nowMs, writes);
  writes[RECORD + issueId] = mine;
}

// A chain's session that no event mapped, as one a reset's rebuild began (relay.ts, `learned`),
// added to the caller's `writes` as `threadWrites` adds a thread's.
export async function sessionWrites(storage: Storage, issueId: string, sessionId: string, nowMs: number, writes: Record<string, unknown>): Promise<void> {
  const mine = copy((writes[RECORD + issueId] as IssueRecord | undefined) ?? (await storage.get<IssueRecord>(RECORD + issueId)));
  (mine.sessions ??= {})[sessionId] = true;
  writes[SESSION + sessionId] = issueId;
  arm(mine, issueId, nowMs, writes);
  writes[RECORD + issueId] = mine;
}

// The issue's identifier, kept for its commands, and its record armed so the identifier retires
// with it.
export async function rememberIdentifier(storage: Storage, issueId: string, identifier: string, nowMs: number): Promise<void> {
  const writes: Record<string, unknown> = {};
  if ((await storage.get<string>(IDENTIFIER + issueId)) !== identifier) writes[IDENTIFIER + issueId] = identifier;
  const mine = await record(storage, issueId);
  if (Object.keys(writes).length === 0 && mine.expires === dayOf(nowMs + KEEP_MS)) return;
  arm(mine, issueId, nowMs, writes);
  writes[RECORD + issueId] = mine;
  await storage.put(writes);
}

export async function identifierOf(storage: Storage, issueId: string): Promise<string | null> {
  return (await storage.get<string>(IDENTIFIER + issueId)) ?? null;
}

// A copy of the issue's record as held, for a write to change.
export async function record(storage: Storage, issueId: string): Promise<IssueRecord> {
  return copy(await storage.get<IssueRecord>(RECORD + issueId));
}

function copy(held: IssueRecord | undefined): IssueRecord {
  return { ...held, ...(held?.sessions && { sessions: { ...held.sessions } }), ...(held?.threads && { threads: { ...held.threads } }) };
}

const branchKey = (repo: string, branch: string) => `${BRANCH}${repo}:${branch}`;
const dayOf = (ms: number) => new Date(ms).toISOString().slice(0, 10);
const seenKey = (day: string, event: Routed) => `${SEEN}${day}:${event.kind}:${event.deliveryId}:${event.issueId}`;

// The days a delivery's key may be under while it is kept: the cutoff's through today's.
function seenDays(nowMs: number): string[] {
  const today = dayOf(nowMs);
  const days = [dayOf(nowMs - SEEN_KEEP_MS)];
  while (days[days.length - 1] !== today) days.push(dayOf(Date.parse(days[days.length - 1]) + DAY_MS));
  return days;
}

// Sets the issue's expiry, adding its key to `writes` when it moved. The key it had is left
// for the prune, which drops it as superseded.
function arm(r: IssueRecord, issueId: string, nowMs: number, writes: Record<string, unknown>): void {
  const expires = dayOf(nowMs + KEEP_MS);
  if (r.expires === expires) return;
  r.expires = expires;
  writes[`${EXPIRY}${expires}:${issueId}`] = issueId;
}
const mapped = (issueId: string | null | undefined): issueId is string => typeof issueId === "string";

async function correlate(storage: Storage, event: Exclude<Ingress, RepoPushed>, openPrs: OpenPrs): Promise<Routed[]> {
  switch (event.kind) {
    case "sessionStarted":
    case "ownerPrompted":
    case "ownerStopped":
    case "delegateChanged":
    case "issueStateChanged":
    case "priorityChanged":
    case "ownerCommented":
      return [event];
    case "checksCompleted": {
      if (event.branches === null) return (await recorded(storage, openPrs, event.repo, (pr) => pr.head === event.sha)).map(({ issueId }) => ({ ...event, issueId }));
      const issues = await storage.get<string | null>(event.branches.map((b) => branchKey(event.repo, b)));
      return [...new Set([...issues.values()].filter(mapped))].map((issueId) => ({ ...event, issueId }));
    }
    default: {
      if ("conversation" in event) {
        // Stamped with the head its store holds, on the branch its worker last recorded.
        const { deliveryId, kind, repo, number, reviewerId, copilot, at } = event;
        return (await recorded(storage, openPrs, repo, (pr) => pr.number === number)).map(({ issueId, head, branch }) => ({ deliveryId, kind, repo, number, branch, head, sha: head, reviewerId, copilot, at, issueId }));
      }
      const issueId = await storage.get<string | null>(branchKey(event.repo, event.branch));
      return mapped(issueId) ? [{ ...event, issueId }] : [];
    }
  }
}

// The issues with a PR open in the repo that `matches`, each with its worker's branch: only
// one whose worker's branch is still recorded in that repo, so an issue whose branch another
// took, or whose worker moved to another repo, hears no more of the PR it had.
async function recorded(storage: Storage, openPrs: OpenPrs, repo: string, matches: (pr: { number: number; head: string }) => boolean): Promise<{ issueId: string; head: string; branch: string }[]> {
  const prs = (await openPrs(repo)).filter(matches);
  const records = await storage.get<IssueRecord>(prs.map((pr) => RECORD + pr.issueId));
  return prs.flatMap(({ issueId, head }) => {
    const branch = records.get(RECORD + issueId)?.branch;
    return branch?.repo === repo ? [{ issueId, head, branch: branch.branch }] : [];
  });
}

// Reads, then writes, with no I/O but storage's between: the Durable Object's input gate
// keeps every other event out until the write, so none applies on a read this one replaced.
async function apply(storage: Storage, event: Routed, nowMs: number, also: Also, chained: Chained): Promise<Intaken> {
  const seen = seenDays(nowMs).map((day) => seenKey(day, event));
  const recordKey = RECORD + event.issueId;
  const held = await storage.get<unknown>([...seen, recordKey]);
  if (seen.some((key) => held.has(key))) return { event, outcome: "replay" };
  const before = held.get(recordKey) as IssueRecord | undefined;
  const after = copy(before);
  const byChain = ("trust" in event && event.trust === "chain") || (event.kind === "delegateChanged" && event.by === "chain");
  const refused = byChain && !(await chained(event.issueId));
  const writes: Record<string, unknown> = { [seen[seen.length - 1]]: nowMs };
  // Written again by each, so a map a retire part way through deleted comes back.
  if (!refused && (event.kind === "sessionStarted" || event.kind === "ownerPrompted" || event.kind === "ownerStopped")) {
    (after.sessions ??= {})[event.sessionId] = true;
    writes[SESSION + event.sessionId] = event.issueId;
  }
  arm(after, event.issueId, nowMs, writes);
  if (JSON.stringify(after) !== JSON.stringify(before ?? {})) writes[recordKey] = after;
  if (!refused) Object.assign(writes, await also(event));
  await storage.put(writes);
  return { event, outcome: refused ? "refused" : "accepted" };
}

// The days of deliveries before the cutoff `seenDays` starts at, and the issues whose expiry
// has passed, at most once per PRUNE_EVERY_MS while nothing is left over.
async function prune(storage: Storage, nowMs: number, holds: Holds): Promise<void> {
  if (!(await pruneDue(storage, PRUNED_AT, nowMs))) return;
  const seen = await storage.list<number>({ prefix: SEEN, end: SEEN + dayOf(nowMs - SEEN_KEEP_MS), limit: PRUNE_LIMIT });
  if (seen.size > 0) await storage.delete([...seen.keys()]);
  const due = await storage.list<string>({ prefix: EXPIRY, end: EXPIRY + dayOf(nowMs), limit: PRUNE_LIMIT });
  for (const [key, issueId] of due) {
    // One that fails is tried again by the next prune, without holding back the rest.
    try {
      await retire(storage, key, issueId, nowMs, holds);
    } catch (thrown) {
      warn("intake_retire_failed", { issueId, error: describe(thrown) });
    }
  }
  if (seen.size < PRUNE_LIMIT && due.size < PRUNE_LIMIT) await storage.put({ [PRUNED_AT]: nowMs });
}

// An issue whose expiry `key` names: its record, session, thread and branch maps and its identifier go together once
// its store no longer holds it. Otherwise it is armed again. They go in one delete, which
// storage takes atomically up to 128 keys; past that the record and key go last, so a
// failure part way leaves them for the next prune to finish.
async function retire(storage: Storage, key: string, issueId: string, nowMs: number, holds: Holds): Promise<void> {
  const held = await storage.get<IssueRecord>(RECORD + issueId);
  if (held?.expires !== key.slice(EXPIRY.length, EXPIRY.length + 10)) {
    await storage.delete([key]);
    return;
  }
  if (await holds(issueId)) {
    const r = copy(held);
    const writes: Record<string, unknown> = {};
    arm(r, issueId, nowMs, writes);
    await storage.put({ ...writes, [RECORD + issueId]: r });
    await storage.delete([key]);
    return;
  }
  const maps = [...Object.keys(held.sessions ?? {}).map((id) => SESSION + id), ...Object.keys(held.threads ?? {}).map((root) => THREAD + root), IDENTIFIER + issueId];
  if (held.branch !== undefined) {
    const branch = branchKey(held.branch.repo, held.branch.branch);
    if ((await storage.get<string | null>(branch)) === issueId) maps.push(branch);
  }
  const keys = [...maps, RECORD + issueId, key];
  const last = keys.splice(-128);
  for (let i = 0; i < keys.length; i += 128) await storage.delete(keys.slice(i, i + 128));
  await storage.delete(last);
  info("intake_retired", { issueId });
}
