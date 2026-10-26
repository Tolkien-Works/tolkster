import type { Delegated } from "../shared/linear.ts";
import type { Timer } from "./alarm.ts";
import { describe, info, warn } from "./log.ts";
import { chainHolds } from "./machine/chain.ts";
import type { Store } from "./machine/types.ts";
import type { Storage } from "./storage.ts";

// What a storage reset (storage.ts), or `listen`'s connect, rebuilds: each issue Linear shows
// delegated to Tolkster is asked of its store (machine/store.ts, `rebuildAsked`,
// `connectAsked`), whose read asks the owner to reply in a delegation's session the intake
// never mapped, and takes a parent's chain back after a reset. It migrates nothing: Linear is
// read afresh.

// When the rebuild is due, written by the reset or a connect and deleted once every issue was
// asked; and whether a reset owes it, deleted with it. A connect brings no owed rebuild
// forward, so a reconnecting `listen` keeps its retry's wait.
const REBUILD = "rebuild-due";
const RESET = "rebuild-reset";
const RETRY_MS = 10 * 60 * 1000;

export async function oweRebuild(storage: Storage, nowMs: number, reset: boolean): Promise<void> {
  if (reset) await storage.put({ [REBUILD]: nowMs, [RESET]: true });
  else if ((await storage.get<number>(REBUILD)) === undefined) await storage.put(REBUILD, nowMs);
}

// `read` is null when Linear couldn't be read, already logged. A read or an ask that fails
// leaves the rebuild owed RETRY_MS on: an issue already asked asks nothing more.
export function rebuildTimer(storage: Storage, read: (reset: boolean) => Promise<Delegated[] | null>, ask: (issue: Delegated, nowMs: number, reset: boolean) => Promise<void>): Timer {
  return {
    next: async () => (await storage.get<number>(REBUILD)) ?? null,
    run: async (nowMs) => {
      const due = await storage.get<number>(REBUILD);
      if (due === undefined || due > nowMs) return;
      const reset = (await storage.get<boolean>(RESET)) === true;
      try {
        const issues = await read(reset);
        if (issues === null) throw new Error("Linear unread");
        for (const issue of issues) await ask(issue, nowMs, reset);
        await storage.delete([REBUILD, RESET]);
        info("rebuild_asked", { issues: issues.length, reset });
      } catch (thrown) {
        warn("rebuild_failed", { error: describe(thrown), retryInMs: RETRY_MS, reset });
        await storage.put(REBUILD, nowMs + RETRY_MS);
      }
    },
  };
}

// The chain's sessions a request machine runs on while the chain holds: what a reset's rebuild
// begins on sessions the intake lost, for the relay to map again (relay.ts, `learned`).
export function chainSessions(s: Store): string[] {
  if (!chainHolds(s.facts)) return [];
  return Object.keys(s.sessions).filter((id) => s.facts.sessions[id]?.kind === "chain" && s.sessions[id].machine !== null);
}
