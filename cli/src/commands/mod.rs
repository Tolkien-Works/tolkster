pub mod activity;
pub mod asker;
pub mod comment;
pub mod config;
pub mod converse;
pub mod doctor;
pub mod git_credential;
pub mod github;
pub mod help;
pub mod hook;
pub mod host;
pub mod hosts;
pub mod images;
pub mod init;
pub mod issue;
pub mod land;
pub mod link;
pub mod list;
pub mod listen;
pub mod logs;
pub mod members;
pub mod messages;
pub mod plan;
pub mod pr;
pub mod reap;
pub mod repo;
pub mod sessions;
pub mod setup;
pub mod show;
pub mod start;
pub mod timing;
pub mod triage;
pub mod version;
pub mod whoami;
pub mod worker;

use ureq::Agent;

use crate::args::Kind;
use crate::error::Error;

// A command `main` runs: the words that name it, the options its `run` parses and whether it
// takes arguments, which a brief's `tolkster` line is checked against (brief/tests/commands.rs)
// and the gate's tables too (allowlist/tests.rs), and its entry in `tolkster help`, which its
// --help prints and its usage errors end with.
pub struct Spec {
    // The command, then any subcommand.
    pub words: &'static [&'static str],
    #[cfg_attr(not(test), allow(dead_code))]
    pub options: &'static [(&'static str, Kind)],
    #[cfg_attr(not(test), allow(dead_code))]
    pub arguments: bool,
    // Given what follows the words.
    pub run: fn(&Agent, &[String]) -> Result<u8, Error>,
    // Empty for a command the help leaves out. The newline that starts it is dropped
    // (help::entry), there so the source lines up as the help prints.
    pub help: &'static str,
}

// Every command, in the help's order. One with subcommands has an entry of its name alone,
// whose run refuses a subcommand it doesn't take.
pub const COMMANDS: &[Spec] = &[
    Spec { words: &["version"], options: &[], arguments: false, run: |_, rest| version::run(rest), help: r#"
  version                                  print the CLI's version and commit (also --version, -V)"# },
    Spec { words: &["--version"], options: &[], arguments: false, run: |_, rest| version::run(rest), help: "" },
    Spec { words: &["-V"], options: &[], arguments: false, run: |_, rest| version::run(rest), help: "" },
    Spec { words: &["setup"], options: setup::OPTIONS, arguments: false, run: setup::run, help: r#"
  setup [--team <KEY>] [--agent-name <name>] [--app-name <name>] [--org <org>]
                                           set Tolkster up from scratch, on a terminal: deploys
                                           this build's Worker, sets its secrets, asks for the
                                           Linear app's and installs it, picks the owner, then
                                           runs github app create and init; each step is
                                           skipped when done. Prints {"ok", "steps", "next"}:
                                           each step "done", "set" or "failed", and what is
                                           left to the owner; exits 1 on a failed step"# },
    Spec { words: &["whoami"], options: &[], arguments: false, run: whoami::run, help: r#"
  whoami                                   check the credentials; print the app user"# },
    Spec { words: &["sessions"], options: sessions::OPTIONS, arguments: false, run: sessions::run, help: r#"
  sessions [--issue <TW-123>]              list Tolkster's agent sessions, newest first"# },
    Spec { words: &["activity"], options: activity::OPTIONS, arguments: true, run: activity::run, help: r#"
  activity <sessionId> <type> [options]    post an activity to a session
      type: thought | action | elicitation | response | error
      --body <text> | --body-file <path|->  (thought, elicitation, response, error)
      --action <verb> --parameter <text> [--result <text>]  (action; a durable one
                                           also sets a worker's worktree card's comment)
      --ephemeral                           (thought, action)
      --option <name>                       (elicitation, repeatable) a select option
                                           Linear shows as a button: {CHOICES}
      --state <name>                        (thought) an ephemeral thought, one of the
                                           state's phrases at random: {STATES}"# },
    Spec { words: &["in-review"], options: &[], arguments: true, run: link::run_in_review, help: r#"
  in-review <sessionId>                    move the session's issue to its team's review state,
                                           unless it is there or past it; prints {"review":
                                           "moved" | "unchanged" | "no review state"}"# },
    Spec { words: &["plan"], options: plan::OPTIONS, arguments: true, run: plan::run, help: r#"
  plan <sessionId> --file <path|->         set the session's plan, the checklist Linear shows:
                                           a JSON array of {"content": "...", "status": "..."},
                                           status pending | inProgress | completed | canceled,
                                           at most one inProgress. Replaces the whole plan"# },
    Spec { words: &["messages"], options: messages::OPTIONS, arguments: true, run: messages::run, help: r#"
  messages <sessionId> [--after <time>]    the owner's messages in a session that nothing has
                                           answered; with --after, those after that time, answered
                                           or not, as a conversation's turn reads them
  messages --thread <commentId> [--after <time>]
                                           the owner's replies in the thread of Tolkster's
                                           comment, as a reply's turn reads them"# },
    Spec { words: &["start"], options: &[], arguments: true, run: start::run, help: r#"
  start <sessionId>                        a worker's first read: the session's issue with its
                                           comments, as issue show prints it, and "messages",
                                           as messages prints them, read at once, as JSON"# },
    Spec { words: &["members"], options: members::OPTIONS, arguments: false, run: members::run, help: r#"
  members [--team <KEY>]                   the workspace's active users, or a team's, with their
                                           Linear IDs, as JSON: how setup finds the owner's ID"# },
    Spec { words: &["logs"], options: logs::OPTIONS, arguments: false, run: logs::run, help: r#"
  logs [--session <id>] [--issue <TW-123>] [--since <time>] [--until <time>]
                                           the Worker's log lines for a session, an issue and
                                           its sessions, or both, from --since to --until, a
                                           time or 30m, 6h, 2d back (by default 7d before
                                           --until, now when not given), oldest first: {"lines",
                                           "more"}, "more" when a query hit its cap: narrow
                                           the window"# },
    Spec { words: &["timing"], options: timing::OPTIONS, arguments: false, run: timing::run, help: r#"
  timing [--since <time>] [--until <time>] [--by size|model|build] [--checks] [--json]
                                           on listen's host, where workers' time went: per-issue
                                           medians of each issue's first run started in the
                                           window (by default 30d before --until), by group;
                                           --checks, why checks ran as often as they did. A
                                           table, or with --json docs/timing.md's shape; exits
                                           3 when no worker started in the window"# },
    Spec { words: &["config"], options: &[], arguments: true, run: config::run, help: r#"
  config [schema]                          the settings, as JSON: owner, review, triage, ladder,
                                           repos, slots;
                                           "schema": the JSON Schema config.json is checked by"# },
    Spec { words: &["doctor"], options: doctor::OPTIONS, arguments: false, run: doctor::run, help: r#"
  doctor [--team <KEY>] [--repo <name>]    whether a team and a repo (by default each repo the
                                           team's mapping reaches) are ready for delegation, as
                                           JSON checks, each ok, warn or fail; exits 1 on a fail"# },
    Spec { words: &["init"], options: init::OPTIONS, arguments: false, run: init::run, help: r#"
  init --team <KEY> [--default-repo <name|none>] [--yes]
                                           create the team's missing flags, the workspace's
                                           tolkster group and its states, and the team's repos
                                           entries, asking on a terminal; prints "created",
                                           "changed", the owner's "steps", and doctor's "ok" and
                                           "checks", exiting as doctor does. Without a terminal,
                                           exits 2 when it needs a flag it lacks"# },
    Spec { words: &["repo", "list"], options: &[], arguments: false, run: |_, rest| repo::list(rest), help: r#"
  repo list                                each repos entry as config prints it, with "inOrca",
                                           whether Orca has its repo, as a JSON array"# },
    Spec { words: &["repo", "map"], options: repo::MAP_OPTIONS, arguments: true, run: repo::map, help: r#"
  repo map <repo> [--project <name|id>]... [--team <KEY>]... [--reviewers <login,...|none>]
      [--max-rounds <n>]                   add to the repos entry for the Orca repo with that
                                           display name, or create it, once Orca has the repo
                                           and the app the teams and their projects (else
                                           exits 1); prints {"repo", "changed"}. Exits 2 when
                                           a project or team would be in two entries, 3 when
                                           nothing changed"# },
    Spec { words: &["repo", "unmap"], options: repo::UNMAP_OPTIONS, arguments: true, run: |_, rest| repo::unmap(rest), help: r#"
  repo unmap <repo> [--project <name|id>]... [--team <KEY>]... [--reviewers] [--max-rounds]
                                           take what the flags name out of the repo's entries,
                                           or with none the whole entry; exits 3 when nothing
                                           changed"# },
    Spec { words: &["repo"], options: &[], arguments: false, run: |_, _| Err(Error::Usage("repo needs list, map <repo> or unmap <repo>".into())), help: "" },
    Spec { words: &["github", "app", "create"], options: github::OPTIONS, arguments: false, run: github::run, help: r#"
  github app create [--name <app name>] [--org <org>]
                                           make the GitHub App by GitHub's manifest flow, from
                                           the checkout: sets the Worker's GITHUB_ secrets with
                                           wrangler, coAuthor and, once installed, the
                                           installation; --name only to make one; prints
                                           {"slug", "created", "coAuthor", "installation"};
                                           exits 3 when all was already set"# },
    Spec { words: &["github"], options: &[], arguments: false, run: |_, _| Err(Error::Usage("github needs app create".into())), help: "" },
    Spec { words: &["host", "add"], options: host::add::OPTIONS, arguments: true, run: host::add::run, help: r#"
  host add <environment> --destination <path> [--rejoin]
                                           make a paired Orca server with one repo set up a
                                           worker host: git, Claude, auto mode, tolkster and
                                           each mapped repo, each checked first (--rejoin joins
                                           again however tolkster there answers); prints {"host", "ok",
                                           "steps"}; exits 1 unless every step is ok"# },
    Spec { words: &["host", "rm"], options: &[], arguments: true, run: host::rm, help: r#"
  host rm <host>                           revoke a worker host's credential, so it takes no
                                           worker, and destroy a recipe's machine; prints
                                           {"host", "held", "destroyed"}"# },
    Spec { words: &["host"], options: &[], arguments: false, run: |_, _| Err(Error::Usage("host needs add <environment> --destination <path> [--rejoin], add <name> --recipe <id> [--repo <name>], or rm <host>".into())), help: "" },
    Spec { words: &["hosts"], options: &[], arguments: false, run: hosts::run, help: r#"
  hosts                                    this machine and each paired host, as a JSON array of
                                           {"name" (null for this machine), "answers", "cap",
                                           "workers" (issue identifiers)}; asks each host afresh"# },
    Spec { words: &["issue", "show"], options: show::OPTIONS, arguments: true, run: show::run, help: r#"
  issue show <TW-123>... [--comments] [--children] [--attachments]
                                           read an issue as Tolkster, as JSON: its fields, parent
                                           and relations, and with the flags its comments (oldest
                                           first), its sub-issues (each with its links) and its
                                           links; several, read at once, as an array. Its shape
                                           is in docs/cli.md"# },
    Spec { words: &["issue", "list"], options: list::OPTIONS, arguments: false, run: list::run, help: r#"
  issue list [--team <KEY>] [--project <name>] [--state open|<name>] [--query <text>]
      [--parent <TW-123>] [--limit <n>]    find issues as Tolkster, as JSON: issues and more,
                                           each with identifier, title, state, priority, estimate,
                                           parent and updatedAt. Open ones unless --state names a
                                           state; --query is Linear's issue search; --limit is
                                           1 to 250 (50). Its shape is in docs/cli.md"# },
    Spec { words: &["issue", "update"], options: issue::UPDATE, arguments: true, run: issue::issue_update, help: r#"
  issue update <TW-123> [--session <id> | --thread <commentId>] [--title <text>] [--description-file <path|->]
      [--priority <0-4|none|urgent|high|medium|low>] [--estimate <n|none>] [--state <name>]
      [--project <name|none>] [--assignee <me|none|userId>] [--add-label <name>]...
      [--remove-label <name>]... [--delegate]
                                           --estimate must be on the issue's team's scale,
                                           else exits 2
                                           --assignee me is the owner, and a user ID comes
                                           from tolkster members; not with --delegate
                                           --delegate needs --session, and delegates it to
                                           Tolkster: only an unfinished sub-issue of the
                                           session's issue, which must be delegated to
                                           Tolkster, else exits 1
                                           --thread, in a reply's turn, changes only the
                                           issue its thread is on, else exits 2"# },
    Spec { words: &["issue", "create"], options: issue::CREATE, arguments: false, run: issue::issue_create, help: r#"
  issue create [--session <id> | --thread <commentId>] --title <text> [--description-file <path|->] [--parent <TW-123|none>]
      [--team <KEY>] [--project <name|none>] [--priority <p>] [--estimate <n>]
      [--assignee <me|none|userId>] [--state <name> | --triage] [--label <name>]...
      [--blocks <TW-456>]... [--blocked-by <TW-456>]... [--related <TW-456>]... [--duplicate-of <TW-456>]...
                                           prints the new issue's identifier and url, as JSON,
                                           and the relations it made. Every other issue is
                                           read, and from a thread checked as relate checks,
                                           before it is filed: a refusal or an unknown ID
                                           files nothing. If a relation fails it still
                                           prints, then exits 1: add the rest with relate,
                                           never file again.
                                           --triage files it in the team's triage state, or
                                           its first backlog state without one, by type.
                                           Its team and project are the parent's, or else
                                           the session's issue's, unless --team names another;
                                           --project, one of the team's, overrides it.
                                           Without --session, pass --parent or --team.
                                           With --thread it is a sub-issue of the thread's
                                           issue unless --parent names another or none, and
                                           one of an issue delegated to Tolkster goes to Triage"# },
    Spec { words: &["issue", "relate"], options: issue::RELATE, arguments: true, run: |agent, rest| issue::issue_relate(agent, rest, true), help: RELATE_HELP },
    Spec { words: &["issue", "unrelate"], options: issue::RELATE, arguments: true, run: |agent, rest| issue::issue_relate(agent, rest, false), help: RELATE_HELP },
    Spec { words: &["issue", "chain"], options: issue::CHAIN, arguments: true, run: issue::issue_chain, help: r#"
  issue chain <TW-123> --session <id> [--block <TW-3>:<TW-2>]...
                                           from the owner's session on TW-123, delegated
                                           to Tolkster: blocks each sub-issue on its
                                           blocker, keeping relations already there, then
                                           delegates each unfinished sub-issue, unblocked
                                           first; prints what it did, as JSON. Refuses (exit
                                           1, nothing written) an ID that isn't an unfinished
                                           sub-issue of TW-123, a loop, new or already
                                           there, or more than it reads at once;
                                           exits 3 when none is left to delegate"# },
    Spec { words: &["issue"], options: &[], arguments: false, run: |_, _| Err(Error::Usage("tolkster issue takes show, list, update, create, relate, unrelate or chain".into())), help: "" },
    Spec { words: &["comment"], options: comment::OPTIONS, arguments: true, run: comment::run, help: r#"
  comment <TW-123> [--session <id>] (--body <text> | --body-file <path|->)
  comment <TW-123> --closed <TW-456> (--body <text> | --body-file <path|->)
                                           context for an open issue related to TW-456,
                                           which must be completed and Tolkster's; at most
                                           one per issue for TW-456, else exits 3
  comment <TW-123> --reply-to <commentId> [--session <id>] (--body <text> | --body-file <path|->)
                                           a reply in the thread of Tolkster's own comment on
                                           TW-123; a reply's turn posts one, else exits 3"# },
    Spec { words: &["triage"], options: triage::OPTIONS, arguments: true, run: triage::run, help: r#"
  triage <TW-123> --session <id> --verdict <v> (--body <text> | --body-file <path|->)
      accept [--estimate <n>] --priority <p> [--blocked-by <TW-456>]... [--related <TW-456>]...
      | fixed --by <sha> | duplicate --of <TW-456> | decline
                                           a triage session's one verdict on a Triage issue,
                                           as its brief says: the comment, then the move to
                                           the backlog (accept) or a cancelled state (fixed,
                                           duplicate) with the relations; decline moves
                                           nothing. Refuses an issue not in Triage, not the
                                           owner's or Tolkster's, or not this session's, and
                                           a related issue that is closed or in another
                                           team, with exit 1; a second verdict exits 3.
                                           accept's --estimate must be on the team's scale,
                                           and left out when it doesn't use estimates,
                                           else exits 2.
                                           Prints the issue, verdict and new state as JSON"# },
    Spec { words: &["pr", "create"], options: pr::CREATE, arguments: false, run: pr::create, help: r#"
  pr create [--repo <owner/name>] --head <branch> --base <branch> --title <text>
      --body-file <path|-> [--session <id>] [--auto-merge]
                                           open a PR as the GitHub App, so its author is the
                                           app's bot; prints its number, url and "existing"
                                           (true when the branch had an open PR already,
                                           which is used instead), as JSON. With --session,
                                           the owner's session: links the PR there and moves
                                           its issue to review, as in-review, adding "linked"
                                           and "review" ("failed" if it couldn't). With
                                           --auto-merge, turns it on as pr auto-merge does,
                                           adding "autoMerge": "on", "refused" or
                                           "unavailable"; it still exits 0. A PR GitHub
                                           refused as mergeable already is "on" once the
                                           session's machine is asked to merge it. Every pr
                                           command's token is minted by the Worker, and
                                           --repo defaults to the origin remote"# },
    Spec { words: &["pr", "feedback"], options: pr::FEEDBACK, arguments: false, run: pr::feedback, help: r#"
  pr feedback --pr <n> [--repo <owner/name> | --session <id>]
                                           read that PR's feedback from "review.reviewers"
                                           as the app: comments, each one's latest review with a
                                           body, and unresolved threads with their ids, as JSON;
                                           with --session, record the read on it"# },
    Spec { words: &["pr", "checks"], options: pr::CHECKS, arguments: false, run: pr::checks, help: r#"
  pr checks [--repo <owner/name>] --pr <n> [--head <sha>]
                                           read that PR's checks once as the app, as JSON. A
                                           check Actions never ran (no minutes, or billing)
                                           is "unavailable", not failed.
                                           Exits 0 passed, 1 failed, 8 pending, 3 no checks,
                                           4 CI couldn't run, 5 merged, 6 closed unmerged"# },
    Spec { words: &["pr", "reply"], options: pr::REPLY, arguments: false, run: pr::reply, help: r#"
  pr reply [--repo <owner/name>] --pr <n> --thread <id> --body-file <path|->
                                           reply on a review thread of that PR as the app;
                                           prints the comment's id and url, as JSON"# },
    Spec { words: &["pr", "resolve"], options: pr::RESOLVE, arguments: false, run: pr::resolve, help: r#"
  pr resolve [--repo <owner/name>] --pr <n> --thread <id>
                                           resolve a review thread of that PR as the app"# },
    Spec { words: &["pr", "auto-merge"], options: pr::AUTO_MERGE, arguments: false, run: pr::auto_merge, help: r#"
  pr auto-merge [--repo <owner/name>] --pr <n>
                                           turn on auto-merge (squash) for that PR as the
                                           app; prints {"pr": n, "autoMerge": true}.
                                           Exits 1 when GitHub refused it, 8 when its
                                           budget is still spent, or GitHub still
                                           unavailable, after the retries (not on)"# },
    Spec { words: &["pr", "describe"], options: pr::DESCRIBE, arguments: false, run: pr::describe, help: r#"
  pr describe [--repo <owner/name>] --pr <n> --body-file <path|->
                                           replace that open PR's description with the
                                           file's text, as the app; prints
                                           {"pr": n, "described": true}"# },
    Spec { words: &["pr", "review-pass"], options: pr::REVIEW_PASS, arguments: false, run: pr::review_pass, help: r#"
  pr review-pass --size <below|at|above|unsized> --copilot <off|opening|each-push>
      [--session <id>]
                                           decide the pre-push review from the claim's size
                                           and Copilot's review, the "tolkster-review" block
                                           in AGENTS.md or CLAUDE.md on the default branch
                                           and HEAD's diff since it: prints "pass" (skip,
                                           light or deep), "passes" (the deep pass's
                                           reviewers), "reason", "escalated", "newLines",
                                           and when a trigger escalated it, "trigger" and
                                           "file", and when it went by the worker's
                                           estimate, "sized", as JSON. With --session,
                                           posts a thought there saying why when escalated
                                           or sized"# },
    Spec { words: &["pr"], options: &[], arguments: false, run: |_, _| Err(Error::Usage("tolkster pr takes create, feedback, checks, reply, resolve, auto-merge, describe or review-pass".into())), help: "" },
    Spec { words: &["land"], options: land::OPTIONS, arguments: false, run: land::run, help: r#"
  land --session <id> --title <text> --body-file <path|->
                                           in a repo whose "land" is "direct", squash
                                           HEAD's commits into one, "<title> (TW-123)",
                                           rebase it onto origin's default branch and push
                                           it there fast-forward only, as the app; then move
                                           the session's issue to Done, link the commit there,
                                           delete origin's copy of the branch and report the
                                           landing. Prints "sha", "url", "pushed" (false
                                           when an earlier run landed it), "done" ("moved",
                                           "unchanged" or "failed"), "linked",
                                           "branchDeleted" and "reported", as JSON. Exits 2
                                           in a "pr" repo, for a manual-merge or finished
                                           issue, or outside the issue's own worktree, 3
                                           with nothing to land, and 7 on conflicts,
                                           printing {"conflicts": [paths]}"# },
    Spec { words: &["reap"], options: reap::OPTIONS, arguments: false, run: reap::run, help: r#"
  reap [--dry-run]                         remove worktrees whose PR has merged, whose
                                           issue was cancelled a day ago, or whose issue
                                           was completed with no PR, and forget their
                                           worker records"# },
    Spec { words: &["reap-here"], options: &crate::reap::STEP_OPTIONS, arguments: false, run: reap::run_here, help: "" },
    Spec { words: &["listen"], options: listen::OPTIONS, arguments: false, run: listen::run, help: r#"
  listen [--print]                         hold a connection to the Worker's realtime relay
                                           and act on each command as the owner's webhooks
                                           arrive: start, message or stop a worker. Prints
                                           one timestamped line per event in and per write
                                           out; --print only prints the events. Also
                                           watches Orca: an agent's exit reaps its
                                           worktree and starts work waiting for the slot.
                                           Run from Tolkster's own primary checkout, it
                                           fast-forwards main when a push arrives, and when
                                           cli/ changed rebuilds and starts again in place.
                                           Runs until stopped; reconnects by itself"# },
    // Its run takes the thread's form too, wherever --thread is, with
    // commands::converse::THREAD_OPTIONS, which the brief check would refuse: no brief names it.
    Spec { words: &["converse"], options: &crate::converse::TURN_OPTIONS, arguments: true, run: converse::run, help: r#"
  converse <sessionId> --agent-session <uuid> [--request <time>] [--messages-after <time>] --brief <path> [--stale]
  converse --thread <commentId> --request <time> [--replies-after <time>] --brief <path> [--stale]
                                           one turn of a conversation, run by listen in the
                                           repo's conversations checkout: Claude on the
                                           turn's brief, reading only; with --thread, a turn
                                           in the thread of Tolkster's comment; not for people"# },
    Spec { words: &["hook", "tool"], options: &[], arguments: false, run: |agent, rest| hook::run(agent, "tool", rest), help: r#"
  hook tool                                a worker's hook before each tool call, its input
                                           on stdin: posts what the worker is doing, or
                                           ticks while a test, build, push or sub-agent
                                           runs; not for people"# },
    Spec { words: &["hook", "start"], options: &[], arguments: false, run: |agent, rest| hook::run(agent, "start", rest), help: r#"
  hook start                               a worker's hook before each shell call or
                                           sub-agent, its input on stdin: posts a test's,
                                           build's, push's or sub-agent's start; not for
                                           people"# },
    Spec { words: &["hook", "state"], options: &[], arguments: false, run: |agent, rest| hook::run(agent, "state", rest), help: r#"
  hook state                               a worker's hook when its agent's state changes,
                                           its input on stdin: reports the state; not for
                                           people"# },
    Spec { words: &["hook", "after"], options: &[], arguments: false, run: |agent, rest| hook::run(agent, "after", rest), help: r#"
  hook after                               a worker's hook after each shell call or
                                           sub-agent, its input on stdin: posts a commit's
                                           Committed action, or a paired call's outcome;
                                           not for people"# },
    Spec { words: &["hook"], options: &[], arguments: false, run: |_, _| hook::usage(), help: "" },
    Spec { words: &["worker"], options: &crate::setup::OPTIONS, arguments: true, run: worker::run, help: r#"
  worker <sessionId> --issue <TW-123> [--repo <name>] [--model <m> --effort <e>]
      (--review <size> [--plan-first] [--orchestrate] [--manual-merge] [--then <why>]
      | --continue --then <why> | --round review|last|rebase --pr <n>)
                                           the line listen starts a worker's terminal on, on
                                           the worker's own host: writes its briefs and
                                           settings, names and pushes its branch, trusts its
                                           worktree, then starts claude; not for people"# },
    Spec { words: &["host", "join"], options: host::JOIN, arguments: true, run: host::join, help: r#"
  host join <code> --worker-url <url> | host clone <owner/name> --into <folder>
                                           typed on a new host by host add; not for people"# },
    Spec { words: &["host", "clone"], options: host::CLONE, arguments: true, run: |_, rest| host::clone(rest), help: "" },
    Spec { words: &["host", "identify"], options: host::IDENTIFY, arguments: true, run: |_, rest| host::identify(rest), help: "" },
    Spec { words: &[crate::credential::COMMAND], options: &[], arguments: true, run: git_credential::run, help: r#"
  git-credential <owner/name> <get|store|erase>
                                           git's credential helper for a worker's pushes,
                                           set by worker; not for people"# },
    Spec { words: &["help"], options: &[], arguments: false, run: |_, rest| help::print(rest), help: "" },
    Spec { words: &["--help"], options: &[], arguments: false, run: |_, rest| help::print(rest), help: "" },
];

const RELATE_HELP: &str = r#"
  issue relate|unrelate <TW-123> [--session <id> | --thread <commentId>] (--blocks | --blocked-by | --related | --duplicate-of) <TW-456>"#;

// The entry naming most of `argv`'s first words, and the words after them.
pub fn find(argv: &[String]) -> Option<(&'static Spec, &[String])> {
    let names = |spec: &&Spec| spec.words.len() <= argv.len() && spec.words.iter().zip(argv).all(|(word, arg)| word == arg);
    let spec = COMMANDS.iter().filter(names).max_by_key(|spec| spec.words.len())?;
    Some((spec, &argv[spec.words.len()..]))
}
