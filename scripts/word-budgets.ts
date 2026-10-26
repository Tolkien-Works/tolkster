// The word budgets of the markdown every session or worker reads (TW-631), counted by
// test/word-budgets.test.ts, which enforces them, and settled by scripts/gate.ts, which
// lowers the ones a trim left more than 5% under.

import { readdirSync, readFileSync } from "node:fs";

const SLACK = 0.05;

// Each budgeted file, from the repo's root: every `.md` straight under these directories,
// and these files.
export function budgeted(root: URL): string[] {
  const files = ["AGENTS.md", "README.md"];
  for (const dir of ["docs/", "cli/briefs/"]) {
    for (const name of readdirSync(new URL(dir, root))) {
      if (name.endsWith(".md")) files.push(`${dir}${name}`);
    }
  }
  return files.sort();
}

export function words(text: string): number {
  return text.split(/\s+/).filter((w) => w !== "").length;
}

export function counts(root: URL): Record<string, number> {
  return Object.fromEntries(budgeted(root).map((file) => [file, words(readFileSync(new URL(file, root), "utf8"))]));
}

// What breaks the budgets, one line a file: none when every file is within its own.
export function problems(budgets: Record<string, number>, counts: Record<string, number>): string[] {
  const found: string[] = [];
  for (const [file, count] of Object.entries(counts)) {
    const budget = budgets[file];
    if (budget === undefined) found.push(`${file} has ${count} words and no budget: add it to test/word-budgets.json, and say why in the PR`);
    else if (count > budget) found.push(`${file} has ${count} words, ${count - budget} over its budget of ${budget}: cut it, or raise the budget and say why in the PR`);
    else if (count < budget * (1 - SLACK)) found.push(`${file} has ${count} words, more than 5% under its budget of ${budget}: lower the budget to ${count}`);
  }
  for (const file of Object.keys(budgets)) {
    if (!(file in counts)) found.push(`${file} has a budget but no file: remove it from test/word-budgets.json`);
  }
  return found;
}

// The budgets with each one a trim left more than 5% above its file's count lowered to it,
// and each one whose file is gone dropped; `changes` says each, `over` is what still breaks
// them: growth and new files stay a decision, never settled here.
export function settle(budgets: Record<string, number>, counts: Record<string, number>): { settled: Record<string, number>; changes: string[]; over: string[] } {
  const settled = { ...budgets };
  const changes: string[] = [];
  for (const [file, budget] of Object.entries(budgets)) {
    const count = counts[file];
    if (count === undefined) {
      delete settled[file];
      changes.push(`${file}: dropped, the file is gone`);
    } else if (count < budget * (1 - SLACK)) {
      settled[file] = count;
      changes.push(`${file}: ${budget} to ${count}`);
    }
  }
  return { settled, changes, over: problems(settled, counts) };
}
