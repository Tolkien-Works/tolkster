// The simulator's seeded runs, which machine-sim.test.ts runs in a child of its own with V8's
// coverage on, so it can hold the lines they leave unrun to their ceilings. Not a .test.ts
// file, so `npm test` doesn't run the seeds a second time without it.

import assert from "node:assert/strict";
import { test } from "node:test";
import { COMMANDS, REQUEST_STATES, WORK_STATES } from "../src/worker/machine/types.ts";
import { TRIAGE_STATES } from "../src/worker/machine/triage.ts";
import { SEEDS, TRACE_SEEDS } from "./machine-sim-steps.ts";
import { spread } from "./machine-sim-seeds.ts";

// Why the lines machine-sim-coverage.json counts go unreached: the shapes the world never sends,
// validate.ts's optional fields and rejections and the store's failed writes, which
// machine.test.ts and machine-store.test.ts cover, and the `??` defaults of a field the world always sets.

test(`the machines hold every invariant over ${SEEDS} seeded runs of random, redelivered and reordered events`, async () => {
  // The traces' seeds past these run too: they were picked for what they reach, as an open PR
  // merged, which the first seeds reach rarely.
  const seeds = [...Array.from({ length: SEEDS }, (_, i) => i + 1), ...TRACE_SEEDS.filter((s) => s > SEEDS)];
  const shares = await spread(seeds);
  const [failed] = shares.flatMap((s) => (s.failure === null ? [] : [s.failure])).sort((a, b) => a.seed - b.seed);
  if (failed !== undefined) assert.fail(failed.message);
  // So a change that stops the seeds reaching a state or a command fails here, not silently:
  // what every share reached, together.
  const reached = new Set(shares.flatMap((s) => s.reached));
  const missed = [...WORK_STATES, ...REQUEST_STATES, ...COMMANDS, ...TRIAGE_STATES.map((t) => `triage:${t}`)].filter((x) => !reached.has(x));
  assert.deepEqual(missed, []);
});
