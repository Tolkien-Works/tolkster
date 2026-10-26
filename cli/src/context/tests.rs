use super::*;
use crate::plain::Sha;
use crate::ports::fake::{Git, Orca as FakeOrca, Records as FakeRecords, World};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use crate::state::tests::Fake;

// `issue` as CLOSED answers it, decoded first.
fn parse_closed(issue: &Value) -> Result<Closed, Error> {
    super::parse_closed(&crate::json::decode(issue.clone(), "Linear's answer to Closed")?)
}

fn other(identifier: &str, state: &str, archived: bool) -> Value {
    json!({ "identifier": identifier, "archivedAt": if archived { json!("2026-09-27T00:00:00.000Z") } else { Value::Null }, "state": { "type": state } })
}

fn issue(state: &str) -> Value {
    json!({
        "id": "i10", "identifier": "TW-10", "createdAt": "2026-09-20T01:00:00.000Z", "canceledAt": null, "archivedAt": null, "state": { "type": state },
        "team": { "key": "TW", "issueEstimationType": "notUsed", "issueEstimationExtended": false, "issueEstimationAllowZero": false }, "project": { "id": "p1", "name": "Tolkster" },
        "delegate": { "id": "app" }, "creator": { "id": "owner" }, "children": { "nodes": [] },
        "parent": { "identifier": "TW-1", "children": { "nodes": [
            other("TW-10", "completed", false),
            other("TW-20", "started", false),
            other("TW-21", "completed", false),
            other("TW-22", "canceled", false),
            other("TW-24", "unstarted", true),
            other("TW-12", "unstarted", false),
            other("tw-23", "backlog", false),
        ] } },
        "relations": { "nodes": [
            { "type": "blocks", "relatedIssue": other("TW-12", "unstarted", false) },
            { "type": "related", "relatedIssue": other("tw-11", "started", false) },
            { "type": "duplicate", "relatedIssue": other("TW-13", "unstarted", false) },
            { "type": "blocks", "relatedIssue": other("TW-14", "completed", false) },
            { "type": "related", "relatedIssue": other("TW-15", "backlog", true) },
            { "type": "blocks", "relatedIssue": null },
        ] },
        "inverseRelations": { "nodes": [
            { "type": "blocks", "issue": other("TW-9", "started", false) },
            { "type": "related", "issue": other("TW-12", "unstarted", false) },
            { "type": "similar", "issue": other("TW-16", "unstarted", false) },
            { "type": "blocks", "issue": other("TW-8", "canceled", false) },
        ] },
    })
}

#[test]
fn only_open_issues_blocking_blocked_related_or_under_the_same_parent_count() {
    let closed = parse_closed(&issue("completed")).unwrap();
    let place = Place { team: Some("TW".into()), project_id: Some("p1".into()), project: Some("Tolkster".into()) };
    assert_eq!(closed, Closed { identifier: "TW-10".into(), completed: true, place, open_related: vec!["TW-11".into(), "TW-12".into(), "TW-9".into()], open_siblings: vec!["TW-20".into(), "TW-23".into()] });
    assert!(!parse_closed(&issue("canceled")).unwrap().completed);
    let mut orphan = issue("completed");
    orphan["parent"] = Value::Null;
    assert!(parse_closed(&orphan).unwrap().open_siblings.is_empty());
}

#[test]
fn a_comment_goes_only_to_an_open_related_issue_or_sibling_of_a_completed_one() {
    let closed = parse_closed(&issue("completed")).unwrap();
    for target in ["TW-12", "tw-9", "TW-20", "TW-23"] {
        assert!(may_comment(&closed, &target.into()).is_ok(), "{target}");
    }
    for target in ["TW-13", "TW-14", "TW-15", "TW-16", "TW-8", "TW-10", "TW-1", "TW-21", "TW-22", "TW-24", "TW-99"] {
        assert!(may_comment(&closed, &target.into()).is_err(), "{target}");
    }
    let cancelled = parse_closed(&issue("canceled")).unwrap();
    assert!(may_comment(&cancelled, &"TW-12".into()).is_err());
}


// Linear's comments, by ID: the issue and the body.
#[derive(Default)]
struct Linear {
    comments: RefCell<HashMap<String, (String, String)>>,
    // Each create fails, after it is posted when `lost` is set.
    refuse: Cell<bool>,
    lost: Cell<bool>,
}

impl Comments for Linear {
    fn on_issue(&self, issue_id: &Id, comment_id: &Id) -> Result<bool, Error> {
        Ok(self.comments.borrow().get(comment_id.as_str()).is_some_and(|(issue, _)| issue == issue_id))
    }
    fn create(&self, issue_id: &Id, comment_id: &Id, body: &str) -> Result<(), Error> {
        if self.refuse.get() {
            return Err(Error::Failed("refused".into()));
        }
        assert!(self.comments.borrow_mut().insert(comment_id.to_string(), (issue_id.to_string(), body.into())).is_none(), "an ID used twice");
        if self.lost.get() {
            return Err(Error::Failed("its answer was lost".into()));
        }
        Ok(())
    }
    fn update(&self, comment_id: &Id, body: &str) -> Result<(), Error> {
        let mut comments = self.comments.borrow_mut();
        let comment = comments.get_mut(comment_id.as_str()).ok_or_else(|| Error::Failed("no such comment".into()))?;
        comment.1 = body.into();
        Ok(())
    }
}

fn t(minute: u32) -> Time {
    Time::check(&format!("2026-10-04T01:{minute:02}:00.000Z")).unwrap()
}

// Seen at `seen`, written at `at`, by the minute.
fn write(records: &FakeRecords, linear: &Linear, body: &str, seen: u32, at: u32, token: &str) -> Result<Option<Wrote>, Error> {
    write_comment(records, linear, &Id::from("i-12"), body, &t(seen), &|| Ok(t(at)), token)
}

fn bodies(linear: &Linear) -> Vec<String> {
    linear.comments.borrow().values().map(|(issue, body)| format!("{issue}: {body}")).collect()
}

#[test]
fn an_issue_has_one_context_comment_created_then_updated_in_place() {
    let (records, linear) = (FakeRecords::default(), Linear::default());
    assert_eq!(write(&records, &linear, "first", 0, 1, "t1").unwrap(), Some(Wrote::Created));
    let first = records.context_comment("i-12").unwrap().unwrap();
    assert_eq!((first.issue_id.as_str(), first.at.as_str()), ("i-12", t(1).as_str()));
    assert_eq!(write(&records, &linear, "second", 1, 2, "t2").unwrap(), Some(Wrote::Updated));
    assert_eq!(bodies(&linear), ["i-12: second"]);
    let second = records.context_comment("i-12").unwrap().unwrap();
    assert_eq!((second.comment_id, second.at), (first.comment_id, t(2).to_string()));
    // Given back each time, so the next run needn't wait.
    assert!(records.claims.borrow().is_empty());
}

#[test]
fn a_comment_written_after_the_session_read_it_is_not_written_over() {
    let (records, linear) = (FakeRecords::default(), Linear::default());
    write(&records, &linear, "from TW-10", 0, 2, "t1").unwrap();
    // Another session read the issue at minute 1, before that comment.
    assert_eq!(write(&records, &linear, "from TW-11", 1, 3, "t2").unwrap(), Some(Wrote::Changed(t(2))));
    assert_eq!(bodies(&linear), ["i-12: from TW-10"]);
    assert!(records.claims.borrow().is_empty());
    // Read again, it writes with that time.
    assert_eq!(write(&records, &linear, "from TW-10 and TW-11", 2, 3, "t2").unwrap(), Some(Wrote::Updated));
    assert_eq!(bodies(&linear), ["i-12: from TW-10 and TW-11"]);
}

#[test]
fn a_deleted_context_comment_is_replaced_and_its_record_with_it() {
    let (records, linear) = (FakeRecords::default(), Linear::default());
    write(&records, &linear, "first", 0, 1, "t1").unwrap();
    let first = records.context_comment("i-12").unwrap().unwrap().comment_id;
    linear.comments.borrow_mut().clear();
    assert_eq!(write(&records, &linear, "second", 1, 2, "t2").unwrap(), Some(Wrote::Replaced));
    let second = records.context_comment("i-12").unwrap().unwrap().comment_id;
    assert_ne!(first, second);
    assert_eq!(bodies(&linear), ["i-12: second"]);
    // A recorded ID that isn't plain is no comment's, so it is replaced too.
    records.set_context_comment(&ContextComment { comment_id: "not plain!".into(), ..records.context_comment("i-12").unwrap().unwrap() }).unwrap();
    assert_eq!(write(&records, &linear, "third", 2, 3, "t3").unwrap(), Some(Wrote::Replaced));
    assert_ne!(records.context_comment("i-12").unwrap().unwrap().comment_id, "not plain!");
}

#[test]
fn a_failed_write_keeps_the_claim_and_its_create_is_never_posted_twice() {
    let (records, linear) = (FakeRecords::default(), Linear::default());
    linear.lost.set(true);
    assert!(write(&records, &linear, "first", 0, 1, "t1").is_err());
    // Held until it lapses, so a create landing late isn't raced by a replacement.
    assert_eq!(write(&records, &linear, "again", 1, 2, "t2").unwrap(), None);
    records.release(&[comment_claim(&Id::from("i-12"))], "t1").unwrap();
    linear.lost.set(false);
    assert_eq!(write(&records, &linear, "again", 1, 2, "t2").unwrap(), Some(Wrote::Updated));
    assert_eq!(bodies(&linear), ["i-12: again"]);
    // One refused outright leaves a record of nothing, which the next run replaces.
    let (records, linear) = (FakeRecords::default(), Linear::default());
    linear.refuse.set(true);
    assert!(write(&records, &linear, "first", 0, 1, "t1").is_err());
    records.release(&[comment_claim(&Id::from("i-12"))], "t1").unwrap();
    linear.refuse.set(false);
    assert_eq!(write(&records, &linear, "again", 1, 2, "t2").unwrap(), Some(Wrote::Replaced));
    assert_eq!(bodies(&linear), ["i-12: again"]);
}

#[test]
fn another_run_writing_the_issues_comment_holds_it_off() {
    let (records, linear) = (FakeRecords::default(), Linear::default());
    records.take(&[(comment_claim(&Id::from("i-12")), Some(COMMENT_HOLD))], "other").unwrap();
    assert_eq!(write(&records, &linear, "first", 0, 1, "t1").unwrap(), None);
    assert!(linear.comments.borrow().is_empty() && records.context_comment("i-12").unwrap().is_none());
    records.release(&[comment_claim(&Id::from("i-12"))], "other").unwrap();
    assert_eq!(write(&records, &linear, "first", 0, 1, "t1").unwrap(), Some(Wrote::Created));
}

#[test]
fn the_brief_names_only_identifiers() {
    let closed = parse_closed(&issue("completed")).unwrap();
    let start = Start { closed: &closed, owner_name: "Sam", briefs_dir: "/briefs/<repo>", repos: &RepoMap::default(), at: &t(0) };
    let text = start.render("tolkster", &Merge::Absent);
    assert!(text.contains("`TW-11`, `TW-12`, `TW-9`") && text.contains("`TW-20`, `TW-23`"), "{text}");
    assert!(text.contains("--closed TW-10 --seen 2026-10-04T01:00:00.000Z --body-file -"), "{text}");
    assert!(text.contains("2. No squash merge for TW-10 was on the default branch"), "{text}");
    let sha = Sha::parse(&"c".repeat(40)).unwrap();
    let merged = start.render("tolkster", &Merge::Found(sha.clone()));
    assert!(merged.contains(&format!("2. TW-10 merged on the default branch as `{sha}`: read what it changed with `git show {sha}`.")) && !merged.contains("No squash merge"), "{merged}");
    let unknown = start.render("tolkster", &Merge::Unknown);
    assert!(unknown.contains("2. Looking for TW-10's merge failed, so find it: `git fetch origin`, then `git log origin/HEAD --grep \"TW-10\"`"), "{unknown}");
    let only = Closed { open_siblings: Vec::new(), ..closed.clone() };
    let alone = Start { closed: &only, ..start }.render("tolkster", &Merge::Absent);
    assert!(alone.contains("none") && !alone.contains("`TW-20`"));
    for placeholder in ["<identifier>", "<related>", "<siblings>", "<repo displayName>", "<OWNER NAME>", "<started>", "<SESSION PREAMBLE>", "<CHECKOUT>", "<CLOSE TERMINAL>", "<MERGED>"] {
        assert!(![&text, &merged, &unknown].iter().any(|t| t.contains(placeholder)), "{placeholder}");
    }
}

// `two_repos` with git in the tolkster primary checkout: origin's tip is `a…a`, and the default
// branch's log has a commit that only has TW-100 in its subject, then TW-10's squash merge.
fn repo_world(primary: bool) -> Rc<RefCell<World>> {
    let world = World::two_repos(primary);
    {
        let mut world = world.borrow_mut();
        world.tip = "a".repeat(40);
        world.log = format!("{}\tAfter TW-100 (#6)\n{}\tThe thing (TW-10) (#5)\n", "b".repeat(40), "c".repeat(40));
        world.git.insert("/src/tolkster".into(), Git::default());
    }
    world
}

fn started(closed: &Closed, orca: &mut FakeOrca, store: &Fake, token: &str, mine: bool) -> (Result<Context, Error>, Vec<(String, String)>) {
    let start = Start { closed, owner_name: "Sam", briefs_dir: "/briefs/<repo>", repos: &RepoMap::default(), at: &t(0) };
    let mut written = Vec::new();
    let result = super::start(&start, orca, &mut |path, text| {
        written.push((path.display().to_string(), text.to_string()));
        Ok(())
    }, store, token, || Ok(mine));
    (result, written)
}

#[test]
fn a_completed_issue_of_mine_starts_context_once_in_the_primary_checkout() {
    let closed = parse_closed(&issue("completed")).unwrap();
    let (store, mut orca) = (Fake::default(), FakeOrca::on(&repo_world(true)));
    let (result, written) = started(&closed, &mut orca, &store, "t1", true);
    assert_eq!(result.unwrap(), Context::Started { issue: "TW-10".into(), issues: vec!["TW-11".into(), "TW-12".into(), "TW-9".into(), "TW-20".into(), "TW-23".into()] });
    assert_eq!(written.len(), 1);
    assert!(written[0].0.ends_with("TW-10-context.md"));
    // The merge is the one whose subject names TW-10 whole, found after a fetch of the primary checkout.
    assert!(written[0].1.contains(&format!("merged on the default branch as `{}`", "c".repeat(40))), "{}", written[0].1);
    let ran = orca.world.borrow().ran.clone();
    assert!(ran.iter().any(|c| c.starts_with("[/src/tolkster] git ") && c.contains("fetch --quiet origin")) && ran.iter().any(|c| c.starts_with("[/src/tolkster] git log --first-parent")), "{ran:?}");
    let create = orca.world.borrow().opened_as_asked().into_iter().next().unwrap();
    assert!(create.contains("--worktree id:r1::/src/tolkster --title TW-10 context --command claude \"Read the file "), "{create}");
    // One shell call for the brief's turn: the completed issue, then the related ones and the siblings.
    let reads = ["TW-10", "TW-11", "TW-12", "TW-9", "TW-20", "TW-23"].join(" ");
    assert!(create.contains(&format!("in full and, in the same turn, run tolkster issue show {reads} --comments as one shell call.")), "{create}");
    assert!(store.claims.borrow().contains_key("context-TW-10"));
    // Again, from another run: the claim holds, and nothing more is opened.
    let mut again = FakeOrca::on(&repo_world(true));
    assert_eq!(started(&closed, &mut again, &store, "t2", true).0.unwrap(), Context::Already { issue: "TW-10".into() });
    assert!(again.world.borrow().opened.is_empty());
}

#[test]
fn a_briefs_dir_that_cant_be_typed_takes_no_claim() {
    let closed = parse_closed(&issue("completed")).unwrap();
    let (store, mut orca) = (Fake::default(), FakeOrca::on(&World::two_repos(true)));
    let start = Start { closed: &closed, owner_name: "Sam", briefs_dir: "/briefs/$x/<repo>", repos: &RepoMap::default(), at: &t(0) };
    assert!(super::start(&start, &mut orca, &mut |_, _| panic!("written"), &store, "t1", || Ok(true)).is_err());
    assert!(store.claims.borrow().is_empty());
    assert!(orca.world.borrow().opened.is_empty());
}

fn closed_again() -> Closed {
    parse_closed(&issue("completed")).unwrap()
}

#[test]
fn nothing_starts_without_something_to_look_at() {
    let store = Fake::default();
    let mut orca = FakeOrca::on(&World::two_repos(true));
    let cancelled = parse_closed(&issue("canceled")).unwrap();
    assert_eq!(started(&cancelled, &mut orca, &store, "t", true).0.unwrap(), Context::NotCompleted { issue: "TW-10".into() });
    let lonely = Closed { open_related: Vec::new(), open_siblings: Vec::new(), ..parse_closed(&issue("completed")).unwrap() };
    assert_eq!(started(&lonely, &mut orca, &store, "t", true).0.unwrap(), Context::NoneOpen { issue: "TW-10".into() });
    let siblings_only = Closed { open_related: Vec::new(), ..parse_closed(&issue("completed")).unwrap() };
    assert_eq!(started(&siblings_only, &mut FakeOrca::on(&repo_world(true)), &Fake::default(), "t", true).0.unwrap(), Context::Started { issue: "TW-10".into(), issues: vec!["TW-20".into(), "TW-23".into()] });
    let closed = parse_closed(&issue("completed")).unwrap();
    assert_eq!(started(&closed, &mut orca, &store, "t", false).0.unwrap(), Context::NotMine { issue: "TW-10".into() });
    let elsewhere = Closed { place: Place { project: Some("Elsewhere".into()), ..Place::default() }, ..closed };
    assert_eq!(started(&elsewhere, &mut orca, &store, "t", true).0.unwrap(), Context::Missing { issue: "TW-10".into(), missing: crate::trace::Missing::Repo });
    let mut unchecked = FakeOrca::on(&World::two_repos(false));
    assert_eq!(started(&closed_again(), &mut unchecked, &store, "t", true).0.unwrap(), Context::Missing { issue: "TW-10".into(), missing: crate::trace::Missing::Primary });
    assert!(store.claims.borrow().is_empty());
    assert!(orca.world.borrow().opened.is_empty());
}

#[test]
fn a_terminal_that_wont_open_gives_the_claim_back() {
    let closed = parse_closed(&issue("completed")).unwrap();
    let store = Fake::default();
    let mut orca = FakeOrca::on(&repo_world(true));
    orca.world.borrow_mut().launch_fails = true;
    assert!(started(&closed, &mut orca, &store, "t1", true).0.is_err());
    assert!(store.claims.borrow().is_empty());
    // So a retry starts it.
    let mut orca = FakeOrca::on(&repo_world(true));
    assert!(matches!(started(&closed, &mut orca, &store, "t2", true).0.unwrap(), Context::Started { .. }));
}

#[test]
fn a_merge_that_isnt_there_or_cant_be_looked_up_still_starts_and_the_brief_says_which() {
    let closed = parse_closed(&issue("completed")).unwrap();
    // No git in the primary checkout: the fetch fails, and the session looks itself.
    let (store, mut orca) = (Fake::default(), FakeOrca::on(&World::two_repos(true)));
    let (result, written) = started(&closed, &mut orca, &store, "t1", true);
    assert!(matches!(result.unwrap(), Context::Started { .. }));
    assert!(written[0].1.contains("Looking for TW-10's merge failed"), "{}", written[0].1);
    // A default branch with no merge for it.
    let world = repo_world(true);
    world.borrow_mut().log = format!("{}\tAfter TW-100 (#6)\n", "b".repeat(40));
    let (result, written) = started(&closed, &mut FakeOrca::on(&world), &Fake::default(), "t2", true);
    assert!(matches!(result.unwrap(), Context::Started { .. }));
    assert!(written[0].1.contains("No squash merge for TW-10 was on the default branch"), "{}", written[0].1);
}
