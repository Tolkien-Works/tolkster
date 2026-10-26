import assert from "node:assert/strict";
import { test } from "node:test";
import { budgetFields, budgetWait, clearDelegateMadeBy, IDENTIFIER, isUsable, RateLimitedError, readBudget, readBlockers, TIME, WEBHOOK_TYPES } from "../src/shared/linear.ts";
import contract from "./contract.json" with { type: "json" };

// The contract with the CLI: TIME accepts what JavaScript, and so Linear, writes for each
// instant in test/contract.json, which cli/src/time.rs writes the same, and refuses each
// near miss, which the CLI never writes.
test("the timestamp format is the one the CLI writes, and none of the near misses", () => {
  for (const { ms, iso } of contract.times) {
    assert.equal(new Date(ms).toISOString(), iso);
    assert.ok(TIME.test(iso), iso);
  }
  for (const time of contract.notTimes) assert.ok(!TIME.test(time), time);
});

// The manifest `tolkster setup` hands Linear subscribes the app to the types in test/contract.json,
// which cli/src/linear.rs holds its own list to.
test("the webhook types are the ones the CLI subscribes the app to", () => {
  assert.deepEqual([...WEBHOOK_TYPES].sort(), contract.linearWebhookTypes);
});

test("a token within a day of expiry is not used", () => {
  const now = 1_790_000_000_000;
  const day = 24 * 60 * 60 * 1000;
  assert.equal(isUsable(null, now), false);
  assert.equal(isUsable({ accessToken: "t", expiresAt: now + day + 1 }, now), true);
  assert.equal(isUsable({ accessToken: "t", expiresAt: now + day }, now), false);
});

test("an identifier is a team's key and a number, and a timestamp has Linear's one format", () => {
  for (const identifier of ["TW-1", "A-123456789", "T2W-7", "ABCDEFGHIJKLMNOP-1"]) assert.ok(IDENTIFIER.test(identifier), identifier);
  for (const identifier of ["tw-1", "TW-", "-1", "TW 1", "2W-1", "ABCDEFGHIJKLMNOPQ-1", "TW-1234567890", ""]) assert.ok(!IDENTIFIER.test(identifier), identifier);
  assert.ok(TIME.test("2026-09-28T10:52:52.593Z"));
  for (const time of ["2026-09-28T10:52:52Z", "2026-09-28T10:52:52.5Z", "2026-09-28T10:52:52.593+00:00", "2026-09-28", ""]) assert.ok(!TIME.test(time), time);
});

// The hand-back of a stranger's delegation reads the issue's history first, and clears the
// delegate only if the session's creator made the current one.
const STRANGER = "stranger-user-id";
const OWNER = "owner-user-id";
const TOLKSTER = "tolkster-app-id";

function delegation(delegate: string | null, history: { at: string; actor: string | null; to: string | null; from?: string }[], creator: string = STRANGER) {
  return {
    delegate: delegate ? { id: delegate } : null,
    creator: { id: creator },
    history: {
      nodes: history.map((h) => ({ createdAt: h.at, actor: h.actor ? { id: h.actor } : null, fromDelegate: h.from ? { id: h.from } : null, toDelegate: h.to ? { id: h.to } : null })),
    },
  };
}

// Runs `clearDelegateMadeBy` against a Linear that answers the read with `issue`, and
// returns whether it cleared, with the GraphQL operations it sent.
async function handBack(issue: unknown): Promise<{ cleared: boolean; sent: string[] }> {
  const sent: string[] = [];
  const real = globalThis.fetch;
  globalThis.fetch = (async (_url: unknown, init: { body: string }) => {
    const { query } = JSON.parse(init.body) as { query: string };
    sent.push(query.includes("issueUpdate") ? "issueUpdate" : "issue");
    const data = query.includes("issueUpdate") ? { issueUpdate: { success: true } } : { issue };
    return new Response(JSON.stringify({ data }), { status: 200 });
  }) as typeof fetch;
  try {
    return { cleared: await clearDelegateMadeBy("token", "issue-1", STRANGER, TOLKSTER), sent };
  } finally {
    globalThis.fetch = real;
  }
}

test("a stranger's delegation is handed back when the stranger made the issue's current one", async () => {
  const issue = delegation(TOLKSTER, [{ at: "2026-09-29T01:00:00.000Z", actor: STRANGER, to: TOLKSTER }]);
  assert.deepEqual(await handBack(issue), { cleared: true, sent: ["issue", "issueUpdate"] });
});

test("a late stranger's webhook leaves a delegation the owner made after it", async () => {
  const issue = delegation(TOLKSTER, [
    { at: "2026-09-29T01:00:00.000Z", actor: STRANGER, to: TOLKSTER },
    { at: "2026-09-29T01:00:05.000Z", actor: OWNER, to: null },
    { at: "2026-09-29T01:00:09.000Z", actor: OWNER, to: TOLKSTER },
  ]);
  assert.deepEqual(await handBack(issue), { cleared: false, sent: ["issue"] });
  // The order the entries come in makes no difference.
  assert.equal((await handBack(delegation(TOLKSTER, [...issue.history.nodes].reverse().map((n) => ({ at: n.createdAt, actor: n.actor?.id ?? null, to: n.toDelegate?.id ?? null }))))).cleared, false);
});

test("a delegation with no actor, an unread one or none at all is left", async () => {
  assert.equal((await handBack(delegation(TOLKSTER, [{ at: "2026-09-29T01:00:00.000Z", actor: null, to: TOLKSTER }]))).cleared, false);
  assert.equal((await handBack(delegation(TOLKSTER, [{ at: "2026-09-29T01:00:00.000Z", actor: STRANGER, to: "someone-else" }]))).cleared, false);
  // Nor is a delegation to another agent, which the stranger made after Tolkster's.
  assert.equal((await handBack(delegation("other-agent", [{ at: "2026-09-29T01:00:00.000Z", actor: STRANGER, to: "other-agent" }]))).cleared, false);
  assert.deepEqual(await handBack(delegation(null, [{ at: "2026-09-29T01:00:00.000Z", actor: STRANGER, to: TOLKSTER }])), { cleared: false, sent: ["issue"] });
});

test("a delegation no history entry names is its creator's: handed back for a stranger, left for the owner", async () => {
  assert.deepEqual(await handBack(delegation(TOLKSTER, [])), { cleared: true, sent: ["issue", "issueUpdate"] });
  // Entries about other fields are no change of delegate.
  assert.equal((await handBack(delegation(TOLKSTER, [{ at: "2026-09-29T01:00:00.000Z", actor: STRANGER, to: null }]))).cleared, true);
  assert.deepEqual(await handBack(delegation(TOLKSTER, [], OWNER)), { cleared: false, sent: ["issue"] });
});

test("a creator is not taken for who delegated when history shows a change or may hold more", async () => {
  // The delegate was changed away from Tolkster and not set back in what was read.
  assert.equal((await handBack(delegation(TOLKSTER, [{ at: "2026-09-29T01:00:00.000Z", actor: OWNER, to: null, from: "other-agent" }]))).cleared, false);
  // A full page: an older delegation may sit beyond it.
  const full = Array.from({ length: 10 }, (_, i) => ({ at: `2026-09-29T01:00:0${i}.000Z`, actor: OWNER, to: null }));
  assert.equal((await handBack(delegation(TOLKSTER, full))).cleared, false);
});

const headers = (values: Record<string, string>) => new Headers(values);

test("a response's budget is read from Linear's headers, and what it didn't send is left out", () => {
  const budget = readBudget(
    headers({ "X-Complexity": "7", "X-RateLimit-Requests-Remaining": "4990", "X-RateLimit-Complexity-Remaining": "bad", "X-RateLimit-Requests-Reset": "1790000000000" }),
  );
  assert.deepEqual(budgetFields(budget), { complexity: 7, requestsRemaining: 4990, requestsReset: "2026-09-21T14:13:20.000Z" });
  assert.equal(budgetFields(readBudget(headers({}))), undefined);
});

test("the wait is until the spent budget resets, within 5 s and an hour", () => {
  const now = 1_790_000_000_000;
  const spent = readBudget(headers({ "x-ratelimit-requests-remaining": "0", "x-ratelimit-requests-reset": String(now + 90_500), "x-ratelimit-complexity-reset": String(now + 600_000) }));
  assert.equal(budgetWait(spent, now), 91);
  assert.equal(budgetWait(readBudget(headers({ "x-ratelimit-complexity-reset": String(now + 600_000) })), now), 601);
  assert.equal(budgetWait(readBudget(headers({ "x-ratelimit-requests-remaining": "0", "x-ratelimit-requests-reset": String(now - 5000) })), now), 5);
  assert.equal(budgetWait(readBudget(headers({ "x-ratelimit-requests-remaining": "0", "x-ratelimit-requests-reset": String(now + 9e9) })), now), 3600);
  assert.equal(budgetWait({}, now), 60);
});

test("a spent budget is told apart from other failures, with its wait", async () => {
  const real = globalThis.fetch;
  globalThis.fetch = (async () =>
    new Response(JSON.stringify({ errors: [{ message: "Rate limit exceeded", extensions: { code: "RATELIMITED" } }] }), {
      status: 400,
      headers: { "x-ratelimit-requests-remaining": "0", "x-ratelimit-requests-reset": String(Date.now() + 120_000) },
    })) as typeof fetch;
  try {
    await assert.rejects(readBlockers("token", "session-1"), (error: unknown) => error instanceof RateLimitedError && error.waitSeconds >= 119 && error.waitSeconds <= 121);
  } finally {
    globalThis.fetch = real;
  }
});

test("a 429 is a spent budget even when its body is not JSON", async () => {
  const real = globalThis.fetch;
  globalThis.fetch = (async () => new Response("<html>slow down</html>", { status: 429 })) as typeof fetch;
  try {
    await assert.rejects(readBlockers("token", "session-1"), (error: unknown) => error instanceof RateLimitedError && error.waitSeconds === 60);
  } finally {
    globalThis.fetch = real;
  }
});

test("the blocker read pages through 50 relations at a time, at most five pages, and counts a page past them as blocked", async () => {
  const relation = (state: string) => ({ type: "blocks", issue: { state: { type: state } } });
  const page = (nodes: unknown[], after: string | null) => ({ nodes, pageInfo: { hasNextPage: after !== null, endCursor: after } });
  const read = async (pages: number, last: unknown[]) => {
    const sent: { first: string; after: unknown }[] = [];
    const real = globalThis.fetch;
    globalThis.fetch = (async (_url: unknown, init: { body: string }) => {
      const { query, variables } = JSON.parse(init.body) as { query: string; variables: { after?: string } };
      sent.push({ first: /first: (\d+)/.exec(query)?.[1] ?? "", after: variables.after ?? null });
      const n = sent.length;
      const relations = page(n < pages ? [relation("completed")] : last, n < pages || n >= 5 && pages > 5 ? `c${n}` : null);
      const data = n === 1 ? { agentSession: { issue: { id: "ab-1", identifier: "TW-1", inverseRelations: relations } } } : { issue: { inverseRelations: relations } };
      return new Response(JSON.stringify({ data }), { status: 200 });
    }) as typeof fetch;
    try {
      return { blockers: await readBlockers("token", "session-1"), sent };
    } finally {
      globalThis.fetch = real;
    }
  };
  const one = await read(1, [relation("completed")]);
  assert.deepEqual(one.blockers, { issueId: "ab-1", identifier: "TW-1", unfinished: 0 });
  assert.deepEqual(one.sent, [{ first: "50", after: null }]);
  // Cancelled and duplicate blockers have ended too.
  assert.equal((await read(1, [relation("canceled"), relation("duplicate")])).blockers?.unfinished, 0);
  const three = await read(3, [relation("started")]);
  assert.equal(three.blockers?.unfinished, 1);
  assert.deepEqual(three.sent.map((s) => s.after), [null, "c1", "c2"]);
  // Past five pages, what is left unread counts as an unfinished blocker.
  const many = await read(9, [relation("completed")]);
  assert.equal(many.sent.length, 5);
  assert.equal(many.blockers?.unfinished, 1);
});
