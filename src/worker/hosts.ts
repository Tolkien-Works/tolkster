// Worker hosts' own credentials (docs/hosts.md): one per host, revocable alone, which the
// host calls POST /state with and trades at /token (index.ts) for the tokens it acts with.
// Only a digest is stored, by the host's ID, never the credential, and a revoke deletes it,
// so the host's next call is refused.
//
// A credential is `<host>:<secret>`, the secret 32 random bytes in base64url: random enough
// that one SHA-256 is all the digest needs.
//
// A new host gets its first credential by trading an enrolment code at POST /enrol (index.ts):
// the owner's `tolkster host add` mints one for a host and hands it the settings the host is
// to run with, which the code alone unseals. The code is single-use and lives ENROL_MS; only
// its digest is stored, beside the settings sealed under a key derived from it, so storage
// holds no readable settings and a code can't be recovered from it.
//
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node.

import { object, str } from "./check.ts";
import { HOST, type Outcome } from "./state.ts";
import type { Storage } from "./storage.ts";

const KEY = "host-credential:";
const ENROLMENT = "host-enrolment:";
const SECRET = /^[A-Za-z0-9_-]{43}$/;
// An enrolment code is typed into a shell and read as an argument, so it is hex: no dash to
// start an option, nothing a shell expands.
const CODE = /^[0-9a-f]{64}$/;
export const ENROL_MS = 15 * 60 * 1000;

// `admittedMs` once `host add` passed every step for this credential (`admitHost`): only then
// does the host take work. A new credential starts without it.
type Stored = { digest: string; issuedMs: number; admittedMs?: number };
type Enrolment = { host: string; expiresMs: number; iv: string; sealed: string };

// A new credential for `host`, replacing any it held: a host issued another can't call with
// the old one.
export async function issue(storage: Storage, host: string, nowMs: number): Promise<string> {
  const secret = randomSecret();
  await storage.put<Stored>(KEY + host, { digest: await digest(secret), issuedMs: nowMs });
  return `${host}:${secret}`;
}

// Whether `host` held a credential. Its codes not yet traded go too, so none undoes the revoke.
async function revoke(storage: Storage, host: string, nowMs: number): Promise<boolean> {
  await dropEnrolments(storage, host, nowMs);
  return await storage.delete(KEY + host);
}

// Deletes `host`'s enrolment codes and every code expired.
async function dropEnrolments(storage: Storage, host: string, nowMs: number): Promise<void> {
  const dropped = [...(await storage.list<Enrolment>({ prefix: ENROLMENT }))].filter(([, e]) => e.host === host || e.expiresMs <= nowMs).map(([key]) => key);
  if (dropped.length > 0) await storage.delete(dropped);
}

// Whether `bearer` has a credential's shape: the Worker refuses one that hasn't before it
// reaches the Durable Object.
export function credentialShaped(bearer: string): boolean {
  const at = bearer.indexOf(":");
  return at > 0 && HOST.test(bearer.slice(0, at)) && SECRET.test(bearer.slice(at + 1));
}

// The host `credential` was issued to, or null for one malformed, revoked or replaced.
export async function hostOf(storage: Storage, credential: string): Promise<string | null> {
  if (!credentialShaped(credential)) return null;
  const at = credential.indexOf(":");
  const [host, secret] = [credential.slice(0, at), credential.slice(at + 1)];
  const stored = await storage.get<Stored>(KEY + host);
  if (stored === undefined) return null;
  const given = await digest(secret);
  let difference = given.length ^ stored.digest.length;
  for (let i = 0; i < given.length; i++) difference |= given.charCodeAt(i) ^ stored.digest.charCodeAt(i);
  return difference === 0 ? host : null;
}

// A new enrolment code for `host`, which trades once, before ENROL_MS is up, for a credential
// and `config`, replacing any the host had: one code per host is live. Codes expired are
// deleted on the way.
export async function enrol(storage: Storage, host: string, config: Record<string, unknown>, nowMs: number): Promise<string> {
  await dropEnrolments(storage, host, nowMs);
  const code = [...crypto.getRandomValues(new Uint8Array(32))].map((b) => b.toString(16).padStart(2, "0")).join("");
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const sealed = await crypto.subtle.encrypt({ name: "AES-GCM", iv }, await sealKey(code, ["encrypt"]), new TextEncoder().encode(JSON.stringify(config)));
  await storage.put<Enrolment>(ENROLMENT + (await digest(code)), { host, expiresMs: nowMs + ENROL_MS, iv: base64url(iv), sealed: base64url(new Uint8Array(sealed)) });
  return code;
}

// What `code` trades for: the host it was minted for, a new credential and the settings as
// JSON text, or
// null for a code unknown, used or expired. The record is deleted before anything else, so
// two trades of one code can't both get a credential.
export async function redeem(storage: Storage, code: string, nowMs: number): Promise<{ host: string; credential: string; config: string } | null> {
  if (!CODE.test(code)) return null;
  const key = ENROLMENT + (await digest(code));
  const enrolment = await storage.get<Enrolment>(key);
  if (enrolment === undefined || !(await storage.delete(key)) || enrolment.expiresMs <= nowMs) return null;
  const opened = await crypto.subtle.decrypt({ name: "AES-GCM", iv: unbase64url(enrolment.iv) }, await sealKey(code, ["decrypt"]), unbase64url(enrolment.sealed));
  return { host: enrolment.host, credential: await issue(storage, enrolment.host, nowMs), config: new TextDecoder().decode(opened) };
}

// Whether `code` is an enrolment code not yet traded nor expired. Nothing is used up: a new
// host downloads `tolkster` with its code before `host join` trades it.
export async function enrolled(storage: Storage, code: string, nowMs: number): Promise<boolean> {
  if (!CODE.test(code)) return false;
  const enrolment = await storage.get<Enrolment>(ENROLMENT + (await digest(code)));
  return enrolment !== undefined && enrolment.expiresMs > nowMs;
}

// Whether `code` is shaped as an enrolment code: the Worker refuses one that isn't before it
// reaches the Durable Object.
export function codeShaped(code: unknown): code is string {
  return typeof code === "string" && CODE.test(code);
}

// POST /state's host ops, the owner's alone: `issueHost`, answered with the credential, which
// nothing logs; `enrolHost`, answered with an enrolment code for the host and the `config`
// object it carries, which nothing logs either; `revokeHost`, answered with whether the
// host held one: none for another name, or for a resend of a revoke that landed;
// `admitHost`, answered with whether the host held one to admit; and `enrolledHosts`,
// answered with the IDs of the hosts holding an admitted credential, which alone decides
// that a host takes work. Null for any other op.
export async function hostOp(storage: Storage, body: unknown, owner: boolean, nowMs: number): Promise<Outcome | null> {
  const op = (body as { op?: unknown } | null)?.op;
  if (op !== "issueHost" && op !== "enrolHost" && op !== "revokeHost" && op !== "admitHost" && op !== "enrolledHosts") return null;
  if (!owner) return { status: 403, body: JSON.stringify({ error: "owner only" }), log: { op, reason: "owner only" } };
  if (op === "enrolledHosts") return { status: 200, body: JSON.stringify({ hosts: await admittedHosts(storage) }), log: { op } };
  let host: string;
  try {
    host = str(object(body, "body").host, HOST, "host");
  } catch {
    return { status: 400, body: JSON.stringify({ error: "invalid host" }), log: { op, reason: "invalid host" } };
  }
  if (op === "issueHost") return { status: 200, body: JSON.stringify({ credential: await issue(storage, host, nowMs) }), log: { op, host } };
  if (op === "enrolHost") {
    const config = (body as { config?: unknown }).config;
    if (typeof config !== "object" || config === null || Array.isArray(config)) return { status: 400, body: JSON.stringify({ error: "invalid config" }), log: { op, host, reason: "invalid config" } };
    return { status: 200, body: JSON.stringify({ code: await enrol(storage, host, config as Record<string, unknown>, nowMs) }), log: { op, host } };
  }
  if (op === "admitHost") {
    const stored = await storage.get<Stored>(KEY + host);
    if (stored !== undefined) await storage.put<Stored>(KEY + host, { ...stored, admittedMs: nowMs });
    return { status: 200, body: JSON.stringify({ admitted: stored !== undefined }), log: { op, host, admitted: stored !== undefined } };
  }
  const held = await revoke(storage, host, nowMs);
  return { status: 200, body: JSON.stringify({ held }), log: { op, host, held } };
}

// The IDs of the hosts holding an admitted credential.
export async function admittedHosts(storage: Storage): Promise<string[]> {
  return [...(await storage.list<Stored>({ prefix: KEY }))].flatMap(([key, stored]) => (typeof stored.admittedMs === "number" ? [key.slice(KEY.length)] : []));
}

// What a reset (storage.ts) keeps: every host's credential and its admission, so a new storage
// version revokes none. Undefined for anything else, enrolment codes included.
export function keptAtReset(key: string, value: unknown): unknown {
  if (!key.startsWith(KEY)) return undefined;
  const stored = value as Partial<Stored> | null;
  if (typeof stored?.digest !== "string" || typeof stored.issuedMs !== "number") return undefined;
  return typeof stored.admittedMs === "number" ? { digest: stored.digest, issuedMs: stored.issuedMs, admittedMs: stored.admittedMs } : { digest: stored.digest, issuedMs: stored.issuedMs };
}

async function digest(secret: string): Promise<string> {
  const bytes = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(secret)));
  return [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
}

// The code's own key, distinct from its stored digest, which can't open what it sealed.
async function sealKey(code: string, usages: ("encrypt" | "decrypt")[]): ReturnType<typeof crypto.subtle.importKey> {
  const bytes = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(`enrol-seal:${code}`));
  return await crypto.subtle.importKey("raw", bytes, "AES-GCM", false, usages);
}

// 32 random bytes in base64url: a credential's secret.
function randomSecret(): string {
  return base64url(crypto.getRandomValues(new Uint8Array(32)));
}

function unbase64url(text: string): Uint8Array<ArrayBuffer> {
  return Uint8Array.from(atob(text.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));
}

function base64url(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes)).replace(/=+$/, "").replace(/\+/g, "-").replace(/\//g, "_");
}
