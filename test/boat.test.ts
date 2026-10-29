// The boat recipe's own rules (scripts/orca-vm/boat.ts): retries, the trial's fallback, deletes
// polled to the end, the credential check, sizes and the snapshot's name.

import assert from "node:assert/strict";
import { afterEach, test } from "node:test";
import { assertNoCredentials, createSandbox, deleteSandbox, retire, retryDelay, size, snapshotName } from "../scripts/orca-vm/boat.ts";

type Call = { method: string; path: string; body: any; headers: Record<string, string> };
const realFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = realFetch;
});

// Answers each call with the next of `answers`, recording what was sent.
function serve(answers: [number, unknown][]): Call[] {
  process.env.BOAT_API_KEY = "boat_test";
  const calls: Call[] = [];
  globalThis.fetch = (async (url: string, init: RequestInit) => {
    calls.push({
      method: init.method as string,
      path: url.replace("https://boat.dev/api/v1", ""),
      body: init.body ? JSON.parse(init.body as string) : undefined,
      headers: init.headers as Record<string, string>,
    });
    const [status, body] = answers.shift() ?? [500, {}];
    return new Response(JSON.stringify(body), { status });
  }) as typeof fetch;
  return calls;
}

test("a call is sent again only where boat's guidance says it may be", () => {
  assert.equal(retryDelay(503, { retryAfterSeconds: 7 }), 7000);
  assert.equal(retryDelay(503, {}), 5000);
  assert.equal(retryDelay(503, { retryAfterSeconds: 600 }), 60_000);
  assert.equal(retryDelay(502, { error: { code: "sandbox_direct_failed", details: { retryable: true } } }), 2000);
  assert.equal(retryDelay(502, { error: { code: "sandbox_direct_failed", details: { retryable: false } } }), undefined);
  assert.equal(retryDelay(409, { error: { code: "idempotency_in_progress" } }), 5000);
  assert.equal(retryDelay(409, { error: { code: "idempotency_key_reused" } }), undefined);
  assert.equal(retryDelay(409, { error: { code: "machine_not_running" } }), undefined);
  assert.equal(retryDelay(400, { error: { code: "trial_auto_stop_required" } }), undefined);
  assert.equal(retryDelay(404, {}), undefined);
});

test("a sandbox has no auto-stop and no account environment, and on the trial stops after two hours", async () => {
  const calls = serve([
    [400, { ok: false, code: "trial_auto_stop_required", error: { code: "trial_auto_stop_required" } }],
    [202, { ok: true, sandbox: { id: "bx_a" } }],
  ]);
  assert.deepEqual(await createSandbox({ from: "s" }, "k"), { id: "bx_a" });
  assert.deepEqual(calls.map((c) => [c.body.noEnv, c.body.ttlSeconds, c.headers["Idempotency-Key"]]), [[true, null, "k"], [true, 7200, "k-trial"]]);
});

test("a delete is polled until boat completes it, and one already gone is done", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const op = (status: string) => [202, { ok: true, operation: { id: "bdop_1", status } }] as [number, unknown];
  const calls = serve([op("pending"), [200, { ok: true, operation: { id: "bdop_1", status: "processing" } }], [200, { ok: true, operation: { id: "bdop_1", status: "completed" } }]]);
  let done = false;
  const deleting = deleteSandbox("bx_a").then(() => (done = true));
  while (!done) {
    await new Promise((resolve) => setImmediate(resolve));
    t.mock.timers.tick(5000);
  }
  await deleting;
  assert.deepEqual(calls.map((c) => `${c.method} ${c.path}`), ["DELETE /sandboxes/bx_a", "GET /deletion-operations/bdop_1", "GET /deletion-operations/bdop_1"]);
  assert.equal(calls[0].headers["X-Ascii-Confirm-Delete"], "bx_a");
  serve([[404, { ok: false, code: "not_found" }]]);
  await deleteSandbox("bx_a");
});

test("a sandbox holding any of the owner's credentials is refused, naming only what it found", async () => {
  serve([[200, { exitCode: 0, stdout: "", stderr: "" }]]);
  await assertNoCredentials("bx_a");
  serve([[200, { exitCode: 0, stdout: "CLAUDE_CODE_OAUTH_TOKEN\n/home/user/.git-credentials\n", stderr: "" }]]);
  await assert.rejects(assertNoCredentials("bx_a"), /CLAUDE_CODE_OAUTH_TOKEN, \/home\/user\/\.git-credentials/);
});

test("the size comes from the create line, default without one, and an unknown one is refused", () => {
  assert.equal(size([]), "default");
  assert.equal(size(["--size", "small"]), "small");
  assert.equal(size(["--size", "xlarge"]), undefined);
  assert.equal(size(["--size"]), undefined);
});

test("the snapshot's name follows the base's inputs and the Orca version, within boat's rules", () => {
  const name = snapshotName("sha256:a", "1.4.241");
  assert.match(name, /^[a-z0-9][a-z0-9-]{0,62}$/);
  assert.equal(snapshotName("sha256:a", "1.4.241"), name);
  assert.notEqual(snapshotName("sha256:b", "1.4.241"), name);
  assert.notEqual(snapshotName("sha256:a", "1.4.242"), name);
});

test("a new snapshot retires the old one, never itself", () => {
  assert.deepEqual(retire({ snapshot: "a" }, "b"), ["a"]);
  assert.deepEqual(retire({ snapshot: "b", retired: ["a"] }, "b"), ["a"]);
  assert.deepEqual(retire({ snapshot: "b", retired: ["a"] }, "a"), ["b"]);
  assert.deepEqual(retire({}, "a"), []);
});
