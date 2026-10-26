// Every call to git but `update.rs`'s, which hides git's stderr on purpose: fetch, the default
// tip, history, status and diff reads, what a branch last pushed, detach, clean
// and delete, a landing's squash and rebase, push, the remote's URL, the worker's hook config and a new host's clone. They
// run in the checkout at `dir`, or the process's own without one, and `listen` shows those
// that go through `git_at` with a verb in `WRITES`. Every name reaches git
// as one argument, never through a shell, and a branch as a `BranchName`.

use std::sync::{Mutex, MutexGuard};

use crate::error::Error;
use crate::exec::{Run, call, run_command_with};
use crate::plain::{BranchName, Commit, IssueKey, Sha, Time};
use crate::sync::lock;
use crate::trace::{self, Call};

// What writes a repo's shared refs, config or worktree list runs one at a time across
// `listen`'s lanes (docs/realtime.md, "Lanes"). Taken last of the lanes' locks, around
// steps that take no other lock and never wait on another lane. Static, as every git write in
// the process takes it, from whatever runs it.
static WRITING: Mutex<()> = Mutex::new(());

pub fn writing() -> MutexGuard<'static, ()> {
    lock(&WRITING)
}

// A transfer that stalls for a minute is given up: every caller holds a claim or a lock
// meanwhile.
const STALL_LIMITS: [&str; 4] = ["-c", "http.lowSpeedLimit=1000", "-c", "http.lowSpeedTime=60"];

fn git(run: &mut Run, dir: &str, args: &[&str]) -> Result<String, Error> {
    git_at(run, Some(dir), args)
}

// `dir` None is the directory the process runs in.
fn git_at(run: &mut Run, dir: Option<&str>, args: &[&str]) -> Result<String, Error> {
    let result = call(run, "git", args, dir);
    // Git's error can carry a remote URL, and with it a credential: not shown.
    if let Some((site, detail)) = written(args, dir) {
        trace::event(Call::Git { site, detail, ok: result.is_ok() });
    }
    result.map(|out| out.trim().to_string())
}

const WRITES: [&str; 6] = ["branch", "fetch", "switch", "config", "clean", "push"];

// What a git command changes, for `listen`'s line, as its site and its arguments, or None for
// a read.
fn written(args: &[&str], dir: Option<&str>) -> Option<(String, String)> {
    let at = args.chunks(2).take_while(|pair| pair[0] == "-c" && pair.len() == 2).count() * 2;
    let verb = args.get(at).filter(|verb| WRITES.contains(verb))?;
    let rest: Vec<&str> = args[..at].iter().chain(&args[at + 1..]).copied().collect();
    Some((format!("git {verb}"), format!("{}{}", rest.join(" "), dir.map_or(String::new(), |dir| format!(" in {dir}")))))
}

// Git's error can carry the remote's URL, and with it a credential, so a failure of the
// fetch or the push says only that it failed.
pub fn fetch(run: &mut Run, dir: &str) -> Result<(), Error> {
    let _writing = writing();
    git(run, dir, &[&STALL_LIMITS[..], &["fetch", "--quiet", "origin"]].concat())
        .map(drop)
        .map_err(|_| Error::Failed("fetching origin failed".into()))
}

// The SHA of the remote's default branch, as the clone recorded it, else `main`'s, as last
// fetched.
pub fn default_tip(run: &mut Run, dir: &str) -> Result<Sha, Error> {
    let tip = match git(run, dir, &["rev-parse", "--verify", "--quiet", "origin/HEAD^{commit}"]) {
        Ok(tip) => tip,
        Err(_) => git(run, dir, &["rev-parse", "--verify", "--quiet", "origin/main^{commit}"])?,
    };
    sha(tip)
}

// Whether `commit` is on the remote's default branch as last fetched (`default_tip`). Err
// when git couldn't say.
pub fn on_default(run: &mut Run, dir: &str, commit: &Sha) -> Result<bool, Error> {
    let tip = default_tip(run, dir)?;
    let output = run("git", &["merge-base", "--is-ancestor", commit, &tip], Some(dir))?;
    match output.code {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(Error::Failed(output.stderr.trim().to_string())),
    }
}

fn sha(said: String) -> Result<Sha, Error> {
    Sha::parse(&said).ok_or_else(|| Error::Failed(format!("git named no commit: {said}")))
}

// One move of HEAD, by its reflog: the commit it moved to, when, in seconds, and git's line
// for why, as `commit: <subject>`.
#[derive(Debug, PartialEq)]
pub struct Move {
    pub sha: Sha,
    pub at: i64,
    pub why: String,
}

// HEAD's newest `n` reflog entries, newest first; a line in another shape is left out.
pub fn last_moves(run: &mut Run, dir: &str, n: u32) -> Result<Vec<Move>, Error> {
    let said = git(run, dir, &["log", "-g", &format!("-{n}"), "--date=unix", "--format=%H%x09%gd%x09%gs", "HEAD"])?;
    Ok(said
        .lines()
        .filter_map(|line| {
            let (sha, rest) = line.split_once('\t')?;
            let (at, why) = rest.strip_prefix("HEAD@{")?.split_once("}\t")?;
            Some(Move { sha: Sha::parse(sha)?, at: at.parse().ok()?, why: why.to_string() })
        })
        .collect())
}

// The commit HEAD is at.
pub fn head(run: &mut Run, dir: &str) -> Result<Sha, Error> {
    sha(git(run, dir, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])?)
}

// The checked-out branch's name, None when HEAD is detached or the name isn't plain.
pub fn current_branch(run: &mut Run, dir: &str) -> Result<Option<BranchName>, Error> {
    Ok(BranchName::parse(&git(run, dir, &["rev-parse", "--abbrev-ref", "HEAD"])?).filter(|name| name != "HEAD"))
}

// The checked-out branch and the SHA its upstream is at, which `git push -u` and the
// pushes after it leave there: what the checkout last pushed. Err when HEAD is no branch or
// the branch has no upstream.
pub fn pushed(run: &mut Run, dir: &str) -> Result<(String, Sha), Error> {
    let branch = git(run, dir, &["symbolic-ref", "--short", "HEAD"])?;
    let upstream = sha(git(run, dir, &["rev-parse", "--verify", "--quiet", "@{upstream}^{commit}"])?)?;
    Ok((branch, upstream))
}

// The newest first-parent commit of `tip` at or before `time`, None when there is none.
pub fn last_before(run: &mut Run, dir: &str, time: &Time, tip: &Sha) -> Result<Option<Sha>, Error> {
    let said = git(run, dir, &["rev-list", "-1", "--first-parent", &format!("--before={time}"), tip])?;
    if said.is_empty() { Ok(None) } else { sha(said).map(Some) }
}

// The newest first-parent commit of `tip` whose subject ends as a squash merge's does,
// `… (TW-1) (#2)`, or a landing's, `… (TW-1)` (docs/landing.md), with `identifier` as a whole
// word in that last tag; None when there is none.
// The message search only narrows the log: a body or a subject that merely mentions the issue,
// or a revert of its merge (whose title ends in a quote), never counts. Only the newest merge
// is named, though an issue may have shipped in several.
pub fn merge_of(run: &mut Run, dir: &str, tip: &Sha, identifier: &IssueKey) -> Result<Option<Sha>, Error> {
    let said = git(run, dir, &["log", "--first-parent", "--fixed-strings", &format!("--grep={identifier}"), "--format=%H%x09%s", tip])?;
    let names = |subject: &str| {
        let Some(tag) = last_tag(subject) else { return false };
        tag.match_indices(identifier.as_str()).any(|(at, _)| {
            let around = [tag[..at].chars().next_back(), tag[at + identifier.len()..].chars().next()];
            around.into_iter().flatten().all(|c| !(c.is_ascii_alphanumeric() || c == '-'))
        })
    };
    Ok(said.lines().filter_map(|line| line.split_once('\t')).find(|(_, subject)| names(subject)).and_then(|(sha, _)| Sha::parse(sha)))
}

// The last parenthesised tag of a squash merge's subject, `TW-1` of `… (TW-1) (#2)`, or a
// landing's, `… (TW-1)`.
fn last_tag(subject: &str) -> Option<&str> {
    let head = match subject.rsplit_once(" (#") {
        Some((head, number)) if number.strip_suffix(')').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) => head,
        _ => subject,
    };
    head.strip_suffix(')').and_then(|head| head.rsplit_once('(')).map(|(_, tag)| tag)
}

// The issues the first-parent commits after `from` up to `to` name in their subjects' last
// tags (`last_tag`), each once, upper-cased, newest first: `to`'s own alone when `from` is
// None or isn't one of its ancestors, as after a rewrite of main.
#[cfg_attr(not(feature = "develop"), allow(dead_code))]
pub fn landed(run: &mut Run, dir: &str, from: Option<&Sha>, to: &Sha) -> Result<Vec<IssueKey>, Error> {
    let range = match from {
        Some(from) if run("git", &["merge-base", "--is-ancestor", from, to], Some(dir))?.code == Some(0) => format!("{from}..{to}"),
        _ => format!("{to}^!"),
    };
    let said = git(run, dir, &["log", "--first-parent", "--format=%s", &range])?;
    let mut keys: Vec<IssueKey> = Vec::new();
    for word in said.lines().filter_map(last_tag).flat_map(|tag| tag.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))) {
        let Some((team, number)) = word.split_once('-') else { continue };
        let named = team.starts_with(|c: char| c.is_ascii_alphabetic()) && !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit());
        if let Some(key) = IssueKey::parse(word).filter(|_| named).map(|key| key.upper())
            && !keys.contains(&key)
        {
            keys.push(key);
        }
    }
    Ok(keys)
}

// What a brief says of an issue's merge. `Unknown` when git couldn't look, so the session
// looks itself: a session never waits on the owner's checkout or network.
#[derive(Clone, Debug, PartialEq)]
pub enum Merge {
    Found(Sha),
    Absent,
    Unknown,
}

// The squash merge of each of `identifiers` on the default branch as origin has it now, in
// their order: fetched once first, so a merge that just landed is found, and not at all for
// none. Every one `Unknown` when the fetch or the tip failed.
pub fn merges(run: &mut Run, dir: &str, identifiers: &[&IssueKey]) -> Vec<Merge> {
    if identifiers.is_empty() {
        return Vec::new();
    }
    let Ok(tip) = fetch(run, dir).and_then(|()| default_tip(run, dir)) else {
        return identifiers.iter().map(|_| Merge::Unknown).collect();
    };
    identifiers
        .iter()
        .map(|identifier| match merge_of(run, dir, &tip, identifier) {
            Ok(Some(sha)) => Merge::Found(sha),
            Ok(None) => Merge::Absent,
            Err(_) => Merge::Unknown,
        })
        .collect()
}

// The first-parent commits after `from` up to `to`, or all of `to`'s when `from` is None.
pub fn count_first_parent(run: &mut Run, dir: &str, from: Option<&Sha>, to: &Sha) -> Result<u32, Error> {
    let range = match from {
        Some(from) => format!("{from}..{to}"),
        None => to.to_string(),
    };
    let counted = git(run, dir, &["rev-list", "--count", "--first-parent", &range])?;
    counted.parse().map_err(|_| Error::Failed(format!("git counted {counted} commits")))
}

// Which of `names` exist at the root of `tip`, in the order git lists them, from any
// directory of the checkout.
pub fn files_at(run: &mut Run, dir: &str, tip: &Commit, names: &[&str]) -> Result<Vec<String>, Error> {
    let listed = git(run, dir, &[&["ls-tree", "--full-tree", "--name-only", tip, "--"], names].concat())?;
    Ok(listed.lines().map(str::to_string).collect())
}

// A file's text at `tip`.
pub fn show(run: &mut Run, dir: &str, tip: &Commit, name: &str) -> Result<String, Error> {
    git(run, dir, &["show", &format!("{tip}:{name}")])
}

// The files HEAD changes since it left `tip`, both sides of a rename.
pub fn changed_files(run: &mut Run, dir: &str, tip: &Sha) -> Result<Vec<String>, Error> {
    let listed = git(run, dir, &["-c", "core.quotePath=off", "diff", "--name-only", "--no-renames", &format!("{tip}...HEAD")])?;
    Ok(listed.lines().filter(|name| !name.is_empty()).map(str::to_string).collect())
}

// What HEAD changes since it left `tip`, without context and whatever the user's diff
// settings, for review_pass::diff. Every file as text, so no `.gitattributes` on the
// branch can hide its lines.
pub fn diff_lines(run: &mut Run, dir: &str, tip: &Sha) -> Result<String, Error> {
    git(run, dir, &["-c", "core.quotePath=off", "diff", "--no-renames", "--no-color", "--no-ext-diff", "--no-textconv", "--text", "--src-prefix=a/", "--dst-prefix=b/", "-U0", &format!("{tip}...HEAD")])
}

// The root of the checkout the process runs in.
pub fn toplevel(run: &mut Run) -> Result<String, Error> {
    git_at(run, None, &["rev-parse", "--show-toplevel"])
}

// The URL of `origin` in the checkout at `dir`, or the one the process runs in.
pub fn origin_url(run: &mut Run, dir: Option<&str>) -> Result<String, Error> {
    git_at(run, dir, &["remote", "get-url", "origin"])
}

// Ok when `commit` is on `origin/HEAD`, as the checkout the process runs in has it. Err
// carries git's reason.
pub fn on_origin_head(run: &mut Run, commit: &Commit) -> Result<(), Error> {
    git_at(run, None, &["merge-base", "--is-ancestor", commit, "origin/HEAD"]).map(drop)
}

// What `git status --porcelain` prints, untrimmed: a line starts with two status columns,
// either blank. `--ignored` lists what `.gitignore` hides, and `--untracked-files=normal`
// pins what `status.showUntrackedFiles` in any git config would change.
pub fn status_porcelain(run: &mut Run, dir: &str) -> Result<String, Error> {
    call(run, "git", &["status", "--porcelain", "--ignored", "--untracked-files=normal"], Some(dir))
}

// How many commits on HEAD or on `branch` are on no remote. Err when `branch` is missing;
// None when git's count isn't a number.
pub fn unpushed(run: &mut Run, dir: &str, branch: &BranchName) -> Result<Option<u64>, Error> {
    let counted = git(run, dir, &["rev-list", "--count", "HEAD", &format!("refs/heads/{branch}"), "--not", "--remotes"])?;
    Ok(counted.parse().ok())
}

// How many commits origin's copy of `branch`, as last fetched, holds that its default
// branch doesn't: none when origin has no such branch. A missing `origin/HEAD` counts every
// commit on it. None when git's count isn't a number.
pub fn pushed_work(run: &mut Run, dir: &str, branch: &BranchName) -> Result<Option<u64>, Error> {
    let counted = git(run, dir, &["rev-list", "--count", "--ignore-missing", &format!("refs/remotes/origin/{branch}"), "--not", "origin/HEAD"])?;
    Ok(counted.parse().ok())
}

// A new worker's checkout moved onto the branch of the issue's earlier worker, as last fetched,
// when origin holds work of it: the newest branch named as Orca names a worker's, `<prefix>/<key>`
// and then `<prefix>/<key>-2` and on, that has commits its default branch lacks, a branch
// Orca had to number because the earlier one's is on origin. Only a checkout with no commit of
// its own and no change to a tracked file is moved, so it loses nothing. The branch it
// moved onto, or None.
pub fn continue_issue_branch(run: &mut Run, dir: &str, key: &IssueKey) -> Result<Option<BranchName>, Error> {
    let own = git(run, dir, &["rev-list", "--count", "HEAD", "--not", "origin/HEAD"])?;
    if own != "0" || uncommitted(run, dir)? {
        return Ok(None);
    }
    let listed = git(run, dir, &["for-each-ref", "--sort=-committerdate", "--format=%(refname:lstrip=3)", "refs/remotes/origin"])?;
    for branch in listed.lines().filter_map(BranchName::parse) {
        if issue_branch(&branch, key) && pushed_work(run, dir, &branch)?.is_some_and(|n| n > 0) {
            git(run, dir, &["reset", "--hard", "--quiet", &format!("refs/remotes/origin/{branch}")])?;
            return Ok(Some(branch));
        }
    }
    Ok(None)
}

// Whether `branch` is named as Orca names a worker's for the issue, `<prefix>/<key>` and then
// `<prefix>/<key>-2` and on, ignoring case.
pub fn issue_branch(branch: &str, key: &IssueKey) -> bool {
    let (last, key) = (branch.rsplit('/').next().unwrap_or_default().to_ascii_lowercase(), key.to_ascii_lowercase());
    let numbered = last.strip_prefix(&format!("{key}-")).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    last == key || numbered
}

// Whether a tracked file has a change not committed, staged or not.
pub fn uncommitted(run: &mut Run, dir: &str) -> Result<bool, Error> {
    Ok(!git(run, dir, &["status", "--porcelain", "--untracked-files=no"])?.is_empty())
}

// HEAD's subject line.
pub fn subject(run: &mut Run, dir: &str) -> Result<String, Error> {
    git(run, dir, &["log", "-1", "--format=%s", "HEAD"])
}

// The name of the remote's default branch, as the clone recorded it, else `main`.
pub fn default_branch(run: &mut Run, dir: &str) -> Result<BranchName, Error> {
    let recorded = git(run, dir, &["symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD"]).ok();
    let name = recorded.as_deref().and_then(|name| name.strip_prefix("origin/")).unwrap_or("main");
    BranchName::parse(name).ok_or_else(|| Error::Failed(format!("origin's default branch has a name git can't take plainly: {name}")))
}

// Every commit HEAD has since it left `tip` squashed into one, whose message is `subject`
// and then `body` as written: `--cleanup=whitespace` keeps a markdown heading's `#`. The
// commit hooks run, so the co-author trailer goes on it (coauthor.rs). A failed commit puts
// HEAD back where it was.
pub fn squash(run: &mut Run, dir: &str, tip: &Sha, subject: &str, body: &str) -> Result<Sha, Error> {
    let was = head(run, dir)?;
    let base = git(run, dir, &["merge-base", "HEAD", tip])?;
    git(run, dir, &["reset", "--quiet", "--soft", &base])?;
    let mut args = vec!["commit", "--quiet", "--cleanup=whitespace", "-m", subject];
    if !body.trim().is_empty() {
        args.extend(["-m", body]);
    }
    if let Err(error) = git(run, dir, &args) {
        git(run, dir, &["reset", "--quiet", "--soft", &was])?;
        return Err(error);
    }
    head(run, dir)
}

// Whether `branch`'s own reflog shows it was moved to `commit` by a commit or a rebase made in
// this repo, not created or reset there.
pub fn made_on(run: &mut Run, dir: &str, branch: &BranchName, commit: &Sha) -> Result<bool, Error> {
    let said = git(run, dir, &["reflog", "show", "--format=%H%x09%gs", &format!("refs/heads/{branch}")])?;
    Ok(said.lines().filter_map(|line| line.split_once('\t')).any(|(sha, why)| sha == commit.as_str() && (why.starts_with("commit") || why.starts_with("rebase"))))
}

#[derive(Debug, PartialEq)]
pub enum Rebased {
    Clean,
    // The paths that conflicted. The rebase was aborted, leaving the branch as it was.
    Conflicts(Vec<String>),
}

// HEAD's commits replayed onto `tip`.
pub fn rebase(run: &mut Run, dir: &str, tip: &Sha) -> Result<Rebased, Error> {
    if run("git", &["rebase", "--quiet", tip], Some(dir))?.ok {
        return Ok(Rebased::Clean);
    }
    let conflicted = git(run, dir, &["-c", "core.quotePath=off", "diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
    // Only once a rebase has begun is there one to abort.
    let _ = git(run, dir, &["rebase", "--abort"]);
    let paths: Vec<String> = conflicted.lines().filter(|name| !name.is_empty()).map(str::to_string).collect();
    if paths.is_empty() {
        return Err(Error::Failed(format!("rebasing onto {tip} failed")));
    }
    Ok(Rebased::Conflicts(paths))
}

#[derive(Debug, PartialEq)]
pub enum Push {
    Pushed,
    // Origin's branch has commits HEAD lacks: fetch, rebase and push again.
    Behind,
}

// HEAD pushed onto origin's `branch`, fast-forward only. `--porcelain` marks a ref git refused
// `!`, and its own fast-forward check says `[rejected]`; the remote's refusal, `[remote
// rejected]`, is a failure. As with `fetch`, a failure says only that it failed.
pub fn push_fast_forward(run: &mut Run, dir: &str, branch: &BranchName) -> Result<Push, Error> {
    let _writing = writing();
    let target = format!("HEAD:refs/heads/{branch}");
    let failed = || Error::Failed(format!("pushing to {branch} failed"));
    let output = run("git", &[&STALL_LIMITS[..], &["push", "--porcelain", "origin", &target]].concat(), Some(dir)).map_err(|_| failed())?;
    if output.ok {
        return Ok(Push::Pushed);
    }
    let behind = output.stdout.lines().any(|line| line.starts_with('!') && line.contains("[rejected]"));
    if behind { Ok(Push::Behind) } else { Err(failed()) }
}

// Origin's branches, as last fetched, named as the issue's (`issue_branch`).
pub fn issue_branches(run: &mut Run, dir: &str, key: &IssueKey) -> Result<Vec<BranchName>, Error> {
    let listed = git(run, dir, &["for-each-ref", "--format=%(refname:lstrip=3)", "refs/remotes/origin"])?;
    Ok(listed.lines().filter_map(BranchName::parse).filter(|branch| issue_branch(branch, key)).collect())
}

// Deletes origin's `branch`, as a merged PR's is, so no later worker resumes from it
// (`continue_issue_branch`). One origin hasn't got is already gone. As with `fetch`, a failure
// says only that it failed.
pub fn delete_remote_branch(run: &mut Run, dir: &str, branch: &BranchName) -> Result<(), Error> {
    let _writing = writing();
    let output = run("git", &[&STALL_LIMITS[..], &["push", "--porcelain", "origin", &format!(":refs/heads/{branch}")]].concat(), Some(dir))
        .map_err(|_| Error::Failed(format!("deleting origin's {branch} failed")))?;
    match output.ok || format!("{}{}", output.stdout, output.stderr).contains("remote ref does not exist") {
        true => Ok(()),
        false => Err(Error::Failed(format!("deleting origin's {branch} failed"))),
    }
}

// Whether the local branch exists: git's exit 1 is no, any other failure is an Err, never a
// no.
pub fn branch_exists(run: &mut Run, dir: &str, branch: &BranchName) -> Result<bool, Error> {
    let output = run("git", &["show-ref", "--verify", "--quiet", &format!("refs/heads/{branch}")], Some(dir))?;
    match output.code {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(Error::Failed(output.stderr.trim().to_string())),
    }
}

// The path of every worktree git lists for the repo at `dir`, as git writes it.
pub fn worktree_paths(run: &mut Run, dir: &str) -> Result<Vec<String>, Error> {
    let listed = git(run, dir, &["worktree", "list", "--porcelain"])?;
    Ok(listed.lines().filter_map(|line| line.strip_prefix("worktree ")).map(String::from).collect())
}

// Leaves HEAD detached at `tip` with tracked changes discarded, whatever it was on. No
// branch moves: a branch HEAD was on keeps its commits.
pub fn detach(run: &mut Run, dir: &str, tip: &Sha) -> Result<(), Error> {
    let _writing = writing();
    git(run, dir, &["switch", "--quiet", "--detach", "--discard-changes", tip]).map(drop)
}

// Removes untracked files and directories. Ignored ones stay.
pub fn clean(run: &mut Run, dir: &str) -> Result<(), Error> {
    git(run, dir, &["clean", "--quiet", "-fd"]).map(drop)
}

// `-d`, so a branch holding commits that neither HEAD nor its upstream has is kept (Err).
pub fn delete_merged_branch(run: &mut Run, dir: &str, branch: &BranchName) -> Result<(), Error> {
    let _writing = writing();
    git(run, dir, &["branch", "--quiet", "-d", branch]).map(drop)
}

// `-D`: the branch goes with commits no other ref holds. For a branch whose work the caller
// has proved is on a remote or merged there, as a squash merge leaves it.
pub fn force_delete_branch(run: &mut Run, dir: &str, branch: &BranchName) -> Result<(), Error> {
    let _writing = writing();
    git(run, dir, &["branch", "-D", branch]).map(drop)
}

// A clone of `url` into `into`, with `env` (credential::git_env) for its login.
pub fn clone_with(env: &[(String, String)], url: &str, into: &str) -> Result<(), Error> {
    let args = [&STALL_LIMITS[..], &["clone", "--quiet", "--", url, into]].concat();
    match run_command_with(env, "git", &args, None)?.ok {
        true => Ok(()),
        false => Err(Error::Failed(format!("cloning into {into} failed"))),
    }
}

// A fetch of `dir`'s origin, with `env` (credential::git_env) for its login.
pub fn fetch_with(env: &[(String, String)], dir: &str) -> Result<(), Error> {
    let args = [&STALL_LIMITS[..], &["fetch", "--quiet", "origin"]].concat();
    match run_command_with(env, "git", &args, Some(dir))?.ok {
        true => Ok(()),
        false => Err(Error::Failed(format!("fetching origin in {dir} failed"))),
    }
}

pub fn push_upstream(run: &mut Run, dir: &str, branch: &BranchName) -> Result<(), Error> {
    let _writing = writing();
    git(run, dir, &[&STALL_LIMITS[..], &["push", "--quiet", "-u", "origin", branch]].concat())
        .map(drop)
        .map_err(|_| Error::Failed(format!("pushing {branch} failed")))
}

// The checkout's own git dir and the repo's common one, which are the same in a primary
// checkout and differ in a linked worktree.
pub fn git_dirs(run: &mut Run, dir: &str) -> Result<(String, String), Error> {
    let dirs = git(run, dir, &["rev-parse", "--path-format=absolute", "--git-dir", "--git-common-dir"])?;
    let mut dirs = dirs.lines().map(str::trim);
    match (dirs.next(), dirs.next()) {
        (Some(git_dir), Some(common_dir)) => Ok((git_dir.to_string(), common_dir.to_string())),
        _ => Err(Error::Failed(format!("git rev-parse in {dir} printed no git dir"))),
    }
}

// The checkout's own git dir, None anywhere git can't be asked.
pub fn git_dir(run: &mut Run, dir: Option<&str>) -> Option<String> {
    git_at(run, dir, &["rev-parse", "--path-format=absolute", "--git-dir"]).ok()
}

// Unsets `hook.<name>` as a whole, and does nothing when none is set: `--remove-section`
// fails on a missing section, so it is asked about first, under the same lock.
pub fn remove_hook_config(run: &mut Run, dir: &str, name: &str) -> Result<(), Error> {
    let _writing = writing();
    let set = git(run, dir, &["config", "--local", "--get-regexp", &format!(r"^hook\.{name}\.")]);
    if set.is_ok_and(|keys| !keys.is_empty()) {
        git(run, dir, &["config", "--local", "--remove-section", &format!("hook.{name}")])?;
    }
    Ok(())
}

// Sets `hook.<name>` to run `command` on each of `events`, replacing what an earlier call set.
pub fn set_hook_config(run: &mut Run, dir: &str, name: &str, events: &[&str], command: &str) -> Result<(), Error> {
    let _writing = writing();
    let key = |field: &str| format!("hook.{name}.{field}");
    for (n, event) in events.iter().enumerate() {
        git(run, dir, &["config", "--local", if n == 0 { "--replace-all" } else { "--add" }, &key("event"), event])?;
    }
    git(run, dir, &["config", "--local", "--replace-all", &key("command"), command]).map(drop)
}

// A global setting, None where it is unset or empty.
pub fn global_config(run: &mut Run, key: &str) -> Option<String> {
    git_at(run, None, &["config", "--global", "--get", key]).ok().filter(|value| !value.is_empty())
}

pub fn set_global_config(run: &mut Run, key: &str, value: &str) -> Result<(), Error> {
    let _writing = writing();
    git_at(run, None, &["config", "--global", key, value]).map(drop)
}

// Whether `git hook list <event>` names the hook `name`: a Git that doesn't run hooks from
// config lists none.
pub fn hook_listed(run: &mut Run, dir: &str, event: &str, name: &str) -> Result<bool, Error> {
    let output = run("git", &["hook", "list", event], Some(dir))?;
    Ok(output.ok && output.stdout.lines().any(|line| line.trim() == name))
}

#[cfg(test)]
mod tests;
