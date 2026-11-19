use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use super::paths::*;
use super::test_support::Scratch;

#[test]
fn the_runtime_dir_gives_orcastudio_under_it() {
    let p = resolve(Some(Path::new("/run/user/1000")), Some(Path::new("/home/a/.local/share/orcastudio"))).unwrap();
    assert_eq!(p.dir, PathBuf::from("/run/user/1000/orcastudio"));
    assert_eq!(p.lock, PathBuf::from("/run/user/1000/orcastudio/mcp.lock"));
    assert_eq!(p.socket, PathBuf::from("/run/user/1000/orcastudio/mcp.sock"));
    assert_eq!(p.base, PathBase::RuntimeDir);
}

#[test]
fn an_unset_or_relative_runtime_dir_falls_back_to_the_data_dir() {
    let data = Path::new("/home/a/.local/share/orcastudio");
    for runtime in [None, Some(Path::new("run/user/1000")), Some(Path::new(""))] {
        let p = resolve(runtime, Some(data)).unwrap();
        assert_eq!(p.dir, data.join("mcp"), "{runtime:?}");
        assert_eq!(p.socket, data.join("mcp/mcp.sock"));
        assert_eq!(p.base, PathBase::DataDir);
    }
    assert_eq!(resolve(None, None), Err(PathRefusal::NoBaseDir));
}

#[test]
fn sun_path_max_is_the_measured_107() {
    // M0 measured 107 binds and 108 fails (mcp-transport-probe.md, fact 7).
    assert_eq!(sun_path_max(), 107);
}

/// A runtime dir under `scratch` whose socket path is exactly `len` bytes.
fn runtime_dir_for_socket_len(scratch: &Scratch, len: usize) -> PathBuf {
    let fixed = scratch.dir.as_os_str().as_bytes().len() + "/".len() + "/orcastudio/mcp.sock".len();
    let pad = "p".repeat(len - fixed);
    scratch.dir.join(pad)
}

/// Control (M2 amendment item 1): an over-long `sun_path` bound instead of refused. The limit is
/// re-measured: the longest accepted path really binds, and one byte more really cannot.
#[test]
fn a_socket_path_over_sun_path_is_refused_and_the_longest_accepted_one_binds() {
    let scratch = Scratch::new("sunpath");
    let max = sun_path_max();

    let fits = resolve(Some(&runtime_dir_for_socket_len(&scratch, max)), None).unwrap();
    assert_eq!(fits.socket.as_os_str().as_bytes().len(), max);
    std::fs::create_dir_all(&fits.dir).unwrap();
    UnixListener::bind(&fits.socket).expect("a 107-byte socket path binds");

    let runtime = runtime_dir_for_socket_len(&scratch, max + 1);
    match resolve(Some(&runtime), None) {
        Err(PathRefusal::SocketPathTooLong { len, max: m, .. }) => assert_eq!((len, m), (max + 1, max)),
        other => panic!("a {}-byte socket path must be refused, got {other:?}", max + 1),
    }
    let too_long = runtime.join("orcastudio/mcp.sock");
    std::fs::create_dir_all(too_long.parent().unwrap()).unwrap();
    assert!(UnixListener::bind(&too_long).is_err(), "the kernel refuses 108 bytes too");
}

#[test]
fn the_refusal_says_why() {
    let long = PathBuf::from(format!("/{}", "x".repeat(200)));
    let refusal = resolve(Some(&long), None).unwrap_err();
    assert!(refusal.to_string().contains("sun_path"), "{refusal}");
}
