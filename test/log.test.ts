import assert from "node:assert/strict";
import { test } from "node:test";
import { deliveryId } from "../src/worker/log.ts";

test("a delivery ID is logged only if it has a UUID's shape", () => {
  assert.equal(deliveryId("72d3162e-cc78-11e3-81ab-4c9367dc0958"), "72d3162e-cc78-11e3-81ab-4c9367dc0958");
  assert.equal(deliveryId("72D3162E-CC78-11E3-81AB-4C9367DC0958"), "72D3162E-CC78-11E3-81AB-4C9367DC0958");
  for (const header of [null, "", "jane@example.com", "72d3162e-cc78-11e3-81ab-4c9367dc0958 extra", "x".repeat(10_000)]) assert.equal(deliveryId(header), null);
});
