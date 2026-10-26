# Pre-push review

A problem a local review would catch costs a whole push → CI → Copilot → resume → push
cycle, so a work worker reviews its own work once, before it opens the PR; with Copilot's
review off ([Auto-merge](auto-merge.md)) it is the only one the PR gets. Its depth is
decided in code by `tolkster pr review-pass`, never by the worker, from the issue's size,
whether Copilot reviews and what the diff adds. It is skipped when every changed file is
one the repo's CI skips, and never when the repo lists none. [Review
feedback](review-feedback.md) rounds don't repeat it: with Copilot reviewing each push
they run nothing of their own, otherwise a round's push gets the light pass.

## Which pass

`tolkster worker`, rendering the work brief, reads the active rules on the PR's
base as the app (`copilot_reviews`, [Reading in listen](github-app.md#reading-in-listen)).
Copilot reviews when the `copilot_code_review` rule is among them (each push when its
`review_on_push` is set, else the opening only); a failed read takes it as off.

The start also reads the issue's size (`review_size` in `brief.rs`) against
`review.deepAt` ([Setup](setup.md), default M), keeps it in `Taken`, and renders both
into the command the brief runs (`review_pass.rs`, [Output](cli.md#output)). If the
command fails, the brief takes the deep pass, with three reviewers. A start with no
estimate renders `unsized`, and the command, given `--session`, reads the estimate as
Linear has it now and takes that size's row. An under-sized issue is still caught by the
triggers, which read the diff, not the size.

The repo's list is the fenced `tolkster-review` block of the first of `AGENTS.md` and
`CLAUDE.md` on the default branch: `skip` and `keep` (the paths CI skips, and the
exceptions it still runs on), `trust` (the files where ordering and trust live), `tests`,
`comment` and `import`.

| Size at the start | Copilot off | Copilot on |
| -- | -- | -- |
| Below `review.deepAt` (default zero, XS, S) | Light pass, deep on its triggers (200 lines, 1 reviewer) | Light pass, deep on its triggers (500 lines, 1 reviewer) |
| `review.deepAt` (default M) | Deep pass, 2 reviewers | Light pass, deep on its triggers (500 lines, 3 reviewers) |
| Above it (default L and up) | Deep pass, 3 reviewers | Deep pass, 3 reviewers |
| No estimate, or a team without estimates | Deep pass, 3 reviewers | Light pass, deep on its triggers (500 lines, 3 reviewers) |

An empty branch skips it too.

### Triggers

A trigger takes the light pass deep. Each counts only **new** lines of the diff against
the default branch: added lines that aren't blank, aren't in a `skip` path and aren't
moved. The first that holds is the `trigger`:

- `trust-file`: a branch that changes the list itself, or a new line, or a deleted one
  that didn't move, not a comment or an import line, in a `trust` file;
- `new-lock`: a new line, not a comment and not in a `tests` file, naming a lock, a mutex,
  a permission, allowed or disallowed tools, or a Durable Object's `blockConcurrency…` as
  whole words after splitting identifiers (so `blocked` or `clock` never match);
- `new-lines`: more new lines than the row's count.

## The light and deep passes

The light pass reads the diff once; a deleted invariant comment, or needless markdown, is
a finding, and its fixes go unreviewed.

The deep pass runs in sub-agents, as the worker's context is largest by then
(`<ID>-deep.md`). Each reads `<ID>-reviewer.md` and the same diff, with its own lens so
they don't converge: ordering, trust or lifecycle. Meanwhile the worker drafts the PR;
then it fixes what is real, and a red gate, in new commits; only a runtime fix gets a
sub-agent checking them. Either pass ends in one durable action, its verdict the parameter, posted by `pr create
--review`.

## Invariants

- The shared briefs name no path, tool or check of one repo: `tolkster pr review-pass`
  takes the CI skip paths, the trust and test files and the comment syntax from the repo's
  `tolkster-review` block, and a worker takes its checks and where invariants go from the
  repo's `AGENTS.md` / `CLAUDE.md`; silent, the block names none and invariants go in the
  PR's description.
- The review list is read on the default branch, never the branch under review, so a
  branch can't loosen its own review.
- A trigger counts new lines alone: a moved line, a comment, or a lock in a test file never
  deepens the review, and an import line in a trust file is no trust-file trigger, though
  the lock trigger still reads it.
- The escalation thought is fixed text from `voice.rs`, the trigger and a plain path or a
  count, and the sized one the size's name: never Linear or GitHub text.
- The size is read at the start and kept in `Taken`: a later estimate never lightens the
  review, unless the start had none.
- Only the gate changes the worktree while it runs beside the review, and the PR
  waits for it and its failures' fixes.
- A deep pass's reviewers all see the same diff, from the same commit, and none sees
  another's findings; only the fix check runs after them, over fix commits alone.
- Every file a test of one side reads across is on that side in `AGENTS.md`'s `sides`
  block, and a test on each side fails otherwise.
