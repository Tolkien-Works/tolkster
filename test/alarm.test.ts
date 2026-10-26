import assert from "node:assert/strict";
import { test } from "node:test";
import { bringForward, earliest, runAll, type Timer } from "../src/worker/alarm.ts";
import { machineTimer } from "../src/worker/machine/store.ts";
import { MemoryStorage } from "./storage.ts";

function timer(due: number | null, ran: number[], fails = false): Timer {
  return {
    next: async () => due,
    run: async (nowMs) => {
      ran.push(nowMs);
      if (fails) throw new Error("failed");
    },
  };
}

test("the alarm is set to the earliest thing due, and to nothing when nothing is", async () => {
  assert.equal(await earliest([timer(300, []), timer(null, []), timer(100, [])]), 100);
  assert.equal(await earliest([timer(null, [])]), null);
  assert.equal(await earliest([]), null);
});

test("every timer runs on the alarm, and one that throws stops none of the others", async () => {
  const ran: number[] = [];
  await assert.rejects(runAll([timer(1, ran, true), timer(2, ran)], 50), /failed/);
  assert.deepEqual(ran, [50, 50]);
});

test("an event that changed one timer's rows only brings the alarm forward", async () => {
  assert.equal(await bringForward(timer(100, []), null), 100);
  assert.equal(await bringForward(timer(100, []), 300), 100);
  assert.equal(await bringForward(timer(300, []), 100), null);
  assert.equal(await bringForward(timer(100, []), 100), null);
  assert.equal(await bringForward(timer(null, []), 100), null);
  assert.equal(await bringForward(timer(null, []), null), null);
});

const NOW = Date.parse("2026-10-03T00:00:00.000Z");

// The relay's timers over one storage, as Relay.timers lists them.
function sources(storage: MemoryStorage): Timer[] {
  const machine = machineTimer(storage, () => {}, () => true);
  return [{ next: () => machine.next(), run: async () => {} }];
}

test("the alarm is set to the machines' earliest due", async () => {
  const storage = new MemoryStorage();
  assert.equal(await earliest(sources(storage)), null);

  await storage.put("machine:due", { "issue-1": NOW + 1000 });
  assert.equal(await earliest(sources(storage)), NOW + 1000);
});

test("the machines' timer alone lists nothing to bring the alarm forward", async () => {
  const storage = new MemoryStorage();
  await storage.put("machine:due", { "issue-1": NOW + 1000 });
  const machine = sources(storage)[0];
  assert.equal(await bringForward(machine, null), NOW + 1000);
  assert.equal(storage.lists, 0);
});
