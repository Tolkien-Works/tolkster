## The rubric

Whether a finding is worth an issue is judged on these, together:

- **Does a real run reach this path?** A window no workstation reaches, or one that
  needs two rare events at once, is weak.
- **What does it cost when it happens?** A duplicated message, or one missing until the
  session's next event, is cheap. Lost work, a wrong merge, a leaked secret, or a worker acting
  on the wrong issue is not.
- **How big is the fix compared with that cost?** A one-line re-check for a cheap window
  isn't worth an issue; a day's refactor for one is worse.
- **Is it already inside a planned or in-flight issue** (an audit's sub-issues, a
  refactor that removes the path)? Then it is a comment there, not an issue.
- **Security findings and broken `AGENTS.md` / `CLAUDE.md` rules are always worth
  doing.**
