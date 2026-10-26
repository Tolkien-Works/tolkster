import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { buildOf, fetchBuild, TARGETS, unzipOne } from "../src/worker/build.ts";
import { enrol, enrolled, ENROL_MS, redeem } from "../src/worker/hosts.ts";
import contract from "./contract.json" with { type: "json" };
import { MemoryStorage } from "./storage.ts";

const env = { GITHUB_APP_KEY: readFileSync(new URL("./throwaway-rsa.pem", import.meta.url), "utf8"), GITHUB_APP_ID: "1234567", GITHUB_INSTALLATION_ID: "87654321" };
const COMMIT = "a".repeat(40);
const NAME = `tolkster-linux-x64-${COMMIT}.tar.gz`;
const TARBALL = Uint8Array.from([0x1f, 0x8b, 8, 0, ...Array.from({ length: 200 }, (_, i) => i % 7)]);

// A zip of one file, as upload-artifact packs one: the sizes in the central directory.
async function zip(name: string, data: Uint8Array, deflate: boolean): Promise<Uint8Array> {
  const packed = deflate ? new Uint8Array(await new Response(new Blob([data]).stream().pipeThrough(new CompressionStream("deflate-raw"))).arrayBuffer()) : data;
  const label = new TextEncoder().encode(name);
  const u16 = (n: number) => [n & 255, n >> 8];
  const u32 = (n: number) => [...u16(n & 0xffff), ...u16(n >>> 16)];
  const method = deflate ? 8 : 0;
  const local = [...u32(0x04034b50), ...u16(20), ...u16(0), ...u16(method), ...u32(0), ...u32(0), ...u32(0), ...u32(0), ...u16(label.length), ...u16(0)];
  const central = [...u32(0x02014b50), ...u16(20), ...u16(20), ...u16(0), ...u16(method), ...u32(0), ...u32(0), ...u32(packed.length), ...u32(data.length), ...u16(label.length), ...u16(0), ...u16(0), ...u16(0), ...u16(0), ...u32(0), ...u32(0)];
  const at = local.length + label.length + packed.length;
  const end = [...u32(0x06054b50), ...u16(0), ...u16(0), ...u16(1), ...u16(1), ...u32(central.length + label.length), ...u32(at), ...u16(0)];
  return Uint8Array.from([...local, ...label, ...packed, ...central, ...label, ...end]);
}

test("the targets are the CLI's", () => {
  assert.deepEqual([...TARGETS].sort(), contract.buildTargets);
});

test("a build path names a known target and a full commit, nothing else", () => {
  assert.deepEqual(buildOf(`/build/linux-x64/${COMMIT}`), { target: "linux-x64", commit: COMMIT });
  for (const path of [`/build/linux-x64`, `/build/linux-x64/main`, `/build/linux-x64/${"A".repeat(40)}`, `/build/linux-arm/${COMMIT}`, `/build/linux-x64/${COMMIT}/x`, `/build/linux-x64/${COMMIT.slice(1)}`, `/x/linux-x64/${COMMIT}`]) {
    assert.equal(buildOf(path), null, path);
  }
});

test("a zip's one file comes out, stored or deflated, and nothing else does", async () => {
  for (const deflate of [false, true]) assert.deepEqual(await unzipOne(await zip(NAME, TARBALL, deflate), NAME), TARBALL);
  assert.equal(await unzipOne(await zip("other.tar.gz", TARBALL, true), NAME), null);
  assert.equal(await unzipOne(new Uint8Array(100), NAME), null);
  // Sizes the central directory misstates are refused, not read past.
  const lying = await zip(NAME, TARBALL, true);
  new DataView(lying.buffer).setUint32(lying.length - 22 - 46 - NAME.length + 24, 5, true);
  assert.equal(await unzipOne(lying, NAME), null);
});

// Answers GitHub's calls, recording them: the mint, the artifact list, the zip and the revoke.
async function github(artifacts: unknown[], run: (calls: [string, RequestInit][]) => Promise<void>, zipped: Promise<Uint8Array> = zip(NAME, TARBALL, true)) {
  const real = globalThis.fetch;
  const calls: [string, RequestInit][] = [];
  const bytes = await zipped;
  globalThis.fetch = (async (url: string, init: RequestInit) => {
    calls.push([url, init]);
    if (url.endsWith("/access_tokens")) return new Response(JSON.stringify({ token: "ghs_secret", repositories: [{ name: "tolkster", owner: { login: "Tolkien-Works" } }] }), { status: 201 });
    if (url.includes("/actions/artifacts?")) return new Response(JSON.stringify({ artifacts }));
    if (url.endsWith("/zip")) return new Response(bytes);
    return new Response(null, { status: 204 });
  }) as typeof fetch;
  try {
    await run(calls);
  } finally {
    globalThis.fetch = real;
  }
}

const artifact = (id: number, over: Record<string, unknown> = {}) => ({ id, expired: false, workflow_run: { head_branch: "main", head_sha: COMMIT, repository_id: 9, head_repository_id: 9 }, ...over });

test("the build is read with an Actions-only token, which is revoked after", async () => {
  await github([artifact(7)], async (calls) => {
    const built = await fetchBuild(env, "Tolkien-Works/tolkster", "linux-x64", COMMIT, Date.now());
    assert.deepEqual("tarball" in built && built.tarball, TARBALL);
    assert.deepEqual(JSON.parse(calls[0][1].body as string), { repositories: ["tolkster"], permissions: { actions: "read" } });
    assert.ok(calls[1][0].includes(`name=tolkster-linux-x64-${COMMIT}`));
    assert.equal(calls[2][0], "https://api.github.com/repos/Tolkien-Works/tolkster/actions/artifacts/7/zip");
    assert.equal(calls[3][0], "https://api.github.com/installation/token");
    assert.equal(calls[3][1].method, "DELETE");
  });
});

test("only a main build of that very commit is served", async () => {
  const wrong = [artifact(1, { expired: true }), artifact(2, { workflow_run: { head_branch: "feature", head_sha: COMMIT, repository_id: 9, head_repository_id: 9 } }), artifact(6, { workflow_run: { head_branch: "main", head_sha: COMMIT, repository_id: 9, head_repository_id: 10 } }), artifact(3, { workflow_run: { head_branch: "main", head_sha: "b".repeat(40), repository_id: 9, head_repository_id: 9 } }), artifact(4, { workflow_run: null })];
  await github(wrong, async (calls) => {
    const built = await fetchBuild(env, "Tolkien-Works/tolkster", "linux-x64", COMMIT, Date.now());
    assert.equal("status" in built && built.status, 404);
    // Nothing downloaded, and the token revoked all the same.
    assert.ok(!calls.some(([url]) => url.endsWith("/zip")));
    assert.equal(calls.at(-1)?.[1].method, "DELETE");
  });
  await github([artifact(5)], async () => {
    const built = await fetchBuild(env, "Tolkien-Works/tolkster", "linux-x64", COMMIT, Date.now());
    assert.equal("status" in built && built.status, 502);
  }, zip("another.tar.gz", TARBALL, true));
  await github([artifact(5)], async () => {
    const built = await fetchBuild(env, "not a repo", "linux-x64", COMMIT, Date.now());
    assert.equal("status" in built && built.status, 503);
  });
});

test("an enrolment code opens a download without being used up, until it is traded or late", async () => {
  const storage = new MemoryStorage();
  const code = await enrol(storage, "wsl", { host: "wsl" }, 1000);
  assert.equal(await enrolled(storage, code, 1001), true);
  assert.equal(await enrolled(storage, code, 1002), true);
  assert.equal(await enrolled(storage, "0".repeat(64), 1001), false);
  assert.equal(await enrolled(storage, "not a code", 1001), false);
  assert.equal(await enrolled(storage, code, 1000 + ENROL_MS), false);
  assert.ok(await redeem(storage, code, 1002));
  assert.equal(await enrolled(storage, code, 1003), false);
});
