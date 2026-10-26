// The relay's one alarm. A Durable Object has a single alarm, so everything the relay does
// on a timer is a Timer in one list (Relay.timers): when it is next due, read from
// storage, and what it does when the alarm fires, which reads what is due from storage
// too. The alarm is set to the earliest of them, and every one runs when it fires. A new
// timer is a new entry in that list, never a second alarm.
//
// Nothing here imports Cloudflare's runtime, so the tests can run it under Node.

export interface Timer {
  // When it is next due, or null if nothing is.
  next(): Promise<number | null>;
  // Does what is due at `nowMs`, and nothing that isn't.
  run(nowMs: number): Promise<void>;
}

// The earliest time any timer is due, or null if none is.
export async function earliest(timers: Timer[]): Promise<number | null> {
  let first: number | null = null;
  for (const timer of timers) {
    const due = await timer.next();
    if (due !== null && (first === null || due < first)) first = due;
  }
  return first;
}

// For an event that changed only one timer's rows: when the alarm should now be set, or null
// to leave it. It is only ever brought forward, never later or cleared, because the other
// timers' rows aren't read: an alarm left too early runs, finds nothing due and is set again.
export async function bringForward(timer: Timer, alarm: number | null): Promise<number | null> {
  const due = await timer.next();
  return due !== null && (alarm === null || due < alarm) ? due : null;
}

// Runs every timer in turn. One that throws stops none of the others; the first error is
// thrown after, so the runtime retries the alarm.
export async function runAll(timers: Timer[], nowMs: number): Promise<void> {
  let failed: unknown = null;
  for (const timer of timers) {
    try {
      await timer.run(nowMs);
    } catch (thrown) {
      failed ??= thrown;
    }
  }
  if (failed !== null) throw failed;
}
