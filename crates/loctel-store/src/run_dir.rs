//! The run directory: where one run of loctel keeps its segment files.
//!
//! Each run makes `<base>/run-<pid>-<start_unix_secs>/` (the app's base is
//! `std::env::temp_dir()/loctel`) and deletes it when the [`RunDir`] is
//! dropped. A run that was killed leaves its directory behind, so the next one
//! deletes every sibling `run-<pid>-*` whose process is gone before it makes
//! its own. Nothing in the directory is meant to outlive the run.
//!
//! This module is public only so that tests can reach it.

use std::fs;
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// How many directory names are tried before giving up: the start time is
/// moved on by a second whenever the name is taken.
const NAME_ATTEMPTS: u32 = 1000;

/// One run's directory, deleted when it is dropped.
#[derive(Debug)]
pub struct RunDir {
    path: PathBuf,
}

impl RunDir {
    /// Makes this run's directory under `base` (and `base` itself, when it is
    /// missing), after deleting the stale siblings: directories called
    /// `run-<pid>-<anything>` whose pid is not a live process. Anything else in
    /// `base` is left alone, and so is a directory that cannot be deleted (it
    /// is said on stderr, and the run goes on).
    ///
    /// Directories are made readable by their owner only: they hold copies of
    /// the user's traces and logs.
    ///
    /// Two runs in one process, in one second, get different names (the start
    /// time moves on), so a test can make several.
    pub fn create(base: &Path) -> io::Result<RunDir> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(base)?;
        remove_stale(base);

        let pid = std::process::id();
        let mut started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let mut attempt = 1;
        loop {
            let path = base.join(format!("run-{pid}-{started}"));
            match fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(RunDir { path }),
                Err(error)
                    if error.kind() == io::ErrorKind::AlreadyExists && attempt < NAME_ATTEMPTS =>
                {
                    started += 1;
                    attempt += 1;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RunDir {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.path)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!(
                "loctel: cannot delete the run directory {}: {error}",
                self.path.display()
            );
        }
    }
}

/// The pid in a run directory's name, `None` when the name is not one.
fn run_pid(name: &str) -> Option<u32> {
    let (pid, _) = name.strip_prefix("run-")?.split_once('-')?;
    if pid.is_empty() || !pid.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    pid.parse().ok()
}

/// Deletes the run directories in `base` that no live process owns.
fn remove_stale(base: &Path) {
    let Ok(entries) = fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(run_pid) else {
            continue;
        };
        // Not a link (`DirEntry::file_type` does not follow them), and not a
        // file: only a real directory with a run's name is a run's.
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) || pid_is_alive(pid) {
            continue;
        }
        let path = entry.path();
        if let Err(error) = fs::remove_dir_all(&path)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!(
                "loctel: cannot delete the stale run directory {}: {error}",
                path.display()
            );
        }
    }
}

/// Whether a process with this pid exists: `kill(pid, 0)` sends nothing and
/// only reports whether it could. A process we may not signal (`EPERM`) is
/// alive. A pid that is not a pid (0, or too big) is not ours to judge, so it
/// counts as alive and its directory stays.
///
/// A pid that has been given to a different process since reads as alive, and
/// so does the pid of a process nobody has waited for yet; either leaves a
/// directory behind until that process ends.
fn pid_is_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return true;
    };
    if pid <= 0 {
        return true;
    }
    // SAFETY: signal 0 delivers nothing; `kill` only checks the pid.
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
