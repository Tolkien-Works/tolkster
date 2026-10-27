# Budgets, lanes and trying again

GitHub and Linear each give the app an hourly rate limit that every worker and `listen` shares. One gateway (`cli/src/gateway.rs`) holds what is left of each, decides which calls may go now, and is the one policy for a call that fails. Costs are in [PRs opened as the agent](github-app.md#the-budget) and [Linear's API](linear-api.md#the-budget).

## The budgets

Responses to calls made at once land in any order, and a count believed for an hour at
most bounds a wild header (Invariants, below). A spent
answer (`Error::RateLimited`) holds its budgets for the wait it gave and no longer. 

## Lanes

Every call runs in its thread's lane (`gateway::in_lane`).

- **Urgent**, the default: a worker's command, a post, `pr create`, every command the
  machine sends to `listen`, and a terminal's end. It waits only on a spent budget. The
  machine's commands are urgent because one held keeps its issue's place for three
  timeouts and then ends `lost` ([Commands](state-machine.md#commands)); a terminal's end,
  because it starts the next triage batch and is acted on once, held or not.
- **Normal**: `listen`'s other jobs. A read also waits while a budget it spends is under
  20% left.  



## Failures and waits

`gateway::Failure` classifies every failure as `Spent`, `Transient` or `Refused` (never
retried), and `gateway::wait` gives the one wait. `listen` holds a local job until the
reset; a machine's command ends `retryable`, on a server or connection that failed too
(`Unavailable`, never a refusal or Orca's), and the machine sends it again. The relay's reconnect and resends are [Realtime](realtime.md)'s.

## Invariants

- Only the gateway decides that a call through it waits or repeats: no other module sleeps for a budget. `http::retried` alone repeats calls outside it.
- A call is admitted before it is made, and its response's count noted, at its API's one home (`github/app.rs` `send`, `linear.rs` `post_graphql`); only the app's own calls (`/app/`) and a token's revoke go unadmitted.
- A count noted late never raises a budget within its window, nor lifts a spent answer's hold; none is believed for more than an hour after it was first said.
- An urgent call never waits on a low budget, only a spent one; a normal read waits on a low one too.
- Every command the machine sends, and a terminal's end, runs urgent; only `listen`'s other jobs run normal.
