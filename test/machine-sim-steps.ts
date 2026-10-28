// The simulator's steps (machine-sim.ts): a seeded RNG, and the steps it generates from a
// seed up front, each one total, so a failing run replays from its JSON alone.

import type { AgentStatus, IssueStateType } from "../src/worker/machine/types.ts";

// ---- The seeded RNG: mulberry32, written out so no dependency is needed. ----

export type Rng = { next(): number; int(n: number): number; pick<T>(xs: readonly T[]): T };

export function rng(seed: number): Rng {
  let a = seed >>> 0;
  const next = () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  return { next, int: (n) => Math.floor(next() * n), pick: (xs) => xs[Math.floor(next() * xs.length)] };
}

// ---- The steps ----

export type PrAct = "open" | "push" | "review" | "copilot" | "checks" | "merge" | "close" | "autoOff";
export type Outcome = "ok" | "failed" | "retryable" | "lost" | "held" | "gone" | "none";
// What the issue's next round, line or stop meets in a live agent: one that exits before its
// turn begins, its exit reported after the result or before it, or one nothing names; a message
// that finds the agent gone just as it exits.
export type Meets = { to: "startRound" | "deliverMessage"; as: "typed" | "exiting" } | { to: "deliverMessage"; as: "gone" } | { to: "startRound" | "stopWorker"; as: "unnamed" };

export type Step =
  // The owner, in Linear: a delegation with its session, or on an issue with one none, as
  // Linear opens (`created`: the issue created delegated, which no history entry names, and
  // moved on after; `lagging`: one the next read doesn't show), a mention with its own, a prompt or a stop in the newest
  // delegation's session, or with `mention` a stop in the newest mention's, an undelegation,
  // the issue's state, a comment, a reply to a note of Tolkster's or with `root` a comment
  // rooting one, and the blocker's state.
  | { k: "delegate"; i: number; as?: "created" | "lagging" }
  | { k: "mention"; i: number }
  | { k: "prompt"; i: number }
  | { k: "stop"; i: number; mention?: true }
  | { k: "undelegate"; i: number }
  | { k: "state"; i: number; to: IssueStateType }
  | { k: "comment"; i: number }
  | { k: "note"; i: number; root: boolean }
  | { k: "blocker"; done: boolean }
  // The owner sets the issue's priority, 0 (none) to 4.
  | { k: "priority"; i: number; to: number }
  // GitHub, on the issue's PR.
  | { k: "pr"; i: number; act: PrAct }
  // The issue's worker lands its change on the default branch with no PR.
  | { k: "land"; i: number }
  // What Orca shows of the issue's agent, as listen reports it, and the worker's own
  // reports that it read or answered the session it was last given.
  | { k: "agent"; i: number; status: AgentStatus }
  | { k: "worker"; i: number; act: "read" | "answered" }
  // The world: the repo's default branch pushed, conflicting with each open PR or not, the
  // issue's agent acting on what it took, and what its next line meets.
  | { k: "base"; conflict: boolean }
  // The sub-issue moved under its parent, or off it.
  | { k: "child"; act: "add" | "leave" }
  | { k: "work"; i: number }
  | { k: "line"; i: number; meets: Meets }
  // What the issue's PR's next branch update meets: a push landing first, a conflict, or
  // GitHub failing it.
  | { k: "update"; i: number; meets: "moved" | "refused" | "failed" }
  // The relay asks the issue's store for the context of its completion, as its webhook comes.
  | { k: "context"; i: number }
  // listen carries out the n-th command sent and not yet carried out; `none` sends no result.
  | { k: "listen"; n: number; out: Outcome }
  // The network: the n-th event waiting is delivered, dropped, or delivered with the next
  // one interleaved at its `at`-th storage read; an event already applied comes again.
  | { k: "deliver"; n: number }
  | { k: "drop"; n: number }
  | { k: "race"; n: number; at: number }
  | { k: "again"; n: number }
  // The next storage write fails.
  | { k: "fault" }
  // The clock moves on and the alarm runs; `away` while nothing listens, with the next
  // event waiting delivered while the relay posts an away message.
  | { k: "time"; ms: number; away?: boolean }
  // Triage: the issue created in Triage, the open session's terminal closing, `listen`
  // connecting again, and listen carrying out the n-th triage command sent.
  | { k: "triage"; act: "ask" | "end" | "connect"; i: number }
  | { k: "triageListen"; n: number; out: Outcome }
  // listen failing every start of a triage session, until triage gives up.
  | { k: "triageDown" }
  // Naps: a complete listing naming idle the recipe hosts of the mask `idle` (NAP_HOSTS), with
  // `minutes`, and dormant the machines of the mask `dormant` (NAP_MACHINES), with `minutes`
  // too, or, null, leaving them out as while a machine is made, and listen carrying out the
  // n-th nap command sent.
  | { k: "naps"; idle: number; minutes: number; dormant: number | null }
  | { k: "napsListen"; n: number; out: Outcome };

export const NAP_HOSTS = ["h1", "h2"];
export const NAP_MACHINES = ["m1", "m2"];

const MIN = 60_000;
export const DAY = 24 * 60 * MIN;
// The last is past a settled store's retirement (RETIRE_AFTER_MS).
const TIMES = [1000, 30_000, 2 * MIN + 1000, 10 * MIN + 1000, 16 * MIN, 61 * MIN, 7 * 60 * MIN, 25 * 60 * MIN, 8 * DAY + MIN];

// Each seed runs at a level of chaos: 0 delivers in order and mostly succeeds, so a run gets
// deep into the work (a PR, its rounds, a merge); 2 reorders, drops, races and fails often.
function weights(chaos: number): [number, (r: Rng) => Step][] {
  const any = (r: Rng) => (chaos === 0 ? 0 : r.int(8));
  const outcomes = chaos === 0 ? (["ok"] as const) : (["ok", "ok", "ok", "ok", "failed", "retryable", "lost", "held", "none"] as const);
  return [
    [30, (r) => ({ k: "deliver", n: any(r) })],
    [22, (r) => ({ k: "listen", n: any(r), out: r.pick(outcomes) })],
    [8, (r) => ({ k: "time", ms: r.pick(chaos === 0 ? TIMES.slice(0, 3) : TIMES), away: chaos > 0 && r.int(3) === 0 })],
    [chaos === 0 ? 2 : 4, (r) => ({ k: "delegate", i: r.int(2) })],
    [5, (r) => ({ k: "prompt", i: r.int(2) })],
    [1 + chaos, (r) => ({ k: "stop", i: r.int(2) })],
    [chaos, (r) => ({ k: "undelegate", i: r.int(2) })],
    [1 + chaos, (r) => ({ k: "state", i: r.int(2), to: r.pick(["completed", "cancelled", "open", "open"] as const) })],
    [2, (r) => ({ k: "comment", i: r.int(2) })],
    [2, (r) => ({ k: "blocker", done: r.int(3) > 0 })],
    [10, (r) => ({ k: "pr", i: r.int(2), act: r.pick(["open", "open", "push", "review", "review", "copilot", "checks", "merge", "close", "autoOff"] as const) })],
    // Weighty enough that a launched agent is seen working and then idle: before its first turn, an idle one is ignored.
    [12, (r) => ({ k: "agent", i: r.int(2), status: r.pick(["working", "idle", "idle", "waiting", "exited", "exited", "gone"] as const) })],
    [4, (r) => ({ k: "worker", i: r.int(2), act: r.pick(["read", "answered"] as const) })],
    [chaos * 2, (r) => ({ k: "drop", n: r.int(8) })],
    [chaos * 2, (r) => ({ k: "again", n: r.int(16) })],
    [chaos * 2, (r) => ({ k: "race", n: r.int(8), at: 1 + r.int(6) })],
    [chaos, () => ({ k: "fault" })],
  ];
}

// Triage's steps come from a stream of their own, between the others, so they change nothing
// of what the issues' steps are.
function triageStep(r: Rng, chaos: number): Step {
  const outcomes = chaos === 0 ? (["ok"] as const) : (["ok", "ok", "ok", "failed", "retryable", "lost", "none"] as const);
  return r.int(2) === 0 ? { k: "triageListen", n: r.int(4), out: r.pick(outcomes) } : { k: "triage", act: r.pick(["ask", "ask", "end", "connect"] as const), i: r.int(2) };
}

// Naps' steps likewise, from a stream of their own: listings mostly name a host idle, so its
// clock runs out between them.
function napStep(r: Rng, chaos: number): Step {
  const outcomes = chaos === 0 ? (["ok"] as const) : (["ok", "ok", "failed", "retryable", "none"] as const);
  return r.int(3) === 0 ? { k: "napsListen", n: r.int(2), out: r.pick(outcomes) } : { k: "naps", idle: r.pick([1, 1, 2, 3, 3, 0]), minutes: r.pick([30, 30, 45]), dormant: r.pick([1, 2, 3, 3, 0, null]) };
}

// The seeds machine-sim-seeded.ts runs, 1 to SEEDS, and each run's length.
export const SEEDS = 200;
export const LENGTH = 150;

// The seeds whose commands cli/src/command/tests/replays.rs replays through listen's own command path
// on its fakes, with the model's answers, between them each kind replayed that the seeds reach
// (`closeAgent` in 2293, `stopWorker` in 1072), each result the model gives listen's own (ok,
// failed (1722) and held (87)), turns (1931), a reap that removes for a merged PR and one that
// keeps for an agent running (1614) and one of a completion with no PR (2293), and a start that
// made its worktree (87), one that found it with its agent there (18) and one that named it and
// launched on the brief there (1722), one with none that it didn't name (3132), an orchestrating one (2293), a
// resume (81), a read of a sub-issue's parent and of a parent's sub-issues (2293), and of a
// reply to a note and of a comment rooting one (87). Past them the seeds reach what the first
// don't: a push from a PR's read (2755), a PR's round past its cap for a reviewer and its notice
// otherwise (1722), a branch update refused past the cap, moved or failed (1931) and one whose
// head moved before it ended (3318), and a resume that failed, a round's line typed or unnamed
// and a PR opened at rest (3132); and what machine-sim-mutant.test.ts's mutants need, a round
// typed into an agent that exits (472) and an observation while launching (832).
export const TRACE_SEEDS = [18, 81, 87, 472, 832, 1072, 1614, 1722, 1931, 2293, 2755, 3132, 3318];

// The world's steps likewise: a base pushed, an agent at work, and what a line or a branch
// update meets.
function worldStep(r: Rng): Step {
  const x = r.int(9);
  if (x < 3) return { k: "base", conflict: r.int(3) === 0 };
  if (x < 6) return { k: "work", i: r.int(2) };
  if (x < 7) return { k: "update", i: r.int(2), meets: r.pick(["moved", "refused", "failed"] as const) };
  const as = r.pick(["typed", "typed", "exiting", "unnamed"] as const);
  const to = r.pick(as === "unnamed" ? (["startRound", "stopWorker"] as const) : (["startRound", "startRound", "deliverMessage"] as const));
  // A message the agent exits before taking finds the agent gone when it is a comment's, which the world answers `gone`.
  return { k: "line", i: r.int(2), meets: (to === "deliverMessage" && as === "typed" ? { to, as: "gone" } : { to, as }) as Meets };
}

// Mentions likewise, in the most chaotic third of the seeds only, so a run reaches a
// conversation's turn while the others, the deepest into the work, stay as they were. In a
// quarter of the seeds, at every level of chaos, CHILD starts as a sub-issue of PARENT, half
// the owner's delegations of it go to its parent instead, so its parent's chain delegates it,
// and steps of their own move it, set the parent's agent to work or halt it. Notes, contexts
// and a mention's stop come from a stream of their own too, rarely, as each sets listen more
// to carry out, and never in a chain seed, which runs as it was; so does which of the owner's
// delegations the issue is created with, or its next read lags behind. Priorities change in a
// seventh of the seeds, none of TRACE_SEEDS, so the two issues' places in the queue swap.
// Workers land their change with no PR in another seventh, none of TRACE_SEEDS either.
export function generate(seed: number, length: number): Step[] {
  const t = rng(seed + 7919);
  const m = rng(seed + 104_729);
  const g = rng(seed + 15_485_863);
  const c = rng(seed + 32_452_843);
  const n = rng(seed + 49_979_687);
  const d = rng(seed + 86_028_121);
  const p = rng(seed + 179_424_673);
  const a = rng(seed + 122_949_829);
  const q = rng(seed + 198_491_317);
  const l = rng(seed + 236_887_691);
  const chains = seed % 4 === CHAIN_SEEDS;
  const steps = issueSteps(seed, length).flatMap((issueStep) => {
    let step: Step = chains && issueStep.k === "delegate" && issueStep.i === CHILD && c.int(2) === 0 ? { k: "delegate", i: PARENT } : issueStep;
    if (!chains && step.k === "delegate" && a.int(4) === 0) step = { ...step, as: a.pick(["created", "lagging"] as const) };
    let steps: Step[] = t.int(4) === 0 ? [step, triageStep(t, seed % 3)] : [step];
    if (g.int(8) === 0) steps = [...steps, worldStep(g)];
    if (chains && c.int(12) === 0) steps = [...steps, chainStep(c)];
    if (seed % 40 === 3 && d.int(60) === 0) steps = [...steps, { k: "triageDown" }];
    if (p.int(6) === 0) steps = [...steps, napStep(p, seed % 3)];
    if (seed % 7 === 5 && q.int(8) === 0) steps = [...steps, { k: "priority", i: q.int(2), to: q.int(5) }];
    if (seed % 7 === 2 && l.int(4) === 0) steps = [...steps, { k: "land", i: l.int(2) }];
    if (!chains && n.int(24) === 0) steps = [...steps, noteStep(n, seed % 3 === 2)];
    return seed % 3 === 2 && m.int(16) === 0 ? [...steps, { k: "mention", i: m.int(2) } as const] : steps;
  });
  return chains ? [{ k: "child", act: "add" }, ...steps] : steps;
}

// The seeds, by their remainder mod 4, that make a parent and its sub-issue. The parent is
// the second issue and the first its sub-issue, which its blocker can hold up as the chain's.
const CHAIN_SEEDS = 1;
export const PARENT = 1;
export const CHILD = 0;

// A chain seed's own steps: the sub-issue moved, the parent's agent at work, or the parent
// halted, taken from Tolkster or cancelled.
function chainStep(c: Rng): Step {
  const x = c.int(10);
  if (x < 2) return { k: "child", act: c.int(2) === 0 ? "add" : "leave" };
  if (x < 9) return { k: "work", i: PARENT };
  return c.int(2) === 0 ? { k: "undelegate", i: PARENT } : { k: "state", i: PARENT, to: "cancelled" };
}

// A note's step: the owner's reply to a note or comment rooting one, the relay's ask for a
// context, and, in the seeds that make mentions, a stop in a mention's session.
function noteStep(n: Rng, mentions: boolean): Step {
  const x = n.int(mentions ? 6 : 4);
  if (x < 2) return { k: "note", i: n.int(2), root: n.int(2) === 0 };
  if (x < 4) return { k: "context", i: n.int(2) };
  return { k: "stop", i: n.int(2), mention: true };
}

function issueSteps(seed: number, length: number): Step[] {
  const r = rng(seed);
  const table = weights(seed % 3);
  const total = table.reduce((n, [w]) => n + w, 0);
  const steps: Step[] = [];
  for (let i = 0; i < length; i++) {
    let x = r.int(total);
    for (const [w, make] of table) {
      if (x < w) {
        steps.push(make(r));
        break;
      }
      x -= w;
    }
  }
  return steps;
}
