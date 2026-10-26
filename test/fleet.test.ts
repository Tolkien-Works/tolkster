import assert from "node:assert/strict";
import { test } from "node:test";
import { FLEET_KEY, loadFleet, parseHosts, roomOf, slotTotal, type Fleet, type Settings } from "../src/worker/machine/fleet.ts";
import { fleetReported, loadCoordinator } from "../src/worker/machine/store.ts";
import { parseReport } from "../src/worker/machine.ts";
import contract from "./contract.json" with { type: "json" };
import { MemoryStorage } from "./storage.ts";

const NOW = 1_790_000_000_000;
const settings = (cap: number): Settings => ({ cap, slots: {}, repos: {}, machines: {} });

// cli/src/hosts/tests.rs writes each case with what `hosts::total` gives: this machine alone over
// its cap, a paired host, an asleep machine holding a worker, a host over its cap beside one with
// no project ready.
test("the contract's fleet cases total as listen's hosts::total does", () => {
  for (const [i, c] of contract.fleet.entries()) {
    const hosts = parseHosts(c.fleet);
    assert.deepEqual(hosts, c.fleet, `case ${i}`);
    assert.equal(slotTotal({ settings: settings(c.cap), hosts }), c.total, `case ${i}`);
  }
  // Nothing until a config report says this machine's cap; at most MAX_SLOTS.
  assert.equal(slotTotal({ settings: null, hosts: contract.fleet[1].fleet }), null);
  assert.equal(slotTotal({ settings: settings(100), hosts: contract.fleet[1].fleet }), 100);
});

test("the fleet holds the settings and every paired host, and the slots follow its total", async () => {
  const storage = new MemoryStorage();
  const sender = () => {};
  const config = parseReport(contract.reports.find((r) => r.report === "config"));
  const listing = parseReport(contract.reports.find((r) => r.report === "listing"));
  assert.ok(config?.report === "config" && listing?.report === "listing" && listing.fleet !== null);
  // A listing before any config report is kept, and leaves the slots as they were.
  await fleetReported(storage, { hosts: listing.fleet }, NOW, sender);
  const before = await loadCoordinator(storage);
  const { report: _, ...said } = config;
  await fleetReported(storage, { settings: said }, NOW, sender);
  const fleet: Fleet = await loadFleet(storage);
  assert.deepEqual(fleet, { settings: said, hosts: listing.fleet });
  assert.deepEqual([fleet.hosts?.hosts.map((h) => h.id), Object.keys(fleet.settings?.machines ?? {})], [["mac", "pi"], ["tolkster-1"]]);
  // 6 here, mac's 2 with 1 held, pi silent holding none.
  assert.equal((await loadCoordinator(storage)).slots, 8);
  assert.notEqual(before.slots, 8);
  // A listing that leaves the total as it is writes no coordinator step; one that moves it does.
  const rev = (await loadCoordinator(storage)).rev;
  await fleetReported(storage, { hosts: { ...listing.fleet, held: 2 } }, NOW, sender);
  assert.equal((await loadCoordinator(storage)).rev, rev);
  await fleetReported(storage, { hosts: { ...listing.fleet, held: 7 } }, NOW, sender);
  assert.deepEqual([(await loadCoordinator(storage)).slots, (await storage.get<Fleet>(FLEET_KEY))?.hosts?.held], [9, 7]);
});

test("a slot goes only to a host that answered, holds a credential the Durable Object admitted and isn't being put to sleep", () => {
  const heard = (id: string, answering: boolean) => ({ id, name: id, cap: 2, admitted: true, answering, ready: ["github:acme/app"], held: 1 });
  const fleet: Fleet = { settings: settings(3), hosts: { held: 2, hosts: [heard("mac", true), heard("pi", true), heard("nuc", true), heard("old", false)], projects: { app: "github:acme/app" } } };
  assert.deepEqual(roomOf(fleet, ["mac", "nuc", "old"], ["nuc"]), { cap: 3, held: 2, hosts: [{ id: "mac", cap: 2, held: 1, ready: ["github:acme/app"] }], silent: ["old"], projects: { app: "github:acme/app" } });
  assert.deepEqual(roomOf({ settings: null, hosts: null }, [], []), { cap: null, held: 0, hosts: [], silent: [], projects: {} });
});
