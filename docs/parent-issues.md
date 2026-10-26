# Parent issues

Delegating a parent issue to Tolkster works through its sub-issues side by side, each
delegated to Tolkster and run as a delegated issue always is (its own session, worker,
branch and PR), one waiting for another only where it depends on it. Sub-issues may nest
to any depth.

## Working through the sub-issues

A delegated issue with unfinished sub-issues starts an **orchestrating worker**. It takes
a slot, and `startWorker`'s `orchestrate` gives its brief the "Sub-issues" section
(`cli/briefs/children.md`). The worker adds a blocked-by relation only for a genuine
dependency: one sub-issue can't proceed without something the other builds, or the wrong
order would mean a redesign, major rework or conflicts too big for a routine rebase;
small overlaps are left to each worker's rebase. It delegates the
unblocked sub-issues, then the rest, in one `tolkster issue chain` call, and exits with no
branch or PR. The blocker rules start each blocked sub-issue when its last blocker
finishes, so one parent can fill every slot; the owner can serialise them by adding
relations.

## The parent picks up again

Each sub-issue's change reaches the parent's store as `childChanged`. Once the last finishes and the session's
newest request is answered, the session asks again, once per orchestration, with the
sub-issues' end, and the parent's worker moves it to Done or does its own work. A stop or
a halt clears the end owed, so it never resumes a worker the owner stopped.

The pick-up doesn't need the orchestrator's worktree, which holds no work.
With none linked, a resume for the end, for added sub-issues, or for the owner's message on
an issue with sub-issues starts a new orchestrator in a new worktree on the same session,
on the brief and then the resume's fixed line. Any other worker's resume then fails.

A sub-issue added once the worker has chained them is picked up while the orchestration
is live. Once no worker runs, the session asks again (`childrenAdded`), which sends the
worker back to `issue chain`; that leaves the sub-issues already running as they are. A
sub-issue counts as added when it is unfinished, out of Triage, and neither among those
the worker was last sent to chain (`known`) nor asked or taken by its chain. One delegated
to someone else is still added, since the parent's store doesn't know its delegate: it
costs one resume.

A sub-issue in Triage doesn't hold up its parent's end; accepted out of Triage it is
picked up as an added one. The first 50 sub-issues are read with each issue; any beyond
count as unfinished, so a parent never picks up early, nor by itself.

## Trust: issues Tolkster delegated to itself

The session Linear starts on a sub-issue Tolkster delegated is Tolkster's, not the
owner's, and is trusted like it by checks in code, never by a worker's word:

- `issue update --delegate` and `issue chain` delegate only to Tolkster, and only an
  unfinished sub-issue of the session's issue, where the session is the owner's (or
  trusted as below) and its issue is delegated to Tolkster and not cancelled. One
  delegated to someone else is left with them; one in Triage, or blocked by one, is
  skipped, since it would hold the parent's end until someone accepts it. Just before
  each, they record the delegation in the Durable Object ([state](state.md)).
- The Worker acknowledges a session the owner didn't start only when that record exists
  for its issue and the session isn't a mention (`src/worker/decide.ts`).
- `Session::is_owners` accepts such a session while the parent, read fresh, is still
  delegated to Tolkster and not cancelled, unless someone other than the owner or Tolkster
  delegated it since, and only if Linear names Tolkster, or no one, as its creator. Every
  check that keys on the owner goes through it.

## The chain in the machines

The events between stores (`chainRecorded`, `childChanged`, `parentHalted`,
`takeBackChain`) are in [Parent issues](state-machine.md#parent-issues).
`chainRecorded` is taken only from the session whose request is `orchestrated`.
`parentHalted` (a parent taken from Tolkster or cancelled) halts a sub-issue's worker
unless the sub-issue is completed; `takeBackChain` takes back only the delegations
Tolkster's record says the chain made.

## Invariants

- `issue chain` decides every refusal on one read and the parent's store's answer to
  `chainRecorded`, before its first write, so a refusal writes nothing; it creates every
  blocked-by relation before any delegation, so no sub-issue starts ahead of its blocker.
- A sub-issue delegated to someone other than Tolkster is never delegated over, and a
  cancelled parent delegates nothing (`delegating_parent`).
- A delegation's record is written in the Durable Object before its Linear delegation, so
  the sub-issue's session is trusted when it starts.
- A sub-issue identifier reaches an orchestrating worker's first line only as an
  `IssueKey` (`plain.rs`).
- One failed release never stops the take-back of the others; `takeBackChain` still
  fails, so it is sent again.
- A sub-issue in Triage is never delegated or blocked on, nor is one waiting on it, and
  never holds up its parent's end.
- A sub-issue added mid-orchestration resumes the parent only while no worker runs, the
  orchestration is owed and its chain is recorded and not halted, once for every sub-issue
  added together, on a fixed line that names no Linear text.
- A sub-issue tells the parent it held whenever it learns of a move, however late, and
  that parent keeps it as finished at the move's `updatedAt`, so only a newer change
  counts it there again.
- A move off the parent whose chain delegated a sub-issue ends that chain on both sides,
  so neither halts nor takes back the other (unless a take-back was owed), and
  halts the sub-issue's own chain unless the owner delegated it.
