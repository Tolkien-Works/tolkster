# Plan first and reconcile first

A worker asks the owner only on genuine ambiguity, a real choice between approaches or unclear scope, and otherwise decides and says why, whatever the size. Two gates make a new work worker ask before changing anything: a `plan-first` label ([plan first](#plan-first)), and code that moved against the issue ([reconcile first](#reconcile-first)). Both get one elicitation; a conversation's turn takes neither.

## Plan first

`startWorker` decides in code (`executor::plans_first`), by the label alone. The worker posts its plan and an `elicitation`, then
exits. Until the go it changes nothing: `listen` holds back its push. Only the owner's messages resume it: a clear approval starts the work, anything else revises the plan.

## Reconcile first

Commits landed on the default branch since the baseline: the later of the issue's
creation and its last description edit (`setup::baseline`). If the history can't be
read, or the edit is older than its newest 100 entries, it is the creation, so the worker
checks more commits, never fewer. The brief gets only times, SHAs and a count. A rename, a
move or part already done it adapts to and says so; it waits for the go, as a plan-first
worker does, only when a later change took another approach, the issue contradicts the
code, or all of it is done. Which applies
is its call: it gates nothing that matters to security, the owner confirms every pause, and
the worker never edits the description.
