// `tolkster issue`: an issue read as Tolkster (commands/show.rs), issues found as Tolkster
// (commands/list.rs), and edits, new issues and relations, made as Tolkster (see issues.rs).

use serde_json::{Value, json};
use ureq::Agent;

use crate::args::{self, Kind, js_trim, parse};
use crate::cli::{NOTHING_TO_DO, print_json, read_body};
use crate::commands::asker::{Asker, SESSION_OPTION, THREAD_OPTION};
use crate::config::{files, owner};
use crate::request::{Issue, Session, natural_dedup};
use crate::error::Error;
use crate::issue_state::{filing_target, in_order};
use crate::issues;
use crate::linear::{IssueRef, Ref, Relation, StateType, create_issue, delete_relation, relate, update_issue, with_token};
use crate::sessions::lookup::{ByDelegate, ChainTarget, FilingContext, MyWork, UpdateTarget};
use crate::sessions::{lookup, workspace};
use crate::machine::chain_recorded;
use crate::state::{Delegation, State, new_uuid};
use crate::plain::IssueKey;
use crate::time::{iso, now_ms};

pub const UPDATE: &[(&str, Kind)] = &[
    SESSION_OPTION,
    THREAD_OPTION,
    ("title", Kind::String),
    ("description-file", Kind::String),
    ("priority", Kind::String),
    ("estimate", Kind::String),
    ("state", Kind::String),
    ("project", Kind::String),
    ("assignee", Kind::String),
    ("add-label", Kind::List),
    ("remove-label", Kind::List),
    ("delegate", Kind::Boolean),
];

// Each of issues::RELATIONS, repeatable in `create`, are named by hand: allowlist/tests.rs
// keeps them in step.
pub const CREATE: &[(&str, Kind)] = &[
    SESSION_OPTION,
    THREAD_OPTION,
    ("title", Kind::String),
    ("description-file", Kind::String),
    ("parent", Kind::String),
    ("team", Kind::String),
    ("project", Kind::String),
    ("priority", Kind::String),
    ("estimate", Kind::String),
    ("state", Kind::String),
    ("assignee", Kind::String),
    ("triage", Kind::Boolean),
    ("label", Kind::List),
    ("blocks", Kind::List),
    ("blocked-by", Kind::List),
    ("related", Kind::List),
    ("duplicate-of", Kind::List),
];

pub const RELATE: &[(&str, Kind)] =
    &[SESSION_OPTION, THREAD_OPTION, ("blocks", Kind::String), ("blocked-by", Kind::String), ("related", Kind::String), ("duplicate-of", Kind::String)];

pub const CHAIN: &[(&str, Kind)] = &[SESSION_OPTION, ("block", Kind::List)];

const DELEGATE_NEEDS_SESSION: &str = "--delegate needs --session <sessionId>: I delegate a sub-issue only from the owner's session on its parent";

// A new issue without a session has no issue to take a team from: it needs `--parent` or
// `--team`, a usage error without either, before anything is read.
fn check_create_context(asker: &Asker, parent: Option<&str>, team: Option<&str>) -> Result<(), Error> {
    if asker.is_owner() && parent.is_none() && team.is_none() {
        return Err(Error::Usage("without --session, pass --parent <TW-123> or --team <KEY>: there is no session to take the team from".into()));
    }
    Ok(())
}

// Why `target`, read with `lookup::update_target`, may not be delegated to Tolkster from `session`,
// or None if it may: only a sub-issue of the session's issue, which must itself be
// delegated to Tolkster, and only while it is unfinished, out of Triage and not delegated to
// someone else.
// The session is already known to be the owner's (`trusted_session`).
fn refuse_delegation(session: &Session, data: &UpdateTarget) -> Option<String> {
    let issue = match delegating_parent(session, Some(data.viewer.id.clone())) {
        Ok(issue) => issue,
        Err(reason) => return Some(reason),
    };
    let target = &data.issue.identifier;
    if data.issue.parent.as_ref().map(|p| p.id.as_str()) != Some(issue.id.as_str()) {
        return Some(format!("{target} isn't a sub-issue of {}: I delegate only those to myself", issue.identifier));
    }
    let state = data.issue.state.kind;
    if state.is_finished() {
        return Some(format!("{target} has finished: there's nothing to delegate"));
    }
    if state == StateType::Triage {
        return Some(format!("{target} is in Triage: nobody has accepted it, so I don't delegate it"));
    }
    data.issue.delegate.as_ref().is_some_and(|d| d.id != data.viewer.id).then(|| format!("{target} is delegated to someone else: I leave it with them"))
}

// The session's issue, if Tolkster works through its sub-issues: it must be delegated to
// `viewer`, Tolkster itself, and not cancelled: a cancelled parent's chain has ended
// (the machine's `takeBackChain`), so nothing is delegated for it.
fn delegating_parent(session: &Session, viewer: Option<String>) -> Result<&Issue, String> {
    let Some(issue) = &session.issue else { return Err("the session has no issue".into()) };
    if issue.delegate_id.is_none() || issue.delegate_id != viewer {
        return Err(format!("{} isn't delegated to me, so I don't work through its sub-issues", issue.identifier));
    }
    if issue.state_type.is_cancelled() {
        return Err(format!("{} was cancelled, so I delegate none of its sub-issues", issue.identifier));
    }
    Ok(issue)
}

// Which workflow state an issue goes to: one by name, or a follow-up's (issue_state::filing_target).
#[derive(Clone, Copy, Debug, PartialEq)]
enum StateChoice<'a> {
    Named(&'a str),
    Triage,
    // Triage and nothing else, for an issue nobody may start until someone accepts it.
    OnlyTriage,
}

// The team's workflow state `state` picks, and the labels named `labels`, as ids.
fn team_lookups(agent: &Agent, token: &str, team_id: &str, state: Option<StateChoice>, labels: &[&[String]]) -> Result<(Option<String>, Vec<Vec<String>>), Error> {
    let state_id = match state {
        Some(choice) => {
            let team = workspace::team_states(agent, token, team_id)?;
            let states = in_order(team.states.nodes);
            Some(match choice {
                StateChoice::Named(name) => issues::pick_state(&states, name)?,
                StateChoice::Triage | StateChoice::OnlyTriage => {
                    let target = filing_target(&states, team.triage_enabled)?;
                    if choice == StateChoice::OnlyTriage && target.state_type != StateType::Triage {
                        return Err(Error::Failed("the team has no triage, where a sub-issue of an issue delegated to me waits: pass --parent none, or another parent".into()));
                    }
                    target.id.clone()
                }
            })
        }
        None => None,
    };
    let names: Vec<&String> = labels.iter().flat_map(|l| l.iter()).collect();
    let label_ids = if names.is_empty() {
        labels.iter().map(|_| Vec::new()).collect()
    } else {
        let filter = json!({ "or": names.iter().map(|n| json!({ "name": { "eqIgnoreCase": n.trim() } })).collect::<Vec<_>>() });
        let found = workspace::labels(agent, token, filter)?;
        labels.iter().map(|group| group.iter().map(|n| issues::pick_label(&found, team_id, n)).collect::<Result<Vec<_>, _>>()).collect::<Result<Vec<_>, _>>()?
    };
    Ok((state_id, label_ids))
}

// The team's project named `name` as its id, or null for `none`.
fn team_project(agent: &Agent, token: &str, team_id: &str, name: &str) -> Result<Value, Error> {
    if name.trim().eq_ignore_ascii_case("none") {
        return Ok(Value::Null);
    }
    issues::pick_project(&workspace::team_projects(agent, token, team_id, name.trim())?, name)
}

// The fields both update and create set straight from their options. `owner` is what
// `--assignee me` means.
fn plain_fields(parsed: &args::Parsed, owner: &str) -> Result<serde_json::Map<String, Value>, Error> {
    let mut input = serde_json::Map::new();
    if let Some(title) = parsed.string("title") {
        if js_trim(title).is_empty() {
            return Err(Error::Usage("--title is empty".into()));
        }
        input.insert("title".into(), json!(title));
    }
    if let Some(source) = parsed.string("description-file") {
        input.insert("description".into(), json!(args::file_body(source, "description", read_body)?));
    }
    if let Some(priority) = parsed.string("priority") {
        input.insert("priority".into(), json!(issues::priority(priority)?));
    }
    if let Some(estimate) = parsed.string("estimate") {
        input.insert("estimate".into(), issues::estimate(estimate)?);
    }
    if let Some(assignee) = parsed.string("assignee") {
        input.insert("assigneeId".into(), issues::assignee(assignee, owner)?);
    }
    Ok(input)
}

pub fn issue_update(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, UPDATE, true)?;
    let Some(id) = parsed.first() else {
        return Err(Error::Usage("issue update needs the issue's ID".into()));
    };
    let delegate = parsed.flag("delegate");
    let (asker, owner) = (Asker::parse(&parsed)?, owner(&files()?)?);
    if delegate && !asker.is_session() {
        return Err(Error::Usage(DELEGATE_NEEDS_SESSION.into()));
    }
    // A delegation is recorded before Linear takes the change, so a change that can fail
    // there, an assignee that names no user, never rides with one.
    if delegate && parsed.string("assignee").is_some() {
        return Err(Error::Usage("--assignee and --delegate don't go together: assigning isn't delegating".into()));
    }
    let fields = plain_fields(&parsed, &owner)?;
    let (state, project) = (parsed.non_empty("state"), parsed.non_empty("project"));
    let (added, removed) = (parsed.list("add-label"), parsed.list("remove-label"));
    if fields.is_empty() && state.is_none() && project.is_none() && added.is_empty() && removed.is_empty() && !delegate {
        return Err(Error::Usage("nothing to change: pass --title, --description-file, --priority, --estimate, --state, --project, --assignee, --add-label, --remove-label or --delegate".into()));
    }
    with_token(agent, |token| {
        // A thread's turn changes only the issue its thread is on.
        let (session, only) = match asker {
            Asker::Thread(_) => (None, asker.issue(agent, token, &owner)?),
            _ => (asker.trusted(agent, token, &owner)?, None),
        };
        let data = lookup::update_target(agent, token, id)?;
        only_its_issue(only.as_deref(), &data.issue.id, id)?;
        if only.is_some() && state.is_some() {
            outside_my_work(agent, token, id)?;
        }
        // Every name is looked up before anything is recorded or changed, so a wrong one
        // leaves no trace.
        let team_id = data.issue.team.id.clone();
        if let Some(estimate) = fields.get("estimate") {
            issues::check_estimate(&data.issue.team.estimation, estimate)?;
        }
        let (state_id, labels) = team_lookups(agent, token, &team_id, state.map(StateChoice::Named), &[added, removed])?;
        let project_id = project.map(|name| team_project(agent, token, &team_id, name)).transpose()?;
        // Delegating a sub-issue to Tolkster, to work through its parent: checked here, in
        // code, and recorded in Tolkster's state before Linear starts the sub-issue's
        // session, so that session is trusted (Session::is_owners).
        let delegated_to = if delegate {
            let Some(session) = &session else { return Err(Error::Usage(DELEGATE_NEEDS_SESSION.into())) };
            if let Some(reason) = refuse_delegation(session, &data) {
                return Err(Error::Failed(reason));
            }
            let parent_issue_id = session.issue.as_ref().map(|i| i.id.clone()).unwrap_or_default();
            let delegation = Delegation { issue_id: data.issue.id.clone(), parent_issue_id, session_id: session.id.clone(), at: iso(now_ms()) };
            state_of(agent)?.delegate(&delegation)?;
            Some(data.viewer.id.clone())
        } else {
            None
        };
        let mut input = fields.clone();
        if let Some(state_id) = state_id {
            input.insert("stateId".into(), json!(state_id));
        }
        if let Some(project_id) = project_id {
            input.insert("projectId".into(), project_id);
        }
        if !labels[0].is_empty() {
            input.insert("addedLabelIds".into(), json!(labels[0]));
        }
        if !labels[1].is_empty() {
            input.insert("removedLabelIds".into(), json!(labels[1]));
        }
        if let Some(viewer) = delegated_to {
            input.insert("delegateId".into(), json!(viewer));
        }
        update_issue(agent, token, &data.issue.id, Value::Object(input))
    })?;
    Ok(0)
}

// A reply's turn changes only `only`, the issue its thread is on: `issue_id` is the issue named.
fn only_its_issue(only: Option<&str>, issue_id: &str, id: &str) -> Result<(), Error> {
    match only {
        Some(only) if issue_id != only => Err(Error::Usage(format!("a reply's turn changes only the issue its thread is on, not {id}"))),
        _ => Ok(()),
    }
}

fn state_of(agent: &Agent) -> Result<crate::state::State<'_>, Error> {
    State::open(agent, &files()?)
}

pub fn issue_create(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, CREATE, false)?;
    if parsed.string("title").is_none() {
        return Err(Error::Usage("issue create needs --title".into()));
    }
    let (asker, owner) = (Asker::parse(&parsed)?, owner(&files()?)?);
    let fields = plain_fields(&parsed, &owner)?;
    // `--parent none` files it with no parent, which is what leaving it out means but in a
    // reply's turn, whose issue is the parent then.
    let (parent, no_parent) = match parsed.non_empty("parent") {
        Some(parent) if parent.trim().eq_ignore_ascii_case("none") => (None, true),
        parent => (parent, false),
    };
    let team = parsed.non_empty("team").map(str::to_ascii_uppercase);
    check_create_context(&asker, parent, team.as_deref())?;
    let (state, project) = (parsed.non_empty("state"), parsed.non_empty("project"));
    // A follow-up goes to the team's triage state, picked by type, not by name.
    let state = match (state, parsed.flag("triage")) {
        (Some(_), true) => return Err(Error::Usage("pass --state <name> or --triage, not both".into())),
        (Some(name), false) => Some(StateChoice::Named(name)),
        (None, true) => Some(StateChoice::Triage),
        (None, false) => None,
    };
    let labels = parsed.list("label");
    let wanted = wanted_relations(&parsed)?;
    let (created, relations) = with_token(agent, |token| {
        let asked_on = asker.issue(agent, token, &owner)?;
        let parent = parent.map(String::from).or_else(|| asked_on.clone().filter(|_| matches!(asker, Asker::Thread(_)) && !no_parent));
        // The issue the new one follows from, for its team and project: its parent, or the
        // session's or thread's issue. With neither, --team alone names the team.
        let context = parent.clone().or(asked_on);
        if context.is_none() && team.is_none() {
            return Err(Error::Failed("the session has no issue: pass --parent or --team".into()));
        }
        // Every issue it is to be related to is checked and read before the new one is filed,
        // so a refusal or an unknown ID files nothing.
        if matches!(asker, Asker::Thread(_)) {
            for (_, _, other) in &wanted {
                outside_my_work(agent, token, other)?;
            }
        }
        let relations = resolve_relations(agent, token, &wanted)?;
        let data = context.as_deref().map(|context| lookup::filing_context(agent, token, context)).transpose()?;
        refuse_relating_filed_work(&asker, parent.is_some(), data.as_ref(), !wanted.is_empty())?;
        let no_context = || Error::Failed("the session has no issue: pass --parent or --team".into());
        let (team_node, project_id) = match (&team, &data) {
            (Some(key), _) if Some(key.as_str()) != data.as_ref().map(|d| d.issue.team.key.as_str()) => (workspace::team_by_key(agent, token, key)?.ok_or_else(|| Error::Failed(format!("no team with key {key}")))?, None),
            (_, Some(data)) => (data.issue.team.team.clone(), data.issue.project.as_ref().map(|p| p.id.clone())),
            (_, None) => return Err(no_context()),
        };
        let team_id = team_node.id.clone();
        if let Some(estimate) = fields.get("estimate") {
            issues::check_estimate(&team_node.estimation, estimate)?;
        }
        // --project, looked up in the new issue's team, overrides the project it follows from.
        let project_id = match project {
            Some(name) => team_project(agent, token, &team_id, name)?.as_str().map(String::from),
            None => project_id,
        };
        let state = filing_state(&asker, parent.is_some(), data.as_ref(), state)?;
        let (state_id, label_ids) = team_lookups(agent, token, &team_id, state, &[labels])?;
        let mut input = fields.clone();
        input.insert("teamId".into(), json!(team_id));
        if parent.is_some() {
            input.insert("parentId".into(), json!(data.as_ref().ok_or_else(no_context)?.issue.id));
        }
        if let Some(project_id) = project_id {
            input.insert("projectId".into(), json!(project_id));
        }
        if let Some(state_id) = state_id {
            input.insert("stateId".into(), json!(state_id));
        }
        if !label_ids[0].is_empty() {
            input.insert("labelIds".into(), json!(label_ids[0]));
        }
        let created = create_issue(agent, token, Value::Object(input))?;
        Ok((created, relations))
    })?;
    // Its own run of the token, so a rejected token never files the issue again: only the
    // relations not yet made are tried with the next.
    let mut made = 0;
    let related = with_token(agent, |token| relate_all(&relations, &mut made, |relation, other| relate(agent, token, relation, &created.id, other)));
    let mut printed = json!({ "identifier": created.identifier, "url": created.url });
    if !relations.is_empty() {
        printed["relations"] = relations[..made].iter().map(|(name, _, _, identifier)| json!({ "relation": name, "issue": identifier })).collect();
    }
    print_json(&printed)?;
    related.map_err(|error| Error::Failed(format!("{} is filed, but only {made} of its {} relations were made, so add the rest with issue relate, and file nothing again: {error}", created.identifier, relations.len())))?;
    Ok(0)
}

// What `issue create` was asked to relate the new issue to, as (flag, relation, the other
// issue's ID), in the table's order, each once.
fn wanted_relations(parsed: &args::Parsed) -> Result<Vec<(&'static str, Relation, String)>, Error> {
    let mut wanted: Vec<(&'static str, Relation, String)> = Vec::new();
    for (name, relation) in issues::RELATIONS.iter() {
        for other in parsed.list(name) {
            let other = other.trim();
            if other.is_empty() {
                return Err(Error::Usage(format!("--{name} is empty")));
            }
            if !wanted.iter().any(|(n, _, o)| n == name && o.eq_ignore_ascii_case(other)) {
                wanted.push((name, *relation, other.to_string()));
            }
        }
    }
    Ok(wanted)
}

// `wanted`'s issues read, as (flag, relation, the other's id, its identifier): an ID that
// names no issue fails.
fn resolve_relations(agent: &Agent, token: &str, wanted: &[(&'static str, Relation, String)]) -> Result<Vec<(&'static str, Relation, String, String)>, Error> {
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<&str> = wanted.iter().map(|(_, _, other)| other.as_str()).collect();
    resolved_relations(&lookup::issues_by_id(agent, token, &ids)?, wanted)
}

// `found` is each of `wanted`'s issues, in order, None where Linear has none.
fn resolved_relations(found: &[Option<IssueRef>], wanted: &[(&'static str, Relation, String)]) -> Result<Vec<(&'static str, Relation, String, String)>, Error> {
    let resolved: Vec<_> = wanted
        .iter()
        .enumerate()
        .map(|(i, (name, relation, other))| match found.get(i).and_then(Option::as_ref) {
            Some(issue) => Ok((*name, *relation, issue.id.clone(), issue.identifier.clone())),
            None => Err(Error::Failed(format!("--{name} {other} names no issue: nothing was filed"))),
        })
        .collect::<Result<_, _>>()?;
    // One issue twice, by identifier and by ID or under two flags, would make a second relation
    // Linear may refuse after the issue is filed.
    if let Some((_, _, _, identifier)) = resolved.iter().enumerate().find(|(i, (_, _, id, _))| resolved[..*i].iter().any(|(_, _, seen, _)| seen == id)).map(|(_, r)| r) {
        return Err(Error::Usage(format!("{identifier} is named twice: relate it once, nothing was filed")));
    }
    Ok(resolved)
}

// Makes the relations from the `made`th on, one at a time, counting each in `made` as it is
// made: stopped at the first that fails, with the rest left for `issue relate`.
fn relate_all(relations: &[(&'static str, Relation, String, String)], made: &mut usize, mut relate: impl FnMut(Relation, &str) -> Result<(), Error>) -> Result<(), Error> {
    for (_, relation, id, _) in &relations[*made..] {
        relate(*relation, id)?;
        *made += 1;
    }
    Ok(())
}

// A sub-issue of an issue delegated to Tolkster joins its work once out of Triage
// (docs/parent-issues.md), so one a reply's turn files waits in Triage for someone to accept
// it: a thread never starts code work. `data` is the issue it follows from, read with its
// delegate and the viewer.
fn filing_state<'a>(asker: &Asker, has_parent: bool, data: Option<&FilingContext>, state: Option<StateChoice<'a>>) -> Result<Option<StateChoice<'a>>, Error> {
    match state {
        _ if !matches!(asker, Asker::Thread(_)) || !has_parent || !delegated_to_me(data) => Ok(state),
        Some(StateChoice::Named(_)) => Err(Error::Failed("its parent is delegated to me, so from a thread it goes to Triage: leave out --state".into())),
        _ => Ok(Some(StateChoice::OnlyTriage)),
    }
}

// A thread files a sub-issue of an issue delegated to Tolkster into its work (in Triage, but
// `in_my_work` counts it), which `issue relate` refuses a thread to relate: so does `create`.
// `data` is the issue it follows from, read with its delegate and the viewer.
fn refuse_relating_filed_work(asker: &Asker, has_parent: bool, data: Option<&FilingContext>, relating: bool) -> Result<(), Error> {
    match relating && matches!(asker, Asker::Thread(_)) && has_parent && delegated_to_me(data) {
        true => Err(Error::Failed("its parent is delegated to me, so the new sub-issue is part of my work: from a thread I don't relate it".into())),
        false => Ok(()),
    }
}

// The issue in `data` is delegated to Tolkster, the viewer there.
fn delegated_to_me(data: Option<&FilingContext>) -> bool {
    data.is_some_and(|data| data.issue.delegate.as_ref().is_some_and(|d| d.id == data.viewer.id))
}

// A reply's turn moves or relates issue `id` only outside Tolkster's work: a state move, a
// blocker or a duplicate decides when that work starts or halts, which a thread leaves to
// the owner's session and the work machine.
fn outside_my_work(agent: &Agent, token: &str, id: &str) -> Result<(), Error> {
    let data = lookup::my_work(agent, token, id)?;
    match in_my_work(&data) {
        true => Err(Error::Failed(format!("{id} is part of my work, or blocks it: from a thread I don't move it or change its relations"))),
        false => Ok(()),
    }
}

// The issue `lookup::my_work` read is delegated to Tolkster, a sub-issue or parent of one that is, or
// blocks either, counting sub-issues and relations past the page read as such.
fn in_my_work(data: &MyWork) -> bool {
    let Some(issue) = &data.issue else { return false };
    let me = |delegate: &Option<Ref>| delegate.as_ref().is_some_and(|d| d.id == data.viewer.id);
    let mine = |delegate: &Option<Ref>, parent: &Option<ByDelegate>| me(delegate) || parent.as_ref().is_some_and(|p| me(&p.delegate));
    mine(&issue.delegate, &issue.parent)
        || issue.children.page_info.has_next_page
        || issue.blocking.page_info.has_next_page
        || issue.children.nodes.iter().any(|c| me(&c.delegate))
        || issue.blocking.nodes.iter().any(|r| r.kind == "blocks" && r.related_issue.as_ref().is_some_and(|i| mine(&i.delegate, &i.parent)))
}

// relate adds the relation; unrelate removes it, and exits 3 when there is none.
pub fn issue_relate(agent: &Agent, argv: &[String], add: bool) -> Result<u8, Error> {
    let parsed = parse(argv, RELATE, true)?;
    let named: Vec<(Relation, &str)> = issues::RELATIONS.iter().filter_map(|(name, r)| parsed.non_empty(name).map(|o| (*r, o))).collect();
    let (Some(id), [(relation, other)]) = (parsed.first(), named.as_slice()) else {
        let verb = if add { "relate" } else { "unrelate" };
        return Err(Error::Usage(format!("issue {verb} needs the issue's ID and one of --blocks, --blocked-by, --related or --duplicate-of")));
    };
    let (asker, owner) = (Asker::parse(&parsed)?, owner(&files()?)?);
    let found = with_token(agent, |token| {
        asker.trusted(agent, token, &owner)?;
        if matches!(asker, Asker::Thread(_)) {
            outside_my_work(agent, token, id)?;
            outside_my_work(agent, token, other)?;
        }
        let pair = lookup::relation_pair(agent, token, id, other)?;
        if add {
            return relate(agent, token, *relation, &pair.a.id, &pair.b.id).map(|()| true);
        }
        let ids = relation.find(&pair.a.relations, &pair.b.identifier);
        for relation_id in &ids {
            delete_relation(agent, token, relation_id)?;
        }
        Ok(!ids.is_empty())
    })?;
    if !found {
        eprintln!("{id} has no such relation to {other}");
        return Ok(NOTHING_TO_DO);
    }
    Ok(0)
}

// What `issue chain` writes, decided from one read before anything is written.
#[derive(Debug, PartialEq)]
struct Chain {
    // (sub-issue, blocker), as (id, identifier) each: only the relations not already there.
    blocks: Vec<((String, String), (String, String))>,
    // The unfinished sub-issues, unblocked first, as (id, identifier).
    delegate: Vec<(String, String)>,
    // The unfinished sub-issues delegated to someone else, in Triage, or waiting on an issue in
    // Triage (directly or through another sub-issue), which nobody has accepted yet, by
    // identifier: left alone.
    skipped: Vec<String>,
}

// A sub-issue as `lookup::chain_target` reads it.
struct Child<'a> {
    id: &'a str,
    finished: bool,
    // Delegated to someone other than Tolkster.
    elsewhere: bool,
    triage: bool,
    // Blocked by an unfinished issue in Triage, sub-issue or not.
    behind_triage: bool,
    // The identifiers of the unfinished issues blocking it, sub-issues or not.
    blocked_by: Vec<&'a str>,
}

// Parses each `--block <sub-issue>:<blocker>`, both plain identifiers.
fn parse_blocks(values: &[String]) -> Result<Vec<(String, String)>, Error> {
    values
        .iter()
        .map(|value| {
            let pair = value.split_once(':').and_then(|(sub, blocker)| Some((IssueKey::parse(sub)?.upper().into(), IssueKey::parse(blocker)?.upper().into())));
            pair.ok_or_else(|| Error::Usage(format!("--block takes <sub-issue>:<blocker>, e.g. TW-3:TW-2: {value}")))
        })
        .collect()
}

// Decides the chain for the session's issue, `data` from `lookup::chain_target`, or why not: every ID
// in `blocks` must be an unfinished sub-issue of the session's issue, which is delegated to
// Tolkster, and the blocks, with those already there, must close no loop, where each
// sub-issue on it would wait forever.
fn plan_chain(session: &Session, asked: &str, data: &ChainTarget, blocks: &[(String, String)]) -> Result<Chain, String> {
    let parent = delegating_parent(session, Some(data.viewer.id.clone()))?;
    let Some(issue) = data.issue.as_ref().filter(|i| i.id == parent.id) else {
        return Err(format!("{asked} isn't the session's issue: I chain only {}'s sub-issues from here", parent.identifier));
    };
    if issue.children.page_info.has_next_page {
        return Err(format!("{} has more sub-issues than I read at once", parent.identifier));
    }
    let mut children: Vec<(&str, Child)> = Vec::new();
    for node in &issue.children.nodes {
        let (identifier, state) = (node.identifier.as_str(), node.state.kind);
        if node.relations.inverse_relations.as_ref().is_some_and(|r| r.page_info.has_next_page) {
            return Err(format!("{identifier} has more relations than I read at once"));
        }
        let open: Vec<_> = Relation::BlockedBy.related(&node.relations).into_iter().filter(|r| !r.other.state.kind.is_finished()).collect();
        let behind_triage = open.iter().any(|r| r.other.state.kind == StateType::Triage);
        let blocked_by = open.iter().map(|r| r.other.identifier.as_str()).collect();
        let elsewhere = node.delegate.as_ref().is_some_and(|d| d.id != data.viewer.id);
        children.push((identifier, Child { id: &node.id, finished: state.is_finished(), elsewhere, triage: state == StateType::Triage, behind_triage, blocked_by }));
    }
    let child = |identifier: &str| children.iter().find(|(i, _)| *i == identifier).map(|(_, c)| c);
    let mut added: Vec<(&str, &str)> = Vec::new();
    for (sub, blocker) in blocks {
        for identifier in [sub, blocker] {
            match child(identifier) {
                None => return Err(format!("{identifier} isn't a sub-issue of {}: I chain only those", parent.identifier)),
                Some(c) if c.finished => return Err(format!("{identifier} has finished: there's nothing to chain")),
                // A block on it would hold its sub-issue, and the parent's end, until someone accepts it.
                Some(c) if c.triage => return Err(format!("{identifier} is in Triage: nobody has accepted it, so I don't chain it")),
                Some(_) => {}
            }
        }
        if sub == blocker {
            return Err(format!("{sub} can't wait for itself"));
        }
        let there = child(sub).is_some_and(|c| c.blocked_by.contains(&blocker.as_str()));
        if !there && !added.contains(&(sub.as_str(), blocker.as_str())) {
            added.push((sub, blocker));
        }
    }
    let mut unfinished: Vec<&str> = children.iter().filter(|(_, c)| !c.finished).map(|(i, _)| *i).collect();
    natural_dedup(&mut unfinished, |i| i);
    let blockers_of = |identifier: &str| -> Vec<&str> {
        let there = child(identifier).map(|c| c.blocked_by.clone()).unwrap_or_default();
        there.into_iter().chain(added.iter().filter(|(s, _)| *s == identifier).map(|(_, b)| *b)).collect()
    };
    let there = |identifier: &str| child(identifier).map(|c| c.blocked_by.clone()).unwrap_or_default();
    if let Some(looped) = find_loop(&unfinished, &there) {
        return Err(format!("{}'s sub-issues already wait on each other in a loop through {}: each would wait forever", parent.identifier, looped.join(", ")));
    }
    if let Some(looped) = find_loop(&unfinished, &blockers_of) {
        return Err(format!("those blocks would close a loop through {}: each would wait forever", looped.join(", ")));
    }
    let id_of = |identifier: &str| (child(identifier).map(|c| c.id.to_string()).unwrap_or_default(), identifier.to_string());
    // Stable: the unblocked ones first, each group in the identifiers' order.
    // One waiting on an issue in Triage would hold the parent's end, and so would one waiting on
    // a sub-issue that does: follow the waits until no more join.
    let mut held: Vec<&str> = unfinished.iter().copied().filter(|i| child(i).is_some_and(|c| c.behind_triage)).collect();
    while let Some(next) = unfinished.iter().copied().find(|i| !held.contains(i) && child(i).is_some_and(|c| c.blocked_by.iter().any(|b| held.contains(b)))) {
        held.push(next);
    }
    let (skipped, mut delegate): (Vec<&str>, Vec<&str>) = unfinished.iter().partition(|i| child(i).is_some_and(|c| c.elsewhere || c.triage || held.contains(i)));
    delegate.sort_by_key(|i| !blockers_of(i).is_empty());
    Ok(Chain { blocks: added.iter().map(|(s, b)| (id_of(s), id_of(b))).collect(), delegate: delegate.into_iter().map(id_of).collect(), skipped: skipped.into_iter().map(String::from).collect() })
}

// A blocking loop among `nodes`, following each to those of its blockers, from
// `blockers_of`, that are among them too: the identifiers on it, or None.
fn find_loop<'a>(nodes: &[&'a str], blockers_of: &dyn Fn(&str) -> Vec<&'a str>) -> Option<Vec<&'a str>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        New,
        OnPath,
        Done,
    }
    fn visit<'a>(node: &'a str, nodes: &[&'a str], blockers_of: &dyn Fn(&str) -> Vec<&'a str>, marks: &mut [Mark], path: &mut Vec<&'a str>) -> Option<Vec<&'a str>> {
        let at = nodes.iter().position(|n| *n == node)?;
        match marks[at] {
            Mark::Done => return None,
            Mark::OnPath => return Some(path[path.iter().position(|n| *n == node).unwrap_or_default()..].to_vec()),
            Mark::New => {}
        }
        marks[at] = Mark::OnPath;
        path.push(node);
        for next in blockers_of(node) {
            if let Some(looped) = visit(next, nodes, blockers_of, marks, path) {
                return Some(looped);
            }
        }
        path.pop();
        marks[at] = Mark::Done;
        None
    }
    let mut marks = vec![Mark::New; nodes.len()];
    nodes.iter().find_map(|node| visit(node, nodes, blockers_of, &mut marks, &mut Vec::new()))
}

// The most sub-issues one chain asks the parent's store for, as the relay takes a report
// (MAX_CHILDREN in src/worker/machine.ts).
const CHAIN_MOST: usize = 50;

// `issue chain`: a parent's plan for its sub-issues, set going in one call its worker's
// launch line names. Every check is made on one read before the first write, so a refusal
// writes nothing. Run again, it adds no relation twice, and delegating again is harmless. A sub-issue
// delegated to someone else is skipped, not delegated over, and so is one in Triage.
pub fn issue_chain(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, CHAIN, true)?;
    let (Some(id), Some(_)) = (parsed.first(), parsed.non_empty("session")) else {
        return Err(Error::Usage("issue chain needs the issue's ID and --session".into()));
    };
    let blocks = parse_blocks(parsed.list("block"))?;
    let (asker, owner) = (Asker::parse(&parsed)?, owner(&files()?)?);
    // One chain run, should a spent token run the read and the ask again.
    let run = new_uuid();
    let chain = with_token(agent, |token| {
        let Some(session) = asker.trusted(agent, token, &owner)? else { return Err(Error::Usage("issue chain needs --session <sessionId>".into())) };
        let data = lookup::chain_target(agent, token, id)?;
        let mut chain = plan_chain(&session, id, &data, &blocks).map_err(Error::Failed)?;
        // The parent's store takes the chain before anything is written, and only the
        // sub-issues it says took it are delegated: a refusal writes nothing.
        // `plan_chain` checked it is the session's issue.
        let (viewer, parent_issue_id) = (data.viewer.id.clone(), data.issue.as_ref().map(|i| i.id.clone()).unwrap_or_default());
        let children: Vec<String> = chain.delegate.iter().map(|(issue_id, _)| issue_id.clone()).collect();
        if children.len() > CHAIN_MOST {
            return Err(Error::Failed(format!("{id} has {} sub-issues to delegate; a chain takes at most {CHAIN_MOST}", children.len())));
        }
        if !children.is_empty() {
            let taken = state_of(agent)?.chain_recorded(chain_recorded(&parent_issue_id, &session.id, &run, &children), &children)?;
            let (took, refused): (Vec<_>, Vec<_>) = std::mem::take(&mut chain.delegate).into_iter().partition(|(issue_id, _)| taken.contains(issue_id));
            // No relation is made on a sub-issue the chain didn't take.
            chain.blocks.retain(|((sub, _), (blocker, _))| !refused.iter().any(|(r, _)| r == sub || r == blocker));
            chain.delegate = took;
            chain.skipped.extend(refused.into_iter().map(|(_, identifier)| identifier));
        }
        for ((sub, _), (blocker, _)) in &chain.blocks {
            relate(agent, token, Relation::BlockedBy, sub, blocker)?;
        }
        // Each delegation is recorded in Tolkster's state before Linear starts the
        // sub-issue's session, so that session is trusted (Session::is_owners).
        for (issue_id, _) in &chain.delegate {
            let delegation = Delegation { issue_id: issue_id.clone(), parent_issue_id: parent_issue_id.clone(), session_id: session.id.clone(), at: iso(now_ms()) };
            state_of(agent)?.delegate(&delegation)?;
            update_issue(agent, token, issue_id, json!({ "delegateId": viewer }))?;
        }
        Ok(chain)
    })?;
    let added: Vec<Value> = chain.blocks.iter().map(|((_, sub), (_, blocker))| json!({ "issue": sub, "blockedBy": blocker })).collect();
    let delegated: Vec<&str> = chain.delegate.iter().map(|(_, identifier)| identifier.as_str()).collect();
    print_json(&json!({ "added": added, "delegated": delegated, "skipped": chain.skipped }))?;
    Ok(if delegated.is_empty() { NOTHING_TO_DO } else { 0 })
}

#[cfg(test)]
mod tests;
