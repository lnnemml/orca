use std::fs::File;
use std::os::fd::AsRawFd;
use std::process::Command;

use super::lock::*;
use super::test_support::Scratch;

fn clear_cloexec(file: &File) {
    // SAFETY: a valid fd; F_SETFD with 0 clears FD_CLOEXEC.
    assert_eq!(unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, 0) }, 0);
}

/// Rule #10, settled by a run: std's `File::open` sets `O_CLOEXEC` on its own (the source of std is
/// not installed on this machine; this run is the record). The lock's explicit flag and its
/// assertion stay either way.
#[test]
fn std_file_open_already_sets_cloexec() {
    let scratch = Scratch::new("std-cloexec");
    let path = scratch.dir.join("plain");
    std::fs::write(&path, b"").unwrap();
    let plain = File::open(&path).unwrap();
    assert!(has_cloexec(&plain).unwrap());
}

#[test]
fn the_lock_is_exclusive_and_released_on_drop() {
    let scratch = Scratch::new("lock-excl");
    let path = scratch.dir.join("mcp.lock");
    let held = match try_acquire(&path).unwrap() {
        Acquire::Acquired(l) => l,
        Acquire::HeldElsewhere => panic!("a fresh lock must be free"),
    };
    assert!(matches!(try_acquire(&path).unwrap(), Acquire::HeldElsewhere));
    drop(held);
    assert!(matches!(try_acquire(&path).unwrap(), Acquire::Acquired(_)));
}

#[test]
fn the_held_lock_fd_carries_cloexec() {
    let scratch = Scratch::new("lock-cloexec");
    let path = scratch.dir.join("mcp.lock");
    let Acquire::Acquired(_held) = try_acquire(&path).unwrap() else { panic!("free") };
    // Re-read through /proc: the fd that holds the lock is the one we check.
    let mut found = false;
    for entry in std::fs::read_dir("/proc/self/fd").unwrap().flatten() {
        if std::fs::read_link(entry.path()).ok().as_deref() == Some(path.as_path()) {
            let fd: i32 = entry.file_name().to_str().unwrap().parse().unwrap();
            // SAFETY: a valid fd of this process.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            assert!(flags >= 0 && flags & libc::FD_CLOEXEC != 0, "fd {fd} lacks FD_CLOEXEC");
            found = true;
        }
    }
    assert!(found, "the lock's fd is open");
}

/// Control (M0 amendment item 9 / M2 amendment item 1): the lock fd without `FD_CLOEXEC`. A fd
/// stripped of the flag after the open is refused, and no lock is held.
#[test]
fn a_lock_fd_without_cloexec_is_refused() {
    let scratch = Scratch::new("lock-nocloexec");
    let path = scratch.dir.join("mcp.lock");
    let refused = try_acquire_with(&path, clear_cloexec);
    assert!(matches!(refused, Err(LockError::NoCloexec)), "{refused:?}");
    assert!(matches!(try_acquire(&path).unwrap(), Acquire::Acquired(_)), "the refused attempt holds nothing");
}

/// The measured hazard itself (M0 fact 7): a child process does not inherit the lock's fd.
#[test]
fn a_child_process_does_not_inherit_the_lock_fd() {
    let scratch = Scratch::new("lock-child");
    let path = scratch.dir.join("mcp.lock");
    let Acquire::Acquired(_held) = try_acquire(&path).unwrap() else { panic!("free") };
    let out = Command::new("ls").arg("-l").arg("/proc/self/fd/").output().unwrap();
    let listing = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(!listing.contains(path.to_str().unwrap()), "the child sees the lock fd:\n{listing}");
}
