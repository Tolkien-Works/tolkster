import { DurableObject } from "cloudflare:workers";
import { describe, error, info, warn } from "./log.ts";
import { createActivity, type Delegated, IDENTIFIER as PLAIN_IDENTIFIER, readDelegated } from "../shared/linear.ts";
import { bringForward, earliest, runAll, type Timer } from "./alarm.ts";
import { AWAY, NOT_HELD } from "./decide.ts";
import { fromIssue as issueEvents, type Ingress } from "./events.ts";
import { contextAsked as askedContext, type IssuePayload, issueIds, triageAsked } from "./issue.ts";
import { branchWrites, identifierOf, intake, received, rememberIdentifier, sessionIssue, sessionWrites, threadIssue, threadWrites } from "./intake.ts";
import { fromHook, logged, newest, parseReport, refusedReport, type Report } from "./machine.ts";
import { kindOf } from "./machine/ctx.ts";
import { LOCAL } from "./machine/queue.ts";
import { acknowledgedAway, chained, chainHolds, connectAsked, connected, contextAsked, deliver, holds, keptAtReset as machinesKept, fleetEvent, fleetReconciled, loadCoordinator, loadStore, lostAtReset, machineTimer, openPrs, rebuildAsked, repoPushed, fleetReported, settle, staged, terminalClosed, triageEvent, TURN_TERMINAL, waitsCleared } from "./machine/store.ts";
import { FLEET, fleetData } from "./machine/fleet.ts";
import { TRIAGE } from "./machine/triage.ts";
import { parseWaitsOn } from "./machine/validate.ts";
import type { Command, Store } from "./machine/types.ts";
import { frameOf } from "./machine/frame.ts";
import { acceptedAfter, type BaseTip, bases, type Connection, listening as anyListening, moveBase, PING, PONG, publish, supersede } from "./realtime.ts";
import { parseTriageEvent, TERMINAL } from "./machine/validate.ts";
import { applyListing, keepHook, keepListing, keptListing, WORKTREE } from "./listing.ts";
import { fleetWire, fromReport, toEvent, toWire, triageWire } from "./route.ts";
import { enrolled, hostOf, hostOp, keptAtReset as hostsKept, redeem } from "./hosts.ts";
import { hostScope } from "./scope.ts";
import { chains, delegated, handle, ID, keptAtReset as recordsKept, type Outcome, recordSessionEvent, tolkstersIssues, WORKTREE as WORKTREE_ID } from "./state.ts";
import { chainSessions, oweRebuild, rebuildTimer } from "./rebuild.ts";
import { resetOnce, STORAGE_VERSION } from "./storage.ts";
import { forwardTiming } from "./timing.ts";
import { type LinearApp, withToken } from "./token.ts";
import { refusedSessions, tellRefused } from "./unanswered.ts";

// The owner's Durable Object: one instance, named after the owner. It holds the
// workstation's WebSocket through the Hibernation API, so an idle connection costs
// nothing. It runs the state machines (machine/): the Worker's events come in through the
// intake (intake.ts), `listen`'s results, observations and workers' reports come back as
// events, and the machines' commands go out to `listen` (route.ts). It also keeps the
// claim lock and Tolkster's other records (state.ts), and keeps each repo's base tip for
// `listen`'s update (realtime.ts). The logic lives in those modules; this only wires it
// to the runtime.

// What a socket carries through hibernation: when the relay accepted it.
// `print`: `tolkster listen --print`, which only prints, so it is never the newest.
type Attachment = { acceptedAt: number; print?: boolean };

const PRINT_HEADER = "X-Tolkster-Print";

const ALARM_AFTER_FAILURE_MS = 60 * 1000;

// A result, an observation, a listing or listen's settings, which a worker's own reports never come as.
const fromSocket = (report: Report): boolean => report.report === "commandResult" || report.report === "config" || report.report === "listing" || "sequence" in report;
const isPrint = (socket: WebSocket): boolean => (socket.deserializeAttachment() as Attachment | null)?.print === true;


export class Relay extends DurableObject<LinearApp> {
  constructor(ctx: DurableObjectState, env: LinearApp) {
    super(ctx, env);
    this.ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair(PING, PONG));
    // Before any event reads storage: one of another STORAGE_VERSION is reset, keeping only
    // the machines' counters, the context comments' records and the hosts' credentials, and
    // its alarm goes; the rebuild (rebuild.ts) is owed, and the alarm set for it.
    void ctx.blockConcurrencyWhile(async () => {
      const keep = (key: string, value: unknown) => machinesKept(key, value) ?? recordsKept(key, value) ?? hostsKept(key, value);
      const reset = await resetOnce(ctx.storage, keep, async () => {
        await ctx.storage.deleteAlarm();
        await oweRebuild(ctx.storage, Date.now(), true);
      }, (key, value) => {
        for (const lost of lostAtReset(key, value)) warn("storage_reset_dropped", lost);
      });
      if (!reset) return;
      warn("storage_reset", { version: STORAGE_VERSION });
      await ctx.storage.setAlarm(Date.now());
    });
  }

  // The workstation connecting. The Worker has already checked its secret. When it was
  // accepted rides on the socket, through hibernation, for `listening`. It is sent each
  // repo's base tip, and the newest connection every command in flight at once, with its ID,
  // rather than at the command's timeout. Once it is, every older connection but a `--print`
  // is closed as superseded, so only one `listen` acts. It owes the rebuild (rebuild.ts), so
  // a delegation whose webhook never landed is asked about by the next connect.
  async fetch(request: Request): Promise<Response> {
    const [client, server] = Object.values(new WebSocketPair());
    const print = request.headers.get(PRINT_HEADER) === "1";
    const acceptedAt = acceptedAfter(Date.now(), this.ctx.getWebSockets().map((socket) => this.connection(socket).acceptedAt));
    this.ctx.acceptWebSocket(server);
    server.serializeAttachment({ acceptedAt, print } satisfies Attachment);
    const tips = await bases(this.ctx.storage, server);
    const commands = print ? [] : await connected(this.ctx.storage, Date.now(), (issue, thrown) => warn("presence_unrecorded", { issueId: issue, error: describe(thrown) }));
    await this.dispatch(commands, server);
    await this.publish(server);
    if (!print) await oweRebuild(this.ctx.storage, Date.now(), false);
    if (!print) await this.schedule();
    const superseded = print ? 0 : supersede(this.ctx.getWebSockets().map((socket) => ({ socket, print: isPrint(socket), acceptedAt: this.connection(socket).acceptedAt })), acceptedAt);
    info("realtime_connected", { tips, commands: commands.length, connections: this.ctx.getWebSockets().length, superseded });
    return new Response(null, { status: 101, webSocket: client });
  }

  // An Issue webhook, reduced to its IDs (issue.ts, `issueFacts`): judged against what the
  // relay knows of Tolkster (state.ts, `tolkstersIssues`), then its machines' events
  // ingested, its triage ask made and its context asked of its store, in that order, with
  // the commands sent and the alarm brought forward once. Throws when one fails, skipping
  // those after it, for the Worker to answer with an error and Linear to send it again: the
  // intake drops a delivery it has seen, an ask already held starts nothing more, and a
  // context `listen` has started starts nothing more.
  async issue(payload: IssuePayload, delivery: string, ownerUserId: string): Promise<{ events: number; triage: boolean; context: boolean }> {
    const { appUserId, withSessions } = await tolkstersIssues(this.ctx.storage, issueIds(payload), Date.now());
    const events = issueEvents(payload, delivery, ownerUserId, appUserId);
    const context = askedContext(payload, appUserId === null ? null : { appUserId, withSessions });
    const asked = triageAsked(payload);
    await this.running(async (pending) => {
      if (events.length > 0) {
        await this.remember(events[0].issueId, payload.data?.identifier);
        await this.intakeEach(events, pending);
      }
      if (asked !== null) await this.askTriage(asked, delivery || `create:${asked}`, pending);
      if (context !== null) {
        await this.remember(context.issueId, payload.data?.identifier);
        await contextAsked(this.ctx.storage, context.issueId, context.at, Date.now(), (c) => pending.push(...c));
        info("event_received", { kind: "contextAsked", deliveryId: delivery, issueId: context.issueId });
      }
    });
    return { events: events.length, triage: asked !== null, context: context !== null };
  }

  // The GitHub App's webhook, mapped (events.ts, `fromGitHub` and `fromPush`): its events
  // ingested, then the push's base tip kept. Throws as `issue` does.
  async github(events: Ingress[], push: BaseTip | null, pushedAt: number): Promise<void> {
    if (events.length > 0) await this.ingest(events, null);
    if (push !== null) {
      const sent = await moveBase(this.ctx.storage, this.ctx.getWebSockets(), push, pushedAt);
      info("base_moved", { repo: push.repo, after: push.after, sent });
    }
  }

  // An issue created in Triage, for the triage machine (machine/triage.ts). A redelivery
  // needs no dedupe: an issue asked about already, or while a session runs, starts nothing
  // more.
  private async askTriage(issueId: string, delivery: string, pending: Command[]): Promise<void> {
    await triageEvent(this.ctx.storage, parseTriageEvent({ id: delivery, kind: "triageAsked", issue: issueId }), Date.now(), (c) => pending.push(...c));
    info("event_received", { kind: "triageAsked", deliveryId: delivery, issueId });
  }

  // The Worker's events (events.ts) into the machines, each through the intake, which maps
  // it to its issues and dedupes it: the delivery and the transition it runs land in one
  // write. Throws when one fails, for the Worker to answer with an error. `identifier`, the
  // issue's as the webhook gave it, is kept for its commands.
  async ingest(events: Ingress[], identifier: { issueId: string; identifier: string } | null): Promise<void> {
    if (identifier !== null) await this.remember(identifier.issueId, identifier.identifier);
    // Told even when the ingest threw after the intake refused, as the redelivery finds the event
    // seen. Once and best effort: an eviction before the post leaves it unsaid.
    const refused: string[] = [];
    try {
      await this.running((pending) => this.intakeEach(events, pending, refused));
    } finally {
      await tellRefused(refused, (session) => withToken(this.env, session, "refusal_failed", (token) => createActivity(token, { agentSessionId: session, content: { type: "response", body: NOT_HELD } })));
    }
  }

  private async remember(issueId: string, identifier: unknown): Promise<void> {
    if (typeof identifier === "string" && PLAIN_IDENTIFIER.test(identifier)) await rememberIdentifier(this.ctx.storage, issueId, identifier, Date.now());
  }

  // Runs `work` on the machines, then sends what it left to send and brings the alarm
  // forward, even when it threw part way.
  private async running(work: (pending: Command[]) => Promise<void>): Promise<void> {
    const pending: Command[] = [];
    try {
      await work(pending);
    } finally {
      await this.reconcileFleet(Date.now(), (c) => pending.push(...c));
      await this.dispatch(pending);
      await this.publish();
      await this.scheduleMachine();
    }
  }

  // `refused` collects the sessions whose chain no longer holds.
  private async intakeEach(events: Ingress[], pending: Command[], refused: string[] = []): Promise<void> {
    for (const event of events) {
      const nowMs = Date.now();
      // A push reaches each issue with a PR open in the repo through the coordinator's own
      // index, as `basePushed`, which only a store or the coordinator makes; a base already
      // held is stale, so a redelivery changes nothing.
      if (event.kind === "repoPushed") {
        await repoPushed(this.ctx.storage, event.deliveryId, event.repo, event.sha, nowMs, (c) => pending.push(...c));
        info("event_received", { kind: event.kind, deliveryId: event.deliveryId });
        continue;
      }
      const staging: { issue: string; send: Command[] }[] = [];
      const results = await intake(this.ctx.storage, event, nowMs, (issue) => holds(this.ctx.storage, issue), (repo) => openPrs(this.ctx.storage, repo, nowMs), async (accepted) => {
        const step = await staged(this.ctx.storage, toEvent(accepted), nowMs);
        staging.push(step);
        return step.writes;
      }, (issue) => chainHolds(this.ctx.storage, issue));
      refused.push(...refusedSessions(results));
      for (const step of staging) await settle(this.ctx.storage, step.issue, step.send, nowMs, (c) => pending.push(...c));
      for (const result of results) info("event_received", received(result));
    }
  }

  // The Worker's acknowledgement of the owner's request at `at` was the away message, sent
  // beside its event's ingest: the request's away notice is recorded, so its `quiet` deadline
  // doesn't say it again. A request not stored (yet, or no longer the session's newest) is
  // left alone.
  async acknowledgedAway(issueId: string, sessionId: string, at: string): Promise<void> {
    const pending: Command[] = [];
    await acknowledgedAway(this.ctx.storage, issueId, sessionId, at, Date.now(), (c) => pending.push(...c));
    await this.dispatch(pending);
    await this.publish();
  }

  // Whether `tolkster listen` holds a live connection (realtime.ts, `listening`). Read
  // from the sockets alone, without waking them or touching storage, so it costs the
  // Worker's acknowledgement next to nothing.
  async listening(): Promise<boolean> {
    return anyListening(this.ctx.getWebSockets().map((socket) => this.connection(socket)), Date.now());
  }

  private connection(socket: WebSocket): Connection {
    const { acceptedAt } = socket.deserializeAttachment() as Attachment;
    return { pingedAt: this.ctx.getWebSocketAutoResponseTimestamp(socket)?.getTime() ?? null, acceptedAt };
  }

  // The host a worker host's credential was issued to, or null (hosts.ts): what /token
  // checks before it hands anything out.
  async host(credential: string): Promise<string | null> {
    return await hostOf(this.ctx.storage, credential);
  }

  // Whether `code` is live, for GET /build (hosts.ts, `enrolled`).
  async enrolled(code: string): Promise<boolean> {
    return await enrolled(this.ctx.storage, code, Date.now());
  }

  // What an enrolment code trades for at POST /enrol, once (hosts.ts, `redeem`).
  async enrol(code: string): Promise<{ host: string; credential: string; config: string } | null> {
    return await redeem(this.ctx.storage, code, Date.now());
  }

  // POST /state, from `tolkster`: the owner's, whose realtime secret the Worker has already
  // checked, with `credential` null, or a worker host's, with the credential it carried,
  // checked here, in the same call as the op, so a revoked one does nothing.
  async state(body: unknown, credential: string | null): Promise<Outcome> {
    const host = credential === null ? null : await hostOf(this.ctx.storage, credential);
    if (credential !== null && host === null) return { status: 401, body: JSON.stringify({ error: "unauthorized" }), log: { reason: "unauthorized" } };
    const outcome = await this.stateAs(body, host);
    return host === null ? outcome : { ...outcome, log: { ...outcome.log, host } };
  }

  // `host` is the worker host calling, or null for the owner's own call.
  private async stateAs(body: unknown, host: string | null): Promise<Outcome> {
    const hosts = await hostOp(this.ctx.storage, body, host === null, Date.now());
    if (hosts !== null) return hosts;
    const scoped = host === null ? null : await hostScope(this.ctx.storage, host, body, Date.now());
    if (scoped !== null) return scoped;
    // A session's timing, which the machines never see: only `listen` keeps it (timing.ts).
    if ((body as { report?: unknown } | null)?.report === "sessionTiming") return await forwardTiming(this.ctx.storage, this.listener(), body);
    // A worker's own `tolkster` report (machine.ts), which the machines take as an event.
    if ((body as { op?: unknown } | null)?.op === "report") {
      const report = parseReport(body);
      if (report === null || fromSocket(report)) return { status: 400, body: JSON.stringify({ error: "report" }), log: { reason: "report" } };
      if (fromHook(report)) {
        if (!(await keepHook(this.ctx.storage, report, Date.now()))) return { status: 200, body: "{}", log: { ...logged(report), stale: true } };
      }
      // A hook's resend would be stale by the time `keepHook` kept: the listing is its backstop.
      if (!(await this.received(report, 0)) && !fromHook(report)) return refusedReport(report);
      // `issue chain` delegates only the sub-issues its answer names: those the parent's
      // store took for the run. `taken: null`, unanswered, delegates none and is an error,
      // never an answer that every sub-issue refused (docs/state-machine.md, Parent issues).
      const answer = report.report === "chainRecorded" ? JSON.stringify({ taken: await chained(this.ctx.storage, report.issueId, report.run) }) : "{}";
      return { status: 200, body: answer, log: logged(report) };
    }
    // What a session's request machine holds of its reader, for `tolkster messages`.
    if ((body as { op?: unknown } | null)?.op === "session") {
      const sessionId = (body as { sessionId?: unknown }).sessionId;
      if (typeof sessionId !== "string" || !ID.test(sessionId)) return { status: 400, body: JSON.stringify({ error: "invalid sessionId" }), log: { reason: "invalid sessionId" } };
      const reading = await this.reading(sessionId);
      return { status: 200, body: JSON.stringify(reading), log: { op: "session", sessionId, found: reading.readThrough !== null || reading.turnReadThrough !== null || reading.answeredThrough !== null } };
    }
    return await handle(this.ctx.storage, body, Date.now());
  }

  // The issue a session is on: a work session's, or the one a note's thread stands for.
  private async issueOfSession(id: string): Promise<string | null> {
    return (await sessionIssue(this.ctx.storage, id)) ?? await threadIssue(this.ctx.storage, id);
  }

  // The owner's newest request the session's worker, and its turn, have read, and the newest
  // answered, as its request machine holds them; null for a session the Durable Object doesn't know.
  private async reading(sessionId: string): Promise<{ readThrough: string | null; turnReadThrough: string | null; answeredThrough: string | null }> {
    const issue = await this.issueOfSession(sessionId);
    const facts = issue === null ? undefined : (await loadStore(this.ctx.storage, issue)).sessions[sessionId]?.facts;
    return { readThrough: facts?.readThrough ?? null, turnReadThrough: facts?.turnReadThrough ?? null, answeredThrough: facts?.answeredThrough ?? null };
  }

  // A signed session event: Tolkster's app user ID and the session's issue recorded, for
  // Issue webhooks to be judged against (state.ts, `recordSessionEvent`), and, when `ask`,
  // whether Tolkster delegated the issue to itself, as a sub-issue of one it works through:
  // the Worker then acknowledges the session Linear starts for it (index.ts). A record that
  // fails costs only the filter's knowledge, and never the answer: an issue it didn't record
  // is judged by its payload alone.
  async sessionEvent(appUserId: string | null, issueId: string | null, ask: boolean): Promise<boolean> {
    if (appUserId !== null) {
      try {
        await recordSessionEvent(this.ctx.storage, appUserId, issueId, Date.now());
      } catch (thrown) {
        error("session_event_record_failed", { issueId, error: describe(thrown) });
      }
    }
    return ask && issueId !== null && (await delegated(this.ctx.storage, issueId, Date.now())) !== null;
  }

  async webSocketError(_socket: WebSocket, thrown: unknown): Promise<void> {
    warn("realtime_error", { error: describe(thrown) });
  }

  async webSocketMessage(socket: WebSocket, message: string | ArrayBuffer): Promise<void> {
    if (typeof message !== "string") return;
    const report = this.socketReport(message);
    if (report !== null) {
      // Only the newest connection's results and observations count: an older one's
      // command was sent again to the newest, and its listing is superseded.
      if (this.listener() !== socket) warn("report_dropped", logged(report));
      else await this.received(report, this.connection(socket).acceptedAt);
      return;
    }
    warn("realtime_message_ignored", { length: message.length });
  }

  // The connection commands go to, and the only one whose results and observations count:
  // the newest accepted, never a `--print`.
  private listener(): WebSocket | null {
    return newest(this.ctx.getWebSockets().filter((s) => !isPrint(s)).map((s) => ({ socket: s, acceptedAt: this.connection(s).acceptedAt })));
  }

  private socketReport(message: string): Report | null {
    let parsed: unknown;
    try {
      parsed = JSON.parse(message);
    } catch {
      return null;
    }
    const report = parseReport(parsed);
    return report !== null && fromSocket(report) ? report : null;
  }

  // Each report as its event (docs/state-machine.md, Events). `connection` is when the
  // socket it came on was accepted, which with its sequence names an observation. One that
  // names no issue the Durable Object knows is logged and dropped. One whose event the store
  // fails to take is logged, and false, for a worker's report to be sent again
  // (`refusedReport`); one taken is true whatever fails after, so a resend never runs it twice.
  private async received(report: Report, connection: number): Promise<boolean> {
    const nowMs = Date.now();
    const storage = this.ctx.storage;
    const pending: Command[] = [];
    const sender = (c: Command[]) => pending.push(...c);
    let landed = false;
    const taken = async () => {
      landed = true;
    };
    try {
      // The Durable Object's own posts are answered by the relay alone, never by `listen`.
      if (report.report === "commandResult" && report.issueId !== TRIAGE && report.issueId !== FLEET && (await loadStore(storage, report.issueId)).queue.some((c) => c.id === report.id && LOCAL.includes(c.kind))) {
        warn("report_refused", { ...logged(report), error: "local" });
        return true;
      }
      if (report.report === "config") {
        const { report: _, ...settings } = report;
        await fleetReported(storage, { settings }, nowMs, sender);
        await waitsCleared(storage, { all: true }, nowMs, sender);
      } else if (report.report === "listing") {
        if (!(await keepListing(storage, report, connection, nowMs, sender))) info("listing_stale", logged(report));
        else {
          // Only a complete listing says a host holds nothing.
          if (report.fleet !== null) await fleetReported(storage, { hosts: report.fleet }, nowMs, sender);
          if (report.listed !== null) await fleetEvent(storage, { kind: "listed", idle: report.idle, dormant: report.dormant, elapsedMs: report.elapsedMs }, nowMs, sender);
          // `listen` sends no listing whose own Orca didn't answer.
          await waitsCleared(storage, { hosts: report.hosts, orca: true }, nowMs, sender);
        }
      } else if (report.report === "terminalClosed") {
        await terminalClosed(storage, report.terminal, `${connection}:${report.sequence}`, nowMs, sender);
      } else if (report.report === "commandResult" && report.issueId === TRIAGE) {
        const event = { id: `result:${report.id}`, kind: "commandResult", command: report.id, result: report.result, data: report.detail };
        await triageEvent(storage, parseTriageEvent(event), nowMs, sender);
      } else if (report.report === "commandResult" && report.issueId === FLEET) {
        const waitsOn = report.detail.waitsOn === undefined ? undefined : parseWaitsOn(report.detail.waitsOn);
        await fleetEvent(storage, { kind: "commandResult", command: report.id, result: report.result, data: fleetData(report.detail, waitsOn) }, nowMs, sender);
        // A machine command that ran has ended, and with it the lock it held.
        if (report.result !== "retryable") await waitsCleared(storage, { lock: true }, nowMs, sender);
      } else {
        const event = await fromReport(report, {
          sessionIssue: (id) => this.issueOfSession(id),
          worktreeIssue: async (id) => (await storage.get<string>(WORKTREE + id)) ?? null,
          connection,
        });
        if (event === null) {
          warn("report_unmapped", logged(report));
          return true;
        }
        // What a result teaches lands in its step's own put: nothing sends a result again once
        // its command is no longer in flight. A result the store drops still teaches it, as a
        // start that went `lost` may have launched a worker whose reports need its worktree;
        // a superseded start's names its own worktree's branch, which is recorded only while the
        // store holds no other worktree.
        const also = report.report === "commandResult" ? (after: Store) => this.learned(report, nowMs, after) : undefined;
        await deliver(storage, event, nowMs, sender, taken, also);
        // The kept listing, which may have come before the start that names its worktree
        // (TW-486), is applied to it once the index has it.
        const started = report.report === "commandResult" ? startedIn(report) : null;
        const kept = started === null ? null : await keptListing(storage);
        if (started !== null && kept !== null) await applyListing(storage, kept, started.id, event.issue as string, nowMs, sender);
        if (report.report === "commandResult" && report.result === "retryable") await heardSince(storage, event.issue as string, report.id, nowMs, sender);
        // A worker's command waiting on a host holds it awake, and wakes it should it sleep.
        const waitsOn = report.report === "commandResult" && report.result === "retryable" && report.detail.waitsOn !== undefined ? parseWaitsOn(report.detail.waitsOn) : null;
        if (waitsOn !== null && "host" in waitsOn) await fleetEvent(storage, { kind: "wanted", host: waitsOn.host }, nowMs, sender);
      }
      landed = true;
      info("report_received", logged(report));
    } catch (thrown) {
      warn("report_refused", { ...logged(report), error: describe(thrown) });
    }
    await this.reconcileFleet(nowMs, sender);
    await this.dispatch(pending);
    await this.publish();
    // What the event armed, a deadline or a command's timeout, is due on the alarm.
    await this.scheduleMachine();
    return landed;
  }

  // What a command's result teaches the Durable Object beyond the machine, as writes for the
  // put that lands the machine's step on it, so neither lands without the other: a started
  // worker's worktree and branch, which its observations and its PR's events are mapped
  // by, and a note's thread, which its turn's reports come through. A branch the intake
  // refuses is logged, never failing the result it came with; one from a worktree other
  // than the store's live one is left unrecorded, as it would unmap the live worker's PR.
  private async learned(report: Extract<Report, { report: "commandResult" }>, nowMs: number, after: Store): Promise<Record<string, unknown>> {
    const writes: Record<string, unknown> = {};
    if (report.result !== "ok") return writes;
    const detail = report.detail as { comment?: { root?: unknown } };
    const worktree = startedIn(report);
    if (worktree !== null) {
      writes[WORKTREE + worktree.id] = report.issueId;
      const live = after.facts.worktree;
      if ((live === null || live.id === worktree.id) && typeof worktree.repo === "string" && typeof worktree.branch === "string") {
        try {
          Object.assign(writes, await branchWrites(this.ctx.storage, report.issueId, worktree.repo, worktree.branch, nowMs));
        } catch (thrown) {
          warn("branch_unrecorded", { commandId: report.id, issueId: report.issueId, error: describe(thrown) });
        }
      }
    }
    if (typeof detail.comment?.root === "string" && ID.test(detail.comment.root)) {
      await threadWrites(this.ctx.storage, report.issueId, detail.comment.root, nowMs, writes);
    }
    // A chain's session with a machine that the intake doesn't map, as after a reset: listen's
    // read lists it on this sub-issue, and its chain, taken back from Tolkster's own
    // delegation records, holds.
    if ((report.detail as { read?: unknown }).read !== undefined) {
      for (const id of chainSessions(after)) if ((await sessionIssue(this.ctx.storage, id)) === null) await sessionWrites(this.ctx.storage, report.issueId, id, nowMs, writes);
    }
    // Only a `startTurn`'s result names a terminal.
    const terminal = (report.detail as { terminal?: unknown }).terminal;
    if (typeof terminal === "string" && TERMINAL.test(terminal)) writes[TURN_TERMINAL + terminal] = report.issueId;
    return writes;
  }

  // Sends each command, with its args read from its store as it is now, to the newest
  // connection, or to `to`. Sent to none, it waits in its store for its timeout or the next
  // connection (`fetch`).
  private async dispatch(commands: Command[], to: WebSocket | null = this.listener()): Promise<void> {
    if (commands.length === 0) return;
    for (const c of commands) {
      if (c.kind === "postAway") {
        await this.postAway(c);
        continue;
      }
      if (c.issue === TRIAGE) {
        this.sendTo(to, c, () => triageWire(c));
        continue;
      }
      if (c.issue === FLEET) {
        this.sendTo(to, c, () => fleetWire(c));
        continue;
      }
      const s = await loadStore(this.ctx.storage, c.issue);
      const identifier = await identifierOf(this.ctx.storage, c.issue);
      if (identifier === null) {
        // Every machine begins with a Linear event naming its issue, so this is one whose
        // record was lost: its timeouts end it.
        warn("command_unsent", { commandId: c.id, issueId: c.issue, kind: c.kind, reason: "identifier" });
        continue;
      }
      this.sendTo(to, c, () => toWire(c, s, identifier));
    }
  }

  // The fleet's reconcile after an event (machine/store.ts, `fleetReconciled`): one that fails is
  // logged, and the next event or the alarm reconciles again.
  private async reconcileFleet(nowMs: number, sender: (c: Command[]) => void): Promise<void> {
    try {
      await fleetReconciled(this.ctx.storage, nowMs, sender);
    } catch (thrown) {
      warn("fleet_unreconciled", { error: describe(thrown) });
    }
  }

  // The coordinator's frame (machine/frame.ts) to every connection when it changed, and to
  // `fresh`, one just accepted, whatever it is. One that fails to build changes nothing the
  // machines hold: the next change sends it.
  private async publish(fresh: WebSocket | null = null): Promise<void> {
    try {
      const c = await loadCoordinator(this.ctx.storage);
      await publish(this.ctx.storage, this.ctx.getWebSockets(), await frameOf(this.ctx.storage, c), fresh);
    } catch (thrown) {
      warn("frame_unsent", { error: describe(thrown) });
    }
  }

  private sendTo(to: WebSocket | null, c: Command, wire: () => unknown): void {
    let sent = false;
    if (to !== null) {
      try {
        to.send(JSON.stringify(wire()));
        sent = true;
      } catch {
        // A socket that has just died: the next connection is sent it.
      }
    }
    (sent ? info : warn)("command_sent", { commandId: c.id, issueId: c.issue, kind: c.kind, urgent: c.class1, sent });
  }

  // What runs on the alarm (alarm.ts): the machines' own deadlines, timeouts and away
  // message (machine/store.ts, `machineTimer`).
  private timers(): Timer[] {
    return [this.machineEntry(), this.rebuildEntry()];
  }

  // The rebuild a reset or a connect owes (rebuild.ts): Linear read as the app. After a reset,
  // each parent is given the chain Tolkster's delegation records name, of its sub-issues Linear
  // still shows delegated; storage kept holds its chains. Any other issue is asked only while
  // no worker has had it, as one that has may have a worker still running, or a PR its owner
  // reviews.
  private rebuildEntry(): Timer {
    let byParent = new Map<string, { session: string; children: string[] }>();
    const read = async (reset: boolean): Promise<Delegated[] | null> => {
      let found: Delegated[] | null = null;
      await withToken(this.env, "rebuild", "rebuild_unread", async (token) => void (found = await readDelegated(token)));
      if (found === null || !reset) return found;
      const delegatedNow = new Set((found as Delegated[]).map((d) => d.issueId));
      byParent = new Map([...(await chains(this.ctx.storage, Date.now()))].map(([parent, c]) => [parent, { session: c.session, children: c.children.filter((child) => delegatedNow.has(child)) }]));
      return found;
    };
    return rebuildTimer(this.ctx.storage, read, async ({ issueId, identifier, started }, nowMs, reset) => {
      const chain = reset ? (byParent.get(issueId) ?? null) : null;
      if (started && chain === null) return;
      const pending: Command[] = [];
      await this.remember(issueId, identifier);
      const sender = (c: Command[]) => void pending.push(...c);
      try {
        if (reset) await rebuildAsked(this.ctx.storage, issueId, chain, nowMs, sender);
        else await connectAsked(this.ctx.storage, issueId, nowMs, sender);
      } finally {
        await this.dispatch(pending);
      }
    });
  }

  private machineEntry(): Timer {
    const pending: Command[] = [];
    const live = () => anyListening(this.ctx.getWebSockets().filter((s) => !isPrint(s)).map((s) => this.connection(s)), Date.now());
    const machine = machineTimer(this.ctx.storage, (c) => pending.push(...c), live);
    return {
      next: () => machine.next(),
      run: async (nowMs) => {
        try {
          await machine.run(nowMs);
        } finally {
          await this.dispatch(pending.splice(0));
        }
      },
    };
  }

  // The away message (`postAway`), posted by the Durable Object itself, as `listen` is the
  // one missing, in the session's own thread: a note's thread has no session to say it in,
  // and counts as said. Its outcome goes back to the store as the command's result, so a
  // failed post goes once more, then clears its notice; should that delivery fail, the
  // command's timeout posts it again.
  private async postAway(c: Command): Promise<void> {
    const session = c.machine;
    const s = await loadStore(this.ctx.storage, c.issue);
    // A resend's, once its result landed after all, or once `listen` has connected, says
    // nothing: the latter fails, clearing its key for a later absence.
    if (!s.queue.some((q) => q.id === c.id)) return;
    const said = s.away && (kindOf(s, session) === "note" || (await withToken(this.env, session, "away_failed", (token) => createActivity(token, { agentSessionId: session, content: { type: "response", body: AWAY } }))));
    if (s.away) (said ? info : warn)("away_said", { sessionId: session, issueId: c.issue, said });
    const pending: Command[] = [];
    try {
      await deliver(this.ctx.storage, { id: `result:${c.id}`, kind: "commandResult", issue: c.issue, command: c.id, result: said ? "ok" : "failed" }, Date.now(), (more) => pending.push(...more));
    } catch (thrown) {
      warn("away_unrecorded", { commandId: c.id, issueId: c.issue, error: describe(thrown) });
    }
    await this.dispatch(pending);
  }

  // Set again even when a timer throws, so what is due isn't stranded once the
  // runtime's retries of the alarm run out.
  // After a timer threw, what it left due waits ALARM_AFTER_FAILURE_MS, so a timer that
  // keeps throwing can't fire the alarm in a tight loop.
  async alarm(): Promise<void> {
    try {
      await runAll(this.timers(), Date.now());
    } catch (thrown) {
      error("alarm_failed", { error: describe(thrown) });
      // What the timers before the throw changed is sent all the same.
      await this.publish();
      await this.schedule(Date.now() + ALARM_AFTER_FAILURE_MS);
      return;
    }
    await this.publish();
    await this.schedule();
  }

  // Sets the alarm to the earliest thing due, no sooner than `notBefore`, or clears it
  // when nothing is.
  private async schedule(notBefore = 0): Promise<void> {
    const at = await earliest(this.timers());
    if (at === null) await this.ctx.storage.deleteAlarm();
    else await this.ctx.storage.setAlarm(Math.max(at, notBefore));
  }

  // After an event that changed only the machines' rows (a report, an ingested event, a
  // triage ask): lists nothing, and only brings the alarm forward.
  private async scheduleMachine(): Promise<void> {
    const at = await bringForward(this.machineEntry(), await this.ctx.storage.getAlarm());
    if (at !== null) await this.ctx.storage.setAlarm(at);
  }

  async webSocketClose(socket: WebSocket, code: number, _reason: string, wasClean: boolean): Promise<void> {
    info("realtime_closed", { code, wasClean });
    // Completes the closing handshake. The runtime may already have done so.
    try {
      socket.close(code === 1005 || code === 1006 ? 1000 : code, "Closing");
    } catch {
      // Already closed.
    }
  }
}

// A listing begun since the command was sent may have heard the host or Orca it now waits on
// before its result came: that clears the wait on its own issue, as the next listing would.
async function heardSince(storage: DurableObjectStorage, issue: string, id: string, nowMs: number, sender: (c: Command[]) => void): Promise<void> {
  const c = (await loadStore(storage, issue)).queue.find((q) => q.id === id);
  if (c?.sentMs == null || c.waitsOn == null || "lock" in c.waitsOn) return;
  const kept = await keptListing(storage);
  if (kept === null || kept.began < c.sentMs) return;
  if ("orca" in c.waitsOn) await waitsCleared(storage, { orca: true }, nowMs, sender, [issue]);
  else if (kept.listing.hosts.includes(c.waitsOn.host)) await waitsCleared(storage, { hosts: [c.waitsOn.host] }, nowMs, sender, [issue]);
}

// The worktree an ok result names, by a valid ID, with its branch and repo as they came.
function startedIn(report: Extract<Report, { report: "commandResult" }>): { id: string; branch?: unknown; repo?: unknown } | null {
  if (report.result !== "ok") return null;
  const worktree = (report.detail as { worktree?: { id?: unknown; branch?: unknown; repo?: unknown } | null }).worktree;
  return worktree && typeof worktree.id === "string" && WORKTREE_ID.test(worktree.id) ? { ...worktree, id: worktree.id } : null;
}
