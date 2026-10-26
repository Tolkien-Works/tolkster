# Context for related issues

When an issue Tolkster worked on is completed, the open issues related to it, and the other open sub-issues of its parent, get a comment where one is needed. Each has at most one such comment of Tolkster's, updated in place by each later completion, and no description is rewritten. It complements [Reconcile first](plan-first.md#reconcile-first).

- **When.** `startContext` is queued once per completion the Worker saw, never a cancel. `listen` checks in code
  (`context::start`) that the issue is still completed, that a related issue or sibling is
  still open, and that the issue is Tolkster's (delegated to it, or with a session the owner
  started): nobody else's closed issue starts anything. It starts once per issue, under the
  claim `context-<TW-n>`, so a redelivered or reopened issue never starts a second.
- **The context session** is Claude in the repo's primary checkout: it changes no code, takes no slot and leaves nothing to reap. It writes each comment as the whole picture as of the default branch's tip, folding in its earlier one.
- **Posting.** `tolkster comment <TW-123> --closed <TW-n> --seen <time>` checks again, in
  code, that `TW-n` is completed and Tolkster's and `TW-123` is related to it or a sibling
  and still open. Under the claim `comment-<issueId>` it then creates the comment, updates
  the one the issue's record ([Other records](state.md#other-records)) names, or replaces it
  if gone. `--seen` is when the session read the issue: a record written after it makes the command exit 3, so of two sessions the later never writes over the earlier's.

Whether an issue needs a comment, and what it says, is the session's call; the code decides where a comment may go and how many.

## Invariants

- An identifier that is not an `IssueKey` (`plain.rs`) is refused where it is parsed, so none reaches a brief, a claim name or a path unchecked.
- The merge SHA in a brief is a `Sha` read from git's log of the default branch's tip, never from Linear or GitHub text, and only from a commit whose subject ends `(<tag>) (#<n>)` or, landed, `(<tag>)`.
- The comment record holds two IDs and a time, never text. It is written before each create or update, a create with the record's ID, so a create whose answer was lost is found, never posted twice; a failed read is never taken for a deleted comment.
- The record is read, compared with `--seen` and written under the `comment-<issueId>` claim, only while at least 2 of its 5 minutes are left by the run's own clock from before the take. Its time is taken again once the comment has landed, so a session that read the issue meanwhile is refused.
