// A git credential helper for a worker host's pushes (docs/github-app.md). `tolkster worker`
// puts it in the environment of what it runs and of its agent, for the worktree's repo
// alone, so no config file on the host is touched and the owner's own git use there is not.
// It answers git with the app's token for that repo (`github::push_token`) and with nothing
// for another host or protocol. The token goes to git on stdout and nowhere else: not to a
// terminal, a log or an error.

use crate::error::Error;
use crate::plain::split_repo;

pub const COMMAND: &str = "git-credential";

// The only host it answers for: a token for one repo is not for another host's eyes.
const HOST: &str = "github.com";
const SCOPE: &str = "credential.https://github.com.helper";

// The variables that make git ask `exe git-credential <repo> get` for github.com's
// credentials, after `count` configuration entries already given through
// `GIT_CONFIG_COUNT`. The empty helper first clears the host's own, so a worker's push is the
// app's whatever login the host has. None for what can't be put in a helper line safely.
pub fn git_env(count: Option<&str>, exe: &str, repo: &str) -> Option<Vec<(String, String)>> {
    let first = match count {
        None => 0,
        Some(count) => count.trim().parse::<usize>().ok()?,
    };
    split_repo(repo)?;
    if exe.contains(['\'', '\n']) {
        return None;
    }
    // A Windows path in a `!` helper, which git runs through `sh`.
    let exe = exe.replace('\\', "/");
    let helper = format!("!'{exe}' {COMMAND} {repo}");
    let mut env = Vec::new();
    for (n, value) in [(first, ""), (first + 1, helper.as_str())] {
        env.push((format!("GIT_CONFIG_KEY_{n}"), SCOPE.to_string()));
        env.push((format!("GIT_CONFIG_VALUE_{n}"), value.to_string()));
    }
    env.push(("GIT_CONFIG_COUNT".into(), (first + 2).to_string()));
    Some(env)
}

// What git is told for `get` with `input` on stdin: the credential `token` makes, only when
// git asks for https on github.com; else nothing, and `token` isn't called.
pub fn answer(input: &str, token: impl FnOnce() -> Result<String, Error>) -> Result<String, Error> {
    let field = |key: &str| input.lines().take_while(|line| !line.is_empty()).find_map(|line| line.strip_prefix(key)?.strip_prefix('='));
    if field("protocol") != Some("https") || !field("host").is_some_and(|host| host.eq_ignore_ascii_case(HOST)) {
        return Ok(String::new());
    }
    let token = token()?;
    if token.is_empty() || token.chars().any(char::is_control) {
        return Err(Error::Failed("the Worker's github token had another shape".into()));
    }
    Ok(format!("username=x-access-token\npassword={token}\n"))
}

#[cfg(test)]
mod tests;
