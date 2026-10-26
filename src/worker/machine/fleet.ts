// What `listen` says of its hosts and settings (docs/realtime.md, Commands and reports), kept as
// observed state in one record: the config report's settings and the newest listing's host
// entries. It decides the coordinator's slot total, as `hosts::total` sums it in
// cli/src/hosts.rs; test/contract.json's `fleet` holds cases both sides sum the same. Names
// and numbers only.
//
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node.

import { HOST } from "../state.ts";
import { byTurn, place, type Coordinator, type Room } from "./coordinator.ts";
import type { Storage } from "../storage.ts";
import { clearedBy, dueOf, expiry, newCommand, resentCleared, sent } from "./queue.ts";
import { MINUTE, type Clears, type Command, type CommandKind, type CommandResult, type WaitsOn } from "./types.ts";

export const FLEET_KEY = "machine:fleet";
// The most workers `listen` may say run at once (cli/src/hosts.rs, MAX_SLOTS).
export const MAX_SLOTS = 100;
// The most hosts, repos or machines a report names; more fails it.
const MAX_ENTRIES = 1000;
// The most ready projects kept of a host (cli/src/orca.rs, MAX_READY).
const MAX_READY = 100;

// `revision`: the image revision its machines are brought to, as its last `prepare` said, null
// before one has.
export type Recipe = { id: string; machines: number; slotsEach: number; idleMinutes: number; destroyAfter: number; revision: string | null };
export type Made = { repo: string; made: boolean; asleep: boolean; environment: string | null; revision: string | null };
// The config report's: this machine's cap, `slots` by host name, each repo's recipe settings by
// repo, and each recipe machine `machines.json` records, by name.
export type Settings = { cap: number; slots: Record<string, number>; repos: Record<string, Recipe>; machines: Record<string, Made> };
// A paired host as the listing has it: `ready`, the projects set up and ready there, none for a
// silent one; `held`, the workers on it; `admitted`, as `listen` read it, for display only.
export type Heard = { id: string; name: string; cap: number; admitted: boolean; answering: boolean; ready: string[]; held: number };
// `held`: this machine's workers; `projects`: the project (`github:owner/name`) of each of its
// Orca repos, by repo ID, as its worktrees' origins say; `recipes`: the repo each of them is in
// the settings' `repos`, by repo ID, for those with a recipe.
export type Hosts = { held: number; hosts: Heard[]; projects: Record<string, string>; recipes?: Record<string, string> };
export type Fleet = { settings: Settings | null; hosts: Hosts | null };

// Some text, no control character, at most 100 characters (cli/src/hosts.rs, `plain_name`).
const NAME = /^(?=.*\S)[^\u0000-\u001f\u007f-\u009f]{1,100}$/u;
const PROJECT = /^[A-Za-z0-9_.:\/-]{1,200}$/;
const REVISION = /^[A-Za-z0-9_.:-]{1,100}$/;
// An Orca repo's ID, as a worktree's ID holds it (state.ts, WORKTREE).
const REPO_ID = /^[A-Za-z0-9_-]{1,64}$/;

const count = (v: unknown): v is number => typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
const name = (v: unknown): v is string => typeof v === "string" && NAME.test(v);
const object = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const entries = (v: unknown): [string, unknown][] | null => (object(v) && Object.keys(v).length <= MAX_ENTRIES ? Object.entries(v).filter(([key]) => key !== "__proto__") : null);

function recipe(v: unknown): Recipe | null {
  if (!object(v) || !name(v.id) || ![v.machines, v.slotsEach, v.idleMinutes, v.destroyAfter].every(count)) return null;
  const revision = v.revision ?? null;
  if (revision !== null && (typeof revision !== "string" || !REVISION.test(revision))) return null;
  return { id: v.id, machines: v.machines as number, slotsEach: v.slotsEach as number, idleMinutes: v.idleMinutes as number, destroyAfter: v.destroyAfter as number, revision };
}

function made(v: unknown): Made | null {
  if (!object(v) || !name(v.repo) || typeof v.made !== "boolean" || typeof v.asleep !== "boolean") return null;
  const environment = v.environment ?? null;
  const revision = v.revision ?? null;
  if (environment !== null && (typeof environment !== "string" || !HOST.test(environment))) return null;
  if (revision !== null && (typeof revision !== "string" || !REVISION.test(revision))) return null;
  return { repo: v.repo, made: v.made, asleep: v.asleep, environment, revision };
}

// Each entry whose key and value read, the rest left out.
function each<T>(listed: [string, unknown][], read: (v: unknown) => T | null): Record<string, T> {
  return Object.fromEntries(listed.flatMap(([key, v]) => {
    const value = read(v);
    return name(key) && value !== null ? [[key, value]] : [];
  }));
}

// A config report's settings, or null for one of the wrong shape.
export function parseSettings(r: Record<string, unknown>): Settings | null {
  const [slots, repos, machines] = [entries(r.slots), entries(r.repos), entries(r.machines)];
  if (!count(r.cap) || r.cap > MAX_SLOTS || slots === null || repos === null || machines === null) return null;
  return { cap: r.cap, slots: each(slots, (v) => (count(v) ? v : null)), repos: each(repos, recipe), machines: each(machines, made) };
}

// A name that doesn't read goes as the ID, and a ready project that doesn't is left out, so
// one host's odd setup never fails the entries (cli/src/hosts.rs keeps to the same shapes).
function heard(v: unknown): Heard | null {
  if (!object(v) || typeof v.id !== "string" || !HOST.test(v.id) || !count(v.cap) || !count(v.held)) return null;
  if (typeof v.admitted !== "boolean" || typeof v.answering !== "boolean" || !Array.isArray(v.ready)) return null;
  const ready = v.ready.filter((p): p is string => typeof p === "string" && PROJECT.test(p)).slice(0, MAX_READY);
  return { id: v.id, name: name(v.name) ? v.name : v.id, cap: v.cap, admitted: v.admitted, answering: v.answering, ready, held: v.held };
}

// A listing's host entries, null from a `listen` that sends none or of the wrong shape: an
// entry with no ID or counts fails them all, as a host left out would drop its held workers.
export function parseHosts(v: unknown): Hosts | null {
  const listed = object(v) ? entries(v.projects) : null;
  if (!object(v) || !count(v.held) || !Array.isArray(v.hosts) || v.hosts.length > MAX_ENTRIES || listed === null) return null;
  const hosts = v.hosts.map(heard);
  const projects = Object.fromEntries(listed.flatMap(([repo, project]) => (REPO_ID.test(repo) && typeof project === "string" && PROJECT.test(project) ? [[repo, project]] : [])));
  const recipes = Object.fromEntries((entries(v.recipes) ?? []).flatMap(([repo, named]) => (REPO_ID.test(repo) && name(named) ? [[repo, named]] : [])));
  return hosts.every((h) => h !== null) ? { held: v.held, hosts: hosts as Heard[], projects, ...(Object.keys(recipes).length === 0 ? {} : { recipes }) } : null;
}

// The coordinator's slots: this machine's cap or the workers it holds, if more; each host that
// answered with a cap and a project ready, its cap or its workers, if more; any other, the
// workers it holds, as their slots are held whatever its cap says. At most MAX_SLOTS. Null
// until a config report has said this machine's cap.
export function slotTotal(fleet: Fleet): number | null {
  if (fleet.settings === null) return null;
  const { held, hosts } = fleet.hosts ?? { held: 0, hosts: [] };
  const share = (h: Heard) => (h.answering && h.cap > 0 && h.ready.length > 0 ? Math.max(h.cap, h.held) : h.held);
  return Math.min(MAX_SLOTS, hosts.reduce((sum, h) => sum + share(h), Math.max(fleet.settings.cap, held)));
}

// Where the coordinator may grant a slot (coordinator.ts, Room): each host that answered, holds
// a credential the Durable Object admitted (hosts.ts) and isn't being put to sleep or destroyed
// (`napping`, the fleet's command in flight), in the order the listing names them.
export function roomOf(fleet: Fleet, admitted: string[], napping: string[]): Room {
  const heard = fleet.hosts ?? { held: 0, hosts: [], projects: {} };
  const open = heard.hosts.filter((h) => h.answering && admitted.includes(h.id) && !napping.includes(h.id));
  const silent = heard.hosts.filter((h) => !h.answering).map((h) => h.id);
  return { cap: fleet.settings?.cap ?? null, held: heard.held, hosts: open.map(({ id, cap, held, ready }) => ({ id, cap, held, ready })), silent, projects: heard.projects };
}

export async function loadFleet(storage: Storage): Promise<Fleet> {
  return (await storage.get<Fleet>(FLEET_KEY)) ?? { settings: null, hosts: null };
}

// The fleet's own machine (docs/state-machine.md, "Fleet"): one for the owner, beside the
// triage machine, deciding every recipe machine's make, wake, sleep, update and destroy from the record
// above, the ledger and its own clocks, so a restart of `listen` neither sleeps a machine early
// nor forgets one. `reconcileFleet` is pure and idempotent, run after every event that changes
// what any of them holds, and sends at most one machine command at a time across all repos.
// `listen` carries each out under its own guards, answering it kept or `retryable`, never
// choosing another. Every value is a host ID, a machine's or repo's name, a number or a time.

// The issue and identifier its commands carry, which no issue's are: `listen` runs them in a
// lane of their own.
export const FLEET = "fleet";
export const RUN_KEY = "machine:fleet:run";
export const NAP_RETRY_MS = 10 * MINUTE;
export const DESTROY_RETRY_MS = 60 * MINUTE;
// The most idle minutes taken, so a due time stays a safe integer.
export const MAX_IDLE_MINUTES = 1_000_000;
// A make or wake that failed holds back its repo's next for this long, as `listen`'s own does.
export const COOL_DOWN_MS = 10 * MINUTE;
// An update asked of a machine holds it from another this long, unless its revision is its
// recipe's sooner.
export const UPDATE_RETRY_MS = 30 * MINUTE;
// A machine marked awake whose host is silent this long is woken, once in REVIVE_EVERY_MS.
export const UNHEARD_GRACE_MS = 3 * MINUTE;
export const REVIVE_EVERY_MS = 30 * MINUTE;
// A make or wake begun runs until a complete listing of a `listen` making nothing that itself
// began SETTLE_MS after it, by the listing's own `elapsedMs`, so it read `making` after the
// make's claim; RUN_MAX_MS is its backstop.
export const SETTLE_MS = 5_000;
export const RUN_MAX_MS = 60 * MINUTE;
// A worker's command's wait on a host holds it awake this long at most, should none hear it.
export const WANTED_MS = 30 * MINUTE;

// Why a reconcile asked for no machine for a repo whose request waits: here or a host has room,
// a make or wake runs, an awake machine of it has been silent under UNHEARD_GRACE_MS, its
// `machines` are all made, a failure's cool-down, no recipe.
export const REASONS = ["hereHasRoom", "hostHasRoom", "running", "hostComing", "allMade", "coolingDown", "noRecipe"] as const;
export type Reason = (typeof REASONS)[number];

// Idle, or dormant, since `since` with its repo's minutes as the listing said them; `retryMs`,
// not before then, after a command for it failed or found it held.
export type Clock = { since: number; minutes: number; retryMs: number | null };
export type Run = {
  // The compare-and-swap's revision, moved by every write.
  rev: number;
  commandSeq: number;
  // The one machine command in flight.
  command: Command | null;
  // A make or wake `listen` began, for its repo, until a listing shows it ended.
  running: { repo: string; sinceMs: number } | null;
  // By host ID, each idle recipe host; by machine name, each dormant machine.
  idle: Record<string, Clock>;
  dormant: Record<string, Clock>;
  // By host ID, since when a worker's command has waited on it.
  wanted: Record<string, number>;
  // By machine name: since its host was marked awake and silent, and when it was last woken so,
  // or a wake of it wasn't begun, which holds it from another for REVIVE_EVERY_MS.
  unheard: Record<string, number>;
  revived: Record<string, number>;
  // By machine name, when an update of it was last asked and ended (UPDATE_RETRY_MS).
  updated: Record<string, number>;
  // By repo, lower-cased, when its make failed or wasn't begun, or a wake of it failed or met
  // `listen`'s cool-down.
  cooling: Record<string, number>;
  // By repo (an Orca repo's ID for one with no recipe), why the last reconcile asked for none.
  why: Record<string, Reason>;
};

export type FleetEvent =
  // A complete listing's idle hosts, by host ID, with their idle minutes, and its dormant
  // machines, by name, with their minutes: null while `listen` makes, wakes, sleeps or destroys
  // one. `elapsedMs`: how long before it was sent it began.
  | { kind: "listed"; idle: Record<string, number>; dormant: Record<string, number> | null; elapsedMs: number }
  // `began`: a make or wake began; `coolDown`: `listen` refused it in its cool-down.
  | { kind: "commandResult"; command: string; result: CommandResult; data?: { began?: boolean; coolDown?: boolean; waitsOn?: WaitsOn } }
  // A worker's command waits on the host.
  | { kind: "wanted"; host: string }
  | { kind: "cleared"; clears: Clears };

export type FleetStepped = { run: Run; send: Command[] };

// A result's data as the fleet reads it (test/contract.json's `fleetBegan`): whether a make or
// wake began, and what a retryable one waits on.
export function fleetData(detail: Record<string, unknown>, waitsOn: WaitsOn | undefined): { began: boolean; coolDown?: boolean; waitsOn?: WaitsOn } {
  return { began: detail.began === true, ...(detail.coolDown === true ? { coolDown: true } : {}), ...(waitsOn === undefined ? {} : { waitsOn }) };
}

// What a reconcile reads: the fleet record, the ledger, where it may grant and the hosts admitted.
export type FleetView = { fleet: Fleet; c: Coordinator; room: Room; admitted: string[] };

export function newRun(): Run {
  return { rev: 0, commandSeq: 0, command: null, running: null, idle: {}, dormant: {}, wanted: {}, unheard: {}, revived: {}, updated: {}, cooling: {}, why: {} };
}

function dueAt(clock: Clock): number {
  return Math.max(clock.retryMs ?? 0, clock.since + clock.minutes * MINUTE);
}

// The host a sleep or a destroy in flight is stopping work on, which takes no grant.
export function napping(r: Run, fleet: Fleet): string[] {
  const c = r.command;
  if (c?.kind === "suspendHost") return [String(c.params.host)];
  if (c?.kind !== "destroyHost") return [];
  const name = String(c.params.name);
  const host = fleet.settings?.machines[name]?.environment ?? fleet.hosts?.hosts.find((h) => h.name === name)?.id;
  return host == null ? [] : [host];
}

// Applies one event. The machine returned is the one given when nothing changed.
export function stepFleet(before: Run, e: FleetEvent, nowMs: number): FleetStepped {
  const r = structuredClone(before);
  const send: Command[] = [];
  switch (e.kind) {
    case "listed":
      clocks(r.idle, e.idle, nowMs);
      if (e.dormant === null) break;
      clocks(r.dormant, e.dormant, nowMs);
      if (r.running !== null && nowMs - e.elapsedMs - r.running.sinceMs >= SETTLE_MS) r.running = null;
      // `listen` makes, wakes and sleeps nothing: the lock a command waits on is free.
      if (r.command !== null && clearedBy(r.command, { lock: true })) {
        r.command = resentCleared(r.command, nowMs);
        send.push({ ...r.command });
      }
      break;
    case "commandResult": {
      const c = r.command;
      if (c?.id !== e.command) return { run: before, send };
      if (e.result === "retryable") {
        c.waitsOn = e.data?.waitsOn ?? null;
        break;
      }
      r.command = null;
      if (c.kind === "makeHost" || c.kind === "wakeHost") {
        const repo = String(c.params.repo);
        // A make not begun, or one failed or in `listen`'s cool-down, cools its repo; a wake kept
        // for another reason (not admitted, gone) holds that machine alone.
        const kept = c.kind === "wakeHost" && e.result === "ok" && e.data?.coolDown !== true;
        if (e.result === "ok" && e.data?.began === true) r.running = { repo, sinceMs: nowMs };
        else if (kept) r.revived[String(c.params.name)] = nowMs;
        else r.cooling[repo.toLowerCase()] = nowMs;
      } else if (c.kind === "updateHost") {
        // Done, kept or failed, it isn't asked again before the config report can say its revision.
        r.updated[String(c.params.name)] = nowMs;
      } else {
        const [held, key, retry] = c.kind === "suspendHost" ? [r.idle, String(c.params.host), NAP_RETRY_MS] : [r.dormant, String(c.params.name), DESTROY_RETRY_MS];
        // Done or kept, its clock ends, and a listing naming it again begins a new one.
        if (e.result === "ok") delete held[key];
        else if (Object.hasOwn(held, key)) held[key].retryMs = nowMs + retry;
      }
      break;
    }
    case "wanted":
      if (Object.hasOwn(r.wanted, e.host)) return { run: before, send };
      r.wanted[e.host] = nowMs;
      break;
    case "cleared":
      if (r.command === null || !clearedBy(r.command, e.clears)) return { run: before, send };
      r.command = resentCleared(r.command, nowMs);
      send.push({ ...r.command });
      break;
  }
  return { run: same(r, before) ? before : r, send };
}

// A listing's hosts or machines, `named` with their minutes, onto their clocks: one no longer
// named (busy, awake, silent or gone) begins again when it next is.
function clocks(held: Record<string, Clock>, named: Record<string, number>, nowMs: number): void {
  for (const key of Object.keys(held)) if (!Object.hasOwn(named, key)) delete held[key];
  for (const [key, minutes] of Object.entries(named)) {
    if (!Object.hasOwn(held, key)) held[key] = { since: nowMs, minutes, retryMs: null };
    else held[key].minutes = minutes;
  }
}

function same(a: Run, b: Run): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

// The reconcile: what the fleet observes brought up to date, then the one command, if any, that
// moves it toward what is wanted, in this order. A machine that a grant, a worker's command or
// a waiting place pinned to it needs is woken. The first waiting place of each repo in turn
// with nowhere to go wakes an asleep machine of its repo, else makes one under its `machines`,
// neither while an awake machine of it has been silent under UNHEARD_GRACE_MS.
// A machine marked awake but silent past UNHEARD_GRACE_MS is woken once. A host idle its
// minutes, or a machine dormant its own, that nothing holds awake is put to sleep, or destroyed,
// before an asleep machine off its recipe's revision, admitted and held by nothing, is updated.
// Nothing new is asked while a command is in flight or a make or wake runs.
export function reconcileFleet(before: Run, v: FleetView, nowMs: number): FleetStepped {
  const settings = v.fleet.settings;
  if (settings === null) return { run: before, send: [] };
  const r = structuredClone(before);
  const heard = v.fleet.hosts?.hosts ?? [];
  const machines = Object.entries(settings.machines);
  for (const [name, m] of machines) {
    if (!(m.made && !m.asleep && heard.some((h) => h.name === name && !h.answering))) delete r.unheard[name];
    else r.unheard[name] ??= nowMs;
  }
  for (const name of Object.keys(r.unheard)) if (!Object.hasOwn(settings.machines, name)) delete r.unheard[name];
  for (const name of Object.keys(r.revived)) if (nowMs - r.revived[name] >= REVIVE_EVERY_MS) delete r.revived[name];
  for (const repo of Object.keys(r.cooling)) if (nowMs - r.cooling[repo] >= COOL_DOWN_MS) delete r.cooling[repo];
  // A machine behind: its recipe has said a revision, and it runs another or hasn't said one.
  const recipeOf = (m: Made) => Object.entries(settings.repos).find(([repo]) => repo.toLowerCase() === m.repo.toLowerCase())?.[1];
  const behind = (m: Made) => {
    const wanted = recipeOf(m)?.revision ?? null;
    return wanted !== null && m.revision !== wanted;
  };
  for (const name of Object.keys(r.updated)) {
    const m = settings.machines[name];
    if (m === undefined || !behind(m) || nowMs - r.updated[name] >= UPDATE_RETRY_MS) delete r.updated[name];
  }
  for (const host of Object.keys(r.wanted)) if (nowMs - r.wanted[host] >= WANTED_MS || heard.some((h) => h.id === host && h.answering)) delete r.wanted[host];
  if (r.running !== null && nowMs - r.running.sinceMs >= RUN_MAX_MS) r.running = null;

  const held = new Set([...Object.values(v.c.grants).flatMap((g) => g.host ?? []), ...Object.keys(r.wanted), ...v.c.queue.flatMap((p) => p.on ?? [])]);
  const hostOf = (name: string) => settings.machines[name]?.environment ?? heard.find((h) => h.name === name)?.id;
  const wake = (name: string) => ({ kind: "wakeHost" as CommandKind, params: { name, repo: settings.machines[name].repo } });
  const busy = r.command !== null || r.running !== null;
  let ask: { kind: CommandKind; params: Record<string, unknown> } | null = null;
  const wakeable = (name: string, m: Made) => m.asleep && m.environment !== null && v.admitted.includes(m.environment) && !Object.hasOwn(r.revived, name) && !Object.hasOwn(r.cooling, m.repo.toLowerCase());
  const needed = [...held].flatMap((host) => machines.filter(([name, m]) => m.environment === host && wakeable(name, m)).map(([name]) => name));
  if (!busy && needed.length > 0) ask = wake(needed[0]);
  const why: Record<string, Reason> = {};
  const waiting = v.c.queue.filter((p) => v.c.grants[p.issue] === undefined && p.on === undefined && p.repo != null).sort(byTurn);
  for (const p of waiting) {
    const recipes = v.fleet.hosts?.recipes ?? {};
    const named = Object.hasOwn(recipes, p.repo!) ? recipes[p.repo!] : undefined;
    const key = named ?? p.repo!;
    if (Object.hasOwn(why, key) || String(ask?.params.repo ?? "").toLowerCase() === key.toLowerCase()) continue;
    const at = place(v.c, v.room, p);
    const recipe = named !== undefined && Object.hasOwn(settings.repos, named) ? settings.repos[named] : undefined;
    const ours = machines.filter(([, m]) => m.repo.toLowerCase() === key.toLowerCase());
    // An awake machine of the repo paired but silent less than UNHEARD_GRACE_MS is room still to
    // come, as a listing just after a make or wake may not hear it yet; past that, the unheard
    // wake is its own.
    const coming = ours.some(([name, m]) => m.made && !m.asleep && Object.hasOwn(r.unheard, name) && nowMs - r.unheard[name] < UNHEARD_GRACE_MS);
    if (at !== null) why[key] = at.host === null ? "hereHasRoom" : "hostHasRoom";
    else if (recipe === undefined) why[key] = "noRecipe";
    else if (busy || ask !== null) why[key] = "running";
    else if (Object.hasOwn(r.cooling, key.toLowerCase())) why[key] = "coolingDown";
    else {
      const asleep = ours.find(([name, m]) => wakeable(name, m));
      if (coming) why[key] = "hostComing";
      else if (asleep !== undefined) ask = wake(asleep[0]);
      else if (ours.length < recipe.machines) ask = { kind: "makeHost", params: { repo: key } };
      else why[key] = "allMade";
    }
  }
  if (ask === null && !busy) {
    const unheard = Object.keys(r.unheard).find((name) => nowMs - r.unheard[name] >= UNHEARD_GRACE_MS && !Object.hasOwn(r.revived, name));
    if (unheard !== undefined) {
      ask = wake(unheard);
      r.revived[unheard] = nowMs;
    }
  }
  // A clock due on a machine something holds awake waits NAP_RETRY_MS, or the event that frees it.
  const due = (clock: Clock, host: string | undefined) => {
    if (dueAt(clock) > nowMs) return false;
    if (host === undefined || !held.has(host)) return true;
    clock.retryMs = nowMs + NAP_RETRY_MS;
    return false;
  };
  if (ask === null && !busy) {
    const host = Object.keys(r.idle).find((h) => due(r.idle[h], h));
    if (host !== undefined) ask = { kind: "suspendHost", params: { host } };
  }
  if (ask === null && !busy) {
    const name = Object.keys(r.dormant).find((n) => due(r.dormant[n], hostOf(n)));
    if (name !== undefined) ask = { kind: "destroyHost", params: { name } };
  }
  if (ask === null && !busy) {
    const outdated = machines.find(([name, m]) => m.made && m.asleep && m.environment !== null && v.admitted.includes(m.environment) && !held.has(m.environment) && behind(m) && !Object.hasOwn(r.updated, name));
    if (outdated !== undefined) ask = { kind: "updateHost", params: { name: outdated[0] } };
  }
  r.why = why;
  const send: Command[] = [];
  if (ask !== null) {
    r.commandSeq += 1;
    r.command = newCommand(`${FLEET}.${r.commandSeq}`, FLEET, ask.kind, FLEET, r.commandSeq, ask.params, false);
    send.push(sent(r.command, nowMs));
  }
  return { run: same(r, before) ? before : r, send };
}

// When the machine is next due: its command's timeout, a make's or wake's backstop, a wait on a
// host lapsing, and while neither is in flight, the earliest clock, grace or cool-down to end.
export function fleetDue(r: Run): number | null {
  const due: number[] = Object.values(r.wanted).map((at) => at + WANTED_MS);
  const timeout = r.command === null ? null : dueOf(r.command, false);
  if (timeout !== null) due.push(timeout);
  if (r.running !== null) due.push(r.running.sinceMs + RUN_MAX_MS);
  if (r.command === null && r.running === null) {
    due.push(...Object.values(r.idle).map(dueAt), ...Object.values(r.dormant).map(dueAt));
    due.push(...Object.entries(r.unheard).map(([name, since]) => (Object.hasOwn(r.revived, name) ? r.revived[name] + REVIVE_EVERY_MS : since + UNHEARD_GRACE_MS)));
    due.push(...Object.values(r.cooling).map((at) => at + COOL_DOWN_MS), ...Object.values(r.revived).map((at) => at + REVIVE_EVERY_MS));
    due.push(...Object.values(r.updated).map((at) => at + UPDATE_RETRY_MS));
  }
  return due.length === 0 ? null : Math.min(...due);
}

// The alarm's turn for its command: resent or, past its last send, ended `lost`.
export function expireFleet(before: Run, nowMs: number): FleetStepped {
  const c = before.command;
  const next = c === null ? "wait" : expiry(c, nowMs, false);
  if (next === "lost") return stepFleet(before, { kind: "commandResult", command: c!.id, result: "lost" }, nowMs);
  if (next === "wait") return { run: before, send: [] };
  const r = structuredClone(before);
  return { run: r, send: [sent(r.command!, nowMs)] };
}
