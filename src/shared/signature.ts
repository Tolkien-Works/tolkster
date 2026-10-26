// Linear signs each webhook with HMAC-SHA256 over the raw body, hex-encoded, in
// the Linear-Signature header. The body must be verified as received: re-serialising
// parsed JSON can change the bytes and fail an honest signature.

export const SIGNATURE_HEADER = "linear-signature";

// Linear's guidance: reject a webhookTimestamp more than a minute from our clock.
export const MAX_SKEW_MS = 60_000;

export async function verifySignature(rawBody: string, signature: string | null, secret: string): Promise<boolean> {
  if (!secret || !signature || !/^[0-9a-f]{64}$/i.test(signature)) return false;
  // subtle.verify compares in constant time, which a string comparison of hex would not.
  return crypto.subtle.verify("HMAC", await hmacKey(secret), hexToBytes(signature), new TextEncoder().encode(rawBody));
}

type HmacKey = ReturnType<typeof crypto.subtle.importKey>;

// Imported once per isolate, and again only if the secret changes.
let cachedKey: { secret: string; key: HmacKey } | null = null;

function hmacKey(secret: string): HmacKey {
  if (cachedKey?.secret !== secret) {
    const key = crypto.subtle.importKey("raw", new TextEncoder().encode(secret), { name: "HMAC", hash: "SHA-256" }, false, ["verify"]);
    cachedKey = { secret, key };
    // A failed import isn't kept.
    key.catch(() => {
      if (cachedKey?.key === key) cachedKey = null;
    });
  }
  return cachedKey.key;
}

export function isFresh(webhookTimestamp: unknown, nowMs: number): boolean {
  return typeof webhookTimestamp === "number" && Math.abs(nowMs - webhookTimestamp) <= MAX_SKEW_MS;
}

function hexToBytes(hex: string): Uint8Array<ArrayBuffer> {
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < bytes.length; i++) bytes[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return bytes;
}
