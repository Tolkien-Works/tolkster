// `tolkster issue show`: an issue read as Tolkster, printed as JSON. What people wrote
// reaches an agent through this print, `messages` or `pr feedback`, in its own terminal:
// never through `tolkster listen`, a prompt or a brief, which carry only the owner's
// own messages (AGENTS.md).

use serde_json::{Value, json};
use ureq::Agent;

use crate::args::{Kind, parse};
use crate::cli::print_exit;
use crate::error::Error;
use crate::issues::{RELATIONS, Relations};
use crate::json::{decode_ref, nodes};
use crate::sessions::shown::Show;
use crate::plain::split_identifier;

pub const OPTIONS: &[(&str, Kind)] = &[("comments", Kind::Boolean), ("children", Kind::Boolean), ("attachments", Kind::Boolean), ("images", Kind::Boolean)];

// Several IDs are read at once, so a session's first reads wait on one round trip, not one
// per issue, and print as an array in the order given. One that failed keeps its place, so
// a reader skimming the array can't miss it, and exits 1.
pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, OPTIONS, true)?;
    let given: Vec<&str> = parsed.positionals.iter().map(String::as_str).filter(|id| !id.is_empty()).collect();
    let mut seen = std::collections::HashSet::new();
    let ids: Vec<&str> = given.iter().copied().filter(|id| seen.insert(*id)).collect();
    if ids.is_empty() {
        return Err(Error::Usage("issue show needs an issue's ID".into()));
    }
    let (comments, children, attachments, images) = (parsed.flag("comments"), parsed.flag("children"), parsed.flag("attachments"), parsed.flag("images"));
    let show = Show::new(comments, children, attachments);
    let mut reads = at_once(ids.iter().map(|id| {
        let show = &show;
        move || -> Result<Value, Error> {
            let issue = show.read(agent, id)?.ok_or_else(|| Error::Failed(format!("no issue {id}")))?;
            let mut out = render(&issue, comments, children, attachments)?;
            if images {
                out["images"] = Value::Array(crate::commands::images::fetch(agent, &issue)?);
            }
            Ok(out)
        }
    }));
    // One ID prints its object, as it always has, or fails; several print an array, even
    // when they name one issue.
    if given.len() == 1 {
        return print_exit(&reads.remove(0)?, 0);
    }
    let mut code = 0;
    let shown: Vec<Value> = ids.iter().zip(reads).map(|(id, read)| read.unwrap_or_else(|error| {
        eprintln!("{error}");
        code = 1;
        failed(id)
    })).collect();
    print_exit(&shown, code)
}

// A failed read's place in the array. Fixed text: the error, on stderr, may hold Linear's.
pub fn failed(id: &str) -> Value {
    json!({ "identifier": id, "error": "not read: see stderr" })
}

// At most this many reads run at once: the gateway admits each from counts that earlier
// answers noted, so a burst all admitted together could overspend the budget `listen` shares.
const AT_ONCE: usize = 6;

// Runs the reads on threads of their own, `AT_ONCE` at a time, and returns what they
// returned in order.
pub fn at_once<T: Send, F: FnOnce() -> T + Send>(reads: impl IntoIterator<Item = F>) -> Vec<T> {
    let mut reads: Vec<F> = reads.into_iter().collect();
    let mut done = Vec::with_capacity(reads.len());
    while !reads.is_empty() {
        let batch: Vec<F> = reads.drain(..reads.len().min(AT_ONCE)).collect();
        done.extend(std::thread::scope(|scope| {
            let running: Vec<_> = batch.into_iter().map(|read| scope.spawn(read)).collect();
            running.into_iter().map(|read| read.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))).collect::<Vec<T>>()
        }));
    }
    done
}

// What is printed: the issue, and each list the flags asked for, or nothing for it. The issue
// is passed through as Linear answered it (shown::Show), so its shape is open: what is read
// here only picks and orders what is printed, and decides nothing.
pub fn render(issue: &Value, comments: bool, children: bool, attachments: bool) -> Result<Value, Error> {
    let mut out = json!({ "issue": render_issue(issue)? });
    if comments {
        out["comments"] = Value::Array(render_comments(issue));
    }
    if children {
        out["children"] = Value::Array(render_children(issue)?);
    }
    if attachments {
        out["attachments"] = Value::Array(render_links(issue));
    }
    Ok(out)
}

// An issue's fields, as read. A field Linear left null stays null.
fn render_issue(issue: &Value) -> Result<Value, Error> {
    let person = |pointer: &str, name: &str| match issue.pointer(pointer) {
        Some(p) if !p.is_null() => json!({ "id": p["id"], name: p[name] }),
        _ => Value::Null,
    };
    Ok(json!({
        "id": issue["id"], "identifier": issue["identifier"], "title": issue["title"], "url": issue["url"], "description": issue["description"],
        "state": { "name": issue["state"]["name"], "type": issue["state"]["type"] },
        "team": { "key": issue["team"]["key"], "name": issue["team"]["name"] },
        "project": issue["project"]["name"],
        "assignee": person("/assignee", "displayName"),
        "delegate": person("/delegate", "name"),
        "labels": nodes(issue, "/labels/nodes").iter().map(|l| l["name"].clone()).collect::<Vec<_>>(),
        "priority": issue["priority"], "priorityLabel": issue["priorityLabel"], "estimate": issue["estimate"], "dueDate": issue["dueDate"],
        "branchName": issue["branchName"],
        "parent": if issue["parent"].is_null() { Value::Null } else { json!({ "identifier": issue["parent"]["identifier"], "title": issue["parent"]["title"], "url": issue["parent"]["url"] }) },
        "relations": render_relations(issue)?,
        "createdAt": issue["createdAt"], "updatedAt": issue["updatedAt"], "completedAt": issue["completedAt"], "canceledAt": issue["canceledAt"],
    }))
}

// Both ends of the issue's relations, each as `tolkster issue relate` names it: what this
// issue blocks, is blocked by, is related to, or duplicates, with the other issue's state.
fn render_relations(issue: &Value) -> Result<Vec<Value>, Error> {
    let relations: Relations<Value> = decode_ref(issue, "Linear's answer to Show")?;
    Ok(RELATIONS
        .iter()
        .flat_map(|(name, relation)| {
            relation.related(&relations).into_iter().map(move |r| json!({ "type": name, "identifier": r.other["identifier"], "url": r.other["url"], "state": r.other["state"]["type"] }))
        })
        .collect())
}

// The comments, oldest first, each with the comment it replies to, if any.
fn render_comments(issue: &Value) -> Vec<Value> {
    let mut comments: Vec<&Value> = nodes(issue, "/comments/nodes").iter().collect();
    // Linear's timestamps share one ISO format, so text order is time order.
    comments.sort_by(|a, b| a["createdAt"].as_str().cmp(&b["createdAt"].as_str()));
    comments
        .into_iter()
        .map(|c| {
            json!({
                "id": c["id"], "body": c["body"], "createdAt": c["createdAt"], "updatedAt": c["updatedAt"],
                "parentId": c["parent"]["id"],
                "user": if c["user"].is_null() { Value::Null } else { json!({ "id": c["user"]["id"], "displayName": c["user"]["displayName"] }) },
            })
        })
        .collect()
}

// The sub-issues, in the order the parent lists them, each with its links, so a worker
// can find a sub-issue's PR without another read.
fn render_children(issue: &Value) -> Result<Vec<Value>, Error> {
    let mut children: Vec<&Value> = nodes(issue, "/children/nodes").iter().collect();
    children.sort_by(|a, b| {
        let order = |c: &Value| c["sortOrder"].as_f64();
        let number = |c: &Value| c["identifier"].as_str().and_then(split_identifier).map(|(_, n)| n);
        order(a).partial_cmp(&order(b)).unwrap_or(std::cmp::Ordering::Equal).then_with(|| number(a).cmp(&number(b)))
    });
    children
        .into_iter()
        .map(|c| {
            let mut child = render_issue(c)?;
            child["attachments"] = Value::Array(render_links(c));
            Ok(child)
        })
        .collect()
}

// An issue's links: its PRs, and whatever else was attached.
fn render_links(issue: &Value) -> Vec<Value> {
    nodes(issue, "/attachments/nodes").iter().map(|a| json!({ "title": a["title"], "url": a["url"] })).collect()
}

#[cfg(test)]
mod tests;
