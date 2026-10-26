// The tolkster CLI. Everything it writes to Linear appears as Tolkster, not as the
// person running it.

mod allowlist;
mod args;
mod brief;
mod card;
mod cli;
mod cloudflare;
mod coauthor;
mod command;
mod commands;
mod config;
mod context;
mod converse;
mod credential;
mod error;
mod estimates;
mod exec;
mod executor;
mod gateway;
mod git;
mod github;
mod harness;
mod harnesses;
mod hook;
mod health;
mod hosts;
mod http;
mod issue_state;
mod issues;
mod json;
mod linear;
mod listen;
mod live;
mod machine;
mod offline;
mod onboard;
mod orca;
mod panel;
mod plain;
mod ports;
mod post;
mod prebuilt;
mod reap;
mod recipe;
mod request;
mod review;
mod review_pass;
mod runtime;
mod scheduler;
mod sessions;
mod setup;
mod state;
mod sync;
mod time;
mod timing;
mod trace;
mod trade;
mod triage;
mod update;
mod voice;
mod watch;
mod wrangler;

use std::process::ExitCode;

use cli::print;
use commands::help;
use error::Error;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    match run(&argv) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(if matches!(error, Error::Usage(_)) { 2 } else { 1 })
        }
    }
}

fn run(argv: &[String]) -> Result<u8, Error> {
    let found = commands::find(argv);
    let dispatched = found.map(|(spec, _)| spec.words);
    if let Some(own) = cli::conversation_session() {
        allowlist::gate(argv, dispatched, &own)?;
    } else if let Some((root, _)) = cli::thread_turn()? {
        allowlist::gate_thread(argv, dispatched, &root)?;
    }
    if help::asked(argv) && let Some(block) = help::block(argv) {
        print(&block);
        return Ok(0);
    }
    let agent = http::agent();
    let result = match found {
        Some((spec, rest)) => (spec.run)(&agent, rest),
        None if argv.is_empty() => help::print(argv),
        None => Err(Error::Usage(format!("unknown command: {}\n\n{}", argv[0], help::usage()))),
    };
    // A command's usage error ends with its own block of the help, so a caller can correct
    // itself from the error alone.
    result.map_err(|error| match error {
        Error::Usage(message) => match help::block(argv) {
            Some(block) => Error::Usage(format!("{message}\n\n{block}")),
            None => Error::Usage(message),
        },
        error => error,
    })
}
