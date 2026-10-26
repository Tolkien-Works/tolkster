## Pushing in a round

Auto-merge may merge the PR while you work, and GitHub may then delete its branch. The
SHA `git ls-remote origin refs/heads/<branchName>` printed at the round's opening is your
lease. **The ancestor check**, right after the opening and before you change anything:
if `git merge-base --is-ancestor <sha> HEAD` exits 1, someone else pushed (GitHub's own
merge of the base included): `git rebase <sha>` first, and if that conflicts, post an
`elicitation` saying so and stop there.

Right before each push, here and in Finishing, run `tolkster pr checks --pr <n>`. Push
only with the SHA as a lease, `git push --force-with-lease=<branchName>:<sha> origin
HEAD:<branchName>`, and after each push use the SHA you just pushed: never a plain `git
push` or a bare `--force-with-lease`, and never recreate, force over or delete the branch.

An empty `git ls-remote`, a rejected lease or exit 5 or 6 from `tolkster pr checks`
ends the round, with nothing pushed; exit 7 doesn't stop the push. A round that
committed nothing ends on exit 5, answering that the PR merged. Otherwise post an
`elicitation`: the PR merged without your commits (5), closed unmerged (6), or the
branch moved under you. Then stop there.

## Rebasing

Run `git rebase origin/main`. Resolve its conflicts in a batch: list every conflicted file
and print all their hunks in one call (`git diff --name-only --diff-filter=U; git diff`),
resolve them all, `git add` them and `git rebase --continue`, once per commit that
conflicts, never once per hunk. If you can't, post an `elicitation` saying what
conflicts, and stop. Then run only the narrowest checks covering the files whose
conflicts you resolved, as your task brief's "While working" says: a clean rebase runs
none, and no rebase runs `npm ci` or a whole list.
