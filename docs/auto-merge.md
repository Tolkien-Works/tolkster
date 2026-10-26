# Auto-merge

A Tolkster PR merges by itself once GitHub's rules on `main` are met. The worker asks for it, as the GitHub App (`tolkster pr create --auto-merge`, see [PRs opened as the agent](github-app.md)), never directly nor as the owner; GitHub's auto-merge or Tolkster merges it. Nothing in the briefs or `listen` asks Copilot for a review (each costs the org AI credits): the ruleset does. A `direct` repo has no PR: its worker runs `tolkster land` ([Landing on main](landing.md)).

## The worker's side

- **Whether.** On unless the issue has `manual-merge` when `startWorker` reads it, or the worker's change needs a step only the owner can take before it merges: the worker leaves `--auto-merge` off, by its own knowledge of the change, and ends saying the PR waits on those steps.
- **No watching CI.** The worker ends once its PR is open, never waiting on its checks: a failed one brings it back as a review round (see [Review feedback](review-feedback.md)), and checks still pending an hour after a push tell the owner (`checksPending`).
- **When it can't be turned on** (the budget spent or GitHub unavailable after the retries,
  [The budget](github-app.md#the-budget)), the worker stops with an `elicitation` rather than leave the PR for a hand merge; when GitHub refuses, it carries on.

## Who merges

GitHub's auto-merge fires only as a requirement changes, so never on a PR that met them before it was asked: GitHub refuses it then, which `pr create` reports (`mergeAsked`) in a repo that allows auto-merge.

- A read with no worker mid-turn (`evaluatePr`) that finds GitHub's own `CLEAN`, checks passed, and auto-merge on or `mergeAsked` has `listen` squash-merge it as the app at the head read (`mergePr`); never a PR whose auto-merge the owner turned off, whose issue has `manual-merge`, or capped at its head, which `listen` checks again as it merges.
- One merged already, as by GitHub's auto-merge, is done; with `listen` down, GitHub's still merges. `UNKNOWN` after a push is read again at 10, 20 and 40 s.

## What holds a merge

- A push dismisses an approval, so while one is required GitHub waits for a fresh one after each round that pushed. An approval never resumes the worker; its threads do.
- When Copilot approves with edge cases as threads, the merge waits for the worker's follow-up round, which files, replies and resolves each; a person's thread is left for them. A PR capped at `review.maxRounds` merges on the owner's approval.

- When CI can't run, the machine tells the owner (`ciCouldntRun`), and the PR merges once they re-run the workflow. A round would add nothing: the gate ran every check before the PR, and the merge waits on CI anyway.

## What GitHub must require

Auto-merge is only as safe as the rules on `main`. Copilot is the owner's to switch:

- *Copilot on:* the "Copilot code review" rule and one required approving review, from
  Copilot or the owner. With "Review new pushes" off, a round's push dismisses Copilot's
  approval and nothing asks again, so auto-merge waits for the owner's.
- *Copilot off:* no rule and no approval required, so the worker's pre-push review is the only review (see [Pre-push review](pre-push-review.md)).

In both: every review thread resolved, and the CI job `check` required. It runs on every pull request so it can be: a docs-only change skips its steps inside the job, where a workflow-level `paths-ignore` would leave it pending forever.
