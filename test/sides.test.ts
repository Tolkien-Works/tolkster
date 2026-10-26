// AGENTS.md's `sides` block says which side's checks each path needs (TW-614): every file
// outside src/ and test/ that a TypeScript test reads or imports is on the TypeScript
// side, and so is each Rust test file a test opens to find a test it names, so a worker
// whose diff touches one runs `npm test`. cli/src/brief/tests/repo_rules.rs holds the Rust side to it.

import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { posix } from "node:path";
import { test } from "node:test";
import { block, covers } from "../scripts/gate.ts";

const ROOT = new URL("../", import.meta.url);

function side(name: string): string[] {
  const patterns: string[] = [];
  for (const line of block(readFileSync(new URL("../AGENTS.md", import.meta.url), "utf8"), "sides")) {
    const [s, pattern, extra] = line.split(" ");
    assert.ok((s === "rust" || s === "ts") && pattern !== undefined && extra === undefined, `the sides block's line ${line}`);
    if (s === name) patterns.push(pattern);
  }
  return patterns;
}

// Each file a test or the Worker reads or imports, from the repo's root. A path built at run
// time isn't seen, except a Rust test file a quoted `<file>:<test>` names.
function reads(): string[] {
  const found: string[] = [];
  for (const dir of ["src", "test"]) {
    for (const entry of readdirSync(new URL(`${dir}/`, ROOT), { recursive: true, encoding: "utf8" })) {
      const file = posix.join(dir, entry.replaceAll("\\", "/"));
      if (!file.endsWith(".ts")) continue;
      const text = readFileSync(new URL(file, ROOT), "utf8");
      for (const m of text.matchAll(/new URL\(\s*["'`](\.\.?\/[^"'`$]*)["'`]|from\s+["'](\.\.?\/[^"']+)["']/g)) {
        const path = posix.normalize(posix.join(posix.dirname(file), m[1] ?? m[2]));
        if (path !== "." && !path.endsWith("/")) found.push(path);
      }
      for (const m of text.matchAll(/["'](?:row:)?(cli\/[^"':]+):/g)) found.push(m[1]);
    }
  }
  return found;
}

test("every file a TypeScript test reads across is on the TypeScript side", () => {
  const ts = side("ts");
  const found = reads();
  for (const read of ["AGENTS.md", "docs/state-machine.md", "src/shared/voice.json", "cli/src/command/tests/workers.rs"]) {
    assert.ok(found.includes(read), `the scan missed ${read}`);
  }
  for (const read of found) {
    assert.ok(ts.some((pattern) => covers(pattern, read)), `a TypeScript test reads ${read}, which AGENTS.md's sides block doesn't put on the TypeScript side`);
  }
});

test("a side's pattern covers as the review list's does", () => {
  assert.ok(covers("src/", "src/worker/state.ts") && !covers("src/", "srcs/a.ts"));
  assert.ok(covers("tsconfig*", "tsconfig.test.json") && covers("wrangler.*", "wrangler.toml") && !covers("wrangler.*", "wranglerx"));
  assert.ok(covers("AGENTS.md", "AGENTS.md") && !covers("AGENTS.md", "docs/AGENTS.md"));
  assert.ok(side("rust").includes("cli/"));
  // test/word-budgets.test.ts reads these through paths built at run time, which the scan misses.
  for (const read of ["README.md", "docs/cli.md", "cli/briefs/work.md"]) assert.ok(side("ts").some((p) => covers(p, read)), read);
});
