import assert from "node:assert/strict";
import { test } from "node:test";
import { codeShaped, credentialShaped, ENROL_MS, enrol, hostOf, hostOp, issue, keptAtReset, redeem } from "../src/worker/hosts.ts";
import type { Storage } from "../src/worker/storage.ts";
import { MemoryStorage } from "./storage.ts";

// Whether the host held a credential, as the revoke's log line says.
const revoke = async (storage: Storage, host: string) => (await hostOp(storage, { op: "revokeHost", host }, true, 1))?.log.held;

test("a host's credential is its alone, until it is revoked or replaced", async () => {
  const storage = new MemoryStorage();
  const credential = await issue(storage, "wsl", 1);
  assert.match(credential, /^wsl:[A-Za-z0-9_-]{43}$/);
  assert.equal(await hostOf(storage, credential), "wsl");
  // Only its digest is stored.
  assert.ok(!JSON.stringify([...(await storage.list({ prefix: "" })).values()]).includes(credential.slice(4)));
  // Another host's name, a changed secret or none at all is nobody.
  const other = await issue(storage, "mac", 1);
  for (const forged of [`mac:${credential.slice(4)}`, `${credential.slice(0, -1)}${credential.endsWith("A") ? "B" : "A"}`, "wsl", "wsl:", "", `bad host:${credential.slice(4)}`]) {
    assert.equal(await hostOf(storage, forged), null, forged);
  }
  const replaced = await issue(storage, "wsl", 2);
  assert.equal(await hostOf(storage, credential), null);
  assert.equal(await hostOf(storage, replaced), "wsl");
  assert.equal(await revoke(storage, "wsl"), true);
  assert.equal(await hostOf(storage, replaced), null);
  assert.equal(await revoke(storage, "wsl"), false);
  assert.equal(await hostOf(storage, other), "mac");
  // The Worker's own check, before the Durable Object.
  assert.ok(credentialShaped(other) && !credentialShaped("wsl") && !credentialShaped("realtime-secret") && !credentialShaped(`:${other.slice(4)}`));
});

test("only the owner issues or revokes a host's credential", async () => {
  const storage = new MemoryStorage();
  assert.equal(await hostOp(storage, { op: "workers" }, true, 1), null);
  const refused = await hostOp(storage, { op: "issueHost", host: "wsl" }, false, 1);
  assert.equal(refused?.status, 403);
  assert.equal((await hostOp(storage, { op: "issueHost", host: "a b" }, true, 1))?.status, 400);
  const issued = await hostOp(storage, { op: "issueHost", host: "wsl" }, true, 1);
  const { credential } = JSON.parse(issued!.body) as { credential: string };
  assert.equal(await hostOf(storage, credential), "wsl");
  // The log keeps the host, never the credential.
  assert.ok(!JSON.stringify(issued!.log).includes(credential));
  assert.equal((await hostOp(storage, { op: "revokeHost", host: "wsl" }, false, 1))?.status, 403);
  assert.equal(await hostOf(storage, credential), "wsl");
  // A second revoke, or a resend, finds none held.
  for (const held of [true, false]) assert.deepEqual(JSON.parse((await hostOp(storage, { op: "revokeHost", host: "wsl" }, true, 1))!.body), { held });
  assert.equal(await hostOf(storage, credential), null);
});

test("only the owner admits a host and lists the admitted ones, by ID alone", async () => {
  const storage = new MemoryStorage();
  const enrolled = async () => JSON.parse((await hostOp(storage, { op: "enrolledHosts" }, true, 1))!.body);
  const admit = async (host: string, owner = true) => await hostOp(storage, { op: "admitHost", host }, owner, 2);
  assert.deepEqual(await enrolled(), { hosts: [] });
  const credential = await issue(storage, "wsl", 1);
  await issue(storage, "mac", 1);
  await enrol(storage, "new", {}, 1);
  // Held but not yet admitted: `host add` hasn't passed.
  assert.deepEqual(await enrolled(), { hosts: [] });
  assert.equal((await admit("wsl", false))?.status, 403);
  assert.deepEqual(JSON.parse((await admit("wsl"))!.body), { admitted: true });
  assert.deepEqual(JSON.parse((await admit("new"))!.body), { admitted: false });
  await admit("mac");
  const listed = await hostOp(storage, { op: "enrolledHosts" }, true, 1);
  assert.deepEqual(JSON.parse(listed!.body).hosts.sort(), ["mac", "wsl"]);
  assert.ok(!listed!.body.includes(credential.slice(4)));
  assert.equal((await hostOp(storage, { op: "enrolledHosts" }, false, 1))?.status, 403);
  await revoke(storage, "wsl");
  assert.deepEqual(await enrolled(), { hosts: ["mac"] });
  // A new credential, as a rejoin's, is admitted again only by its own `host add`.
  await issue(storage, "mac", 3);
  assert.deepEqual(await enrolled(), { hosts: [] });
  // A reset keeps the admission.
  await admit("mac");
  for (const [key, value] of await storage.list({ prefix: "host-credential:" })) assert.deepEqual(keptAtReset(key, value), value);
});

test("a storage reset keeps every host's credential", async () => {
  const storage = new MemoryStorage();
  const credential = await issue(storage, "wsl", 1);
  for (const [key, value] of await storage.list({ prefix: "" })) assert.deepEqual(keptAtReset(key, value), value);
  assert.equal(keptAtReset("worker:x", { digest: "a", issuedMs: 1 }), undefined);
  assert.equal(keptAtReset("host-credential:wsl", { digest: 1 }), undefined);
  assert.equal(await hostOf(storage, credential), "wsl");
});

test("an enrolment code trades once, in time, for its host's credential and settings", async () => {
  const storage = new MemoryStorage();
  const config = { owner: { id: "owner" }, host: "wsl", repos: { tolkster: {} } };
  const code = await enrol(storage, "wsl", config, 1000);
  assert.ok(codeShaped(code) && !codeShaped(`${code}0`) && !codeShaped(code.toUpperCase()) && !codeShaped(7));
  // Storage holds neither the code nor the settings readably.
  const stored = JSON.stringify([...(await storage.list({ prefix: "" })).entries()]);
  assert.ok(!stored.includes(code) && !stored.includes("owner") && !stored.includes("tolkster"));
  const traded = await redeem(storage, code, 2000);
  assert.equal(traded?.host, "wsl");
  assert.deepEqual(JSON.parse(traded!.config), config);
  assert.equal(await hostOf(storage, traded!.credential), "wsl");
  // A second use, a changed code, or one used late gets nothing, and issues nothing.
  assert.equal(await redeem(storage, code, 2000), null);
  assert.equal(await redeem(storage, `${code.slice(0, -1)}${code.endsWith("a") ? "b" : "a"}`, 2000), null);
  const late = await enrol(storage, "mac", config, 1000);
  assert.equal(await redeem(storage, late, 1000 + ENROL_MS), null);
  assert.equal(await redeem(storage, late, 1000), null);
  assert.equal(await hostOf(storage, "mac:" + "A".repeat(43)), null);
  assert.equal(storage.data.has("host-credential:mac"), false);
});

test("minting a code deletes the host's others and those expired, a revoke the host's, and a reset keeps none", async () => {
  const storage = new MemoryStorage();
  await enrol(storage, "wsl", {}, 0);
  const replaced = await enrol(storage, "mac", {}, ENROL_MS);
  const live = await enrol(storage, "mac", {}, ENROL_MS);
  assert.equal([...(await storage.list({ prefix: "host-enrolment:" })).keys()].length, 1);
  for (const [key, value] of await storage.list({ prefix: "" })) assert.equal(keptAtReset(key, value), undefined);
  assert.equal(await redeem(storage, replaced, ENROL_MS + 1), null);
  assert.equal((await redeem(storage, live, ENROL_MS + 1))?.host, "mac");
  // A revoke leaves no code that would issue the host a credential again.
  const pending = await enrol(storage, "mac", {}, ENROL_MS);
  assert.equal(await revoke(storage, "mac"), true);
  assert.equal(await redeem(storage, pending, ENROL_MS + 1), null);
  assert.equal(storage.data.has("host-credential:mac"), false);
});

test("only the owner mints an enrolment code, for a host and a settings object", async () => {
  const storage = new MemoryStorage();
  assert.equal((await hostOp(storage, { op: "enrolHost", host: "wsl", config: {} }, false, 1))?.status, 403);
  for (const config of [undefined, null, [], "x"]) assert.equal((await hostOp(storage, { op: "enrolHost", host: "wsl", config }, true, 1))?.status, 400);
  const minted = await hostOp(storage, { op: "enrolHost", host: "wsl", config: { secretish: "value" } }, true, 1);
  const { code } = JSON.parse(minted!.body) as { code: string };
  // The log keeps the host, never the code or the settings.
  assert.ok(!JSON.stringify(minted!.log).includes(code) && !JSON.stringify(minted!.log).includes("value"));
  assert.deepEqual(JSON.parse((await redeem(storage, code, 2))!.config), { secretish: "value" });
});
