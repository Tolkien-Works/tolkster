// What a worker host's credential may do at POST /state (docs/hosts.md): the reports, reads
// and records of its own workers, nothing of another host's, the workstation's or the
// owner's. A worker is its own when its worktree's ID starts `<host>@`, the host's name in the
// ID Orca gives its remote worktrees; an issue, session or delegation is when the issue's
// store, or the worktree index, holds such a worktree. Whatever isn't named here is refused, so a new op is closed to
// hosts until it is listed.
//
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node.

import { sessionIssue } from "./intake.ts";
import { fromHook, parseReport, type Report } from "./machine.ts";
import { loadStore, WORKTREE } from "./machine/store.ts";
import { handle, type Operation, operation, type Outcome } from "./state.ts";
import { Invalid } from "./check.ts";
import { parseTiming } from "./timing.ts";
import type { Storage } from "./storage.ts";

const refused = (op: string): Outcome => ({ status: 403, body: JSON.stringify({ error: "not this host's" }), log: { op, reason: "not this host's" } });

const ownsWorktree = (host: string, worktreeId: string): boolean => worktreeId.startsWith(`${host}@`);

// What `host` runs the workers of, read once per call. An issue is its when its store holds
// one of the host's worktrees, or the worktree index does, which a start's result writes
// even when the store drops it, as a `lost` start's.
class Owned {
  private readonly issues = new Map<string, Promise<boolean>>();
  private indexed: Promise<Set<string>> | null = null;

  readonly storage: Storage;
  private readonly host: string;

  constructor(storage: Storage, host: string) {
    this.storage = storage;
    this.host = host;
  }

  worktree(worktreeId: string): boolean {
    return ownsWorktree(this.host, worktreeId);
  }

  issue(issueId: string): Promise<boolean> {
    let owned = this.issues.get(issueId);
    if (owned === undefined) {
      owned = this.holds(issueId);
      this.issues.set(issueId, owned);
    }
    return owned;
  }

  // A session on an issue it runs the worker of. A session the Durable Object holds no issue
  // for is no one's.
  async session(sessionId: string): Promise<boolean> {
    const issue = await sessionIssue(this.storage, sessionId);
    return issue !== null && (await this.issue(issue));
  }

  private async holds(issueId: string): Promise<boolean> {
    const worktree = (await loadStore(this.storage, issueId)).facts.worktree;
    if (worktree !== null && this.worktree(worktree.id)) return true;
    this.indexed ??= this.storage.list<string>({ prefix: `${WORKTREE}${this.host}@` }).then((index) => new Set(index.values()));
    return (await this.indexed).has(issueId);
  }
}

// A worker's own `tolkster` report. A turn's (`reader: "turn"`, `turnIdle`) runs on the
// `listen` host, and the rest come on its socket, so neither is a host's. A chain's session
// is the parent's own, and a landing's or merge's its issue's.
async function reportOwned(owned: Owned, report: Report): Promise<boolean> {
  switch (report.report) {
    case "workerRead":
    case "workerAnswered":
      return report.reader === "worker" && (await owned.session(report.sessionId));
    case "chainRecorded":
    case "landed":
    case "mergeAsked":
      return (await owned.issue(report.issueId)) && (await sessionIssue(owned.storage, report.sessionId)) === report.issueId;
    default:
      return fromHook(report) && owned.worktree(report.worktreeId);
  }
}

const keep = async <T>(items: T[], owns: (item: T) => Promise<boolean>): Promise<T[]> => (await Promise.all(items.map(owns))).flatMap((own, i) => (own ? [items[i]] : []));

// `host`'s refusal (403) of the body, or its answer for an op that returns only its own
// records (the rest of what it asks for is null, as for something never recorded), or null
// for one it may make, which `handle` and the relay then run. A body that doesn't parse is
// null too: they answer it 400.
export async function hostScope(storage: Storage, host: string, body: unknown, nowMs: number): Promise<Outcome | null> {
  const owned = new Owned(storage, host);
  const op = (body as { op?: unknown } | null)?.op;
  if (op === "report" && (body as { report?: unknown }).report === "sessionTiming") {
    const timing = parseTiming(body);
    return timing === null || owned.worktree(timing.worktreeId) ? null : refused("report");
  }
  if (op === "report") {
    const report = parseReport(body);
    return report === null || (await reportOwned(owned, report)) ? null : refused("report");
  }
  if (op === "session") {
    const sessionId = (body as { sessionId?: unknown }).sessionId;
    if (typeof sessionId === "string" && (await owned.session(sessionId))) return null;
    // As for a session the Durable Object doesn't know: nothing read.
    return reply("session", { readThrough: null, turnReadThrough: null, answeredThrough: null });
  }
  let parsed: Operation;
  try {
    parsed = operation(body);
  } catch (error) {
    if (error instanceof Invalid) return null;
    throw error;
  }
  switch (parsed.op) {
    // A host records only what it has read: a `conversation` names the terminal `listen`
    // types a handover into.
    case "record":
      return parsed.change.conversation === undefined && (await owned.session(parsed.sessionId)) ? null : refused("record");
    // A delegation is the parent worker's, made from its own session.
    case "delegate":
      return (await owned.issue(parsed.delegation.parentIssueId)) && (await sessionIssue(storage, parsed.delegation.sessionId)) === parsed.delegation.parentIssueId ? null : refused("delegate");
    case "read": {
      const own = await keep(parsed.sessions, (id) => owned.session(id));
      const answered = own.length === 0 ? { records: {} } : await answer(storage, { ...parsed, sessions: own }, nowMs);
      return reply(parsed.op, { records: { ...Object.fromEntries(parsed.sessions.map((id) => [id, null])), ...answered.records } });
    }
    case "delegations": {
      const own = await keep(parsed.issues, (id) => owned.issue(id));
      const answered = own.length === 0 ? { delegations: {} } : await answer(storage, { ...parsed, issues: own }, nowMs);
      return reply(parsed.op, { delegations: { ...Object.fromEntries(parsed.issues.map((id) => [id, null])), ...answered.delegations } });
    }
    // The hook finds its worker here.
    case "workers": {
      const { workers } = await answer(storage, parsed, nowMs);
      return reply(parsed.op, { workers: workers.filter((w: { worktreeId: string }) => owned.worktree(w.worktreeId)) });
    }
    default:
      return refused(parsed.op);
  }
}

// An op's answer, parsed.
async function answer(storage: Storage, body: Operation, nowMs: number): Promise<any> {
  return JSON.parse((await handle(storage, body, nowMs)).body);
}

const reply = (op: string, body: Record<string, unknown>): Outcome => ({ status: 200, body: JSON.stringify(body), log: { op } });
