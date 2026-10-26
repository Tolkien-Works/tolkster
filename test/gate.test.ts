// scripts/gate.ts runs the gate in one command (TW-622): the sides a branch's paths are on,
// by AGENTS.md's own blocks, and each side's list from its "Checks".

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { checks, filtered, leftToCi, needsSimulator, sides } from "../scripts/gate.ts";

const AGENTS = readFileSync(new URL("../AGENTS.md", import.meta.url), "utf8");

test("a branch's paths pick the sides it runs", () => {
  const on = (changed: string[], deleted: string[] = []) => sides(AGENTS, changed, deleted);
  assert.deepEqual(on(["cli/src/hook.rs"]), ["rust"]);
  assert.deepEqual(on(["src/worker/machine/work.ts", "test/machine-work.test.ts"]), ["ts"]);
  assert.deepEqual(on(["cli/src/hook.rs", "src/worker/relay.ts"]), ["rust", "ts"]);
  // A file both sides' tests read, or one no line covers, is on both.
  assert.deepEqual(on(["AGENTS.md"]), ["rust", "ts"]);
  assert.deepEqual(on([".github/workflows/ci.yml"]), ["rust", "ts"]);
  // CI's skip paths run nothing, less the kept ones.
  assert.deepEqual(on([".github/copilot-instructions.md", "design/x.html"]), []);
  assert.deepEqual(on(["cli/briefs/work.md"]), ["rust", "ts"]);
  assert.deepEqual(on(["docs/cli.md", "README.md"]), ["ts"]);
  // A deleted path is on both: a Rust test checks the review list's files exist.
  assert.deepEqual(on(["src/worker/old.ts"], ["src/worker/old.ts"]), ["rust", "ts"]);
  // `--all` runs both sides for any path CI doesn't skip, and still none for those it does.
  assert.deepEqual(sides(AGENTS, ["cli/src/hook.rs"], [], true), ["rust", "ts"]);
  assert.deepEqual(sides(AGENTS, ["design/x.html"], [], true), []);
});

test("the simulator's tests run only for a branch touching its paths or a Rust test it names", () => {
  const named = "see cli/src/command/tests/replays.rs and cli/src/listen/tests.rs, not cli/src/ports.rs";
  const on = (...changed: string[]) => needsSimulator(AGENTS, changed, named);
  assert.equal(on("cli/briefs/work.md", "docs/cli.md", "AGENTS.md", "src/worker/relay.ts"), false);
  assert.equal(on("src/worker/machine/work.ts"), true);
  assert.equal(on("src/worker/machine.ts"), true);
  assert.equal(on("src/worker/route.ts"), true);
  assert.equal(on("test/machine-sim-world.ts"), true);
  assert.equal(on("docs/state-machine.md"), true);
  assert.equal(on("cli/src/command/tests/replays.rs"), true);
  assert.equal(on("cli/src/ports.rs"), false);
  // The listed paths are the real ones, and the files naming Rust tests name some.
  const dir = new URL("../", import.meta.url);
  const sim = readFileSync(new URL("test/machine-sim-steps.ts", dir), "utf8");
  assert.equal(needsSimulator(AGENTS, ["cli/src/command/tests/replays.rs"], sim), true);
});

test("each side's list is the Checks block's", () => {
  const { rust, ts } = checks(AGENTS);
  assert.deepEqual(rust, [
    "cargo test --locked --manifest-path cli/Cargo.toml",
    "cargo clippy --locked --manifest-path cli/Cargo.toml --all-targets -- -D warnings",
    "cargo test --locked --manifest-path cli/Cargo.toml --features develop",
    "cargo clippy --locked --manifest-path cli/Cargo.toml --all-targets --features develop -- -D warnings",
  ]);
  assert.deepEqual(ts, ["npm ci", "npm run typecheck", "npm test", "npx wrangler deploy --dry-run --outdir dist"]);
});

test("the gate leaves the non-develop cargo test to CI and runs the other three", () => {
  const { rust, ts } = checks(AGENTS);
  const left = leftToCi(rust);
  assert.deepEqual(left, ["cargo test --locked --manifest-path cli/Cargo.toml"]);
  assert.equal(rust.filter((c) => !left.includes(c)).length, 3);
  // Without a develop test in the list, nothing is left out, and the TypeScript side loses none.
  assert.deepEqual(leftToCi(rust.filter((c) => c !== rust[2])), []);
  assert.deepEqual(leftToCi(ts), []);
  // `--all` leaves nothing to CI.
  assert.deepEqual(leftToCi(rust, true), []);
});

test("a passing check prints its counts, else its last lines", () => {
  const cargo = "running 3 tests\ntest a ... ok\ntest result: ok. 3 passed; 0 failed\n";
  assert.deepEqual(filtered("cargo test --locked --manifest-path cli/Cargo.toml", cargo), ["test result: ok. 3 passed; 0 failed"]);
  assert.deepEqual(filtered("npm test", "✔ a\nℹ tests 2\nℹ pass 2\nℹ fail 0\nℹ duration_ms 9\n"), ["ℹ tests 2", "ℹ pass 2", "ℹ fail 0"]);
  assert.deepEqual(filtered("npm run typecheck", "a\nb\nc\nd\n\n"), ["b", "c", "d"]);
});
