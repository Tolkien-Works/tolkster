// The simulator's own mutants, in a file of their own so node --test runs them beside
// machine-sim.test.ts's seeds: a dropped compare-and-swap is caught, and shrinks, and each
// outcome check catches the row it guards dropping an event's meaning.

import assert from "node:assert/strict";
import { test } from "node:test";
import { generate, LENGTH, SEEDS } from "./machine-sim-steps.ts";
import { run, shrink, type Mutant } from "./machine-sim.ts";
import { spread } from "./machine-sim-seeds.ts";

// The lowest seed that catches the mutant, each share having stopped at its own first.
async function caught(mutant: Mutant): Promise<{ seed: number; check: string }> {
  const found = (await spread(Array.from({ length: SEEDS }, (_, i) => i + 1), mutant)).flatMap((s) => (s.caught === null ? [] : [s.caught]));
  assert.ok(found.length > 0, `no seed caught ${mutant}`);
  return found.reduce((a, b) => (b.seed < a.seed ? b : a));
}

test("a write that drops the compare-and-swap is caught, and its run shrinks to the steps that fail it", async () => {
  const { seed, check } = await caught("casDropped");
  const steps = generate(seed, LENGTH);
  const min = await shrink(steps, check, "casDropped");
  assert.ok(min.length < steps.length);
  assert.equal((await run(min, "casDropped"))?.check, check);
  // What fails it is the mutant's, not the machines'.
  assert.equal(await run(min), null);
});

test("an observation dropped in launching, or an exit run beside a request's line, fails a seed's outcome check", async () => {
  assert.equal((await caught("launchDropsObservation")).check, "workAgreesWithAgent");
  assert.equal((await caught("exitBesideLine")).check, "stallAfterLine");
});

test("a round counted for a line its exiting agent never took, or a branch update never carried out, fails a seed's liveness check", async () => {
  assert.equal((await caught("typedRoundCounted")).check, "prWorkDone");
  assert.equal((await caught("noUpdateBranch")).check, "prWorkDone");
});

test("an owner's stop that posts nothing fails a seed's liveness check", async () => {
  assert.equal((await caught("stopSaysNothing")).check, "requestAnswered");
});
