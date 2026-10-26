// The simulator's seeds against docs/state-machine.md's invariants (machine-sim.ts), and
// each invariant held to a named check: the simulator's, a test's, or an issue to add one.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";
import { AT_QUIET, CHECKS } from "./machine-sim-checks.ts";
import { problems, settle, unreached } from "./machine-sim-coverage.ts";
import { DAY, generate, LENGTH, SEEDS, TRACE_SEEDS, type Step } from "./machine-sim-steps.ts";
import { replays, run } from "./machine-sim.ts";
import { ISSUES, type Replay } from "./machine-sim-world.ts";

// Each invariant, by its opening words, and what checks it: `sim:<check>` after every step
// of every seed, `<file>:<test>` a test there by its name (a Rust test by its function's),
// `row:<file>:<row>|<when>` a transition table's case there, or the issue that will add one.
const INVARIANTS: [string, string[]][] = [
  ["A transition writes only with a compare-and-swap", ["sim:casAppliesOnce", "test/machine-store.test.ts:the compare-and-swap refuses a write over one it didn't read, and runs the step again on what won"]],
  ["At most one command is in flight per issue", ["sim:oneInFlight", "sim:waitsOnlyBehindQueue", "test/machine-decide.test.ts:waiting events apply by class: the owner's request before an observation, before a deadline"]],
  ["A waiting command is sent early only when what it names clears", ["test/machine-store.test.ts:a command waiting on a host or the lock is sent again as soon as that clears, and at its timeout without it", "test/machine-store.test.ts:a command waiting again after its clearing's send spends its sends, and ends lost", "test/machine-fleet.test.ts:the fleet's command is resent at its timeout, lost after its sends, and sent early as its wait clears"]],
  ["A command result's transition runs in the state", ["sim:noWorkPastStop", "test/machine-decide.test.ts:a class 1 stop runs beside the command in flight, drops what was queued and never resends what it overtook"]],
  ["A fact is replaced only by a newer value", ["sim:factsOnlyNewer", "sim:redeliveryMovesNothing", "test/machine-decide.test.ts:a fact whose clock isn't newer is dropped, and an event with nothing newer runs no transition"]],
  ["A deadline is named by the version that armed it", ["sim:deadlinesByVersion", "test/machine-decide.test.ts:a deadline armed again, or whose state was left, is dropped", "test/machine-store.test.ts:an older run's take-back leaves a newer one owed"]],
  ["A waiting place keeps its age", ["test/machine-sim.test.ts:an issue waiting for the slot, raised in priority, keeps its place's age and takes the new priority", "test/machine-decide.test.ts:a waiting request asked again takes its issue's new priority and keeps its age"]],
  ["A recipe machine is made, woken, slept or destroyed only by a fleet command", ["test/machine-fleet.test.ts:a dormant machine is destroyed once due and free, one command at a time", "test/machine-fleet.test.ts:a request with nowhere to go makes its repo's first machine, once"]],
  ["A request with nowhere to go has a machine of its repo woken or made", ["sim:fleetAsksForWaiting", "test/machine-fleet.test.ts:a request seen while here had room gets a machine once here fills", "test/machine-fleet.test.ts:a request, a grant or a command needing an asleep machine wakes it"]],
  ["No `suspendHost` goes to a host while a grant names it", ["test/machine-fleet.test.ts:an idle host is slept only once nothing holds it awake"]],
  ["A slot is granted only by the coordinator's ledger", ["sim:slotLedger", "test/machine-store.test.ts:two issues can't both take the last slot: the second waits, then re-checks before its claim", "test/machine-requests.test.ts:each way a request ends gives back the slot reserved for it, and only a start hands it to the work"]],
  ["Every command is safe to run again", ["sim:casAppliesOnce", "test/machine-store.test.ts:a command with no result is sent again with its ID, three times in all, then ended as lost", "cli/src/command/tests/workers.rs:a_posts_activity_id_is_made_from_its_commands"]],
  ["`listen` types only fixed text", ["cli/src/orca/tests.rs:an_agent_is_launched_on_its_command_and_nothing_is_typed", "cli/src/setup/tests.rs:the_launch_line_is_plain_and_read_back_as_it_was_written", "cli/src/orca/tests.rs:a_typed_line_counts_by_its_proof_and_a_launch_line_must_be_plain", "cli/src/orca/tests.rs:a_failed_send_is_tried_again_only_as_orcas_own_answer_allows", "cli/src/orca/tests.rs:a_refused_send_is_not_typed_again_when_the_listing_fails", "cli/src/converse/tests.rs:a_handover_line_is_times_and_a_flag_only"]],
  ["The machine hands a warm turn only an owner's message", ["test/machine-requests.test.ts:a warm turn is handed a reply at once, and closed for a delegation", "row:test/machine-requests.test.ts:requested commandResult|A warm turn's `startTurn` failed or was `lost`", "cli/src/command/tests/turns.rs:an_open_turn_is_handed_the_request_and_its_close_says_nothing"]],
  ["A stop is owed until confirmed", ["sim:stopOwed", "test/machine-work-stops.test.ts:a cancel reopened with its stop unconfirmed tries the stop again in 2 minutes", "row:test/machine-work.test.ts:halted agentWorking|Its stop isn't confirmed"]],
  ["A request older than a stop or halt", ["sim:noResumeAfterStop", "test/machine-decide.test.ts:rule 8: a stop after the request supersedes it"]],
  ["A stall is the work's own mark", ["test/machine-work-starts.test.ts:a stall left to its request marks the work, not the notices: the request's own stalled still posts after a resume"]],
  ["A stall is told only once the request's own line has landed", ["sim:stallAfterLine", "test/machine-sim-mutant.test.ts:an observation dropped in launching, or an exit run beside a request's line, fails a seed's outcome check"]],
  ["A worker's state agrees with what it last saw of its agent", ["sim:workAgreesWithAgent", "test/machine-sim-mutant.test.ts:an observation dropped in launching, or an exit run beside a request's line, fails a seed's outcome check"]],
  ["Every state but `done`", ["sim:everyStateHasDeadline"]],
  ["An owner's delegation always reaches a request machine", ["test/machine-sim.test.ts:an owner's delegation on the session a store has forgotten, which Linear opens no new one for, launches a worker", "test/machine-reads.test.ts:an owner's delegation on a store that keeps no session reads the issue, and begins on the session rule 3 asks"]],
  ["Only the owner's sessions, prompts and delegations", ["test/events.test.ts:a session no one trusted, or a prompt by anyone else, makes no event", "test/events.test.ts:a delegation to Tolkster counts by the owner or Tolkster's chain, and one taken away by anyone", "cli/src/review/tests.rs:the_newest_feedback_comes_with_the_newest_reviewers", "test/intake.test.ts:a PR maps only through a worker's branch in its repo"]],
  ["The owner check, the blocker re-check", ["test/machine-decide.test.ts:rule 2: a request that isn't the owner's is refused, saying nothing", "test/machine-decide.test.ts:rule 10: the blocker re-check and the repo come before a slot", "test/machine-store.test.ts:an event's IDs, times and enums are checked before any store sees it"]],
  ["A transition reads and writes only its own issue's store", ["sim:outboxInOrder"]],
  ["A blocker's or sub-issue's state is kept by its own `updatedAt`", ["sim:blockersOnlyNewer", "test/machine-decide.test.ts:a stale read can't put back a blocker its webhook finished"]],
  ["A class 1 event never waits behind a command", ["sim:waitsOnlyBehindQueue", "sim:supersededNeverSent", "sim:noWorkPastStop"]],
  ["The owner's comment on Tolkster's issue is a live worker's or its own thread's", ["test/machine-work-notes.test.ts:work: the owner's comment on Tolkster's issue opens its thread's turn at rest, and a running worker's line instead"]],
  ["A plain comment tells a worker once", ["sim:commentToldOnce", "test/machine-store.test.ts:a comment's line that landed covers the owner's comments up to the time it carried"]],
  ["At one GitHub second", ["test/machine-decide.test.ts:at one GitHub second, a PR's later kind of event wins, and a higher-numbered PR over another"]],
  ["No machine waits on a GitHub redelivery", ["sim:everyStateHasDeadline", "test/machine-work-rounds.test.ts:a PR read older than the PR event held is stale, and a newer head is applied as a push"]],
  ["A note's thread's turn is known by its record", ["cli/src/command/tests/turns.rs:a_restart_mid_turn_neither_loses_nor_repeats_the_answer", "cli/src/command/tests/turns.rs:a_thread_turn_opens_on_its_batch_and_answers_with_its_terminal"]],
  ["A turn is held only while its `startTurn` is queued", ["sim:turnHasTerminal", "cli/src/command/tests/turns.rs:a_thread_turn_opens_on_its_batch_and_answers_with_its_terminal"]],
  ["`listen` decides nothing", ["cli/src/command/tests/workers.rs:a_command_acts_only_on_its_own_issues_things"]],
  ["A failed check at the head held starts at most one round", ["test/machine-work-rounds.test.ts:a failed check at the held head starts one round, never again for that head, and again for a new head that fails"]],
  ["No worker waits on CI", ["sim:prWorkDone", "test/machine-work-rounds.test.ts:a head still pending an hour after its push is told once, from its own read, however the work moved meanwhile", "test/machine-sim.test.ts:checks still pending an hour after the push are told to the owner once"]],
  ["A PR fact is evaluated as it lands", ["sim:evaluationsFolded", "test/machine-work-rounds.test.ts:a burst of PR facts landing while evaluatePr is in flight is evaluated once more, after its result", "test/machine-work-rounds.test.ts:a finishing worker's PR is read neither by an own deadline armed before it opened, nor a recheck, but at its exit"]],
  ["A recipe machine is sent `suspendHost` only once", ["sim:napsNeverEarly", "test/machine-fleet.test.ts:an idle host is slept only once nothing holds it awake", "test/machine-fleet.test.ts:a dormant machine is destroyed once due and free, one command at a time"]],
  ["One triage session runs at a time", ["sim:triageOneSession", "test/machine-triage.test.ts:a late result is held only from the start that last failed, and only once", "test/state.test.ts:a triage session holds an issue only while the triage machine runs it with that issue"]],
  ["Tolkster merges only by `mergePr`", ["test/machine-work-rounds.test.ts:a PR mergeable now is merged at the head read, with auto-merge on or asked for, and never one the owner holds", "cli/src/command/tests/workers.rs:a_merge_writes_only_the_issues_pr_at_the_head_read_and_counts_one_merged_already"]],
  ["Nothing acts on time passing", ["test/machine-work-rounds.test.ts:a burst of PR facts landing while evaluatePr is in flight is evaluated once more, after its result", "test/machine-work-rounds.test.ts:a merge state still UNKNOWN is read again at 10, 20 and 40 s, then left to the next fact"]],
  ["A round is counted and its markers move only by a `startRound` result", ["test/machine-work-rounds.test.ts:a round's markers are committed by a result that may have reached the worker, in a running worker too", "test/machine-work-rounds.test.ts:a startRound that fails with nothing typed counts nothing, and the next read starts the round once", "row:test/machine-work.test.ts:launching commandResult|`startRound` failed with no line typed: its proposal is dropped, so nothing is marked or counted"]],
  ["Work finishes", ["sim:prWorkDone", "sim:requestAnswered", "test/machine-sim-mutant.test.ts:a round counted for a line its exiting agent never took, or a branch update never carried out, fails a seed's liveness check", "test/machine-sim-mutant.test.ts:an owner's stop that posts nothing fails a seed's liveness check"]],
  ["The intake keeps none of an issue's facts", ["test/intake.test.ts:the intake holds none of the issue's facts: an older event reaches the store, which judges it"]],
  ["A step's records are each read once and written in one put", ["sim:oneWrite", "test/machine-store.test.ts:a step writes each record it touches once, in one put, and hands listen its commands once that has landed", "test/machine-store.test.ts:the compare-and-swap refuses a write over one it didn't read, and runs the step again on what won"]],
  ["A read reaches the machines only as the events it shows", ["test/machine-reads.test.ts:a read repairs a lost delegateChanged as the webhook would have", "test/machine-reads.test.ts:a read repairs a lost issueStateChanged as the webhook would have", "test/machine-reads.test.ts:a read repairs a lost blockersChanged as the webhook would have", "test/machine-reads.test.ts:a read repairs a lost prMerged or prClosed as the webhook would have", "row:test/machine-requests.test.ts:queued commandResult|`readIssue` succeeded"]],
];

function invariants(): string[] {
  const doc = readFileSync(new URL("../docs/state-machine.md", import.meta.url), "utf8").replace(/\r\n/g, "\n");
  const section = doc.slice(doc.indexOf("\n## Invariants\n") + 1);
  const end = section.slice(1).search(/^## /m);
  const body = end === -1 ? section : section.slice(0, end + 1);
  const bullets: string[] = [];
  for (const line of body.split("\n")) {
    if (line.startsWith("- ")) bullets.push(line.slice(2));
    else if (line.trim() !== "" && bullets.length > 0 && !line.startsWith("#")) bullets[bullets.length - 1] += ` ${line.trim()}`;
  }
  return bullets;
}

function names(ref: string): boolean {
  if (/^TW-\d+$/.test(ref)) return true;
  if (ref.startsWith("sim:")) return Object.hasOwn(CHECKS, ref.slice(4)) || Object.hasOwn(AT_QUIET, ref.slice(4));
  const row = ref.startsWith("row:");
  const rest = row ? ref.slice(4) : ref;
  const at = rest.indexOf(":");
  const [file, name] = [rest.slice(0, at), rest.slice(at + 1)];
  const source = readFileSync(new URL(`../${file}`, import.meta.url), "utf8").replace(/\r\n/g, "\n");
  if (row) {
    const [r, when] = name.split("|");
    return source.includes(`{ row: ${JSON.stringify(r)}, when: ${JSON.stringify(when)},`);
  }
  return file.endsWith(".rs") ? source.includes(`#[test]\nfn ${name}()`) : source.includes(`\ntest(${JSON.stringify(name)}`);
}

test("each invariant in the spec maps to a named check, and every check the simulator runs to one", () => {
  const bullets = invariants();
  assert.ok(bullets.length > 0);
  for (const bullet of bullets) {
    const found = INVARIANTS.filter(([opening]) => bullet.startsWith(opening));
    assert.equal(found.length, 1, `the invariant "${bullet.slice(0, 80)}…" maps to ${found.length} entries`);
  }
  for (const [opening, refs] of INVARIANTS) {
    assert.ok(bullets.some((b) => b.startsWith(opening)), `no invariant opens "${opening}"`);
    assert.ok(refs.length > 0, `"${opening}" names no check`);
    for (const ref of refs) assert.ok(names(ref), `"${opening}" names ${ref}, which isn't there`);
  }
  const used = new Set(INVARIANTS.flatMap(([, refs]) => refs).filter((r) => r.startsWith("sim:")).map((r) => r.slice(4)));
  for (const check of [...Object.keys(CHECKS), ...Object.keys(AT_QUIET)]) assert.ok(used.has(check), `the simulator's ${check} checks no invariant`);
});

const CEILINGS = new URL("./machine-sim-coverage.json", import.meta.url);
const ROOT = new URL("../", import.meta.url);

test(`the machines hold every invariant over ${SEEDS} seeded runs, and reach every branch of theirs they reached before`, () => {
  // In a child of its own, so V8 writes each seeds thread's coverage. A child of a test
  // inherits NODE_TEST_CONTEXT, which would make it report to us.
  const dir = mkdtempSync(join(tmpdir(), "machine-sim-"));
  const env: NodeJS.ProcessEnv = { ...process.env, NODE_V8_COVERAGE: dir };
  delete env.NODE_TEST_CONTEXT;
  try {
    const child = spawnSync(process.execPath, ["test/machine-sim-seeded.ts"], { cwd: fileURLToPath(ROOT), env, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, timeout: 5 * 60 * 1000 });
    if (child.status !== 0) assert.fail(`the seeded runs failed${child.error === undefined ? "" : ` (${child.error.message})`}:\n${(child.stdout + child.stderr).trimEnd().split("\n").slice(-60).join("\n")}`);
    const found = unreached(dir, ROOT, "src/worker/machine/");
    let ceilings = JSON.parse(readFileSync(CEILINGS, "utf8")) as Record<string, number>;
    if (process.env.COVERAGE === "write") {
      ceilings = settle(ceilings, found);
      writeFileSync(CEILINGS, `${JSON.stringify(ceilings, null, 2)}\n`);
    }
    assert.deepEqual(problems(ceilings, found), []);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("a block is unreached only if no thread ran it, whichever thread names it, and its ceiling holds both ways", () => {
  const root = mkdtempSync(join(tmpdir(), "machine-sim-cov-"));
  try {
    mkdirSync(join(root, "src"));
    writeFileSync(join(root, "src", "a.ts"), "function f(x) {\n  if (x) {\n    g();\n  }\n  if (!x) {\n    if (x === 0) h();\n  }\n}\n");
    const url = pathToFileURL(join(root, "src", "a.ts")).href;
    const fn = (count: number, ...blocks: [number, number, number][]) => ({ url, functions: [{ ranges: [{ startOffset: 0, endOffset: 80, count }, ...blocks.map(([startOffset, endOffset, c]) => ({ startOffset, endOffset, count: c }))] }] });
    mkdirSync(join(root, "cov"));
    // One thread ran the second `if` and not the first; another ran the first only, so names
    // the second at 0; a third never ran f. Only the innermost `if (x === 0)` ran nowhere.
    writeFileSync(join(root, "cov", "1.json"), JSON.stringify({ result: [fn(1, [25, 39, 0], [69, 73, 0])] }));
    writeFileSync(join(root, "cov", "2.json"), JSON.stringify({ result: [fn(1, [50, 77, 0])] }));
    writeFileSync(join(root, "cov", "3.json"), JSON.stringify({ result: [fn(0)] }));
    const base = pathToFileURL(`${root}/`);
    const found = unreached(join(root, "cov"), base, "src/");
    assert.deepEqual(found, { "src/a.ts": [6] });
    // Had the second `if` run nowhere, all its lines count, not one block as before.
    writeFileSync(join(root, "cov", "1.json"), JSON.stringify({ result: [fn(1, [25, 39, 0], [50, 77, 0])] }));
    assert.deepEqual(unreached(join(root, "cov"), base, "src/"), { "src/a.ts": [5, 6, 7] });
    assert.deepEqual(problems({ "src/a.ts": 1 }, found), []);
    assert.match(problems({ "src/a.ts": 0 }, found).join(), /over its ceiling of 0: 6;/);
    assert.match(problems({ "src/a.ts": 2 }, found).join(), /lower the ceiling to 1/);
    assert.match(problems({}, found).join(), /no ceiling/);
    assert.match(problems({ "src/a.ts": 1, "src/b.ts": 0 }, found).join(), /src\/b\.ts has a ceiling but no coverage/);
    assert.deepEqual(settle({ "src/a.ts": 0, "src/b.ts": 3 }, { ...found, "src/c.ts": [] }), { "src/a.ts": 0, "src/c.ts": 0 });
    assert.deepEqual(settle({ "src/a.ts": 4 }, found), { "src/a.ts": 1 });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a cancel reopened with its stop unconfirmed tries the stop again, as the seeds found it", async () => {
  const steps: Step[] = [
    { k: "delegate", i: 1 }, { k: "race", n: 1, at: 1 }, { k: "listen", n: 2, out: "ok" }, { k: "deliver", n: 6 }, { k: "listen", n: 5, out: "ok" },
    { k: "state", i: 1, to: "cancelled" }, { k: "deliver", n: 1 }, { k: "state", i: 1, to: "open" }, { k: "deliver", n: 0 },
    { k: "time", ms: 121000, away: true }, { k: "deliver", n: 5 }, { k: "time", ms: 691260000, away: true }, { k: "time", ms: 960000, away: false },
    { k: "time", ms: 90000000, away: false }, { k: "time", ms: 90000000, away: false },
  ];
  assert.equal(await run(steps), null);
});

test("a notice whose post failed goes once more, so the request it answers is told", async () => {
  const steps: Step[] = [
    { k: "mention", i: 0 }, { k: "deliver", n: 0 }, { k: "listen", n: 0, out: "ok" }, { k: "deliver", n: 0 }, { k: "listen", n: 0, out: "failed" }, { k: "deliver", n: 0 },
    { k: "listen", n: 0, out: "failed" }, { k: "deliver", n: 0 }, { k: "listen", n: 0, out: "ok" }, { k: "deliver", n: 0 },
  ];
  assert.equal(await run(steps), null);
});

test("an owner's reply decided before the worker's exit was taken resumes it, with no stalled notice", async () => {
  // The worker answers (its plan) and exits; the owner's reply lands first, goes to the
  // worker it still holds as running, whose line fails, and then the exit lands.
  const steps: Step[] = [
    { k: "delegate", i: 1 }, { k: "deliver", n: 0 }, { k: "deliver", n: 0 }, { k: "listen", n: 0, out: "ok" }, { k: "deliver", n: 0 },
    { k: "listen", n: 0, out: "ok" }, { k: "deliver", n: 0 }, { k: "agent", i: 1, status: "working" }, { k: "deliver", n: 0 },
    { k: "worker", i: 1, act: "read" }, { k: "deliver", n: 0 }, { k: "worker", i: 1, act: "answered" }, { k: "deliver", n: 0 },
    { k: "agent", i: 1, status: "exited" }, { k: "prompt", i: 1 }, { k: "deliver", n: 1 },
    { k: "listen", n: 0, out: "ok" }, { k: "deliver", n: 1 }, { k: "listen", n: 0, out: "ok" }, { k: "deliver", n: 1 }, { k: "listen", n: 0, out: "ok" }, { k: "deliver", n: 1 },
    { k: "deliver", n: 0 }, { k: "listen", n: 0, out: "ok" }, { k: "deliver", n: 0 },
  ];
  const sent: string[] = [];
  const failure = await run(steps, null, (_, __, seen) => sent.push(...seen.sent.map((s) => (s.command.kind === "postNotice" ? `postNotice:${s.command.params.notice}` : s.command.kind))));
  assert.equal(failure, null);
  assert.ok(sent.includes("observe") && sent.indexOf("observe") < sent.indexOf("resumeWorker"), sent.join(","));
  assert.ok(!sent.includes("postNotice:stalled"), sent.join(","));
});

test("a worker held at a prompt after a mention's line tells the delegation it was started for, too", async () => {
  const listened: Step[] = [{ k: "listen", n: 0, out: "ok" }, { k: "deliver", n: 0 }];
  const steps: Step[] = [
    { k: "delegate", i: 1 }, { k: "deliver", n: 0 }, { k: "deliver", n: 0 }, ...listened, ...listened, { k: "agent", i: 1, status: "working" }, { k: "deliver", n: 0 },
    { k: "mention", i: 1 }, { k: "deliver", n: 0 }, ...listened, { k: "agent", i: 1, status: "waiting" }, ...listened, ...listened,
    { k: "time", ms: 16 * 60_000 }, ...listened, ...listened,
  ];
  const told = new Set<unknown>();
  const failure = await run(steps, null, (_, __, seen) => {
    for (const { command } of seen.sent) if (command.kind === "postNotice" && command.params.notice === "heldAtPrompt") told.add(command.params.session);
  });
  assert.equal(failure, null);
  assert.equal(told.size, 2);
});

// Where the seeds found it: a request handed on to the worker before it was answered.
test("a request a worker took before another's line held it is answered or told, as seed 158 without its second fault found", async () => {
  const faults = generate(158, LENGTH).flatMap((s, i) => (s.k === "fault" ? [i] : []));
  assert.equal(await run(generate(158, LENGTH).filter((_, i) => i !== faults[1])), null);
});

// A merged or landed worker held at a prompt is closed and reaped, so a request queued on the
// reap is answered. The delegation that starts the second request is the issue's repro.
for (const [what, ending, last] of [
  ["merge", [{ k: "pr", i: 1, act: "open" }, { k: "pr", i: 1, act: "merge" }], 5],
  ["landing", [{ k: "land", i: 1 }], 4],
] as [string, Step[], number][]) {
  test(`a request queued behind a ${what}'s worker held at a prompt is answered once it is reaped`, async () => {
    const steps: Step[] = [
      { k: "delegate", i: 1 }, { k: "deliver", n: 6 }, { k: "deliver", n: 6 }, { k: "listen", n: 7, out: "ok" }, { k: "deliver", n: 1 },
      { k: "listen", n: 7, out: "ok" }, { k: "delegate", i: 0 }, ...ending, { k: "agent", i: 1, status: "waiting" },
      { k: "delegate", i: 1 }, { k: "deliver", n: last }, { k: "deliver", n: 0 },
    ];
    assert.equal(await run(steps), null);
  });
}

// The issue's worker started and seen working.
const STARTED: Step[] = [
  { k: "delegate", i: 1 }, { k: "deliver", n: 0 }, { k: "deliver", n: 0 }, { k: "listen", n: 0, out: "ok" }, { k: "deliver", n: 0 },
  { k: "listen", n: 0, out: "ok" }, { k: "deliver", n: 0 }, { k: "agent", i: 1, status: "working" }, { k: "deliver", n: 0 },
];
const OPENED: Step[] = [...STARTED, { k: "pr", i: 1, act: "open" }, { k: "deliver", n: 0 }];
const LISTENED: Step[] = [{ k: "listen", n: 0, out: "ok" }, { k: "deliver", n: 0 }];

// A worker's landing ends in `merged` and its worktree reaped on the landing's rule, after its
// exit, or after its stop when a cancel lands first.
for (const [what, steps, before] of [
  ["its exit", [...STARTED, { k: "land", i: 1 }, { k: "deliver", n: 0 }, { k: "work", i: 1 }], "running"],
  ["a cancel that lands first", [...STARTED, { k: "land", i: 1 }, { k: "state", i: 1, to: "cancelled" }, { k: "deliver", n: 1 }, { k: "deliver", n: 0 }], "halted"],
] as [string, Step[], string][]) {
  test(`a worker's landing is its merge, reaped after ${what}`, async () => {
    const states: string[] = [];
    const rules: unknown[] = [];
    const failure = await run(steps, null, (_, after, seen) => {
      const state = after.stores[ISSUES[1]]?.work?.state;
      if (state !== undefined && states.at(-1) !== state) states.push(state);
      for (const s of seen.sent) if (s.command.kind === "reapWorktree") rules.push(s.command.params.rule);
    });
    assert.equal(failure, null);
    assert.deepEqual(states.slice(-3), [before, "merged", "reaped"], states.join(","));
    assert.ok(rules.length > 0 && rules.every((r) => r === "landed"), rules.join(","));
  });
}

// The steps each command kind was sent, and each event kind landed (`landed:` its kind), at,
// in a run that holds every invariant.
async function sentAt(steps: Step[]): Promise<Record<string, number[]>> {
  const at: Record<string, number[]> = {};
  const failure = await run(steps, null, (i, _, seen) => {
    for (const s of seen.sent) (at[s.command.kind] ??= []).push(i);
    for (const l of seen.landed) (at[`landed:${l.event.kind}`] ??= []).push(i);
  });
  assert.equal(failure, null);
  return at;
}

for (const [what, event] of [["a conflict", [{ k: "base", conflict: true }]], ["a review", [{ k: "pr", i: 1, act: "review" }, { k: "deliver", n: 0 }]]] as [string, Step[]][]) {
  test(`${what} while the worker finishes after opening its PR starts its round only after the exit`, async () => {
    // The world's worker answers its request and exits.
    const at = await sentAt([...OPENED, ...event, ...LISTENED, { k: "work", i: 1 }]);
    const rounds = at.startRound ?? [];
    assert.ok(rounds.length > 0 && rounds[0] > at["landed:agentExited"][0], rounds.join(","));
  });
}

test("a review while a resumed worker runs, its PR opened before, is read at once and starts the round in it", async () => {
  const resumed: Step[] = [
    ...OPENED, { k: "worker", i: 1, act: "answered" }, { k: "deliver", n: 0 }, { k: "agent", i: 1, status: "exited" }, { k: "deliver", n: 0 },
    { k: "prompt", i: 1 }, { k: "deliver", n: 0 }, ...LISTENED, ...LISTENED, ...LISTENED, { k: "agent", i: 1, status: "working" }, { k: "deliver", n: 0 },
    { k: "pr", i: 1, act: "review" }, { k: "deliver", n: 0 },
  ];
  const at = await sentAt([...resumed, ...LISTENED]);
  assert.ok(at.evaluatePr?.includes(resumed.length - 1), (at.evaluatePr ?? []).join(","));
  assert.deepEqual(at.startRound?.[0], resumed.length + 1);
});

test("a comment whose line finds its agent gone is not a failure, and is told again when an agent works", async () => {
  const gone: Step[] = [...OPENED, { k: "line", i: 1, meets: { to: "deliverMessage", as: "gone" } }, { k: "comment", i: 1 }, { k: "deliver", n: 0 }, ...LISTENED, ...LISTENED, ...LISTENED];
  const lines: { at: number; through: unknown }[] = [];
  const results: string[] = [];
  const failure = await run([...gone, { k: "agent", i: 1, status: "working" }, { k: "deliver", n: 0 }, ...LISTENED, ...LISTENED], null, (i, _, seen) => {
    for (const s of seen.sent) if (s.command.kind === "deliverMessage" && s.command.params.comment === true) lines.push({ at: i, through: s.command.params.through });
    for (const l of seen.landed) if (l.event.kind === "commandResult" && l.event.command.includes("deliverMessage")) results.push(l.event.result);
  });
  assert.equal(failure, null);
  assert.equal(lines.length, 2, JSON.stringify(lines));
  assert.ok(lines[0].through === lines[1].through && lines[1].at >= gone.length, JSON.stringify(lines));
});

test("a worker that opens its PR and never exits gets its round an hour after it opened", async () => {
  const waiting: Step[] = [...OPENED, { k: "pr", i: 1, act: "review" }, { k: "deliver", n: 0 }];
  const at = await sentAt([...waiting, { k: "time", ms: 61 * 60_000 }]);
  const rounds = at.startRound ?? [];
  assert.ok(rounds.length > 0 && rounds[0] >= waiting.length && rounds[0] < (at["landed:agentExited"]?.[0] ?? Infinity), rounds.join(","));
});

// No worker waits on CI: it answers and exits once its PR is open, and the machine brings it
// back for a failed check, or tells the owner of checks still pending an hour after the push.
const ANSWERED: Step[] = [...OPENED, ...LISTENED, { k: "work", i: 1 }, { k: "deliver", n: 0 }];

test("a failed check after the worker answered and exited starts its round", async () => {
  // The world's third head fails its checks.
  const pushed: Step[] = [...ANSWERED, { k: "pr", i: 1, act: "push" }, { k: "deliver", n: 0 }, ...LISTENED, { k: "pr", i: 1, act: "push" }, { k: "deliver", n: 0 }, ...LISTENED];
  const at = await sentAt([...pushed, { k: "pr", i: 1, act: "checks" }, { k: "deliver", n: 0 }, ...LISTENED]);
  assert.ok((at.startRound ?? []).some((i) => i > pushed.length), (at.startRound ?? []).join(","));
});

test("checks still pending an hour after the push are told to the owner once", async () => {
  const sent: string[] = [];
  const failure = await run([...ANSWERED, { k: "time", ms: 61 * 60_000 }, ...LISTENED, ...LISTENED], null, (i, _, seen) => {
    if (i >= ANSWERED.length) sent.push(...seen.sent.filter((s) => s.command.kind === "postNotice").map((s) => String(s.command.params.notice)));
  });
  assert.equal(failure, null);
  assert.deepEqual(sent.filter((n) => n === "checksPending"), ["checksPending"]);
});

// The issue unblocked and delegated, its request decided on a finished issue, undelegated, reopened
// and settled.
const SETTLED: Step[] = [
  { k: "blocker", done: true }, { k: "delegate", i: 0 }, { k: "state", i: 0, to: "completed" }, { k: "deliver", n: 0 }, { k: "deliver", n: 0 }, { k: "deliver", n: 0 }, { k: "deliver", n: 0 },
  ...LISTENED, ...LISTENED, ...LISTENED, { k: "undelegate", i: 0 }, { k: "deliver", n: 0 }, { k: "state", i: 0, to: "open" }, { k: "deliver", n: 0 },
];

// The owner delegates it again, which Linear opens no new session for: a worker launches,
// whether or not its store has retired since.
async function delegatedAgain(retires: boolean): Promise<void> {
  const before: Step[] = [...SETTLED, ...(retires ? [{ k: "time", ms: 8 * DAY + 60_000 } as Step] : [])];
  let retired = false;
  const at: Record<string, number[]> = {};
  const failure = await run([...before, { k: "delegate", i: 0, again: true }, { k: "deliver", n: 0 }], null, (i, _, seen) => {
    retired ||= seen.retired.includes(ISSUES[0]);
    for (const s of seen.sent) (at[s.command.kind] ??= []).push(i);
  });
  assert.equal(failure, null);
  assert.equal(retired, retires);
  assert.ok((at.startWorker ?? []).some((i) => i >= before.length), (at.startWorker ?? []).join(","));
}

test("an owner's delegation on the session a store has forgotten, which Linear opens no new one for, launches a worker", () => delegatedAgain(true));

test("an owner's delegation on a session its store still holds launches a worker", () => delegatedAgain(false));

test("an issue delegated while no listen is connected starts its worker when one connects", async () => {
  const away: Step[] = [{ k: "delegate", i: 1 }, { k: "deliver", n: 0 }, { k: "deliver", n: 0 }, { k: "time", ms: 20 * 60_000, away: true }, { k: "time", ms: 60 * 60_000, away: true }];
  const at = await sentAt(away);
  assert.ok((at.startWorker ?? []).some((i) => i >= away.length), (at.startWorker ?? []).join(","));
});

// A read that doesn't show the delegation, the issue as it was before, or one created
// delegated, which no history entry names, read at a later clock than its event, whichever of
// the session and the delegation lands first.
for (const as of ["lagging", "created"] as const) {
  for (const [first, then] of [["the session", [{ k: "deliver", n: 1 }, ...LISTENED]], ["the delegation", [{ k: "deliver", n: 0 }, { k: "deliver", n: 0 }, ...LISTENED]]] as [string, Step[]][]) {
    test(`an issue whose first read shows no delegator it can name (${as}), ${first} landing first, starts`, async () => {
      const sent: string[] = [];
      const failure = await run([{ k: "delegate", i: 1, as }, ...then], null, (_, __, seen) => sent.push(...seen.sent.map((s) => (s.command.kind === "postNotice" ? `postNotice:${s.command.params.notice}` : s.command.kind))));
      assert.equal(failure, null);
      assert.ok(sent.includes("startWorker"), sent.join(","));
      if (as === "created") assert.ok(!sent.includes("postNotice:notDelegated"), sent.join(","));
    });
  }
}

// The seeds' priority steps never land while a request waits for the slot, whose windows are
// short: this run waits in one on purpose.
test("an issue waiting for the slot, raised in priority, keeps its place's age and takes the new priority", async () => {
  const listened: Step[] = Array.from({ length: 6 }, () => [{ k: "deliver", n: 0 }, ...LISTENED]).flat() as Step[];
  const steps: Step[] = [{ k: "blocker", done: true }, { k: "deliver", n: 0 }, { k: "delegate", i: 1 }, ...listened, { k: "delegate", i: 0 }, ...listened, { k: "priority", i: 0, to: 1 }, { k: "deliver", n: 0 }];
  const places: string[] = [];
  const failure = await run(steps, null, (_, after) => places.push(...after.coordinator.queue.filter((p) => p.issue === ISSUES[0]).map((p) => `${p.priority}@${p.at}`)));
  assert.equal(failure, null);
  const [first, last] = [places[0], places.at(-1)!];
  assert.deepEqual([first.split("@")[0], last.split("@")[0], first.split("@")[1] === last.split("@")[1]], ["0", "1", true]);
});

const TRACES = new URL("./listen-traces.json", import.meta.url);

function tracesText(traces: { seed: number; length: number; replays: Replay[] }[]): string {
  const trace = (t: (typeof traces)[number]) => `    { "seed": ${t.seed}, "length": ${t.length}, "replays": [\n${t.replays.map((r) => `      ${JSON.stringify(r)}`).join(",\n")}\n    ] }`;
  return `{\n  "traces": [\n${traces.map(trace).join(",\n")}\n  ]\n}\n`;
}

test("test/listen-traces.json holds the model's answers to its seeds' commands, which cargo test replays", async () => {
  const traces = [];
  for (const seed of TRACE_SEEDS) traces.push({ seed, length: LENGTH, replays: await replays(generate(seed, LENGTH)) });
  const text = tracesText(traces);
  if (process.env.TRACES === "write") writeFileSync(TRACES, text);
  assert.equal(readFileSync(TRACES, "utf8").replaceAll("\r\n", "\n"), text, "the model's answers changed: write them again with TRACES=write, then run cargo test");
});
