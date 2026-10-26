use super::*;
use crate::exec::Output;

// A checkout as git would answer for it: what each guard reads, and what ran.
struct Repo {
    path: String,
    worktree: bool,
    branch: &'static str,
    fetch_ok: bool,
    head: &'static str,
    target: &'static str,
    ancestor: bool,
    // origin/main holds the push's commit.
    holds: bool,
    dirty: bool,
    build_ok: bool,
    ran: Vec<String>,
}

impl Repo {
    fn new(path: &str) -> Repo {
        Repo { path: path.into(), worktree: false, branch: "main", fetch_ok: true, head: "a3d0e65aaaa", target: "b41c2f0bbbb", ancestor: true, holds: true, dirty: false, build_ok: true, ran: Vec::new() }
    }

    fn run(&mut self) -> impl FnMut(&str, &[&str], Option<&str>) -> Result<Output, Error> + '_ {
        move |program, args, _dir| {
            self.ran.push(format!("{program} {}", args.join(" ")));
            let (ok, code, stdout) = match (program, args) {
                ("git", ["rev-parse", "--path-format=absolute", "--show-toplevel", "--git-dir", "--git-common-dir"]) => {
                    (true, 0, format!("{}\n{}/.git\n{}\n", self.path, self.path, if self.worktree { "/primary/.git".to_string() } else { format!("{}/.git", self.path) }))
                }
                ("git", ["remote", "get-url", "origin"]) => (true, 0, "https://github.com/Tolkien-Works/tolkster.git\n".into()),
                ("git", ["rev-parse", "HEAD"]) => (true, 0, format!("{}\n", self.head)),
                ("git", ["rev-parse", "origin/main"]) => (true, 0, format!("{}\n", self.target)),
                ("git", ["symbolic-ref", "--quiet", "--short", "HEAD"]) => (true, 0, format!("{}\n", self.branch)),
                ("git", ["fetch", "--quiet", "origin", "main"]) => (self.fetch_ok, if self.fetch_ok { 0 } else { 128 }, String::new()),
                ("git", ["merge-base", "--is-ancestor", "HEAD", "origin/main"]) => (self.ancestor, if self.ancestor { 0 } else { 1 }, String::new()),
                ("git", ["merge-base", "--is-ancestor", _, "origin/main"]) => (self.holds, if self.holds { 0 } else { 1 }, String::new()),
                ("git", ["status", "--porcelain"]) => (true, 0, if self.dirty { " M cli/src/main.rs\n".into() } else { String::new() }),
                ("git", ["merge", "--ff-only", "--quiet", "origin/main"]) => {
                    self.head = self.target;
                    (true, 0, String::new())
                }
                ("cargo", ["build", "--profile", "local", "--locked", "--features", "develop", "--manifest-path", _]) => (self.build_ok, if self.build_ok { 0 } else { 101 }, String::new()),
                _ => panic!("{program} {}", args.join(" ")),
            };
            Ok(Output { ok, code: Some(code), stdout, stderr: "fatal: a URL with a credential in it".into() })
        }
    }

    fn wrote(&self, program: &str) -> bool {
        self.ran.iter().any(|line| line.starts_with(&format!("{program} merge ")) || line.starts_with(&format!("{program} build")))
    }
}

// A directory with a tolkster manifest in it, as the checkout's path: one per call,
// as the tests run at once.
fn checkout_dir() -> String {
    static MADE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let made = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tolkster-update-{}-{made}", std::process::id()));
    fs::create_dir_all(dir.join("cli")).unwrap();
    fs::write(dir.join(MANIFEST), "[package]\nname = \"tolkster\"\n").unwrap();
    dir.to_string_lossy().replace('\\', "/")
}

fn open(repo: &mut Repo) -> Result<Checkout, Unfit> {
    Checkout::open(&mut repo.run())
}

// A pull and its build in one go, as the shared lane and the build's thread make them.
fn update(checkout: &Checkout, run: &mut Run, say: &mut dyn FnMut(Said), place: &mut Place) -> Option<Restart> {
    let Pulled::At(head) = checkout.check(run, say, None) else { return None };
    checkout.rebuild(run, &head, say, place).ok().flatten()
}

// One pass, with a swap that only notes the build it was given.
fn pass(checkout: &Checkout, repo: &mut Repo) -> (Vec<Said>, Option<Restart>, Vec<PathBuf>) {
    let (mut said, mut placed) = (Vec::new(), Vec::new());
    let restart = update(checkout, &mut repo.run(), &mut |line| said.push(line), &mut |build| {
        placed.push(build.to_path_buf());
        Ok(true)
    });
    (said, restart, placed)
}

#[test]
fn only_tolksters_own_primary_checkout_is_opened() {
    let dir = checkout_dir();
    let mut repo = Repo::new(&dir);
    let checkout = open(&mut repo).unwrap();
    assert_eq!((checkout.path.as_str(), checkout.repo.as_str()), (dir.as_str(), "Tolkien-Works/tolkster"));
    let updater = Updater::new(checkout);
    assert!(updater.is_own("tolkien-works/tolkster", "refs/heads/main"));
    assert!(!updater.is_own("Tolkien-Works/other", "refs/heads/main") && !updater.is_own("Tolkien-Works/tolkster", "refs/heads/release"));
    // A worktree, and a checkout of something else.
    let mut worktree = Repo { worktree: true, ..Repo::new(&dir) };
    assert_eq!(open(&mut worktree).unwrap_err(), Unfit::Worktree { path: dir.clone() });
    let other = std::env::temp_dir().to_string_lossy().replace('\\', "/");
    let mut elsewhere = Repo::new(&other);
    assert_eq!(open(&mut elsewhere).unwrap_err(), Unfit::NotTolkster { path: other.clone() });
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_checkout_off_main_dirty_or_ahead_is_left_alone_with_one_line() {
    let dir = checkout_dir();
    let cases: [(Repo, Said); 4] = [
        (Repo { branch: "feature", ..Repo::new(&dir) }, Said::Skipped(Unpulled::OffBranch { branch: "feature".into() })),
        (Repo { dirty: true, ..Repo::new(&dir) }, Said::Skipped(Unpulled::Dirty)),
        (Repo { ancestor: false, ..Repo::new(&dir) }, Said::Skipped(Unpulled::Ahead)),
        // Git's own error, which can hold a credential, is never shown.
        (Repo { fetch_ok: false, ..Repo::new(&dir) }, Said::Skipped(Unpulled::Fetch(128))),
    ];
    for (mut repo, line) in cases {
        let checkout = open(&mut repo).unwrap();
        let (said, restart, placed) = pass(&checkout, &mut repo);
        assert!(restart.is_none() && placed.is_empty(), "{line:?}");
        assert_eq!(said, [line]);
        assert!(!repo.wrote("git") && !repo.wrote("cargo"));
    }
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_pass_builds_and_restarts_only_on_a_build_that_differs() {
    let dir = checkout_dir();
    // A fast-forward: built, put in place, and the restart names the commit.
    let mut moved = Repo::new(&dir);
    let checkout = open(&mut moved).unwrap();
    let (said, restart, placed) = pass(&checkout, &mut moved);
    assert_eq!(said, [Said::Moved { from: "a3d0e65".into(), head: "b41c2f0".into() }]);
    assert_eq!(restart, Some(Restart::Built { head: "b41c2f0".into() }));
    assert_eq!(placed, [Path::new(&dir).join(BUILD).join(format!("tolkster{}", std::env::consts::EXE_SUFFIX))]);
    assert!(moved.ran.iter().any(|l| l == &format!("cargo build --profile local --locked --features develop --manifest-path {}", Path::new(&dir).join(MANIFEST).to_string_lossy())));
    // Nothing new to pull: still built, since what the binary compiles in, such as
    // src/shared/voice.json, may have changed outside cli/.
    let mut current = Repo { head: "b41c2f0bbbb", ..Repo::new(&dir) };
    let checkout = open(&mut current).unwrap();
    let (said, restart, placed) = pass(&checkout, &mut current);
    assert_eq!(said, [Said::At { head: "b41c2f0".into() }]);
    assert!(restart.is_some() && placed.len() == 1 && current.wrote("cargo") && !current.ran.iter().any(|l| l.starts_with("git merge")));
    // A build that is the binary in place: said, and no restart.
    let mut same = Repo { head: "b41c2f0bbbb", ..Repo::new(&dir) };
    let checkout = open(&mut same).unwrap();
    let mut said = Vec::new();
    let restart = update(&checkout, &mut same.run(), &mut |line| said.push(line), &mut |_| Ok(false));
    assert_eq!(said, [Said::At { head: "b41c2f0".into() }, Said::Built { head: "b41c2f0".into() }]);
    assert!(restart.is_none() && same.wrote("cargo"));
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_failed_build_keeps_the_current_binary_and_is_tried_again() {
    let dir = checkout_dir();
    let mut repo = Repo { build_ok: false, ..Repo::new(&dir) };
    let checkout = open(&mut repo).unwrap();
    let (said, restart, placed) = pass(&checkout, &mut repo);
    assert_eq!(said, [Said::Moved { from: "a3d0e65".into(), head: "b41c2f0".into() }, Said::BuildFailed(Unbuilt::Exited(101))]);
    assert!(restart.is_none() && placed.is_empty());
    // The next pass finds HEAD at origin/main, and builds it all the same.
    repo.build_ok = true;
    let (said, restart, placed) = pass(&checkout, &mut repo);
    assert_eq!(said, [Said::At { head: "b41c2f0".into() }]);
    assert!(restart.is_some() && placed.len() == 1);
    // A swap that fails is the same: said, and tried again.
    let mut again = Repo::new(&dir);
    let checkout = open(&mut again).unwrap();
    let mut said = Vec::new();
    let restart = update(&checkout, &mut again.run(), &mut |line| said.push(line), &mut |_| Err(Error::Failed("access denied".into())));
    assert_eq!(said[1], Said::NotPlaced(Error::Failed("access denied".into())));
    assert!(restart.is_none());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_dirty_tree_is_never_built_however_head_reached_origin_main() {
    let dir = checkout_dir();
    // After a failed build, or a pull by hand, HEAD is already at origin/main: the pass
    // has no fast-forward to guard, yet the build would take the working tree.
    let mut repo = Repo { head: "b41c2f0bbbb", dirty: true, ..Repo::new(&dir) };
    let checkout = open(&mut repo).unwrap();
    let (said, restart, placed) = pass(&checkout, &mut repo);
    assert_eq!(said, [Said::Skipped(Unpulled::Dirty)]);
    assert!(restart.is_none() && placed.is_empty() && !repo.wrote("cargo") && !repo.wrote("git"));
    fs::remove_dir_all(&dir).unwrap();
}
#[test]
fn the_running_binary_is_renamed_aside_and_the_build_copied_over_it() {
    let dir = std::env::temp_dir().join(format!("tolkster-swap-{}-{}", std::process::id(), now_ms()));
    fs::create_dir_all(&dir).unwrap();
    let (exe, build, aside) = (dir.join("tolkster.exe"), dir.join("new.exe"), dir.join("tolkster.exe.1.old"));
    fs::write(&exe, "old").unwrap();
    fs::write(&build, "new").unwrap();
    swap_files(&exe, &build, &aside).unwrap();
    assert_eq!((fs::read_to_string(&exe).unwrap().as_str(), fs::read_to_string(&aside).unwrap().as_str()), ("new", "old"));
    // A build with the installed binary's bytes is no difference; other bytes, or a
    // missing file, are.
    fs::write(dir.join("copy.exe"), "new").unwrap();
    assert!(same_file(&exe, &dir.join("copy.exe")) && !same_file(&exe, &aside) && !same_file(&exe, &dir.join("missing.exe")));
    // A build that is missing puts the old binary back.
    let error = swap_files(&exe, &dir.join("missing.exe"), &dir.join("tolkster.exe.2.old")).unwrap_err();
    assert!(error.to_string().starts_with("copying"));
    assert_eq!(fs::read_to_string(&exe).unwrap(), "new");
    // The build's symbols go beside the exe; with none to copy, a stale copy goes.
    fs::write(dir.join("new.pdb"), "symbols").unwrap();
    place_symbols(&exe, &build);
    assert_eq!(fs::read_to_string(dir.join("tolkster.pdb")).unwrap(), "symbols");
    place_symbols(&exe, &dir.join("missing.exe"));
    assert!(!dir.join("tolkster.pdb").exists());
    fs::remove_dir_all(&dir).unwrap();
}

// A pass in the shared lane: what it said, and the build it handed on, if any.
fn hand_on(updater: &Updater, repo: &mut Repo) -> (Vec<Said>, Option<Build>) {
    let (said, held, handed) = pushed(updater, repo, None);
    assert!(held);
    (said, handed)
}

// A pass for a push of `after`: also whether origin/main held it.
fn pushed(updater: &Updater, repo: &mut Repo, after: Option<&str>) -> (Vec<Said>, bool, Option<Build>) {
    let (mut said, mut handed) = (Vec::new(), None);
    let after = after.map(crate::plain::Sha::from);
    let pass = updater.pass(&mut repo.run(), &mut |line| said.push(line), after.as_ref(), |build| handed = Some(build));
    let held = matches!(pass, Pass::Pulled | Pass::Skipped);
    (said, held, handed)
}

// The build made, as its own thread would, with a swap that only succeeds.
fn make(build: Build, repo: &mut Repo) -> Vec<Said> {
    let mut said = Vec::new();
    build.make(&mut repo.run(), &mut |line| said.push(line), &mut |_| Ok(true), &mut released);
    said
}

// A release sync that says it synced the commit it was given.
fn released(_: &mut Run, _: &str, head: &str) -> Option<Said> {
    Some(Said::Released { head: head[..7].into(), issues: 1 })
}

#[test]
fn a_build_runs_apart_from_the_pull_and_leaves_the_restart_to_drain_for() {
    let dir = checkout_dir();
    let mut repo = Repo::new(&dir);
    let updater = Updater::new(open(&mut repo).unwrap());
    let (said, build) = hand_on(&updater, &mut repo);
    assert_eq!(said, [Said::Moved { from: "a3d0e65".into(), head: "b41c2f0".into() }]);
    let build = build.unwrap();
    assert!(repo.wrote("git") && !repo.wrote("cargo") && updater.update.building().is_some());
    // A push while it builds: nothing pulled, as cargo reads the working tree, and the
    // pass is put off until the build ends.
    let ran = repo.ran.len();
    let (said, handed) = hand_on(&updater, &mut repo);
    assert_eq!(said, [Said::Building]);
    assert!(handed.is_none() && repo.ran.len() == ran && !updater.update.take_put_off());
    assert_eq!(make(build, &mut repo), [Said::Released { head: "b41c2f0".into(), issues: 1 }, Said::Restarting { head: "b41c2f0".into() }]);
    assert!(repo.wrote("cargo") && updater.update.building().is_none() && updater.update.draining());
    // Draining: the put-off pass isn't made, as the restart pulls as it starts, and no
    // other pass is either.
    assert!(!updater.update.take_put_off());
    let (said, handed) = hand_on(&updater, &mut repo);
    assert_eq!(said, [Said::Draining]);
    assert!(handed.is_none() && repo.ran.len() == ran + 1);
    assert_eq!(updater.update.take_restart(), Some(Restart::Built { head: "b41c2f0".into() }));
    assert!(!updater.update.draining());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_start_pulls_and_builds_nothing_until_connected_then_builds_apart() {
    let dir = checkout_dir();
    let mut repo = Repo::new(&dir);
    let updater = Updater::new(open(&mut repo).unwrap());
    let opened = repo.ran.len();
    // HEAD is behind origin/main, yet the start runs nothing: it connects at once, and the
    // connection's loop makes the pass (`take_put_off`), once.
    updater.catch_up();
    assert!(repo.ran.len() == opened && updater.update.building().is_none());
    assert!(updater.update.take_put_off() && !updater.update.take_put_off());
    // That pass pulls, hands the build to its own thread, and the swap leaves the restart to drain for.
    let (said, build) = hand_on(&updater, &mut repo);
    assert_eq!(said, [Said::Moved { from: "a3d0e65".into(), head: "b41c2f0".into() }]);
    assert!(!repo.wrote("cargo") && updater.update.building().is_some() && !updater.update.draining());
    assert_eq!(make(build.unwrap(), &mut repo), [Said::Released { head: "b41c2f0".into(), issues: 1 }, Said::Restarting { head: "b41c2f0".into() }]);
    assert!(repo.wrote("cargo") && updater.update.draining());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_failed_build_ends_the_build_and_makes_the_pass_it_put_off_once() {
    let dir = checkout_dir();
    let mut repo = Repo { build_ok: false, ..Repo::new(&dir) };
    let updater = Updater::new(open(&mut repo).unwrap());
    let build = hand_on(&updater, &mut repo).1.unwrap();
    hand_on(&updater, &mut repo);
    assert_eq!(make(build, &mut repo), [Said::BuildFailed(Unbuilt::Exited(101))]);
    assert!(updater.update.building().is_none() && !updater.update.draining());
    assert!(updater.update.take_put_off() && !updater.update.take_put_off());
    // The put-off pass builds again, though HEAD is at origin/main.
    repo.build_ok = true;
    let (said, build) = hand_on(&updater, &mut repo);
    assert_eq!(said, [Said::At { head: "b41c2f0".into() }]);
    make(build.unwrap(), &mut repo);
    assert!(updater.update.draining());
    // A build handed on and dropped unmade ends too.
    let mut other = Repo::new(&dir);
    let updater = Updater::new(open(&mut other).unwrap());
    drop(hand_on(&updater, &mut other).1.unwrap());
    assert!(updater.update.building().is_none());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_push_is_pulled_once_the_fetch_holds_its_commit_and_fails_until_then() {
    let dir = checkout_dir();
    let after = "b41c2f0bbbb0000000000000000000000000000b";
    let mut repo = Repo { holds: false, ..Repo::new(&dir) };
    let updater = Updater::new(open(&mut repo).unwrap());
    // Fetched, with no wait first, and nothing merged or built: the relay's resend tries again.
    let (said, held, handed) = pushed(&updater, &mut repo, Some(after));
    assert_eq!(said, [Said::NotHeld { after: "b41c2f0".into() }]);
    assert!(!held && handed.is_none() && !repo.wrote("git") && repo.ran.iter().any(|r| r.starts_with("git fetch")));
    repo.holds = true;
    let (said, held, handed) = pushed(&updater, &mut repo, Some(after));
    assert_eq!(said, [Said::Moved { from: "a3d0e65".into(), head: "b41c2f0".into() }]);
    assert!(held && repo.wrote("git"));
    // A push while that build runs, or while draining, isn't pulled: it fails for the resend.
    let (said, held, again) = pushed(&updater, &mut repo, Some(after));
    assert_eq!(said, [Said::Building]);
    assert!(!held && again.is_none());
    make(handed.unwrap(), &mut repo);
    assert!(!pushed(&updater, &mut repo, Some(after)).1);
    // A failed fetch fails a push too, but a guard's skip doesn't.
    let mut unfetched = Repo { fetch_ok: false, ..Repo::new(&dir) };
    let updater = Updater::new(open(&mut unfetched).unwrap());
    let (said, held, _) = pushed(&updater, &mut unfetched, Some(after));
    assert_eq!(said, [Said::Unfetched(Unpulled::Fetch(128))]);
    assert!(!held);
    let mut dirty = Repo { dirty: true, ..Repo::new(&dir) };
    assert!(pushed(&updater, &mut dirty, Some(after)).1);
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_pass_builds_again_only_once_main_has_moved_since_its_last_build() {
    let dir = checkout_dir();
    let mut repo = Repo::new(&dir);
    let updater = Updater::new(open(&mut repo).unwrap());
    let build = hand_on(&updater, &mut repo).1.unwrap();
    // A push during the build is put off, and the build finds its binary in place already.
    hand_on(&updater, &mut repo);
    let mut said = Vec::new();
    build.make(&mut repo.run(), &mut |line| said.push(line), &mut |_| Ok(false), &mut released);
    assert_eq!(said, [Said::Built { head: "b41c2f0".into() }, Said::Released { head: "b41c2f0".into(), issues: 1 }]);
    assert!(updater.update.take_put_off() && !updater.update.draining());
    // The put-off pass finds main where it was built: no second build.
    let (said, handed) = hand_on(&updater, &mut repo);
    assert_eq!(said, [Said::At { head: "b41c2f0".into() }, Said::BuiltAlready { head: "b41c2f0".into() }]);
    assert!(handed.is_none() && updater.update.building().is_none());
    // Moved meanwhile: built again.
    repo.target = "c0ffee0cccc";
    let (said, handed) = hand_on(&updater, &mut repo);
    assert_eq!(said, [Said::Moved { from: "b41c2f0".into(), head: "c0ffee0".into() }]);
    assert!(handed.is_some());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_release_links_only_the_workspaces_teams_issues_and_at_most_the_newest_hundred() {
    let keys = |names: &[&str]| names.iter().map(|name| IssueKey::from(*name)).collect::<Vec<_>>();
    let teams = ["TW".to_string(), "WEB".to_string()];
    assert_eq!(linked(keys(&["TW-3", "UTF-8", "WEB-2", "HR-1", "TWX-4"]), &teams), keys(&["TW-3", "WEB-2"]));
    let many: Vec<String> = (0..150).map(|n| format!("TW-{n}")).collect();
    let linked = linked(many.iter().map(|name| IssueKey::from(name.as_str())).collect(), &teams);
    assert_eq!((linked.len(), linked[0].as_str()), (RELEASE_ISSUES, "TW-0"));
}
