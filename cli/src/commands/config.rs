// `tolkster config`: the settings, as JSON; `config schema` prints the JSON Schema they are
// validated against.

use ureq::Agent;

use crate::args::parse;
use crate::cli::{home_dir, print, print_json};
use crate::config::{self, files, owner};
use crate::error::Error;

pub fn run(_agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    let parsed = parse(argv, &[], true)?;
    match parsed.positionals.as_slice() {
        [] => {}
        [sub] if sub == "schema" => {
            print(config::SCHEMA.trim_end());
            return Ok(0);
        }
        _ => return Err(Error::Usage("config takes no argument but schema".into())),
    }
    let files = files()?;
    print_json(&config::config(&files, owner(&files)?, &home_dir()?)?)?;
    Ok(0)
}
