// The markdown every session or worker reads stays within its word budget (TW-631): a file
// over its budget fails, so growth is a reviewed line in word-budgets.json; one more than
// 5% under fails too, so a trim past that lowers its number and leaves little to grow back
// into. The gate (scripts/gate.ts) lowers those numbers itself. Its reads are built at run
// time, so the sides scan can't see them: test/sides.test.ts holds them on the TypeScript
// side by name.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { counts, problems, settle, words } from "../scripts/word-budgets.ts";

const ROOT = new URL("../", import.meta.url);

test("every budgeted file is within its word budget", () => {
  const budgets = JSON.parse(readFileSync(new URL("./word-budgets.json", import.meta.url), "utf8")) as Record<string, number>;
  const found = counts(ROOT);
  assert.ok("AGENTS.md" in found && "docs/state-machine.md" in found && "cli/briefs/work.md" in found);
  assert.deepEqual(problems(budgets, found), []);
});

test("a file over its budget, more than 5% under it, without one, or gone fails", () => {
  assert.deepEqual(problems({ "a.md": 100, "b.md": 100 }, { "a.md": 100, "b.md": 95 }), []);
  assert.match(problems({ "a.md": 100 }, { "a.md": 101 }).join(), /1 over its budget of 100/);
  assert.match(problems({ "a.md": 100 }, { "a.md": 94 }).join(), /lower the budget to 94/);
  assert.match(problems({}, { "a.md": 10 }).join(), /no budget/);
  assert.match(problems({ "a.md": 10 }, {}).join(), /no file/);
});

test("settling lowers trimmed budgets and drops gone ones, and leaves growth to a decision", () => {
  const budgets = { "a.md": 100, "b.md": 100, "c.md": 100, "d.md": 100 };
  const { settled, changes, over } = settle(budgets, { "a.md": 94, "b.md": 95, "c.md": 130, "e.md": 7 });
  assert.deepEqual(settled, { "a.md": 94, "b.md": 100, "c.md": 100 });
  assert.deepEqual(changes, ["a.md: 100 to 94", "d.md: dropped, the file is gone"]);
  assert.equal(over.length, 2);
  assert.match(over.join(), /c\.md has 130 words, 30 over its budget of 100/);
  assert.match(over.join(), /e\.md has 7 words and no budget/);
  assert.deepEqual(settle(settled, { "a.md": 94, "b.md": 100, "c.md": 100 }), { settled, changes: [], over: [] });
});

test("a word is a run of anything but whitespace", () => {
  assert.equal(words("# A title\n\n- `one`, two:\r\n  three.\n"), 7);
  assert.equal(words(""), 0);
});
