// How a program runs, for every adapter that runs one: its `Output`, the `Run` each takes so a
// test can stand in for the process, and `call`, which fails saying what the program said.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::{Context, Error};
use crate::trace;

pub struct Output {
    pub ok: bool,
    // None when the process ended without an exit code.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

#[cfg(test)]
impl Output {
    pub fn ok(stdout: impl Into<String>) -> Output {
        Output { ok: true, code: Some(0), stdout: stdout.into(), stderr: String::new() }
    }

    pub fn failed(code: i32, stderr: impl Into<String>) -> Output {
        Output { ok: false, code: Some(code), stdout: String::new(), stderr: stderr.into() }
    }
}

pub type Run<'a> = dyn FnMut(&str, &[&str], Option<&str>) -> Result<Output, Error> + 'a;

// The program's stdout, or a failure with its stderr.
pub fn call(run: &mut Run, program: &str, args: &[&str], dir: Option<&str>) -> Result<String, Error> {
    match run(program, args, dir)? {
        output if output.ok => Ok(output.stdout),
        output => Err(Error::Failed(failure(program, args, &output, None))),
    }
}

// A failed run's line: the program and its subcommand, then `detail`, else its stderr.
pub fn failure(program: &str, args: &[&str], output: &Output, detail: Option<String>) -> String {
    let detail = detail.or_else(|| Some(output.stderr.trim().to_string()).filter(|stderr| !stderr.is_empty()));
    format!("{program} {} failed{}", args[..args.len().min(2)].join(" "), detail.map_or(String::new(), |detail| format!(": {detail}")))
}

pub fn run_command(program: &str, args: &[&str], dir: Option<&str>) -> Result<Output, Error> {
    run_command_with(&[], program, args, dir)
}

// `run_command` with `env` added to the program's environment.
pub fn run_command_with(env: &[(String, String)], program: &str, args: &[&str], dir: Option<&str>) -> Result<Output, Error> {
    let mut command = Command::new(program);
    command.args(args).envs(env.iter().map(|(k, v)| (k, v))).stdin(Stdio::null());
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let started = Instant::now();
    let output = command.output().context(program);
    // A wait for a terminal's exit is meant to be long.
    if args.get(..2) != Some(&["terminal", "wait"][..]) || !args.contains(&"exit") {
        trace::slow_after(|| command_site(program, args), started, slow_cutoff(program, args));
    }
    let output = output?;
    Ok(Output {
        ok: output.status.success(),
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

// A worktree create is a full checkout, a registration and a terminal: seconds are normal.
// Any other Orca call over `SLOW` is routine, so only one past 5 s says where a job's time went.
fn slow_cutoff(program: &str, args: &[&str]) -> Duration {
    match program {
        "orca" if args.get(..2) == Some(&["worktree", "create"][..]) => Duration::from_secs(10),
        "orca" => Duration::from_secs(5),
        _ => trace::SLOW,
    }
}

// A command's call site, as a slow one is logged: the program and its subcommand, the
// leading words of plain lowercase letters after any `-c <setting>` pairs, never a path,
// a branch or an ID.
fn command_site(program: &str, args: &[&str]) -> String {
    let settings = args.chunks(2).take_while(|pair| pair[0] == "-c" && pair.len() == 2).count() * 2;
    let words = args[settings..].iter().take(2).take_while(|a| !a.is_empty() && a.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') && !a.starts_with('-'));
    std::iter::once(program).chain(words.copied()).collect::<Vec<_>>().join(" ")
}

// `run_command`, calling `beat` first: each command an act runs shows it is still making
// progress, however many worktrees or PRs a step goes through.
pub fn beating(beat: &dyn Fn()) -> impl FnMut(&str, &[&str], Option<&str>) -> Result<Output, Error> + '_ {
    move |program, args, dir| {
        beat();
        run_command(program, args, dir)
    }
}

#[cfg(test)]
mod tests;
