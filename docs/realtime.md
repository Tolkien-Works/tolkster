# Realtime

Every webhook becomes events for the machines of [The state machine](state-machine.md),
which the Durable Object holds and steps; their commands reach the workstation as they are
sent over a WebSocket, and `tolkster listen` carries each out and reports what happened as
events ([How dispatching works](dispatcher.md)). The relay is one Durable Object, named after
the owner. `tolkster setup` sets its secret and the Worker's address ([Set it up](setup.md#3-tolkster-setup)).

## Webhooks

Each delivery is verified, mapped to events (`src/worker/events.ts`) and handed to the relay's
`ingest`, which puts each through the intake ([Events](state-machine.md#events)): it correlates
the event with its issue, drops a redelivery, and writes the event and the machines' step on it
in one storage write; the commands that step queued are sent once the write lands. An event
leaves **no Linear or GitHub text** behind: IDs, enums, numbers and times.

- **Agent sessions.** After `decide`, the Worker ingests what the owner's session or prompt
  started or wrote and acknowledges it ([Tolkster's voice](voice.md)); a refusal ingests nothing.
- **Issues.** An update counts only when it is Tolkster's, judged by the relay from its
  payload against the stored app user ID and the issues with a Tolkster session
  ([Tolkster's state](state.md#other-records)): a delegate change to or from Tolkster, or a
  change to an issue delegated to it, subscribed to by it, or with a session on it or its
  parent; before a first session stores the app user ID, every finish counts. **Linear sends no webhook for issue relations**, so a blocked request reads its issue
  again at its deadline ([Request transitions](state-machine.md#request-transitions)).
  Subscribing to Issues and Comments is a webhook setting, not a `SCOPES` change.
- **Comments.** A Comment `create` by the owner (`OWNER_USER_ID`), not one an integration
  posted as the owner (`botActor`), is `ownerCommented`. The payload has no link to an agent
  session, so only `listen` can tell a mention, a reply or a thread's root on an issue
  Tolkster created from a plain comment, by reading the comment, its parent and the issue's
  creator and newest 50 sessions (`read_comment` in
  `sessions/issues.rs`). A mention's session event names its comment, so a session made after
  the read is known.
- **GitHub.** The App's webhook ([The webhook](github-app.md#the-webhook)) is mapped to the
  PR's events by lower-cased repo and number, with the delivery's `X-GitHub-Delivery`; a push
  to a default branch is `repoPushed`. Never review, comment or PR text.
- **A failure answers 503.** Nothing is acknowledged in Linear or GitHub for an issue, comment
  or GitHub delivery, so the Worker ingests before it answers, and answers 503 should the relay
  fail, for the sender to deliver again. A session's events are ingested after the
  acknowledgement and tried again there, three times.

## The base tip and the frame

A repo's base tip, `{type: "base", repo, ref, after}`, is level state: its default branch as
the relay last heard it pushed (`fromPush` in `src/worker/github.ts`). The relay keeps one per
repo and sends it, never queued or acknowledged, to every connection when a push moves it and
to a connection as it connects. A push older than the held tip is dropped, so it can't move a release `listen` down. `listen` brings the repo's idle conversations checkout to it ([Turns](dispatcher.md)) and
updates itself on its own repo's ([Keeping up with main](#keeping-up-with-main)).

The frame (`src/worker/machine/frame.ts`) is level state too: the slot ledger, the queue in
order, each slot's host, work state, deadline and PR (auto-merge on or off), and its `anomaly` (its issue seen completed or
cancelled), parked work, live chains, triage, each paired host's cap and why the fleet asks for no
machine per repo. The relay sends it to every connection when anything changes it, and on
connecting. Only the panel and `listen.log` read it. The relay's constants that mirror `listen`'s (`LIVE_WITHIN_MS`, `AWAY_AFTER_MS`) are pinned by tests in `listen.rs`.

## The relay and listen

- **On connecting** the relay tells every machine with a command in flight (`listenConnected`),
  which begins their timeouts afresh, and sends each such command, each repo's base tip and the frame.
  Nothing polls Linear or GitHub; what no event reaches is in [Gaps left by
  design](dispatcher.md#gaps-left-by-design).
- **Who's listening.** A connection is live when it has pinged, or was accepted, within 75 s.
  The Worker asks once before acknowledging the owner's session or prompt, with 2 s to answer;
  no answer counts as listening. With nothing listening it acknowledges with the durable "I'm
  off my perch…" (`AWAY`) and marks the request told so (`acknowledgedAway`); the events are
  ingested either way. `--print` counts as listening here, never for the alarm's away message.
- **`tolkster listen`** connects to `<WORKER_URL>/realtime` over `wss`, pings every 30 s, and
  reconnects with a backoff if it hears nothing for 75 s. It sends its settings, the `config` report, again whenever they change.
  One of its own jobs that comes again as a different line while queued or running runs once
  more after it, as the newest line. A job, listing or build with no progress for 10 minutes
  is stuck, so `listen` restarts itself in the same terminal.

### Trying again

A job of `listen`'s own that found GitHub's or Linear's budget spent, or a base tip `origin`
doesn't hold yet (`Acted::Held`, [Budgets](budget.md)), is held and run again after its wait. A
command is never held here: its result says how it ended, and the machine's timeout sends it
again ([Commands](state-machine.md#commands)).

Everything the relay does on a timer is an entry in `Relay.timers` (`alarm.ts`), as a Durable
Object has one alarm. The one entry is the machines' deadlines and timeouts, which also tells
them whether anything listens.

### Saying it's away

When the alarm finds no connection live but a `--print` one, it tells each machine with a
command in flight (`listenGone`): that store then holds `listen` away, and none of its commands
is sent again or ended `lost` until `listen` connects. A request `requested` or `claimed` on
such a store means `listen` died or the workstation slept after the ephemeral acknowledgement.
It arms a `quiet` deadline of 15 minutes (`AWAY_AFTER_MS`), which outlasts a self-update's
drain, restart and reconnect, so a shorter one would be a false alarm, and, unless the store
no longer holds `listen` away, sends `postAway`, which the relay posts itself: the durable
`AWAY` message, once per request by its key in `notices`. A request the Worker already told so
arms none. The relay reads nothing from Linear, so a session archived meanwhile still gets it.

## Commands and reports

The machines talk to `listen` in shapes `test/contract.json` pins for both sides. The relay
sends each command as its store's write lands, to the newest connection, with its args read
from the store as it is then (`toWire` in `src/worker/route.ts`), and maps each report back to
its issue's event (`fromReport`).

- **A command** is IDs, enums, numbers and times, each with a plain shape checked before it
  reaches Orca, a terminal or a post; one whose `args` don't read ends `failed`. What each does
  is in [What is done](dispatcher.md#what-is-done).
- **Its result** is `ok`, `failed`, `retryable` ([worth sending
  again](budget.md#failures-and-waits)), `held` (an agent waiting at
  a prompt, so nothing was typed) or `gone` (no agent in the worktree to type to: no failure), with `detail` as `ResultData` has it
  (`src/worker/machine/types.ts`), never an error's text. It goes back on the connection its
  command's newest copy came on, or not at all: the Durable Object sends the command again to
  the newest. A resend of a command in flight isn't carried out again, and one done `ok`,
  `failed` or `gone` is answered with its last result.
- **The listing**, whole, is `agents` (each worktree an agent runs in, as `working`, `idle` or
  `waiting`), `listed` (every worker's worktree, null when incomplete), `hosts` (those
  paired that answered), `paired` (all of them) and `fleet` (the workers and repos' projects here, and each
  paired host's cap, admission, workers and ready projects), sent on each
  connection, whenever it changes, and at least once a minute. The relay
  applies the newest connection's last to each worktree its index holds: in `agents`, that
  state; missing from a complete `listed`, nothing, as that is `worktreeGone`'s; otherwise
  exited, as the invariants allow, never within 2 minutes of an exit its own hook said. A
  complete `listed` marks each worker record whose worktree it lacks, clears the mark of one it has, and reports each marked more than an hour
  (`MISSING_KEEP_MS`) as `worktreeGone` and forgets it: time alone says nothing. One on a
  host `paired` lacks is reported at once.
- **The fleet.** The relay keeps the `config` report and the newest listing's `fleet`, names
  and numbers, in `machine:fleet`, sets the coordinator's slots to their sum as
  `hosts::total` makes it, and grants each place it now has room for.
- **An agent's own hooks** report its state to `POST /state` ([Workers](dispatcher.md#workers)),
  at the time the hook ran on its host's clock, which orders only that worktree's hooks: one no
  newer than the last (`hookAt:`) is dropped. A hook or a listing says how long before sending
  it was made (`elapsedMs`, monotonic), and the Durable Object dates it by its receipt less that. A listing
  that began no later than a hook says nothing of that worktree. The listing stays the
  backstop for an agent that ends without its hook, and the only one that sees a terminal
  close. Another worker report whose event the store fails to take answers 503, which
  `tolkster` resends.
- **One acting connection.** The relay takes results and observations only from its newest
  socket and closes every older one as `SUPERSEDED` (4000). `listen` then drains and exits;
  any other close reconnects. `--print` is never closed and is sent no command.

## Lanes

`listen` acts on different issues at once (`scheduler.rs`), so a slow job on one issue holds up
no other, the owner's above all. A command runs in its issue's lane, one job at a time in order,
up to 4 lanes at once. A class 1 event's command (the owner's stop, an undelegation, a cancel,
a parent's halt) is `urgent`: it runs in a lane of its issue's own, needing no room among the
4, and a free thread takes it first, so it never waits behind the command it overtakes. The
shared lane runs what isn't one issue's: a base tip, a closed checkout or terminal and the
self-update. A job waiting for its lane is in flight, so the same line again isn't queued.

What keeps an issue's commands apart is the machine's ([Stores](state-machine.md#stores)). Two
locks are left, each taken last: the checkouts' (`Checkouts::lock`) and git's (`git::writing`),
which a worker's setup, in a process of its own, doesn't share.

## Running tolkster listen

Run `tolkster listen` in a terminal from `<CHECKOUT>`, Tolkster's primary checkout, so it can
reach Orca and Claude and keep itself current. Nothing is lost while it's
stopped: the relay keeps each command in flight and each repo's base tip. The terminal shows the
log, and `~/.tolkster/listen.log` keeps each line, UTC-dated, across restarts: past 5 MB it
becomes `listen.log.1`, replacing the older. Only the owner reads it: it has `--verbose`'s
errors' text, which can quote Linear or GitHub, and opaque IDs, and a recipe line's last stderr
lines, less a pairing code. A git write's failure just says `failed`, as git's errors
can carry a credential. Each line leads with the
local time and the job's issue, its area or `·`: a job is one line, its kind, outcome and time,
and a cost shows only for a low budget or a slow call. A queued request's wait, from a
command's `queued` arg, is a line of its own, `queued: blocked by TW-683`, said
again only when it changes. `✖` marks what the owner must act on or a command that didn't
happen, `!` what was likely done but is worth knowing, and `✔` a recovery. `--verbose` adds UTC
times, opaque IDs, errors' text, frames, costs and writes.
`--print` prints the commands it is sent, carries out nothing and never updates itself; it
can run beside the `listen` that acts.

In a terminal the log sits under a full-screen panel of what `listen` holds now (slots, agents,
the frame's queue, parked work, chains and triage, or until it comes, queued requests as a
command last said them, lanes, budgets, hosts, connection, update), by IDs,
enums, hosts and times alone, beside the log's own lines. A slot's agent is the frame's work state when it has one, working (background) while its hook says so: the agent's hooks reach it first. A PR names CI state in words, at rest with auto-merge off waits on the owner, and blocked work through it; a landing, with none, shows as merged. `l` shows the log alone, arrows or
`j`/`k` pick a slot, `Enter` shows its issue's lines, and `q` takes no new command, answers what
it holds while connected and exits. The panel only reads: a panic in it, a failing terminal or a restart restores
the terminal and replays its log. `--plain`, `NO_COLOR`, a piped stream and `--print` keep
the plain log. The `listen` started by hand only starts the one that acts, again after each
restart, and waits on it, so the shell never takes the terminal back.

### Keeping up with main

A push to `main` reaches `listen` as its repo's base tip, and so does each connection. A tip
whose commit, `after`, isn't there to take yet is held and tried again a minute later, as
nothing resends it, and a newer tip replaces it; one still not there after 30 tries, as after a
rewrite of main, is left for the next push.

Without the `develop` feature `listen` swaps in the Worker's build of `after`
([Builds](hosts.md)), the minutes CI takes to build it being the hold, then drains and restarts
as below. A build made by hand carries no commit, so has no update.

A `develop` build, when the repo is Tolkster's own, fetches at once and merges nothing until
`origin/main` holds `after`. Then it fast-forwards the checkout, builds the `local` profile
(release with line tables, unstripped, so a dump of the running `listen` reads by file and line:
on Windows from the `tolkster.pdb` the swap keeps beside the exe) and, when the build differs
from the installed `tolkster`, puts it in place and restarts in the same terminal. It checks on start, once connected.

Once a build of HEAD is in place, before any drain, a `develop` build syncs a Linear release
of its commit to `releasePipeline`'s pipeline, which is this `listen`'s alone: a merge is live
only from this rebuild, as briefs are compiled in. It links the newest 100 of the workspace's
teams' issues in the last tags of the first-parent subjects since the pipeline's newest
release, the keys read from git, never Linear text, or the commit's own when that release
isn't an ancestor. A failed build or a commit released already syncs nothing; a failed sync is
logged, and the next build or start makes it. Other builds have no checkout, so sync none.

What decides a `develop` build's swap is the build, not git: a pass builds HEAD unless this run's last build that
ended was of HEAD already, and swaps only when the build's bytes differ from the installed
binary's. The stamped commit follows the binary's inputs (`cli/`'s sources, briefs and manifest,
`src/shared/voice.json`): a merge that changes none builds, swaps and restarts nothing.

A `develop` build and its swap run on a thread of their own, so commands go on being carried out. A
push that lands during a build isn't pulled, as cargo is reading the working tree; it is pulled
once the build ends. Once the new build is in place `listen` **drains** before it restarts: it
takes no new command (the Durable Object sends it again to the new run), finishes what it had
already taken and the watcher's jobs, so an exit seen during the drain isn't lost, and drops a
held job (the next base tip stands in). Workers and turns run in their own terminals and aren't
waited for.

A `develop` build's guards (`update/develop.rs`), each printing `update: skipped: …`: the primary checkout of tolkster
with a GitHub origin, read once as `listen` starts; on `main` with no uncommitted or untracked
files, checked on every pass, since the build takes the working tree; fast-forward only. A
failed build keeps the current binary. Windows won't overwrite a running exe but will rename
one, so the running `tolkster.exe` is renamed aside and the new one started in the same
console. It needs your git credentials and `cargo` on PATH.

## Invariants

- Only one self-update step touches the checkout at a time: a pull runs only when no build is
  running, a build starts only after its pull, and none starts once a new build is in place. A
  pass checks the tree is clean and builds HEAD unless this run's last build was of it.
- While draining, `listen` takes no new command. A build still progressing holds the watchdog's
  restart at most 3 minutes more, within 13 minutes of the last progress.
- A connection's turn (`Scheduler::step`) sends each result before it takes a restart.
- The panel never decides what is carried out or when: `q`, a drain, is all it changes.
- The terminal is restored before `listen` exits.
- The relay has one alarm, set to the earliest `Relay.timers` entry due; a new timer is an
  entry there, never a second alarm.
- `AWAY_AFTER_MS` is longer than a self-update's drain, restart and reconnect.
- A release is synced only for a build of HEAD in place, before any restart is set, linking
  issue keys read from git, never from Linear text.
- Relay routes and Linear webhooks refuse while `OWNER_USER_ID` is unset. A relay route's 503 names the unset setting (`{ "missing": [name] }`), which `listen` prints.
- A worker waiting on a call in the background runs at most 30 minutes past its last hook:
  `agentBackground` counts calls that recent, and holds off a listing's `idle` that long.
- A listing says exited only of an agent the machine saw or whose hook ran, never one
  before a launch in flight or watched.
- A hook is set against a listing only by the Durable Object's times, never a host's wall clock;
  two hooks of one worktree, only by their host's `at`.
- `listen` answers in the thread of its own comment, or the owner's on its issue, only the
  owner's non-bot comments, re-read before its turn opens, in a thread no
  session is rooted at. One turn runs in a thread at a
  time, and each request gets one answer, by a comment ID made from the root and the request's
  time (`post::answer_key`).
- A lane runs one job at a time, in order, except the owner's urgent command, which goes ahead
  of `listen`'s own jobs in a lane of its own. A fetch, a branch's push or deletion,
  the self-update's pull, a checkout's detach, a hook's config and a worktree's creation or
  removal run one at a time across lanes (`git::writing`). A spent budget holds every lane.
- An Issue webhook is judged from its own payload, never from what an earlier delivery said.
  Once the app user ID is stored, every delegation and un-delegation counts. A webhook the relay fails is answered 503, never 200. An event is written with the machines'
  step on it in one storage write, and its commands are sent only after that write lands. Any
  session event but a mention's `sessionStarted` is ingested only after its acknowledgement is
  posted.
- A command's result goes back only on the connection its newest copy came on, a command ID is
  carried out at most once at a time, and a resend of one done is answered with its result. The
  relay counts results and observations only from its newest accepted connection and closes older
  ones.
- Only the Durable Object says a worktree is gone, and only on a complete listing received more
  than an hour, by its own clock, after the first complete one without it, or on a listing
  whose `paired` lacks its host; an incomplete one counts for nothing, and the record is forgotten after `worktreeGone` is tried, only while its
  mark is the one found.
- The Durable Object ends only the turn that holds a closed terminal as its session's newest
  (`turn.terminal`), so a late close never ends a newer turn.
- A command's args are read from its store when it is sent.
- The coordinator's slots are the fleet's sum, from the newest connection's reports alone.
- An agent's hook reports apply in the order they happened: its state hooks run in turn, and
  the relay drops one no newer than the last it took for the worktree. A hook's state is never
  the last word: a listing that began after it is applied over it. A worker's exit is never
  reported from its wait, only from the listing it starts.
- `listen`'s lines are worded only in `trace`, from typed events: a plain line drops an opaque
  ID and shows an error by its kind; `--verbose` and the disk log have both.
- Neither side waits on a timer for another system to catch up: what stays timed is a retry
  with backoff, a rate-limit wait, a heartbeat and a deadline for a lost event.
- A completed issue's context is its store's `startContext`, held by no machine; `listen`
  holds the context's claim, so a redelivered completion starts no second session.
