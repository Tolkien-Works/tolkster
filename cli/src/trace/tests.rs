use super::*;
use crate::error::Error;
use crate::machine;
use crate::plain::Id;
use serde_json::json;

fn job(issue: Option<&str>, kind: Kind) -> Job {
    Job { id: "command:c-1.7".into(), issue: issue.map(String::from), kind, started: Instant::now(), summed_up: false }
}

// What an event shows, as its line's text, plain or with `--verbose`.
fn shown(event: impl Into<Event>, verbose: bool) -> String {
    event.into().line().parts.render(verbose)
}

fn both(event: impl Into<Event>) -> (String, String) {
    let line = event.into().line();
    (line.parts.render(false), line.parts.render(true))
}

fn mark(event: impl Into<Event>) -> Mark {
    event.into().line().mark
}

fn unavailable() -> Error {
    Error::Unavailable("worktree r::/w/tw-1 didn't answer".into())
}

#[test]
fn a_line_has_the_clock_a_lead_column_and_the_text() {
    assert_eq!(formatted("14:51:37", Mark::Routine, "TW-694", true, "readIssue", false), "14:51:37  TW-694   readIssue");
    assert_eq!(formatted("12:21:47", Mark::Error, "host", false, "mac not answering (unavailable)", false), "12:21:47  ✖ host   mac not answering (unavailable)");
    assert_eq!(formatted("12:21:47", Mark::Recovered, "realtime", false, "connected", false), "12:21:47  ✔ realtime connected");
}

#[test]
fn colour_marks_the_sign_and_the_issue_only_with_the_padding_outside() {
    assert_eq!(formatted("14:51:37", Mark::Warning, "TW-694", true, "slow", true), "14:51:37  \x1b[33m!\x1b[0m \x1b[36mTW-694\x1b[0m slow");
    assert_eq!(formatted("14:51:37", Mark::Routine, "·", false, "o/r pushed", true), "14:51:37  ·        o/r pushed");
    assert!(coloured(false, false, true));
    assert!(!coloured(true, false, true), "--plain");
    assert!(!coloured(false, true, true), "NO_COLOR");
    assert!(!coloured(false, false, false), "piped");
}

#[test]
fn a_job_is_one_line_led_by_its_issue_and_named_by_its_id_only_with_verbose() {
    let command = job(Some("TW-694"), Kind::Command("readIssue".into()));
    assert_eq!(shaped(Some(&command), None, None, "ok        0.6 s", false, true), ("TW-694".into(), true, "readIssue          ok        0.6 s".into()));
    assert_eq!(shaped(Some(&command), None, None, "TW-694 maps to no repo", false, false), ("TW-694".into(), true, "TW-694 maps to no repo".into()));
    assert_eq!(shaped(Some(&command), Some("linear"), None, "show.Show complexity=7", true, false).2, "command:c-1.7: linear: show.Show complexity=7");
    // The scheduler's own jobs have a plain label, never their ID, which `--verbose` adds.
    for (kind, label) in [(Kind::Pushed("o/r".into()), "o/r pushed"), (Kind::TerminalClosed, "terminal closed"), (Kind::WorktreeClosed, "worktree closed"), (Kind::UpdateCheck, "update check")] {
        let own = Job { id: "ended:term_adbbd702".into(), ..job(None, kind) };
        assert_eq!(shaped(Some(&own), None, None, "reported  0.0 s", false, true), ("·".into(), false, format!("{:<18} reported  0.0 s", if label.ends_with("pushed") { "r pushed" } else { label })));
        assert_eq!(shaped(Some(&own), None, None, "reported", true, false).2, "ended:term_adbbd702: reported");
    }
    // An update check's own lines don't name their area again; another job's do.
    let update = job(None, Kind::UpdateCheck);
    assert_eq!(shaped(Some(&update), Some("update"), None, "main is at b41c2f0", false, false).2, "update check       main is at b41c2f0");
    let pushed = job(None, Kind::Pushed("o/r".into()));
    assert_eq!(shaped(Some(&pushed), Some("update"), None, "main is at b41c2f0", false, false).2, "r pushed           update: main is at b41c2f0");
    // A repo's `owner/` and any overrun don't push the text out of line; the disk log keeps the whole.
    let long = job(None, Kind::Pushed("Tolkien-Works/tolkster-with-a-long-name".into()));
    assert_eq!(shaped(Some(&long), None, None, "nothing to do", false, true).2, "tolkster-with-a-l… nothing to do");
    assert_eq!(shaped(Some(&pushed), None, None, "nothing to do", true, true).2, "o/r pushed         nothing to do");
}

#[test]
fn a_line_of_no_job_leads_with_its_issue_else_its_area() {
    assert_eq!(shaped(None, Some("watch"), None, "listing Orca failed (Orca)", false, false), ("watch".into(), false, "listing Orca failed (Orca)".into()));
    assert_eq!(shaped(None, None, Some("TW-7"), "readIssue: trying again in 5 s", false, false), ("TW-7".into(), true, "readIssue: trying again in 5 s".into()));
    assert_eq!(shaped(None, None, None, "a command's connection has gone", false, false).0, "·");
}

#[test]
fn an_opaque_id_and_an_errors_text_show_only_with_verbose() {
    let opaque = "OPAQUE-1";
    let events: Vec<Event> = vec![
        Work::NoNoteThread { session_id: Id::from(opaque), issue: "TW-1".into() }.into(),
        Work::HandoverFailed { thread: Some(Id::from(opaque)), error: Error::Failed("x".into()) }.into(),
        Work::Retrying { issue: None, kind: Kind::Command("readIssue".into()), job: opaque.into(), wait: Duration::from_secs(5) }.into(),
        Work::ResultHeld { command: opaque.into() }.into(),
        Work::PostedAlready { key: opaque.into(), error: Error::Failed("x".into()) }.into(),
        Call::Wrote { site: "linear commentCreate".into(), detail: format!("agentSessionId={opaque}"), failed: Some(Error::Failed("x".into())) }.into(),
        Call::Git { site: "git push".into(), detail: format!("origin {opaque}"), ok: false }.into(),
        Refresh::Failed { repo_id: opaque.into(), error: Error::Failed("x".into()) }.into(),
        Reap { issue: "TW-1".into(), what: Reaping::GoneRecordStays { worktree_id: format!("r::{opaque}").as_str().into(), error: Error::Failed("x".into()) } }.into(),
        Host::NotAnswering { name: None, id: opaque.into(), error: Error::Failed("x".into()) }.into(),
        Update::Restart(Restart::Stuck { what: Stuck::Job(opaque.into()), minutes: 10 }).into(),
        Update::LeftForResend { command: opaque.into() }.into(),
        Update::Dropped { job: opaque.into() }.into(),
        Work::Failed(Error::Orca(format!("orca worktree rm --worktree {opaque} failed"))).into(),
    ];
    for event in events {
        let line = event.line();
        assert!(!line.parts.render(false).contains(opaque), "{event:?}");
        assert!(line.parts.render(true).contains(opaque), "{event:?}");
    }
}

#[test]
fn calls_show_their_site_and_a_failure_its_errors_kind() {
    let wrote = |failed| Call::Wrote { site: "linear commentCreate".into(), detail: "agentSessionId=s-1".into(), failed };
    assert_eq!(wrote(None).line().show, Show::Verbose);
    assert_eq!(shown(wrote(None), true), "linear commentCreate agentSessionId=s-1");
    let failed = wrote(Some(unavailable()));
    assert_eq!(failed.line().show, Show::Always);
    assert_eq!(mark(wrote(Some(unavailable()))), Mark::Error);
    assert_eq!(shown(failed, false), "linear commentCreate: failed (unavailable)");
    assert_eq!(shown(wrote(Some(unavailable())), true), "linear commentCreate agentSessionId=s-1: failed: worktree r::/w/tw-1 didn't answer");
    // Git's error can carry a credential: never shown.
    assert_eq!(shown(Call::Git { site: "git push".into(), detail: "origin tw-1".into(), ok: false }, true), "git push origin tw-1: failed");
    assert_eq!(shown(Call::Slow { site: "linear Recent".into(), took: Duration::from_millis(2100) }, false), "slow: linear Recent took 2.1 s");
    let low = Call::BudgetLow { api: "github", budget: "graphql".into(), remaining: 999, limit: 5000, resets_in: Duration::from_secs(600) };
    assert_eq!((low.line().area, shown(low, false)), (Some("github"), "graphql budget low: 999 of 5000 left, resets in 600 s".into()));
    let cost = Cost { api: "github", site: "checks".into(), resource: Some("graphql".into()), counts: vec![("cost", "1".into()), ("remaining", "5".into())], log_env: "TOLKSTER_LOG_GITHUB" };
    assert_eq!(cost.to_string(), "github: checks graphql cost=1 remaining=5");
    let cost = Call::Cost(cost);
    assert_eq!((cost.line().area, cost.line().show), (Some("github"), Show::Verbose));
    assert_eq!(shown(Call::Inbound { job: "base:o/r@abc".into() }, true), "in base:o/r@abc");
}

#[test]
fn work_says_what_came_of_a_command_and_names_its_issue() {
    assert_eq!(both(Work::Failed(Error::Orca("orca said no".into()))), ("failed (Orca)".into(), "failed: orca said no".into()));
    assert_eq!(shown(Work::NoWorkerRecord { issue: "TW-1".into(), pr: Some(7) }, false), "TW-1's worktree has no worker's record for PR #7");
    assert_eq!(shown(Work::NoWorktree { issue: "TW-1".into(), purpose: Purpose::Branch(7) }, false), "TW-1 has no worktree branch for PR #7");
    assert_eq!(shown(Work::NotItsWorktree { path: "/w/tw-2".into(), issue: "TW-1".into() }, false), "/w/tw-2 isn't a worktree of TW-1");
    assert_eq!(both(Work::NoNoteThread { session_id: Id::from("s-1"), issue: "TW-1".into() }).0, "a session is no note's thread on TW-1, so nothing said");
    let started = Work::Started { mode: Mode::PlanFirst, rung: "Opus".into(), effort: "high".into(), worktree: "tw-1".into(), host: Some("mac".into()), problems: vec![Problem::NotNamed(unavailable()), Problem::Rung(RungNote::Agent)] };
    assert_eq!(shown(started, false), "started a worker (work, planning first, Opus, high effort) in tw-1 on mac; naming it failed (unavailable); an agent: label names a harness no rung runs on; ignored");
    assert_eq!(shown(Work::InThread(Turn::Opened { behind: Some(unavailable()) }), false), "opened a turn in the thread; refreshing the conversations checkout failed, so the turn is told it may be behind (unavailable)");
    let retrying = Work::Retrying { issue: Some("TW-7".into()), kind: Kind::Command("readIssue".into()), job: "command:c-1".into(), wait: Duration::from_secs(5) };
    assert_eq!(retrying.line().issue.as_deref(), Some("TW-7"));
    assert_eq!(both(retrying), ("readIssue: trying again in 5 s".into(), "readIssue command:c-1: trying again in 5 s".into()));
    assert_eq!(mark(Work::UpdateRefused { repo: "o/r".into(), pr: 7 }), Mark::Warning);
    let kept = Work::ConfigKept(Error::Usage("\"slots\" in config.json must map host names".into()));
    assert_eq!(both(kept), ("config.json wasn't applied: it didn't read or failed its checks, so the last config stays in use (bad input)".into(), "config.json wasn't applied: it didn't read or failed its checks, so the last config stays in use: \"slots\" in config.json must map host names".into()));
    assert_eq!(shown(Work::ConfigChanged { changes: vec!["repos.r.trace = true".into(), "slots.local = 2".into()] }, false), "config.json changed: repos.r.trace = true; slots.local = 2");
}

#[test]
fn a_queued_line_names_what_the_request_waits_for() {
    let queued = |waiting: machine::Waiting, blocked_by: &[&str]| shown(Work::Queued { waiting, blocked_by: blocked_by.iter().map(|b| b.to_string()).collect() }, false);
    assert_eq!(queued(machine::Waiting::Slot, &[]), "queued: waiting for a slot");
    assert_eq!(queued(machine::Waiting::Held, &[]), "queued: waiting for a held worker");
    assert_eq!(queued(machine::Waiting::Merged, &[]), "queued: waiting for a reap");
    assert_eq!(queued(machine::Waiting::Blockers, &["TW-683", "TW-685"]), "queued: blocked by TW-683, TW-685");
    assert_eq!(queued(machine::Waiting::Blockers, &["TW-1", "TW-2", "TW-3", "TW-4", "TW-5", "TW-6", "TW-7"]), "queued: blocked by TW-1, TW-2, TW-3, TW-4, TW-5 and 2 more");
    assert_eq!(queued(machine::Waiting::Blockers, &[]), "queued: blocked");
}

#[test]
fn an_outcome_sums_up_its_job_with_its_mark() {
    for (outcome, word, sign) in [
        (Outcome::Answered(machine::Outcome::Ok), "ok", Mark::Quiet),
        (Outcome::Answered(machine::Outcome::Held), "held", Mark::Routine),
        (Outcome::Answered(machine::Outcome::Gone), "gone", Mark::Routine),
        (Outcome::Answered(machine::Outcome::Retryable), "retryable", Mark::Warning),
        (Outcome::Answered(machine::Outcome::Failed), "failed", Mark::Error),
        (Outcome::Awaits, "waits for a machine", Mark::Routine),
        (Outcome::Unconfirmed, "typed, not confirmed", Mark::Warning),
        (Outcome::Reported, "reported", Mark::Quiet),
        (Outcome::NothingToDo, "nothing to do", Mark::Quiet),
        (Outcome::Panicked, "panicked", Mark::Error),
    ] {
        let line = outcome.line();
        assert_eq!((line.parts.render(false), line.mark, line.show), (word.into(), sign, Show::Outcome));
    }
    assert_eq!(shown(Outcome::Error(unavailable()), false), "error (unavailable)");
}

// A push to Tolkster's own main says nothing of a branch already at its head and built.
#[test]
fn a_push_that_changes_nothing_logs_its_outcome_alone() {
    assert_eq!(Update::At { head: "b41c2f0".into() }.line().show, Show::Verbose);
    assert_eq!(Update::BuiltAlready { head: "b41c2f0".into() }.line().show, Show::Verbose);
    assert_eq!(Update::Moved { from: "a".into(), head: "b".into() }.line().show, Show::Always);
}

#[test]
fn a_terminal_exit_leads_with_its_issue_and_names_its_handle_only_verbose() {
    let waited = |issue: Option<&str>, said| Event::from(Watch::Waited { issue: issue.map(String::from), handle: "term_ec09".into(), said }).line();
    let line = waited(Some("TW-749"), Closed::Exited(Some(1)));
    assert_eq!((line.issue.as_deref(), line.area, line.mark), (Some("TW-749"), None, Mark::Routine));
    assert_eq!((line.parts.render(false), line.parts.render(true)), ("agent terminal closed (exit code 1)".into(), "agent terminal term_ec09 closed (exit code 1)".into()));
    let line = waited(None, Closed::Stale);
    assert_eq!((line.issue, line.area, line.parts.render(false)), (None, Some("watch"), "a terminal is gone".into()));
}

#[test]
fn triage_context_and_reap_say_what_they_did_by_issue() {
    let notes = vec![TriageNote::TriagedAlready(1), TriageNote::History { issue: "TW-2".into(), error: unavailable(), rest_wait: true }, TriageNote::Started(vec!["TW-1".into(), "TW-3".into()])];
    assert_eq!(shown(Triage::Started(notes), false), "triage: 1 triaged already; TW-2: its history couldn't be read (unavailable), so the rest wait; started triage for TW-1, TW-3");
    assert_eq!(shown(Triage::Started(Vec::new()), false), "triage: nothing to triage");
    assert_eq!(shown(Triage::Refused { issue: "TW-4".into(), why: Refusal::Archived }, false), "triage: TW-4 is archived, so no triage");
    assert_eq!(shown(Context::Started { issue: "TW-10".into(), issues: vec!["TW-11".into(), "TW-12".into()] }, false), "TW-10: started context for TW-11, TW-12");
    let mut why = Why::of(Rule::Merged(7));
    why.record_stays = Some(unavailable());
    let removed = Reap { issue: "TW-1".into(), what: Reaping::Removed { path: "/w/tw-1".into(), branch: "tw-1".into(), why, branch_kept: false } };
    assert_eq!(removed.to_string(), "TW-1: removed /w/tw-1 and branch tw-1 (PR #7 merged; its worker record stays: worktree r::/w/tw-1 didn't answer)");
    // Gone with its worker record left: half done, so marked.
    assert_eq!(mark(Reap { issue: "TW-1".into(), what: Reaping::GoneFromHost { path: "/w/tw-1".into(), host: "mac".into(), why: Why { rule: Rule::Completed, closed_agent: false, record_stays: Some(unavailable()) } } }), Mark::Error);
    assert_eq!(mark(Reap { issue: "TW-1".into(), what: Reaping::GoneFromHost { path: "/w/tw-1".into(), host: "mac".into(), why: Why::of(Rule::Completed) } }), Mark::Routine);
    assert_eq!(shown(removed, false), "TW-1: removed /w/tw-1 and branch tw-1 (PR #7 merged; its worker record stays (unavailable))");
    let kept = Reap { issue: "TW-1".into(), what: Reaping::Kept { path: "/w/tw-1".into(), rule: Rule::Cancelled, because: Keep::Unpushed(2) } };
    assert_eq!(kept.to_string(), "TW-1: kept /w/tw-1: issue cancelled, but its branch is on origin with work, so a PR may be open, and 2 commits on HEAD or the branch are on no remote");
}

#[test]
fn listens_own_lines_lead_with_their_area() {
    let reconnecting = Realtime::Reconnecting { ended: Ended::Status(502), delay: Duration::from_secs(5) };
    let line = reconnecting.line();
    assert_eq!((line.area, line.mark, line.show), (Some("realtime"), Mark::Warning, Show::Notice));
    assert_eq!(line.parts.render(false), "the Worker answered HTTP 502; reconnecting in 5 s");
    let unset = Realtime::Reconnecting { ended: Ended::Unset(Setting::OwnerUserId), delay: Duration::from_secs(5) };
    assert_eq!(unset.line().parts.render(false), "the Worker has no OWNER_USER_ID set; reconnecting in 5 s");
    let failed = Realtime::Reconnecting { ended: Ended::failed(Step::Ping, "broken pipe"), delay: Duration::from_secs(5) };
    assert_eq!(both(failed), ("sending a ping failed (unavailable); reconnecting in 5 s".into(), "sending a ping failed: broken pipe; reconnecting in 5 s".into()));
    assert_eq!((Realtime::Superseded.line().mark, Realtime::Superseded.line().area), (Mark::Warning, Some("realtime")));
    assert_eq!(mark(Realtime::Connected { again: true }), Mark::Recovered);
    assert_eq!(mark(Realtime::Connected { again: false }), Mark::Routine);
    assert_eq!(Watch::EndedAtOnce.line().area, Some("watch"));
    assert_eq!(both(Host::NotAnswering { name: Some("mac".into()), id: "env-1".into(), error: unavailable() }).0, "mac not answering (unavailable)");
    assert_eq!(Update::Building.line().area, Some("update"));
    assert_eq!(shown(Work::Started { mode: Mode::Work, rung: "Opus".into(), effort: "high".into(), worktree: "tw-1".into(), host: None, problems: Vec::new() }, false), "started a worker (work, Opus, high effort) in tw-1 on this machine");
    assert_eq!(shown(Update::Moved { from: "a3d0e65".into(), head: "b41c2f0".into() }, false), "main a3d0e65 -> b41c2f0");
    assert_eq!(shown(Update::Off(Unfit::Worktree { path: "/w/tw-1".into() }), false), "/w/tw-1 is a worktree, not the primary checkout; won't update itself");
    assert_eq!(shown(Update::Skipped(Unpulled::Fetch(128)), false), "skipped: fetch failed with 128; run git fetch in the checkout to see why");
    // A stuck run's restart isn't the update's.
    let stuck = Update::Restart(Restart::Stuck { what: Stuck::Listing, minutes: 10 });
    assert_eq!((stuck.line().area, shown(stuck, false)), (None, "Orca's listing has made no progress for 10 minutes or more; starting again".into()));
}

#[test]
fn only_ids_enums_and_flags_are_picked_from_a_request() {
    let activity = json!({ "input": { "agentSessionId": "s-1", "content": { "type": "thought", "body": "Perched, watching the checks…" }, "ephemeral": true } });
    assert_eq!(ids(&activity), "agentSessionId=s-1 type=thought ephemeral=true");
    let take = json!({ "op": "take", "claims": [{ "name": "request-s-1@2026", "ttlMs": 900000 }, { "name": "start-TW-1", "ttlMs": null }], "token": "t-secret" });
    assert_eq!(ids(&take), "name=request-s-1@2026 name=start-TW-1");
    let release = json!({ "op": "release", "claims": ["request-s-1@2026", "start-TW-1"], "token": "t-secret" });
    assert_eq!(ids(&release), "claims=request-s-1@2026 claims=start-TW-1");
    // A list under a key that isn't shown, such as every worktree listed, stays out.
    assert_eq!(ids(&json!({ "op": "report", "report": "listing", "listed": ["r::/w/tw-1", "r::/w/tw-2"] })), "");
    let issue = json!({ "id": "i-1", "input": { "stateId": "st-1", "title": "A title", "description": "Text" } });
    assert_eq!(ids(&issue), "id=i-1 stateId=st-1");
    assert_eq!(ids(&json!({ "body": "text", "plan": [{ "content": "Read", "status": "completed" }] })), "status=completed");
    assert_eq!(ids(&json!("text")), "");
}

#[test]
fn a_time_is_shown_in_seconds_to_a_tenth() {
    assert_eq!(seconds(Duration::from_millis(61_240)), "61.2 s");
    assert_eq!(seconds(Duration::from_millis(40)), "0.0 s");
}

// A recipe line's stderr is bounded in the disk log: its last lines, each cut short.
#[test]
fn a_recipe_s_stderr_keeps_its_last_lines_each_cut_short() {
    let text: String = (1..=30).map(|n| format!("line {n}\n\n")).collect::<String>() + &"x".repeat(1000);
    let kept = tail(&text);
    assert_eq!(kept.len(), TAIL_LINES);
    assert_eq!(kept[0], "line 12");
    assert_eq!(kept[TAIL_LINES - 1].chars().count(), TAIL_WIDTH);
    assert!(tail("  \n").is_empty());
}

// Each step of a recipe machine's life is said as it begins, by names, repo keys, counts and
// issue identifiers; a decline or a failed destroy says why, and a destroy reaches the owner's
// stderr too, as `host rm` has no log.
#[test]
fn a_recipe_machine_s_life_is_said_step_by_step() {
    let said = |event: Host| {
        let line = event.line();
        assert_eq!(line.area, Some("host"));
        line.parts.render(false)
    };
    assert_eq!(said(Host::Making { repo: "tolkster".into(), name: "tolkster-3".into(), counted: 3, machines: 3, issue: "fleet.9".into() }), "making tolkster-3 from tolkster's recipe, machine 3 of 3, for fleet.9");
    assert_eq!(said(Host::Waking { name: "tolkster-2".into(), issue: "fleet.9".into() }), "waking tolkster-2 for fleet.9");
    assert_eq!(said(Host::Sleeping { name: "tolkster-2".into() }), "putting tolkster-2 to sleep");
    assert_eq!(said(Host::NotSleeping { name: Some("tolkster-2".into()), id: "env-2".into(), why: Unslept::Busy }), "tolkster-2 not put to sleep: an agent works on it, or it changed");
    assert_eq!(said(Host::NotSleeping { name: None, id: "env-2".into(), why: Unslept::NotAnswering }), "a host not put to sleep: it isn't answering");
    assert_eq!(said(Host::NotMaking { repo: "tolkster".into(), why: Declined::CoolDown }), "no machine made from tolkster's recipe: cooling down after a failure");
    assert_eq!(said(Host::NotWaking { name: "tolkster-2".into(), why: Declined::Running }), "tolkster-2 not woken: a make, wake or sleep is running");
    for (event, mark) in [(Host::Destroying { name: "tolkster-3".into() }, Mark::Routine), (Host::Destroyed { name: "tolkster-3".into() }, Mark::Routine), (Host::NotDestroyed { name: "tolkster-3".into() }, Mark::Error)] {
        let line = event.line();
        assert_eq!((line.mark, line.show, line.area), (mark, Show::Notice, Some("host")));
    }
    assert_eq!(said(Host::NotDestroyed { name: "tolkster-3".into() }), "tolkster-3 not destroyed: its recipe's destroy failed");
}
