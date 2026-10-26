import { WEBHOOK_DELIVERY } from "./check.ts";

// The Worker's structured log. One object per line, so Workers Logs indexes each field
// and a query can filter on `event`, `sessionId`, `status` and the rest. Traces are
// Cloudflare's own (`observability.traces` in wrangler.jsonc): every fetch to Linear,
// every Durable Object call and storage operation is a span, and these lines attach to
// the invocation's trace.
//
// Fields are IDs, identifiers, enums, counts and durations only. Never issue, comment,
// review or PR text, people's details, or secrets: see AGENTS.md.
//
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node.

export type Fields = Record<string, unknown>;

export function info(event: string, fields: Fields = {}): void {
  console.log({ event, ...fields });
}

export function warn(event: string, fields: Fields = {}): void {
  console.warn({ event, ...fields });
}

export function error(event: string, fields: Fields = {}): void {
  console.error({ event, ...fields });
}

// An error's message, for the `error` field. Our own errors and Linear's carry no text
// from issues or people (see src/shared/linear.ts).
export function describe(thrown: unknown): string {
  return thrown instanceof Error ? `${thrown.name}: ${thrown.message}` : String(thrown);
}

// A webhook's delivery ID for the log, once the body's signature has checked out, and
// only if it has a UUID's shape, as Linear's and GitHub's do. The signature covers the
// body, not the headers, so anything else is dropped rather than logged.
export function deliveryId(header: string | null): string | null {
  return header !== null && WEBHOOK_DELIVERY.test(header) ? header : null;
}
