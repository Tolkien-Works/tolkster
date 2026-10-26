// The triage machine (docs/state-machine.md, "Triage"): one for the owner, beside the
// coordinator, which starts one triage session at a time (docs/triage.md) and holds the
// session it started, so `tolkster triage` is checked against it. The pure step,
// (machine, event) → (machine, commands to send); store.ts keeps it with a compare-and-swap
// on `rev`, sends its command, and fires its deadline and its command's timeouts.
//
// Every value is IDs, identifiers, times and enums: never issue or comment text.

import { dueOf, expiry, newCommand, resent, sent, waitsOnListen } from "./queue.ts";
import { MINUTE, type Command, type CommandResult } from "./types.ts";

// The issue and identifier its commands carry, which no issue's are: listen runs them in a
// lane of their own.
export const TRIAGE = "triage";
// Its record in the Durable Object's storage.
export const TRIAGE_KEY = "machine:triage";

export const TRIAGE_STATES = ["idle", "starting", "running", "retrying"] as const;
export type TriageState = (typeof TRIAGE_STATES)[number];
export const TRIAGE_COMMANDS = ["startTriage", "observeTriage"] as const;
export type TriageCommand = (typeof TRIAGE_COMMANDS)[number];
export const TRIAGE_EVENTS = ["triageAsked", "triageEnded", "listenConnected", "listenGone"] as const;

// `running`'s deadline: Orca's listing read again, for a session's end the watcher missed
// (one that closed while nothing listened).
export const TRIAGE_OBSERVE_MS = 10 * MINUTE;
// A start that failed is sent again this long after, doubling each time to the last, and
// after this many failures in a row the machine rests until the next ask.
export const TRIAGE_RETRY_FIRST_MS = 30 * 1000;
export const TRIAGE_RETRY_LAST_MS = 15 * MINUTE;
export const TRIAGE_RETRIES = 20;
// A held session that ends this soon after it was held never got to work, as when Claude
// fails to launch, which takes seconds; reading one issue and posting its verdict takes far
// longer. Counted as a failed start, so it is retried with the backoff, not at once.
export const TRIAGE_QUICK_END_MS = 30 * 1000;
// The issues asked about kept for the next start; the oldest go first past it, and the
// queue's read still finds them.
export const MAX_ASKED = 50;
export const MAX_ENDED = 16;

// The session a start opened: its id, which its brief carries and `tolkster triage` proves
// itself with, its Orca terminal, and the identifiers of the issues it was given.
export type TriageSession = { id: string; terminal: string; issues: string[] };

export type Triage = {
  // The compare-and-swap's revision, moved by every write.
  rev: number;
  state: TriageState;
  // Moved by every transition that sends a command or arms a deadline; a deadline names
  // the version that armed it.
  version: number;
  deadline: { version: number; dueMs: number } | null;
  commandSeq: number;
  // The one command in flight.
  command: Command | null;
  // Issues created in Triage that no start has settled yet, by ID: the next start reads each
  // fresh beside the queue, so the queue's one page never hides a new one.
  asked: string[];
  session: TriageSession | null;
  // When the session was held, by the Durable Object's clock.
  since: number | null;
  // The last start that failed, which alone may still be held should its result come late.
  lost: string | null;
  // The terminals closed while a start's result was still to come, the newest MAX_ENDED, whatever
  // ran in them: that result, if its session ran in one, counts as a failed start, as the
  // session ended before the machine held it.
  ended: string[];
  // Starts failed in a row.
  failures: number;
  // `listen` was found gone while the command was in flight, and hasn't connected since: its
  // timeouts don't run (queue.ts). Cleared once no command is in flight.
  away: boolean;
};

// `waiting`: the asked issues, by ID, a start passed by for now (past the batch, in another
// repo, or with a history it couldn't read), which stay asked.
export type TriageData = { session?: TriageSession; waiting?: string[]; open?: boolean };

export type TriageEvent =
  | { kind: "triageAsked"; id: string; issue: string }
  // A terminal closed: the session's, if it ran in it.
  | { kind: "triageEnded"; id: string; terminal: string }
  | { kind: "commandResult"; id: string; command: string; result: CommandResult; data?: TriageData }
  | { kind: "deadline"; id: string; version: number }
  // The Durable Object, as `listen` connects, or as its alarm finds nothing listening.
  | { kind: "listenConnected" | "listenGone"; id: string };

export function newTriage(): Triage {
  return { rev: 0, state: "idle", version: 0, deadline: null, commandSeq: 0, command: null, asked: [], session: null, since: null, lost: null, ended: [], failures: 0, away: false };
}

export type TriageStepped = { triage: Triage; send: Command[] };

function send(t: Triage, kind: TriageCommand, params: Record<string, unknown>, nowMs: number): Command {
  t.commandSeq += 1;
  t.command = newCommand(`${TRIAGE}.${t.commandSeq}`, TRIAGE, kind, TRIAGE, t.version, params, false);
  return sent(t.command, nowMs);
}

function arm(t: Triage, afterMs: number, nowMs: number): void {
  t.deadline = { version: t.version, dueMs: nowMs + afterMs };
}

// A start, for the issues asked so far and the queue.
function start(t: Triage, nowMs: number): Command {
  t.state = "starting";
  t.version += 1;
  t.deadline = null;
  t.session = null;
  t.since = null;
  t.ended = [];
  t.lost = null;
  return send(t, "startTriage", { issueIds: [...t.asked] }, nowMs);
}

// Failures in a row are counted on until a session runs past TRIAGE_QUICK_END_MS, or a
// start finds nothing to triage.
function run(t: Triage, session: TriageSession, nowMs: number): TriageStepped {
  t.state = "running";
  t.session = session;
  t.since = nowMs;
  t.lost = null;
  t.version += 1;
  arm(t, TRIAGE_OBSERVE_MS, nowMs);
  return { triage: t, send: [] };
}

function failed(t: Triage, nowMs: number, lost: string | null): Command[] {
  t.failures += 1;
  t.lost = lost;
  t.session = null;
  t.since = null;
  t.version += 1;
  if (t.failures >= TRIAGE_RETRIES) {
    t.state = "idle";
    t.failures = 0;
    t.deadline = null;
    return [];
  }
  t.state = "retrying";
  arm(t, Math.min(TRIAGE_RETRY_FIRST_MS * 2 ** (t.failures - 1), TRIAGE_RETRY_LAST_MS), nowMs);
  return [];
}

// Applies one event. The machine returned is the one given when nothing changed. It holds
// `listen` away only while a command waits on it.
export function stepTriage(before: Triage, e: TriageEvent, nowMs: number): TriageStepped {
  const out = transition(before, e, nowMs);
  if (out.triage !== before && out.triage.away && out.triage.command === null) out.triage.away = false;
  return out;
}

function transition(before: Triage, e: TriageEvent, nowMs: number): TriageStepped {
  const unchanged = { triage: before, send: [] };
  const t = structuredClone(before);
  switch (e.kind) {
    case "triageAsked": {
      const known = t.asked.includes(e.issue);
      if (!known) t.asked = [...t.asked, e.issue].slice(-MAX_ASKED);
      if (t.state === "idle") return { triage: t, send: [start(t, nowMs)] };
      // A start under way, or waiting to be sent again, takes it; a running session's end
      // starts one that does.
      return known ? unchanged : { triage: t, send: [] };
    }
    case "triageEnded": {
      // A session that closed at once, before its start's result, is heard first, when its
      // terminal isn't known yet: the newest one closed is kept for that result to match.
      if (t.state === "starting" && !t.ended.includes(e.terminal)) {
        t.ended = [...t.ended, e.terminal].slice(-MAX_ENDED);
        return { triage: t, send: [] };
      }
      if (t.state !== "running" || t.session?.terminal !== e.terminal) return unchanged;
      if (t.since !== null && nowMs - t.since < TRIAGE_QUICK_END_MS) return { triage: t, send: failed(t, nowMs, null) };
      t.failures = 0;
      return { triage: t, send: [start(t, nowMs)] };
    }
    case "listenGone": {
      if (t.away || t.command === null || !waitsOnListen(t.command)) return unchanged;
      t.away = true;
      return { triage: t, send: [] };
    }
    case "listenConnected": {
      // Time with nothing listening is no failure: a command held away is sent again, or
      // ended `lost` once it has had its sends (queue.ts, `resent`), one in flight otherwise
      // runs on as it was, and a start owed is sent now.
      if (t.command !== null) {
        if (!t.away) return unchanged;
        t.away = false;
        const again = resent(t.command, nowMs);
        if (again === null) return transition(t, { kind: "commandResult", id: `lost:${t.command.id}`, command: t.command.id, result: "lost" }, nowMs);
        t.command = again;
        return { triage: t, send: [{ ...again }] };
      }
      if (t.state === "retrying" || (t.state === "idle" && t.asked.length > 0)) return { triage: t, send: [start(t, nowMs)] };
      return unchanged;
    }
    case "deadline": {
      if (t.deadline === null || t.deadline.version !== e.version) return unchanged;
      if (t.state === "retrying") return { triage: t, send: [start(t, nowMs)] };
      if (t.state === "running" && t.session !== null) {
        t.deadline = null;
        t.version += 1;
        return { triage: t, send: [send(t, "observeTriage", { session: t.session.id, terminal: t.session.terminal }, nowMs)] };
      }
      return unchanged;
    }
    case "commandResult": {
      const command = t.command;
      const late = e.data?.session;
      if (command === null || command.id !== e.command) {
        // The start that last failed, ended `lost`, opened its session after all: held,
        // rather than left to run beside the next one, while no other start is in flight.
        if ((t.state === "idle" || t.state === "retrying") && e.command === t.lost && e.result === "ok" && late !== undefined && !t.ended.includes(late.terminal)) return run(t, late, nowMs);
        return unchanged;
      }
      t.command = null;
      if (command.kind === "startTriage") {
        if (e.result !== "ok" || (late !== undefined && t.ended.includes(late.terminal))) return { triage: t, send: failed(t, nowMs, command.id) };
        const sent = (command.params.issueIds as string[] | undefined) ?? [];
        const waiting = e.data?.waiting ?? [];
        t.asked = t.asked.filter((a) => !sent.includes(a) || waiting.includes(a));
        if (late !== undefined) return run(t, late, nowMs);
        t.failures = 0;
        // Nothing to triage: an ask that came during the start is started now. One passed
        // by waits for the next ask or session's end.
        if (t.asked.some((a) => !sent.includes(a))) return { triage: t, send: [start(t, nowMs)] };
        t.state = "idle";
        t.version += 1;
        return { triage: t, send: [] };
      }
      // observeTriage, in `running`: the session's terminal gone ends it; anything else
      // reads again at the next deadline.
      if (t.state !== "running") return { triage: t, send: [] };
      if (e.result === "ok" && e.data?.open === false) {
        t.failures = 0;
        return { triage: t, send: [start(t, nowMs)] };
      }
      t.version += 1;
      arm(t, TRIAGE_OBSERVE_MS, nowMs);
      return { triage: t, send: [] };
    }
  }
}

// When the machine is next due: its deadline, or its command's timeout unless it holds
// `listen` away. Then the command stays in flight, for `listenConnected` to give its
// timeouts afresh, so a start that is still running isn't ended `lost` and sent again
// beside it.
export function triageDue(t: Triage): number | null {
  const due = [t.deadline?.dueMs, t.command === null ? null : dueOf(t.command, t.away)].filter((d): d is number => d != null);
  return due.length === 0 ? null : Math.min(...due);
}

// The alarm's turn: the command's resend or loss (queue.ts, `expiry`), or a deadline due.
export function expireTriage(before: Triage, nowMs: number): TriageStepped {
  const c = before.command;
  const next = c === null ? "wait" : expiry(c, nowMs, before.away);
  if (c !== null && next === "resend") {
    const t = structuredClone(before);
    return { triage: t, send: [sent(t.command!, nowMs)] };
  }
  if (c !== null && next === "lost") return stepTriage(before, { kind: "commandResult", id: `lost:${c.id}`, command: c.id, result: "lost" }, nowMs);
  if (before.deadline !== null && before.deadline.dueMs <= nowMs) {
    return stepTriage(before, { kind: "deadline", id: `${TRIAGE}:deadline:${before.deadline.version}`, version: before.deadline.version }, nowMs);
  }
  return { triage: before, send: [] };
}

// Whether `session` is the running session and was given `identifier`: the check
// `tolkster triage` makes before its verdict.
export function triaging(t: Triage | undefined, session: string, identifier: string): boolean {
  return t?.state === "running" && t.session?.id === session && t.session.issues.includes(identifier);
}
