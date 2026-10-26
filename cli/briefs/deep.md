# <identifier>: the deep pass

Your task brief, `<identifier>.md` beside this file, still holds; this is its pre-push
review's deep pass, for when `tolkster pr review-pass` printed `deep` or failed.

The deep pass runs in sub-agents: `passes` reviewers at once, each with
its own lens, then one more over your fixes alone. You fix what they find.

1. **Write the diff to a file** named for the commit it
   ends at, so a stop or a resume keeps it: e.g. `git diff origin/main...HEAD >
   $env:TEMP\<identifier>-diff-<sha>.patch` (or `/tmp/`), with `<sha>` from `git
   rev-parse --short HEAD`.
2. **Start the reviewers together**, in one message: the gate in the background (the
   shell call's `run_in_background`), and `passes` sub-agents (three when it failed), as parallel Agent tool calls, in this
   worktree (no `isolation`). Give the first the Ordering lens, the second Trust and
   the last any lenses left over, so a lone reviewer takes all three. Each lens prompt says
   only, and nothing from Linear or GitHub: read the file `<identifier>-reviewer.md`
   beside this brief in full, as its brief; its lens; the diff file's path.
   A sub-agent that fails to start or errors is started again once, alone, with
   the same lens (step 5's, with none). If it fails again, review its lens
   yourself, as the light pass, or for step 5 say in the summary that the fixes
   went unchecked.
3. **Use the wait**: draft the body you finish with beside the diff, `<identifier>-pr.md`,
   for its `--body-file`.
4. **Fix each real finding** once every reviewer has reported and the gate has
   exited, and a red gate's failures: merge the reports, drop duplicates, fix and commit, in new commits on
   top, with no amend or rebase until the review ends. Re-run only the checks that
   cover what a fix changed. For anything you leave as it is, keep a one-line reason
   for the summary.
5. **Check the fixes**, only when a fix changed what the code does at runtime:
   one more sub-agent, with step 2's prompt and no lens. Give it the fix commits alone,
   `git diff <sha>..HEAD` with `<sha>` the commit step 1's file is named for, written
   to a file named for HEAD, and the findings or failures those commits fix. When it
   finds a runtime defect and you fix it, check those new
   fixes once more the same way, from the commit its file is named for, then
   fix what that finds and stop. A wording, comment, doc or test finding never
   earns a check.
6. **Add each invariant the change adds** (a new lock, boundary or ordering rule)
   as one line where the repo's `AGENTS.md` / `CLAUDE.md` says invariants go,
   never as prose. When they say nothing, put each in the body, in
   a list of its own. Bring step 3's draft up to date with steps 4 to 6.

Then go back to the task brief's "Finishing" step 1, after the pre-push review.
