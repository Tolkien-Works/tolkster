# Landing on main

A repo whose `repos` entry sets `"land": "direct"` takes a worker's change straight onto
its default branch, with no PR. `"pr"`, the default, keeps the PR flow ([PRs opened as the
agent](github-app.md)), as does an issue with `manual-merge` in either. A `direct` repo's worker
finishes with `land`, after a gate that runs what CI would (`scripts/gate.ts --all`).

## `tolkster land`

`tolkster land --session <id> --title <text> --body-file -` takes the place of `pr create`:

1. **Checks**, before the branch changes. The session must be the owner's (exit 1). It refuses
   (exit 2) when the checkout's Orca repo doesn't land `direct`, the issue has
   `manual-merge`, the title isn't one line, or the checkout isn't the worktree Orca links
   to the issue, on its branch `<prefix>/<key>` or `<prefix>/<key>-<n>`. A changed tracked
   file exits 1.
2. **The squash.** Every commit since origin's default branch becomes one, `<title>
   (TW-123)`, with the body as its message's body. The commit hooks run, so the co-author
   trailer goes on it ([Co-author](co-author.md)). A finished issue, or a branch that
   changes nothing there, gets no further (exit 2 and 3).
3. **The push.** Rebased onto origin's default branch, then pushed there as the app
   with no force. A push refused because the branch moved on is fetched, rebased and made
   again, four tries in all. A conflict aborts the rebase and exits 7 with
   `{"conflicts": [paths]}`, pushing nothing; the branch is left squashed for the worker
   to rebase. A rebase that drops the commit, its change already there, exits 3.
4. **After the push**, each failure said in the JSON, never undoing it: the issue moves to
   its team's first completed state (`issue_state::complete_issue`, never from a finished
   one), since no PR's merge will move it; the commit is linked on the session; origin's
   copies of the issue's branches are deleted, as a merged PR's is, so no later worker
   resumes from their unsquashed commits; and, unless that move failed, the landing is
   reported to the Durable Object (`landed`), which takes it as the work's merge ([The
   state machine](state-machine.md)).

A HEAD already on origin's default branch, whose subject ends ` (TW-123)` and which this
branch's reflog shows a commit or rebase here made, was landed by an earlier run: `land`
prints it again with `"pushed": false`, pushes nothing and repeats step 4. Any other HEAD
there exits 3.

## The owner's steps before a repo turns `direct` on

1. On the default branch's ruleset, add the GitHub App as a bypass actor for the
   pull-request and required-status-check rules (or drop them), keeping
   `non_fast_forward` and `deletion`.
2. Turn off Copilot review for the default branch, which nothing would use.
3. Set `"land": "direct"` on the repo's `repos` entry.

## Invariants

- In a `direct` repo the guard on the default branch is the worker's own gate and
  pre-push review, not GitHub's ruleset: every worker's push token can push there.
  `land`'s checks (the owner's session, its own issue's worktree, fast-forward only) keep
  an honest worker right; they don't stop a misled one.
- `land` never forces a push: what is on the default branch stays there.
- The push is the one step that can't be taken back, and the last that can fail the
  command: a run again pushes nothing and makes every step after it again.
