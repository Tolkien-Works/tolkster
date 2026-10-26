import assert from "node:assert/strict";
import { test } from "node:test";
import { describeGitHub, fromPush, GITHUB_EVENTS, githubSignature, pushedAt } from "../src/worker/github.ts";
import { verifySignature } from "../src/shared/signature.ts";

const repository = { full_name: "Tolkien-Works/tolkster", default_branch: "main" };

test("the Worker takes the events the app subscribes to", () => {
  // Change docs/github-app.md's subscriptions with this list.
  assert.deepEqual([...GITHUB_EVENTS].sort(), ["check_suite", "issue_comment", "pull_request", "pull_request_review", "pull_request_review_comment", "pull_request_review_thread", "push", "status"]);
});

test("a delivery is described for the log by its event, action, repo and PR alone", () => {
  assert.deepEqual(describeGitHub("pull_request", { repository, action: "closed", pull_request: { number: 7 } }), { githubEvent: "pull_request", action: "closed", repo: "Tolkien-Works/tolkster", number: 7 });
  assert.deepEqual(describeGitHub("issue_comment", { repository, action: "created", issue: { number: 7, pull_request: {} } }).number, 7);
  // A comment on an issue's conversation, not a PR's, names none.
  assert.equal(describeGitHub("issue_comment", { repository, action: "created", issue: { number: 7 } }).number, null);
  assert.equal(describeGitHub("check_suite", { repository, action: "completed", check_suite: { pull_requests: [{ number: 9 }, { number: 9 }] } }).number, 9);
  assert.deepEqual(describeGitHub("status", { repository }), { githubEvent: "status", action: null, repo: "Tolkien-Works/tolkster", number: null });
});

test("a push to the default branch is the repo's base tip; any other push isn't", () => {
  const after = "a1fe200000000000000000000000000000000000";
  const push = (ref: unknown, extra: object = {}) => fromPush("push", { repository, ref, after, ...extra });
  assert.deepEqual(push("refs/heads/main"), { type: "base", repo: "Tolkien-Works/tolkster", ref: "refs/heads/main", after });
  // A push names the commit it moved to, plainly.
  assert.equal(push("refs/heads/main", { after: undefined }), null);
  assert.equal(push("refs/heads/main", { after: "HEAD; rm" }), null);
  assert.equal(push("refs/heads/feature"), null);
  assert.equal(push("refs/tags/main"), null);
  assert.equal(push("refs/heads/main", { deleted: true }), null);
  assert.equal(fromPush("push", { repository: { full_name: "a/b" }, ref: "refs/heads/undefined" }), null);
  assert.equal(fromPush("pull_request", { repository, ref: "refs/heads/main" }), null);
});

test("a push's time is its head commit's, and 0 when it names none", () => {
  assert.equal(pushedAt({ head_commit: { timestamp: "2026-10-20T18:34:20+10:00" } }), Date.parse("2026-10-20T08:34:20Z"));
  for (const head_commit of [undefined, null, {}, { timestamp: "soon" }]) assert.equal(pushedAt({ head_commit }), 0);
});

test("a repo name that isn't plain is logged as none", () => {
  for (const full_name of ["a b/c", "owner/repo; rm", "owner", "owner/repo/extra", 7, "", "-o/../x y"]) {
    assert.equal(describeGitHub("pull_request", { repository: { full_name }, action: "closed", pull_request: { number: 7 } }).repo, null, String(full_name));
  }
});

test("GitHub's signature header is read as sha256=<hex> and verified over the raw body", async () => {
  const body = JSON.stringify({ action: "closed" });
  const key = await crypto.subtle.importKey("raw", new TextEncoder().encode("s3cret"), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  const hex = Buffer.from(await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(body))).toString("hex");
  assert.equal(githubSignature(`sha256=${hex}`), hex);
  assert.ok(await verifySignature(body, githubSignature(`sha256=${hex}`), "s3cret"));
  assert.ok(!(await verifySignature(body, githubSignature(`sha256=${hex}`), "other")));
  for (const bad of [null, "", hex, `sha1=${hex}`, `sha256=${hex}0`, "sha256=xyz"]) assert.equal(githubSignature(bad), null, String(bad));
});
