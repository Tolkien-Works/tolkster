// What the Agent37 recipe's scripts share (orca.yaml's `agent37` recipe): the Agent37 API,
// exec on an instance, the state file and the steps more than one script runs. Every script
// keeps stdout for its one JSON result; progress and errors go to stderr. The key is
// AGENT37_API_KEY and the base's git token `gh auth token`: neither is written anywhere.

import { execFileSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { existsSync, readFileSync, readdirSync, renameSync, statSync, writeFileSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const API = "https://api.agent37.com/v1";
const SANDBOX = "https://sandbox.agent37.app/v1";
const HERE = fileURLToPath(new URL(".", import.meta.url));
const STATE = join(HERE, "agent37-state.json");

// The port `orca-ide serve` listens on in a workspace, published through a public port.
export const SERVE_PORT = 6768;
export const HOME = "/home/node";
export const PROJECT_ROOT = `${HOME}/tolkster`;

// Ids, names and digests only, never a credential. `baseInstanceId` is set only once its base
// is ready to fork. The inputs are the digests (`inputs`) the template and the base were last
// built from, and `imageRevision` the revision that build published; `buildingBase` is a base
// being built and `retired` the instances left to delete, which a cut-off `prepare` leaves to
// the next one.
export type State = {
  template?: string; orcaVersion?: string; baseInstanceId?: string; repoUrl?: string; repoRef?: string;
  imageInputs?: string; imageRevision?: string; baseInputs?: string; buildingBase?: string; retired?: string[];
};

export function readState(): State {
  return existsSync(STATE) ? (JSON.parse(readFileSync(STATE, "utf8")) as State) : {};
}

// Renamed into place, so a `create` reading it as `prepare` writes never reads half a file.
export function mergeState(change: State): State {
  const state = { ...readState(), ...change };
  writeFileSync(`${STATE}.${process.pid}`, `${JSON.stringify(state, null, 2)}\n`);
  renameSync(`${STATE}.${process.pid}`, STATE);
  return state;
}

export function log(message: string): void {
  console.error(message);
}

// Runs `main`, printing its result as the script's one line of stdout.
export function run(main: () => Promise<unknown>): void {
  main().then(
    (result) => {
      if (result !== undefined) console.log(JSON.stringify(result));
    },
    (error: unknown) => {
      console.error(error instanceof Error ? error.message : String(error));
      process.exitCode = 1;
    },
  );
}

function key(): string {
  const value = process.env.AGENT37_API_KEY;
  if (!value) throw new Error("AGENT37_API_KEY is not set");
  return value;
}

export const sleep = (ms: number) => new Promise((done) => setTimeout(done, ms));

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

export type Exec = { exit_code: number; stdout: string; stderr: string };

// A command run in the instance by `sh -c`; it must finish within Node fetch's five minutes.
export function exec(id: string, command: string, user?: "root"): Promise<Exec> {
  return api<Exec>("POST", `/instances/${id}/exec`, { command, ...(user ? { user } : {}) }, SANDBOX);
}

// Like exec, but fails on a nonzero exit, with the command's stderr.
export async function sh(id: string, command: string, what: string): Promise<string> {
  const result = await exec(id, command);
  if (result.exit_code !== 0) throw new Error(`${what} failed (exit ${result.exit_code}): ${result.stderr || result.stdout}`);
  return result.stdout;
}

// A long bash script, run in the background and polled, so no single exec outlives fetch. The
// script file is private and deletes itself as it starts: it may hold a token.
export async function shLong(id: string, script: string, what: string, minutes: number): Promise<void> {
  const tag = `/tmp/orca-${Date.now()}`;
  const encoded = Buffer.from(`rm -f -- "$0"\n${script}`).toString("base64");
  await sh(
    id,
    `umask 077 && echo ${encoded} | base64 -d > ${tag}.sh && nohup bash -c 'bash ${tag}.sh > ${tag}.log 2>&1; echo $? > ${tag}.exit' >/dev/null 2>&1 &`,
    `starting ${what}`,
  );
  for (let waited = 0; waited < minutes * 60_000; waited += 10_000) {
    await sleep(10_000);
    const done = await exec(id, `cat ${tag}.exit 2>/dev/null`);
    if (done.stdout.trim() === "") continue;
    const tail = await exec(id, `tail -n 40 ${tag}.log`);
    if (done.stdout.trim() !== "0") throw new Error(`${what} failed (exit ${done.stdout.trim()}):\n${tail.stdout}`);
    return;
  }
  throw new Error(`${what} still running after ${minutes} minutes; see ${tag}.log on instance ${id}`);
}

export function ghToken(): string {
  const token = process.env.GH_TOKEN || process.env.GITHUB_TOKEN ||
    execFileSync("gh", ["auth", "token"], { encoding: "utf8" }).trim();
  if (!token) throw new Error("no git token: set GH_TOKEN or log in with gh");
  return token;
}

// Bash that runs `body` with git authenticated by the token, through an askpass helper the
// script removes after; the token is in a 0600 file for the run, never in a URL or argument.
export function withGitToken(token: string, body: string): string {
  const encoded = Buffer.from(token).toString("base64");
  return [
    "set -euo pipefail",
    "umask 077",
    "auth=$(mktemp -d)",
    "trap 'rm -rf \"$auth\"' EXIT",
    `echo ${encoded} | base64 -d > "$auth/token"`,
    `printf '%s\\n' '#!/bin/sh' 'case "$1" in *Username*) echo x-access-token;; *) cat "$(dirname "$0")/token";; esac' > "$auth/askpass"`,
    'chmod 700 "$auth/askpass"',
    'export GIT_ASKPASS="$auth/askpass" GIT_TERMINAL_PROMPT=0',
    body,
  ].join("\n");
}

// What a machine does as it boots, on the home volume so it outlives a restart: the image's
// CMD only runs it (agent37/Dockerfile names this path), since Agent37 wakes a sleeping
// instance by restarting its container, outside any recipe `resume`. `create` and `resume`
// rewrite it from the repo each time, so a boot change needs no new image. A fork of the base
// has none yet. Its name predates it holding more than serve's launch: a new name would need a
// new image, and every machine on the old one would boot nothing on a wake.
const BOOT_SCRIPT = `${HOME}/.orca-serve.sh`;

// Writes the boot script and runs it, which starts `orca-ide serve` in the background, and
// returns the recipe JSON serve writes. Run again after a resume: a stopped instance keeps its
// files but not its processes.
export async function boot(id: string, pairingAddress: string): Promise<Record<string, unknown>> {
  // A resume's container start runs the script the last phase wrote, beside this run of the
  // new one; whichever takes the lock second keeps its serve, so a run whose script has been
  // replaced since it started steps aside rather than bring the old boot back.
  const stamp = randomBytes(8).toString("hex");
  const script = [
    `# boot ${stamp}`,
    // A boot and a resume may both run this: the lock takes them one at a time, and each
    // stops the serve the other started. Serve itself doesn't inherit the lock.
    "exec 9> /tmp/orca-serve.lock",
    "flock 9",
    `grep -qx '# boot ${stamp}' ${BOOT_SCRIPT} || exit 0`,
    // The bracket keeps the pattern from matching this command's own shell. The old serve
    // must be gone before the new one binds its port: ten seconds, then SIGKILL. The launcher
    // hands off to an `orcad` under orcad-artifacts and exits, and orcad owns the data root:
    // stopping only the launcher leaves the new serve refused by orcad's lock.
    "old='[c]li/index.js serve|[o]rcad-artifacts'",
    'pkill -f "$old" || true',
    'for i in $(seq 50); do pgrep -f "$old" > /dev/null || break; sleep 0.2; done',
    'pkill -9 -f "$old" || true',
    "rm -f /tmp/orca-recipe.json /tmp/orca-serve.log",
    `cd ${PROJECT_ROOT} && nohup setsid orca-ide serve --port ${SERVE_PORT} --project-root ${PROJECT_ROOT}` +
      ` --pairing-address '${pairingAddress}' --recipe-json > /tmp/orca-recipe.json 2> /tmp/orca-serve.log < /dev/null 9>&- &`,
    "echo $! > /tmp/orca-serve.pid",
  ].join("\n");
  const encoded = Buffer.from(`${script}\n`).toString("base64");
  await sh(
    id,
    `echo ${encoded} | base64 -d > ${BOOT_SCRIPT}.new && mv ${BOOT_SCRIPT}.new ${BOOT_SCRIPT} && sh ${BOOT_SCRIPT}`,
    "starting orca-ide serve",
  );
  for (let i = 0; i < 60; i++) {
    await sleep(2000);
    const out = await exec(id, "cat /tmp/orca-recipe.json 2>/dev/null; echo; kill -0 $(cat /tmp/orca-serve.pid) 2>/dev/null && echo ALIVE");
    const [json, alive] = [out.stdout.replace(/\n?ALIVE\n?$/, "").trim(), out.stdout.trimEnd().endsWith("ALIVE")];
    try {
      if (json) return JSON.parse(json) as Record<string, unknown>;
    } catch {}
    if (!alive) {
      const serveLog = await exec(id, "tail -n 40 /tmp/orca-serve.log");
      throw new Error(`orca-ide serve exited before printing its recipe JSON:\n${serveLog.stdout}`);
    }
  }
  throw new Error("orca-ide serve printed no recipe JSON within two minutes");
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

// The recipe result Orca reads: serve's JSON, with the instance id for the lifecycle scripts
// and the template revision it runs, which `listen` reports for the machine.
export function result(serve: Record<string, unknown>, id: string, revision: string): Record<string, unknown> {
  return { ...serve, schemaVersion: 1, userData: { provider: "agent37", resourceId: id, revision } };
}

// The template revision an instance object says it runs: Agent37 reads a missing one as 1.
export function revision(instance: { template_revision?: number | null }): string {
  return String(instance.template_revision ?? 1);
}

// A digest of the files under `paths`, each a file or a directory's whole tree: their paths
// below this directory and their text, CRLF read as LF so a checkout's line endings move nothing.
export function inputs(paths: string[]): string {
  const files = paths.flatMap((path) =>
    statSync(path).isDirectory()
      ? readdirSync(path, { recursive: true, encoding: "utf8" }).map((file) => join(path, file)).filter((file) => statSync(file).isFile())
      : [path],
  );
  const named = files.map((file) => [relative(HERE, file).replaceAll("\\", "/"), file]).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  const hash = createHash("sha256");
  for (const [name, file] of named) hash.update(`${name}\0${readFileSync(file, "utf8").replaceAll("\r\n", "\n")}\0`);
  return `sha256:${hash.digest("hex")}`;
}

// What `prepare` does to the base, given the base's inputs now, the revision the ready base
// runs (undefined without one) and the template's: a base built from other inputs is replaced,
// as its setup is on its disk; one on an older image is moved to the new one in place.
export function baseStep(state: State, setup: string, baseRevision: string | undefined, templateRevision: string): "new" | "update" | "none" {
  if (baseRevision === undefined || state.baseInputs !== setup) return "new";
  return baseRevision === templateRevision ? "none" : "update";
}

// The instance id from the lifecycle payload Orca passes on stdin.
export function payloadInstance(): string {
  const payload = JSON.parse(readFileSync(0, "utf8") || "{}");
  const id = payload?.recipeResult?.userData?.resourceId;
  if (typeof id !== "string" || id === "") throw new Error("no resource id in the lifecycle payload");
  return id;
}
