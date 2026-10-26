## Sub-issues

<identifier> has sub-issues, so you work through them rather than doing them yourself:
each runs as an issue of its own, delegated to you, with its own session, worker, branch
and PR, side by side unless one depends on another. This section overrides "Before any
work" and "Finishing", except that each part ends as Finishing's last step does:
print `<identifier> DONE` and close your terminal, in one command. Which part applies depends on the
line that started or resumed you: one that says every sub-issue has finished is the
second.

**While any sub-issue is unfinished**, you only set the sub-issues going. Don't push
the branch, commit, or open a PR.

1. Read <identifier> and its sub-issues with `tolkster issue show <identifier>
   --children --comments`, and the unfinished ones you need in one `tolkster issue show
   <sub-issue>… --comments`. Each sub-issue comes with its `relations`,
   so you can see which blocking relations are already there.
2. Decide which unfinished sub-issues must wait for another, counting the blocking
   relations already there. Block one on another only for a genuine dependency: it
   can't proceed without something the other builds, or doing them in the wrong order
   would mean a redesign, major rework or conflicts too big for a routine rebase.
   Shared files, docs and other small overlaps are no reason: each worker rebases
   onto the default branch before it opens or updates its PR. Post the plan as a
   `thought` on this session: one line naming the sub-issues that start together,
   then one line for each block you add with the dependency that justifies it, e.g.
   "Together: TW-10, TW-12." and "TW-11 after TW-10: it calls the command TW-10
   adds.", then carry straight on. If this brief has a "Plan first" section, this is
   your plan instead: post it as that section says, wait for <OWNER NAME>'s go, then
   carry on here.
3. Set them going in one call, the one your first line names: `tolkster issue chain
   <identifier> --session <sessionId>`, with `--block <sub-issue>:<blocker>` for each
   block, e.g. `--block TW-11:TW-10`. It adds those blocks, keeps the blocking relations
   already there, and delegates the unfinished sub-issues to you, the unblocked ones
   first. It prints what it `added`, `delegated` and `skipped`: those it leaves alone,
   because another delegate has them, they are in Triage or wait on one in Triage, or
   they weren't taken into the chain. It exits 3 when it delegated none. It exits 1 and
   writes nothing when an ID isn't an unfinished sub-issue of <identifier>, a block names
   one in Triage, or a block would close a loop, where every sub-issue on it would wait
   forever: drop that block, or, for a loop already there or anything else, say so in
   step 4 rather than work around it. Any other failure may have left part done: run the
   same call once more, which adds nothing twice. The unblocked ones start as slots free
   up, and each blocked one once its last blocker finishes and a slot is free.
4. Post a `response` naming which sub-issues start together, and each block and its
   reason, and anything the call refused. Then end as above. Tolkster resumes you once
   every sub-issue has finished, those in Triage aside.

If <identifier> is delegated back to you after it was taken back, its sub-issues'
delegations were taken back with it: do steps 1 to 4 again for those still unfinished.
When a line says some sub-issues aren't chained yet, do steps 1 to 4 for the new ones.

**Once you're told every sub-issue has finished**:

1. Read <identifier> and its sub-issues again with `tolkster issue show <identifier>
   --children --comments`, and each sub-issue's PR from its `attachments`.
2. Decide whether <identifier>'s own description asks for anything its sub-issues
   didn't do. A parent often has no work of its own.
3. No work of its own: run `tolkster issue update <identifier> --session <sessionId>
   --state Done`, then post a `response` that sums up what each sub-issue shipped,
   with links to their PRs. Then end as above.
4. Work of its own: do it as a work worker does, from "Before any work" through
   "Finishing", and put the summary of the sub-issues at the top of your final
   `response`.
