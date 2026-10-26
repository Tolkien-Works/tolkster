// A development build keeping itself current: the pull runs in the shared lane
// (`Updater::pass`), under git's lock, so it never runs beside another lane's fetch, push or
// worktree change, and the build and its swap on a thread of their own (`Build`).
// A swap is decided by the build against the installed binary, never by what changed in
// git, so a failed build or a pull by hand still ends on a build of HEAD. A pass builds
// unless this run's last build that ended was of HEAD already.
// Windows won't overwrite a running exe but will rename one, hence `swap_files`. A build put
// in place syncs a Linear release of its commit (`release`).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use ureq::Agent;

use super::{BRANCH, Pass, Update, own_branch, short};
use crate::config;
use crate::error::{Context, Error};
use crate::exec::Run;
use crate::git;
use crate::linear::{sync_release, with_token};
use crate::orca::origin_repo;
use crate::plain::{IssueKey, Sha};
use crate::prebuilt::{retry, swap_files};
use crate::gateway::{self, Lane};
use crate::sessions::workspace::release_base;
use crate::sync::lock;
use crate::time::now_ms;
use crate::trace::{self, Restart, Unbuilt, Unfit, Unpulled, Update as Said};

const MANIFEST: &str = "cli/Cargo.toml";
const CRATE: &str = "name = \"tolkster\"";
const BUILD: &str = "cli/target/local";

// Puts a build in place of the installed binary: whether it did, false when the build is
// that binary already.
pub type Place<'a> = dyn FnMut(&Path) -> Result<bool, Error> + 'a;

// Syncs the release of a build put in place (`release`), given the checkout's path and the
// commit built: what it said, if anything.
pub type Release<'a> = dyn FnMut(&mut Run, &str, &str) -> Option<Said> + 'a;

#[derive(Debug)]
pub struct Checkout {
    pub path: String,
    pub repo: String,
}

impl Checkout {
    pub fn open(run: &mut Run) -> Result<Checkout, Unfit> {
        let dirs = git(run, &["rev-parse", "--path-format=absolute", "--show-toplevel", "--git-dir", "--git-common-dir"], None).map_err(|_| Unfit::NotACheckout)?;
        let [path, git_dir, common] = <[&str; 3]>::try_from(dirs.lines().map(str::trim).collect::<Vec<_>>()).map_err(|_| Unfit::NotACheckout)?;
        if git_dir != common {
            return Err(Unfit::Worktree { path: path.into() });
        }
        if !fs::read_to_string(Path::new(path).join(MANIFEST)).is_ok_and(|manifest| manifest.contains(CRATE)) {
            return Err(Unfit::NotTolkster { path: path.into() });
        }
        let repo = origin_repo(run, Some(path)).map_err(|_| Unfit::NoOrigin { path: path.into() })?;
        Ok(Checkout { path: path.to_string(), repo })
    }

    fn check(&self, run: &mut Run, say: &mut dyn FnMut(Said), after: Option<&Sha>) -> Pulled {
        let (from, head) = match self.pull(run, after) {
            Ok(Some(moved)) => moved,
            Ok(None) => {
                say(Said::NotHeld { after: short(after.map_or("", Sha::as_str)).into() });
                return Pulled::Behind;
            }
            Err(NotPulled::Unfetched(why)) if after.is_some() => {
                say(Said::Unfetched(why));
                return Pulled::Unfetched;
            }
            Err(NotPulled::Left(why) | NotPulled::Unfetched(why)) => {
                say(Said::Skipped(why));
                return Pulled::Skipped;
            }
        };
        say(if from == head { Said::At { head: short(&head).into() } } else { Said::Moved { from: short(&from).into(), head: short(&head).into() } });
        Pulled::At(head)
    }

    // Err once it has said why the build isn't in place, so the next pass builds again.
    fn rebuild(&self, run: &mut Run, head: &str, say: &mut dyn FnMut(Said), place: &mut Place) -> Result<Option<Restart>, ()> {
        let build = match self.build(run) {
            Ok(build) => build,
            Err(why) => {
                say(Said::BuildFailed(why));
                return Err(());
            }
        };
        match place(&build) {
            Ok(true) => Ok(Some(Restart::Built { head: short(head).into() })),
            Ok(false) => {
                say(Said::Built { head: short(head).into() });
                Ok(None)
            }
            Err(error) => {
                say(Said::NotPlaced(error));
                Err(())
            }
        }
    }

    // None when origin/main, fetched, doesn't hold `after`, the push's commit, yet: nothing
    // is merged.
    fn pull(&self, run: &mut Run, after: Option<&Sha>) -> Result<Option<(String, String)>, NotPulled> {
        let _writing = crate::git::writing();
        let dir = Some(self.path.as_str());
        let branch = git(run, &["symbolic-ref", "--quiet", "--short", "HEAD"], dir).map_err(|_| Unpulled::NoBranch)?;
        if branch != BRANCH {
            return Err(Unpulled::OffBranch { branch }.into());
        }
        // Untracked files count: the build takes the working tree. Checked on every pass, as
        // HEAD may already be at `origin/main`.
        if !git(run, &["status", "--porcelain"], dir).map_err(Unpulled::Status)?.is_empty() {
            return Err(Unpulled::Dirty.into());
        }
        git(run, &["fetch", "--quiet", "origin", BRANCH], dir).map_err(|code| NotPulled::Unfetched(Unpulled::Fetch(code)))?;
        // An unknown commit fails too.
        if let Some(after) = after
            && git(run, &["merge-base", "--is-ancestor", after, &format!("origin/{BRANCH}")], dir).is_err()
        {
            return Ok(None);
        }
        let head = git(run, &["rev-parse", "HEAD"], dir).map_err(Unpulled::Head)?;
        let target = git(run, &["rev-parse", &format!("origin/{BRANCH}")], dir).map_err(Unpulled::Target)?;
        if head == target {
            return Ok(Some((head, target)));
        }
        if git(run, &["merge-base", "--is-ancestor", "HEAD", &format!("origin/{BRANCH}")], dir).is_err() {
            return Err(Unpulled::Ahead.into());
        }
        git(run, &["merge", "--ff-only", "--quiet", &format!("origin/{BRANCH}")], dir).map_err(Unpulled::FastForward)?;
        Ok(Some((head, target)))
    }


    // What cargo said goes to stderr when it failed. The build keeps the feature, or the next
    // run would have no rebuild.
    fn build(&self, run: &mut Run) -> Result<PathBuf, Unbuilt> {
        let manifest = Path::new(&self.path).join(MANIFEST);
        let manifest = manifest.to_string_lossy();
        let output = run("cargo", &["build", "--profile", "local", "--locked", "--features", "develop", "--manifest-path", &manifest], Some(&self.path)).map_err(Unbuilt::NotRun)?;
        if !output.ok {
            trace::printed(output.stderr.trim_end());
            return Err(Unbuilt::Exited(output.code.unwrap_or(-1)));
        }
        Ok(Path::new(&self.path).join(BUILD).join(format!("tolkster{}", std::env::consts::EXE_SUFFIX)))
    }
}

// What a pass's pull found.
enum Pulled {
    At(String),
    Skipped,
    // A tip's commit isn't pulled yet: origin/main doesn't hold it.
    Behind,
    // Nor is it, as the fetch failed.
    Unfetched,
}

// Why a pull left the checkout as it was: a guard, which a retry won't change, or a fetch
// that failed, which one may.
enum NotPulled {
    Left(Unpulled),
    Unfetched(Unpulled),
}

impl From<Unpulled> for NotPulled {
    fn from(why: Unpulled) -> NotPulled {
        NotPulled::Left(why)
    }
}

pub struct Updater {
    checkout: Arc<Mutex<Checkout>>,
    repo: String,
    pub update: Update,
}

pub struct Build {
    checkout: Arc<Mutex<Checkout>>,
    head: String,
    update: Update,
}

impl Updater {
    pub fn open(_agent: &Agent, run: &mut Run) -> Result<Updater, Unfit> {
        Checkout::open(run).map(Updater::new)
    }

    // The backstop for a push that came while nothing listened: a pass made once connected,
    // as a put-off one is (`take_put_off`), so the start never waits on a build.
    pub fn catch_up(&self) {
        self.update.put_off.store(true, Ordering::SeqCst);
    }

    pub fn new(checkout: Checkout) -> Updater {
        let repo = checkout.repo.clone();
        Updater { checkout: Arc::new(Mutex::new(checkout)), repo, update: Update::default() }
    }

    pub fn is_own(&self, repo: &str, reference: &str) -> bool {
        self.repo.eq_ignore_ascii_case(repo) && own_branch(reference)
    }

    // Put off while a build runs, as the build takes the working tree, and skipped while
    // draining, as the restart makes a pass as it starts. Behind or Unready when a base tip's
    // commit, `after`, isn't pulled: origin/main doesn't hold it yet, or the fetch failed, or
    // a build or a drain kept this pass from pulling. The tip's job is held and tried again
    // (`Tips`), with no wait here, so neither the put-off pass nor the restart's, which know
    // no `after`, stands in for the tip's.
    pub fn pass(&self, run: &mut Run, say: &mut dyn FnMut(Said), after: Option<&Sha>, start: impl FnOnce(Build)) -> Pass {
        let unready = if after.is_none() { Pass::Skipped } else { Pass::Unready };
        // Building first: a build puts its restart in place before it ends, so one that has
        // ended is seen draining.
        if self.update.building().is_some() {
            self.update.put_off.store(true, Ordering::SeqCst);
            say(Said::Building);
            return unready;
        }
        if self.update.draining() {
            say(Said::Draining);
            return unready;
        }
        let head = match lock(&self.checkout).check(run, say, after) {
            Pulled::At(head) => head,
            Pulled::Skipped => return Pass::Skipped,
            Pulled::Behind => return Pass::Behind,
            Pulled::Unfetched => return Pass::Unready,
        };
        if lock(&self.update.built).as_deref() == Some(head.as_str()) {
            say(Said::BuiltAlready { head: short(&head).into() });
            return Pass::Pulled;
        }
        self.update.set_building(Some(Instant::now()));
        start(Build { checkout: Arc::clone(&self.checkout), head, update: self.update.clone() });
        Pass::Pulled
    }
}

impl Build {
    pub fn make(self, run: &mut Run, say: &mut dyn FnMut(Said), place: &mut Place, release: &mut Release) {
        let update = self.update.clone();
        let mut beating = |program: &str, args: &[&str], dir: Option<&str>| {
            update.set_building(Some(Instant::now()));
            run(program, args, dir)
        };
        let mut placing = |build: &Path| {
            update.set_building(Some(Instant::now()));
            update.placing.store(true, Ordering::SeqCst);
            let placed = place(build);
            update.placing.store(false, Ordering::SeqCst);
            placed
        };
        let rebuilt = lock(&self.checkout).rebuild(&mut beating, &self.head, say, &mut placing);
        *lock(&self.update.built) = rebuilt.is_ok().then(|| self.head.clone());
        // Also for a build in place already, so a sync that failed or was cut off is made by
        // the next build, or the next start's; before the restart is in place, as the drain
        // could exit before it.
        if rebuilt.is_ok() {
            let path = lock(&self.checkout).path.clone();
            if let Some(said) = release(run, &path, &self.head) {
                say(said);
            }
        }
        if let Ok(Some(restart)) = rebuilt {
            say(Said::Restarting { head: short(&self.head).into() });
            self.update.in_place(restart);
        }
    }
}

// Ended, built or not, even by a panic, so the next pass may pull and build again.
impl Drop for Build {
    fn drop(&mut self) {
        self.update.set_building(None);
    }
}

// The most issues one release links: the newest, should the pipeline's newest release be an
// old commit.
const RELEASE_ISSUES: usize = 100;

// Syncs the release of `head`, built and in place, to `releasePipeline`'s pipeline, linking
// the issues of the workspace's teams landed since the pipeline's newest release
// (`git::landed`): None with no pipeline set. Nothing when that release is of `head` already,
// so a sync is safe to repeat. In the normal lane, as nothing waits on it. A failure is said
// and left: the build stays in place.
pub fn release(agent: &Agent, run: &mut Run, dir: &str, head: &str) -> Option<Said> {
    let pipeline = match config::files().and_then(|files| config::release_pipeline(&files)) {
        Ok(Some(pipeline)) => pipeline,
        Ok(None) => return None,
        Err(error) => return Some(Said::NotReleased(error)),
    };
    let sync = || {
        let commit = Sha::parse(head).ok_or_else(|| Error::Failed("git named no commit".into()))?;
        let (last, teams) = release_base(agent, &pipeline)?;
        if last.as_ref() == Some(&commit) {
            return Ok(None);
        }
        let issues = linked(git::landed(run, dir, last.as_ref(), &commit)?, &teams);
        with_token(agent, |token| sync_release(agent, token, &pipeline, &commit, &issues))?;
        Ok(Some(issues.len()))
    };
    let head = short(head).to_string();
    Some(match gateway::in_lane(Lane::Normal, sync) {
        Ok(Some(issues)) => Said::Released { head, issues },
        Ok(None) => Said::ReleasedAlready { head },
        Err(error) => Said::NotReleased(error),
    })
}

// The issues of `teams` among `landed`, newest first, at most RELEASE_ISSUES: a tag such as
// `(UTF-8)` names no team.
fn linked(mut landed: Vec<IssueKey>, teams: &[String]) -> Vec<IssueKey> {
    landed.retain(|key| key.as_str().split_once('-').is_some_and(|(team, _)| teams.iter().any(|known| known == team)));
    landed.truncate(RELEASE_ISSUES);
    landed
}

// Never shows git's stderr: it can carry a remote URL, and with it a credential.
fn git(run: &mut Run, args: &[&str], dir: Option<&str>) -> Result<String, i32> {
    let output = run("git", args, dir).map_err(|_| -1)?;
    if output.ok { Ok(output.stdout.trim().to_string()) } else { Err(output.code.unwrap_or(-1)) }
}

pub fn swap(build: &Path) -> Result<bool, Error> {
    let exe = std::env::current_exe().context("finding this tolkster")?;
    let swapped = !same_file(&exe, build);
    if swapped {
        let name = exe.file_name().and_then(|n| n.to_str()).ok_or_else(|| Error::Failed("the running binary has no file name".into()))?;
        let aside = exe.with_file_name(format!("{name}.{}.old", now_ms()));
        swap_files(&exe, build, &aside)?;
    }
    // Also for a build that is the binary in place, as `cargo install` copies no symbols.
    if cfg!(windows) {
        place_symbols(&exe, build);
    }
    Ok(swapped)
}

// Windows keeps a build's line tables in the `.pdb` beside it, which the next build
// overwrites while this one runs: the copy beside the installed exe is what matches it. One
// that can't be copied is removed, so no stale one stands for the new build; a debugger
// holding the old one open is waited out as the exe's swap is.
pub(crate) fn place_symbols(exe: &Path, build: &Path) {
    let (from, to) = (build.with_extension("pdb"), exe.with_extension("pdb"));
    if !from.is_file() || retry(|| fs::copy(&from, &to).map(|_| ())).is_err() {
        let _ = retry(|| match fs::remove_file(&to) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            done => done,
        });
    }
}

// Whether both files read and hold the same bytes: one that can't be read is a
// difference, so the swap is tried and says why it can't.
fn same_file(a: &Path, b: &Path) -> bool {
    matches!((fs::read(a), fs::read(b)), (Ok(a), Ok(b)) if a == b)
}

#[cfg(test)]
mod tests;
