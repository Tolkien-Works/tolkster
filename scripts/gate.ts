// The gate in one command (AGENTS.md, "At the gate"): the whole list of each side the
// branch's changed paths are on, by AGENTS.md's `sides` block, less CI's skip paths, with
// each check's output filtered, and `cargo test` without `develop` left to CI (`leftToCi`).
// `--all`, for a branch no CI runs after (docs/landing.md), runs what CI would: both sides
// unless CI skips every path, the simulator's tests, and what it leaves to CI.
// `node scripts/gate.ts [--all]`, from anywhere in the worktree,
// with the work committed. First it settles the word budgets (test/word-budgets.json): a
// trimmed file's number is lowered for the worker to commit, and every file over its budget
// is listed in one place, which stops the gate. Exits 0 when every check passed or none had
// to run, 1 when one failed or a file is over its budget, and 2 when it couldn't tell what
// to run: work uncommitted, whose paths the branch's diff can't see, or git or AGENTS.md
// failing it.

import { spawn, spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { counts, settle } from "./word-budgets.ts";

export type Side = "rust" | "ts";

const ROOT_URL = new URL("../", import.meta.url);
const ROOT = fileURLToPath(ROOT_URL);

// A fenced block's lines, trimmed, blank ones dropped: the first opened with "```<name>",
// or with a bare "```" after the heading `after`.
export function block(agents: string, name: string, after = ""): string[] {
  const text = agents.replace(/\r\n/g, "\n");
  const fence = `\`\`\`${name}\n`;
  const start = after === "" ? 0 : text.indexOf(after);
  const from = start === -1 ? -1 : text.indexOf(fence, start);
  if (from === -1) throw new Error(`AGENTS.md has no ${name || after.trim()} block`);
  const body = from + fence.length;
  return text.slice(body, text.indexOf("```", body)).split("\n").map((l) => l.trim()).filter((l) => l !== "");
}

// As the review list's patterns (cli/src/review_pass.rs, `covers`): `dir/` covers what's
// under it, `*` any run of characters, `/` included.
export function covers(pattern: string, path: string): boolean {
  const body = pattern.replace(/[.+?^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*");
  return new RegExp(`^${body}${pattern.endsWith("/") ? "" : "$"}`).test(path);
}

// The sides `changed` needs: each path's by the sides block, both for a path no line covers
// and for one the branch deletes (a Rust test checks the review list's files exist), and
// none for a path CI skips, the review list's `skip` patterns less its `keep` ones. With
// `all`, both sides whenever any path is one CI doesn't skip.
export function sides(agents: string, changed: string[], deleted: string[], all = false): Side[] {
  const review = block(agents, "tolkster-review").map((l) => l.split(" "));
  const listed = (kind: string) => review.filter(([k]) => k === kind).map(([, p]) => p);
  const [skip, keep] = [listed("skip"), listed("keep")];
  const lines = block(agents, "sides").map((l) => l.split(" ") as [Side, string]);
  const needed = new Set<Side>();
  for (const path of changed) {
    if (skip.some((p) => covers(p, path)) && !keep.some((p) => covers(p, path))) continue;
    const on = lines.filter(([, p]) => covers(p, path)).map(([s]) => s);
    for (const side of all || deleted.includes(path) || on.length === 0 ? (["rust", "ts"] as Side[]) : on) needed.add(side);
  }
  return (["rust", "ts"] as Side[]).filter((s) => needed.has(s));
}

// Each side's list, from the block under "## Checks": the cargo commands are the Rust
// side's, the rest the TypeScript side's.
export function checks(agents: string): Record<Side, string[]> {
  const all = block(agents, "", "\n## Checks\n");
  return { rust: all.filter((c) => c.startsWith("cargo ")), ts: all.filter((c) => !c.startsWith("cargo ")) };
}

// Whether the branch needs the simulator's tests (test/machine-sim*.test.ts): it changes a
// path of AGENTS.md's `simulator` block, or a Rust test file the simulator's own files or
// docs/state-machine.md name. CI runs them whatever the branch changes.
export function needsSimulator(agents: string, changed: string[], named: string): boolean {
  const paths = block(agents, "simulator");
  const rustTests = new Set(named.match(/cli\/src\/[\w/-]*?tests(?:\.rs|\/[\w/-]*\.rs)/g) ?? []);
  return changed.some((path) => paths.some((p) => covers(p, path)) || rustTests.has(path));
}

// The checks of `list` the gate leaves to CI: `cargo test` without `develop`, when the list
// has it with `develop`, whose build holds every test of the other. Both clippy runs stay:
// they type-check each feature set, tests included. None with `all`.
export function leftToCi(list: string[], all = false): string[] {
  if (all) return [];
  const develop = (c: string) => c.startsWith("cargo test ") && c.includes("--features develop");
  return list.filter((c) => c.startsWith("cargo test ") && !c.includes("--features develop") && list.some(develop));
}

const SIMULATOR_TESTS = /^machine-sim.*\.test\.ts$/;

// What of a passing check's output is printed: its counts, else its last lines.
export function filtered(command: string, output: string): string[] {
  const lines = output.replace(/\r\n/g, "\n").split("\n").filter((l) => l.trim() !== "");
  if (command.startsWith("cargo test")) return lines.filter((l) => /^test result|FAILED|panicked/.test(l));
  if (command === "npm test") return lines.filter((l) => /^ℹ (tests|pass|fail)|^✖/.test(l));
  return lines.slice(-3);
}

function git(...args: string[]): string[] {
  const run = spawnSync("git", args, { cwd: ROOT, encoding: "utf8" });
  if (run.status !== 0) throw new Error(`git ${args.join(" ")} failed: ${run.stderr.trim()}`);
  return run.stdout.split("\n").map((l) => l.trim()).filter((l) => l !== "");
}

function run(command: string): Promise<{ ok: boolean; output: string }> {
  return new Promise((done) => {
    // COVERAGE=write has the simulator's test lower the coverage ceilings a rise left slack in.
    const child = spawn(command, { cwd: ROOT, shell: true, env: { ...process.env, COVERAGE: "write" } });
    let output = "";
    child.stdout.on("data", (d) => (output += d));
    child.stderr.on("data", (d) => (output += d));
    child.on("close", (code) => done({ ok: code === 0, output }));
  });
}

// `npm test`'s command less the simulator's files.
function withoutSimulator(): string {
  const files = readdirSync(`${ROOT}test`).filter((f) => f.endsWith(".test.ts") && !SIMULATOR_TESTS.test(f));
  return `node --test ${files.map((f) => `"test/${f}"`).join(" ")}`;
}

// One side's list in order, stopping at its first failure. `npm ci` only when Orca's setup
// marker is missing or the branch changes the lockfile (AGENTS.md, "While you work").
async function runSide(side: Side, list: string[], changed: string[], simulator: boolean, all: boolean): Promise<{ ok: boolean; report: string[] }> {
  const report: string[] = [];
  const ci = leftToCi(list, all);
  for (const command of list) {
    if (ci.includes(command)) {
      report.push(`[${side}] skipped: ${command}, the develop run covers it and CI runs it`);
      continue;
    }
    if (command === "npm ci" && existsSync(`${ROOT}node_modules/.tolkster-setup`) && !changed.includes("package-lock.json")) {
      report.push(`[${side}] skipped: ${command}, the worktree is set up`);
      continue;
    }
    const skipping = command === "npm test" && !simulator;
    const started = Date.now();
    const { ok, output } = await run(skipping ? withoutSimulator() : command);
    const seconds = Math.round((Date.now() - started) / 1000);
    report.push(`[${side}] ${ok ? "passed" : "FAILED"} in ${seconds}s: ${command}${skipping ? " (simulator skipped: the branch touches none of its paths)" : ""}`);
    report.push(...(ok ? filtered(command, output) : output.replace(/\r\n/g, "\n").trimEnd().split("\n").slice(-40)).map((l) => `  ${l}`));
    if (!ok) return { ok, report };
  }
  return { ok: true, report };
}

// Lowers the budgets a trim left slack in, and says each; true when no file is still over
// its budget, else lists them all.
function settleBudgets(): boolean {
  const file = `${ROOT}test/word-budgets.json`;
  const { settled, changes, over } = settle(JSON.parse(readFileSync(file, "utf8")) as Record<string, number>, counts(ROOT_URL));
  if (changes.length > 0) {
    writeFileSync(file, `${JSON.stringify(settled, null, 2)}\n`);
    console.log(`Settled test/word-budgets.json, commit it:\n${changes.map((c) => `  ${c}`).join("\n")}`);
  }
  if (over.length > 0) console.log(`Word budgets broken, decide once for each:\n${over.map((o) => `  ${o}`).join("\n")}`);
  return over.length === 0;
}

async function main(all: boolean): Promise<number> {
  if (git("status", "--porcelain").length > 0) {
    console.log("Commit your work first: the gate goes by the branch's diff, which can't see it.");
    return 2;
  }
  if (!settleBudgets()) return 1;
  const agents = readFileSync(`${ROOT}AGENTS.md`, "utf8");
  const changed = git("diff", "--name-only", "--no-renames", "origin/main...HEAD");
  const deleted = git("diff", "--name-only", "--no-renames", "--diff-filter=D", "origin/main...HEAD");
  const needed = sides(agents, changed, deleted, all);
  if (needed.length === 0) {
    console.log("No side to run: the branch changes only paths CI skips.");
    return 0;
  }
  console.log(`Running the ${needed.join(" and ")} side${needed.length > 1 ? "s, together" : ""}.`);
  const lists = checks(agents);
  const named = ["docs/state-machine.md", ...readdirSync(`${ROOT}test`).filter((f) => /^machine-sim.*\.ts$/.test(f)).map((f) => `test/${f}`)]
    .map((f) => readFileSync(`${ROOT}${f}`, "utf8")).join("\n");
  const simulator = all || needsSimulator(agents, changed, named);
  const results = await Promise.all(needed.map((side) => runSide(side, lists[side], changed, simulator, all)));
  for (const { report } of results) console.log(report.join("\n"));
  if (git("status", "--porcelain", "test/machine-sim-coverage.json").length > 0) console.log("Lowered the simulator's coverage ceilings in test/machine-sim-coverage.json, commit it.");
  return results.every((r) => r.ok) ? 0 : 1;
}

if (process.argv[1] !== undefined && pathToFileURL(process.argv[1]).href === import.meta.url) {
  process.exitCode = await main(process.argv.includes("--all")).catch((error: unknown) => {
    console.log(`The gate couldn't run: ${error instanceof Error ? error.message : String(error)}`);
    return 2;
  });
}
