//! `orcastudio --mcp`, the built binary run as a real process (ADR-027 M2 amendment, items 1 and 3;
//! M2a Part B). Each run gets its own `XDG_RUNTIME_DIR`, `HOME` and `XDG_DATA_HOME` under a scratch
//! directory, and no `DISPLAY`/`WAYLAND_DISPLAY`: a binary that wrongly started the GUI fails fast
//! instead of opening a window, and could not touch the real data dir.
//!
//! Required: stdout carries the socket's bytes and nothing else; with nothing listening, exactly the
//! one stderr line and a non-zero exit.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// The relay's one stderr line when the socket cannot be reached (`mcp::server::relay::NOT_RUNNING`).
const NOT_RUNNING: &str = "OrcaStudio is not running, or its MCP server is off";
/// A run that takes longer than this has hung (or started the GUI).
const DEADLINE: Duration = Duration::from_secs(30);

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("osbin-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

struct Run {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
}

/// Run `orcastudio --mcp` with `runtime_dir` as `XDG_RUNTIME_DIR`, feed it `input`, and keep its
/// stdin open (as a client session does) unless `close_stdin`.
fn run_relay(home: &Path, runtime_dir: &Path, input: &[u8], close_stdin: bool) -> Run {
    let mut child = Command::new(env!("CARGO_BIN_EXE_orcastudio"))
        .arg("--mcp")
        .env("XDG_RUNTIME_DIR", runtime_dir)
        .env("HOME", home.join("home"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the orcastudio binary runs");
    let mut stdin = child.stdin.take().unwrap();
    let _ = stdin.write_all(input);
    let _ = stdin.flush();
    let stdin = if close_stdin {
        drop(stdin);
        None
    } else {
        Some(stdin)
    };
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let out_reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        buf
    });
    let err_reader = thread::spawn(move || {
        let mut buf = String::new();
        let _ = err.read_to_string(&mut buf);
        buf
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if started.elapsed() > DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(20));
    };
    drop(stdin);
    let run = Run { code: status.and_then(|s| s.code()), stdout: out_reader.join().unwrap(), stderr: err_reader.join().unwrap() };
    assert!(status.is_some(), "orcastudio --mcp did not exit within {DEADLINE:?}; stderr: {}", run.stderr);
    run
}

/// Control (Part B): the built `orcastudio --mcp` with no app running. It must exit non-zero (2),
/// write exactly the one stderr line, and write nothing to stdout. Bites on exit 0, on a missing
/// stderr line, on any stdout byte, and on the branch placed after `run()` (the GUI would start).
#[test]
fn with_no_app_running_it_exits_2_with_the_one_stderr_line_and_an_empty_stdout() {
    let scratch = Scratch::new("none");
    let run = run_relay(&scratch.0, &scratch.0, b"", false);
    assert_eq!(run.stdout, b"", "stdout carried something: {:?}", String::from_utf8_lossy(&run.stdout));
    assert_eq!(run.stderr, format!("{NOT_RUNNING}\n"), "stderr must be exactly the one line");
    assert_eq!(run.code, Some(2), "the exit code with nothing listening; stderr: {}", run.stderr);
}

/// stdout carries exactly the socket's bytes: the relay forwards the client's line to the socket and
/// the server's reply to stdout, and ends with code 1 when the server closes.
#[test]
fn it_relays_the_sockets_bytes_and_only_those_to_stdout() {
    let scratch = Scratch::new("relay");
    let dir = scratch.0.join("orcastudio");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let listener = UnixListener::bind(dir.join("mcp.sock")).unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        (&stream).write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n").unwrap();
        line // the stream drops here: the server closes
    });

    let run = run_relay(&scratch.0, &scratch.0, b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n", false);
    assert_eq!(server.join().unwrap(), "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n");
    assert_eq!(String::from_utf8(run.stdout).unwrap(), "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n");
    assert_eq!(run.stderr, "", "a diagnostic on a clean relay");
    assert_eq!(run.code, Some(1), "the server closed first");
}

/// A socket path over `sun_path` is refused: one stderr line saying so, non-zero, empty stdout.
#[test]
fn a_refused_socket_path_is_one_stderr_line_and_a_non_zero_exit() {
    let scratch = Scratch::new("long");
    let long = scratch.0.join("r".repeat(120));
    let run = run_relay(&scratch.0, &long, b"", true);
    assert_eq!(run.stdout, b"");
    assert_eq!(run.stderr.lines().count(), 1, "{}", run.stderr);
    assert!(run.stderr.starts_with("orcastudio --mcp: ") && run.stderr.contains("sun_path"), "{}", run.stderr);
    assert_ne!(run.code, Some(0));
    assert!(run.code.is_some());
}
