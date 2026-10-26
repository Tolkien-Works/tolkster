import type { AgentSessionEventWebhookPayload } from "@linear/sdk/webhooks";
import { LINEAR_ID, TIME } from "../shared/linear.ts";
import type { Decision, SessionTrust } from "./decide.ts";
import { type CommentPayload, fromComment as ownerComment } from "./comment.ts";
import type { IssuePayload } from "./issue.ts";
import { WEBHOOK_DELIVERY } from "./check.ts";
import { BRANCH, REPO, SHA } from "./github.ts";
import type { Checks } from "./machine/types.ts";
import { UUID } from "./state.ts";

// Linear's and GitHub's webhooks as the state machine's events (docs/state-machine.md,
// "Events"), named as test/contract.json's `machine` names them. Each carries its delivery
// ID and its clock, and only IDs, times, SHAs and enums: never issue, comment or review
// text, nor people's details. Which issue an event is on is the intake's (intake.ts).
//
// The checks at ingress stay where they are: a session event becomes one only once
// `decide` has trusted it, as the owner's, the owner's reply in Tolkster's own thread, or
// a sub-issue's that Tolkster's chain delegated, which the intake takes only while the
// issue's `chain` holds.
//
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node.

// A Linear time is compared only with Linear's, a GitHub time only with GitHub's. GitHub's
// are to the second and leave here with `.000` milliseconds, the machine's `TIME`, as
// listen's `evaluatePr` writes them: a read and an event from the same second then compare
// equal, where `…:04Z` would sort after `…:04.000Z`.
const GITHUB_TIME = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/;

// Copilot's review logins, as REST and its app give them (GraphQL's is in
// cli/src/github.rs, `COPILOT`).
const COPILOT = ["copilot", "copilot-pull-request-reviewer[bot]"];
const copilot = (login: unknown) => typeof login === "string" && COPILOT.includes(login.toLowerCase());

type Delivered = { deliveryId: string };

export type SessionEvent = Delivered & {
  kind: "sessionStarted" | "ownerPrompted" | "ownerStopped";
  sessionId: string;
  issueId: string;
  trust: SessionTrust;
  // A mention's, from a comment, or a delegation's, as `decide` tells them apart.
  sessionKind: "mention" | "delegation";
  // The owner's comment that mentioned Tolkster, by its ID, for the owner's mention's session:
  // the machine tells that comment's read apart by it, if the session's event lands first.
  commentId: string | null;
  // The session's or the activity's `createdAt`.
  at: string;
};

// What an Issue webhook says of the issue, at its `updatedAt`.
export type IssueSnapshot = { state: string; priority: number | null; parentId: string | null };

export type IssueEvent = Delivered & { issueId: string; at: string; issue: IssueSnapshot } & (
    // `by` is who delegated it to Tolkster: the owner, or Tolkster's chain on a sub-issue.
    // Null when it was taken away from Tolkster, by anyone.
    | { kind: "delegateChanged"; delegated: boolean; by: "owner" | "chain" | null }
    // `from` is the parent the issue left, when this update moved it.
    | { kind: "issueStateChanged"; from: string | null }
    | { kind: "priorityChanged" }
  );

// A PR in its base's repo, from a head in that same repo: a fork's never becomes one.
export type PrRef = { repo: string; number: number; branch: string; head: string };

// A push's `before` is the head it replaced, so pushes in one GitHub second are ordered by
// their chain.
export type PrEvent = Delivered & PrRef & ({ kind: "prOpened" | "prMerged" | "prClosed" | "autoMergeDisabled"; at: string } | { kind: "prPushed"; before: string; at: string });

// Whether a webhook's push to the open PR held is a replay of the held push or one before
// it, by GitHub's chain of `before` SHAs, as the machine's `mergeEvent` judges it:
// no newer than the held push, to its head, or to the head it replaced but not from its head
// (a force-push back). Time orders the rest: the PR's `updated_at` never goes back, so a
// push from the held head with an older time is a cycle, not the next.
export function earlier(held: { head: string; before?: string }, heldAt: string, push: { head: string; before: string; at: string }): boolean {
  return push.at <= heldAt && (push.head === held.head || (push.head === held.before && push.before !== held.head));
}

// `sha` is the commit reviewed, `at` its submission, and `reviewerId` the GitHub user's
// ID: which of them are the repo's `reviewers` is `evaluatePr`'s to read.
export type ReviewEvent = Delivered & PrRef & { kind: "prReviewed"; sha: string; reviewerId: number; copilot: boolean; at: string };

// A comment on a PR's conversation names no commit: the intake maps it by the PR's number
// and stamps it with the head its store's `pr` holds when it arrives, as a `prReviewed`.
export type ConversationComment = Delivered & { kind: "prReviewed"; conversation: true; repo: string; number: number; reviewerId: number; copilot: boolean; at: string };

// A check suite, for the branches of its PRs whose head is `sha`, or a commit status, which
// names no PR (`branches` null): mapped by `sha` to the issue whose `pr` has it as head.
// `source` names the suite, or the status's context by a hash, so each is kept apart.
export type ChecksEvent = Delivered & { kind: "checksCompleted"; repo: string; branches: string[] | null; sha: string; source: string; result: Checks; at: string };

// The owner's new comment on an issue, which `readComment` tells apart from a mention or a
// reply in a session's thread.
export type OwnerCommented = Delivered & { kind: "ownerCommented"; issueId: string; commentId: string; at: string };

// A push to a repo's default branch, for the coordinator to send each issue with a PR open
// in it as `basePushed`. Its clock is the Durable Object's receipt.
export type RepoPushed = Delivered & { kind: "repoPushed"; repo: string; sha: string };

export type Ingress = SessionEvent | IssueEvent | OwnerCommented | PrEvent | ReviewEvent | ConversationComment | ChecksEvent | RepoPushed;

// The owner's session or prompt, once `decide` has trusted it and found its issue: the
// decision's trust, kind and comment as an event, their IDs and time checked for shape.
export function fromSession(payload: AgentSessionEventWebhookPayload, decision: Decision, deliveryId: string): SessionEvent | null {
  const session = payload.agentSession;
  const issueId = session?.issueId ?? session?.issue?.id;
  const { request } = decision;
  if (!WEBHOOK_DELIVERY.test(deliveryId) || !request || typeof session.id !== "string" || !LINEAR_ID.test(session.id)) return null;
  if (typeof issueId !== "string" || !LINEAR_ID.test(issueId) || !TIME.test(request.at)) return null;
  const { trust, sessionKind, commentId, at } = request;
  const base = { deliveryId, sessionId: session.id, issueId, trust, sessionKind, commentId: typeof commentId === "string" && UUID.test(commentId) ? commentId : null, at } as const;
  switch (decision.kind) {
    case "start":
      return { ...base, kind: "sessionStarted" };
    case "deliver":
    case "stop":
      return { ...base, kind: decision.kind === "stop" ? "ownerStopped" : "ownerPrompted" };
    default:
      return null;
  }
}

// Linear's Issue updates: a delegate change that involves Tolkster, a change of state or of
// parent, and one of priority; and a create delegated to Tolkster, and one under a parent, which reaches
// that parent as a move under it.
// A delegation to Tolkster counts only when the owner made it, or Tolkster itself, which
// only its chain does; anyone else's is cleared at ingress (decide.ts) and makes no event.
// A delegation taken away from Tolkster counts whoever took it. Without Tolkster's app
// user ID (`appUserId` null) no delegate change can be told apart, and none is made. A
// change of state or parent is made for any new one; the store moves no machine on one whose
// state's type, Triage and parent are those held.
export function fromIssue(payload: IssuePayload & { actor?: { id?: unknown } | null }, deliveryId: string, ownerUserId: string, appUserId: string | null): IssueEvent[] {
  if ((payload.action !== "update" && payload.action !== "create") || !WEBHOOK_DELIVERY.test(deliveryId)) return [];
  const data = payload.data ?? {};
  const { id, updatedAt, state, delegateId, parentId } = data;
  const priority = (data as { priority?: unknown }).priority;
  const changed = payload.updatedFrom;
  if (typeof id !== "string" || !LINEAR_ID.test(id) || typeof updatedAt !== "string" || !TIME.test(updatedAt)) return [];
  if (typeof state?.type !== "string" || !/^[a-z]{1,32}$/.test(state.type)) return [];
  const issue: IssueSnapshot = {
    state: state.type,
    priority: Number.isSafeInteger(priority) ? (priority as number) : null,
    parentId: typeof parentId === "string" && LINEAR_ID.test(parentId) ? parentId : null,
  };
  const base = { deliveryId, issueId: id, at: updatedAt, issue };
  const actor = payload.actor?.id;
  const by = actor === ownerUserId ? "owner" : actor === appUserId ? "chain" : null;
  const now = appUserId !== null && delegateId === appUserId;
  if (payload.action === "create") {
    // An issue created delegated to Tolkster has no update to say so, and no history entry
    // that names who delegated it.
    const events: IssueEvent[] = now && by !== null ? [{ ...base, kind: "delegateChanged", delegated: true, by }] : [];
    if (issue.parentId !== null) events.push({ ...base, kind: "issueStateChanged", from: null });
    return events;
  }
  if (changed === null || typeof changed !== "object") return [];
  const events: IssueEvent[] = [];
  if ("delegateId" in changed && appUserId !== null) {
    const was = (changed as { delegateId?: unknown }).delegateId === appUserId;
    if (now && !was && by !== null) events.push({ ...base, kind: "delegateChanged", delegated: true, by });
    if (was && !now) events.push({ ...base, kind: "delegateChanged", delegated: false, by: null });
  }
  const left = (changed as { parentId?: unknown }).parentId;
  const from = "parentId" in changed && typeof left === "string" && UUID.test(left) && left !== issue.parentId ? left : null;
  if ("stateId" in changed || "parentId" in changed) events.push({ ...base, kind: "issueStateChanged", from });
  // Linear's priorities are 0 to 4, as the machine takes them.
  if ("priority" in changed && issue.priority !== null && issue.priority <= 4 && issue.priority >= 0) events.push({ ...base, kind: "priorityChanged" });
  return events;
}

// Linear's Comment webhooks: the owner's new comment on an issue, not one an integration
// posted as the owner, as comment.ts judges it.
export function fromLinearComment(payload: CommentPayload, deliveryId: string, ownerUserId: string): OwnerCommented | null {
  const comment = WEBHOOK_DELIVERY.test(deliveryId) ? ownerComment(payload, ownerUserId) : null;
  if (comment === null) return null;
  return { deliveryId, kind: "ownerCommented", issueId: comment.issueId, commentId: comment.commentId, at: comment.at };
}

type GitHubPr = {
  number?: unknown;
  updated_at?: unknown;
  merged_at?: unknown;
  closed_at?: unknown;
  merged?: unknown;
  head?: { ref?: unknown; sha?: unknown; repo?: { full_name?: unknown } | null };
};

type GitHubUser = { id?: unknown; login?: unknown; type?: unknown } | null;

export type GitHubPayload = {
  action?: unknown;
  repository?: { full_name?: unknown; default_branch?: unknown };
  sender?: GitHubUser;
  pull_request?: GitHubPr;
  review?: { commit_id?: unknown; submitted_at?: unknown; user?: GitHubUser };
  comment?: { commit_id?: unknown; created_at?: unknown; user?: GitHubUser };
  issue?: { number?: unknown; pull_request?: unknown };
  check_suite?: { id?: unknown; conclusion?: unknown; head_sha?: unknown; updated_at?: unknown; pull_requests?: unknown };
  sha?: unknown;
  state?: unknown;
  context?: unknown;
  updated_at?: unknown;
  ref?: unknown;
  before?: unknown;
  after?: unknown;
  deleted?: unknown;
};

const PASSED = ["success", "neutral", "skipped"];
const FAILED = ["failure", "timed_out", "cancelled", "action_required"];
// CI that never ran the change: `tolkster pr checks`' exit 4.
const COULDNT_RUN = ["startup_failure"];

// GitHub's webhooks for the app's repos. A base's retargeting has no event in the spec,
// and makes none.
export function fromGitHub(githubEvent: string, payload: GitHubPayload, deliveryId: string): Ingress[] {
  const fullName = payload.repository?.full_name;
  if (!WEBHOOK_DELIVERY.test(deliveryId) || typeof fullName !== "string" || !REPO.test(fullName)) return [];
  const repo = fullName.toLowerCase();
  const action = payload.action;

  if (githubEvent === "push") {
    const branch = payload.repository?.default_branch;
    if (payload.deleted === true || typeof branch !== "string" || payload.ref !== `refs/heads/${branch}`) return [];
    const sha = payload.after;
    return typeof sha === "string" && SHA.test(sha) ? [{ deliveryId, kind: "repoPushed", repo, sha }] : [];
  }

  if (githubEvent === "check_suite") {
    const suite = payload.check_suite;
    if (action !== "completed" || !suite) return [];
    const result = checksResult(suite.conclusion);
    const { id, head_sha: sha, updated_at: at } = suite;
    // The suite's PRs, whose heads are in this repo: GitHub lists none for a fork's, whose
    // own branch could take any name.
    const prs = Array.isArray(suite.pull_requests) ? suite.pull_requests : [];
    const branches = prs.map((pr) => (pr as { head?: { ref?: unknown; sha?: unknown } } | null)?.head).filter((h) => h?.sha === sha).map((h) => h?.ref).filter((r): r is string => typeof r === "string" && BRANCH.test(r));
    if (result === null || !isSha(sha) || !Number.isSafeInteger(id) || !isGitHubTime(at) || branches.length === 0) return [];
    return [{ deliveryId, kind: "checksCompleted", repo, branches: [...new Set(branches)], sha, source: `suite:${id}`, result, at: ms(at) }];
  }

  if (githubEvent === "status") {
    const { sha, state, context, updated_at: at } = payload;
    const result = state === "success" ? "passed" : state === "failure" || state === "error" ? "failed" : null;
    if (result === null || !isSha(sha) || typeof context !== "string" || !isGitHubTime(at)) return [];
    return [{ deliveryId, kind: "checksCompleted", repo, branches: null, sha, source: `status:${hash(context)}`, result, at: ms(at) }];
  }

  if (githubEvent === "issue_comment") {
    const { number, pull_request: onPr } = payload.issue ?? {};
    const { created_at: at, user } = payload.comment ?? {};
    const reviewerId = user?.id;
    if (action !== "created" || !onPr || !Number.isSafeInteger(number) || (number as number) <= 0 || !isGitHubTime(at)) return [];
    if (!Number.isSafeInteger(reviewerId) || (reviewerId as number) <= 0) return [];
    return [{ deliveryId, kind: "prReviewed", conversation: true, repo, number: number as number, reviewerId: reviewerId as number, copilot: copilot(user?.login), at: ms(at) }];
  }

  const pr = prRef(repo, payload.pull_request);
  if (pr === null) return [];
  const event = { deliveryId, ...pr };
  const raw = payload.pull_request as GitHubPr;

  if (githubEvent === "pull_request") {
    if (action === "opened" || action === "reopened" || action === "synchronize" || action === "auto_merge_disabled") {
      if (!isGitHubTime(raw.updated_at)) return [];
      if (action === "synchronize") return isSha(payload.before) ? [{ ...event, kind: "prPushed", before: payload.before, at: ms(raw.updated_at) }] : [];
      return [{ ...event, kind: action === "auto_merge_disabled" ? "autoMergeDisabled" : "prOpened", at: ms(raw.updated_at) }];
    }
    if (action === "closed") {
      const merged = raw.merged === true;
      const at = merged ? raw.merged_at : raw.closed_at;
      return isGitHubTime(at) ? [{ ...event, kind: merged ? "prMerged" : "prClosed", at: ms(at) }] : [];
    }
    return [];
  }

  if (githubEvent === "pull_request_review" && action === "submitted") {
    const { commit_id: sha, submitted_at: at, user } = payload.review ?? {};
    return review(event, sha, at, user);
  }

  if (githubEvent === "pull_request_review_comment" && action === "created") {
    const { commit_id: sha, created_at: at, user } = payload.comment ?? {};
    return review(event, sha, at, user);
  }

  // A thread resolved or unresolved, against the PR's head when the PR last changed. A
  // bot's, such as the app resolving its own worker's threads, is no one's feedback.
  if (githubEvent === "pull_request_review_thread") {
    if (payload.sender?.type === "Bot") return [];
    return review(event, pr.head, raw.updated_at, payload.sender ?? null);
  }

  return [];
}

function review(event: Delivered & PrRef, sha: unknown, at: unknown, user: GitHubUser | undefined): ReviewEvent[] {
  const reviewerId = user?.id;
  if (!isSha(sha) || !isGitHubTime(at) || !Number.isSafeInteger(reviewerId) || (reviewerId as number) <= 0) return [];
  return [{ ...event, kind: "prReviewed", sha, reviewerId: reviewerId as number, copilot: copilot(user?.login), at: ms(at) }];
}

function prRef(repo: string, pr: GitHubPr | undefined): PrRef | null {
  if (!pr) return null;
  const { number, head } = pr;
  if (!Number.isSafeInteger(number) || (number as number) <= 0) return null;
  // A fork's head is in another repo: its PR is never a worker's.
  const headRepo = head?.repo?.full_name;
  if (typeof headRepo !== "string" || headRepo.toLowerCase() !== repo) return null;
  if (typeof head?.ref !== "string" || !BRANCH.test(head.ref) || !isSha(head.sha)) return null;
  return { repo, number: number as number, branch: head.ref, head: head.sha };
}

function checksResult(conclusion: unknown): Checks | null {
  if (typeof conclusion !== "string") return null;
  if (PASSED.includes(conclusion)) return "passed";
  if (FAILED.includes(conclusion)) return "failed";
  return COULDNT_RUN.includes(conclusion) ? "couldntRun" : null;
}

// FNV-1a, so a status's context, a CI's own name, is kept apart without keeping its text.
function hash(text: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 0x01000193) >>> 0;
  return h.toString(16).padStart(8, "0");
}

const isSha = (value: unknown): value is string => typeof value === "string" && SHA.test(value);
const isGitHubTime = (value: unknown): value is string => typeof value === "string" && GITHUB_TIME.test(value);
const ms = (githubTime: string): string => `${githubTime.slice(0, -1)}.000Z`;
