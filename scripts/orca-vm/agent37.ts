// The Agent37 API behind orca.yaml's `agent37` recipe, as the `Provider` the shared recipe steps
// (recipe.ts) take, with the steps only Agent37's base needs. The key is AGENT37_API_KEY and the
// base's git token `gh auth token`: neither is written anywhere.

import {
  SERVE_PORT, type Exec, type Provider, ghToken, log, projectRoot, shLong as shLongOn, sleep, stateFile, withGitToken,
} from "./recipe.ts";

export { ghToken, log, withGitToken };

const API = "https://api.agent37.com/v1";
const SANDBOX = "https://sandbox.agent37.app/v1";

export const HOME = "/home/node";

// Ids, names and digests only, never a credential. `baseInstanceId` is set only once its base
// is ready to fork. The inputs are the digests (`inputs`) the template and the base were last
// built from, and `imageRevision` the revision that build published; `buildingBase` is a base
// being built and `retired` the instances left to delete, which a cut-off `prepare` leaves to
// the next one.
export type State = {
  template?: string; orcaVersion?: string; baseInstanceId?: string; repoUrl?: string; repoRef?: string;
  imageInputs?: string; imageRevision?: string; baseInputs?: string; buildingBase?: string; retired?: string[];
};

// `agent37-state.json`, read as `prepare` writes it.
const state = stateFile<State>("agent37");
export const readState = state.read;
export const mergeState = state.merge;

function key(): string {
  const value = process.env.AGENT37_API_KEY;
  if (!value) throw new Error("AGENT37_API_KEY is not set");
  return value;
}

// One API call; `try_again` and a capacity miss are retried, as Agent37 asks.
export async function api<T = any>(method: string, path: string, body?: unknown, base = API): Promise<T> {
  const headers: Record<string, string> =
    base === API ? { Authorization: `Bearer ${key()}` } : { "X-Agent37-Key": key() };
  if (body !== undefined) headers["Content-Type"] = "application/json";
  for (let attempt = 1; ; attempt++) {
    const response = await fetch(`${base}${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await response.text();
    if (response.ok) return (text ? JSON.parse(text) : {}) as T;
    const retry = (response.status === 409 || response.status === 503) && text.includes("try_again");
    if (!retry || attempt === 8) throw new Error(`Agent37 ${method} ${path}: ${response.status} ${text}`);
    await sleep(attempt * 5000);
  }
}

// The instance object, once it runs.
export async function waitRunning(id: string): Promise<any> {
  for (let i = 0; i < 120; i++) {
    const instance = await api("GET", `/instances/${id}`);
    if (instance.status === "running") return instance;
    if (instance.status === "failed") throw new Error(`instance ${id} failed to start`);
    await sleep(5000);
  }
  throw new Error(`instance ${id} did not reach running`);
}

// A command run in the instance by `sh -c`; it must finish within Node fetch's five minutes.
export function exec(id: string, command: string, user?: "root"): Promise<Exec> {
  return api<Exec>("POST", `/instances/${id}/exec`, { command, ...(user ? { user } : {}) }, SANDBOX);
}

// The pairing address Orca dials: the workspace's public port, as a WebSocket URL.
export async function pairingAddress(id: string): Promise<string> {
  const ports = await api<{ data?: { port: number; url: string }[] } | { port: number; url: string }[]>(
    "GET",
    `/instances/${id}/public-ports`,
  );
  const list = Array.isArray(ports) ? ports : (ports.data ?? []);
  let entry = list.find((p) => p.port === SERVE_PORT);
  entry ??= await api<{ port: number; url: string }>("POST", `/instances/${id}/public-ports`, { port: SERVE_PORT, label: "orca serve" });
  return entry.url.replace(/^https:/, "wss:");
}

export const agent37: Provider = { name: "agent37", home: HOME, exec, pairingAddress };
export const PROJECT_ROOT = projectRoot(agent37);

// agent37-base.ts's own call: it is the base's inputs, so it keeps its text and the base stays.
export const shLong = (id: string, script: string, what: string, minutes: number) => shLongOn(agent37, id, script, what, minutes);

// The template revision an instance object says it runs: Agent37 reads a missing one as 1.
export function revision(instance: { template_revision?: number | null }): string {
  return String(instance.template_revision ?? 1);
}

// What `prepare` does to the base, given the base's inputs now, the revision the ready base
// runs (undefined without one) and the template's: a base built from other inputs is replaced,
// as its setup is on its disk; one on an older image is moved to the new one in place.
export function baseStep(state: State, setup: string, baseRevision: string | undefined, templateRevision: string): "new" | "update" | "none" {
  if (baseRevision === undefined || state.baseInputs !== setup) return "new";
  return baseRevision === templateRevision ? "none" : "update";
}
