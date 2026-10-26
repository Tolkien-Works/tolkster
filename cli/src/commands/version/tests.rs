use super::*;

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}


#[test]
fn an_extra_argument_is_a_usage_error() {
    assert!(matches!(run(&argv(&["--bogus"])), Err(Error::Usage(_))));
}
