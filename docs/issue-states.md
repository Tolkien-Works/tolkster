# Issue states

Tolkster moves a worker's issue through its team's workflow itself, so the state follows the work whether or not the worker remembers. The rules are in `cli/src/issue_state.rs`. A start or review resume moves it to started (a conversation's turn leaves it alone), an open PR to review, never back from a later or finished state, and a triage verdict out of Triage. A follow-up is filed in the first triage state, else the first backlog state, found by type, never by name.

## Whether an issue still wants work

The machine's decision says it ([The decision](state-machine.md#the-decision), rules 6 and 7): delegated to Tolkster by the owner, or by a chain that holds, and not completed or cancelled, from the issue's newest webhook or read, never from anything Tolkster said. It decides everything that starts, stops or resumes a worker:

- a delegation or review feedback starts or resumes a worker only while it wants work;
- a mention on an issue that wants none is a conversation;
- a running worker is stopped only for a cancel or an undelegation: one on a Done issue finishes its round.

Reaping asks only whether the issue is Tolkster's (`Session::is_owners`).

## Invariants

- Every guarded move (`guarded_update`: a worker's start and a triage verdict) reads the issue's state just before its update; the read that decided it is never reused.
- A team's states are parsed once, by `parse_states`, in position order; `doctor` and the rules above share it.
