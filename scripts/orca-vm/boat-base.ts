// The base the `boat` recipe's machines start from, built by its `prepare` (boat-recipe.ts) when
// its inputs change. boat runs no image of ours, so `newBase` makes a sandbox on boat's own
// image and lays on it what the worker base and the repo layer hold for Agent37 (docs/hosts.md):
// git new enough to run hooks from config, Orca's Linux app at the Orca version agent37/Dockerfile
// pins, Claude Code's first-run screens marked seen, the checkout and a warm build. Node, Rust
// with clippy, a C compiler and Claude Code come with boat's image. `prepare` saves the result as
// a saved snapshot, which never expires, unlike a stopped sandbox's data.
// This file and that pin are the base's inputs: a change to either builds a new base. Nothing on
// it is signed in, and `orca-ide serve` never runs on it, so no machine inherits another's Orca
// identity.

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { ghToken, log, projectRoot, shLong, withGitToken } from "./recipe.ts";
import { HOME, type State, assertNoCredentials, boat, createSandbox, mergeState, waitReady } from "./boat.ts";

export const BASE_SETUP = fileURLToPath(import.meta.url);

// The one Orca pin, so a raise there brings both recipes' machines to it.
export function orcaVersion(): string {
  const dockerfile = readFileSync(fileURLToPath(new URL("agent37/Dockerfile", import.meta.url)), "utf8");
  const version = /ARG ORCA_VERSION=(\S+)/.exec(dockerfile)?.[1];
  if (!version) throw new Error("agent37/Dockerfile pins no ORCA_VERSION");
  return version;
}

// The Ubuntu Git Maintainers' PPA signing key, checked against the fingerprint Launchpad publishes.
const GIT_PPA_KEY = "F911AB184317630C59970973E363C90F8F1B6217";

// A sandbox set up as the base, still running: its id. It is `building` from its creation, so a
// run cut off partway leaves it to delete.
export async function newBase(state: State, version: string): Promise<string> {
  const repoUrl = state.repoUrl ?? execFileSync("git", ["remote", "get-url", "origin"], { encoding: "utf8" }).trim();
  const repoRef = state.repoRef ?? "main";
  const sandbox = await createSandbox({ type: "default", name: "tolkster-orca-base" });
  const id: string = sandbox.id;
  mergeState({ building: id, repoUrl, repoRef });
  log(`created base ${id}; installing Orca, cloning and warming the build (about 15 minutes)`);
  await waitReady(id);
  await assertNoCredentials(id);
  const root = projectRoot(boat);
  const system = [
    "set -euo pipefail",
    "sudo install -d -m 755 /etc/apt/keyrings",
    `curl -fsSL 'https://keyserver.ubuntu.com/pks/lookup?op=get&search=0x${GIT_PPA_KEY}' | gpg --dearmor | sudo tee /etc/apt/keyrings/git-core-ppa.gpg > /dev/null`,
    `gpg --show-keys --with-colons /etc/apt/keyrings/git-core-ppa.gpg | grep -qx 'fpr:::::::::${GIT_PPA_KEY}:'`,
    `printf 'Types: deb\\nURIs: https://ppa.launchpadcontent.net/git-core/ppa/ubuntu\\nSuites: %s\\nComponents: main\\nSigned-By: /etc/apt/keyrings/git-core-ppa.gpg\\n' "$(lsb_release -cs)" | sudo tee /etc/apt/sources.list.d/git-core-ppa.sources > /dev/null`,
    `curl -fsSL -o /tmp/orca.deb https://github.com/stablyai/orca/releases/download/v${version}/orca-ide_${version}_amd64.deb`,
    "sudo apt-get update -q",
    "sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -q --no-install-recommends git libgbm1 libasound2t64 /tmp/orca.deb",
    "rm -f /tmp/orca.deb",
    "git --version",
    "orca-ide --version",
  ].join("\n");
  // A worker's Claude starts unattended, so first-run setup and auto mode's outside-read notice
  // (its brief is outside the worktree) are marked seen, or each waits on a keypress.
  const claude = [
    "claude --version",
    `mkdir -p ${HOME}/.claude`,
    `node -e 'const fs = require("fs"), f = process.argv[1], j = fs.existsSync(f) ? JSON.parse(fs.readFileSync(f, "utf8")) : {}; j.hasCompletedOnboarding = true; j.hasSeenAutoModeOutsideReadPrompt = true; fs.writeFileSync(f, JSON.stringify(j, null, 2))' ${HOME}/.claude.json`,
    `test -e ${HOME}/.claude/settings.json || echo '{"permissions":{"defaultMode":"auto"}}' > ${HOME}/.claude/settings.json`,
  ].join("\n");
  const clone = `git clone --quiet --branch ${repoRef} ${repoUrl} ${root}`;
  const warm = [
    `cd ${root}`,
    // A worktree's own setup, so the build it leaves in cli/target is the one worktrees seed from.
    "node scripts/setup.ts",
    // A machine would share an Orca runtime profile, and with it a pairing identity.
    `test ! -e ${HOME}/.config/orca || { echo "an Orca profile exists on the base" >&2; exit 1; }`,
  ].join("\n");
  await shLong(boat, id, system, "installing git and Orca", 15);
  // Two runs: the token is in the first one's script, which bash holds open as it runs, so
  // none of the third-party code the second runs (npm and build scripts) can read it.
  await shLong(boat, id, withGitToken(ghToken(), clone), "cloning", 10);
  await shLong(boat, id, `set -euo pipefail\n${claude}\n${warm}`, "warming the build", 45);
  await assertNoCredentials(id);
  return id;
}
