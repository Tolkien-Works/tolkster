import { IDENTIFIER, LINEAR_ID, TIME } from "../shared/linear.ts";
import { ID } from "./state.ts";

// Linear's Issue webhooks: which become the triage machine's ask (docs/triage.md), or the
// issue's store's `startContext` (docs/context.md). The issue machines' own events from the
// same webhook are events.ts's.
// The Worker never does the work. Each only names the issue; `listen` reads it fresh from
// Linear, and decides whether to triage it, or to start its context session.
//
// Issue text is untrusted, and never read here: only the issue's id and identifier, its
// state's type, and which fields changed, each checked against a plain pattern (the
// identifier's and the timestamp's are Linear's shapes, from src/shared/linear.ts).
//
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node.

export type IssuePayload = {
  action?: unknown;
  data?: { id?: unknown; identifier?: unknown; updatedAt?: unknown; state?: { type?: unknown } | null; delegateId?: unknown; subscriberIds?: unknown; parentId?: unknown; priority?: unknown };
  updatedFrom?: unknown;
  actor?: { id?: unknown } | null;
};
// What was delivered, for the request's log line: the issue's id, and the names of the
// fields that changed, never their values.
export type IssueDelivery = { issueId: string | null; changed: string[] };

export function describeIssue(payload: IssuePayload): IssueDelivery {
  const id = payload.data?.id;
  const changed = payload.updatedFrom;
  return {
    issueId: typeof id === "string" && LINEAR_ID.test(id) ? id : null,
    changed: changed !== null && typeof changed === "object" ? Object.keys(changed).filter((k) => FIELD.test(k)).slice(0, 50) : [],
  };
}

// A field's name, as Linear's API names them.
const FIELD = /^[A-Za-z][A-Za-z0-9]{0,63}$/;

// The changed fields the filters read: whether each changed, and the delegate and parent it was.
const CHANGES = ["stateId", "delegateId", "parentId", "priority"] as const;

// The payload as the Durable Object judges it (relay.ts, `issue`): only what this module's
// and events.ts's `fromIssue` and `triageAsked` read, each value of its plain shape or left
// out, so no issue text crosses into it. A changed field's old value that isn't plain is null,
// which still says it changed.
export function issueFacts(payload: IssuePayload): IssuePayload {
  const data = payload.data ?? {};
  // A user's or state's ID, which the filters only compare: as wide as the stored app user ID's.
  const plain = (v: unknown) => (typeof v === "string" && ID.test(v) ? v : null);
  const changed = payload.updatedFrom;
  return {
    action: payload.action === "create" || payload.action === "update" ? payload.action : undefined,
    data: {
      id: typeof data.id === "string" && LINEAR_ID.test(data.id) ? data.id : undefined,
      identifier: typeof data.identifier === "string" && IDENTIFIER.test(data.identifier) ? data.identifier : undefined,
      updatedAt: typeof data.updatedAt === "string" && TIME.test(data.updatedAt) ? data.updatedAt : undefined,
      state: typeof data.state?.type === "string" && /^[a-z]{1,32}$/.test(data.state.type) ? { type: data.state.type } : null,
      delegateId: plain(data.delegateId),
      subscriberIds: Array.isArray(data.subscriberIds) ? data.subscriberIds.filter((s): s is string => plain(s) !== null) : undefined,
      parentId: plain(data.parentId),
      priority: Number.isSafeInteger(data.priority) ? data.priority : undefined,
    },
    updatedFrom: changed !== null && typeof changed === "object" ? Object.fromEntries(CHANGES.filter((k) => k in changed).map((k) => [k, plain((changed as Record<string, unknown>)[k])])) : undefined,
    actor: { id: plain(payload.actor?.id) },
  };
}

// What the Durable Object knows of Tolkster (state.ts, `tolkstersIssues`): its app user
// ID, and which of the issue and its parent had a Tolkster session.
export type Tolksters = { appUserId: string; withSessions: string[] };

// The issue and its parent, for the Durable Object to say which had a Tolkster session.
export function issueIds(payload: IssuePayload): string[] {
  const { id, parentId } = payload.data ?? {};
  return [id, parentId].filter((i): i is string => typeof i === "string" && LINEAR_ID.test(i));
}

// The issue a create in a triage state names, for the triage machine's `triageAsked`,
// whoever made it: `listen` decides whether it is triaged.
export function triageAsked(payload: IssuePayload): string | null {
  const { id, state } = payload.data ?? {};
  return payload.action === "create" && state?.type === "triage" && typeof id === "string" && LINEAR_ID.test(id) ? id : null;
}

// The completion an Issue webhook asks a context for, if any: an update that completed the
// issue (docs/context.md), at Linear's `updatedAt`. Given what the Durable Object knows of
// Tolkster, a completion counts only when it is Tolkster's: delegated to it before or after
// the update, subscribed to by it, or with a Tolkster session on it or its parent, as
// `listen`'s own test of whose an issue is reads it (`sessions::is_tolksters`). Without that
// knowledge (`null`), every one counts: `listen` decides, from its fresh read. Judged from
// the webhook's `updatedFrom`, so nothing guesses the change from times.
export function contextAsked(payload: IssuePayload, tolksters: Tolksters | null): { issueId: string; at: string } | null {
  const { id, updatedAt, state, delegateId, subscriberIds, parentId } = payload.data ?? {};
  if (typeof id !== "string" || !LINEAR_ID.test(id) || typeof updatedAt !== "string" || !TIME.test(updatedAt)) return null;
  if (payload.action !== "update") return null;
  const changed = payload.updatedFrom;
  if (changed === null || typeof changed !== "object") return null;
  if (!("stateId" in changed) || state?.type !== "completed") return null;
  if (tolksters !== null) {
    const { appUserId, withSessions } = tolksters;
    const delegated = delegateId === appUserId || ("delegateId" in changed && (changed as { delegateId?: unknown }).delegateId === appUserId);
    const subscribed = Array.isArray(subscriberIds) && subscriberIds.includes(appUserId);
    const sessions = withSessions.includes(id) || (typeof parentId === "string" && withSessions.includes(parentId));
    if (!delegated && !subscribed && !sessions) return null;
  }
  return { issueId: id, at: updatedAt };
}
