// orca.yaml's `boat` recipe: `create`, `suspend`, `resume`, `update` or `destroy` one
// workspace's boat.dev sandbox, or `prepare` the saved snapshot they start from. A workspace is
// made from that snapshot (boat-base.ts), so it boots with Orca, the toolchains and the checkout,
// as old as the snapshot: `tolkster host add --recipe` fetches it through the host's own
// credential once it joins, so no git token is handed to the sandbox here.

import { boot, inputs, log, machineName, payloadInstance, prepareLock, result, run, sleep } from "./recipe.ts";
import {
  assertNoCredentials, api, boat, createSandbox, deleteSandbox, mergeState, missing, readState, retire, size, snapshotName, stop,
  waitReady,
} from "./boat.ts";
import { BASE_SETUP, newBase, orcaVersion } from "./boat-base.ts";

// The label a machine carries its snapshot's name in, which is the revision it runs.
const REVISION = "tolkster.revision";

async function create() {
  const type = size(process.argv.slice(3));
  if (!type) {
    console.error("usage: boat-recipe.ts create [--size small|default|large]");
    process.exitCode = 2;
    return;
  }
  const name = machineName(boat);
  const made = (snapshot: string | undefined) => {
    if (!snapshot) throw new Error("no saved snapshot: run boat-recipe.ts prepare");
    const idempotency = process.env.ORCA_VM_INSTANCE_ID && `${name}-${snapshot}`;
    return createSandbox({ from: snapshot, type, name, labels: { [REVISION]: snapshot } }, idempotency);
  };
  const { snapshot: read } = readState();
  // A `prepare` may have replaced and deleted the snapshot this read: the new one is used then.
  let snapshot = read as string;
  const sandbox = await made(read).catch((error: unknown) => {
    const now = readState().snapshot;
    if (!missing(error) || now === read) throw error;
    snapshot = now as string;
    return made(now);
  });
  const id: string = sandbox.id;
  log(`made ${id} from ${snapshot}`);
  try {
    await waitReady(id);
    await assertNoCredentials(id);
    return result(boat, await boot(boat, id), id, snapshot);
  } catch (error) {
    await deleteSandbox(id).catch((e: unknown) => log(`could not delete ${id}; delete it by hand: ${e}`));
    throw error;
  }
}

async function suspend() {
  await stop(payloadInstance());
}

// boat wakes nothing by itself, and a stop keeps the disk but no process, so serve is started
// again here. One still being stopped is resumed once it is; one already running only boots.
async function resume() {
  const id = payloadInstance();
  for (let i = 0; (await api("GET", `/sandboxes/${id}`)).sandbox.state === "archiving"; i++) {
    if (i === 180) throw new Error(`sandbox ${id} is still stopping after 15 minutes`);
    await sleep(5000);
  }
  const { sandbox } = await api("GET", `/sandboxes/${id}`);
  if (!["ready", "idle", "running"].includes(sandbox.state)) await api("POST", `/sandboxes/${id}/resume`, { noEnv: true });
  const ready = await waitReady(id);
  await assertNoCredentials(id);
  return result(boat, await boot(boat, id), id, ready.labels?.[REVISION] ?? "");
}

// boat can't move a sandbox onto another snapshot in place, so a machine stays on its revision
// until the fleet destroys it.
async function update() {
  process.exitCode = 3;
}

async function destroy() {
  await deleteSandbox(payloadInstance());
}

// The saved snapshot brought up to the repo's inputs, as `listen` runs it after each pull: a new
// base built and saved under the name its inputs give when no ready snapshot has it, then the
// old one deleted. `create` uses the old one until the new one is ready. Exit 3 when nothing
// changed. Either way it prints the snapshot's name, the revision `listen` reports.
async function prepare() {
  // Read before any wait: a pull meanwhile must not be recorded as built by this run's code.
  const version = orcaVersion();
  const wanted = snapshotName(inputs([BASE_SETUP]), version);
  await prepareLock("boat");
  const building = readState().building;
  if (building) {
    await deleteSandbox(building).then(
      () => mergeState({ building: undefined }),
      (e: unknown) => log(`could not delete ${building}; the next prepare tries again: ${e}`),
    );
  }
  await deleteRetired();
  // A run cut off as it saved leaves the save going, or done under the name it would record.
  let saved = await settled(wanted);
  if (saved?.status === "ready" && readState().snapshot === wanted) {
    process.exitCode = 3;
    return { revision: wanted };
  }
  if (saved?.status !== "ready") {
    const id = await newBase(readState(), version).catch(async (error: unknown) => {
      const left = readState().building;
      if (left) await deleteSandbox(left).then(() => mergeState({ building: undefined }), () => {});
      throw error;
    });
    await api("POST", "/saved-snapshots", { sandboxId: id, name: wanted });
    saved = await settled(wanted);
    if (saved?.status !== "ready") throw new Error(`saving ${wanted} from ${id} failed: ${saved?.error ?? "no snapshot"}`);
    await deleteSandbox(id).then(
      () => mergeState({ building: undefined }),
      (e: unknown) => log(`could not delete ${id}; the next prepare tries again: ${e}`),
    );
  }
  mergeState({ snapshot: wanted, retired: retire(readState(), wanted) });
  await deleteRetired();
  return { snapshot: wanted, revision: wanted };
}

// The saved snapshot `name` once no save of it is running, or undefined without one.
async function settled(name: string): Promise<any> {
  for (let i = 0; i < 360; i++) {
    const saved = await api("GET", `/saved-snapshots/${name}`).then(
      (found) => found.snapshot,
      (error: unknown) => {
        if (!missing(error)) throw error;
      },
    );
    if (saved?.status !== "saving") return saved;
    await sleep(5000);
  }
  throw new Error(`saved snapshot ${name} is still saving after 30 minutes`);
}

// Deletes the saved snapshots `retired` lists, each taken out once it is gone.
async function deleteRetired() {
  for (const name of readState().retired ?? []) {
    if (name === readState().snapshot) continue;
    try {
      await api("DELETE", `/saved-snapshots/${name}`).catch((error: unknown) => {
        if (!missing(error)) throw error;
      });
      mergeState({ retired: (readState().retired ?? []).filter((other) => other !== name) });
    } catch (error) {
      log(`could not delete saved snapshot ${name}; the next prepare tries again: ${error}`);
    }
  }
}

const phases: Record<string, () => Promise<unknown>> = { create, suspend, resume, update, destroy, prepare };
const phase = phases[process.argv[2] ?? ""];
if (!phase) {
  console.error("usage: boat-recipe.ts create|suspend|resume|update|destroy|prepare");
  process.exit(2);
}
run(phase);
