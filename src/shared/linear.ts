// What the Worker needs from Linear: an app-actor token, and the shape of an agent
// activity. The CLI (cli/, in Rust) has its own copy in cli/src/linear.rs.

import { info, warn } from "../worker/log.ts";

const TOKEN_URL = "https://api.linear.app/oauth/token";
export const GRAPHQL_URL = "https://api.linear.app/graphql";

// Every client_credentials request must ask for exactly these scopes, and they must
// match the install URL in docs/setup.md. Linear replaces all of an app's actor tokens
// when one is requested with a different set, and the app's capabilities go with them:
// asking for only read,write once took away app:assignable, and Linear then refused
// to let anyone delegate to Tolkster. The CLI requests them too: SCOPES in
// cli/src/linear.rs must stay identical, and `cargo test` fails if it differs.
export const SCOPES = "read,write,app:assignable,app:mentionable";

// The types of webhook the Linear app is subscribed to, which `tolkster setup` asks for in the app's
// manifest (`WEBHOOK_TYPES` in cli/src/linear.rs, pinned in test/contract.json). The Worker has a
// handler for each (index.ts, `WEBHOOKS`) and ignores any other.
export const WEBHOOK_TYPES = ["AgentSessionEvent", "Comment", "Issue"] as const;
export type WebhookType = (typeof WEBHOOK_TYPES)[number];

// The shapes of Linear's own values that the Worker checks wherever they come in, from a
// webhook or from the CLI: an issue identifier, the team's key then a number (`TW-123`),
// and a timestamp, in Linear's one format, with milliseconds. Every timestamp being in
// that format is what lets the code compare them as strings.
export const IDENTIFIER = /^[A-Z][A-Z0-9]{0,15}-\d{1,9}$/;
export const TIME = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/;

export type AppToken = { accessToken: string; expiresAt: number };

export async function fetchAppToken(clientId: string, clientSecret: string, nowMs: number): Promise<AppToken> {
  const response = await fetch(TOKEN_URL, {
    method: "POST",
    headers: {
      "Content-Type": "application/x-www-form-urlencoded",
      Authorization: `Basic ${btoa(`${clientId}:${clientSecret}`)}`,
    },
    body: new URLSearchParams({ grant_type: "client_credentials", scope: SCOPES }),
  });
  if (!response.ok) {
    // Linear explains a refusal in error_description, e.g. that client credentials
    // are not switched on for the app. It never contains the secret.
    const detail = (await response.json().catch(() => null)) as { error_description?: string } | null;
    throw new Error(`Linear token request failed: HTTP ${response.status}${detail?.error_description ? `: ${detail.error_description}` : ""}`);
  }
  const body = (await response.json()) as { access_token: string; expires_in: number };
  return { accessToken: body.access_token, expiresAt: nowMs + body.expires_in * 1000 };
}

// A token within a day of expiry is treated as expired, so a long worker session
// never starts with one that runs out part way through.
export function isUsable(token: AppToken | null, nowMs: number): token is AppToken {
  return token !== null && token.expiresAt - nowMs > 24 * 60 * 60 * 1000;
}

export type ActivityContent =
  | { type: "thought"; body: string }
  | { type: "elicitation"; body: string }
  | { type: "response"; body: string }
  | { type: "error"; body: string }
  | { type: "action"; action: string; parameter: string; result?: string };

export type ActivityInput = { agentSessionId: string; content: ActivityContent; ephemeral?: boolean };

const CREATE_ACTIVITY = `mutation AgentActivityCreate($input: AgentActivityCreateInput!) {
  agentActivityCreate(input: $input) { success }
}`;

const CLEAR_DELEGATE = `mutation ClearDelegate($id: String!) {
  issueUpdate(id: $id, input: { delegateId: null }) { success }
}`;

// How many of an issue's newest history entries are read. A delegation older than these
// goes unseen, and is left alone.
const HISTORY = 10;

// The issue's delegate and creator, and its newest history entries, which say who made the
// delegation. Only ids and times are read.
const READ_DELEGATION = `query Delegation($id: String!) {
  issue(id: $id) {
    delegate { id }
    creator { id }
    history(first: ${HISTORY}) { nodes { createdAt actor { id } fromDelegate { id } toDelegate { id } } }
  }
}`;

export class UnauthorizedError extends Error {}

// Linear's budget for the app is spent (HTTP 400, an error of code RATELIMITED). `waitSeconds`
// is until it resets, so a caller can hold rather than fail.
export class RateLimitedError extends Error {
  readonly waitSeconds: number;

  constructor(waitSeconds: number) {
    super(`Linear's rate limit is spent: it resets in ${waitSeconds} s`);
    this.waitSeconds = waitSeconds;
  }
}

export async function createActivity(accessToken: string, input: ActivityInput): Promise<void> {
  await mutate(accessToken, "agentActivityCreate", CREATE_ACTIVITY, { input });
}

export async function clearDelegate(accessToken: string, issueId: string): Promise<void> {
  await mutate(accessToken, "issueUpdate", CLEAR_DELEGATE, { id: issueId });
}

// Takes the delegation to `appUserId` (the app the session is for) back only if
// `creatorId`, the creator of the session being refused, made the issue's current one. The webhook can be handled after the owner delegated the
// issue again, and a delegation with no actor (as a synced comment's mention reads) is no
// stranger's, so the issue's history is read first, and a delegation that isn't provably
// the creator's, or that can't be read, is left. Returns whether it was cleared.
export async function clearDelegateMadeBy(accessToken: string, issueId: string, creatorId: string, appUserId: string): Promise<boolean> {
  const issue = await query(accessToken, "issue", READ_DELEGATION, { id: issueId });
  if (delegationMadeBy(issue, appUserId) !== creatorId) return false;
  await clearDelegate(accessToken, issueId);
  return true;
}

// "A blocks B" is a relation on A of type `blocks`, so B's blockers are its inverse
// relations of that type, as cli/src/sessions.rs reads them. Only IDs and state types, a
// page of BLOCKER_PAGE at a time: Linear prices a read by the page asked for, and can't
// filter these by type, so an issue with many related ones takes more pages.
const BLOCKER_PAGE = 50;
// The pages read at most, BLOCKER_PAGE × BLOCKER_PAGES relations, as the read had in one.
const BLOCKER_PAGES = 5;
const RELATIONS = `nodes { type issue { state { type } } } pageInfo { hasNextPage endCursor }`;
const READ_BLOCKERS = `query Blockers($id: String!) {
  agentSession(id: $id) {
    issue { id identifier inverseRelations(first: ${BLOCKER_PAGE}) { ${RELATIONS} } }
  }
}`;
const READ_MORE_BLOCKERS = `query MoreBlockers($id: String!, $after: String!) {
  issue(id: $id) { inverseRelations(first: ${BLOCKER_PAGE}, after: $after) { ${RELATIONS} } }
}`;

export type Blockers = { issueId: string; identifier: string | null; unfinished: number };

// The session's issue and how many of its blockers are unfinished (neither completed,
// cancelled nor duplicate; a relation whose blocker is gone counts as none, as the CLI reads it), or
// null when the session has no issue, or it isn't a plain ID. Relations past the last page
// read count as one unfinished blocker: the check never queues an issue it didn't see clear.
export async function readBlockers(accessToken: string, sessionId: string): Promise<Blockers | null> {
  const session = await query(accessToken, "agentSession", READ_BLOCKERS, { id: sessionId });
  const blockers = blockersOf(session);
  if (!blockers) return null;
  let page = pageOf((session as { issue: { inverseRelations?: Relations } }).issue.inverseRelations);
  let unfinished = page.unfinished;
  for (let read = 1; page.after !== null && read < BLOCKER_PAGES; read++) {
    const issue = await query(accessToken, "issue", READ_MORE_BLOCKERS, { id: blockers.issueId, after: page.after });
    page = pageOf((issue as { inverseRelations?: Relations }).inverseRelations);
    unfinished += page.unfinished;
  }
  return { ...blockers, unfinished: unfinished + (page.more ? 1 : 0) };
}

// A duplicate has ended, as a completed or cancelled issue has.
const FINISHED = ["completed", "canceled", "duplicate"];
const DELEGATED_PAGE = 50;
const DELEGATED_PAGES = 5;
const READ_DELEGATED = `query Delegated($after: String) {
  issues(first: ${DELEGATED_PAGE}, after: $after, filter: { delegate: { isMe: { eq: true } }, state: { type: { nin: ${JSON.stringify(FINISHED)} } } }) {
    nodes { id identifier state { type } } pageInfo { hasNextPage endCursor }
  }
}`;

// `started`: no longer in Triage, the backlog or unstarted, so a worker has had it.
export type Delegated = { issueId: string; identifier: string | null; started: boolean };

// The unfinished issues delegated to the app whose token this is, IDs and state types only, up
// to DELEGATED_PAGES pages; one past them is logged.
export async function readDelegated(accessToken: string): Promise<Delegated[]> {
  const found: Delegated[] = [];
  let after: string | null = null;
  for (let read = 0; read < DELEGATED_PAGES; read++) {
    const page = (await query(accessToken, "issues", READ_DELEGATED, { after })) as { nodes?: { id?: unknown; identifier?: unknown; state?: { type?: unknown } | null }[]; pageInfo?: { hasNextPage?: unknown; endCursor?: unknown } | null };
    for (const { id, identifier, state } of page.nodes ?? []) {
      const started = !["triage", "backlog", "unstarted"].includes(state?.type as string);
      if (typeof id === "string" && LINEAR_ID.test(id)) found.push({ issueId: id, identifier: typeof identifier === "string" && IDENTIFIER.test(identifier) ? identifier : null, started });
    }
    const cursor = page.pageInfo?.endCursor;
    if (page.pageInfo?.hasNextPage !== true || typeof cursor !== "string" || cursor === "") return found;
    after = cursor;
  }
  warn("delegated_unread", { read: found.length });
  return found;
}

// A Linear ID (an issue's, a session's, a comment's), checked wherever one comes in.
export const LINEAR_ID = /^[0-9a-f-]{1,64}$/i;

type Relations = { nodes?: { type?: unknown; issue?: { state?: { type?: unknown } | null } | null }[]; pageInfo?: { hasNextPage?: unknown; endCursor?: unknown } | null } | null | undefined;

// One page of relations: its unfinished blockers, whether more follow, and the cursor to
// read them by, null when none follow or Linear gave no cursor.
export function pageOf(relations: Relations): { unfinished: number; more: boolean; after: string | null } {
  const unfinished = (relations?.nodes ?? []).filter((r) => r.type === "blocks" && r.issue && !FINISHED.includes(r.issue.state?.type as string)).length;
  const more = relations?.pageInfo?.hasNextPage === true;
  const cursor = relations?.pageInfo?.endCursor;
  return { unfinished, more, after: more && typeof cursor === "string" && cursor !== "" ? cursor : null };
}

// The first page's count, a further page counting as one unfinished blocker.
export function blockersOf(session: unknown): Blockers | null {
  const issue = (session as { issue?: { id?: unknown; identifier?: unknown; inverseRelations?: Relations } | null } | null)?.issue;
  if (typeof issue?.id !== "string" || !LINEAR_ID.test(issue.id)) return null;
  const page = pageOf(issue.inverseRelations);
  const identifier = typeof issue.identifier === "string" && IDENTIFIER.test(issue.identifier) ? issue.identifier : null;
  return { issueId: issue.id, identifier, unfinished: page.unfinished + (page.more ? 1 : 0) };
}

type HistoryEntry = { createdAt?: string; actor?: { id?: string } | null; fromDelegate?: { id?: string } | null; toDelegate?: { id?: string } | null };

// Who made the issue's current delegation to `delegateId`: the actor of the newest history
// entry that set it. With no entry changing the delegate, and the history not full, the
// delegation was made as the issue was created (history names none then): its creator.
// Undefined when the issue's delegate is someone else or none, when that entry is not among
// those read, or when it names no actor.
export function delegationMadeBy(issue: unknown, delegateId: string): string | undefined {
  const { delegate, creator, history } = (issue ?? {}) as {
    delegate?: { id?: string } | null;
    creator?: { id?: string } | null;
    history?: { nodes?: HistoryEntry[] } | null;
  };
  if (delegate?.id !== delegateId) return undefined;
  const entries = history?.nodes ?? [];
  // Newest first. Linear's timestamps share one ISO format, so text order is time order.
  const newest = entries
    .filter((entry) => entry.toDelegate?.id === delegate.id && typeof entry.createdAt === "string")
    .sort((a, b) => (b.createdAt as string).localeCompare(a.createdAt as string))[0];
  if (newest) return newest.actor?.id ?? undefined;
  const changed = entries.some((entry) => entry.fromDelegate?.id || entry.toDelegate?.id);
  return changed || entries.length >= HISTORY ? undefined : creator?.id ?? undefined;
}

// Linear's rate-limit headers on a response, those it sent (docs/linear-api.md, "The
// budget"). The resets are UTC epoch milliseconds.
export type Budget = {
  complexity?: number;
  requestsRemaining?: number;
  complexityRemaining?: number;
  requestsReset?: number;
  complexityReset?: number;
};

export function readBudget(headers: Pick<Headers, "get">): Budget {
  const number = (name: string): number | undefined => {
    const value = headers.get(name);
    const n = value === null || value.trim() === "" ? Number.NaN : Number(value);
    return Number.isFinite(n) ? n : undefined;
  };
  return {
    complexity: number("x-complexity"),
    requestsRemaining: number("x-ratelimit-requests-remaining"),
    complexityRemaining: number("x-ratelimit-complexity-remaining"),
    requestsReset: number("x-ratelimit-requests-reset"),
    complexityReset: number("x-ratelimit-complexity-reset"),
  };
}

// Seconds until the spent budget resets, a second over, within 5 s and an hour. With neither
// said to be spent, the later reset; with no reset at all, a minute.
export function budgetWait(budget: Budget, nowMs: number): number {
  const spent = [
    budget.requestsRemaining === 0 ? budget.requestsReset : undefined,
    budget.complexityRemaining === 0 ? budget.complexityReset : undefined,
  ].filter((reset): reset is number => reset !== undefined);
  const known = [budget.requestsReset, budget.complexityReset].filter((reset): reset is number => reset !== undefined);
  const resets = spent.length > 0 ? spent : known;
  const seconds = resets.length > 0 ? Math.floor(Math.max(0, Math.max(...resets) - nowMs) / 1000) + 1 : 60;
  return Math.min(Math.max(seconds, 5), 60 * 60);
}

// The fields of a call's log line: IDs-free numbers and times only. Undefined when the
// response carried no count at all.
export function budgetFields(budget: Budget): Record<string, number | string> | undefined {
  const time = (ms: number | undefined) => (ms !== undefined && Math.abs(ms) <= 8.64e15 ? new Date(ms).toISOString() : undefined);
  const fields = Object.fromEntries(
    Object.entries({
      complexity: budget.complexity,
      requestsRemaining: budget.requestsRemaining,
      complexityRemaining: budget.complexityRemaining,
      requestsReset: time(budget.requestsReset),
      complexityReset: time(budget.complexityReset),
    }).filter(([, value]) => value !== undefined),
  ) as Record<string, number | string>;
  return Object.keys(fields).length > 0 ? fields : undefined;
}

function spent(field: string, budget: Budget): RateLimitedError {
  const waitSeconds = budgetWait(budget, Date.now());
  warn("linear_rate_limited", { site: field, waitSeconds });
  return new RateLimitedError(waitSeconds);
}

type GraphQLError = { message: string; extensions?: { code?: string; type?: string } };

async function mutate(accessToken: string, field: string, query: string, variables: object): Promise<void> {
  const { data, status, errors } = await post(accessToken, field, query, variables);
  if (!(data?.[field] as { success?: boolean } | undefined)?.success) {
    throw new Error(`${field} failed: HTTP ${status}: ${errors?.map((e) => e.message).join("; ") ?? "no detail"}`);
  }
}

async function query(accessToken: string, field: string, query: string, variables: object): Promise<unknown> {
  const { data, status, errors } = await post(accessToken, field, query, variables);
  if (!data?.[field] || errors?.length) throw new Error(`${field} failed: HTTP ${status}: ${errors?.map((e) => e.message).join("; ") ?? "no detail"}`);
  return data[field];
}

async function post(accessToken: string, field: string, query: string, variables: object) {
  const response = await fetch(GRAPHQL_URL, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${accessToken}` },
    body: JSON.stringify({ query, variables }),
  });
  // One line per call, under the call's fixed name (`field`): what it cost and what is left.
  const budget = readBudget(response.headers);
  const fields = budgetFields(budget);
  if (fields) info("linear_call", { site: field, status: response.status, ...fields });
  if (response.status === 401) throw new UnauthorizedError("Linear rejected the app token");
  if (response.status === 429) throw spent(field, budget);
  let body: { data?: Record<string, unknown>; errors?: GraphQLError[] };
  try {
    body = (await response.json()) as typeof body;
  } catch {
    throw new Error(`${field} failed: HTTP ${response.status}, body was not JSON`);
  }
  // Linear usually reports a bad token as HTTP 400 with an error carrying both of these.
  if (body.errors?.some((e) => e.extensions?.code === "AUTHENTICATION_ERROR" || e.extensions?.type === "authentication error")) {
    throw new UnauthorizedError("Linear rejected the app token");
  }
  if (body.errors?.some((e) => e.extensions?.code === "RATELIMITED")) throw spent(field, budget);
  return { data: body.data, status: response.status, errors: body.errors };
}
