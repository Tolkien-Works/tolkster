// `tolkster triage <TW-n> --session <id> --verdict <v> ...`: a triage session's one verdict
// on an issue, as Tolkster. The command is the gate, not the brief (docs/triage.md). The
// comment goes under a claim held for good, so a second run exits 3. Should the relations
// or the move after it fail, the comment stands, the failure is printed and it exits 1.

use serde_json::{Value, json};
use ureq::Agent;

use crate::linear::{Relation, StateType, relate, with_token};
use crate::args::{self, Kind, Parsed, parse};
use crate::cli::{NOTHING_TO_DO, print_json, read_body};
use crate::config::{files, owner};
use crate::exec::run_command;
use crate::error::Error;
use crate::estimates::{Estimation, Scale};
use crate::issue_state::{Verdict, guarded_update, in_order, verdict_target};
use crate::issues;
use crate::post::comment_once;
use crate::plain::{Commit, Id};
use crate::state::{State, new_token};
use crate::sessions::{read_other, read_triaged, state_history};
use crate::triage::{filed_in_triage, may_relate, may_triage, merged, parse_candidate, verdict_claim};

// Shape only (usage errors): what the verdict names is checked against Linear in `run`.
pub fn parse_verdict(parsed: &Parsed) -> Result<Verdict, Error> {
    let usage = |why: &str| Error::Usage(why.into());
    let only = |allowed: &[&str]| -> Result<(), Error> {
        for option in ["estimate", "priority", "by", "of"] {
            if parsed.string(option).is_some() && !allowed.contains(&option) {
                return Err(usage(&format!("--{option} doesn't go with this verdict")));
            }
        }
        for option in ["blocked-by", "related"] {
            if !parsed.list(option).is_empty() && !allowed.contains(&option) {
                return Err(usage(&format!("--{option} doesn't go with this verdict")));
            }
        }
        Ok(())
    };
    match parsed.non_empty("verdict") {
        Some("accept") => {
            only(&["estimate", "priority", "blocked-by", "related"])?;
            let Some(priority) = parsed.non_empty("priority") else {
                return Err(usage("accept needs --priority"));
            };
            // Checked against the issue's team's scale in `run`, once it is read.
            let estimate = match parsed.string("estimate").map(issues::estimate).transpose()? {
                None => None,
                Some(Value::Number(n)) => Some(n.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| usage("--estimate must be a whole number of points"))?),
                Some(_) => return Err(usage("--estimate must be a whole number of points")),
            };
            let priority = issues::priority(priority)?;
            if priority == 0 {
                return Err(usage("--priority must be urgent, high, medium or low"));
            }
            // Each once, however often it was named: the same relation twice fails in Linear.
            let mut relations: Vec<(Relation, String)> = Vec::new();
            for (relation, named) in [(Relation::BlockedBy, parsed.list("blocked-by")), (Relation::Related, parsed.list("related"))] {
                for other in named.iter().map(|i| i.to_ascii_uppercase()) {
                    if !relations.iter().any(|(r, o)| *r == relation && *o == other) {
                        relations.push((relation, other));
                    }
                }
            }
            Ok(Verdict::Accept { estimate, priority, relations })
        }
        Some("fixed") => {
            only(&["by"])?;
            let by = parsed.non_empty("by").ok_or_else(|| usage("fixed needs --by <sha>"))?;
            let by = Commit::parse(&by.to_ascii_lowercase()).ok_or_else(|| Error::Usage(format!("--by must be a commit's SHA, not {by:?}")))?;
            Ok(Verdict::Fixed { by })
        }
        Some("duplicate") => {
            only(&["of"])?;
            let of = parsed.non_empty("of").ok_or_else(|| usage("duplicate needs --of <TW-456>"))?;
            Ok(Verdict::Duplicate { of: of.to_ascii_uppercase() })
        }
        Some("decline") => {
            only(&[])?;
            Ok(Verdict::Decline)
        }
        _ => Err(usage("--verdict must be accept, fixed, duplicate or decline")),
    }
}

pub const OPTIONS: &[(&str, Kind)] = &[
    ("session", Kind::String),
    ("verdict", Kind::String),
    ("body", Kind::String),
    ("body-file", Kind::String),
    ("estimate", Kind::String),
    ("priority", Kind::String),
    ("blocked-by", Kind::List),
    ("related", Kind::List),
    ("by", Kind::String),
    ("of", Kind::String),
];

// An accept's estimate is one on the team's scale when it uses estimates, none when it
// doesn't. A usage error otherwise.
pub fn accepted_estimate(team: &Estimation, estimate: Option<u32>) -> Result<(), Error> {
    let scale = Scale::of(team).ok_or_else(|| Error::Failed("the issue's team's estimate scale couldn't be read".into()))?;
    match (estimate, scale.describe()) {
        (Some(points), _) => scale.check(points).map_err(Error::Usage),
        (None, Some(values)) => Err(Error::Usage(format!("accept needs --estimate on the issue's team's scale: {values}"))),
        (None, None) => Ok(()),
    }
}

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, OPTIONS, true)?;
    let Some(id) = parsed.first() else { return Err(Error::Usage("triage needs the issue's ID".into())) };
    let Some(session) = parsed.non_empty("session").and_then(Id::parse) else {
        return Err(Error::Usage("--session <id> is required: the triage session's id, from its brief".into()));
    };
    let verdict = parse_verdict(&parsed)?;
    let body = args::body(&parsed, "comment", read_body)?;
    let files = files()?;
    let owner = owner(&files)?;

    // The issue, read fresh: still in Triage, filed there, and the owner's or Tolkster's.
    let data = read_triaged(agent, id)?;
    let Some(read) = data.issue else { return Err(Error::Failed(format!("no issue {id}"))) };
    let viewer_id = data.viewer.id;
    let issue = &read.issue;
    let candidate = parse_candidate(issue)?;
    let identifier = candidate.identifier.clone();
    may_triage(&candidate, &owner, &viewer_id).map_err(|why| Error::Failed(format!("{identifier} {why}")))?;
    if !filed_in_triage(&state_history(agent, &identifier)?) {
        return Err(Error::Failed(format!("{identifier} was moved into Triage after it was created")));
    }
    // TRIAGED asks for the team's ID and states, which a queue's read leaves out.
    let unread = || Error::Failed("Linear's answer to Triaged didn't decode: its team had no `id` or `states`".into());
    let (issue_id, team_id) = (read.id.clone(), issue.team.id.clone().ok_or_else(unread)?);
    if let Verdict::Accept { estimate, .. } = &verdict {
        accepted_estimate(&issue.team.estimation, *estimate)?;
    }
    let states = in_order(issue.team.states.as_ref().ok_or_else(unread)?.nodes.clone());
    let target = verdict_target(&states, &verdict)?;

    // This session's to triage.
    let state = State::open(agent, &files)?;
    holds(&state, &identifier, &session)?;

    let named: Vec<(Relation, String)> = match &verdict {
        Verdict::Accept { relations, .. } => relations.clone(),
        Verdict::Duplicate { of } => vec![(Relation::DuplicateOf, of.clone())],
        Verdict::Fixed { .. } | Verdict::Decline => Vec::new(),
    };
    let mut relations: Vec<(Relation, String, String)> = Vec::new();
    for (relation, other) in &named {
        let found = read_other(agent, other)?.ok_or_else(|| Error::Failed(format!("no issue {other}")))?;
        may_relate(&identifier, &team_id, &found).map_err(Error::Failed)?;
        relations.push((*relation, found.id, found.identifier));
    }
    if let Verdict::Fixed { by } = &verdict {
        merged(&mut run_command, by)?;
    }

    // The verdict itself, once: the claim is given back only when the comment wasn't
    // posted (state::holding), so trying again can post it.
    let posted = comment_once(agent, &state, &issue_id, None, &body, &[verdict_claim(&identifier)], &new_token())?;
    if posted.is_none() {
        // Refused while a start probed the verdict claim too, which it does only once the
        // triage machine no longer holds this session as running: that is no verdict.
        holds(&state, &identifier, &session)?;
        eprintln!("{identifier}: has my verdict already");
        return Ok(NOTHING_TO_DO);
    }

    // Then what follows from it. The comment stands whatever happens here.
    let mut failures: Vec<String> = Vec::new();
    for (relation, other_id, other) in &relations {
        if let Err(error) = with_token(agent, |token| relate(agent, token, *relation, &issue_id, other_id)) {
            failures.push(format!("the relation to {other} wasn't added: {error}"));
        }
    }
    let mut input = serde_json::Map::new();
    if let Some(target) = target {
        input.insert("stateId".into(), json!(target.id));
    }
    if let Verdict::Accept { estimate, priority, .. } = &verdict {
        if let Some(estimate) = estimate {
            input.insert("estimate".into(), json!(estimate));
        }
        input.insert("priority".into(), json!(priority));
    }
    if !input.is_empty() {
        let moved = with_token(agent, |token| {
            guarded_update(agent, token, &issue_id, |current| {
                if current.state_type != StateType::Triage {
                    return Err(Error::Failed("it left Triage meanwhile".into()));
                }
                Ok(Some(Value::Object(input.clone())))
            })
        });
        if let Err(error) = moved {
            failures.push(format!("it wasn't moved: {error}"));
        }
    }
    if !failures.is_empty() {
        return Err(Error::Failed(format!("commented on {identifier}, but {}", failures.join("; "))));
    }
    print_json(&json!({ "issue": identifier, "verdict": verdict.name(), "state": target.map(|t| json!(t.name)).unwrap_or(Value::Null) }))?;
    Ok(0)
}

// This session's to triage: the one the triage machine holds as running, given the issue
// when it started (docs/triage.md, "The command is the gate").
fn holds(state: &State, identifier: &str, session: &str) -> Result<(), Error> {
    match state.triaging(session, identifier)? {
        true => Ok(()),
        false => Err(Error::Failed(format!("{identifier} isn't the running triage session's {session} to triage: it wasn't given the issue, or has ended"))),
    }
}

#[cfg(test)]
mod tests;
