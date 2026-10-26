# The Worker's logs and traces

The Worker sends Workers Logs and traces for every invocation (`observability` in
`wrangler.jsonc`, sampled at 1), kept about 7 days. Traces, a span per fetch and storage
operation, are in the dashboard's **Observability** tab. Logs
are one object per line (`src/worker/log.ts`), each field indexed; the events and their
fields are in the code that logs them.

## Reading the logs

`tolkster logs --session <id>` or `--issue <TW-123>`, with `--since` and `--until` an instant
or `6h`, prints the matching lines as JSON, oldest first; an issue's bring its sessions'.
`more`: a query hit its cap; narrow the window. wrangler's
login can't read logs, so it takes the owner's own token:

1. In the Cloudflare dashboard, **My Profile → API Tokens**, create a custom token with only
   **Account → Workers Observability → Read**, on the Worker's account.
2. Put it in `~/.tolkster/credentials.json` as `cloudflareLogsToken`, and the account's ID as
   `cloudflareAccountId`.

## Following one delegation

1. `request` on `/webhooks/linear` with `decision: "start"`.
2. `activity_posted`: the acknowledgement, and how long Linear took.
3. `relay_ingested`, or `relay_failed` per try, then `event_received` for its
   `sessionStarted`, with its `outcome`; `session_unanswered` says why none was answered.
4. `command_sent` per command (`sent: false` waits for `tolkster listen`), and
   `report_received` per result.

## Invariants

- A log field is an ID, identifier, enum, count or duration, never issue, comment, review or PR text, people's details, tokens or secrets, and never a worktree ID, which holds the workstation's paths.
- `tolkster logs` prints a line's scalar fields only; its token goes to Cloudflare's API alone.
