// A worker session's timing (docs/timing.md), as its own `tolkster` reports it
// (cli/src/timing.rs): the relay checks its shape, finds its issue by its worktree, and sends
// it on to `listen`'s newest connection without the worktree, whose ID names a path, keeping
// nothing of it. test/contract.json's `timing` holds the report and the forward. IDs, enums,
// counts, durations and offsets only.
//
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node.

import { TIME } from "../shared/linear.ts";
import { SHA } from "./github.ts";
import { WORKTREE as INDEX } from "./machine/store.ts";
import type { RelaySocket } from "./realtime.ts";
import { type Outcome, WORKTREE } from "./state.ts";
import type { Storage } from "./storage.ts";

export const KINDS = ["start", "resume", "round", "rebase"] as const;
export const CHECKS = ["cargoTest", "clippy", "cargoCheck", "nodeTest", "simulator", "typecheck", "gate"] as const;
export const BY = ["main", "subAgent"] as const;
const TOOLS = ["read", "edit", "check", "git", "bookkeeping", "agent", "other"] as const;
const OFFSETS = ["firstEdit", "firstCommit", "firstGate", "reviewStart", "reviewEnd", "pr"] as const;
// The most check runs a summary carries (cli/src/timing.rs, MOST_CHECKS).
export const MOST_CHECKS = 1000;
// Claude's session ID, in the CLI's plain shape (cli/src/plain.rs, `plain_id`).
const SESSION = /^[A-Za-z0-9][A-Za-z0-9-]{0,63}$/;

type CheckRun = { kind: (typeof CHECKS)[number]; failed: boolean | null; edited: boolean; seconds: number; at: number; by: (typeof BY)[number] };
export type Summary = {
  session: string;
  launched: string;
  kind: (typeof KINDS)[number];
  build: string | null;
  at: string;
  wall: number;
  active: number;
  model: number;
  tools: Record<(typeof TOOLS)[number], number>;
  apiTurns: number;
  toolTurns: number;
  offsets: Record<(typeof OFFSETS)[number], number | null>;
  checks: CheckRun[];
};
export type Timing = { worktreeId: string; timing: Summary };

const count = (v: unknown): v is number => typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
const one = <T extends string>(list: readonly T[], v: unknown): v is T => typeof v === "string" && (list as readonly string[]).includes(v);
const record = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const time = (v: unknown): v is string => typeof v === "string" && TIME.test(v);

// Each named field of `v`, checked by `is`; null when one fails.
function fields<K extends string, T>(v: unknown, keys: readonly K[], is: (v: unknown) => v is T): Record<K, T> | null {
  if (!record(v) || !keys.every((key) => is(v[key]))) return null;
  return Object.fromEntries(keys.map((key) => [key, v[key]])) as Record<K, T>;
}

function checkRun(v: unknown): CheckRun | null {
  if (!record(v) || !one(CHECKS, v.kind) || !(v.failed === null || typeof v.failed === "boolean") || typeof v.edited !== "boolean" || !count(v.seconds) || !count(v.at) || !one(BY, v.by)) return null;
  return { kind: v.kind, failed: v.failed, edited: v.edited, seconds: v.seconds, at: v.at, by: v.by };
}

// The report `body` holds, rebuilt from its checked fields alone; null for any other body or
// one of the wrong shape.
export function parseTiming(body: unknown): Timing | null {
  if (!record(body) || body.op !== "report" || body.report !== "sessionTiming" || typeof body.worktreeId !== "string" || !WORKTREE.test(body.worktreeId)) return null;
  const t = body.timing;
  if (!record(t) || typeof t.session !== "string" || !SESSION.test(t.session) || !time(t.launched) || !one(KINDS, t.kind) || !(t.build === null || (typeof t.build === "string" && SHA.test(t.build))) || !time(t.at)) return null;
  if (!count(t.wall) || !count(t.active) || !count(t.model) || !count(t.apiTurns) || !count(t.toolTurns)) return null;
  const tools = fields(t.tools, TOOLS, count);
  const offsets = fields(t.offsets, OFFSETS, (v): v is number | null => v === null || count(v));
  const checks = Array.isArray(t.checks) && t.checks.length <= MOST_CHECKS ? t.checks.map(checkRun) : null;
  if (tools === null || offsets === null || checks === null || checks.some((c) => c === null)) return null;
  const timing: Summary = { session: t.session, launched: t.launched, kind: t.kind, build: t.build, at: t.at, wall: t.wall, active: t.active, model: t.model, tools, apiTurns: t.apiTurns, toolTurns: t.toolTurns, offsets, checks: checks as CheckRun[] };
  return { worktreeId: body.worktreeId, timing };
}

// POST /state's `sessionTiming`: sent to `to`, `listen`'s newest connection, with the issue the
// worktree index names for it. 503 while no connection takes it, which `tolkster` sends again;
// 200 with nothing sent for a worktree the index doesn't hold, whose issue can't be named.
export async function forwardTiming(storage: Storage, to: RelaySocket | null, body: unknown): Promise<Outcome> {
  const report = parseTiming(body);
  if (report === null) return { status: 400, body: JSON.stringify({ error: "report" }), log: { reason: "report" } };
  const issueId = await storage.get<string>(INDEX + report.worktreeId);
  const log = { report: "sessionTiming", session: report.timing.session, issueId: issueId ?? null };
  if (issueId === undefined) return { status: 200, body: "{}", log: { ...log, unindexed: true } };
  try {
    if (to === null) throw new Error("no connection");
    to.send(JSON.stringify({ type: "timing", issueId, timing: report.timing }));
  } catch {
    return { status: 503, body: JSON.stringify({ error: "not listening" }), log: { ...log, sent: false } };
  }
  return { status: 200, body: "{}", log };
}
