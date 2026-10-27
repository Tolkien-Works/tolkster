use super::*;
use crate::brief::review_size;
use crate::review_pass::ReviewSize;
use crate::request::Issue;
use crate::estimates::Size;

// A plain value, for the life of the test.
fn plain<T: From<&'static str>>(value: &'static str) -> &'static T {
    Box::leak(Box::new(T::from(value)))
}

// A claim's decision, for the life of the test.
fn taken(plan_first: bool, orchestrate: bool, auto_merge: bool) -> &'static Taken {
    Box::leak(Box::new(Taken { plan_first, orchestrate, auto_merge, worktree_name: "tw-7".into(), rung: { let ladder = crate::config::ladder::Ladder::default(); let (rung, notes) = ladder.choose(None, &[]); (rung.clone(), notes) }, scale: Some(crate::estimates::Scale { kind: crate::estimates::Kind::TShirt, extended: false, zero: false }), review: ReviewSize::Unsized, plan_from: None }))
}

fn brief(plan_first: bool, reviewers: &[String]) -> Brief<'_> {
    Brief {
        identifier: plain("TW-7"),
        session_id: plain(SESSION),
        branch_name: plain("sam/tw-7-add-a-thing"),
        repo: plain("tolkster"),
        taken: taken(plan_first, false, true),
        reconcile: None,
        copilot: CopilotReview::OnEachPush,
        owner_name: "Sam",
        reviewers,
        startup: Startup::Pushed,
        land: Land::Pr,
    }
}

const SESSION: &str = "0a1b2c3d-0000-4000-8000-000000000001";
const COMMENT: &str = "0a1b2c3d-0000-4000-8000-00000000c001";

// Placeholders the brief's reader fills in themselves, such as `<n>` for a PR number.
fn left_for_the_worker(text: &str) -> Vec<String> {
    let mut found: Vec<String> = text.split('<').skip(1).filter_map(|rest| rest.split_once('>')).map(|(name, _)| name.to_string()).collect();
    found.sort();
    found.dedup();
    found
}

fn turn() -> TurnBrief<'static> {
    TurnBrief { identifier: plain("TW-7"), session_id: plain(SESSION), repo: plain("tolkster"), owner_name: "Sam" }
}

fn thread() -> ThreadBrief<'static> {
    ThreadBrief { identifier: plain("TW-7"), comment_id: plain(COMMENT), repo: plain("tolkster"), owner_name: "Sam" }
}

fn triage() -> String {
    let start = crate::triage::Start { candidates: &[], batch: 5, owner_name: "Sam", briefs_dir: "/briefs/<repo>", repos: &crate::config::repo_map::RepoMap::default() };
    start.render(plain("tolkster"), &["TW-7".into(), "TW-8".into()], plain(SESSION), &[])
}

fn context() -> String {
    let closed = crate::context::Closed { identifier: "TW-7".into(), completed: true, place: Default::default(), open_related: vec!["TW-8".into()], open_siblings: Vec::new() };
    let start = crate::context::Start { closed: &closed, owner_name: "Sam", briefs_dir: "/briefs/<repo>", repos: &crate::config::repo_map::RepoMap::default(), at: plain("2026-10-04T01:00:00.000Z") };
    start.render("tolkster", &crate::git::Merge::Absent)
}

// Every brief, in each variant the claim, the startup and Copilot's review render.
fn briefs(reviewers: &[String]) -> Vec<(&'static str, String)> {
    let sized = |review| &*Box::leak(Box::new(Taken { review, ..taken(false, false, true).clone() }));
    vec![
        ("work", brief(false, reviewers).render()),
        ("plan-first", brief(true, reviewers).render()),
        ("children", Brief { taken: taken(true, true, true), ..brief(true, reviewers) }.render()),
        ("manual merge", Brief { taken: taken(false, false, false), ..brief(false, reviewers) }.render()),
        ("direct", Brief { land: Land::Direct, ..brief(false, reviewers) }.render()),
        ("direct deep", Brief { land: Land::Direct, ..brief(false, reviewers) }.render_beside(Beside::Deep)),
        ("reconcile", Brief { reconcile: Some(landed(2)), ..brief(false, reviewers) }.render()),
        ("reconcile and plan first", Brief { reconcile: Some(landed(2)), ..brief(true, reviewers) }.render()),
        ("unpushed", Brief { startup: Startup::Unpushed, ..brief(false, reviewers) }.render()),
        ("unreviewed", Brief { copilot: CopilotReview::Off, ..brief(false, reviewers) }.render()),
        ("opening only", Brief { copilot: CopilotReview::OnOpening, ..brief(false, reviewers) }.render()),
        ("small unreviewed", Brief { copilot: CopilotReview::Off, taken: sized(ReviewSize::Below), ..brief(false, reviewers) }.render()),
        ("medium unreviewed", Brief { copilot: CopilotReview::Off, taken: sized(ReviewSize::At), ..brief(false, reviewers) }.render()),
        ("large", Brief { taken: sized(ReviewSize::Above), ..brief(false, reviewers) }.render()),
        ("turn", turn().render()),
        ("thread", thread().render()),
        ("triage", triage()),
        ("round", brief(false, reviewers).render_beside(Beside::Round)),
        ("round unreviewed", Brief { copilot: CopilotReview::Off, ..brief(false, reviewers) }.render_beside(Beside::Round)),
        ("round opening only", Brief { copilot: CopilotReview::OnOpening, ..brief(false, reviewers) }.render_beside(Beside::Round)),
        ("rebase", brief(false, reviewers).render_beside(Beside::Rebase)),
        ("deep", brief(false, reviewers).render_beside(Beside::Deep)),
        ("reviewer", brief(false, reviewers).render_beside(Beside::Reviewer)),
        ("last round", brief(false, reviewers).render_beside(Beside::LastRound)),
    ]
}

// A partial's first line, which says whether a brief carries it.
fn first(partial: &str) -> &str {
    partial.lines().next().unwrap()
}

#[test]
fn every_placeholder_is_filled_in() {
    let logins = vec!["a-reviewer".to_string(), "copilot-pull-request-reviewer".to_string()];
    for (which, text) in briefs(&logins).into_iter().chain([("context", context())]) {
        let partials = PARTIALS.into_iter().chain(SESSION_PARTIALS).map(|(placeholder, _)| placeholder);
        let filled = ["<identifier>", "<sessionId>", "<commentId>", "<session>", "<branchName>", "<repo displayName>", "<OWNER NAME>", "<REVIEWERS>", "<answer>", MERGE, AUTO_MERGE_PLACEHOLDER, FINISH, GATE, VERDICT, REVIEW_PASS, ROUND_PUSH, STARTUP, SIZING, APPROVED_PUSH, DENIED_PLACEHOLDER];
        for placeholder in partials.chain(filled) {
            assert!(!text.contains(placeholder), "{placeholder} in {which}");
        }
        for name in left_for_the_worker(&text) {
            assert!(["n", "PR url", "path", "handle", "ID", "text", "p", "name", "KEY", "other ID", "TW-123", "TW-n", "TW-m", "createdAt", "TW-124", "paths", "owner/name", "command", "sub-issue", "blocker", "new", "reason", "skipped", "sha", "line"].contains(&name.as_str()), "<{name}> in {which}");
        }
    }
    let round = brief(false, &logins).render_beside(Beside::Round);
    assert!(round.contains(&reviewers(&logins)) && brief(false, &[]).render_beside(Beside::Round).contains(&reviewers(&[])));
}

#[test]
fn each_brief_takes_its_partials() {
    // The work brief reports progress and closes its terminal; the round brief judges
    // follow-ups against the rubric; it and the rebase brief push and rebase alike.
    let text = brief(false, &[]).render();
    let round = brief(false, &[]).render_beside(Beside::Round);
    let rebase = brief(false, &[]).render_beside(Beside::Rebase);
    for (placeholder, partial) in PARTIALS {
        let first = first(partial);
        // The hold and the approval are read only where plan first or reconcile applies.
        let (work, rounds, rebases) = match placeholder {
            "<HOLD>" | "<APPROVED>" => (0, 0, 0),
            p if p == RUBRIC.0 => (0, 1, 0),
            p if p == ROUND_GIT.0 => (0, 1, 1),
            _ => (1, 0, 0),
        };
        assert_eq!((text.matches(first).count(), round.matches(first).count(), rebase.matches(first).count()), (work, rounds, rebases), "{placeholder}");
    }
    // A turn has its own progress lines and closes no terminal: it ends by itself. It files no
    // follow-ups, so it has no rubric. Every session posts by the same rules.
    for text in [turn().render(), thread().render()] {
        for partial in [REPORT_PROGRESS.1, CLOSE_TERMINAL.1, RUBRIC.1] {
            assert!(!text.contains(first(partial)), "{}", first(partial));
        }
    }
    for text in [turn().render(), thread().render(), triage(), context()] {
        assert_eq!(text.matches(first(POSTING.1)).count(), 1);
    }
}

// Auto-merge is turned on with the PR, by `pr create` (TW-619).
#[test]
fn the_merge_step_follows_the_claim() {
    let render = |auto_merge| Brief { taken: taken(false, false, auto_merge), ..brief(false, &[]) }.render();
    let (auto, manual) = (render(true), render(false));
    assert!(auto.contains(&format!("--session {SESSION}{AUTO_MERGE_FLAG}`")) && !manual.contains(AUTO_MERGE_FLAG));
    assert!(auto.contains(first(AUTO_MERGE)) && !auto.contains(first(MANUAL_MERGE)));
    assert!(manual.contains(&first(MANUAL_MERGE).replace("<OWNER NAME>", "Sam")) && !manual.contains(first(AUTO_MERGE)));
}

// A direct repo lands with `tolkster land` after a gate that leaves nothing to CI, unless the
// issue is left to the owner's merge, which opens a PR as a PR repo does.
#[test]
fn a_direct_repo_lands_unless_the_owner_merges() {
    let render = |land, auto_merge| Brief { land, taken: taken(false, false, auto_merge), ..brief(false, &[]) }.render();
    let open_pr = first(OPEN_PR).replace("<OWNER NAME>", "Sam");
    let direct = render(Land::Direct, true);
    assert!(direct.contains(&format!("`tolkster land --session {SESSION} ")) && direct.contains("`--all`"));
    assert!(!direct.contains(&open_pr) && !direct.contains(first(AUTO_MERGE)) && !direct.contains(AUTO_MERGE_FLAG));
    for (land, auto_merge) in [(Land::Pr, true), (Land::Pr, false), (Land::Direct, false)] {
        let text = render(land, auto_merge);
        assert!(text.contains(&open_pr) && !text.contains("tolkster land") && !text.contains("`--all`"), "{land:?} {auto_merge}");
    }
}

#[test]
fn plan_first_goes_right_after_the_task() {
    let text = brief(true, &[]).render();
    let plan = first(PLAN_FIRST);
    assert!(text.find(plan).unwrap() < text.find(AFTER_THE_TASK).unwrap());
    assert_eq!(text.matches(plan).count(), 1);
    assert!(!brief(false, &[]).render().contains(plan));
    // A conversation's turn never plans first.
    assert!(!turn().render().contains(plan));
}

#[test]
fn a_parent_is_worked_through_its_sub_issues() {
    let text = Brief { taken: taken(true, true, true), reconcile: Some(landed(2)), ..brief(true, &[]) }.render();
    let at = |heading: &str| text.find(heading).unwrap();
    let (reconcile, plan, children) = (first(RECONCILE), first(PLAN_FIRST), first(CHILDREN));
    assert!(at(reconcile) < at(plan) && at(plan) < at(children) && at(children) < at(AFTER_THE_TASK));
    // An orchestrating parent has no branch of its own to push on the owner's go.
    assert!(!text.contains("git push -u origin"));
    assert!(!brief(false, &[]).render().contains(children));
    assert!(!turn().render().contains(children));
}

fn landed(commits: u32) -> Reconcile {
    Reconcile { baseline: "2026-09-26T01:00:00.000Z".into(), from: Some("aaaa111100000000000000000000000000000000".into()), to: "bbbb222200000000000000000000000000000000".into(), commits }
}

// The section is the template with the time, the range and the count filled in, and the
// brief's own values: nothing from the issue or the commits.
#[test]
fn reconcile_first_comes_only_with_commits_since_the_baseline() {
    let template = PARTIALS.into_iter().fold(RECONCILE.to_string(), |text, (placeholder, partial)| text.replace(placeholder, partial.trim_end())).replace(APPROVED_PUSH, APPROVED_PUSH_LINE);
    for (reconcile, commits) in [(landed(3), "3 commits have"), (landed(1), "1 commit has"), (Reconcile { from: None, ..landed(2) }, "2 commits have")] {
        let text = Brief { reconcile: Some(reconcile.clone()), ..brief(false, &[]) }.render();
        let section = text[text.find(first(RECONCILE)).unwrap()..text.find(AFTER_THE_TASK).unwrap()]
            .replace(reconcile.baseline.as_str(), "<baseline>")
            .replace(commits, "<commits>")
            .replace(&reconcile.range(), "<range>")
            .replace("TW-7", "<identifier>")
            .replace("sam/tw-7-add-a-thing", "<branchName>")
            .replace(SESSION, "<sessionId>")
            .replace("Sam", "<OWNER NAME>");
        assert_eq!(section.trim_end(), template.trim_end());
    }
    // All of it is from the range: `to` alone when nothing is older than the baseline.
    assert_eq!(Reconcile { from: None, ..landed(2) }.range(), "bbbb222200000000000000000000000000000000");
    // Before the plan, whose elicitation then covers the drift too.
    let text = Brief { reconcile: Some(landed(3)), ..brief(true, &[]) }.render();
    assert!(text.find(first(RECONCILE)).unwrap() < text.find(first(PLAN_FIRST)).unwrap());
    // Nothing landed, or a conversation: no section.
    assert!(!brief(false, &[]).render().contains(first(RECONCILE)));
    assert!(!turn().render().contains(first(RECONCILE)));
}

#[test]
fn only_plain_values_are_taken() {
    assert!(brief(false, &[]).check().is_ok());
    assert!(Brief { repo: plain("transfer-collect (C&C)"), ..brief(false, &[]) }.check().is_ok());
    for repo in ["..", "a/b", "a\\b", "C:x", "", "a\"b", "$(x)"] {
        assert!(RepoName::parse(repo).is_none(), "{repo}");
    }
    for branch_name in ["", "a b", "a;b", "a..b", "-x", "a`b"] {
        assert!(BranchName::parse(branch_name).is_none(), "{branch_name}");
    }
    for identifier in ["TW 7", "TW-+5", "7-1", "TW-1234567890"] {
        assert!(IssueKey::parse(identifier).is_none(), "{identifier}");
    }
    assert!(Id::parse("s 1").is_none());
    assert!(Brief { reconcile: Some(landed(1)), ..brief(false, &[]) }.check().is_ok());
    assert!(Brief { reconcile: Some(Reconcile { from: None, ..landed(1) }), ..brief(false, &[]) }.check().is_ok());
    for bad in ["HEAD", "a..b"] {
        assert!(Sha::parse(bad).is_none(), "{bad}");
    }
    for bad in ["--all", "yesterday"] {
        assert!(Time::parse(bad).is_none(), "{bad}");
    }
}

// The issue and the request are read in the turn that reads the brief (TW-622).

#[test]
fn a_parents_first_line_names_the_sub_issues_it_may_chain() {
    let path = Path::new("/home/sam/.claude/tolkster-briefs/TW-7.md");
    let s1 = Id::parse("s1").unwrap();
    let children: [IssueKey; 3] = ["TW-8".into(), "TW-9".into(), "TW-10".into()];
    let line = prompt(path, &"TW-7".into(), &s1, Some((&children, &[]))).unwrap();
    assert!(line.ends_with(" You may chain and delegate the sub-issues TW-8, TW-9 and TW-10 of TW-7 with tolkster issue chain."), "{line}");
    assert!(!line.contains(['"', '\'', '\n']));
    let one = prompt(path, &"TW-7".into(), &s1, Some((&children[..1], &[]))).unwrap();
    assert!(one.ends_with(" the sub-issue TW-8 of TW-7 with tolkster issue chain."), "{one}");
    // Those in Triage are named as skipped.
    let triaged: [IssueKey; 2] = ["TW-11".into(), "TW-12".into()];
    let skipping = prompt(path, &"TW-7".into(), &s1, Some((&children[..1], &triaged[..1]))).unwrap();
    assert!(skipping.ends_with(" of TW-7 with tolkster issue chain. Skip TW-11: it is in Triage, so nobody has accepted it."), "{skipping}");
    let both = prompt(path, &"TW-7".into(), &s1, Some((&children[..1], &triaged))).unwrap();
    assert!(both.ends_with(" Skip TW-11 and TW-12: they are in Triage, so nobody has accepted them."), "{both}");
    // No sub-issue left to set going, those in Triage aside: the line is a worker's.
    assert_eq!(prompt(path, &"TW-7".into(), &s1, Some((&[], &triaged))).unwrap(), prompt(path, &"TW-7".into(), &s1, None).unwrap());
    // An identifier that isn't plain is never an `IssueKey`, so never reaches the terminal.
    for bad in ["TW-8; rm", "TW-8\"", "-TW-8"] {
        assert!(IssueKey::parse(bad).is_none(), "{bad}");
    }
}

#[test]
fn the_path_and_the_prompt() {
    let path = path("/home/sam/.claude/<repo>-briefs", "tolkster", "TW-7");
    assert_eq!(path, Path::new("/home/sam/.claude/tolkster-briefs/TW-7.md"));
    let s1 = Id::parse("s1").unwrap();
    let prompt = prompt(&path, &"TW-7".into(), &s1, None).unwrap();
    assert!(prompt.starts_with(&format!("Read the file {} in full and,", path.display())));
    assert!(prompt.contains("run tolkster start s1"), "{prompt}");
    assert!(!prompt.contains(['"', '\'', '\n']));
    for dir in ["/home/$(whoami)/<repo>", "/home/a`b/<repo>", "/home/\"x\"/<repo>", "/home/a\nb/<repo>"] {
        assert!(super::prompt(&super::path(dir, "tolkster", "TW-7"), &"TW-7".into(), &s1, None).is_err(), "{dir}");
    }
}

// A new worker starts reading at once (TW-257): `tolkster listen` has pushed its branch and
// posted its first thought, so "Before any work" asks for a command only for what listen held
// back.
#[test]
fn the_startup_asks_only_for_what_listen_held_back() {
    let startup = |startup| Brief { startup, ..brief(false, &[]) }.render();
    let filled = |text: &str| text.replace("<branchName>", "sam/tw-7-add-a-thing").replace("<OWNER NAME>", "Sam");
    let (pushed, unpushed) = (startup(Startup::Pushed), startup(Startup::Unpushed));
    assert!(pushed.contains(&filled(first(STARTUP_DONE))) && !pushed.contains(&filled(first(STARTUP_PUSH))));
    assert!(unpushed.contains(&filled(first(STARTUP_PUSH))) && !unpushed.contains(first(STARTUP_DONE)));
}

#[test]
fn the_sizing_step_gives_the_teams_own_scale_or_none() {
    use crate::estimates::{Kind, Scale};
    let on = |scale: Option<Scale>| {
        let taken = Taken { scale, ..taken(false, false, true).clone() };
        Brief { taken: &taken, ..brief(false, &[]) }.render()
    };
    let used = first(SIZING_USED);
    for scale in [Scale { kind: Kind::TShirt, extended: false, zero: false }, Scale { kind: Kind::Exponential, extended: true, zero: false }] {
        let text = on(Some(scale));
        assert!(text.contains(used) && text.contains(&scale.describe().unwrap()) && !text.contains(first(SIZING_NONE)));
    }
    // No estimates, or a scale that wasn't read: no sizing, and still a step 3.
    for text in [on(Some(Scale { kind: Kind::NotUsed, extended: false, zero: false })), on(None)] {
        assert!(text.contains(SIZING_NONE.trim_end()) && !text.contains(used));
    }
}

// A plan only where there was planning: a small issue's repeats the hook's phases.
#[test]
fn a_plan_is_kept_only_for_planned_sub_issued_or_larger_work() {
    use crate::estimates::{Kind, Scale};
    let on = |plan_first: bool, orchestrate: bool, review: ReviewSize, plan_from: Option<u32>| {
        let taken = Taken { review, plan_from, ..taken(plan_first, orchestrate, true).clone() };
        Brief { taken: &taken, ..brief(plan_first, &[]) }.render()
    };
    let (kept, none) = (format!("{PLAN_KEPT} "), PLAN_NONE);
    for text in [on(true, false, ReviewSize::Below, None), on(false, true, ReviewSize::Below, None), on(false, false, ReviewSize::At, None), on(false, false, ReviewSize::Above, None)] {
        assert!(text.contains(&kept) && !text.contains(none) && !text.contains(PLAN));
    }
    for text in [on(false, false, ReviewSize::Below, Some(3)), on(false, false, ReviewSize::Unsized, None)] {
        assert!(text.contains(none) && !text.contains(&kept));
    }
    assert!(on(false, false, ReviewSize::Unsized, Some(5)).contains(&PLAN_IF_SIZED.replace("<points>", "5")));
    // The points are the smallest on the team's scale at or above the threshold.
    let t_shirt = Scale { kind: Kind::TShirt, extended: false, zero: false };
    assert_eq!((plan_from(Some(t_shirt), Size::M), plan_from(Some(t_shirt), Size::L)), (Some(3), Some(5)));
    assert_eq!(plan_from(Some(Scale { kind: Kind::NotUsed, ..t_shirt }), Size::M), None);
    assert_eq!(plan_from(None, Size::M), None);
}

#[test]
fn a_parents_sizing_step_counts_its_own_work_never_its_sub_issues() {
    use crate::estimates::{Kind, Scale};
    let on = |orchestrate: bool, scale: Scale| {
        let taken = Taken { scale: Some(scale), ..taken(false, orchestrate, true).clone() };
        Brief { taken: &taken, ..brief(false, &[]) }.render()
    };
    let parent = |smallest: &str| SIZING_PARENT.replace("<smallest estimate>", smallest).replace("<identifier>", "TW-7");
    let t_shirt = Scale { kind: Kind::TShirt, extended: false, zero: false };
    assert!(on(true, t_shirt).contains(parent("1").trim_end()));
    // The smallest is 0 where the team allows a zero estimate.
    assert!(on(true, Scale { zero: true, ..t_shirt }).contains(parent("0").trim_end()));
    // A brief that doesn't orchestrate is unchanged.
    assert_eq!(on(false, t_shirt), brief(false, &[]).render());
}

#[test]
fn a_session_opens_in_the_primary_checkout_on_one_plain_line() {
    use crate::ports::fake::{Orca as FakeOrca, World};
    let path = path("/briefs/<repo>", "tolkster", "TW-10-context");
    let prompt = reading_prompt(&path, true, &[]).unwrap();
    assert!(prompt.starts_with("Read the file ") && prompt.contains("TW-10-context.md"));
    let (mut orca, mut written) = (FakeOrca::on(&World::two_repos(true)), Vec::new());
    let mut write = |p: &Path, text: &str| {
        written.push((p.display().to_string(), text.to_string()));
        Ok(())
    };
    let terminal = open_session(&mut orca, &mut write, "r1::/src/tolkster", "TW-10 context", &path, &prompt, "the brief").unwrap();
    assert_eq!(terminal, "term_1");
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].1, "the brief");
    let create = orca.world.borrow().opened_as_asked().into_iter().next().unwrap();
    assert!(create.contains(&format!("--worktree id:r1::/src/tolkster --title TW-10 context --command claude \"{prompt}\"")), "{create}");
    // A line that can't be typed as it is is never built, so nothing opens or is written.
    assert!(reading_prompt(Path::new("/briefs/$x/TW-10-context.md"), true, &[]).is_err());
    // A brief that can't be written opens nothing, where a terminal would open.
    let mut orca = FakeOrca::on(&World::two_repos(true));
    assert!(open_session(&mut orca, &mut |_, _| Err(Error::Failed("disk".into())), "r1::/src/tolkster", "t", &path, &prompt, "x").is_err());
    assert!(orca.world.borrow().opened.is_empty());
}

#[test]
fn a_session_reads_its_issues_in_the_turn_it_reads_the_brief() {
    let path = path("/briefs/<repo>", "tolkster", "TW-10-context");
    let issues: Vec<IssueKey> = vec!["TW-10".into(), "TW-12".into()];
    let issues: Vec<&IssueKey> = issues.iter().collect();
    let prompt = reading_prompt(&path, false, &issues).unwrap();
    assert!(prompt.contains("TW-10-context.md in full and, in the same turn, run tolkster issue show TW-10 TW-12 --comments as one shell call. It is your complete task brief."), "{prompt}");
    // Triage's fetches first.
    let fetching = reading_prompt(&path, true, &issues).unwrap();
    assert!(fetching.contains("in the same turn, run git fetch origin; tolkster issue show TW-10 TW-12 --comments as one shell call."), "{fetching}");
    assert!(reading_prompt(Path::new("/briefs/$x/TW-10-context.md"), false, &[]).is_err());
}

mod repo_rules;
mod layers;
mod reviews;
mod commands;
