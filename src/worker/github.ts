// The GitHub App's webhook: its signature, what of a delivery the log keeps, and the push
// `listen` updates itself on (`fromPush`). Which deliveries become the state machine's
// events is events.ts's (`fromGitHub`).
//
// Review, comment and PR text is untrusted, and never read here: only the repo's name and
// the PR's number, each checked against a plain pattern.
//
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node.

import type { BaseTip } from "./realtime.ts";

// GitHub signs each delivery with HMAC-SHA256 over the raw body, as `sha256=<hex>`.
export const GITHUB_SIGNATURE_HEADER = "x-hub-signature-256";
export const GITHUB_EVENT_HEADER = "x-github-event";
export const GITHUB_DELIVERY_HEADER = "x-github-delivery";

// The hex digest in an `X-Hub-Signature-256` header, or null if it has another shape.
export function githubSignature(header: string | null): string | null {
  const match = /^sha256=([0-9a-f]{64})$/i.exec(header ?? "");
  return match ? match[1] : null;
}

// Every event the Worker takes: the app's webhook subscribes to these (docs/github-app.md),
// and a test pins them. A failed suite's own check runs, or its workflow run, add nothing,
// since GitHub sends `check_suite` `completed` to every app that can read checks, whoever
// created the suite.
export const GITHUB_EVENTS: readonly string[] = ["pull_request", "pull_request_review", "pull_request_review_comment", "pull_request_review_thread", "issue_comment", "check_suite", "push", "status"];

// GitHub's own rules for owner and repo names, and a branch (or a ref to one) without
// anything a command line could read as more than one argument. The one home of these
// patterns, for every boundary a repo, a branch or a commit crosses.
export const REPO = /^[A-Za-z0-9-]{1,39}\/[A-Za-z0-9._-]{1,100}$/;
export const BRANCH = /^[A-Za-z0-9._\/-]{1,200}$/;
export const SHA = /^[0-9a-f]{40}$/;
const REF = new RegExp(`^refs/heads/${BRANCH.source.slice(1)}`);

// What was delivered, for the log: the event, its action, the repo and the PR's number.
export type Delivery = { githubEvent: string; action: string | null; repo: string | null; number: number | null };

type Payload = {
  action?: unknown;
  repository?: { full_name?: unknown; default_branch?: unknown };
  ref?: unknown;
  after?: unknown;
  deleted?: unknown;
  head_commit?: { timestamp?: unknown } | null;
  pull_request?: { number?: unknown };
  issue?: { number?: unknown; pull_request?: unknown };
  check_suite?: { pull_requests?: unknown };
};

export function describeGitHub(githubEvent: string, payload: Payload): Delivery {
  const action = typeof payload.action === "string" ? payload.action : null;
  const fullName = payload.repository?.full_name;
  const repo = typeof fullName === "string" && REPO.test(fullName) ? fullName : null;
  const number = githubEvent === "check_suite" ? (suitePrs(payload)[0] ?? null) : prNumber(githubEvent, payload);
  return { githubEvent, action, repo, number };
}

// A push to a repo's default branch, as the tip the relay keeps for `listen`'s own update
// (cli/src/update.rs): the machines take the same push as their own event. `after` is the
// commit it moved to, which `listen` fetches before it pulls. A deleted branch moves nothing.
export function fromPush(githubEvent: string, payload: Payload): BaseTip | null {
  const fullName = payload.repository?.full_name;
  const branch = payload.repository?.default_branch;
  const { ref, after } = payload;
  if (githubEvent !== "push" || typeof fullName !== "string" || !REPO.test(fullName) || payload.deleted === true) return null;
  if (typeof ref !== "string" || !REF.test(ref) || typeof branch !== "string" || ref !== `refs/heads/${branch}`) return null;
  if (typeof after !== "string" || !SHA.test(after)) return null;
  return { type: "base", repo: fullName, ref, after };
}

// When a push's head commit was made, in ms, or 0 when it names no time: the relay orders
// tips by it (`moveBase`).
export function pushedAt(payload: Payload): number {
  const made = Date.parse(String(payload.head_commit?.timestamp));
  return Number.isNaN(made) ? 0 : made;
}

// The PRs a check suite ran for: those in its own repo whose head is the suite's commit.
// A fork's PRs aren't listed, and a worker's never is one.
function suitePrs(payload: Payload): number[] {
  const prs = payload.check_suite?.pull_requests;
  if (!Array.isArray(prs)) return [];
  const numbers = prs.map((pr) => (pr as { number?: unknown } | null)?.number).filter((n): n is number => Number.isSafeInteger(n) && (n as number) > 0);
  return [...new Set(numbers)];
}

// The PR a delivery is about. A comment on the conversation of an issue, rather than a
// PR, names none.
function prNumber(githubEvent: string, payload: Payload): number | null {
  const number = githubEvent === "issue_comment" ? (payload.issue?.pull_request ? payload.issue.number : null) : payload.pull_request?.number;
  return Number.isSafeInteger(number) && (number as number) > 0 ? (number as number) : null;
}
