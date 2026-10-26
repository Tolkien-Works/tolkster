// The simulator's world (machine-sim.ts): the issues, PRs, agents and triage sessions that
// `listen` and the webhooks report, and the fake `listen` that carries out each command
// against them.

import type { Recipe } from "../src/worker/machine/fleet.ts";
import { type TriageEvent, type TriageSession } from "../src/worker/machine/triage.ts";
import { type AgentStatus, type Checks, type Command, type CommandResult, type CommentKind, type DelegatedBy, type Event, type IssueStateType, type PrRead, type ResultData, UNKNOWN_RECHECKS } from "../src/worker/machine/types.ts";
import { type Command as Wire, type Result } from "../src/worker/machine.ts";
import { CHILD, type Meets, NAP_HOSTS, type Outcome, PARENT, type PrAct } from "./machine-sim-steps.ts";

export const ISSUES = ["11111111-1111-4111-8111-111111111111", "22222222-2222-4222-8222-222222222222"];
// The blocker of the first issue.
export const BLOCKER = "33333333-3333-4333-8333-333333333333";
// The simulator's clock starts here, and Linear's with it.
export const START_MS = 1_800_000_000_000;
export const REPO = "repo";
// The repo's project, as a listing's fleet names it.
export const PROJECT = "github:owner/repo";
// The repo's recipe, as the config report says it, by the name the settings give the repo: of
// its three machines the paired host is one, awake, and another is asleep, so a worker with
// nowhere to go waits for the first, wakes the second, or, while a wake is held back, asks for
// the third to be made. The asleep one runs an older revision than the recipe's, which no report
// ever moves, so it is updated again after each UPDATE_RETRY_MS.
export const RECIPE: Recipe = { id: "recipe", machines: 3, slotsEach: 1, idleMinutes: 30, destroyAfter: 60, revision: "2" };
export const RECIPE_REPO = "repo-name";
// A PR's repo is GitHub's `owner/name`, where a read names the repo by listen's setting.
export const PR_REPO = "owner/repo";
const BASE = "b".repeat(40);

// `base` is the base the head was last brought up to, `conflict` whether the base has moved
// in a way only a worker can merge, and `fix` whether the head is a worker's own push, which
// passes its checks; `answeredAt`, the newest feedback a worker's round has acted on;
// `commentedHead`, the head Copilot's review only commented on; `maxRounds`, the repo's cap on
// its rounds; `unknown`, how many reads GitHub answers before it has computed the PR's
// mergeability since the base last moved; `refused`, GitHub refused its auto-merge as mergeable
// already, and its worker asked for the merge instead.
type Pr = {
  number: number; head: string; status: "open" | "merged" | "closed"; at: string; checks: Checks | "pending"; feedbackAt: string | null; reviewerAt: string | null; autoMergeOff: boolean; refused: boolean;
  base: string; conflict: boolean; fix: boolean; answeredAt: string | null; commentedHead: string | null; maxRounds: number; unknown: number;
};
export type IssueWorld = {
  delegate: DelegatedBy;
  state: IssueStateType;
  stateAt: string;
  updatedAt: string;
  priority: number;
  // A chain's session is the one Linear opens on a sub-issue Tolkster's chain delegated.
  sessions: { id: string; kind: "delegation" | "mention" | "chain"; createdAt: string }[];
  newestComment: string | null;
  pr: Pr | null;
  // The commit the worker landed on the default branch with no PR, until a new worker's start.
  landed: string | null;
  agent: AgentStatus | null;
  worktree: boolean;
  // The paired host its worktree is on, null for this machine.
  host: string | null;
  // When it was cancelled, by the simulator's clock, while it is.
  cancelledMs: number | null;
  // What the agent took and has yet to act on: a round's line, and the sessions whose request
  // it was given. It goes with the agent's turn.
  owes: { round: boolean; sessions: string[] };
  // The session the agent was last given, the one its own reports are in.
  took: string | null;
  // Each session's open conversation turn, its terminal, the request it was handed, and
  // whether it has answered what it was given.
  turns: { session: string; terminal: string; request: unknown; answered: boolean }[];
  // What the issue's next line or stop meets in a live agent, once, and the issue's next branch
  // update, on whichever PR it is then: a push landing first, a conflict its read didn't show,
  // or GitHub failing it.
  meets: Meets | null;
  update: "moved" | "refused" | "failed" | null;
  // When the owner last stopped, undelegated or cancelled it: work asked before then is no
  // longer wanted.
  stoppedAt: string | null;
  // The session the issue's orchestrating worker reports in, from its start until a start that
  // doesn't orchestrate, and whether its sub-issues' end is still to do, when it moves its
  // issue to Done rather than chain them.
  orchestrating: { session: string; end: boolean } | null;
  // Whether Tolkster's record says its parent's chain delegated it, which is never cleared.
  chained: boolean;
  // No history entry changed its delegate, as for an issue the owner created delegated, so a
  // read says `created`, or `unknown` once an entry took it from Tolkster (`taken`).
  unnamed: boolean;
  taken: boolean;
  // What the next read shows instead, of a delegation at `at` Linear doesn't show yet, while
  // nothing has changed since.
  lag: { delegate: DelegatedBy; updatedAt: string; at: string } | null;
};

export function uuid(n: number, tag: string): string {
  return `${n.toString(16).padStart(8, "0")}-${tag}-4000-8000-000000000000`;
}

export function identifier(i: number): string {
  return `TW-${i + 1}`;
}

function worktreeId(i: number, host: string | null): string {
  return `${host === null ? "" : `${host}@`}${REPO}::C:/w/tw-${i}`;
}

// As cli/src/machine.rs reads a notice's args.
function noticeReadable(args: Record<string, unknown>): boolean {
  const positive = (v: unknown) => typeof v === "number" && v > 0;
  switch (args.notice) {
    case "blocked":
      return Array.isArray(args.blockers) && args.blockers.length > 0;
    case "roundsCap":
      return positive(args.pr) && positive(args.rounds);
    case "prClosed":
    case "autoMergeOff":
    case "ciCouldntRun":
    case "checksPending":
    case "copilotWontApprove":
      return positive(args.pr);
    default:
      return true;
  }
}

// A head whose checks never settle, as one waiting on a person: every seventh.
export function stuck(p: { head: string }): boolean {
  return parseInt(p.head, 16) % 7 === 4;
}

function sha(n: number): string {
  return n.toString(16).padStart(40, "0");
}

// A command as the relay sent it, the world the model carried it out against, and the
// result the model gave, which cli/src/command/tests/replays.rs replays through listen's own
// command path on its fakes (test/listen-traces.json). Only an `ok` step's command is
// kept: any other outcome is the network's or the step's, not what listen would answer.
// `world.worktree` is the issue's worktree's ID while there is one, clean at its PR's head with
// every commit pushed, `world.pr` the newest PR on its branch, the only one GitHub has, and
// `parent` and `children` the issue's parent and sub-issues by their IDs, and `comment` what
// the comment a `readComment` names is.
export type Replay = {
  command: Wire;
  world: {
    state: IssueStateType; worktree: string | null; agent: AgentStatus | null; sessions: string[]; newestComment: string | null;
    pr: { number: number; head: string; status: Pr["status"] } | null; parent: string | null; children: string[]; comment: Note | null;
  };
  result: Result;
  data: ResultData;
};

// The kinds replayed: those listen carries out through its ports (cli/src/ports.rs) and
// whose world the model holds. Not a PR's evaluation, whose checks and reviews it doesn't
// keep, triage's, or a note's thread's, whose comments it doesn't.
export const REPLAYED: readonly Command["kind"][] = ["reapWorktree", "startTurn", "readIssue", "startWorker", "resumeWorker", "deliverMessage", "readComment", "stopWorker", "closeAgent", "postNotice", "observe"];

// A comment of the owner's that is a note's: a reply to one of Tolkster's, or rooting one.
type Note = { kind: Extract<CommentKind, "noteReply" | "noteRoot">; root: string };

export class World {
  // The simulator's clock, which Linear's keeps up with.
  clock: () => number = () => START_MS;
  linear = 0;
  shas = 0;
  ids = 0;
  issues: IssueWorld[] = ISSUES.map(() => ({ delegate: null, state: "open", stateAt: this.time(), updatedAt: this.time(), priority: 0, sessions: [], newestComment: null, pr: null, landed: null, agent: null, worktree: false, host: null, cancelledMs: null, owes: { round: false, sessions: [] }, took: null, turns: [], meets: null, update: null, stoppedAt: null, orchestrating: null, chained: false, unnamed: false, taken: false, lag: null }));
  blocker = { done: false, at: this.time() };
  // Whether CHILD is a sub-issue of PARENT, and how many times `issue chain` has run.
  subIssue = false;
  runs = 0;
  triageIds = 0;
  // Triage sessions open in the world, every one ever opened, and the one this run of
  // `listen` opened last, whose end its watcher reports.
  sessions: TriageSession[] = [];
  opened: TriageSession[] = [];
  watched: TriageSession | null = null;
  // While recording, the replays of what listen carried out.
  replays: Replay[] | null = null;
  // The repo's default branch, and how often it has been pushed.
  base = BASE;
  bases = 0;
  terminals = 0;
  // What the world has reported and the network has yet to take, and whether a redelivery
  // can bring it again.
  events: { event: Event; clocked: boolean }[] = [];
  // Each owner's request by its session, the newest, and whether its session was answered or
  // told something since, in the order of asks and answers.
  requests = new Map<string, { i: number; kind: "delegation" | "mention" | "note"; at: string; asked: number; answered: number | null }>();
  answers = 0;
  // The sessions an event has landed for in the machine.
  heard = new Set<string>();
  // A stop's "Stopped." is never posted (the `stopSaysNothing` mutant).
  silentStops = false;
  // Each notice posted, with the issue's PR head as it was.
  told: { i: number; notice: string; head: string | null }[] = [];
  // The owner's comments that are notes', by their IDs.
  notes = new Map<string, Note>();

  // A second past Linear's last time, and never behind the simulator's clock, so a reply's
  // age is the same on both.
  time(): string {
    this.linear = Math.max(this.linear + 1, Math.ceil((this.clock() - START_MS) / 1000));
    return new Date(START_MS + this.linear * 1000).toISOString();
  }

  // `ms` past Linear's last time, moving no clock.
  since(ms: number): string {
    return new Date(START_MS + this.linear * 1000 + ms).toISOString();
  }

  parentOf(i: number): string | null {
    return i === CHILD && this.subIssue ? ISSUES[PARENT] : null;
  }

  // CHILD moves under PARENT, or off it, as its webhook says.
  move(under: boolean): void {
    if (this.subIssue === under) return;
    const w = this.issues[CHILD];
    const at = this.time();
    this.subIssue = under;
    w.updatedAt = at;
    this.post({ kind: "issueStateChanged", id: this.id("move"), issue: ISSUES[CHILD], state: w.state, parent: this.parentOf(CHILD), ...(under ? {} : { from: ISSUES[PARENT] }), at });
  }

  // `issue chain`'s delegation of a sub-issue its parent's store took: recorded, even over the
  // owner's own delegation to Tolkster, and in Linear, where only one no one holds changes and
  // has Tolkster's own session opened on it.
  chainDelegate(i: number): void {
    const w = this.issues[i];
    w.chained = true;
    if (w.delegate !== null) return;
    const at = this.time();
    w.delegate = "chain";
    w.updatedAt = at;
    const session = uuid(++this.ids, "c4a1");
    w.sessions.push({ id: session, kind: "chain", createdAt: at });
    this.post({ kind: "delegateChanged", id: this.id("chain"), issue: ISSUES[i], by: "chain", at });
    this.post({ kind: "sessionStarted", id: this.id("sess"), issue: ISSUES[i], session, sessionKind: "chain", byOwner: false, at });
  }

  id(prefix: string): string {
    this.ids += 1;
    return `${prefix}-${this.ids}`;
  }

  asked(i: number, session: string, kind: "delegation" | "mention" | "note", at: string): void {
    this.requests.set(session, { i, kind, at, asked: ++this.answers, answered: null });
  }

  // The owner's reply to the issue's note of Tolkster's, whose thread its request is in, or
  // with `root` a comment of theirs on the issue, Tolkster's own, which roots a thread. A
  // note's request is heard once the machine holds it (machine-sim.ts).
  note(i: number, root: boolean): void {
    const w = this.issues[i];
    const at = this.time();
    const comment = uuid(++this.ids, "c0aa");
    const thread = root ? comment : uuid(i + 1, "407e");
    w.updatedAt = at;
    if (root) w.newestComment = at;
    this.notes.set(comment, { kind: root ? "noteRoot" : "noteReply", root: thread });
    this.asked(i, thread, "note", at);
    this.heard.delete(thread);
    this.post({ kind: "ownerCommented", id: this.id("comment"), issue: ISSUES[i], comment, at });
  }

  // The owner's stop, an undelegation or a cancel halts the worker (rule 8). Only an
  // undelegation ends a session in Linear itself, the delegations'; a stop is owed its
  // "Stopped." and a conversation goes on.
  halted(i: number, at: string, undelegated = false): void {
    this.issues[i].stoppedAt = at;
    if (undelegated) for (const r of this.requests.values()) if (r.i === i && r.kind === "delegation") r.answered = ++this.answers;
  }

  answered(session: string | null): void {
    const r = session === null ? undefined : this.requests.get(session);
    if (r !== undefined) r.answered = ++this.answers;
  }

  post(event: Event, clocked = true): void {
    this.events.push({ event, clocked });
  }

  // GitHub acts on the issue's PR, and says so.
  pr(i: number, act: PrAct): void {
    const w = this.issues[i];
    const issue = ISSUES[i];
    const at = this.time();
    const p = w.pr;
    if (act === "open") {
      // A worker's branch has one PR open at a time, and none once one has merged or its work
      // landed. Every other
      // PR, by its head, has its repo cap its rounds at one, so a run meets the cap.
      if ((p !== null && p.status !== "closed") || w.landed !== null) return;
      const number = (p?.number ?? i * 100) + 1;
      const asker = w.took ?? w.sessions.at(-1)?.id ?? null;
      w.pr = {
        number, head: sha(++this.shas), status: "open", at, checks: "pending", feedbackAt: null, reviewerAt: null, autoMergeOff: false, refused: this.shas % 3 === 0 && asker !== null, base: this.base, conflict: false, fix: false, answeredAt: null,
        commentedHead: null, maxRounds: this.shas % 2 === 0 ? 1 : 3, unknown: 0,
      };
      this.post({ kind: "prOpened", id: this.id("gh"), issue, pr: number, head: w.pr.head, repo: PR_REPO, at });
      // Its `pr create` runs twice, as after auto-merge was unavailable: the second asks nothing new.
      if (w.pr.refused) for (let n = 0; n < 2; n++) this.post({ kind: "mergeAsked", id: this.id("ask"), issue, session: asker!, pr: number }, false);
      return;
    }
    if (p === null || p.status !== "open") return;
    p.at = at;
    switch (act) {
      case "push": {
        const before = p.head;
        p.head = sha(++this.shas);
        p.checks = "pending";
        p.fix = false;
        return this.post({ kind: "prPushed", id: this.id("gh"), issue, pr: p.number, head: p.head, before, at });
      }
      case "review":
      case "copilot":
        // Copilot's review of every other head only comments: nothing for a worker to act on.
        if (act === "copilot" && this.shas % 2 === 0) p.commentedHead = p.head;
        else p.feedbackAt = at;
        if (act === "review") p.reviewerAt = at;
        return this.post({ kind: "prReviewed", id: this.id("gh"), issue, pr: p.number, sha: p.head, reviewer: true, copilot: act === "copilot", at });
      // A head that isn't a worker's fix fails every third and couldn't run every fifth; a fix
      // fails every sixth, so one may still fail past the cap.
      case "checks":
        if (stuck(p)) return;
        p.checks = this.shas % 3 === 0 && (!p.fix || this.shas % 2 === 0) ? "failed" : !p.fix && this.shas % 5 === 2 ? "couldntRun" : "passed";
        return this.post({ kind: "checksCompleted", id: this.id("gh"), issue, sha: p.head, result: p.checks, at });
      case "merge":
      case "close":
        p.status = act === "merge" ? "merged" : "closed";
        return this.post({ kind: act === "merge" ? "prMerged" : "prClosed", id: this.id("gh"), issue, pr: p.number, at });
      // Off only once on: a refused PR's never was.
      case "autoOff":
        if (p.refused) return;
        p.autoMergeOff = true;
        return this.post({ kind: "autoMergeDisabled", id: this.id("gh"), issue, pr: p.number, at });
    }
  }

  // The worker's landing, in the session it was last given, while its agent runs and its
  // branch has no PR but a closed one: once only, until a new worker's start.
  land(i: number): void {
    const w = this.issues[i];
    if (!w.worktree || w.agent === null || w.agent === "exited" || w.took === null || (w.pr !== null && w.pr.status !== "closed") || w.landed !== null) return;
    w.landed = sha(++this.shas);
    this.post({ kind: "landed", id: this.id("land"), issue: ISSUES[i], session: w.took, head: w.landed }, false);
  }

  // A push to the default branch, which leaves each open PR behind it, and with `conflict`
  // one only a worker can bring up to date. After every other push GitHub reads an open PR's
  // mergeability as UNKNOWN for a read, and after every sixth for as many reads as the machine
  // makes on its own, counted again from each push. Its webhook is the caller's to deliver.
  pushBase(conflict: boolean): string {
    this.bases += 1;
    this.base = `b${this.bases.toString(16).padStart(39, "0")}`;
    for (const w of this.issues) {
      if (w.pr?.status !== "open") continue;
      if (conflict) w.pr.conflict = true;
      w.pr.unknown = this.bases % 6 === 0 ? UNKNOWN_RECHECKS + 1 : this.bases % 2;
    }
    return this.base;
  }

  // The worker's agent acts on what it took: a round rebases its PR onto the base and pushes
  // a fix that answers the feedback and passes its checks, a request is read and answered in
  // its session, and an open turn answers its own; then the agent exits.
  work(i: number): void {
    const w = this.issues[i];
    const issue = ISSUES[i];
    for (const turn of w.turns.filter((t) => !t.answered)) {
      const { session, terminal } = turn;
      turn.answered = true;
      const at = this.time();
      this.post({ kind: "workerRead", id: this.id("read"), issue, session, reader: "turn", through: at }, false);
      this.post({ kind: "workerAnswered", id: this.id("answer"), issue, session, reader: "turn", at });
      this.answered(session);
      this.post({ kind: "turnIdle", id: this.id("idle"), issue, session, terminal, at });
    }
    if (w.agent !== "working" || (!w.owes.round && w.owes.sessions.length === 0)) return;
    this.post({ kind: "agentWorking", id: this.id("obs"), issue }, false);
    const p = w.pr;
    if (w.owes.round && p !== null && p.status === "open") {
      const before = p.head;
      p.base = this.base;
      p.conflict = false;
      p.answeredAt = p.feedbackAt;
      p.head = sha(++this.shas);
      p.checks = "pending";
      p.fix = true;
      p.at = this.time();
      this.post({ kind: "prPushed", id: this.id("gh"), issue, pr: p.number, head: p.head, before, at: p.at });
      // A reviewer asks for more on every other fix, and Copilot on half the rest, so
      // a run meets feedback past the cap, a reviewer's and one that isn't.
      if (this.shas % 4 !== 3) {
        const reviewer = this.shas % 2 === 0;
        p.feedbackAt = this.time();
        if (reviewer) p.reviewerAt = p.feedbackAt;
        this.post({ kind: "prReviewed", id: this.id("gh"), issue, pr: p.number, sha: p.head, reviewer: true, copilot: !reviewer, at: p.feedbackAt });
      }
    }
    // A parent's worker brought back for its sub-issues' end moves it to Done.
    if (w.orchestrating?.end === true && w.state === "open") {
      const at = this.time();
      w.state = "completed";
      w.stateAt = at;
      w.updatedAt = at;
      w.orchestrating = { ...w.orchestrating, end: false };
      this.post({ kind: "issueStateChanged", id: this.id("state"), issue, state: "completed", parent: this.parentOf(i), at });
    }
    for (const session of w.owes.sessions) {
      const at = this.time();
      this.post({ kind: "workerRead", id: this.id("read"), issue, session, reader: "worker", through: at }, false);
      this.post({ kind: "workerAnswered", id: this.id("answer"), issue, session, reader: "worker", at });
      this.answered(session);
    }
    w.agent = "exited";
    w.owes = { round: false, sessions: [] };
    this.post({ kind: "agentExited", id: this.id("obs"), issue }, false);
  }

  // listen's triage commands, as command.rs carries them out: a start answers with the
  // session this run opened while it is still open, and otherwise opens one.
  listenTriage(c: Command, out: Exclude<Outcome, "none">): TriageEvent {
    let data: object | undefined;
    if (out === "ok" && c.kind === "startTriage") {
      const open = this.watched !== null && this.sessions.some((s) => s.id === this.watched!.id) ? this.watched : null;
      const asked = (c.params.issueIds as string[]).filter((id) => ISSUES.includes(id));
      if (open !== null) data = { session: open, waiting: asked };
      else if (asked.length > 0 || this.triageIds % 2 === 0) {
        this.triageIds += 1;
        const session = { id: `s${this.triageIds}`, terminal: `term_${this.triageIds}`, issues: asked.length > 0 ? asked.map((id) => `TW-${ISSUES.indexOf(id) + 1}`) : ["TW-9"] };
        this.sessions.push(session);
        this.opened.push(session);
        this.watched = session;
        data = { session, waiting: [] };
      } else data = {};
    } else if (out === "ok" && c.kind === "observeTriage") {
      data = { open: this.sessions.some((s) => s.terminal === c.params.terminal) };
    }
    return { kind: "commandResult", id: `result:${c.id}`, command: c.id, result: out, ...(data === undefined ? {} : { data }) } as TriageEvent;
  }

  // listen carries out a command against the world, as it is now, and reports its result. A
  // round, line or stop `ok` into a live agent meets what the issue's `meets` says, once. `held` is a
  // line's or, as only the simulator gives it, a turn's; elsewhere listen carries the command
  // out.
  listen(c: Command, wire: Wire | undefined, asked: Exclude<Outcome, "none">): void {
    const i = ISSUES.indexOf(c.issue);
    const w = this.issues[i];
    const issue = c.issue;
    const pr = w.pr === null ? null : { number: w.pr.number, head: w.pr.head, status: w.pr.status };
    const comment = (c.kind === "readComment" ? this.notes.get(wire?.args.commentId as string) : undefined) ?? null;
    const world = { state: w.state, worktree: w.worktree ? worktreeId(i, w.host) : null, agent: w.agent, sessions: w.sessions.map((s) => s.id), newestComment: w.newestComment, pr, parent: this.parentOf(i), children: this.read(i).children.map((c) => c.id), comment };
    const live = w.agent === "working" || w.agent === "background" || w.agent === "idle";
    const line = c.kind === "startRound" || c.kind === "deliverMessage";
    // A `gone` one waits for a comment's line.
    const meets = asked === "ok" && live && w.meets?.to === c.kind && (w.meets.as !== "gone" || wire?.args.comment === true) ? w.meets.as : null;
    if (meets !== null) w.meets = null;
    const out = meets ?? (asked === "held" && !line && c.kind !== "startTurn" ? "ok" : asked);
    const answer = (result: CommandResult, data?: ResultData) => this.post({ kind: "commandResult", id: c.id, issue, command: c.id, result, ...(data === undefined ? {} : { data }) });
    const exit = () => {
      w.agent = "exited";
      w.owes = { round: false, sessions: [] };
      this.post({ kind: "agentExited", id: this.id("obs"), issue }, false);
    };
    // A line typed into an agent that exits before its turn begins, its exit reported after
    // the result or, as the send lands while it exits, before.
    if (out === "typed" || out === "exiting") {
      if (out === "exiting") exit();
      answer("failed", { typed: true });
      if (out === "typed") exit();
      return;
    }
    if (out === "gone") {
      answer("gone");
      return exit();
    }
    if (out === "unnamed") return answer("failed", { unnamed: true });
    const session = typeof wire?.args.sessionId === "string" ? wire.args.sessionId : null;
    const owe = () => {
      if (session !== null && !w.owes.sessions.includes(session)) w.owes.sessions.push(session);
      w.took = session ?? w.took;
    };
    let result: CommandResult = out;
    let data: ResultData | undefined;
    const after: Event[] = [];
    if (result === "ok") {
      switch (c.kind) {
        case "readIssue":
          data = { read: w.lag?.at === w.updatedAt ? { ...this.read(i), delegate: w.lag.delegate, updatedAt: w.lag.updatedAt } : this.read(i) };
          w.lag = null;
          break;
        // A worktree there already is answered with, made false: its agent left as it is, and
        // launched is whether one is listed there (an exited one's terminal is gone), unless,
        // with none, the start names it, when one is launched on the brief.
        case "startWorker": {
          const made = !w.worktree;
          const listed = w.agent !== null && w.agent !== "exited";
          const relaunch = !made && !listed && wire?.args.worktreeId === worktreeId(i, w.host);
          const launched = made || listed || relaunch;
          // A new worktree is made on the host the start names.
          if (made) {
            w.landed = null;
            w.host = typeof wire?.args.host === "string" ? wire.args.host : null;
          }
          if (made || relaunch) {
            w.agent = "working";
            owe();
            w.orchestrating = wire?.args.orchestrate === true && session !== null ? { session, end: false } : null;
          }
          w.worktree = true;
          data = { worktree: { id: worktreeId(i, w.host), branch: `tw-${i}` }, madeWorktree: made, launched };
          break;
        }
        // An exited worker is brought back; one whose agent is there already is left to it. A
        // parent's with no worktree, resumed for its sub-issues or a message on an issue that
        // has some, is started again in a new one.
        case "resumeWorker": {
          const end = wire?.args.childrenEnd === true;
          const forChildren = end || wire?.args.childrenAdded === true;
          if (w.agent === null && (forChildren || this.read(i).children.length > 0)) {
            // On this machine, as listen makes a parent's new worktree.
            w.worktree = true;
            w.host = null;
            w.agent = "working";
            owe();
            w.orchestrating = session === null ? null : { session, end };
            data = { worktree: { id: worktreeId(i, w.host), branch: `tw-${i}` }, madeWorktree: true, launched: true };
            break;
          }
          if (w.agent === null) result = "failed";
          else if (w.agent === "exited") {
            w.agent = "working";
            owe();
            if (forChildren && session !== null) w.orchestrating = { session, end };
            else if (w.orchestrating !== null) w.orchestrating = { ...w.orchestrating, end: false };
          }
          data = result === "ok" ? { launched: true } : undefined;
          break;
        }
        // A line is never typed into an agent waiting at a prompt. A message's goes only into an
        // agent's terminal; a round brings an exited worker back, and needs its PR and the
        // session it reports in.
        case "startRound":
        case "deliverMessage":
          if (c.kind === "deliverMessage" && (w.agent === null || w.agent === "exited")) result = "gone";
          else if (w.agent === null) result = "failed";
          else if (c.kind === "startRound" && wire !== undefined && (typeof wire.args.repo !== "string" || wire.args.sessionId == null)) result = "failed";
          else if (w.agent === "waiting") result = "held";
          else {
            // A round launches an agent only for an exited worker; a line typed into a running one
            // says nothing.
            if (c.kind === "startRound" && w.agent === "exited") {
              data = { launched: true };
              if (w.orchestrating !== null) w.orchestrating = { ...w.orchestrating, end: false };
            }
            w.agent = "working";
            w.took = session ?? w.took;
            if (c.kind === "startRound") w.owes.round = true;
            else if (wire?.args.comment !== true) owe();
          }
          break;
        // Args listen can't read, read from the store as it was sent, end it failed: a blocked
        // notice whose blockers have all finished, or a PR's with no PR held.
        case "postNotice":
          if (wire !== undefined && !noticeReadable(wire.args)) result = "failed";
          else {
            this.told.push({ i, notice: String(c.params.notice), head: w.pr?.head ?? null });
            if (!(this.silentStops && c.params.notice === "stopped")) this.answered(session);
          }
          break;
        // A stop says "Stopped." in the session it is for.
        case "stopWorker":
          if (!this.silentStops) this.answered(session);
          if (w.agent !== null) w.agent = "exited";
          w.owes = { round: false, sessions: [] };
          break;
        // The agent's exit is seen by listen's watch, as the machine hears it.
        case "closeAgent":
          if (w.agent !== null) {
            w.agent = "exited";
            after.push({ kind: "agentExited", id: this.id("obs"), issue });
          }
          w.owes = { round: false, sessions: [] };
          break;
        // As listen's take-back: each sub-issue the chain named that Tolkster's record says the
        // chain delegated, and that Tolkster holds, whoever delegated it last, is given back.
        case "takeBackChain":
          for (const child of (wire?.args.children ?? []) as string[]) {
            const ci = ISSUES.indexOf(child);
            if (ci === -1 || !this.issues[ci].chained || this.issues[ci].delegate === null) continue;
            const at = this.time();
            this.issues[ci].delegate = null;
            this.issues[ci].updatedAt = at;
            this.halted(ci, at, true);
            after.push({ kind: "delegateChanged", id: this.id("undeleg"), issue: child, by: null, at });
          }
          break;
        // As listen carries out the rule the machine named (cli/src/reap.rs, `reap_by`), of a
        // worktree clean and pushed: gone already, or removed on a merged or landed rule whose
        // head is the checkout's, or a cancelled or completed one while the issue is still so. One
        // with its agent running stays. A rule it can't read ends it failed.
        case "reapWorktree": {
          const rule = wire?.args.rule as { kind: string; head?: string } | undefined;
          if (rule === undefined) {
            result = "failed";
            break;
          }
          const holds = rule.kind === "merged" ? w.pr !== null && rule.head === w.pr.head : rule.kind === "landed" ? w.landed !== null : rule.kind === w.state;
          const removed = (w.agent === null || w.agent === "exited") && holds;
          if (removed) {
            w.worktree = false;
            w.agent = null;
            w.owes = { round: false, sessions: [] };
          }
          data = { reaped: !w.worktree };
          break;
        }
        case "evaluatePr":
          if (w.pr === null) result = "failed";
          else {
            data = { pr: this.prRead(w.pr) };
            w.pr.unknown = Math.max(0, w.pr.unknown - 1);
          }
          break;
        // As GitHub's update-branch, leased on the head the machine read, once what the issue's
        // `update` says has landed: refused for a conflict, and otherwise a merge of the base,
        // pushed as a new head.
        case "updateBranch": {
          const p = w.pr;
          const meets = w.update;
          w.update = null;
          if (p !== null && p.status === "open" && meets === "refused") p.conflict = true;
          if (p !== null && p.status === "open" && meets === "moved") {
            const before = p.head;
            p.head = sha(++this.shas);
            p.checks = "pending";
            p.fix = false;
            p.at = this.time();
            after.push({ kind: "prPushed", id: this.id("gh"), issue, pr: p.number, head: p.head, before, at: p.at });
          }
          if (p === null || p.status !== "open" || meets === "failed") result = "failed";
          else if (wire?.args.head !== p.head) data = { moved: true };
          else if (p.conflict) {
            result = "failed";
            data = { refused: true };
          } else if (p.base !== this.base) {
            const before = p.head;
            p.base = this.base;
            p.head = sha(++this.shas);
            p.checks = "pending";
            p.fix = false;
            p.at = this.time();
            after.push({ kind: "prPushed", id: this.id("gh"), issue, pr: p.number, head: p.head, before, at: p.at });
          }
          break;
        }
        // As GitHub's merge, pinned to the head the machine read: one merged already, as by its
        // own auto-merge, is done, and one no longer mergeable, or held by the owner, refused. With
        // no worktree its branch can't be told, as listen's.
        case "mergePr": {
          const p = w.pr;
          if (p === null || p.status === "closed" || !w.worktree) result = "failed";
          else if (p.status === "open" && wire?.args.head !== p.head) data = { moved: true };
          else if (p.status === "open" && (this.prRead(p).mergeable !== true || p.autoMergeOff)) {
            result = "failed";
            data = { refused: true };
          } else if (p.status === "open") {
            p.status = "merged";
            p.at = this.time();
            after.push({ kind: "prMerged", id: this.id("gh"), issue, pr: p.number, head: p.head, at: p.at });
          }
          break;
        }
        // As command/workers.rs's `observe`: the worktree's agent as Orca lists it, one waiting in the
        // background idle and one not running exited, and a session's turn ended when none is open.
        case "observe":
          if (w.worktree) after.push({ kind: w.agent === "working" ? "agentWorking" : w.agent === "idle" || w.agent === "background" ? "agentIdle" : w.agent === "waiting" ? "agentWaiting" : "agentExited", id: this.id("obs"), issue });
          if (typeof c.params.session === "string" && !w.turns.some((t) => t.session === c.params.session)) after.push({ kind: "turnEnded", id: this.id("ended"), issue, session: c.params.session });
          break;
        case "readComment":
          data = { comment: { kind: comment?.kind ?? "plain", newestOwnerAt: w.newestComment, state: w.state, ...(comment === null ? {} : { root: comment.root }) } };
          break;
        // A turn opens in a terminal of its own; one open for the session already is handed the
        // request, and answered with, as a resend's, which it has, gives it nothing new.
        case "startTurn": {
          const request = wire?.args.request ?? null;
          const open = w.turns.find((t) => t.session === c.machine);
          if (open !== undefined && open.request !== request) Object.assign(open, { request, answered: false });
          const terminal = open?.terminal ?? `term-${++this.terminals}`;
          if (open === undefined) w.turns.push({ session: c.machine, terminal, request, answered: false });
          data = { terminal };
          break;
        }
        // It says nothing: the machine's `stopped` notice does, once the stop holds.
        case "stopTurn": {
          const turn = w.turns.find((t) => t.session === c.machine);
          if (turn !== undefined) {
            w.turns = w.turns.filter((t) => t !== turn);
            after.push({ kind: "turnEnded", id: this.id("ended"), issue, session: c.machine, terminal: turn.terminal });
          }
          break;
        }
        default:
          break;
      }
    } else if (result === "failed" && c.kind === "startWorker") {
      data = { madeWorktree: w.worktree };
    } else if ((result === "lost" || result === "failed") && c.kind === "postNotice" && (result === "lost" || c.params.again === true)) {
      // A notice `lost` may have been posted, and one that failed again is said only if an event
      // calls for it: neither is sent a third time.
      this.answered(session);
    } else if (result === "retryable") {
      // What it waits on: the paired host, which a listing hearing it clears, Orca, which any
      // listing does, or the lock, which a nap's command ending does.
      data = { waitsOn: c.sends % 3 === 1 ? { host: NAP_HOSTS[0] } : c.sends % 3 === 2 ? { orca: true } : { lock: true } };
    }
    // The replay's fakes hold this machine alone, so nothing on a paired host is replayed.
    const here = world.worktree === null ? wire?.args.host === undefined : !world.worktree.includes("@");
    if (this.replays !== null && out === "ok" && REPLAYED.includes(c.kind) && wire !== undefined && wire.args.thread !== true && here) this.replays.push({ command: wire, world, result: result as Result, data: data ?? {} });
    answer(result, data);
    for (const e of after) this.post(e, e.kind === "turnEnded" || e.kind === "prPushed" || e.kind === "delegateChanged");
  }

  read(i: number): NonNullable<ResultData["read"]> {
    const w = this.issues[i];
    return {
      updatedAt: w.updatedAt, delegate: w.unnamed && w.delegate === "owner" ? (w.taken ? "unknown" : "created") : w.delegate, state: w.state, triage: false, stateChangedAt: w.stateAt, priority: w.priority,
      blockers: i === 0 ? [{ id: BLOCKER, done: this.blocker.done, updatedAt: this.blocker.at }] : [], moreBlockers: false,
      parent: this.parentRead(i), children: i === PARENT && this.subIssue ? [{ id: ISSUES[CHILD], done: this.issues[CHILD].state !== "open", triage: false, updatedAt: this.issues[CHILD].updatedAt }] : [], moreChildren: false, repo: REPO, sessions: w.sessions.map((s) => ({ ...s })),
    };
  }

  parentRead(i: number): NonNullable<ResultData["read"]>["parent"] {
    const id = this.parentOf(i);
    if (id === null) return null;
    const p = this.issues[ISSUES.indexOf(id)];
    return { id, delegated: p.delegate === "owner" || p.delegate === "chain", by: p.delegate, state: p.state, priority: p.priority, updatedAt: p.updatedAt };
  }

  prRead(p: Pr): PrRead {
    const mergeState = p.unknown > 0 ? "UNKNOWN" : p.conflict ? "DIRTY" : p.base !== this.base ? "BEHIND" : "CLEAN";
    return {
      number: p.number, status: p.status, at: p.at, head: p.head, mergeState, base: this.base,
      autoMergeOff: p.autoMergeOff, autoMerge: !p.autoMergeOff && !p.refused, mergeable: mergeState === "CLEAN" && p.checks === "passed" && p.commentedHead !== p.head, manualMerge: false, checks: p.checks, feedback: p.feedbackAt === null ? null : { at: p.feedbackAt, reviewerAt: p.reviewerAt },
      copilotWontApprove: p.commentedHead === p.head, maxRounds: p.maxRounds,
    };
  }
}
