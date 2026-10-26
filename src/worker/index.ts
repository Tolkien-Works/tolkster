import type { AgentSessionEventWebhookPayload } from "@linear/sdk/webhooks";
import { clearDelegateMadeBy, createActivity, IDENTIFIER, type WebhookType } from "../shared/linear.ts";
import { isFresh, SIGNATURE_HEADER, verifySignature } from "../shared/signature.ts";
import { type CommentPayload, describeComment } from "./comment.ts";
import { acknowledgement, type Decision, decide } from "./decide.ts";
import { fromGitHub, fromLinearComment, fromSession, type GitHubPayload, type Ingress, type SessionEvent } from "./events.ts";
import { describeIssue, issueFacts, type IssuePayload } from "./issue.ts";
import { describeGitHub, fromPush, GITHUB_DELIVERY_HEADER, GITHUB_EVENT_HEADER, GITHUB_SIGNATURE_HEADER, githubSignature, pushedAt } from "./github.ts";
import { deliveryId, describe, error, type Fields, info, warn } from "./log.ts";
import { bearerMatches, unset } from "./realtime.ts";
import type { Relay } from "./relay.ts";
import { buildOf, fetchBuild } from "./build.ts";
import { codeShaped, credentialShaped } from "./hosts.ts";
import { type Answer, APP_KEY, APP_SECRETS, appRecord, type GitHubApp, forHosts, installationToken, isPurpose } from "./mint.ts";
import { appToken, HANDED_LEFT_MS, type LinearApp, withToken } from "./token.ts";
import { unanswered, whyUnanswered } from "./unanswered.ts";

export { Relay } from "./relay.ts";

export interface Env extends LinearApp, GitHubApp {
  OWNER_USER_ID: string;
  LINEAR_WEBHOOK_SECRET: string;
  // The bearer secret `tolkster listen` connects with, and `tolkster` records its state with.
  REALTIME_SECRET: string;
  // The GitHub App's webhook secret.
  GITHUB_WEBHOOK_SECRET: string;
  // The owner's Claude token (`claude setup-token`), handed to worker hosts at /token. The
  // GitHub App's key and IDs (`GitHubApp`) mint every host's GitHub tokens there.
  CLAUDE_TOKEN?: string;
  // The repo, `owner/name`, whose CI keeps the builds of `tolkster` that /build serves.
  BUILD_REPO?: string;
  RELAY: DurableObjectNamespace<Relay>;
}

// Linear's ID for a webhook delivery, the same on each retry of it.
const LINEAR_DELIVERY_HEADER = "linear-delivery";

export default {
  // Every request ends in one `request` line: its route, status and duration, and what
  // the handler added to `trail` on the way: why it was refused, the delivery, session
  // or PR, and what was decided. Work done after the response, in `waitUntil`, logs its
  // own lines.
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const started = Date.now();
    const url = new URL(request.url);
    const route = ROUTES.has(url.pathname) ? url.pathname : url.pathname.startsWith("/build/") ? "/build" : "other";
    const trail: Fields = {};
    let response: Response;
    try {
      response = await handle(request, url, env, ctx, trail);
    } catch (thrown) {
      error("request", { route, method: request.method, status: 500, durationMs: Date.now() - started, ...trail, error: describe(thrown) });
      throw thrown;
    }
    const { status } = response;
    const level = status >= 500 ? error : status >= 400 && route !== "other" ? warn : info;
    level("request", { route, method: request.method, status, durationMs: Date.now() - started, ...trail });
    return response;
  },
} satisfies ExportedHandler<Env>;

// Anything else is logged as "other": scanners' paths say nothing useful.
const ROUTES = new Set(["/enrol", "/oauth/callback", "/realtime", "/state", "/token", "/webhooks/github", "/webhooks/linear"]);

const RELAY_ROUTES = new Set(["/enrol", "/realtime", "/state", "/webhooks/github"]);

async function handle(request: Request, url: URL, env: Env, ctx: ExecutionContext, trail: Fields): Promise<Response> {
  // Where Linear sends the browser after an admin approves the install. Tolkster
  // never uses the code: its tokens come from client credentials.
  if (url.pathname === "/oauth/callback") return new Response("The agent is installed. You can close this tab.");
  // The Relay is named after the owner: with no owner set, these would write to another one.
  if (!env.OWNER_USER_ID && RELAY_ROUTES.has(url.pathname)) {
    trail.reason = "not_configured";
    return unset("OWNER_USER_ID");
  }
  if (url.pathname === "/realtime") return connect(request, env, trail);
  if (url.pathname === "/state") return state(request, env, trail);
  if (url.pathname === "/token") return token(request, env, trail);
  if (url.pathname === "/enrol") return enrol(request, env, trail);
  if (url.pathname.startsWith("/build/")) return build(request, url, env, trail);
  if (url.pathname === "/webhooks/github") return github(request, env, trail);
  if (url.pathname === "/webhooks/linear") return linear(request, env, ctx, trail);
  return new Response("Not found", { status: 404 });
}

// A refusal: `reason` goes in the request's log line, `text` to the caller.
function refuse(trail: Fields, status: number, reason: string, text: string): Response {
  trail.reason = reason;
  return new Response(text, { status });
}

async function linear(request: Request, env: Env, ctx: ExecutionContext, trail: Fields): Promise<Response> {
  if (request.method !== "POST") return refuse(trail, 405, "method", "Method not allowed");
  // Deployed before the Linear app exists, the secrets are unset. Refuse everything
  // then: an undefined secret would otherwise be encoded as the string "undefined"
  // and become a key anyone could sign with. An unset owner would match a session or
  // comment with no author, so it is refused too.
  if (!env.LINEAR_WEBHOOK_SECRET || !env.LINEAR_CLIENT_ID || !env.LINEAR_CLIENT_SECRET || !env.OWNER_USER_ID) {
    return refuse(trail, 503, "not_configured", "Not configured");
  }

  const rawBody = await request.text();
  if (!(await verifySignature(rawBody, request.headers.get(SIGNATURE_HEADER), env.LINEAR_WEBHOOK_SECRET))) {
    return refuse(trail, 401, "bad_signature", "Bad signature");
  }
  const delivery = request.headers.get(LINEAR_DELIVERY_HEADER) ?? "";
  trail.deliveryId = deliveryId(delivery);

  let payload: { type?: unknown; action?: unknown; webhookTimestamp?: unknown };
  try {
    payload = JSON.parse(rawBody);
  } catch {
    return refuse(trail, 400, "malformed", "Malformed body");
  }
  trail.webhookType = typeof payload.type === "string" ? payload.type : null;
  trail.webhookAction = typeof payload.action === "string" ? payload.action : null;
  // How late it arrived, so slow deliveries and clock skew show before `stale` does.
  if (typeof payload.webhookTimestamp === "number") trail.ageMs = Date.now() - payload.webhookTimestamp;
  if (!isFresh(payload.webhookTimestamp, Date.now())) return refuse(trail, 401, "stale", "Stale");
  if (typeof payload.type !== "string" || !Object.hasOwn(WEBHOOKS, payload.type)) {
    trail.decision = "ignored";
    return new Response(null, { status: 200 });
  }
  return WEBHOOKS[payload.type as WebhookType](env, payload, delivery, trail, ctx);
}

type Webhook = (env: Env, payload: object, delivery: string, trail: Fields, ctx: ExecutionContext) => Promise<Response>;

// A handler for each of `WEBHOOK_TYPES`, no more and no fewer: any other type is ignored.
const WEBHOOKS = {
  AgentSessionEvent: (env, payload, delivery, trail, ctx) => agentSession(env, payload as AgentSessionEventWebhookPayload, delivery, trail, ctx),
  Comment: (env, payload, delivery, trail) => comment(env, payload as CommentPayload, delivery, trail),
  Issue: (env, payload, delivery, trail) => issue(env, payload as IssuePayload, delivery, trail),
} satisfies Record<WebhookType, Webhook>;

async function agentSession(env: Env, event: AgentSessionEventWebhookPayload, delivery: string, trail: Fields, ctx: ExecutionContext): Promise<Response> {
  trail.sessionId = event.agentSession?.id ?? null;
  trail.issue = event.agentSession?.issue?.identifier ?? null;
  const decision = decide(event, env.OWNER_USER_ID, Math.random, await sessionEvent(env, event, ctx));
  trail.decision = decision?.kind ?? "ignored";
  // Linear allows five seconds for this response and ten for the first activity,
  // so answer now and post the activity after.
  if (decision) ctx.waitUntil(act(env, event, decision, delivery));
  return new Response(null, { status: 200 });
}

// One call records what a signed session event says of Tolkster, for Issue webhooks to be
// judged against (issue.ts), and reads whether the session is on a sub-issue Tolkster
// delegated to itself (state.ts, `Delegation`), asked only for a session its owner didn't
// start. Only a call that reads holds up the answer, with the record before the read: an
// event that asks nothing more is recorded in `waitUntil`. A record that fails is logged by
// the relay. A call that fails says no: the session is refused, as any other not the
// owner's, and a record it may have lost costs only the filter's knowledge.
async function sessionEvent(env: Env, event: AgentSessionEventWebhookPayload, ctx: ExecutionContext): Promise<boolean> {
  const session = event.agentSession;
  const issueId = session?.issueId ?? session?.issue?.id ?? null;
  const appUserId = typeof event.appUserId === "string" ? event.appUserId : null;
  const ask = Boolean(issueId) && (session.creatorId ?? session.creator?.id) !== env.OWNER_USER_ID;
  if (!ask && appUserId === null) return false;
  const call = relay(env).sessionEvent(appUserId, issueId, ask).catch((thrown) => {
    error(ask ? "delegation_read_failed" : "session_event_record_failed", { sessionId: session?.id ?? null, recorded: appUserId === null ? null : "unknown", error: describe(thrown) });
    return false;
  });
  if (ask) return call;
  ctx.waitUntil(call);
  return false;
}

// Linear's Issue webhooks: a delegate change that involves Tolkster, or a change of
// state, becomes the machines' event (events.ts, `fromIssue`), an issue created in Triage
// the triage machine's ask, and an issue finished its store's `startContext` (issue.ts).
// There is nothing to acknowledge in Linear, so each is handed on before answering, and a
// Durable Object that fails answers 503, for Linear to send it again: the intake drops a
// delivery it has seen, an ask already held starts nothing more, and a context `listen`
// has started already starts nothing more (its `context_claim`).
async function issue(env: Env, payload: IssuePayload, delivery: string, trail: Fields): Promise<Response> {
  Object.assign(trail, describeIssue(payload));
  try {
    const { events, triage, context } = await relay(env).issue(issueFacts(payload), delivery, env.OWNER_USER_ID);
    trail.events = events;
    trail.decision = context ? "context" : triage ? "triage" : events > 0 ? "intake" : "ignored";
  } catch (thrown) {
    error("relay_failed", { ...describeIssue(payload), error: describe(thrown) });
    return refuse(trail, 503, "relay_failed", "Relay unavailable");
  }
  return new Response(null, { status: 200 });
}

// Linear's Comment webhooks: the owner's new comment on an issue becomes the machines'
// `ownerCommented` (events.ts), whose `readComment` tells a mention, a reply or a reply to
// a note of Tolkster's apart. Handed on before answering, as an Issue's is, so a Durable
// Object that fails answers 503 for Linear to send it again.
async function comment(env: Env, payload: CommentPayload, delivery: string, trail: Fields): Promise<Response> {
  Object.assign(trail, describeComment(payload));
  const event = fromLinearComment(payload, delivery, env.OWNER_USER_ID);
  trail.decision = event ? "intake" : "ignored";
  if (!event) return new Response(null, { status: 200 });
  try {
    await relay(env).ingest([event], identified(event.issueId, payload.data?.issue?.identifier));
  } catch (thrown) {
    error("relay_failed", { commentId: event.commentId, issueId: event.issueId, error: describe(thrown) });
    return refuse(trail, 503, "relay_failed", "Relay unavailable");
  }
  return new Response(null, { status: 200 });
}

// An issue's identifier as a webhook gave it, for the Durable Object to keep beside its ID.
function identified(issueId: string, identifier: unknown): { issueId: string; identifier: string } | null {
  return typeof identifier === "string" && IDENTIFIER.test(identifier) ? { issueId, identifier } : null;
}

// A session's event is ingested inside `waitUntil`: a mention's start from the first, beside its
// acknowledgement, because its comment is read as soon as the Comment webhook is ingested and the
// read's result should find the event, with its comment's ID, already stored (docs/state-machine.md,
// Events); any other after the acknowledgement is posted, as what `listen` posts for it must
// follow that acknowledgement. Tried this many times, INGEST_RETRY_MS apart and then twice that,
// well inside its 30 s.
const INGEST_TRIES = 3;
const INGEST_RETRY_MS = 2000;

// Whether the relay took the event. Linear has had its answer, so nothing delivers it again: a
// failure is tried again here, which the intake's dedupe makes safe. Never throws.
async function ingestSession(env: Env, event: SessionEvent, identifier: string | null, started: number): Promise<boolean> {
  for (let attempt = 1; ; attempt++) {
    try {
      await relay(env).ingest([event], identified(event.issueId, identifier));
      info("relay_ingested", { sessionId: event.sessionId, kind: event.kind, attempt, durationMs: Date.now() - started });
      return true;
    } catch (thrown) {
      error("relay_failed", { sessionId: event.sessionId, kind: event.kind, attempt, error: describe(thrown) });
      if (attempt === INGEST_TRIES) return false;
      await new Promise((resolve) => setTimeout(resolve, INGEST_RETRY_MS * attempt));
    }
  }
}

async function act(env: Env, payload: AgentSessionEventWebhookPayload, decision: Decision, delivery: string): Promise<void> {
  const sessionId = decision.activity.agentSessionId;
  const started = Date.now();
  const event = decision.request ? fromSession(payload, decision, delivery) : null;
  const identifier = decision.request?.identifier ?? null;
  // A mention's start is awaited only after the presence check and the post, so a slow or broken
  // relay never delays the acknowledgement.
  const early = event?.kind === "sessionStarted" && event.commentId !== null ? ingestSession(env, event, identifier, started) : null;
  // Whether anything is listening for the action decides only the wording: the owner is
  // told when nothing is. The relay gets PRESENCE_DEADLINE_MS to say; past that, or on
  // a failure, the acknowledgement is as if it were. A failure logs its error; a timeout
  // shows as `presence_unknown` with the deadline's duration.
  const listening = () => relay(env).listening().catch((thrown) => {
    error("presence_failed", { sessionId, error: describe(thrown) });
    throw thrown;
  });
  const { activity, presence } = await acknowledgement(decision, listening, PRESENCE_DEADLINE_MS);
  if (presence === "unknown") warn("presence_unknown", { sessionId, durationMs: Date.now() - started });
  const posted = await withToken(env, sessionId, "activity_failed", (token) => createActivity(token, activity));
  if (posted) info("activity_posted", { sessionId, decision: decision.kind, type: activity.content.type, presence, durationMs: Date.now() - started });
  // Only once the acknowledgement is posted, so a slow or broken relay never delays it.
  const ingested = early !== null ? await early : event === null ? false : await ingestSession(env, event, identifier, started);
  const why = whyUnanswered({ request: decision.request !== undefined, mapped: event !== null, posted, ingested });
  if (why !== null) unanswered(sessionId, why, { decision: decision.kind });
  // The away message is recorded on its request once both the post and the event are in, so
  // its `quiet` deadline doesn't say it again.
  if (ingested && posted && presence === "away" && event !== null && (event.kind === "sessionStarted" || event.kind === "ownerPrompted")) {
    try {
      await relay(env).acknowledgedAway(event.issueId, event.sessionId, event.at);
    } catch (thrown) {
      error("away_mark_failed", { sessionId, error: describe(thrown) });
    }
  }
  if (decision.undelegate) {
    const { issueId, creatorId, appUserId } = decision.undelegate;
    // The webhook may be handled late, so the delegation is read again: only the creator's
    // own is taken back.
    let cleared = false;
    const read = await withToken(env, sessionId, "undelegate_failed", async (token) => {
      cleared = await clearDelegateMadeBy(token, issueId, creatorId, appUserId);
    });
    if (read) info(cleared ? "undelegated" : "undelegate_skipped", { sessionId, issueId });
  }
}

// The GitHub App's webhook: each delivery becomes the machines' events (events.ts,
// `fromGitHub`), mapped to their issues by the intake, before it is answered. GitHub
// redelivers only on request, so one the Durable Object fails is answered with an error,
// which GitHub's log shows, and the machines read the PR again at their deadlines
// (docs/state-machine.md, Events).
async function github(request: Request, env: Env, trail: Fields): Promise<Response> {
  if (request.method !== "POST") return refuse(trail, 405, "method", "Method not allowed");
  // Unset, there is no secret to check against: refuse everything, as for Linear.
  if (!env.GITHUB_WEBHOOK_SECRET) return refuse(trail, 503, "not_configured", "Not configured");
  const rawBody = await request.text();
  if (!(await verifySignature(rawBody, githubSignature(request.headers.get(GITHUB_SIGNATURE_HEADER)), env.GITHUB_WEBHOOK_SECRET))) {
    return refuse(trail, 401, "bad_signature", "Bad signature");
  }
  const delivery = request.headers.get(GITHUB_DELIVERY_HEADER) ?? "";
  trail.deliveryId = deliveryId(delivery);
  let payload: GitHubPayload;
  try {
    payload = JSON.parse(rawBody);
  } catch {
    return refuse(trail, 400, "malformed", "Malformed body");
  }
  if (payload === null || typeof payload !== "object") return refuse(trail, 400, "malformed", "Malformed body");
  const githubEvent = request.headers.get(GITHUB_EVENT_HEADER) ?? "";
  // IDs only, so a missed change can be checked against whether the webhook came.
  Object.assign(trail, describeGitHub(githubEvent, payload));
  const events: Ingress[] = fromGitHub(githubEvent, payload, delivery);
  const push = fromPush(githubEvent, payload);
  trail.events = events.length;
  if (events.length === 0 && push === null) return new Response(null, { status: 200 });
  try {
    await relay(env).github(events, push, pushedAt(payload));
  } catch (thrown) {
    error("relay_failed", { deliveryId: deliveryId(delivery), error: describe(thrown) });
    return refuse(trail, 503, "relay_failed", "Relay unavailable");
  }
  return new Response(null, { status: 200 });
}

// The workstation's `tolkster listen`, opening its WebSocket to the owner's relay. A worker
// host's credential never opens one.
async function connect(request: Request, env: Env, trail: Fields): Promise<Response> {
  const caller = await callerOf(request, env, trail);
  if (caller instanceof Response) return caller;
  if (caller !== null) return refuse(trail, 401, "unauthorized", "Unauthorized");
  if (request.headers.get("Upgrade")?.toLowerCase() !== "websocket") return refuse(trail, 426, "not_websocket", "Expected a WebSocket");
  return relay(env).fetch(request);
}

// Tolkster's own state: `tolkster` reads and records what it did, with the realtime
// secret or a worker host's credential, which the relay checks. The body is checked field by
// field in state.ts, which also says what of it the request's log line keeps.
async function state(request: Request, env: Env, trail: Fields): Promise<Response> {
  const caller = await callerOf(request, env, trail);
  if (caller instanceof Response) return caller;
  if (request.method !== "POST") return refuse(trail, 405, "method", "Method not allowed");
  const text = await request.text();
  trail.bytes = text.length;
  if (text.length > MAX_STATE_BODY) return refuse(trail, 413, "too_large", "Too large");
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    return refuse(trail, 400, "malformed", "Malformed body");
  }
  const outcome = await relay(env).state(body, caller);
  Object.assign(trail, outcome.log);
  return new Response(outcome.body, { status: outcome.status, headers: { "Content-Type": "application/json" } });
}

// A read of every session an exit's start plans from fits well inside this.
const MAX_STATE_BODY = 512 * 1024;

// How long the acknowledgement waits for the relay to say whether anything is listening:
// the relay answers from its sockets alone, in milliseconds, and Linear allows ten
// seconds for the first activity.
const PRESENCE_DEADLINE_MS = 2000;

// Who a request to /realtime, /state or /token is from: null for the owner, carrying the
// realtime secret, else a bearer shaped as a worker host's credential, for the relay to
// check, or the refusal of a request with neither, before it reaches the relay. Unset, there is no secret to check
// against: refuse everything, like the webhook.
async function callerOf(request: Request, env: Env, trail: Fields): Promise<string | null | Response> {
  if (!env.REALTIME_SECRET) {
    trail.reason = "not_configured";
    return unset("REALTIME_SECRET");
  }
  const header = request.headers.get("Authorization");
  if (await bearerMatches(header, env.REALTIME_SECRET)) return null;
  const bearer = header?.startsWith("Bearer ") ? header.slice("Bearer ".length) : "";
  return credentialShaped(bearer) ? bearer : refuse(trail, 401, "unauthorized", "Unauthorized");
}

// What a host trades its credential for, by POST /token's `for`: the Linear app token the
// Worker acts with, never `rejected`, the one the host last had, or the owner's Claude token;
// a GitHub App token for one `repo` and `purpose` (mint.ts), or the app's own record;
// and which of the secrets it hands out the Worker lacks (`missing`). The owner's machine,
// with the realtime secret, holds its own Linear and Claude credentials, so it may ask for
// the rest alone. Tokens go in the body alone, never in the log line.
async function token(request: Request, env: Env, trail: Fields): Promise<Response> {
  if (request.method !== "POST") return refuse(trail, 405, "method", "Method not allowed");
  const caller = await callerOf(request, env, trail);
  if (caller instanceof Response) return caller;
  let ask: { for?: unknown; rejected?: unknown; repo?: unknown; purpose?: unknown };
  try {
    ask = (JSON.parse(await request.text()) as typeof ask | null) ?? {};
  } catch {
    return refuse(trail, 400, "malformed", "Malformed body");
  }
  const kind = ask.for;
  if (typeof kind !== "string" || !Object.hasOwn(NEEDS, kind)) return refuse(trail, 400, "unknown_for", "Unknown token");
  trail.for = kind;
  if (caller !== null) {
    const host = await relay(env).host(caller);
    if (host === null) return refuse(trail, 401, "unauthorized", "Unauthorized");
    trail.host = host;
  } else if (kind === "linear" || kind === "claude") {
    return refuse(trail, 403, "owner", "Hosts only");
  }
  const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json", "Cache-Control": "no-store" } });
  const missing = [...new Set(Object.values(NEEDS).flat())].filter((name) => !env[name]);
  if (kind === "missing") return json({ missing });
  const lacking = NEEDS[kind as keyof typeof NEEDS].filter((name) => !env[name]);
  if (lacking.length > 0) {
    trail.reason = "not_configured";
    return json({ missing: lacking }, 503);
  }
  if (kind === "claude") return json({ token: env.CLAUDE_TOKEN });
  if (kind === "linear") return json(await appToken(env, HANDED_LEFT_MS, typeof ask.rejected === "string" ? ask.rejected : null));
  const app = env as Required<GitHubApp>;
  if (kind === "githubApp") return answered(trail, await appRecord(app, Date.now()), json);
  if (!isPurpose(ask.purpose)) return refuse(trail, 400, "purpose", "Unknown purpose");
  trail.purpose = ask.purpose;
  if (caller !== null && !forHosts(ask.purpose)) return refuse(trail, 403, "listen_only", "Not for hosts");
  return answered(trail, await installationToken(app, typeof ask.repo === "string" ? ask.repo : "", ask.purpose, Date.now()), json);
}

// A new host's `tolkster host join`, trading the enrolment code `host add` minted for its
// credential and settings (hosts.ts). The code is the only authority, so nothing else is
// asked of the caller; a code unknown, used or expired gets the same 404. Neither the code
// nor what it opens reaches the log line.
async function enrol(request: Request, env: Env, trail: Fields): Promise<Response> {
  if (request.method !== "POST") return refuse(trail, 405, "method", "Method not allowed");
  let code: unknown;
  try {
    code = (JSON.parse(await request.text()) as { code?: unknown } | null)?.code;
  } catch {
    return refuse(trail, 400, "malformed", "Malformed body");
  }
  const traded = codeShaped(code) ? await relay(env).enrol(code) : null;
  if (traded === null) return refuse(trail, 404, "unknown_code", "Unknown code");
  trail.host = traded.host;
  return new Response(JSON.stringify({ ...traded, config: JSON.parse(traded.config) }), { headers: { "Content-Type": "application/json", "Cache-Control": "no-store" } });
}

// A download of the prebuilt `tolkster` for a target and commit (build.ts), by a host's
// credential, by an enrolment code not yet traded (a new host has no credential until `host
// join`, which needs this build), or by the realtime secret, which is how a `listen` that
// doesn't build from source takes a new build. No secret, credential or code reaches the log line.
async function build(request: Request, url: URL, env: Env, trail: Fields): Promise<Response> {
  if (request.method !== "GET") return refuse(trail, 405, "method", "Method not allowed");
  const wanted = buildOf(url.pathname);
  if (wanted === null) return refuse(trail, 404, "unknown_build", "Not found");
  const header = request.headers.get("Authorization");
  const bearer = header?.startsWith("Bearer ") ? header.slice("Bearer ".length) : "";
  const owner = Boolean(env.REALTIME_SECRET) && (await bearerMatches(header, env.REALTIME_SECRET));
  const host = owner ? "" : credentialShaped(bearer) ? await relay(env).host(bearer) : codeShaped(bearer) && (await relay(env).enrolled(bearer)) ? "" : null;
  if (host === null) return refuse(trail, 401, "unauthorized", "Unauthorized");
  Object.assign(trail, wanted, owner ? { owner } : host === "" ? { enrolling: true } : { host });
  const json = (body: unknown, status: number) => new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json", "Cache-Control": "no-store" } });
  const lacking = [...APP_SECRETS, "BUILD_REPO" as const].filter((name) => !env[name]);
  if (lacking.length > 0) {
    trail.reason = "not_configured";
    return json({ missing: lacking }, 503);
  }
  const built = await fetchBuild(env as Required<GitHubApp>, env.BUILD_REPO!, wanted.target, wanted.commit, Date.now());
  if ("tarball" in built) return new Response(built.tarball, { headers: { "Content-Type": "application/gzip", "Cache-Control": "no-store" } });
  return answered(trail, built, json);
}

// The secrets each kind of token needs: one unset answers 503 naming it.
const NEEDS = {
  linear: ["LINEAR_CLIENT_ID", "LINEAR_CLIENT_SECRET"],
  claude: ["CLAUDE_TOKEN"],
  github: APP_SECRETS,
  githubApp: APP_KEY,
  missing: [],
} as const satisfies Record<string, readonly (keyof Env)[]>;

function answered(trail: Fields, answer: Answer, json: (body: unknown, status: number) => Response): Response {
  if (answer.reason) trail.reason = answer.reason;
  return json(answer.body, answer.status);
}

function relay(env: Env): DurableObjectStub<Relay> {
  return env.RELAY.get(env.RELAY.idFromName(env.OWNER_USER_ID));
}
