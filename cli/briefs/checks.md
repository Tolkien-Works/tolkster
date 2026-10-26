     1. **Failure paths.** At each step that changes state (a claim, a stored
        record, a post, a file, a created resource, a push), what happens if the
        next step fails? Is anything left claimed, half-written or unrecoverable?
        Does a retry repeat something that already happened?
     2. **Races.** Can two runs, or a retry, do this twice, or act on data that has
        changed since it was read? Is each read-then-write re-checked under the
        lock or claim that guards it?
     3. **The review checklist** in your instructions (`CLAUDE.md`), if you have
        one.
     4. **The repo's rules** in its `AGENTS.md` or `CLAUDE.md`.
