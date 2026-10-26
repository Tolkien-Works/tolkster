# Co-author

Every commit a worker makes carries `Co-authored-by: <owner.coAuthor>`, so GitHub shows the agent
beside the owner; commits anywhere else never do. A Git hook does it, so no brief has to remember, and it can't live in a repo's `.githooks`.

- **The marker.** `tolkster worker` writes a `tolkster-session` file into a worker's
  worktree's own git dir (`cli/src/coauthor.rs`), never the primary checkout's shared one,
  so worktrees you create yourself are never marked.
- **The hook** is a Git config-based hook, which Git runs as well as the repo's own, so
  nothing is replaced and `core.hooksPath` is left alone. It is plain `sh` and `git`, so a
  commit never depends on `tolkster` being on PATH, and without the marker it does nothing.
- **Unsetting `owner.coAuthor`** removes the hook at the next worker's start; marked worktrees
  keep crediting until then. If Git doesn't run config hooks, the worker runs uncredited.
- **On a worker host** the hook reads the host's own `config.json`, which `host add` enrols with
  the `owner.coAuthor` this machine resolves (`TOLKSTER_CO_AUTHOR` included), and fails a host
  that doesn't credit it until `--rejoin`. Its commits are authored as the owner.
