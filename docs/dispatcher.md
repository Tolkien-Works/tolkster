# How dispatching works

The machines of [The state machine](state-machine.md), in the Durable Object, decide
what each of the owner's requests gets; `tolkster listen`, on the workstation
([Running tolkster listen](realtime.md#running-tolkster-listen)), carries out the
commands they send, in code, and reports what happened and what Orca shows. This file
says what holds those commands together; the commands and reports themselves are in
[Commands and reports](realtime.md#commands-and-reports).

## Events and the watcher

- **Webhooks** become events on their issue ([Webhooks](realtime.md#webhooks)) and the
  machines step on them at once ([The decision](state-machine.md#the-decision)). The
  owner's plain comment is passed to a running worker as one fixed line. Their reply to a
  comment Tolkster posted outside a session, or with no worker running their comment on an
  issue Tolkster created, is a request a turn takes in that thread, opening no session ([A reply in Tolkster's thread](#a-reply-in-tolksters-thread)).
- **The watcher** exists because no webhook says a worker's terminal closed. `listen`
  lists Orca's terminals (`Watcher` in `watch.rs`) when a terminal it waits on exits or a job
  changes its worktrees or terminals, once when a frame shows a worker running, idle or held
  that the newest listing lacks (its agent's first hook follows the listing its launch asked
  for; a slot unlisted after that stays `unseen`), and every 60 s as the backstop. It reports
  each worktree's agent as `agentWorking`, `agentIdle`, `agentWaiting` or `agentExited` when
  it changes, reading states and times only, never a terminal's contents. The machine's
  deadlines do the rest ([Work transitions](state-machine.md#work-transitions)). A failed
  listing is skipped; a failed read of the states alone keeps the last one, so exits are
  still seen. A wait's `exited` is no proof (Orca gave it for a terminal it still listed): a
  terminal `listen` opened ends only when a fresh listing lacks it, else its wait restarts a
  few times and then the regular listings decide.
- **The same listing** ends turns and triage: a turn's terminal closing is `turnEnded`,
  the triage session's closing starts the next batch ([Triage](triage.md)), and the last
  terminal closing in a conversations checkout refreshes it. The Durable Object also
  marks a worker record whose worktree a complete listing lacks and, an hour later on
  another, reports `worktreeGone` and forgets it ([Tolkster's
  state](state.md#other-records)); an incomplete listing counts for nothing, nor does
  one for a silent host. A record on a host the listing's `paired` leaves out is gone at
  once: unpairing is how the owner says a host is gone for good (after `host rm`), and
  delegating the issue again starts afresh from its branch on `origin`.
- **The first listing after start** has nothing to compare with, so what happened while
  `listen` was down is read from the Durable Object (`missed`): a worker record with no
  agent running is reported `agentExited`, and the machine's state says whether that is
  news.

## Gaps left by design

Each of these waits for the next event that reaches it, or for the owner:

- **A delivery lost before the Worker.** Nothing asks GitHub to redeliver and Linear
  retries only briefly, so what it would have started waits for the next event on that
  session, issue or PR, or a deadline that reads it again (a `running` worker's issue is
  read hourly). A delegation Tolkster never acted on is asked about, for the owner's reply, at `listen`'s next connect.
- **A session event the Durable Object fails on every try** isn't retried; the session waits
  for its next event. A session left with no reply the owner can see is logged
  `session_unanswered`, with its ID and why.
- **A review landing during a round** resumes nothing until the PR's next event
  ([Review feedback](review-feedback.md)).
- **A removed blocker, or a finished one that isn't Tolkster's,** is seen at a blocked
  request's next read, at most five minutes.
- **A command that never comes back** ends `lost` after three sends
  ([Commands](state-machine.md#commands)); a `startContext` that does isn't asked again.
- **A worktree reap kept** waits for the work machine's next event or `tolkster reap`.
- **A failed conversations-checkout refresh** waits for the next turn, its close or a push.
- **A Triage issue no batch reaches** (none running, one delegated later, one past the
  queue's page, or any after 20 failed starts in a row) waits for the next create in
  Triage, the next session's end or `listen`'s next connect.
- **Archived or deleted issues:** what Linear sends is unverified, so none is an event.

## Who waits

- **Waiting on Tolkster** is the request machine's: a session's newest request is the
  owner's, read from Linear as facts ([Facts](state-machine.md#facts)); only the owner's
  prompts count. It is new, a reply or a stop.
- **An answer** is a response, elicitation or error posted by `tolkster activity`
  (`workerAnswered`), and answers only up to the newest request that reader's
  `tolkster messages` last showed it (`readThrough`, `turnReadThrough`), so a reply typed
  into a busy worker and not yet read still waits. Thoughts, actions, the Worker's
  acknowledgements, notices and the stalled message answer nothing.
- **One worker per issue:** a request on an issue with a worker, running or exited, goes
  to it, and `startWorker` starts none beside a worktree Orca links to the issue.
- **The cap.** `slots` caps work workers by host name, `local` (default 6) here; a
  paired host without one, or 0, takes none, and a recipe machine takes its repo's `slotsEach`. The coordinator's slots sum each host's cap, or its workers if more,
  a silent host's or one with no repo set up its workers alone;
  only new work waits,
  by priority (a sub-issue's own, else its parent's), oldest first,
  with no ageing and nothing running stopped. A work machine in an agent state holds a
  slot ([Stores](state-machine.md#stores)); your own worktrees and
  conversations take none.
- **Hosts.** An Orca server paired here (`orca host list`, read each time) whose
  credential `host add` admitted is one. The coordinator grants each slot with its host: here
  while it has room, else the first host, in paired order, that answered the last listing, is
  admitted, isn't being slept or destroyed, has room (its cap less its grants, or its listed
  workers if more) and has the repo `ready` by its project ID, its `tolkster config` exiting 0
  (the panel names a host that refuses); else the request waits. `startWorker` names the
  host; `listen` turns it back, `retryable` before it moves or says anything, when it doesn't
  answer, isn't admitted as read then or hasn't the repo ready, never picking another, and the
  coordinator moves a grant off a host no longer open or ready, not a silent one, before its
  worker is there (`slotMoved`). A
  worker never moves host: a request whose issue has a worktree waits for that host's room, or
  the fleet's while it isn't open; a parent's new orchestrator, its slot held, starts here;
  turns, triage and context stay here.
- **Machines.** A request with nowhere to go has the Durable Object's fleet wake an asleep
  machine of its repo, else make one from its recipe ([Hosts](hosts.md)) under its `machines`,
  marks included, while none is being made or woken and none failed in the last 10 minutes;
  one whose worktree is on an asleep machine wakes that one
  ([Fleet](state-machine.md#fleet)). It looks again after every event, so a request seen while
  here had room gets its machine once here fills. Once enrolled, the machine is a host with
  room for its repo alone. When it asks for none, the panel and `listen.log` say why.
- **A silent host's workers' issues** carry `host offline`, a label of the workspace group
  `tolkster` (one state per issue; the owner's flags stay outside it), added when the host
  stops answering and removed when a listing hears it, a restart's included, or on
  unpairing, before the listing that releases its workers. An asleep machine is silent, unlabelled.

## What is done

In `command.rs`, `command/` and `executor.rs`, each command for the issue it names. Commands reach
Linear, Orca, the records and GitHub only through the ports of `ports.rs`, which answer
typed facts; tests run them on the fakes of `ports/fake.rs`.

- **`stopWorker`, `stopTurn`:** a turn's stop takes the checkouts' lock, so a turn opening
  meanwhile opened first or is stopped again on its start's result. A stopped worker stays
  stopped unless the owner asks, so review feedback never resumes it. An agent with no
  plain handle can't be closed by name, so its stop fails.
- **`deliverMessage`** types one fixed line pointing at `tolkster messages`. An agent
  waiting in its worktree is never typed to, as the line's Enter could answer it: the
  command ends `held` and the machine sends it again. A message lands mid-turn, which it
  can't start, so it counts as delivered once the send's receipt shows Orca took the input
  (`input_accepted`), with no further wait. A round's line counts only once it begins a
  turn (`turn_started`), which its send waits for. Any other receipt ends `held` or
  `failed`, and the line, which may sit unsent in the agent's input, is never typed again.
  A send Orca refuses before writing is tried again for half a minute while no agent waits.
  A round's send refused with proof nothing was typed (no agent, a handle gone), with no
  agent listed afterwards, relaunches the worker as an exited one.
- **`startWorker`** treats a worktree Orca links to the issue as the worker's, unless it is
  the one the command names, with no agent and recorded for the issue: a start that never
  began. The repo comes from `repo_map::repo_for`, the
  one resolver work, conversations, triage and context share
  ([setup](setup.md#6-orca-repos-and-tolkster-listen)). It creates the worktree,
  records it, and opens a terminal there on `tolkster worker` (`setup.rs`), one plain line,
  never Linear's or GitHub's text. On the worktree's host, that step
  moves a fresh worktree onto the issue's newest branch on `origin` with work, pushes it, writes the briefs ([The brief](#the-brief)) and settings
  under `~/.claude/<repo>-briefs`, trusts the worktree for Claude, then starts `claude`; resumes and
  rounds relaunch through it. Its failures print there, and one that stops it
  starts no `claude`. If the record fails or the terminal doesn't open, a worktree it made
  is removed. The branch is Orca's, prefix included, never renamed. The result's worktree and
  branch are indexed for the issue's events, bar a superseded start's branch.
- **`startTurn`** opens the turn's terminal in the repo's conversations checkout. A
  resumed turn is sent the brief's path, not its text, so a brief changed mid-conversation
  reaches only a fresh session.
- **A failed launch** asks the session once, as an elicitation offering "Try again", so
  it awaits the owner; its result is `said`, so the machine adds no `stalled` or
  `couldntPass` notice. `listen` never relaunches: a terminal Orca timed out on may have
  opened, and the owner's reply decides. A spent budget is waited out instead.
- **Reads and posts** report IDs, enums and times only. A post's activity ID comes from
  the command's, so a resend posts nothing twice, and its text is fixed ([voice](voice.md)).

## Model and effort

A new work worker runs on a rung of `ladder` in config.json ([Setup](setup.md)): rungs
from weakest, each a name, a harness (Claude when left out), a model and an
effort as it names them, and `upTo`, the largest size it takes. By default:

| Rung | Model | Effort | Size |
|---|---|---|---|
| 1 | Haiku 5.5 | high | up to XS |
| 2 | Sonnet 5.5 | high | S |
| 3, the default | Opus 5.5 | medium | M |
| 4 | Opus 5.5 | high | above |

- **A size is a place on the issue's team's scale** (`estimates.rs`), not points, so one
  ladder means the same on every team. The size picks the first rung whose `upTo` takes
  it; no estimate, or a team without estimates, takes the default. Priority says how soon,
  not how hard, so it plays no part.
- **Labels** `agent:<harness>`, `model:<m>` and `effort:<e>` keep to the rungs that match,
  and of those take the nearest the size's rung; an `agent:` label no rung matches is
  ignored.
- **Chosen once.** The record keeps the rung with `rungFrom` (`claim` or `noEstimate`),
  and resumes and rounds launch with it. The one exception: when the record says
  `noEstimate` and the issue now has an estimate, the next launch takes that rung
  (`executor::relaunch`), recording it first, as a running session can't change model,
  unless the rung is on another harness.

## The brief

A work brief is `cli/briefs/work.md` plus the partials its start needs; a turn gets
`conversation.md`. The templates are compiled in ([Keeping up with
main](realtime.md#keeping-up-with-main)), so each host's `tolkster` renders them, and a
worker starts on a prompt naming its file. Only values from code and configuration are filled in (`brief.rs`), never
Linear's or commits' text. A parent's sizing is for its own work alone, never its
sub-issues' estimates.

Comment and prompt text reaches a worker through `tolkster start` or `messages`, never typed
into a terminal or a prompt; a conversation's turn also gets the owner's messages
on stdin (Rule 3). A command's pickup, like notices and the stalled message, answers
nothing, so a worker's first `response` is its final summary unless it answers a message.

## Workers

Workers report as Tolkster with `tolkster activity`; a response ends a round
([Voice](voice.md)). Workers and turns start with no MCP servers (`harness::NO_MCP`): one
would act as the owner, and connecting costs seconds. A worker's **hook**, set on its
command line alone and told its worker by its launch (`card::launched`), runs four commands:

- **`tolkster hook tool`**, before each tool call in the background, posts an ephemeral
  action: the plan's step, else a phase, then a fixed phrase under `tools` (`hook.rs`),
  naming a file only when plain and inside the worktree. It posts only in the session the
  worker's newest `tolkster activity` or `tolkster plan` names, never after a response,
  elicitation or error, nor what the call before it posted. A paired call it ticks
  instead, every 3 minutes.
- **`tolkster hook start`**, which the call waits on, posts the plan as moved on and
  puts the trail's lines in (`hook/trail.rs`): a durable action per plan step completed,
  else per phase passed, every line once by ID. No `until` is a gate, which differs by
  repo. Then a paired call's start, ephemeral: a test, build, push, rebase or sub-agent,
  never a commit.
- **`tolkster hook after`** posts an ephemeral `Committed` per commit, the oldest HEAD
  reflog entry since the call's second, and a paired call's outcome; in a traced session,
  the call's action in place of both ([Tracing](voice.md#tracing)).
- **`tolkster hook state`**, in turn and never in the background, reports the agent's
  state (`UserPromptSubmit` working, `Stop` idle, or background while a call there has no
  notice, `Notification` waiting, `SessionEnd`
  exited, but not at `/clear` or `/resume`) to the Durable Object, so the work machine
  hears it before Orca's next listing. Input it can't be sure of reports nothing.

Whatever fails, a hook exits 0, and a spent budget isn't tried again.

A work worker reviews before its PR ([Pre-push review](pre-push-review.md)) and
turns on auto-merge instead of merging ([Auto-merge](auto-merge.md)) and never waits on
CI; a reply resumes it, and a conversation's turn answers and ends.

## Reaping

`tolkster reap` removes a worker's Orca worktree and forgets its record, then deletes the
branch once git drops the worktree, unless Orca did (a merged PR's). The work machine's
`reapWorktree` runs it after a PR's merge or a landing, a day after a cancellation or a
completion with no PR. It looks only at worktrees linked to Tolkster's issues.

The command carries the rule the machine decided (`merged`, `landed`, `cancelled` or `completed`)
and runs only that rule's guards: it reads no PR and closes no agent. Linear is read again just before removal, as a queued reap may outlast a reopen
and a new cancel, and a cancelled issue's cancel must be a day old, give or take an hour
of clock skew.

- **Merged:** the checkout is clean and at the head the rule names. Ignored files such as
  `.env` count as unclean, since `--force` deletes them; ignored directories don't.
- **Landed:** origin fetched first, a clean checkout whose HEAD is on origin's default branch,
  with every commit on a remote. Only the machine decides it; another host's step also needs
  the issue completed and no PR open.
- **Cancelled** (or Duplicate): no agent terminal and no open PR. Uncommitted work goes
  with it, except where the store holds no PR (`noPr`) and the branch is on origin with
  commits its default lacks, as a missed PR would be.
- **Completed with no PR:** no agent terminal, a clean checkout and every commit on a remote.
- **Agents:** a worktree with an agent open is kept; `closeAgent` first closes an idle or
  held one in a merged worktree.
- **Another host's worktree** is reaped there, as git here can't read it: `tolkster
  reap-here`, run in that host's primary checkout, runs the rule's guards on its
  own checkout and deletes the branch in its own primary. Its exit code says kept; only
  the worktree gone from its host's complete listing says removed; else the reap is
  retried. A recipe machine's primary is the image's checkout; its destroy reaps each
  worktree there so ([Destroying](hosts.md)).

## Sub-issues and blockers

A delegated issue with unfinished sub-issues is worked through them side by side
([Parent issues](parent-issues.md)). An issue is **blocked** while any issue that blocks it
is unfinished: a new worker for it waits `queued` on blockers, takes no slot, and starts within
seconds of its last blocker finishing when that is Tolkster's (the coordinator's blocker
index), else within five minutes, as Linear sends no webhook for a relation's removal.
A mention on it is a conversation; blockers don't hold back messages or stops for an
existing worker. `readIssue` reads at most five pages of 50 relations, so an issue with
more counts as blocked (`moreBlockers`) and fails no other read.
## Work or conversation

**Delegation and blockers gate code work only.** Code work (a branch, commits, a PR, the
issue moved to started) needs the issue delegated to Tolkster and no unfinished blockers;
everything else the owner asks is a conversation, which never waits for a slot or a
blocker, takes no slot and doesn't move the issue. The check is the machine's decision
([The decision](state-machine.md#the-decision)).

- **Where a message goes.** A mention on an issue with a worktree goes to its worker;
  otherwise it is a turn. A reply in a conversation that has had a turn resumes it, unless
  the issue has since been delegated: then it is work, which waits for its own session's
  running turn to end.
- **A turn** is one headless run ([harness](harness.md#turns)) in an Orca terminal of its own. Any
  turn stays **warm**: once it has answered and gone idle, the owner's next words are
  handed to it, times typed into its terminal that `converse` reads them after, until 5
  idle minutes close it ([Requests](state-machine.md#requests)), or 10 by `converse`
  itself as a backstop. Turns run in the repo's one **conversations
  checkout**: a worktree detached at the default branch and linked to no issue, so no rule
  takes it for a worker and `reap` never looks at it. It is refreshed as its last terminal
  closes and on a push to the default branch, never with a terminal open in it, under the
  checkouts' lock. A turn's start resets it, fetching only when HEAD isn't at the relay's
  last tip.

Three rules hold the turns together. A change that breaks one changes the rule here first.

- **Rule 1: one gate on `tolkster`, and guidance for the rest.** A turn runs with
  `TOLKSTER_CONVERSATION` set to its session, and `tolkster` then runs only what one table
  names (`allowlist.rs`, each option one the command takes), checked in `main.rs` before
  any command, in the turn's own session, a body only from stdin. Anything else exits 2.
  The gate guards Tolkster's interface and doesn't sandbox the turn: Claude runs in auto
  mode with no tool list and no MCP servers, so "no code work" and "Linear only through
  `tolkster`" are the brief's guidance, by design. The checkout's refresh bounds a slip.
- **Rule 2: one turn per conversation at a time.** The request machine sends one
  `startTurn` at a time, and decides the next once the turn has ended, or has answered and
  gone idle (`turnIdle`), when it is handed to the warm turn; `listen` opens a turn, and a
  stop closes it, under the checkouts' lock. A request that came meanwhile waits for that.
- **Rule 3: one boundary.** A turn's messages are the owner's after the newest a turn or
  worker was shown (`after`), and it has answered once its
  `tolkster activity` reports `workerAnswered`. A turn that ends without answering is
  asked "I stopped before finishing" by the machine (`stalled`).

**The prompt holds the request.** The owner's messages reach a turn in its prompt, not as
tool output, so Claude Code's auto-mode classifier can count them as the user's intent,
an approval to retry a denied write included. Only the owner's own messages go there;
everything else reaches the turn through `tolkster issue show`. `converse` passes them on
without reading what they ask. A denied write is reported to the owner, never worked
around. Reading them marks them read, so the turn's answer answers up to the newest
request it read.

### A reply in Tolkster's thread

A turn takes in the owner's reply in the thread of a comment Tolkster posted outside a
session, or, with no live worker, their top-level comment, no mention, on an issue
Tolkster created (by creator ID), rooting its own thread, and their replies under it.
It runs in the same checkout, keyed on the thread: one plain reply or none, and no
session, thought, action or plan. A thread a session began on is that session's.

- **Its record is a session's:** the request machine, keyed by the root comment, holds the
  request and the newest reply shown (`turnReadThrough`), and the conversation is
  recorded under the root, whose ID is the agent session's.
- **Opening** (`converse::thread_turn`). Under the checkouts' lock, a turn open by the
  record is handed the request and answered with; otherwise the batch is the owner's
  replies after the newest shown, up to the request (`converse::batch`), so one written
  while a turn ran is the next turn's and none means already answered.
- **The turn** runs with `TOLKSTER_THREAD=<root>@<request>`, and `tolkster` then runs only
  `messages --thread`, `issue` (not `chain`), `comment --reply-to` and `help`
  (`allowlist::gate_thread`): no session's commands and no delegation. It changes Linear
  in its own thread ([The owner and sessions](cli.md#the-owner-and-sessions)), never code.
- **One answer.** The reply, the machine's `stalled` for that request and a `noRepo` share
  one comment ID made from the root and the request's time (`post::answer_key`), which
  Linear refuses a second time, so whichever comes first is the only one.
- **No reply needed** is the turn's judgement, never code's: `comment --reply-to <root>
  --none` posts nothing and reports the request answered (`workerAnswered`): no stall.

## Invariants

- A worktree on a paired host not heard is unknown, never gone: no listing marks it or exits
  its agent, and a command that misses it while a record names it there is `retryable`,
  never a second worker. Only a host `orca host list` was read without is unpaired.
- A line typed into an agent, a close of terminals a listing named and a removal decide on
  a listing of their worktree's host alone, read after the command's last Orca write before
  that decision; a listing of every host asks them at once.
- A worker's briefs (`<ID>.md` and those beside it) are all written before its worker
  starts, and a failed write removes the worktree.
- A hook's outcome comes from a result's error flag or a whole task notice's status,
  passed or failed only when the paired program runs alone, setup aside; a durable one passes the owner
  and issue check.
- `listen` pushes a new worker's branch only where the worker would push it at once: never
  before the owner's go (plan first, a reconcile) nor for a parent working through its
  sub-issues. The work brief is rendered after the branch's setup, from what it did
  (`brief::Startup`).
- A change to an issue passes `--session <id>`, refused unless the owner started that
  session, or in a reply's turn `--thread` for its own thread, refused unless the owner
  replied there ([The CLI's interface](cli.md)). A thread never delegates, nor moves or
  relates an issue in Tolkster's work or blocking it.
- Nothing on the workstation decides whether to act: `listen` carries out the machines'
  commands, each checked, in code, to act only on its own issue's session, worktree,
  comment and PR (`command.rs`, `command/`, `executor.rs`, `reap.rs`, `converse.rs`) on the facts a
  port answers, never in a port, so a fake or new adapter can't skip one.
- Only an event acts: `listen` runs nothing on a timer of its own over Linear or GitHub,
  and the machines' deadlines are the Durable Object's.
- `startWorker` starts no worker beside a worktree Orca links to the issue, and says
  whether a failed start may have left one.
- `listen` makes or wakes a recipe machine only for work no host could take as it looked,
  one at a time and never while a sleep runs, on settings read after the last make, wake or
  sleep ended.
- A conversations checkout is refreshed only while no terminal is open in it, a warm
  turn's included, under the lock that opens turns, and a turn's start closes no terminal
  in a recorded one but its session's turn that couldn't take the request.
- A turn starts on a detached, cleaned checkout: only the fetch is skipped, at the relay's
  last tip. A warm turn answers on the tree it began on, told when the tip has moved since.
- Workers and turns launch with no MCP servers, `--mcp-config` always followed by a flag.
- A turn's prompt holds only its brief (or, resumed, its path) and the owner's own
  messages, read by `tolkster converse` just before Claude starts or takes the next
  request; `listen` types only times.
- A turn's stop leaves the issue's worker alone, and a worker's the turns.
- A thread's turn answers under the newest request `converse` showed it, once idle
  (`state::thread_request`, `converse::Takes`).
- The Worker hands a refused session's issue back only when its comment has no author and
  no `sourceCommentId` (a delegation), and only after reading the issue's history again
  shows the session's creator made its current delegation to the app (`decide.ts`,
  `clearDelegateMadeBy`), so no mention by a workspace member clears the owner's. With no
  history entry changing the delegate, the issue's creator made it.
- A session event's trust, kind and comment come from `decide`'s request, the one place
  they are worked out; `fromSession` only checks shapes.
- The CLI counts a session as a mention by the Worker's rule (`mentioning_comment` in
  `cli/src/sessions/began.rs`), and one begun with its issue, by its creator, while the issue
  wasn't delegated to Tolkster, as a mention in the description (`in_description`). None
  of the description's text passes as the owner's.
- A session with no creator or source comment, rooted at Tolkster's own comment, is
  trusted as the owner's (`on_own_note`, `decide.ts`), and its messages are only the
  owner's replies from before it began.
- A worker's launch comes only from `ladder` (a label chooses among its rungs, its model
  a plain name and its effort its harness's, checked at load and before any command is
  built), is chosen once, by the `startWorker` that starts it, and is stored in both its
  records before its terminal opens; the rung is picked again only at a launch, while its record says
  `noEstimate`.
- `review.deepAt` and the ladder's `upTo` are sizes on the
  issue's team's scale (`Issue::size`), never raw points.
- `deliverMessage` types only a fixed line (`voice::new_message`, `voice::new_comment`)
  naming a plain `Id` or `IssueKey`, never twice within one command; it and `startRound`
  read the agents' states afresh just before they type and end `held` where one waits at a
  prompt.
- A worker's or a turn's own answer answers no request past the newest `tolkster
  messages` has shown that reader.
- An agent's first prompt is its `claude` argument, built on its host, never typed into its
  terminal: no wait on a terminal proves the agent can take a line (`Orca::start_agent`).
- `tolkster reap-here` takes only a worktree from its line: its rule holds only as the
  branch's PRs and Linear say on its host, a cancel a day old (`reap::reap_here`).
- `tolkster worker` acts only in its issue's worker's recorded worktree on its host
  (`setup::set_up`), and gives its agent that worktree and issue. A stop, and a relaunch
  before it launches, close every terminal there, so a setup Orca doesn't list as an agent
  yet never starts `claude` after the one or beside the other.
- A terminal's line or command, an Orca call's worktree ID, git's commits and branches
  (`git.rs`) and the IDs a claim's name is built from take a `plain.rs` type, never a
  `&str`: only `plain.rs` builds one, from a value of its shape, so an unchecked one doesn't
  compile.
- `listen` asks Orca only through the `Orca` port, which routes a call to its worktree's or
  handle's host, never an unpaired one, and lists every paired host. One that doesn't answer
  is silent until `listen`'s background ask hears it (after each listing, every 10 s while it
  wakes), which lists at once: nothing else asks it, and a call aimed at it is `retryable`,
  waiting on it; this machine failing, or a host after
  it answered, fails the call; the shell beside it runs git alone; the watcher's wait for a
  terminal's exit (`orca::wait_exit`) is the one call outside it. A handle sent to Orca is a `plain::Terminal`, never empty, as
  `--terminal ""` names Orca's active terminal, and an agent with no plain handle keeps its
  worktree and is never typed to or closed.
- The `tolkster` group's labels are written, never read: no decision uses them.
- A failed fetch or push says only that it failed, never git's error, which can carry a
  credential; a launch that failed in Orca is said as Orca's by the error's type, never
  with its text.
- A worker's hook posts only fixed phrases built in code from the tool's name and, for a
  file, a plain path inside its worktree, never a command's text, a tool's output or
  Linear's or GitHub's text, plus the worker's own plan steps, durable and moved only
  from a plan whose call returned unfailed, and its commits' subjects, read from git; none durable
  after its response or question. Auto mode doesn't judge hooks, so this is its only
  gate. Its settings file and command line carry no ID or text.
- The hook ends a plan step only on its `until`, a commit in HEAD's reflog or `pr create`
  in its session, and finishes the plan only at a response once the PR is open, or with no
  `pr` step.
