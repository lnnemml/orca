//! The core over a **fake** runner: the order of the calls, what the database holds at each call
//! and after each failure point, the refusals and retry. The real scripts run end to end
//! in `remote::backend_e2e_tests`.

use std::cell::RefCell;
use std::sync::Mutex;

use super::*;
use crate::db::init_db;
use crate::remote::prepare::tests::{entry, prepare_wire, ready_body};
use crate::remote::prepare::{INSTALL_HEADER, PREPARE_HEADER};
use crate::remote::run::{MKJOB_HEADER, RUN_HEADER};
use crate::remote::ssh::TransportError;
use crate::remote::submit::{LABEL_HEADER, OUTPUT_HEADER};
use crate::remote::wire::HEADER as SNAPSHOT_HEADER;

pub(crate) type Reply = Result<ProcessOutput, TransportError>;

pub(crate) const ROOT: &str = "/home/anton/.orcastudio";
pub(crate) const JOB: &str = "j1";
const PROFILE: &str = "p1";

// ---- the world: a database with a stamped profile and a draft ------------------------------

pub(crate) struct World {
    pub(crate) db: DbState,
    pub(crate) dir: PathBuf,
    /// Where an attempt's `job:status` goes (a write of the pending flag, o23.5).
    pub(crate) sink: crate::poller::RecordingSink,
}

impl World {
    pub(crate) fn new() -> World {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "orcastudio-sshbackend-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::remove_dir_all(&dir).ok();
        let conn = init_db(&dir.join("db")).unwrap();
        conn.execute(
            "INSERT INTO server_profiles (id, name, host, remote_orca_path, remote_scratch_dir, core_mask, \
             orca_version, core_count, verified_at) \
             VALUES (?1, 'uni', 'uni', '/opt/orca/orca', ?2, '0-23', '6.1.1', 48, datetime('now'))",
            params![PROFILE, ROOT],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO jobs (id, title, input_content, status) VALUES (?1, 'water', '! HF def2-SVP\n', 'draft')",
            params![JOB],
        )
        .unwrap();
        World { db: DbState(Mutex::new(conn)), dir, sink: crate::poller::RecordingSink::default() }
    }

    pub(crate) fn data_dir(&self) -> PathBuf {
        self.dir.join("data")
    }

    pub(crate) fn job(&self) -> Job {
        get_job_conn(&self.db.lock().unwrap(), JOB).unwrap()
    }

    fn stamped(&self) -> bool {
        get_profile_conn(&self.db.lock().unwrap(), PROFILE).unwrap().verified_at.is_some()
    }

    pub(crate) fn sql(&self, sql: &str) {
        self.db.lock().unwrap().execute_batch(sql).unwrap();
    }

    fn submit(&self, runner: &dyn CommandRunner) -> Result<SubmitOutcome, AppError> {
        self.attempt(runner).map(|a| a.outcome)
    }

    fn attempt(&self, runner: &dyn CommandRunner) -> Result<SubmitAttempt, AppError> {
        submit_remote(&self.db, runner, &self.sink, &self.data_dir(), JOB, PROFILE)
    }

    fn resubmit(&self, runner: &dyn CommandRunner) -> Result<SubmitOutcome, AppError> {
        resubmit_remote(&self.db, runner, &self.sink, &self.data_dir(), JOB).map(|a| a.outcome)
    }
}

impl Drop for World {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

pub(crate) fn expected_coords() -> RemoteCoordinates {
    RemoteCoordinates {
        host: "uni".into(),
        job_dir: format!("{ROOT}/jobs/{JOB}"),
        socket: format!("{ROOT}/tsp/slot0.sock"),
    }
}

/// The row as a call saw it: status, coordinates, error message.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RowState {
    status: JobStatus,
    coords: Option<RemoteCoordinates>,
    error: Option<String>,
}

fn row(job: &Job) -> RowState {
    RowState { status: job.status, coords: coordinates(job).unwrap(), error: job.error_message.clone() }
}

// ---- the fake runner ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Prepare,
    Install,
    Upload,
    Submit,
    Label,
    Mkjob,
    /// The trampoline (`run.sh`).
    Run,
}
use Kind::*;

fn kind_of(program: &str, stdin: &[u8]) -> (Kind, Vec<String>) {
    if program == RSYNC_PROGRAM {
        return (Upload, Vec::new());
    }
    assert_eq!(program, SSH_PROGRAM);
    for (kind, script) in [(Prepare, PREPARE), (Install, INSTALL), (Submit, SUBMIT), (Label, LABEL), (Mkjob, MKJOB), (Run, RUN)] {
        if let Some(rest) = stdin.strip_prefix(script.as_bytes()) {
            let mut values: Vec<String> = rest.split(|b| *b == 0).map(|v| String::from_utf8(v.to_vec()).unwrap()).collect();
            assert_eq!(values.pop().as_deref(), Some(""), "every value ends with a NUL");
            return (kind, values);
        }
    }
    panic!("a script the core never sends");
}

/// Answers each call with `respond(kind, values)` and records, for each call, its kind and the
/// row as it was at that moment. The core must hold no database lock during a call: the record
/// uses `try_lock`, so a held lock fails the test instead of deadlocking it.
struct Fake<'a> {
    db: &'a DbState,
    respond: Box<dyn Fn(Kind, &[String]) -> Reply + 'a>,
    calls: RefCell<Vec<(Kind, RowState)>>,
}

impl<'a> Fake<'a> {
    fn new(world: &'a World, respond: impl Fn(Kind, &[String]) -> Reply + 'a) -> Self {
        Fake { db: &world.db, respond: Box::new(respond), calls: RefCell::default() }
    }

    fn kinds(&self) -> Vec<Kind> {
        self.calls.borrow().iter().map(|(k, _)| *k).collect()
    }

    fn row_at(&self, i: usize) -> RowState {
        self.calls.borrow()[i].1.clone()
    }
}

impl CommandRunner for Fake<'_> {
    fn run(&self, program: &str, args: &[String], stdin: &[u8], _timeout: Duration) -> Reply {
        let (kind, values) = kind_of(program, stdin);
        if kind == Upload {
            assert!(!args.iter().any(|a| a.starts_with("--delete")), "never --delete");
            assert!(args.contains(&"--checksum".to_string()));
        } else {
            assert_eq!(&args[args.len() - 4..], ["--", "uni", "bash", "-s"], "ssh runs on the recorded host");
        }
        let conn = self.db.0.try_lock().expect("the database lock is held across an ssh call");
        let state = row(&get_job_conn(&conn, JOB).unwrap());
        drop(conn);
        self.calls.borrow_mut().push((kind, state));
        (self.respond)(kind, &values)
    }
}

pub(crate) fn exited(code: i32, stdout: Vec<u8>) -> Reply {
    Ok(ProcessOutput { code: Some(code), stdout, stderr: b"stderr text".to_vec() })
}

pub(crate) fn ok(stdout: Vec<u8>) -> Reply {
    exited(0, stdout)
}

pub(crate) fn rec(name: &str, text: &str) -> String {
    format!("{name} {}\n{text}\n", text.len())
}

/// The header and the echo of `values`, as every script prints them.
pub(crate) fn echo(header: &str, values: &[String]) -> String {
    let mut out = format!("{header}\nargc {}\n", values.len());
    for v in values {
        out.push_str(&rec("arg", v));
    }
    out
}

fn prepare_args(values: &[String]) -> PrepareArgs {
    PrepareArgs {
        root: values[0].clone(),
        job_dir: values[1].clone(),
        shas: [values[2].clone(), values[3].clone(), values[4].clone()],
    }
}

pub(crate) fn prepare_ready(values: &[String]) -> Reply {
    let sent = prepare_args(values);
    ok(prepare_wire(&sent, &ready_body(&sent)))
}

pub(crate) fn prepare_fresh(values: &[String]) -> Reply {
    let body = ["root", "jobs", "job", "bin", "tsp", "wrapper", "cancel", "collect"].map(|n| entry(n, None)).concat();
    ok(prepare_wire(&prepare_args(values), &body))
}

/// Ready, except that the wrapper's bytes are not the ones its name carries.
fn prepare_wrong_wrapper(values: &[String]) -> Reply {
    let sent = prepare_args(values);
    let body = ready_body(&sent).replace(&format!("sha256 {}", sent.shas[0]), &format!("sha256 {}", "0".repeat(64)));
    ok(prepare_wire(&sent, &body))
}

/// `<root>/bin` reached through a symlink: refused by Rust's check, whatever the wrapper.
pub(crate) fn prepare_symlinked_bin(values: &[String]) -> Reply {
    let sent = prepare_args(values);
    let bin = format!("{}/bin", sent.root);
    let body = ready_body(&sent).replace(
        &entry("bin", Some(("directory", Some(&bin)))),
        &entry("bin", Some(("symbolic link", Some("/data/bin")))),
    );
    ok(prepare_wire(&sent, &body))
}

pub(crate) fn install_ok(values: &[String]) -> Reply {
    ok(format!("{}wrapper installed\ncancel installed\ncollect kept\nend\n", echo(INSTALL_HEADER, values)).into_bytes())
}

pub(crate) fn submit_reply(values: &[String], outcome: &str) -> Reply {
    ok(format!("{}{outcome}end\n", echo(OUTPUT_HEADER, values)).into_bytes())
}

/// A `blocked` outcome record: one pinned process on the mask whose cwd is `/elsewhere`.
pub(crate) fn blocked_record() -> String {
    blocked_by(&[(4242, "/elsewhere")])
}

/// A `blocked` outcome record naming pinned processes `(pid, cwd)`.
pub(crate) fn blocked_by(blockers: &[(u32, &str)]) -> String {
    let records: String = blockers.iter().map(|(pid, cwd)| format!("process {pid}\nkind pinned\n{}{}", rec("cores", "0-23"), rec("job", cwd))).collect();
    format!("blocked {}\n{records}", blockers.len())
}

fn kup_record(rc: u8, stdout: &str) -> String {
    let ev = format!("rc {rc}\n{}{}", rec("stdout", stdout), rec("stderr", ""));
    format!("refused-kup {}\n{ev}\n", ev.len())
}

const NET_UNIX_HEADER: &str = "Num       RefCount Protocol Flags    Type St Inode Path\n";

/// A label reply: `dir no`, or the dir with these markers and no daemon on the socket.
pub(crate) fn label_reply(values: &[String], markers: Option<[bool; 5]>) -> Reply {
    let body = match markers {
        None => "dir no\n".to_string(),
        Some(m) => {
            let names = ["started", "exit_code", "cancelled", "enqueued", "submitting"];
            let marks: String = names.iter().zip(m).map(|(n, on)| format!("{n} {}\n", if on { "yes" } else { "no" })).collect();
            format!("dir yes\n{marks}{}nodaemon\n", rec("net_unix", NET_UNIX_HEADER))
        }
    };
    ok(format!("{}{body}end\n", echo(LABEL_HEADER, values)).into_bytes())
}

pub(crate) const NOT_ON_SERVER: Option<[bool; 5]> = None;
pub(crate) const INTERRUPTED: Option<[bool; 5]> = Some([false, false, false, false, true]);
pub(crate) const ENQUEUED: Option<[bool; 5]> = Some([false, false, false, true, true]);

/// The happy server: ready, upload ok, the submit call enqueues with id 3.
fn happy(kind: Kind, values: &[String]) -> Reply {
    match kind {
        Prepare => prepare_ready(values),
        Install => install_ok(values),
        Upload => ok(Vec::new()),
        Submit => submit_reply(values, "enqueued 3\n"),
        Label => label_reply(values, NOT_ON_SERVER),
        Mkjob => mkjob_ok(values),
        Run => match values[1].as_str() {
            "cancel" => ran(values, 0, b"", b""),
            _ => ran(values, 0, &snapshot(true, None, None, (None, None)), b""),
        },
    }
}

/// A mkjob reply whose realpaths are the paths themselves.
pub(crate) fn mkjob_ok(values: &[String]) -> Reply {
    let body = format!("{}{}", rec("job", &values[1]), rec("parent", &format!("{}/jobs", values[0])));
    ok(format!("{}{body}end\n", echo(MKJOB_HEADER, values)).into_bytes())
}

/// A trampoline reply: the script ran with `rc` and these streams.
pub(crate) fn ran(values: &[String], rc: u8, stdout: &[u8], stderr: &[u8]) -> Reply {
    let mut out = format!("{}ran\nrc {rc}\nstdout {}\n", echo(RUN_HEADER, values), stdout.len()).into_bytes();
    out.extend_from_slice(stdout);
    out.extend_from_slice(format!("\nstderr {}\n", stderr.len()).as_bytes());
    out.extend_from_slice(stderr);
    out.extend_from_slice(b"\nend\n");
    ok(out)
}

fn queued_with_coords(state: &RowState) -> bool {
    state.status == JobStatus::Queued && state.coords == Some(expected_coords())
}

// ---- submit ----------------------------------------------------------------------------------

#[test]
fn a_submit_persists_the_coordinates_before_any_ssh_then_prepares_uploads_and_submits() {
    let w = World::new();
    let fake = Fake::new(&w, happy);

    assert_eq!(w.submit(&fake).unwrap(), SubmitOutcome::Enqueued { tsp_id: 3 });

    assert_eq!(fake.kinds(), [Prepare, Upload, Submit], "a ready server needs no install");
    assert!(queued_with_coords(&fake.row_at(0)), "the coordinates were persisted before the first ssh: {:?}", fake.row_at(0));
    let job = w.job();
    assert_eq!(row(&job), RowState { status: JobStatus::Queued, coords: Some(expected_coords()), error: None });
    assert_eq!(job.backend_id.as_deref(), Some(PROFILE));
    let local = w.data_dir().join("jobs").join(JOB);
    assert_eq!(job.job_dir.as_deref(), local.to_str(), "the local dir is the upload source");
    assert_eq!(std::fs::read_to_string(local.join("input.inp")).unwrap(), "! HF def2-SVP\n%pal nprocs 24 end\n", "the input, %pal aligned to the 0-23 mask (o 14.2)");
    assert_eq!(job.input_content, "! HF def2-SVP\n", "the database keeps the original");
    assert!(w.stamped());
}

#[test]
fn a_fresh_server_is_installed_then_prepared_again_before_the_upload() {
    let w = World::new();
    let prepares = RefCell::new(0);
    let fake = Fake::new(&w, |kind, values| match kind {
        Prepare => {
            *prepares.borrow_mut() += 1;
            if *prepares.borrow() == 1 { prepare_fresh(values) } else { prepare_ready(values) }
        }
        _ => happy(kind, values),
    });
    assert_eq!(w.submit(&fake).unwrap(), SubmitOutcome::Enqueued { tsp_id: 3 });
    assert_eq!(fake.kinds(), [Prepare, Install, Prepare, Upload, Submit]);
}

/// NEGATIVE CONTROL target (f): the install's own answer is not trusted — the prepare call after
/// it must find the wrapper hashing right. Skip that second prepare call and this goes red (the
/// upload and the submit call run).
#[test]
fn an_install_that_leaves_the_wrapper_wrong_uploads_nothing() {
    let w = World::new();
    let fake = Fake::new(&w, |kind, values| match kind {
        Prepare => prepare_wrong_wrapper(values),
        _ => happy(kind, values),
    });
    let outcome = w.submit(&fake).unwrap();
    assert!(matches!(&outcome, SubmitOutcome::NotClaimed { step: SubmitStep::Install, refusal: Refusal::Other(reason) } if reason.contains("not hashing right [\"wrapper\"]")), "{outcome:?}");
    assert_eq!(fake.kinds(), [Prepare, Install, Prepare], "no upload, no submit call");
    assert!(queued_with_coords(&row(&w.job())));
}

/// NEGATIVE CONTROL target (a): the first ssh call fails, and the row it leaves is the one the
/// label call can resolve — `queued` with the coordinates — because they were persisted before it.
/// Move the persist step after the prepare call and this goes red (the row stays a draft without
/// coordinates while the server may already have been touched).
#[test]
fn a_failure_at_the_first_ssh_leaves_a_recoverable_row() {
    let w = World::new();
    let fake = Fake::new(&w, |_, _| Err(TransportError::Timeout { program: "ssh".into(), secs: 30 }));
    let outcome = w.submit(&fake).unwrap();
    assert!(matches!(outcome, SubmitOutcome::NotClaimed { step: SubmitStep::Prepare, .. }), "{outcome:?}");
    assert_eq!(fake.kinds(), [Prepare]);
    assert!(queued_with_coords(&fake.row_at(0)), "persisted before the first ssh: {:?}", fake.row_at(0));
    let after = row(&w.job());
    assert!(queued_with_coords(&after), "{after:?}");
    assert!(after.error.as_deref().is_some_and(|e| e.contains("timed out") || e.contains("did not finish")), "{after:?}");
}

/// Every failure point after the persist step: the row stays `queued` with its coordinates and
/// carries the failure; nothing after the failing step runs; the stamp is kept (only `RefusedKup`
/// clears it, next test).
#[test]
fn every_failure_point_leaves_a_queued_row_with_coordinates() {
    type Respond = fn(Kind, &[String]) -> Reply;
    let cases: &[(&str, Respond, &[Kind], fn(&SubmitOutcome) -> bool)] = &[
        ("prepare: ssh exit 255", |k, v| if k == Prepare { exited(255, vec![]) } else { happy(k, v) }, &[Prepare],
            |o| matches!(o, SubmitOutcome::NotClaimed { step: SubmitStep::Prepare, refusal: Refusal::Other(reason) } if reason.contains("exit 255"))),
        ("prepare: an error record", |k, v| if k == Prepare { exited(3, format!("{PREPARE_HEADER}\n{}", rec("error", "stat: Permission denied")).into_bytes()) } else { happy(k, v) }, &[Prepare],
            |o| matches!(o, SubmitOutcome::NotClaimed { step: SubmitStep::Prepare, refusal: Refusal::Other(reason) } if reason.contains("Permission denied"))),
        ("prepare: a complete reply with exit 1", |k, v| if k == Prepare { prepare_ready(v).map(|o| ProcessOutput { code: Some(1), ..o }) } else { happy(k, v) }, &[Prepare],
            |o| matches!(o, SubmitOutcome::NotClaimed { step: SubmitStep::Prepare, refusal: Refusal::Other(reason) } if reason.contains("not trusted"))),
        ("prepare: symlinked bin", |k, v| if k == Prepare { prepare_symlinked_bin(v) } else { happy(k, v) }, &[Prepare],
            |o| matches!(o, SubmitOutcome::NotClaimed { step: SubmitStep::Prepare, refusal: Refusal::Other(reason) } if reason.contains("bin"))),
        ("install: an error record", |k, v| match k { Prepare => prepare_fresh(v), Install => exited(3, format!("{}{}", echo(INSTALL_HEADER, v), rec("error", "mkdir failed")).into_bytes()), _ => happy(k, v) }, &[Prepare, Install],
            |o| matches!(o, SubmitOutcome::NotClaimed { step: SubmitStep::Install, refusal: Refusal::Other(reason) } if reason.contains("mkdir failed"))),
        ("upload: rsync exit 23", |k, v| if k == Upload { exited(23, vec![]) } else { happy(k, v) }, &[Prepare, Upload],
            |o| matches!(o, SubmitOutcome::NotClaimed { step: SubmitStep::Upload, refusal: Refusal::Other(reason) } if reason.contains("23"))),
        ("upload: timeout", |k, v| if k == Upload { Err(TransportError::Timeout { program: "rsync".into(), secs: 60 }) } else { happy(k, v) }, &[Prepare, Upload],
            |o| matches!(o, SubmitOutcome::NotClaimed { step: SubmitStep::Upload, .. })),
        ("submit: refused", |k, v| if k == Submit { submit_reply(v, &rec("refused", "marker: /r/jobs/j1/.started exists")) } else { happy(k, v) }, &[Prepare, Upload, Submit],
            |o| matches!(o, SubmitOutcome::NotClaimed { step: SubmitStep::Submit, refusal: Refusal::Other(reason) } if reason.starts_with("marker: "))),
        ("submit: slot busy", |k, v| if k == Submit { submit_reply(v, &blocked_record()) } else { happy(k, v) }, &[Prepare, Upload, Submit],
            |o| matches!(o, SubmitOutcome::NotClaimed { step: SubmitStep::Submit, refusal: Refusal::Blocked { blockers } } if blockers.len() == 1)),
        ("submit: failed after the claim", |k, v| if k == Submit { submit_reply(v, &rec("failed-after-claim", "enqueue: tsp exited 1")) } else { happy(k, v) }, &[Prepare, Upload, Submit],
            |o| matches!(o, SubmitOutcome::FailedAfterClaim { .. })),
        ("submit: timeout", |k, v| if k == Submit { Err(TransportError::Timeout { program: "ssh".into(), secs: 60 }) } else { happy(k, v) }, &[Prepare, Upload, Submit],
            |o| matches!(o, SubmitOutcome::Unknown { .. })),
        ("submit: ssh exit 255", |k, v| if k == Submit { exited(255, vec![]) } else { happy(k, v) }, &[Prepare, Upload, Submit],
            |o| matches!(o, SubmitOutcome::Unknown { .. })),
        ("submit: an unreadable reply", |k, v| if k == Submit { ok(b"orcastudio-submit 1\nargc 1\n".to_vec()) } else { happy(k, v) }, &[Prepare, Upload, Submit],
            |o| matches!(o, SubmitOutcome::Unknown { .. })),
        ("submit: an echo that differs", |k, v| if k == Submit { let mut v = v.to_vec(); v[3] = "0-47".into(); submit_reply(&v, "enqueued 1\n") } else { happy(k, v) }, &[Prepare, Upload, Submit],
            |o| matches!(o, SubmitOutcome::Unknown { reason } if reason.contains("value 3"))),
    ];
    for (case, respond, kinds, want) in cases {
        let w = World::new();
        let fake = Fake::new(&w, respond);
        let outcome = w.submit(&fake).unwrap_or_else(|e| panic!("{case}: {e}"));
        assert!(want(&outcome), "{case}: {outcome:?}");
        assert_eq!(fake.kinds(), *kinds, "{case}: nothing runs after the failing step");
        let after = row(&w.job());
        assert!(queued_with_coords(&after), "{case}: {after:?}");
        assert_eq!(after.error, outcome.failure(), "{case}: the failure is recorded");
        assert!(after.error.is_some(), "{case}");
        assert!(w.stamped(), "{case}: only RefusedKup clears the stamp");
    }
}

/// NEGATIVE CONTROL target (b): only `SubmitReply::RefusedKup` clears `verified_at` (n item 7,
/// o 13.3). A plain refusal whose text reads like the old free-text signal keeps it. Make
/// `record_attempt` also clear on a `NotClaimed` whose reason starts `KillUserProcesses:` and the
/// first case goes red.
#[test]
fn only_refused_kup_clears_the_profile_stamp() {
    let w = World::new();
    let fake = Fake::new(&w, |k, v| if k == Submit { submit_reply(v, &rec("refused", "KillUserProcesses: b true")) } else { happy(k, v) });
    let outcome = w.submit(&fake).unwrap();
    assert!(matches!(&outcome, SubmitOutcome::NotClaimed { step: SubmitStep::Submit, refusal: Refusal::Other(reason) } if reason == "KillUserProcesses: b true"), "{outcome:?}");
    assert!(w.stamped(), "a plain refusal never clears the stamp, whatever its text");

    let w = World::new();
    let fake = Fake::new(&w, |k, v| if k == Submit { submit_reply(v, &kup_record(0, "b true\n")) } else { happy(k, v) });
    let outcome = w.submit(&fake).unwrap();
    assert_eq!(
        outcome,
        SubmitOutcome::KillUserProcesses { evidence: KupEvidence { rc: 0, stdout: "b true\n".into(), stderr: String::new() } }
    );
    assert!(!w.stamped(), "RefusedKup clears the stamp");
    let profile = get_profile_conn(&w.db.lock().unwrap(), PROFILE).unwrap();
    assert_eq!((profile.orca_version, profile.core_count), (None, None), "and the facts it certified");
    assert!(queued_with_coords(&row(&w.job())), "nothing was claimed: still queued with coordinates");
}

/// Refusals before any ssh write nothing: the job stays a draft without coordinates and no call
/// is made.
#[test]
fn refusals_before_any_ssh_write_nothing() {
    let many: String = format!(
        "{{{}}}",
        (0..1001).map(|i| format!("\"f{i}.xyz\": \"x\"")).collect::<Vec<_>>().join(", ")
    );
    let cases: Vec<(&str, String)> = vec![
        ("an unverified profile", "UPDATE server_profiles SET verified_at = NULL".into()),
        ("no core mask", "UPDATE server_profiles SET core_mask = NULL".into()),
        ("a mask beyond the cores", "UPDATE server_profiles SET core_mask = '0-63'".into()),
        ("a job already queued", "UPDATE jobs SET status = 'queued'".into()),
        ("a finished job", "UPDATE jobs SET status = 'completed'".into()),
        ("an aux name outside the path rule", "UPDATE jobs SET aux_files_json = '{\"a b.xyz\": \"x\"}'".into()),
        ("over 1000 files", format!("UPDATE jobs SET aux_files_json = '{many}'")),
    ];
    for (case, sql) in cases {
        let w = World::new();
        w.sql(&sql);
        let before = row(&w.job());
        let fake = Fake::new(&w, happy);
        assert!(w.submit(&fake).is_err(), "{case}");
        assert_eq!(fake.kinds(), [], "{case}: no ssh");
        assert_eq!(row(&w.job()), before, "{case}: nothing written");
        assert_eq!(w.job().backend_id, None, "{case}");
    }
    let w = World::new();
    let fake = Fake::new(&w, happy);
    assert!(matches!(submit_remote(&w.db, &fake, &w.sink, &w.data_dir(), JOB, "nope").map(|a| a.outcome), Err(AppError::NotFound(_))));
    assert_eq!(w.job().status, JobStatus::Draft);
}

/// A submitted job cannot be submitted again; the coordinates are written once.
#[test]
fn a_submitted_job_is_never_submitted_again() {
    let w = World::new();
    let fake = Fake::new(&w, happy);
    w.submit(&fake).unwrap();
    let before = w.job();
    assert!(w.submit(&fake).is_err());
    assert_eq!(fake.kinds(), [Prepare, Upload, Submit], "the second submit made no call");
    assert_eq!(row(&w.job()), row(&before));
}

// ---- retry -----------------------------------------------------------------------------------

fn submitted_but_refused(w: &World) {
    let fake = Fake::new(w, |k, v| if k == Submit { submit_reply(v, &rec("refused", "marker: …")) } else { happy(k, v) });
    assert!(matches!(w.submit(&fake).unwrap(), SubmitOutcome::NotClaimed { .. }));
}

#[test]
fn retry_runs_only_for_a_job_not_on_the_server() {
    let w = World::new();
    submitted_but_refused(&w);
    for (case, markers) in [("submit interrupted", INTERRUPTED), ("enqueued", ENQUEUED)] {
        let fake = Fake::new(&w, move |k, v| if k == Label { label_reply(v, markers) } else { happy(k, v) });
        assert!(w.resubmit(&fake).is_err(), "{case}");
        assert_eq!(fake.kinds(), [Label], "{case}: nothing after the label call");
    }
    let fake = Fake::new(&w, happy);
    assert_eq!(w.resubmit(&fake).unwrap(), SubmitOutcome::Enqueued { tsp_id: 3 });
    assert_eq!(fake.kinds(), [Label, Prepare, Upload, Submit]);
    assert_eq!(row(&w.job()), RowState { status: JobStatus::Queued, coords: Some(expected_coords()), error: None });
}

#[test]
fn retry_refuses_an_unverified_profile_or_a_moved_target() {
    for (case, sql) in [
        ("unverified", "UPDATE server_profiles SET verified_at = NULL"),
        ("another host", "UPDATE server_profiles SET host = 'other'"),
        ("another root", "UPDATE server_profiles SET remote_scratch_dir = '/srv/os'"),
        ("a local job", "UPDATE jobs SET remote_host = NULL, remote_job_dir = NULL, remote_socket = NULL"),
    ] {
        let w = World::new();
        submitted_but_refused(&w);
        w.sql(sql);
        let fake = Fake::new(&w, happy);
        assert!(w.resubmit(&fake).is_err(), "{case}");
        assert_eq!(fake.kinds(), [], "{case}: no ssh");
    }
}

// ---- snapshots ---------------------------------------------------------------------------------

/// A collector output with no `.started`: `cancelled`, an optional `.exit_code` and output tail,
/// no daemon on the socket. `started` = the two bracketing `.started` reads.
pub(crate) fn snapshot(cancelled: bool, exit_code: Option<&str>, tail: Option<&str>, started: (Option<&str>, Option<&str>)) -> Vec<u8> {
    let opt = |name: &str, v: Option<&str>| v.map_or(format!("{name} -\n"), |t| rec(name, t));
    let socket = expected_coords().socket;
    format!(
        "{SNAPSHOT_HEADER}\n{}{}proc skipped\nsockets 1\n{}{}nodaemon\n{}cancelled {}\n{}{}end\n",
        rec("boot_id", "0f6e3c1a-5b7d-4c2e-9a8b-1d2e3f4a5b6c\n"),
        opt("started", started.0),
        rec("socket", &socket),
        rec("net_unix", NET_UNIX_HEADER),
        opt("exit_code", exit_code),
        if cancelled { "yes" } else { "no" },
        opt("tail", tail),
        opt("started", started.1),
    )
    .into_bytes()
}

/// A clean ORCA output tail, for a collector snapshot.
pub(crate) const CLEAN_TAIL: &str = "                             ****ORCA TERMINATED NORMALLY****\n";

// ---- coordinates and refusals ------------------------------------------------------------

fn job_with(status: JobStatus, coords: Option<(&str, &str, &str)>) -> Job {
    let w = World::new();
    // Coordinates first: the v21 CHECK refuses a `cancelling` row that is not (yet) remote.
    if let Some((h, d, s)) = coords {
        w.db.lock().unwrap().execute("UPDATE jobs SET remote_host = ?1, remote_job_dir = ?2, remote_socket = ?3", params![h, d, s]).unwrap();
    }
    w.sql(&format!("UPDATE jobs SET status = '{}'", status.as_str()));
    w.job()
}

#[test]
fn a_live_remote_job_is_refused_locally_and_nothing_else_is() {
    let remote = Some(("uni", "/home/anton/.orcastudio/jobs/j1", "/home/anton/.orcastudio/tsp/slot0.sock"));
    // `cancelling` is remote-live (o22.5). NEGATIVE CONTROL: drop it from the predicate → red.
    for status in [JobStatus::Queued, JobStatus::Running, JobStatus::Cancelling] {
        let err = refuse_if_remote_live(&job_with(status, remote)).unwrap_err();
        assert!(err.to_string().contains("cancel it first (the cancel goes through the server)"), "{err}");
    }
    for status in [JobStatus::Draft, JobStatus::Completed, JobStatus::Parsed, JobStatus::Failed, JobStatus::Cancelled] {
        assert!(refuse_if_remote_live(&job_with(status, remote)).is_ok(), "{status:?}");
    }
    for status in [JobStatus::Queued, JobStatus::Running] {
        assert!(refuse_if_remote_live(&job_with(status, None)).is_ok(), "a local {status:?} job is the local backend's");
    }
}

#[test]
fn coordinates_are_all_or_nothing_and_the_root_comes_from_the_job_dir() {
    let mut job = job_with(JobStatus::Queued, Some(("uni", "/r/jobs/j1", "/r/tsp/slot0.sock")));
    let coords = coordinates(&job).unwrap().unwrap();
    assert_eq!(recorded_root(&coords, "j1").unwrap(), "/r");
    assert!(recorded_root(&coords, "j2").is_err());
    assert!(recorded_root(&RemoteCoordinates { job_dir: "/r/jobs/j1/x".into(), ..coords.clone() }, "j1").is_err());
    job.remote_socket = None;
    assert!(coordinates(&job).is_err(), "a partial set is an error, never local");
}

// ---- %pal, aligned downward only (o 14.2) -------------------------------------------------------

/// NEGATIVE CONTROL target: `min`, never `max` — replace `n.min(mask_cpus)` with `mask_cpus` (a
/// plain local-style alignment) and the "2 on 12" row goes red (a small %pal would be raised).
#[test]
fn pal_is_capped_by_the_distinct_cpus_of_the_mask_and_never_raised() {
    let cases: &[(&str, &str, &str, Option<u32>, u32, &str)] = &[
        ("48 on a 4-CPU mask", "! HF\n%pal nprocs 48 end\n", "0-3", Some(48), 4, "! HF\n%pal nprocs 4 end\n"),
        ("2 on a 12-CPU mask", "! HF\n%pal nprocs 2 end\n", "0-11", Some(2), 2, "! HF\n%pal nprocs 2 end\n"),
        ("overlapping ranges count once", "! HF\n%pal nprocs 48 end\n", "0-3,2-5", Some(48), 6, "! HF\n%pal nprocs 6 end\n"),
        ("no %pal: the CPU count, as a local run", "! HF\n* xyz 0 1\nO 0 0 0\n*\n", "0-11,24-35", None, 24, "! HF\n%pal nprocs 24 end\n* xyz 0 1\nO 0 0 0\n*\n"),
        ("the block form", "! HF\n%pal\n  nprocs 16\nend\n* xyz 0 1\n", "0-7", Some(16), 8, "! HF\n%pal nprocs 8 end\n* xyz 0 1\n"),
        ("upper case", "! HF\n%PAL NPROCS 3 END\n", "0-7", Some(3), 3, "! HF\n%pal nprocs 3 end\n"),
    ];
    for (case, input, mask, from, n, want) in cases {
        let (out, pal) = align_remote_pal(input, mask).unwrap_or_else(|e| panic!("{case}: {e}"));
        assert_eq!((pal.input_nprocs, pal.nprocs), (*from, *n), "{case}");
        assert_eq!(out, *want, "{case}");
        assert_eq!(pal.rewritten, out != *input, "{case}");
        assert_eq!(pal.notice().is_some(), pal.rewritten, "{case}: a change is announced, no change is not");
    }
    let (_, pal) = align_remote_pal("! HF\n%pal nprocs 48 end\n", "0-3").unwrap();
    assert_eq!(
        pal.notice().unwrap(),
        "[OrcaStudio] %pal nprocs aligned to 4 (the server profile's core mask has 4 CPUs; the input had nprocs 48)"
    );
}

/// An unreadable `%pal` is refused, never guessed; two `%pal` directives fail the post-condition.
#[test]
fn an_unreadable_or_doubled_pal_is_refused() {
    for (case, input) in [
        ("no nprocs", "! HF\n%pal end\n"),
        ("nprocs not a number", "! HF\n%pal nprocs many end\n"),
        ("nprocs 0", "! HF\n%pal nprocs 0 end\n"),
        ("two %pal directives", "! HF\n%pal nprocs 4 end\n%pal nprocs 8 end\n"),
    ] {
        assert!(align_remote_pal(input, "0-3").is_err(), "{case}");
    }
}

/// The refusal comes before the persist: the row is untouched and no ssh is made (o 14.4).
#[test]
fn an_unreadable_pal_refuses_the_submit_before_anything_is_written() {
    let w = World::new();
    w.sql("UPDATE jobs SET input_content = '! HF\n%pal end\n'");
    let before = row(&w.job());
    let fake = Fake::new(&w, happy);
    assert!(w.submit(&fake).is_err());
    assert_eq!(fake.kinds(), []);
    assert_eq!(row(&w.job()), before);
}

/// The attempt carries the alignment, and a retry derives it again from `input_content` with the
/// profile's current mask (the local dir is rewritten before it is listed and hashed).
#[test]
fn a_retry_aligns_to_the_current_mask() {
    let w = World::new();
    w.sql("UPDATE jobs SET input_content = '! HF\n%pal nprocs 48 end\n'");
    let fake = Fake::new(&w, |k, v| if k == Submit { submit_reply(v, &rec("refused", "marker: …")) } else { happy(k, v) });
    assert_eq!(w.attempt(&fake).unwrap().pal.nprocs, 24, "the 0-23 mask");
    w.sql("UPDATE server_profiles SET core_mask = '0-5'");
    let fake = Fake::new(&w, happy);
    let retry = resubmit_remote(&w.db, &fake, &w.sink, &w.data_dir(), JOB).unwrap();
    assert_eq!((retry.outcome, retry.pal.nprocs), (SubmitOutcome::Enqueued { tsp_id: 3 }, 6));
    let local = w.data_dir().join("jobs").join(JOB).join("input.inp");
    assert_eq!(std::fs::read_to_string(local).unwrap(), "! HF\n%pal nprocs 6 end\n");
    assert_eq!(w.job().input_content, "! HF\n%pal nprocs 48 end\n", "the database keeps the original");
}

/// Verifier LOW-2: the label call comes before any write. A retry the label refuses leaves the local
/// input as the last attempt wrote it, even after a mask change. NEGATIVE CONTROL: write the local
/// dir before the label call and the file shows the new mask's `nprocs 6` → red.
#[test]
fn a_retry_refused_by_the_label_leaves_the_local_dir_unchanged() {
    let w = World::new();
    w.sql("UPDATE jobs SET input_content = '! HF\n%pal nprocs 48 end\n'");
    submitted_but_refused(&w);
    let local = w.data_dir().join("jobs").join(JOB).join("input.inp");
    assert_eq!(std::fs::read_to_string(&local).unwrap(), "! HF\n%pal nprocs 24 end\n");
    w.sql("UPDATE server_profiles SET core_mask = '0-5'");
    let fake = Fake::new(&w, |k, v| if k == Label { label_reply(v, INTERRUPTED) } else { happy(k, v) });
    assert!(w.resubmit(&fake).is_err());
    assert_eq!(fake.kinds(), [Label]);
    assert_eq!(std::fs::read_to_string(&local).unwrap(), "! HF\n%pal nprocs 24 end\n", "untouched");
}

/// Verifier LOW-7: the retry reads the coordinates, makes the label call, then reads them again
/// before any write — a row whose coordinates changed in between (with a profile that now matches
/// them, so every other check passes) is refused with nothing written and nothing more sent. The
/// fake's label handler makes the change, as a concurrent writer would. NEGATIVE CONTROL: disable
/// the `now != coords` check in `resubmit_remote` and the retry carries on with the stale
/// coordinates — red (`Ok(… Enqueued …)`, the local input rewritten to `nprocs 6`).
#[test]
fn a_retry_whose_coordinates_change_during_the_label_call_writes_nothing() {
    let w = World::new();
    w.sql("UPDATE jobs SET input_content = '! HF\n%pal nprocs 48 end\n'");
    submitted_but_refused(&w);
    let local = w.data_dir().join("jobs").join(JOB).join("input.inp");
    assert_eq!(std::fs::read_to_string(&local).unwrap(), "! HF\n%pal nprocs 24 end\n");
    let fake = Fake::new(&w, |k, v| {
        if k == Label {
            w.sql(
                "UPDATE jobs SET remote_host = 'uni2'; \
                 UPDATE server_profiles SET host = 'uni2', core_mask = '0-5';",
            );
        }
        happy(k, v)
    });
    let err = resubmit_remote(&w.db, &fake, &w.sink, &w.data_dir(), JOB).map(|a| a.outcome);
    assert!(
        matches!(&err, Err(AppError::Conflict(m)) if m.contains("changed during the retry")),
        "refused as a conflict: {err:?}"
    );
    assert_eq!(fake.kinds(), [Label], "nothing is sent after the label call");
    assert_eq!(std::fs::read_to_string(&local).unwrap(), "! HF\n%pal nprocs 24 end\n", "the local dir is untouched");
    let after = w.job();
    assert_eq!((after.status, after.remote_host.as_deref()), (JobStatus::Queued, Some("uni2")));
    assert!(
        after.error_message.as_deref().is_some_and(|m| m.contains("marker: …")),
        "the row keeps the last attempt's reason: {:?}",
        after.error_message
    );
}

/// The `SshBackend` arm of the cancel core's guarded path refuses every remote job — a live one never
/// reaches it (the core records and delivers, o22.3), and if one did (the belt), it gets the remote
/// reason: it is never told "nothing to cancel", and never reaches a local `Cancelled`. NEGATIVE
/// CONTROL: drop the `refuse_if_remote_live` line from `cancel_remote` and the live rows go red
/// (they get "nothing to cancel").
#[test]
fn a_remote_cancel_is_refused_and_a_live_one_with_the_remote_reason() {
    let remote = Some(("uni", "/home/anton/.orcastudio/jobs/j1", "/home/anton/.orcastudio/tsp/slot0.sock"));
    for status in [JobStatus::Queued, JobStatus::Running, JobStatus::Cancelling] {
        let err = cancel_remote(&job_with(status, remote)).unwrap_err().to_string();
        assert!(err.contains("cancel it first (the cancel goes through the server)"), "{status:?}: {err}");
    }
    for status in [JobStatus::Draft, JobStatus::Completed, JobStatus::Parsed, JobStatus::Failed, JobStatus::Cancelled] {
        let err = cancel_remote(&job_with(status, remote)).unwrap_err().to_string();
        assert!(err.contains("nothing to cancel"), "{status:?}: {err}");
    }
}

// ---- the pending submit (ADR-024 o23) ------------------------------------------------------------

/// o23.1, o23.5: a submit the user started becomes pending only when the slot check refuses it —
/// slot busy or slot check error — set in `record_attempt`'s one statement, with `job:status`
/// emitted; every other outcome leaves the flag NULL ("not on the server", the manual Retry).
/// NEGATIVE CONTROLS: "a slot-busy answer that does not make the row pending" (`after_user_attempt`
/// → `Keep` for it) → red; "a flag write that emits no job:status" (drop the flag from the emit's
/// comparison and give the row the same message beforehand) → red.
#[test]
fn only_a_slot_refusal_makes_a_first_submit_pending() {
    let kup = kup_record(0, "b true\n");
    let cases: Vec<(&str, String, bool)> = vec![
        ("slot busy", blocked_record(), true),
        ("slot check error", rec("slot-check-error", "the scan timed out after 8 s"), true),
        ("lock busy", rec("lock-busy", "77 bash -s"), false),
        ("a marker", rec("refused", "marker: x"), false),
        ("upload mismatch", "upload-mismatch\nmissing 1\nname 9\ninput.inp\nextra 0\ndiffering 0\n".to_string(), false),
        ("KillUserProcesses", kup, false),
        ("failed after the claim", rec("failed-after-claim", "enqueue: tsp exited 1"), false),
        ("enqueued", "enqueued 3\n".to_string(), false),
    ];
    for (case, outcome, pending) in cases {
        let w = World::new();
        let fake = Fake::new(&w, move |k, v| if k == Submit { submit_reply(v, &outcome) } else { happy(k, v) });
        w.submit(&fake).unwrap();
        assert_eq!(w.job().submit_pending_since.is_some(), pending, "{case}");
        let emitted = w.sink.events().iter().any(|e| matches!(e, crate::poller::Event::Status(id, JobStatus::Queued) if id == JOB));
        assert_eq!(emitted, pending || case != "enqueued", "{case}: a changed row emits job:status");
    }
    // A submit call that got no reply is not pending either: the label decides.
    let w = World::new();
    let fake = Fake::new(&w, |k, v| if k == Submit { exited(255, vec![]) } else { happy(k, v) });
    assert!(matches!(w.submit(&fake).unwrap(), SubmitOutcome::Unknown { .. }));
    assert_eq!(w.job().submit_pending_since, None);
}

/// o23.5: the flag's own write emits even when the message did not change. The row already carries
/// the attempt's message, so only the flag changes. NEGATIVE CONTROL: compare only `error_message`
/// in `record_attempt`'s emit → red.
#[test]
fn setting_the_flag_alone_emits_job_status() {
    let w = World::new();
    let fake = Fake::new(&w, |k, v| if k == Submit { submit_reply(v, &blocked_record()) } else { happy(k, v) });
    let first = w.submit(&fake).unwrap();
    w.sql("UPDATE jobs SET submit_pending_since = NULL");
    w.sink.take();
    let plan = {
        let conn = w.db.lock().unwrap();
        pending_plan(&conn, &w.data_dir(), JOB, &expected_coords()).unwrap()
    };
    assert!(record_attempt(&w.db, &w.sink, &plan, &first, PendingWrite::Set).unwrap());
    assert!(w.job().submit_pending_since.is_some());
    assert_eq!(w.sink.take(), [crate::poller::Event::Status(JOB.into(), JobStatus::Queued)], "the flag alone changed, and it emits");
}

/// o23.5, round 1 LOW-4: `Set` is `COALESCE(submit_pending_since, now)` — a later refusal keeps the
/// first time. NEGATIVE CONTROL: "a later refusal overwriting `submit_pending_since`" (`Set` writes
/// `datetime('now')`) → red.
#[test]
fn a_later_refusal_keeps_the_first_pending_time() {
    let w = World::new();
    let fake = Fake::new(&w, |k, v| if k == Submit { submit_reply(v, &blocked_record()) } else { happy(k, v) });
    let outcome = w.submit(&fake).unwrap();
    w.sql("UPDATE jobs SET submit_pending_since = '2026-10-01 00:00:00'");
    let plan = {
        let conn = w.db.lock().unwrap();
        pending_plan(&conn, &w.data_dir(), JOB, &expected_coords()).unwrap()
    };
    assert!(record_attempt(&w.db, &w.sink, &plan, &outcome, PendingWrite::Set).unwrap());
    assert_eq!(w.job().submit_pending_since.as_deref(), Some("2026-10-01 00:00:00"));
    assert!(record_attempt(&w.db, &w.sink, &plan, &outcome, PendingWrite::Clear).unwrap());
    assert_eq!(w.job().submit_pending_since, None);
}

/// Rule #9 on the attempt's write: a row that is no longer this remote `queued` job takes nothing,
/// and the write says so (`false`), emitting nothing.
#[test]
fn an_attempt_write_on_a_row_that_moved_on_takes_nothing() {
    let w = World::new();
    let fake = Fake::new(&w, |k, v| if k == Submit { submit_reply(v, &blocked_record()) } else { happy(k, v) });
    let outcome = w.submit(&fake).unwrap();
    let plan = {
        let conn = w.db.lock().unwrap();
        pending_plan(&conn, &w.data_dir(), JOB, &expected_coords()).unwrap()
    };
    w.sql("UPDATE jobs SET status = 'cancelled', submit_pending_since = NULL");
    w.sink.take();
    assert!(!record_attempt(&w.db, &w.sink, &plan, &outcome, PendingWrite::Set).unwrap());
    assert_eq!(w.job().submit_pending_since, None);
    assert!(w.sink.take().is_empty());
}

/// o23.8: Retry submit is refused on a pending row ("a submit is pending; the slot is re-checked
/// every period"), before any call. NEGATIVE CONTROL: "`retry_remote_submit` accepted on a pending
/// row" (drop the refusal from `retry_checks`) → red.
#[test]
fn retry_is_refused_on_a_pending_row() {
    let w = World::new();
    let fake = Fake::new(&w, |k, v| if k == Submit { submit_reply(v, &blocked_record()) } else { happy(k, v) });
    w.submit(&fake).unwrap();
    assert!(w.job().submit_pending_since.is_some());
    let fake = Fake::new(&w, happy);
    let err = w.resubmit(&fake).unwrap_err().to_string();
    assert!(err.contains("a submit is pending; the slot is re-checked every period"), "{err}");
    assert_eq!(fake.kinds(), [], "no call");
}

/// o23.13.5: an attempt whose derived bytes are unchanged does not rewrite the local dir — on a
/// retry too, since the writer is shared.
#[test]
fn an_unchanged_local_input_is_not_rewritten_by_a_retry() {
    let w = World::new();
    submitted_but_refused(&w);
    let input = w.data_dir().join("jobs").join(JOB).join("input.inp");
    let old = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    std::fs::File::options().write(true).open(&input).unwrap().set_modified(old).unwrap();
    let fake = Fake::new(&w, happy);
    assert_eq!(w.resubmit(&fake).unwrap(), SubmitOutcome::Enqueued { tsp_id: 3 });
    assert_eq!(std::fs::metadata(&input).unwrap().modified().unwrap(), old);
}

/// A server for a first attempt from outside this module: ready, the upload taken, `submit.sh`
/// answering `outcome` — through the same fake as every test here.
pub(crate) struct FirstAttempt {
    outcome: String,
}

impl FirstAttempt {
    pub(crate) fn new(outcome: &str) -> Self {
        FirstAttempt { outcome: outcome.to_string() }
    }
}

impl CommandRunner for FirstAttempt {
    fn run(&self, program: &str, _args: &[String], stdin: &[u8], _timeout: Duration) -> Reply {
        let (kind, values) = kind_of(program, stdin);
        if kind == Submit { submit_reply(&values, &self.outcome) } else { happy(kind, &values) }
    }
}
