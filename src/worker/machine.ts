import { TIME } from "../shared/linear.ts";
import { READERS, RESULTS, type CommandKind, type Reader, type Result } from "./machine/types.ts";
import { TERMINAL } from "./machine/validate.ts";
import { MAX_IDLE_MINUTES } from "./machine/fleet.ts";
import { SHA } from "./github.ts";
import { HOST, ID, type Outcome, WORKTREE } from "./state.ts";
import { type Hosts, parseHosts, parseSettings, type Settings } from "./machine/fleet.ts";

export { COMMANDS, READERS, RESULTS, type CommandKind, type Reader, type Result } from "./machine/types.ts";
export { MAX_SLOTS } from "./machine/fleet.ts";

// The wire between the machines of docs/state-machine.md and `tolkster listen`: the
// commands the Durable Object sends, and what comes back as events: a command's result,
// Orca's observations, and the worker's own `tolkster` calls. test/contract.json's
// `commands` and `reports` hold one of each, which cli/src/machine.rs writes and reads
// the same. Every field is an ID, an enum, a number or a time: never Linear or GitHub text.

// What `listen` sees in Orca, by worktree: a turn's end is `turnEnded`, by session.
export const OBSERVATIONS = ["agentWorking", "agentIdle", "agentWaiting", "agentExited", "worktreeGone"] as const;
export type Observation = (typeof OBSERVATIONS)[number];

// What an agent's own Claude Code hooks report of it (`tolkster hook state`), as it happens:
// `agentBackground`, idle at its prompt while a call it sent to the background runs, only they say.
export const AGENT_REPORTS = ["agentWorking", "agentBackground", "agentIdle", "agentWaiting", "agentExited"] as const;
export type AgentReport = (typeof AGENT_REPORTS)[number];

// What Orca's listing shows an agent doing, by worktree (`listing`).
export const LISTED_STATES = ["working", "idle", "waiting"] as const;
export type ListedState = (typeof LISTED_STATES)[number];

// `id` is the command's: the same on every resend, and the one its result names.
// `version` is the machine's when it was sent, which the result carries back. `urgent` is
// a class 1 event's command, which `listen` runs beside the one in flight. `args` are the
// kind's own fields, as cli/src/machine.rs reads them.
export type Command = { type: "command"; id: string; kind: CommandKind; issueId: string; identifier: string; version: number; urgent: boolean; args: Record<string, unknown> };

export type Report =
  // `detail` is what the kind's result carries (docs/state-machine.md, Commands), as IDs,
  // enums and times.
  | { report: "commandResult"; id: string; issueId: string; version: number; result: Result; detail: Record<string, unknown> }
  // An `observe` command's, and a worktree gone from Orca. `sequence` counts from 1 on each
  // connection.
  | { report: Observation; worktreeId: string; sequence: number }
  | { report: "turnEnded"; sessionId: string; sequence: number }
  // From a session's turn's own `converse`, to POST /state: its Claude, in `terminal`, is done
  // with its input, at `at` on the workstation's clock.
  | { report: "turnIdle"; sessionId: string; terminal: string; at: string }
  // A terminal closed, by its handle, as `listen`'s watcher saw it: the Durable Object maps it
  // to the turn or triage session it ran, if it holds one as the newest.
  | { report: "terminalClosed"; terminal: string; sequence: number }
  // From the agent's own hook, to POST /state: never on the socket, whose observations alone
  // carry a `sequence`. `at` is when the hook ran on its host's clock, which orders only
  // that worktree's hooks; `elapsedMs` is how long before the report was made, on the host's
  // monotonic clock, which is all the Durable Object compares with a listing.
  | { report: AgentReport; worktreeId: string; at: string; elapsedMs: number }
  // From a worker's or turn's `tolkster messages`, by the newest activity it read, and its
  // `tolkster activity` response or elicitation, by the activity it posted. `reader` is who
  // reported: the issue's worker, or the session's or a note's thread's turn.
  | { report: "workerRead" | "workerAnswered"; sessionId: string; activityId: string; at: string; reader: Reader }
  // `tolkster issue chain`, before its first delegation: the parent, the chain's session,
  // a run of its own, and the IDs of the sub-issues it delegates.
  | { report: "chainRecorded"; issueId: string; sessionId: string; run: string; children: string[] }
  // A worker's own report that it pushed its change to the default branch with no PR: the
  // issue, the worker's session, and `head`, the commit pushed, whole.
  | { report: "landed"; issueId: string; sessionId: string; head: string }
  // A worker's own report that GitHub refused auto-merge on its PR as mergeable already, so
  // the machine merges it (docs/auto-merge.md).
  | { report: "mergeAsked"; issueId: string; sessionId: string; pr: number }
  // `listen`'s settings (machine/fleet.ts), sent as it connects and whenever they change.
  | ({ report: "config" } & Settings)
  // Orca's whole listing, as one snapshot: `at` is when it began, on the workstation's
  // clock, which only names it; `elapsedMs` how long before it was sent that was, on the
  // workstation's monotonic clock; `agents` each worktree an agent runs in; `listed` every
  // worktree Orca has, null when its listing was incomplete; `hosts` the paired hosts that
  // answered, as a worktree on any other is in neither; `paired` every host paired, answering
  // or not, so a worktree on a host in neither list is on one unpaired; `idle` the recipe
  // machines that hold nothing, by host ID, with their idle minutes, and `dormant` those whose
  // destroy clock runs, by name, with their minutes, null while one is made, woken, slept or
  // destroyed (machine/fleet.ts); `fleet` this machine's workers and each paired host's entry
  // (machine/fleet.ts), null from a `listen` that sends none.
  | Listing;

export type Listing = { report: "listing"; at: string; elapsedMs: number; agents: Record<string, ListedState>; listed: string[] | null; hosts: string[]; paired: string[]; idle: Record<string, number>; dormant: Record<string, number> | null; fleet: Hosts | null };

export type Kind = Report["report"];
export type HookReport = Extract<Report, { worktreeId: string; at: string }>;

export function fromHook(report: Report): report is HookReport {
  return "worktreeId" in report && "at" in report;
}

const COMMAND_ID = /^[A-Za-z0-9_.:@-]{1,200}$/;
const MAX_DETAIL = 16 * 1024;
const MAX_CHILDREN = 50;
// The most worktrees a listing names, in `agents` and in `listed` (cli/src/machine.rs, MAX_LISTED).
export const MAX_LISTED = 1000;

// Predicates, not check.ts's throwing checks: a report that fails is dropped (null), not refused.
const id = (v: unknown): v is string => typeof v === "string" && ID.test(v);
const count = (v: unknown): v is number => typeof v === "number" && Number.isSafeInteger(v) && v >= 0;
const one = <T extends string>(list: readonly T[], v: unknown): v is T => typeof v === "string" && (list as readonly string[]).includes(v);

// The report `message` holds, or null for anything else (an acknowledgement, a ping) or
// a report of the wrong shape: a report is only ever checked, never trusted to name
// anything it can't.
export function parseReport(message: unknown): Report | null {
  const r = (message ?? {}) as Record<string, unknown>;
  switch (r.report) {
    case "commandResult": {
      if (typeof r.id !== "string" || !COMMAND_ID.test(r.id) || !id(r.issueId) || !count(r.version) || !one(RESULTS, r.result)) return null;
      const detail = r.detail ?? {};
      if (typeof detail !== "object" || Array.isArray(detail) || JSON.stringify(detail).length > MAX_DETAIL) return null;
      return { report: "commandResult", id: r.id, issueId: r.issueId, version: r.version, result: r.result, detail: detail as Record<string, unknown> };
    }
    case "turnEnded":
      return id(r.sessionId) && count(r.sequence) ? { report: r.report, sessionId: r.sessionId, sequence: r.sequence } : null;
    case "terminalClosed":
      return typeof r.terminal === "string" && TERMINAL.test(r.terminal) && count(r.sequence) ? { report: r.report, terminal: r.terminal, sequence: r.sequence } : null;
    case "turnIdle":
      return id(r.sessionId) && typeof r.terminal === "string" && TERMINAL.test(r.terminal) && typeof r.at === "string" && TIME.test(r.at) ? { report: r.report, sessionId: r.sessionId, terminal: r.terminal, at: r.at } : null;
    case "workerRead":
    case "workerAnswered":
      return id(r.sessionId) && id(r.activityId) && typeof r.at === "string" && TIME.test(r.at) && one(READERS, r.reader) ? { report: r.report, sessionId: r.sessionId, activityId: r.activityId, at: r.at, reader: r.reader } : null;
    case "chainRecorded": {
      const children = r.children;
      if (!id(r.issueId) || !id(r.sessionId) || !id(r.run) || !Array.isArray(children) || children.length > MAX_CHILDREN || !children.every(id)) return null;
      return { report: "chainRecorded", issueId: r.issueId, sessionId: r.sessionId, run: r.run, children: children as string[] };
    }
    case "landed":
      return id(r.issueId) && id(r.sessionId) && typeof r.head === "string" && SHA.test(r.head) ? { report: "landed", issueId: r.issueId, sessionId: r.sessionId, head: r.head } : null;
    case "mergeAsked":
      return id(r.issueId) && id(r.sessionId) && count(r.pr) && r.pr > 0 ? { report: "mergeAsked", issueId: r.issueId, sessionId: r.sessionId, pr: r.pr } : null;
    case "config": {
      const settings = parseSettings(r);
      return settings === null ? null : { report: "config", ...settings };
    }
    case "listing": {
      const agents = r.agents;
      if (typeof r.at !== "string" || !TIME.test(r.at) || !count(r.elapsedMs) || typeof agents !== "object" || agents === null || Array.isArray(agents)) return null;
      const states = Object.entries(agents as Record<string, unknown>);
      if (states.length > MAX_LISTED || !states.every(([worktree, state]) => WORKTREE.test(worktree) && one(LISTED_STATES, state))) return null;
      const listed = r.listed;
      if (listed !== null && (!Array.isArray(listed) || listed.length > MAX_LISTED || !listed.every((w) => typeof w === "string" && WORKTREE.test(w)))) return null;
      const hosts = r.hosts;
      if (!Array.isArray(hosts) || hosts.length > MAX_LISTED || !hosts.every((h) => typeof h === "string" && HOST.test(h))) return null;
      const paired = r.paired;
      if (!Array.isArray(paired) || paired.length > MAX_LISTED || !paired.every((h) => typeof h === "string" && HOST.test(h))) return null;
      return { report: "listing", at: r.at, elapsedMs: r.elapsedMs, agents: Object.fromEntries(states) as Record<string, ListedState>, listed: listed as string[] | null, hosts: hosts as string[], paired: paired as string[], idle: idleHosts(r.idle, hosts as string[]), dormant: dormantMachines(r.dormant), fleet: parseHosts(r.fleet) };
    }
    default:
      if (typeof r.worktreeId !== "string" || !WORKTREE.test(r.worktreeId)) return null;
      if ("sequence" in r) return one(OBSERVATIONS, r.report) && count(r.sequence) ? { report: r.report, worktreeId: r.worktreeId, sequence: r.sequence } : null;
      return one(AGENT_REPORTS, r.report) && typeof r.at === "string" && TIME.test(r.at) && count(r.elapsedMs) ? { report: r.report, worktreeId: r.worktreeId, at: r.at, elapsedMs: r.elapsedMs } : null;
  }
}

// A listing's idle hosts, each an answering host's with its idle minutes: one that isn't is
// left out, never failing the listing, as from a `listen` that sends none.
function idleHosts(idle: unknown, hosts: string[]): Record<string, number> {
  if (typeof idle !== "object" || idle === null || Array.isArray(idle)) return {};
  const minutes = (v: unknown): v is number => count(v) && v >= 1 && v <= MAX_IDLE_MINUTES;
  return Object.fromEntries(Object.entries(idle).filter(([host, v]) => host !== "__proto__" && hosts.includes(host) && minutes(v)));
}

// A listing's dormant machines, each a plain name with its minutes, as `idleHosts`; null, which
// leaves their clocks, when it sends none, or more than a listing holds.
function dormantMachines(dormant: unknown): Record<string, number> | null {
  if (typeof dormant !== "object" || dormant === null || Array.isArray(dormant) || Object.keys(dormant).length > MAX_LISTED) return null;
  const minutes = (v: unknown): v is number => count(v) && v >= 1 && v <= MAX_IDLE_MINUTES;
  return Object.fromEntries(Object.entries(dormant).filter(([name, v]) => name !== "__proto__" && HOST.test(name) && minutes(v)));
}

// The connection commands go to, and the only one whose reports count: the newest
// accepted.
export function newest<S>(sockets: { socket: S; acceptedAt: number }[]): S | null {
  let best: { socket: S; acceptedAt: number } | null = null;
  for (const { socket, acceptedAt } of sockets) if (best === null || acceptedAt > best.acceptedAt) best = { socket, acceptedAt };
  return best?.socket ?? null;
}

// A worker's report, not a hook's, whose event the store failed to take: 503, which `tolkster`
// sends again (cli/src/state.rs), so a `workerAnswered` lost to it doesn't leave the request open.
export function refusedReport(report: Report): Outcome {
  return { status: 503, body: JSON.stringify({ error: "refused" }), log: { ...logged(report), refused: true } };
}

// What the log keeps of a report: no worktree IDs, which carry the workstation's paths.
export function logged(report: Report): Record<string, string | number> {
  switch (report.report) {
    case "commandResult":
      return { report: report.report, commandId: report.id, issueId: report.issueId, version: report.version, result: report.result };
    case "turnEnded":
      return { report: report.report, sessionId: report.sessionId, sequence: report.sequence };
    case "terminalClosed":
      return { report: report.report, terminal: report.terminal, sequence: report.sequence };
    case "turnIdle":
      return { report: report.report, sessionId: report.sessionId, terminal: report.terminal, at: report.at };
    case "workerRead":
    case "workerAnswered":
      return { report: report.report, sessionId: report.sessionId, activityId: report.activityId };
    case "chainRecorded":
      return { report: report.report, issueId: report.issueId, sessionId: report.sessionId, children: report.children.length };
    case "landed":
      return { report: report.report, issueId: report.issueId, sessionId: report.sessionId, head: report.head };
    case "mergeAsked":
      return { report: report.report, issueId: report.issueId, sessionId: report.sessionId, pr: report.pr };
    case "config":
      return { report: report.report, cap: report.cap, repos: Object.keys(report.repos).length, machines: Object.keys(report.machines).length };
    case "listing":
      return { report: report.report, at: report.at, agents: Object.keys(report.agents).length, listed: report.listed === null ? "incomplete" : report.listed.length };
    default:
      return "sequence" in report ? { report: report.report, sequence: report.sequence } : { report: report.report, at: report.at };
  }
}
