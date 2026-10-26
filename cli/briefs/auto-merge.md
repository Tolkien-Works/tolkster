2. **Auto-merge**, so GitHub merges the PR by itself once the checks pass, every
   review thread is resolved and any approval its ruleset requires is in: `pr create`'s
   `--auto-merge` turned it on as the app right after opening the PR, waiting out a
   spent GitHub budget itself. Never merge it any other way. Go by its JSON's `autoMerge`:
   - **`on`**: it is on.
   - **`refused`**, GitHub refused (the repo doesn't allow auto-merge, the PR isn't
     open): carry on without it, and say so in the final `response`. It never blocks the work.
   - **`unavailable`**, the budget was still spent, or GitHub still unavailable, after
     its retries: run the same `tolkster pr create` once more, which finds the PR. If
     `autoMerge` still isn't `on`, post an `elicitation` saying auto-merge couldn't be
     turned on, and stop there, before step 3. Never finish as though it had been
     refused.
