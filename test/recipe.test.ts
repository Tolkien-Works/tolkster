// The steps every recipe provider shares (scripts/orca-vm/recipe.ts).

import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, readdirSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { type Exec, type Provider, boot, prepareLock, result, stateFile, withGitToken } from "../scripts/orca-vm/recipe.ts";

const ok = (stdout = ""): Exec => ({ exit_code: 0, stdout, stderr: "" });

test("a provider's state file keeps what an earlier merge wrote, and leaves no partial file", () => {
  const dir = mkdtempSync(join(tmpdir(), "recipe-state-"));
  const state = stateFile<{ a?: string; b?: string }>("p", dir);
  assert.deepEqual(state.read(), {});
  state.merge({ a: "1" });
  assert.deepEqual(state.merge({ b: "2" }), { a: "1", b: "2" });
  assert.deepEqual(stateFile("p", dir).read(), { a: "1", b: "2" });
  assert.deepEqual(readdirSync(dir), ["p-state.json"]);
});

test("the result names its provider, beside serve's JSON", () => {
  const provider = { name: "p" } as Provider;
  assert.deepEqual(result(provider, { pairing: "x" }, "m1", "4"), {
    pairing: "x", schemaVersion: 1, userData: { provider: "p", resourceId: "m1", revision: "4" },
  });
});

test("the git token never appears in the script in plain text", () => {
  const script = withGitToken("ghs_secret", "git clone x");
  assert.ok(!script.includes("ghs_secret"));
  assert.ok(script.endsWith("\ngit clone x"));
});

test("a prepare lock whose holder has gone is taken over", async () => {
  const dir = mkdtempSync(join(tmpdir(), "recipe-lock-"));
  const lock = join(dir, "p-prepare.lock");
  writeFileSync(lock, "0");
  utimesSync(lock, new Date(0), new Date(0));
  await prepareLock("p", dir);
  assert.equal(readFileSync(lock, "utf8"), String(process.pid));
});

test("a prepare lock it can't write fails, rather than waiting on it", async () => {
  await assert.rejects(prepareLock("p", join(tmpdir(), "recipe-lock-missing", "nowhere")), { code: "ENOENT" });
});

test("boot starts serve at the provider's pairing address and home, and returns its JSON", async () => {
  const commands: string[] = [];
  const provider: Provider = {
    name: "p",
    home: "/home/u",
    pairingAddress: async (id) => `wss://${id}.example`,
    exec: async (_id, command) => {
      commands.push(command);
      return command.startsWith("cat /tmp/orca-recipe.json") ? ok('{"pairing":"x"}\nALIVE\n') : ok();
    },
  };
  assert.deepEqual(await boot(provider, "m1"), { pairing: "x" });
  const script = Buffer.from(/echo (\S+) \| base64 -d/.exec(commands[0])![1], "base64").toString();
  assert.match(commands[0], /> \/home\/u\/\.orca-serve\.sh\.new && mv /);
  assert.match(script, /cd \/home\/u\/tolkster && .*--pairing-address 'wss:\/\/m1\.example'/);
});
