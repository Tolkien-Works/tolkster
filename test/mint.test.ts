import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { appRecord, covers, forHosts, installationToken, jwt, PURPOSES, sign } from "../src/worker/mint.ts";
import contract from "./contract.json" with { type: "json" };

// A throwaway key, generated for these tests and never used anywhere else.
const PKCS1 = readFileSync(new URL("./throwaway-rsa.pem", import.meta.url), "utf8");
const PKCS8 = readFileSync(new URL("./throwaway-rsa-pkcs8.pem", import.meta.url), "utf8");
const env = { GITHUB_APP_KEY: PKCS1, GITHUB_APP_ID: "1234567", GITHUB_INSTALLATION_ID: "87654321" };

const base64 = (bytes: Uint8Array) => Buffer.from(bytes).toString("base64");
const part = (text: string) => JSON.parse(Buffer.from(text, "base64url").toString("utf8"));

// Answers each call to GitHub with `answer`, recording them.
async function github<T>(answer: (url: string, init: RequestInit) => Response, run: (calls: [string, RequestInit][]) => Promise<T>): Promise<T> {
  const real = globalThis.fetch;
  const calls: [string, RequestInit][] = [];
  globalThis.fetch = (async (url: string, init: RequestInit) => {
    calls.push([url, init]);
    return answer(url, init);
  }) as typeof fetch;
  try {
    return await run(calls);
  } finally {
    globalThis.fetch = real;
  }
}

const minted = (owner: string, name: string) => new Response(JSON.stringify({ token: "ghs_secret", expires_at: "2026-10-19T12:00:00Z", repositories: [{ name, owner: { login: owner } }] }), { status: 201 });

test("the key signs as openssl does, whether GitHub's PKCS#1 or PKCS#8", async () => {
  // `printf tolkster | openssl dgst -sha256 -sign throwaway-rsa.pem | base64`
  const expected = "A07bRSqMs2fecAgEWlQz+zq0f9hI7+wMb209LGwsop9Yze5jJj/jS4HIuU0jUBYEws0LwZdxYo0IreiW2KCRoOwjGSFjQpwtfopEU6F9/4z5CI2ErF84dTBVqzrFqp8yRF29GIWLBbQsweX01h3yVFT1z/NJdg06uqbR6tAOsFw2Cu6XWMHLFVvfvb5uaq4EmSNuUcN9KC4XqXqfwN5Nuyg5oMsvxyw917xH76OT/MXpUmzXImmIyLO8SuLhgcVkTWls7Uc4vUuWI2s6T68o0GLiGDeG7xkZX124rWSsEAgnPBFo74buKfCEvFXic47SPBIvbcYd+8TXVtGDz0C5Qw==";
  for (const pem of [PKCS1, PKCS8, PKCS1.replace(/\r?\n/g, "\n")]) assert.equal(base64(await sign(pem, "tolkster")), expected);
  const broken = PKCS1.replace("MII", "MIX");
  await assert.rejects(sign(broken, "tolkster"), (thrown: Error) => !thrown.message.includes(PKCS1.split("\n")[1].slice(0, 16)));
  await assert.rejects(sign(PKCS1.replace(/RSA PRIVATE KEY/g, "CERTIFICATE"), "tolkster"));
});

test("the JWT is RS256 with the app as issuer", async () => {
  // A secret piped in with its line break reads the same.
  const [header, claims] = (await jwt({ ...env, GITHUB_APP_ID: "1234567\n" }, 1_790_000_000_000)).split(".");
  assert.deepEqual(part(header), { alg: "RS256", typ: "JWT" });
  assert.deepEqual(part(claims), { iat: 1_789_999_940, exp: 1_790_000_540, iss: 1234567 });
});

test("a token is for one repo and its purpose's permissions, and only a repo of that owner", async () => {
  await github(() => minted("Tolkien-Works", "tolkster"), async (calls) => {
    const answer = await installationToken(env, "tolkien-works/Tolkster", "read", 1_790_000_000_000);
    assert.deepEqual(answer, { status: 200, body: { token: "ghs_secret", expiresAt: "2026-10-19T12:00:00Z" } });
    assert.equal(calls[0][0], "https://api.github.com/app/installations/87654321/access_tokens");
    assert.deepEqual(JSON.parse(calls[0][1].body as string), { repositories: ["Tolkster"], permissions: PURPOSES.read });
  });
  // Another owner's repo of the installed name: the token is revoked, never handed out.
  await github((url) => (url.endsWith("/access_tokens") ? minted("Tolkien-Works", "tolkster") : new Response(null, { status: 204 })), async (calls) => {
    const answer = await installationToken(env, "other/tolkster", "create", 1_790_000_000_000);
    assert.equal(answer.status, 422);
    assert.ok(!JSON.stringify(answer.body).includes("ghs_secret"));
    assert.deepEqual([calls[1][0], calls[1][1].method, (calls[1][1].headers as Record<string, string>).Authorization], ["https://api.github.com/installation/token", "DELETE", "Bearer ghs_secret"]);
  });
  await github(() => minted("x", "y"), async (calls) => {
    assert.equal((await installationToken(env, "not a repo", "read", 0)).status, 400);
    assert.equal(calls.length, 0);
  });
});

test("GitHub's refusal is the Worker's own words: a 5xx or a spent budget may pass, others won't", async () => {
  // A suspended installation's 403 says nothing of a budget, so it won't pass.
  const cases: [number, Record<string, string>, number, number | undefined][] = [
    [500, {}, 502, undefined],
    [404, {}, 422, undefined],
    [403, {}, 422, undefined],
    [429, { "retry-after": "30" }, 429, 30],
    [403, { "x-ratelimit-remaining": "0", "x-ratelimit-reset": "1790000100" }, 429, 101],
    [429, {}, 429, 60],
  ];
  for (const [status, headers, ours, retryAfter] of cases) {
    await github(() => new Response(JSON.stringify({ message: "GitHub's text" }), { status, headers }), async () => {
      const answer = await installationToken(env, "a/b", "read", 1_790_000_000_000);
      assert.equal(answer.status, ours, `${status}`);
      assert.equal((answer.body as { retryAfter?: number }).retryAfter, retryAfter, `${status}`);
      assert.ok(!JSON.stringify(answer.body).includes("GitHub's text"));
    });
  }
  // A secondary limit may come with no headers, only GitHub's message.
  await github(() => new Response(JSON.stringify({ message: "You have exceeded a secondary rate limit." }), { status: 403 }), async () => {
    assert.deepEqual(await installationToken(env, "a/b", "read", 0), { status: 429, body: { refused: "GitHub's budget is spent for an installation token", retryAfter: 60 }, reason: "github_403" });
  });
  // A key that won't sign is the owner's to fix: no call is made, and nothing to try again.
  await github(() => minted("a", "b"), async (calls) => {
    assert.equal((await installationToken({ ...env, GITHUB_APP_KEY: "not a key" }, "a/b", "read", 0)).status, 422);
    assert.equal(calls.length, 0);
  });
  const record = (url: string) => new Response(JSON.stringify(url.includes("/installations") ? [{ id: 7, account: { login: "o" } }, { id: "8" }] : { events: ["push", 3, "status"], slug: "owl", owner: { login: "o" } }), { status: 200 });
  // The app's record needs no installation, as `github app create` reads it before there is one.
  await github(record, async () => {
    assert.deepEqual((await appRecord({ GITHUB_APP_KEY: PKCS1, GITHUB_APP_ID: "5078799" }, 0)).body, { events: ["push", "status"], slug: "owl", installations: [7], installed: false });
    // An installation ID left from another app isn't this app's.
    const installed = async (id: string) => ((await appRecord({ ...env, GITHUB_INSTALLATION_ID: id }, 0)).body as { installed: boolean }).installed;
    assert.equal(await installed(" 7\n"), true);
    assert.equal(await installed("164949790"), false);
  });
});

test("the purposes are the CLI's, and only those that must may write contents", () => {
  assert.deepEqual(Object.keys(PURPOSES).sort(), contract.githubPurposes);
  const writes = Object.entries(PURPOSES).filter(([, p]) => (p as Record<string, string>).contents === "write").map(([name]) => name);
  assert.deepEqual(writes.sort(), ["autoMerge", "merge", "push", "resolve", "updateBranch"]);
  assert.ok(Object.values(PURPOSES.read).every((level) => level === "read"));
  // Merging a base into a branch, or a PR into its base, is `listen`'s alone.
  assert.deepEqual(Object.keys(PURPOSES).filter((p) => !forHosts(p as keyof typeof PURPOSES)), ["updateBranch", "merge"]);
});

test("a mint covers a repo only when its response names that owner", () => {
  const mint = (login: unknown, name: string) => ({ repositories: [{ name, owner: { login } }] });
  assert.ok(covers(mint("Tolkien-Works", "Tolkster"), "tolkien-works", "tolkster"));
  // A prefix or a suffix of the owner is another owner, and another name another repo.
  for (const [owner, name] of [["Tolkien", "tolkster"], ["Tolkien-Works2", "tolkster"], ["Tolkien-Works", "other"]]) {
    assert.ok(!covers(mint("Tolkien-Works", "tolkster"), owner, name), `${owner}/${name}`);
  }
  // A missing, empty or malformed owner or repositories list covers nothing.
  for (const bad of [mint(null, "tolkster"), mint("", "tolkster"), {}, { repositories: [] }, { repositories: null }, { repositories: [{ name: "tolkster" }] }, null]) {
    assert.ok(!covers(bad, "Tolkien-Works", "tolkster"), JSON.stringify(bad));
  }
});
