# Triage

Nearly everything in Triage is Tolkster's own: follow-ups workers file from review threads
(see [Review feedback](review-feedback.md)). Filed against moving code, many are soon
fixed, obsolete, duplicated or never worth doing. A **triage session** reads each against today's default branch and posts one verdict through
`tolkster triage`, which checks in code everything that matters (`cli/src/triage.rs`,
`cli/src/commands/triage.rs`). The brief is `cli/briefs/triage.md`, the rubric
`cli/briefs/rubric.md`.

## What starts a session

Decided in code, never by an agent, by [the triage machine](state-machine.md#triage), which
holds the one session it started:

- **A create in Triage, or a session's end.** Every create in a triage state reaches the
  machine as `triageAsked`, whoever made it. The held session's terminal closing starts the
  next batch, so the queue is worked through one batch after another. A failed start is sent again with backoff.
- **The batch** (`start_triage` in `command.rs`) takes the owner's and Tolkster's issues
  that aren't archived, the asked first and then the oldest, up to `triage.batch` (default
  5, at most 50), in the repo of the first with no verdict.
- **Not an issue moved into Triage.** One that entered Triage after it was created is how
  the owner parks an issue, so no batch takes it, whoever created it. An unreadable history
  passes the issue by, and `tolkster triage` reads it again, so an issue parked while its
  session ran is refused.
- **Once per issue.** What marks an issue triaged is its verdict claim, not a label, so a
  declined issue, which stays in Triage, isn't taken again until the claim is pruned (30
  days). An issue whose session ended without a verdict is taken by the next batch.
- **A start sent again after `listen` restarted** may open a second session, whose verdicts
  are refused: the machine holds only the session whose result landed.

## The session

Claude in the repo's primary checkout: it changes no code, takes no slot and leaves nothing
to reap. Its brief names only identifiers, SHAs and the session's id, the token the machine
holds it by. The code finds the squash merge of each issue a taken one is related to (the issue
it was filed from), and the launch line has the session fetch and read them in its first
turn. It decides whether the issue still holds and whether another open issue covers it (it may
ask Jev), judges it against the rubric, and posts every verdict in one turn, each comment piped
in on stdin. It comments only on the issue it triages.

## The verdicts

`tolkster triage <TW-n> --session <id> --verdict accept|fixed|duplicate|decline`. `accept`
moves the issue to the team's first backlog state with an estimate, a priority and
relations; `fixed` and `duplicate` cancel it; `decline` leaves it in Triage with the
reasoning. Only the owner closes an issue for not being worth doing, so what is left in
Triage with a triage comment is the owner's review list. 

## The command is the gate

`tolkster triage` reads the issue fresh and refuses one that isn't the owner's or Tolkster's
in a triage state, or was moved into Triage; a session the machine doesn't hold as running,
or that wasn't given the issue; a relation to itself, another team or a finished issue; a
`--by` that isn't a SHA on `origin/HEAD`; and a team without the state the verdict needs.
The move goes only from Triage to a backlog or cancelled state. It isn't in a conversation turn's allowlist.

**One verdict per issue.** The comment is posted under the claim `verdict-<TW-n>`, held for
good through `holding`: a second run exits 3, and a comment that wasn't posted gives the
claim back. The relations and the move come after the comment, which names its verdict, so
if one fails the owner can finish by hand. The state is read again just before the move, so
an issue delegated meanwhile is left where it is.

Which verdict, and what it says, is the session's call; the code decides which issues are
triaged, where they move and how many verdicts each gets.

## Invariants

- A candidate whose identifier is not an `IssueKey` (`plain.rs`) is refused where it is parsed.
- The cap counts claims taken of issues with no verdict, not candidates read, and the repo is the first such claim's: a declined issue at the head of the queue never starves another repo's.
- A start probes a verdict claim only while the machine holds no session as running.
- Whether an issue was filed in Triage is read from its state history, as a batch reaches it and again by the verdict, never from times.
- A history read covers at most `HISTORIES_PER_READ` candidates and fails whole, so a failed window is read again for its first candidate alone, and only a spent budget ends the reads.
- The merge SHA in a brief is a `Sha` read from git's log of the default branch's tip, never from Linear or GitHub text.
- A verdict is posted only by the session the machine held as running when `tolkster triage` checked, on an issue that session was given.
