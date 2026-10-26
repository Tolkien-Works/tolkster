import assert from "node:assert/strict";
import { mock, test } from "node:test";
import type { SessionEvent } from "../src/worker/events.ts";
import { intake } from "../src/worker/intake.ts";
import { refusedSessions, tellRefused, unanswered, whyUnanswered } from "../src/worker/unanswered.ts";
import { MemoryStorage } from "./storage.ts";

const SESSION = "0a1b2c3d-0000-4000-8000-0000000000aa";
const ISSUE = "0a1b2c3d-0000-4000-8000-000000000001";

// The lines the Worker logs as warnings, as the objects they print.
async function logged(run: () => void | Promise<void>): Promise<Record<string, unknown>[]> {
  const warn = mock.method(console, "warn", () => {});
  try {
    await run();
    return warn.mock.calls.map((c) => c.arguments[0] as Record<string, unknown>);
  } finally {
    warn.mock.restore();
  }
}

test("each way a session ends with no reply has a reason, and a session something will answer has none", () => {
  const ok = { request: true, mapped: true, posted: true, ingested: true };
  assert.equal(whyUnanswered(ok), null);
  // The machine answers a request it holds, whether or not the acknowledgement was posted.
  assert.equal(whyUnanswered({ ...ok, posted: false }), null);
  assert.equal(whyUnanswered({ ...ok, ingested: false }), "ingest_failed");
  assert.equal(whyUnanswered({ ...ok, mapped: false, ingested: false }), "unmapped");
  // A refusal has no request: its own post is its only reply.
  assert.equal(whyUnanswered({ request: false, mapped: false, posted: true, ingested: false }), null);
  assert.equal(whyUnanswered({ request: false, mapped: false, posted: false, ingested: false }), "post_failed");
});

test("session_unanswered names the session and why, and nothing else of it", async () => {
  const lines = await logged(() => unanswered(SESSION, "post_failed", { decision: "not_owner" }));
  assert.deepEqual(lines, [{ event: "session_unanswered", sessionId: SESSION, why: "post_failed", decision: "not_owner" }]);
});

test("a chain's session the intake refuses is logged and told, whether or not the notice lands", async () => {
  const event: SessionEvent = { deliveryId: "00000000-0000-4000-8000-000000000001", kind: "sessionStarted", sessionId: SESSION, issueId: ISSUE, trust: "chain", sessionKind: "delegation", commentId: null, at: "2026-10-01T10:00:00.000Z" };
  const storage = new MemoryStorage();
  const results = await intake(storage, event, Date.parse("2026-10-01T12:00:00.000Z"), async () => false, async () => [], async () => ({}), async () => false);
  assert.deepEqual(refusedSessions(results), [SESSION]);
  // A stop is refused too, and is not told "delegate it again".
  const stop = await intake(new MemoryStorage(), { ...event, kind: "ownerStopped", deliveryId: "00000000-0000-4000-8000-000000000002" }, Date.parse("2026-10-01T12:00:00.000Z"), async () => false, async () => [], async () => ({}), async () => false);
  assert.deepEqual(stop.map((r) => r.outcome), ["refused"]);
  assert.deepEqual(refusedSessions(stop), []);
  // The owner's own session, and the same event held by its chain, are refused none.
  const held = await intake(new MemoryStorage(), event, Date.parse("2026-10-01T12:00:00.000Z"), async () => false, async () => [], async () => ({}), async () => true);
  assert.deepEqual(refusedSessions(held), []);

  const told: string[] = [];
  for (const landed of [true, false]) {
    const lines = await logged(() => tellRefused([SESSION, SESSION], async (id) => (told.push(id), landed)));
    assert.deepEqual(lines, [{ event: "session_unanswered", sessionId: SESSION, why: "chain_not_held", told: landed }]);
  }
  // Once per session however many of its events were refused.
  assert.deepEqual(told, [SESSION, SESSION]);
});
