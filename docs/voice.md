# Tolkster's voice

How Tolkster sounds is in [`src/shared/voice.md`](../src/shared/voice.md); this file covers how phrases get posted.

## One vocabulary, two readers

[`src/shared/voice.json`](../src/shared/voice.json) holds the phrases for each **state**,
something Tolkster may be doing for a while, such as `reading-issue` or `rebasing`.
The Worker imports it and the CLI compiles it in, so both speak from the same lists, and
tests on each side check it. Nothing reads a phrase back, so phrases change freely; a state's name is in the briefs, so rename one only in step with `cli/briefs/`.

## Who posts what

- **Acknowledgements and pickups** (the Worker's and `listen`'s) are ephemeral thoughts, so
  what `listen` posts next replaces them. A new worker's pickup links its issue by
  identifier, never the worktree.
- **Workers** post no progress but the state that opens a round, naming its
  session for the hook. Their trail is the briefs' posts and the hook's durable line
  per plan step completed, else per phase, its commits and checks behind ▸. Its
  ephemeral action is the plan's step, else the phase, then the call's phrase; a test,
  build, push, rebase or sub-agent ticks and ends with an outcome. Only plan
  first, sub-issues or a size at `review.deepAt` or above get a plan (`brief::PLAN`).
- **A response ends a round**, and Linear stops the turn's timer there: a worker posts one
  only for its final summary or to answer a message; the next call's line reopens it.
  An `elicitation` or `error` stays silent.
  Every line is the worker's own, never Linear's or GitHub's (`posting.md`), but in a
  traced session (below).
- **A denied step** is never worked around: the worker asks with an `elicitation` in fixed
  words, naming the command by its first words, never the denial's text. A worker held at a
  permission prompt can't post, so the machine asks for it (the `heldAtPrompt` notice,
  [Events and the watcher](dispatcher.md#events-and-the-watcher)).
- **Settled messages** (answers, notices, errors) are fixed text in `cli/src/voice.rs`,
  `review.rs` or `review_pass.rs`: specific, never rotating, and promising a later start
  only when there really is a wait. One that asks for a reply is an elicitation, so the session awaits the owner.
- **Select options** on an elicitation are `voice.rs`'s fixed choices, which a worker
  names with `--option` and never writes. A pick arrives as the owner's prompt, its label
  or its value, each reading as the same reply.

## Tracing

A repo's `trace` (`listen`'s config.json, put on the launch line) traces the session a
worker is launched in: `hook after` posts the text since the last call as a durable thought,
then the call as a durable action with its outcome, a commit's named by its subject, in
place of its phrase, paired outcome and `Committed`. Ticks, the plan and the trail stay.
Text before the worker's own post goes at `hook start`, which the post waits on; nothing
goes after a response, question or error, or for `orca`. A call in any other session, and
any with trace off, posts as untraced. Hooks run apart, so calls made at once, or a hook
slower than the worker's next message, can land out of order, and a stop doesn't silence
one already running.

`hook/redact.rs` makes paths relative, `~` or their last part, and host names, held secrets
and secret shapes `<secret>`.

## Invariants

- A trace's text reaches Linear only through `hook/redact.rs`, and a call's output only as a count, but a failed check's tail, never for `tolkster` or `orca`.
- A trace posts only in the session the worker's launch names, never one the transcript names.
