// `tolkster help`, printed from each command's entry in the command table (commands::COMMANDS),
// and a command's own block of it, for its usage errors and its --help.

use crate::args::parse;
use crate::cli::{config_file, credentials_file, print as print_line, shown};
use crate::commands::{COMMANDS, Spec};
use crate::error::Error;
use crate::voice;

// What the help prints before a command's entry: a blank line between its groups, or a
// paragraph of prose.
const BEFORE: &[(&[&str], &str)] = &[
    (&["issue", "show"], "\n"),
    (&["issue", "list"], "\n"),
    (
        &["issue", "update"],
        "
  Changes to issues, as Tolkster. --session <id> names the owner's session that asked for
  the change, and is checked to be theirs. The owner's own agent, at their checkout, leaves
  it out; a worker or a conversation's turn, which each have a session, is refused that (exit 1):

",
    ),
    (&["triage"], "\n"),
    (&["pr", "create"], "\n"),
    (&["land"], "\n"),
    (&["reap"], "\n"),
];

pub fn usage() -> String {
    let mut help = String::from("tolkster <command>\n\n");
    let mut last = "";
    for spec in COMMANDS.iter().filter(|spec| !spec.help.is_empty()) {
        // Subcommands that share an entry, `issue relate|unrelate`, print it once.
        if spec.help == last {
            continue;
        }
        last = spec.help;
        help += BEFORE.iter().find(|(words, _)| *words == spec.words).map_or("", |(_, before)| before);
        help += &entry(spec);
        help += "\n";
    }
    help + &format!(
        "
  The owner, the settings, the credentials and which commands need the Worker are in
  docs/cli.md (\"The owner and sessions\") and docs/setup.md (\"Making it yours\").
  Settings: {}. Credentials: {}.",
        shown(config_file(), "~/.tolkster/config.json"),
        shown(credentials_file(), "~/.tolkster/credentials.json"),
    )
}

// `tolkster help`, `--help`, or nothing at all.
pub fn print(argv: &[String]) -> Result<u8, Error> {
    parse(argv, &[], false)?;
    print_line(&usage());
    Ok(0)
}

fn entry(spec: &Spec) -> String {
    let help = spec.help.strip_prefix('\n').unwrap_or(spec.help);
    help.replace("{CHOICES}", &voice::CHOICES.map(|c| c.name).join(", ")).replace("{STATES}", &voice::states().collect::<Vec<_>>().join(", "))
}

// A command's entries in the help: those of the subcommand `argv` names, or every one of the
// command's when it names none the help has.
pub fn block(argv: &[String]) -> Option<String> {
    let command = argv.first()?;
    let entries: Vec<&Spec> = COMMANDS.iter().filter(|spec| spec.words[0] == command && !spec.help.is_empty()).collect();
    let named: Vec<&Spec> = entries.iter().copied().filter(|spec| spec.words.get(1).copied() == argv.get(1).map(String::as_str)).collect();
    let mut lines: Vec<String> = if named.is_empty() { entries } else { named }.into_iter().map(entry).collect();
    lines.dedup();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

// `--help` among a command's options, before any `--`: a value can't be `--help`, since
// args.rs takes one that starts with '-' only as `--name=<value>`.
pub fn asked(argv: &[String]) -> bool {
    argv.iter().skip(1).take_while(|a| *a != "--").any(|a| a == "--help")
}

#[cfg(test)]
mod tests;
