// The checks every input from outside is read through, here and in the Durable Object: a
// check that fails throws Invalid, so what reached a stored fact, a command or a key was
// checked whole or refused whole. Ingress that drops a bad input instead of refusing it
// (events.ts, the report parser in machine.ts) tests its patterns directly.

export class Invalid extends Error {}

// Linear's `linear-delivery` and GitHub's `X-GitHub-Delivery`, neither covered by the
// signature: anything but a UUID's shape makes no event, as it couldn't be deduped.
export const WEBHOOK_DELIVERY = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function check(ok: boolean, what: string): void {
  if (!ok) throw new Invalid(`invalid ${what}`);
}

export function object(value: unknown, what: string): Record<string, unknown> {
  check(typeof value === "object" && value !== null && !Array.isArray(value), what);
  return value as Record<string, unknown>;
}

export function str(value: unknown, pattern: RegExp, what: string): string {
  check(typeof value === "string" && pattern.test(value), what);
  return value as string;
}

export function oneOf<T>(value: unknown, values: readonly T[], what: string): T {
  check(values.includes(value as T), what);
  return value as T;
}

export function bool(value: unknown, what: string): boolean {
  check(typeof value === "boolean", what);
  return value as boolean;
}

export function whole(value: unknown, what: string, max = 2 ** 31): number {
  check(typeof value === "number" && Number.isInteger(value) && value >= 0 && value <= max, what);
  return value as number;
}

export function list(value: unknown, what: string, max: number): unknown[] {
  check(Array.isArray(value) && value.length <= max, what);
  return value as unknown[];
}
