// A new worktree's setup (orca.yaml's `scripts.setup`), one script for Windows and POSIX
// hosts: dependencies, the marker AGENTS.md checks for, then the cargo warm-up. Stops at the
// first failing step and exits with its status, so the marker means `npm ci` finished.
// agent37-base.ts runs it too, so the base's warm build is the one a worktree is seeded from.

import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = fileURLToPath(new URL("../", import.meta.url));

// `npm` is a .cmd shim on Windows, which only a shell starts.
function run(command: string, args: string[]): void {
  const { status, error } = spawnSync(command, args, { cwd: ROOT, stdio: "inherit", shell: process.platform === "win32" });
  if (error) console.error(`${command} didn't start: ${error.message}`);
  if (status !== 0) process.exit(status ?? 1);
}

// A cold build costs minutes of CPU that other workers on the host compete for; a copy of the
// primary checkout's `cli/target/debug` leaves only the `tolkster` crate to build. It must hold the
// same dependencies and profile, which `Cargo.lock` and `Cargo.toml` decide: otherwise cargo
// would rebuild everything over it, so build cold. Only `debug` comes, which `listen`'s own
// release rebuild never writes, and not the primary's `tolkster` binary. The timestamps carry
// over because cargo compares them, so the crate's own fingerprints go: a primary built after
// this worktree's files would otherwise look fresh. The copy lands whole or not at all.
function seedTarget(): void {
  const target = `${ROOT}cli/target`;
  const partial = `${target}.seeding`;
  rmSync(partial, { recursive: true, force: true });
  if (existsSync(target)) return;
  const common = spawnSync("git", ["rev-parse", "--git-common-dir"], { cwd: ROOT, encoding: "utf8" });
  if (common.status !== 0) return;
  const primary = dirname(resolve(ROOT, common.stdout.trim()));
  if (resolve(primary) === resolve(ROOT)) return;
  const from = `${primary}/cli/target/debug`;
  const same = (file: string) => {
    try {
      return readFileSync(`${primary}/cli/${file}`).equals(readFileSync(`${ROOT}cli/${file}`));
    } catch {
      return false;
    }
  };
  if (!existsSync(from) || !same("Cargo.lock") || !same("Cargo.toml")) return;
  try {
    cpSync(from, `${partial}/debug`, { recursive: true, preserveTimestamps: true, verbatimSymlinks: true });
    for (const name of readdirSync(`${partial}/debug/.fingerprint`)) {
      if (name.startsWith("tolkster-")) rmSync(`${partial}/debug/.fingerprint/${name}`, { recursive: true, force: true });
    }
    for (const bin of ["tolkster", "tolkster.exe"]) rmSync(`${partial}/debug/${bin}`, { force: true });
    renameSync(partial, target);
  } catch (e) {
    console.error(`seeding cli/target failed, building cold: ${(e as Error).message}`);
    try {
      rmSync(partial, { recursive: true, force: true });
    } catch {}
  }
}

run("npm", ["ci", "--no-audit", "--no-fund"]);
mkdirSync(`${ROOT}node_modules`, { recursive: true });
writeFileSync(`${ROOT}node_modules/.tolkster-setup`, "done\n");
seedTarget();
run("cargo", ["test", "--locked", "--manifest-path", "cli/Cargo.toml", "--no-run"]);
run("cargo", ["clippy", "--locked", "--manifest-path", "cli/Cargo.toml", "--all-targets"]);
