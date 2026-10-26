# PRs opened as the agent

A worker opens its PR as the GitHub App, so the PR's author is the app's bot and the
owner can **Approve** it like anyone else's. Replies on review threads, resolving them,
[auto-merge](auto-merge.md), the description, a behind PR's base merge and a clean PR's
merge go through the app too. Commits are the owner's, with the agent as co-author ([Co-author](co-author.md)),
and so are the `listen` host's pushes; a worker host's are the app's ([Pushing](#pushing)).
Every read of GitHub goes through the app as well, so Tolkster needs no `gh`. The app is
set up in [Set it up, step 4](setup.md#4-the-github-app).

## Opening and auto-merge

- `pr create` (`github.rs`) returns a branch's open PR (`"existing": true`) rather than
  open a second, so a retry after a lost answer ends where the first would have. With
  `--session` it links the PR and moves the issue to review as Tolkster ([Issue
  states](issue-states.md)); with `--auto-merge` it also turns auto-merge on.
- `pr auto-merge` is safe to repeat. Exit 8 with nothing on stdout means the budget or
  GitHub was still unavailable after its retries: a later try may succeed, a refusal's
  (exit 1) won't.
- `pr create` tells GitHub's refusal on a PR mergeable now by reading it `CLEAN` again, in a
  repo allowing auto-merge, never by its text, and prints `on` once it has reported
  `mergeAsked` for the checkout's repo ([Who merges](auto-merge.md#who-merges)).
  `mergePr` merges only the issue's own branch's PR, with `expectedHeadOid`.

## Pushing

On a host with a credential, `tolkster worker` puts git's credential helper, `tolkster
git-credential`, in the environment of its branch push and its `claude`, for the worktree's
repo on github.com alone: no config file is touched, and the host's own git use isn't. It
clears the host's other helpers for github.com and answers with a `push` token (`contents:
write`, one repo), asked of the Worker per push and given to git alone.

- The pusher is the app's bot. Authors and the co-author trailer come from the commits, so
  are unchanged; a rule restricting who may push to a branch needs the app listed.
- CI starts as for any push. A change to `.github/workflows` is refused: the app has no
  `workflows` permission.

## Copilot's review

Nothing in Tolkster requests it: whether Copilot reviews the app's PRs is the ruleset's
setting on `main` ([Auto-merge](auto-merge.md)). GitHub bills a bot's Copilot review to
the org as AI credits, so the org (Copilot Business or Enterprise) needs a budget for AI
credits and "Allow members without a Copilot license to use Copilot code review". Without
them the ruleset's request on an app-authored PR silently requests nothing, and a
required approval leaves auto-merge waiting for good.

## Reading and describing a PR

- **`pr feedback`** prints only what the configured `review.reviewers` wrote: no one else's
  text, not even the PR author's bot's, reaches the worker. It fails (exit 1) rather than
  leave feedback out. With `--session` it records the read on the session once printed
  ([Unread feedback](review-feedback.md#whose-feedback-and-when-it-resumes)); a session it
  can't check fails the read first. `followUp` marks a thread opened by an `APPROVED`
  review ([Review feedback](review-feedback.md)).
- **`pr checks`** lists the checks of the commit the worker pushed. GitHub moves a PR's
  head a moment after a push, so until the PR's head, the commit whose rollup was read and
  the pushed commit are one, the state is `pending` with no checks. A PR merged or closed
  is `merged` (exit 5) or `closed` (exit 6). A check is `unavailable` when it needs a person (`ACTION_REQUIRED`) or Actions
  concluded it without running (`never_ran`, as when minutes run out): no worker can clear
  it, and the machine tells the owner (`ciCouldntRun`). An open PR that conflicts with its base
  is `conflicts` (exit 7) whatever its checks, as GitHub runs no `pull_request` workflow
  on it; behind alone isn't. No checks while its mergeability is `UNKNOWN` is `pending`. Workers run it only
  before a push, never to wait on CI: a red, conflicted or stuck head comes back through
  the machine ([Review feedback](review-feedback.md)).
- **`pr describe`** replaces the whole description of an open PR; a retry is safe.

## Reading in listen

- A worker's setup, on its host, reads its PR's base's active rules once (`copilot_reviews`)
  to render whether Copilot reviews the PR ([Pre-push review](pre-push-review.md)).
- `branch_prs` is one light query of the PRs on a worker's branch; reap decides on that
  list alone. The machine's `evaluatePr` reads one PR's reviews, threads and comments by
  number (`read_pr`).
- `listen` keeps a token per repo and purpose for 50 minutes and revokes all as it ends; a
  command mints its token for its one call and revokes it after.

## The budget

Workers, `listen`, reviews and reaping spend one installation's rate limit.

- A spent budget is its own error (`Error::RateLimited`, exit 1) carrying the wait until
  its reset (`github/app.rs`, `respond`). The gateway holds calls unmade while it is spent
  ([Budgets, lanes and trying again](budget.md)). `pr auto-merge` waits it out once; every other `tolkster pr` command fails at once.
- A command that finds it spent (`evaluatePr`, `startRound`) ends `retryable` and the
  machine sends it again ([Commands](state-machine.md#commands)). A start that can't read
  the base's rules still starts, taking Copilot's review as off.
- Each call's cost is logged under its function's name (`github/cost.rs`), to find who
  spends the budget: always in `listen`, in a worker's command with `TOLKSTER_LOG_GITHUB`.

## Threads, tokens and signing

- `pr reply` and `pr resolve` first read the thread and refuse one not on that PR in that
  repo. Resolving again is safe; a reply is never retried by the command.
- The app's key and IDs are Worker secrets: the Worker mints every host's tokens
  ([`/token`](hosts.md)). A token lives in memory only, never on disk, in the environment
  or output, and never given to `git`. Nothing prints the key, the JWT or a token.
- A repo's owner must be the one the mint's response names, ignoring case: a token is
  minted by repo name alone, so another owner's repo of the same name would otherwise get
  the installed one's.

## Permissions

The app has Pull requests: read and write, Contents: read and write, Checks: read, Commit
statuses: read, Actions: read (a host's download of CI's build), Issues: read (for the webhook's `issue_comment` alone) and Metadata: read.
Each token asks for only what its one call needs (`PURPOSES` in `src/worker/mint.ts`): opening a
PR needs Contents read (else 422 "not all refs are readable"); resolving a thread needs
Contents write (else GitHub refuses `resolveReviewThread`), as do a branch's update and a
merge, `listen`'s alone, and auto-merge, which only someone who could merge may enable;
reads ask read alone. The app
is never widened quietly: a change that needs more says so.

## No fallback

- The worker never opens a PR, answers or resolves a thread, or turns on auto-merge as
  the owner. If `pr create`, `pr reply` or `pr resolve` fails, it posts an `elicitation`
  naming the step and stops, never quoting the error, which can hold GitHub's text or a
  local path. Auto-merge `refused` doesn't stop it; `unavailable` runs `pr create` once
  more, then stops.
- Nothing else depends on the author: reap and the review resume find a worker's PR by its
  branch. The one use: the bot of a PR the app opened is never a blessed reviewer on it.

## The webhook

The app's webhook tells the Worker of each PR event in a repo it is installed on, and `listen` resumes the PR's worker or reaps its worktree within seconds
([Review feedback](review-feedback.md#the-webhook)).

- **Endpoint.** `POST <WORKER_URL>/webhooks/github`. The Worker verifies
  `X-Hub-Signature-256` against `GITHUB_WEBHOOK_SECRET` and refuses everything while it is
  unset. It maps a delivery to IDs-only events through the intake, and keeps a push to a
  default branch as the repo's base tip ([The base tip](realtime.md#the-base-tip)).
- **Events.** `GITHUB_EVENTS` in `src/worker/github.ts` is the list, pinned by a test;
  `doctor` fails each the app isn't subscribed to. `status` names no PR, so the
  intake maps its SHA to an issue ([events](state-machine.md#events)). `check_suite`
  `completed` reaches every app that can read checks, so a failed Actions run on a
  worker's branch arrives without `check_run` or `workflow_run`.
- **Set-up.** `github app create` subscribes the app to the list and sets
  `GITHUB_WEBHOOK_SECRET` ([step 4](setup.md#4-the-github-app)); a later event is subscribed
  in its settings. The app's **Advanced** tab shows the Worker's
  answer per delivery: 200 once set up, 503 while the secret is unset, 401 for a wrong one.

## Invariants

- `github app create` sets the app's ID, key and webhook secret in one `wrangler secret
  bulk`, its stdin the only place they go, and exchanges only a return to its own
  `127.0.0.1` address with its run's state (`commands/github.rs`).
- A push token reaches git alone, for https on github.com only (`credential::answer`).
- A token is never handed out for a repo whose owner the mint's response doesn't name
  (`installationToken`, which revokes it); held tokens are keyed by owner too.
- A fork's PR is never one of a worker's branch's PRs: `branch_prs` keeps only
  `isCrossRepository` false, and a list over a page fails the read.
- Reap never reads an open PR's reviews or threads.
- `pr checks` never reports a commit's checks as the pushed commit's (`github::checks`),
  and reads whether the PR is open before it compares heads, so a merged or closed PR is
  never `pending`, and whether it merges only after both, so `conflicts` is never an old
  head's.
- A read never records past a review it didn't print (`sift_feedback`).
- A check is classified once, by `CheckState::of`, so `pr checks` and `evaluatePr` never
  differ on whether a PR is green.
- Only `github::` reads GitHub's JSON for a PR or its checks: an open PR's first page
  (`FirstPage`) is opaque until `read_open_pr` has read the rest.
- A cost line holds a fixed call-site name, the budget's name and numbers only
  (`cost::line`).
