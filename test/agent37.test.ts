// The Agent37 recipe's `prepare` (scripts/orca-vm/agent37-recipe.ts): what it rebuilds, by the
// digests of its inputs.

import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { baseStep, inputs } from "../scripts/orca-vm/agent37.ts";

test("an input's digest follows its files' names and text, not their line endings", () => {
  const dir = mkdtempSync(join(tmpdir(), "agent37-inputs-"));
  mkdirSync(join(dir, "nested"));
  writeFileSync(join(dir, "Dockerfile"), "FROM a\nRUN b\n");
  writeFileSync(join(dir, "nested", "x"), "1\n");
  const first = inputs([dir]);
  writeFileSync(join(dir, "Dockerfile"), "FROM a\r\nRUN b\r\n");
  assert.equal(inputs([dir]), first);
  writeFileSync(join(dir, "Dockerfile"), "FROM a\nRUN c\n");
  assert.notEqual(inputs([dir]), first);
  writeFileSync(join(dir, "Dockerfile"), "FROM a\nRUN b\n");
  writeFileSync(join(dir, "nested", "y"), "1\n");
  assert.notEqual(inputs([dir]), first);
});

test("prepare replaces a base from other inputs, updates one on an older image and leaves a current one", () => {
  const state = { baseInstanceId: "base1", baseInputs: "sha256:a" };
  assert.equal(baseStep(state, "sha256:a", "4", "4"), "none");
  assert.equal(baseStep(state, "sha256:a", "3", "4"), "update");
  assert.equal(baseStep(state, "sha256:b", "4", "4"), "new");
  assert.equal(baseStep({}, "sha256:a", undefined, "1"), "new");
  assert.equal(baseStep(state, "sha256:a", undefined, "4"), "new");
});
