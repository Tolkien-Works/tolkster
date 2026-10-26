// Seeds dealt out across worker threads, so node --test's seeded runs use more than one core.
// Each thread runs its share of the seeds in this module, and the test that dealt them
// judges what every share reports, together.

import { readFileSync } from "node:fs";
import { availableParallelism } from "node:os";
import { isMainThread, parentPort, Worker, workerData } from "node:worker_threads";
import { generate, LENGTH, type Step } from "./machine-sim-steps.ts";
import { run, shrink, type Mutant } from "./machine-sim.ts";

// What a share of the seeds reached, states and commands, and its first failure, naming its
// seed and the shortest run that still fails it; with a mutant, the first seed that caught it.
export type Share = { reached: string[]; failure: { seed: number; message: string } | null; caught: { seed: number; check: string } | null };

type Job = { seeds: number[]; mutant: Mutant | null };

// node --test runs the two simulator files at once, so each gets half the cores, as far as
// the container's CPU quota allows (availableParallelism can report the host's).
function threads(): number {
  let cores = availableParallelism();
  try {
    const [max, period] = readFileSync("/sys/fs/cgroup/cpu.max", "utf8").trim().split(" ");
    const quota = Math.ceil(Number(max) / Number(period));
    if (quota >= 1) cores = Math.min(cores, quota);
  } catch {}
  return Math.max(1, Math.min(8, Math.floor(cores / 2)));
}

// Dealt out in turn, so each share holds as many long seeds as short, and in order, so a
// share's first failure is its lowest seed's, and the lowest of the shares' is the run's.
export async function spread(seeds: number[], mutant: Mutant | null = null): Promise<Share[]> {
  const n = threads();
  return Promise.all(Array.from({ length: n }, (_, j) => share({ seeds: seeds.filter((_, i) => i % n === j), mutant })));
}

function share(job: Job): Promise<Share> {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL(import.meta.url), { workerData: job });
    worker.once("message", resolve);
    worker.once("error", reject);
    worker.once("exit", (code) => reject(new Error(`a seeds worker exited with ${code} before it reported`)));
  });
}

// A failure names its seed and the shortest run that still fails, as JSON, for a
// regression test to replay with `run`.
async function fails(seed: number, steps: Step[]): Promise<string> {
  const failure = (await run(steps))!;
  const min = await shrink(steps, failure.check);
  const again = await run(min);
  return `seed ${seed} fails ${failure.check}: ${again?.message ?? failure.message}\n${JSON.stringify(min)}`;
}

async function runShare({ seeds, mutant }: Job): Promise<Share> {
  const reached = new Set<string>();
  for (const seed of seeds) {
    const steps = generate(seed, LENGTH);
    if (mutant !== null) {
      const failure = await run(steps, mutant);
      if (failure !== null) return { reached: [], failure: null, caught: { seed, check: failure.check } };
      continue;
    }
    const failure = await run(steps, null, (_at, after, seen) => {
      for (const s of Object.values(after.stores)) {
        if (s.work !== null) reached.add(s.work.state);
        for (const sess of Object.values(s.sessions)) if (sess.machine !== null) reached.add(sess.machine.state);
      }
      for (const { command } of seen.sent) reached.add(command.kind);
      reached.add(`triage:${after.triage.state}`);
    });
    if (failure !== null) return { reached: [...reached], failure: { seed, message: await fails(seed, steps) }, caught: null };
  }
  return { reached: [...reached], failure: null, caught: null };
}

if (!isMainThread) parentPort!.postMessage(await runShare(workerData as Job));
