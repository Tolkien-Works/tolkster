// orca.yaml's `agent37` recipe: `create`, `suspend`, `resume` or `destroy` one workspace's
// Agent37 instance. A workspace is a fork of the stopped base instance (agent37-base.ts), so
// it boots with the image, Claude Code and the checkout, as old as the base: `tolkster host add
// --recipe` fetches it through the host's own credential once it joins, so no git token is
// handed to the instance here.

import {
  api, boot, exec, log, pairingAddress, payloadInstance, readState, result, run, waitRunning,
} from "./agent37.ts";

// One fixed line in the instance's logs (GET /instances/{id}/logs is its PID 1 stdout), so the
// machine's own record says why it stopped or restarted. It runs as the image's user: root is
// refused PID 1's descriptors. A failed write never fails the phase.
async function note(id: string, phase: string): Promise<void> {
  try {
    if (!/^[a-z0-9]+$/.test(id)) throw new Error("unexpected instance id");
    const written = await exec(id, `echo 'tolkster: ${phase} ${id}' > /proc/1/fd/1`);
    if (written.exit_code !== 0) log(`could not write the ${phase} line to ${id}'s logs: ${written.stderr}`);
  } catch (error) {
    log(`could not write the ${phase} line to ${id}'s logs: ${error}`);
  }
}

async function create() {
  const { baseInstanceId, baseReady } = readState();
  if (!baseInstanceId || !baseReady) throw new Error("no finished base instance: run agent37-base.ts image, then base (after deleting a half-built one)");
  const slug = `orca-${process.env.ORCA_RECIPE_ID ?? "agent37"}-${process.env.ORCA_VM_INSTANCE_ID ?? Date.now()}`;
  const name = slug.toLowerCase().replace(/[^a-z0-9-]+/g, "-").slice(-63);
  const fork = await api("POST", `/instances/${baseInstanceId}/fork`, { name });
  const id: string = fork.id;
  log(`forked ${baseInstanceId} as ${id}`);
  try {
    await waitRunning(id);
    await note(id, "create");
    return result(await boot(id, await pairingAddress(id)), id);
  } catch (error) {
    await api("DELETE", `/instances/${id}`).catch((e: unknown) => log(`could not delete ${id}; delete it by hand: ${e}`));
    throw error;
  }
}

async function suspend() {
  const id = payloadInstance();
  await note(id, "suspend");
  await api("POST", `/instances/${id}/stop`);
}

async function resume() {
  const id = payloadInstance();
  await api("POST", `/instances/${id}/start`);
  await waitRunning(id);
  await note(id, "resume");
  return result(await boot(id, await pairingAddress(id)), id);
}

async function destroy() {
  const id = payloadInstance();
  await api("DELETE", `/instances/${id}`).catch((error: Error) => {
    if (!error.message.includes(": 404 ")) throw error;
  });
}

const phases: Record<string, () => Promise<unknown>> = { create, suspend, resume, destroy };
const phase = phases[process.argv[2] ?? ""];
if (!phase) {
  console.error("usage: agent37-recipe.ts create|suspend|resume|destroy");
  process.exit(2);
}
run(phase);
