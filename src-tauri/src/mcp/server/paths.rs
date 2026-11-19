//! Where the lock and the socket live (ADR-027 decision 2, "Location" and "Fallback").
//!
//! The directory is `$XDG_RUNTIME_DIR/orcastudio/`. When `XDG_RUNTIME_DIR` is unset or not absolute,
//! it is `mcp/` inside the app data dir instead, with the same modes. Both hold `mcp.lock` and
//! `mcp.sock`. A socket path longer than `sun_path` allows is refused, with the reason, before
//! anything is created.

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// The lock file's name inside the MCP directory.
pub const LOCK_FILE: &str = "mcp.lock";
/// The socket's name inside the MCP directory.
pub const SOCKET_FILE: &str = "mcp.sock";

/// The resolved MCP directory and the two files in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPaths {
    /// Created `0700`; holds the two files below.
    pub dir: PathBuf,
    pub lock: PathBuf,
    pub socket: PathBuf,
    pub base: PathBase,
}

/// Which base the directory was resolved from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathBase {
    /// `$XDG_RUNTIME_DIR/orcastudio`.
    RuntimeDir,
    /// `<app data dir>/mcp`: `XDG_RUNTIME_DIR` was unset or not absolute.
    DataDir,
}

/// Why MCP cannot start at this path. Shown as the status reason.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathRefusal {
    #[error(
        "the MCP socket path {path} is {len} bytes, over the {max}-byte sun_path limit, so MCP cannot start"
    )]
    SocketPathTooLong { path: PathBuf, len: usize, max: usize },
    #[error("neither XDG_RUNTIME_DIR nor the app data dir is known, so MCP cannot start")]
    NoBaseDir,
}

/// The longest socket path `bind` accepts, in bytes: `sun_path` less its terminating NUL. 107 on
/// Linux (measured by M0: 107 binds, 108 fails; `paths_tests` re-measures it).
pub fn sun_path_max() -> usize {
    // SAFETY: `sockaddr_un` is plain old data; all-zero is a valid value of it.
    let addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_path.len() - 1
}

/// Resolve the paths from `runtime_dir` (`XDG_RUNTIME_DIR`, used only when absolute) and
/// `app_data_dir` (`<data dir>/orcastudio`, the fallback). Pure: nothing is created.
pub fn resolve(runtime_dir: Option<&Path>, app_data_dir: Option<&Path>) -> Result<McpPaths, PathRefusal> {
    let (dir, base) = match (runtime_dir.filter(|p| p.is_absolute()), app_data_dir) {
        (Some(runtime), _) => (runtime.join("orcastudio"), PathBase::RuntimeDir),
        (None, Some(data)) => (data.join("mcp"), PathBase::DataDir),
        (None, None) => return Err(PathRefusal::NoBaseDir),
    };
    let socket = dir.join(SOCKET_FILE);
    let len = socket.as_os_str().as_bytes().len();
    let max = sun_path_max();
    if len > max {
        return Err(PathRefusal::SocketPathTooLong { path: socket, len, max });
    }
    Ok(McpPaths { lock: dir.join(LOCK_FILE), socket, dir, base })
}

/// [`resolve`] for this user: `dirs::runtime_dir()` (which already returns `XDG_RUNTIME_DIR` only
/// when it is absolute) and the app data dir the rest of the app uses.
pub fn resolve_for_this_user() -> Result<McpPaths, PathRefusal> {
    let data = dirs::data_dir().map(|d| d.join("orcastudio"));
    resolve(dirs::runtime_dir().as_deref(), data.as_deref())
}
