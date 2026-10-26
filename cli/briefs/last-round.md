# <identifier>: the last review round

This is the last review round: Tolkster sends Copilot's reviews on this PR to
<OWNER NAME> from now on. Run it as the round brief says, with three changes:

- Fix only blocking findings: a correctness bug on a path a real run takes, a
  security issue, a broken `AGENTS.md` / `CLAUDE.md` rule, or documentation wrong in a
  way that would lead a worker or the owner to do the wrong thing.
- File a follow-up for every other finding you would have fixed, in a thread or in
  the review's body (a body finding that fails the rubric gets its "Not worth an
  issue" line in your `response`). Decline what you would have declined anyway.
- Your `response` names what you filed and ends "Ready for your review.", since the
  PR's reviews now go to <OWNER NAME>. Only a round that filed nothing but `followUp`
  threads from an approving review ends as any round does: that approval stands.
