// orca.yaml's `agent37` recipe: `create`, `suspend`, `resume`, `update` or `destroy` one
// workspace's Agent37 instance, or `prepare` the image and the base. A workspace is a fork of the stopped
// base instance (agent37-base.ts), so it boots with the image, Claude Code and the checkout, as
// old as the base: `tolkster host add --recipe` fetches it through the host's own credential
// once it joins, so no git token is handed to the instance here.

import { readFileSync, rmSync, statSync, unlinkSync, utimesSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
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
  const slug = `orca-${process.env.ORCA_RECIPE_ID ?? "agent37"}-${process.env.ORCA_VM_INSTANCE_ID ?? Date.now()}`;
  const name = slug.toLowerCase().replace(/[^a-z0-9-]+/g, "-").slice(-63);
  const forked = (base: string | undefined) => {
    if (!base) throw new Error("no base instance: run agent37-recipe.ts prepare");
    return api("POST", `/instances/${base}/fork`, { name });
  };
  const { baseInstanceId: read } = readState();
  // A `prepare` may have replaced and deleted the base this read: the new one is forked then.
  const fork = await forked(read).catch((error: Error) => {
    const now = readState().baseInstanceId;
    if (!error.message.includes(": 404 ") || now === read) throw error;
    return forked(now);
  });
  const id: string = fork.id;
  log(`forked the base as ${id}`);
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

// The stopped instance moved onto its template's current revision in place, as `prepare` moves
// the base: Agent37 keeps its home volume and leaves it stopped, so its next `resume` boots the
// new image. Exit 3 for one not stopped, whose update would restart it.
async function update() {
  const id = payloadInstance();
  if ((await api("GET", `/instances/${id}`)).status !== "stopped") {
    process.exitCode = 3;
    return;
  }
  const updated = await api("POST", `/instances/${id}/update`);
  return { schemaVersion: 1, userData: { provider: "agent37", resourceId: id, revision: revision(updated) } };
}

async function destroy() {
  const id = payloadInstance();
  await api("DELETE", `/instances/${id}`).catch((error: Error) => {
    if (!error.message.includes(": 404 ")) throw error;
  });
}

// The image and the base brought up to the repo's inputs, as `listen` runs it after each pull:
// a template rebuilt when agent37/ changed or another checkout published the newest revision,
// then the base moved to it in place, as Agent37's update keeps the home volume and leaves a
// stopped instance stopped, or a new base built alongside when this file's setup changed.
// `create` forks the old base until the new one is ready. Exit 3 when nothing changed. Either
// way it prints the template's revision, which `listen` brings its machines to.
async function prepare() {
  // Read before any wait: a pull meanwhile must not be recorded as built by this run's code.
  const [image, setup] = [inputs([IMAGE_DIR]), inputs([BASE_SETUP])];
  await lock();
  let state = readState();
  const left = [...(state.retired ?? []), ...(state.buildingBase ? [state.buildingBase] : [])];
  state = mergeState({ buildingBase: undefined, retired: left });
  await deleteRetired();
  const done: Record<string, string> = {};
  const newest = async () => state.template && revision({ template_revision: (await api("GET", `/templates/${state.template}`)).revision });
  let template = await newest();
  if (state.imageInputs !== image || template !== state.imageRevision) {
    state = await buildImage();
    template = await newest();
    state = mergeState({ imageInputs: image, imageRevision: template });
    done.image = image;
  }
  const old = state.baseInstanceId;
  const base = old ? await api("GET", `/instances/${old}`).catch((error: Error) => {
    if (!error.message.includes(": 404 ")) throw error;
  }) : undefined;
  switch (baseStep(state, setup, base && revision(base), template as string)) {
    case "new": {
      const id = await newBase(state);
      mergeState({ baseInstanceId: id, baseInputs: setup, buildingBase: undefined, retired: [...(readState().retired ?? []), ...(old ? [old] : [])] });
      done.base = id;
      // A fork of the old base begun before the swap gets a minute to copy it; one begun
      // later finds it gone and forks the new one.
      await sleep(60_000);
      await deleteRetired();
      break;
    }
    case "update": {
      const updated = await api("POST", `/instances/${old}/update`);
      if (revision(updated) !== template) throw new Error(`base ${old} updated to revision ${revision(updated)}, not ${template}`);
      done.base = `${old}:${template}`;
      break;
    }
    case "none":
      break;
  }
  // Forks copy the base at rest: an update or a wake leaves it running, and a run cut off
  // after either leaves it so for the next to stop.
  const id = readState().baseInstanceId;
  if (id && (await api("GET", `/instances/${id}`)).status === "running") {
    await api("POST", `/instances/${id}/stop`);
    done.stopped = id;
  }
  if (Object.keys(done).length === 0) {
    process.exitCode = 3;
    return template ? { revision: template } : undefined;
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

// One `prepare` at a time per checkout, waited for: a `listen` that restarts leaves its run
// going, and the next start's runs once it ends, so the pull it was asked for is prepared. The
// holder touches the lock each minute; one whose process has gone, or untouched for five
// minutes, as a reused process ID could pass for alive, is taken over.
const LOCK = fileURLToPath(new URL("agent37-prepare.lock", import.meta.url));

async function lock(): Promise<void> {
  for (let said = false; ; ) {
    try {
      writeFileSync(LOCK, String(process.pid), { flag: "wx" });
      break;
    } catch {
      if (!held()) {
        rmSync(LOCK, { force: true });
        continue;
      }
      if (!said) log("waiting for another prepare to end");
      said = true;
      await sleep(10_000);
    }
  }
  setInterval(() => {
    try {
      utimesSync(LOCK, new Date(), new Date());
    } catch {}
  }, 60_000).unref();
  process.on("exit", () => {
    try {
      if (readFileSync(LOCK, "utf8") === String(process.pid)) unlinkSync(LOCK);
    } catch {}
  });
}

// An empty lock is one being written.
function held(): boolean {
  try {
    const pid = Number(readFileSync(LOCK, "utf8"));
    return Date.now() - statSync(LOCK).mtimeMs < 5 * 60_000 && (pid === 0 || alive(pid));
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

const phases: Record<string, () => Promise<unknown>> = { create, suspend, resume, update, destroy, prepare };
const phase = phases[process.argv[2] ?? ""];
if (!phase) {
  console.error("usage: agent37-recipe.ts create|suspend|resume|update|destroy|prepare");
  process.exit(2);
}
run(phase);
