import assert from "node:assert/strict";
import type { Storage } from "../src/worker/storage.ts";

// Durable Object storage in memory, as the Durable Object's: values are copied in and out,
// a `get` takes at most 128 keys and returns only those found, a `put` of several entries
// lands whole and `list` returns keys in order.
export class MemoryStorage<V = unknown> implements Storage {
  data = new Map<string, V>();
  gets = 0;
  lists = 0;
  // Every key a `list` returned, for a test that reads which ones it walked.
  listed: string[] = [];

  async get<T>(key: string): Promise<T | undefined>;
  async get<T>(keys: string[]): Promise<Map<string, T>>;
  async get<T>(keys: string | string[]): Promise<T | undefined | Map<string, T>> {
    this.gets++;
    if (typeof keys === "string") return structuredClone(this.data.get(keys)) as T | undefined;
    assert.ok(keys.length <= 128, `${keys.length} keys in one get`);
    return new Map(keys.filter((k) => this.data.has(k)).map((k) => [k, structuredClone(this.data.get(k)) as T]));
  }

  async put<T>(key: string, value: T): Promise<void>;
  async put<T>(entries: Record<string, T>): Promise<void>;
  async put<T>(key: string | Record<string, T>, value?: T): Promise<void> {
    const entries = typeof key === "string" ? { [key]: value } : key;
    for (const [k, v] of Object.entries(entries)) this.data.set(k, structuredClone(v) as V);
  }

  async delete(key: string): Promise<boolean>;
  async delete(keys: string[]): Promise<number>;
  async delete(keys: string | string[]): Promise<boolean | number> {
    if (typeof keys === "string") return this.data.delete(keys);
    return keys.filter((k) => this.data.delete(k)).length;
  }

  async list<T>({ prefix, end, limit }: { prefix: string; end?: string; limit?: number }): Promise<Map<string, T>> {
    this.lists++;
    const keys = [...this.data.keys()].filter((k) => k.startsWith(prefix) && (end === undefined || k < end)).sort().slice(0, limit);
    this.listed.push(...keys);
    return new Map(keys.map((k) => [k, structuredClone(this.data.get(k)) as T]));
  }
}
