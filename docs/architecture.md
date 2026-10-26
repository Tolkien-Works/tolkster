# How it fits together

```
Linear --agent session, Issue, Comment webhook--> Worker (Cloudflare): verify, acknowledge a session within 10 s
GitHub App --PR, push, status webhook--> Worker: verify
Worker --events, IDs only--> Durable Object: the intake, the request and work machines, the coordinator, the triage machine
Durable Object --command, over a WebSocket--> `tolkster listen`: start, message, resume or stop a worker, open a turn, read, post, reap
`tolkster listen` --result, Orca's observations--> Durable Object
Claude worker --> `tolkster messages` / `activity` --> the session in Linear, as the agent
`tolkster` <--POST /state--> the Durable Object: reports, and Tolkster's own records
```

- **The Worker** (`src/worker`, TypeScript on Cloudflare) verifies each webhook and
  turns it into events, IDs only. Linear marks a session unresponsive without a first
  activity within 10 s, and the workstation may be asleep, so the Worker answers for it
  with a fixed phrase ([voice](voice.md), [Realtime](realtime.md)). Anyone but
  the owner is refused.
- **The Durable Object** decides. Each issue's events go through the intake, which
  drops redeliveries, to its request and work machines; the coordinator holds what
  crosses issues (the slots, the blockers, the open PRs), and the triage machine starts
  one triage session at a time ([state machine](state-machine.md)). **Tolkster acts
  only on the owner's sessions and prompts**, and the checks live in code, so no agent
  decides what is dispatched.
- **`tolkster listen`**, on the workstation, carries out each command as it arrives and
  reports its result ([Commands and reports](realtime.md#commands-and-reports)). Its
  watcher lists Orca when an agent's terminal exits or a job changes its worktrees,
  and every 60 s, for what no webhook can say. Nothing polls Linear or GitHub ([dispatching](dispatcher.md)).
- **The `tolkster` CLI** (`cli/`, Rust) runs `listen`, and is the one way workers read
  and write Linear, as the agent, on the app's token. Nothing needs a personal Linear
  key. `listen`'s commands and a worker's setup act through the ports
  (`cli/src/ports.rs`), which tests fake; other subcommands call each call's home
  directly.

`SCOPES` must be identical in the Worker and the CLI, and in the install URL: see
[`AGENTS.md`](../AGENTS.md) and [setup](setup.md#3-tolkster-setup).
