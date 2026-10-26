// A seeded simulator for the machines (docs/state-machine.md): it delivers each event as
// the relay does (store.ts's `deliver`) and runs the alarm's `reconcile`, on storage in
// memory, with a fake `listen` that carries out each command against a small world
// (machine-sim-world.ts), and
// checks the spec's invariants (machine-sim-checks.ts) after every step. The steps
// (machine-sim-steps.ts) are generated up front from a seed, and each one is total (it does
// nothing where it finds nothing to act on), so a failing run shrinks by dropping steps and
// replays from its JSON alone.
//
// The steps include what the network and the alarm do to events: a redelivery, a delivery
// out of order, one dropped, a write that fails, results never sent, `lost` or `retryable`,
// the clock jumping past deadlines, timeouts and a settled store's retirement, and a
// delivery while the relay posts the away message, which the Durable Object's input gate
// lets in. A `race` goes further than the gate allows: two deliveries, whose
// compare-and-swap is what orders them, one run inside the other's read.
//
// The triage machine runs beside them: issues created in Triage, a session's terminal
// closing and `listen` connecting again, with its commands carried out by the same fake
// `listen`, which opens sessions in the world.

import { newCoordinator, type Coordinator, type Room } from "../src/worker/machine/coordinator.ts";
import { retired } from "../src/worker/machine/ctx.ts";
import { nextDue } from "../src/worker/machine/step.ts";
import { FLEET, type Run } from "../src/worker/machine/fleet.ts";
import { chained, Conflict, connected, contextAsked, deliver, fleetEvent, fleetReconciled, loadRoom, loadRun, reconcile, repoPushed, fleetReported, triageEvent, waitsCleared } from "../src/worker/machine/store.ts";
import { newTriage, TRIAGE, TRIAGE_KEY, TRIAGE_RETRIES, type Triage, type TriageEvent } from "../src/worker/machine/triage.ts";
import { type Command, type Event, type Store } from "../src/worker/machine/types.ts";
import { AGENT_REPORTS, type Command as Wire } from "../src/worker/machine.ts";
import { toWire } from "../src/worker/route.ts";
import { AT_QUIET, CHECKS, Memo, quiet } from "./machine-sim-checks.ts";
import { DAY, NAP_HOSTS, NAP_MACHINES, type Outcome, type Step } from "./machine-sim-steps.ts";
import { BLOCKER, identifier, ISSUES, PR_REPO, PROJECT, RECIPE, RECIPE_REPO, REPO, type Replay, START_MS, stuck, uuid, World } from "./machine-sim-world.ts";
import { hostOp } from "../src/worker/hosts.ts";
import { MemoryStorage } from "./storage.ts";

// An event waiting on the network; once applied, when (`atMs`) and its place in the run's
// order of sends and landings (`order`). A triage machine's event
// goes to it, not to an issue's store.
type Envelope = { event: Event; clocked: boolean; atMs?: number; order?: number } | { triage: TriageEvent; clocked: false; atMs?: number; order?: number };

// A command as listen was handed it, with its store as it stood then and its place in the
// run's order of sends and landings.
export type Sent = { command: Command; store: Store | undefined; order: number };

// What one step saw, for the checks.
export type Seen = {
  step: Step;
  // The commands handed to listen in this step.
  sent: Sent[];
  // The event already applied that the step delivered again, with every outbox empty
  // before it.
  again: Event | null;
  // The keys of each storage write in this step.
  puts: string[][];
  // The events this step applied to an issue's store, each with its place in the run's order.
  landed: { event: Event; order: number }[];
  // The issues whose store a write in this step left with no machine where it had one: only
  // the alarm's retirement drops a machine, starting the store's facts and machines over.
  retired: string[];
  // Each store written in this step with the due index not saying when it is next due.
  undue: string[];
};

export type View = { nowMs: number; stores: Record<string, Store>; coordinator: Coordinator; triage: Triage; room: Room; run: Run };

function machineless(s: Store): boolean {
  return s.work === null && Object.values(s.sessions).every((sess) => sess.machine === null);
}

// What `retired` (ctx.ts) leaves: a new store but for its counters and its sessions' kinds
// and starts.
function retiredShape(s: Store): boolean {
  return JSON.stringify(retired({ ...s, settledMs: null })) === JSON.stringify({ ...s, settledMs: null });
}

class Fault extends Error {}

// Tolkster's own chain's session or delegation of a sub-issue.
function byChain(e: Event): boolean {
  return (e.kind === "sessionStarted" && e.sessionKind === "chain") || (e.kind === "delegateChanged" && e.by === "chain");
}

// The shared storage, with faults: a write may be made to fail, a read may run another
// delivery before it returns, and a mutant may break it.
class SimStorage extends MemoryStorage {
  failNext = false;
  puts: string[][] = [];
  retired: string[] = [];
  undue: string[] = [];
  // The simulator's clock, for each store's due time as written.
  clock: () => number = () => 0;
  // Runs before the `countdown`-th read of a store or the coordinator returns, a write's
  // read of the revisions it checks included.
  hook: { countdown: number; run: () => Promise<void> } | null = null;
  mutant: Mutant | null;
  constructor(mutant: Mutant | null) {
    super();
    this.mutant = mutant;
  }

  async get<T>(key: string): Promise<T | undefined>;
  async get<T>(keys: string[]): Promise<Map<string, T>>;
  async get<T>(keys: string | string[]): Promise<T | undefined | Map<string, T>> {
    const machine = (k: string) => k.startsWith("machine:issue:") || k === "machine:coordinator" || k === TRIAGE_KEY;
    if (this.hook !== null && (typeof keys === "string" ? machine(keys) : keys.some(machine)) && --this.hook.countdown === 0) {
      const run = this.hook.run;
      this.hook = null;
      await run();
    }
    return typeof keys === "string" ? super.get<T>(keys) : super.get<T>(keys);
  }

  async put<T>(key: string, value: T): Promise<void>;
  async put<T>(entries: Record<string, T>): Promise<void>;
  async put<T>(key: string | Record<string, T>, value?: T): Promise<void> {
    if (this.failNext) {
      this.failNext = false;
      throw new Fault("write failed");
    }
    const entries = typeof key === "string" ? { [key]: value as T } : key;
    this.puts.push(Object.keys(entries));
    const due = (entries["machine:due"] ?? this.data.get("machine:due") ?? {}) as Record<string, number>;
    for (const [k, v] of Object.entries(entries)) {
      if (!k.startsWith("machine:issue:")) continue;
      const issue = k.slice("machine:issue:".length);
      const was = this.data.get(k) as Store | undefined;
      if (was !== undefined && !machineless(was) && retiredShape(v as Store)) this.retired.push(issue);
      if ((due[issue] ?? null) !== nextDue(v as Store, this.clock())) this.undue.push(issue);
    }
    // The mutant that drops the compare-and-swap: no write moves a record's revision.
    const kept = Object.entries(entries).map(([k, v]): [string, unknown] => [k, this.mutant === "casDropped" && (k.startsWith("machine:issue:") || k === "machine:coordinator" || k === TRIAGE_KEY) ? { ...(v as object), rev: 0 } : v]);
    await super.put(Object.fromEntries(kept));
  }

  async view(nowMs: number): Promise<View> {
    const stores: Record<string, Store> = {};
    for (const [k, v] of this.data) if (k.startsWith("machine:issue:")) stores[k.slice("machine:issue:".length)] = structuredClone(v as Store);
    const coordinator = structuredClone((this.data.get("machine:coordinator") as Coordinator | undefined) ?? newCoordinator());
    const triage = structuredClone((this.data.get(TRIAGE_KEY) as Triage | undefined) ?? newTriage());
    return { nowMs, stores, coordinator, triage, room: await loadRoom(this), run: await loadRun(this) };
  }
}

// A deliberately broken piece, to show the checks catch it: a write that drops the
// compare-and-swap, an observation dropped while a launch is in flight, an exit run beside a
// line to the worker, a typed round counted before its turn is seen begin, or no branch
// update ever sent, or an owner's stop that posts nothing.
export type Mutant = "casDropped" | "launchDropsObservation" | "exitBesideLine" | "typedRoundCounted" | "noUpdateBranch" | "stopSaysNothing";

export type Failure = { check: string; message: string; at: number; step: Step };

class Sim {
  storage: SimStorage;
  nowMs = START_MS;
  world = new World();
  mentions = 0;
  pool: Envelope[] = [];
  applied: Envelope[] = [];
  // Counts each send and landing, which one step's clock can't order.
  order = 0;
  // Commands sent and not yet carried out, in the order sent; a resend comes again.
  todo: Command[] = [];
  // The away messages the relay is to post, which never reach `listen`.
  posts: Command[] = [];
  sent: Sent[] = [];
  memo = new Memo();
  // Whether the paired host answered the last fleet listing.
  answering = false;
  // The triage commands sent and not yet carried out, apart from the issues' so a seed's
  // issue steps run as they would without them.
  triageTodo: Command[] = [];
  triagePending: Envelope[] = [];
  // The fleet's commands sent and not yet carried out, likewise.
  napsTodo: Command[] = [];
  // Each send in `todo` as the relay put it on the wire, its args read at that send.
  wires = new WeakMap<Command, Wire>();
  // How long the run to quiet goes on, once it has begun.
  quietUntil: number | null = null;

  constructor(mutant: Mutant | null) {
    this.storage = new SimStorage(mutant);
    this.storage.clock = () => this.nowMs;
    this.world.clock = () => this.nowMs;
    this.world.silentStops = mutant === "stopSaysNothing";
  }

  time(): string {
    return this.world.time();
  }

  id(prefix: string): string {
    return this.world.id(prefix);
  }

  // Each sender is called once its store's write has landed, so the store read here is
  // the one the command was sent from.
  sender = (commands: Command[]): void => {
    for (const command of commands.filter((c) => c.issue === TRIAGE || c.issue === FLEET)) {
      (command.issue === FLEET ? this.napsTodo : this.triageTodo).push(command);
      this.sent.push({ command: { ...command }, store: undefined, order: ++this.order });
    }
    commands = commands.filter((c) => c.issue !== TRIAGE && c.issue !== FLEET);
    for (const command of commands) {
      const store = this.storage.data.get(`machine:issue:${command.issue}`) as Store | undefined;
      this.sent.push({ command: { ...command }, store: store === undefined ? undefined : structuredClone(store), order: ++this.order });
      if (command.kind === "postAway") {
        this.posts.push(command);
        continue;
      }
      if (this.storage.mutant === "noUpdateBranch" && command.kind === "updateBranch") continue;
      const send = { ...command };
      if (store !== undefined) this.wires.set(send, structuredClone(toWire(command, store, identifier(ISSUES.indexOf(command.issue)))));
      this.todo.push(send);
    }
  };

  post(event: Event, clocked = true): void {
    this.pool.push({ event, clocked });
  }

  async start(): Promise<void> {
    // One slot for the two issues, so they contend for it: this machine alone, heard before and
    // after its settings, as `listen` lists and connects, the repo's recipe among them. The
    // fleet's listings pair NAP_HOSTS[0].
    await hostOp(this.storage, { op: "issueHost", host: NAP_HOSTS[0] }, true, this.nowMs);
    await hostOp(this.storage, { op: "admitHost", host: NAP_HOSTS[0] }, true, this.nowMs);
    // The repo's other machine, asleep, paired as NAP_HOSTS[1].
    await hostOp(this.storage, { op: "issueHost", host: NAP_HOSTS[1] }, true, this.nowMs);
    await hostOp(this.storage, { op: "admitHost", host: NAP_HOSTS[1] }, true, this.nowMs);
    const hosts = { held: 0, hosts: [], projects: {}, recipes: { [REPO]: RECIPE_REPO } };
    await fleetReported(this.storage, { hosts }, this.nowMs, this.sender);
    await fleetReported(this.storage, { settings: { cap: 1, slots: {}, repos: { [RECIPE_REPO]: RECIPE }, machines: { [NAP_HOSTS[0]]: { repo: RECIPE_REPO, made: true, asleep: false, environment: null, revision: null }, [NAP_MACHINES[0]]: { repo: RECIPE_REPO, made: true, asleep: true, environment: NAP_HOSTS[1], revision: "1" } } } }, this.nowMs, this.sender);
    await fleetReported(this.storage, { hosts }, this.nowMs, this.sender);
  }

  // The session an event lands for is one the machine has heard of, unless the event is a
  // stop, which carries no request. A note's thread's request is heard once its issue's store
  // holds it, never when its reply was too old to answer or told to a running worker instead.
  heard(event: Event): void {
    const session = (event as { session?: unknown }).session;
    if (typeof session === "string" && event.kind !== "ownerStopped" && this.world.requests.get(session)?.kind !== "note") this.world.heard.add(session);
    const s = this.storage.data.get(`machine:issue:${event.issue}`) as Store | undefined;
    for (const [root, r] of this.world.requests) if (r.kind === "note" && ISSUES[r.i] === event.issue && s?.sessions[root]?.facts.request?.at === r.at) this.world.heard.add(root);
  }

  // As the intake refuses Tolkster's own chain's session or delegation on an issue whose
  // store no longer holds that chain: seen, and never delivered again.
  refused(e: Event): boolean {
    return byChain(e) && ((this.storage.data.get(`machine:issue:${e.issue}`) as Store | undefined)?.facts.chain ?? null) === null;
  }

  // `tolkster issue chain` from the issue's orchestrating worker: its ask lands as the relay
  // delivers a report, and each sub-issue the parent's store answers it took is delegated. A
  // write that fails, or no answer, is an error to the worker, which delegates nothing and,
  // as its brief says, runs it once more.
  async chain(i: number): Promise<void> {
    const w = this.world.issues[i];
    const children = this.world.read(i).children.filter((c) => !c.done).map((c) => c.id);
    if (w.orchestrating === null || children.length === 0) return;
    const { session } = w.orchestrating;
    for (let attempt = 0; attempt < 2; attempt++) {
      const run = `run-${++this.world.runs}`;
      const event: Event = { kind: "chainRecorded", id: `${session}:${run}`, issue: ISSUES[i], role: "ask", session, run, children };
      try {
        await deliver(this.storage, event, this.nowMs, this.sender, async () => {
          this.applied.push({ event, clocked: false, atMs: this.nowMs, order: ++this.order });
        });
      } catch (error) {
        if (!(error instanceof Fault) && !(error instanceof Conflict)) throw error;
        continue;
      }
      const taken = await chained(this.storage, ISSUES[i], run);
      if (taken === null) continue;
      for (const child of taken) this.world.chainDelegate(ISSUES.indexOf(child));
      return;
    }
  }

  // Delivers one event as the relay does. A put that fails leaves nothing written and puts
  // the event back on the network, as its sender tries it again; a failure after it leaves
  // the outbox to the alarm, as the relay does.
  async deliverOne(e: Envelope): Promise<void> {
    if ("triage" in e) {
      try {
        await triageEvent(this.storage, e.triage, this.nowMs, this.sender);
        this.applied.push({ ...e, atMs: this.nowMs, order: ++this.order });
      } catch (error) {
        if (!(error instanceof Fault) && !(error instanceof Conflict)) throw error;
        this.pool.push(e);
      }
      return;
    }
    if (this.refused(e.event)) return;
    const key = `machine:issue:${e.event.issue}`;
    const was = this.storage.data.get(key) as Store | undefined;
    // The exit runs at once, as though nothing were queued, and what it queued waits behind
    // the line, unsent.
    const beside = this.storage.mutant === "exitBesideLine" && e.event.kind === "agentExited" && was !== undefined && was.queue.some((c) => c.kind === "deliverMessage" && c.sentMs !== null);
    if (beside) this.storage.data.set(key, { ...was, queue: [], waiting: [] });
    let landed = false;
    try {
      await deliver(this.storage, e.event, this.nowMs, beside ? (cs: Command[]) => this.sender(cs.filter((c) => c.issue !== e.event.issue)) : this.sender, async () => {
        landed = true;
        this.heard(e.event);
        this.applied.push({ ...e, atMs: this.nowMs, order: ++this.order });
      });
    } catch (error) {
      if (!(error instanceof Fault) && !(error instanceof Conflict)) throw error;
      if (!landed) this.pool.push(e);
    }
    const now = this.storage.data.get(key) as Store | undefined;
    if (beside) {
      const after = now ?? was;
      const added = after.queue.map((c) => ({ ...c, firstSentMs: null, sentMs: null, sends: 0 }));
      this.storage.data.set(key, { ...after, queue: [...was.queue, ...added], waiting: [...was.waiting, ...after.waiting] });
    }
    // A round whose line was typed, counted at once, as before a turn had to be seen begin.
    const unseen = now?.work?.unseen;
    if (this.storage.mutant === "typedRoundCounted" && unseen !== undefined) {
      let r = now!.facts.rounds;
      for (const p of unseen) r = { total: r.total + 1, byHead: { ...r.byHead, [p.head]: (r.byHead[p.head] ?? 0) + 1 }, lastAt: p.lastAt !== undefined && (r.lastAt === null || p.lastAt > r.lastAt) ? p.lastAt : r.lastAt, lastBase: p.lastBase ?? r.lastBase, lastFailed: p.lastFailed ?? r.lastFailed };
      this.storage.data.set(key, { ...now!, facts: { ...now!.facts, rounds: r }, work: { ...now!.work!, unseen: undefined } });
    }
    if (this.storage.mutant === "launchDropsObservation" && landed && now?.work?.state === "launching" && (AGENT_REPORTS as readonly string[]).includes(e.event.kind)) {
      this.storage.data.set(key, { ...now, waiting: now.waiting.filter((w) => w.event.id !== e.event.id) });
    }
  }

  // Delivers one event through `deliver`, whose compare-and-swap orders it against a race.
  async deliverChecked(e: Envelope): Promise<void> {
    if ("triage" in e) return this.deliverOne(e);
    if (this.refused(e.event)) return;
    let landed = false;
    try {
      await deliver(this.storage, e.event, this.nowMs, this.sender, async () => {
        landed = true;
        this.heard(e.event);
        this.applied.push({ ...e, atMs: this.nowMs, order: ++this.order });
      });
    } catch (error) {
      if (!(error instanceof Fault) && !(error instanceof Conflict)) throw error;
      if (!landed) this.pool.push(e);
    }
  }

  async step(st: Step): Promise<Event | null> {
    for (const e of this.triagePending.splice(0)) await this.deliverOne(e);
    // Triage's and naps' steps and mentions take no time, so the issues' run as they would
    // without them.
    if (!["triage", "triageListen", "naps", "napsListen", "mention"].includes(st.k)) this.nowMs += 1000;
    let again: Event | null = null;
    switch (st.k) {
      case "delegate": {
        const w = this.world.issues[st.i];
        // The session rule 3 asks, as the store would find it with none trusted.
        const asked = w.sessions.findLast((x) => x.kind === "delegation") ?? (w.sessions.every((x) => x.kind === "mention") ? w.sessions.at(-1) : undefined);
        if (st.again && asked !== undefined) {
          if (w.delegate === "owner") break;
          const at = this.time();
          w.delegate = "owner";
          w.updatedAt = at;
          w.unnamed = false;
          this.world.asked(st.i, asked.id, "delegation", at);
          this.post({ kind: "delegateChanged", id: this.id("deleg"), issue: ISSUES[st.i], by: "owner", at });
          break;
        }
        const before = { delegate: w.delegate, updatedAt: w.updatedAt };
        const at = this.time();
        w.delegate = "owner";
        w.updatedAt = at;
        // Only an issue never delegated before can have been created delegated.
        w.unnamed = st.as === "created" && w.sessions.length === 0 && !w.taken;
        w.lag = st.as === "lagging" ? { ...before, at } : null;
        const session = uuid(++this.world.ids, "5e55");
        w.sessions.push({ id: session, kind: "delegation", createdAt: at });
        this.world.asked(st.i, session, "delegation", at);
        const delegated: Event = { kind: "delegateChanged", id: this.id("deleg"), issue: ISSUES[st.i], by: "owner", at };
        const started: Event = { kind: "sessionStarted", id: this.id("sess"), issue: ISSUES[st.i], session, sessionKind: "delegation", byOwner: true, at };
        // A create's own webhook comes after its session's, and the issue moves on.
        for (const e of w.unnamed ? [started, delegated] : [delegated, started]) this.post(e);
        if (w.unnamed) w.updatedAt = this.time();
        break;
      }
      // A mention's session is a conversation's, and asks for a turn. Its clock and IDs are
      // its own, its time half a second past Linear's last, so the issues' are as they
      // would be without it.
      case "mention": {
        const w = this.world.issues[st.i];
        const at = this.world.since(500);
        const session = uuid(++this.mentions, "3e57");
        w.sessions.push({ id: session, kind: "mention", createdAt: at });
        this.world.asked(st.i, session, "mention", at);
        this.post({ kind: "sessionStarted", id: `mention-${this.mentions}`, issue: ISSUES[st.i], session, sessionKind: "mention", byOwner: true, at });
        break;
      }
      case "prompt":
      case "stop": {
        const w = this.world.issues[st.i];
        const kind = st.k === "stop" && st.mention === true ? "mention" : "delegation";
        const session = w.sessions.findLast((s) => s.kind === kind);
        if (session === undefined) break;
        const at = this.time();
        w.updatedAt = at;
        if (st.k === "prompt") this.world.asked(st.i, session.id, "delegation", at);
        // A mention's stop halts the worker only when it is the session the worker was given.
        else if (kind === "delegation" || session.id === w.took) this.world.halted(st.i, at);
        this.post({ kind: st.k === "prompt" ? "ownerPrompted" : "ownerStopped", id: this.id(st.k), issue: ISSUES[st.i], session: session.id, byOwner: true, at });
        break;
      }
      case "undelegate": {
        const w = this.world.issues[st.i];
        if (w.delegate === null) break;
        const at = this.time();
        w.delegate = null;
        w.updatedAt = at;
        w.taken = true;
        this.world.halted(st.i, at, true);
        this.post({ kind: "delegateChanged", id: this.id("undeleg"), issue: ISSUES[st.i], by: null, at });
        break;
      }
      case "state": {
        const w = this.world.issues[st.i];
        if (w.state === st.to) break;
        const at = this.time();
        w.state = st.to;
        w.stateAt = at;
        w.cancelledMs = st.to === "cancelled" ? this.nowMs : null;
        if (st.to === "cancelled") this.world.halted(st.i, at);
        w.updatedAt = at;
        this.post({ kind: "issueStateChanged", id: this.id("state"), issue: ISSUES[st.i], state: st.to, parent: this.world.parentOf(st.i), at });
        break;
      }
      case "comment": {
        const w = this.world.issues[st.i];
        const at = this.time();
        w.newestComment = at;
        w.updatedAt = at;
        this.post({ kind: "ownerCommented", id: this.id("comment"), issue: ISSUES[st.i], comment: uuid(++this.world.ids, "c0aa"), at });
        break;
      }
      case "note":
        this.world.note(st.i, st.root);
        break;
      case "priority": {
        const w = this.world.issues[st.i];
        if (w.priority === st.to) break;
        const at = this.time();
        w.priority = st.to;
        w.updatedAt = at;
        this.post({ kind: "priorityChanged", id: this.id("priority"), issue: ISSUES[st.i], priority: st.to, at });
        break;
      }
      case "blocker": {
        if (this.world.blocker.done === st.done) break;
        const at = this.time();
        this.world.blocker = { done: st.done, at };
        this.post({ kind: "issueStateChanged", id: this.id("blocker"), issue: BLOCKER, state: st.done ? "completed" : "open", parent: null, at });
        break;
      }
      case "pr":
        this.world.pr(st.i, st.act);
        break;
      case "land":
        this.world.land(st.i);
        break;
      // A push to the repo's default branch, whose webhook reaches the coordinator at once.
      case "base": {
        const base = this.world.pushBase(st.conflict);
        try {
          await repoPushed(this.storage, uuid(this.world.bases, "ba5e"), PR_REPO, base, this.nowMs, this.sender);
        } catch (error) {
          if (!(error instanceof Fault) && !(error instanceof Conflict)) throw error;
        }
        break;
      }
      // An orchestrating worker chains its sub-issues before it answers.
      case "work": {
        const w = this.world.issues[st.i];
        if (w.orchestrating?.end === false && w.agent === "working" && w.owes.sessions.length > 0) await this.chain(st.i);
        this.world.work(st.i);
        break;
      }
      case "child":
        this.world.move(st.act === "add");
        break;
      // As the relay asks it of a completed issue's webhook, again as it is redelivered.
      case "context": {
        const w = this.world.issues[st.i];
        if (w.state !== "completed") break;
        try {
          await contextAsked(this.storage, ISSUES[st.i], w.stateAt, this.nowMs, this.sender);
        } catch (error) {
          if (!(error instanceof Fault) && !(error instanceof Conflict)) throw error;
        }
        break;
      }
      case "line":
        this.world.issues[st.i].meets = st.meets;
        break;
      case "update":
        this.world.issues[st.i].update = st.meets;
        break;
      case "agent": {
        const w = this.world.issues[st.i];
        if (!w.worktree) break;
        w.agent = st.status === "gone" ? null : st.status;
        // What the agent took goes with its turn.
        if (st.status !== "working" && st.status !== "waiting") w.owes = { round: false, sessions: [] };
        if (st.status === "gone") w.worktree = false;
        const kind = ({ working: "agentWorking", background: "agentBackground", idle: "agentIdle", waiting: "agentWaiting", exited: "agentExited", gone: "worktreeGone" } as const)[st.status];
        this.post({ kind, id: this.id("obs"), issue: ISSUES[st.i] }, false);
        break;
      }
      case "worker": {
        const w = this.world.issues[st.i];
        const session = w.took;
        // Only a live agent reports through `tolkster`, in the session it was given.
        if (!w.worktree || w.agent === null || w.agent === "exited" || session === null) break;
        const at = this.time();
        if (st.act === "read") this.post({ kind: "workerRead", id: this.id("read"), issue: ISSUES[st.i], session, reader: "worker", through: at }, false);
        else this.post({ kind: "workerAnswered", id: this.id("answer"), issue: ISSUES[st.i], session, reader: "worker", at });
        break;
      }
      case "listen":
        this.listen(st.n, st.out);
        break;
      case "deliver": {
        if (this.pool.length === 0) break;
        const [e] = this.pool.splice(st.n % this.pool.length, 1);
        await this.deliverOne(e);
        break;
      }
      case "drop":
        if (this.pool.length > 0) this.pool.splice(st.n % this.pool.length, 1);
        break;
      case "race": {
        if (this.pool.length === 0) break;
        const [a] = this.pool.splice(st.n % this.pool.length, 1);
        const b = this.pool.length === 0 ? null : this.pool.splice(st.n % this.pool.length, 1)[0];
        // The intake checks a chain's own event and writes it in one put, which no delivery
        // runs inside.
        let ran = b === null;
        if (b !== null && !("event" in a && byChain(a.event))) this.storage.hook = { countdown: st.at, run: async () => { ran = true; await this.deliverChecked(b); } };
        await this.deliverChecked(a);
        this.storage.hook = null;
        if (!ran && b !== null) await this.deliverChecked(b);
        break;
      }
      case "again": {
        // Within the intake's 7 days, past which nothing redelivers.
        const clocked = this.applied.filter((e): e is Extract<Envelope, { event: Event }> => e.clocked && this.nowMs - e.atMs! < 7 * DAY);
        if (clocked.length === 0) break;
        const e = clocked[st.n % clocked.length];
        if (quiet(await this.storage.view(this.nowMs))) again = e.event;
        await this.deliverOne(e);
        break;
      }
      case "fault":
        this.storage.failNext = true;
        break;
      case "triage":
        await this.triage(st.act, st.i);
        break;
      case "triageListen": {
        if (this.triageTodo.length === 0) break;
        const [c] = this.triageTodo.splice(st.n % this.triageTodo.length, 1);
        if (st.out !== "none") this.postTriage(this.world.listenTriage(c, st.out));
        break;
      }
      // A fault waiting is the issues' steps', so a fleet's write never takes it.
      case "naps": {
        const fault = this.storage.failNext;
        this.storage.failNext = false;
        const named = (keys: string[], mask: number) => Object.fromEntries(keys.flatMap((key, i) => ((mask >> i) & 1 ? [[key, st.minutes]] : [])));
        const idle = named(NAP_HOSTS, st.idle);
        const dormant = st.dormant === null ? null : named(NAP_MACHINES, st.dormant);
        const kept = (memo: Record<string, { since: number; minutes: number }>, keys: string[], now: Record<string, number>) => {
          for (const key of keys) {
            if (!Object.hasOwn(now, key)) delete memo[key];
            else memo[key] = { since: memo[key]?.since ?? this.nowMs, minutes: st.minutes };
          }
        };
        // The listing pairs NAP_HOSTS[0], with room for one worker of the repo, answering at 30
        // minutes and silent otherwise, so a worker there is sometimes on a host open to none.
        // Each counts the workers whose agent is there, as a listing does.
        const held = (host: string | null) => this.world.issues.filter((w) => w.worktree && w.host === host && w.agent !== null && w.agent !== "exited").length;
        const paired = { id: NAP_HOSTS[0], name: NAP_HOSTS[0], cap: 1, admitted: true, answering: st.minutes === 30, ready: [PROJECT], held: held(NAP_HOSTS[0]) };
        try {
          await fleetReported(this.storage, { hosts: { held: held(null), hosts: [paired], projects: { [REPO]: PROJECT }, recipes: { [REPO]: RECIPE_REPO } } }, this.nowMs, this.sender);
          await fleetEvent(this.storage, { kind: "listed", idle, dormant, elapsedMs: 0 }, this.nowMs, this.sender);
          this.answering = paired.answering;
          kept(this.memo.naps, NAP_HOSTS, idle);
          if (dormant !== null) kept(this.memo.dormant, NAP_MACHINES, dormant);
        } catch (error) {
          if (!(error instanceof Fault) && !(error instanceof Conflict)) throw error;
        }
        this.storage.failNext = fault;
        break;
      }
      case "napsListen": {
        if (this.napsTodo.length === 0) break;
        const [c] = this.napsTodo.splice(st.n % this.napsTodo.length, 1);
        if (st.out === "none") break;
        const fault = this.storage.failNext;
        this.storage.failNext = false;
        try {
          const retryable = st.out === "retryable";
          await fleetEvent(this.storage, { kind: "commandResult", command: c.id, result: st.out, data: retryable ? { began: false, waitsOn: { lock: true } } : { began: st.out === "ok" && !(c.kind === "wakeHost" && c.version % 2 === 0) } }, this.nowMs, this.sender);
          if (!retryable) await waitsCleared(this.storage, { lock: true }, this.nowMs, this.sender);
        } catch (error) {
          if (!(error instanceof Fault) && !(error instanceof Conflict)) throw error;
        }
        this.storage.failNext = fault;
        break;
      }
      case "triageDown": {
        // A host that fails every start: one asked for, then each retry failed as it comes due.
        this.postTriage({ kind: "triageAsked", id: `ask-down-${this.world.triageIds}`, issue: ISSUES[0] });
        for (let failures = 0, rounds = 0; failures < TRIAGE_RETRIES && rounds < 4 * TRIAGE_RETRIES; rounds++) {
          for (const e of this.triagePending.splice(0)) await this.deliverOne(e);
          const at = this.triageTodo.findIndex((c) => c.kind === "startTriage");
          if (at === -1) {
            const due = Object.values((this.storage.data.get("machine:due") ?? {}) as Record<string, number>);
            await this.step({ k: "time", ms: Math.max(1000, (due.length > 0 ? Math.min(...due) : this.nowMs + DAY) - this.nowMs) });
            continue;
          }
          const [c] = this.triageTodo.splice(at, 1);
          this.postTriage(this.world.listenTriage(c, "failed"));
          failures += 1;
        }
        break;
      }
      case "time": {
        this.nowMs += st.ms;
        try {
          // A minute or more on, `listen` has listed, hearing the paired host while it answers,
          // before a command's timeout of two minutes or more.
          if (st.away !== true && st.ms >= 60_000 && this.answering) await waitsCleared(this.storage, { hosts: [NAP_HOSTS[0]] }, this.nowMs, this.sender);
          await reconcile(this.storage, this.nowMs, this.sender, st.away !== true);
        } catch (error) {
          if (!(error instanceof Fault) && !(error instanceof Conflict)) throw error;
        }
        // The relay posts each away message itself, an event waiting delivered while it does,
        // then hands the store its result.
        for (const post of this.posts.splice(0)) {
          const next = this.pool.shift();
          if (next !== undefined) await this.deliverOne(next);
          try {
            await deliver(this.storage, { kind: "commandResult", id: `result:${post.id}`, issue: post.issue, command: post.id, result: "ok" }, this.nowMs, this.sender);
          } catch (error) {
            if (!(error instanceof Fault) && !(error instanceof Conflict)) throw error;
          }
        }
        break;
      }
    }
    // What the world reported in this step goes onto the network.
    for (const { event, clocked } of this.world.events.splice(0)) this.post(event, clocked);
    // The relay reconciles the fleet after each report and event, as the alarm does: a fault or
    // a read's hook waiting is the issues' steps'.
    const [fault, hook] = [this.storage.failNext, this.storage.hook];
    [this.storage.failNext, this.storage.hook] = [false, null];
    await fleetReconciled(this.storage, this.nowMs, this.sender);
    [this.storage.failNext, this.storage.hook] = [fault, hook];
    return again;
  }

  async triage(act: "ask" | "end" | "connect", i: number): Promise<void> {
    this.world.triageIds += 1;
    if (act === "ask") return this.postTriage({ kind: "triageAsked", id: `ask-${this.world.triageIds}`, issue: ISSUES[i] });
    if (act === "connect") {
      try {
        // `listen` carries out a resend of a command it holds once: only the rest are new to it.
        this.sender((await connected(this.storage, this.nowMs, () => {})).filter((c) => ![...this.todo, ...this.triageTodo, ...this.napsTodo].some((t) => t.id === c.id)));
      } catch (error) {
        if (!(error instanceof Fault) && !(error instanceof Conflict)) throw error;
      }
      return;
    }
    // A session closes; the watcher says so only of the one this run opened last.
    const closed = this.world.sessions.shift();
    if (closed === undefined) return;
    if (this.world.watched?.id === closed.id) {
      this.world.watched = null;
      this.postTriage({ kind: "triageEnded", id: `ended-${this.world.triageIds}`, terminal: closed.terminal });
    }
  }

  // Delivered at once: the network's reordering and drops are the issues' steps', and a
  // triage event's redelivery is a no-op or a start, which the machine's own tests cover.
  postTriage(event: TriageEvent): void {
    this.triagePending.push({ triage: event, clocked: false });
  }

  // The next step to quiet, with no event from outside: `listen` connected, what the network
  // holds delivered, every command carried out `ok`, each agent acting on what it took, CI
  // ending on each head, and the clock moved to each deadline until QUIET_MS past the
  // world's last change, a second on for one past due, as the alarm runs again at once; null
  // once there.
  toQuiet(): Step | null {
    if (this.quietUntil === null) {
      this.quietUntil = this.nowMs + QUIET_MS;
      return { k: "triage", act: "connect", i: 0 };
    }
    if (this.pool.length > 0) return { k: "deliver", n: 0 };
    if (this.todo.length > 0) return { k: "listen", n: 0, out: "ok" };
    if (this.triageTodo.length > 0) return { k: "triageListen", n: 0, out: "ok" };
    if (this.napsTodo.length > 0) return { k: "napsListen", n: 0, out: "ok" };
    const issues = this.world.issues;
    const busy = issues.findIndex((w) => w.turns.some((t) => !t.answered) || (w.agent === "working" && (w.owes.round || w.owes.sessions.length > 0)));
    const ci = issues.findIndex((w) => w.pr?.status === "open" && w.pr.checks === "pending" && !stuck(w.pr));
    if (busy !== -1 || ci !== -1) this.quietUntil = Math.max(this.quietUntil, this.nowMs + QUIET_MS);
    if (busy !== -1) return { k: "work", i: busy };
    if (ci !== -1) return { k: "pr", i: ci, act: "checks" };
    if (this.nowMs >= this.quietUntil) return null;
    const due = Object.values((this.storage.data.get("machine:due") ?? {}) as Record<string, number>);
    const next = Math.min(this.quietUntil, due.length > 0 ? Math.min(...due) : this.nowMs + DAY);
    return { k: "time", ms: Math.max(1000, next - this.nowMs) };
  }

  // listen carries out a command against the world, as it is now, and reports its result
  // onto the network.
  listen(n: number, out: Outcome): void {
    if (this.todo.length === 0) return;
    const [c] = this.todo.splice(n % this.todo.length, 1);
    if (c.issue === TRIAGE) {
      if (out !== "none") this.postTriage(this.world.listenTriage(c, out));
      return;
    }
    if (out === "none" || !ISSUES.includes(c.issue)) return;
    this.world.listen(c, this.wires.get(c), out);
  }
}

// ---- Running and shrinking ----

// How long the run to quiet moves the clock past the world's last change: past each state's
// own deadline, a day at most, and the retries after it.
const QUIET_MS = DAY + 2 * 60 * 60_000;
// The most steps a run to quiet takes, a bound no seed comes near.
const QUIET_STEPS = 5000;

// `trace` sees each step's result, for reading a failure through. After its steps the run
// goes on to quiet, checked as they were, and then the world is checked against what the
// machines claim to deliver (AT_QUIET).
export async function run(steps: Step[], mutant: Mutant | null = null, trace?: (at: number, after: View, seen: Seen) => void): Promise<Failure | null> {
  const sim = new Sim(mutant);
  await sim.start();
  let before = await sim.storage.view(sim.nowMs);
  for (let at = 0; at < steps.length + QUIET_STEPS; at++) {
    const step = at < steps.length ? steps[at] : sim.toQuiet();
    if (step === null) break;
    const sentFrom = sim.sent.length;
    const appliedFrom = sim.applied.length;
    sim.storage.puts = [];
    sim.storage.retired = [];
    sim.storage.undue = [];
    let again: Event | null = null;
    try {
      again = await sim.step(step);
    } catch (error) {
      return { check: "noCrash", message: error instanceof Error ? error.stack ?? error.message : String(error), at, step };
    }
    const after = await sim.storage.view(sim.nowMs);
    const seen: Seen = { step, sent: sim.sent.slice(sentFrom), again, landed: sim.applied.slice(appliedFrom).flatMap((e) => ("event" in e ? [{ event: e.event, order: e.order! }] : [])), puts: sim.storage.puts, retired: sim.storage.retired, undue: sim.storage.undue };
    sim.memo.triage = { open: [...sim.world.sessions], opened: [...sim.world.opened] };
    trace?.(at, after, seen);
    for (const [name, check] of Object.entries(CHECKS)) {
      const message = check(before, after, seen, sim.memo);
      if (message !== null) return { check: name, message, at, step };
    }
    before = after;
  }
  for (const [name, check] of Object.entries(AT_QUIET)) {
    const message = check(sim.world, before);
    if (message !== null) return { check: name, message, at: steps.length, step: steps.at(-1) ?? { k: "fault" } };
  }
  return null;
}

// What listen carried out in a run of `steps`, as the model answered it.
export async function replays(steps: Step[]): Promise<Replay[]> {
  const sim = new Sim(null);
  sim.world.replays = [];
  await sim.start();
  for (const step of steps) await sim.step(step);
  return sim.world.replays;
}

// The shortest run that still fails the same check, by dropping chunks of steps, then
// single steps, while it does.
export async function shrink(steps: Step[], check: string, mutant: Mutant | null = null): Promise<Step[]> {
  let best = steps;
  const fails = async (s: Step[]) => (await run(s, mutant))?.check === check;
  for (let shorter = true; shorter; ) {
    shorter = false;
    for (let size = Math.max(1, Math.floor(best.length / 2)); size >= 1; size = Math.floor(size / 2)) {
      for (let i = 0; i < best.length; ) {
        const candidate = [...best.slice(0, i), ...best.slice(i + size)];
        if (candidate.length > 0 && (await fails(candidate))) {
          best = candidate;
          shorter = true;
        } else i += size;
      }
    }
  }
  // The steps the failure needs end at the failing one.
  const failure = await run(best, mutant);
  return failure === null ? best : best.slice(0, failure.at + 1);
}
