import type { Intaken } from "./intake.ts";
import { warn } from "./log.ts";

// A delegation or prompt should end in a reply the owner can see: an acknowledgement, a
// notice, a refusal or AWAY. When one doesn't, this is where it is said, by the session's ID
// and why, so a session Linear marks stale is found in the log and not by its owner alone.
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node.
//
// - `post_failed`: a refusal the Worker could not post, and no request behind it.
// - `unmapped`: the event could not be made from a session that carries a request.
// - `ingest_failed`: the Durable Object failed the event on every try (index.ts, `ingestSession`).
// - `chain_not_held`: the intake took the session but its chain no longer holds, so the
//   machine made no request (intake.ts, `refused`); the relay tells the owner on the session.
export type Why = "post_failed" | "unmapped" | "ingest_failed" | "chain_not_held";

export function unanswered(sessionId: string, why: Why, fields: Record<string, unknown> = {}): void {
  warn("session_unanswered", { sessionId, why, ...fields });
}

// Why the session the Worker acted on may be left with no reply, or null when something will
// answer it: a refusal is answered by its own post, and a request by the machine once the
// Durable Object has it, even when the acknowledgement was not posted.
export function whyUnanswered(a: { request: boolean; mapped: boolean; posted: boolean; ingested: boolean }): Why | null {
  if (!a.request) return a.posted ? null : "post_failed";
  if (!a.mapped) return "unmapped";
  return a.ingested ? null : "ingest_failed";
}

// The sessions the intake took no request from because their chain no longer holds. A stop is
// left out: "delegate it again" would read as if nothing runs.
export function refusedSessions(results: Intaken[]): string[] {
  return results.flatMap(({ event, outcome }) => (outcome === "refused" && (event.kind === "sessionStarted" || event.kind === "ownerPrompted") ? [event.sessionId] : []));
}

// Tells the owner, on each session the intake refused, that nothing was taken from it, and
// logs it either way. `post` says whether the notice landed.
export async function tellRefused(sessions: string[], post: (sessionId: string) => Promise<boolean>): Promise<void> {
  for (const sessionId of new Set(sessions)) unanswered(sessionId, "chain_not_held", { told: await post(sessionId) });
}
