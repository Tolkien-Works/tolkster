// A command's lifecycle, the same for an issue's queue (step.ts) and the triage machine's
// one command (triage.ts): how it is made, sent, timed out and sent again, and how time
// spent with nothing listening counts (docs/state-machine.md, "Commands").
//
// While its machine holds `listen` away, a command in flight that waits on `listen` runs no
// timeout: it isn't sent again to no one, nor ended `lost` while it may still be running.
// `listenConnected` sends it again at once, counted as one of its sends, and begins its
// timeout again; one that has had all its sends is ended `lost` instead, so a command that
// keeps `listen` from staying connected still ends.

import { COMMAND_TIMEOUT_MS, SENDS, START_TIMEOUT_MS, type Clears, type Command, type CommandKind, type MachineRef, type WaitsOn } from "./types.ts";

// Posted by the Durable Object itself, never sent to `listen`: whether `listen` is away
// doesn't hold it, and a connection isn't sent it.
export const LOCAL: readonly CommandKind[] = ["postAway"];

export function newCommand(id: string, issue: string, kind: CommandKind, machine: MachineRef, version: number, params: Record<string, unknown>, class1: boolean): Command {
  return { id, issue, kind, machine, version, params, class1, firstSentMs: null, sentMs: null, sends: 0, superseded: false };
}

// Marks the command sent now, and returns what goes to `listen`.
export function sent(c: Command, nowMs: number): Command {
  c.firstSentMs ??= nowMs;
  c.sentMs = nowMs;
  c.sends += 1;
  c.waitsOn = null;
  c.cleared = false;
  return { ...c };
}

export function timeoutOf(c: Command): number {
  return c.kind === "startWorker" || c.kind === "startTriage" || c.kind === "destroyHost" ? START_TIMEOUT_MS : COMMAND_TIMEOUT_MS;
}

export function inFlight(c: Command): boolean {
  return c.sentMs !== null;
}

// In flight, and waiting on `listen` for its result: what `listen`'s absence holds. A
// superseded command is never sent again, so its timeouts run on to `lost` regardless, and
// the Durable Object's own posts aren't `listen`'s.
export function waitsOnListen(c: Command): boolean {
  return c.sentMs !== null && !c.superseded && !LOCAL.includes(c.kind);
}

// When the command's timeout falls, or null while it waits for nothing: unsent, or held by
// `listen`'s absence.
export function dueOf(c: Command, away: boolean): number | null {
  if (c.sentMs === null || (away && waitsOnListen(c))) return null;
  return c.sentMs + timeoutOf(c);
}

// The one resend-or-lose rule: a command past its timeout is sent again with the same ID,
// SENDS times in all, and then ended as `lost`.
export function expiry(c: Command, nowMs: number, away: boolean): "wait" | "resend" | "lost" {
  const due = dueOf(c, away);
  if (due === null || due > nowMs) return "wait";
  return c.sends < SENDS ? "resend" : "lost";
}

// `listen` has connected after its machine held it away: a command waiting on it is sent
// again now, as a send, its timeout beginning again, as time spent away is no failure; null
// once it has had its SENDS, when it is ended `lost`.
export function resent(c: Command, nowMs: number): Command | null {
  return c.sends < SENDS ? { ...c, sentMs: nowMs, sends: c.sends + 1, waitsOn: null, cleared: false } : null;
}

// Whether `clears` frees what `waitsOn` names.
export function frees(clears: Clears, waitsOn: WaitsOn | null | undefined): boolean {
  if (waitsOn == null) return false;
  return clears.all === true || ("host" in waitsOn ? clears.hosts?.includes(waitsOn.host) === true : "orca" in waitsOn ? clears.orca === true : clears.lock === true);
}

// Whether the command, in flight and answered `retryable`, waits on what `clears` frees, with a
// send left should this one be spent.
export function clearedBy(c: Command, clears: Clears): boolean {
  return c.sentMs !== null && !c.superseded && frees(clears, c.waitsOn) && spent(c) <= SENDS;
}

// What it waited on has cleared: it is sent again now, and its timeout begins again. It waits
// on nothing until it is answered again. The first such send spends nothing, as it was only
// waiting; one after a send that was itself a clearing's is spent, so a command that waits
// again on what keeps clearing (a host that answers while the command fails otherwise) still
// ends `lost`.
export function resentCleared(c: Command, nowMs: number): Command {
  return { ...c, sentMs: nowMs, sends: spent(c), waitsOn: null, cleared: true };
}

// Its sends once a clearing has sent it.
function spent(c: Command): number {
  return c.sends + Number(c.cleared === true);
}
