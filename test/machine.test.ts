import assert from "node:assert/strict";
import { test } from "node:test";
import { type Command, COMMANDS, fromHook, logged, MAX_LISTED, MAX_SLOTS, newest, OBSERVATIONS, parseReport, refusedReport, RESULTS } from "../src/worker/machine.ts";
import { Invalid } from "../src/worker/check.ts";
import { parseEvent } from "../src/worker/machine/validate.ts";
import contract from "./contract.json" with { type: "json" };
import { ISSUE } from "./machine-fixture.ts";

// The contract with the CLI: cli/src/machine.rs reads each of the fixture's commands and
// writes each of its reports as they are, so a field added on one side fails the other's
// test until the fixture has it.
test("the commands and results are the machine's, but for the alarm's postAway", () => {
  assert.deepEqual([...COMMANDS, "postAway"], contract.machine.commands);
  assert.deepEqual([...RESULTS], contract.machine.results);
  for (const observed of OBSERVATIONS) assert.ok(observed in contract.machine.events, observed);
});

test("listen's listing cap and slots cap are the machine's", () => {
  assert.equal(MAX_LISTED, contract.machine.maxListed);
  assert.equal(MAX_SLOTS, contract.machine.maxSlots);
});

test("the contract fixture has every kind of command, in order, with the envelope's fields", () => {
  const commands = contract.commands as Command[];
  assert.deepEqual([...new Set(commands.map((c) => c.kind))], [...COMMANDS]);
  for (const command of commands) assert.deepEqual(Object.keys(command).sort(), ["args", "id", "identifier", "issueId", "kind", "type", "urgent", "version"], command.id);
});

test("each report in the contract fixture reads back as itself", () => {
  for (const report of contract.reports) assert.deepEqual(parseReport(report), report, report.report);
  assert.deepEqual(contract.reports.filter((r) => "sequence" in r).map((r) => r.report).filter((r) => (OBSERVATIONS as readonly string[]).includes(r)), [...OBSERVATIONS]);
});

test("the contract fixture has the hook's reports", () => {
  assert.deepEqual(contract.reports.map(parseReport).filter((r) => r !== null && fromHook(r)).map((r) => r.report), ["agentIdle", "agentBackground"]);
});

// listen sends `evaluatePr`'s result as the machine's `PrRead` (cli/src/command.rs,
// pr_read), which the machine takes as its command result's data unchanged.
test("the evaluatePr result in the contract fixture is the machine's PrRead", () => {
  const result = contract.reports.find((r) => r.report === "commandResult" && r.id === "c-9") as { detail: { pr: object } } | undefined;
  assert.ok(result);
  const event = { kind: "commandResult", id: `${ISSUE}.11`, issue: ISSUE, command: `${ISSUE}.11`, result: "ok", data: result.detail };
  assert.deepEqual(parseEvent(event), event);
  // GitHub's own form, to the second, is not the machine's.
  assert.throws(() => parseEvent({ ...event, data: { pr: { ...result.detail.pr, at: "2026-09-26T01:04:00Z" } } }), Invalid);
});

test("a report of the wrong shape, or anything else, reads as none", () => {
  const [result, working, , , , , , read, , chain] = contract.reports;
  const listing = contract.reports.find((r) => r.report === "listing");
  const config = contract.reports.find((r) => r.report === "config");
  for (const bad of [
    null,
    "pong",
    { ack: "start:s-1", at: "2026-09-26T01:00:00.000Z" },
    { ...result, result: "maybe" },
    { ...result, version: -1 },
    { ...result, id: "c 1" },
    { ...result, detail: [] },
    { ...result, detail: { text: "x".repeat(20_000) } },
    { ...working, worktreeId: "no-separator" },
    { ...working, sequence: 1.5 },
    // An agent's own report is of its state only, at a time, and never both forms at once.
    { report: "worktreeGone", worktreeId: "r1::D:/wt/tw-1", at: "2026-09-26T01:05:00.000Z" },
    { report: "agentBackground", worktreeId: "r1::D:/wt/tw-1", sequence: 1 },
    { report: "agentIdle", worktreeId: "r1::D:/wt/tw-1", at: "soon" },
    { report: "agentIdle", worktreeId: "r1::D:/wt/tw-1" },
    { report: "agentIdle", worktreeId: "r1::D:/wt/tw-1", at: "2026-09-26T01:05:00.000Z" },
    { report: "agentIdle", worktreeId: "r1::D:/wt/tw-1", at: "2026-09-26T01:05:00.000Z", elapsedMs: -1 },
    { ...working, sequence: "1", at: "2026-09-26T01:05:00.000Z" },
    { ...read, at: "yesterday" },
    { ...chain, children: ["TW 1"] },
    { ...chain, children: Array.from({ length: 51 }, (_, i) => `c-${i}`) },
    { ...listing, at: "soon" },
    { ...listing, elapsedMs: undefined },
    { ...listing, elapsedMs: 1.5 },
    { ...listing, agents: { "r1::D:/wt/tw-1": "exited" } },
    { ...listing, agents: { "no-separator": "working" } },
    { ...listing, agents: [] },
    { ...listing, listed: ["no-separator"] },
    { ...listing, listed: undefined },
    { ...listing, hosts: undefined },
    { ...listing, hosts: ["-mac"] },
    { ...listing, paired: undefined },
    { ...listing, paired: ["-mac"] },
    { ...config, cap: MAX_SLOTS + 1 },
    { ...config, cap: -1 },
    { ...config, slots: undefined },
    { ...config, machines: [] },
    { report: "terminalClosed", terminal: "term 1", sequence: 1 },
    { report: "terminalClosed", terminal: "term_1" },
    { report: "triageEnded", sessionId: "s-3", sequence: 7 },
    { ...listing, agents: Object.fromEntries(Array.from({ length: 1001 }, (_, i) => [`r1::D:/wt/${i}`, "working"])) },
    { ...listing, listed: Array.from({ length: 1001 }, (_, i) => `r1::D:/wt/${i}`) },
  ]) {
    assert.equal(parseReport(bad), null, JSON.stringify(bad)?.slice(0, 80));
  }
  // No slots at all is a setting: every host's cap at 0 takes no work. An entry that doesn't
  // read is left out of the settings; a host entry that doesn't fails the listing's fleet alone.
  assert.deepEqual(parseReport({ ...config, cap: 0, slots: { mac: -1, pi: 0 }, machines: { "tolkster-2": { repo: "Tolkster" } } }), { ...config, cap: 0, slots: { pi: 0 }, machines: {} });
  assert.deepEqual(parseReport({ ...listing, fleet: { held: 0, hosts: [{ id: "-mac" }] } }), { ...listing, fleet: null });
  assert.deepEqual(parseReport({ ...listing, fleet: undefined }), { ...listing, fleet: null });
  // A host's odd name goes as its ID and an odd project is left out, keeping its workers.
  const odd = { id: "mac", name: " ", cap: 2, admitted: true, answering: true, ready: ["github:acme/app", "a b", 7], held: 3 };
  // A project entry that doesn't read is left out.
  const projects = { r1: "github:acme/app", "r 2": "github:acme/two", r3: "not a project" };
  assert.deepEqual(parseReport({ ...listing, fleet: { held: 0, hosts: [odd], projects } }), { ...listing, fleet: { held: 0, hosts: [{ ...odd, name: "mac", ready: ["github:acme/app"] }], projects: { r1: "github:acme/app" } } });
});

test("the newest accepted connection is the one that counts", () => {
  assert.equal(newest([{ socket: "a", acceptedAt: 1 }, { socket: "b", acceptedAt: 3 }, { socket: "c", acceptedAt: 2 }]), "b");
  assert.equal(newest([]), null);
});

test("the log keeps no worktree ID", () => {
  for (const report of contract.reports) {
    const parsed = parseReport(report);
    assert.ok(parsed);
    assert.ok(!JSON.stringify(logged(parsed)).includes("D:/wt"), report.report);
  }
});

// `tolkster` sends a report again on a 5xx alone (cli/src/state.rs, `once`).
test("a refused report answers a status tolkster sends it again on, logging no worktree ID", () => {
  for (const report of contract.reports) {
    const parsed = parseReport(report);
    assert.ok(parsed);
    const refused = refusedReport(parsed);
    assert.ok(refused.status >= 500, report.report);
    assert.ok(!JSON.stringify(refused).includes("D:/wt"), report.report);
  }
});

// What listen's results carry (cli/src/command.rs) is what the machine's validator takes, field
// for field: a field dropped or renamed on either side fails here or in cargo test.
test("each result's data in the contract fixture is the machine's, as it reads it", () => {
  for (const [name, data] of Object.entries(contract.resultData)) {
    const event = parseEvent({ kind: "commandResult", id: "r", issue: "1b2c3d4e-0000-4000-8000-000000000001", command: "1b2c3d4e-0000-4000-8000-000000000001.1", result: "ok", data });
    assert.ok(event.kind === "commandResult");
    assert.deepEqual(event.data, data, name);
  }
});
