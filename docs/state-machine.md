# The state machine

The spec `src/worker/machine/` is built to: a **request** per session and the **work** per
issue, each in its issue's store in the Durable Object, the **coordinator** for what crosses
issues, and the owner's one **triage**. `tolkster listen` carries out their commands and
reports back as events. Tests hold these tables to `test/contract.json`'s names, take one
case per transition row, and map each [invariant](#invariants) to a check
(`test/machine-sim.test.ts`), so a new one needs its entry there.

Every trigger ends with work under way, or with a clear, accurate message in Linear and a
way to resume: the owner's reply, or delegating again.

## The machines

A request machine begins at `requested` with its session's first `sessionStarted` or
`ownerPrompted`, so a session whose start was lost still asks. A work machine begins at
`launching` with the first `workRequested`, again after `reaped`, and at `reaped` with an
`ownerCommented` on an issue with none.

**Versions and deadlines.** Every transition that writes moves its machine's version;
"unchanged" with "none" writes nothing. A machine holds at most one deadline of each kind:
its state's own, and on the work `recheck`, `takeBack` and `pending`. Entering a state arms its own.
`prOpened`, `prPushed` and a worker begun on an open PR arm `pending` (1 h) on work not ended, which survives a state change as
`takeBack` does and goes with a merge or close: no worker waits on CI, so it reads a head that never settles.

**A note's thread**, the owner's reply to a comment of Tolkster's that no session began
on, or their comment on an issue Tolkster created (`noteRoot`), is a request machine of its own (kind `note`), keyed by its root comment.

### Requests

One per session, holding its newest request, begun by its start or prompt; a stop in a
session with none is told it stopped. Linear opens no session for a delegation on an issue
that has one, so an owner's delegation no machine takes (the store retired or reset)
begins one on the session rule 3 asks, the owner's first: one the store keeps, or, once the
store has sent a command, one an issue `readIssue` lists. A session's own events, landed or
to come, begin it. After a reset, [the rebuild](state.md#the-reset) begins a chain's
delegation the same way while its chain holds.

| State | Means | Deadline |
| --- | --- | --- |
| `requested` | Awaits `readIssue` for the decision. | 15 min: `readIssue`. |
| `queued` | Waits for what its `waiting` names: a slot, a held worker, a reap or the issue's blockers. | 5 min: `slotAsked` for a slot, `readIssue` for blockers. |
| `claimed` | Given to the worker or the session's turn. | 1 h: `observe`. |
| `done` | Answered, stopped, superseded, refused or said why. | None: at rest. |
| `stopping` | The owner's stop of the turn. | 2 min: `stopTurn`. |

Any turn is **warm** once its Claude is idle with the request it was given answered
(`turnIdle`), and keeps its own `warm` deadline in any state: 5 min on, the prompt cache's
life, it is closed (`stopTurn` with `idle`), and the close is sent again every 2 min until
its end lands.

### Work

One per issue. A PR is a fact, not a state. A landing (`landed`: the worker pushed to the default
branch with no PR) is the work's merge, over a PR's too. It drops the PR held, and PRs count
again once a new worker begins or the worker is given another request. One with no worker left is
stale.

| State | Means | Deadline |
| --- | --- | --- |
| `launching` | A start, resume or round sent (`launch`). | A start's: none, its `lost` ends it; otherwise 10 min. |
| `running` | The agent works, or waits on a call it sent to the background (`agentBackground`). | 1 h: `observe`, `readIssue`; 5 min after a launch until it is seen working. |
| `idle` | Paused, terminal open. | 10½ min: the paused notice; then 1 h. |
| `held` | At a prompt or question. | 10 min: the held notice; then 1 h. |
| `exited` | Exited unanswered, or its start failed: `stalled` once it says so, or leaves it said. | 10 min: the stalled notice; then 24 h: `evaluatePr`, `readIssue`. |
| `rest` | Exited after answering. | 6 h with its PR open, else 24 h: `evaluatePr`, `readIssue`. |
| `halted` | Stopped, halted or cancelled. | 2 min while unconfirmed; then 24 h, from the cancel while the issue is cancelled: reaped unless a PR is open. |
| `merged` | Merged or landed, being reaped. | 2 min while a stop is unconfirmed; then 10 min. |
| `reaped` | Worktree gone. | None: at rest. |

`launching`, `running`, `idle` and `held`, **the agent states**,
each take a slot; a turn takes none. Every change of work state sends `workChanged` to the
issue's request machines.

### Triage

One for the owner, beside the coordinator (`machine/triage.ts`), for [triage](triage.md). It
keeps the newest 50 issues created in Triage that no start has settled, for the next
`startTriage` to read beside the queue's one page.

| State | Means | Deadline |
| --- | --- | --- |
| `idle` | Nothing runs or is owed. | None: at rest. |
| `starting` | `startTriage` sent. | None: its timeouts end it. |
| `running` | A session runs. | 10 min: `observeTriage`. |
| `retrying` | A start failed. | 30 s, doubling to 15 min; `idle` after 20 in a row. |

### Fleet

One for the owner (`machine/fleet.ts`), deciding every recipe machine ([Hosts](hosts.md)). A
pure reconcile after every report, event and alarm sends at most one machine command at a time
(`fleet.<n>`), none while one is in flight or a make or wake it began runs. In order: a machine
a grant, a waiting command or a pinned request needs is woken; a repo's first request with
nowhere to go wakes an asleep machine of its repo, else makes one, neither while one awake and paired has been silent under 3 minutes; a machine marked awake but
silent 3 minutes is woken, once in 30; a host idle its minutes is slept, and a machine dormant
its `destroyAfter` destroyed, each clock running while every complete listing names it; an
asleep machine off its recipe's revision is updated, once in 30. A
failed sleep goes again 10 minutes on, a destroy an hour on; a make not begun or failed, or a
wake failed or in `listen`'s cool-down, cools its repo 10 minutes; a wake kept otherwise holds
its machine 30; a complete listing frees the lock. Asking none, it records why per repo for the frame: `hereHasRoom`,
`hostHasRoom`, `running`, `hostComing`, `allMade`, `coolingDown` or `noRecipe`.

## Stores

**Each issue has a store**: its machines, their facts, its command queue and deadlines.
One with nothing owed is **settled**, and 8 days on, past the intake's replay window, the
alarm shrinks it to counters that never go back, so its IDs never repeat.

**The coordinator** holds the slot ledger (`slots`, and each issue's one grant, on a host,
reserved for a request or held by its work), the queue (by `priority`, then oldest,
skipping an issue that holds a grant), the blocker index and the open-PR index.

| Event | Does |
| --- | --- |
| `slotAsked` | A place held takes the priority asked, keeping its age; a re-rank from a request already granted gets no answer. Reserves each place in turn a slot on a host with room, `slotGranted` naming it; else `slotWaiting`. |
| `slotHeld` | Holds the reservation, or a slot past the cap, on its worktree's host. |
| `slotReleased` | Gives back what it names, then grants each place with room. |
| `blockersRead` | Replaces the issue's entries, unless a read sent later has. |
| `blockerChanged` | `blockersChanged` to each issue blocked. |
| `prRecorded` | Updates the open-PR index. |
| `repoPushed` | `basePushed` to each issue with a PR open there. |

All stores and the coordinator live in the owner's one Durable Object, which runs one event
at a time and answers a coordinator's event in the step that sent it, so no two decisions
count the same free slot.

**A grant is never stranded.** A grant not yet on its worker's worktree leaves a host closed to its repo, never a silent
one (`slotMoved`).
A request holds a reservation or a place only while it
waits for a slot, and a reservation only 30 minutes unused; it gives back what it may not
hold after each step. A `claimed` that sends `workRequested` hands its
reservation to the work.

## Events

Each event is mapped to its issue and deduped by kind, delivery ID and issue at intake
(`src/worker/intake.ts`). A commit status names no PR: it maps by SHA to the issue whose `pr`
has it as head.

The owner check is at ingress: a session, prompt or delegation by anyone else makes no
event. The one exception is a chain: a delegation's session on a sub-issue whose `chain`
holds makes a `sessionStarted`, and Tolkster's delegation of it a `delegateChanged`, which
rule 2 checks again. A chain's session whose chain no longer holds makes none; the relay answers it.

| Event | Machine |
| --- | --- |
| `sessionStarted` | request |
| `ownerPrompted` | request |
| `ownerStopped` | request |
| `delegateChanged` | request, work |
| `issueStateChanged` | request, work |
| `blockersChanged` | request |
| `prOpened` | work |
| `prPushed` | work |
| `prReviewed` | work |
| `checksCompleted` | work |
| `ownerCommented` | work |
| `prMerged` | work |
| `prClosed` | work |
| `autoMergeDisabled` | work |
| `mergeAsked` | work |
| `basePushed` | work |
| `agentWorking` | work |
| `agentBackground` | work |
| `agentIdle` | work |
| `agentWaiting` | work |
| `agentExited` | work |
| `worktreeGone` | work |
| `landed` | work |
| `turnEnded` | request |
| `turnIdle` | request |
| `priorityChanged` | request |
| `workerRead` | request |
| `workerAnswered` | request, work |
| `deadline` | request, work, triage |
| `commandResult` | request, work, triage |
| `workRequested` | work |
| `stopRequested` | work |
| `workChanged` | request |
| `slotGranted` | request |
| `slotWaiting` | request |
| `childChanged` | request |
| `parentHalted` | request, work |
| `chainRecorded` | work |
| `triageAsked` | triage |
| `triageEnded` | triage |
| `listenConnected` | request, triage |
| `listenGone` | request, triage |

**A read lands as the events it shows.** A `readIssue` result is the `delegateChanged`,
`issueStateChanged`, `priorityChanged` (its own and its parent's), `blockersChanged` and `childChanged` its webhooks would be, at their
own `updatedAt`s, and an `evaluatePr` result the `prMerged`, `prClosed` or `prPushed` it
shows; each goes through `mergeEvent`, so a read repairs a lost webhook as the webhook
would have. The reader's own row runs after those that run at once, only if they left its machine
where it was; one waiting behind a command runs after it. Repo and sessions, which no event carries, are merged as read.
A parent's store forwards its priority to the sub-issues it has read; a sub-issue without
one, moved while waiting for a slot, re-ranks, read first under a new parent.

## Commands

A step's commands carry the issue, the machine's version and an ID, and queue on the issue,
sent to the newest connection only; triage's and the fleet's commands share its
code (`machine/queue.ts`). A class 1 event's are sent at once ([Precedence](#precedence)),
and once one moves a machine to a new state, its command in flight, unless a stop or
`takeBackChain`, is never sent again: its timeouts run on to `lost`, even while `listen` is
away. Shapes are `Command` and `PrRead` in `types.ts`.

A command with no result in its timeout
(2 min; 10 for `startWorker` and `startTriage`) is sent again with its ID, three times,
then ends `lost`, applied as `failed`, though it may have run: `startWorker`'s leaves
`exited`, and `startTurn`'s holds its claim until `observe` finds or ends the turn. `listenConnected` sends a command held away again as one of its three. One answered `retryable` names its wait (`waitsOn`: a host, recipe-machine lock, or Orca) and goes again once that clears (a listing begun after its answer hears host or Orca, a machine command ends, or `listen`'s settings change), its send unspent unless its last was one too. One `waking` gets its send back for 30 minutes.

**A stop owed.** In `halted`, `merged` and `stopping`, a halt's, cancel's or
owner's stop goes every 2 minutes until confirmed by the newest stop's success,
or the agent or turn seen closed since it was sent (a resend with nothing begun keeps that
time). Leaving them drops it.

| Command | Does |
| --- | --- |
| `readIssue` | Reads the issue's facts. |
| `startWorker` | Makes the worktree and launches the agent. |
| `deliverMessage` | Types a fixed line into the worker. |
| `readComment` | Tells what kind of comment it is. |
| `resumeWorker` | The harness's resume line, or a parent's new worker in a new worktree ([Parent issues](parent-issues.md)). |
| `startRound` | A review round. |
| `stopWorker` | Closes the worker's terminal and says "Stopped." |
| `startTurn` | A conversation's turn; its stdin holds only the owner's own messages. A session's turn open in the checkout is handed it instead, its time typed into the turn's terminal; one not typed into is closed, and a turn opens in its place. |
| `stopTurn` | Closes the turn's terminal silently; `idle` also reports a gone one ended. |
| `evaluatePr` | Reads the PR as the app. |
| `observe` | Reports the agents and turn. |
| `closeAgent` | Closes an idle or held agent. |
| `reapWorktree` | Removes the worktree by `rule` ([Reaping](dispatcher.md#reaping)). |
| `postNotice` | One fixed message of [Tolkster's voice](voice.md). |
| `takeBackChain` | Takes back the chain's delegations. |
| `startContext` | A [context](context.md) session; moves no machine. |
| `startTriage` | Opens a triage session. |
| `observeTriage` | Whether it is still open. |
| `updateBranch` | Brings the PR up to date with its base as the app, leased on the head read. |
| `mergePr` | Merges the PR as the app at the head read; merged already is done. |
| `makeHost` | Makes a recipe machine for a repo, or says it didn't begin. |
| `wakeHost` | Wakes a recipe machine, or says it didn't begin. |
| `suspendHost` | Puts an idle recipe machine to sleep, or keeps it. |
| `updateHost` | Moves an asleep recipe machine onto its recipe's image, or keeps it. |
| `destroyHost` | Destroys a dormant recipe machine, or keeps it. |
| `postAway` | The away message. |

A notice's kinds: `blocked`, `busy`, `held`, `notDelegated`, `noLongerMine`,
`cancelled`, `alreadyDone`, `noRepo`, `stopped`, `stalled`, `paused`, `heldAtPrompt`,
`couldntPass`, `roundsCap`, `prClosed`, `autoMergeOff`, `ciCouldntRun`, `checksPending`, `copilotWontApprove`, `stopUnnamed`, `roundUnnamed` and `unheard`. Each is
keyed (request and blockers, or PR and head) and sent once; one that fails goes once more, then its key clears. A result
`said` means `listen` asked already: no notice follows. Work notices go to the
worker's latest requesting session, and a pause's to every unanswered claim on it too.

## Facts

Each fact keeps the clock it came with (`machine/facts.ts`), and a row's **When** reads
facts as they were before its event, so arrival order doesn't change the result.
Linear's times compare as ISO strings. `rounds` keeps the last round's feedback time and base. History can lag a delegation, and delegating an issue already Tolkster's writes no entry, so a read finding no entry says
`unknown`, never someone else; `chain` when Tolkster's record of its chain's delegation is newer than any take-away read; `created` when none changed the delegate of an issue the owner created. An `aged` or `created` read keeps a recorded chain's. A create delegated to Tolkster is a `delegateChanged`.

## Precedence

An event updates its facts at once. A class 1 event's transition runs at once too, beside
the command in flight, and drops what is queued for a machine it moves but `readIssue`,
`readComment`, `postNotice`, `takeBackChain` and a stop its state holds; so does a
machine's own deadline with a stop owed. Any other event waits for an empty queue, then waiting
events apply one at a time, in this order:

1. The owner's stop, an undelegation, a cancel and a parent's halt.
2. The owner's requests: a session's start, a prompt, a delegation to Tolkster, and the
   issue's other Linear facts (completion, reopening, blockers, a comment).
3. PR facts and a landing.
4. Orca's observations, a turn's end and the worker's own `tolkster` reports, but a
   chain's.
5. Deadlines.

The wait isn't redundant with the rows that leave a command's result to decide: an exit
beside a request's `deliverMessage` turns on whether the line typed anything, which only
its result says, and observations, prompts and deadlines beside a launch or a turn's start
need its result first too.

The six PR facts that ask for `evaluatePr` (a push, a review, a check, auto-merge off, a
merge asked, a base push) have no transition: each moves `prVersion`, and one check ending every step, with
the queue empty, reads the open PR while the work is `running`, `idle`, `exited` or `rest` and its facts are newer
than its last read. Within a class, events go by arrival. `workRequested`,
`stopRequested`, `workChanged`, `chainRecorded` and the coordinator's events apply in the
step that sent them.

## The decision

A `requested` request's `readIssue` result is decided in this order; the first that holds
wins:

1. Not the request the read was sent for: nothing.
2. Neither the owner's nor backed by them (a chain's delegation whose `chain` names the
   `parent`, read as delegated to Tolkster and not cancelled, the issue delegated by no one
   else since, or the sub-issues' end or added of such an orchestration): `done`, silently. The sub-issues added, none added
   or a worker in the agent states: `done`.
3. A delegation in a session it doesn't ask: `done`. A delegation asks the newest
   delegation's session (a chain's only while its chain holds), or the newest mention's when every
   session is a mention's.
4. A turn runs in this session: `claimed`; decided again when it ends. A warm one is handed
   an owner's message that rule 11 gives a turn at once (`startTurn` with `warm`), and closed
   for any other request.
5. A mention's or thread reply's session is a **conversation** when the issue isn't
   delegated by the owner, is blocked or finished, and a note's always; else **work**.
6. Work delegated by `unknown`: read again every 30 s for 2 minutes, then as anyone else's, or the owner's for `created`, which later `created` reads keep; a `created` read keeps someone else's delegation or a take-away held.
   Work not delegated by the owner or a chain that passed rule 2: `done`, `notDelegated`
   or `noLongerMine`. The wait comes before rule 5, for every session but a note's, and in
   place of rule 2's refusal of a chain's session.
7. Work on a finished issue: `done`, `cancelled` or `alreadyDone`.
8. The worker stopped or halted after this request: `done`, `stopped` unless its stop said so; a conversation goes on.
9. A work machine short of `reaped`: `held`, `queued` with `held`; another agent state,
   `claimed` with `workRequested`; `merged`, `queued` until reaped; at rest, as rule 10's slot, as blockers and repo
   hold back only a new worker.
10. No worker: unfinished blockers, or past the 50 read, `queued` with `blockers` and the `blocked` notice; no repo, `done` with `noRepo`; a reserved slot
    (`grant`), `claimed` with `workRequested`; else `queued` with `slotAsked`. This read is
    the re-check before the claim.
11. A conversation: as rule 9 unless the work is `merged`, `reaped` or halted after the
    request, or the session is a note's; else `claimed`, with `startTurn`.

A start or resume on an issue with unfinished `children` is an orchestration, recorded as
the session's `orchestrated`.

## Parent issues

Each step between parent and sub-issue stores is an event (`machine/chain.ts`, [Parent
issues](parent-issues.md)):

- **The chain.** `tolkster issue chain` sends `chainRecorded` to the parent's store, taken
  only from the `orchestrated` session while the work is in the agent states, delegated,
  not cancelled and with no `takeBackChain` queued; otherwise `issue chain` exits 1 having
  delegated nothing. The store forwards `chain` to each sub-issue named, which takes it
  unless its own `parent` or `chain` names another, or a worker still runs for a chain it
  left. `chained` records those that took it, and `issue chain` delegates only those. A
  sub-issue Tolkster still holds whose chain session no longer runs is taken back first, so
  Linear opens it a new session.
- **`childChanged`.** Every issue with a `parent` sends it with each `issueStateChanged`,
  as the parent's `readIssue` shows. An issue that learns it left a parent tells
  that parent, which drops it; if that parent's chain delegated it, both end the
  chain; it halts its own `chained` unless the owner delegated it.
- **The sub-issues' end.** Once every sub-issue has finished or is in Triage and the
  session's newest request is answered, the `orchestrated` session asks again.
- **A sub-issue added.** One pending, not in `orchestrated`'s `known` nor in `chained`,
  makes the session ask again; its decision resumes the worker or ends `done`.
- **Stopping the chain.** A parent taken from Tolkster or cancelled sends `parentHalted` to
  each sub-issue in `chained`, and owes `takeBackChain` until it succeeds; a later
  `chainRecorded` ends one owed. A sub-issue's `parentHalted` clears its `chain` and halts
  a worker its chain's session started, unless the sub-issue is completed, and passes the
  halt on to its own `chained`.

## Request transitions

Each row is a (state, event) pair, or several; a pair's rows are read in order, and the
first whose **When** holds wins. A new state arms its own deadline; an "unchanged" row
names any it arms, and otherwise keeps what is set.

| State | Event | When | New state | Commands |
| --- | --- | --- | --- | --- |
| `requested`, `queued`, `claimed`, `done`, `stopping` | `sessionStarted` | A redelivery the dedupe missed | unchanged | none |
| `requested`, `queued`, `claimed`, `done`, `stopping` | `ownerPrompted` | Not newer | unchanged | none |
| `requested`, `queued`, `claimed`, `done`, `stopping` | `ownerPrompted` | Newer | `requested` | `readIssue` |
| `requested`, `queued`, `claimed`, `done`, `stopping` | `ownerStopped` | Not newer | unchanged | none |
| `stopping` | `ownerStopped` | A second stop while the first runs | unchanged | none |
| `requested`, `queued`, `claimed`, `done` | `ownerStopped` | A turn runs in this session, or its `startTurn` is in flight | `stopping` | `stopTurn` |
| `requested`, `queued`, `claimed`, `done` | `ownerStopped` | The issue's worker runs, and this session gave it a request | `done` | `stopRequested` |
| `requested`, `queued`, `claimed`, `done` | `ownerStopped` | Otherwise | `done` | `postNotice` (`stopped`) |
| `queued` | `delegateChanged` | Taken from Tolkster, and the request is work | `done` | `postNotice` (`noLongerMine` or `notDelegated`) |
| `requested` | `delegateChanged` | Delegated to Tolkster by the owner or a chain, newer, while the decision waits on who delegated | unchanged; 15 min | `readIssue` |
| `requested`, `queued` | `delegateChanged` | Otherwise | unchanged | none |
| `done` | `delegateChanged` | Delegated to Tolkster | `requested` | `readIssue` |
| `done` | `delegateChanged` | Otherwise | unchanged | none |
| `claimed` | `delegateChanged` | Delegated to Tolkster, newer, mid-turn, not a note's: kept as the request | unchanged | none |
| `claimed`, `stopping` | `delegateChanged` | The work machine halts the worker | unchanged | none |
| `queued` | `issueStateChanged` | Completed or cancelled, and the request is work | `done` | `postNotice` (`alreadyDone` or `cancelled`) |
| `requested`, `queued` | `issueStateChanged` | Otherwise | unchanged | none |
| `claimed`, `done`, `stopping` | `issueStateChanged` | The work machine acts on a cancel | unchanged | none |
| `queued` | `blockersChanged` | Waiting on blockers, every one completed or cancelled | `requested` | `readIssue` |
| `queued` | `blockersChanged` | Waiting on blockers, a different set still unfinished | unchanged | `postNotice` (`blocked`) |
| `queued` | `blockersChanged` | Waiting on blockers, the same set | unchanged | none |
| `requested`, `queued`, `claimed`, `done`, `stopping` | `blockersChanged` | Otherwise: blockers hold back only a new worker | unchanged | none |
| `requested`, `queued`, `claimed`, `done`, `stopping` | `turnEnded` | From a terminal a newer turn replaced | unchanged | none |
| `claimed` | `turnEnded` | A kept delegation another session asks waits, and the turn didn't answer | `requested` | `postNotice` (`stalled`), `readIssue` |
| `claimed` | `turnEnded` | A request newer than the turn's start waits | `requested` | `readIssue` |
| `claimed` | `turnEnded` | A note's thread's turn that ran answered, but was never shown its request | `requested` | `readIssue` |
| `claimed` | `turnEnded` | The turn answered its request, or marked it needing none | `done` | none |
| `claimed` | `turnEnded` | The turn ended without answering | `done` | `postNotice` (`stalled`) |
| `stopping` | `turnEnded` | The turn being stopped ended, received after the stop owed was sent | `done` | `postNotice` (`stopped`) |
| `stopping` | `turnEnded` | Received before | unchanged | none |
| `requested`, `queued`, `done` | `turnEnded` | No turn holds the request: a kept turn is forgotten | unchanged | none |
| `claimed` | `turnIdle` | A working turn answered the request it was given, and a newer one waits | `requested` | `readIssue` |
| `requested`, `queued`, `claimed`, `done`, `stopping` | `turnIdle` | A working turn answered the request it was given: warm | unchanged; warm, 5 min | none |
| `requested`, `queued`, `claimed`, `done`, `stopping` | `turnIdle` | A working turn stopped short of answering | unchanged; warm, 2 min | `stopTurn`, idle |
| `requested`, `queued`, `claimed`, `done`, `stopping` | `turnIdle` | Otherwise: no turn, one warm or closing, or from another terminal (a turn whose lost start named none takes it) | unchanged | none |
| `queued` | `workerRead` | Waiting on blockers or the merge: the read is a fact | unchanged | none |
| `requested`, `queued` | `workerRead` | By the issue's worker or this session's turn, reaching this request | `claimed` | none |
| `requested`, `queued` | `workerRead` | Otherwise | unchanged | none |
| `claimed`, `done`, `stopping` | `workerRead` | The read is a fact | unchanged | none |
| `claimed` | `workerAnswered` | After the request, by a reader whose read reached it | `done` | none |
| `claimed` | `workerAnswered` | Otherwise | unchanged | none |
| `requested`, `queued`, `done`, `stopping` | `workerAnswered` | An answer handles only a claimed request | unchanged | none |
| `queued` | `workChanged` | Its issue's worker left `held`, or its merged worktree was reaped | `requested` | `readIssue` |
| `queued` | `workChanged` | Waiting for a slot, and the work entered the agent states for another session | `requested` | `readIssue` |
| `queued` | `workChanged` | Otherwise | unchanged | none |
| `claimed` | `workChanged` | The work left the slot-taking states, this session's newest request is answered, its `orchestrated` end is owed, and every sub-issue has finished | `requested` | `readIssue`, once |
| `claimed` | `workChanged` | The work left the slot-taking states, this session's request is answered, and a sub-issue was added to its live orchestration | `requested` | `readIssue` |
| `claimed` | `workChanged` | The work left the slot-taking states, and this session's request is answered | `done` | none |
| `claimed` | `workChanged` | The work left them unanswered, and its own notice told this session why | `done` | none |
| `claimed` | `workChanged` | The work left them for `exited` unanswered, after this request's line typed nothing | `requested` | `readIssue` |
| `claimed` | `workChanged` | The work left them unanswered | `done` | `postNotice` (`stalled`) |
| `claimed` | `workChanged` | Otherwise | unchanged | none |
| `done` | `workChanged` | The work left the slot-taking states, and the sub-issues' end is owed, or else a sub-issue was added to this session's live orchestration | `requested` | `readIssue` |
| `requested`, `done`, `stopping` | `workChanged` | Otherwise | unchanged | none |
| `queued` | `slotGranted` | For this request, waiting for a slot, granted in the step whose decision asked for it | `claimed` | `workRequested` |
| `queued` | `slotGranted` | For this request, waiting for a slot, granted later | `requested` | `readIssue` |
| `requested` | `slotGranted` | For this request: the read under way decides | unchanged | none |
| `requested`, `queued`, `claimed`, `done`, `stopping` | `slotGranted` | Otherwise | unchanged | `slotReleased` |
| `queued` | `slotWaiting` | For this request, waiting for a slot | unchanged | `postNotice` (`busy`) |
| `requested`, `queued`, `claimed`, `done`, `stopping` | `slotWaiting` | Otherwise | unchanged | none |
| `queued` | `priorityChanged` | Waiting for a slot | unchanged | `slotAsked` |
| `requested`, `queued`, `claimed`, `done`, `stopping` | `priorityChanged` | Otherwise | unchanged | none |
| `done` | `childChanged` | Every sub-issue has finished or is in Triage (never over 50 sub-issues), this session's `orchestrated` end is owed, and the issue is still delegated to Tolkster and not cancelled | `requested` | `readIssue`, once |
| `done` | `childChanged` | A sub-issue was added to this session's live orchestration, and no worker is in the agent states | `requested` | `readIssue` |
| `requested`, `queued`, `claimed`, `done`, `stopping` | `childChanged` | Otherwise | unchanged | none |
| `requested`, `queued` | `parentHalted` | The session is a chain's delegation | `done` | none |
| `requested`, `queued`, `claimed`, `done`, `stopping` | `parentHalted` | Otherwise: the owner's own sessions stand | unchanged | none |
| `requested`, `claimed` | `deadline` | `quiet`: the issue still holds `listen` away, and the request isn't told | unchanged | `postAway` |
| `requested`, `queued`, `claimed`, `done`, `stopping` | `deadline` | `warm`: the turn warm 5 min, or its close sent 2 min ago | unchanged; warm, 2 min | `stopTurn`, idle |
| `requested` | `deadline` | The read had no result, and a slot was reserved 30 minutes ago or more | unchanged; 15 min | `readIssue`, `slotReleased` |
| `requested` | `deadline` | The read had no result, or the decision waits on who delegated | unchanged; 15 min | `readIssue` |
| `queued` | `deadline` | Waiting on blockers: the blocker check, as no webhook says a relation was removed | unchanged; 5 min | `readIssue` |
| `queued` | `deadline` | Waiting for a slot | unchanged; 5 min | `slotAsked` |
| `queued` | `deadline` | Otherwise | unchanged; 5 min | none |
| `claimed` | `deadline` | The worker's or turn's terminal | unchanged; 1 h | `observe` |
| `stopping` | `deadline` | The stop is owed | unchanged; 2 min | `stopTurn` |
| `done` | `deadline` | At rest | unchanged | none |
| `requested` | `commandResult` | `readIssue` succeeded | as [The decision](#the-decision) | as the decision |
| `queued` | `commandResult` | `readIssue` succeeded: only the events it shows | as those events | as those events |
| `requested`, `queued`, `claimed`, `done` | `commandResult` | A warm turn's `startTurn` succeeded: the turn's request is the one handed | unchanged | none |
| `requested`, `queued`, `claimed`, `done` | `commandResult` | A warm turn's `startTurn` failed or was `lost` | unchanged; warm, 2 min | `stopTurn`, idle |
| `claimed` | `commandResult` | `startTurn` failed | `done` | `postNotice` (`couldntPass`), unless said |
| `stopping` | `commandResult` | The stop owed succeeded | `done` | `postNotice` (`stopped`) |
| `stopping` | `commandResult` | `startTurn` opened a turn other than the one held, or was `lost` | unchanged; 2 min | `stopTurn` |
| `requested`, `queued`, `done` | `commandResult` | `startTurn` opened a turn or was `lost`, with no turn held | unchanged | `stopTurn` |
| `requested`, `queued`, `claimed`, `done`, `stopping` | `commandResult` | Otherwise | unchanged | none |
| `requested`, `claimed` | `listenGone` | A command in flight on the issue, and the request not told | unchanged; quiet, 15 min | none |
| `requested`, `queued`, `claimed`, `done`, `stopping` | `listenGone` | Otherwise | unchanged | none |
| `requested`, `queued`, `claimed`, `done`, `stopping` | `listenConnected` | — | unchanged; quiet dropped | none |

## Work transitions

Read as the request transitions are. Every transition into the agent states from outside
sends `slotHeld`, and every one out of them `slotReleased`.

| State | Event | When | New state | Commands |
| --- | --- | --- | --- | --- |
| `reaped` | `workRequested` | A new worker | `launching` | `startWorker` |
| `running`, `idle` | `workRequested` | — | `running` | `deliverMessage` |
| `exited`, `rest`, `halted` | `workRequested` | No worktree recorded, or its start never began (`neverBegan`) | `launching` | `startWorker`, on the brief |
| `exited`, `rest`, `halted` | `workRequested` | Otherwise | `launching` | `resumeWorker` |
| `launching` | `workRequested` | The worker reads every unanswered request | unchanged | none |
| `held`, `merged` | `workRequested` | The decision queues a request for these | unchanged | none |
| `launching`, `running`, `idle`, `held` | `stopRequested` | — | `halted` | `stopWorker` |
| `exited`, `rest`, `halted`, `merged`, `reaped` | `stopRequested` | No agent runs, or the stop is owed | unchanged | none |
| `launching`, `running`, `idle`, `held` | `delegateChanged` | Taken from Tolkster | `halted` | `stopWorker` |
| `launching`, `running`, `idle`, `held` | `delegateChanged` | Otherwise | unchanged | none |
| `exited`, `rest`, `halted`, `merged`, `reaped` | `delegateChanged` | No agent runs | unchanged | none |
| `launching`, `running`, `idle`, `held` | `parentHalted` | Its `chain` held, the worker's request came from the chain's session, and the issue isn't completed | `halted` | `stopWorker` |
| `launching`, `running`, `idle`, `held` | `parentHalted` | Otherwise | unchanged | none |
| `exited`, `rest`, `halted`, `merged`, `reaped` | `parentHalted` | No agent runs | unchanged | none |
| `launching`, `running`, `idle`, `held`, `exited`, `rest`, `halted`, `merged`, `reaped` | `chainRecorded` | A fact | unchanged | none |
| `launching`, `running`, `idle`, `held` | `issueStateChanged` | Cancelled | `halted` | `stopWorker` |
| `exited`, `rest`, `halted` | `issueStateChanged` | Cancelled: 24 h from now, or 2 min with a stop owed | `halted` | none |
| `exited`, `rest`, `halted` | `issueStateChanged` | Completed, with no PR ever opened | unchanged | `reapWorktree` |
| `launching`, `running`, `idle`, `held`, `exited`, `rest`, `halted` | `issueStateChanged` | Otherwise: a reopened issue's `halted` reads it at its deadline | unchanged | none |
| `merged`, `reaped` | `issueStateChanged` | Already going or gone | unchanged | none |
| `rest` | `prOpened` | Its PR opened, or reopened | unchanged; 6 h | `evaluatePr` |
| `exited` | `prOpened` | Stalled | `rest` | `evaluatePr` if PR facts came since |
| `running`, `idle`, `held` | `prOpened` | Opened or reopened while its agent runs: finishing for an hour, unless it leaves these states or is given a request | unchanged | none |
| `launching`, `exited`, `halted`, `merged`, `reaped` | `prOpened` | A fact | unchanged | none |
| `running`, `idle`, `exited`, `rest` | `prPushed`, `prReviewed`, `checksCompleted`, `autoMergeDisabled`, `mergeAsked`, `basePushed` | Its PR is open, not finishing: evaluated at once | unchanged | `evaluatePr` |
| `running`, `idle`, `exited`, `rest` | `prPushed`, `prReviewed`, `checksCompleted`, `autoMergeDisabled`, `mergeAsked`, `basePushed` | Otherwise, or finishing: no round races its exit | unchanged | none |
| `launching`, `held` | `prPushed`, `prReviewed`, `checksCompleted`, `autoMergeDisabled`, `mergeAsked`, `basePushed` | `evaluatePr` is sent on the next entry into an evaluating state | unchanged | none |
| `halted`, `merged`, `reaped` | `prPushed`, `prReviewed`, `checksCompleted`, `autoMergeDisabled`, `mergeAsked`, `basePushed` | A halted worker is never resumed for review | unchanged | none |
| `launching`, `running`, `idle`, `held`, `exited`, `rest`, `halted`, `merged`, `reaped` | `ownerCommented` | Each comment is read | unchanged | `readComment` |
| `halted` | `prMerged`, `landed` | Its stop isn't confirmed: the reap waits for the close | `merged` | `stopWorker` |
| `exited`, `rest`, `halted` | `prMerged`, `landed` | Otherwise | `merged` | `reapWorktree` |
| `idle`, `held` | `prMerged`, `landed` | `merged`'s deadline closes an idle or held agent | `merged` | none |
| `launching`, `running` | `prMerged`, `landed` | A fact: the agent's exit or idle reaps | unchanged | none |
| `merged`, `reaped` | `prMerged`, `landed` | A replay, or a landing over the merge, reaped at the deadline | unchanged | none |
| `rest` | `prClosed` | — | unchanged; 24 h | `postNotice` (`prClosed`) |
| `launching`, `running`, `idle`, `held`, `exited` | `prClosed` | — | unchanged | `postNotice` (`prClosed`) |
| `halted`, `merged`, `reaped` | `prClosed` | Nothing is removed and no one is told | unchanged | none |
| `idle`, `held` | `agentWorking`, `agentBackground` | — | `running` | `evaluatePr` if PR facts came |
| `exited`, `rest` | `agentWorking`, `agentBackground` | An agent continued by hand, or whose first turn began after its stall | `running` | none |
| `halted`, `merged` | `agentWorking`, `agentBackground` | Its stop isn't confirmed | unchanged; 2 min | `stopWorker` |
| `halted` | `agentWorking`, `agentBackground` | Otherwise: not Tolkster's until a request resumes it | unchanged | none |
| `running` | `agentBackground` | A stop in the background: `observe` waits past the listing's hold | unchanged; 1 h | none |
| `launching`, `running` | `agentWorking`, `agentBackground` | Already running, or the command's result decides | unchanged | none |
| `merged`, `reaped` | `agentWorking`, `agentBackground` | Otherwise: the reap waits for the agent to stop | unchanged | none |
| `running` | `agentIdle` | A launch's first turn not seen yet (`firstTurn`) | unchanged | none |
| `running`, `held` | `agentIdle` | Otherwise | `idle` | `evaluatePr` if PR facts came or the last read found a failed check or a merge |
| `launching`, `idle` | `agentIdle` | Already idle, or the command's result decides | unchanged | none |
| `exited`, `rest`, `halted`, `merged`, `reaped` | `agentIdle` | No worker of Tolkster's runs | unchanged | none |
| `running`, `idle` | `agentWaiting` | — | `held` | none |
| `launching`, `held`, `exited`, `rest`, `halted`, `merged`, `reaped` | `agentWaiting` | Already held, the command's result decides, or no worker runs | unchanged | none |
| `running`, `idle`, `held` | `agentExited` | The work merged | `merged` | `reapWorktree` |
| `running`, `idle`, `held` | `agentExited` | The issue completed with no PR ever opened | `rest` | `reapWorktree` |
| `running`, `idle`, `held` | `agentExited` | The session that last gave it a request is answered | `rest` | `evaluatePr`, with its PR open, if PR facts came or the last read found a failed check or a merge |
| `running`, `idle`, `held` | `agentExited` | Not answered | `exited` | `evaluatePr`, as above |
| `merged` | `agentExited` | The agent exited since its stop owed was sent: the reap follows | unchanged; 10 min | `reapWorktree` |
| `halted` | `agentExited` | The agent exited since its stop owed was sent | unchanged; 24 h, from the cancel while the issue is cancelled | none |
| `merged`, `halted` | `agentExited` | The exit came before the stop owed was sent | unchanged | none |
| `merged` | `agentExited` | Otherwise | unchanged; 10 min | `reapWorktree` |
| `halted` | `agentExited` | Otherwise: the close was confirmed already | unchanged | none |
| `launching` | `agentExited` | The command's result decides | unchanged | none |
| `exited`, `rest`, `reaped` | `agentExited` | A replay | unchanged | none |
| `launching`, `running`, `idle`, `held`, `exited`, `rest`, `halted`, `merged` | `worktreeGone` | Removed by hand, or reaped by someone else | `reaped` | none |
| `reaped` | `worktreeGone` | A replay | unchanged | none |
| `exited` | `workerAnswered` | A late answer; the worker's own clears `neverBegan` in any state | `rest` | none |
| `launching`, `running`, `idle`, `held`, `rest`, `halted`, `merged`, `reaped` | `workerAnswered` | A fact: the exit reads it | unchanged | none |
| `launching`, `running`, `idle`, `held`, `exited`, `rest`, `halted`, `merged`, `reaped` | `deadline` | `takeBack`, with a take-back owed | unchanged; takeBack, 2 min | `takeBackChain` |
| `running`, `idle`, `exited`, `rest` | `deadline` | `recheck`: when finishing, armed again for its end | unchanged | `evaluatePr` |
| `launching`, `running`, `idle`, `held`, `exited`, `rest` | `deadline` | `pending`, its PR open: the head's read is owed, made when the work evaluates, unpaced and not finishing; one failed stays owed, recheck 10 min | unchanged | `evaluatePr` |
| `launching` | `deadline` | A start's own: none is set | unchanged | none |
| `launching` | `deadline` | A resume's: no result came | `rest` | `postNotice` (`couldntPass`) |
| `launching` | `deadline` | A round's: no result came | `rest` | none |
| `running` | `deadline` | Its own, with a launch's first turn never seen and the work merged | `merged` | none |
| `running` | `deadline` | Its own, with a launch's first turn never seen: the agent never took its prompt | `exited`; 24 h | `closeAgent`, `postNotice` (`stalled`) |
| `running` | `deadline` | Its own, otherwise | unchanged; 1 h | `observe`, `readIssue` |
| `idle`, `held` | `deadline` | The work merged | `merged` | none |
| `idle` | `deadline` | Not told yet | unchanged; 1 h | `postNotice` (`paused`) |
| `held` | `deadline` | Not told yet | unchanged; 1 h | `postNotice` (`heldAtPrompt`) |
| `idle`, `held` | `deadline` | Told already | unchanged; 1 h | `observe`, `readIssue` |
| `exited` | `deadline` | Its own, not stalled yet: now `stalled` | unchanged; 24 h | `postNotice` (`stalled`), unless its request, decided again, is still deciding or done |
| `exited` | `deadline` | Its own, stalled: a lost cancel or completion, or PR event | unchanged; 24 h | `evaluatePr` with its PR open, `readIssue` |
| `rest` | `deadline` | Its own: a lost cancel or completion, or PR event | unchanged; 6 h with its PR open, else 24 h | `evaluatePr` with its PR open, `readIssue` |
| `halted`, `merged` | `deadline` | Its stop isn't confirmed | unchanged; 2 min | `stopWorker` |
| `halted` | `deadline` | The issue cancelled, no PR open | unchanged; 24 h | `reapWorktree` |
| `halted` | `deadline` | The issue cancelled, a PR open: Tolkster never closes one | unchanged; 24 h | `evaluatePr` |
| `halted` | `deadline` | Otherwise: a lost cancel or completion, or PR event | unchanged; 24 h | `evaluatePr` with its PR open, `readIssue` |
| `merged` | `deadline` | Otherwise: an agent idle or held for 10 minutes | unchanged; 10 min | `closeAgent` |
| `merged` | `deadline` | An agent at work | unchanged; 10 min | none |
| `merged` | `deadline` | No agent | unchanged; 10 min | `reapWorktree` |
| `reaped` | `deadline` | Otherwise: at rest | unchanged | none |
| `launching` | `commandResult` | `startWorker` named no worktree and found one with no agent, after a start that never began, was `lost` or failed before its worktree was known | unchanged | `startWorker`, naming it, once |
| `launching` | `commandResult` | `startWorker` succeeded | `running` | none |
| `launching` | `commandResult` | `startWorker` `lost`: `neverBegan` is set unless an agent was seen working | `exited`; 24 h | `postNotice` (`stalled`) |
| `launching` | `commandResult` | `startWorker` failed after making the worktree, setting `neverBegan` | `exited`; 24 h | `postNotice` (`stalled`), unless said |
| `launching` | `commandResult` | `startWorker` failed | `reaped` | `postNotice` (`stalled`), unless said |
| `launching` | `commandResult` | A resume's or round's command succeeded | `running` | `evaluatePr` if PR facts came |
| `launching` | `commandResult` | `resumeWorker` failed | `rest` | `postNotice` (`couldntPass`), unless said |
| `launching` | `commandResult` | `startRound` failed with its line typed but no turn seen begin (`typed`): its proposal waits for one | `running` | `evaluatePr` if PR facts came |
| `launching` | `commandResult` | `startRound` failed with no line typed: its proposal is dropped, so nothing is marked or counted | `rest`; 10 min | `postNotice` (`roundUnnamed`) if unnamed |
| `launching` | `commandResult` | `startRound` lost | `rest` | `evaluatePr` |
| `launching`, `running`, `idle`, `held`, `exited`, `rest`, `halted`, `merged`, `reaped` | `commandResult` | `readComment`: a reply to a note of Tolkster's, from the owner, under a week old, on a comment no session began on, or a `noteRoot` with no agent | unchanged | `sessionStarted` or `ownerPrompted`, applied at once |
| `launching`, `running`, `idle`, `held`, `exited`, `rest`, `halted`, `merged`, `reaped` | `commandResult` | `readComment` failed, or was `lost`, the first time | unchanged | `readComment`, once |
| `running`, `idle` | `commandResult` | `readComment`: a plain comment, or a `noteRoot`, newer than `commentTold`, on an unfinished issue, that no session began on | unchanged | `deliverMessage` |
| `running`, `idle` | `commandResult` | `deliverMessage` of a comment's line succeeded, moving `commentTold` | unchanged | none |
| `running`, `idle` | `commandResult` | `deliverMessage` or `startRound` held | `held` | none |
| `running`, `idle` | `commandResult` | `deliverMessage` failed or found no agent (`gone`), marking a request's line that typed nothing | unchanged | `observe` |
| `running`, `idle`, `exited`, `rest` | `commandResult` | `evaluatePr`, and the issue no longer wants work | unchanged | none |
| `exited`, `rest` | `commandResult` | `evaluatePr`: `BEHIND` on a base newer than the last round's, and nothing else new | unchanged | `updateBranch` |
| `exited`, `rest` | `commandResult` | `updateBranch` `refused` at the head it was sent for | `launching` | `startRound` |
| `exited`, `rest` | `commandResult` | `updateBranch` failed otherwise, or lost, at the head it was sent for | unchanged; recheck: 10 min | none |
| `launching`, `running`, `idle`, `held`, `exited`, `rest`, `halted`, `merged`, `reaped` | `commandResult` | `updateBranch` succeeded, unless the head had moved: its base is the last round's | unchanged | none |
| `exited`, `rest` | `commandResult` | `evaluatePr`: feedback or a needed rebase since the last round, or a failed check at a head no round has started on red, under `maxRounds`, or a reviewer's other than Copilot's at the cap; a needed rebase at any count | `launching` | `startRound` |
| `running`, `idle` | `commandResult` | `evaluatePr`: the same, a failed check at `running` aside | unchanged | `startRound` |
| `running`, `idle` | `commandResult` | `evaluatePr`: feedback newer than the last round, on the head a round still in flight began on | unchanged | none |
| `running` | `commandResult` | `evaluatePr`: a failed check | unchanged | none |
| `running`, `idle`, `exited`, `rest` | `commandResult` | `evaluatePr`: Copilot's feedback at the cap, or, unless `running`, a failed check at a head no round has started on red, and no reviewer's since | unchanged | `postNotice` (`roundsCap`) |
| `exited`, `rest` | `commandResult` | `evaluatePr`: auto-merge turned off after being on, without `manual-merge` | unchanged | `postNotice` (`autoMergeOff`) |
| `running`, `idle`, `exited`, `rest` | `commandResult` | `evaluatePr`: CI couldn't run | unchanged | `postNotice` (`ciCouldntRun`) |
| `running`, `idle`, `exited`, `rest` | `commandResult` | `evaluatePr`: the owed `pending` read, its checks still pending | unchanged | `postNotice` (`checksPending`) |
| `exited`, `rest` | `commandResult` | `evaluatePr`: Copilot's newest review only commented on the head, with no open thread or approval and the merge held, without `manual-merge` | unchanged | `postNotice` (`copilotWontApprove`) |
| `idle`, `exited`, `rest` | `commandResult` | `evaluatePr`: GitHub's own `CLEAN`, checks passed, auto-merge on or `mergeAsked`, no hold of the owner's | unchanged | `mergePr` |
| `running`, `idle`, `exited`, `rest` | `commandResult` | `mergePr` failed, not `refused`, or `lost`, at the head held | unchanged; recheck: 10 min | none |
| `running`, `idle`, `exited`, `rest` | `commandResult` | `evaluatePr`: merge state `UNKNOWN`, which GitHub sends no event for once computed | unchanged; recheck: 10, 20, 40 s per base and head | none |
| `halted` | `commandResult` | The stop owed succeeded | unchanged; 24 h, from the cancel while the issue is cancelled | none |
| `merged` | `commandResult` | The stop owed succeeded | unchanged; 10 min | `reapWorktree` |
| `halted`, `merged` | `commandResult` | The stop owed failed, its agent without a handle, once per stop owed | unchanged; 2 min | `postNotice` (`stopUnnamed`) |
| `halted`, `merged` | `commandResult` | A start succeeded or was `lost` | unchanged; 2 min | `stopWorker` |
| `exited`, `rest`, `halted`, `merged` | `commandResult` | `reapWorktree` reaped | `reaped` | none |
| `launching`, `running`, `idle`, `held`, `exited`, `rest`, `halted`, `merged`, `reaped` | `commandResult` | `takeBackChain` succeeded for the run owed | unchanged | none |
| `running`, `idle`, `exited`, `rest`, `halted` | `commandResult` | `evaluatePr` shows the PR merged, closed or at another head | as that event | as that event |
| `running`, `idle`, `held`, `exited`, `rest`, `halted` | `commandResult` | `readIssue`: applied as the events it shows | as that event | as that event |
| `launching`, `running`, `idle`, `held`, `exited`, `rest`, `halted`, `merged`, `reaped` | `commandResult` | Otherwise | unchanged | none |

## Triage transitions

Read as the request transitions are.

| State | Event | When | New state | Commands |
| --- | --- | --- | --- | --- |
| `idle` | `triageAsked` | — | `starting` | `startTriage` |
| `starting`, `running`, `retrying` | `triageAsked` | — | unchanged, kept for the next start | none |
| `running` | `triageEnded` | The session held, ended within 30 seconds of being held | `retrying`, or `idle` at the 20th in a row | none |
| `running` | `triageEnded` | The session held | `starting` | `startTriage` |
| `starting` | `triageEnded` | Before its start's result: the terminal is kept (the newest 16) | unchanged | none |
| `idle`, `starting`, `running`, `retrying` | `triageEnded` | Otherwise | unchanged | none |
| `starting`, `running` | `listenConnected` | A command in flight, held away, that has had its three sends | as its `commandResult` `lost` | as that row |
| `starting`, `running` | `listenConnected` | A command in flight, held away | unchanged | the command, again |
| `starting`, `running` | `listenConnected` | A command in flight otherwise | unchanged | none |
| `retrying` | `listenConnected` | — | `starting` | `startTriage` |
| `idle` | `listenConnected` | An issue asked | `starting` | `startTriage` |
| `idle`, `running` | `listenConnected` | Otherwise | unchanged | none |
| `starting`, `running` | `listenGone` | A command in flight | unchanged | none |
| `idle`, `starting`, `running`, `retrying` | `listenGone` | Otherwise | unchanged | none |
| `idle`, `retrying` | `commandResult` | Not the command in flight, but the `startTriage` that last failed, opening a session after all | `running` | none |
| `idle`, `starting`, `running`, `retrying` | `commandResult` | Not the command in flight: stale | unchanged | none |
| `starting` | `commandResult` | `startTriage` failed, `retryable`, `held` or `lost`, or ok with a session whose end was heard already, the 20th in a row | `idle` | none |
| `starting` | `commandResult` | `startTriage` failed, `retryable`, `held` or `lost`, or ok with a session whose end was heard already | `retrying` | none |
| `starting` | `commandResult` | `startTriage` ok with a session | `running` | none |
| `starting` | `commandResult` | `startTriage` ok with none, and an issue was asked during it | `starting` | `startTriage` |
| `starting` | `commandResult` | `startTriage` ok with none | `idle` | none |
| `running` | `commandResult` | `observeTriage` ok, and the terminal is gone | `starting` | `startTriage` |
| `running` | `commandResult` | `observeTriage` otherwise | unchanged; 10 min | none |
| `idle`, `starting`, `running`, `retrying` | `deadline` | Not the one armed | unchanged | none |
| `running` | `deadline` | The one armed | unchanged | `observeTriage` |
| `retrying` | `deadline` | The one armed | `starting` | `startTriage` |

## Invariants

- A transition writes only with a compare-and-swap on its machine's version, so a replay or a race applies once.
- At most one command is in flight per issue, besides a class 1 event's, and none for longer than three timeouts while `listen` is connected.
- A waiting command is sent early only when what it names clears, and each early send after its first is spent, so it still ends.
- A command result's transition runs in the state its machine is in when it lands, so it never acts past a stop, undelegation, cancel or halt that came first.
- A fact is replaced only by a newer value from its own clock, and an event carrying nothing newer moves no machine; a GitHub SHA is matched, never ordered. A landing has no clock: the first is kept. At one clock, a `delegateChanged` beats a read's `unknown` and the owner a `created` read was taken for.
- A deadline is named by the version that armed it, and dropped once its state is left or it is armed again; `takeBack` only once none is owed.
- A slot is granted only by the coordinator's ledger, on a host with room, and reaches `claimed` only through a decision on a read landing after the grant, or in the step that asked.
- A waiting place keeps its age and takes the issue's priority as last asked, its own else its parent's.
- A recipe machine is made, woken, slept, updated or destroyed only by a fleet command, one in flight at a time across all repos, idempotent by what `listen` finds.
- A request with nowhere to go has a machine of its repo woken or made, unless one is on its way, and otherwise the fleet records why not.
- No `suspendHost` or `updateHost` goes to a host while a grant names it, a worker's command waits on it or a request is pinned to it, and `updateHost` only to one asleep.
- Every command is safe to run again by what `listen` finds, and every post carries an ID made from its command's. Only a read waits on Orca absent; a write's Orca failure fails.
- `listen` types only fixed text, into an agent it has just listed as not at a prompt, or times into a session's turn open in the conversations checkout.
- The machine hands a warm turn only an owner's message in a conversation, after it answered the request it was given; a hand-over that may not have landed closes it, and its end decides the request again.
- A stop is owed until confirmed, through a merge too; an agent seen working before then, or a start that may have begun one there, is stopped again. A turn's stop says "Stopped." once.
- A request older than a stop or halt never resumes the worker.
- A stall is the work's own mark (`stalled`), never read from `notices`, whose key a request's own `stalled` shares.
- A stall is told only once the request's own line has landed: no `stalled` notice goes out behind its `deliverMessage`.
- A worker's state agrees with what it last saw of its agent: once no start is left to land, none seen exiting since is `running`, `idle` or `held`, and none past its first turn seen idle or waiting is `running`.
- Every state but `done`, `reaped` and a start's `launching`, which `startWorker`'s `lost` ends, sets a deadline.
- An owner's delegation always reaches a request machine, held or begun on the session rule 3 asks.
- Only the owner's sessions, prompts and delegations, and a chain's while its `chain` holds, make events; feedback counts only from the repo's `reviewers`, and a PR is the issue's only from its worktree's branch and repo.
- The owner check, the blocker re-check before a claim and the plain shapes of IDs stay in code.
- A transition reads and writes only its own issue's store; what crosses issues leaves as an event, written with it and delivered after, in order from one store to another.
- A blocker's or sub-issue's state is kept by its own `updatedAt`, and membership by when the read was sent, so a stale read never puts back a finished blocker.
- A class 1 event never waits behind a command.
- The owner's comment on Tolkster's issue is a live worker's or its own thread's, never both.
- A plain comment tells a worker once: `readComment` rules out a mention or thread reply, and `commentTold`, set only by a line that landed, covers the burst; a line that found no agent (`gone`) is told again once an agent works.
- At one GitHub second a PR's later kind of event (open, push, close, merge) wins, a higher-numbered PR over another in its repo, and of two pushes the later arrival; a merged PR stays merged.
- No machine waits on a GitHub redelivery: each state holding an open PR reads it again at its deadline.
- A note's thread's turn is known by its record under the root, never by `listen`'s memory, so a restart neither ends it, loses its stop, nor opens a second.
- A turn is held only while its `startTurn` is queued or its terminal is known, but for one whose start was `lost`, which takes the terminal its idle reports, and a note's thread's with nothing in its batch.
- `listen` decides nothing: the Durable Object alone decides a request, the work, a slot, its host and a machine.
- A failed check at the head held starts at most one round per head, counted against `maxRounds`, and never while the work is `running`.
- No worker waits on CI: a head's checks still pending are told only by the read its `pending` deadline owed, once per head.
- A PR fact is evaluated as it lands, never after a wait for its burst to end, and a burst adds one read; one landing while the worker finishes, after its exit, a request, or an hour from its PR's opening.
- A recipe machine is sent `suspendHost` only once every complete listing for its idle minutes has named it idle, and `destroyHost` likewise dormant for its `destroyAfter`.
- One triage session runs at a time, and `tolkster triage` posts a verdict only on an issue the session the machine holds was given.
- Tolkster merges only by `mergePr`, at the head read; never a PR whose auto-merge was turned off, whose issue has `manual-merge`, or capped at that head.
- Nothing acts on time passing where it means another system will have caught up: deadlines are backstops for a lost event, and a bounded retry is for a state that sends none (GitHub's `UNKNOWN` merge state).
- A round is counted and its markers move only by a `startRound` result that may have reached the worker (ok, held or `lost`), or, for a `typed` one, once a turn is seen begin after its line was sent; the agent's exit first drops it, and the PR is read again at the retry's deadline.
- Work finishes: once nothing new comes in, every owner's request the machine heard of is answered or told, and no open PR of wanted work is left conflicted, behind a base read, with feedback unanswered or a failed check, unless the owner was told why at its head.
- The intake keeps none of an issue's facts: only the store (`mergeEvent`) judges an event newer.
- A step's records are each read once and written in one put, checked at the revision read; its commands reach `listen` only once it has landed.
- A read reaches the machines only as the events it shows, through `mergeEvent`.
