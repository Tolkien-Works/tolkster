// What any recipe provider's scripts share (orca.yaml's recipes): the one JSON result, a state
// file and a `prepare` lock per provider, the input digest, the boot script that starts
// `orca-ide serve`, long commands, the git token and the result Orca reads. A provider brings
// only its API, through `Provider`. Every script keeps stdout for its one JSON result; progress
// and errors go to stderr.

import { execFileSync } from "node:child_process";
import { createHash, randomBytes } from "node:crypto";
import { existsSync, readFileSync, readdirSync, renameSync, rmSync, statSync, unlinkSync, utimesSync, writeFileSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = fileURLToPath(new URL(".", import.meta.url));

// The port `orca-ide serve` listens on in a workspace, published through a public port.
export const SERVE_PORT = 6768;

export type Exec = { exit_code: number; stdout: string; stderr: string };

// What the shared steps need of a provider: its name in `userData.provider`, the home of the
// user its machines run as, a command run there by `sh -c`, and the address Orca pairs with.
export type Provider = {
  name: string;
  home: string;
  exec(id: string, command: string): Promise<Exec>;
  pairingAddress(id: string): Promise<string>;
};

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

export const sleep = (ms: number) => new Promise((done) => setTimeout(done, ms));

// `<provider>-state.json` beside these scripts: ids, names and digests only, never a credential.
// Renamed into place, so a `create` reading it as `prepare` writes never reads half a file.
export function stateFile<S extends object>(provider: string, dir = HERE) {
  const path = join(dir, `${provider}-state.json`);
  const read = (): S => (existsSync(path) ? (JSON.parse(readFileSync(path, "utf8")) as S) : ({} as S));
  const merge = (change: S): S => {
    const state = { ...read(), ...change };
    writeFileSync(`${path}.${process.pid}`, `${JSON.stringify(state, null, 2)}\n`);
    renameSync(`${path}.${process.pid}`, path);
    return state;
  };
  return { read, merge };
}

// The project's checkout on a provider's machines.
export const projectRoot = (provider: Provider) => `${provider.home}/tolkster`;

// Like exec, but fails on a nonzero exit, with the command's stderr.
export async function sh(provider: Provider, id: string, command: string, what: string): Promise<string> {
  const result = await provider.exec(id, command);
  if (result.exit_code !== 0) throw new Error(`${what} failed (exit ${result.exit_code}): ${result.stderr || result.stdout}`);
  return result.stdout;
}

// A long bash script, run in the background and polled, so no single exec outlives fetch. The
// script file is private and deletes itself as it starts: it may hold a token.
export async function shLong(provider: Provider, id: string, script: string, what: string, minutes: number): Promise<void> {
  const tag = `/tmp/orca-${Date.now()}`;
  const encoded = Buffer.from(`rm -f -- "$0"\n${script}`).toString("base64");
  await sh(
    provider,
    id,
    `umask 077 && echo ${encoded} | base64 -d > ${tag}.sh && nohup bash -c 'bash ${tag}.sh > ${tag}.log 2>&1; echo $? > ${tag}.exit' >/dev/null 2>&1 &`,
    `starting ${what}`,
  );
  for (let waited = 0; waited < minutes * 60_000; waited += 10_000) {
    await sleep(10_000);
    const done = await provider.exec(id, `cat ${tag}.exit 2>/dev/null`);
    if (done.stdout.trim() === "") continue;
    const tail = await provider.exec(id, `tail -n 40 ${tag}.log`);
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

// Writes the boot script and runs it, which starts `orca-ide serve` in the background, and
// returns the recipe JSON serve writes. Run again after a resume: a stopped machine keeps its
// files but not its processes.
//
// The script is what a machine does as it boots, on the home volume so it outlives a restart:
// an image's CMD only runs it (agent37/Dockerfile names this path), since a provider may wake a
// sleeping machine by restarting its container, outside any recipe `resume`. `create` and
// `resume` rewrite it from the repo each time, so a boot change needs no new image. A fork of a
// base has none yet. Its name predates it holding more than serve's launch: a new name would
// need a new image, and every machine on the old one would boot nothing on a wake.
export async function boot(provider: Provider, id: string): Promise<Record<string, unknown>> {
  const pairingAddress = await provider.pairingAddress(id);
  const bootScript = `${provider.home}/.orca-serve.sh`;
  const root = projectRoot(provider);
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
    `grep -qx '# boot ${stamp}' ${bootScript} || exit 0`,
    // The bracket keeps the pattern from matching this command's own shell. The old serve
    // must be gone before the new one binds its port: ten seconds, then SIGKILL. The launcher
    // hands off to an `orcad` under orcad-artifacts and exits, and orcad owns the data root:
    // stopping only the launcher leaves the new serve refused by orcad's lock.
    "old='[c]li/index.js serve|[o]rcad-artifacts'",
    'pkill -f "$old" || true',
    'for i in $(seq 50); do pgrep -f "$old" > /dev/null || break; sleep 0.2; done',
    'pkill -9 -f "$old" || true',
    "rm -f /tmp/orca-recipe.json /tmp/orca-serve.log",
    `cd ${root} && nohup setsid orca-ide serve --port ${SERVE_PORT} --project-root ${root}` +
      ` --pairing-address '${pairingAddress}' --recipe-json > /tmp/orca-recipe.json 2> /tmp/orca-serve.log < /dev/null 9>&- &`,
    "echo $! > /tmp/orca-serve.pid",
  ].join("\n");
  const encoded = Buffer.from(`${script}\n`).toString("base64");
  await sh(
    provider,
    id,
    `echo ${encoded} | base64 -d > ${bootScript}.new && mv ${bootScript}.new ${bootScript} && sh ${bootScript}`,
    "starting orca-ide serve",
  );
  for (let i = 0; i < 60; i++) {
    await sleep(2000);
    const out = await provider.exec(id, "cat /tmp/orca-recipe.json 2>/dev/null; echo; kill -0 $(cat /tmp/orca-serve.pid) 2>/dev/null && echo ALIVE");
    const [json, alive] = [out.stdout.replace(/\n?ALIVE\n?$/, "").trim(), out.stdout.trimEnd().endsWith("ALIVE")];
    try {
      if (json) return JSON.parse(json) as Record<string, unknown>;
    } catch {}
    if (!alive) {
      const serveLog = await provider.exec(id, "tail -n 40 /tmp/orca-serve.log");
      throw new Error(`orca-ide serve exited before printing its recipe JSON:\n${serveLog.stdout}`);
    }
  }
  throw new Error("orca-ide serve printed no recipe JSON within two minutes");
}

// The recipe result Orca reads: serve's JSON, with the machine's id for the lifecycle scripts
// and the image revision it runs, which `listen` reports for the machine.
export function result(provider: Provider, serve: Record<string, unknown>, id: string, revision: string): Record<string, unknown> {
  return { ...serve, schemaVersion: 1, userData: { provider: provider.name, resourceId: id, revision } };
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

// The machine id from the lifecycle payload Orca passes on stdin.
export function payloadInstance(): string {
  const payload = JSON.parse(readFileSync(0, "utf8") || "{}");
  const id = payload?.recipeResult?.userData?.resourceId;
  if (typeof id !== "string" || id === "") throw new Error("no resource id in the lifecycle payload");
  return id;
}

// One `prepare` at a time per provider and checkout, waited for: a `listen` that restarts leaves
// its run going, and the next start's runs once it ends, so the pull it was asked for is
// prepared. The holder touches `<provider>-prepare.lock` each minute; one whose process has
// gone, or untouched for five minutes, as a reused process ID could pass for alive, is taken over.
export async function prepareLock(provider: string, dir = HERE): Promise<void> {
  const lock = join(dir, `${provider}-prepare.lock`);
  for (let said = false; ; ) {
    try {
      writeFileSync(lock, String(process.pid), { flag: "wx" });
      break;
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
      if (!held(lock)) {
        rmSync(lock, { force: true });
        continue;
      }
      if (!said) log("waiting for another prepare to end");
      said = true;
      await sleep(10_000);
    }
  }
  setInterval(() => {
    try {
      utimesSync(lock, new Date(), new Date());
    } catch {}
  }, 60_000).unref();
  process.on("exit", () => {
    try {
      if (readFileSync(lock, "utf8") === String(process.pid)) unlinkSync(lock);
    } catch {}
  });
}

// An empty lock is one being written.
function held(lock: string): boolean {
  try {
    const pid = Number(readFileSync(lock, "utf8"));
    return Date.now() - statSync(lock).mtimeMs < 5 * 60_000 && (pid === 0 || alive(pid));
  } catch {
    return false;
  }
}

function alive(pid: number): boolean {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
}
