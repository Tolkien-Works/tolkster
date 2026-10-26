// The state machines' names and shapes, built to docs/state-machine.md: a request machine
// per session and a work machine per issue, each kept in its issue's store (store.ts),
// with what crosses issues kept by the coordinator (coordinator.ts). test/contract.json's
// `machine` holds the same names for listen's side, and test/machine.test.ts holds these
// lists to it.
//
// Every value is IDs, times and enums: never issue, comment or review text, nor people's
// details. Linear's times are its one ISO format and compare as strings; the Durable
// Object's own clock is milliseconds and is never compared with Linear's.

export const REQUEST_STATES = ["requested", "queued", "claimed", "done", "stopping"] as const;
export type RequestState = (typeof REQUEST_STATES)[number];

export const WORK_STATES = ["launching", "running", "idle", "held", "exited", "rest", "halted", "merged", "reaped"] as const;
export type WorkState = (typeof WORK_STATES)[number];

export const COMMANDS = [
  "readIssue", "startWorker", "deliverMessage", "readComment", "resumeWorker", "startRound", "stopWorker", "startTurn",
  "stopTurn", "evaluatePr", "observe", "closeAgent", "reapWorktree", "postNotice", "takeBackChain", "startContext",
  "startTriage", "observeTriage", "updateBranch", "mergePr", "makeHost", "wakeHost", "suspendHost", "updateHost",
  "destroyHost",
] as const;
// `listen`'s commands, and `postAway`, which the Durable Object posts itself (queue.ts, `LOCAL`).
export type CommandKind = (typeof COMMANDS)[number] | "postAway";

// What `listen` answers a command with; `lost` is the Durable Object's own, after the last send.
export const RESULTS = ["ok", "failed", "retryable", "held", "gone"] as const;
export type Result = (typeof RESULTS)[number];

// Who reported a read or an answer: the issue's worker, or the session's or a note's thread's turn.
export const READERS = ["worker", "turn"] as const;
export type Reader = (typeof READERS)[number];

export const NOTICES = [
  "blocked", "busy", "held", "notDelegated", "noLongerMine", "cancelled", "alreadyDone", "noRepo", "stopped",
  "stalled", "paused", "heldAtPrompt", "couldntPass", "roundsCap", "prClosed", "autoMergeOff", "ciCouldntRun",
  "checksPending", "copilotWontApprove", "stopUnnamed", "roundUnnamed", "unheard",
] as const;
export type NoticeKind = (typeof NOTICES)[number];

export const COORDINATOR_EVENTS = ["slotAsked", "slotHeld", "slotReleased", "blockersRead", "blockerChanged", "prRecorded", "repoPushed"] as const;

// The four work states that take a slot.
export const AGENT_STATES: readonly WorkState[] = ["launching", "running", "idle", "held"];
// No worker runs, and a request resumes it; a cancel moves these to `halted`, and a finished
// issue's worktree with no PR is reaped from them.
export const AT_REST_WORK: readonly WorkState[] = ["exited", "rest", "halted"];
// Where a read of the PR is evaluated: its worker may still push to it. A PR fact is evaluated
// as it lands in these, with its PR open, and in any other on the next entry into one.
export const EVALUATES_PR: readonly WorkState[] = ["running", "idle", "exited", "rest"];
// The work is over, halted or merged: a PR closed now goes unsaid.
export const WORK_ENDED: readonly WorkState[] = ["halted", "merged", "reaped"];

export const MINUTE = 60 * 1000;
export const HOUR = 60 * MINUTE;

// A command with no result after its timeout is sent again with the same ID, SENDS times
// in all, then ended by the Durable Object as `lost`.
export const COMMAND_TIMEOUT_MS = 2 * MINUTE;
export const START_TIMEOUT_MS = 10 * MINUTE;
export const SENDS = 3;
// How long a command waits on its worker's recipe machine to wake (`waking`), a failed wake's
// cool-down and its retry included.
export const WAKE_WAIT_MS = 30 * MINUTE;
// A slot reserved for a request this long without the work taking it is given back.
export const GRANT_UNUSED_MS = 30 * MINUTE;
// A merge state still `UNKNOWN` after the base moved is read again this long after, then
// twice as long each time, this many times at most.
export const RECHECK_MS = 10 * 1000;
export const UNKNOWN_RECHECKS = 3;
// The away message, for a request still waiting this long after its store found `listen`
// away (its `quiet` deadline).
export const AWAY_AFTER_MS = 15 * MINUTE;
// How long a settled issue's store is kept before it shrinks to its counters: past the 7
// days the intake keeps a delivery to drop its replay (`SEEN_KEEP_MS`, intake.ts).
export const RETIRE_AFTER_MS = 8 * 24 * HOUR;
// A request whose read couldn't say who delegated the issue is read again this often, for
// its history entry, until the delegation's event lands or this long has passed.
export const ATTRIBUTION_RECHECK_MS = 30 * 1000;
export const ATTRIBUTION_WAIT_MS = 2 * MINUTE;
// listen's own default, until it reports its setting.
export const DEFAULT_SLOTS = 6;
export const DEFAULT_MAX_ROUNDS = 3;
// A session's turn stays open this long after Claude was last idle with its request
// answered, the prompt cache's lifetime, then is closed (its `warm` deadline).
export const WARM_MS = 5 * MINUTE;

export type DeadlineKind = "own" | "recheck" | "takeBack" | "pending" | "quiet" | "warm";
// `version` is the machine's version that armed it: one armed again, or whose state was
// left, no longer matches the deadline event that fires for it, which is then dropped.
// `fired` marks one the alarm has fired, waiting for its turn; it is checked again when it
// runs.
export type Deadline = { kind: DeadlineKind; version: number; dueMs: number; fired?: boolean };

// A stop owed (ctx.ts, `stop`), until its close is confirmed or the machine leaves the states
// that hold one: the newest stop's command ID, the one stop whose `ok` confirms it, and when it was
// sent, on the Durable Object's clock. An exit or a turn's end received before `sentMs` may
// be of an agent or turn the stop was sent again for, so it doesn't confirm. A resend with
// nothing begun since keeps `sentMs`; one sent for a start that may have outlived it takes its own.
// `since`, when it was first owed, is kept by every resend.
export type StopOwed = { command: string; sentMs: number; since: number };

export type Machine<S> = { state: S; version: number; deadlines: Deadline[]; stop: StopOwed | null };

// A `note` is the thread of a comment of Tolkster's that no session began on, keyed by its root comment.
export const SESSION_KINDS = ["delegation", "chain", "mention", "reply", "note"] as const;
export type SessionKind = (typeof SESSION_KINDS)[number];
// The sub-issues' end is a request of its own kind.
export type RequestKind = "start" | "prompt" | "stop" | "delegation" | "childrenEnd" | "childrenAdded";
// Who delegated the issue to Tolkster, or null when it isn't. `unknown`, only ever a read's,
// is Tolkster's with no history entry read that says by whom, as it isn't written yet; `aged`,
// only ever a read's and never kept, the same with the delegation older than the entries read;
// `created`, only ever a read's and kept as `unknown`, the same with no entry that changed the
// delegate, on an issue the owner created; `other` is an entry naming someone else.
export type DelegatedBy = "owner" | "chain" | "other" | "unknown" | "aged" | "created" | null;
export const ISSUE_STATES = ["open", "completed", "cancelled"] as const;
export type IssueStateType = (typeof ISSUE_STATES)[number];
// `background`: idle at its prompt while a call it sent to the background runs, which is working.
export type AgentStatus = "working" | "background" | "idle" | "waiting" | "exited" | "gone";
export const CHECKS = ["passed", "failed", "couldntRun"] as const;
export type Checks = (typeof CHECKS)[number];
export const MERGE_STATES = ["CLEAN", "BEHIND", "DIRTY", "UNKNOWN"] as const;
export type MergeState = (typeof MERGE_STATES)[number];
export const PR_STATUSES = ["open", "merged", "closed"] as const;
export type PrStatus = (typeof PR_STATUSES)[number];
export const COMMENT_KINDS = ["plain", "mention", "reply", "noteReply", "noteRoot", "gone"] as const;
export type CommentKind = (typeof COMMENT_KINDS)[number];
export type HaltedBy = "stop" | "undelegation" | "cancel" | "parentHalt";

// A Linear fact, with the clock it came with.
export type Clocked<T> = { value: T; at: string };
// A related issue's state, versioned by its own `updatedAt`. `triage` only for a sub-issue:
// one in Triage, which nobody has accepted, doesn't hold up its parent's end.
export type Related = { done: boolean; triage?: boolean; at: string };
// A set only ever read: which issues it names is versioned by when the read was sent, on
// the Durable Object's clock; each one's state by its own `updatedAt`. `added`: each issue
// an event put in it, by when it landed on that clock, kept by a read sent before then.
export type ReadSet = { sentMs: number; ids: string[]; states: Record<string, Related>; added?: Record<string, number> };

export type IssueFacts = {
  // `created`: the `unknown` a `created` read said, and then the owner it is taken for once
  // the wait on it is over (rule 6), which such reads keep.
  delegate: (Clocked<DelegatedBy> & { created?: true }) | null;
  // Whether the issue was ever delegated to Tolkster, for `notDelegated` against `noLongerMine`.
  everDelegated: boolean;
  // The state's type, and when it changed, by the issue's `updatedAt`; `triage` while it is
  // open in Triage.
  issueState: (Clocked<IssueStateType> & { changedAt: string; triage?: boolean }) | null;
  // The issue's own, by its `updatedAt`; with none (0), the parent's own is its place in the
  // queue (`priorityOf`), kept on `parent` by the parent's.
  priority: Clocked<number> | null;
  // `more` says the read found more than its first 50 (unfinished first), so it counts as blocked.
  blockers: (ReadSet & { more: boolean }) | null;
  parent: { id: string; at: string; delegated: boolean; by: DelegatedBy; state: IssueStateType; parentAt: string | null; priority?: Clocked<number> } | null;
  // When a webhook or a read last said it has no parent, while it has none: an older read
  // gives it none back.
  unparented?: string;
  // `more` says the read found more than its first 50, which count as unfinished.
  children: (ReadSet & { more: boolean }) | null;
  // The parent whose chain delegated this issue, while that chain holds.
  chain: { parent: string; run: string } | null;
  // A parent's take-back owed, for the chain run it names.
  takeBack: { run: string } | null;
  // The sub-issues that took this parent's chain, of any run since it was last halted;
  // `asked` and `answered` are the newest run's forwards and the answers back.
  // `haltedAt` is when the parent was halted, after which the run delegates nothing more and
  // a late taker is halted too.
  chained: { run: string; issues: string[]; asked: string[]; answered: string[]; haltedAt: string | null } | null;
  // Tolkster's sessions on the issue, with their kind and start, and for a mention's, when
  // its event named it, the ID of the owner's comment that mentioned Tolkster. `started`:
  // its own `sessionStarted` has been merged, whose kind and start replaced a read's; absent
  // while only a read has listed it.
  sessions: Record<string, { kind: SessionKind; createdAt: string; comment?: string; started?: true }>;
  repo: Clocked<string | null> | null;
  worktree: { id: string; branch: string; version: number } | null;
  // `workedMs`: when it was last seen working, whatever its status since.
  agent: { status: AgentStatus; sinceMs: number; workedMs?: number } | null;
  halted: Halted | null;
  // `receivedMs`: when the event that set it was received, for a read from its second.
  // `before`: the head a webhook's push to `head` replaced; absent when no push set `head`.
  pr: { number: number; head: string; repo: string; status: PrStatus; at: string; receivedMs: number; before?: string } | null;
  // The commit its worker landed on the default branch with no PR, the work's merge, until a
  // new worker begins or the worker is given another request. A landing holds no PR: one
  // held goes with it.
  landed?: { head: string };
  checks: Record<string, Clocked<Checks>>;
  mergeState: { value: MergeState; base: string } | null;
  // Whether GitHub holds auto-merge on for PR `pr`, as its newest read showed.
  autoMerge: { pr: number; on: boolean } | null;
  // The PR its worker asked to merge when GitHub refused auto-merge for being mergeable already.
  mergeAsked?: number;
  feedback: Record<string, { at: string; copilot: boolean }>;
  // `lastFailed`: the head whose failed checks last started a round.
  rounds: { total: number; byHead: Record<string, number>; lastAt: string | null; lastBase: string | null; lastFailed: string | null };
  // The base SHA of the newest push to the repo's default branch, for a rebase's need.
  base: string | null;
  // Counts each fresh PR fact that asks for `evaluatePr`.
  prVersion: number;
  // Each notice's key, once sent, for work and requests alike.
  notices: string[];
  // The `createdAt` of the newest owner comment a delivered line has covered.
  commentTold: string | null;
  // The `createdAt` of the newest owner comment whose line found no agent to take it: told again
  // when an agent next works.
  commentOwed?: string;
};

// When the worker was halted, and by what: `at` is the Linear time of what
// halted it. Whether its close is confirmed is the work machine's `stop`.
export type Halted = { at: string; by: HaltedBy };

export type SessionFacts = {
  request: { kind: RequestKind; at: string } | null;
  // Whether the session's request is the owner's: the Worker's ingress check, kept here.
  byOwner: boolean;
  // `known`: the sub-issues its worker was last sent to chain, which a sub-issue added later
  // isn't.
  orchestrated: { at: string; known: string[] } | null;
  // `host`: where the coordinator granted it, this machine when absent.
  grant: { sinceMs: number; host?: string } | null;
  // What the request waits for while `queued`: a slot (and its place in the queue), the
  // worker held at a prompt, its merged worktree's reap, or the issue's blockers.
  waiting: "slot" | WaitReason;
  // The ID of the slotAsked the request's decision sent, so a grant answering it is known
  // to come in the step that asked.
  asked: string | null;
  // Each reader's own mark: a fresh worker has read none of what a turn showed the owner's
  // prompts to. `readThrough` is the worker's, the one `tolkster messages` bounds it by.
  readThrough: string | null;
  turnReadThrough: string | null;
  answeredThrough: string | null;
  // `idleMs`: since when its Claude has been idle with `for` answered, the turn warm and
  // handed the next request at once; null while it works. `closingMs`: when its warm close
  // was sent, sent again until its end lands.
  turn: { terminal: string | null; sinceMs: number; for: string; idleMs: number | null; closingMs: number | null } | null;
  // The request whose decision found who delegated `unknown`, and when it first did.
  unattributed?: { for: string; sinceMs: number };
  // The request whose line to the running worker failed with nothing typed: the agent may
  // have exited before its exit was taken.
  undelivered?: string;
};

// What a `queued` request waits for besides a slot, which its entry names.
export type WaitReason = "held" | "merged" | "blockers" | null;

export type Session = { facts: SessionFacts; machine: Machine<RequestState> | null };

// What a round's `startRound` will mark in `rounds` once its line is known to have reached the
// worker: the head it was sent for, and each marker it moves, absent when it moves none.
export type Proposal = { head: string; lastAt?: string; lastBase?: string; lastFailed?: string };

// What a `launching` worker's command in flight is: a start on its brief, a resume, or a review round.
export type Launch = "start" | "resume" | "round";

export type WorkMachine = Machine<WorkState> & {
  // Set while `launching`, null in any other state.
  launch: Launch | null;
  // An `exited` worker stopped short of its request and said so, or left it to `listen` or the
  // request to say; false in any other state.
  stalled: boolean;
  // The session that last gave the worker a request, and the request's time.
  requestedBy: { session: string; at: string } | null;
  // The PR facts' version the last `evaluatePr` whose result landed in an evaluating state
  // was sent at, and whether that read found a failed check, or a PR to merge, mid-turn, left
  // to read again once the turn ends.
  prEvaluated: { version: number; redWhileRunning?: boolean };
  // The `UNKNOWN` merge state's rechecks so far.
  unknownRechecks: number;
  // The head a review round in flight began on, until the worker stops running it (idle,
  // exited or gone): feedback on that head that lands meanwhile is left to the round.
  inRound?: string | null;
  // The proposals of rounds whose line was typed but whose turn hasn't been seen begin: counted
  // when the agent is seen working since the newest was sent (`unseenSentMs`), dropped when it
  // exits.
  unseen?: Proposal[];
  unseenSentMs?: number;
  // When it last went idle or held, which keys the paused and held notices: once per pause.
  pausedSinceMs: number | null;
  // When the launch of an agent on its prompt was first sent, until it is seen working: an
  // agent idle before then never took its prompt, and hasn't paused.
  firstTurn?: number | null;
  // Its start launched an agent on the brief, or failed or was lost after making the
  // worktree, and no agent has been seen working, nor the worker answered, since: no turn
  // ever read the brief, so the next request starts it again on the brief, never with
  // `--continue`. Kept across `exited`.
  neverBegan?: boolean;
  // When the PR opened, or reopened, while its agent ran: the worker is finishing, and exits
  // once it has answered, so the PR isn't read, nor a round started into the exit, for an
  // hour from then, unless the agent leaves the agent states or is given a request first.
  finishing?: number;
  // The head whose `pending` deadline fired, until the PR's next read: that read tells the
  // owner if its checks are still pending.
  pendingDue?: string;
  // When the cancel was received, for `halted`'s 24 hours after it while the issue is cancelled.
  cancelledMs: number | null;
};

// `receivedMs`: when the event was received, which a turn's end is judged by.
export type Waiting = { event: Inbound; cls: number; receivedMs: number };

// "work", a session's ID, or "issue" for a command of the issue's own that no machine holds.
export type MachineRef = string;

export type CommandResult = Result | "lost";

export type Command = {
  id: string;
  issue: string;
  kind: CommandKind;
  machine: MachineRef;
  version: number;
  params: Record<string, unknown>;
  // Sent beside the one in flight, by a class 1 event.
  class1: boolean;
  // When first and last sent, and how many times; null while queued.
  firstSentMs: number | null;
  sentMs: number | null;
  sends: number;
  // Its machine moved on under a class 1 event: never sent again, its timeouts run to `lost`.
  superseded: boolean;
  // Answered `retryable`, what it waits on, until it is sent again; `cleared`: its newest send
  // was for that clearing (queue.ts, `resentCleared`).
  waitsOn?: WaitsOn | null;
  cleared?: boolean;
};

// What a retryable command waits on, sent again as soon as it clears (queue.ts, `clearedBy`): a
// paired host, by its ID, that a listing hears, no recipe machine being made, woken or put
// to sleep, or `listen`'s own Orca, which any listing heard.
export type WaitsOn = { host: string } | { lock: true } | { orca: true };
// What cleared: the hosts a listing heard, the lock, as a machine command ended, Orca, as a
// listing came, or anything, as `listen`'s settings changed.
export type Clears = { hosts?: readonly string[]; lock?: boolean; orca?: boolean; all?: boolean };

// An event leaving the store, written with the transition that sends it. Its ID is its
// kind, the sending store and that store's outbox sequence.
export type Outgoing = { id: string; seq: number; to: "coordinator" | string; event: Inbound | CoordinatorEvent };

// Omit, over each member of a union.
export type Unsent<T> = T extends unknown ? Omit<T, "id"> : never;

export type Store = {
  issue: string;
  // The compare-and-swap's revision, moved by every write.
  rev: number;
  facts: IssueFacts;
  sessions: Record<string, Session>;
  work: WorkMachine | null;
  queue: Command[];
  // Events whose transition waits for the queue to empty, with their precedence class and
  // whether their facts were newer.
  waiting: Waiting[];
  seq: number;
  outbox: Outgoing[];
  // The newest outbox sequence taken from each sending store: events from one store
  // arrive in order, so anything at or below it is a redelivery.
  heard: Record<string, number>;
  // The command IDs' counter.
  commandSeq: number;
  // `listen` was found gone while a command was in flight, and hasn't connected since: no
  // command's timeout runs (queue.ts). Cleared once nothing is in flight.
  away: boolean;
  // Since when the issue has been settled (`settled`, step.ts), for its store's retirement.
  settledMs: number | null;
};

type At = { at: string };
type Base = { id: string; issue: string };

export type RequestEvent =
  | (Base & At & { kind: "sessionStarted"; session: string; sessionKind: SessionKind; byOwner: boolean; comment?: string })
  | (Base & At & { kind: "ownerPrompted"; session: string; byOwner: boolean })
  | (Base & At & { kind: "ownerStopped"; session: string; byOwner: boolean })
  | (Base & At & { kind: "blockersChanged"; blocker: string; done: boolean })
  // `terminal`: the one whose close ended it, when its close did.
  | (Base & { kind: "turnEnded"; session: string; terminal?: string })
  // A session's turn's agent, in `terminal`, is done with its input, at `at` on the
  // workstation's clock.
  | (Base & At & { kind: "turnIdle"; session: string; terminal: string })
  | (Base & { kind: "workerRead"; session: string; reader: Reader; through: string })
  | (Base & { kind: "workChanged"; from: WorkState | null; to: WorkState; version: number; noticed: string | null })
  // `host`: where the slot is, this machine when absent.
  | (Base & { kind: "slotGranted"; session: string; answers: string; host?: string })
  | (Base & { kind: "slotWaiting"; session: string })
  // The issue's slot moved to `host`, this machine when absent, its own having closed.
  | (Base & { kind: "slotMoved"; host?: string })
  // `left` says the sub-issue moved off this parent, which no longer counts it.
  | (Base & At & { kind: "childChanged"; child: string; done: boolean; triage?: boolean; left?: boolean });

export type WorkEvent =
  | (Base & At & { kind: "prOpened"; pr: number; head: string; repo: string })
  // `before` is the head a webhook's push replaced; null from an `evaluatePr` read.
  | (Base & At & { kind: "prPushed"; pr: number; head: string; before: string | null })
  // A conversation comment names no commit (`sha` null): it is kept against `pr`'s head as
  // held when it arrives.
  | (Base & At & { kind: "prReviewed"; pr: number; sha: string | null; reviewer: boolean; copilot: boolean })
  | (Base & At & { kind: "ownerCommented"; comment: string })
  | (Base & At & { kind: "checksCompleted"; sha: string; result: Checks })
  // `head` is the PR's at its merge, absent from an event stored before it was carried.
  | (Base & At & { kind: "prMerged"; pr: number; head?: string })
  | (Base & At & { kind: "prClosed"; pr: number })
  | (Base & At & { kind: "autoMergeDisabled"; pr: number })
  // The worker's own report, in `session`, that GitHub refused auto-merge on `pr` as mergeable
  // already: its merge is asked for all the same.
  | (Base & { kind: "mergeAsked"; session: string; pr: number })
  // The worker's own report, in `session`, that it pushed `head` to the default branch with
  // no PR.
  | (Base & { kind: "landed"; session: string; head: string })
  | (Base & { kind: "basePushed"; base: string })
  | (Base & { kind: "agentWorking" | "agentBackground" | "agentIdle" | "agentWaiting" | "agentExited" | "worktreeGone" })
  // `host`: where its grant is, for a start; this machine when absent.
  | (Base & { kind: "workRequested"; session: string; at: string; request: RequestKind; orchestrate: boolean; host?: string })
  | (Base & At & { kind: "stopRequested"; session: string })
  // `ask` from `tolkster issue chain` to the parent; `forward` from the parent to each
  // sub-issue; `answer` back from each, saying whether it took the chain.
  | (Base & { kind: "chainRecorded"; role: "ask"; session: string; run: string; children: string[] })
  | (Base & { kind: "chainRecorded"; role: "forward"; parent: string; run: string })
  | (Base & { kind: "chainRecorded"; role: "answer"; child: string; run: string; took: boolean });

export type SharedEvent =
  | (Base & At & { kind: "delegateChanged"; by: DelegatedBy })
  // `from` is the parent a webhook's move took the issue off.
  | (Base & At & { kind: "issueStateChanged"; state: IssueStateType; triage?: boolean; parent: string | null; from?: string })
  | (Base & At & { kind: "workerAnswered"; session: string; reader: Reader })
  | (Base & At & { kind: "parentHalted"; parent: string })
  // The issue's own priority from its webhook, or, naming `parent`, its parent's, which the
  // parent's store forwards to the sub-issues it has read.
  | (Base & At & { kind: "priorityChanged"; priority: number; parent?: string })
  | (Base & { kind: "deadline"; machine: MachineRef; version: number; deadline: DeadlineKind })
  // The Durable Object, as `listen` connects, or as its alarm finds nothing listening.
  | (Base & { kind: "listenConnected" | "listenGone" })
  | (Base & { kind: "commandResult"; command: string; result: CommandResult; data?: ResultData });

export type Event = RequestEvent | WorkEvent | SharedEvent;
// What a store receives: an event, or a coordinator's answer to it.
export type Inbound = Event;

// What a command's result carries, beyond its outcome.
export type ResultData = {
  // readIssue
  read?: IssueRead;
  // startWorker, resumeWorker (a parent's, in a new worktree)
  worktree?: { id: string; branch: string } | null;
  madeWorktree?: boolean;
  // startWorker, resumeWorker, startRound: an agent was launched on its prompt, rather than
  // a line typed into one whose turn was seen begin.
  launched?: boolean;
  // deliverMessage, startRound, failed: a line was typed without the receipt's proof (a
  // message's input taken, a round's turn begun), and may still land. A round's stays in
  // flight; a comment's is told again by the next one.
  typed?: boolean;
  // stopWorker, startRound, failed: the worker's agent runs, but nothing names it to close or
  // type to. The owner is told once.
  unnamed?: boolean;
  // A worker's command, retryable: its recipe machine is waking, so the send is given back
  // (step.ts, WAKE_WAIT_MS).
  waking?: boolean;
  // Retryable: what it waits on.
  waitsOn?: WaitsOn;
  // startTurn
  terminal?: string;
  // startWorker, resumeWorker, startRound, startTurn, failed: `listen` told the session itself,
  // asking for a reply, so the machine posts no notice of its own for it.
  said?: boolean;
  // evaluatePr
  pr?: PrRead;
  // updateBranch, mergePr, ok: the head had moved from the one it was sent for, so nothing was
  // written.
  moved?: boolean;
  // updateBranch, failed: GitHub refused the update at the head it was sent for, as for a
  // conflict, so a worker is needed. mergePr, failed: GitHub or the owner's hold refused it.
  refused?: boolean;
  // reapWorktree: false when it kept a worktree unclean or unpushed
  reaped?: boolean;
  // readComment: what the comment is, the newest owner comment's `createdAt` at the read,
  // and the issue's state; for a `noteReply` or `noteRoot`, its thread's root.
  comment?: { kind: CommentKind; newestOwnerAt: string | null; state: IssueStateType; root?: string };
};

export type IssueRead = {
  updatedAt: string;
  delegate: DelegatedBy;
  state: IssueStateType;
  // Open in Triage. Absent from a `listen` older than it, which counts as false.
  triage: boolean;
  stateChangedAt: string;
  priority: number;
  blockers: { id: string; done: boolean; updatedAt: string }[];
  moreBlockers: boolean;
  parent: { id: string; delegated: boolean; by: DelegatedBy; state: IssueStateType; priority: number; updatedAt: string } | null;
  children: { id: string; done: boolean; triage: boolean; updatedAt: string }[];
  moreChildren: boolean;
  repo: string | null;
  // `inDescription`: the owner's mention that began it is in the issue's description, which
  // a `sessionStarted` can't tell from a delegation (`src/worker/events.ts`). Absent
  // otherwise.
  sessions: { id: string; kind: SessionKind; createdAt: string; inDescription?: boolean }[];
};

export type PrRead = {
  // The PR read, its state and head, and when that state began on GitHub's clock, as its
  // lost event would carry it: `merged_at`, `closed_at`, or an open PR's `updated_at`.
  number: number;
  status: PrStatus;
  at: string;
  head: string;
  mergeState: MergeState;
  base: string;
  // Auto-merge is off now and was turned off after being on: off from the start is the
  // worker's to report, not a notice's.
  autoMergeOff: boolean;
  // Auto-merge is on now. Absent from a CLI older than the field.
  autoMerge?: boolean;
  // GitHub's own `CLEAN`: mergeable now, nothing it requires left. Absent from a CLI older than
  // the field, as false.
  mergeable?: boolean;
  manualMerge: boolean;
  checks: Checks | "pending";
  // The newest review or comment from the repo's `reviewers`, and the newest of those not
  // Copilot's, null when there is none.
  feedback: { at: string; reviewerAt: string | null } | null;
  // Copilot's newest review is a comment on this head, and with no thread or approval open
  // and its checks passed, GitHub still holds the merge: it waits for a person.
  copilotWontApprove: boolean;
  maxRounds: number;
};

// The coordinator's events. Each carries the sender's ID.
export type CoordinatorEvent =
  // `rerank`: asked again for a new priority alone, which a grant held already answers. `repo`
  // and `on` as the coordinator's place has them (coordinator.ts, Place).
  | { kind: "slotAsked"; id: string; issue: string; session: string; priority: number; at: string; repo?: string | null; on?: string | null; rerank?: true }
  // `host`: its worktree's, null for this machine, absent while it has none.
  | { kind: "slotHeld"; id: string; issue: string; host?: string | null }
  | { kind: "slotReleased"; id: string; issue: string; session: string | null }
  | { kind: "blockersRead"; id: string; issue: string; sentMs: number; blockers: string[] }
  | { kind: "blockerChanged"; id: string; issue: string; done: boolean; at: string }
  | { kind: "prRecorded"; id: string; issue: string; repo: string; open: boolean }
  | { kind: "repoPushed"; id: string; repo: string; base: string };
