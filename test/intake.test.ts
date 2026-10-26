import assert from "node:assert/strict";
import { test } from "node:test";
import type { Ingress, IssueEvent, PrEvent, RepoPushed, SessionEvent } from "../src/worker/events.ts";
import { type Chained, type Holds, intake as intakeBare, KEEP_MS, record, branchWrites, recordBranch, received, rememberIdentifier, SEEN_KEEP_MS, sessionIssue, sessionWrites, threadIssue, threadWrites } from "../src/worker/intake.ts";
import { holds, loadStore, openPrs, settle, staged } from "../src/worker/machine/store.ts";
import { toEvent } from "../src/worker/route.ts";
import type { Storage } from "../src/worker/storage.ts";
import { MemoryStorage } from "./storage.ts";

// What the intake takes: every event but a push to a default branch.
type In = Exclude<Ingress, RepoPushed>;

// The intake as the relay runs it: each accepted event staged into its issue's store in the
// intake's own put, and settled after.
async function intake(storage: Storage, event: In, nowMs: number, chained: Chained = async () => false, held: Holds = (issue) => holds(storage, issue)) {
  const staging: Awaited<ReturnType<typeof staged>>[] = [];
  const results = await intakeBare(storage, event, nowMs, held, (repo) => openPrs(storage, repo, nowMs), async (accepted) => {
    const step = await staged(storage, toEvent(accepted), nowMs);
    staging.push(step);
    return step.writes;
  }, chained);
  for (const step of staging) await settle(storage, step.issue, step.send, nowMs, () => {});
  return results;
}

const NOW = Date.parse("2026-10-01T12:00:00.000Z");
const DAY_MS = 24 * 60 * 60 * 1000;
const ISSUE = "0a1b2c3d-0000-4000-8000-000000000001";
const OTHER = "0a1b2c3d-0000-4000-8000-000000000002";
const FOURTH = "0a1b2c3d-0000-4000-8000-000000000004";
const SESSION = "0a1b2c3d-0000-4000-8000-0000000000aa";
const CHAIN_SESSION = "0a1b2c3d-0000-4000-8000-0000000000ab";
const REPO = "tolkien-works/tolkster";
const BRANCH = "fix-it-tw-1";
const HEAD = "a".repeat(40);
const NEXT = "b".repeat(40);
let deliveries = 0;
const delivery = () => `00000000-0000-4000-8000-${String(++deliveries).padStart(12, "0")}`;

const delegated = (at: string, deliveryId = delivery()): Extract<IssueEvent, { kind: "delegateChanged" }> => ({ deliveryId, kind: "delegateChanged", issueId: ISSUE, at, issue: { state: "started", priority: 2, parentId: null }, delegated: true, by: "owner" });
const stateChanged = (state: string, at: string): IssueEvent => ({ deliveryId: delivery(), kind: "issueStateChanged", issueId: ISSUE, at, issue: { state, priority: 2, parentId: null }, from: null });
const pr = (kind: Exclude<PrEvent["kind"], "prPushed">, at: string, number = 7, branch = BRANCH): PrEvent => ({ deliveryId: delivery(), kind, repo: REPO, number, branch, head: HEAD, at });
const pushed = (at: string, head = NEXT, before = HEAD): PrEvent => ({ deliveryId: delivery(), kind: "prPushed", repo: REPO, number: 7, branch: BRANCH, head, before, at });
const started = (at: string): SessionEvent => ({ deliveryId: delivery(), kind: "sessionStarted", sessionId: SESSION, issueId: ISSUE, trust: "owner", sessionKind: "delegation", commentId: null, at });
const commented = (issueId: string, at = "2026-10-01T10:00:00.000Z"): In => ({ deliveryId: delivery(), kind: "ownerCommented", issueId, commentId: "0a1b2c3d-0000-4000-8000-0000000000cc", at });

async function run(storage: MemoryStorage, events: In[], chained?: Chained) {
  const outcomes = [];
  for (const event of events) outcomes.push(...(await intake(storage, event, NOW, chained)).map((r) => `${r.event.kind}:${r.outcome}`));
  return outcomes;
}

// An issue whose worker's PR is open on its branch, with the store holding it.
async function opened(storage: MemoryStorage, issueId = ISSUE, branch = BRANCH, number = 7) {
  await recordBranch(storage, issueId, REPO, branch, NOW);
  await run(storage, [pr("prOpened", "2026-10-01T10:00:00.000Z", number, branch)]);
}

test("a redelivery is a replay, and reaches no machine", async () => {
  const storage = new MemoryStorage();
  const linear = delegated("2026-10-01T10:00:00.000Z");
  assert.deepEqual(await run(storage, [linear, linear]), ["delegateChanged:accepted", "delegateChanged:replay"]);
  const store = await loadStore(storage, ISSUE);
  // Past what each source redelivers, a day's deliveries are forgotten once the next event
  // has pruned them, and it reaches the store again, which finds nothing newer.
  const later = NOW + SEEN_KEEP_MS + DAY_MS;
  await intake(storage, commented(OTHER), later);
  assert.deepEqual((await intake(storage, linear, later)).map((r) => r.outcome), ["accepted"]);
  assert.deepEqual((await loadStore(storage, ISSUE)).facts.delegate, store.facts.delegate);
});

test("the intake holds none of the issue's facts: an older event reaches the store, which judges it", async () => {
  const storage = new MemoryStorage();
  await run(storage, [stateChanged("completed", "2026-10-01T10:02:00.000Z")]);
  assert.deepEqual(await run(storage, [stateChanged("started", "2026-10-01T10:01:00.000Z")]), ["issueStateChanged:accepted"]);
  assert.deepEqual((await loadStore(storage, ISSUE)).facts.issueState?.value, "completed");
  assert.deepEqual(Object.keys(await record(storage, ISSUE)), ["expires"]);
});

test("a PR maps only through a worker's branch in its repo", async () => {
  const storage = new MemoryStorage();
  assert.deepEqual(await intake(storage, pr("prOpened", "2026-10-01T10:00:00.000Z"), NOW), []);
  await recordBranch(storage, ISSUE, "someone/tolkster", BRANCH, NOW);
  assert.deepEqual(await intake(storage, pr("prOpened", "2026-10-01T10:00:00.000Z"), NOW), []);
  await recordBranch(storage, ISSUE, REPO, BRANCH, NOW);
  assert.deepEqual((await intake(storage, pr("prOpened", "2026-10-01T10:00:00.000Z"), NOW)).map((r) => [r.event.issueId, r.outcome]), [[ISSUE, "accepted"]]);
  assert.equal((await loadStore(storage, ISSUE)).facts.pr?.number, 7);
});

test("a check suite reaches each issue whose worker's branch it names, once", async () => {
  const storage = new MemoryStorage();
  await recordBranch(storage, ISSUE, REPO, BRANCH, NOW);
  await recordBranch(storage, OTHER, REPO, "other-tw-2", NOW);
  const checks: In = { deliveryId: delivery(), kind: "checksCompleted", repo: REPO, branches: [BRANCH, "main", "other-tw-2"], sha: HEAD, source: "suite:1", result: "failed", at: "2026-10-01T10:00:00.000Z" };
  assert.deepEqual((await intake(storage, checks, NOW)).map((r) => [r.event.issueId, r.outcome]).sort(), [[ISSUE, "accepted"], [OTHER, "accepted"]]);
  assert.equal((await loadStore(storage, ISSUE)).facts.checks[HEAD]?.value, "failed");
});

test("a review reaches the issue whose worker's branch is its PR's head", async () => {
  const storage = new MemoryStorage();
  await opened(storage);
  const review: In = { deliveryId: delivery(), kind: "prReviewed", repo: REPO, number: 7, branch: BRANCH, head: HEAD, sha: HEAD, reviewerId: 5, copilot: true, at: "2026-10-01T10:00:05.000Z" };
  assert.deepEqual((await intake(storage, review, NOW)).map((r) => [r.event.issueId, r.outcome]), [[ISSUE, "accepted"]]);
  assert.deepEqual((await loadStore(storage, ISSUE)).facts.feedback[HEAD], { at: "2026-10-01T10:00:05.000Z", copilot: true });
});

test("a commit status reaches the issue whose open PR has its SHA as head, as its store holds it", async () => {
  const storage = new MemoryStorage();
  const status = (sha: string): In => ({ deliveryId: delivery(), kind: "checksCompleted", repo: REPO, branches: null, sha, source: "status:ab12cd34", result: "failed", at: "2026-10-01T10:00:05.000Z" });
  assert.deepEqual(await intake(storage, status(HEAD), NOW), []);
  await opened(storage);
  await opened(storage, OTHER, "other-tw-2", 8);
  assert.deepEqual((await intake(storage, status(NEXT), NOW)).length, 0);
  // Both PRs share a head here; each issue hears of it.
  assert.deepEqual((await intake(storage, status(HEAD), NOW)).map((r) => r.event.issueId).sort(), [ISSUE, OTHER]);
  await run(storage, [pushed("2026-10-01T10:00:01.000Z")]);
  assert.deepEqual((await intake(storage, status(NEXT), NOW)).map((r) => [r.event.issueId, r.outcome]), [[ISSUE, "accepted"]]);
  // Once its PR closes, it is out of the coordinator's index, and hears no more.
  await run(storage, [pr("prClosed", "2026-10-01T10:00:02.000Z")]);
  assert.deepEqual(await intake(storage, status(NEXT), NOW), []);
});

test("a PR whose open the coordinator's index hasn't heard of yet still hears its status, while its store is due", async () => {
  const storage = new MemoryStorage();
  await recordBranch(storage, ISSUE, REPO, BRANCH, NOW);
  // The open lands in the store's put, and its delivery to the coordinator fails after.
  await intakeBare(storage, pr("prOpened", "2026-10-01T10:00:00.000Z"), NOW, (issue) => holds(storage, issue), (repo) => openPrs(storage, repo, NOW), async (accepted) => (await staged(storage, toEvent(accepted), NOW)).writes, async () => false);
  assert.deepEqual((await storage.get<{ prs: Record<string, string[]> }>("machine:coordinator"))?.prs ?? {}, {});
  const status: In = { deliveryId: delivery(), kind: "checksCompleted", repo: REPO, branches: null, sha: HEAD, source: "status:ab12cd34", result: "failed", at: "2026-10-01T10:00:05.000Z" };
  assert.deepEqual((await intake(storage, status, NOW + 1000)).map((r) => [r.event.issueId, r.outcome]), [[ISSUE, "accepted"]]);
});

test("a comment on a PR's conversation is feedback for the head its store holds when it arrives", async () => {
  const storage = new MemoryStorage();
  const comment = (number: number): In => ({ deliveryId: delivery(), kind: "prReviewed", conversation: true, repo: REPO, number, reviewerId: 5, copilot: false, at: "2026-10-01T10:00:05.000Z" });
  assert.deepEqual(await intake(storage, comment(7), NOW), []);
  await opened(storage);
  await run(storage, [pushed("2026-10-01T10:00:01.000Z")]);
  assert.deepEqual(await intake(storage, comment(8), NOW), []);
  const [taken] = await intake(storage, comment(7), NOW);
  assert.deepEqual([taken.event.issueId, taken.outcome, taken.event.kind === "prReviewed" && taken.event.sha], [ISSUE, "accepted", NEXT]);
  assert.deepEqual((await loadStore(storage, ISSUE)).facts.feedback[NEXT], { at: "2026-10-01T10:00:05.000Z", copilot: false });
});

test("the owner's comment reaches its issue, once", async () => {
  const storage = new MemoryStorage();
  const comment = commented(ISSUE);
  assert.deepEqual(await run(storage, [comment, comment]), ["ownerCommented:accepted", "ownerCommented:replay"]);
});

test("Tolkster's own chain's events count only on a chain that holds, and map no session otherwise", async () => {
  const storage = new MemoryStorage();
  const chain: IssueEvent = { ...delegated("2026-10-01T10:00:00.000Z"), by: "chain" };
  const session: SessionEvent = { ...started("2026-10-01T10:00:00.000Z"), trust: "chain" };
  assert.deepEqual(await run(storage, [chain, session]), ["delegateChanged:refused", "sessionStarted:refused"]);
  assert.equal(await sessionIssue(storage, SESSION), null);
  assert.equal((await loadStore(storage, ISSUE)).facts.delegate, null);
  // The issue's store says whether a chain holds, asked only for the chain's own event.
  const asked: string[] = [];
  assert.deepEqual(await run(storage, [{ ...session, deliveryId: delivery() }], async (issue) => (asked.push(issue), true)), ["sessionStarted:accepted"]);
  assert.deepEqual(asked, [ISSUE]);
  assert.equal(await sessionIssue(storage, SESSION), ISSUE);
  assert.deepEqual(await run(storage, [delegated("2026-10-01T10:01:00.000Z")], async () => assert.fail("asked for the owner's")), ["delegateChanged:accepted"]);
});

test("what the caller adds is written with the event's own writes, and not for a replay", async () => {
  const storage = new MemoryStorage();
  const also = async (event: { kind: string }) => ({ ["machine:" + ISSUE]: event.kind });
  const event = stateChanged("completed", "2026-10-01T10:00:00.000Z");
  const bare = () => intakeBare(storage, event, NOW, async () => false, async () => [], also, async () => false);
  await bare();
  assert.equal(await storage.get("machine:" + ISSUE), "issueStateChanged");
  await storage.put({ ["machine:" + ISSUE]: "untouched" });
  await bare();
  assert.equal(await storage.get("machine:" + ISSUE), "untouched");
});

test("a worker's new branch unmaps its old one, and a branch another issue takes leaves the first", async () => {
  const storage = new MemoryStorage();
  await recordBranch(storage, ISSUE, REPO, "old-tw-1", NOW);
  await recordBranch(storage, ISSUE, REPO, BRANCH, NOW);
  assert.deepEqual(await intake(storage, pr("prClosed", "2026-10-01T10:02:00.000Z", 7, "old-tw-1"), NOW), []);
  await run(storage, [pr("prOpened", "2026-10-01T10:00:00.000Z")]);
  await recordBranch(storage, OTHER, REPO, BRANCH, NOW);
  assert.equal((await record(storage, ISSUE)).branch, undefined);
  // The first's store still holds its PR open, but a status on its head reaches it no more.
  const status: In = { deliveryId: delivery(), kind: "checksCompleted", repo: REPO, branches: null, sha: HEAD, source: "status:ab12cd34", result: "failed", at: "2026-10-01T10:00:05.000Z" };
  assert.deepEqual(await intake(storage, status, NOW), []);
  assert.deepEqual((await intake(storage, pr("prOpened", "2026-10-01T10:01:00.000Z", 8), NOW)).map((r) => r.event.issueId), [OTHER]);
});

test("a prune lists only the days of deliveries past keeping, and a delivery is kept 7 days at least", async () => {
  const storage = new MemoryStorage();
  const linear = delegated("2026-10-01T10:00:00.000Z");
  await intake(storage, linear, NOW);
  await intake(storage, stateChanged("started", "2026-10-01T11:00:00.000Z"), NOW + SEEN_KEEP_MS);
  assert.deepEqual((await intake(storage, linear, NOW + SEEN_KEEP_MS)).map((r) => r.outcome), ["replay"]);
  storage.listed = [];
  await intake(storage, stateChanged("completed", "2026-10-01T11:01:00.000Z"), NOW + SEEN_KEEP_MS + DAY_MS);
  // The first day's two deliveries, and nothing kept.
  assert.deepEqual(storage.listed.filter((k) => k.startsWith("intake:seen:")).map((k) => k.slice(12, 22)), ["2026-10-01"]);
  assert.equal([...storage.data.keys()].filter((k) => k.startsWith("intake:seen:2026-10-01")).length, 0);
});

const THREAD_ROOT = "11111111-1111-4111-8111-111111111111";
const records = (storage: MemoryStorage, issueId: string) => [...storage.data].filter(([k, v]) => !k.startsWith("intake:seen:") && k.startsWith("intake:") && (k.endsWith(issueId) || v === issueId)).map(([k]) => k.replace(/:\d{4}-\d\d-\d\d:/, ":<day>:"));

test("an issue's maps are retired together once its store holds no machine, and looked at again while it does", async () => {
  const storage = new MemoryStorage();
  await run(storage, [started("2026-10-01T09:00:00.000Z")]);
  await rememberIdentifier(storage, ISSUE, "TW-1", NOW);
  const writes: Record<string, unknown> = {};
  Object.assign(writes, await branchWrites(storage, ISSUE, REPO, BRANCH, NOW));
  await threadWrites(storage, ISSUE, THREAD_ROOT, NOW, writes);
  await sessionWrites(storage, ISSUE, CHAIN_SESSION, NOW, writes);
  await storage.put(writes);
  // A start's, a note's and a rebuilt chain session's writes in one put keep each of the record's maps.
  assert.deepEqual([(await record(storage, ISSUE)).branch?.branch, Object.keys((await record(storage, ISSUE)).threads ?? {}), Object.keys((await record(storage, ISSUE)).sessions ?? {}).sort()], [BRANCH, [THREAD_ROOT], [SESSION, CHAIN_SESSION].sort()]);
  assert.equal(await threadIssue(storage, THREAD_ROOT), ISSUE);
  assert.equal(await sessionIssue(storage, CHAIN_SESSION), ISSUE);
  assert.deepEqual(records(storage, ISSUE).sort(), ["intake:branch:" + REPO + ":" + BRANCH, "intake:expiry:<day>:" + ISSUE, "intake:facts:" + ISSUE, "intake:session:" + SESSION, "intake:session:" + CHAIN_SESSION].sort());
  // Its request machine holds it, so its maps outlive their keeping, armed again.
  const later = NOW + KEEP_MS + DAY_MS;
  await intake(storage, commented(FOURTH), later);
  assert.equal(await sessionIssue(storage, SESSION), ISSUE);
  assert.equal((await record(storage, ISSUE)).expires, new Date(later + KEEP_MS).toISOString().slice(0, 10));
  // Once it holds none, they go together at the next expiry.
  await intake(storage, commented(FOURTH), later + KEEP_MS + DAY_MS, undefined, async () => false);
  assert.deepEqual(records(storage, ISSUE), []);
  assert.deepEqual([...storage.data.keys()].filter((k) => k.startsWith("identifier:") || k.startsWith("thread:")), []);
  assert.deepEqual(await intake(storage, pr("prClosed", "2026-10-20T00:00:00.000Z"), later + KEEP_MS + DAY_MS), []);
});

test("an issue's maps stay while its store holds a PR open, though it holds no machine", async () => {
  const storage = new MemoryStorage();
  await opened(storage);
  assert.equal((await loadStore(storage, ISSUE)).work, null);
  await intake(storage, commented(FOURTH), NOW + KEEP_MS + DAY_MS);
  assert.equal((await record(storage, ISSUE)).branch?.branch, BRANCH);
  assert.deepEqual((await intake(storage, pr("prClosed", "2026-10-20T00:00:00.000Z"), NOW + KEEP_MS + DAY_MS)).map((r) => r.event.issueId), [ISSUE]);
});

test("each session event writes its session's map again, so one lost comes back", async () => {
  const storage = new MemoryStorage();
  await run(storage, [started("2026-10-01T09:00:00.000Z")]);
  await storage.delete(["intake:session:" + SESSION]);
  await run(storage, [{ ...started("2026-10-01T09:05:00.000Z"), kind: "ownerPrompted" }]);
  assert.equal(await sessionIssue(storage, SESSION), ISSUE);
});

test("an event since pushes the expiry on, and the superseded key goes alone", async () => {
  const storage = new MemoryStorage();
  await recordBranch(storage, ISSUE, REPO, BRANCH, NOW);
  await intake(storage, commented(ISSUE), NOW + 10 * DAY_MS);
  const expiries = () => records(storage, ISSUE).filter((k) => k.startsWith("intake:expiry:")).length;
  assert.equal(expiries(), 2);
  await intake(storage, commented(OTHER), NOW + KEEP_MS + DAY_MS, undefined, async () => false);
  assert.equal((await record(storage, ISSUE)).branch?.branch, BRANCH);
  assert.equal(expiries(), 1);
  await intake(storage, commented(OTHER), NOW + 10 * DAY_MS + KEEP_MS + DAY_MS, undefined, async () => false);
  assert.deepEqual(records(storage, ISSUE).filter((k) => !k.startsWith("intake:expiry:")), []);
});

test("an issue whose retiring fails is tried again, and holds back no other", async () => {
  const storage = new MemoryStorage();
  await recordBranch(storage, ISSUE, REPO, BRANCH, NOW);
  await recordBranch(storage, OTHER, REPO, "other-tw-2", NOW);
  const later = NOW + KEEP_MS + DAY_MS;
  const failing: Holds = async (id) => {
    if (id === ISSUE) throw new Error("unreadable");
    return false;
  };
  await intake(storage, commented(FOURTH), later, undefined, failing);
  assert.deepEqual([records(storage, ISSUE).length > 0, records(storage, OTHER)], [true, []]);
  // The next prune, an hour on.
  await intake(storage, commented(FOURTH), later + 60 * 60 * 1000, undefined, async () => false);
  assert.deepEqual(records(storage, ISSUE), []);
});


test("a session event's event_received line names its session, as tolkster logs --session finds it", () => {
  const session = started("2026-10-01T10:00:00.000Z");
  assert.deepEqual(received({ event: session, outcome: "accepted" }), { kind: "sessionStarted", deliveryId: session.deliveryId, issueId: ISSUE, sessionId: SESSION, outcome: "accepted" });
  const state = stateChanged("Todo", "2026-10-01T10:00:00.000Z");
  assert.deepEqual(received({ event: state, outcome: "replay" }), { kind: "issueStateChanged", deliveryId: state.deliveryId, issueId: ISSUE, outcome: "replay" });
});
