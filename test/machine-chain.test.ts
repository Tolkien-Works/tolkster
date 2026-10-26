// A chain more than one level deep, through the stores as the Durable Object runs them: a
// root, a middle issue its chain delegates, and the middle issue's own sub-issues
// (docs/parent-issues.md).

import assert from "node:assert/strict";
import { test } from "node:test";
import { receive } from "../src/worker/machine/step.ts";
import { deliver, loadStore } from "../src/worker/machine/store.ts";
import type { Command, DelegatedBy, Inbound, SessionKind, Store } from "../src/worker/machine/types.ts";
import { NOW } from "./machine-fixture.ts";
import { MemoryStorage } from "./storage.ts";

const ROOT = "a0000000-0000-4000-8000-000000000000";
const MID = "a1000000-0000-4000-8000-000000000000";
const G1 = "a2000000-0000-4000-8000-000000000000";
const G2 = "a3000000-0000-4000-8000-000000000000";
// Added under the middle issue once it has chained, and a parent it moves to.
const G3 = "a4000000-0000-4000-8000-000000000000";
const OTHER = "a5000000-0000-4000-8000-000000000000";
const SESSION: Record<string, string> = { [ROOT]: "b0000000-0000-4000-8000-000000000000", [MID]: "b1000000-0000-4000-8000-000000000000", [G1]: "b2000000-0000-4000-8000-000000000000", [G2]: "b3000000-0000-4000-8000-000000000000" };

// Linear as the stores read it: each issue's delegate and state, by the clock they changed at.
type Issue = { delegate: DelegatedBy; state: "open" | "completed" | "cancelled"; at: string };

// The stores, with `listen` answering every command and Linear's webhooks for what changes.
class World {
  storage = new MemoryStorage();
  sent: Command[] = [];
  nowMs = NOW;
  seq = 0;
  answered = 0;
  linear: Record<string, Issue> = {};
  parents: Record<string, string | null> = { [ROOT]: null, [MID]: ROOT, [G1]: MID, [G2]: MID, [G3]: null, [OTHER]: null };
  children: Record<string, string[]> = { [ROOT]: [MID], [MID]: [G1, G2], [G1]: [], [G2]: [], [G3]: [], [OTHER]: [] };
  blockers: Record<string, string[]> = {};
  // Issues whose reads say `created`: the owner created them delegated, no history entry names
  // a delegation, and no record of the chain's was read.
  unnamed = new Set<string>();

  constructor() {
    for (const id of Object.keys(this.parents)) this.linear[id] = { delegate: null, state: "open", at: this.iso() };
  }

  iso(): string {
    return new Date(this.nowMs).toISOString();
  }

  async send(e: Record<string, unknown>): Promise<void> {
    this.nowMs += 1000;
    this.seq += 1;
    await deliver(this.storage, { id: `e${this.seq}`, ...e }, this.nowMs, (c) => this.sent.push(...c));
  }

  // The newest command of `kind` sent for `issue`.
  last(issue: string, kind: Command["kind"]): Command | undefined {
    return this.sent.findLast((c) => c.issue === issue && c.kind === kind);
  }

  store(issue: string): Promise<Store> {
    return loadStore(this.storage, issue);
  }

  read(issue: string): object {
    const l = this.linear[issue];
    const parent = this.parents[issue];
    const p = parent === null ? null : { id: parent, ...this.linear[parent] };
    return {
      read: {
        updatedAt: this.iso(), delegate: this.unnamed.has(issue) && l.delegate !== null ? "created" : l.delegate, state: l.state, triage: false, stateChangedAt: l.at, priority: 0,
        blockers: (this.blockers[issue] ?? []).map((b) => ({ id: b, done: this.linear[b].state !== "open", updatedAt: this.linear[b].at })), moreBlockers: false,
        parent: p === null ? null : { id: p.id, delegated: p.delegate === "owner" || p.delegate === "chain", by: p.delegate, state: p.state, priority: 0, updatedAt: p.at },
        children: this.children[issue].map((c) => ({ id: c, done: this.linear[c].state !== "open", triage: false, updatedAt: this.linear[c].at })),
        moreChildren: false, repo: "repo", sessions: l.delegate === null ? [] : [{ id: SESSION[issue], kind: kindOf(issue), createdAt: l.at }],
      },
    };
  }

  async result(c: Command, data?: object): Promise<void> {
    await this.send({ kind: "commandResult", issue: c.issue, command: c.id, result: "ok", data });
  }

  // Answers every command sent and not yet answered, as `listen` would, until none is left.
  async settle(): Promise<void> {
    for (; this.answered < this.sent.length; this.answered++) {
      const c = this.sent[this.answered];
      if (c.kind === "readIssue") await this.result(c, this.read(c.issue));
      else if (c.kind === "startWorker" || c.kind === "resumeWorker") {
        await this.result(c, { worktree: { id: `repo::C:/w/${c.issue}`, branch: c.issue }, madeWorktree: c.kind === "startWorker", launched: true });
        await this.send({ kind: "agentWorking", issue: c.issue });
      } else await this.result(c, {});
    }
  }

  async delegate(issue: string, by: DelegatedBy): Promise<void> {
    this.linear[issue] = { ...this.linear[issue], delegate: by, at: this.iso() };
    await this.send({ kind: "delegateChanged", issue, by, at: this.iso() });
    if (by === "owner" || by === "chain") await this.send({ kind: "sessionStarted", issue, session: SESSION[issue], sessionKind: kindOf(issue), byOwner: by === "owner", at: this.iso() });
  }

  async state(issue: string, state: Issue["state"]): Promise<void> {
    this.linear[issue] = { ...this.linear[issue], state, at: this.iso() };
    await this.send({ kind: "issueStateChanged", issue, state, parent: this.parents[issue], at: this.iso() });
  }

  // `issue` moved under `to`, or created there.
  async move(issue: string, to: string): Promise<void> {
    const from = this.parents[issue];
    if (from !== null) this.children[from] = this.children[from].filter((c) => c !== issue);
    this.children[to].push(issue);
    this.parents[issue] = to;
    this.linear[issue] = { ...this.linear[issue], at: this.iso() };
    await this.send({ kind: "issueStateChanged", issue, state: "open", parent: to, ...(from === null ? {} : { from }), at: this.iso() });
  }

  // The worker reads its request, as `tolkster messages` reports it.
  async reads(issue: string): Promise<void> {
    await this.send({ kind: "workerRead", issue, session: SESSION[issue], reader: "worker", through: this.iso() });
  }

  // The orchestrator's `issue chain`, then its delegations, as the CLI makes them.
  async chains(issue: string, run: string): Promise<void> {
    await this.send({ kind: "chainRecorded", issue, role: "ask", session: SESSION[issue], run, children: this.children[issue] });
    for (const c of this.children[issue]) await this.delegate(c, "chain");
  }

  async answersAndExits(issue: string): Promise<void> {
    await this.send({ kind: "workerAnswered", issue, session: SESSION[issue], reader: "worker", at: this.iso() });
    await this.send({ kind: "agentExited", issue });
  }
}

function kindOf(issue: string): SessionKind {
  return issue === ROOT ? "delegation" : "chain";
}

// Up to the grandchildren's workers running, under the middle issue's chain, with the
// root's and the middle issue's orchestrators exited.
async function nested(): Promise<World> {
  const w = new World();
  await w.delegate(ROOT, "owner");
  await w.settle();
  await w.reads(ROOT);
  await w.chains(ROOT, "r-root");
  await w.settle();
  await w.answersAndExits(ROOT);
  await w.reads(MID);
  await w.chains(MID, "r-mid");
  await w.settle();
  await w.answersAndExits(MID);
  for (const g of [G1, G2]) await w.reads(g);
  await w.settle();
  return w;
}

test("a three-level chain runs to the end: the middle issue orchestrates, and it and the root each pick up once their sub-issues are done", async () => {
  const w = await nested();
  for (const [issue, orchestrate] of [[ROOT, true], [MID, true], [G1, false], [G2, false]] as const) {
    assert.equal(w.last(issue, "startWorker")?.params.orchestrate, orchestrate, issue);
    assert.equal((await w.store(issue)).work?.state, orchestrate ? "rest" : "running", issue);
  }
  assert.deepEqual((await w.store(MID)).facts.chain, { parent: ROOT, run: "r-root" });
  assert.deepEqual((await w.store(G1)).facts.chain, { parent: MID, run: "r-mid" });
  // One grandchild's end picks up nothing; both bring back the middle issue, not the root.
  await w.state(G1, "completed");
  await w.answersAndExits(G1);
  await w.settle();
  assert.equal(w.last(MID, "resumeWorker"), undefined);
  await w.state(G2, "completed");
  await w.answersAndExits(G2);
  await w.settle();
  assert.deepEqual(w.last(MID, "resumeWorker")?.params, { session: SESSION[MID], childrenEnd: true, childrenAdded: false });
  assert.equal(w.last(ROOT, "resumeWorker"), undefined);
  // The middle issue finishes, which brings back the root.
  await w.reads(MID);
  await w.state(MID, "completed");
  await w.answersAndExits(MID);
  await w.settle();
  assert.deepEqual(w.last(ROOT, "resumeWorker")?.params, { session: SESSION[ROOT], childrenEnd: true, childrenAdded: false });
  assert.equal(w.sent.filter((c) => c.kind === "resumeWorker").length, 2);
});

test("cancelling the root, or taking it from Tolkster, halts the grandchildren too, one whose chain answer is still on its way included", async () => {
  for (const how of ["cancelled", "taken"]) {
    const w = await nested();
    // G2's answer to the middle issue's chain hasn't landed yet: only G1 counts as taken.
    const mid = await w.store(MID);
    mid.facts.chained = { ...mid.facts.chained!, issues: [G1], answered: [G1] };
    await w.storage.put(`machine:issue:${MID}`, mid);
    if (how === "cancelled") await w.state(ROOT, "cancelled");
    else await w.delegate(ROOT, null);
    await w.settle();
    assert.deepEqual(w.last(ROOT, "takeBackChain")?.params, { run: "r-root" }, how);
    assert.deepEqual(w.last(MID, "takeBackChain")?.params, { run: "r-mid" }, how);
    assert.equal((await w.store(G1)).work?.state, "halted", how);
    assert.equal((await w.store(G1)).facts.chain, null, how);
    // The middle issue is still delegated in Linear until its take-back lands, which keeps no
    // grandchild running: the late answer, which only a store sends, halts G2 as it lands.
    assert.equal((await w.store(G2)).work?.state, "running", how);
    const landed = receive(await w.store(MID), { kind: "chainRecorded", id: "late", issue: MID, role: "answer", child: G2, run: "r-mid", took: true }, w.nowMs).store;
    const halt = landed.outbox.find((o) => o.to === G2)?.event;
    assert.equal(halt?.kind, "parentHalted", how);
    const g2 = receive(await w.store(G2), halt as Inbound, w.nowMs).store;
    assert.equal(g2.work?.state, "halted", how);
    assert.ok(g2.queue.some((c) => c.kind === "stopWorker"), how);
  }
});

test("a sub-issue added under the middle issue once it has chained is picked up by the middle issue, not the root", async () => {
  const w = await nested();
  await w.move(G3, MID);
  await w.settle();
  assert.deepEqual(w.last(MID, "resumeWorker")?.params, { session: SESSION[MID], childrenEnd: false, childrenAdded: true });
  assert.equal(w.last(ROOT, "resumeWorker"), undefined);
});

test("moving the middle issue to another parent ends its chain and halts its grandchildren, so none outlives it", async () => {
  const w = await nested();
  await w.move(MID, OTHER);
  await w.settle();
  assert.equal((await w.store(MID)).facts.chain, null);
  assert.deepEqual(w.last(MID, "takeBackChain")?.params, { run: "r-mid" });
  for (const g of [G1, G2]) assert.equal((await w.store(g)).work?.state, "halted", g);
  // Neither halts nor takes back the other: the root, its last sub-issue gone, picks up.
  assert.equal(w.last(ROOT, "takeBackChain"), undefined);
  assert.deepEqual(w.last(ROOT, "resumeWorker")?.params, { session: SESSION[ROOT], childrenEnd: true, childrenAdded: false });
  // A cancel after that halts nothing twice.
  await w.state(MID, "cancelled");
  await w.settle();
  assert.equal(w.sent.filter((c) => c.issue === MID && c.kind === "takeBackChain").length, 1);
});

test("a sub-issue queued behind its blocker starts once the blocker lands, though reads since name no one for its delegation", async () => {
  const w = new World();
  w.blockers[G2] = [G1];
  await w.delegate(ROOT, "owner");
  await w.settle();
  await w.reads(ROOT);
  await w.chains(ROOT, "r-root");
  await w.settle();
  await w.answersAndExits(ROOT);
  await w.reads(MID);
  await w.chains(MID, "r-mid");
  await w.settle();
  await w.answersAndExits(MID);
  await w.reads(G1);
  await w.settle();
  assert.equal(w.last(G2, "startWorker"), undefined);
  // The blocker lands and writes its context onto the sub-issue, whose history names no one.
  w.unnamed.add(G2);
  await w.state(G1, "completed");
  await w.settle();
  assert.equal((await w.store(G2)).facts.delegate?.value, "chain");
  assert.equal(w.last(G2, "startWorker")?.params.orchestrate, false);
});
