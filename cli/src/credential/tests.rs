use super::*;

fn value(env: &[(String, String)], key: &str) -> String {
    env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).unwrap_or_else(|| panic!("no {key}"))
}

#[test]
fn the_helper_is_added_after_the_entries_already_given() {
    let env = git_env(Some("1"), r"C:\bin\tolkster.exe", "o/tolkster").unwrap();
    assert_eq!(value(&env, "GIT_CONFIG_COUNT"), "3");
    assert_eq!(value(&env, "GIT_CONFIG_KEY_1"), "credential.https://github.com.helper");
    assert_eq!(value(&env, "GIT_CONFIG_VALUE_1"), "");
    assert_eq!(value(&env, "GIT_CONFIG_VALUE_2"), "!'C:/bin/tolkster.exe' git-credential o/tolkster");
    assert_eq!(value(&git_env(None, "/bin/t", "o/r").unwrap(), "GIT_CONFIG_COUNT"), "2");
}

#[test]
fn what_cant_sit_in_a_helper_line_gets_no_helper() {
    assert!(git_env(None, "/it's/tolkster", "o/r").is_none());
    assert!(git_env(None, "/bin/tolkster", "o/r; rm -rf").is_none());
    assert!(git_env(Some("many"), "/bin/tolkster", "o/r").is_none());
}

#[test]
fn only_github_over_https_gets_a_token() {
    let token = || Ok("ghs_secret".to_string());
    let asked = |input: &str| answer(input, token).unwrap();
    assert_eq!(asked("protocol=https\nhost=github.com\npath=o/r.git\n\n"), "username=x-access-token\npassword=ghs_secret\n");
    for other in ["protocol=https\nhost=gitlab.com\n", "protocol=http\nhost=github.com\n", "host=github.com\n", "protocol=https\nhost=github.com.evil.example\n"] {
        assert_eq!(asked(other), "", "{other}");
    }
    // The token isn't asked for when the answer is nothing.
    assert_eq!(answer("protocol=https\nhost=gitlab.com\n", || panic!("asked")).unwrap(), "");
    assert!(answer("protocol=https\nhost=github.com\n", || Ok("a\nb".into())).is_err());
}
