# Tolkster

An agent you can delegate Linear issues to. It appears in Linear's delegate menu like
any agent. Delegate an issue to it, or @mention it in a comment on an issue, and a
Claude Code session in an Orca worktree on your workstation works the issue: it posts
its progress to the issue, opens a PR as its own GitHub App, and answers your
reviewers' feedback. Reply in the session's thread to talk to that worker, or press Stop
to end it. It acts for its owner alone: anyone else is refused.

"Tolkster" is what we called ours; give yours any name ([setup](docs/setup.md) says
where each lives).

## What you need

- **A workstation that stays on** (Windows, macOS or Linux), running **Orca**.
- **Claude Code**, signed in, in **auto mode**.
- **An admin of the Linear workspace**, to install the agent.
- **A Cloudflare account**, for the Worker that takes Linear's and GitHub's webhooks.
- **GitHub**, where the repos live; the agent gets its own GitHub App.

Git and Node too: [the prerequisites](docs/setup.md#0-prerequisites) say which.

## Quickstart

1. Pick the agent's and the Worker's names ([step 1](docs/setup.md#1-names)): the
   first deploy fixes the Worker's.
2. Download the CLI from the latest release onto PATH ([step 2](docs/setup.md#2-the-cli)),
   and add your repos to Orca ([step 6](docs/setup.md#6-orca-repos-and-tolkster-listen)).
3. Run `tolkster setup`. It deploys the Worker and sets its secrets, installs the Linear
   app, makes you the owner, makes the GitHub App and adds a team, then prints `next`:
   what is left to you. Run it again after a failure; it skips what is done.
4. Keep `tolkster listen` running from a checkout of this repo on `main`.
5. Delegate a throwaway issue to the agent ([try it](docs/setup.md#7-try-it)).

## How it works

- [How it fits together](docs/architecture.md): the Worker, the Durable Object,
  `listen` and the CLI.
- [Set it up from scratch](docs/setup.md): the CLI, then `tolkster setup`
  for the rest.
- [How dispatching works](docs/dispatcher.md): the watcher, the cap, blockers, reaping.
- [The state machine](docs/state-machine.md): the request, work and triage machines.
- [Plan first and reconcile first](docs/plan-first.md): when a worker waits for the
  owner's go.
- [Parent issues](docs/parent-issues.md): working a parent through its sub-issues.
- [Context for related issues](docs/context.md): the comment a completion leaves.
- [Triage](docs/triage.md): Tolkster's own Triage queue.
- [Co-author](docs/co-author.md): how a worker commit credits the agent.
- [Pre-push review](docs/pre-push-review.md): a worker's review before its PR.
- [PRs opened as the agent](docs/github-app.md): the PR as the GitHub App.
- [Issue states](docs/issue-states.md): how Tolkster moves a worker's issue.
- [Auto-merge](docs/auto-merge.md): how a PR merges by itself, and how to opt out.
- [Landing on main](docs/landing.md): a repo setting that skips the PR.
- [Review feedback](docs/review-feedback.md): resuming a worker on feedback, a failing
  check or a rebase.
- [Tolkster's voice](docs/voice.md): the shared phrases and who posts them.
- [Realtime](docs/realtime.md): webhooks to events, commands to `listen`.
- [Tolkster's state](docs/state.md): the records in the Durable Object, the claim lock.
- [Timing](docs/timing.md): each worker session's timing, kept by `listen`.
- [The CLI's interface](docs/cli.md): exit codes, sessions, the owner.
- [Worker hosts](docs/hosts.md): setting one up, and what it holds.
- [The agent harness](docs/harness.md): what a worker's agent is launched with.
- [Linear's API](docs/linear-api.md): what each call costs of the hourly budget.
- [Budgets, lanes and trying again](docs/budget.md): the gateway's waits and retries.
- [The Worker's logs and traces](docs/observability.md): what the Worker logs.

## Development

The checks are listed in [`AGENTS.md`](AGENTS.md#checks); CI
(`.github/workflows/ci.yml`) runs them on every pull request, so
`check` can be a required status check (see [Auto-merge](docs/auto-merge.md)). A change
to docs alone skips the expensive steps inside the job, which still finishes green;
`AGENTS.md`, `README.md`, `cli/briefs/` and `docs/`, which tests read, count as code.
Don't bring back a workflow-level `paths-ignore`: a required check it skips sits
"pending" forever.

## Licence

MIT: see [`LICENSE`](LICENSE).
