# Timing

How long a worker takes, and where its time goes, is measured where it runs: each worker
session's hook summarises its own transcript, and `listen` keeps the summaries, with each
issue's start and merge, in `~/.tolkster/timing.jsonl` on its host. A recipe machine's
transcripts go with the machine, so nothing reads them later.

## Capture

`tolkster worker` puts how it launched the agent in its environment (`TOLKSTER_LAUNCH`: start,
resume, round or rebase, the time and its build). The hook summarises the session's
transcript, and its sub-agents', from that time on (`hook/timing.rs`), at each stop, at the
session's end and before a call that closes its own terminal, as a worker's last does, after
which no stop comes:

- **Seconds:** wall, active and the model's. A gap of 4 minutes or more with no call running
  across it is idle, not active; a reply takes the time since the moment before it.
- **Tools:** seconds by kind (read, edit, check, git, bookkeeping for `tolkster` and `orca`,
  sub-agent, other), API turns, and those that called a tool.
- **Offsets** from the start: the first edit, commit and gate, the review's start
  (`pr review-pass`) and end (its last sub-agent's), and the PR (`pr create`), which is
  when the PR opened: nothing else tells `listen` that in time.
- **Each check run:** its kind, whether it failed when its end says so (as a paired call's
  does), whether a file was edited since the session's previous run, its seconds, its offset
  and whether a sub-agent ran it.

## The setting

`timing: false` in `listen`'s `config.json` puts `--no-timing` on each worker's command, so
no launch reaches its agent and its hook reports nothing, on a recipe machine too. `listen`
reads the setting when a report arrives, so a worker launched earlier writes nothing either. A period off leaves gaps: a merge may have no start or session.

## Transit and the file

The worker's `tolkster` sends the summary as the `sessionTiming` report. The relay checks it,
names its issue by its worktree and sends it to `listen`'s newest connection without the
worktree (`src/worker/timing.ts`), keeping nothing. With none to take it, it answers 503 and
`tolkster` tries twice more; a report lost so, or sent to a `listen` dying unseen, leaves the
session's earlier line, and one whose worktree the relay hasn't indexed is dropped. `listen`
also writes each worker it starts (issue, size and its rung's name), and each merge: its own,
a reap after a merge or landing, or a PR read merged, at GitHub's time.

Each line has a `key` and an `at`. A start is written once by its key, a merge again only at
GitHub's time (`exact`) in place of when `listen` heard of it, and a session's again only by
a fuller report of it, longer or with more check runs, so every report is safe to repeat. A
session's key names its issue, so a report can't replace another issue's line. A line older
than 90 days is dropped as the file is written, off the connection's loop.

## Reports

`tolkster timing`, on `listen`'s host, reads the file. An issue counts once, by its first run:
from its first start to the next, with its resumes, rounds and rebases, and its first merge
before that; one with no session read isn't counted. A wait on the owner, from the last
session's end to the resume after it (a
plan-first reply, a parked terminal answered), is `ownerWait`, taken out of every elapsed time;
a wait for a round isn't the owner's. Orienting runs from the start to the first edit,
implementing to the gate or review, whichever starts first, and review and gate to the PR;
the deep review's share is its span over that last. Shares are of the run's active seconds.
Every figure is a median per issue in its group, null when no issue has it.

`--json` prints `{"since", "until", "by", "groups"}`, each group `{"group", "n", "elapsed":
{"startToPr", "prToMerge", "startToMerge"}, "phases": {"orienting", "implementing",
"reviewAndGate", "deepReviewShare"}, "active", "ownerWait", "shares": {"model", "check",
"read", "edit", "git", "bookkeeping", "agent", "other"}, "toolTurns", "checkRuns"}`, in seconds
and fractions. With `--checks`, `{"group", "n", "checkRuns", "byKind", "by": {"main",
"subAgent"}, "byPhase": {"beforeReview", "review", "after"}, "byOutcome": {"failed", "passed",
"unknown"}, "byEdit": {"noEdit", "afterEdit", "fixAfterFailure"}, "minutes"}`: check runs per
issue, a round's after, and check minutes by kind with a `total`. A group is a size, a rung's
name (`model`) or a build's first seven characters, else `none`.

## Invariants

- The summary and the file hold IDs, enums, counts, durations and offsets, and a rung's name
  from config: never a command's text, a path, output, or Linear or GitHub text. A key is a
  hash.
- One line per session (its issue, Claude's session and its launch), start and merge.
- The file is read only to keep each line once, and for reports, never to decide anything
  else.
- Turning `timing` off deletes nothing kept.
- A report counts an issue once, by its first run, reads a session line only in its plain
  shape, and never counts a wait on the owner as the worker's.
- The relay keeps nothing of a timing report, and a host reports only its own workers'.
