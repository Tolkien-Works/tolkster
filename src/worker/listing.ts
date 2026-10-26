import { describe, info, warn } from "./log.ts";
import type { HookReport, Listing } from "./machine.ts";
import { deliver, HOOK_AT, type HookTime, loadStore, type Sender, WORKTREE } from "./machine/store.ts";
import type { AgentStatus } from "./machine/types.ts";
import { forgetGone, goneWorkers, heard, markMissing, unpairedWorkers } from "./state.ts";
import { pruneDue, type Storage } from "./storage.ts";

// Each worker's agent state, as the level `listen`'s newest listing of Orca shows it
// (machine.ts, `listing`), set against what the machine holds: the relay keeps the newest
// listing, and applies it to each worktree it indexes, and again to a worktree whose start
// is learned after it came (TW-486). An agent's own hook at or after the listing began is
// newer, and wins: both by the Durable Object's clock, as each report says how long before its
// receipt it was made (`elapsedMs`), so no two machines' clocks are compared. All IDs and times.

export { HOOK_AT, type HookTime, WORKTREE };
const LISTING = "listing";
const HOOKS_PRUNED_AT = "hooks-pruned-at";

// How long a hook's time is kept for a worktree the index doesn't hold: a hook can come
// before the start result that names its worktree, and one after the worktree was dropped
// is never read.
export const HOOK_AT_KEEP_MS = 60 * 60 * 1000;

// How long after an agent's own hook said it exited a listing showing it still there isn't
// believed: the hook runs before Claude exits, and Orca sees the pane go after.
export const EXIT_GRACE_MS = 2 * 60 * 1000;

// How long after an agent's own hook said it waits on a call in the background a listing
// showing it idle isn't believed: Orca reads the same stop as idle, and a notice lost to the
// transcript mustn't hold the worker as running for good.
export const BACKGROUND_HOLD_MS = 30 * 60 * 1000;

// A listing with when it began by the Durable Object's clock: its receipt less its `elapsedMs`.
export type Held = { listing: Listing; began: number };

// The kept listing, with when the connection it came on was accepted.
type Kept = Held & { connection: number };

const AGENT_EVENT = { working: "agentWorking", idle: "agentIdle", waiting: "agentWaiting", exited: "agentExited" } as const;

export async function keptListing(storage: Storage): Promise<Held | null> {
  const kept = await storage.get<Kept>(LISTING);
  return kept === undefined ? null : { listing: kept.listing, began: kept.began };
}

// Keeps an agent's hook as its worktree's newest; false for one no newer than that, which
// is dropped so one that lands late never undoes what its agent did since. Two hooks of one
// worktree share their host's clock, so they are ordered by it (`at`); the hook's time by the
// Durable Object's is its receipt less the time it says since (`elapsedMs`). Nothing awaited
// between the read and the write but storage, so no other report comes between them.
export async function keepHook(storage: Storage, report: HookReport, nowMs: number): Promise<boolean> {
  const taken = await storage.get<HookTime>(HOOK_AT + report.worktreeId);
  if (taken !== undefined && taken.at >= report.at) return false;
  await storage.put(HOOK_AT + report.worktreeId, { at: report.at, seen: nowMs - report.elapsedMs } satisfies HookTime);
  return true;
}

// Keeps a listing, and when it is complete marks the workers' records missing from it
// (state.ts, `markMissing`) and reports those missing since more than MISSING_KEEP_MS ago
// gone, and those on a host no longer paired at once (`reportGone`), then applies it to every
// worktree the index holds; false for one from a connection older than the kept one's, which
// is dropped. One connection sends its listings in order, so the newest is the last taken, whatever the workstation's clock says.
// Nothing awaited but storage between the read and the write, so no other listing comes
// between them.
export async function keepListing(storage: Storage, listing: Listing, connection: number, nowMs: number, sender: Sender): Promise<boolean> {
  const kept = await storage.get<Kept>(LISTING);
  if (kept !== undefined && kept.connection > connection) return false;
  const held = { listing, began: nowMs - listing.elapsedMs };
  await storage.put(LISTING, { ...held, connection } satisfies Kept);
  if (listing.listed !== null) {
    await markMissing(storage, listing.listed, listing.hosts, nowMs);
  }
  const gone = listing.listed === null ? [] : await goneWorkers(storage, listing.hosts, nowMs);
  await reportGone(storage, [...gone, ...(await unpairedWorkers(storage, listing.paired))], nowMs, sender);
  for (const [key, issue] of await storage.list<string>({ prefix: WORKTREE })) await applyListing(storage, held, key.slice(WORKTREE.length), issue, nowMs, sender);
  if (await pruneDue(storage, HOOKS_PRUNED_AT, nowMs)) await pruneHooks(storage, nowMs);
  return true;
}

// The listing's state of one worktree, to its issue while it is that issue's worktree. One
// the machine refuses is logged, and the next listing derives it again.
export async function applyListing(storage: Storage, held: Held, worktreeId: string, issue: string, nowMs: number, sender: Sender): Promise<void> {
  try {
    const store = await loadStore(storage, issue);
    const facts = store.facts;
    if (facts.worktree?.id !== worktreeId) {
      // An issue that names another worktree is done with this one (store.ts, `drops`): what a
      // start's late result indexed goes the same way. One with none is kept: a start's result may still name it, or its agent's hooks route by it until `reportGone` finds it gone.
      if (facts.worktree !== null) await storage.delete([WORKTREE + worktreeId, HOOK_AT + worktreeId]);
      return;
    }
    const seen = (await storage.get<HookTime>(HOOK_AT + worktreeId))?.seen ?? null;
    // A hook from before the launch whose first turn is watched is the agent it replaced.
    const w = store.work;
    const launching = w?.state === "launching" || (w?.firstTurn != null && seen !== null && seen < w.firstTurn);
    const event = fromListing(held, worktreeId, issue, seen, facts.agent?.status ?? null, launching);
    if (event !== null) await deliver(storage, event, nowMs, sender);
  } catch (thrown) {
    warn("listing_refused", { issueId: issue, error: describe(thrown) });
  }
}

// Deletes the hook times of worktrees the index doesn't hold, once older than HOOK_AT_KEEP_MS:
// a hook from a worktree its issue has left lands after the index dropped it.
async function pruneHooks(storage: Storage, nowMs: number): Promise<void> {
  await storage.put(HOOKS_PRUNED_AT, nowMs);
  for (const [key, hook] of await storage.list<HookTime>({ prefix: HOOK_AT })) {
    if (nowMs - hook.seen > HOOK_AT_KEEP_MS && (await storage.get(WORKTREE + key.slice(HOOK_AT.length))) === undefined) await storage.delete(key);
  }
}

// A listing as the event of one worktree, or null when it says nothing new of it, as of one
// on a host that didn't answer it. A worktree a complete listing lacks is left to
// `worktreeGone`; one it has with no agent in
// it has exited, but only an agent the machine has seen, or whose hook ran there, exits: one
// it never saw is its start's deadlines'. A hook that ran before its start's result indexed the
// worktree reached no machine, so its exit is the listing's to say once the start is answered;
// while a launch is in flight, or one from before it while its first turn is watched
// (`launching`), it may be the agent a relaunch replaces. An agent shown still there within EXIT_GRACE_MS of its hook saying
// it exited is its pane not yet gone, not an agent continued by hand, and one shown idle within
// BACKGROUND_HOLD_MS of its hook saying it waits in the background is that wait. `seen` is when the
// worktree's newest hook ran, by the Durable Object's clock, as `began` is when the listing did.
export function fromListing({ listing, began }: Held, worktreeId: string, issue: string, seen: number | null, agent: AgentStatus | null, launching = false): Record<string, unknown> | null {
  if (seen !== null && seen >= began) return null;
  if (!heard(worktreeId, listing.hosts)) return null;
  const state = listing.agents[worktreeId];
  if (state === undefined && listing.listed !== null && !listing.listed.includes(worktreeId)) return null;
  if (state === undefined ? (agent === null && (seen === null || launching)) || agent === "exited" || agent === "gone" : state === agent) return null;
  if (state !== undefined && agent === "exited" && seen !== null && began - seen < EXIT_GRACE_MS) return null;
  if (state === "idle" && agent === "background" && seen !== null && began - seen < BACKGROUND_HOLD_MS) return null;
  return { id: `listing:${listing.at}`, issue, kind: AGENT_EVENT[state ?? "exited"] };
}

// Time alone says nothing: each worktree missing from every complete listing for more than
// MISSING_KEEP_MS (state.ts), as only a complete listing received after the hour shows, and
// each on a host the listing's `paired` leaves out, which needs neither the hour nor a
// complete listing, is `worktreeGone` to its issue while it is that issue's worktree, then its
// record is forgotten, and its index entries with it, whether or not it was its issue's
// worktree: the machine's own drop (store.ts, `drops`) leaves none of the current one's.
// Delivered first, so a failure between the two delivers it again on the next listing, which
// finds the machine's worktree already cleared and says nothing. One that throws, refused or
// not, is logged, and its record forgotten all the same; its index entries stay when the
// issue still names it, as that issue's hooks and observations map by them.
async function reportGone(storage: Storage, gone: { worktreeId: string; missingMs?: number }[], nowMs: number, sender: Sender): Promise<string[]> {
  const issues: string[] = [];
  for (const { worktreeId, missingMs } of gone) {
    const issue = (await storage.get<string>(WORKTREE + worktreeId)) ?? null;
    let refused = false;
    try {
      if (issue !== null && (await loadStore(storage, issue)).facts.worktree?.id === worktreeId) {
        await deliver(storage, { id: `gone:${missingMs ?? nowMs}`, issue, kind: "worktreeGone" }, nowMs, sender);
        issues.push(issue);
        info("worktree_gone", { issueId: issue });
      }
    } catch (thrown) {
      refused = true;
      warn("gone_refused", { issueId: issue, error: describe(thrown) });
    }
    if ((await forgetGone(storage, worktreeId, missingMs)) && !refused) await storage.delete([WORKTREE + worktreeId, HOOK_AT + worktreeId]);
  }
  return issues;
}
