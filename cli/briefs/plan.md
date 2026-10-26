Linear shows it as a checklist, and the hook posts a durable line for each step you
  complete. Post it at "The task" step 4, beside the task thought, before any code
  change: a step per slice of work that ends in a commit, then the gate and review, then
  "PR open". Pipe it in as a JSON array, with a quoted heredoc (`<<'EOF'`, or a
  PowerShell `@'` here-string) into `tolkster plan <sessionId> --file -`: `[{"content":
  "Read the issue and plan", "status": "completed"}, {"content": "...", "status":
  "inProgress", "until": "commit"}, ...]`. A status is `pending`, `inProgress` (one step
  only), `completed` or `canceled`; an open step's `until` is what ends it, `commit`,
  `review` or `pr`. The hook moves the plan on, and your `response` once the PR is open
  completes it. Send it whole again only when it changes.
