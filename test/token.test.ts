import assert from "node:assert/strict";
import { test } from "node:test";
import { appToken, HANDED_LEFT_MS } from "../src/worker/token.ts";

const DAY_S = 24 * 60 * 60;
const env = { LINEAR_CLIENT_ID: "id", LINEAR_CLIENT_SECRET: "secret" };

// Each mint lasts `lifetimes`' next value, in seconds, and is named by its count.
async function minting<T>(lifetimes: number[], run: () => Promise<T>): Promise<T> {
  const real = globalThis.fetch;
  let minted = 0;
  globalThis.fetch = (async () => {
    minted++;
    return new Response(JSON.stringify({ access_token: `t${minted}`, expires_in: lifetimes[minted - 1] }), { status: 200 });
  }) as typeof fetch;
  try {
    return await run();
  } finally {
    globalThis.fetch = real;
  }
}

test("a host is never handed back the token Linear refused it, nor one about to expire", async () => {
  await minting([30 * DAY_S, 30 * DAY_S, 1.5 * DAY_S, 30 * DAY_S], async () => {
    const first = await appToken(env, HANDED_LEFT_MS);
    assert.equal((await appToken(env, HANDED_LEFT_MS, "another")).accessToken, first.accessToken);
    const second = await appToken(env, HANDED_LEFT_MS, first.accessToken);
    assert.notEqual(second.accessToken, first.accessToken);
    // A day and a half left is enough for the Worker's own calls, not to hand out.
    const third = await appToken(env, HANDED_LEFT_MS, second.accessToken);
    assert.equal((await appToken(env)).accessToken, third.accessToken);
    assert.notEqual((await appToken(env, HANDED_LEFT_MS)).accessToken, third.accessToken);
  });
});
