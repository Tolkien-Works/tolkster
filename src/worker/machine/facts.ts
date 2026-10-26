// The facts an event carries, merged into its issue's store at once, whether or not its
// transition waits (docs/state-machine.md, "Facts"). Each fact is kept with the clock it
// came with, and a value whose clock isn't newer than the one held is dropped. Linear's
// times compare as ISO strings; a GitHub SHA is never ordered, only matched; a set only
// ever read (blockers, sub-issues) is versioned by when the read was sent, on the Durable
// Object's clock, and each member's state by its own `updatedAt`, so a read sent before a
// blocker finished can't put back what its webhook replaced.

import { earlier } from "../events.ts";
import { mergeChildChanged } from "./chain.ts";
import { newSession } from "./ctx.ts";
import type { DelegatedBy, Event, IssueFacts, IssueRead, Reader, ReadSet, Session, Store } from "./types.ts";

// What merging an event's facts found: something newer, nothing newer (the event runs no
// transition), an event that carries no fact of its own, or a change of state that left the
// state as the machines see it, such as a move between parents (`moved`): it only tells the
// parents.
export type Merged = "fresh" | "stale" | "none" | "moved";

export function session(s: Store, id: string): Session {
  return (s.sessions[id] ??= newSession());
}

// `sentMs`: for an event a read shows, when that read was first sent; null for a webhook's.
// A read names every fact again, so a value the store holds already only moves its clock.
export function mergeEvent(s: Store, e: Event, nowMs: number, sentMs: number | null = null): Merged {
  const f = s.facts;
  switch (e.kind) {
    case "sessionStarted": {
      const sess = session(s, e.session);
      const held = f.sessions[e.session];
      const started = held?.started === true;
      // A description mention arrives as a delegation (`inDescription`); a read that found it
      // one has already narrowed it, which this must not widen again.
      const kind = held?.kind === "mention" && e.sessionKind === "delegation" ? "mention" : e.sessionKind;
      if (!started) f.sessions[e.session] = { ...held, kind, createdAt: e.at, started: true };
      // At once, so a read of its comment that ends after the event arrived tells nothing.
      if (e.comment !== undefined) f.sessions[e.session].comment ??= e.comment;
      if (started) return "stale";
      sess.facts.byOwner = e.byOwner;
      if (sess.facts.request === null || sess.facts.request.at < e.at) sess.facts.request = { kind: kind === "delegation" || kind === "chain" ? "delegation" : "start", at: e.at };
      return "fresh";
    }
    case "ownerPrompted":
    case "ownerStopped": {
      const facts = session(s, e.session).facts;
      if (facts.request !== null && facts.request.at >= e.at) return "stale";
      facts.request = { kind: e.kind === "ownerPrompted" ? "prompt" : "stop", at: e.at };
      facts.byOwner = e.byOwner;
      return "fresh";
    }
    case "delegateChanged":
      // A read's `unknown` yields to the event at its own clock, which says who delegated, as
      // does the owner a `created` one was taken for.
      if (f.delegate !== null && (f.delegate.at > e.at || (f.delegate.at === e.at && f.delegate.value !== "unknown" && f.delegate.created !== true))) return "stale";
      if (f.delegate !== null && f.delegate.value === e.by) {
        f.delegate.at = e.at;
        delete f.delegate.created;
        return "stale";
      }
      f.delegate = { value: e.by, at: e.at };
      if (e.by === "owner" || e.by === "chain") f.everDelegated = true;
      return "fresh";
    case "issueStateChanged": {
      if (f.issueState !== null && f.issueState.at >= e.at) return "stale";
      const same = f.issueState !== null && f.issueState.value === e.state && (f.issueState.triage === true) === (e.triage === true);
      const stayed = (f.parent?.id ?? null) === e.parent;
      f.issueState = { value: e.state, at: e.at, changedAt: f.issueState?.value === e.state ? f.issueState.changedAt : e.at, triage: e.triage === true };
      // A new parent's own facts are unknown until a read says them (`mergeParent`).
      if (e.parent === null) {
        f.parent = null;
        f.unparented = e.at;
      } else {
        // Named again, it is still that parent at this clock, against an older read's.
        if (f.parent === null || f.parent.id !== e.parent) f.parent = { id: e.parent, at: e.at, delegated: false, by: null, state: "open", parentAt: null };
        else f.parent.at = e.at;
        delete f.unparented;
      }
      // A move between Linear states of one type, under the same parent, tells no one: only
      // its clocks are kept, so an older event stays stale.
      if (same && stayed) return "stale";
      return same ? "moved" : "fresh";
    }
    // A parent's is kept only for the parent held, by the parent's own clock, apart from
    // `parentAt`, which a forward of it doesn't move.
    case "priorityChanged": {
      if (e.parent !== undefined && f.parent?.id !== e.parent) return "stale";
      const held = e.parent === undefined ? f.priority : (f.parent!.priority ?? null);
      if (held !== null && held.at >= e.at) return "stale";
      const value = { value: e.priority, at: e.at };
      if (e.parent === undefined) f.priority = value;
      else f.parent!.priority = value;
      return held?.value === e.priority ? "stale" : "fresh";
    }
    case "blockersChanged":
      return related(f.blockers, e.blocker, e.done, e.at) === "fresh" ? "fresh" : "stale";
    case "childChanged":
      return mergeChildChanged(s, e, nowMs);
    // A merged PR stays merged. Otherwise a newer time wins; at one GitHub second, which is all
    // GitHub's times give, the PR's later kind of event wins (an open, a push, a close, a
    // merge), and a higher-numbered PR over another in the repo, as the newer of the two.
    case "prOpened":
      // A landing holds no PR until a new worker begins: one opening now is an old one's.
      if (f.landed !== undefined) return "stale";
      if (f.pr !== null && ((f.pr.repo === e.repo && merged(f.pr, e.pr)) || f.pr.at > e.at || (f.pr.at === e.at && !(f.pr.repo === e.repo && e.pr > f.pr.number)))) return "stale";
      f.pr = { number: e.pr, head: e.head, repo: e.repo, status: "open", at: e.at, receivedMs: nowMs };
      return "fresh";
    case "prPushed": {
      // A push to a closed PR waits for its reopen's own event. A read names no `before`: only
      // a head other than the one held is a push it shows.
      if (f.pr === null || f.pr.number !== e.pr || f.pr.status !== "open" || (e.before === null && f.pr.head === e.head)) return "stale";
      // A replay of a webhook's held push, or one before it, is stale (`earlier`, events.ts).
      // Otherwise GitHub's times are to the second: at the held one, a webhook's push is the
      // later arrival and wins, and a read only when sent after the held event was received.
      if (e.before !== null && earlier(f.pr, f.pr.at, { head: e.head, before: e.before, at: e.at })) return "stale";
      if (f.pr.at > e.at || (f.pr.at === e.at && e.before === null && (sentMs === null || sentMs <= f.pr.receivedMs))) return "stale";
      // A read names no `before`, so the held one goes with the head it replaces.
      const before = e.before ?? undefined;
      f.pr = { ...f.pr, head: e.head, before, at: e.at, receivedMs: nowMs };
      return "fresh";
    }
    case "prMerged":
    case "prClosed": {
      if (f.pr === null || f.pr.number !== e.pr || merged(f.pr, e.pr) || f.pr.at > e.at) return "stale";
      const status = e.kind === "prMerged" ? "merged" : "closed";
      if (f.pr.status === status) return "stale";
      // A merge names the head it merged, which a lost push left behind: the merged rule's
      // reap keeps a checkout at any other.
      const head = e.kind === "prMerged" && e.head !== undefined ? { head: e.head } : {};
      f.pr = { ...f.pr, ...head, status, at: e.at, receivedMs: nowMs };
      return "fresh";
    }
    case "autoMergeDisabled":
      return f.pr !== null && f.pr.number === e.pr && f.pr.status === "open" ? "fresh" : "stale";
    // Kept by the PR's number alone, as it may land before the PR's own event: a read merges
    // only the PR held of that number.
    case "mergeAsked":
      if (f.mergeAsked === e.pr) return "stale";
      f.mergeAsked = e.pr;
      return "fresh";
    // A landing is the work's merge, over a PR's merge too, and has no clock: the first is
    // kept. One with no worker, or its worktree reaped, is a worker's gone since. The PR it
    // leaves no longer held is dropped as it moves on (step.ts, `acrossIssues`).
    case "landed":
      if (f.landed !== undefined || s.work === null || s.work.state === "reaped") return "stale";
      f.landed = { head: e.head };
      return "fresh";
    // Kept per head, not per reviewer or suite: one older than the held is stale, as the
    // `evaluatePr` the held one asked for was sent after it arrived and read all GitHub had
    // by then, or, failing, leaves it to the deadline's read. One in the held one's second may
    // have come after that read, so it is fresh and reads again.
    case "prReviewed": {
      const sha = e.sha ?? f.pr?.head;
      if (!e.reviewer || sha === undefined) return "stale";
      const held = f.feedback[sha];
      if (held !== undefined && held.at > e.at) return "stale";
      f.feedback[sha] = { at: e.at, copilot: e.copilot };
      return "fresh";
    }
    case "checksCompleted": {
      const held = f.checks[e.sha];
      if (held !== undefined && held.at > e.at) return "stale";
      f.checks[e.sha] = { value: e.result, at: e.at };
      return "fresh";
    }
    case "basePushed":
      if (f.base === e.base) return "stale";
      f.base = e.base;
      return "fresh";
    case "agentWorking":
    case "agentBackground":
    case "agentIdle":
    case "agentWaiting":
    case "agentExited":
    case "worktreeGone": {
      const status = ({ agentWorking: "working", agentBackground: "background", agentIdle: "idle", agentWaiting: "waiting", agentExited: "exited", worktreeGone: "gone" } as const)[e.kind];
      // A stop in the background ends a turn: it says none began.
      const workedMs = status === "working" ? nowMs : f.agent?.workedMs;
      f.agent = { status, sinceMs: f.agent?.status === status ? f.agent.sinceMs : nowMs, ...(workedMs === undefined ? {} : { workedMs }) };
      return "none";
    }
    case "workerRead": {
      const facts = session(s, e.session).facts;
      const held = readOf(facts, e.reader);
      if (held === null || held < e.through) {
        if (e.reader === "worker") facts.readThrough = e.through;
        else facts.turnReadThrough = e.through;
      }
      return "none";
    }
    case "workerAnswered": {
      const facts = session(s, e.session).facts;
      if (facts.answeredThrough === null || facts.answeredThrough < e.at) facts.answeredThrough = e.at;
      return "none";
    }
    default:
      return "none";
  }
}

// How far a reader's own `tolkster messages` has shown the owner's requests.
export function readOf(facts: Session["facts"], reader: Reader): string | null {
  return reader === "worker" ? facts.readThrough : facts.turnReadThrough;
}

const merged = (pr: NonNullable<IssueFacts["pr"]>, number: number) => pr.number === number && pr.status === "merged";

// `same`: newer, with the state held, so only its clock moved, as a read names every member again.
export function related(set: ReadSet | null, id: string, done: boolean, at: string, triage = false): Merged | "same" {
  if (set === null) return "stale";
  const held = set.states[id];
  if (held !== undefined && held.at >= at) return "stale";
  set.states[id] = state(done, triage, at);
  return held !== undefined && held.done === done && (held.triage === true) === triage ? "same" : "fresh";
}

function state(done: boolean, triage: boolean, at: string): ReadSet["states"][string] {
  return triage ? { done, triage, at } : { done, at };
}

// A session a read lists: kept as held, except that one the read says began in the issue's
// description, which the machine has as a delegation's (its event can't tell), becomes a
// mention's, in its kind and in the request its start made. Never the other way, so a read
// never widens what a session may ask. Its request is only the start's: a later one
// (`delegateChanged`) is another's, at a later clock.
function narrowedOrKept(s: Store, read: IssueRead["sessions"][number]): void {
  const f = s.facts;
  const held = f.sessions[read.id];
  if (held === undefined) {
    f.sessions[read.id] = { kind: read.inDescription === true && read.kind === "delegation" ? "mention" : read.kind, createdAt: read.createdAt };
    return;
  }
  if (read.inDescription !== true || held.kind !== "delegation") return;
  held.kind = "mention";
  const request = s.sessions[read.id]?.facts.request;
  if (request?.kind === "delegation" && request.at === held.createdAt) request.kind = "start";
}

// What a readIssue result holds that no event carries: repo and sessions by the
// issue's `updatedAt`, and which issues block it or are its sub-issues by when the read was
// sent. Who delegated, its state and parent, its priority and its parent's, and each member's
// state are the events the read shows (step.ts).
export function mergeReadOnly(s: Store, read: IssueRead, sentMs: number): void {
  const f = s.facts;
  if (f.repo === null || f.repo.at < read.updatedAt) f.repo = { value: read.repo, at: read.updatedAt };
  for (const sess of read.sessions) narrowedOrKept(s, sess);
  f.blockers = { ...readSet(f.blockers, read.blockers, sentMs), more: f.blockers !== null && f.blockers.sentMs > sentMs ? f.blockers.more : read.moreBlockers };
  f.children = { ...readSet(f.children, read.children, sentMs), more: f.children !== null && f.children.sentMs > sentMs ? f.children.more : read.moreChildren };
}

// The parent's own facts a read names, by the parent's `updatedAt`, once its
// `issueStateChanged` has merged which parent the issue has.
export function mergeParent(s: Store, read: IssueRead): void {
  const p = s.facts.parent;
  if (read.parent === null || p === null || p.id !== read.parent.id || (p.parentAt !== null && p.parentAt >= read.parent.updatedAt)) return;
  s.facts.parent = { ...p, delegated: read.parent.delegated, by: read.parent.by, state: read.parent.state, parentAt: read.parent.updatedAt };
}

// The priority the queue ranks the issue by: its own, else its parent's (docs/dispatcher.md).
export function priorityOf(f: IssueFacts): number {
  return f.priority?.value || (f.parent?.priority?.value ?? 0);
}

function readSet(held: ReadSet | null, members: { id: string }[], sentMs: number): ReadSet {
  // Which issues it names is only replaced by a read sent later than the one held, and keeps
  // those an event added at or after the read was sent: in the same millisecond, the event
  // may be the later, so a removed one stays until a read sent after.
  const added = Object.fromEntries(Object.entries(held?.added ?? {}).filter(([, ms]) => ms >= sentMs));
  const read = members.map((m) => m.id);
  const ids = held !== null && held.sentMs > sentMs ? held.ids : [...read, ...Object.keys(added).filter((id) => !read.includes(id))];
  return { sentMs: Math.max(sentMs, held?.sentMs ?? sentMs), ids, states: { ...(held?.states ?? {}) }, ...(Object.keys(added).length > 0 ? { added } : {}) };
}

// The issue taken from Tolkster, or given it by someone the machine doesn't trust.
export function taken(by: DelegatedBy): boolean {
  return by === "other" || by === null;
}

export function unfinished(set: ReadSet | null): string[] {
  if (set === null) return [];
  return set.ids.filter((id) => set.states[id]?.done !== true).sort();
}

export function same(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((x, i) => x === b[i]);
}

// A blocker is unfinished among those read, or there are more than the first 50 read.
export function blocked(f: IssueFacts): boolean {
  return unfinished(f.blockers).length > 0 || f.blockers?.more === true;
}
