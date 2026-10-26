// `tolkster whoami`: checks the credentials, and prints the app user.

use ureq::Agent;

use crate::args::parse;
use crate::cli::print;
use crate::error::Error;
use crate::sessions::workspace::viewer;

pub fn run(agent: &Agent, argv: &[String]) -> Result<u8, Error> {
    parse(argv, &[], false)?;
    let viewer = viewer(agent)?;
    print(&format!("{} ({})", viewer.name, viewer.id));
    Ok(0)
}
