use super::*;

fn updater() -> Updater {
    Updater { repo: "Tolkien-Works/tolkster".into(), own: Sha::parse(&"a".repeat(40)).unwrap(), target: "linux-x64", agent: Agent::new_with_defaults(), update: Update::default() }
}

fn take(after: Option<&Sha>, fetch: impl FnOnce(&Sha) -> Result<Option<Vec<u8>>, Error>) -> (Pass, Vec<Said>) {
    let mut said = Vec::new();
    let pass = updater().take(&mut |_, _, _| panic!("ran a command"), &mut |line| said.push(line), after, fetch);
    (pass, said)
}

#[test]
fn only_a_push_to_the_build_repos_main_is_taken() {
    let updater = updater();
    assert!(updater.is_own("tolkien-works/Tolkster", "refs/heads/main"));
    assert!(!updater.is_own("Tolkien-Works/tolkster", "refs/heads/other"));
    assert!(!updater.is_own("Tolkien-Works/other", "refs/heads/main"));
}

#[test]
fn a_tip_is_held_while_the_worker_has_no_build_and_asks_nothing_when_already_on_it() {
    let b = Sha::parse(&"b".repeat(40)).unwrap();
    let (pass, said) = take(Some(&b), |_| Ok(None));
    assert_eq!((pass, said), (Pass::Behind, vec![Said::NotHeld { after: "bbbbbbb".into() }]));
    let own = updater().own;
    let (pass, said) = take(Some(&own), |_| panic!("asked for the build it runs"));
    assert_eq!((pass, said), (Pass::Pulled, vec![Said::At { head: "aaaaaaa".into() }]));
    assert_eq!(take(None, |_| panic!("asked with no commit")).0, Pass::Skipped);
}

#[test]
fn an_unreachable_worker_is_tried_again_and_a_refusal_is_not() {
    let b = Sha::parse(&"b".repeat(40)).unwrap();
    assert_eq!(take(Some(&b), |_| Err(Error::Unavailable("down".into()))).0, Pass::Unready);
    assert_eq!(take(Some(&b), |_| Err(Error::Failed("unauthorized".into()))).0, Pass::Skipped);
}
