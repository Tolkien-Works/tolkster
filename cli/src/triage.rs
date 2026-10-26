// Which issues get triaged, and the start of the session that triages them (docs/triage.md).
// The rules are in code; the session only judges, and its verdict is checked again by
// `tolkster triage` (commands/triage.rs).
//
// Nothing here reads issue text: only identifiers, state types, times, ids, the team's key
// and the project's id and name.

use crate::brief::{self, Checkouts, Found, RUBRIC, open_session, reading_prompt};
use crate::exec::Run;
use crate::linear::{Relation, StateType};
use crate::error::Error;
use crate::estimates::Scale;
use crate::git::{self, Merge};
use crate::orca::Orca;
use crate::plain::{Commit, Id, IssueKey, RepoName};
use crate::config::repo_map::{Missing, Place, RepoMap};
use crate::ports::Candidate;
use crate::sessions::{Other, StateHistory, TriageNode};
use crate::state::{ClaimKind, Store, claim_name, new_token};
use crate::trace::{Refusal, TriageNote};

const TRIAGE: &str = include_str!("../briefs/triage.md");

pub fn parse_candidate(issue: &TriageNode) -> Result<Candidate, Error> {
    let identifier = IssueKey::parse(&issue.identifier).ok_or_else(|| Error::Failed("an issue's identifier isn't plain".into()))?.upper();
    let mut related: Vec<IssueKey> = Relation::Related.related(&issue.relations).into_iter().filter_map(|r| Some(IssueKey::parse(&r.other.identifier)?.upper())).filter(|other| *other != identifier).collect();
    related.sort();
    related.dedup();
    Ok(Candidate {
        identifier,
        related,
        state_type: issue.state.kind,
        creator: issue.creator.as_ref().map(|c| c.id.clone()),
        created_at: Some(issue.created_at.clone()),
        place: Place::of(Some(&issue.team.key), issue.project.as_ref().map(|p| p.parts())),
        scale: Scale::of(&issue.team.estimation),
        archived: issue.archived_at.is_some(),
    })
}

// The issues of the queue that `may_triage`, oldest first.
pub fn queue(candidates: Vec<Candidate>, owner_id: &str, app_user_id: &str) -> Vec<Candidate> {
    let mut queue: Vec<Candidate> = candidates.into_iter().filter(|c| may_triage(c, owner_id, app_user_id).is_ok()).collect();
    queue.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.identifier.cmp(&b.identifier)));
    queue.dedup_by(|a, b| a.identifier == b.identifier);
    queue
}

// Nobody else's issue starts anything. Err says why not. Whether it was filed in Triage
// is its history's to say (`filed_in_triage`).
pub fn may_triage(candidate: &Candidate, owner_id: &str, app_user_id: &str) -> Result<(), Refusal> {
    if candidate.state_type != StateType::Triage {
        return Err(Refusal::NotInTriage);
    }
    if candidate.archived {
        return Err(Refusal::Archived);
    }
    match candidate.creator.as_deref() {
        Some(creator) if creator == owner_id || creator == app_user_id => {}
        _ => return Err(Refusal::NotOurs),
    }
    Ok(())
}

// Whether `history` (sessions::state_history) shows the issue filed in Triage: no entry
// moved it into a triage state from another state, as the owner parks an issue by moving
// it there, which leaves it alone. History past one page counts as moved.
pub fn filed_in_triage(history: &StateHistory) -> bool {
    if history.page_info.has_next_page {
        return false;
    }
    let kind = |state: &Option<crate::linear::StateOf>| state.as_ref().map(|s| s.kind);
    !history.nodes.iter().any(|entry| kind(&entry.to_state) == Some(StateType::Triage) && kind(&entry.from_state).is_some_and(|from| from != StateType::Triage))
}

// A probe's own claim is held this long, so a probe whose give-back fails holds up the
// claim's real taker only briefly.
const PROBE: std::time::Duration = std::time::Duration::from_secs(60);

// A take under a fresh token succeeds only when nobody holds `claim`, and is then given
// back at once.
fn held(store: &dyn Store, claim: &str) -> Result<bool, Error> {
    let fresh = new_token();
    if store.take(&[(claim.to_string(), Some(PROBE))], &fresh)? {
        store.release(&[claim.to_string()], &fresh)?;
        return Ok(false);
    }
    Ok(true)
}

// Taken for good before `tolkster triage` posts its verdict: one verdict per issue.
pub fn verdict_claim(identifier: &IssueKey) -> String {
    claim_name(ClaimKind::Verdict, &identifier.upper())
}

// Whether a verdict on `identifier`, of the team `team_id`, may name `other`: another
// issue, open (neither finished nor archived) and in the same team. Err says why not.
pub fn may_relate(identifier: &str, team_id: &str, other: &Other) -> Result<(), String> {
    let name = other.identifier.to_ascii_uppercase();
    if name.eq_ignore_ascii_case(identifier) {
        return Err(format!("{identifier} can't be related to itself"));
    }
    if other.team.id != team_id {
        return Err(format!("{name} isn't in the same team as {identifier}"));
    }
    if other.state.kind.is_finished() || other.archived_at.is_some() {
        return Err(format!("{name} isn't open"));
    }
    Ok(())
}

// Whether a `fixed` verdict's commit is on the default branch, as `origin/HEAD` names it
// in the checkout the command runs in. Err says why not.
pub fn merged(run: &mut Run, sha: &Commit) -> Result<(), Error> {
    git::on_origin_head(run, sha).map_err(|error| Error::Failed(format!("{sha} isn't on origin/HEAD: {error}")))
}

// `candidates` are oldest first; at most `batch` are taken.
pub struct Start<'a> {
    pub candidates: &'a [Candidate],
    pub batch: usize,
    pub owner_name: &'a str,
    pub briefs_dir: &'a str,
    pub repos: &'a RepoMap,
}

impl Start<'_> {
    // `merges` are those of the issues `issues` are related to, one each.
    pub fn render(&self, repo: &RepoName, issues: &[IssueKey], session: &Id, merges: &[(IssueKey, Merge)]) -> String {
        let (placeholder, rubric) = RUBRIC;
        let listed = issues.iter().map(|i| format!("`{i}`")).collect::<Vec<_>>().join(", ");
        brief::session_brief(TRIAGE)
            .replace(placeholder, rubric.strip_suffix('\n').unwrap_or(rubric))
            .replace("<SOURCES>", &self.sources(issues, merges))
            .replace("<estimate scale>", &self.estimate_scale(issues))
            .replace("<issues>", &listed)
            .replace("<session>", session)
            .replace("<repo displayName>", repo)
            .replace("<OWNER NAME>", self.owner_name)
    }

    // A line each of `issues` with what merged for the issues it is related to, as the log of
    // the default branch said: the merge's SHA, none, or that git couldn't look.
    fn sources(&self, issues: &[IssueKey], merges: &[(IssueKey, Merge)]) -> String {
        let merged = |related: &IssueKey| match merges.iter().find(|(key, _)| key == related).map(|(_, merge)| merge) {
            Some(Merge::Found(sha)) => format!("`{related}` merged as `{sha}` (`git show {sha}`)"),
            Some(Merge::Absent) => format!("`{related}` had no squash merge on the default branch when this brief was written"),
            _ => format!("`{related}`'s merge wasn't looked up: find it with `git log origin/HEAD --grep \"{related}\"`"),
        };
        let lines: Vec<String> = issues
            .iter()
            .map(|issue| {
                let related = self.candidates.iter().find(|c| c.identifier == *issue).map_or(&[][..], |c| &c.related[..]);
                let said = if related.is_empty() { "no related issue".to_string() } else { related.iter().map(merged).collect::<Vec<_>>().join("; ") };
                format!("- `{issue}`: {said}.")
            })
            .collect();
        lines.join("\n   ")
    }

    // What an accept's estimate is for `issues`, from each one's team's scale: one phrase
    // when they share it, else one per issue.
    fn estimate_scale(&self, issues: &[IssueKey]) -> String {
        let scale = |identifier: &IssueKey| self.candidates.iter().find(|c| c.identifier == *identifier).and_then(|c| c.scale);
        let phrase = |scale: Option<Scale>| match scale {
            Some(scale) => match scale.describe() {
                Some(values) => format!("one of {values}"),
                None => "left out, as the team doesn't use estimates".to_string(),
            },
            None => "one on the team's scale, which `tolkster triage` lists if it refuses one".to_string(),
        };
        let scales: Vec<Option<Scale>> = issues.iter().map(scale).collect();
        match scales.split_first() {
            Some((first, rest)) if rest.iter().all(|s| s == first) => phrase(*first),
            _ => issues.iter().zip(scales).map(|(i, s)| format!("for `{i}`, {}", phrase(s))).collect::<Vec<_>>().join("; "),
        }
    }
}

// The most issues one session is given: as many as the Durable Object holds of a session
// (src/worker/machine/validate.ts), so a larger `triage.batch` never has its start refused.
pub const MAX_BATCH: usize = 50;

// The most issues the session's launch line has it read in its first turn, the batch first:
// the line is typed into a terminal, so it stays short, and each merge is a scan of the log.
// One past this is read and looked up by the session itself, from the brief's list.
const MAX_READS: usize = 60;

// A session a start opened: its id, which its brief carries and `tolkster triage` proves
// itself with, its Orca terminal, and the identifiers of the issues it was given.
#[derive(Clone, Debug, PartialEq)]
pub struct Opened {
    pub session: Id,
    pub terminal: String,
    pub issues: Vec<IssueKey>,
}

// What a start came to: the session it opened, if any; the candidates it passed by for now
// (past the batch, in another repo, with no primary checkout or a history it couldn't
// read), by identifier, which a later start takes; and what it came to, for the log.
#[derive(Debug, PartialEq)]
pub struct Started {
    pub opened: Option<Opened>,
    pub waiting: Vec<IssueKey>,
    pub notes: Vec<TriageNote>,
}

// Opens one triage session, in the repo of the first candidate with no verdict, for up to
// `batch` of that repo's. Only the triage machine sends a start, one at a time and while no
// session it holds runs (docs/state-machine.md, Triage), so nothing here orders starts or
// claims an issue. `token` is the session's id. `filed` says whether a candidate was filed
// in Triage, asked only of one the batch reaches with no verdict. No session opened when
// there was nothing to triage; the notes, for the log, say why.
pub fn start(
    start: &Start,
    filed: &mut dyn FnMut(&Candidate) -> Result<bool, Error>,
    orca: &mut dyn Orca,
    write: &mut dyn FnMut(&std::path::Path, &str) -> Result<(), Error>,
    store: &dyn Store,
    token: &Id,
) -> Result<Started, Error> {
    if start.candidates.is_empty() {
        return Ok(Started { opened: None, waiting: Vec::new(), notes: Vec::new() });
    }
    let checkouts = Checkouts::list(orca)?;
    let mut notes = Vec::new();
    let mut eligible: Vec<(&Candidate, Found)> = Vec::new();
    // Settled: taken, triaged already, moved into Triage or with no repo. The rest wait.
    let mut settled: Vec<&IssueKey> = Vec::new();
    for candidate in start.candidates {
        match checkouts.find(start.repos, &candidate.place) {
            Err(Missing::Primary) if !eligible.is_empty() => {}
            Err(missing) => {
                if !matches!(missing, Missing::Primary) {
                    settled.push(&candidate.identifier);
                }
                notes.push(TriageNote::Missing { issue: candidate.identifier.clone(), missing: missing.into() });
            }
            Ok(found) => eligible.push((candidate, found)),
        }
    }
    // The first issue taken with no verdict picks the repo; the candidates of other repos
    // wait for another start. An issue with a verdict, which a declined one keeps in Triage,
    // takes no place, so it never holds back another repo's.
    let mut chosen: Option<(&Found, RepoName)> = None;
    let mut taken: Vec<IssueKey> = Vec::new();
    let mut triaged = 0;
    let mut unread: Option<Error> = None;
    for (candidate, found) in &eligible {
        if taken.len() >= start.batch.min(MAX_BATCH) {
            break;
        }
        if chosen.as_ref().is_some_and(|(c, _)| c.repo_id != found.repo_id) {
            continue;
        }
        let repo = match &chosen {
            Some((_, repo)) => repo.clone(),
            None => RepoName::check(&found.repo)?,
        };
        if held(store, &verdict_claim(&candidate.identifier))? {
            settled.push(&candidate.identifier);
            triaged += 1;
            continue;
        }
        // A history that can't be read skips the issue, not the batch. Kept for when the
        // batch takes nothing, so the start fails and the machine sends it again.
        match filed(candidate) {
            Ok(true) => {}
            Ok(false) => {
                settled.push(&candidate.identifier);
                notes.push(TriageNote::MovedIn { issue: candidate.identifier.clone() });
                continue;
            }
            // Linear's budget spent: the rest would fail too, so the batch goes with what it took.
            Err(error) if error.rate_limited().is_some() => {
                notes.push(TriageNote::History { issue: candidate.identifier.clone(), error: error.clone(), rest_wait: true });
                unread = Some(error);
                break;
            }
            Err(error) => {
                notes.push(TriageNote::History { issue: candidate.identifier.clone(), error: error.clone(), rest_wait: false });
                unread.get_or_insert(error);
                continue;
            }
        }
        settled.push(&candidate.identifier);
        chosen = Some((found, repo));
        taken.push(candidate.identifier.clone());
    }
    if triaged > 0 {
        notes.push(TriageNote::TriagedAlready(triaged));
    }
    let waiting: Vec<IssueKey> = start.candidates.iter().map(|c| c.identifier.clone()).filter(|i| !settled.contains(&i)).collect();
    let Some((Found { primary, path: checkout, .. }, repo)) = chosen else {
        return unread.map_or_else(|| Ok(Started { opened: None, waiting, notes }), Err);
    };
    // The issues each taken one is related to are read and their merges looked up once each,
    // before the session opens, so its first turn reads them and its second decides.
    let related = |issue: &IssueKey| start.candidates.iter().find(|c| c.identifier == *issue).map_or(&[][..], |c| &c.related[..]);
    let mut sources: Vec<&IssueKey> = taken.iter().flat_map(related).collect();
    sources.sort();
    sources.dedup();
    let reads: Vec<&IssueKey> = taken.iter().chain(sources.iter().copied().filter(|source| !taken.contains(source))).take(MAX_READS).collect();
    let path = brief::path(start.briefs_dir, &repo, &format!("triage-{token}"));
    // Before the fetch, so a line that can't be typed costs none.
    let prompt = reading_prompt(&path, true, &reads)?;
    // Those past the cap are left for the session, which the brief says to look up itself.
    let looked = &sources[..sources.len().min(MAX_READS)];
    let merges: Vec<(IssueKey, Merge)> = looked.iter().map(|source| (*source).clone()).zip(git::merges(orca.shell(), checkout, looked)).collect();
    let listed: Vec<&str> = taken.iter().map(IssueKey::as_str).collect();
    let terminal = open_session(orca, write, primary, &format!("triage {}", listed.join(" ")), &path, &prompt, &start.render(&repo, &taken, token, &merges))?;
    notes.push(TriageNote::Started(taken.clone()));
    Ok(Started { opened: Some(Opened { session: token.clone(), terminal, issues: taken }), waiting, notes })
}

#[cfg(test)]
mod tests;
