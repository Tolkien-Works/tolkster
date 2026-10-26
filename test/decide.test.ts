import assert from "node:assert/strict";
import { mock, test } from "node:test";
import type { AgentSessionEventWebhookPayload } from "@linear/sdk/webhooks";
import { acknowledgement, AWAY, decide, NO_ISSUE, NOT_OWNER } from "../src/worker/decide.ts";
import { phrase, phrases } from "../src/shared/voice.ts";

const OWNER = "owner-user-id";
const STRANGER = "stranger-user-id";
// Fixes each acknowledgement to its state's first phrase.
const first = () => 0;
const STARTING = phrase("acknowledging", first);
const HEARD = phrase("heard", first);
const STOPPING = phrase("stopping", first);

function event(action: string, session: Record<string, unknown>, prompt?: { signal?: string; userId?: string }): AgentSessionEventWebhookPayload {
  return {
    type: "AgentSessionEvent",
    action,
    appUserId: "app-1",
    agentSession: {
      id: "session-1",
      issueId: "issue-1",
      createdAt: "2026-09-25T01:00:00.000Z",
      issue: { id: "issue-1", identifier: "TW-1", url: "https://linear.app/x/issue/TW-1", title: "Secret title", description: "Secret text" },
      ...session,
    },
    ...(action === "prompted" ? { agentActivity: { id: "activity-1", createdAt: "2026-09-25T02:00:00.000Z", content: { body: "Secret prompt" }, userId: OWNER, ...prompt } } : {}),
  } as unknown as AgentSessionEventWebhookPayload;
}

test("the owner's delegation is acknowledged with an ephemeral thought, and stays delegated", () => {
  assert.deepEqual(decide(event("created", { creatorId: OWNER }), OWNER, first), {
    kind: "start",
    activity: { agentSessionId: "session-1", content: { type: "thought", body: STARTING }, ephemeral: true },
    request: { trust: "owner", sessionKind: "delegation", commentId: null, identifier: "TW-1", at: "2026-09-25T01:00:00.000Z" },
  });
  // Linear's own thread comment for a delegation has no author, so it's not a mention.
  const thread = { id: "c-0", body: "This thread is for an agent session with tolkster.", userId: null };
  const threaded = decide(event("created", { creatorId: OWNER, comment: thread }), OWNER, first);
  assert.deepEqual(threaded?.activity.content, { type: "thought", body: STARTING });
});

test("the owner's mention in a comment is acknowledged with an ephemeral thought that says I heard", () => {
  const comment = { id: "c-1", body: "@tolkster why is this slow?", userId: OWNER };
  const decision = decide(event("created", { creatorId: OWNER, comment, sourceCommentId: "c-1" }), OWNER, first);
  assert.deepEqual(decision?.activity, { agentSessionId: "session-1", content: { type: "thought", body: HEARD }, ephemeral: true });
  // A reply in another thread, whose root has no author, is a mention too.
  const reply = decide(event("created", { creatorId: OWNER, comment: { id: "c-0", body: "Synced", userId: null }, sourceCommentId: "c-1" }), OWNER, first);
  assert.deepEqual(reply?.activity.content, { type: "thought", body: HEARD });
  // The request only names the comment: tolkster listen reads the mention from Linear.
  assert.deepEqual(decision?.request, { trust: "owner", sessionKind: "mention", commentId: "c-1", identifier: "TW-1", at: "2026-09-25T01:00:00.000Z" });
});

test("a session Tolkster opened on its own comment is heard, and only the owner's prompts in it count", () => {
  const note = { id: "c-0", body: "Context note", userId: "app-1" };
  assert.deepEqual(decide(event("created", { comment: note }), OWNER, first)?.activity.content, { type: "thought", body: HEARD });
  assert.equal(decide(event("prompted", { comment: note }), OWNER, first)?.kind, "deliver");
  assert.equal(decide(event("prompted", { comment: note }, { userId: STRANGER }), OWNER, first)?.kind, "not_owner");
  // No creator on anyone else's comment, or with a source comment, is not Tolkster's own.
  assert.equal(decide(event("created", { comment: { ...note, userId: STRANGER } }), OWNER)?.kind, "not_owner");
  assert.equal(decide(event("created", { comment: note, sourceCommentId: "c-1" }), OWNER)?.kind, "not_owner");
});

test("someone else's delegation is refused with a response and handed back", () => {
  assert.deepEqual(decide(event("created", { creatorId: STRANGER }), OWNER), {
    kind: "not_owner",
    activity: { agentSessionId: "session-1", content: { type: "response", body: NOT_OWNER } },
    undelegate: { issueId: "issue-1", creatorId: STRANGER, appUserId: "app-1" },
  });
});

test("someone else's mention is refused with a response, and the issue stays delegated", () => {
  const comment = { id: "c-1", body: "@tolkster stop", userId: STRANGER };
  assert.deepEqual(decide(event("created", { creatorId: STRANGER, comment, sourceCommentId: "c-1" }), OWNER), {
    kind: "not_owner",
    activity: { agentSessionId: "session-1", content: { type: "response", body: NOT_OWNER } },
  });
  // A reply in a thread whose root has no author, such as a synced comment's, is a mention too.
  const root = { id: "c-0", body: "Synced", userId: null };
  assert.equal(decide(event("created", { creatorId: STRANGER, comment: root, sourceCommentId: "c-1" }), OWNER)?.undelegate, undefined);
  // A stranger's reply of that kind on a sub-issue Tolkster delegated to itself is refused.
  const sub = decide(event("created", { creatorId: STRANGER, comment: root, sourceCommentId: "c-1" }), OWNER, first, true);
  assert.equal(sub?.kind, "not_owner");
  assert.equal(sub?.undelegate, undefined);
  // The same on a sub-issue Tolkster delegated to itself, so the parent's chain holds.
  assert.equal(decide(event("created", { creatorId: STRANGER, comment }), OWNER, first, true)?.undelegate, undefined);
});

test("a refused session with no issue, such as a mention in a document, has nothing to hand back", () => {
  const decision = decide(event("created", { creatorId: STRANGER, issueId: undefined, issue: undefined }), OWNER);
  assert.equal(decision?.activity.content.type, "response");
  assert.equal(decision?.undelegate, undefined);
});

test("the owner's session with no issue is answered that only issues work", () => {
  const sourceMetadata = { type: "document", documentId: "doc-1", title: "Secret plans", agentSessionMetadata: { sourceCommentId: "c-1", body: "hi" } };
  const decision = decide(event("created", { creatorId: OWNER, issueId: undefined, issue: undefined, sourceMetadata }), OWNER);
  assert.deepEqual(decision, {
    kind: "no_issue",
    activity: { agentSessionId: "session-1", content: { type: "response", body: NO_ISSUE } },
  });
  assert.deepEqual(decide(event("prompted", { creatorId: OWNER, issueId: undefined, issue: undefined }), OWNER)?.activity.content, { type: "response", body: NO_ISSUE });
});

test("replies in a refused session are not answered again", () => {
  assert.equal(decide(event("prompted", { creatorId: STRANGER }, { userId: STRANGER }), OWNER), null);
});

test("the creator can also arrive as a nested user rather than creatorId", () => {
  assert.equal(decide(event("created", { creator: { id: OWNER } }), OWNER)?.activity.content.type, "thought");
});

test("a session with no creator, as automations and other agents start, is not the owner's", () => {
  assert.equal(decide(event("created", {}), OWNER)?.activity.content.type, "response");
});

test("a reply from the owner is acknowledged with an ephemeral thought, for tolkster listen to pass on", () => {
  assert.deepEqual(decide(event("prompted", { creatorId: OWNER }), OWNER, first), {
    kind: "deliver",
    activity: { agentSessionId: "session-1", content: { type: "thought", body: HEARD }, ephemeral: true },
    request: { trust: "owner", sessionKind: "delegation", commentId: null, identifier: "TW-1", at: "2026-09-25T02:00:00.000Z" },
  });
});

test("a stop from the owner is acknowledged with an ephemeral thought, for tolkster listen to act on", () => {
  const stop = decide(event("prompted", { creatorId: OWNER }, { signal: "stop" }), OWNER, first);
  assert.deepEqual(stop?.activity, {
    agentSessionId: "session-1",
    content: { type: "thought", body: STOPPING },
    ephemeral: true,
  });
  assert.deepEqual(stop?.request, { trust: "owner", sessionKind: "delegation", commentId: null, identifier: "TW-1", at: "2026-09-25T02:00:00.000Z" });
});

test("a reply from someone else in the owner's session is refused, and the issue stays delegated", () => {
  assert.deepEqual(decide(event("prompted", { creatorId: OWNER }, { userId: STRANGER }), OWNER), {
    kind: "not_owner",
    activity: { agentSessionId: "session-1", content: { type: "response", body: NOT_OWNER } },
  });
  // A stop counts only from the owner, too.
  assert.equal(decide(event("prompted", { creatorId: OWNER }, { userId: STRANGER, signal: "stop" }), OWNER)?.activity.content.type, "response");
  // No author at all is not the owner.
  const anonymous = { ...event("prompted", { creatorId: OWNER }), agentActivity: {} } as unknown as AgentSessionEventWebhookPayload;
  assert.deepEqual(decide(anonymous, OWNER)?.activity.content, { type: "response", body: NOT_OWNER });
});

test("the prompt's author can also arrive as a nested user", () => {
  const nested = { ...event("prompted", { creatorId: OWNER }), agentActivity: { user: { id: OWNER } } } as unknown as AgentSessionEventWebhookPayload;
  assert.deepEqual(decide(nested, OWNER, first)?.activity.content, { type: "thought", body: HEARD });
});

test("an archived session is left alone, so it is not brought back", () => {
  assert.equal(decide(event("prompted", { creatorId: OWNER, archivedAt: "2026-09-25T00:00:00Z" }), OWNER), null);
  assert.equal(decide(event("created", { creatorId: STRANGER, archivedAt: "2026-09-25T00:00:00Z" }), OWNER), null);
});

test("a payload without a session is ignored rather than thrown on", () => {
  const empty = { type: "AgentSessionEvent", action: "created" } as unknown as AgentSessionEventWebhookPayload;
  assert.equal(decide(empty, OWNER), null);
});

test("an unknown action is ignored", () => {
  assert.equal(decide(event("archived", { creatorId: OWNER }), OWNER), null);
});

test("refusals and ignored events send nothing to the workstation", () => {
  const refusals = [
    decide(event("created", { creatorId: STRANGER }), OWNER),
    decide(event("prompted", { creatorId: OWNER }, { userId: STRANGER }), OWNER),
    decide(event("prompted", { creatorId: OWNER }, { userId: STRANGER, signal: "stop" }), OWNER),
    decide(event("created", { creatorId: OWNER, issueId: undefined, issue: undefined }), OWNER),
    decide(event("prompted", { creatorId: OWNER, issueId: undefined, issue: undefined }), OWNER),
  ];
  for (const decision of refusals) {
    assert.equal(decision?.activity.content.type, "response");
    assert.equal(decision?.request, undefined);
  }
});

test("a request carries no issue, comment or prompt text", () => {
  const comment = { id: "c-1", body: "Secret comment", userId: OWNER };
  for (const decision of [
    decide(event("created", { creatorId: OWNER, comment }), OWNER),
    decide(event("prompted", { creatorId: OWNER }), OWNER),
    decide(event("prompted", { creatorId: OWNER }, { signal: "stop" }), OWNER),
  ]) {
    const wire = JSON.stringify(decision?.request);
    assert.ok(wire.length > 2);
    assert.doesNotMatch(wire, /Secret/);
  }
});

test("each acknowledgement is one of its state's phrases, picked at random", () => {
  const cases = [
    ["acknowledging", event("created", { creatorId: OWNER })],
    ["heard", event("created", { creatorId: OWNER, comment: { id: "c-1", body: "hi", userId: OWNER } })],
    ["heard", event("prompted", { creatorId: OWNER })],
    ["stopping", event("prompted", { creatorId: OWNER }, { signal: "stop" })],
  ] as const;
  for (const [state, payload] of cases) {
    const seen = new Set<string>();
    for (let i = 0; i < 200; i++) {
      const content = decide(payload, OWNER)?.activity.content;
      assert.ok(content?.type === "thought" && phrases(state).includes(content.body), state);
      seen.add(content.body);
    }
    assert.ok(seen.size > 1, `${state} always said the same`);
  }
});

test("a sub-issue Tolkster delegated to itself is taken like the owner's, whoever Linear names as creator", () => {
  for (const creator of [{ creatorId: "tolkster-app-user" }, { creatorId: null }, {}]) {
    const decision = decide(event("created", creator), OWNER, first, true);
    assert.equal(decision?.kind, "start");
    assert.equal(decision?.undelegate, undefined);
    assert.deepEqual(decision?.activity.content, { type: "thought", body: STARTING });
  }
  // Only the owner's prompts in it count.
  assert.equal(decide(event("prompted", { creatorId: null }), OWNER, first, true)?.kind, "deliver");
  assert.equal(decide(event("prompted", { creatorId: null }, { userId: STRANGER }), OWNER, first, true)?.kind, "not_owner");
  // A stranger's mention on it is theirs, not Tolkster's delegation.
  const comment = { id: "c-1", body: "@tolkster delete it", userId: STRANGER };
  assert.equal(decide(event("created", { creatorId: STRANGER, comment }), OWNER, first, true)?.kind, "not_owner");
});

test("with nothing listening, the owner's start, reply and stop are acknowledged with the away response", async () => {
  const away = { agentSessionId: "session-1", content: { type: "response", body: AWAY } };
  const start = decide(event("created", { creatorId: OWNER }), OWNER, first)!;
  const reply = decide(event("prompted", { creatorId: OWNER }), OWNER, first)!;
  const stop = decide(event("prompted", { creatorId: OWNER }, { signal: "stop" }), OWNER, first)!;
  for (const decision of [start, reply, stop]) {
    assert.deepEqual(await acknowledgement(decision, async () => false, 1000), { activity: away, presence: "away" });
    // Nothing else about the decision changes: the same action is queued, and the same acknowledgement recorded.
    assert.deepEqual(await acknowledgement(decision, async () => true, 1000), { activity: decision.activity, presence: "listening" });
  }
  assert.deepEqual(start.activity.content, { type: "thought", body: STARTING });
  assert.deepEqual(stop.activity.content, { type: "thought", body: STOPPING });
});

test("a relay that fails or is slow to say leaves the acknowledgement as it was", async () => {
  const decision = decide(event("created", { creatorId: OWNER }), OWNER, first)!;
  assert.deepEqual(await acknowledgement(decision, () => Promise.reject(new Error("relay down")), 1000), { activity: decision.activity, presence: "unknown" });
  mock.timers.enable({ apis: ["setTimeout"] });
  try {
    const pending = acknowledgement(decision, () => new Promise<boolean>(() => {}), 2000);
    mock.timers.tick(2000);
    assert.deepEqual(await pending, { activity: decision.activity, presence: "unknown" });
  } finally {
    mock.timers.reset();
  }
});

test("a refusal asks nothing about who is listening", async () => {
  const refused = decide(event("created", { creatorId: STRANGER }), OWNER)!;
  let asked = 0;
  assert.deepEqual(await acknowledgement(refused, async () => (asked++, false), 1000), { activity: refused.activity, presence: null });
  assert.equal(asked, 0);
});

test("a refused delegation from a session with no creator is not handed back", () => {
  const decision = decide(event("created", { creatorId: undefined }), OWNER);
  assert.equal(decision?.kind, "not_owner");
  assert.equal(decision?.undelegate, undefined);
});
