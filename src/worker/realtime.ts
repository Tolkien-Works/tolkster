// The realtime transport beside the state machines, whose commands, results and reports go
// over the same WebSocket (relay.ts): the connection's liveness, the secret it is opened
// with, and each repo's base tip, the coordinator's frame and the head waiting for a slot, level
// state (`moveBase`, `bases`, `publish`): sent, never queued or acknowledged.
//
// A base tip carries a repo's name, a branch and a SHA only. No titles, descriptions,
// comments, reviews or prompt bodies go on the wire or into the relay's storage: Linear
// and GitHub text is untrusted, and workers read it themselves.
//
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node.

import type { Frame } from "./machine/frame.ts";
import type { Storage } from "./storage.ts";

// A repo's default branch as the relay last heard it pushed (github.ts, `fromPush`): level
// state, so it has no `id` or `at` and is never acknowledged. `listen` takes only
// Tolkster's own, for its update (cli/src/update.rs). The tip carries no time: the relay
// orders pushes by their commit's time (`moveBase`), since a release `listen` swaps to the
// Worker's build of whatever tip it is sent, older or not.
export type BaseTip = { type: "base"; repo: string; ref: string; after: string };

// Answered by the relay without waking it (setWebSocketAutoResponse). The CLI sends
// these to find out that a connection has died.
export const PING = "ping";
export const PONG = "pong";

// The close code of a connection a newer acting one replaced (`supersede`): `listen`
// drains and exits on it rather than reconnecting. Pinned in test/contract.json.
export const SUPERSEDED = 4000;

// The settings whose absence refuses /realtime: the 503 body names the unset one, `{ "missing":
// [name] }`, as /token does, so `listen` says which to set. A fixed set of names, never a value;
// pinned in test/contract.json.
export const UNSET_SETTINGS = ["OWNER_USER_ID", "REALTIME_SECRET"] as const;

export function unset(name: (typeof UNSET_SETTINGS)[number]): Response {
  return new Response(JSON.stringify({ missing: [name] }), { status: 503, headers: { "Content-Type": "application/json", "Cache-Control": "no-store" } });
}

// How recently a connection must have pinged, or been accepted, to count as live:
// `listen` pings every 30 s and gives up on a connection it hasn't heard from in 75 s
// (DEAD_AFTER in cli/src/listen.rs), so the relay reads a half-open one as gone at the
// same moment `listen` does.
export const LIVE_WITHIN_MS = 75 * 1000;

// What the relay knows of a connection without waking it: when its last ping was
// auto-answered, if one has been, and when it was accepted. The runtime keeps the first
// (getWebSocketAutoResponseTimestamp); the relay attaches the second to the socket when
// it accepts it, since the first ping comes only 30 s in.
export type Connection = { pingedAt: number | null; acceptedAt: number };

// Whether something is listening: a connection is open that has pinged, or was accepted,
// within LIVE_WITHIN_MS. `tolkster listen --print` holds a connection too, so it counts,
// as docs/realtime.md says. The Worker asks this before acknowledging the owner, so it can
// say when nothing is listening (decide.ts, AWAY); it changes the wording only, never
// what is queued or recorded.
export function listening(connections: Connection[], nowMs: number): boolean {
  return connections.some(({ pingedAt, acceptedAt }) => {
    return nowMs - Math.max(pingedAt ?? -Infinity, acceptedAt) <= LIVE_WITHIN_MS;
  });
}

// When a connection is accepted: later than every one held, so the order of acceptance is
// total (`Date.now()` advances only across I/O, and two connects can land in one millisecond).
export function acceptedAfter(nowMs: number, held: number[]): number {
  return Math.max(nowMs, ...held.map((at) => at + 1));
}

// Closes every connection accepted before `acceptedAt` that isn't `--print` with SUPERSEDED,
// so only one `listen` acts. By time, never by identity: two connects overlapping each close
// only the other's older, so the newer survives. A `--print` one is never closed. Returns
// how many it closed; one already dead is left alone.
export function supersede<S extends { close(code: number, reason: string): void }>(sockets: { socket: S; print: boolean; acceptedAt: number }[], acceptedAt: number): number {
  let closed = 0;
  for (const { socket, print, acceptedAt: at } of sockets) {
    if (print || at >= acceptedAt) continue;
    try {
      socket.close(SUPERSEDED, "Superseded");
      closed++;
    } catch {
      // Already closed.
    }
  }
  return closed;
}

// Sends the frame (machine/frame.ts) to every socket when it differs
// from the last of its type sent, kept as its JSON, and to `fresh`, a connection just
// accepted, whatever it is. Returns how many sockets took it.
export async function publish(storage: Storage, sockets: RelaySocket[], level: Frame, fresh: RelaySocket | null = null): Promise<number> {
  const key = `${level.type}:sent`;
  const message = JSON.stringify(level);
  const changed = (await storage.get<string>(key)) !== message;
  if (changed) await storage.put(key, message);
  return sendAll(changed ? sockets : fresh === null ? [] : [fresh], message);
}

// Each repo's `BaseTip`, by its lowercased name.
const BASE = "basetip:";

export interface RelaySocket {
  send(message: string): void;
}

// When the held tip's commit was made, in ms: kept beside the tip, off the wire.
const BASE_AT = "baseat:";

// A push to a repo's default branch, whose head commit was made at `at` (0 when the push
// names no time): kept as the repo's tip, and sent to every connected socket when it moved
// it. A push whose commit is older than the held one's is dropped, so a late or replayed one
// can't move a release `listen` down. Returns, for the log, how many sockets took it, none
// for a tip already held, as a redelivery's is, or one dropped.
export async function moveBase(storage: Storage, sockets: RelaySocket[], tip: BaseTip, at: number): Promise<number> {
  const name = tip.repo.toLowerCase();
  const [held, heldAt] = await Promise.all([storage.get<BaseTip>(BASE + name), storage.get<number>(BASE_AT + name)]);
  if (held?.ref === tip.ref && held.after === tip.after) return 0;
  if (at !== 0 && at < (heldAt ?? 0)) return 0;
  await storage.put({ [BASE + name]: tip, [BASE_AT + name]: Math.max(at, heldAt ?? 0) });
  return sendAll(sockets, JSON.stringify(tip));
}

// Sends a newly connected socket every repo's tip, so a push that came while nothing
// listened reaches it. Returns how many it sent.
export async function bases(storage: Storage, socket: RelaySocket): Promise<number> {
  const tips = [...(await storage.list<BaseTip>({ prefix: BASE })).values()];
  for (const tip of tips) send(socket, JSON.stringify(tip));
  return tips.length;
}

// Returns how many sockets took it.
function sendAll(sockets: RelaySocket[], message: string): number {
  let sent = 0;
  for (const socket of sockets) if (send(socket, message)) sent++;
  return sent;
}

// A socket that has just died throws: the next connection is sent every tip (`bases`).
// Returns whether it was sent.
function send(socket: RelaySocket, message: string): boolean {
  try {
    socket.send(message);
    return true;
  } catch {
    return false;
  }
}

// Whether `Authorization: Bearer <secret>` carries the secret. Both sides are hashed
// first, so the comparison takes the same time whatever the header holds.
export async function bearerMatches(header: string | null, secret: string): Promise<boolean> {
  if (!secret || !header?.startsWith("Bearer ")) return false;
  const [given, expected] = await Promise.all([sha256(header.slice("Bearer ".length)), secretDigest(secret)]);
  let difference = 0;
  for (let i = 0; i < expected.length; i++) difference |= given[i] ^ expected[i];
  return difference === 0;
}

// The secret's digest, computed once per isolate, and again only if the secret changes.
let cachedDigest: { secret: string; digest: Promise<Uint8Array> } | null = null;

function secretDigest(secret: string): Promise<Uint8Array> {
  if (cachedDigest?.secret !== secret) cachedDigest = { secret, digest: sha256(secret) };
  return cachedDigest.digest;
}

async function sha256(text: string): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text)));
}
