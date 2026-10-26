import { type AppToken, fetchAppToken, isUsable, UnauthorizedError } from "../shared/linear.ts";
import { describe, error, info, warn } from "./log.ts";

// The app's client credentials, from the Worker's environment, which the Durable Object
// shares.
export interface LinearApp {
  LINEAR_CLIENT_ID: string;
  LINEAR_CLIENT_SECRET: string;
}

// Held per isolate, so a cold isolate mints a new token, well inside the ten-second
// window. Tokens with the same scopes coexist (Linear allows up to 1000 active), and
// each lasts 30 days, so at a handful of delegations a day this stays far below the cap.
let cachedToken: AppToken | null = null;

const DAY_MS = 24 * 60 * 60 * 1000;
export const HANDED_LEFT_MS = 2 * DAY_MS;

// Runs `call`, and once more with a fresh token if Linear rejects the cached one:
// it has expired, or the client secret was rotated. Failures are logged, never thrown,
// since nothing is waiting on them. Returns whether it succeeded.
export async function withToken(env: LinearApp, sessionId: string, event: string, call: (token: string) => Promise<void>): Promise<boolean> {
  try {
    try {
      await call(await token(env));
    } catch (thrown) {
      if (!(thrown instanceof UnauthorizedError)) throw thrown;
      warn("linear_token_rejected", { sessionId });
      cachedToken = null;
      await call(await token(env));
    }
    return true;
  } catch (thrown) {
    // Session id only: the issue text and the people in it stay out of the logs.
    error(event, { sessionId, error: describe(thrown) });
    return false;
  }
}

async function token(env: LinearApp): Promise<string> {
  return (await appToken(env)).accessToken;
}

// The token, with when it expires, minted again when it has `leftMs` or less to run. A worker
// host is handed one at /token (index.ts) with HANDED_LEFT_MS, twice the day within which the
// host treats one as expired, so a host clock that runs ahead doesn't send it back for another
// on every call; and never `rejected`, the one a host last had, which Linear refused, so a
// token Linear revoked isn't handed back.
export async function appToken(env: LinearApp, leftMs = DAY_MS, rejected: string | null = null): Promise<AppToken> {
  const now = Date.now();
  const refused = rejected !== null && cachedToken?.accessToken === rejected;
  if (refused) warn("linear_token_rejected", { by: "host" });
  if (refused || !isUsable(cachedToken, now) || cachedToken.expiresAt - now <= leftMs) {
    const reason = cachedToken === null ? "none" : refused ? "rejected" : "expiring";
    cachedToken = await fetchAppToken(env.LINEAR_CLIENT_ID, env.LINEAR_CLIENT_SECRET, now);
    // When and why, never the token.
    info("linear_token_minted", { reason, durationMs: Date.now() - now });
  }
  return cachedToken;
}
