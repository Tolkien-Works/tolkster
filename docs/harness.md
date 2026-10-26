# The agent harness

A worker's agent, and a conversation's turn, run on a harness: the program `tolkster
worker` starts in the worker's terminal once its host has set it up, or `tolkster converse`
in a turn's. A worker's rung names its harness ([Model and
effort](dispatcher.md#model-and-effort)), and its record keeps it; a harness names its
program, flags and effort levels. Claude Code is the one harness. `harnesses.rs` holds those,
which config.json's rungs are checked against, the variable the owner's token goes in
([Worker hosts](hosts.md)) and `host add`'s steps. `harness.rs` launches it: the MCP and
settings flags, the settings files beside the briefs, the worktree's trust, the files it
leaves in a worktree that `reap` ignores, its event adapter (`harness/events.rs`) and its
conversation adapter (`harness/turn.rs`).

## Events

The adapter reads Claude's hook input and transcript into neutral calls, ends and agent
states, all the hooks read of the harness ([Workers](dispatcher.md#workers)). A harness
keeps a transcript of its tool calls and their ends, and runs a `tolkster hook` command on
each event:

- **A shell call or sub-agent's start**, waited on (`hook start`), and **every tool call's
  start** (`hook tool`). Without them, no progress line, trail line or plan move is posted.
- **Every tool call's end** (`hook after`). Without it, no `Committed`, outcome or trace.
- **A prompt submitted, a stop, a permission prompt or question, and the session's end**
  (`hook state`); a stop while a background call has no notice isn't idle. Without one, the work machine learns that state from Orca's listing, up to a minute later.

## Turns

The conversation adapter runs a turn's agent headless and keeps it warm
([Turns](dispatcher.md#work-or-conversation)). A harness offers:

- **A resumable session under an ID Tolkster chooses**: a session's recorded one, or a
  thread's root comment's. It resumes once saved, so a turn that died first starts again.
- **Streamed input**: the prompt, then each next request, on an input open until the turn ends.
- **A done signal per message**, counted to the turn's idle (`turnIdle`).

## Invariants

- A launch line holds only values from code or configuration, its model a plain name and
  its effort its harness's, so a record read back can't add a flag.
- A worker resumes on the harness it started on, even when its rung is picked again.
- A worktree is trusted before its agent starts. A failed trust, or a session that drops it
  from Claude's file meanwhile, leaves the worker at the dialog, which the wait for a
  launched worker to be seen working catches.
- A harness runs unattended only if it blocks risky actions with no person (its `host add`
  check, which lists no host failing it), is trusted before it starts, and starts no MCP
  servers, which would act as the owner. Claude's settings do the last two, another
  harness's own flags. A launch or rung naming one short of that is refused, and `doctor`
  reports it.
- The event adapter reads only a call's tool kind, a shell call's command, a file, a result's flags
  and a notice's status, and what it can't read is no call, end or state.
- A turn is idle only once every message written to it has its done signal.
