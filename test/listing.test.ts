import assert from "node:assert/strict";
import { test } from "node:test";
import { applyListing, BACKGROUND_HOLD_MS, fromListing, HOOK_AT, HOOK_AT_KEEP_MS, type HookTime, keepHook, keepListing, keptListing, WORKTREE } from "../src/worker/listing.ts";
import type { HookReport, Listing } from "../src/worker/machine.ts";
import { deliver, loadCoordinator, loadStore } from "../src/worker/machine/store.ts";
import type { AgentStatus, Command, Store } from "../src/worker/machine/types.ts";
import { newCoordinator } from "../src/worker/machine/coordinator.ts";
import { given, ISSUE, MIN, NOW, result, S } from "./machine-fixture.ts";
import { handle, MISSING_KEEP_MS } from "../src/worker/state.ts";
import { MemoryStorage } from "./storage.ts";

const WT = "repo::C:/w/tw-1";
const OTHER = "repo::C:/w/tw-0";
const AT = "2026-10-01T03:00:00.000Z";
const listing = (agents: Listing["agents"], listed: string[] | null = null, at = AT, hosts: string[] = [], elapsedMs = 0, paired: string[] = hosts): Listing => ({ report: "listing", at, elapsedMs, agents, listed, hosts, paired, idle: {}, dormant: null, fleet: null });
const hook = (at: string, elapsedMs: number): HookReport => ({ report: "agentIdle", worktreeId: WT, at, elapsedMs });
const sender = (_: Command[]) => {};
const BEGAN = Date.parse(AT);
// A listing that began at `began` by the Durable Object's clock.
const from = (l: Listing, worktree: string, seen: number | null, agent: AgentStatus | null, began = BEGAN) => fromListing({ listing: l, began }, worktree, ISSUE, seen, agent);

async function held(agent: AgentStatus | null, worktrees: string[] = [WT]): Promise<MemoryStorage> {
  const storage = new MemoryStorage();
  const s: Store = given({ work: "running", facts: { agent: agent === null ? null : { status: agent, sinceMs: NOW - 1000 } } });
  await storage.put(`machine:issue:${ISSUE}`, s);
  for (const id of worktrees) await storage.put(WORKTREE + id, ISSUE);
  return storage;
}

const status = async (storage: MemoryStorage) => (await loadStore(storage, ISSUE)).facts.agent?.status ?? null;

test("a listing's state of a worktree is its event, when the machine holds another", () => {
  assert.deepEqual(from(listing({ [WT]: "idle" }), WT, null, "working"), { id: `listing:${AT}`, issue: ISSUE, kind: "agentIdle" });
  assert.equal(from(listing({ [WT]: "idle" }), WT, null, "idle"), null);
  assert.equal(from(listing({ [WT]: "waiting" }), WT, null, null)?.kind, "agentWaiting");
});

test("a worktree with no agent listed has exited, only once the machine has seen its agent or its hook ran with no launch in flight", () => {
  assert.equal(from(listing({}), WT, null, "working")?.kind, "agentExited");
  assert.equal(from(listing({}, [WT]), WT, null, "idle")?.kind, "agentExited");
  for (const agent of [null, "exited", "gone"] as const) assert.equal(from(listing({}), WT, null, agent), null, String(agent));
  assert.equal(from(listing({}, [WT]), WT, BEGAN - 60_000, null)?.kind, "agentExited");
  assert.equal(fromListing({ listing: listing({}, [WT]), began: BEGAN }, WT, ISSUE, BEGAN - 60_000, null, true), null);
});

test("a worktree a complete listing lacks is left to worktreeGone", () => {
  assert.equal(from(listing({}, [OTHER]), WT, null, "working"), null);
});

test("a listing says nothing of a worktree on a host that didn't answer it, or isn't paired", () => {
  const MAC = "mac@r9::/Users/x/tw-1";
  assert.equal(from(listing({}, null, AT, ["linux"]), MAC, null, "working"), null);
  assert.equal(from(listing({}, null, AT, ["mac"]), MAC, null, "working")?.kind, "agentExited");
  assert.equal(from(listing({ [MAC]: "idle" }, [MAC], AT, ["mac"]), MAC, null, "working")?.kind, "agentIdle");
});

test("an agent its hook said exited, still listed soon after, is its pane not yet gone", () => {
  assert.equal(from(listing({ [WT]: "working" }), WT, BEGAN - 60_000, "exited"), null);
  assert.equal(from(listing({ [WT]: "working" }), WT, BEGAN - 120_001, "exited")?.kind, "agentWorking");
  assert.equal(from(listing({ [WT]: "working" }), WT, null, "exited")?.kind, "agentWorking");
});

test("an agent its hook said waits in the background, listed idle, runs until the hold ends", async () => {
  assert.equal(from(listing({ [WT]: "idle" }), WT, BEGAN - BACKGROUND_HOLD_MS + 1, "background"), null);
  assert.equal(from(listing({ [WT]: "idle" }), WT, BEGAN - BACKGROUND_HOLD_MS, "background")?.kind, "agentIdle");
  // Working, waiting or gone from the listing still apply at once.
  assert.equal(from(listing({ [WT]: "working" }), WT, BEGAN - 60_000, "background")?.kind, "agentWorking");
  assert.equal(from(listing({ [WT]: "waiting" }), WT, BEGAN - 60_000, "background")?.kind, "agentWaiting");
  assert.equal(from(listing({}), WT, BEGAN - 60_000, "background")?.kind, "agentExited");
  // Through the machine: the worker stays running, with no paused deadline, until the hold ends.
  const storage = await held("working");
  const ran = new Date(NOW).toISOString();
  assert.equal(await keepHook(storage, { report: "agentBackground", worktreeId: WT, at: ran, elapsedMs: 0 }, NOW), true);
  await deliver(storage, { id: `hook:agentBackground:${ran}`, issue: ISSUE, kind: "agentBackground" }, NOW, sender);
  const later = (min: number) => listing({ [WT]: "idle" }, null, new Date(NOW + min * 60_000).toISOString());
  await keepListing(storage, later(29), 1, NOW + 29 * 60_000, sender);
  assert.deepEqual([(await loadStore(storage, ISSUE)).work?.state, await status(storage)], ["running", "background"]);
  await keepListing(storage, later(31), 1, NOW + 31 * 60_000, sender);
  assert.deepEqual([(await loadStore(storage, ISSUE)).work?.state, await status(storage)], ["idle", "idle"]);
});

test("a hook that ran at or after the listing began wins over it", () => {
  assert.equal(from(listing({ [WT]: "working" }), WT, BEGAN, "idle"), null);
  assert.equal(from(listing({ [WT]: "working" }), WT, BEGAN + 1, "idle"), null);
  assert.equal(from(listing({ [WT]: "working" }), WT, BEGAN - 1, "idle")?.kind, "agentWorking");
});

// A host's clock a minute off, either way, changes `at` and nothing else: the Durable Object
// sets a hook against a listing by when each began, receipt less `elapsedMs`.
for (const skew of [60_000, -60_000]) {
  test(`a hook from a host ${Math.abs(skew) / 1000}s ${skew > 0 ? "ahead" : "behind"} is ordered against a listing by when each ran`, async () => {
    const ran = (agoMs: number) => new Date(NOW - agoMs + skew).toISOString();
    // The hook ran 1s before it was received; a listing began 3s after, and says the agent is gone.
    const before = await held("working");
    assert.equal(await keepHook(before, hook(ran(1000), 1000), NOW), true);
    await keepListing(before, listing({}, null, AT, [], 2000), 1, NOW + 5000, sender);
    assert.equal(await status(before), "exited");
    // The listing began 2s before the hook ran, so the hook is the newer, and the listing says nothing of it.
    const after = await held("working");
    assert.equal(await keepHook(after, hook(ran(500), 500), NOW), true);
    await keepListing(after, listing({}, null, AT, [], 2500), 1, NOW, sender);
    assert.equal(await status(after), "working");
  });

  test(`a late-landing older hook from a host ${Math.abs(skew) / 1000}s ${skew > 0 ? "ahead" : "behind"} loses to the newer, and a newer one wins`, async () => {
    const storage = await held("working");
    const at = (ms: number) => new Date(NOW + ms + skew).toISOString();
    assert.equal(await keepHook(storage, hook(at(0), 100), NOW), true);
    // Received 30s later than the newer one, though it ran 2s before it: by the host's clock it is older.
    assert.equal(await keepHook(storage, hook(at(-2000), 100), NOW + 30_000), false);
    assert.deepEqual(await storage.get<HookTime>(HOOK_AT + WT), { at: at(0), seen: NOW - 100 });
    assert.equal(await keepHook(storage, hook(at(1000), 100), NOW + 31_000), true);
    assert.deepEqual(await storage.get<HookTime>(HOOK_AT + WT), { at: at(1000), seen: NOW + 30_900 });
  });
}

test("after a restart, the first listing catches a worker that exited while nothing listened", async () => {
  const storage = await held("working");
  assert.equal(await keepListing(storage, listing({}), 1, NOW, sender), true);
  assert.equal(await status(storage), "exited");
  assert.deepEqual(await keptListing(storage), { listing: listing({}), began: NOW });
});

// An agent that works and exits while its start's result is owed: its hooks name a worktree
// no index holds yet, so they reach no machine, and its PR merges meanwhile.
test("a start answered after its agent exited leaves the agent states as its result lands: merged, reaped, its slot given back", async () => {
  const storage = new MemoryStorage();
  const merged = { number: 7, head: "0123456789abcdef0123456789abcdef01234567", repo: "o/r", status: "merged" as const, at: AT, receivedMs: NOW - 8 * MIN };
  const start = { kind: "startWorker" as const, machine: "work", params: { session: S, orchestrate: false, worktree: null, host: null }, sentMs: NOW - 30 * MIN, firstSentMs: NOW - 30 * MIN };
  await storage.put(`machine:issue:${ISSUE}`, given({ work: "launching", facts: { worktree: null, agent: null, pr: merged }, queue: [start] }));
  await storage.put("machine:coordinator", { ...newCoordinator(), grants: { [ISSUE]: { session: null, sinceMs: NOW - 30 * MIN, repo: "repo" } } });
  for (const [report, ago] of [["agentWorking", 20], ["agentExited", 10]] as const) {
    assert.equal(await keepHook(storage, { report, worktreeId: WT, at: new Date(NOW - ago * MIN).toISOString(), elapsedMs: 0 }, NOW - ago * MIN), true);
  }
  await keepListing(storage, listing({}, [WT], new Date(NOW - 5 * MIN).toISOString()), 1, NOW - 5 * MIN, sender);
  // As the relay lands a start's result: its worktree indexed, then the kept listing applied.
  const sent: Command[] = [];
  const record = (c: Command[]) => void sent.push(...c);
  await storage.put(WORKTREE + WT, ISSUE);
  await deliver(storage, result(`${ISSUE}.1`, "ok", { worktree: { id: WT, branch: "tw-1" }, madeWorktree: false, launched: false }), NOW, record);
  await applyListing(storage, (await keptListing(storage))!, WT, ISSUE, NOW, record);
  const after = await loadStore(storage, ISSUE);
  assert.deepEqual([after.work?.state, after.queue.map((c) => c.kind)], ["merged", ["reapWorktree"]]);
  assert.equal((await loadCoordinator(storage)).grants[ISSUE], undefined);
});

// A start lost after its agent ran leaves that agent's hook; the relaunch on its worktree is
// watched for its first turn, which a listing from before it lists no agent of.
test("a hook from before a watched launch says nothing of the agent it launched, and that agent's own hook lets its exit through", async () => {
  const storage = new MemoryStorage();
  await storage.put(`machine:issue:${ISSUE}`, given({ work: "running", facts: { agent: null }, w: { firstTurn: NOW - 5 * MIN } }));
  await storage.put(WORKTREE + WT, ISSUE);
  await keepHook(storage, { report: "agentExited", worktreeId: WT, at: new Date(NOW - 20 * MIN).toISOString(), elapsedMs: 0 }, NOW - 20 * MIN);
  await keepListing(storage, listing({}, [WT], new Date(NOW - 10 * MIN).toISOString()), 1, NOW - 10 * MIN, sender);
  assert.equal((await loadStore(storage, ISSUE)).work?.state, "running");
  // The launched agent's own hook, then a listing with no agent: its exit.
  await keepHook(storage, { report: "agentExited", worktreeId: WT, at: new Date(NOW - 2 * MIN).toISOString(), elapsedMs: 0 }, NOW - 2 * MIN);
  await keepListing(storage, listing({}, [WT], new Date(NOW - MIN).toISOString()), 1, NOW - MIN, sender);
  assert.notEqual((await loadStore(storage, ISSUE)).work?.state, "running");
});

test("a listing from a connection older than the kept one's is dropped, and applies nothing", async () => {
  const storage = await held("working");
  assert.equal(await keepListing(storage, listing({ [WT]: "working" }), 2, NOW, sender), true);
  assert.equal(await keepListing(storage, listing({}, null, "2026-10-01T04:00:00.000Z"), 1, NOW, sender), false);
  assert.equal(await status(storage), "working");
  assert.equal((await keptListing(storage))?.listing.agents[WT], "working");
  // On one connection the last sent is the newest, whatever its time says.
  assert.equal(await keepListing(storage, listing({}, null, "2026-10-01T02:00:00.000Z"), 2, NOW, sender), true);
  assert.equal(await status(storage), "exited");
});

test("a listing says nothing of an issue's old worktree, nor of one a newer hook reported", async () => {
  const storage = await held("working", [OTHER]);
  await keepListing(storage, listing({ [OTHER]: "idle" }), 1, NOW, sender);
  assert.equal(await status(storage), "working");
  const hooked = await held("working");
  await hooked.put(HOOK_AT + WT, { at: "2026-10-01T03:00:01.000Z", seen: NOW + 1 } satisfies HookTime);
  await keepListing(hooked, listing({}), 1, NOW, sender);
  assert.equal(await status(hooked), "working");
});

// TW-486: the listing saw the new worker's agent before its start's result indexed the worktree.
test("the kept listing applies to a worktree indexed after it came", async () => {
  const storage = await held(null, []);
  await keepListing(storage, listing({ [WT]: "working" }), 1, NOW, sender);
  assert.equal(await status(storage), null);
  await storage.put(WORKTREE + WT, ISSUE);
  await applyListing(storage, (await keptListing(storage))!, WT, ISSUE, NOW, sender);
  assert.equal(await status(storage), "working");
});

test("a worker's worktree missing from complete listings for MISSING_KEEP_MS is gone", async () => {
  const storage = await held("working");
  const worker = { issue: "TW-1", worktreeId: WT, sessionId: "s-1", at: AT, launch: { harness: "claude", model: "claude-opus-5-5", effort: "medium" }, rungFrom: "claim" };
  assert.equal((await handle(storage, { op: "startWorker", worker }, NOW)).status, 200);
  const work = async () => (await loadStore(storage, ISSUE)).work?.state;
  const missing = async () => (await storage.get<{ missingMs?: number }>(`worker:${WT}`))?.missingMs;
  const recorded = async () => storage.data.has(`worker:${WT}`);
  // A partial listing never counts, however late it comes.
  await keepListing(storage, listing({}, null), 1, NOW, sender);
  await keepListing(storage, listing({}, null), 1, NOW + 2 * MISSING_KEEP_MS, sender);
  assert.equal(await missing(), undefined);
  // Missing from a complete one, and back in the next: the hour starts again.
  await keepListing(storage, listing({}, [OTHER]), 1, NOW, sender);
  assert.equal(await missing(), NOW);
  await keepListing(storage, listing({ [WT]: "working" }, [WT]), 1, NOW + 1000, sender);
  assert.equal(await missing(), undefined);
  // Back only after the hour, as when listen was away: cleared before anything is said.
  await keepListing(storage, listing({}, [OTHER]), 1, NOW + 1000, sender);
  await keepListing(storage, listing({ [WT]: "working" }, [WT]), 1, NOW + 1001 + MISSING_KEEP_MS, sender);
  assert.equal(await missing(), undefined);
  assert.notEqual(await work(), "reaped");
  await keepListing(storage, listing({}, [OTHER]), 1, NOW + 2000, sender);
  // Time alone says nothing, nor a partial listing after the hour, nor a complete one within it.
  await keepListing(storage, listing({}, null), 1, NOW + 2001 + MISSING_KEEP_MS, sender);
  await keepListing(storage, listing({}, [OTHER]), 1, NOW + 2000 + MISSING_KEEP_MS, sender);
  assert.notEqual(await work(), "reaped");
  // A complete listing after the hour, still without it, says it is gone.
  await keepListing(storage, listing({}, [OTHER]), 1, NOW + 2001 + MISSING_KEEP_MS, sender);
  assert.equal(await work(), "reaped");
  // Said once: its record is deleted.
  assert.equal(await recorded(), false);
});

test("a worker on a host the listing leaves out of the paired is gone at once, one on a paired host that is silent is not", async () => {
  const MAC = "mac@r9::/Users/x/tw-1";
  const storage = new MemoryStorage();
  await storage.put(`machine:issue:${ISSUE}`, given({ work: "running", facts: { worktree: { id: MAC, branch: "tw-1", version: 1 }, agent: { status: "working", sinceMs: NOW - 1000 } } }));
  await storage.put(WORKTREE + MAC, ISSUE);
  const worker = { issue: "TW-1", worktreeId: MAC, sessionId: "s-1", at: AT, launch: { harness: "claude", model: "claude-opus-5-5", effort: "medium" }, rungFrom: "claim" };
  assert.equal((await handle(storage, { op: "startWorker", worker }, NOW)).status, 200);
  const work = async () => (await loadStore(storage, ISSUE)).work?.state;
  // Silent, and paired: unknown, whether the listing is complete or not.
  await keepListing(storage, listing({}, null, AT, [], 0, ["mac"]), 1, NOW, sender);
  await keepListing(storage, listing({}, [], AT, [], 0, ["mac"]), 1, NOW + 2 * MISSING_KEEP_MS, sender);
  assert.equal(await work(), "running");
  assert.equal(storage.data.has(`worker:${MAC}`), true);
  // Not among the paired, answering or not: gone in that listing, though it is incomplete and no hour has passed.
  await keepListing(storage, listing({}, null, AT, [], 0, ["linux"]), 1, NOW + 2 * MISSING_KEEP_MS, sender);
  assert.equal(await work(), "reaped");
  assert.equal(storage.data.has(`worker:${MAC}`), false);
  assert.equal(storage.data.has(WORKTREE + MAC), false);
});

test("a gone worktree that is no longer its issue's is forgotten without an event", async () => {
  const storage = await held("working", [OTHER]);
  await handle(storage, { op: "startWorker", worker: { issue: "TW-1", worktreeId: OTHER, sessionId: "s-1", at: AT, launch: { harness: "claude", model: "claude-opus-5-5", effort: "medium" }, rungFrom: "claim" } }, NOW);
  await keepListing(storage, listing({ [WT]: "working" }, [WT]), 1, NOW, sender);
  await keepListing(storage, listing({ [WT]: "working" }, [WT]), 1, NOW + MISSING_KEEP_MS + 1, sender);
  assert.equal((await loadStore(storage, ISSUE)).work?.state, "running");
  assert.equal(storage.data.has(`worker:${OTHER}`), false);
});

test("a worktree's index entries go in the write that drops it from its issue, and stay while it is null after a lost start", async () => {
  const storage = await held("working", [WT, OTHER]);
  await storage.put(HOOK_AT + WT, { at: AT, seen: NOW } satisfies HookTime);
  await storage.put(HOOK_AT + OTHER, { at: AT, seen: NOW } satisfies HookTime);
  await deliver(storage, { id: "gone:1", issue: ISSUE, kind: "worktreeGone" }, NOW, sender);
  assert.equal((await loadStore(storage, ISSUE)).facts.worktree, null);
  assert.deepEqual([...storage.data.keys()].filter((k) => k.startsWith(WORKTREE) || k.startsWith(HOOK_AT)).sort(), [HOOK_AT + OTHER, WORKTREE + OTHER]);
  // A listing walks the entry of an issue with no worktree fact: a start's result may still name it.
  await keepListing(storage, listing({}), 1, NOW, sender);
  assert.ok(storage.data.has(WORKTREE + OTHER));
});

test("a listing drops what an issue that names another worktree left in the index, and hooks nothing holds go after an hour", async () => {
  const storage = await held("working", [WT, OTHER]);
  await storage.put(HOOK_AT + OTHER, { at: AT, seen: NOW } satisfies HookTime);
  await storage.put(HOOK_AT + "repo::C:/w/late", { at: AT, seen: NOW } satisfies HookTime);
  await keepListing(storage, listing({ [WT]: "working" }), 1, NOW, sender);
  assert.deepEqual([...storage.data.keys()].filter((k) => k.startsWith(WORKTREE) || k.startsWith(HOOK_AT)).sort(), [HOOK_AT + "repo::C:/w/late", WORKTREE + WT]);
  await keepListing(storage, listing({ [WT]: "working" }), 1, NOW + HOOK_AT_KEEP_MS + 1, sender);
  assert.equal(storage.data.has(HOOK_AT + "repo::C:/w/late"), false);
});
