import { IDENTIFIER, LINEAR_ID, TIME } from "../shared/linear.ts";

// Linear's Comment webhooks: which comments are the owner's, for the machines'
// `ownerCommented` (events.ts), whose `readComment` tells the issue's worker, if it has
// one, that there is something new to read, or, for a reply to a comment of Tolkster's
// outside a session, starts a turn in its thread. The Worker never does the work, and
// posts nothing in Linear in answer.
//
// Only the owner's new comments on an issue count: others' are information a worker reads
// when it reads the issue anyway, and the owner is the one the Worker knows. Whether a
// comment is a mention of Tolkster or a reply in one of its sessions' threads, which
// arrive as agent session events already, the payload can't say (it has the parent's id,
// but no link to a session), so `readComment` reads that from Linear.
//
// Comment text is untrusted, and never read here: only the comment's id, its issue's id
// and identifier, its author's id and when it was created, each checked against a plain
// pattern (the identifier's and the timestamp's are Linear's shapes, from
// src/shared/linear.ts).
//
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node.

export type CommentPayload = {
  action?: unknown;
  data?: { id?: unknown; issueId?: unknown; userId?: unknown; botActor?: unknown; createdAt?: unknown; issue?: { identifier?: unknown } | null };
};

// What was delivered, for the request's log line: the comment's and its issue's ids.
export type CommentDelivery = { commentId: string | null; issueId: string | null };

export function describeComment(payload: CommentPayload): CommentDelivery {
  const { id, issueId } = payload.data ?? {};
  return {
    commentId: typeof id === "string" && LINEAR_ID.test(id) ? id : null,
    issueId: typeof issueId === "string" && LINEAR_ID.test(issueId) ? issueId : null,
  };
}

// The owner's new comment on an issue, not one an integration posted as the owner.
export type OwnerComment = { issueId: string; commentId: string; identifier: string | null; at: string };

export function fromComment(payload: CommentPayload, ownerId: string): OwnerComment | null {
  if (payload.action !== "create") return null;
  const { id, issueId, userId, botActor, createdAt, issue } = payload.data ?? {};
  if (typeof id !== "string" || !LINEAR_ID.test(id) || typeof issueId !== "string" || !LINEAR_ID.test(issueId)) return null;
  if (!ownerId || userId !== ownerId || (botActor !== undefined && botActor !== null) || typeof createdAt !== "string" || !TIME.test(createdAt)) return null;
  const identifier = issue?.identifier;
  return {
    issueId,
    commentId: id,
    identifier: typeof identifier === "string" && IDENTIFIER.test(identifier) ? identifier : null,
    at: createdAt,
  };
}
