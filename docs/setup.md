# Set it up from scratch

For the owner: install the CLI, then run `tolkster setup`, which does the rest in order and
prints what only you can do. The placeholders: `<AGENT>`, the agent's
display name; `<WORKER_URL>`, the Worker's address; `<GH_LOGIN>`, your GitHub login;
`<CHECKOUT>`, a checkout of this repo.

## 0. Prerequisites

- **A workstation** (Windows, macOS or Linux) that stays on to run workers, with **Orca** running and the
  `orca` CLI on PATH.
- **Claude Code**, signed in and **unattended**: turn on auto mode (`/config`) and accept
  its first prompt once. Your `~/.claude/CLAUDE.md` applies to every worker.
- **git** that runs hooks from config (`git hook list pre-commit` works) and **Node 24**.
  Tolkster needs no `gh`.
- **A Cloudflare account**, and **a Linear workspace admin**.

## 1. Names

`<AGENT>` is the name people see, on the Linear app and in the co-author line. The Worker's
name, `"name"` in `wrangler.jsonc`, only shapes `<WORKER_URL>`. The internal name `tolkster` (the binary, `~/.tolkster/`,
`.githooks`) shows nowhere: leave it.

## 2. The CLI

Download main's latest release, `<repo URL>/releases/latest/download/tolkster-<target>.tar.gz`
for `linux-x64`, `macos-arm64` or `windows-x64`, and unpack it onto PATH. A private repo's
release needs your GitHub login and holds Linux x64 alone ([the others](#a-second-workstation)). A build
downloaded by a browser on a Mac carries the quarantine flag: clear it with
`xattr -d com.apple.quarantine tolkster`. Anyone who can push can replace a release's files;
a worker host takes CI's own builds from the Worker ([Worker hosts](hosts.md)). Its `listen`
takes the Worker's build of each commit pushed to `main`.

The host that develops Tolkster builds its own from `<CHECKOUT>`, after `npm ci`, with a
**Rust toolchain**: `cargo install --path cli --profile local --locked --features develop`. Its
`listen` rebuilds when a merge changes `cli/` ([Keeping up with main](realtime.md#keeping-up-with-main)).

## 3. `tolkster setup`

Run it on a terminal (`--team`, `--agent-name`, `--app-name` and `--org` answer its questions ahead). Each
step skips what is done, so after a failure run it again. It prints each step `done`, `set` or
`failed`, and `next`, what is left to you.

1. **The Worker's checkout.** The Worker deployed is the CLI's own commit, so the two keep one
   contract: from the checkout `setup` runs in when that is at the commit, else from its clone
   of the repo at it, `~/.tolkster/worker`.
2. **Cloudflare.** `wrangler login` unless signed in; check its account, and with more
   than one set `CLOUDFLARE_ACCOUNT_ID`.
3. **Deploy**, unless the Worker exists, and write `worker`. Later deploys are
   `.github/workflows/deploy-worker.yml`'s, on a push to `main` that touches the Worker, with
   two repo secrets: `CLOUDFLARE_API_TOKEN`, a custom token with the
   **Editor** role on the one Worker step 3 deployed (Individual Workers), and `CLOUDFLARE_ACCOUNT_ID`. Deploying by hand, when
   Actions couldn't run, is yours, never a worker's.
4. **The Linear app**, from an OAuth app manifest: `setup` opens Linear's create page filled in with
   `<AGENT>`, the Worker's callback, its webhook on exactly the types it handles, and the client
   credentials grant. Create it as an admin, with the agent's avatar, then paste its client ID, client
   secret and webhook secret, which Linear shows on its page alone.
5. **The Worker's secrets**, each typed with no echo, or made: `REALTIME_SECRET`, kept as
   `realtimeSecret` in `~/.tolkster/credentials.json` with the client ID and secret, readable by
   you alone; `CLAUDE_TOKEN`, `claude setup-token`'s, for worker hosts. Rotate `REALTIME_SECRET`
   by removing `realtimeSecret`, unset in the environment, and rerunning `setup`.
6. **Install**: it opens the install URL; approve it, granting the teams the agent works in.
   The scope list must be exactly `SCOPES` in `src/shared/linear.ts` and `cli/src/linear.rs`
   (a test pins the two). **Linear revokes every actor token for the app whenever one is
   requested with a different set of scopes**, so a Worker and a CLI asking for different ones
   would keep revoking each other's. Nothing exchanges the returned `code`: both sides mint
   tokens with the `client_credentials` grant.
7. **The owner**: pick yourself from the workspace's people. It writes `owner.id` and
   `owner.name` to `~/.tolkster/config.json`, and sets `OWNER_USER_ID` once: that names the
   relay's Durable Object, so it never changes. No flag sets the owner ([The CLI's
   interface](cli.md)).
8. **The GitHub App** (step 4).
9. **A team**: `tolkster init --team <KEY>` ([Adding a team or repo](#adding-a-team-or-repo)),
   ending with `doctor`'s checks.
10. **The release pipeline** (`develop` builds): the `Tolkster` pipeline, made if
    none on step 9's team, as `releasePipeline` ([Keeping up with main](realtime.md#keeping-up-with-main)).
    Alpha, on some plans only.

**Check:** `tolkster whoami` prints `<AGENT>` and the app user's ID, not you.

## 4. The GitHub App

The app gives the agent its own GitHub identity, a `<slug>[bot]` user: it co-authors
commits, opens PRs so your approval counts, and makes every GitHub read. Without it a
worker stops at opening its PR. `setup` runs `tolkster github app create --name <APP NAME>
--org <ORG>` in the Worker's checkout.

The name must be unused across GitHub; without `--org` the app is your own. Your browser hands GitHub the app's manifest, with its [permissions](github-app.md#permissions)
and its webhook. Create the app there, and GitHub sends you back: the command sets its ID,
key and webhook secret as the Worker's secrets, writes `owner.coAuthor` ([Co-author](co-author.md))
from its bot and `<AGENT>`, and opens its install page. Install it on the agent's repos,
with the agent's avatar as its logo, and it sets `GITHUB_INSTALLATION_ID`. The key never
reaches disk or the terminal. Run again, it finds the app and does only what is missing.
**Copilot reviews**: [Copilot's review](github-app.md#copilots-review).

## 5. Making it yours

No code or prompt names the owner: what makes an install yours is `config.json`, and
`tolkster config` prints every setting with its default. A key it doesn't know exits 2 naming
it and its replacement. `tolkster config schema` prints the JSON Schema that says the same;
`init` and `host join` write it beside the file, so an editor flags a misspelt key.

- `owner`: `id` and `name` (step 3) and `coAuthor` (step 4).
- `worker`, `<WORKER_URL>`, where `listen` connects: `https://`, or `http://localhost` for
  `npx wrangler dev`.
- `review`: `reviewers`, the GitHub logins, `<GH_LOGIN>`, whose PR feedback counts
  ([Blessed reviewers](review-feedback.md#whose-feedback-and-when-it-resumes)); `maxRounds`
  (3), review rounds before Copilot's reviews go to you ([A cap on review
  rounds](review-feedback.md#a-cap-on-review-rounds)); `deepAt` (`M`), the size, `XS` to
  `XXXL`, at which a pre-push review goes deep ([Which pass](pre-push-review.md#which-pass)).
- `triage`: `batch` (5); `ladder`: the model and effort per size ([Model and
  effort](dispatcher.md#model-and-effort)).
- `slots`, by host name, `local` (6) here: work workers at once, none on a host without one
  ([Who waits](dispatcher.md#who-waits)).
- `timing` (on): `false` stops [timing](timing.md#the-setting) sessions.
- `repos` (step 6).

`listen` and the commands that report to the Durable Object need `worker` and
`realtimeSecret`. **Check:** `tolkster listen --print` prints `connected`, and
each delegation or reply then prints an `in` line within seconds, without acting on it.

`listen` re-reads `config.json` at each listing, so an edit reaches the next launch, not
`worker`, which it reads once. It logs each change once, a flag or number by its value and any
other key by its path; a file that fails to read is logged once and the last config stays.

## 6. Orca repos and `tolkster listen`

Add every repo the agent should work in (`orca repo add --path <path> --json`). An issue
reaches a repo by the first of these that matches:

1. its Linear project, by ID or name, in a `repos` entry's `projects`;
2. its team, by key, in a `repos` entry's `teams`: the team's default repo;
3. its project's name equal to an Orca repo's display name.

`tolkster repo map <repo> --project <name|id> --team <KEY>` writes an entry, checked against
Orca and Linear first; `repo unmap` takes from it, and `repo list` shows each entry:

```json
"repos": {
  "Plan Portal": { "projects": ["Portal v2"], "teams": ["PP"], "review": { "reviewers": ["<GH_LOGIN>"], "maxRounds": 2 } },
  "Site": { "teams": ["WEB"] },
  "Tolkster": { "review": { "reviewers": [] } }
}
```

An entry that matches decides: if its repo isn't in Orca, or two Orca repos have its name,
the issue has no repo rather than falling to a later rule, and is handed back. Each entry is
keyed by its repo's Orca display name, once ignoring case, and a project or team in two
entries is an error (`tolkster config` and `repo map` exit 2). An entry's `review` replaces
the global settings for its repo; `"reviewers": []` leaves its PRs alone, and an
entry with only `review` maps nothing.

A repo opts in to machines made from its Orca recipe ([Worker hosts](hosts.md)) with
`"recipe": { "id": "<recipe id>", "machines": 2, "slotsEach": 3, "idleMinutes": 30 }` in its
entry: how many may exist at once, counted from `machines.json`, each one's cap in place of
`slots`, and, optional, the idle minutes before one sleeps and `destroyAfter`, the minutes,
an hour or more, before a dormant one is destroyed ([Destroying](hosts.md)). `tolkster hosts` shows a host's
recipe. `"trace": true` shows its workers' text and calls in their sessions, redacted
([Tracing](voice.md#tracing)), from each worker's next launch.

An entry's `"land": "direct"` lands its repo's workers straight on its default branch, with
no PR, after the owner's steps in [Landing on main](landing.md); `"pr"` is the default.

Keep **`tolkster listen`** running in a terminal from `<CHECKOUT>`, on `main` ([Running
tolkster listen](realtime.md#running-tolkster-listen)): it does all the dispatching.

## 7. Try it

Delegate a throwaway issue, in a project whose name matches an Orca repo, to `<AGENT>`:
"Read README.md and post a two-sentence summary. Make no changes." With `listen` running,
the worker starts within seconds. If nothing happens, each event is a line in `listen`'s
terminal. Cancel the issue and remove its worktree afterwards.

## A second workstation

A worker host runs workers for the `listen` host and holds only its own revocable
credential ([Worker hosts](hosts.md)). Any of Windows, macOS or Linux; no Node, Rust, `gh`,
git login or Claude sign-in for Tolkster's sake.

1. **Before:** install what needs root: git that runs hooks from config (`git hook list
   pre-commit` works), Orca, and the toolchains the repos need.
2. **Pair it** in Orca, as an SSH target or a managed server, under the name `slots` will
   know it by, and set up one repo on it there.
3. **Add it,** on the `listen` host: `tolkster host add <environment> --destination
   <absolute path>`. It installs Claude Code and the prebuilt `tolkster`, delivers the
   credential and `config.json`, clones the repos under `<path>/<name>` and runs `doctor`. It prints each step, exits 1 unless all pass, and is safe to
   rerun. For a machine made from a recipe instead, `host add <name> --recipe <id>`
   ([Worker hosts](hosts.md)) needs no pairing or first repo. A host whose `tolkster` can't read
   its `config.json` takes no work: rerun `host add` to rejoin it.
4. **By hand, on the host:** make auto mode Claude's default (it is checked, not set), put
   `~/.local/bin` on PATH and copy your `~/.claude/CLAUDE.md`. Worktrees' trust needs
   nothing.
5. **Check:** give it a `slots` entry; `tolkster hosts` shows it answering. Set `local` low, then
   delegate a throwaway issue (step 7): its worker lands under the new host.

A new worktree's setup seeds `cli/target/debug` from the primary checkout's when their `Cargo.lock` and
`Cargo.toml` match, else builds cold. After a dependency or `[profile]` change, drop `baseInputs` from
`scripts/orca-vm/agent37-state.json` and rerun `agent37-recipe.ts prepare` there, or a recipe's seed stops helping.

A host keeps itself on `listen`'s build. A private repo's push to `main` builds Linux x64
alone: for a macOS or Windows host there, run the `Build` workflow with "Build all targets"
once `listen` is on main's tip, which a manual run builds.

**Removing it:** `tolkster host rm <host>` revokes its credential, and for a recipe machine
destroys it; then unpair it in Orca once its workers finish.

## Adding a team or repo

1. **A team:** grant the Linear app the team (`init` prints the install URL).
2. **A repo:** add it to Orca (step 6) with its `origin` on GitHub.
3. **`tolkster init --team <KEY>`** creates the team's `plan-first` and `manual-merge`
   labels and the workspace's `tolkster` group with `host offline` where missing, and
   merges the team's `repos` entries, asking for its default repo, without one a repo per
   project reaching none, and each repo's `review` settings (without a terminal,
   `--default-repo <name|none>`, and `--yes` for its proposals). It lists what is yours:
   the install URL, a started state with "review" in its name after the one work starts
   in, and on each repo the GitHub App, *Allow auto-merge*, a required check and an
   `AGENTS.md` or `CLAUDE.md` with a `tolkster-review` block ([Pre-push
   review](pre-push-review.md#which-pass)). It ends with `doctor --team`'s checks; run
   again, it changes only what is missing.
4. **Check it:** `tolkster doctor --team <KEY>` (or `--repo <name>`) changes nothing; fix
   each `fail` before the first delegation.
