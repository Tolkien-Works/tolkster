import assert from "node:assert/strict";
import { test } from "node:test";
import { acceptedAfter, type BaseTip, bases, bearerMatches, listening, LIVE_WITHIN_MS, moveBase, SUPERSEDED, supersede, unset, UNSET_SETTINGS } from "../src/worker/realtime.ts";
import contract from "./contract.json" with { type: "json" };
import { keys } from "./keys.ts";
import { MemoryStorage } from "./storage.ts";

class FakeSocket {
  sent: string[] = [];
  dead = false;
  send(message: string): void {
    if (this.dead) throw new Error("closed");
    this.sent.push(message);
  }
}

// The contract with the CLI: test/contract.json has a base tip, which cli/src/machine.rs parses
// into what it acts on and keeps.
test("the contract fixture's base tip has its fields", () => {
  assert.deepEqual(Object.keys(contract.baseTip).sort(), keys<BaseTip>({ type: true, repo: true, ref: true, after: true }));
});

test("the bearer secret must match exactly, and an unset secret matches nothing", async () => {
  assert.equal(await bearerMatches("Bearer s3cret", "s3cret"), true);
  assert.equal(await bearerMatches("Bearer s3cre", "s3cret"), false);
  assert.equal(await bearerMatches("Bearer s3cret ", "s3cret"), false);
  assert.equal(await bearerMatches("bearer s3cret", "s3cret"), false);
  assert.equal(await bearerMatches("s3cret", "s3cret"), false);
  assert.equal(await bearerMatches(null, "s3cret"), false);
  assert.equal(await bearerMatches("Bearer ", ""), false);
  assert.equal(await bearerMatches("Bearer undefined", undefined as unknown as string), false);
});

test("a base tip is kept per repo, sent on a move and to each connection, and never queued", async () => {
  const storage = new MemoryStorage<unknown>();
  const socket = new FakeSocket();
  const tip = (after: string): BaseTip => ({ type: "base", repo: "Tolkien-Works/tolkster", ref: "refs/heads/main", after });
  assert.equal(await moveBase(storage, [socket], tip("a".repeat(40)), 100), 1);
  // A redelivery moves nothing, and sends nothing.
  assert.equal(await moveBase(storage, [socket], tip("a".repeat(40)), 100), 0);
  assert.equal(await moveBase(storage, [socket], { ...tip("b".repeat(40)), repo: "tolkien-works/TOLKSTER" }, 100), 1);
  // A push whose commit is older than the held one's is dropped; one naming no time is taken.
  assert.equal(await moveBase(storage, [socket], tip("d".repeat(40)), 50), 0);
  assert.equal(await moveBase(storage, [socket], tip("e".repeat(40)), 0), 1);
  // ...and doesn't forget the held time, so an older replay is still dropped after it.
  assert.equal(await moveBase(storage, [socket], tip("d".repeat(40)), 50), 0);
  assert.equal(await moveBase(storage, [socket], tip("b".repeat(40)), 100), 1);
  assert.deepEqual(socket.sent.map((m) => JSON.parse(m).after), ["a".repeat(40), "b".repeat(40), "e".repeat(40), "b".repeat(40)]);
  // A dead socket takes nothing: the next connection is sent every tip.
  const dead = new FakeSocket();
  dead.dead = true;
  assert.equal(await moveBase(storage, [dead], { ...tip("c".repeat(40)), repo: "o/r" }, 0), 0);
  const fresh = new FakeSocket();
  assert.equal(await bases(storage, fresh), 2);
  assert.deepEqual(fresh.sent.map((m) => JSON.parse(m) as BaseTip).map((t) => t.after).sort(), ["b".repeat(40), "c".repeat(40)]);
});

test("something is listening while a connection has pinged within 75 s, and not once every one has gone quiet", () => {
  const now = 1_000_000;
  assert.equal(listening([], now), false);
  assert.equal(listening([{ pingedAt: now - 10_000, acceptedAt: now - 3_600_000 }], now), true);
  assert.equal(listening([{ pingedAt: now - LIVE_WITHIN_MS, acceptedAt: now - 3_600_000 }], now), true);
  // A half-open connection: the runtime still lists it, but nothing has pinged since listen's own threshold.
  assert.equal(listening([{ pingedAt: now - LIVE_WITHIN_MS - 1, acceptedAt: now - 3_600_000 }], now), false);
  // One live connection among dead ones is enough: `listen --print` beside the `listen` that acts, say.
  assert.equal(listening([{ pingedAt: now - 600_000, acceptedAt: now - 3_600_000 }, { pingedAt: now - 20_000, acceptedAt: now - 3_600_000 }], now), true);
});

test("a connection just accepted counts before its first ping", () => {
  const now = 1_000_000;
  // listen's first ping comes 30 s after connecting: the accept stands in for it until then.
  assert.equal(listening([{ pingedAt: null, acceptedAt: now - 10_000 }], now), true);
  assert.equal(listening([{ pingedAt: null, acceptedAt: now - LIVE_WITHIN_MS - 1 }], now), false);
});

test("a newer acting connection closes every older one but a --print, with the pinned code", () => {
  assert.equal(SUPERSEDED, contract.supersededClose);
  class Closing {
    closed: [number, string] | null = null;
    dead = false;
    close(code: number, reason: string): void {
      if (this.dead) throw new Error("closed");
      this.closed = [code, reason];
    }
  }
  const [older, olderPrint, dead, accepted, newer] = [new Closing(), new Closing(), new Closing(), new Closing(), new Closing()];
  dead.dead = true;
  const sockets = [{ socket: older, print: false, acceptedAt: 1 }, { socket: olderPrint, print: true, acceptedAt: 1 }, { socket: dead, print: false, acceptedAt: 1 }, { socket: accepted, print: false, acceptedAt: 5 }, { socket: newer, print: false, acceptedAt: 9 }];
  // The connection accepted at 5 closes the older ones only: not itself, not a newer one that overlapped it.
  assert.equal(supersede(sockets, 5), 1);
  assert.deepEqual(older.closed, [SUPERSEDED, "Superseded"]);
  assert.deepEqual([olderPrint.closed, dead.closed, accepted.closed, newer.closed], [null, null, null, null]);
});

test("a connection accepted in the same millisecond as a held one is still later", () => {
  assert.equal(acceptedAfter(100, []), 100);
  assert.equal(acceptedAfter(100, [40, 100]), 101);
  assert.equal(acceptedAfter(100, [40]), 100);
});

test("an unset setting is refused with a 503 naming it, from the pinned set", async () => {
  assert.deepEqual([...UNSET_SETTINGS], contract.unsetSettings);
  for (const name of UNSET_SETTINGS) {
    const response = unset(name);
    assert.equal(response.status, 503);
    assert.deepEqual(await response.json(), { missing: [name] });
  }
});
