// A worker host's `tolkster` made level with the primary's before its setup starts
// (docs/hosts.md, "Builds"): the `tolkster worker` line carries the commit the primary runs
// (`--build`), and a host on another fetches that commit's build from the Worker and swaps it in.
// The swap is a download beside the binary, then a rename over it, which a Linux process
// running that binary survives, on Linux and macOS alike. The setup then starts again on the new build, with the same
// arguments. A build the Worker can't give, or a swap that fails, leaves the host's own build
// running, with what happened printed.

use std::fs;
use std::path::{Path, PathBuf};
#[cfg(any(feature = "develop", not(unix)))]
use std::thread::sleep;
#[cfg(any(feature = "develop", not(unix)))]
use std::time::Duration;

use ureq::Agent;

use crate::config::{Files, host_credential};
use crate::error::{Context, Error};
use crate::exec::{Run, run_command};
use crate::plain::Sha;
use crate::trade;

// The swap's tries: a worker's `tolkster` run may hold the exe for a moment.
#[cfg(any(feature = "develop", not(unix)))]
const TRIES: usize = 3;
#[cfg(any(feature = "develop", not(unix)))]
const TRY_AFTER: Duration = Duration::from_secs(1);

// Set on the new build's start, so a build that isn't the one asked for doesn't swap again.
const SWAPPED: &str = "TOLKSTER_SWAPPED";

// The commit this build was made from (build.rs): None for a debug build, or one made without git.
pub fn commit() -> Option<Sha> {
    option_env!("TOLKSTER_COMMIT").and_then(Sha::parse)
}

// The repo whose `main` this build follows (build.rs): None unless CI stamped it.
#[cfg(not(feature = "develop"))]
pub fn repo() -> Option<String> {
    option_env!("TOLKSTER_REPO").map(String::from)
}

// The names CI gives its builds (.github/workflows/build.yml): the Worker's `TARGETS`, as
// test/contract.json pins.
pub const TARGETS: [&str; 3] = ["linux-x64", "macos-arm64", "windows-x64"];

// The name CI gives this build.
pub fn target() -> Option<&'static str> {
    let own = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => 0,
        ("macos", "aarch64") => 1,
        ("windows", "x86_64") => 2,
        _ => return None,
    };
    Some(TARGETS[own])
}

// Makes this host's build the one of `wanted`, and starts the worker again on it: only a host
// with a credential, asking the Worker only when its own commit differs. Returns what to print
// when the worker goes on with this build.
pub fn level(agent: &Agent, files: &Files, wanted: &Sha, argv: &[String]) -> Option<String> {
    if commit().as_ref() == Some(wanted) || host_credential(files).is_none() {
        return None;
    }
    let short = &wanted[..7];
    if std::env::var_os(SWAPPED).is_some() {
        return Some(format!("the build fetched for {short} says it is another commit: running it"));
    }
    let Some(target) = target() else {
        return Some(format!("the primary runs {short}, and no build is made for this platform: running its own"));
    };
    // Read before the download: a worker that started beside this one may swap first, after which
    // Linux names this process's binary "<path> (deleted)".
    let exe = match own_exe() {
        Ok(exe) => exe,
        Err(error) => return Some(format!("couldn't find its own binary to swap: {error}: running the current build")),
    };
    match trade::build(agent, files, target, wanted) {
        Ok(Some(tarball)) => match swap_in(&exe, &tarball, wanted, &mut run_command) {
            Ok(()) => Some(restart(&exe, argv)),
            Err(error) => Some(format!("couldn't swap in the build of {short}: {error}: running the current build")),
        },
        Ok(None) => Some(format!("the Worker has no build of {short} for {target} yet: running the current build")),
        Err(error) => Some(format!("couldn't fetch the build of {short}: {error}: running the current build")),
    }
}

// This process's binary, by the name a swap has taken from it: after a rename over it Linux
// names it "<path> (deleted)".
pub fn own_exe() -> std::io::Result<PathBuf> {
    Ok(PathBuf::from(std::env::current_exe()?.to_string_lossy().trim_end_matches(" (deleted)")))
}

// Unpacks `tarball` beside `exe`, checks that what it holds says it is `wanted`, and renames it
// over `exe`: until the rename nothing of `exe` has changed, and what was unpacked goes either
// way. Two workers starting together each swap in the same build.
pub(crate) fn swap_in(exe: &Path, tarball: &[u8], wanted: &Sha, run: &mut Run) -> Result<(), Error> {
    let dir = exe.parent().ok_or_else(|| Error::Failed("the binary has no folder".into()))?;
    let stage = dir.join(format!(".tolkster-{}-{}", &wanted[..7], std::process::id()));
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage).context(stage.display())?;
    let swapped = unpack_and_rename(&stage, exe, tarball, wanted, run);
    let _ = fs::remove_dir_all(&stage);
    swapped
}

fn unpack_and_rename(stage: &Path, exe: &Path, tarball: &[u8], wanted: &Sha, run: &mut Run) -> Result<(), Error> {
    let archive = stage.join("build.tar.gz");
    fs::write(&archive, tarball).context(archive.display())?;
    let (archive, folder) = (archive.display().to_string(), stage.display().to_string());
    if !run("tar", &["-xzf", &archive, "-C", &folder], None)?.ok {
        return Err(Error::Failed("tar couldn't unpack it".into()));
    }
    let unpacked = stage.join(if cfg!(windows) { "tolkster.exe" } else { "tolkster" });
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&unpacked, fs::Permissions::from_mode(0o755)).context(unpacked.display())?;
    }
    let said = run(&unpacked.display().to_string(), &["version"], None)?;
    if !said.ok || !said.stdout.contains(wanted.as_str()) {
        return Err(Error::Failed("what it unpacked isn't a build of that commit".into()));
    }
    place(&unpacked, exe)
}

// Unix renames the build over a running binary. Windows refuses that, but renames the running
// exe aside first: calls in flight keep it, and `clear_old` removes it at a later start.
#[cfg(unix)]
fn place(build: &Path, exe: &Path) -> Result<(), Error> {
    fs::rename(build, exe).context(format!("renaming the build over {}", exe.display()))
}

#[cfg(not(unix))]
fn place(build: &Path, exe: &Path) -> Result<(), Error> {
    use crate::time::now_ms;
    let name = exe.file_name().and_then(|n| n.to_str()).ok_or_else(|| Error::Failed("the binary has no file name".into()))?;
    swap_files(exe, build, &exe.with_file_name(format!("{name}.{}.old", now_ms())))
}

// The worker again, on the build just swapped in, in this process's place where the system
// allows it. Only a failure to start it comes back.
#[cfg(unix)]
fn restart(exe: &Path, argv: &[String]) -> String {
    use std::os::unix::process::CommandExt;
    let error = std::process::Command::new(exe).arg("worker").args(argv).env(SWAPPED, "1").exec();
    format!("swapped the build in, but couldn't start it: {error}: running the old one")
}

#[cfg(not(unix))]
fn restart(exe: &Path, argv: &[String]) -> String {
    match std::process::Command::new(exe).arg("worker").args(argv).env(SWAPPED, "1").status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => format!("swapped the build in, but couldn't start it: {error}: running the old one"),
    }
}

#[cfg(any(feature = "develop", not(unix)))]
pub(crate) fn swap_files(exe: &Path, build: &Path, aside: &Path) -> Result<(), Error> {
    retry(|| fs::rename(exe, aside)).context(format!("renaming {} aside", exe.display()))?;
    if let Err(error) = retry(|| fs::copy(build, exe).map(|_| ())) {
        let _ = fs::rename(aside, exe);
        return Err(error).context(format!("copying {} to {}", build.display(), exe.display()));
    }
    Ok(())
}

#[cfg(any(feature = "develop", not(unix)))]
pub(crate) fn retry(mut step: impl FnMut() -> std::io::Result<()>) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match step() {
            Ok(()) => return Ok(()),
            Err(error) => {
                tries += 1;
                if tries == TRIES {
                    return Err(error);
                }
                sleep(TRY_AFTER);
            }
        }
    }
}

#[cfg(test)]
mod tests;
