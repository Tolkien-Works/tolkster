import assert from "node:assert/strict";
import { test } from "node:test";
import type { AgentSessionEventWebhookPayload } from "@linear/sdk/webhooks";
import { decide } from "../src/worker/decide.ts";
import { fromGitHub, fromIssue, fromLinearComment, fromSession, type Ingress } from "../src/worker/events.ts";
import contract from "./contract.json" with { type: "json" };

const OWNER = "owner-1";
const APP = "app-1";
const DELIVERY = "0f6a1c2e-1111-4222-8333-444455556666";
const AT = "2026-10-01T10:00:00.000Z";
const ISSUE = "0a1b2c3d-0000-4000-8000-000000000001";
const SESSION = "5e550000-0000-4000-8000-000000000001";
const repository = { full_name: "Tolkien-Works/tolkster", default_branch: "main" };
const REPO = "tolkien-works/tolkster";
const HEAD = "a".repeat(40);
const head = { ref: "fix-it-tw-1", sha: HEAD, repo: { full_name: "Tolkien-Works/tolkster" } };

const known = new Set([...Object.keys(contract.machine.events), ...contract.machine.coordinator]);
function pinned(events: Ingress[]): Ingress[] {
  for (const event of events) assert.ok(known.has(event.kind), `${event.kind} is in test/contract.json's machine`);
  return events;
}

function session(action: "created" | "prompted", extra: object = {}): AgentSessionEventWebhookPayload {
  return {
    type: "AgentSessionEvent",
    action,
    appUserId: APP,
    agentSession: { id: SESSION, creatorId: OWNER, issueId: ISSUE, issue: { id: ISSUE, identifier: "TW-1" }, createdAt: AT },
    ...extra,
  } as unknown as AgentSessionEventWebhookPayload;
}

test("the owner's start, prompt and stop become session events with their clocks", () => {
  const started = session("created");
  assert.deepEqual(fromSession(started, decide(started, OWNER)!, DELIVERY), { deliveryId: DELIVERY, kind: "sessionStarted", sessionId: SESSION, issueId: ISSUE, trust: "owner", sessionKind: "delegation", commentId: null, at: AT });
  const later = "2026-10-01T10:05:00.000Z";
  const prompt = session("prompted", { agentActivity: { id: "a1", userId: OWNER, createdAt: later } });
  assert.equal(fromSession(prompt, decide(prompt, OWNER)!, DELIVERY)?.kind, "ownerPrompted");
  const stop = session("prompted", { agentActivity: { id: "a2", userId: OWNER, createdAt: later, signal: "stop" } });
  const event = fromSession(stop, decide(stop, OWNER)!, DELIVERY);
  assert.deepEqual([event?.kind, event?.at], ["ownerStopped", later]);
  pinned([event!]);
});

test("a mention's session names the owner's comment that mentioned Tolkster", () => {
  const C = "11111111-1111-4111-8111-111111111111";
  const R = "22222222-2222-4222-8222-222222222222";
  const commentOf = (agentSession: object) => {
    const started = session("created");
    Object.assign(started.agentSession, agentSession);
    return fromSession(started, decide(started, OWNER)!, DELIVERY)?.commentId;
  };
  assert.equal(commentOf({ comment: { id: C, userId: OWNER } }), C);
  // A reply in another thread: the thread's root is no mention, its source comment is.
  assert.equal(commentOf({ comment: { id: R }, sourceCommentId: C }), C);
  // A delegation's thread is Linear's own, with no author.
  assert.equal(commentOf({ comment: { id: R } }), null);
  assert.equal(commentOf({ comment: { id: "not-an-id", userId: OWNER } }), null);
  // The owner's reply in Tolkster's own thread names Tolkster's comment, which is no mention.
  assert.equal(commentOf({ creatorId: undefined, comment: { id: C, userId: APP } }), null);
});

test("a session no one trusted, or a prompt by anyone else, makes no event", () => {
  const stranger = session("created");
  (stranger.agentSession as { creatorId: string }).creatorId = "someone";
  assert.equal(fromSession(stranger, decide(stranger, OWNER)!, DELIVERY), null);
  const prompt = session("prompted", { agentActivity: { id: "a1", userId: "someone", createdAt: AT } });
  assert.equal(fromSession(prompt, decide(prompt, OWNER)!, DELIVERY), null);
});

function issueUpdate(changed: object, data: object = {}, actor: string | null = OWNER) {
  return { action: "update", actor: actor === null ? null : { id: actor }, data: { id: ISSUE, updatedAt: AT, state: { type: "started" }, delegateId: APP, priority: 2, ...data }, updatedFrom: changed };
}

test("a delegation to Tolkster counts by the owner or Tolkster's chain, and one taken away by anyone", () => {
  const [byOwner] = pinned(fromIssue(issueUpdate({ delegateId: null }), DELIVERY, OWNER, APP));
  assert.deepEqual(byOwner, { deliveryId: DELIVERY, issueId: ISSUE, at: AT, issue: { state: "started", priority: 2, parentId: null }, kind: "delegateChanged", delegated: true, by: "owner" });
  const [byChain] = fromIssue(issueUpdate({ delegateId: null }, {}, APP), DELIVERY, OWNER, APP);
  assert.equal(byChain?.kind === "delegateChanged" && byChain.by, "chain");
  assert.deepEqual(fromIssue(issueUpdate({ delegateId: null }, {}, "someone"), DELIVERY, OWNER, APP), []);
  const away = fromIssue(issueUpdate({ delegateId: APP }, { delegateId: "someone" }, "someone"), DELIVERY, OWNER, APP);
  assert.deepEqual(away.map((e) => e.kind === "delegateChanged" && [e.delegated, e.by]), [[false, null]]);
  // Without the app's user ID, no delegate change can be told apart.
  assert.deepEqual(fromIssue(issueUpdate({ delegateId: null }), DELIVERY, OWNER, null), []);
  // A delegate change between two others isn't Tolkster's.
  assert.deepEqual(fromIssue(issueUpdate({ delegateId: "a" }, { delegateId: "b" }), DELIVERY, OWNER, APP), []);
});

test("a change of state is an event, beside a delegate change in the same update", () => {
  const events = fromIssue(issueUpdate({ stateId: "s0", delegateId: null }, { state: { type: "completed" } }), DELIVERY, OWNER, APP);
  assert.deepEqual(events.map((e) => e.kind), ["delegateChanged", "issueStateChanged"]);
  assert.deepEqual(fromIssue({ ...issueUpdate({ stateId: "s0" }, { delegateId: null }), action: "create" }, DELIVERY, OWNER, APP), []);
  assert.deepEqual(fromIssue(issueUpdate({ title: "x" }), DELIVERY, OWNER, APP), []);
});

test("a change of priority is an event of its own, with the priority it now has", () => {
  assert.deepEqual(pinned(fromIssue(issueUpdate({ priority: 3 }, { priority: 1 }), DELIVERY, OWNER, APP)), [{ deliveryId: DELIVERY, issueId: ISSUE, at: AT, issue: { state: "started", priority: 1, parentId: null }, kind: "priorityChanged" }]);
  assert.deepEqual(fromIssue(issueUpdate({ priority: 3 }, { priority: "high" }), DELIVERY, OWNER, APP), []);
  assert.deepEqual(fromIssue(issueUpdate({ priority: 3 }, { priority: 5 }), DELIVERY, OWNER, APP), []);
});

test("an issue created delegated to Tolkster is a delegation, by the owner or Tolkster's chain", () => {
  const created = (actor: string | null, data: object = {}) => fromIssue({ ...issueUpdate({}, data, actor), action: "create", updatedFrom: undefined }, DELIVERY, OWNER, APP);
  assert.deepEqual(pinned(created(OWNER)), [{ deliveryId: DELIVERY, issueId: ISSUE, at: AT, issue: { state: "started", priority: 2, parentId: null }, kind: "delegateChanged", delegated: true, by: "owner" }]);
  assert.deepEqual(created(APP).map((e) => e.kind === "delegateChanged" && e.by), ["chain"]);
  assert.deepEqual(created("someone"), []);
  assert.deepEqual(created(OWNER, { delegateId: "someone" }), []);
  assert.deepEqual(fromIssue({ ...issueUpdate({}), action: "create", updatedFrom: undefined }, DELIVERY, OWNER, null), []);
});

test("a sub-issue created or moved under a parent is a change of state, naming the parent it left", () => {
  const PARENT = "0a1b2c3d-0000-4000-8000-000000000002";
  const OLD = "0a1b2c3d-0000-4000-8000-000000000003";
  const issue = { state: "started", priority: 2, parentId: PARENT };
  const created = pinned(fromIssue({ ...issueUpdate({}, { parentId: PARENT, delegateId: null }), action: "create", updatedFrom: undefined }, DELIVERY, OWNER, APP));
  assert.deepEqual(created, [{ deliveryId: DELIVERY, issueId: ISSUE, at: AT, issue, kind: "issueStateChanged", from: null }]);
  assert.deepEqual(fromIssue(issueUpdate({ parentId: OLD }, { parentId: PARENT }), DELIVERY, OWNER, APP), [{ deliveryId: DELIVERY, issueId: ISSUE, at: AT, issue, kind: "issueStateChanged", from: OLD }]);
  assert.deepEqual(fromIssue(issueUpdate({ parentId: null }, { parentId: PARENT }), DELIVERY, OWNER, APP).map((e) => e.kind === "issueStateChanged" && e.from), [null]);
  // Moved off, it names the parent it left, and none now.
  const away = fromIssue(issueUpdate({ parentId: OLD }, { parentId: null }), DELIVERY, OWNER, APP);
  assert.deepEqual(away.map((e) => e.kind === "issueStateChanged" && [e.issue.parentId, e.from]), [[null, OLD]]);
  // A create with no parent, not delegated to Tolkster, is nothing.
  assert.deepEqual(fromIssue({ ...issueUpdate({}, { delegateId: null }), action: "create", updatedFrom: undefined }, DELIVERY, OWNER, APP), []);
});

test("a PR's webhooks become its events, each with its own clock", () => {
  const pr = { number: 7, head, updated_at: "2026-10-01T10:00:01Z", merged_at: "2026-10-01T10:00:02Z", closed_at: "2026-10-01T10:00:03Z" };
  const kinds = (event: string, payload: object) => pinned(fromGitHub(event, { repository, pull_request: pr, ...payload }, DELIVERY)).map((e) => [e.kind, "at" in e ? e.at : null]);
  assert.deepEqual(kinds("pull_request", { action: "opened" }), [["prOpened", "2026-10-01T10:00:01.000Z"]]);
  assert.deepEqual(kinds("pull_request", { action: "reopened" }), [["prOpened", "2026-10-01T10:00:01.000Z"]]);
  assert.deepEqual(kinds("pull_request", { action: "synchronize", before: "c".repeat(40) }), [["prPushed", "2026-10-01T10:00:01.000Z"]]);
  // A push names the head it replaced, or it can't be ordered.
  assert.deepEqual(pinned(fromGitHub("pull_request", { repository, pull_request: pr, action: "synchronize", before: "c".repeat(40) }, DELIVERY)).map((e) => e.kind === "prPushed" && e.before), ["c".repeat(40)]);
  assert.deepEqual(kinds("pull_request", { action: "synchronize" }), []);
  assert.deepEqual(kinds("pull_request", { action: "synchronize", before: "not a sha" }), []);
  assert.deepEqual(kinds("pull_request", { action: "auto_merge_disabled" }), [["autoMergeDisabled", "2026-10-01T10:00:01.000Z"]]);
  assert.deepEqual(kinds("pull_request", { action: "closed", pull_request: { ...pr, merged: true } }), [["prMerged", "2026-10-01T10:00:02.000Z"]]);
  assert.deepEqual(kinds("pull_request", { action: "closed", pull_request: { ...pr, merged: false } }), [["prClosed", "2026-10-01T10:00:03.000Z"]]);
  assert.deepEqual(kinds("pull_request", { action: "edited" }), []);
  const review = fromGitHub("pull_request_review", { repository, action: "submitted", pull_request: pr, review: { commit_id: "b".repeat(40), submitted_at: "2026-10-01T10:00:09Z", user: { id: 42, login: "Copilot" } } }, DELIVERY);
  assert.deepEqual(review, [{ deliveryId: DELIVERY, repo: REPO, number: 7, branch: head.ref, head: HEAD, kind: "prReviewed", sha: "b".repeat(40), reviewerId: 42, copilot: true, at: "2026-10-01T10:00:09.000Z" }]);
  // A conversation's comment names no PR head: the intake stamps it.
  const comment = (issue: object) => fromGitHub("issue_comment", { repository, action: "created", issue, comment: { created_at: "2026-10-01T10:00:09Z", user: { id: 42, login: "x" } } }, DELIVERY);
  assert.deepEqual(pinned(comment({ number: 7, pull_request: {} })), [{ deliveryId: DELIVERY, kind: "prReviewed", conversation: true, repo: REPO, number: 7, reviewerId: 42, copilot: false, at: "2026-10-01T10:00:09.000Z" }]);
  assert.deepEqual(comment({ number: 7 }), []);
});

test("a fork's PR makes no event", () => {
  const fork = { number: 7, head: { ...head, repo: { full_name: "someone/tolkster" } }, updated_at: "2026-10-01T10:00:01Z" };
  assert.deepEqual(fromGitHub("pull_request", { repository, action: "opened", pull_request: fork }, DELIVERY), []);
  // GitHub lists no PRs for a fork's suite, whatever its branch is called.
  const suite = { id: 11, conclusion: "failure", head_sha: HEAD, updated_at: "2026-10-01T10:00:01Z", pull_requests: [] };
  assert.deepEqual(fromGitHub("check_suite", { repository, action: "completed", check_suite: suite }, DELIVERY), []);
});

test("checks complete from a suite or a status, for the branches at the commit", () => {
  const suite = (conclusion: string) => fromGitHub("check_suite", { repository, action: "completed", check_suite: { id: 11, conclusion, head_sha: HEAD, updated_at: "2026-10-01T10:00:01Z", pull_requests: [{ number: 7, head: { ref: head.ref, sha: HEAD } }] } }, DELIVERY);
  assert.deepEqual(pinned(suite("success")), [{ deliveryId: DELIVERY, kind: "checksCompleted", repo: REPO, branches: [head.ref], sha: HEAD, source: "suite:11", result: "passed", at: "2026-10-01T10:00:01.000Z" }]);
  assert.equal((suite("failure")[0] as { result: string }).result, "failed");
  assert.equal((suite("startup_failure")[0] as { result: string }).result, "couldntRun");
  assert.deepEqual(suite("stale"), []);
  const status = (state: string) => fromGitHub("status", { repository, sha: HEAD, state, context: "ci/build", updated_at: "2026-10-01T10:00:01Z" }, DELIVERY);
  // A status names no PR: the intake maps it by its SHA.
  assert.deepEqual(status("error").map((e) => e.kind === "checksCompleted" && [e.result, e.branches]), [["failed", null]]);
  assert.deepEqual(status("pending"), []);
});

test("a push to the default branch is the repo's, and any other push is nothing", () => {
  const push = (ref: string, deleted = false) => fromGitHub("push", { repository, ref, after: HEAD, deleted }, DELIVERY);
  assert.deepEqual(pinned(push("refs/heads/main")), [{ deliveryId: DELIVERY, kind: "repoPushed", repo: REPO, sha: HEAD }]);
  assert.deepEqual(push("refs/heads/feature"), []);
  assert.deepEqual(push("refs/heads/main", true), []);
});

test("an event needs a delivery ID of a UUID's shape, as it is deduped by it", () => {
  const started = session("created");
  assert.equal(fromSession(started, decide(started, OWNER)!, ""), null);
  assert.deepEqual(fromIssue(issueUpdate({ stateId: "s0" }), "x".repeat(4000), OWNER, APP), []);
  assert.deepEqual(fromGitHub("push", { repository, ref: "refs/heads/main", after: HEAD }, ""), []);
});

test("a session says why it was trusted, and whether a mention's", () => {
  const mention = session("created", { agentSession: { id: SESSION, creatorId: OWNER, issueId: ISSUE, createdAt: AT, comment: { userId: OWNER } } });
  const event = fromSession(mention, decide(mention, OWNER)!, DELIVERY);
  assert.deepEqual([event?.trust, event?.sessionKind], ["owner", "mention"]);
  const chain = session("created", { agentSession: { id: SESSION, creatorId: APP, issueId: ISSUE, createdAt: AT } });
  assert.equal(fromSession(chain, decide(chain, OWNER, Math.random, true)!, DELIVERY)?.trust, "chain");
  // A chain's session from no one, without the app's user ID, is never taken for a reply.
  const anonymous = session("created", { appUserId: undefined, agentSession: { id: SESSION, issueId: ISSUE, createdAt: AT, comment: {} } });
  assert.equal(fromSession(anonymous, decide(anonymous, OWNER, Math.random, true)!, DELIVERY)?.trust, "chain");
});

test("a review thread counts as its sender's, never a bot's", () => {
  const pr = { number: 7, head, updated_at: "2026-10-01T10:00:01Z" };
  const thread = (type: string) => fromGitHub("pull_request_review_thread", { repository, action: "resolved", pull_request: pr, sender: { id: 9, login: "x", type } }, DELIVERY);
  assert.deepEqual(thread("User").map((e) => "reviewerId" in e && "sha" in e && [e.sha, e.reviewerId, e.copilot]), [[HEAD, 9, false]]);
  assert.deepEqual(thread("Bot"), []);
});

test("a repo is kept in lower case, as GitHub matches it", () => {
  const upper = { full_name: "Tolkien-Works/Tolkster", default_branch: "main" };
  const pr = { number: 7, head: { ...head, repo: { full_name: "tolkien-works/tolkster" } }, updated_at: "2026-10-01T10:00:01Z" };
  assert.deepEqual(fromGitHub("pull_request", { repository: upper, action: "opened", pull_request: pr }, DELIVERY).map((e) => "repo" in e && e.repo), ["tolkien-works/tolkster"]);
});

test("the owner's own comment on an issue is an event, and anyone else's, or an integration's, isn't", () => {
  const comment = (data: object) => fromLinearComment({ action: "create", data: { id: "c1", issueId: ISSUE, userId: OWNER, createdAt: AT, ...data } }, DELIVERY, OWNER);
  assert.deepEqual(pinned([comment({})!]), [{ deliveryId: DELIVERY, kind: "ownerCommented", issueId: ISSUE, commentId: "c1", at: AT }]);
  assert.equal(comment({ userId: "someone" }), null);
  assert.equal(comment({ botActor: { id: "bot" } }), null);
  assert.equal(fromLinearComment({ action: "create", data: { id: "c1", issueId: ISSUE, userId: OWNER, createdAt: AT } }, "", OWNER), null);
});
