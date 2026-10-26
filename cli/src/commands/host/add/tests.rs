use super::*;

#[test]
fn a_recipe_host_takes_no_destination_and_no_yes() {
    let agent = ureq::agent();
    for argv in [["x", "--recipe", "r", "--destination", "/srv"], ["x", "--recipe", "r", "--yes", ""]] {
        let argv: Vec<String> = argv.iter().filter(|a| !a.is_empty()).map(|a| a.to_string()).collect();
        assert!(matches!(run(&agent, &argv), Err(Error::Usage(_))), "{argv:?}");
    }
}
