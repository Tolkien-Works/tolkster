import assert from "node:assert/strict";
import { test } from "node:test";
import { fromIssue as issueEvents } from "../src/worker/events.ts";
import { contextAsked, describeIssue, issueFacts, type IssuePayload, issueIds, triageAsked } from "../src/worker/issue.ts";

const AT = "2026-09-28T00:00:00.000Z";
const ID = "22f14dcb-a0e9-4ad5-bc3b-7896c9f14212";
const data = (type: string) => ({ id: ID, identifier: "TW-163", title: "Not read", updatedAt: AT, state: { type } });
const action = { issueId: ID, at: AT };

const APP = "app-1";
const PARENT = "9a0e2f6b-1c1d-4e55-8a77-0b1f0c2d3e4f";
const KNOWN = { appUserId: APP, withSessions: [] };

test("an issue completed asks its store for its context, by its ID and time alone", () => {
  assert.deepEqual(contextAsked({ action: "update", data: data("completed"), updatedFrom: { stateId: "s0" } }, null), action);
  // A cancel, or a delegate change alone, is the state machine's (events.ts), not a context.
  assert.equal(contextAsked({ action: "update", data: data("canceled"), updatedFrom: { stateId: "s0" } }, null), null);
  assert.equal(contextAsked({ action: "update", data: data("started"), updatedFrom: { delegateId: null } }, null), null);
});

test("an issue created in a triage state asks the triage machine, whoever created it, and asks for no context", () => {
  // Not judged by the app user ID: `listen` checks the creator.
  assert.equal(triageAsked({ action: "create", data: data("triage") }), ID);
  assert.equal(contextAsked({ action: "create", data: data("triage") }, null), null);
  assert.equal(contextAsked({ action: "create", data: data("triage") }, KNOWN), null);
  for (const type of ["backlog", "unstarted", "started", "completed", "canceled"]) {
    assert.equal(triageAsked({ action: "create", data: data(type) }), null, type);
  }
  assert.equal(triageAsked({ action: "update", data: data("triage"), updatedFrom: { stateId: "s0" } }), null);
  assert.equal(triageAsked({ action: "create", data: { ...data("triage"), state: null } }), null);
  assert.equal(triageAsked({ action: "create", data: { ...data("triage"), id: "not an id" } }), null);
});

test("with what the Durable Object knows, only a completion that is Tolkster's asks for its context", () => {
  const ours: object[] = [
    { action: "update", data: { ...data("completed"), delegateId: APP }, updatedFrom: { stateId: "s0" } },
    { action: "update", data: { ...data("completed"), delegateId: null }, updatedFrom: { stateId: "s0", delegateId: APP } },
    { action: "update", data: { ...data("completed"), delegateId: null, subscriberIds: ["u1", APP] }, updatedFrom: { stateId: "s0" } },
  ];
  for (const payload of ours) assert.deepEqual(contextAsked(payload, KNOWN), action, JSON.stringify(payload));
  // Completed with a Tolkster session on it, as a mention or an undelegated issue leaves it,
  // or on its parent, as a parent worked through its sub-issues has.
  const finished = { action: "update", data: { ...data("completed"), delegateId: null, parentId: PARENT }, updatedFrom: { stateId: "s0" } };
  assert.deepEqual(contextAsked(finished, { appUserId: APP, withSessions: [ID] }), action);
  assert.deepEqual(contextAsked(finished, { appUserId: APP, withSessions: [PARENT] }), action);
  assert.equal(contextAsked(finished, { appUserId: APP, withSessions: ["other"] }), null);
  assert.deepEqual(issueIds(finished), [ID, PARENT]);
  assert.deepEqual(issueIds({ data: { id: ID, parentId: "not an id!" } }), [ID]);
  const others: object[] = [
    { action: "update", data: data("completed"), updatedFrom: { stateId: "s0" } },
    { action: "update", data: { ...data("completed"), delegateId: "other-app", subscriberIds: ["u1"] }, updatedFrom: { stateId: "s0" } },
    // The previous delegate counts only when the delegate is what changed.
    { action: "update", data: { ...data("completed"), delegateId: null }, updatedFrom: { stateId: "s0", title: APP } },
    { action: "update", data: { ...data("completed"), subscriberIds: APP }, updatedFrom: { stateId: "s0" } },
  ];
  for (const payload of others) assert.equal(contextAsked(payload, KNOWN), null, JSON.stringify(payload));
});

test("any other issue change queues nothing", () => {
  const ignored: object[] = [
    { action: "update", data: data("started"), updatedFrom: { stateId: "s0" } },
    { action: "update", data: data("completed"), updatedFrom: { title: "Old" } },
    { action: "update", data: data("completed") },
    { action: "create", data: data("completed"), updatedFrom: { stateId: "s0" } },
    { action: "remove", data: data("canceled"), updatedFrom: { stateId: "s0" } },
    // Without a plain id or Linear's timestamp, there is nothing to name.
    { action: "update", data: { ...data("completed"), id: "x; rm" }, updatedFrom: { stateId: "s0" } },
    { action: "update", data: { ...data("completed"), updatedAt: "yesterday" }, updatedFrom: { stateId: "s0" } },
    { action: "update", data: { ...data("completed"), updatedAt: "2026-09-28T00:00:00Z" }, updatedFrom: { stateId: "s0" } },
    { action: "update", updatedFrom: { stateId: "s0" } },
  ];
  for (const payload of ignored) assert.equal(contextAsked(payload, null), null, JSON.stringify(payload));
});

test("each delivery is logged by the names of the fields that changed, never their values", () => {
  const logged = describeIssue({ action: "update", data: data("started"), updatedFrom: { title: "Secret plans", updatedAt: AT, "bad key": 1 } });
  assert.deepEqual(logged, { issueId: ID, changed: ["title", "updatedAt"] });
  assert.ok(!JSON.stringify(logged).includes("Secret"));
  assert.deepEqual(describeIssue({ action: "remove", data: { id: "not an id" } }), { issueId: null, changed: [] });
});

test("the payload the Durable Object judges keeps only IDs and plain values, and judges as the whole one does", () => {
  const OWNER = "5b1e2f6b-1c1d-4e55-8a77-0b1f0c2d3e4f";
  const DELIVERY = "0f8e2f6b-1c1d-4e55-8a77-0b1f0c2d3e4f";
  const payloads = [
    { action: "update", data: { ...data("completed"), description: "Secret plans", delegateId: APP, priority: 2 }, updatedFrom: { stateId: "s0", title: "Secret old" }, actor: { id: OWNER } },
    { action: "update", data: { ...data("started"), delegateId: APP, parentId: PARENT }, updatedFrom: { delegateId: null, parentId: "not a uuid" }, actor: { id: OWNER } },
    { action: "update", data: { ...data("completed"), subscriberIds: ["u1", APP, "Secret plans"] }, updatedFrom: { stateId: "s0" } },
    { action: "create", data: { ...data("triage"), parentId: PARENT } },
    { action: "remove", data: data("canceled"), updatedFrom: { stateId: "s0" } },
    // The chain's own delegation, a subscription alone, a delegate that isn't plain, and no changes or actor.
    { action: "update", data: { ...data("started"), delegateId: APP }, updatedFrom: { delegateId: null }, actor: { id: APP } },
    { action: "update", data: { ...data("completed"), delegateId: null, subscriberIds: [APP] }, updatedFrom: { stateId: "s0" } },
    { action: "update", data: { ...data("completed"), delegateId: "Secret plans" }, updatedFrom: { stateId: "s0", delegateId: APP } },
    { action: "update", data: { ...data("completed"), delegateId: "user_1" } },
    { action: "update", data: { ...data("started"), priority: 1 }, updatedFrom: { priority: 3 } },
  ] as IssuePayload[];
  for (const payload of payloads) {
    const facts = issueFacts(payload);
    assert.ok(!JSON.stringify(facts).includes("Secret"), JSON.stringify(facts));
    assert.deepEqual(contextAsked(facts, KNOWN), contextAsked(payload, KNOWN));
    assert.deepEqual(contextAsked(facts, null), contextAsked(payload, null));
    assert.equal(triageAsked(facts), triageAsked(payload));
    assert.deepEqual(issueIds(facts), issueIds(payload));
    assert.deepEqual(issueEvents(facts, DELIVERY, OWNER, APP), issueEvents(payload, DELIVERY, OWNER, APP));
  }
  // A change of priority reaches the machine.
  assert.deepEqual(issueEvents(issueFacts(payloads.at(-1)!), DELIVERY, OWNER, APP).map((e) => e.kind), ["priorityChanged"]);
  // An ID as wide as the stored app user ID's is kept.
  assert.equal(issueFacts({ data: { delegateId: "user_1" } }).data?.delegateId, "user_1");
  // A changed field's value that isn't plain still says it changed.
  assert.deepEqual(issueFacts({ action: "update", updatedFrom: { delegateId: "x; rm", title: "t" } }).updatedFrom, { delegateId: null });
});
