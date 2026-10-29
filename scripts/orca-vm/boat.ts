// The boat.dev API behind orca.yaml's `boat` recipe, as the `Provider` the shared recipe steps
// (recipe.ts) take, with what only boat's sandboxes need. The key is BOAT_API_KEY, read only
// from the environment and written nowhere. Every sandbox is made and resumed with `noEnv`, so
// none of the account's environment (its GitHub connection, AI credentials or secrets) reaches
// a machine beside Tolkster's own credentials.

import { createHash } from "node:crypto";
import { type Exec, type Provider, SERVE_PORT, log, sh, sleep, stateFile } from "./recipe.ts";

const API = "https://boat.dev/api/v1";

export const HOME = "/home/user";

// Ids, names and digests only, never a credential. `snapshot` is the saved snapshot `create`
// starts from, set only once it is ready, and `inputs` what it was built from (`snapshotName`);
// `building` is a sandbox a `prepare` is setting up and `retired` the saved snapshots left to
// delete, which a cut-off `prepare` leaves to the next one.
export type State = {
  snapshot?: string; inputs?: string; building?: string; retired?: string[]; repoUrl?: string; repoRef?: string;
};

// `boat-state.json`, read as `prepare` writes it.
const state = stateFile<State>("boat");
export const readState = state.read;
export const mergeState = state.merge;

// The sizes a sandbox may take, from the recipe's `create` line; `default` is 4 vCPU and 8 GB.
export const SIZES = ["small", "default", "large"] as const;
export type Size = (typeof SIZES)[number];

// The trial refuses a sandbox without auto-stop, and allows at most two hours.
const TRIAL_TTL = 7200;

export class BoatError extends Error {
  status: number;
  code: string | undefined;
  constructor(message: string, status: number, code: string | undefined) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

function key(): string {
  const value = process.env.BOAT_API_KEY;
  if (!value) throw new Error("BOAT_API_KEY is not set");
  return value;
}

// How long to wait before sending a call again, as boat's retry guidance has it, or undefined
// for an answer that is final: a 503's `retryAfterSeconds`, a 502 that says it is retryable, and
// a 409 for a create still being made or a machine still starting.
export function retryDelay(status: number, body: any): number | undefined {
  const code = body?.error?.code ?? body?.code;
  if (status === 503) {
    const seconds = Number(body?.retryAfterSeconds ?? body?.error?.retryAfterSeconds ?? body?.error?.details?.retryAfterSeconds);
    return Math.min(Number.isFinite(seconds) && seconds > 0 ? seconds : 5, 60) * 1000;
  }
  if (status === 502 && body?.error?.details?.retryable === true) return 2000;
  if (status === 409 && (code === "idempotency_in_progress" || code === "sandbox_starting")) return 5000;
  return undefined;
}

// One API call, retried as `retryDelay` says.
export async function api<T = any>(method: string, path: string, body?: unknown, headers: Record<string, string> = {}): Promise<T> {
  const sent: Record<string, string> = { Authorization: `Bearer ${key()}`, ...headers };
  if (body !== undefined) sent["Content-Type"] = "application/json";
  for (let attempt = 1; ; attempt++) {
    const response = await fetch(`${API}${path}`, { method, headers: sent, body: body === undefined ? undefined : JSON.stringify(body) });
    const text = await response.text();
    let parsed: any = {};
    try {
      parsed = text ? JSON.parse(text) : {};
    } catch {}
    if (response.ok) return parsed as T;
    const wait = retryDelay(response.status, parsed);
    if (wait === undefined || attempt === 8) {
      throw new BoatError(`boat ${method} ${path}: ${response.status} ${text}`, response.status, parsed?.error?.code ?? parsed?.code);
    }
    await sleep(wait);
  }
}

export const missing = (error: unknown) => error instanceof BoatError && error.status === 404;

// A new sandbox, with auto-stop off so only the fleet sleeps it. On the trial, which refuses
// that, it is made again to stop by itself after two hours. `idempotency` makes a retried create
// return the sandbox the first made; the trial's own key, as boat refuses one key for two bodies.
export async function createSandbox(request: Record<string, unknown>, idempotency?: string): Promise<any> {
  const made = (ttlSeconds: number | null, k?: string) =>
    api("POST", "/sandboxes", { ...request, noEnv: true, ttlSeconds }, k ? { "Idempotency-Key": k } : {});
  try {
    return (await made(null, idempotency)).sandbox;
  } catch (error) {
    if (!(error instanceof BoatError && error.code === "trial_auto_stop_required")) throw error;
    log("boat's trial refuses a sandbox without auto-stop: this one stops by itself after two hours");
    return (await made(TRIAL_TTL, idempotency && `${idempotency}-trial`)).sandbox;
  }
}

// The sandbox object, once it takes commands.
export async function waitReady(id: string): Promise<any> {
  for (let i = 0; i < 120; i++) {
    const { sandbox } = await api("GET", `/sandboxes/${id}`);
    if (["ready", "idle", "running"].includes(sandbox.state)) return sandbox;
    if (sandbox.state === "error" || sandbox.state === "cancelled") throw new Error(`sandbox ${id} is ${sandbox.state}: ${sandbox.error ?? ""}`);
    await sleep(5000);
  }
  throw new Error(`sandbox ${id} did not become ready`);
}

// Stops the sandbox and waits until its disk is saved, so a resume that follows finds it at rest.
// One already stopped, as the trial's auto-stop leaves it, is done.
export async function stop(id: string): Promise<void> {
  if ((await api("GET", `/sandboxes/${id}`)).sandbox.state === "archived") return;
  const stopping = await api("POST", `/sandboxes/${id}/stop`);
  const stopId: string | undefined = stopping.operation?.id ?? stopping.sandbox?.stop?.id;
  for (let i = 0; i < 180; i++) {
    if (stopId) {
      const { stop } = await api("GET", `/sandboxes/${id}/stops/${stopId}`);
      if (stop.status === "completed") return;
      if (stop.status === "superseded") throw new Error(`a resume ended sandbox ${id}'s stop`);
    } else if ((await api("GET", `/sandboxes/${id}`)).sandbox.state === "archived") {
      return;
    }
    await sleep(5000);
  }
  throw new Error(`sandbox ${id} did not stop within 15 minutes`);
}

// Deletes the sandbox and its snapshots, done once boat says so; one already gone is done. boat
// takes it out of every list, resume and fork as soon as it accepts the deletion, so a purge
// still going after five minutes is left to finish.
export async function deleteSandbox(id: string): Promise<void> {
  let operation: any;
  try {
    ({ operation } = await api("DELETE", `/sandboxes/${id}`, undefined, { "X-Ascii-Confirm-Delete": id }));
  } catch (error) {
    if (missing(error)) return;
    throw error;
  }
  for (let i = 0; operation?.id && operation.status !== "completed"; i++) {
    if (i === 60) {
      log(`boat is still deleting ${id} (${operation.status}); it finishes by itself`);
      return;
    }
    await sleep(5000);
    ({ operation } = await api("GET", `/deletion-operations/${operation.id}`));
  }
}

// A command run in the sandbox by bash; it must finish within Node fetch's five minutes.
export async function exec(id: string, command: string): Promise<Exec> {
  const done = await api("POST", `/sandboxes/${id}/commands`, { command, timeoutSeconds: 240 });
  return { exit_code: done.exitCode ?? (done.timedOut ? 124 : 1), stdout: done.stdout ?? "", stderr: done.stderr ?? "" };
}

// The pairing address Orca dials: serve's port hosted at a public URL, as a WebSocket URL. A
// protected URL would carry a `_token` into the recipe JSON and machines.json; Orca's pairing
// code still gates the server. Hosting again returns the same URL, across a stop and resume too.
export async function pairingAddress(id: string): Promise<string> {
  const hosted = await api<{ url: string }>("POST", `/sandboxes/${id}/host`, { port: SERVE_PORT, public: true, title: "orca serve" });
  return hosted.url.replace(/^https:/, "wss:");
}

export const boat: Provider = { name: "boat", home: HOME, exec, pairingAddress };

// Fails unless the sandbox holds none of the owner's credentials: a Claude, Anthropic or GitHub
// token or an AI proxy in a login shell's environment or set in a shell's startup files, a git
// credential, auth header or key, a signed-in `gh` or Claude, or Claude settings that bring a key.
// `noEnv` should keep them all out; this checks it did, before anything of Tolkster's runs there.
// Only names are printed, each marked, so what a profile prints counts for nothing.
export async function assertNoCredentials(id: string): Promise<void> {
  const tokens = "CLAUDE_CODE_OAUTH_TOKEN|ANTHROPIC_API_KEY|ANTHROPIC_AUTH_TOKEN|ANTHROPIC_BASE_URL|GH_TOKEN|GITHUB_TOKEN";
  const check = [
    `for v in ${tokens.replaceAll("|", " ")} GIT_ASKPASS SSH_AUTH_SOCK; do printenv $v > /dev/null && echo "found: $v"; done`,
    `for f in ${HOME}/.git-credentials ${HOME}/.config/git/credentials ${HOME}/.netrc ${HOME}/.config/gh/hosts.yml ${HOME}/.claude/.credentials.json ${HOME}/.ssh/id_*; do test -e "$f" && echo "found: $f"; done`,
    `grep -l -E 'apiKeyHelper|primaryApiKey|${tokens}' ${HOME}/.claude.json ${HOME}/.claude/settings.json /etc/claude-code/managed-settings.json 2> /dev/null | sed 's/^/found: /'`,
    `grep -l -E '(${tokens})=' ${HOME}/.bashrc ${HOME}/.profile ${HOME}/.bash_profile /etc/environment /etc/bash.bashrc /etc/profile.d/* 2> /dev/null | sed 's/^/found: /'`,
    "git config --name-only --get-regexp '^(credential|http\\..*extraheader|url\\.)' 2> /dev/null | sed 's/^/found: /'",
    "true",
  ].join("\n");
  const encoded = Buffer.from(check).toString("base64");
  const out = await sh(boat, id, `echo ${encoded} | base64 -d | bash -l`, "checking the sandbox for credentials");
  const found = out.split("\n").filter((line) => line.startsWith("found: ")).map((line) => line.slice(7));
  if (found.length > 0) throw new Error(`sandbox ${id} holds credentials it should not: ${found.join(", ")}`);
}

// The size on the recipe's `create` line (`--size <size>`), `default` without one; any other
// value is a usage error.
export function size(args: string[]): Size | undefined {
  const at = args.indexOf("--size");
  if (at === -1) return "default";
  const value = args[at + 1];
  return (SIZES as readonly string[]).includes(value) ? (value as Size) : undefined;
}

// What names a base built from these inputs: the setup's digest and the Orca version it
// installs. Each build's saved snapshot is this and its time, the revision a machine made from it
// runs.
export function snapshotName(setup: string, orcaVersion: string): string {
  return `tolkster-orca-${createHash("sha256").update(`${setup}\0${orcaVersion}`).digest("hex").slice(0, 16)}`;
}

// The saved snapshots left to delete once `wanted` replaces the current one: never `wanted`
// itself, listed while it was saved.
export function retire(state: State, wanted: string): string[] {
  const old = state.snapshot && state.snapshot !== wanted ? [state.snapshot] : [];
  return [...new Set([...(state.retired ?? []), ...old])].filter((name) => name !== wanted);
}
