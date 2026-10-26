# Linear's API

## The budget

The CLI, `listen` and the Worker all spend one app's rate limit, which Linear counts per hour in two budgets: requests, and the complexity of each query. 

- **Each call is logged** under a fixed call site, from Linear's rate-limit headers
  (`cli/src/linear/cost.rs`; the Worker logs a `linear_call` event). A worker's command prints its lines only with `TOLKSTER_LOG_LINEAR` set, so its share shows in `listen`'s log as the fall in `requests_remaining`.
- **A worker's hook** (see [Workers](dispatcher.md#workers)) posts per call it
  describes, and every 3 minutes of a long one, each a request of the budget; a traced
  call's thought and action go in one.
- **A spent budget is its own error** (`Error::RateLimited`, the Worker's
  `RateLimitedError`), and the gateway holds both budgets until its reset ([Budgets, lanes
  and trying again](budget.md)). A job that gets it is held (`Acted::Held`, [Realtime](realtime.md#trying-again)) unless the code wraps it in a `Failed` of its own, which its command's result answers.
- **A release** costs a read and a `releaseSync` per rebuild put in place ([Keeping up with
  main](realtime.md#keeping-up-with-main)); `setup`'s pipeline step a read and at most one
  `releasePipelineCreate`.
- **A check reads only what it decides on.** The owner check (`asker::trusted_session`) and
  the issue's size read no activities; the answer check adds the newest prompts, the
  sub-issues and the newest sessions. None reads blockers, labels, the project or the text
  of comments and activities.
- **A session's activities are read as its prompts alone**, filtered by Linear in the
  session's own query: Linear goes on listing an ephemeral thought after the next activity
  replaces it, but none is a prompt, so none takes a place in the page. A full page means older prompts are unread.

## Invariants

- A Linear call's log line holds fixed names, numbers and times only: never issue, comment or session text, nor an ID from a response.
- Only `RATELIMITED` or a 429 is a spent budget: no other error is held.
- A slim read parses as the full read does, and decides the owner check and the answer check the same: a field either reads is in its slim query.
- The owner's prompts are read from Linear, filtered to prompts in the session's own read; nothing reads a further page of a session's activities.
