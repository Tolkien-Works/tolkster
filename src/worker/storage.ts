// The parts of Durable Object storage the Worker uses, as one type: the intake, the
// machines, the state and the relay each take it, and the tests' one in-memory storage
// (`test/storage.ts`) implements it. A `put` of several entries is written atomically;
// `list` returns keys in order, `end` excluded; a `get` of several keys returns only those found.
export interface Storage {
  get<T>(key: string): Promise<T | undefined>;
  get<T>(keys: string[]): Promise<Map<string, T>>;
  put<T>(key: string, value: T): Promise<void>;
  put<T>(entries: Record<string, T>): Promise<void>;
  delete(key: string): Promise<boolean>;
  delete(keys: string[]): Promise<number>;
  list<T>(options: { prefix: string; end?: string; limit?: number }): Promise<Map<string, T>>;
}

export const PRUNE_EVERY_MS = 60 * 60 * 1000;

// The version of every shape stored here. Stored state is reset, never migrated: a change to
// any stored shape bumps it, and the Durable Object's first start on the new code resets
// storage (relay.ts), so no load path reads an older shape.
export const STORAGE_VERSION = 7;
const VERSION = "storage-version";

// What a reset keeps of a stored entry, by its key: the value to write back in its place, or
// undefined to delete it.
export type Keep = (key: string, value: unknown) => unknown;

// Resets storage of another version, unless it holds this one; whether it reset. Each entry
// `keep` returns nothing for is deleted, and each it keeps is written back as it returns it;
// then `dropAlarm` runs and the version is recorded, last. One that fails part way records
// nothing, and the next start runs it again: `keep` takes back what it wrote as it took the
// original. `lose` sees each entry as stored before anything changes, to log what the reset
// loses of it. A retry may log again.
export async function resetOnce(storage: Storage, keep: Keep, dropAlarm: () => Promise<void>, lose: (key: string, value: unknown) => void = () => {}): Promise<boolean> {
  if ((await storage.get<number>(VERSION)) === STORAGE_VERSION) return false;
  const gone: string[] = [];
  const kept: [string, unknown][] = [];
  for (const [key, value] of await storage.list<unknown>({ prefix: "" })) {
    lose(key, value);
    const held = keep(key, value);
    if (held === undefined) gone.push(key);
    else kept.push([key, held]);
  }
  for (let i = 0; i < gone.length; i += BATCH) await storage.delete(gone.slice(i, i + BATCH));
  for (let i = 0; i < kept.length; i += BATCH) await storage.put(Object.fromEntries(kept.slice(i, i + BATCH)));
  await dropAlarm();
  await storage.put(VERSION, STORAGE_VERSION);
  return true;
}

// Keys one storage call takes.
const BATCH = 128;

// Whether a prune is due: the time under `key` is at least PRUNE_EVERY_MS old, or later than
// now (a clock that went back). The pruner writes `key` itself, when it is done.
export async function pruneDue(storage: Storage, key: string, nowMs: number): Promise<boolean> {
  const prunedAt = await storage.get<number>(key);
  return prunedAt === undefined || nowMs - prunedAt >= PRUNE_EVERY_MS || nowMs < prunedAt;
}
