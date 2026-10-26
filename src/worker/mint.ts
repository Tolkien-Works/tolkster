// The GitHub App's installation tokens, minted here for every host (docs/hosts.md): the app's
// key is a Worker secret, and no host holds it. A token is narrowed to one repo and to the
// permissions its purpose needs (`PURPOSES`), and handed out only once the mint's response
// says it covers that repo's owner: a token is minted by repo name alone, so `owner/name`
// with another owner would otherwise get a token for the installed `name`.
//
// The key, the JWT and the tokens never reach a log line or an error. What a refusal says is
// this file's own text, never GitHub's.
//
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node.

import { REPO } from "./github.ts";

export interface GitHubApp {
  GITHUB_APP_KEY?: string;
  GITHUB_APP_ID?: string;
  GITHUB_INSTALLATION_ID?: string;
}

export const APP_SECRETS = ["GITHUB_APP_KEY", "GITHUB_APP_ID", "GITHUB_INSTALLATION_ID"] as const;

// What a call as the app itself, not an installation, needs.
export const APP_KEY = ["GITHUB_APP_KEY", "GITHUB_APP_ID"] as const;
type AppKey = Required<Pick<GitHubApp, (typeof APP_KEY)[number]>>;

// What a token is for, and the permissions it asks for: only those its calls need. The CLI's
// `Purpose` names the same, as test/contract.json pins.
export const PURPOSES = {
  // Without `contents: read`, GitHub refuses the PR with 422 "not all refs are readable",
  // though its permissions table lists only `pull_requests`.
  create: { pull_requests: "write", contents: "read" },
  reply: { pull_requests: "write" },
  // Replacing an open PR's description.
  describe: { pull_requests: "write" },
  // Without `contents: write`, GitHub refuses `resolveReviewThread` with "Resource not
  // accessible by integration". One of the tokens that could push.
  resolve: { pull_requests: "write", contents: "write" },
  // GitHub lets only someone who could merge the PR turn on its auto-merge.
  autoMerge: { pull_requests: "write", contents: "write" },
  // Merging a PR's base into its branch: a commit pushed to it.
  updateBranch: { pull_requests: "write", contents: "write" },
  // Merging a PR into its base, as auto-merge would.
  merge: { pull_requests: "write", contents: "write" },
  // A branch pushed through the `git-credential` helper (cli/src/credential.rs).
  push: { contents: "write" },
  // Reading PRs, their reviews, how their branch compares with its base, and their checks. A
  // compare and the base's commit date need `contents: read`, as does a PR's `commits` in
  // GraphQL, which the checks' rollup hangs off.
  read: { pull_requests: "read", contents: "read", checks: "read", statuses: "read" },
} as const satisfies Record<string, Record<string, "read" | "write">>;

export type Purpose = keyof typeof PURPOSES;

// What reading a build's artifact needs (build.ts): no purpose a host may ask /token for.
export const BUILD_PERMISSIONS = { actions: "read" } as const;

export function isPurpose(purpose: unknown): purpose is Purpose {
  return typeof purpose === "string" && Object.hasOwn(PURPOSES, purpose);
}

// Whether a worker host may be handed a token for `purpose`: merging a base into a branch, or
// a PR into its base, is a write `listen` alone makes, with the realtime secret.
export function forHosts(purpose: Purpose): boolean {
  return purpose !== "updateBranch" && purpose !== "merge";
}

// What /token answers: its status and body, and why, for the request's log line, when it
// refuses.
export type Answer = { status: number; body: unknown; reason?: string };

export const API = "https://api.github.com";

// What of GitHub's answers is read, each field checked before use.
type Reply = { token?: unknown; expires_at?: unknown; repositories?: unknown; events?: unknown; slug?: unknown; message?: unknown };
export const HEADERS = { Accept: "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28", "User-Agent": "tolkster" };

// A token for `repo`, `owner/name`, with `purpose`'s permissions, and when it expires.
export async function installationToken(env: Required<GitHubApp>, repo: string, purpose: Purpose | typeof BUILD_PERMISSIONS, nowMs: number): Promise<Answer> {
  if (!REPO.test(repo)) return { status: 400, body: { refused: "not a GitHub repo, owner/name" }, reason: "repo" };
  const [owner, name] = repo.split("/");
  const minted = await call(env, "POST", `/app/installations/${env.GITHUB_INSTALLATION_ID.trim()}/access_tokens`, nowMs, { repositories: [name], permissions: typeof purpose === "string" ? PURPOSES[purpose] : purpose });
  if (minted.status !== 201) return refusal(minted, "an installation token");
  const token = typeof minted.body?.token === "string" ? minted.body.token : "";
  if (!token) return { status: 502, body: { refused: "GitHub's mint had no token" }, reason: "github_shape" };
  if (!covers(minted.body, owner, name)) {
    await revoke(token);
    return { status: 422, body: { refused: `the GitHub App's token does not cover ${repo}` }, reason: "not_covered" };
  }
  return { status: 200, body: { token, expiresAt: typeof minted.body?.expires_at === "string" ? minted.body.expires_at : null } };
}

// The app's own record, which only its JWT reads, so it needs no installation: the events
// its webhook is subscribed to, for `doctor`, and its slug, its installations' IDs and
// whether GITHUB_INSTALLATION_ID is one of them, for `github app create` to find an app it
// made and the installation the owner made.
export async function appRecord(env: AppKey & GitHubApp, nowMs: number): Promise<Answer> {
  const app = await call(env, "GET", "/app", nowMs);
  if (app.status !== 200) return refusal(app, "the app's record");
  const listed = await call(env, "GET", "/app/installations?per_page=100", nowMs);
  if (listed.status !== 200) return refusal(listed, "the app's installations");
  const events = Array.isArray(app.body?.events) ? app.body.events.filter((event) => typeof event === "string") : [];
  const slug = typeof app.body?.slug === "string" ? app.body.slug : null;
  const installs: unknown[] = Array.isArray(listed.body) ? listed.body : [];
  const installations = installs.map((install) => (install as { id?: unknown } | null)?.id).filter((id) => Number.isSafeInteger(id));
  const installed = installations.some((id) => String(id) === env.GITHUB_INSTALLATION_ID?.trim());
  return { status: 200, body: { events, slug, installations, installed } };
}

// Whether a mint's response lists `owner/name` among the repositories its token covers,
// ignoring case, as GitHub does.
export function covers(response: Reply | null, owner: string, name: string): boolean {
  const repos: ({ owner?: { login?: unknown } | null; name?: unknown } | null)[] = Array.isArray(response?.repositories) ? response.repositories : [];
  const same = (a: unknown, b: string) => typeof a === "string" && a.toLowerCase() === b.toLowerCase();
  return repos.some((repo) => same(repo?.owner?.login, owner) && same(repo?.name, name));
}

// A call as the app: its status, 0 when GitHub couldn't be reached and -1 when the key
// wouldn't sign; its body, or null when that isn't JSON; and, when its budget is spent,
// GitHub's wait in seconds.
export type Called = { status: number; body: Reply | null; retryAfter: number | null };

// GitHub's 5xx, or no answer, may pass: 502, which the CLI tries again. A spent budget is 429
// with GitHub's wait. Anything else, a key that won't sign included, won't pass: 422.
export function refusal({ status, retryAfter }: Called, what: string): Answer {
  if (status === -1) return { status: 422, body: { refused: "GITHUB_APP_KEY is not an RSA key in PEM" }, reason: "bad_key" };
  const reason = `github_${status}`;
  const said = status === 0 ? "GitHub couldn't be reached" : `GitHub answered HTTP ${status}`;
  if (retryAfter !== null) return { status: 429, body: { refused: `GitHub's budget is spent for ${what}`, retryAfter }, reason };
  return { status: status === 0 || status >= 500 ? 502 : 422, body: { refused: `${said} for ${what}` }, reason };
}

async function call(env: AppKey, method: string, path: string, nowMs: number, body?: unknown): Promise<Called> {
  let authorization: string;
  try {
    authorization = `Bearer ${await jwt(env, nowMs)}`;
  } catch {
    return { status: -1, body: null, retryAfter: null };
  }
  try {
    const response = await fetch(`${API}${path}`, { method, headers: { ...HEADERS, Authorization: authorization }, body: body === undefined ? undefined : JSON.stringify(body) });
    const reply = (await response.json().catch(() => null)) as Reply | null;
    return { status: response.status, body: reply, retryAfter: spentFor(response, reply, nowMs) };
  } catch {
    return { status: 0, body: null, retryAfter: null };
  }
}

// A 429, or a 403 with a time to retry after, none of the budget left or GitHub's message for
// a secondary limit: until it resets, as GitHub asks, a second over, else a minute. Null for
// any other answer, a suspended installation's 403 included.
export function spentFor(response: Response, reply: Reply | null, nowMs: number): number | null {
  if (response.status !== 403 && response.status !== 429) return null;
  const header = (name: string) => {
    const value = Number(response.headers.get(name) ?? Number.NaN);
    return Number.isFinite(value) && value >= 0 ? value : null;
  };
  const retryAfter = header("retry-after");
  if (retryAfter !== null) return retryAfter;
  const reset = header("x-ratelimit-reset");
  if (header("x-ratelimit-remaining") === 0 && reset !== null) return Math.max(0, reset - Math.floor(nowMs / 1000)) + 1;
  const secondary = typeof reply?.message === "string" && reply.message.toLowerCase().includes("secondary rate limit");
  return response.status === 429 || secondary || header("x-ratelimit-remaining") === 0 ? 60 : null;
}

// Best effort: a token that isn't handed out expires within the hour anyway.
export async function revoke(token: string): Promise<void> {
  try {
    await fetch(`${API}/installation/token`, { method: "DELETE", headers: { ...HEADERS, Authorization: `Bearer ${token}` } });
  } catch {}
}

// The app's JWT, RS256. `iat` is a minute back, for clock drift, and `exp` nine minutes on,
// inside GitHub's ten. `iss` is the app's ID, or its client ID, which GitHub also takes.
export async function jwt(env: AppKey, nowMs: number): Promise<string> {
  const now = Math.floor(nowMs / 1000);
  const id = env.GITHUB_APP_ID.trim();
  const iss = /^\d+$/.test(id) ? Number(id) : id;
  const part = (value: unknown) => base64url(new TextEncoder().encode(JSON.stringify(value)));
  const message = `${part({ alg: "RS256", typ: "JWT" })}.${part({ iat: now - 60, exp: now + 9 * 60, iss })}`;
  return `${message}.${base64url(await sign(env.GITHUB_APP_KEY, message))}`;
}

export async function sign(pem: string, message: string): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.sign("RSASSA-PKCS1-v1_5", await signingKey(pem), new TextEncoder().encode(message)));
}

// WebCrypto's key, named so under both Workers' types and Node's.
type Key = Awaited<ReturnType<typeof crypto.subtle.importKey>>;

// Imported once per isolate, and again only when the secret changes.
let signing: { pem: string; key: Promise<Key> } | null = null;

function signingKey(pem: string): Promise<Key> {
  if (signing?.pem !== pem) {
    const key = importKey(pem);
    signing = { pem, key };
    key.catch(() => {
      if (signing?.key === key) signing = null;
    });
  }
  return signing.key;
}

// GitHub's download is PKCS#1 ("RSA PRIVATE KEY"), which WebCrypto doesn't import, so it is
// wrapped as PKCS#8; a key already converted ("PRIVATE KEY") is taken as it is. A secret
// pasted with its line breaks as `\n` reads the same. The error never quotes the key.
async function importKey(pem: string): Promise<Key> {
  const invalid = new Error("the GitHub App's key is not an RSA key in PEM");
  const match = /-----BEGIN (RSA )?PRIVATE KEY-----([^-]+)-----END (RSA )?PRIVATE KEY-----/.exec(pem);
  if (!match || match[1] !== match[3]) throw invalid;
  let der: Uint8Array;
  try {
    der = Uint8Array.from(atob(match[2].replace(/\\n|\s/g, "")), (c) => c.charCodeAt(0));
  } catch {
    throw invalid;
  }
  try {
    return await crypto.subtle.importKey("pkcs8", match[1] ? pkcs8(der) : der, { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" }, false, ["sign"]);
  } catch {
    throw invalid;
  }
}

// PKCS#8's PrivateKeyInfo around a PKCS#1 key: version 0, rsaEncryption, the key.
function pkcs8(pkcs1: Uint8Array): Uint8Array {
  const version = [0x02, 0x01, 0x00];
  const rsaEncryption = [0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00];
  return der(0x30, [...version, ...rsaEncryption, ...der(0x04, pkcs1)]);
}

function der(tag: number, body: ArrayLike<number>): Uint8Array {
  const n = body.length;
  const length = n < 0x80 ? [n] : n < 0x100 ? [0x81, n] : n < 0x10000 ? [0x82, n >> 8, n & 0xff] : [0x83, n >> 16, (n >> 8) & 0xff, n & 0xff];
  return Uint8Array.from([tag, ...length, ...Array.from(body)]);
}

function base64url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/=+$/, "").replace(/\+/g, "-").replace(/\//g, "_");
}
