# Tolkster's state

What Tolkster itself did lives in the owner's Durable Object, beside the relay's queue: the
machines' stores ([The state machine](state-machine.md#stores)), the intake's records
([Events](state-machine.md#events)), and the records here (`src/worker/state.ts`,
`cli/src/state.rs`); a worker session's timing alone `listen` keeps on its host
([Timing](timing.md)). What people do (delegation, blockers, archiving, prompts) is read from
Linear, so the record can't drift from it.

## The session record

IDs, timestamps, enums and issue identifiers only, never issue or comment text and nothing
about people. What a session asked is its request machine's
([Facts](state-machine.md#facts)); the record keeps only what the CLI reads back itself: the
newest read of each PR's review feedback ([Review feedback](review-feedback.md)), and
the agent session and Orca terminal a conversation's turns resume.

**Where a session's reader got to** is its request machine's: `tolkster messages` prints a
worker only the owner's requests after the later of `readThrough` and `answeredThrough`.
`workerRead` moves its mark only forward, reported before it prints.

## Other records

- **A delegation Tolkster made to itself** is written just before delegating, so the Worker
  and `Session::is_owners` can trust the session Linear starts on the sub-issue (see
  [Parent issues](parent-issues.md)).
- **A worker**: if its write fails, the start fails and a worktree made here is removed.
  One missing from every complete Orca listing for an hour, or on an unpaired host, is
  reported as `worktreeGone` and deleted (see [Commands and reports](realtime.md#commands-and-reports)).
- **An issue's context comment** and **Tolkster's app user ID and sessions' issues** are
  in [Context for related issues](context.md) and [Webhooks](realtime.md#webhooks).
- **The relay's indexes** are the relay's alone. A worktree's entries go in the write that
  drops it from its issue's `worktree` fact, so a listing walks only live workers.

## Writing and claims

The Worker records directly; the CLI through `POST <WORKER_URL>/state` with the realtime
secret, or a [worker host's credential](hosts.md). The Durable Object checks every field and stores only what it rebuilt from them. A
failed or 5xx call is sent again twice, so every operation is safe to repeat. With the
Durable Object unreachable a command fails.

Linear has no lock, so claims live here. A take is atomic: a run gets every claim it
asks for or none. A take by the token that holds a claim takes its TTL again, keeping the
time it was first taken. Each take carries the run's token, so a lost response can be
retried, and a run whose stale claim was taken over can't release the newer holder's. Claims older than 30 days are removed. The machines take no claim: a command in
flight per issue keeps an issue's work apart.

## The reset

A reset logs each request it loses (`storage_reset_dropped`); it loses every queued request and the blocker index, so it owes a rebuild
(`src/worker/rebuild.ts`), retried until it succeeds. Each issue Linear shows delegated to
Tolkster and not yet started, whose store has no machine, reads its issue. The intake
mapped none of their sessions, and a worker's reports name its session, so the owner's
delegation begins nothing: its session is asked once for a reply (`unheard`), and the reply
begins the request. A started one is left, as its worker may still run. `listen`'s connect
owes the same rebuild, unless one is owed, asking only issues whose store never sent a
command: a retired store keeps no trace of a request that ended, which would otherwise be
asked again. A parent the kept delegations name, while the owner
still stands behind it, takes its chain back of the sub-issues still delegated, so their
sessions are trusted again, each mapped by its issue's read, and its take-back halts
them, and its orchestrating session rests owing the sub-issues' end, asked at once if they
have all finished. A chained sub-issue that is a parent itself waits for its own parent's
chain, then takes its chain back.

## The contract

`test/contract.json` holds every shape the Durable Object stores as the CLI writes it. Both sides test against it, so a field added on one fails its test until the other has it.

## Invariants

- The record holds IDs, timestamps, enums and identifiers only, and a worker's model, a plain name from config.json checked on both sides. Times only move forward, so a late or repeated write can't take a record back.
- A claim is judged stale by the TTL its holder took it with, never the next taker's.
- Every claim that guards an action goes through `state::holding`.
- A worker's record is written once, after Orca names the worktree, and is deleted, never kept marked.
- A worktree's ID and a terminal's handle name their host (`<host>@<repoId>::<path>`, `<host>@<handle>`) from the call that made or listed them, never a row's `hostId`; none is this machine's.
- A worker's or turn's answer never answers a request `tolkster messages` hasn't shown that reader (its own mark, `readThrough` or `turnReadThrough`, reported before it prints).
- A new worker of a session starts with the worker's mark cleared; a turn's read never counts as the worker's.
- Tolkster's own posts are never read back from Linear's activities to decide anything.
- A rebuild never begins an owner's request on a session the intake didn't map: the owner's reply does, and a chain's session, any read of its issue while its chain holds.
- The relay's indexes name an issue by its ID alone, written only from a webhook or a command's result for that issue, and retire with its records.
- Stored state is reset, never migrated: a stored shape's change bumps `STORAGE_VERSION`, whose first start deletes all but the machines' counters (no command ID repeats), context comments and delegations.
