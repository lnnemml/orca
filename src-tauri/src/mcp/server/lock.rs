//! The MCP lock (ADR-027 decision 2; M0 amendment, item 4): an exclusive non-blocking `flock` on
//! `mcp.lock`. Only its holder reconciles, unlinks a stale socket and binds.
//!
//! **The fd must not leak into a child.** M0 measured that a forked child which inherited the fd
//! kept the `flock` after its parent was `kill -9`ed. So the file is opened `O_CLOEXEC`, and as a
//! post-condition (rule #9) the fd's `FD_CLOEXEC` is read back with `fcntl(F_GETFD)` before the
//! `flock` is taken; a lock whose fd lacks it is refused. std's `File::open` already sets
//! `O_CLOEXEC` on Linux (measured: `lock_tests::std_file_open_already_sets_cloexec`), so the
//! explicit flag is belt and braces; the assertion is what guards.
//!
//! The lock is held while the [`McpLock`] lives and released when it drops (the fd closes). The
//! lifecycle keeps it from the first acquisition until the exit flow, never dropping it on
//! switch-off.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// A held MCP lock. Dropping it closes the fd, which releases the `flock`.
#[derive(Debug)]
pub struct McpLock {
    // Never read: it exists to keep the fd (and so the flock) open.
    _file: File,
    path: PathBuf,
}

impl McpLock {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// The outcome of one non-blocking attempt.
#[derive(Debug)]
pub enum Acquire {
    Acquired(McpLock),
    /// Another open file description holds the lock: another OrcaStudio instance.
    HeldElsewhere,
}

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("cannot open the MCP lock file {path}: {source}")]
    Open { path: PathBuf, source: io::Error },
    #[error("fcntl(F_GETFD) on the MCP lock fd failed: {0}")]
    Fcntl(io::Error),
    #[error("the MCP lock fd does not carry FD_CLOEXEC, so a child process could keep the lock; refused")]
    NoCloexec,
    #[error("flock on the MCP lock file {path} failed: {source}")]
    Flock { path: PathBuf, source: io::Error },
}

/// One non-blocking attempt at the lock on `path` (the file is created `0600` if missing; its
/// directory must exist).
pub fn try_acquire(path: &Path) -> Result<Acquire, LockError> {
    try_acquire_with(path, |_| {})
}

/// [`try_acquire`] with a hook between the open and the `FD_CLOEXEC` check, so a test can strip
/// the flag and see the post-condition refuse.
pub(crate) fn try_acquire_with(path: &Path, after_open: impl FnOnce(&File)) -> Result<Acquire, LockError> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC)
        .open(path)
        .map_err(|source| LockError::Open { path: path.to_path_buf(), source })?;
    after_open(&file);

    // Post-condition before the lock is taken, so a lock a child could inherit is never held.
    if !has_cloexec(&file).map_err(LockError::Fcntl)? {
        return Err(LockError::NoCloexec);
    }

    loop {
        // SAFETY: `file` owns a valid open fd for the duration of the call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Acquire::Acquired(McpLock { _file: file, path: path.to_path_buf() }));
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EWOULDBLOCK) => return Ok(Acquire::HeldElsewhere),
            Some(libc::EINTR) => continue,
            _ => return Err(LockError::Flock { path: path.to_path_buf(), source: err }),
        }
    }
}

/// Whether `file`'s fd carries `FD_CLOEXEC` (`fcntl(F_GETFD)`).
pub fn has_cloexec(file: &File) -> io::Result<bool> {
    // SAFETY: `file` owns a valid open fd; F_GETFD takes no argument.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(flags & libc::FD_CLOEXEC != 0)
}
