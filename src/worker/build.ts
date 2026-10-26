// The prebuilt `tolkster` a host downloads at GET /build/<target>/<commit> (docs/hosts.md): the
// tarball CI kept as the artifact `tolkster-<target>-<commit>` (.github/workflows/build.yml),
// read as the GitHub App with `actions: read` and served as it was packed. Only `main` builds
// the artifact, so one from another branch, or for another commit, is never served: a host runs
// what it downloads.
//
// GitHub serves an artifact as a zip around the one tarball, which this unzips itself.
//
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node.

import { REPO } from "./github.ts";
import { type Answer, API, BUILD_PERMISSIONS, type GitHubApp, HEADERS, installationToken, refusal, revoke, spentFor } from "./mint.ts";

// The targets CI builds. The CLI names the same (`Os::target`), as test/contract.json pins.
export const TARGETS = ["linux-x64", "macos-arm64", "windows-x64"] as const;
export type Target = (typeof TARGETS)[number];

const COMMIT = /^[0-9a-f]{40}$/;

// A built `tolkster` is a few MB; these keep a wrong artifact from filling the Worker's memory.
const MAX_ZIP = 64 * 1024 * 1024;
const MAX_TARBALL = 128 * 1024 * 1024;

// The target and commit a /build path names, or null.
export function buildOf(pathname: string): { target: Target; commit: string } | null {
  const [, route, target, commit, ...rest] = pathname.split("/");
  const known = (TARGETS as readonly string[]).includes(target);
  return route === "build" && known && COMMIT.test(commit ?? "") && rest.length === 0 ? { target: target as Target, commit } : null;
}

export type Built = { tarball: Uint8Array } | Answer;

// The build of `commit` for `target`, from the artifacts of `repo`, `owner/name`. 404 when CI has
// kept none (still running, or never built for that target).
export async function fetchBuild(env: Required<GitHubApp>, repo: string, target: Target, commit: string, nowMs: number): Promise<Built> {
  if (!REPO.test(repo)) return { status: 503, body: { missing: ["BUILD_REPO"] }, reason: "not_configured" };
  const minted = await installationToken(env, repo, BUILD_PERMISSIONS, nowMs);
  const token = (minted.body as { token?: unknown } | null)?.token;
  if (minted.status !== 200 || typeof token !== "string") return minted;
  try {
    return await download(token, repo, `tolkster-${target}-${commit}`, commit, nowMs);
  } finally {
    await revoke(token);
  }
}

const none: Answer = { status: 404, body: { refused: "no build of that commit for that target" }, reason: "no_build" };

async function download(token: string, repo: string, name: string, commit: string, nowMs: number): Promise<Built> {
  const headers = { ...HEADERS, Authorization: `Bearer ${token}` };
  const listed = await get(`${API}/repos/${repo}/actions/artifacts?name=${name}&per_page=10`, headers, nowMs);
  if (!(listed instanceof Response)) return listed;
  const body = (await listed.json().catch(() => null)) as { artifacts?: unknown } | null;
  const artifacts = (Array.isArray(body?.artifacts) ? body.artifacts : []) as { id?: unknown; expired?: unknown; workflow_run?: { head_branch?: unknown; head_sha?: unknown; repository_id?: unknown; head_repository_id?: unknown } | null }[];
  // Newest first: a re-run of the same commit overwrites, so the first that qualifies.
  const found = artifacts.find((a) => a.expired === false && Number.isSafeInteger(a.id) && fromMain(a.workflow_run, commit));
  if (!found) return none;
  const zip = await get(`${API}/repos/${repo}/actions/artifacts/${found.id}/zip`, headers, nowMs);
  if (!(zip instanceof Response)) return zip;
  if (Number(zip.headers.get("content-length") ?? 0) > MAX_ZIP) return unreadable("too large");
  const bytes = new Uint8Array(await zip.arrayBuffer());
  const tarball = bytes.length > MAX_ZIP ? null : await unzipOne(bytes, `${name}.tar.gz`);
  // gzip's magic number: what CI packed with `tar -czf`.
  return tarball !== null && tarball[0] === 0x1f && tarball[1] === 0x8b ? { tarball } : unreadable("had another shape");
}

// A run of this repo's own `main` at `commit`: a fork's branch may be named `main` too.
function fromMain(run: { head_branch?: unknown; head_sha?: unknown; repository_id?: unknown; head_repository_id?: unknown } | null | undefined, commit: string): boolean {
  return run?.head_branch === "main" && run.head_sha === commit && typeof run.repository_id === "number" && run.head_repository_id === run.repository_id;
}

function unreadable(what: string): Answer {
  return { status: 502, body: { refused: `the build's artifact ${what}` }, reason: "artifact_shape" };
}

// A GET as the app's token: the response, or the refusal to give. Redirects are followed, and
// fetch leaves the token behind when one leaves GitHub's host.
async function get(url: string, headers: Record<string, string>, nowMs: number): Promise<Response | Answer> {
  let response: Response;
  try {
    response = await fetch(url, { headers });
  } catch {
    return refusal({ status: 0, body: null, retryAfter: null }, "the build's artifact");
  }
  if (response.status === 200) return response;
  const reply = (await response.json().catch(() => null)) as { message?: unknown } | null;
  if (response.status === 404 || response.status === 410) return none;
  return refusal({ status: response.status, body: reply, retryAfter: spentFor(response, reply, nowMs) }, "the build's artifact");
}

// The one file of a zip, if it is the one named `name`, stored or deflated, else null. Read from
// the central directory, which a zip streamed to GitHub has the sizes in; zip64 isn't read.
export async function unzipOne(zip: Uint8Array, name: string): Promise<Uint8Array | null> {
  const view = new DataView(zip.buffer, zip.byteOffset, zip.byteLength);
  let end = -1;
  for (let at = zip.length - 22; at >= Math.max(0, zip.length - 22 - 0xffff); at--) {
    if (view.getUint32(at, true) === 0x06054b50) {
      end = at;
      break;
    }
  }
  if (end < 0 || view.getUint16(end + 10, true) !== 1) return null;
  const directory = view.getUint32(end + 16, true);
  if (directory === 0xffffffff || directory + 46 > zip.length || view.getUint32(directory, true) !== 0x02014b50) return null;
  const [method, compressed, size, nameLength, local] = [view.getUint16(directory + 10, true), view.getUint32(directory + 20, true), view.getUint32(directory + 24, true), view.getUint16(directory + 28, true), view.getUint32(directory + 42, true)];
  if (new TextDecoder().decode(zip.subarray(directory + 46, directory + 46 + nameLength)) !== name) return null;
  if (size > MAX_TARBALL || local + 30 > zip.length || view.getUint32(local, true) !== 0x04034b50) return null;
  const start = local + 30 + view.getUint16(local + 26, true) + view.getUint16(local + 28, true);
  if (start + compressed > zip.length) return null;
  const data = zip.subarray(start, start + compressed);
  if (method === 0) return compressed === size ? data : null;
  if (method !== 8) return null;
  const out = new Uint8Array(size);
  let length = 0;
  const reader = new Blob([data]).stream().pipeThrough(new DecompressionStream("deflate-raw")).getReader();
  for (let chunk = await reader.read(); !chunk.done; chunk = await reader.read()) {
    if (length + chunk.value.length > size) {
      await reader.cancel();
      return null;
    }
    out.set(chunk.value, length);
    length += chunk.value.length;
  }
  return length === size ? out : null;
}
