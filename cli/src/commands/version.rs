// `tolkster version` and `tolkster --version`: the crate's version and, for a build stamped with one, its commit, to tell which build is installed.

use crate::args::parse;
use crate::cli::print;
use crate::error::Error;

pub fn run(argv: &[String]) -> Result<u8, Error> {
    parse(argv, &[], false)?;
    print(&match crate::prebuilt::commit() {
        Some(commit) => format!("{} ({commit})", env!("CARGO_PKG_VERSION")),
        None => env!("CARGO_PKG_VERSION").into(),
    });
    Ok(0)
}

#[cfg(test)]
mod tests;
