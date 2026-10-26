# The CLI's interface

Workers read what `tolkster` prints and how it exits, as the briefs in `cli/briefs/`
tell them: change either only in step with the briefs. `tolkster help` lists the commands.

## Exit codes

- **0** it worked. **1** it failed: Linear, GitHub or the Worker refused or couldn't be
  reached, or the command refused what it may not do, having written nothing.
- **2** a usage error a retry won't fix; every `tolkster pr` command exits 2 when the
  GitHub App isn't set up.
- **3** there was nothing to do: `issue unrelate` finding no relation, `issue chain`
  delegating none, `pr checks` finding no checks, `triage` on an issue that has its
  verdict, a second `comment --reply-to` answer in one reply's turn.
- **`pr checks`:** 0 all passed, 1 one failed (or, with nothing on stdout, it couldn't read
  them), 8 still pending, 4 CI couldn't run and nothing that ran failed, 5 the PR merged,
  6 it closed unmerged, 7 it conflicts with its base ([PRs opened as the agent](github-app.md)).
  It reads once: no worker waits on CI.
- **`pr auto-merge`:** 0 on, 1 refused or the app isn't set up, 8 with nothing on stdout
  when the budget or GitHub was still unavailable after retries: a later try may succeed,
  which a refusal's won't.
- **`land`:** 2 refused, 3 nothing to land, 7 conflicts ([Landing on main](landing.md)).
- **`doctor`:** 0 when no check fails, 1 when one does, 2 without `--team` or `--repo`.
  **`init`:** as its closing `doctor --team`; 2 without `--team` or,
  off a terminal, a flag it needs.

## The owner and sessions

- **The owner** is read from `TOLKSTER_OWNER_ID`, else `owner.id` in the config file. No
  flag overrides it: every security check keys on it.
- **Changes need a session.** `issue update|create|relate|unrelate|chain` and `comment`
  take `--session <id>`, the owner's session that asked, which the CLI reads back and
  refuses unless the owner started it; `pr feedback --session` checks it the same way.
  The owner's own agent at the primary checkout leaves it out, and the change is still
  made as Tolkster. That session-less path is refused (exit 1) wherever Tolkster's own
  agents run: in a worker's marked worktree ([Co-author](co-author.md)) and in a
  conversation's turn. It guards against a slip, not a security boundary (`Asker`,
  `commands/asker.rs`). `--delegate` and `issue chain` need `--session` (else exit 2).
- **A reply's turn names its thread** instead, `--thread <commentId>`, checked to be the
  turn's own and to hold the owner's reply by its request. `issue update` changes only
  the thread's issue (else exit 2); `issue create` files a sub-issue of it unless
  `--parent` names another or `none`, in Triage when the parent is delegated to Tolkster.
  It moves and relates (`issue create`'s relation flags too) no issue delegated to
  Tolkster, a parent or sub-issue of one, or one blocking either, so it never starts or
  halts code work.

## Output

The commands that print JSON keep their shapes. Errors go to stderr, one message, never
holding a token or a secret.

- `pr create` prints `number`, `url` and `existing`; `--session` adds `"linked"` and
  `"review"`, `--auto-merge` adds `"autoMerge"`: `"on"`, `"refused"` (as `pr auto-merge`'s
  exit 1) or `"unavailable"` (as its exit 8). It exits 0 once the PR is open.
  `--review light|deep` (and `--findings`) posts the review's verdict first.
- `land` prints `sha`, `url`, `pushed`, `done`, `linked`, `branchDeleted` and `reported`,
  exiting 0 once the commit is on the default branch.
- `pr review-pass` prints what [Pre-push review](pre-push-review.md#which-pass) decides.
  `pr feedback` and `pr checks` print what [PRs opened as the agent](github-app.md)
  describes.
- `issue show <ID>` is the one read of an issue a worker or a turn makes; `--comments`,
  `--children` and `--attachments` add a list beside `{"issue": ...}` (`[]` when empty,
  absent when not asked for), one page each. `--images` adds `images`: the upload URLs in
  the description and comments read, each `{"url", "path"}` (a temp file) or
  `{"url", "error"}`, at most 20 images of 10 MB. Several IDs print an array in their order,
  a failed one as `{"identifier", "error"}`, exiting 1. What `issue show`, `start` and
  `issue list` print is untrusted, and reaches only the worker's own terminal, never
  `listen`.
- `start <session>` prints its session's issue as `issue show --comments` does, with
  `"messages"` beside it. A failed half is left out (exit 1); the other prints, since
  `messages` may have reported its read.

## Other commands

- `messages <session>` from a worker prints the owner's unanswered messages it hasn't read
  yet ([`readThrough`](state.md#the-session-record)); from anywhere else, every unanswered
  one.
- `hook tool|start|state|after` are for a worker's Claude Code hooks alone ([Workers](dispatcher.md#workers)).
  Each exits 0 whatever becomes of it, a usage error included, which Claude Code would
  take as the hook blocking the agent.
- `timing` reads only `listen`'s timing file on its host ([Timing](timing.md#reports)), and
  exits 3 when no worker started in the window.
- `hosts` is read-only: this machine (`name` null) and each paired host, asked afresh, with
  `answers`, `cap` and `workers`, the issues the Durable Object holds there.
- `repo map`, `repo unmap` and `init`, the owner's alone, write `repos` through the one
  writer of the settings files, merged into the file as it is then; the same `repo` run
  again exits 3.
- `doctor` checks that a team and a repo are ready for delegation ([Adding a
  team or repo](setup.md#adding-a-team-or-repo)); a `warn` is something that works without
  it, a `fail` is not. A reason names only what Tolkster already holds (keys, IDs, repo
  names, an error from Orca or GitHub), never Linear text or a secret.

## Invariants

- `issue update` and `issue create` resolve every name (state, labels, project, a relation's
  issue) and check an estimate against the team's scale before they record a delegation or
  write to Linear, so a wrong one leaves nothing behind; `triage` checks the estimate
  likewise.
- `doctor` only reads, and prints no Linear text.
- A module imports only from its own layer or lower, in the order
  `cli/src/brief/tests/layers.rs` holds beside the upward imports it still allows, a list
  that only shrinks, and no two in one layer reach each other. `commands/` is the top
  layer, which no production file but `main.rs` names: a command parses and prints, and
  what `listen` runs too lives in the core.
- What `listen` keeps between commands is one context, `runtime::Shared`, that `Runtime`
  and the ports carry, so each test builds its own. A static or thread-local says why it is
  the process's or a thread's; a lock is taken through `sync::lock`.
- Every command but `host join` reads both files through `config::check`: an unknown key, a
  group that isn't an object or a setting left in credentials.json exits 2 before it acts.
- `init` takes every answer before it writes, and merges `repos` before it creates a label,
  so a missing answer or a refused edit leaves nothing behind.
- Linear's token goes only to its API and, for an upload URL in the issue's own text,
  `https://uploads.linear.app/`, never across a redirect.
- A command's `commands::COMMANDS` entry is its dispatch, a brief line's check and its help,
  which its usage errors end with and `--help` prints, touching nothing.
- A body, from `--body` or a body file, is read through `args::body` or `args::file_body`:
  a blank one exits 2 before anything is posted or written.
- A worker's worktree card's comment is set by `pr create` and a
  durable `activity` action, which never replaces a PR's checkpoint; its column follows its
  issue: in review at `pr create`, in progress at a review round. Only for the worker its
  launch names (`card::launched`), after the command's answer is printed, and a failed read
  or Orca call never fails the command.
- `comment --reply-to` in a reply's turn posts only with its request's comment ID
  (`post::answer_key`), which Linear refuses a second time, so the request gets one answer.
- Answers from Linear, GitHub and Orca decode into their shape where read: one lacking a
  needed field fails, naming the call. A failed read, write, run or decode of anything else
  names its target or step; an I/O failure keeps its source (`error::Context`).
