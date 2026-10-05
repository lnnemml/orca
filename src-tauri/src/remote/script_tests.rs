//! Tests that run the **real** embedded scripts (unit 5.2 Part B, ADR-024 l "Tests").
//!
//! Everything runs on this machine: a stub `tsp` on `PATH` (task-spooler is not installed here,
//! and the stub never is the real one), a stub ORCA, real processes in real sessions. Each test
//! owns a [`Lab`] — a unique dir under `std::env::temp_dir()` — and **signals only processes it
//! started itself**: its direct children through their `Child` handles, every other process it
//! caused (stub-ORCA children, perl helpers) by the PID that process wrote to a file, and only
//! while that PID still has the start time recorded when it was first seen. Nothing is ever
//! signalled by name. `Drop` cleans up even when an assertion fails.

use std::collections::{BTreeSet, HashMap};
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use super::classify::{classify, is_alive, is_our_wrapper, job_session, sid_reused, Classification, Outcome};
use super::markers::{parse_started, BootId};
use super::procfs::{parse_stat, ProcStat};
use super::scripts::{sha256_hex, CANCEL, COLLECT, WRAPPER};
use super::snapshot::{Attempt, JobIdentity, Snapshot, SocketState};
use super::tsp::fixtures::{queued_row_for, running_row_for, TSP_L_P4};
use super::wire::{parse_snapshot, WireError};

static NEXT_LAB: AtomicU32 = AtomicU32::new(0);
const WAIT: Duration = Duration::from_secs(10);

/// The stub ORCA. Records what it saw (cwd, TMPDIR, env, args, affinity), optionally starts
/// extra session members, optionally sleeps, prints the normal-termination banner and exits with
/// `STUB_ORCA_EXIT`. `STUB_RAN_LOG` (an absolute path) gets one line per run, so a test can tell
/// that ORCA never ran even when it would have run in the wrong dir.
const STUB_ORCA: &str = r#"#!/bin/bash
# Stub ORCA for the remote-script tests. Never the real ORCA.
set -u
printf '%s\n' "$PWD" >>"$STUB_RAN_LOG"
printf '%s\n' "$PWD" >orca.cwd
printf '%s\n' "${TMPDIR-unset}" >orca.tmpdir
printf '%s\n' "${HWLOC_COMPONENTS-unset}" "${OMPI_MCA_hwloc_base_binding_policy-unset}" >orca.env
printf '%s\n' "$@" >orca.args
grep '^Cpus_allowed_list:' /proc/self/status >orca.affinity
for extra in ${STUB_ORCA_EXTRAS-}; do
    case $extra in
        # An MPI-rank-like member: own process group, same session, cwd = the job dir. Each
        # helper publishes its own PID file only once its setup is done.
        rank) perl -e 'setpgrp(0, 0) or die; open my $f, ">", "rank.pid.tmp" or die; print $f "$$\n";
                       close $f; rename "rank.pid.tmp", "rank.pid" or die; exec "sleep", "20"' & ;;
        # Same session, own group, cwd elsewhere.
        foreign) perl -e 'setpgrp(0, 0) or die; open my $f, ">", "foreign.pid.tmp" or die; print $f "$$\n";
                          close $f; rename "foreign.pid.tmp", "foreign.pid" or die;
                          chdir $ARGV[0] or die; exec "sleep", "20"' "$STUB_FOREIGN_DIR" & ;;
        # A parent that never waits, and its exited child: a zombie member (probe 5.2c).
        zombie) perl -e 'my $p = fork // die; exit 0 if $p == 0;
                         open my $f, ">", "zombie.pid.tmp" or die; print $f "$p\n"; close $f;
                         rename "zombie.pid.tmp", "zombie.pid" or die; sleep 20' &
                echo "$!" >zparent.pid ;;
        # A member whose /proc/<pid>/cwd is EACCES even to its owner: PR_SET_DUMPABLE 0.
        nodump) python3 -c 'import ctypes, time
assert ctypes.CDLL(None).prctl(4, 0, 0, 0, 0) == 0
open("nodump.ready", "w").close()
time.sleep(20)' & echo "$!" >nodump.pid ;;
    esac
done
if [[ -n ${STUB_ORCA_TERM_LATE_FILE-} ]]; then
    # A shutdown that writes into TMPDIR late, after a TERM (an MPI runtime's session files, say):
    # it recreates the dir if it is gone and, only if that late write succeeded, leaves orca.trapped.
    # Set before orca.pid is written, so a test that has orca.pid knows the trap is in place.
    trap 'sleep 0.5; mkdir -p -- "$TMPDIR" && : >"$TMPDIR/$STUB_ORCA_TERM_LATE_FILE" && : >orca.trapped; exit 143' TERM
fi
echo "ORCA stub output"
if [[ ${STUB_ORCA_SLEEP-0} != 0 ]]; then
    sleep "$STUB_ORCA_SLEEP" &
    echo "$!" >sleep.pid
    echo "$$" >orca.pid
    wait "$!"
else
    echo "$$" >orca.pid
fi
echo "                             ****ORCA TERMINATED NORMALLY****"
exit "${STUB_ORCA_EXIT-0}"
"#;

/// The stub `tsp`: logs `<TS_SOCKET> <args>` to `tsp.log` beside itself, and to `tsp.fd9.log`
/// too when it was started with fd 9 open (the submit lock's fd, ADR-024 o item 3.3.1); `-l`
/// prints `<TS_SOCKET>.rows` (or fails if `<TS_SOCKET>.fail` exists; first sleeps the seconds in
/// `<TS_SOCKET>.sleep` if that exists); `-r` succeeds; a command
/// (no leading `-`) is enqueued: unless `<TS_SOCKET>.enqueue-fail` exists, it appends a `queued`
/// row in the recorded shape to `<TS_SOCKET>.rows`, records `TMPDIR` in `<TS_SOCKET>.tmpdir`,
/// starts a "daemon" if nothing listens on the socket, and prints the id (0, 1, … per socket).
/// The daemon is a setsid'd perl listener that inherits the client's fds, as tsp's own daemon
/// forks from its first client (probe 5.3c); its PID goes to `daemon.pids`.
const STUB_TSP: &str = r#"#!/bin/bash
# Stub tsp for the remote-script tests. Never the real task-spooler.
set -u
dir=${BASH_SOURCE[0]%/*}
printf '%s %s\n' "${TS_SOCKET-unset}" "$*" >>"$dir/tsp.log"
if [[ -e /proc/$$/fd/9 ]]; then
    printf '%s %s\n' "${TS_SOCKET-unset}" "$*" >>"$dir/tsp.fd9.log"
fi
listening() {
    awk -v p="$TS_SOCKET" '$NF == p { found = 1 } END { exit !found }' /proc/net/unix
}
case ${1-} in
    -l) if [[ -e $TS_SOCKET.fail ]]; then echo "stub: request failed" >&2; exit 255; fi
        if [[ -e $TS_SOCKET.sleep ]]; then sleep "$(<"$TS_SOCKET.sleep")"; fi
        cat -- "$TS_SOCKET.rows" ;;
    -r) exit 0 ;;
    -*|'') exit 99 ;;
    *) if [[ -e $TS_SOCKET.enqueue-fail ]]; then echo "stub: enqueue failed" >&2; exit 1; fi
       id=0
       [[ -e $TS_SOCKET.nextid ]] && id=$(<"$TS_SOCKET.nextid")
       echo "$(( id + 1 ))" >"$TS_SOCKET.nextid"
       if [[ ! -e $TS_SOCKET.rows ]]; then
           echo "ID   State      Output               E-Level  Times(r/u/s)   Command [run=1/1]" >"$TS_SOCKET.rows"
       fi
       printf '%-4s queued     (file)                                       %s\n' "$id" "$*" >>"$TS_SOCKET.rows"
       printf '%s\n' "${TMPDIR-unset}" >>"$TS_SOCKET.tmpdir"
       if ! listening; then
           ( cd -- "$dir" && exec setsid perl -MIO::Socket::UNIX -e \
               'unlink $ARGV[0] if -S $ARGV[0];
                my $s = IO::Socket::UNIX->new(Type => SOCK_STREAM(), Local => $ARGV[0], Listen => 1) or die $!;
                sleep 60' "$TS_SOCKET" </dev/null >/dev/null 2>&1 ) &
           echo "$!" >>"$dir/daemon.pids"
           for _ in $(seq 250); do listening && break; sleep 0.02; done
       fi
       printf '%s\n' "$id" ;;
esac
"#;

/// The stub `busctl`: after sleeping `STUB_KUP_SLEEP` seconds (default 0), the
/// `KillUserProcesses` property as `STUB_KUP` says (default `b false`), with exit status
/// `STUB_KUP_RC` (default 0); with `STUB_KUP_NUL` set, `b false\n\0junk` and exit 0.
const STUB_BUSCTL: &str = r#"#!/bin/bash
# Stub busctl for the remote-script tests.
sleep "${STUB_KUP_SLEEP-0}"
if [[ -n ${STUB_KUP_NUL-} ]]; then printf 'b false\n\0junk'; exit 0; fi
printf '%s\n' "${STUB_KUP-b false}"
exit "${STUB_KUP_RC-0}"
"#;

/// A process the test caused, identified by PID **and** start time, so cleanup can never signal
/// a process that reused the PID.
#[derive(Clone, Copy, Debug)]
pub(super) struct Tracked {
    pub(super) pid: u32,
    starttime: u64,
}

pub(super) struct Lab {
    pub(super) root: PathBuf,
    pub(super) wrapper: PathBuf,
    cancel: PathBuf,
    collect: PathBuf,
    pub(super) stub_dir: PathBuf,
    children: Vec<Child>,
    pub(super) tracked: Vec<Tracked>,
}

impl Lab {
    pub(super) fn new() -> Lab {
        let n = NEXT_LAB.fetch_add(1, Ordering::SeqCst);
        // Short on purpose: a socket path must fit sun_path (108 bytes).
        let root = std::env::temp_dir().join(format!("os52-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let bin = root.join("bin");
        let stub_dir = root.join("stub");
        for dir in [&bin, &stub_dir, &root.join("jobs")] {
            fs::create_dir_all(dir).unwrap();
        }
        // The wrapper under its content-addressed upload name, so "ours" sees the real shape.
        let wrapper = bin.join(format!("wrapper-{}.sh", sha256_hex(WRAPPER)));
        let cancel = bin.join(format!("cancel-{}.sh", sha256_hex(CANCEL)));
        let collect = bin.join(format!("collect-{}.sh", sha256_hex(COLLECT)));
        fs::write(&wrapper, WRAPPER).unwrap();
        fs::write(&cancel, CANCEL).unwrap();
        fs::write(&collect, COLLECT).unwrap();
        for (name, text) in [("orca", STUB_ORCA), ("tsp", STUB_TSP), ("busctl", STUB_BUSCTL)] {
            let path = stub_dir.join(name);
            fs::write(&path, text).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(stub_dir.join("tsp.log"), "").unwrap();
        Lab { root, wrapper, cancel, collect, stub_dir, children: Vec::new(), tracked: Vec::new() }
    }

    pub(super) fn str(path: &Path) -> &str {
        path.to_str().expect("lab paths are UTF-8")
    }

    pub(super) fn path_env(&self) -> String {
        format!("{}:{}", Lab::str(&self.stub_dir), std::env::var("PATH").unwrap_or_default())
    }

    pub(super) fn ran_log(&self) -> PathBuf {
        self.root.join("orca-ran.log")
    }

    fn orca_runs(&self) -> usize {
        fs::read_to_string(self.ran_log()).map(|s| s.lines().count()).unwrap_or(0)
    }

    pub(super) fn tsp_log(&self) -> String {
        fs::read_to_string(self.stub_dir.join("tsp.log")).unwrap()
    }

    pub(super) fn clear_tsp_log(&self) {
        fs::write(self.stub_dir.join("tsp.log"), "").unwrap();
    }

    /// A fresh job dir with an input.
    pub(super) fn job(&self, name: &str) -> PathBuf {
        let dir = self.root.join("jobs").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("input.inp"), "! HF\n").unwrap();
        dir
    }

    fn identity(&self, job: &Path) -> JobIdentity {
        JobIdentity { job_dir: Lab::str(job).into(), root: Lab::str(&self.root).into() }
    }

    /// The wrapper command line exactly as tsp runs it (`bash <wrapper> <job> <mask> <orca>`).
    fn wrapper_command(&self, job: &Path, env: &[(&str, &str)]) -> Command {
        let mut cmd = Command::new("bash");
        cmd.arg(&self.wrapper).arg(job).arg("0").arg(self.stub_dir.join("orca"));
        cmd.env("STUB_RAN_LOG", self.ran_log())
            .env("STUB_FOREIGN_DIR", self.root.join("elsewhere"))
            .envs(env.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(self.root.join(format!("wrapper-{}.err", self.children.len()))).unwrap());
        cmd
    }

    /// Start the wrapper as tsp does: in a new session (PID = PGID = SID, probe P2).
    fn spawn_wrapper(&mut self, job: &Path, env: &[(&str, &str)]) -> u32 {
        let mut cmd = self.wrapper_command(job, env);
        new_session(&mut cmd);
        self.spawn(cmd)
    }

    /// Run the wrapper to completion (in its own session) and return its status.
    fn run_wrapper(&mut self, job: &Path, env: &[(&str, &str)]) -> ExitStatus {
        let pid = self.spawn_wrapper(job, env);
        self.wait_child(pid)
    }

    pub(super) fn spawn(&mut self, mut cmd: Command) -> u32 {
        let child = cmd.spawn().expect("spawn");
        let pid = child.id();
        self.children.push(child);
        pid
    }

    pub(super) fn wait_child(&mut self, pid: u32) -> ExitStatus {
        let child = self.children.iter_mut().find(|c| c.id() == pid).expect("our child");
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "child {pid} did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Wait for a PID file written by a process the test caused, and track that process.
    pub(super) fn tracked_pid(&mut self, file: &Path) -> u32 {
        wait_for(&format!("{}", file.display()), || {
            fs::read_to_string(file).is_ok_and(|s| s.ends_with('\n'))
        });
        let pid: u32 = fs::read_to_string(file).unwrap().trim().parse().unwrap();
        if let Some(stat) = stat_of(pid) {
            self.tracked.push(Tracked { pid, starttime: stat.starttime });
        }
        pid
    }

    fn collect(&self, job: &Path, sockets: &[PathBuf]) -> Output {
        Command::new("bash")
            .arg(&self.collect)
            .arg(job)
            .args(sockets)
            .env("PATH", self.path_env())
            .output()
            .unwrap()
    }

    fn snapshot(&self, job: &Path, sockets: &[PathBuf]) -> Result<Snapshot, WireError> {
        let out = self.collect(job, sockets);
        let result = parse_snapshot(
            &out.stdout,
            self.identity(job),
            Attempt::First,
            &sockets.iter().map(|s| Lab::str(s).to_string()).collect::<Vec<_>>(),
        );
        // The exit status and the error record say the same thing.
        assert_eq!(
            out.status.success(),
            !matches!(result, Err(WireError::Collector(_))),
            "collector status {:?} vs parse {result:?}; stderr {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        result
    }

    /// Collect and classify. With no sockets given, the lab's slot socket is used: a path no
    /// daemon listens on (NoDaemon), since a real collection always carries the profile's slots.
    pub(super) fn classify(&self, job: &Path, sockets: &[PathBuf]) -> Outcome {
        let slot = [self.root.join("slot.sock")];
        let sockets = if sockets.is_empty() { &slot[..] } else { sockets };
        let snap = self.snapshot(job, sockets).expect("snapshot");
        match classify(&snap, 0).expect("classify") {
            Classification::Decided(outcome) => outcome,
            Classification::Retake => panic!("unexpected retake"),
        }
    }

    fn cancel(&self, mode: &str, job: &Path) -> Output {
        Command::new("bash")
            .arg(&self.cancel)
            .arg(mode)
            .arg(job)
            .arg(&self.root)
            .env("PATH", self.path_env())
            .output()
            .unwrap()
    }

    /// A listening Unix socket at `path`, as a live tsp daemon's would be (only the listening
    /// matters: the collector and cancel script read `/proc/net/unix`, never connect).
    pub(super) fn listen(&mut self, path: &Path) -> u32 {
        let mut cmd = Command::new("perl");
        cmd.arg("-MIO::Socket::UNIX")
            .arg("-e")
            .arg("my $s = IO::Socket::UNIX->new(Type => SOCK_STREAM(), Local => $ARGV[0], Listen => 1) or die $!; sleep 20")
            .arg(path)
            .stdin(Stdio::null());
        let pid = self.spawn(cmd);
        wait_for("socket listed", || {
            fs::read_to_string("/proc/net/unix").unwrap().lines().any(|l| l.ends_with(Lab::str(path)))
        });
        pid
    }

    /// A stale socket file: its listener was killed, the file stays (probe 5.2b).
    pub(super) fn stale_socket(&mut self, path: &Path) {
        let pid = self.listen(path);
        let child = self.children.iter_mut().find(|c| c.id() == pid).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(path.exists(), "the socket file outlives its listener");
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        // The stub tsp's daemons (perl listeners started in the stub dir).
        let daemons = fs::read_to_string(self.stub_dir.join("daemon.pids")).unwrap_or_default();
        for pid in daemons.lines().filter_map(|l| l.trim().parse::<u32>().ok()) {
            if let Some(stat) = stat_of(pid) {
                if read_link(pid).is_some_and(|cwd| cwd.starts_with(&self.root)) {
                    self.tracked.push(Tracked { pid, starttime: stat.starttime });
                }
            }
        }
        // PID files written after the last `tracked_pid` call are picked up here, so a test that
        // failed early still leaves nothing behind.
        for name in ["orca.pid", "sleep.pid", "rank.pid", "foreign.pid", "zparent.pid", "nodump.pid", "member.pid"] {
            for job in fs::read_dir(self.root.join("jobs")).into_iter().flatten().flatten() {
                if let Some(pid) = fs::read_to_string(job.path().join(name)).ok().and_then(|s| s.trim().parse().ok()) {
                    if let Some(stat) = stat_of(pid) {
                        // Only if the process is still the one the stub started in this lab.
                        if read_link(pid).is_some_and(|cwd| cwd.starts_with(&self.root)) {
                            self.tracked.push(Tracked { pid, starttime: stat.starttime });
                        }
                    }
                }
            }
        }
        // Stop everything before killing anything: a wrapper still running when its ORCA is
        // killed would fork `mv` to publish .exit_code, and that orphaned `mv` can land in the job
        // dir while it is being removed. A stopped process forks nothing; SIGKILL still ends it.
        let ours: Vec<Tracked> = self.tracked.iter().copied().filter(|t| stat_of(t.pid).is_some_and(|s| s.starttime == t.starttime)).collect();
        for sig in [libc::SIGSTOP, libc::SIGKILL] {
            for child in &mut self.children {
                // Only a child not yet reaped: once reaped, its PID may belong to anyone.
                if matches!(child.try_wait(), Ok(None)) {
                    // SAFETY: kill(2) on our own unreaped child: its PID cannot have been reused.
                    unsafe { libc::kill(child.id() as i32, sig) };
                }
            }
            for t in &ours {
                if stat_of(t.pid).is_some_and(|s| s.starttime == t.starttime) {
                    // SAFETY: plain kill(2) on a PID we verified is still the process we caused.
                    unsafe { libc::kill(t.pid as i32, sig) };
                }
            }
        }
        for child in &mut self.children {
            let _ = child.wait();
        }
        // A test may have made a job dir read-only; give it back its write bit so it can go.
        for job in fs::read_dir(self.root.join("jobs")).into_iter().flatten().flatten() {
            let _ = fs::set_permissions(job.path(), fs::Permissions::from_mode(0o755));
        }
        // Bounded retry: a killed process may still be finishing a write (ENOTEMPTY).
        let deadline = Instant::now() + Duration::from_secs(2);
        while let Err(e) = fs::remove_dir_all(&self.root) {
            if e.kind() == std::io::ErrorKind::NotFound || Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Make the command's process a session leader, as tsp does for every task.
pub(super) fn new_session(cmd: &mut Command) {
    // SAFETY: setsid(2) is async-signal-safe, the only requirement on a pre_exec closure.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

pub(super) fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn stat_of(pid: u32) -> Option<ProcStat> {
    fs::read(format!("/proc/{pid}/stat")).ok().and_then(|raw| parse_stat(&raw).ok())
}

fn read_link(pid: u32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// Gone, a zombie, or another process at that PID.
pub(super) fn is_dead(t: Tracked) -> bool {
    stat_of(t.pid).is_none_or(|s| s.is_zombie() || s.starttime != t.starttime)
}

/// After a lab is dropped: no live process anywhere has its cwd in, or an argument naming, that
/// lab's root (`<root>` or `<root>/…` — never a sibling lab whose name extends it). Zombies have no
/// cwd and an empty cmdline, so a dead-but-unreaped process does not count; a bounded wait covers a
/// killed process that has not finished exiting.
pub(super) fn assert_no_process_left(root: &Path) {
    let root = root.as_os_str().as_bytes().to_vec();
    let mut prefix = root.clone();
    prefix.push(b'/');
    let ours = |bytes: &[u8]| bytes == root.as_slice() || bytes.windows(prefix.len()).any(|w| w == prefix.as_slice());
    let me = std::process::id();
    let left = || -> Vec<u32> {
        fs::read_dir("/proc")
            .unwrap()
            .flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()))
            .filter(|&pid| pid != me)
            .filter(|&pid| {
                let cwd = read_link(pid).is_some_and(|c| {
                    let c = c.as_os_str().as_bytes();
                    c == root.as_slice() || c.starts_with(&prefix)
                });
                let argv = fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|a| a.split(|b| *b == 0).any(|arg| ours(arg)));
                cwd || argv
            })
            .collect()
    };
    let deadline = Instant::now() + WAIT;
    loop {
        let pids = left();
        if pids.is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "processes outlived the lab: {pids:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn tracked(pid: u32) -> Tracked {
    Tracked { pid, starttime: stat_of(pid).expect("process exists").starttime }
}

fn current_boot() -> String {
    fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap().trim_end().to_string()
}

/// A `.started` for a process the test started itself, with a chosen start time.
fn write_started(job: &Path, pid: u32, starttime: u64) {
    write_started_text(
        job,
        &format!("pid={pid}\npgid={pid}\nsid={pid}\nboot_id={}\nstarttime={starttime}\nstarted_at=1\n", current_boot()),
    );
}

fn write_started_text(job: &Path, text: &str) {
    fs::write(job.join(".started.tmp"), text).unwrap();
    fs::rename(job.join(".started.tmp"), job.join(".started")).unwrap();
}

/// A valid boot id that is not this boot's.
fn other_boot() -> String {
    let current = current_boot();
    let other = "9b1d0e2f-3a4c-4d5e-8f60-718293a4b5c6".to_string();
    assert_ne!(current, other);
    other
}

/// A session leader we start ourselves: `sleep 20` in `cwd`, setsid'd like a tsp task.
fn session_sleeper(lab: &mut Lab, cwd: &Path) -> Tracked {
    let mut cmd = Command::new("sleep");
    cmd.arg("20").current_dir(cwd).stdin(Stdio::null());
    new_session(&mut cmd);
    let pid = lab.spawn(cmd);
    tracked(pid)
}

fn check_map(lab: &Lab, job: &Path) -> HashMap<String, String> {
    let out = lab.cancel("check", job);
    assert!(out.status.success(), "check failed: {}", String::from_utf8_lossy(&out.stderr));
    stdout(&out)
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The predicates, as the shell (`cancel.sh check`) and as Rust (Part A over the collector's
/// snapshot) see them.
#[derive(Debug, PartialEq, Eq)]
struct Verdict {
    alive: bool,
    ours: bool,
    sid_reused: bool,
    session: BTreeSet<u32>,
}

fn shell_verdict(lab: &Lab, job: &Path) -> Verdict {
    let out = lab.cancel("check", job);
    assert!(out.status.success(), "check failed: {}", String::from_utf8_lossy(&out.stderr));
    let text = stdout(&out);
    let kv: HashMap<&str, &str> = text.lines().filter_map(|l| l.split_once('=')).collect();
    assert_eq!(kv["started"], "ok");
    assert_eq!(kv["boot"], "current");
    let yes = |k: &str| match kv[k] {
        "yes" => true,
        "no" => false,
        other => panic!("{k}={other}"),
    };
    Verdict {
        alive: yes("alive"),
        ours: yes("ours"),
        sid_reused: yes("sid_reused"),
        session: kv["session"].split_whitespace().map(|p| p.parse().unwrap()).collect(),
    }
}

fn rust_verdict(snap: &Snapshot) -> Verdict {
    let started = parse_started(snap.started_first.as_deref().expect(".started")).unwrap();
    let boot = BootId::parse(&snap.current_boot_id).unwrap();
    let stat = snap.wrapper_stat.as_deref().map(|raw| parse_stat(raw).unwrap());
    let reused = sid_reused(snap.sid_stat.as_deref(), &started).unwrap();
    Verdict {
        alive: is_alive(stat.as_ref(), &started, &boot),
        ours: is_our_wrapper(&snap.wrapper_cmdline, &snap.identity),
        sid_reused: reused,
        session: if reused {
            BTreeSet::new()
        } else {
            job_session(&snap.session_members, &snap.identity.job_dir).into_iter().collect()
        },
    }
}

/// Shell and Rust must agree (ADR-024 l, "Shell and Rust liveness agree").
fn parity(lab: &Lab, job: &Path) -> (Verdict, Snapshot) {
    let snap = lab.snapshot(job, &[]).expect("snapshot");
    let rust = rust_verdict(&snap);
    let shell = shell_verdict(lab, job);
    assert_eq!(shell, rust, "shell and Rust predicates disagree");
    (rust, snap)
}

/// inotify on a job dir: which events each marker name received. Proves a marker is only ever
/// published by `rename` (IN_MOVED_TO) and never created or written in place.
struct DirWatch {
    fd: i32,
}

impl DirWatch {
    fn new(dir: &Path) -> DirWatch {
        // SAFETY: plain inotify syscalls on a path we own; the fd is closed in Drop.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        assert!(fd >= 0);
        let path = CString::new(dir.as_os_str().as_bytes()).unwrap();
        let mask = libc::IN_CREATE | libc::IN_MOVED_TO | libc::IN_MODIFY | libc::IN_CLOSE_WRITE;
        let wd = unsafe { libc::inotify_add_watch(fd, path.as_ptr(), mask) };
        assert!(wd >= 0);
        DirWatch { fd }
    }

    /// Every (name, mask) seen so far.
    fn events(&self) -> Vec<(String, u32)> {
        let mut events = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            // SAFETY: reading into a buffer we own, with its length.
            let n = unsafe { libc::read(self.fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                break;
            }
            let mut at = 0;
            while at < n as usize {
                let field = |o: usize| u32::from_ne_bytes(buf[at + o..at + o + 4].try_into().unwrap());
                let (mask, len) = (field(4), field(12) as usize);
                let name = &buf[at + 16..at + 16 + len];
                let name = String::from_utf8_lossy(name.split(|b| *b == 0).next().unwrap_or_default()).into_owned();
                events.push((name, mask));
                at += 16 + len;
            }
        }
        events
    }
}

impl Drop for DirWatch {
    fn drop(&mut self) {
        // SAFETY: closing the fd we opened.
        unsafe { libc::close(self.fd) };
    }
}

/// The marker `name` appeared in one atomic step and was never written under its own name: a
/// rename shows exactly one IN_MOVED_TO, a hard link exactly one IN_CREATE. A write in place
/// (`>`) would add IN_MODIFY / IN_CLOSE_WRITE on the name.
fn assert_published_atomically(events: &[(String, u32)], name: &str, by: u32) {
    let masks: Vec<u32> = events.iter().filter(|(n, _)| n == name).map(|(_, m)| *m).collect();
    assert_eq!(masks, vec![by], "{name}: expected exactly one {by:x} event, saw masks {masks:x?}");
}

// ---- wrapper -------------------------------------------------------------------------------

#[test]
fn wrapper_runs_orca_pinned_in_its_job_dir_and_publishes_markers_atomically() {
    let mut lab = Lab::new();
    let job = lab.job("happy");
    let watch = DirWatch::new(&job);
    let pid = lab.spawn_wrapper(&job, &[]);
    wait_for(".exit_code", || job.join(".exit_code").exists());
    // Not reaped yet: the zombie keeps its start time (probe 5.2c), so .started can be checked
    // against the kernel's own value.
    wait_for("wrapper exit", || stat_of(pid).is_some_and(|s| s.is_zombie()));
    let kernel = stat_of(pid).unwrap();
    assert!(lab.wait_child(pid).success());

    let started = parse_started(&fs::read(job.join(".started")).unwrap()).expect("Part A parses .started");
    assert_eq!((started.pid, started.pgid, started.sid), (pid, pid, pid));
    assert_eq!(started.starttime, kernel.starttime);
    assert_eq!(started.boot_id.as_str(), current_boot());
    assert_eq!(fs::read_to_string(job.join(".exit_code")).unwrap(), "0\n");

    let read = |f: &str| fs::read_to_string(job.join(f)).unwrap();
    assert!(read("output.out").contains("ORCA TERMINATED NORMALLY"));
    assert_eq!(read("orca.cwd").trim_end(), Lab::str(&job), "ORCA runs in its job dir");
    assert_eq!(read("orca.tmpdir").trim_end(), Lab::str(&job.join(".tmp")));
    assert!(job.join(".tmp").is_dir());
    assert_eq!(read("orca.env"), "-gl\nnone\n");
    assert_eq!(read("orca.args"), "input.inp\n");
    assert_eq!(read("orca.affinity").split_whitespace().last(), Some("0"), "taskset mask applied");
    assert!(job.join("stderr.log").exists());

    let events = watch.events();
    assert_published_atomically(&events, ".started", libc::IN_CREATE); // ln -T: linkat(2)
    assert_published_atomically(&events, ".exit_code", libc::IN_MOVED_TO); // mv -fT: rename(2)
}

#[test]
fn wrapper_that_cannot_cd_exits_before_started() {
    let mut lab = Lab::new();
    let job = lab.root.join("jobs").join("missing");
    let status = lab.run_wrapper(&job, &[]);
    assert_eq!(status.code(), Some(1));
    assert!(!job.exists());
    assert_eq!(lab.orca_runs(), 0, "ORCA never runs outside its job dir");
}

#[test]
fn wrapper_refuses_bad_arguments_before_anything() {
    let mut lab = Lab::new();
    let job = lab.job("args");
    let mut cmd = Command::new("bash");
    cmd.arg(&lab.wrapper).arg(&job).arg("0").arg("orca").env("STUB_RAN_LOG", lab.ran_log()).stderr(Stdio::null());
    let pid = lab.spawn(cmd);
    assert_eq!(lab.wait_child(pid).code(), Some(2), "a relative ORCA path is refused (rule #1)");
    let mut cmd = Command::new("bash");
    cmd.arg(&lab.wrapper)
        .arg(format!("{}/../args", Lab::str(&job)))
        .arg("0")
        .arg(lab.stub_dir.join("orca"))
        .stderr(Stdio::null());
    let pid = lab.spawn(cmd);
    assert_eq!(lab.wait_child(pid).code(), Some(2), "a job dir with '..' is refused");
    assert!(!job.join(".started").exists());
    assert_eq!(lab.orca_runs(), 0);
}

#[test]
fn wrapper_under_cancelled_writes_started_but_never_runs_orca() {
    let mut lab = Lab::new();
    let job = lab.job("pre-cancelled");
    fs::write(job.join(".cancelled"), "").unwrap();
    let status = lab.run_wrapper(&job, &[]);
    assert!(status.success());
    assert!(parse_started(&fs::read(job.join(".started")).unwrap()).is_ok());
    assert!(!job.join(".exit_code").exists());
    assert_eq!(lab.orca_runs(), 0);
}

#[test]
fn wrapper_self_check_failure_writes_97_and_never_runs_orca() {
    let mut lab = Lab::new();
    let job = lab.job("self-check");
    // Fault injection: an `ln` that "succeeds" but publishes an empty .started — the disk-full
    // shape of ADR-024 l row 1. Only this test puts it on PATH.
    let fault = lab.root.join("fault");
    fs::create_dir_all(&fault).unwrap();
    fs::write(fault.join("ln"), "#!/bin/bash\n: >\"${@: -1}\"\n").unwrap();
    fs::set_permissions(fault.join("ln"), fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", Lab::str(&fault), std::env::var("PATH").unwrap_or_default());
    let status = lab.run_wrapper(&job, &[("PATH", &path)]);
    assert_eq!(status.code(), Some(97));
    assert_eq!(fs::read(job.join(".started")).unwrap(), b"");
    assert_eq!(fs::read_to_string(job.join(".exit_code")).unwrap(), "97\n");
    assert_eq!(lab.orca_runs(), 0);
}

#[test]
fn wrapper_refuses_a_job_dir_that_already_has_started() {
    let mut lab = Lab::new();
    let job = lab.job("restart");
    assert!(lab.run_wrapper(&job, &[]).success());
    let started = fs::read(job.join(".started")).unwrap();
    let exit_code = fs::read(job.join(".exit_code")).unwrap();
    assert_eq!(lab.orca_runs(), 1);

    let status = lab.run_wrapper(&job, &[]);
    assert_eq!(status.code(), Some(1), "refused");
    assert_eq!(fs::read(job.join(".started")).unwrap(), started, ".started untouched");
    assert_eq!(fs::read(job.join(".exit_code")).unwrap(), exit_code, ".exit_code untouched");
    assert_eq!(lab.orca_runs(), 1, "the second wrapper never runs ORCA");
}

/// A `.started` that is not a regular file still blocks a start (ADR-024 l, Part B detail 5):
/// `ln -T` fails with EEXIST on a dangling symlink and on a directory alike, and the refusal test
/// must see both forms (`-e` alone misses the dangling symlink and would fall to the 97 path).
#[test]
fn wrapper_refuses_a_job_dir_whose_started_is_a_dangling_symlink_or_a_directory() {
    let mut lab = Lab::new();

    let job = lab.job("started-dangling");
    let target = lab.root.join("nowhere");
    std::os::unix::fs::symlink(&target, job.join(".started")).unwrap();
    let status = lab.run_wrapper(&job, &[]);
    assert_eq!(status.code(), Some(1), "dangling symlink: refused");
    assert_eq!(fs::read_link(job.join(".started")).unwrap(), target, "the symlink is untouched");
    assert!(!target.exists(), "nothing was written through the symlink");
    assert!(!job.join(".exit_code").exists(), "dangling symlink: no .exit_code");
    assert_no_temp_markers(&job);

    let job = lab.job("started-dir");
    fs::create_dir(job.join(".started")).unwrap();
    fs::write(job.join(".started").join("inside"), "keep").unwrap();
    let status = lab.run_wrapper(&job, &[]);
    assert_eq!(status.code(), Some(1), "directory: refused");
    assert!(job.join(".started").is_dir());
    assert_eq!(fs::read_dir(job.join(".started")).unwrap().count(), 1, "the directory is untouched");
    assert_eq!(fs::read_to_string(job.join(".started").join("inside")).unwrap(), "keep");
    assert!(!job.join(".exit_code").exists(), "directory: no .exit_code");
    assert_no_temp_markers(&job);

    assert_eq!(lab.orca_runs(), 0, "a refused wrapper never runs ORCA");
}

/// No `.<marker>.tmp.<pid>` left behind in a job dir.
fn assert_no_temp_markers(job: &Path) {
    let leftovers: Vec<String> = fs::read_dir(job)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp."))
        .collect();
    assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
}

#[test]
fn two_wrappers_started_together_on_one_job_dir_run_orca_once() {
    let mut lab = Lab::new();
    for round in 0..5 {
        let job = lab.job(&format!("race{round}"));
        let before = lab.orca_runs();
        let first = lab.spawn_wrapper(&job, &[]);
        let second = lab.spawn_wrapper(&job, &[]);
        let codes = [lab.wait_child(first).code(), lab.wait_child(second).code()];
        assert_eq!(lab.orca_runs() - before, 1, "round {round}: exactly one wrapper runs ORCA");
        let mut sorted = codes;
        sorted.sort();
        assert_eq!(sorted, [Some(0), Some(1)], "round {round}: one ran, one refused");
        let started = parse_started(&fs::read(job.join(".started")).unwrap()).unwrap();
        assert!([first, second].contains(&started.pid));
    }
}

#[test]
fn wrapper_that_cannot_create_its_tmp_dir_writes_96_and_never_runs_orca() {
    let mut lab = Lab::new();
    let job = lab.job("no-tmp");
    fs::write(job.join(".tmp"), "a file, not a dir").unwrap();
    let status = lab.run_wrapper(&job, &[]);
    assert_eq!(status.code(), Some(96));
    assert_eq!(fs::read_to_string(job.join(".exit_code")).unwrap(), "96\n");
    assert_eq!(lab.orca_runs(), 0, "no ORCA with a TMPDIR outside the job dir (rule #3)");
    assert_eq!(
        lab.classify(&job, &[]),
        Outcome::Failed { reason: super::classify::FailReason::NonZeroExit { code: 96 } }
    );
}

#[test]
fn wrapper_records_orcas_non_zero_exit_code() {
    let mut lab = Lab::new();
    let job = lab.job("fails");
    let status = lab.run_wrapper(&job, &[("STUB_ORCA_EXIT", "3")]);
    assert_eq!(status.code(), Some(3));
    assert_eq!(fs::read_to_string(job.join(".exit_code")).unwrap(), "3\n");
    assert_eq!(lab.classify(&job, &[]), Outcome::Failed { reason: super::classify::FailReason::NonZeroExit { code: 3 } });
}

// ---- shell/Rust parity on materialised fixtures (a)–(e) ------------------------------------

/// A running wrapper with a sleeping stub ORCA and the given extra members; returns its PID once
/// every member exists.
fn running_wrapper(lab: &mut Lab, job: &Path, extras: &str) -> u32 {
    fs::create_dir_all(lab.root.join("elsewhere")).unwrap();
    let pid = lab.spawn_wrapper(job, &[("STUB_ORCA_SLEEP", "20"), ("STUB_ORCA_EXTRAS", extras)]);
    lab.tracked_pid(&job.join("orca.pid"));
    lab.tracked_pid(&job.join("sleep.pid"));
    for extra in extras.split_whitespace() {
        let file = if extra == "zombie" { "zparent.pid" } else { &format!("{extra}.pid") };
        lab.tracked_pid(&job.join(file));
    }
    pid
}

#[test]
fn fixture_a_live_wrapper_is_alive_and_ours() {
    let mut lab = Lab::new();
    let job = lab.job("a");
    let pid = running_wrapper(&mut lab, &job, "");
    let (v, _) = parity(&lab, &job);
    assert!(v.alive && v.ours && !v.sid_reused);
    assert!(v.session.contains(&pid));
    assert_eq!(lab.classify(&job, &[]), Outcome::Running);
}

#[test]
fn fixture_b_zombie_wrapper_is_not_alive_but_its_sid_is_still_ours() {
    let mut lab = Lab::new();
    let job = lab.job("b");
    let zpid_file = job.join("wrapper.pid");
    // A perl parent that never waits; its child becomes a session leader and execs the wrapper
    // with the recorded argv shape (probe 5.2c's zombie maker).
    let mut cmd = Command::new("perl");
    cmd.arg("-MPOSIX")
        .arg("-e")
        .arg(
            "my $p = fork // die; if ($p == 0) { POSIX::setsid(); exec @ARGV or die } \
             open my $f, '>', \"$ENV{ZPID}.tmp\" or die; print $f \"$p\\n\"; close $f; \
             rename \"$ENV{ZPID}.tmp\", $ENV{ZPID} or die; sleep 20",
        )
        .arg("bash")
        .arg(&lab.wrapper)
        .arg(&job)
        .arg("0")
        .arg(lab.stub_dir.join("orca"))
        .env("ZPID", &zpid_file)
        .env("STUB_RAN_LOG", lab.ran_log())
        .stdin(Stdio::null());
    lab.spawn(cmd);
    let wrapper = lab.tracked_pid(&zpid_file);
    wait_for("zombie wrapper", || stat_of(wrapper).is_some_and(|s| s.is_zombie()));

    let (v, snap) = parity(&lab, &job);
    assert!(!v.alive, "a zombie is dead for every rule");
    assert!(!v.sid_reused, "same start time: the SID guard says ours");
    assert!(v.session.is_empty());
    assert!(snap.session_members.iter().any(|m| m.pid == wrapper && m.cwd.is_none()));
}

#[test]
fn fixture_c_forged_starttime_is_not_alive_and_the_sid_reads_as_reused() {
    let mut lab = Lab::new();
    let job = lab.job("c");
    let pid = running_wrapper(&mut lab, &job, "");
    let real = stat_of(pid).unwrap().starttime;
    write_started(&job, pid, real + 1);
    let (v, _) = parity(&lab, &job);
    assert!(!v.alive && v.ours && v.sid_reused);
    assert!(v.session.is_empty());
}

#[test]
fn fixture_d_zombie_member_with_enoent_cwd_is_not_in_the_job_session() {
    let mut lab = Lab::new();
    let job = lab.job("d");
    running_wrapper(&mut lab, &job, "zombie");
    let zombie = lab.tracked_pid(&job.join("zombie.pid"));
    wait_for("zombie member", || stat_of(zombie).is_some_and(|s| s.is_zombie()));
    let (v, snap) = parity(&lab, &job);
    assert!(v.alive && v.ours);
    assert!(snap.session_members.iter().any(|m| m.pid == zombie && m.cwd.is_none()), "listed, cwd ENOENT");
    assert!(!v.session.contains(&zombie));
}

#[test]
fn fixture_e_live_member_with_a_foreign_cwd_is_not_in_the_job_session() {
    let mut lab = Lab::new();
    let job = lab.job("e");
    running_wrapper(&mut lab, &job, "foreign");
    let foreign = lab.tracked_pid(&job.join("foreign.pid"));
    let (v, snap) = parity(&lab, &job);
    let elsewhere = lab.root.join("elsewhere");
    assert!(snap
        .session_members
        .iter()
        .any(|m| m.pid == foreign && m.cwd.as_deref() == Some(elsewhere.as_os_str().as_bytes())));
    assert!(!v.session.contains(&foreign));
}

// ---- cancel --------------------------------------------------------------------------------

#[test]
fn cancel_terms_the_group_and_sweeps_an_escaped_member_but_not_a_foreign_cwd() {
    let mut lab = Lab::new();
    let job = lab.job("cancel-running");
    let wrapper = running_wrapper(&mut lab, &job, "rank foreign");
    let pid_of = |f: &str| -> u32 { fs::read_to_string(job.join(f)).unwrap().trim().parse().unwrap() };
    let (orca, sleep, rank, foreign) =
        (tracked(pid_of("orca.pid")), tracked(pid_of("sleep.pid")), tracked(pid_of("rank.pid")), tracked(pid_of("foreign.pid")));
    // The fixture is what it claims: both escaped the wrapper's group but not its session.
    for t in [rank, foreign] {
        let s = stat_of(t.pid).unwrap();
        assert_eq!(s.session, wrapper);
        assert_ne!(s.pgrp, wrapper);
    }

    let out = lab.cancel("cancel", &job);
    let text = stdout(&out);
    assert!(out.status.success(), "{text}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(text.contains(&format!("group: TERM -{wrapper}")), "{text}");
    assert_eq!(lab.wait_child(wrapper).signal(), Some(libc::SIGTERM));
    for (name, t) in [("orca", orca), ("sleep", sleep), ("escaped rank", rank)] {
        wait_for(name, || is_dead(t));
    }
    assert!(!is_dead(foreign), "a member with a foreign cwd is never signalled");
    assert!(!job.join(".exit_code").exists());
    assert!(job.join(".cancelled").exists());
    assert!(!job.join(".tmp").exists(), "the cancel removes <job>/.tmp");
    assert_eq!(lab.classify(&job, &[]), Outcome::Cancelled);
}

#[test]
fn cancel_removes_tmp_after_the_sweep_so_a_late_tmpdir_write_cannot_survive() {
    let mut lab = Lab::new();
    let job = lab.job("late-tmp");
    let wrapper =
        lab.spawn_wrapper(&job, &[("STUB_ORCA_SLEEP", "20"), ("STUB_ORCA_TERM_LATE_FILE", "late.session")]);
    let orca = tracked(lab.tracked_pid(&job.join("orca.pid")));
    lab.tracked_pid(&job.join("sleep.pid"));
    assert!(job.join(".tmp").is_dir());

    let out = lab.cancel("cancel", &job);
    let text = stdout(&out);
    assert!(out.status.success(), "{text}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(text.contains(&format!("group: TERM -{wrapper}")), "{text}");
    // The fixture is what it claims: the sweep waited for ORCA, whose TERM trap wrote into TMPDIR
    // ~0.5 s after the TERM — before the cancel returned, and not killed by the sweep's KILL.
    assert!(job.join("orca.trapped").exists(), "the stub's TERM trap ran to completion: {text}");
    assert!(!text.contains("sweep: KILL"), "{text}");
    wait_for("orca exited", || is_dead(orca));
    assert!(!job.join(".tmp").exists(), "<job>/.tmp is removed after the sweep, not before: {text}");
    assert_eq!(lab.wait_child(wrapper).signal(), Some(libc::SIGTERM));
    let root = lab.root.clone();
    drop(lab);
    assert_no_process_left(&root);
}

#[test]
fn cancel_with_a_reused_sid_signals_nothing() {
    let mut lab = Lab::new();
    let job = lab.job("reused");
    // A session that is not the job's, sitting in the job dir (a debugging shell, say).
    let mut cmd = Command::new("sleep");
    cmd.arg("20").current_dir(&job).stdin(Stdio::null());
    new_session(&mut cmd);
    let leader = lab.spawn(cmd);
    let t = tracked(leader);
    // .started names that SID, with another start time: the number was reused.
    write_started(&job, leader, t.starttime + 1);

    let out = lab.cancel("cancel", &job);
    let text = stdout(&out);
    assert!(out.status.success(), "{text}");
    assert!(text.contains(&format!("sweep: skip sid {leader} reused")), "{text}");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!is_dead(t), "a process under a reused SID is never signalled");
}

#[test]
fn cancel_never_signals_its_own_session() {
    let mut lab = Lab::new();
    let job = lab.job("own");
    // A session leader in the job dir, with a member in the job dir, that runs cancel.sh itself:
    // cancel.sh's own session is the one .started names. The test runner's session is never
    // involved: the leader is setsid'd.
    let script = r#"sleep 20 & echo "$!" >member.pid
for _ in $(seq 500); do [ -e go ] && break; sleep 0.02; done
bash "$1" cancel "$2" "$3" >cancel.out 2>&1
wait"#;
    let mut cmd = Command::new("bash");
    cmd.arg("-c")
        .arg(script)
        .arg("leader")
        .arg(&lab.cancel)
        .arg(&job)
        .arg(&lab.root)
        .current_dir(&job)
        .env("PATH", lab.path_env())
        .stdin(Stdio::null());
    new_session(&mut cmd);
    let leader = lab.spawn(cmd);
    let leader_t = tracked(leader);
    let member = tracked(lab.tracked_pid(&job.join("member.pid")));
    write_started(&job, leader, leader_t.starttime);
    fs::write(job.join("go"), "").unwrap();

    wait_for("cancel.sh done", || {
        fs::read_to_string(job.join("cancel.out")).is_ok_and(|s| s.contains("done\n")) || is_dead(member)
    });
    let text = fs::read_to_string(job.join("cancel.out")).unwrap_or_default();
    assert!(text.contains("running: skip own session"), "{text}");
    assert!(!is_dead(member) && !is_dead(leader_t), "nothing in its own session is signalled");
}

/// The P4 header plus `rows`, as `tsp -l` prints it.
pub(super) fn tsp_listing(rows: &[String]) -> String {
    let header = TSP_L_P4.lines().next().unwrap();
    let mut text = format!("{header}\n");
    for row in rows {
        text.push_str(row);
        text.push('\n');
    }
    text
}

fn write_enqueued(job: &Path, socket: &Path, id: u32) {
    fs::write(job.join(".enqueued"), format!("socket={}\nid={id}\n", Lab::str(socket))).unwrap();
}

#[test]
fn cancel_removes_a_queued_task_only_for_a_verified_queued_row() {
    let mut lab = Lab::new();
    let sock = lab.root.join("s0.sock");
    lab.listen(&sock);
    let rows_file = PathBuf::from(format!("{}.rows", Lab::str(&sock)));

    // (job, .enqueued id, rows, expect `tsp -r`)
    let cases: Vec<(&str, u32, Box<dyn Fn(&str) -> Vec<String>>, bool)> = vec![
        ("verified", 4, Box::new(|j: &str| vec![queued_row_for(j)]), true),
        ("running-row", 3, Box::new(|j: &str| vec![running_row_for(j)]), false),
        ("other-job", 4, Box::new(|j: &str| vec![queued_row_for(&format!("{j}0"))]), false),
        ("other-id", 5, Box::new(|j: &str| vec![queued_row_for(j)]), false),
    ];
    for (name, id, rows, expect_remove) in cases {
        let job = lab.job(name);
        write_enqueued(&job, &sock, id);
        fs::write(&rows_file, tsp_listing(&rows(Lab::str(&job)))).unwrap();
        lab.clear_tsp_log();
        let out = lab.cancel("cancel", &job);
        assert!(out.status.success(), "{name}: {}", stdout(&out));
        let log = lab.tsp_log();
        assert!(log.contains(&format!("{} -l\n", Lab::str(&sock))), "{name}: {log}");
        assert_eq!(
            log.contains(&format!("{} -r {id}\n", Lab::str(&sock))),
            expect_remove,
            "{name}: tsp log {log:?}, output {}",
            stdout(&out)
        );
        assert!(job.join(".cancelled").exists());
    }
}

#[test]
fn cancel_with_no_daemon_never_runs_tsp() {
    let mut lab = Lab::new();
    let sock = lab.root.join("s0.sock");
    lab.stale_socket(&sock);
    let job = lab.job("no-daemon");
    write_enqueued(&job, &sock, 4);
    let out = lab.cancel("cancel", &job);
    assert!(stdout(&out).contains("queued: skip no daemon"), "{}", stdout(&out));
    assert_eq!(lab.tsp_log(), "", "tsp on a dead socket would start a daemon (probe 5.2b)");
}

// ---- the d′ race, real scripts in both sequential orders -----------------------------------

#[test]
fn dprime_cancel_before_the_wrapper_starts_orca_never_runs() {
    let mut lab = Lab::new();
    let job = lab.job("dprime-first");
    let out = lab.cancel("cancel", &job);
    assert!(out.status.success());
    let status = lab.run_wrapper(&job, &[]);
    assert!(status.success());
    // The wrapper published .started BEFORE it looked for .cancelled (start sequence 1 → 2).
    assert!(job.join(".started").exists(), "step 1 precedes the .cancelled check");
    assert_eq!(lab.orca_runs(), 0, "ORCA never runs under .cancelled");
    assert!(!job.join(".exit_code").exists());
    assert_eq!(lab.classify(&job, &[]), Outcome::Cancelled);
}

#[test]
fn dprime_cancel_after_the_wrapper_starts_kills_orca_without_an_exit_code() {
    let mut lab = Lab::new();
    let job = lab.job("dprime-second");
    let wrapper = running_wrapper(&mut lab, &job, "");
    let orca = tracked(fs::read_to_string(job.join("orca.pid")).unwrap().trim().parse().unwrap());
    assert_eq!(lab.orca_runs(), 1);
    let out = lab.cancel("cancel", &job);
    assert!(out.status.success(), "{}", stdout(&out));
    lab.wait_child(wrapper);
    wait_for("orca killed", || is_dead(orca));
    assert!(!job.join(".exit_code").exists());
    assert_eq!(lab.classify(&job, &[]), Outcome::Cancelled);
}

// ---- the collector -------------------------------------------------------------------------

#[test]
fn collector_round_trips_running_and_completed_jobs_through_classify() {
    let mut lab = Lab::new();
    let running = lab.job("running");
    running_wrapper(&mut lab, &running, "");
    assert_eq!(lab.classify(&running, &[]), Outcome::Running);

    let done = lab.job("completed");
    assert!(lab.run_wrapper(&done, &[]).success());
    let snap = lab.snapshot(&done, &[]).unwrap();
    assert_eq!(snap.exit_code.as_deref(), Some(&b"0\n"[..]));
    assert!(!snap.cancelled);
    assert_eq!(snap.started_first, snap.started_last);
    assert_eq!(lab.classify(&done, &[]), Outcome::Completed { late_cancel: false });
}

#[test]
fn collector_reads_tsp_only_on_a_listening_socket() {
    let mut lab = Lab::new();
    let live = lab.root.join("s0.sock");
    let stale = lab.root.join("s1.sock");
    lab.listen(&live);
    lab.stale_socket(&stale);
    let job = lab.job("queued");
    fs::write(format!("{}.rows", Lab::str(&live)), tsp_listing(&[queued_row_for(Lab::str(&job))])).unwrap();

    let snap = lab.snapshot(&job, &[live.clone(), stale.clone()]).unwrap();
    assert!(matches!(&snap.sockets[0].state, SocketState::Rows(rows) if rows.len() == 1));
    assert_eq!(snap.sockets[1].state, SocketState::NoDaemon);
    assert_eq!(lab.tsp_log(), format!("{} -l\n", Lab::str(&live)), "no tsp call on the dead socket");
    assert_eq!(lab.classify(&job, &[live.clone(), stale.clone()]), Outcome::Queued);

    // Only the dead socket: no rows anywhere, never started.
    lab.clear_tsp_log();
    assert_eq!(lab.classify(&job, &[stale.clone()]), Outcome::ReEnqueue);
    assert_eq!(lab.tsp_log(), "");
}

#[test]
fn collector_adds_the_enqueued_socket_and_reports_a_tsp_failure_as_an_error_fact() {
    let mut lab = Lab::new();
    let slot = lab.root.join("s0.sock");
    let old = lab.root.join("s9.sock");
    lab.stale_socket(&slot);
    lab.listen(&old);
    fs::write(format!("{}.fail", Lab::str(&old)), "").unwrap();
    let job = lab.job("enqueued");
    write_enqueued(&job, &old, 7);
    let snap = lab.snapshot(&job, &[slot.clone()]).unwrap();
    assert_eq!(snap.sockets.len(), 2);
    assert_eq!(snap.sockets[1].socket_path, Lab::str(&old));
    assert!(matches!(snap.sockets[1].state, SocketState::Error(_)));
    assert_eq!(lab.classify(&job, &[slot]), Outcome::Indeterminate);
}

#[test]
fn collector_reports_an_unparsable_enqueued_as_an_error_fact_not_as_no_rows() {
    let lab = Lab::new();
    let slot = lab.root.join("s0.sock");
    let job = lab.job("bad-enqueued");
    fs::write(job.join(".enqueued"), "socket=relative.sock\nid=4\n").unwrap();
    let snap = lab.snapshot(&job, &[slot.clone()]).unwrap();
    assert_eq!(snap.sockets.len(), 2);
    assert_eq!(snap.sockets[1].socket_path, format!("{}/.enqueued", Lab::str(&job)));
    assert!(matches!(snap.sockets[1].state, SocketState::Error(_)));
    // Never started, but the queue entry cannot be checked: no re-enqueue (row 10).
    assert_eq!(lab.classify(&job, &[slot]), Outcome::Indeterminate);
}

#[test]
fn collector_read_error_fails_the_whole_snapshot() {
    let mut lab = Lab::new();
    // An unreadable .exit_code (EACCES) is an error, never "no exit code yet".
    let job = lab.job("eacces-file");
    assert!(lab.run_wrapper(&job, &[]).success());
    fs::set_permissions(job.join(".exit_code"), fs::Permissions::from_mode(0o000)).unwrap();
    let result = lab.snapshot(&job, &[]);
    assert!(
        matches!(&result, Err(WireError::Collector(msg)) if msg.contains("Permission denied")),
        "{result:?}"
    );

    // A session member whose cwd is EACCES (PR_SET_DUMPABLE 0): an error, never "not in the job
    // session" — that member may be an MPI rank still holding cores.
    let job = lab.job("eacces-cwd");
    running_wrapper(&mut lab, &job, "nodump");
    wait_for("nodump ready", || job.join("nodump.ready").exists());
    let result = lab.snapshot(&job, &[]);
    assert!(
        matches!(&result, Err(WireError::Collector(msg)) if msg.contains("Permission denied")),
        "{result:?}"
    );
    // The cancel script stops too, before signalling anything.
    let out = lab.cancel("check", &job);
    assert_eq!(out.status.code(), Some(3), "{}", stdout(&out));
}

#[test]
fn collector_refuses_a_missing_job_dir() {
    let lab = Lab::new();
    let result = lab.snapshot(&lab.root.join("jobs").join("nope"), &[]);
    assert!(matches!(result, Err(WireError::Collector(_))), "{result:?}");
}

// ---- verifier Part B round: guards that each need their own test ---------------------------

#[test]
fn cancel_does_not_touch_a_started_from_another_boot() {
    let mut lab = Lab::new();
    let job = lab.job("other-boot");
    let t = session_sleeper(&mut lab, &job);
    // Exact pid and start time, another boot: start times count from boot, so nothing of ours.
    write_started_text(
        &job,
        &format!(
            "pid={0}\npgid={0}\nsid={0}\nboot_id={1}\nstarttime={2}\nstarted_at=1\n",
            t.pid,
            other_boot(),
            t.starttime
        ),
    );
    let out = lab.cancel("cancel", &job);
    let text = stdout(&out);
    assert!(text.contains("running: skip .started is from another boot"), "{text}");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!is_dead(t), "a process under a .started from another boot is never signalled");
}

#[test]
fn cancel_never_terms_the_group_of_a_process_that_is_not_ours() {
    let mut lab = Lab::new();
    let job = lab.job("not-ours");
    let elsewhere = lab.root.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    // Alive, leads its group, this boot — but its cmdline is `sleep 20`, not our wrapper, and its
    // cwd is not the job dir (so the sweep has nothing either).
    let t = session_sleeper(&mut lab, &elsewhere);
    write_started(&job, t.pid, t.starttime);
    let kv = check_map(&lab, &job);
    assert_eq!((kv["alive"].as_str(), kv["ours"].as_str(), kv["leader"].as_str()), ("yes", "no", "yes"));
    let out = lab.cancel("cancel", &job);
    let text = stdout(&out);
    assert!(text.contains("group: skip alive=yes ours=no leader=yes"), "{text}");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!is_dead(t), "a group whose leader is not our wrapper is never signalled");
}

#[test]
fn cancel_never_terms_a_group_the_wrapper_does_not_lead() {
    let mut lab = Lab::new();
    let job = lab.job("not-leader");
    // A perl session leader P whose child C execs the real wrapper WITHOUT its own group: C is
    // ours (argv shape) and alive, but P leads the group. The wrapper writes .started itself
    // (pid = C, pgid = sid = P). P sits outside the job dir, so the cwd-filtered sweep can never
    // reach it: only a group TERM could. (The sweep may still take C and ORCA — they are the job.)
    let parent_file = job.join("parent.pid");
    let elsewhere = lab.root.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let mut cmd = Command::new("perl");
    cmd.arg("-e")
        .arg(
            "my $p = fork // die; if ($p == 0) { exec @ARGV or die } \
             open my $f, '>', \"$ENV{PPID_FILE}.tmp\" or die; print $f \"$$\\n\"; close $f; \
             rename \"$ENV{PPID_FILE}.tmp\", $ENV{PPID_FILE} or die; sleep 20",
        )
        .arg("bash")
        .arg(&lab.wrapper)
        .arg(&job)
        .arg("0")
        .arg(lab.stub_dir.join("orca"))
        .env("PPID_FILE", &parent_file)
        .env("STUB_RAN_LOG", lab.ran_log())
        .env("STUB_ORCA_SLEEP", "20")
        .current_dir(&elsewhere)
        .stdin(Stdio::null());
    new_session(&mut cmd);
    lab.spawn(cmd);
    let parent = tracked(lab.tracked_pid(&parent_file));
    lab.tracked_pid(&job.join("orca.pid"));
    lab.tracked_pid(&job.join("sleep.pid"));
    let started = parse_started(&fs::read(job.join(".started")).unwrap()).unwrap();
    // The wrapper C is P's child, not ours: track it so Drop kills it too (otherwise it could
    // outlive Drop and write .exit_code into a dir being removed). Once P is killed, C is
    // re-parented to init (or the user's systemd) and reaped there, not by the test.
    lab.tracked.push(tracked(started.pid));
    assert_eq!((started.pgid, started.sid), (parent.pid, parent.pid), "the fixture's shape");
    assert_ne!(started.pid, started.pgid);

    let kv = check_map(&lab, &job);
    assert_eq!((kv["alive"].as_str(), kv["ours"].as_str(), kv["leader"].as_str()), ("yes", "yes", "no"));
    let out = lab.cancel("cancel", &job);
    let text = stdout(&out);
    assert!(text.contains("group: skip alive=yes ours=yes leader=no"), "{text}");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!is_dead(parent), "the group's real leader must never get the group TERM");
    let root = lab.root.clone();
    drop(lab);
    assert_no_process_left(&root);
}

#[test]
fn collector_skips_proc_for_a_started_from_another_boot_and_the_job_is_lost() {
    let mut lab = Lab::new();
    let job = lab.job("collect-other-boot");
    let t = session_sleeper(&mut lab, &job);
    let text = format!(
        "pid={0}\npgid={0}\nsid={0}\nboot_id={1}\nstarttime={2}\nstarted_at=1\n",
        t.pid,
        other_boot(),
        t.starttime
    );
    write_started_text(&job, &text);
    let slot = lab.root.join("slot.sock");
    let out = lab.collect(&job, std::slice::from_ref(&slot));
    assert!(out.status.success());
    let wire = String::from_utf8_lossy(&out.stdout);
    assert!(wire.contains("\nproc skipped\n"), "{wire}");
    assert!(!wire.contains("wrapper_stat"), "{wire}");
    assert_eq!(lab.classify(&job, &[slot]), Outcome::Lost { orphans: vec![] });
}

#[test]
fn shell_and_rust_agree_on_which_started_files_are_corrupt() {
    let lab = Lab::new();
    let job = lab.job("kv");
    // Another boot, so `check` stops after the parse and reads no /proc.
    let good = format!(
        "pid=4242\npgid=4242\nsid=4242\nboot_id={}\nstarttime=777\nstarted_at=1\n",
        other_boot()
    );
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("good", good.clone().into_bytes()),
        ("leading zeros", good.replace("starttime=777", "starttime=0777").into_bytes()),
        ("duplicate key", format!("{good}pid=4242\n").into_bytes()),
        ("unknown key", format!("{good}extra=1\n").into_bytes()),
        ("missing key", good.replace("sid=4242\n", "").into_bytes()),
        ("no final newline", good.trim_end().as_bytes().to_vec()),
        ("blank line", good.replace("\nsid", "\n\nsid").into_bytes()),
        ("CRLF", good.replace('\n', "\r\n").into_bytes()),
        ("NUL in a value", good.replace("pid=4242\npgid", "pid=4242\0\npgid").into_bytes()),
        ("empty", Vec::new()),
        ("zero pid", good.replace("pid=4242\npgid", "pid=0\npgid").into_bytes()),
        ("pid overflows u32", good.replace("pid=4242\npgid", "pid=4294967296\npgid").into_bytes()),
        ("uppercase boot id", good.replace(&other_boot(), &other_boot().to_uppercase()).into_bytes()),
    ];
    for (name, bytes) in cases {
        fs::write(job.join(".started"), &bytes).unwrap();
        let rust_ok = parse_started(&bytes).is_ok();
        let kv = check_map(&lab, &job);
        assert_eq!(kv["started"], if rust_ok { "ok" } else { "corrupt" }, "{name}");
    }
}

#[test]
fn wrapper_refuses_when_it_cannot_even_write_its_temp_file_and_started_exists() {
    let mut lab = Lab::new();
    let job = lab.job("read-only");
    assert!(lab.run_wrapper(&job, &[]).success());
    fs::remove_file(job.join(".exit_code")).unwrap();
    let started = fs::read(job.join(".started")).unwrap();
    // The temp-file write fails (a read-only dir) while a valid .started exists: refuse, never 97.
    fs::set_permissions(&job, fs::Permissions::from_mode(0o555)).unwrap();
    let status = lab.run_wrapper(&job, &[]);
    fs::set_permissions(&job, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(status.code(), Some(1), "refused, not the 97 self-check path");
    assert_eq!(fs::read(job.join(".started")).unwrap(), started);
    assert!(!job.join(".exit_code").exists());
    assert_eq!(lab.orca_runs(), 1);
}

#[test]
fn wrapper_rejects_a_bad_core_mask_before_writing_anything() {
    let mut lab = Lab::new();
    for (i, mask) in ["-p", "0-3x", "", "0,,1", "a"].into_iter().enumerate() {
        let job = lab.job(&format!("bad-mask-{i}"));
        let mut fixed = Command::new("bash");
        fixed
            .arg(&lab.wrapper)
            .arg(&job)
            .arg(mask)
            .arg(lab.stub_dir.join("orca"))
            .env("STUB_RAN_LOG", lab.ran_log())
            .stderr(Stdio::null());
        new_session(&mut fixed);
        let pid = lab.spawn(fixed);
        assert_eq!(lab.wait_child(pid).code(), Some(2), "mask {mask:?}");
        assert!(!job.join(".started").exists(), "mask {mask:?}: nothing written");
        assert!(!job.join(".exit_code").exists());
    }
    for (i, mask) in ["0", "0-3", "0,2,4-7", "12-23"].into_iter().enumerate() {
        let job = lab.job(&format!("ok-mask-{i}"));
        let mut fixed = Command::new("bash");
        fixed
            .arg(&lab.wrapper)
            .arg(&job)
            .arg(mask)
            .arg(lab.stub_dir.join("orca"))
            .env("STUB_RAN_LOG", lab.ran_log())
            .stderr(Stdio::null());
        new_session(&mut fixed);
        let pid = lab.spawn(fixed);
        let code = lab.wait_child(pid).code();
        // Whether taskset accepts it depends on the machine's CPUs; the mask passed validation.
        assert!(job.join(".started").exists(), "mask {mask:?} is valid ({code:?})");
    }
}

#[test]
fn cancel_removes_tmp_even_for_a_job_that_never_started() {
    let lab = Lab::new();
    let job = lab.job("never-started-tmp");
    fs::create_dir_all(job.join(".tmp").join("ompi.session")).unwrap();
    let out = lab.cancel("cancel", &job);
    assert!(out.status.success(), "{}", stdout(&out));
    assert!(stdout(&out).contains("running: skip .started absent"), "{}", stdout(&out));
    assert!(!job.join(".tmp").exists(), "<job>/.tmp goes at the end of every cancel");
}

#[test]
fn cwd_comparison_is_byte_exact_a_trailing_newline_is_another_dir() {
    let mut lab = Lab::new();
    let job = lab.job("nl");
    // A sibling dir whose name is the job dir's plus "\n": `$(<…)` would strip it and match.
    let twin = PathBuf::from(format!("{}\n", Lab::str(&job)));
    fs::create_dir_all(&twin).unwrap();
    fs::create_dir_all(lab.root.join("elsewhere")).unwrap();
    let pid = lab.spawn_wrapper(
        &job,
        &[("STUB_ORCA_SLEEP", "20"), ("STUB_ORCA_EXTRAS", "foreign"), ("STUB_FOREIGN_DIR", Lab::str(&twin))],
    );
    lab.tracked_pid(&job.join("orca.pid"));
    lab.tracked_pid(&job.join("sleep.pid"));
    let foreign = lab.tracked_pid(&job.join("foreign.pid"));
    let (v, _) = parity(&lab, &job);
    assert!(v.session.contains(&pid));
    assert!(!v.session.contains(&foreign), "a cwd of <job>\\n is not the job dir");
}

/// One path rule (ADR-024 l, Part B detail 4): the shipped shell `valid_path` and Rust
/// `is_valid_path` agree on every case, valid and invalid.
#[test]
fn shell_and_rust_share_one_path_rule() {
    let cases = [
        ("/home/anton/.orcastudio/jobs/j-1_a.b", true),
        ("/a", true),
        ("/a/...", true),
        ("/a/.hidden", true),
        ("", false),
        ("/", false),
        ("a/b", false),
        ("/a/", false),
        ("/a//b", false),
        ("//a", false),
        ("/a/./b", false),
        ("/a/../b", false),
        ("/a/.", false),
        ("/a/..", false),
        ("/..", false),
        ("/a b", false),
        ("/a\nb", false),
        ("/a$b", false),
    ];
    for (path, valid) in cases {
        assert_eq!(super::classify::is_valid_path(path), valid, "Rust: {path:?}");
        let out = Command::new("bash")
            .arg("-c")
            .arg(format!("{}\nvalid_path \"$1\"", include_str!("scripts/head.sh")))
            .arg("head")
            .arg(path)
            .output()
            .unwrap();
        assert_eq!(out.status.success(), valid, "shell: {path:?}");
    }
}

#[test]
fn composed_scripts_pass_bash_n() {
    let lab = Lab::new();
    for script in [&lab.wrapper, &lab.cancel, &lab.collect] {
        let out = Command::new("bash").arg("-n").arg(script).output().unwrap();
        assert!(out.status.success(), "{}: {}", script.display(), String::from_utf8_lossy(&out.stderr));
    }
}
