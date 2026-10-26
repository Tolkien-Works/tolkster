# Worker hosts

A worker host holds one credential of its own, `hostCredential` in its
`~/.tolkster/credentials.json`, and none of the shared secrets: no Linear client secret,
no realtime secret, no Claude login. The `listen` host, where conversation turns run,
keeps its own credentials and the realtime secret.

- **`tolkster host add`**, on the `listen` host, sets a paired server up in terminals
  in a repo already set up there: Orca opens remote terminals only in worktrees. Each step
  checks first: git running hooks from config, Claude Code and auto mode (any failing stops
  it), `tolkster`, the co-author its settings credit, the owner's git author identity
  (this machine's global `user.name` and `user.email`, set globally on the host), then each
  mapped repo, cloned with the app's token. Last it admits the
  credential, so the host takes work; `--rejoin` joins again regardless.
  `tolkster host rm` revokes the credential.
- **`host add <name> --recipe <id>`** makes the machine first, with no prompt. Once
  `orca vm recipe doctor` passes the recipe in the one Orca repo holding it (`--repo` picks
  between several), it runs `create`, records the recipe, its
  `destroy` line and `userData` in `~/.tolkster/machines.json`, pairs
  Orca with the printed code, sets the image's checkout up in Orca and runs the steps above
  for that repo alone, fetching it through the host's credential. A failure after `create`,
  and `host rm`, run that line, then unpair it. `listen` makes one so off
  its job lanes for the fleet's `makeHost`, naming it `<repo>-<n>`.
- **Sleep and wake.** A recipe's `suspend` and `resume`, when it has both, are recorded
  beside `destroy`. A machine no agent works on for its repo's `idleMinutes`, a worker waiting
  on the owner's included, sleeps once nothing holds it awake: the Durable Object keeps the
  clock ([Fleet](state-machine.md#fleet)) and sends `suspendHost`; `listen` checks again, marks
  it `asleep`, then runs `suspend`. Its worktrees keep their IDs. A worker's command there is
  answered `waking` and the fleet sends `wakeHost`, as for its repo's next worker no host takes
  or a resume of one on it: `resume`, awake once Orca hears it, asked every 10 s. One marked
  awake but silent is woken likewise, once in 30 minutes. A machine its platform wakes by itself starts Orca's server as it boots;
  a wake that hears it skips `resume`.
  The `agent37` recipe logs a fixed line on `create`, `suspend` and `resume`; it survives a sleep.
  The panel lists an asleep machine as `asleep`, never as needing the owner.
- **Destroying.** The fleet sends `destroyHost` for a machine asleep its repo's `destroyAfter`
  minutes (a day by default), awake and empty that long when it can't sleep or isn't
  admitted, or a mark no make is behind for an hour. With no make, wake or sleep running, `listen` wakes
  it, placement stopped there, and reaps its worktrees by their rules' guards
  ([Reaping](dispatcher.md#reaping)). A worktree kept puts it back to sleep; else `host rm`'s
  steps run.
- **Joining.** `host add` mints an enrolment code carrying the owner's settings and types
  one line on the host: fetch `tolkster` from the Worker, then `tolkster host join <code>`,
  which trades it at `POST /enrol` for the credential, kept with `host`, its environment ID,
  in `credentials.json`, and the owner's `config.json`. Joining again replaces the credential.
- **A recipe machine's layers.** It boots from the public `tolkster-worker-base` image (git and
  Orca's Linux app, rebuilt on its releases), under the repo's layer (toolchains and
  caches, warmed on the recipe's base). `create` and `resume` rewrite its boot script from
  the repo; the image only runs it. Claude Code's terms keep it out of the image; the base
  installs it unless the platform ships one. `tolkster` and every credential arrive
  after the machine starts, so no layer holds one for a fork or pulled image to hand on.
- **Rolling out.** A recipe's optional `prepare`, run by `listen` one at a time as it starts
  and after a pull that restarts nothing, brings its image and base up to the repo, so a
  merge is the rollout. The `agent37` recipe's rebuilds the template when `agent37/` changes
  and updates the stopped base to it in place, keeping its disk, or builds a new base when
  `agent37-base.ts` changes. A machine's `userData` holds its template revision, which
  `listen` reports to the fleet.
- **Builds.** `GET /build/<target>/<commit>` serves a host the `tolkster` CI kept for that
  commit ([Set it up](setup.md#2-the-cli)), a new host's first by its untraded enrolment
  code. It serves no "newest". The Worker reads the artifact as
  the GitHub App, only a `main` build of that commit, never a release, which anyone who can
  push can edit. `listen` adds its own commit to each
  `tolkster worker` line (`--build`); a host on another downloads that build beside its
  binary, renames it over (a Windows host renames its running exe aside first) and restarts
  the worker on it. No build of that commit, or a failed swap, runs the host's own. A `listen`
  without `develop` swaps likewise ([Keeping up with main](realtime.md#keeping-up-with-main)).
- **`/state` as a host.** The credential is the bearer, checked by the Durable Object in the
  same call as the op, so a revoked host's next call is refused. It never opens `/realtime`.
  It reaches only the workers whose worktree ID starts `<host>@`, their sessions and chains
  too; it reads others' sessions and delegations as none and is refused anything else with 403.
- **`/token`** trades the credential for the Linear app token the Worker acts with, which
  the host caches in `token.json` as one it minted; for a GitHub App token for one repo the
  app is installed on and one purpose ([PRs opened as the agent](github-app.md)), as the
  `listen` host does with the realtime secret, but never a branch's update, `listen`'s
  alone; or for the owner's Claude token (`CLAUDE_TOKEN`, made with `claude
  setup-token`), which `tolkster worker` puts in `CLAUDE_CODE_OAUTH_TOKEN` in its `claude`'s
  environment. Revoking it means a new `claude setup-token` and secret, which each host
  picks up at its next launch. `tolkster doctor` names a secret the Worker lacks.

## Invariants

- A build goes only to a live credential, the realtime secret or an untraded enrolment code (a
  download doesn't use it up), and only a `main` build of this repo at the commit asked for.
- A swap happens only after the unpacked build says it is the commit asked for. A host
  without a credential, or already on that commit, makes no worker request.
- `listen` knows the admitted hosts before it runs a command, or stops.
- A paired host takes no worker until `host add` admits its credential, checked afresh
  before a start there: Orca lists it on pairing, before its setup passes.
- An enrolment code trades once, within 15 minutes; a new code or a revoke deletes the host's
  others. The Durable Object keeps only its digest and the settings sealed under a key
  derived from it.
- `host add` types no secret on a host but the code, and no Linear or GitHub text.
- A recipe's pairing code goes only to `orca environment add`. A machine is paired only once
  `machines.json` records it, another run never replaces its entry, and it leaves once
  `destroy` succeeds.
- `listen` makes no machine past its repo's `machines`, counting those being made and asleep.
- No grant passes a host's cap, and no start goes to a machine a sleep or destroy has checked
  (`Pacing::acting`).
- A destroy takes no work: a machine goes only once its complete listing shows nothing on it
  but its checkouts and no worker being set up; one asleep and unpaired, once no record names
  it; a mark, with no make running in `listen` and an hour at least, past any `host add`.
- A sleep or wake never pairs or unpairs a machine: only `create` and `destroy` do.
- A rollout always leaves a base to fork: the old one goes a minute after its replacement is ready.
- An Agent37 machine runs one Orca server: a boot and a `resume` start it one at a time,
  each once the last has exited, and a boot script replaced since its run began starts none.
- The Durable Object stores no credential, only its digest, and a storage reset keeps the
  digests.
- No host holds the GitHub App's key: it is a Worker secret.
- A host's `/state` call touches only its own workers; a new op is closed to it until `scope.ts` lists it.
- A revoked credential gets no new token and no `/state` call. A Linear token already handed
  out lasts until it expires: 30 days, which Linear can't shorten; a GitHub one, an hour.
- A host is never handed the Linear token it reports refused, nor one with two days or less
  left: twice the margin within which it treats one as expired.
- `tolkster` never types the Claude token into a terminal, puts it on a command line or
  writes it to disk, and no token appears in its output, logs or errors. What the worker's
  `claude` runs inherits its environment.
