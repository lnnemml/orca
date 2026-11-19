use std::fs::{self, Permissions};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixListener;

use super::listener::own_uid;
use super::paths::McpPaths;
use super::socket::*;
use super::test_support::Scratch;

fn mode(path: &std::path::Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

/// Under the ambient umask (022 on this machine) a bound socket comes out `0755` (M0 fact 7), so
/// `0600` here is the explicit `chmod`'s doing. (The umask is not changed in a test: it is
/// process-wide and tests run in parallel.)
#[test]
fn the_directory_and_the_socket_get_their_explicit_modes() {
    let scratch = Scratch::new("modes");
    let paths = scratch.paths();
    prepare_dir(&paths.dir, own_uid()).unwrap();
    let bound = bind(&paths, own_uid(), None).unwrap();
    assert_eq!(mode(&paths.dir), DIR_MODE);
    assert_eq!(mode(&paths.socket), SOCKET_MODE);
    assert_eq!(bound.identity.ino, fs::symlink_metadata(&paths.socket).unwrap().ino());
}

#[test]
fn an_existing_directory_is_tightened_to_0700() {
    let scratch = Scratch::new("tighten");
    let paths = scratch.paths();
    fs::create_dir_all(&paths.dir).unwrap();
    fs::set_permissions(&paths.dir, Permissions::from_mode(0o755)).unwrap();
    prepare_dir(&paths.dir, own_uid()).unwrap();
    assert_eq!(mode(&paths.dir), DIR_MODE);
}

#[test]
fn a_symlinked_directory_is_refused() {
    let scratch = Scratch::new("symlink");
    let paths = scratch.paths();
    let elsewhere = scratch.dir.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &paths.dir).unwrap();
    assert!(matches!(prepare_dir(&paths.dir, own_uid()), Err(SocketError::NotOurDir { .. })));
}

fn chmod_after_bind(target: &'static str, to: u32) -> impl Fn(&McpPaths) + Send + Sync {
    move |p: &McpPaths| {
        let path = if target == "dir" { &p.dir } else { &p.socket };
        fs::set_permissions(path, Permissions::from_mode(to)).unwrap();
    }
}

/// Control (M0 amendment item 9 / M2 amendment item 1): serving after the socket's mode differs
/// from `0600` post-bind. The bind is refused, and the socket it made is removed.
#[test]
fn a_socket_mode_that_differs_after_bind_is_refused() {
    let scratch = Scratch::new("sockmode");
    let paths = scratch.paths();
    prepare_dir(&paths.dir, own_uid()).unwrap();
    let hook = chmod_after_bind("socket", 0o644);
    let refused = bind(&paths, own_uid(), Some(&hook));
    match refused {
        Err(SocketError::Mismatch { found, expected, .. }) => assert_eq!((found, expected), (0o644, SOCKET_MODE)),
        other => panic!("expected a mode refusal, got {other:?}"),
    }
    assert!(!paths.socket.exists(), "the refused socket is removed");
}

/// The same control for the directory's `0700`.
#[test]
fn a_directory_mode_that_differs_after_bind_is_refused() {
    let scratch = Scratch::new("dirmode");
    let paths = scratch.paths();
    prepare_dir(&paths.dir, own_uid()).unwrap();
    let hook = chmod_after_bind("dir", 0o755);
    let refused = bind(&paths, own_uid(), Some(&hook));
    assert!(matches!(refused, Err(SocketError::Mismatch { found: 0o755, expected: DIR_MODE, .. })), "{refused:?}");
}

#[test]
fn a_stale_socket_is_replaced_and_a_non_socket_is_never_removed() {
    let scratch = Scratch::new("stale");
    let paths = scratch.paths();
    prepare_dir(&paths.dir, own_uid()).unwrap();
    drop(UnixListener::bind(&paths.socket).unwrap()); // a stale socket: the listener is gone
    bind(&paths, own_uid(), None).expect("the holder replaces a stale socket");

    fs::remove_file(&paths.socket).unwrap();
    fs::write(&paths.socket, b"not a socket").unwrap();
    assert!(matches!(bind(&paths, own_uid(), None), Err(SocketError::NotASocket { .. })));
    assert_eq!(fs::read(&paths.socket).unwrap(), b"not a socket");
}

/// Control (M2 amendment item 1): an old stop's unlink removing a newer socket. The unlink is keyed
/// on the inode the old start bound; a newer socket at the same path is left in place.
#[test]
fn an_old_identity_never_unlinks_a_newer_socket() {
    let scratch = Scratch::new("inode");
    let paths = scratch.paths();
    prepare_dir(&paths.dir, own_uid()).unwrap();
    let old = bind(&paths, own_uid(), None).unwrap();
    // The newer start: the old file is gone and a new socket has the path. Keep the old file's
    // inode alive (a hard link) so the new socket cannot reuse its number.
    let keep = scratch.dir.join("old-inode");
    fs::hard_link(&paths.socket, &keep).unwrap();
    fs::remove_file(&paths.socket).unwrap();
    let newer = bind(&paths, own_uid(), None).unwrap();
    assert_ne!(old.identity.ino, newer.identity.ino);

    assert_eq!(unlink_if_same(&old.identity).unwrap(), Unlinked::Replaced);
    assert!(paths.socket.exists(), "the newer socket survives the old stop's unlink");
    assert_eq!(unlink_if_same(&newer.identity).unwrap(), Unlinked::Removed);
    assert!(!paths.socket.exists());
    assert_eq!(unlink_if_same(&newer.identity).unwrap(), Unlinked::AlreadyGone);
}
