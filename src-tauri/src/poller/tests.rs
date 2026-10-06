//! The poller's core over a **fake** server: the status step's order of calls and its writes, the
//! fetch and its post-condition, the strikes, the live log end to end (parity with the local
//! replay, stale chunks under a blocked poll, the drain before the terminal status), and the
//! planner over the database's candidates. The real scripts run in `remote::backend_e2e_tests`.
//!
//! The fake asserts on **every** call that the database lock and the live-log mutex are free — a
//! step that held either across an ssh or rsync call fails the test instead of deadlocking it.

use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::{Barrier, Mutex};
use std::time::{Duration, Instant};

use super::plan::{candidates, plan, plan_inputs, sweep, Periods, PollerMemory, Step};
use super::*;
use crate::local_backend::read_convergence;
use crate::remote::poll::OUTPUT_HEADER as LOG_HEADER;
use crate::remote::scripts::{LABEL, LIST, POLL_LOG, RUN};
use crate::remote::ssh::{TransportError, SSH_PROGRAM};
use crate::remote::sync::{download_selects, list_dir, Digest, LIST_HEADER, RSYNC_PROGRAM};
use crate::ssh_backend::tests::{
    echo, exited, expected_coords, label_reply, ok, ran, rec, snapshot, Reply, World, CLEAN_TAIL, ENQUEUED, JOB,
    NOT_ON_SERVER,
};

const POLICY: FetchPolicy = FetchPolicy::SMALL_ONLY;

// ---- the world: a live remote job, its local dir and the server's job dir ---------------------

/// A [`World`] whose job is a remote `queued` job at the expected coordinates, with its local job
/// dir (the upload source: `input.inp`) and a directory standing in for the server's job dir.
struct Remote {
    w: World,
    in_flight: InFlight,
    memory: PollerMemory,
    live: std::sync::Arc<LiveLog>,
    sink: RecordingSink,
}

impl Remote {
    fn new() -> Remote {
        let w = World::new();
        let local = w.data_dir().join("jobs").join(JOB);
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("input.inp"), "! HF def2-SVP\n").unwrap();
        let c = expected_coords();
        w.db.lock()
            .unwrap()
            .execute(
                "UPDATE jobs SET status = 'queued', backend_id = 'p1', remote_host = ?1, remote_job_dir = ?2, \
                 remote_socket = ?3, job_dir = ?4",
                rusqlite::params![c.host, c.job_dir, c.socket, local.to_str().unwrap()],
            )
            .unwrap();
        let server = w.dir.join("server");
        std::fs::create_dir_all(&server).unwrap();
        std::fs::write(server.join("input.inp"), "! HF def2-SVP\n").unwrap();
        let (live, sink) = RecordingSink::with_live();
        Remote { w, in_flight: InFlight::default(), memory: PollerMemory::new(), live, sink }
    }

    fn local(&self) -> PathBuf {
        self.w.data_dir().join("jobs").join(JOB)
    }

    fn server(&self) -> PathBuf {
        self.w.dir.join("server")
    }

    /// The server's job dir after the job ran: these files, written by "the wrapper".
    fn finished(&self, output: &str, exit_code: &str) {
        let s = self.server();
        std::fs::write(s.join("output.out"), output).unwrap();
        std::fs::write(s.join("stderr.log"), "").unwrap();
        std::fs::write(s.join(".exit_code"), exit_code).unwrap();
        std::fs::write(s.join(".enqueued"), format!("socket={}\nid=0\n", expected_coords().socket)).unwrap();
        std::os::unix::fs::symlink("x", s.join(".submitting")).unwrap();
    }

    fn poller<'a>(&'a self, runner: &'a dyn CommandRunner) -> Poller<'a> {
        Poller {
            db: &self.w.db,
            runner,
            in_flight: &self.in_flight,
            memory: &self.memory,
            live: &self.live,
            sink: &self.sink,
            policy: POLICY,
        }
    }

    fn row(&self) -> (JobStatus, Option<String>) {
        let job = self.w.job();
        (job.status, job.error_message)
    }
}

// ---- the fake server ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    LabelCall,
    Collect,
    Download,
    List,
    PollLog,
}
use Kind::*;

fn kind_of(program: &str, args: &[String], stdin: &[u8], local: &str) -> (Kind, Vec<String>) {
    if program == RSYNC_PROGRAM {
        assert!(args.contains(&"--checksum".to_string()), "a download is --checksum");
        assert!(!args.iter().any(|a| a.starts_with("--partial") || a.starts_with("--delete")), "{args:?}");
        assert_eq!(args.last().map(String::as_str), Some(format!("{local}/").as_str()), "into the local job dir");
        return (Download, Vec::new());
    }
    assert_eq!(program, SSH_PROGRAM);
    assert_eq!(&args[args.len() - 4..], ["--", "uni", "bash", "-s"], "ssh runs on the recorded host");
    for (kind, script) in [(LabelCall, LABEL), (Collect, RUN), (List, LIST), (PollLog, POLL_LOG)] {
        if let Some(rest) = stdin.strip_prefix(script.as_bytes()) {
            let mut values: Vec<String> = rest.split(|b| *b == 0).map(|v| String::from_utf8(v.to_vec()).unwrap()).collect();
            assert_eq!(values.pop().as_deref(), Some(""));
            if kind == Collect {
                let coords = expected_coords();
                assert_eq!(values[1], "collect", "the poller runs only the collector through the trampoline");
                assert_eq!(&values[3..], [coords.job_dir.as_str(), coords.socket.as_str()], "the recorded socket");
            }
            return (kind, values);
        }
    }
    panic!("a script the poller never sends");
}

/// Answers each call with `respond`, records its kind, and checks — on every call — that neither
/// the database lock nor the live-log mutex is held, and (on a log poll) that the job's in-flight
/// guard is free: the log poll takes none (o16.5).
struct Fake<'a> {
    r: &'a Remote,
    respond: Box<dyn Fn(Kind, &[String]) -> Reply + Send + Sync + 'a>,
    calls: Mutex<Vec<Kind>>,
    local: String,
}

impl<'a> Fake<'a> {
    fn new(r: &'a Remote, respond: impl Fn(Kind, &[String]) -> Reply + Send + Sync + 'a) -> Self {
        Fake { r, respond: Box::new(respond), calls: Mutex::default(), local: r.local().to_str().unwrap().to_string() }
    }

    fn kinds(&self) -> Vec<Kind> {
        self.calls.lock().unwrap().clone()
    }
}

impl CommandRunner for Fake<'_> {
    fn run(&self, program: &str, args: &[String], stdin: &[u8], _timeout: Duration) -> Reply {
        let (kind, values) = kind_of(program, args, stdin, &self.local);
        assert!(self.r.w.db.0.try_lock().is_ok(), "the database lock is held across a {kind:?} call");
        assert!(self.r.live.is_unlocked(), "the live-log mutex is held across a {kind:?} call");
        if kind == PollLog {
            assert!(!self.r.in_flight.is_busy(JOB), "a log poll holds the job's in-flight guard");
        }
        self.calls.lock().unwrap().push(kind);
        (self.respond)(kind, &values)
    }
}

/// The trampoline's reply carrying collector output `snap`.
fn collected(values: &[String], snap: Vec<u8>) -> Reply {
    ran(values, 0, &snap, b"")
}

/// `rsync` down: every top-level file and symlink of `server` the filter selects, copied into
/// `local` (what the real rsync does, gate-tested in `remote::sync`).
fn download(server: &Path, local: &Path) -> Reply {
    for entry in std::fs::read_dir(server).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !download_selects(&name, POLICY) {
            continue;
        }
        let to = local.join(&name);
        let _ = std::fs::remove_file(&to);
        if entry.file_type().unwrap().is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(entry.path()).unwrap(), &to).unwrap();
        } else {
            std::fs::copy(entry.path(), &to).unwrap();
        }
    }
    ok(Vec::new())
}

/// The server's listing of `server`, in the `list` script's format.
fn listing(values: &[String], server: &Path) -> Reply {
    let entries: Vec<_> = list_dir(server, &|p| !p.contains('/') && download_selects(p, POLICY)).unwrap();
    let mut out = format!("{}entries {}\n", echo(LIST_HEADER, values), entries.len());
    for f in entries {
        out.push_str(&rec("entry", &f.name));
        match f.digest {
            Digest::Sha256(hex) => out.push_str(&format!("sha256 {hex}\n")),
            Digest::Symlink(target) => out.push_str(&rec("link", &target)),
        }
    }
    out.push_str("end\n");
    ok(out.into_bytes())
}

/// A `poll_log` reply over `log`, of which only the first `visible` bytes exist yet.
fn log_reply(values: &[String], log: &[u8], visible: usize) -> Reply {
    let (offset, cap): (usize, usize) = (values[1].parse().unwrap(), values[2].parse().unwrap());
    let size = visible.min(log.len());
    let bytes = if size > offset { &log[offset..size.min(offset + cap)] } else { &[][..] };
    let mut out = format!("{}size {size}\nbytes {}\n", echo(LOG_HEADER, values), bytes.len()).into_bytes();
    out.extend_from_slice(bytes);
    out.extend_from_slice(b"\nend\n");
    ok(out)
}

/// The finished job's server: label → the classifier, collect → `snap`, download and listing over
/// the server dir. A log poll is a test error unless the test answers it.
fn server<'a>(r: &'a Remote, snap: Vec<u8>) -> impl Fn(Kind, &[String]) -> Reply + Send + Sync + 'a {
    move |kind, values| match kind {
        LabelCall => label_reply(values, ENQUEUED),
        Collect => collected(values, snap.clone()),
        Download => download(&r.server(), &r.local()),
        List => listing(values, &r.server()),
        PollLog => panic!("no log poll expected"),
    }
}

fn completed() -> Vec<u8> {
    snapshot(false, Some("0\n"), Some(CLEAN_TAIL), (None, None))
}

const OUTPUT: &str = "line 1\nFINAL SINGLE POINT ENERGY      -76.026760\n                             ****ORCA TERMINATED NORMALLY****\nTOTAL RUN TIME: 0 days 0 hours 0 minutes 1 seconds 234 msec";

fn log_lines(events: &[Event]) -> Vec<String> {
    events.iter().flat_map(|e| match e { Event::Log(_, lines) => lines.clone(), _ => vec![] }).collect()
}

fn statuses(events: &[Event]) -> Vec<JobStatus> {
    events.iter().filter_map(|e| match e { Event::Status(_, s) => Some(*s), _ => None }).collect()
}

// ---- the status step -----------------------------------------------------------------------

/// The happy path: label → collect (`Completed`) → download → listing → `detect_completion` on the
/// downloaded files → one terminal write with the energy and wall time → `job:status`.
#[test]
fn a_completed_job_is_fetched_verified_and_finalised_from_the_downloaded_files() {
    let r = Remote::new();
    r.finished(OUTPUT, "0\n");
    let fake = Fake::new(&r, server(&r, completed()));
    let step = r.poller(&fake).status_step(JOB).unwrap();
    assert_eq!(step, StatusStep::Finalised { outcome: Outcome::Completed { late_cancel: false }, status: JobStatus::Completed, drain_error: None });
    assert_eq!(fake.kinds(), [LabelCall, Collect, Download, List]);
    let job = r.w.job();
    assert_eq!((job.status, job.error_message.as_deref()), (JobStatus::Completed, None));
    assert!(job.completed_at.is_some());
    assert_eq!(job.energy, Some(-76.026760), "the energy from the downloaded output, as the local finish path");
    assert_eq!(job.wall_time, Some(1.234));
    assert_eq!(std::fs::read_to_string(r.local().join("output.out")).unwrap(), OUTPUT, "the copy is the server's");
    assert_eq!(r.sink.events(), [Event::Status(JOB.into(), JobStatus::Completed)], "unwatched: no log events, the status last");
    assert!(!r.live.has_state(JOB));
    assert_eq!(r.memory.get(JOB), Default::default(), "forgotten once terminal");
    assert!(!r.in_flight.is_busy(JOB), "the guard is released");
}

/// MAIN RISK 1: the status is decided by `detect_completion` over the **downloaded** files, never by
/// the classifier's verdict. The classifier saw a clean tail (`Completed`), but the downloaded
/// `output.out` has no `ORCA TERMINATED NORMALLY` → `failed`, with a message. NEGATIVE CONTROL: write
/// `Completed` for a classifier `Completed` instead of `detect_completion`'s status → red.
#[test]
fn a_downloaded_output_without_normal_termination_fails_the_job() {
    let r = Remote::new();
    r.finished("SCF NOT CONVERGED\nabort\n", "0\n");
    let fake = Fake::new(&r, server(&r, completed()));
    let step = r.poller(&fake).status_step(JOB).unwrap();
    assert!(matches!(step, StatusStep::Finalised { status: JobStatus::Failed, .. }), "{step:?}");
    let (status, message) = r.row();
    assert_eq!(status, JobStatus::Failed);
    let message = message.unwrap();
    assert!(message.contains("did not terminate normally (exit code 0)") && message.contains("abort"), "{message}");
    assert_eq!(r.w.job().energy, None);
}

/// A non-zero exit is a fetching outcome too (its output is the debugging evidence): downloaded,
/// then `failed` with the exit code.
#[test]
fn a_failed_job_is_fetched_and_failed_with_its_exit_code() {
    let r = Remote::new();
    r.finished("ORCA finished by error termination in SCF\n", "1\n");
    let fake = Fake::new(&r, server(&r, snapshot(false, Some("1\n"), Some("x"), (None, None))));
    let step = r.poller(&fake).status_step(JOB).unwrap();
    assert!(matches!(&step, StatusStep::Finalised { outcome: Outcome::Failed { reason: FailReason::NonZeroExit { code: 1 } }, status: JobStatus::Failed, .. }), "{step:?}");
    assert_eq!(fake.kinds(), [LabelCall, Collect, Download, List]);
    assert!(r.row().1.unwrap().contains("exit code 1"));
    assert!(r.local().join("output.out").exists(), "the evidence came down");
}

/// MAIN RISK 1 / rule #9: a download whose listing disagrees (one corrupted byte after the rsync)
/// finalises nothing — the row stays `queued`, a strike is counted and shown, no `job:status` for a
/// terminal state. NEGATIVE CONTROL: drop `compare_download` from `fetch_remote` and the job is
/// finalised from the corrupted copy → red.
#[test]
fn a_download_that_fails_its_listing_finalises_nothing_and_counts_a_strike() {
    let r = Remote::new();
    r.finished(OUTPUT, "0\n");
    let base = server(&r, completed());
    let fake = Fake::new(&r, |kind, values| {
        let reply = base(kind, values);
        if kind == Download {
            std::fs::write(r.local().join("output.out"), OUTPUT.replace("line 1", "line X")).unwrap();
        }
        reply
    });
    let step = r.poller(&fake).status_step(JOB).unwrap();
    let StatusStep::FetchFailed { strikes, reason } = &step else { panic!("{step:?}") };
    assert_eq!(*strikes, 1);
    assert!(reason.contains("differing [\"output.out\"]"), "{reason}");
    let (status, message) = r.row();
    assert_eq!(status, JobStatus::Queued, "nothing is finalised");
    assert!(message.unwrap().contains("attempt 1 of 3"));
    assert_eq!(statuses(&r.sink.events()), [JobStatus::Queued], "the shown strike reloads the row; no terminal status");
    assert!(r.memory.get(JOB).fetching, "the log is final: no more polls (o16.6)");
}

/// An rsync that fails is a strike too, and the listing is not even asked.
#[test]
fn a_failed_rsync_is_a_strike() {
    let r = Remote::new();
    r.finished(OUTPUT, "0\n");
    let base = server(&r, completed());
    let fake = Fake::new(&r, |kind, values| if kind == Download { exited(23, vec![]) } else { base(kind, values) });
    let step = r.poller(&fake).status_step(JOB).unwrap();
    assert!(matches!(&step, StatusStep::FetchFailed { strikes: 1, reason } if reason.contains("rsync exited with 23")), "{step:?}");
    assert_eq!(fake.kinds(), [LabelCall, Collect, Download]);
    assert_eq!(r.row().0, JobStatus::Queued);
}

/// o item 4: after 3 failed fetches in a row the automatic fetches stop — the step makes no call —
/// and the reason is shown; a manual retry (holding the guard, as a command does) starts over and
/// can succeed. NEGATIVE CONTROL: drop the `strikes >= MAX_FETCH_STRIKES` check from `run_status`
/// and the 4th step calls the server again → red.
#[test]
fn three_failed_fetches_stop_the_automatic_fetch_until_a_manual_retry() {
    let r = Remote::new();
    r.finished(OUTPUT, "0\n");
    let healthy = std::sync::atomic::AtomicBool::new(false);
    let base = server(&r, completed());
    let fake = Fake::new(&r, |kind, values| {
        if kind == Download && !healthy.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(TransportError::Timeout { program: "rsync".into(), secs: 300 });
        }
        base(kind, values)
    });
    let poller = r.poller(&fake);
    for n in 1..=3 {
        assert!(matches!(poller.status_step(JOB).unwrap(), StatusStep::FetchFailed { strikes, .. } if strikes == n));
    }
    let message = r.row().1.unwrap();
    assert!(message.contains("failed 3 times in a row; automatic fetches stopped until a manual retry"), "{message}");
    let calls = fake.kinds().len();
    assert_eq!(poller.status_step(JOB).unwrap(), StatusStep::FetchStopped);
    assert_eq!(fake.kinds().len(), calls, "no call after the third strike");
    assert_eq!(r.row().0, JobStatus::Queued);

    healthy.store(true, std::sync::atomic::Ordering::SeqCst);
    let guard = r.in_flight.acquire(JOB).unwrap();
    let step = poller.retry_fetch(&guard).unwrap();
    assert!(matches!(step, StatusStep::Finalised { status: JobStatus::Completed, .. }), "{step:?}");
    assert_eq!(r.row(), (JobStatus::Completed, None));
}

/// o15.3: the poller skips a job another operation holds — no call, nothing written.
#[test]
fn a_busy_job_is_skipped() {
    let r = Remote::new();
    let fake = Fake::new(&r, server(&r, completed()));
    let _held = r.in_flight.acquire(JOB).unwrap();
    assert_eq!(r.poller(&fake).status_step(JOB).unwrap(), StatusStep::Busy);
    assert_eq!(fake.kinds(), []);
    assert_eq!(r.row(), (JobStatus::Queued, None));
}

/// o 3.4: a `queued` job that is not in the server's hands ends at the label call — the collector
/// never runs on a missing dir — and nothing is written.
#[test]
fn a_job_not_on_the_server_ends_at_the_label_call() {
    let r = Remote::new();
    r.w.sql("UPDATE jobs SET error_message = 'not submitted (Upload step): rsync exited with 23'");
    let fake = Fake::new(&r, |kind, values| match kind {
        LabelCall => label_reply(values, NOT_ON_SERVER),
        _ => panic!("nothing after the label call"),
    });
    assert_eq!(r.poller(&fake).status_step(JOB).unwrap(), StatusStep::Labelled(Label::NotOnServer));
    assert_eq!(fake.kinds(), [LabelCall]);
    assert_eq!(r.row().1.as_deref(), Some("not submitted (Upload step): rsync exited with 23"), "the attempt's reason stays");
    assert!(r.sink.events().is_empty());
}

/// A `running` row has started: no label call, straight to the collector.
#[test]
fn a_running_job_skips_the_label_call() {
    let r = Remote::new();
    r.w.sql("UPDATE jobs SET status = 'running'");
    r.finished(OUTPUT, "0\n");
    let fake = Fake::new(&r, server(&r, completed()));
    r.poller(&fake).status_step(JOB).unwrap();
    assert_eq!(fake.kinds(), [Collect, Download, List]);
}

/// A non-fetching outcome is shown and the row stays live; the same outcome again changes nothing
/// and emits nothing (`status_to_emit`).
#[test]
fn a_non_fetching_outcome_is_shown_and_the_row_stays_queued() {
    let r = Remote::new();
    let fake = Fake::new(&r, server(&r, snapshot(false, None, None, (None, None))));
    let poller = r.poller(&fake);
    assert_eq!(poller.status_step(JOB).unwrap(), StatusStep::Shown(Outcome::ReEnqueue));
    let (status, message) = r.row();
    assert_eq!(status, JobStatus::Queued);
    assert!(message.unwrap().contains("handled in unit 5.4"));
    assert_eq!(fake.kinds(), [LabelCall, Collect], "no download for a non-fetching outcome");
    assert_eq!(statuses(&r.sink.take()), [JobStatus::Queued]);
    poller.status_step(JOB).unwrap();
    assert!(r.sink.take().is_empty(), "nothing changed, nothing emitted");
    assert!(!r.memory.get(JOB).fetching);
}

/// The classifier's `Running` moves a `queued` row to `running` (o item 1), clears a stale message
/// and stamps `started_at`; `Queued` clears a stale message and keeps the status. A `running` row is
/// never moved back.
#[test]
fn running_and_queued_outcomes_update_the_row() {
    let r = Remote::new();
    r.w.sql("UPDATE jobs SET error_message = 'submit outcome unknown'");
    let fake = Fake::new(&r, |_, _| panic!("no call"));
    let poller = r.poller(&fake);
    let c = expected_coords();
    assert!(matches!(poller.record_shown(JOB, &c, &Outcome::Queued).unwrap(), Written::Yes(Some(JobStatus::Queued))));
    assert_eq!(r.row(), (JobStatus::Queued, None));
    assert!(matches!(poller.record_shown(JOB, &c, &Outcome::Running).unwrap(), Written::Yes(Some(JobStatus::Running))));
    assert_eq!(r.row(), (JobStatus::Running, None));
    assert!(r.w.job().started_at.is_some());
    assert!(matches!(poller.record_shown(JOB, &c, &Outcome::Queued).unwrap(), Written::Yes(None)), "never back to queued");
    assert_eq!(r.row().0, JobStatus::Running);
    // A finished row is no longer written.
    r.w.sql("UPDATE jobs SET status = 'cancelled'");
    assert!(matches!(poller.record_shown(JOB, &c, &Outcome::Lost { orphans: vec![] }).unwrap(), Written::No));
}

/// o17 (Anton): a classifier `Cancelled` (row 4) on a live remote row writes it terminal `cancelled`
/// as a withdraw does — `completed_at` stamped, the reason shown — with no download, the live state
/// dropped and one `job:status`. For a `queued` row (label call first) and a `running` one (no label
/// call). NEGATIVE CONTROL: send `Cancelled` back to the "shown" arm and both rows stay live → red.
#[test]
fn a_cancelled_outcome_writes_the_row_terminal_without_a_download() {
    for (from, calls) in [("queued", vec![LabelCall, Collect]), ("running", vec![Collect])] {
        let r = Remote::new();
        r.w.sql(&format!("UPDATE jobs SET status = '{from}'"));
        r.live.open(JOB, &r.sink);
        r.live.begin(JOB);
        let fake = Fake::new(&r, server(&r, snapshot(true, None, None, (None, None))));
        assert_eq!(r.poller(&fake).status_step(JOB).unwrap(), StatusStep::Cancelled, "{from}");
        assert_eq!(fake.kinds(), calls, "{from}: no rsync, no listing");
        let job = r.w.job();
        assert_eq!(job.status, JobStatus::Cancelled, "{from}");
        assert!(job.completed_at.is_some(), "{from}");
        assert!(job.error_message.unwrap().contains(".cancelled"), "{from}");
        assert_eq!(r.sink.events(), [Event::Status(JOB.into(), JobStatus::Cancelled)], "{from}: one status event");
        assert!(!r.live.has_state(JOB), "{from}: the live state is dropped");
    }
}

/// A row that changed while a fetch was failing is `RowChanged`: no strike counted, nothing written.
/// NEGATIVE CONTROL: count the strike before the write and `strikes` is 1 → red.
#[test]
fn a_failed_fetch_on_a_changed_row_counts_no_strike() {
    let r = Remote::new();
    r.finished(OUTPUT, "0\n");
    let base = server(&r, completed());
    let fake = Fake::new(&r, |kind, values| {
        if kind == Download {
            r.w.sql("UPDATE jobs SET status = 'cancelled', error_message = 'someone else'");
            return exited(23, vec![]);
        }
        base(kind, values)
    });
    assert_eq!(r.poller(&fake).status_step(JOB).unwrap(), StatusStep::RowChanged);
    assert_eq!(r.memory.get(JOB).strikes, 0);
    assert_eq!(r.row(), (JobStatus::Cancelled, Some("someone else".into())));
    assert!(r.sink.events().is_empty());
}

/// o16.6 / o17.2: a job watched (count > 0) whose state was never created — no log poll ran, e.g. a
/// view opened after launch on a job already finished — gets the whole downloaded copy from offset
/// 0, every line and the unterminated last one, with no `job:log-reset` (it holds nothing to drop),
/// before the terminal `job:status`. NEGATIVE CONTROL: make the drain return `NotWatched` for a job
/// without state → red (no lines).
#[test]
fn a_watched_job_without_state_gets_the_whole_copy_before_the_terminal_status() {
    let r = Remote::new();
    r.finished(OUTPUT, "0\n");
    r.live.open(JOB, &r.sink);
    assert!(!r.live.has_state(JOB), "no step has run for it");
    let fake = Fake::new(&r, server(&r, completed()));
    let step = r.poller(&fake).status_step(JOB).unwrap();
    assert!(matches!(step, StatusStep::Finalised { status: JobStatus::Completed, drain_error: None, .. }), "{step:?}");
    let events = r.sink.take();
    assert!(!events.contains(&Event::Reset(JOB.into())), "no reset is needed");
    let want: Vec<String> = OUTPUT.lines().map(String::from).collect();
    assert_eq!(log_lines(&events), want, "the whole copy from 0, the unterminated last line included");
    assert_eq!(events.last(), Some(&Event::Status(JOB.into(), JobStatus::Completed)));
    assert!(!r.live.has_state(JOB));
}

/// A row that turned terminal while the fetch ran (another writer won) is not overwritten.
#[test]
fn a_row_changed_during_the_fetch_is_not_overwritten() {
    let r = Remote::new();
    r.finished(OUTPUT, "0\n");
    let base = server(&r, completed());
    let fake = Fake::new(&r, |kind, values| {
        if kind == List {
            r.w.sql("UPDATE jobs SET status = 'cancelled', error_message = 'someone else'");
        }
        base(kind, values)
    });
    assert_eq!(r.poller(&fake).status_step(JOB).unwrap(), StatusStep::RowChanged);
    assert_eq!(r.row(), (JobStatus::Cancelled, Some("someone else".into())));
    assert!(r.sink.events().is_empty());
}

/// A finished, vanished or local job: nothing is called, its state and memory are dropped.
#[test]
fn a_job_that_is_no_longer_live_is_dropped_without_a_call() {
    for sql in ["UPDATE jobs SET status = 'completed'", "DELETE FROM jobs"] {
        let r = Remote::new();
        r.live.open(JOB, &r.sink);
        r.live.begin(JOB);
        r.memory.set_fetching(JOB);
        r.w.sql(sql);
        let fake = Fake::new(&r, |_, _| panic!("no call"));
        assert_eq!(r.poller(&fake).status_step(JOB).unwrap(), StatusStep::NotLive, "{sql}");
        assert!(!r.live.has_state(JOB) && !r.memory.get(JOB).fetching, "{sql}");
        assert_eq!(r.live.open_count(JOB), 1, "{sql}: the count stays");
    }
}

/// A label or collect that cannot be done changes nothing (the host may simply be away).
#[test]
fn a_failed_check_changes_nothing() {
    let r = Remote::new();
    let fake = Fake::new(&r, |_, _| Err(TransportError::Timeout { program: "ssh".into(), secs: 60 }));
    assert!(matches!(r.poller(&fake).status_step(JOB).unwrap(), StatusStep::CheckFailed(_)));
    assert_eq!(r.row(), (JobStatus::Queued, None));
    assert!(r.sink.events().is_empty());
}

// ---- the live log ------------------------------------------------------------------------------

/// The job's output, as a fixture builds it.
fn fixture_log() -> Vec<u8> {
    let dex = include_bytes!("../../tests/fixtures/dexketoprofen_output_tail.out");
    let opt = include_bytes!("../../tests/fixtures/opt_output_excerpt.txt");
    let ts = include_bytes!("../../tests/fixtures/optts_not_converged_tail.out");
    [&opt[..], &dex[..], &dex[..], &ts[..]].concat()
}

/// Stream `log` through the remote path — `log_step` over a fake whose file grows by `growth` bytes
/// per poll (`None`: whole from the start, so polls are `CAP`-sized) — then drain the local copy.
/// Returns every line and convergence point the sink got.
fn stream_remotely(log: &[u8], growth: Option<usize>) -> (Vec<String>, Vec<ConvergenceEvent>, bool) {
    let r = Remote::new();
    let copy = r.local().join("output.out");
    std::fs::write(&copy, log).unwrap();
    let visible = Mutex::new(growth.unwrap_or(log.len()));
    let fake = Fake::new(&r, |kind, values| {
        assert_eq!(kind, PollLog);
        let mut v = visible.lock().unwrap();
        let reply = log_reply(values, log, *v);
        *v += growth.unwrap_or(0);
        reply
    });
    r.live.open(JOB, &r.sink);
    let mut saw_catch_up = false;
    for _ in 0..log.len() {
        match log_step(&r.live, &fake, &r.sink, JOB, &expected_coords()) {
            LogStep::Applied(Applied::Lines { catch_up, .. }) => saw_catch_up |= catch_up,
            other => panic!("{other:?}"),
        }
        if r.live.begin(JOB).unwrap().offset == log.len() as u64 {
            break;
        }
    }
    let mut read = |offset| read_log_chunk(&copy, offset, POLL_LOG_MAX_BYTES);
    assert_eq!(r.live.drain(JOB, &mut read, POLL_LOG_MAX_BYTES, &r.sink).unwrap(), Drained::Done);
    let events = r.sink.take();
    let conv = events.iter().flat_map(|e| match e { Event::Convergence(_, c) => c.clone(), _ => vec![] }).collect();
    (log_lines(&events), conv, saw_catch_up)
}

/// o16.8 PARITY: a real ORCA output streamed through the remote path — in `CAP`-sized chunks and in
/// odd-sized ones — yields exactly the lines and convergence points the local replay
/// (`read_convergence`, `BufRead::lines`) gives on the same file. NEGATIVE CONTROL: recreate the
/// `ConvergenceParser` per chunk in `LiveLog::apply` and the odd-sized run goes red (a table split
/// across chunks loses its points).
#[test]
fn the_remote_stream_matches_the_local_replay() {
    let log = fixture_log();
    assert!(log.len() as u64 > POLL_LOG_MAX_BYTES, "the fixture must span more than one CAP-sized poll");
    let dir = std::env::temp_dir().join(format!("orcastudio-parity-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("output.out");
    std::fs::write(&path, &log).unwrap();
    let want_lines: Vec<String> = std::io::BufReader::new(std::fs::File::open(&path).unwrap()).lines().map(Result::unwrap).collect();
    let want_events = read_convergence(&path).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    assert!(want_events.iter().any(|e| matches!(e, ConvergenceEvent::Scf(_))), "the fixture has SCF points");
    assert!(want_events.iter().any(|e| matches!(e, ConvergenceEvent::Opt(_))), "the fixture has optimization points");

    let (lines, events, catch_up) = stream_remotely(&log, None);
    assert!(catch_up, "a CAP-sized chunk asks for catch-up");
    assert_eq!(lines, want_lines, "CAP-sized: lines");
    assert_eq!(events, want_events, "CAP-sized: convergence");
    for odd in [7919, 4093] {
        let (lines, events, _) = stream_remotely(&log, Some(odd));
        assert_eq!(lines, want_lines, "{odd}-byte growth: lines");
        assert_eq!(events, want_events, "{odd}-byte growth: convergence");
    }
}

/// o16.8: a chunk boundary inside a multi-byte character, over the fake ssh: the same lines as one
/// chunk.
#[test]
fn a_character_split_between_two_polls_is_emitted_whole() {
    let log = "Å ü →\nnext\n".as_bytes();
    let (lines, _, _) = stream_remotely(log, Some(1));
    assert_eq!(lines, ["Å ü →", "next"]);
}

/// Two barriers: the fake's `poll_log` reports it has the request, then waits to be released.
struct Gate {
    entered: Barrier,
    release: Barrier,
}

impl Gate {
    fn new() -> Gate {
        Gate { entered: Barrier::new(2), release: Barrier::new(2) }
    }
}

/// Run `act` while the fake's poll is blocked, then release it — also when `act` panics, so a red
/// assertion fails the test instead of leaving the polling thread at the barrier for ever.
fn while_blocked(gate: &Gate, act: impl FnOnce()) {
    gate.entered.wait();
    let acted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(act));
    gate.release.wait();
    if let Err(panic) = acted {
        std::panic::resume_unwind(panic);
    }
}

/// A fake whose first `poll_log` blocks inside the call (between `entered` and `release`) and
/// answers from `log`; later polls answer at once. Every requested offset is recorded.
fn blocking_log<'a>(gate: &'a Gate, log: &'a [u8], offsets: &'a Mutex<Vec<u64>>) -> impl Fn(Kind, &[String]) -> Reply + Send + Sync + 'a {
    move |kind, values| {
        assert_eq!(kind, PollLog);
        let first = offsets.lock().unwrap().is_empty();
        offsets.lock().unwrap().push(values[1].parse().unwrap());
        if first {
            gate.entered.wait();
            gate.release.wait();
        }
        log_reply(values, log, log.len())
    }
}

/// o16.8: an open issued while a poll is blocked in its ssh call makes the next poll ask for offset
/// 0, and no line of the stale chunk is emitted after the reset. NEGATIVE CONTROL: drop the
/// generation check from `LiveLog::apply` → red (the stale lines follow the reset).
#[test]
fn an_open_during_a_blocked_poll_discards_its_chunk() {
    let r = Remote::new();
    let (gate, offsets) = (Gate::new(), Mutex::new(Vec::new()));
    let log = b"one\ntwo\n";
    let fake = Fake::new(&r, blocking_log(&gate, log, &offsets));
    r.live.open(JOB, &r.sink);
    r.live.begin(JOB); // the state exists: the second open must reset it
    std::thread::scope(|s| {
        let polling = s.spawn(|| log_step(&r.live, &fake, &r.sink, JOB, &expected_coords()));
        while_blocked(&gate, || r.live.open(JOB, &r.sink));
        assert_eq!(polling.join().unwrap(), LogStep::Applied(Applied::Discarded));
    });
    assert_eq!(r.sink.take(), [Event::Reset(JOB.into())], "the reset, and no stale line after it");
    log_step(&r.live, &fake, &r.sink, JOB, &expected_coords());
    assert_eq!(*offsets.lock().unwrap(), [0, 0], "the next poll asks for offset 0");
    assert_eq!(log_lines(&r.sink.take()), ["one", "two"], "the re-stream");
}

/// o16.8 (ABA): open, a poll blocked, the last close, open again, the poll released — the stale
/// chunk is discarded although the job has state again. NEGATIVE CONTROL: start each job's
/// generations from its own 1 instead of the launch-long counter → red.
#[test]
fn close_and_reopen_during_a_blocked_poll_discards_its_chunk() {
    let r = Remote::new();
    let (gate, offsets) = (Gate::new(), Mutex::new(Vec::new()));
    let log = b"one\ntwo\n";
    let fake = Fake::new(&r, blocking_log(&gate, log, &offsets));
    r.live.open(JOB, &r.sink);
    std::thread::scope(|s| {
        let polling = s.spawn(|| log_step(&r.live, &fake, &r.sink, JOB, &expected_coords()));
        while_blocked(&gate, || {
            r.live.close(JOB);
            r.live.open(JOB, &r.sink);
            r.live.begin(JOB); // the new view's own first step has made the new state
        });
        assert_eq!(polling.join().unwrap(), LogStep::Applied(Applied::Discarded));
    });
    assert!(log_lines(&r.sink.take()).is_empty(), "no stale line");
}

/// o16.8: a failed poll leaves the offset and the state unchanged and emits nothing.
#[test]
fn a_failed_poll_changes_nothing() {
    let r = Remote::new();
    let fail = std::sync::atomic::AtomicBool::new(false);
    let fake = Fake::new(&r, |_, values| {
        if fail.load(std::sync::atomic::Ordering::SeqCst) {
            Err(TransportError::Timeout { program: "ssh".into(), secs: 10 })
        } else {
            log_reply(values, b"a\nb", 3)
        }
    });
    r.live.open(JOB, &r.sink);
    log_step(&r.live, &fake, &r.sink, JOB, &expected_coords());
    r.sink.take();
    fail.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(log_step(&r.live, &fake, &r.sink, JOB, &expected_coords()), LogStep::Failed(_)));
    assert_eq!(r.live.begin(JOB).map(|t| t.offset), Some(3));
    assert!(r.sink.take().is_empty());
    // A broken reply is a failed poll too (the post-condition refuses it).
    let fake = Fake::new(&r, |_, values| {
        // size 9 from offset 3 promises 6 bytes; 1 arrives.
        ok(format!("{}size 9\nbytes 1\nx\nend\n", echo(LOG_HEADER, values)).into_bytes())
    });
    let step = log_step(&r.live, &fake, &r.sink, JOB, &expected_coords());
    assert!(matches!(&step, LogStep::Failed(why) if why.contains("post-condition")), "{step:?}");
    assert!(r.sink.take().is_empty());
    assert_eq!(r.live.begin(JOB).map(|t| t.offset), Some(3));
}

/// o16.8: a log that shrank below the offset resets once; the next poll reads from 0.
#[test]
fn a_shrunken_log_resets_the_view_once() {
    let r = Remote::new();
    let content = Mutex::new(b"old line\n".to_vec());
    let fake = Fake::new(&r, |_, values| {
        let c = content.lock().unwrap();
        log_reply(values, &c, c.len())
    });
    r.live.open(JOB, &r.sink);
    log_step(&r.live, &fake, &r.sink, JOB, &expected_coords());
    r.sink.take();
    *content.lock().unwrap() = b"new\n".to_vec();
    assert_eq!(log_step(&r.live, &fake, &r.sink, JOB, &expected_coords()), LogStep::Applied(Applied::Reset));
    assert_eq!(r.sink.take(), [Event::Reset(JOB.into())]);
    log_step(&r.live, &fake, &r.sink, JOB, &expected_coords());
    assert_eq!(r.sink.take(), [Event::Log(JOB.into(), vec!["new".into()])]);
}

/// o16.6/o16.8: a watched job's last lines — and its unterminated last line — arrive only in the
/// downloaded copy, and are emitted before the terminal `job:status`. NEGATIVE CONTROL: emit the
/// status before the drain in `fetch_and_finalise` → red (lines after the status).
#[test]
fn the_tail_is_drained_from_the_copy_before_the_terminal_status() {
    let r = Remote::new();
    r.finished(OUTPUT, "0\n");
    let base = server(&r, completed());
    let fake = Fake::new(&r, |kind, values| match kind {
        PollLog => log_reply(values, OUTPUT.as_bytes(), "line 1\nFINAL".len()),
        _ => base(kind, values),
    });
    r.live.open(JOB, &r.sink);
    log_step(&r.live, &fake, &r.sink, JOB, &expected_coords());
    assert_eq!(log_lines(&r.sink.events()), ["line 1"], "the live stream got this far");
    let step = r.poller(&fake).status_step(JOB).unwrap();
    assert!(matches!(step, StatusStep::Finalised { status: JobStatus::Completed, drain_error: None, .. }), "{step:?}");
    let events = r.sink.take();
    let want: Vec<String> = OUTPUT.lines().map(String::from).collect();
    assert_eq!(log_lines(&events), want, "every line once, the unterminated last one included");
    assert_eq!(events.last(), Some(&Event::Status(JOB.into(), JobStatus::Completed)), "the status comes last");
    assert!(!r.live.has_state(JOB), "dropped after the drain");
    assert_eq!(fake.kinds(), [PollLog, LabelCall, Collect, Download, List]);
}

/// o16.8: an unwatched job's successful fetch reads no log chunk and emits no log line.
#[test]
fn an_unwatched_fetch_emits_no_log() {
    let r = Remote::new();
    r.finished(OUTPUT, "0\n");
    let fake = Fake::new(&r, server(&r, completed()));
    r.poller(&fake).status_step(JOB).unwrap();
    assert!(log_lines(&r.sink.events()).is_empty());
    assert!(!r.live.has_state(JOB), "no state was created to read into");
}

// ---- the planner over the database ---------------------------------------------------------

fn tick(r: &Remote, now: Instant) -> Vec<(String, Step)> {
    let c = candidates(&r.w.db.lock().unwrap()).unwrap().jobs;
    sweep(&c, &r.live, &r.memory);
    plan(&plan_inputs(&c, &r.live, &r.memory), &Periods::INITIAL, now)
}

/// Run `step` for the job at `now` as the loop will: marked in memory, then ended.
fn did(r: &Remote, step: Step, now: Instant) {
    assert!(r.memory.begin_step(JOB, step, now));
    r.memory.end_step(JOB);
}

/// o16.2/o16.8: a watched draft is not polled until it has coordinates; then its status step comes
/// first, then the log; a watched local job is never a candidate (no ssh).
#[test]
fn a_watched_draft_is_polled_once_it_has_coordinates_and_a_local_job_never() {
    let w = World::new(); // the draft
    let (live, memory, sink) = (LiveLog::new(), PollerMemory::new(), RecordingSink::default());
    live.open(JOB, &sink);
    let t0 = Instant::now();
    assert!(candidates(&w.db.lock().unwrap()).unwrap().jobs.is_empty(), "a draft has no coordinates");
    w.sql("UPDATE jobs SET status = 'queued'");
    assert!(candidates(&w.db.lock().unwrap()).unwrap().jobs.is_empty(), "a local queued job is not the poller's");
    drop((live, memory));

    let r = Remote::new();
    r.live.open(JOB, &r.sink);
    assert_eq!(tick(&r, t0), [(JOB.to_string(), Step::Status)], "status first");
    did(&r, Step::Status, t0);
    assert_eq!(tick(&r, t0 + Duration::from_secs(2)), [(JOB.to_string(), Step::Log)]);
}

/// o16.8: an unwatched job gets no log poll while its status step still runs; open, open, close
/// keeps polling; the last close stops it.
#[test]
fn only_a_watched_job_is_polled_and_its_status_step_always_runs() {
    let r = Remote::new();
    let t0 = Instant::now();
    did(&r, Step::Status, t0);
    for secs in [2, 4, 14] {
        assert!(tick(&r, t0 + Duration::from_secs(secs)).is_empty(), "unwatched at {secs} s");
    }
    assert_eq!(tick(&r, t0 + Duration::from_secs(15)), [(JOB.to_string(), Step::Status)]);
    did(&r, Step::Status, t0 + Duration::from_secs(15));
    r.live.open(JOB, &r.sink);
    r.live.open(JOB, &r.sink);
    r.live.close(JOB);
    assert_eq!(tick(&r, t0 + Duration::from_secs(17)), [(JOB.to_string(), Step::Log)], "open, open, close keeps polling");
    r.live.close(JOB);
    assert!(tick(&r, t0 + Duration::from_secs(19)).is_empty());
}

/// o16.6/o16.8: once a fetching outcome is classified the job's log is not polled again — here
/// the fetch failed, so the row is still live and watched.
#[test]
fn a_fetching_outcome_stops_the_log_polls() {
    let r = Remote::new();
    r.finished(OUTPUT, "0\n");
    let base = server(&r, completed());
    let fake = Fake::new(&r, |kind, values| if kind == Download { exited(12, vec![]) } else { base(kind, values) });
    r.live.open(JOB, &r.sink);
    let t0 = Instant::now();
    did(&r, Step::Status, t0);
    assert_eq!(tick(&r, t0 + Duration::from_secs(2)), [(JOB.to_string(), Step::Log)], "polled before");
    assert!(matches!(r.poller(&fake).status_step(JOB).unwrap(), StatusStep::FetchFailed { .. }));
    for secs in [4, 6, 10] {
        assert!(tick(&r, t0 + Duration::from_secs(secs)).is_empty(), "no log poll at {secs} s");
    }
}

/// o16.8: a terminal job's live state is dropped by the sweep, with its count kept.
#[test]
fn a_terminal_job_is_swept_with_its_count_kept() {
    let r = Remote::new();
    r.live.open(JOB, &r.sink);
    r.live.begin(JOB);
    r.w.sql("UPDATE jobs SET status = 'completed'");
    assert!(tick(&r, Instant::now()).is_empty());
    assert!(!r.live.has_state(JOB));
    assert_eq!(r.live.open_count(JOB), 1);
}

/// F-3: one row that does not read as a remote job (partial coordinates, written past the v20 CHECK)
/// is set aside and reported once per launch; the good job beside it still gets its step. NEGATIVE
/// CONTROL: make `candidates` fail on such a row (`?` instead of `unreadable`) → red.
#[test]
fn an_unreadable_row_does_not_stop_the_other_jobs() {
    let r = Remote::new();
    {
        let conn = r.w.db.lock().unwrap();
        conn.execute_batch("PRAGMA ignore_check_constraints = ON;").unwrap();
        conn.execute(
            "INSERT INTO jobs (id, title, input_content, status, remote_host, remote_job_dir) \
             VALUES ('bad', 't', '! HF', 'queued', 'uni', '/r/jobs/bad')",
            [],
        )
        .unwrap();
        conn.execute_batch("PRAGMA ignore_check_constraints = OFF;").unwrap();
    }
    let c = candidates(&r.w.db.lock().unwrap()).unwrap();
    assert_eq!(c.jobs.iter().map(|j| j.id.as_str()).collect::<Vec<_>>(), [JOB]);
    assert_eq!(c.unreadable.len(), 1);
    assert_eq!(c.unreadable[0].0, "bad");
    assert!(c.unreadable[0].1.contains("partial remote coordinates"), "{:?}", c.unreadable);
    let planned = ticked(&r, Instant::now());
    assert_eq!(planned.iter().map(|p| p.job_id.as_str()).collect::<Vec<_>>(), [JOB], "the good job is still polled");
    assert!(!r.memory.first_report("bad"), "reported once, by that tick");
}

/// A row with partial coordinates is an error, never a local job skipped in silence.
#[test]
fn candidates_refuse_partial_coordinates() {
    let r = Remote::new();
    // The v20 CHECK forbids the partial set in SQL; the Rust side reads the same rule.
    let job = r.w.job();
    let mut partial = job.clone();
    partial.remote_socket = None;
    assert!(coordinates(&partial).is_err());
    assert_eq!(candidates(&r.w.db.lock().unwrap()).unwrap().jobs.len(), 1);
}

#[test]
fn fetching_outcomes_are_exactly_item_4s() {
    let fetching = [
        Outcome::Completed { late_cancel: false },
        Outcome::Completed { late_cancel: true },
        Outcome::Failed { reason: FailReason::NonZeroExit { code: 1 } },
        Outcome::Failed { reason: FailReason::BadExitCode { detail: String::new() } },
        Outcome::Failed { reason: FailReason::NoNormalTermination },
    ];
    let shown = [
        Outcome::Queued,
        Outcome::Running,
        Outcome::Indeterminate,
        Outcome::ReEnqueue,
        Outcome::Lost { orphans: vec![] },
        Outcome::Cancelling,
        Outcome::Cancelled,
        Outcome::Failed { reason: FailReason::CorruptStarted { detail: String::new() } },
        Outcome::Failed { reason: FailReason::WrapperNeverStarted },
    ];
    assert!(fetching.iter().all(is_fetching));
    assert!(!shown.iter().any(is_fetching));
    for o in &shown {
        assert_eq!(shown_message(o).is_none(), matches!(o, Outcome::Queued | Outcome::Running), "{o:?}");
    }
    assert!(shown_message(&Outcome::Cancelling).unwrap().contains("handled in unit 5.4"));
}

// ---- the loop's body: tick and run_step (unit 5.3 B2 Part B) --------------------------------

use super::run::{run_step, tick as run_tick, Planned, StepRan};

fn ticked(r: &Remote, at: Instant) -> Vec<Planned> {
    run_tick(&r.w.db, &r.live, &r.memory, &Periods::INITIAL, at).unwrap()
}

/// A due status step is handed out once, marked in progress; while it is not ended no later tick
/// hands out another step for the job, however much time passes; `run_step` ends it, and the next
/// one is due a status period later.
#[test]
fn a_tick_hands_out_a_due_status_step_once_and_run_step_ends_it() {
    let r = Remote::new();
    let fake = Fake::new(&r, |kind, values| match kind {
        LabelCall => label_reply(values, NOT_ON_SERVER),
        _ => panic!("nothing after the label call"),
    });
    let t0 = Instant::now();
    let planned = ticked(&r, t0);
    assert_eq!(planned, [Planned { job_id: JOB.into(), step: Step::Status, coords: expected_coords() }]);
    assert!(r.memory.get(JOB).in_progress);
    for later in [0, 2, 20, 60] {
        assert!(ticked(&r, t0 + Duration::from_secs(later)).is_empty(), "a step is running: nothing more at +{later} s");
    }
    let ran = run_step(&r.poller(&fake), &planned[0]);
    assert!(matches!(ran, StepRan::Status(Ok(StatusStep::Labelled(Label::NotOnServer)))), "{ran:?}");
    assert!(!r.memory.get(JOB).in_progress, "ended");
    assert!(ticked(&r, t0 + Duration::from_secs(14)).is_empty());
    assert_eq!(ticked(&r, t0 + Duration::from_secs(15)).len(), 1, "the next status step is due a period later");
}

/// A step that panics still ends: the job is planned again, and its in-flight guard is free.
/// NEGATIVE CONTROL: end the step after the match instead of through the drop guard (so a panic
/// skips it) → red.
#[test]
fn a_step_that_panics_still_ends() {
    let r = Remote::new();
    let fake = Fake::new(&r, |_, _| panic!("a bug inside a step"));
    let t0 = Instant::now();
    let planned = ticked(&r, t0);
    let poller = r.poller(&fake);
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_step(&poller, &planned[0])));
    assert!(unwound.is_err(), "the step panicked");
    assert!(!r.memory.get(JOB).in_progress, "the in-progress mark is cleared while the panic unwinds");
    assert!(!r.in_flight.is_busy(JOB), "the guard too");
    assert_eq!(ticked(&r, t0 + Duration::from_secs(15)).len(), 1, "the job is planned again");
}

/// A watched remote job gets log steps between its status steps; an unwatched one does not.
#[test]
fn a_tick_gives_a_watched_job_log_steps_and_an_unwatched_one_none() {
    let r = Remote::new();
    let fake = Fake::new(&r, |kind, values| match kind {
        LabelCall => label_reply(values, NOT_ON_SERVER),
        PollLog => log_reply(values, b"line 1\n", 7),
        _ => panic!("unexpected {kind:?}"),
    });
    let t0 = Instant::now();
    run_step(&r.poller(&fake), &ticked(&r, t0)[0]);
    assert!(ticked(&r, t0 + Duration::from_secs(2)).is_empty(), "unwatched: no log step");
    r.live.open(JOB, &r.sink);
    let planned = ticked(&r, t0 + Duration::from_secs(2));
    assert_eq!(planned.iter().map(|p| p.step).collect::<Vec<_>>(), [Step::Log]);
    let ran = run_step(&r.poller(&fake), &planned[0]);
    assert!(matches!(ran, StepRan::Log(LogStep::Applied(Applied::Lines { lines: 1, .. }))), "{ran:?}");
    assert!(!r.memory.get(JOB).in_progress);
    assert_eq!(log_lines(&r.sink.take()), ["line 1"]);
}
