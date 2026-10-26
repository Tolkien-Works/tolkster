// orca.yaml's `agent37` recipe: `create`, `suspend`, `resume` or `destroy` one workspace's
// Agent37 instance, or `prepare` the image and the base. A workspace is a fork of the stopped
// base instance (agent37-base.ts), so it boots with the image, Claude Code and the checkout, as
// old as the base: `tolkster host add --recipe` fetches it through the host's own credential
// once it joins, so no git token is handed to the instance here.

import { readFileSync, unlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  api, baseStep, boot, exec, inputs, log, mergeState, pairingAddress, payloadInstance, readState, result, revision, run, sleep, waitRunning,
} from "./agent37.ts";
import { BASE_SETUP, IMAGE_DIR, buildImage, newBase } from "./agent37-base.ts";

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
  const { baseInstanceId } = readState();
  if (!baseInstanceId) throw new Error("no base instance: run agent37-recipe.ts prepare");
  const slug = `orca-${process.env.ORCA_RECIPE_ID ?? "agent37"}-${process.env.ORCA_VM_INSTANCE_ID ?? Date.now()}`;
  const name = slug.toLowerCase().replace(/[^a-z0-9-]+/g, "-").slice(-63);
  const fork = await api("POST", `/instances/${baseInstanceId}/fork`, { name });
  const id: string = fork.id;
  log(`forked ${baseInstanceId} as ${id}`);
  try {
    const instance = await waitRunning(id);
    await note(id, "create");
    return result(await boot(id, await pairingAddress(id)), id, revision(instance));
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
  const instance = await waitRunning(id);
  await note(id, "resume");
  return result(await boot(id, await pairingAddress(id)), id, revision(instance));
}

async function destroy() {
  const id = payloadInstance();
  await api("DELETE", `/instances/${id}`).catch((error: Error) => {
    if (!error.message.includes(": 404 ")) throw error;
  });
}

// The image and the base brought up to the repo's inputs, as `listen` runs it after each pull:
// a template rebuilt when agent37/ changed, then the base moved to its revision in place, as
// Agent37's update keeps the home volume and leaves a stopped instance stopped, or a new base
// built alongside when this file's setup changed. `create` forks the old base until the new
// one is ready. Exit 3 when nothing changed.
async function prepare() {
  if (!locked()) {
    log("another prepare is running");
    process.exitCode = 3;
    return;
  }
  let state = readState();
  const left = [...(state.retired ?? []), ...(state.buildingBase ? [state.buildingBase] : [])];
  state = mergeState({ buildingBase: undefined, retired: left });
  await deleteRetired();
  const done: Record<string, string> = {};
  const image = inputs([IMAGE_DIR]);
  if (state.imageInputs !== image) {
    await buildImage();
    state = mergeState({ imageInputs: image });
    done.image = image;
  }
  const template = revision({ template_revision: (await api("GET", `/templates/${state.template}`)).revision });
  const setup = inputs([BASE_SETUP]);
  const old = state.baseInstanceId;
  const base = old ? await api("GET", `/instances/${old}`).catch((error: Error) => {
    if (!error.message.includes(": 404 ")) throw error;
  }) : undefined;
  switch (baseStep(state, setup, base && revision(base), template)) {
    case "new": {
      const id = await newBase(state);
      mergeState({ baseInstanceId: id, baseInputs: setup, buildingBase: undefined, retired: [...(readState().retired ?? []), ...(old ? [old] : [])] });
      done.base = id;
      // A `create` that read the old base just before may still be forking it.
      await sleep(60_000);
      await deleteRetired();
      break;
    }
    case "update": {
      const updated = await api("POST", `/instances/${old}/update`);
      if (revision(updated) !== template) throw new Error(`base ${old} updated to revision ${revision(updated)}, not ${template}`);
      // Forks copy the base at rest, and one woken from a sleep comes back running.
      if (updated.status !== "stopped") await api("POST", `/instances/${old}/stop`);
      done.base = `${old}:${template}`;
      break;
    }
    case "none":
      break;
  }
  if (Object.keys(done).length === 0) {
    process.exitCode = 3;
    return;
  }
  return { ...done, revision: template };
}

// Deletes the instances `retired` lists, each taken out once it is gone.
async function deleteRetired() {
  for (const id of readState().retired ?? []) {
    try {
      await api("DELETE", `/instances/${id}`).catch((error: Error) => {
        if (!error.message.includes(": 404 ")) throw error;
      });
      mergeState({ retired: (readState().retired ?? []).filter((other) => other !== id) });
    } catch (error) {
      log(`could not delete ${id}; the next prepare tries again: ${error}`);
    }
  }
}

// One `prepare` at a time: a `listen` that restarts leaves its run going, and the next start's
// finds it here. A lock whose process has gone is taken over.
function locked(): boolean {
  const path = join(tmpdir(), "tolkster-agent37-prepare.lock");
  try {
    writeFileSync(path, String(process.pid), { flag: "wx" });
  } catch {
    const holder = Number(readFileSync(path, "utf8"));
    if (alive(holder)) return false;
    writeFileSync(path, String(process.pid));
  }
  process.on("exit", () => {
    try {
      if (readFileSync(path, "utf8") === String(process.pid)) unlinkSync(path);
    } catch {}
  });
  return true;
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

const phases: Record<string, () => Promise<unknown>> = { create, suspend, resume, destroy, prepare };
const phase = phases[process.argv[2] ?? ""];
if (!phase) {
  console.error("usage: agent37-recipe.ts create|suspend|resume|destroy|prepare");
  process.exit(2);
}
run(phase);
