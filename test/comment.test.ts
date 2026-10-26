import assert from "node:assert/strict";
import { test } from "node:test";
import { describeComment, fromComment } from "../src/worker/comment.ts";

const AT = "2026-09-28T00:00:00.000Z";
const OWNER = "7c1d9e20-4b3a-4f58-9a61-0d2e8f3b5c4c";
const ISSUE = "22f14dcb-a0e9-4ad5-bc3b-7896c9f14212";
const COMMENT = "78d61edf-6060-4272-a871-3436549db095";
const data = { id: COMMENT, body: "Not read", issueId: ISSUE, userId: OWNER, createdAt: AT, updatedAt: AT, issue: { id: ISSUE, identifier: "TW-192", title: "Not read" } };
const action = { issueId: ISSUE, commentId: COMMENT, identifier: "TW-192", at: AT };

test("the owner's new comment on an issue is told by its IDs alone", () => {
  assert.deepEqual(fromComment({ action: "create", data }, OWNER), action);
  // A reply in a thread is a comment too: `readComment` tells a session's thread apart.
  const reply: object = { ...data, parentId: "p-1" };
  assert.deepEqual(fromComment({ action: "create", data: reply }, OWNER), action);
  // An identifier that isn't Linear's shape is dropped, not passed on.
  assert.deepEqual(fromComment({ action: "create", data: { ...data, issue: { identifier: "TW-1; rm" } } }, OWNER), { ...action, identifier: null });
  assert.deepEqual(describeComment({ action: "create", data }), { commentId: COMMENT, issueId: ISSUE });
});

test("anyone else's comment, an edit, or one not on an issue queues nothing", () => {
  const ignored: object[] = [
    { action: "create", data: { ...data, userId: "someone-else" } },
    { action: "create", data: { ...data, userId: undefined } },
    // An integration posting as the owner.
    { action: "create", data: { ...data, botActor: { id: "b-1", name: "Not read" } } },
    { action: "update", data },
    { action: "remove", data },
    // A comment on a project update or a document has no issue.
    { action: "create", data: { ...data, issueId: null, issue: null } },
    // Without plain ids or Linear's timestamp, there is nothing to name.
    { action: "create", data: { ...data, id: "x; rm" } },
    { action: "create", data: { ...data, issueId: "x; rm" } },
    { action: "create", data: { ...data, createdAt: "yesterday" } },
    { action: "create" },
  ];
  for (const payload of ignored) assert.equal(fromComment(payload, OWNER), null, JSON.stringify(payload));
  // A Worker without its owner set counts nobody's.
  assert.equal(fromComment({ action: "create", data: { ...data, userId: "" } }, ""), null);
});
