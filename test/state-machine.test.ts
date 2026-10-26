import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import contract from "./contract.json" with { type: "json" };

// docs/state-machine.md is the spec the Durable Object's machine and listen's executor are
// built to; contract.json's `machine` holds its names for both sides to pin to. These tests
// hold the doc's tables to the fixture, and its transition tables to every (state, event)
// pair.
const doc = readFileSync(new URL("../docs/state-machine.md", import.meta.url), "utf8").replace(/\r\n/g, "\n");
const { requestStates, workStates, triageStates, events, coordinator, commands, notices } = contract.machine;

// The text under a heading, up to the next heading of the same or a higher level.
function section(heading: string): string {
  const level = heading.indexOf(" ");
  const start = doc.indexOf(`\n${heading}\n`);
  assert.notEqual(start, -1, `docs/state-machine.md has "${heading}"`);
  const rest = doc.slice(start + heading.length + 2);
  const end = rest.search(new RegExp(`^#{1,${level}} `, "m"));
  return end === -1 ? rest : rest.slice(0, end);
}

// A table's body rows, as their cells; the header and the separator are left out.
function rows(text: string): string[][] {
  return text.split("\n").filter((line) => line.startsWith("|")).slice(2).map((line) => line.split("|").slice(1, -1).map((cell) => cell.trim()));
}

const names = (cell: string) => [...cell.matchAll(/`([^`]+)`/g)].map((m) => m[1]);

test("the doc names the fixture's states, events, coordinator events, commands and notices", () => {
  assert.deepEqual(rows(section("### Requests")).map((r) => names(r[0])[0]), requestStates);
  assert.deepEqual(rows(section("### Work")).map((r) => names(r[0])[0]), workStates);
  assert.deepEqual(rows(section("### Triage")).map((r) => names(r[0])[0]), triageStates);
  assert.deepEqual(Object.fromEntries(rows(section("## Events")).map((r) => [names(r[0])[0], r[1].split(", ")])), events);
  assert.deepEqual(rows(section("## Stores")).map((r) => names(r[0])[0]), coordinator);
  assert.deepEqual(rows(section("## Commands")).map((r) => names(r[0])[0]), commands);
  const kinds = section("## Commands").match(/A notice's kinds:([^]*?)\. Each is/);
  assert.ok(kinds, "the notice's kinds are listed");
  assert.deepEqual(names(kinds[1]), notices);
});

for (const [machine, heading, states] of [["request", "## Request transitions", requestStates], ["work", "## Work transitions", workStates], ["triage", "## Triage transitions", triageStates]] as const) {
  test(`every ${machine} transition names known states, events and commands, and covers every pair`, () => {
    const routed = Object.entries(events).filter(([, to]) => (to as string[]).includes(machine)).map(([event]) => event);
    const pairs = new Map<string, string[]>();
    for (const [state, event, when, next, said] of rows(section(heading))) {
      for (const name of names(state)) assert.ok((states as readonly string[]).includes(name), `${name} is a ${machine} state`);
      for (const name of names(event)) assert.ok(routed.includes(name), `${name} reaches the ${machine} machine`);
      // A new state is one of the machine's, or "as" another row or the decision says.
      if (!next.startsWith("as ")) for (const name of names(next)) assert.ok((states as readonly string[]).includes(name), `${name} is a ${machine} state`);
      for (const name of names(said)) assert.ok(commands.includes(name) || name in events || coordinator.includes(name) || notices.includes(name), `${name} is a command, an event, a coordinator's event or a notice`);
      for (const s of names(state)) for (const e of names(event)) pairs.set(`${s} ${e}`, [...(pairs.get(`${s} ${e}`) ?? []), when]);
    }
    for (const state of states) {
      for (const event of routed) {
        const whens = pairs.get(`${state} ${event}`);
        assert.ok(whens, `(${state}, ${event}) has a row`);
        // Two rows for one pair must be told apart by their condition.
        if (whens.length > 1) assert.ok(!whens.includes("—") && new Set(whens).size === whens.length, `(${state}, ${event})'s rows have distinct conditions`);
      }
    }
  });
}
