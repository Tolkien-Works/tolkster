// Credits Tolkster as co-author on a worker's commits, and on no other (docs/co-author.md).
// Only a linked worktree has a git dir of its own, so the primary checkout is never marked.
// The hook is plain `sh` and `git`, so a commit there never depends on `tolkster` being on PATH.

use std::path::Path;

use crate::error::{Context, Error};
use crate::exec::Run;
use crate::git;

pub const MARKER: &str = "tolkster-session";
const HOOK: &str = "tolkster-co-author";

// False for the primary checkout, a directory outside any repo, and anywhere git can't be asked.
pub fn marked(run: &mut Run, dir: Option<&str>) -> bool {
    git::git_dir(run, dir).is_some_and(|git_dir| Path::new(&git_dir).join(MARKER).is_file())
}

// The session a marked worktree was marked for, None where `marked` is false.
pub fn marked_session(run: &mut Run, dir: Option<&str>) -> Option<String> {
    let git_dir = git::git_dir(run, dir)?;
    std::fs::read_to_string(Path::new(&git_dir).join(MARKER)).ok().map(|id| id.trim().to_string())
}

// No single quote, so the trailer can sit inside the hook command's single-quoted word.
fn check(co_author: &str) -> Result<(), Error> {
    let (name, rest) = co_author.split_once(" <").unwrap_or(("", ""));
    let ok = !name.trim().is_empty()
        && rest.ends_with('>')
        && rest.len() > 1
        && !co_author.contains(['\'', '\n', '\r'])
        && !rest[..rest.len() - 1].contains(['<', '>']);
    if ok {
        Ok(())
    } else {
        Err(Error::Usage(format!("co-author must be \"Name <email>\" on one line, with no single quote: {co_author}")))
    }
}

// A message with only comments in it is left alone: with a trailer added, a commit whose
// editor was closed untouched would go ahead instead of aborting. So the same command runs
// again at `commit-msg`, after the editor, and credits what was written there.
pub fn hook_command(co_author: &str) -> Result<String, Error> {
    check(co_author)?;
    Ok(format!(
        concat!(
            r#"f() {{ test -f "$(git rev-parse --git-dir)/{marker}" || return 0; "#,
            r#"test -n "$(sed '/^. -\{{24\}} >8 -\{{24\}}$/,$d' "$1" | git stripspace --strip-comments)" || return 0; "#,
            r#"git interpret-trailers --in-place --if-exists addIfDifferent --trailer 'Co-authored-by: {co_author}' "$1"; }}; f"#
        ),
        marker = MARKER,
        co_author = co_author
    ))
}

// Safe to run again: the marker is rewritten and the hook's config set, not added to.
pub fn mark(run: &mut Run, worktree: &str, session_id: &str, co_author: Option<&str>) -> Result<(), Error> {
    let command = co_author.map(hook_command).transpose()?;
    let (git_dir, common_dir) = git::git_dirs(run, worktree)?;
    if git_dir == common_dir {
        return Err(Error::Failed(format!("{worktree} is a repo's primary checkout, not a worktree: only a worker's worktree is marked")));
    }
    std::fs::write(Path::new(&git_dir).join(MARKER), format!("{session_id}\n")).context(format!("writing {worktree}'s {MARKER}"))?;
    let Some(command) = command else {
        // No co-author: a hook an earlier mark set would go on crediting the old one.
        return git::remove_hook_config(run, worktree, HOOK);
    };
    git::set_hook_config(run, worktree, HOOK, &["prepare-commit-msg", "commit-msg"], &command)?;
    // A Git that doesn't know config hooks would ignore them, and credit nobody.
    if !git::hook_listed(run, worktree, "prepare-commit-msg", HOOK)? {
        return Err(Error::Failed(format!("this git doesn't run hooks from config, so {HOOK} won't run: update Git")));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
