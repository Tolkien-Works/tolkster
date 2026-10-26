// The spec's invariants (docs/state-machine.md), as machine-sim.ts checks them after every
// step, and what they remember across steps.

import { place } from "../src/worker/machine/coordinator.ts";
import { FLEET } from "../src/worker/machine/fleet.ts";
import type { TriageSession } from "../src/worker/machine/triage.ts";
import { AGENT_STATES, AT_REST_WORK, MINUTE, SENDS, type Event, type Store } from "../src/worker/machine/types.ts";
import { AGENT_REPORTS } from "../src/worker/machine.ts";
import type { Seen, View } from "./machine-sim.ts";
import { ISSUES, RECIPE_REPO, REPO, stuck, type World } from "./machine-sim-world.ts";

function retiredIn(seen: Seen): Set<string> {
  return new Set(seen.retired);
}

// What the checks remember across steps.
export class Memo {
  // The world's triage sessions, as the step saw them.
  triage: { open: TriageSession[]; opened: TriageSession[] } = { open: [], opened: [] };
  // Each recipe host every complete listing since `since` has named idle, with its minutes.
  naps: Record<string, { since: number; minutes: number }> = {};
  // Each recipe machine every complete listing naming any since `since` has named dormant.
  dormant: Record<string, { since: number; minutes: number }> = {};
  napsSent = new Set<string>();
  // Each command's and outbox event's content by its ID.
  commands = new Map<string, string>();
  events = new Map<string, string>();
  // Commands ever seen superseded.
  superseded = new Set<string>();
  // Issues whose agent was found at work, or whose round left rest, holding a slot past the
  // cap while it stays, and the grants at the last quiet step.
  pastCap = new Set<string>();
  granted = new Set<string>();
  // The order of each issue's worker's last start, resume, round or line, and its newest
  // observation, and the order it landed in.
  started = new Map<string, number>();
  observed = new Map<string, { kind: string; order: number }>();
  // Each session's newest `startTurn` sent or seen queued, the request it was for, and how
  // often it had been sent.
  turnStarts = new Map<string, { id: string; request: unknown; sends: number }>();
}

// ---- The invariants, as checks run after every step ----

export type CheckFn = (before: View, after: View, seen: Seen, memo: Memo) => string | null;

type Machine = { state: string; version: number; deadlines: { kind: string; version: number; dueMs: number; fired?: boolean }[] };

function machines(s: Store): [string, Machine][] {
  const out: [string, Machine][] = [];
  if (s.work !== null) out.push(["work", s.work]);
  for (const [id, sess] of Object.entries(s.sessions)) if (sess.machine !== null) out.push([id, sess.machine]);
  return out;
}

// The issue's store before the step, unless the step retired it.
function prior(before: View, seen: Seen, issue: string): Store | undefined {
  return retiredIn(seen).has(issue) ? undefined : before.stores[issue];
}

export function quiet(v: View): boolean {
  return Object.values(v.stores).every((s) => s.outbox.length === 0) && v.coordinator.outbox.length === 0;
}

function first(found: (string | null)[]): string | null {
  return found.find((f) => f !== null) ?? null;
}

const STARTS = ["startWorker", "resumeWorker", "startRound", "deliverMessage"];
const NO_DEADLINE = ["done", "reaped"];
const STOPPED = ["halted", "merged"];
const AGENT: readonly string[] = AGENT_REPORTS;

export const CHECKS: Record<string, CheckFn> = {
  // A write applies once: no store's counters go back, retired or not, and an ID names one
  // command and one event, which a write over another it didn't read would break.
  casAppliesOnce(before, after, seen, memo) {
    for (const [issue, s] of Object.entries(after.stores)) {
      const b = before.stores[issue];
      if (b === undefined) continue;
      for (const key of ["rev", "seq", "commandSeq"] as const) if (s[key] < b[key]) return `${issue}'s ${key} went back from ${b[key]} to ${s[key]}`;
    }
    if (after.coordinator.seq < before.coordinator.seq) return "the coordinator's seq went back";
    for (const { command: c } of seen.sent) {
      const body = JSON.stringify([c.kind, c.machine, c.version, c.params]);
      const held = memo.commands.get(c.id);
      if (held !== undefined && held !== body) return `command ${c.id} named two commands: ${held} and ${body}`;
      memo.commands.set(c.id, body);
    }
    for (const s of [...Object.values(after.stores), after.coordinator]) {
      for (const o of s.outbox) {
        const body = JSON.stringify(o.event);
        const held = memo.events.get(o.id);
        if (held !== undefined && held !== body) return `event ${o.id} named two events`;
        memo.events.set(o.id, body);
      }
    }
    return null;
  },

  // At most one command in flight per issue, besides a class 1 event's, each sent SENDS
  // times at most.
  oneInFlight(_b, after) {
    for (const [issue, s] of Object.entries(after.stores)) {
      const flying = s.queue.filter((c) => !c.class1 && c.sentMs !== null);
      if (flying.length > 1) return `${issue} has ${flying.length} commands in flight: ${flying.map((c) => c.kind).join(", ")}`;
      const over = s.queue.find((c) => c.sends > SENDS);
      if (over !== undefined) return `${issue}'s ${over.kind} was sent ${over.sends} times`;
    }
    return null;
  },

  // An event waits only behind a command, and a class 1 event never does.
  waitsOnlyBehindQueue(_b, after) {
    for (const [issue, s] of Object.entries(after.stores)) {
      if (s.waiting.length > 0 && s.queue.length === 0) return `${issue} has events waiting with nothing queued: ${s.waiting.map((w) => w.event.kind).join(", ")}`;
      const class1 = s.waiting.find((w) => w.cls === 1);
      if (class1 !== undefined) return `${issue}'s class 1 ${class1.event.kind} waits`;
    }
    return null;
  },

  // A burst of PR facts is one evaluation: no PR fact waits as an event, and an issue never
  // has two reads of its PR queued.
  evaluationsFolded(_b, after) {
    const EVALUATES = ["prPushed", "prReviewed", "checksCompleted", "autoMergeDisabled", "mergeAsked", "basePushed"];
    for (const [issue, s] of Object.entries(after.stores)) {
      const waiting = s.waiting.find((w) => EVALUATES.includes(w.event.kind));
      if (waiting !== undefined) return `${issue}'s ${waiting.event.kind} waits as an event`;
      const n = s.queue.filter((c) => c.kind === "evaluatePr").length;
      if (n > 1) return `${issue} has ${n} evaluatePr queued`;
    }
    return null;
  },

  // Nothing starts, resumes or messages a worker that is halted, judged by the
  // store each command was sent from.
  noWorkPastStop(_b, _a, seen) {
    return first(seen.sent.map(({ command: c, store: s }) => {
      const w = s?.work;
      if (c.machine !== "work" || !STARTS.includes(c.kind) || w == null) return null;
      return w.state === "halted" ? `${c.kind} sent on ${c.issue} while its work was ${w.state}` : null;
    }));
  },

  // A request older than a stop or halt never resumes the worker.
  noResumeAfterStop(_b, _a, seen) {
    return first(seen.sent.map(({ command: c, store: s }) => {
      if (s === undefined || c.machine !== "work" || !["resumeWorker", "deliverMessage"].includes(c.kind) || c.params.comment === true) return null;
      const halted = s.facts.halted;
      const by = s.work?.requestedBy;
      if (halted === null || by == null) return null;
      return by.at < halted.at ? `${c.kind} on ${c.issue} for a request at ${by.at}, older than its halt at ${halted.at}` : null;
    }));
  },

  // A deadline is armed at its machine's version, one of each kind, and one armed in a
  // state left is dropped.
  deadlinesByVersion(before, after, seen) {
    for (const [issue, s] of Object.entries(after.stores)) {
      const p = prior(before, seen, issue);
      for (const [ref, m] of machines(s)) {
        const kinds = m.deadlines.map((d) => d.kind);
        if (new Set(kinds).size !== kinds.length) return `${issue}'s ${ref} has two deadlines of a kind: ${kinds.join(", ")}`;
        const ahead = m.deadlines.find((d) => d.version > m.version);
        if (ahead !== undefined) return `${issue}'s ${ref} has a deadline at version ${ahead.version}, past its own ${m.version}`;
        const b = p && machines(p).find(([r]) => r === ref)?.[1];
        if (b === undefined || b.state === m.state) continue;
        // A take-back owed and a head's checks' deadline outlive a move.
        const stale = m.deadlines.find((d) => d.kind !== "takeBack" && d.kind !== "pending" && d.version <= b.version);
        if (stale !== undefined) return `${issue}'s ${ref} left ${b.state} for ${m.state} keeping its ${stale.kind} deadline`;
      }
    }
    return null;
  },

  // Every state but done, reaped and a start's launching sets a deadline, still to fire or
  // fired and waiting its turn: none is dropped with its machine left to wait.
  everyStateHasDeadline(_b, after) {
    for (const [issue, s] of Object.entries(after.stores)) {
      for (const [ref, m] of machines(s)) {
        if (NO_DEADLINE.includes(m.state) || (m.state === "launching" && "launch" in m && m.launch === "start")) continue;
        const own = m.deadlines.find((d) => d.kind === "own");
        if (own === undefined) return `${issue}'s ${ref} is ${m.state} with no deadline`;
        const waits = s.waiting.some((w) => w.event.kind === "deadline" && w.event.machine === ref && w.event.version === own.version && w.event.deadline === "own");
        if (own.fired === true && !waits) return `${issue}'s ${ref} is ${m.state} with its deadline fired and gone`;
      }
    }
    return null;
  },

  // A stop is owed until its close is confirmed, through a merge too: a stopWorker is queued,
  // or one goes again within its 2 minutes.
  stopOwed(_b, after) {
    for (const [issue, s] of Object.entries(after.stores)) {
      const w = s.work;
      if (w === null || w.stop === null || !STOPPED.includes(w.state)) continue;
      if (s.queue.some((c) => c.kind === "stopWorker" && !c.superseded)) continue;
      const own = w.deadlines.find((d) => d.kind === "own");
      if (own === undefined || own.dueMs > after.nowMs + 2 * MINUTE) return `${issue} owes a stop in ${w.state} with no stopWorker due`;
    }
    return null;
  },

  // A slot is held only through the coordinator's ledger: each work at an agent's state
  // holds a grant there, each grant is held by work or reserved for a request, and no more
  // are granted than there are slots, nor on a host than its cap, but for an agent found at
  // work or a review round, which take one past the cap: only new work waits for one.
  slotLedger(before, after, seen, memo) {
    for (const [issue, s] of Object.entries(after.stores)) {
      const was = prior(before, seen, issue)?.work?.state;
      const now = s.work?.state;
      if (now === undefined || !AGENT_STATES.includes(now)) memo.pastCap.delete(issue);
      else if ((now !== "launching" || s.work!.launch === "round") && was !== undefined && AT_REST_WORK.includes(was)) memo.pastCap.add(issue);
    }
    if (!quiet(after)) return null;
    const c = after.coordinator;
    for (const [issue, s] of Object.entries(after.stores)) {
      const agent = s.work !== null && AGENT_STATES.includes(s.work.state);
      const g = c.grants[issue];
      if (agent && (g === undefined || g.session !== null)) return `${issue}'s work is ${s.work!.state} with no slot held in the ledger`;
      if (!agent && g !== undefined && g.session === null) return `${issue} holds a slot with its work ${s.work?.state ?? "none"}`;
      if (g !== undefined && g.session !== null && s.sessions[g.session]?.facts.grant == null) return `${issue}'s slot is reserved for ${g.session}, which doesn't know it`;
    }
    // Past the cap, every grant since the last quiet step is an agent found at work's.
    const granted = Object.keys(c.grants);
    const since = granted.filter((i) => !memo.granted.has(i));
    memo.granted = new Set(granted);
    const over = since.find((i) => !memo.pastCap.has(i));
    if (granted.length > c.slots && over !== undefined) return `${over} was granted a slot with ${granted.length} granted, of ${c.slots}`;
    const on = (host: string | null) => granted.filter((i) => (c.grants[i].host ?? null) === host).length;
    for (const i of since.filter((i) => !memo.pastCap.has(i))) {
      const host = c.grants[i].host ?? null;
      const cap = host === null ? (after.room.cap ?? c.slots) : after.room.hosts.find((h) => h.id === host)?.cap;
      if (cap !== undefined && on(host) > cap) return `${i} was granted a slot on ${host ?? "this machine"} with ${on(host)} there, of ${cap}`;
    }
    return null;
  },

  // A request of the recipe's repo with nowhere to go has a machine asked for once the step's
  // reconcile has run, unless a machine command is in flight or a make or wake runs, or the
  // fleet records why not: a cool-down, a machine of it still to be heard, or all made.
  fleetAsksForWaiting(_before, after) {
    const r = after.run;
    if (r.command !== null || r.running !== null) return null;
    const c = after.coordinator;
    const waits = c.queue.find((p) => c.grants[p.issue] === undefined && p.on === undefined && p.repo === REPO && place(c, after.room, p) === null);
    const why = r.why[RECIPE_REPO];
    if (waits === undefined || why === "coolingDown" || why === "allMade" || why === "hostComing") return null;
    return `${waits.issue} waits for a machine, with none asked for (${why ?? "no reason"})`;
  },

  // A fact is replaced only by a newer value from its own clock; a merged PR stays merged.
  factsOnlyNewer(before, after, seen) {
    for (const [issue, s] of Object.entries(after.stores)) {
      const b = prior(before, seen, issue)?.facts;
      const f = s.facts;
      if (b === undefined) continue;
      for (const key of ["delegate", "issueState", "priority", "repo"] as const) {
        const was = b[key]?.at;
        const now = f[key]?.at;
        if (was !== undefined && (now === undefined || now < was)) return `${issue}'s ${key} went back from ${was} to ${now}`;
      }
      if (b.pr !== null && b.pr.status === "merged" && f.pr !== null && f.pr.number === b.pr.number && f.pr.status !== "merged") return `${issue}'s merged PR became ${f.pr.status}`;
    }
    return null;
  },

  // A blocker's state is kept by its own updatedAt, so a stale read never puts back a
  // finished blocker.
  blockersOnlyNewer(before, after, seen) {
    for (const [issue, s] of Object.entries(after.stores)) {
      const was = prior(before, seen, issue)?.facts.blockers?.states ?? {};
      const now = s.facts.blockers?.states ?? {};
      for (const [id, b] of Object.entries(was)) {
        const n = now[id];
        if (n !== undefined && n.at < b.at) return `${issue}'s blocker ${id} went back from ${b.at} to ${n.at}`;
        if (n !== undefined && b.done && !n.done && n.at <= b.at) return `${issue}'s finished blocker ${id} was put back`;
      }
    }
    return null;
  },

  // An event that carries nothing newer, redelivered, moves no machine. An owner's comment
  // is the exception: its delivery's dedupe is the intake's, and the machine reads it again,
  // which tells nothing twice (commentToldOnce).
  redeliveryMovesNothing(before, after, seen) {
    if (seen.again === null || seen.again.kind === "ownerCommented") return null;
    for (const [issue, s] of Object.entries(after.stores)) {
      const b = prior(before, seen, issue);
      if (b === undefined) continue;
      for (const [ref, m] of machines(s)) {
        const was = machines(b).find(([r]) => r === ref)?.[1];
        if (was === undefined || was.version !== m.version) return `a redelivery moved ${issue}'s ${ref} from ${was?.state ?? "none"} to ${m.state}`;
      }
    }
    return null;
  },

  // The command a class 1 event overtakes is never sent again.
  supersededNeverSent(_b, after, seen, memo) {
    // Superseded by the time it was sent: in an earlier step, or as its copy shows.
    const resent = seen.sent.find(({ command: c }) => c.superseded || memo.superseded.has(c.id));
    if (resent !== undefined) return `superseded ${resent.command.kind} ${resent.command.id} was sent again`;
    for (const s of Object.values(after.stores)) for (const c of s.queue) if (c.superseded) memo.superseded.add(c.id);
    return null;
  },

  // A comment's line is passed on once: none is sent for comments a line that landed had
  // covered when it was sent.
  commentToldOnce(before, after, seen) {
    for (const [issue, s] of Object.entries(after.stores)) {
      const was = prior(before, seen, issue)?.facts.commentTold ?? null;
      if (was !== null && (s.facts.commentTold === null || s.facts.commentTold < was)) return `${issue}'s commentTold went back`;
    }
    return first(seen.sent.map(({ command: c, store: s }) => {
      const told = s?.facts.commentTold ?? null;
      if (c.kind !== "deliverMessage" || c.params.comment !== true || told === null || c.sends > 1) return null;
      return (c.params.through as string) <= told ? `${c.issue} passed on comments through ${c.params.through}, told through ${told}` : null;
    }));
  },

  // An event a store sends has an ID of its kind, its store and its outbox sequence, and one
  // store's events go out in order; what crosses issues goes as an event to the other.
  outboxInOrder(before, after, seen) {
    for (const [issue, s] of Object.entries(after.stores)) {
      let last = 0;
      for (const o of s.outbox) {
        if (o.id !== `${o.event.kind}:${issue}:${o.seq}`) return `${issue}'s outbox event ${o.id} isn't named by its kind, store and sequence ${o.seq}`;
        if (o.seq <= last) return `${issue}'s outbox is out of order`;
        last = o.seq;
        if (o.to !== "coordinator" && (o.event as Event).issue !== o.to) return `${issue} sent ${o.event.kind} to ${o.to} naming another issue`;
      }
      for (const [from, seq] of Object.entries(prior(before, seen, issue)?.heard ?? {})) {
        if ((s.heard[from] ?? 0) < seq) return `${issue}'s heard from ${from} went back`;
      }
    }
    return null;
  },

  // One triage session at a time: the machine's writes never go back, the world never has
  // two sessions open, and the session the machine holds is one listen opened, with the
  // issues it was given.
  triageOneSession(before, after, _seen, memo) {
    for (const key of ["rev", "commandSeq"] as const) if (after.triage[key] < before.triage[key]) return `the triage machine's ${key} went back`;
    const { open, opened } = memo.triage;
    if (open.length > 1) return `${open.length} triage sessions are open: ${open.map((s) => s.id).join(", ")}`;
    const held = after.triage.session;
    if (held !== null && !opened.some((s) => JSON.stringify(s) === JSON.stringify(held))) return `the triage machine holds ${held.id}, which listen never opened as it is`;
    if (after.triage.state === "running" && held === null) return "the triage machine runs no session";
    return null;
  },

  // A recipe host is sent its suspend only once every listing the fleet took has named it idle
  // for its idle minutes, and a machine its destroy likewise dormant; a resend, or one to a
  // connection, is the same command.
  napsNeverEarly(_before, after, seen, memo) {
    for (const { command } of seen.sent.filter((s) => s.command.issue === FLEET && (s.command.kind === "suspendHost" || s.command.kind === "destroyHost") && !memo.napsSent.has(s.command.id))) {
      memo.napsSent.add(command.id);
      const [key, nap] = command.kind === "destroyHost" ? [String(command.params.name), memo.dormant[command.params.name as string]] : [String(command.params.host), memo.naps[command.params.host as string]];
      if (nap === undefined) return `${command.id} (${command.kind}) names ${key}, which the newest listing doesn't`;
      if (after.nowMs < nap.since + nap.minutes * MINUTE) return `${command.id} (${command.kind}) names ${key} ${after.nowMs - nap.since} ms into its ${nap.minutes} minutes`;
    }
    return null;
  },

  // The work's state agrees with what it has seen of its agent: once no start is left to
  // land and no observation waits, a worker the machine holds as there wasn't last seen exiting
  // since its last start, nor one running past its first turn idle or waiting. The fact's own
  // `sinceMs` can't say: a resumed agent that exits again leaves it at the first exit.
  workAgreesWithAgent(_b, after, seen, memo) {
    for (const { command: c, order } of seen.sent) if (c.machine === "work" && STARTS.includes(c.kind)) memo.started.set(c.issue, order);
    for (const { event: e, order } of seen.landed) if (AGENT.includes(e.kind)) memo.observed.set(e.issue, { kind: e.kind, order });
    for (const [issue, s] of Object.entries(after.stores)) {
      const w = s.work;
      const seenLast = memo.observed.get(issue);
      const since = memo.started.get(issue);
      if (w === null || seenLast === undefined || since === undefined || seenLast.order <= since) continue;
      if (s.queue.some((c) => c.machine === "work" && STARTS.includes(c.kind)) || s.waiting.some((x) => AGENT.includes(x.event.kind))) continue;
      if (seenLast.kind === "agentExited" && ["running", "idle", "held"].includes(w.state)) return `${issue}'s work is ${w.state} with its agent seen exiting since its last start`;
      if ((seenLast.kind === "agentIdle" || seenLast.kind === "agentWaiting") && w.state === "running" && w.firstTurn == null) return `${issue}'s work is running past its first turn after ${seenLast.kind}`;
    }
    return null;
  },

  // A stall is told only once the request's own line has landed: no `stalled` notice is queued
  // behind a line to its session still to land.
  stallAfterLine(_b, after) {
    for (const [issue, s] of Object.entries(after.stores)) {
      for (const [i, c] of s.queue.entries()) {
        if (c.kind !== "postNotice" || c.params.notice !== "stalled") continue;
        const line = s.queue.slice(0, i).find((d) => d.kind === "deliverMessage" && !d.superseded && d.params.session === c.params.session);
        if (line !== undefined) return `${issue} told ${c.params.session} it stalled with its line still to land`;
      }
    }
    return null;
  },

  // A turn is held only while its `startTurn` is queued or its terminal is known. A start
  // ended `lost` leaves the turn to take the terminal its idle reports, and so does a `held`,
  // which only the simulator's listen gives a turn (listen holds only a line), as it gives
  // the `lost` the Durable Object's timeouts give. The simulator's listen gives every note's
  // thread's turn a terminal: it makes none with nothing in its batch, which runs in none.
  turnHasTerminal(_b, after, seen, memo) {
    const unanswered = new Set(seen.landed.flatMap(({ event: e }) => (e.kind === "commandResult" && (e.result === "held" || e.result === "lost") ? [e.command] : [])));
    for (const { command: c } of seen.sent) if (c.kind === "startTurn") memo.turnStarts.set(c.machine, { id: c.id, request: c.params.request, sends: c.sends });
    for (const [issue, s] of Object.entries(after.stores)) {
      for (const [id, sess] of Object.entries(s.sessions)) {
        const start = s.queue.find((c) => c.machine === id && c.kind === "startTurn");
        if (start !== undefined) memo.turnStarts.set(id, { id: start.id, request: start.params.request, sends: start.sends });
        const last = memo.turnStarts.get(id);
        const turn = sess.facts.turn;
        if (turn === null || turn.terminal !== null || start !== undefined) continue;
        // Excused only for the turn that start was for, never a later one.
        if (last !== undefined && last.request === turn.for && (last.sends >= SENDS || unanswered.has(last.id))) memo.turnStarts.set(id, { ...last, sends: SENDS });
        else return `${issue}'s session ${id} holds a turn with no terminal and no startTurn queued`;
      }
    }
    return null;
  },

  // An event's delivery, facts and transition are written in one write, with the store's
  // due time whenever it moves.
  oneWrite(_b, _a, seen) {
    return seen.undue.length === 0 ? null : `a store was written without its due time: ${seen.undue.join(", ")}`;
  },
};

// ---- What the machines claim to deliver, checked against the world once a run is quiet ----

export type QuietFn = (world: World, after: View) => string | null;

// The notices that leave a PR's work to the owner. A stall isn't one: it answers the request,
// and the PR's next round still starts the worker again.
const WAITS_ON_OWNER = ["roundsCap", "ciCouldntRun", "checksPending", "roundUnnamed", "heldAtPrompt"];

export const AT_QUIET: Record<string, QuietFn> = {
  // No open PR of a worker whose issue wants work is left `DIRTY`, `BEHIND` a base the machine
  // has read, with feedback no round acted on, a failed check or checks that never settle, unless the owner was
  // told at its head why it waits on them.
  prWorkDone(world, after) {
    for (const [i, w] of world.issues.entries()) {
      const p = w.pr;
      // A note's reply asks for an answer in its thread, not for work.
      const asked = [...world.requests.values()].filter((r) => r.i === i && r.kind !== "note").map((r) => r.at).sort().at(-1);
      const wants = w.delegate === "owner" && w.state === "open" && w.worktree && asked !== undefined && (w.stoppedAt === null || asked > w.stoppedAt);
      // Reaped, the work has no worker: its worktree was removed by hand.
      if (p === null || p.status !== "open" || !wants || after.stores[ISSUES[i]]?.work?.state === "reaped") continue;
      if (world.told.some((t) => t.i === i && t.head === p.head && WAITS_ON_OWNER.includes(t.notice))) continue;
      const f = after.stores[ISSUES[i]]?.facts;
      // A PR whose every event was lost is one the machine never heard of.
      if (f?.pr?.number !== p.number) continue;
      const read = f !== undefined && (f.base === world.base || f.mergeState?.base === world.base);
      const left = p.conflict ? "DIRTY" : p.base !== world.base && read ? "BEHIND" : p.feedbackAt !== null && (p.answeredAt === null || p.feedbackAt > p.answeredAt) ? "with feedback unanswered" : p.checks === "failed" ? "with a failed check" : p.checks === "pending" && stuck(p) ? "with checks that never settle" : null;
      if (left !== null) return `${ISSUES[i]}'s PR ${p.number} is left ${left}, its work ${after.stores[ISSUES[i]]?.work?.state ?? "none"}, with no notice at its head`;
    }
    return null;
  },

  // Every owner's request the machine heard of is answered, or its session told something,
  // once its issue's newest delegation's.
  requestAnswered(world, after) {
    for (const [session, r] of world.requests) {
      const w = world.issues[r.i];
      const s = after.stores[ISSUES[r.i]];
      const newest = w.sessions.findLast((x) => x.kind === "delegation")?.id;
      if ((r.kind === "delegation" && session !== newest) || !world.heard.has(session)) continue;
      if (r.answered === null || r.answered < r.asked) return `${ISSUES[r.i]}'s request at ${r.at} in ${session} was never answered or told, its machine ${s?.sessions[session]?.machine?.state ?? "none"}`;
    }
    return null;
  },
};
