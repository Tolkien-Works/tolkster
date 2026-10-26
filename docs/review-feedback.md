# Review feedback

A worker exits once its PR is open, never waiting on CI; its worktree stays until the PR
merges. When anything
happens on the PR, the work machine has `tolkster listen` read that one PR (`evaluatePr`)
and runs `startRound` when the worker needs to act again ([Work
transitions](state-machine.md#work-transitions)). A round doesn't repeat the [pre-push
review](pre-push-review.md): Copilot reviews its push when the ruleset asks it to, and
otherwise the round gives its changes a light pass.

## Whose feedback, and when it resumes

- **Blessed reviewers.** Only feedback from the logins in `review.reviewers` counts. A
  `repos` entry's own `review` replaces the global settings it sets for its repo's PRs
  (`Config::review_for`; [setup](setup.md#6-orca-repos-and-tolkster-listen)); with none
  for a PR's repo, the PR isn't checked. The bot that authored the PR is never blessed on
  it: its comments are the worker's own replies.
- **The last round** is the newest the work machine started on the PR ([Facts](state-machine.md#facts)).
  Feedback, a failed check and a moved base count only when newer than it.
- **Resume** when any of these is new:
  1. an unresolved, non-outdated review thread whose latest comment is blessed;
  2. a blessed person's `CHANGES_REQUESTED` review, or `COMMENTED` review with a body. A
     bot's review counts only through its threads, and an approval never counts;
  3. a blessed comment on the PR conversation;
  4. the PR is `DIRTY` or `BEHIND` and its base moved. GitHub reports `BEHIND` only when
     branch protection wants branches up to date; a PR merely behind a base that doesn't
     is merged as it is. One only `BEHIND`, with no agent running, wakes no worker:
     `updateBranch` merges its base in as the app; only GitHub's refusal starts a round.
     `UNKNOWN` after a base move is read again, never acted on;
  5. a required check on the PR's head failed, since no worker waits on its checks. One that never ran or waits on a person is
     `unavailable` and resumes nobody ([below](#a-pr-that-wont-merge-by-itself)). The
     resume keeps each check's state, time and whether it is required, never its name or
     output.

## Resuming

- **The line.** The worker gets one fixed line naming the PR, the session and its round
  brief, never the feedback: typed into its terminal if it is running and its agent
  doesn't wait at a prompt (otherwise the command ends `held`), else a new session on its
  briefs, or the harness's resume line if they can't be found. It reads the feedback itself.
- **Who.** Only a worker whose issue still wants work, never a worktree Tolkster has no
  record of; one round in flight per issue.
- **A round in flight owns what lands during it.** From the `startRound` sent until the
  worker goes idle or exits (`inRound`, the head it began on), feedback on that head newer
  than the last round is the round's to read: the machine moves `lastAt` up to it and
  counts no round. A rebase-only round, which reads none, feedback on a later head and a
  failed `startRound` get no such cover.

## What the worker does with findings

The round brief (`cli/briefs/round.md`) holds the steps; a round for a rebase alone gets
`cli/briefs/rebase.md`, with none of the feedback steps. What they rest on:

- **Edge cases become follow-ups.** A thread is `followUp` when the review that opened it
  is `APPROVED`, read from the review's state, never its text. Filing it pushes nothing,
  so the approval stands; the ruleset requires every thread resolved, so the merge waits.
- **A leased push.** Auto-merge can merge the PR mid-round and GitHub delete its branch,
  and with `fetch.prune` a bare `--force-with-lease` recreates it, so the round pushes
  only with `--force-with-lease=<branch>:<sha>`.
- **Replies before resolving.** Copilot reopens findings resolved without a reason. If a
  reply or resolve fails the worker asks the owner and stops
  ([No fallback](github-app.md#no-fallback)).

## A cap on review rounds

`review.maxRounds` (default 3, or the repo's own) caps a PR's rounds (`rounds`, across every
head), follow-ups-only rounds included, rebase-only ones not. The last round (`cli/briefs/last-round.md`)
fixes only blocking findings and ends "Ready for your review.", since after the cap a
Copilot review doesn't resume the worker: the machine asks the owner once per head
(`roundsCap`). Another reviewer's feedback, a failed check or a base the PR fell behind
still resumes it; a base, with a rebase-only round, at any count.

## A PR that won't merge by itself

Five outcomes would leave a session reading as though the work will merge. The machine
says each once per PR and head, as a notice in the session that last gave the worker a
request ([Commands](state-machine.md#commands)); nothing is woken or removed.

- `prClosed`: closed without merging.
- `autoMergeOff`: turned off after being on, with the worker exited; never for an issue
  with the `manual-merge` label, which `evaluatePr` reads from Linear.
- `ciCouldntRun`: the rollup failed and a required check never ran or waits on a person.
- `checksPending`: the head's checks still pending an hour after its push.
- `copilotWontApprove`: Copilot's newest review only commented on the head, with no open
  thread, approval or pending check, and GitHub still holding the merge: Copilot won't
  approve after it, so the merge waits for a person. Never with `manual-merge`.

## The webhook

The GitHub App's webhook ([The webhook](github-app.md#the-webhook)) becomes the PR's
event, by repo and number, and nothing more ([Webhooks](realtime.md#webhooks)).

- A burst is one round: events that land while the PR is read fold into one read after
  its result, and the feedback it finds is left to the round in flight.
- A push to a default branch is the coordinator's `repoPushed`, which sends `basePushed`
  to each issue with a PR open in the repo.
- A lost delivery is read again at the state's deadline.

## Invariants

- A line that resumes a worker is typed into a terminal: `review::line` and `fresh` take
  an `IssueKey` (`plain.rs`).
- A round is decided only by the work machine, from an `evaluatePr` read of the head it
  holds, and a worker's terminal is listed again just before the line is typed.
- A review thread's comment counts from its `publishedAt` (`github::published`), and from
  `createdAt` only while that is null: a comment drafted before a round and submitted
  after it is still new.
- A read is recorded only after it is printed, only on a session the owner started, and
  only of the checkout's own repo (`pr feedback --session` refuses `--repo`).
- A PR is reviewed with its worktree's repo's `review` settings, looked up by
  the repo's ID in Orca's listing; a listing that fails is an error, never the global
  settings.
- Only `running`, `held` and a round's `launching` keep `inRound`, which a failed
  `startRound` clears.
- A `startRound` carries its proposal (`lastAt`, `lastBase`, `lastFailed`, its number),
  which a read of the PR while it is in flight counts as covered. Its result commits it to
  `rounds` when the line may have reached the worker, a typed one only once a turn is seen
  begin. One that fails with no line typed, or whose agent exits first, drops it, and the next read waits for the retry's deadline
  (10 min), or a persisting cause would start the round again with no pause.
- A never-ran check is told once per PR and head and moves no cursor.
- A branch update is leased on the head the machine read (`expectedHeadOid`) and made
  only on the issue's own branch.
