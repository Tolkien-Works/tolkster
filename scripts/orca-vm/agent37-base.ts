// The image and the base the `agent37` recipe forks from, built by its `prepare`
// (agent37-recipe.ts) when their inputs change:
//   buildImage  builds agent37/Dockerfile in Agent37's cloud as the `tolkster-orca` template
//   newBase     creates a base instance from it, installs Claude Code, clones Tolkster, warms
//               npm and cargo, then stops it so forks copy it at rest
// This file is the base's inputs: any change to it builds a new base, as its setup is on the
// base's disk. Nothing on the base is signed in: a fork gets its credentials after it starts
// (docs/hosts.md). `orca-ide serve` never runs on the base, so no fork inherits another's Orca
// identity.

import { execFileSync, execSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  HOME, PROJECT_ROOT, type State, api, ghToken, log, mergeState, shLong, waitRunning, withGitToken,
} from "./agent37.ts";

const TEMPLATE = "tolkster-orca";
export const IMAGE_DIR = fileURLToPath(new URL("agent37/", import.meta.url));
export const BASE_SETUP = fileURLToPath(import.meta.url);
const RESOURCES = { cpu: 4, memory: 8, disk: 20 };
// Asleep, an instance bills only its disk; a fork inherits both settings.
const IDLE_TIMEOUT = 300;

// Publishes a new revision of the template, which no instance runs until it is updated.
export async function buildImage(): Promise<State> {
  const dockerfile = readFileSync(`${IMAGE_DIR}Dockerfile`, "utf8");
  const orcaVersion = /ARG ORCA_VERSION=(\S+)/.exec(dockerfile)?.[1];
  if (!orcaVersion) throw new Error("the Dockerfile pins no ORCA_VERSION");
  // The CLI packs the context with `tar -f <temp path>`; on Windows that must be the system
  // tar, since Git's reads a drive letter (`D:`) as a remote host.
  const path = process.platform === "win32" ? `${process.env.SystemRoot}\\System32;${process.env.PATH}` : process.env.PATH;
  execSync(`npx --yes agent37@0.4.1 templates build . --name ${TEMPLATE}`, {
    cwd: IMAGE_DIR,
    env: { ...process.env, PATH: path },
    stdio: ["ignore", process.stderr, process.stderr],
  });
  return mergeState({ template: TEMPLATE, orcaVersion });
}

// A new base, stopped and ready to fork: its id. It is `buildingBase` from its creation, so a
// run cut off partway leaves it to delete.
export async function newBase(state: State): Promise<string> {
  if (!state.template) throw new Error("no template: build the image first");
  const repoUrl = state.repoUrl ?? execFileSync("git", ["remote", "get-url", "origin"], { encoding: "utf8" }).trim();
  const repoRef = state.repoRef ?? "main";
  const instance = await api("POST", "/instances", {
    template: state.template, name: "tolkster-orca-base", resources: RESOURCES, auto_sleep: true, idle_timeout_seconds: IDLE_TIMEOUT,
  });
  const id: string = instance.id;
  mergeState({ buildingBase: id, repoUrl, repoRef });
  log(`created base ${id}; cloning and warming the build (about 15 minutes)`);
  await waitRunning(id);
  // Claude Code's terms keep it out of the public image, so its installer puts it in the home
  // volume, which forks copy, unless Agent37's platform ships one (its managed settings then
  // refuse the installer). Auto mode is a setting, not a login. A worker's Claude starts
  // unattended, so first-run setup and auto mode's outside-read notice (its brief is outside
  // the worktree) are marked seen, or each waits on a keypress.
  const claude = [
    "command -v claude >/dev/null || curl -fsSL https://claude.ai/install.sh | bash",
    `PATH=${HOME}/.local/bin:$PATH claude --version`,
    `mkdir -p ${HOME}/.claude`,
    `node -e 'const fs = require("fs"), f = process.argv[1], j = fs.existsSync(f) ? JSON.parse(fs.readFileSync(f, "utf8")) : {}; j.hasCompletedOnboarding = true; j.hasSeenAutoModeOutsideReadPrompt = true; fs.writeFileSync(f, JSON.stringify(j, null, 2))' ${HOME}/.claude.json`,
    `test -e ${HOME}/.claude/settings.json || echo '{"permissions":{"defaultMode":"auto"}}' > ${HOME}/.claude/settings.json`,
  ].join("\n");
  const clone = `git clone --quiet --branch ${repoRef} ${repoUrl} ${PROJECT_ROOT}`;
  const warm = [
    `cd ${PROJECT_ROOT}`,
    // A worktree's own setup, so the build it leaves in cli/target is the one worktrees seed from.
    "node scripts/setup.ts",
    // A fork would share an Orca runtime profile, and with it a pairing identity.
    `test ! -e ${HOME}/.config/orca || { echo "an Orca profile exists on the base" >&2; exit 1; }`,
  ].join("\n");
  // Two runs: the token is in the first one's script, which bash holds open as it runs, so
  // none of the third-party code the second runs (installers, npm and build scripts) can read it.
  await shLong(id, withGitToken(ghToken(), clone), "cloning", 10);
  await shLong(id, `set -euo pipefail\n${claude}\n${warm}`, "warming the build", 45);
  await api("POST", `/instances/${id}/stop`);
  log(`base ${id} is stopped and ready to fork`);
  return id;
}
