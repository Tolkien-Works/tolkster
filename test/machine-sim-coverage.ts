// The simulator's branch coverage of src/worker/machine/, from V8's own (NODE_V8_COVERAGE),
// held to a ceiling per file in machine-sim-coverage.json on the code lines of the blocks no
// seed runs: lines, not blocks, so a block that stops running counts though one inside it
// never ran, and unrun code added counts too. A file over its ceiling fails, so a branch the
// seeds reached and no longer do is a reviewed line there; one under it fails too, so a rise
// lowers its ceiling. `COVERAGE=write`, which the gate (scripts/gate.ts) sets, lowers the
// ceilings itself; a raised one stays a decision.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

type Range = { startOffset: number; endOffset: number; count: number };
type Script = { url: string; functions: { ranges: Range[] }[] };

// The lines of the blocks of `under` (a path from `root`) that no thread ran, by file,
// from the coverage files each thread wrote in `dir`. A thread reports a block only where its
// count differs from the block around it, so which blocks a thread names depends on the
// seeds dealt to it, and Node's own merge of them (its test runner's coverage) does too. A
// block no thread ran is the same whatever the dealing: some thread that ran its enclosing
// block names it with a count of 0, and every other thread counts it in an enclosing block
// that ran nowhere.
export function unreached(dir: string, root: URL, under: string): Record<string, number[]> {
  const byFile = new Map<string, Range[][]>();
  for (const name of readdirSync(dir)) {
    const { result } = JSON.parse(readFileSync(join(dir, name), "utf8")) as { result: Script[] };
    for (const script of result) {
      if (!script.url.startsWith(root.href)) continue;
      const file = decodeURIComponent(script.url.slice(root.href.length));
      if (!file.startsWith(under)) continue;
      byFile.set(file, [...(byFile.get(file) ?? []), script.functions.flatMap((f) => f.ranges)]);
    }
  }
  const found: Record<string, number[]> = {};
  for (const [file, runs] of [...byFile].sort(([a], [b]) => a.localeCompare(b))) {
    const keys = new Set<string>();
    const zero: Range[] = [];
    for (const r of runs.flat()) {
      if (r.count === 0 && !keys.has(`${r.startOffset}:${r.endOffset}`)) {
        keys.add(`${r.startOffset}:${r.endOffset}`);
        zero.push(r);
      }
    }
    const never = zero.filter((z) => runs.every((run) => innermost(run, z) === 0));
    const outer = never.filter((z) => !never.some((o) => o !== z && o.startOffset <= z.startOffset && o.endOffset >= z.endOffset));
    const text = readFileSync(fileURLToPath(new URL(file, root)), "utf8");
    const source = text.split("\n");
    const line = (at: number) => text.slice(0, at).split("\n").length;
    // A range ends where the next begins, after its newline and indent: its last line is that
    // of its last character that isn't whitespace.
    const last = (z: Range) => line(text.slice(0, z.endOffset).trimEnd().length);
    const lines = new Set(outer.flatMap((z) => Array.from({ length: last(z) - line(z.startOffset) + 1 }, (_, i) => line(z.startOffset) + i)));
    found[file] = [...lines].filter((n) => !/^\s*(\/\/.*)?\r?$/.test(source[n - 1])).sort((a, b) => a - b);
  }
  return found;
}

// The count of the smallest range of `run` around `z`: a thread that never ran the code
// around it counts it 0.
function innermost(run: Range[], z: Range): number {
  let best: Range | null = null;
  for (const r of run) {
    if (r.startOffset <= z.startOffset && r.endOffset >= z.endOffset && (best === null || r.endOffset - r.startOffset < best.endOffset - best.startOffset)) best = r;
  }
  return best?.count ?? 0;
}

// Sorted lines as spans: 3-5, 9.
function spans(lines: number[]): string {
  const out: string[] = [];
  for (let i = 0; i < lines.length; i++) {
    let j = i;
    while (j + 1 < lines.length && lines[j + 1] === lines[j] + 1) j++;
    out.push(j === i ? `${lines[i]}` : `${lines[i]}-${lines[j]}`);
    i = j;
  }
  return out.join(", ");
}

// What breaks the ceilings, one line a file: none when each file is at its own.
export function problems(ceilings: Record<string, number>, found: Record<string, number[]>): string[] {
  const out: string[] = [];
  for (const [file, lines] of Object.entries(found)) {
    const [n, ceiling] = [lines.length, ceilings[file]];
    if (ceiling === undefined) out.push(`${file} has ${n} lines no seed runs and no ceiling: add it to test/machine-sim-coverage.json`);
    else if (n > ceiling) out.push(`${file} has ${n} lines no seed runs, over its ceiling of ${ceiling}: ${spans(lines)}; reach them again, or raise the ceiling and say why in the PR`);
    else if (n < ceiling) out.push(`${file} has ${n} lines no seed runs, under its ceiling of ${ceiling}: lower the ceiling to ${n}`);
  }
  for (const file of Object.keys(ceilings)) {
    if (!(file in found)) out.push(`${file} has a ceiling but no coverage: remove it from test/machine-sim-coverage.json`);
  }
  return out;
}

// The ceilings with each one a rise left over its file lowered to it, a new file's added at
// what it leaves and a gone one's dropped: growth stays a decision, never settled here.
export function settle(ceilings: Record<string, number>, found: Record<string, number[]>): Record<string, number> {
  const settled: Record<string, number> = {};
  for (const [file, lines] of Object.entries(found)) settled[file] = Math.min(ceilings[file] ?? lines.length, lines.length);
  return settled;
}
