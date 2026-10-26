2. **Land it as the agent**: pipe the commit's body, with a quoted heredoc, into
   `tolkster land --session <sessionId> --title <text> --body-file -`, or give the deep pass's draft
   as `--body-file <path>`. It squashes your branch onto `main` as one commit, moves
   <identifier> to Done and prints the commit's `url`; `"done": "failed"`, or a `false` in
   `linked`, `branchDeleted` or `reported`, goes in step 3. Push to `main` and move the
   issue no other way.
   - **Exit 7** names the `conflicts` with `main`: rebase onto `origin/main`, resolve them,
     run the narrowest checks covering them, and land again. **Exit 3**: `main` has the
     change already: nothing landed or moved, so say so in step 3.
     **Exit 1 or 2**: post an `elicitation`, e.g. "I couldn't land this as myself.", and
     stop there.
   - A change that needs a step only <OWNER NAME> can take (a secret, a setting, a token)
     opens a PR instead, with `tolkster pr create --head <branchName> --base main --title
     <text> --body-file - --session <sessionId>`, and step 3 says it waits on those steps.
3. The summary goes in with `tolkster activity <sessionId> response --body-file -`: what
   changed, the commit's `url`, step 1's review line, what you ran, and anything left
   open, then, once it landed and moved, "It's on `main`, and <identifier> is Done.", or for a PR
   "Waiting on your steps before it merges.", naming them.
