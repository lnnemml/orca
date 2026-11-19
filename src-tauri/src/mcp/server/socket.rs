//! The socket file (ADR-027 decision 2, "Permissions"; M0 amendment, item 2).
//!
//! - **Explicit modes, never umask.** M0 measured a socket bound under umask 022 as `0755`. So the
//!   directory is set to `0700` and the socket to `0600` by `chmod` after they exist. The window
//!   between `bind` and `chmod` is closed by the directory, which is already `0700`.
//! - **Post-condition (rule #9).** After the bind both are `stat`ed. A mode, owner or file type
//!   that differs refuses to serve, and the socket just bound is removed.
//! - **Only the lock holder** calls [`bind`]: it removes a stale socket first. The caller enforces
//!   that (the lifecycle binds only while it holds the lock).
//! - **The inode.** A bound socket is identified by its `(dev, ino)`. [`unlink_if_same`] removes the
//!   file only if it is still that socket, so an old stop can never remove a newer socket (M2
//!   amendment, item 6).

use std::fs::{self, DirBuilder, Permissions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use super::paths::McpPaths;

pub const DIR_MODE: u32 = 0o700;
pub const SOCKET_MODE: u32 = 0o600;

#[derive(Debug, thiserror::Error)]
pub enum SocketError {
    #[error("cannot create the MCP directory {path}: {source}")]
    CreateDir { path: PathBuf, source: io::Error },
    #[error("the MCP directory {path} is not a directory owned by this user (a symlink or another uid); refused")]
    NotOurDir { path: PathBuf },
    #[error("{path} exists and is not a socket; it is not removed, so MCP does not start")]
    NotASocket { path: PathBuf },
    #[error("{op} {path}: {source}")]
    Io { op: &'static str, path: PathBuf, source: io::Error },
    #[error("after bind, {path} has mode {found:o} and owner {owner} (right file type: {right_kind}); expected mode {expected:o} and owner {own_uid}; refused")]
    Mismatch { path: PathBuf, found: u32, expected: u32, owner: u32, own_uid: u32, right_kind: bool },
}

/// A bound socket's identity: its path and the `(dev, ino)` the bind created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketIdentity {
    pub path: PathBuf,
    pub dev: u64,
    pub ino: u64,
}

/// A listener on the MCP socket, with the identity of the file it bound.
#[derive(Debug)]
pub struct BoundSocket {
    pub listener: UnixListener,
    pub identity: SocketIdentity,
}

/// What [`unlink_if_same`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unlinked {
    Removed,
    AlreadyGone,
    /// The path holds a different file (a newer socket): left in place.
    Replaced,
}

/// A hook run after the `chmod`s and before the `stat` post-condition, so a test can change a mode
/// and see the post-condition refuse.
pub type AfterBind<'a> = Option<&'a (dyn Fn(&McpPaths) + Send + Sync)>;

/// Create the MCP directory (`0700`, set explicitly) or adopt an existing one, which must be a real
/// directory owned by `own_uid`. Its parents are created with default modes if missing.
pub fn prepare_dir(dir: &Path, own_uid: u32) -> Result<(), SocketError> {
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent).map_err(|source| SocketError::CreateDir { path: parent.to_path_buf(), source })?;
    }
    match DirBuilder::new().mode(DIR_MODE).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(source) => return Err(SocketError::CreateDir { path: dir.to_path_buf(), source }),
    }
    let meta = fs::symlink_metadata(dir).map_err(|source| io_err("stat", dir, source))?;
    if !meta.file_type().is_dir() || meta.uid() != own_uid {
        return Err(SocketError::NotOurDir { path: dir.to_path_buf() });
    }
    fs::set_permissions(dir, Permissions::from_mode(DIR_MODE)).map_err(|source| io_err("chmod", dir, source))
}

/// Bind the MCP socket. **Only the lock holder may call this.** It removes a stale socket at the
/// path (never any other kind of file), binds, sets `0600`, and checks both modes as the
/// post-condition. On a refusal, the socket it bound is removed.
pub fn bind(paths: &McpPaths, own_uid: u32, after_bind: AfterBind<'_>) -> Result<BoundSocket, SocketError> {
    let path = &paths.socket;
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_socket() => {
            fs::remove_file(path).map_err(|source| io_err("remove the stale socket", path, source))?;
        }
        Ok(_) => return Err(SocketError::NotASocket { path: path.clone() }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(io_err("stat", path, source)),
    }

    let listener = UnixListener::bind(path).map_err(|source| io_err("bind", path, source))?;
    let meta = fs::symlink_metadata(path).map_err(|source| io_err("stat", path, source))?;
    let identity = SocketIdentity { path: path.clone(), dev: meta.dev(), ino: meta.ino() };

    let checked = fs::set_permissions(path, Permissions::from_mode(SOCKET_MODE))
        .map_err(|source| io_err("chmod", path, source))
        .and_then(|()| {
            if let Some(hook) = after_bind {
                hook(paths);
            }
            check_modes(paths, own_uid)
        });
    match checked {
        Ok(()) => Ok(BoundSocket { listener, identity }),
        Err(refusal) => {
            drop(listener);
            // Best effort: the refusal is the error the caller shows.
            let _ = unlink_if_same(&identity);
            Err(refusal)
        }
    }
}

/// The post-condition: the directory is a `0700` directory and the socket a `0600` socket, both
/// owned by `own_uid`.
pub fn check_modes(paths: &McpPaths, own_uid: u32) -> Result<(), SocketError> {
    check_one(&paths.dir, DIR_MODE, own_uid, |t| t.is_dir())?;
    check_one(&paths.socket, SOCKET_MODE, own_uid, |t| t.is_socket())
}

fn check_one(path: &Path, expected: u32, own_uid: u32, kind: impl Fn(&fs::FileType) -> bool) -> Result<(), SocketError> {
    let meta = fs::symlink_metadata(path).map_err(|source| io_err("stat", path, source))?;
    let found = meta.permissions().mode() & 0o7777;
    let is_kind = kind(&meta.file_type());
    if found != expected || meta.uid() != own_uid || !is_kind {
        return Err(SocketError::Mismatch {
            path: path.to_path_buf(),
            found,
            expected,
            owner: meta.uid(),
            own_uid,
            right_kind: is_kind,
        });
    }
    Ok(())
}

/// Remove the socket at `identity.path` only if it is still the file `identity` names.
pub fn unlink_if_same(identity: &SocketIdentity) -> io::Result<Unlinked> {
    match fs::symlink_metadata(&identity.path) {
        Ok(meta) if meta.dev() == identity.dev && meta.ino() == identity.ino => {
            fs::remove_file(&identity.path)?;
            Ok(Unlinked::Removed)
        }
        Ok(_) => Ok(Unlinked::Replaced),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Unlinked::AlreadyGone),
        Err(e) => Err(e),
    }
}

fn io_err(op: &'static str, path: &Path, source: io::Error) -> SocketError {
    SocketError::Io { op, path: path.to_path_buf(), source }
}
