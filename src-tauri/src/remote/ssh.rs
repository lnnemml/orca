//! The ssh transport (ADR-005, ADR-024 n item 11 and l): the one argv shape that runs a static
//! script on a profile's host, and a process runner with a hard timeout.
//!
//! **The argv** is `ssh -o BatchMode=yes -o ConnectTimeout=10 -- <host> bash -s` ([`ssh_bash_argv`]).
//! - The host is one argv element **after `--`**, so ssh cannot read it as an option. Measured on
//!   OpenSSH 9.6p1: `ssh -G '-oProxyCommand=echo PWNED' somehost` resolves `proxycommand echo PWNED`;
//!   with `--` before it, ssh refuses the same value with `hostname contains invalid characters`.
//! - ssh joins the words after the host into the remote shell's command line, so those words are
//!   the static `bash -s` and nothing else. Every profile value travels on **stdin**, after the
//!   script, as a NUL list (`connection_test::conntest_stdin`). No profile value is ever shell text.
//! - [`ssh_bash_argv`] validates the host again ([`validate_host`]) even though save already did,
//!   so a row written before the rule existed still cannot reach ssh as an option.
//! - `BatchMode=yes`: never prompt for a password or a host key; fail instead. Auth stays with the
//!   user's ssh setup (ADR-005), and the app never answers a prompt.
//!
//! **The runner** ([`CommandRunner`]) is a trait so the command's decisions (stamp, clear) are tested
//! with a fake; [`SystemRunner`] is the real one. It writes stdin from a thread, reads stdout and
//! stderr from two more (so no pipe can fill and deadlock), and caps each stream at
//! [`MAX_OUTPUT_BYTES`]: a stream past the cap kills the child's process group at once and is
//! [`TransportError::OutputTooLarge`]. On timeout it kills the whole process group too, so nothing
//! the child started survives. The timeout also covers a grandchild that keeps a pipe open after
//! the child exits.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::models::server_profile::{validate_host, InvalidProfile};

/// The ssh client, found on `PATH` like the user's own shell finds it (ADR-005: system `ssh`).
pub const SSH_PROGRAM: &str = "ssh";

/// ssh's own limit on establishing the TCP connection (`-o ConnectTimeout`). The overall run is
/// bounded separately by the caller's timeout.
pub const CONNECT_TIMEOUT_SECS: u32 = 10;

/// The most bytes read from either stream. The connection test prints a few KB (ORCA's
/// `--version` banner is 183 lines); anything near this cap is not a connection-test output.
pub const MAX_OUTPUT_BYTES: usize = 1 << 20;

/// The arguments after `ssh` that run `bash -s` on `host`, reading the script from stdin. The host
/// is validated and placed right after `--`.
pub fn ssh_bash_argv(host: &str) -> Result<Vec<String>, InvalidProfile> {
    validate_host(host)?;
    let mut argv = ssh_options();
    argv.extend(["--".into(), host.into(), "bash".into(), "-s".into()]);
    Ok(argv)
}

/// The ssh options every OrcaStudio ssh carries: never prompt (`BatchMode=yes`), and a bounded
/// connect. One list, so the script calls ([`ssh_bash_argv`]) and rsync's `-e` transport
/// (`remote::sync`) cannot drift apart (ADR-024 o item 6).
pub fn ssh_options() -> Vec<String> {
    vec![
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        format!("ConnectTimeout={CONNECT_TIMEOUT_SECS}"),
    ]
}

/// How a process ended, and what it printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    /// The exit status; `None` if a signal ended the process.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// The process could not be run to completion, so there is no output to judge.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("could not start {program}: {message}")]
    Spawn { program: String, message: String },
    #[error("{program} did not finish within {secs} s and was killed")]
    Timeout { program: String, secs: u64 },
    #[error("{program} wrote more than {MAX_OUTPUT_BYTES} bytes to {stream}; it was killed")]
    OutputTooLarge { program: String, stream: &'static str },
    #[error("i/o with {program}: {message}")]
    Io { program: String, message: String },
}

/// Runs a program with the given stdin and a hard timeout.
pub trait CommandRunner {
    fn run(
        &self,
        program: &str,
        args: &[String],
        stdin: &[u8],
        timeout: Duration,
    ) -> Result<ProcessOutput, TransportError>;
}

/// The real runner: spawns the process.
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(
        &self,
        program: &str,
        args: &[String],
        stdin: &[u8],
        timeout: Duration,
    ) -> Result<ProcessOutput, TransportError> {
        run_with_timeout(program, args, stdin, timeout)
    }
}

/// One stream's bytes, or the stream exceeded [`MAX_OUTPUT_BYTES`], or reading it failed.
enum Stream {
    Bytes(Vec<u8>),
    TooLarge,
    Failed(String),
}

/// Read one stream up to [`MAX_OUTPUT_BYTES`] + 1 bytes on its own thread and report it, tagged
/// with its name, on `tx`. The runner reacts to a `TooLarge` as soon as it arrives.
fn read_capped(
    mut source: impl Read + Send + 'static,
    name: &'static str,
    tx: mpsc::Sender<(&'static str, Stream)>,
) {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let result = match (&mut source).take(MAX_OUTPUT_BYTES as u64 + 1).read_to_end(&mut buf) {
            Ok(_) if buf.len() > MAX_OUTPUT_BYTES => Stream::TooLarge,
            Ok(_) => Stream::Bytes(buf),
            Err(e) => Stream::Failed(e.to_string()),
        };
        // The receiver is gone only if the runner already gave up; nothing to report.
        let _ = tx.send((name, result));
    });
}

/// Kill the child's process group (it was started as its own group), then the child, and reap it.
fn kill(child: &mut Child) {
    #[cfg(unix)]
    {
        // SAFETY: killpg is a thin syscall wrapper taking a pgid and a signal. The child was
        // spawned with `process_group(0)`, so its pid is its own group's id.
        unsafe {
            libc::killpg(child.id() as i32, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn run_with_timeout(
    program: &str,
    args: &[String],
    stdin: &[u8],
    timeout: Duration,
) -> Result<ProcessOutput, TransportError> {
    let deadline = Instant::now() + timeout;
    let io = |message: String| TransportError::Io { program: program.into(), message };
    let timed_out = || TransportError::Timeout { program: program.into(), secs: timeout.as_secs() };

    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // its own group, so a timeout kills ssh and anything it started
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| TransportError::Spawn { program: program.into(), message: e.to_string() })?;

    let (Some(mut child_stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        kill(&mut child);
        return Err(io("the child's pipes were not created".into()));
    };
    let input = stdin.to_vec();
    // A write error (the remote end exited early) is not reported here: the connection test's
    // post-condition (the echoed values) catches a stdin that did not arrive whole.
    std::thread::spawn(move || {
        let _ = child_stdin.write_all(&input);
    });
    let (tx, events) = mpsc::channel();
    read_capped(stdout, "stdout", tx.clone());
    read_capped(stderr, "stderr", tx);

    // Wait for three things: the child's exit and both streams. A stream past the cap or a read
    // error ends the run at once (the child is killed, not left blocked on a full pipe until the
    // deadline). The deadline bounds everything, including a process the child started that keeps
    // a pipe open after the child exits.
    let mut exit: Option<Option<i32>> = None;
    let mut out: Option<Vec<u8>> = None;
    let mut err: Option<Vec<u8>> = None;
    loop {
        match events.recv_timeout(Duration::from_millis(20)) {
            Ok((name, Stream::Bytes(bytes))) => {
                if name == "stdout" {
                    out = Some(bytes);
                } else {
                    err = Some(bytes);
                }
            }
            Ok((name, Stream::TooLarge)) => {
                kill(&mut child);
                return Err(TransportError::OutputTooLarge { program: program.into(), stream: name });
            }
            Ok((name, Stream::Failed(message))) => {
                kill(&mut child);
                return Err(io(format!("reading {name}: {message}")));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // Both readers have reported; only the exit is left to wait for.
            Err(mpsc::RecvTimeoutError::Disconnected) => std::thread::sleep(Duration::from_millis(20)),
        }
        if exit.is_none() {
            match child.try_wait() {
                Ok(Some(status)) => exit = Some(status.code()),
                Ok(None) => {}
                Err(e) => {
                    kill(&mut child);
                    return Err(io(e.to_string()));
                }
            }
        }
        if let (Some(code), Some(_), Some(_)) = (exit, &out, &err) {
            let (stdout, stderr) = (out.unwrap_or_default(), err.unwrap_or_default());
            return Ok(ProcessOutput { code, stdout, stderr });
        }
        if Instant::now() >= deadline {
            kill(&mut child);
            return Err(timed_out());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // NEGATIVE CONTROL: drop the `"--"` element (or move it after the host) and this goes red. The
    // `--` is what stops ssh reading a host such as `-oProxyCommand=…` as an option.
    #[test]
    fn the_argv_is_the_fixed_shape_with_double_dash_right_before_the_host() {
        let argv = ssh_bash_argv("uni").unwrap();
        assert_eq!(
            argv,
            ["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "--", "uni", "bash", "-s"]
        );
        let host_at = argv.iter().position(|a| a == "uni").unwrap();
        assert_eq!(argv[host_at - 1], "--", "the host must follow `--` directly");
        assert_eq!(&argv[host_at + 1..], ["bash", "-s"], "nothing but `bash -s` after the host");
    }

    #[test]
    fn the_argv_refuses_a_host_that_could_be_an_option_even_if_it_was_stored() {
        for bad in ["-oProxyCommand=sh", "uni host", "uni;id", ""] {
            assert!(ssh_bash_argv(bad).is_err(), "{bad:?}");
        }
    }

    fn sh(script: &str) -> Vec<String> {
        vec!["-c".into(), script.into()]
    }

    #[test]
    fn stdin_reaches_the_process_and_both_streams_and_the_code_come_back() {
        let input: Vec<u8> = (0..=255u8).cycle().take(200_000).collect(); // beyond a pipe buffer
        let out = SystemRunner
            .run("sh", &sh("cat; echo oops >&2; exit 7"), &input, Duration::from_secs(10))
            .unwrap();
        assert_eq!(out.stdout, input);
        assert_eq!(out.stderr, b"oops\n");
        assert_eq!(out.code, Some(7));
    }

    #[test]
    fn a_process_that_outlives_the_timeout_is_killed() {
        let start = Instant::now();
        let err = SystemRunner
            .run("sh", &sh("exec sleep 30"), b"", Duration::from_millis(300))
            .unwrap_err();
        assert!(matches!(err, TransportError::Timeout { .. }), "{err:?}");
        assert!(start.elapsed() < Duration::from_secs(5), "killed promptly: {:?}", start.elapsed());
    }

    /// A process the test itself started (a `sleep 30`), identified by the pid it wrote to a
    /// pidfile. Dropping the guard kills it if it is still that `sleep`, so a failing test leaves
    /// no stray behind; it never signals a pid whose cmdline is not `sleep 30`.
    struct OwnSleep(i32);

    impl OwnSleep {
        fn from_pidfile(path: &std::path::Path) -> OwnSleep {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(pid) = std::fs::read_to_string(path).ok().and_then(|t| t.trim().parse().ok()) {
                    return OwnSleep(pid);
                }
                assert!(Instant::now() < deadline, "the grandchild never wrote its pid");
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        /// Alive = `/proc/<pid>` exists, is not a zombie, and is still our `sleep 30`.
        fn alive(&self) -> bool {
            let cmdline = std::fs::read(format!("/proc/{}/cmdline", self.0)).unwrap_or_default();
            let stat = std::fs::read_to_string(format!("/proc/{}/stat", self.0)).unwrap_or_default();
            let state = stat.rsplit_once(") ").and_then(|(_, rest)| rest.chars().next());
            cmdline == b"sleep\x0030\x00" && !matches!(state, None | Some('Z') | Some('X'))
        }

        /// Wait up to 2 s for the process to be gone.
        fn gone_soon(&self) -> bool {
            let deadline = Instant::now() + Duration::from_secs(2);
            while self.alive() {
                if Instant::now() >= deadline {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            true
        }
    }

    impl Drop for OwnSleep {
        fn drop(&mut self) {
            if self.alive() {
                // SAFETY: kill(2) with a pid we just verified is our own `sleep 30`.
                unsafe {
                    libc::kill(self.0, libc::SIGKILL);
                }
            }
        }
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("orcastudio-ssh-test-{}-{name}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // A timeout kills the child's whole process group, not only the child: a `sleep` it started
    // must be gone afterwards, both while the child still runs (`wait`) and after the child has
    // exited while the grandchild keeps stdout open. NEGATIVE CONTROL: remove `libc::killpg` from
    // `kill()` and both cases go red (the `sleep 30` survives).
    #[test]
    fn a_timeout_kills_the_grandchildren_too() {
        for (case, tail) in [("child still running", "; wait"), ("child exited", "")] {
            let dir = scratch(if tail.is_empty() { "exited" } else { "running" });
            let pidfile = dir.join("pid");
            let script = format!("sleep 30 & echo $! > '{}'{tail}", pidfile.display());
            let start = Instant::now();
            let err = SystemRunner.run("sh", &sh(&script), b"", Duration::from_millis(500)).unwrap_err();
            let grandchild = OwnSleep::from_pidfile(&pidfile);
            assert!(matches!(err, TransportError::Timeout { .. }), "{case}: {err:?}");
            assert!(start.elapsed() < Duration::from_secs(5), "{case}: {:?}", start.elapsed());
            assert!(grandchild.gone_soon(), "{case}: the grandchild {} survived the timeout", grandchild.0);
            std::fs::remove_dir_all(&dir).ok();
        }
    }

    // A child that writes without end (it ignores SIGPIPE, so closing the pipe does not stop it)
    // must be refused as soon as the cap is hit, not left to the deadline. NEGATIVE CONTROL: treat
    // `TooLarge` like ordinary bytes in the wait loop and this times out after 20 s → red.
    #[test]
    fn an_unbounded_stream_is_refused_promptly() {
        let start = Instant::now();
        let err = SystemRunner
            .run(
                "sh",
                &sh("trap '' PIPE; while :; do head -c 65536 /dev/zero 2>/dev/null; done"),
                b"",
                Duration::from_secs(20),
            )
            .unwrap_err();
        assert!(matches!(err, TransportError::OutputTooLarge { stream: "stdout", .. }), "{err:?}");
        assert!(start.elapsed() < Duration::from_secs(5), "refused promptly: {:?}", start.elapsed());
    }

    #[test]
    fn output_beyond_the_cap_is_refused() {
        let script = format!("head -c {} /dev/zero", MAX_OUTPUT_BYTES + 1);
        let err = SystemRunner.run("sh", &sh(&script), b"", Duration::from_secs(10)).unwrap_err();
        assert!(matches!(err, TransportError::OutputTooLarge { stream: "stdout", .. }), "{err:?}");
    }

    #[test]
    fn a_missing_program_is_a_spawn_error() {
        let err = SystemRunner
            .run("/nonexistent/orcastudio-ssh", &[], b"", Duration::from_secs(1))
            .unwrap_err();
        assert!(matches!(err, TransportError::Spawn { .. }), "{err:?}");
    }
}
