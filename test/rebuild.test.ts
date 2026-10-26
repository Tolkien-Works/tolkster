import assert from "node:assert/strict";
import { test } from "node:test";
import { oweRebuild, rebuildTimer } from "../src/worker/rebuild.ts";
import { MemoryStorage } from "./storage.ts";

const ONE = { issueId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa", identifier: "TW-1", started: false };

test("a reset's rebuild asks each delegated issue once Linear is read, and is owed again after a failure", async () => {
  const storage = new MemoryStorage();
  let found: (typeof ONE)[] | null = null;
  const asked: string[] = [];
  const timer = rebuildTimer(storage, async () => found, async (issue) => void asked.push(issue.issueId));
  assert.equal(await timer.next(), null);
  await oweRebuild(storage, 1000, true);
  assert.equal(await timer.next(), 1000);
  await timer.run(999);
  assert.equal(await timer.next(), 1000);
  // Linear unread: owed again later, nothing asked.
  await timer.run(1000);
  assert.equal(await timer.next(), 1000 + 10 * 60 * 1000);
  assert.deepEqual(asked, []);
  found = [ONE];
  await timer.run(1000 + 10 * 60 * 1000);
  assert.deepEqual(asked, [ONE.issueId]);
  assert.equal(await timer.next(), null);
});

test("an ask that fails leaves the rebuild owed", async () => {
  const storage = new MemoryStorage();
  await oweRebuild(storage, 0, false);
  const timer = rebuildTimer(storage, async () => [ONE], async () => {
    throw new Error("conflict");
  });
  await timer.run(0);
  assert.notEqual(await timer.next(), null);
});

test("a reset's rebuild says so to its read and asks, and a connect brings no owed rebuild forward", async () => {
  const storage = new MemoryStorage();
  const resets: boolean[] = [];
  const timer = rebuildTimer(storage, async (reset) => {
    resets.push(reset);
    if (resets.length === 1) throw new Error("unread");
    return [ONE];
  }, async (_issue, _nowMs, reset) => void resets.push(reset));
  await oweRebuild(storage, 0, true);
  await timer.run(0);
  const retry = 10 * 60 * 1000;
  await oweRebuild(storage, 5, false);
  assert.equal(await timer.next(), retry);
  await timer.run(retry);
  assert.deepEqual(resets, [true, true, true]);
  assert.equal(await timer.next(), null);
  await oweRebuild(storage, retry + 1, false);
  await timer.run(retry + 1);
  assert.deepEqual(resets.slice(3), [false, false]);
});
