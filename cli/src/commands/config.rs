// `tolkster config`: the settings, as JSON; `config schema` prints the JSON Schema they are
// validated against.

use ureq::Agent;

use crate::args::parse;
use crate::cli::{print, print_json};
use crate::config::{self, files};
use crate::error::Error;
use crate::runtime::settings;

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
    print_json(&settings(&files)?)?;
    Ok(0)
}
