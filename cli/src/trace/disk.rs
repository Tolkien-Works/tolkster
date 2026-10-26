// `listen`'s lines kept in a file beside the panel, so a restart leaves the run before it
// (docs/realtime.md). Each is the line `--verbose` shows, errors' text and opaque IDs included,
// for the owner alone (0600 on Unix) and never read back. Two files bound it: when the current one would pass
// `MAX` it becomes `<name>.1`, replacing the one before, and a new one begins. One `listen`
// writes at a time; a second beside it (a superseded one finishing) may rotate at the wrong
// moment, losing at most the older file.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::sync::lock;

const MAX: u64 = 5 * 1024 * 1024;

// The process's disk log, written by every thread's lines (trace.rs).
static LOG: Mutex<Option<Log>> = Mutex::new(None);

struct Log {
    path: PathBuf,
    file: File,
    size: u64,
    max: u64,
}

fn append_to(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

impl Log {
    fn open(path: PathBuf, max: u64) -> io::Result<Log> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = append_to(&path)?;
        let size = file.metadata()?.len();
        Ok(Log { path, file, size, max })
    }

    fn keep(&mut self, line: &str) -> io::Result<()> {
        let text: String = line.chars().map(|c| if c.is_control() { ' ' } else { c }).chain(['\n']).collect();
        if self.size > 0 && self.size + text.len() as u64 > self.max {
            let mut older = self.path.clone().into_os_string();
            older.push(".1");
            fs::rename(&self.path, older)?;
            self.file = append_to(&self.path)?;
            self.size = 0;
        }
        self.file.write_all(text.as_bytes())?;
        self.size += text.len() as u64;
        Ok(())
    }
}

// From now on every kept line goes to `path`, after what a run before it left.
pub fn open(path: PathBuf) -> io::Result<()> {
    *lock(&LOG) = Some(Log::open(path, MAX)?);
    Ok(())
}

// A write that fails ends the log for this run, and is the error: the panel and stdout go on
// as they were. A control character (a file name's newline) becomes a space, so a line is one.
pub fn keep(line: &str) -> io::Result<()> {
    let mut log = lock(&LOG);
    let Some(open) = log.as_mut() else { return Ok(()) };
    let kept = open.keep(line);
    if kept.is_err() {
        *log = None;
    }
    kept
}

#[cfg(test)]
mod tests;

