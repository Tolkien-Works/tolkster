  - **Open the PR as the agent**, so <OWNER NAME> can approve it: push, then pipe the
    PR's body, with a quoted heredoc, into `tolkster pr create --head <branchName>
    --base main --title <text> --body-file - --session <sessionId><AUTO MERGE>`, or
    for a body the deep pass drafted, or of thousands of characters, `--body-file <path>`, written as "While working" says. It prints the
    PR's `number` and `url` as JSON, links the PR on this session and moves <identifier>
    to review. `"linked": false` or `"review": "failed"` means the PR is still open: carry
    on, and say which in the final `response`. Don't post comments, attach links or move
    the issue any other way, which would appear as <OWNER NAME>; everything on GitHub but
    pushes goes through the app with `tolkster pr`. A branch that already has an open PR
    gets it printed with `"existing": true`: use it. A PR that needs a step only
    <OWNER NAME> can take before it merges (a secret, a setting, a token) goes without
    `--auto-merge`: skip step 2, and step 3 says it waits on those steps.
  - If it fails, post an `elicitation`, e.g. "I couldn't open the PR as myself.", and
    stop there.
<MERGE>
3. The summary goes in with `tolkster activity <sessionId> response --body-file -`:
   what changed, the PR, step 1's review line, and anything left open. CI hasn't finished: say what you ran
   locally, never that the checks passed. Don't wait for them: a failed required check, a
   conflict or a review brings you back. Then say what happens next:
   "It merges by itself once the checks pass and GitHub's rules are met." when
   auto-merge is on, "Waiting on your steps before it merges.", naming them, when you
   left it off for them, or "Ready for your review." otherwise (with why it's off).
   Don't ask Copilot for a review: GitHub's ruleset decides that.
