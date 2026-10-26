import assert from "node:assert/strict";
import { createHmac } from "node:crypto";
import { test } from "node:test";
import { isFresh, MAX_SKEW_MS, verifySignature } from "../src/shared/signature.ts";

const SECRET = "lin_wh_test_secret";
const BODY = '{"type":"AgentSessionEvent","action":"created","webhookTimestamp":1790000000000}';
const sign = (body: string, secret = SECRET) => createHmac("sha256", secret).update(body).digest("hex");

test("accepts a body signed with the secret", async () => {
  assert.equal(await verifySignature(BODY, sign(BODY), SECRET), true);
});

test("accepts an upper-case hex signature", async () => {
  assert.equal(await verifySignature(BODY, sign(BODY).toUpperCase(), SECRET), true);
});

test("rejects a body signed with another secret", async () => {
  assert.equal(await verifySignature(BODY, sign(BODY, "someone-else"), SECRET), false);
});

test("rejects a body altered after signing", async () => {
  assert.equal(await verifySignature(BODY.replace("created", "prompted"), sign(BODY), SECRET), false);
});

test("rejects the same JSON re-serialised, which is why the raw body is verified", async () => {
  const reformatted = JSON.stringify(JSON.parse(BODY), null, 2);
  assert.equal(await verifySignature(reformatted, sign(BODY), SECRET), false);
});

test("rejects a missing or malformed signature without throwing", async () => {
  assert.equal(await verifySignature(BODY, null, SECRET), false);
  assert.equal(await verifySignature(BODY, "", SECRET), false);
  assert.equal(await verifySignature(BODY, "not-hex", SECRET), false);
  assert.equal(await verifySignature(BODY, sign(BODY).slice(0, 62), SECRET), false);
});

test("an empty secret verifies nothing, even a body signed with an empty key", async () => {
  assert.equal(await verifySignature(BODY, sign(BODY, ""), ""), false);
});

test("a timestamp within a minute either way is fresh", () => {
  const now = 1_790_000_000_000;
  assert.equal(isFresh(now, now), true);
  assert.equal(isFresh(now - MAX_SKEW_MS, now), true);
  assert.equal(isFresh(now + MAX_SKEW_MS, now), true);
});

test("an older or future timestamp is a replay risk and is refused", () => {
  const now = 1_790_000_000_000;
  assert.equal(isFresh(now - MAX_SKEW_MS - 1, now), false);
  assert.equal(isFresh(now + MAX_SKEW_MS + 1, now), false);
});

test("a missing or non-numeric timestamp is not fresh", () => {
  assert.equal(isFresh(undefined, 0), false);
  assert.equal(isFresh("1790000000000", 1_790_000_000_000), false);
});
