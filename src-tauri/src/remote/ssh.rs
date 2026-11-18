//! The ssh transport (ADR-005, ADR-024 n item 11 and l): the one argv shape that runs a static
//! script on a profile's host, and a process runner with a hard timeout.
//!
//! **The argv** is `ssh -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=30 -o
//! ServerAliveCountMax=4 -- <host> bash -s` ([`ssh_bash_argv`]; the keepalive is ADR-024 o24.5).
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
//! the child exits. Every [`SystemRunner`] call first takes a permit of the process-global
//! per-host cap (`gate::REMOTE_CALLS`, at most 8 ssh/rsync per host, ADR-024 o24.2–o24.4); the
//! wait for it is outside the timeout.

use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::gate::{host_key, Gate, REMOTE_CALLS};
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

/// The app's own keepalive (ADR-024 o24.5): a probe every 30 s, and the connection is given up
/// after 4 unanswered ones, so a silently dropped link ends an ssh — and the rsync over it — in
/// ~2 min whatever `~/.ssh/config` says (a command-line `-o` overrides the config file, measured).
/// The values are the author's config's. A call multiplexed over an existing control master uses
/// the master's keepalive instead (residual, o24.5).
pub const SERVER_ALIVE_INTERVAL_SECS: u32 = 30;
pub const SERVER_ALIVE_COUNT_MAX: u32 = 4;

/// The ssh options every OrcaStudio ssh carries: never prompt (`BatchMode=yes`), a bounded
/// connect, and the keepalive. One list, so the script calls ([`ssh_bash_argv`]) and rsync's `-e`
/// transport (`remote::sync`) cannot drift apart (ADR-024 o item 6).
pub fn ssh_options() -> Vec<String> {
    vec![
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        format!("ConnectTimeout={CONNECT_TIMEOUT_SECS}"),
        "-o".into(),
        format!("ServerAliveInterval={SERVER_ALIVE_INTERVAL_SECS}"),
        "-o".into(),
        format!("ServerAliveCountMax={SERVER_ALIVE_COUNT_MAX}"),
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

/// The real runner: spawns the process, under the process-global per-host cap
/// ([`REMOTE_CALLS`], ADR-024 o24.3). The wait for a permit is outside the timeout ([`run_gated`]).
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(
        &self,
        program: &str,
        args: &[String],
        stdin: &[u8],
        timeout: Duration,
    ) -> Result<ProcessOutput, TransportError> {
        run_gated(&REMOTE_CALLS, program, args, stdin, timeout)
    }
}

/// Run one ssh/rsync process under `gate` (ADR-024 o24.4): take the permit of the host the argv
/// connects to ([`host_key`]) **first**, then run with the full `timeout` — the deadline is
/// computed only once the permit is held, so a call that waited in the queue still gets all of its
/// time and never reports a timeout the server did not cause. The permit is a guard on this stack
/// frame: it is given back after the child is reaped on every path (exit, timeout or too-large
/// kill, spawn error, a panic unwinding).
pub fn run_gated(
    gate: &Gate,
    program: &str,
    args: &[String],
    stdin: &[u8],
    timeout: Duration,
) -> Result<ProcessOutput, TransportError> {
    let _permit = gate.acquire(host_key(program, args));
    run_with_timeout(program, args, stdin, timeout)
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
    use crate::remote::gate::{HostKey, MAX_CONCURRENT_REMOTE_CALLS};

    // NEGATIVE CONTROL: drop the `"--"` element (or move it after the host) and this goes red. The
    // `--` is what stops ssh reading a host such as `-oProxyCommand=…` as an option.
    #[test]
    fn the_argv_is_the_fixed_shape_with_double_dash_right_before_the_host() {
        let argv = ssh_bash_argv("uni").unwrap();
        assert_eq!(
            argv,
            [
                "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-o", "ServerAliveInterval=30", "-o",
                "ServerAliveCountMax=4", "--", "uni", "bash", "-s",
            ]
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

    /// The runner without the global gate, for the timing tests here (`elapsed < 5 s` from before
    /// `run`): under cargo's parallelism the many `SystemRunner` calls of other tests share the
    /// global gate's fallback key and could hold them in its queue (o24.10).
    struct Ungated;

    impl CommandRunner for Ungated {
        fn run(&self, program: &str, args: &[String], stdin: &[u8], timeout: Duration) -> Result<ProcessOutput, TransportError> {
            run_with_timeout(program, args, stdin, timeout)
        }
    }

    // o24.5: the keepalive is in the shared options, so both the script calls' argv and rsync's
    // `-e` transport carry it. NEGATIVE CONTROL: drop either option from `ssh_options` → red.
    #[test]
    fn every_ssh_and_every_rsync_transport_carries_the_keepalive() {
        use crate::execution_backend::FetchPolicy;
        use crate::remote::sync::{download_argv, upload_argv};
        let pairs = [["-o", "ServerAliveInterval=30"], ["-o", "ServerAliveCountMax=4"]];
        let argv = ssh_bash_argv("uni").unwrap();
        for pair in pairs {
            assert!(argv.windows(2).any(|w| w == pair), "ssh argv lacks {pair:?}: {argv:?}");
        }
        for rsync in [
            upload_argv("/l/job", "uni", "/r/jobs/j").unwrap(),
            download_argv("uni", "/r/jobs/j", "/l/job", FetchPolicy::SMALL_ONLY).unwrap(),
        ] {
            let transport = &rsync[rsync.iter().position(|a| a == "-e").unwrap() + 1];
            for pair in pairs {
                assert!(transport.contains(&pair.join(" ")), "rsync -e lacks {pair:?}: {transport}");
            }
        }
    }

    // o24.10 wiring: `SystemRunner::run` takes the GLOBAL gate's permit of the argv's host, with
    // the literal cap 8. Nine real rsync calls to one unique host (each runs a fake `-e` transport
    // that sleeps 1.5 s) are observed in the global gate as 8 holders and 1 waiter. NEGATIVE
    // CONTROLS: `SystemRunner::run` calling `run_with_timeout` directly (bypass) → no holders, red;
    // a wrong host key (the local rsync end) → no holders under this host, red.
    #[test]
    fn system_runner_goes_through_the_global_gate_of_eight_per_host() {
        assert_eq!(MAX_CONCURRENT_REMOTE_CALLS, 8);
        let dir = scratch("wiring");
        let stub = dir.join("fake-ssh");
        std::fs::write(&stub, "#!/bin/sh\nsleep 1.5\nexit 1\n").unwrap();
        std::fs::set_permissions(&stub, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let host = format!("wiring-test-{}", std::process::id());
        let key = HostKey::Host(host.clone());
        let argv: Vec<String> = vec![
            "-a".into(),
            "-e".into(),
            stub.to_str().unwrap().into(),
            format!("{host}:/nonexistent/"),
            format!("{}/", dir.join("dst").display()),
        ];
        let (tx, done) = mpsc::channel();
        for _ in 0..9 {
            let (argv, tx) = (argv.clone(), tx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(SystemRunner.run("rsync", &argv, b"", Duration::from_secs(30)));
            });
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while (REMOTE_CALLS.holders(&key), REMOTE_CALLS.waiting(&key)) != (8, 1) {
            assert!(
                Instant::now() < deadline,
                "the global gate never held 8 with 1 waiting for {host}: {} holders, {} waiting",
                REMOTE_CALLS.holders(&key),
                REMOTE_CALLS.waiting(&key)
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        for i in 0..9 {
            let result = done.recv_timeout(Duration::from_secs(20)).unwrap_or_else(|_| panic!("call {i} never ended"));
            let out = result.expect("rsync ran");
            assert_ne!(out.code, Some(0), "the fake transport fails the transfer");
        }
        assert_eq!(REMOTE_CALLS.holders(&key), 0, "every permit released");
        std::fs::remove_dir_all(&dir).ok();
    }

    // o24.4 (MED-1 of the Part A review): `run_gated` holds its permit for the child's whole life.
    // On a one-permit gate the second call's child starts only after the first call's child has
    // ended. NEGATIVE CONTROL: `let _ = gate.acquire(…)` (the permit dropped at once) → the two
    // children overlap → red.
    #[test]
    fn run_gated_holds_the_permit_for_the_childs_whole_life() {
        let dir = scratch("hold");
        let log = dir.join("log");
        let gate = std::sync::Arc::new(Gate::new(1));
        let script = |n: &str| format!("echo start-{n} >> '{p}'; sleep 0.5; echo end-{n} >> '{p}'", p = log.display());
        let first = gated_on_thread(&gate, &script("1"), Duration::from_secs(10));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !std::fs::read_to_string(&log).unwrap_or_default().contains("start-1") {
            assert!(Instant::now() < deadline, "the first child never started");
            std::thread::sleep(Duration::from_millis(5));
        }
        let second = gated_on_thread(&gate, &script("2"), Duration::from_secs(10));
        first.recv_timeout(Duration::from_secs(10)).expect("first ended").expect("first ran");
        second.recv_timeout(Duration::from_secs(10)).expect("second ended").expect("second ran");
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "start-1\nend-1\nstart-2\nend-2\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stdin_reaches_the_process_and_both_streams_and_the_code_come_back() {
        let input: Vec<u8> = (0..=255u8).cycle().take(200_000).collect(); // beyond a pipe buffer
        let out = Ungated
            .run("sh", &sh("cat; echo oops >&2; exit 7"), &input, Duration::from_secs(10))
            .unwrap();
        assert_eq!(out.stdout, input);
        assert_eq!(out.stderr, b"oops\n");
        assert_eq!(out.code, Some(7));
    }

    #[test]
    fn a_process_that_outlives_the_timeout_is_killed() {
        let start = Instant::now();
        let err = Ungated
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
            let err = Ungated.run("sh", &sh(&script), b"", Duration::from_millis(500)).unwrap_err();
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
        let err = Ungated
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
        let err = Ungated.run("sh", &sh(&script), b"", Duration::from_secs(10)).unwrap_err();
        assert!(matches!(err, TransportError::OutputTooLarge { stream: "stdout", .. }), "{err:?}");
    }

    // ---- run_gated (ADR-024 o24.4, o24.10), each on its own local gate ----------------------

    /// Run `run_gated` on a thread and hand its result back, so a test can bound the wait.
    fn gated_on_thread(
        gate: &std::sync::Arc<Gate>,
        script: &str,
        timeout: Duration,
    ) -> mpsc::Receiver<Result<ProcessOutput, TransportError>> {
        let (tx, rx) = mpsc::channel();
        let (gate, args) = (gate.clone(), sh(script));
        std::thread::spawn(move || {
            let _ = tx.send(run_gated(&gate, "sh", &args, b"", timeout));
        });
        rx
    }

    fn wait_until_queued(gate: &Gate, key: &HostKey) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while gate.waiting(key) != 1 {
            assert!(Instant::now() < deadline, "the call never queued");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    // o24.4: the wait for a permit is outside the call's timeout. A 300 ms call queued for ~1 s
    // still runs to completion. NEGATIVE CONTROL: compute the deadline before taking the permit
    // (`run_with_timeout(…, timeout.saturating_sub(waited))`) → it times out → red.
    #[test]
    fn a_call_queued_longer_than_its_timeout_still_gets_its_full_timeout() {
        let gate = std::sync::Arc::new(Gate::new(1));
        let held = gate.acquire(HostKey::Fallback); // `sh` has no host: the fallback key
        let result = gated_on_thread(&gate, "echo ok", Duration::from_millis(300));
        wait_until_queued(&gate, &HostKey::Fallback);
        std::thread::sleep(Duration::from_millis(1000));
        drop(held);
        let out = result.recv_timeout(Duration::from_secs(5)).expect("the call ended").expect("ran to completion");
        assert_eq!((out.code, out.stdout.as_slice()), (Some(0), &b"ok\n"[..]));
    }

    // o24.4: a spawn error and a timeout kill both give the permit back. NEGATIVE CONTROL: keep the
    // permit on an error (`std::mem::forget` it when the result is `Err`) → the next call on the
    // one-permit gate never starts → red within the bound.
    #[test]
    fn a_spawn_error_and_a_timeout_kill_release_the_permit() {
        let gate = std::sync::Arc::new(Gate::new(1));
        let err = run_gated(&gate, "/nonexistent/orcastudio-ssh", &[], b"", Duration::from_secs(1)).unwrap_err();
        assert!(matches!(err, TransportError::Spawn { .. }), "{err:?}");
        let next = gated_on_thread(&gate, "true", Duration::from_secs(5));
        let out = next.recv_timeout(Duration::from_secs(5)).expect("admitted after a spawn error").unwrap();
        assert_eq!(out.code, Some(0));

        let err = run_gated(&gate, "sh", &sh("exec sleep 30"), b"", Duration::from_millis(300)).unwrap_err();
        assert!(matches!(err, TransportError::Timeout { .. }), "{err:?}");
        let next = gated_on_thread(&gate, "true", Duration::from_secs(5));
        let out = next.recv_timeout(Duration::from_secs(5)).expect("admitted after a timeout kill").unwrap();
        assert_eq!(out.code, Some(0));
    }

    #[test]
    fn a_missing_program_is_a_spawn_error() {
        let err = Ungated
            .run("/nonexistent/orcastudio-ssh", &[], b"", Duration::from_secs(1))
            .unwrap_err();
        assert!(matches!(err, TransportError::Spawn { .. }), "{err:?}");
    }
}
