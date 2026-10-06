//! The remote backend's core (`crate::ssh_backend`) end to end against the **real** scripts on
//! this machine (unit 5.3 B1): the 5.2 [`Lab`] (stub `tsp`, stub `busctl`, stub ORCA, real
//! `wrapper.sh`/`cancel.sh`/`collect.sh`) as the server, and a runner that runs each call locally
//! — `bash -s` instead of `ssh <host> bash -s`, and rsync between local paths instead of
//! `<host>:<path>`. Everything else is the production path: the same stdin, the same values, the
//! same parsers, the same database writes. `cancel.sh` and `collect.sh` run through the real
//! trampoline (ADR-024 o item 14.1).
//!
//! The submit's slot check scans this machine's real processes, so every test that submits holds
//! the [`serial`] lock of `call_script_tests` and uses its slot mask.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::params;

use super::call_script_tests::{mask, mutate, run, serial, slot_socket};
use super::classify::Outcome;
use super::prepare::{check_prepare, parse_install_reply, parse_prepare_reply, InstallArgs, Installed, PrepareArgs, Prepared};
use super::run::{parse_mkjob_reply, parse_run_reply, JobScript, MkjobArgs, RunArgs, RunReply};
use super::script_tests::Lab;
use super::scripts::{sha256_hex, CANCEL, INSTALL, LABEL, LIST, MKJOB, POLL_LOG, PREPARE, RUN, SUBMIT, WRAPPER};
use super::ssh::{ssh_options, CommandRunner, ProcessOutput, SystemRunner, TransportError, SSH_PROGRAM};
use super::submit::Label;
use super::sync::RSYNC_PROGRAM;
use crate::commands::jobs::get_job_conn;
use crate::commands::server_profiles::get_profile_conn;
use crate::commands::settings::DbState;
use crate::db::init_db;
use crate::models::job::JobStatus;
use crate::ssh_backend::{
    coordinates, label_remote, resubmit_remote, submit_remote, withdraw_remote, RemoteCoordinates,
    SubmitAttempt, SubmitOutcome, SubmitStep,
};

const HOST: &str = "lab-host";
const PROFILE: &str = "p1";

/// The server's side of every call, run locally against the lab.
struct Loop<'a> {
    lab: &'a Lab,
    /// Extra environment for the scripts (the stub busctl's `STUB_KUP`, say).
    env: Vec<(String, String)>,
    calls: RefCell<Vec<String>>,
    /// Runs right after each rsync (a test's fault injection into the copy).
    after_rsync: Option<Box<dyn Fn() + 'a>>,
}

impl<'a> Loop<'a> {
    fn new(lab: &'a Lab) -> Self {
        Loop { lab, env: Vec::new(), calls: RefCell::default(), after_rsync: None }
    }

    fn with_env(lab: &'a Lab, key: &str, value: &str) -> Self {
        Loop { env: vec![(key.into(), value.into())], ..Loop::new(lab) }
    }
}

impl CommandRunner for Loop<'_> {
    fn run(&self, program: &str, args: &[String], stdin: &[u8], timeout: Duration) -> Result<ProcessOutput, TransportError> {
        match program {
            SSH_PROGRAM => {
                // Exactly the production argv: the shared options, `--`, the host, `bash -s`.
                let mut want = ssh_options();
                want.extend(["--".into(), HOST.into(), "bash".into(), "-s".into()]);
                assert_eq!(args, want);
                let name = [
                    (PREPARE, "prepare"),
                    (INSTALL, "install"),
                    (SUBMIT, "submit"),
                    (LABEL, "label"),
                    (MKJOB, "mkjob"),
                    (RUN, "run"),
                    (LIST, "list"),
                    (POLL_LOG, "poll_log"),
                ]
                    .iter()
                    .find(|(s, _)| stdin.starts_with(s.as_bytes()))
                    .map_or("?", |(_, name)| name);
                self.calls.borrow_mut().push(name.to_string());
                let mut argv = vec![format!("PATH={}", self.lab.path_env()), format!("HOME={}", Lab::str(&self.lab.root))];
                argv.extend(self.env.iter().map(|(k, v)| format!("{k}={v}")));
                argv.extend(["bash".into(), "-s".into()]);
                SystemRunner.run("env", &argv, stdin, timeout)
            }
            RSYNC_PROGRAM => {
                self.calls.borrow_mut().push("rsync".into());
                // rsync between local paths: drop `-e <ssh>`, strip `<host>:`.
                let mut local = Vec::new();
                let mut skip = false;
                for a in args {
                    if skip {
                        skip = false;
                    } else if a == "-e" {
                        skip = true;
                    } else {
                        assert!(!a.starts_with("--delete"), "never --delete");
                        local.push(a.strip_prefix(&format!("{HOST}:")).unwrap_or(a).to_string());
                    }
                }
                let out = SystemRunner.run("rsync", &local, stdin, timeout);
                if let Some(after) = &self.after_rsync {
                    after();
                }
                out
            }
            other => panic!("the core never runs {other}"),
        }
    }
}

/// The laptop's side: a database with a stamped profile on the lab, and the local data dir.
struct Laptop {
    db: DbState,
    data_dir: PathBuf,
}

impl Laptop {
    fn new(lab: &Lab) -> Laptop {
        let conn = init_db(&lab.root.join("laptop-db")).unwrap();
        conn.execute(
            "INSERT INTO server_profiles (id, name, host, remote_orca_path, remote_scratch_dir, core_mask, \
             orca_version, core_count, verified_at) VALUES (?1, 'lab', ?2, ?3, ?4, ?5, '6.1.1', ?6, datetime('now'))",
            params![PROFILE, HOST, Lab::str(&lab.stub_dir.join("orca")), Lab::str(&lab.root), mask(), cores()],
        )
        .unwrap();
        Laptop { db: DbState(Mutex::new(conn)), data_dir: lab.root.join("laptop") }
    }

    /// A draft with `input` and one aux file (a NEB end image).
    fn draft_with(&self, id: &str, input: &str) {
        self.db
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO jobs (id, title, input_content, status, aux_files_json) VALUES (?1, 't', ?2, 'draft', ?3)",
                params![id, input, r#"{"product.xyz": "1\n\nH 0 0 1\n"}"#],
            )
            .unwrap();
    }

    fn draft(&self, id: &str) {
        self.draft_with(id, "! HF\n");
    }

    fn attempt(&self, runner: &dyn CommandRunner, id: &str) -> SubmitAttempt {
        submit_remote(&self.db, runner, &self.data_dir, id, PROFILE).unwrap()
    }

    fn submit(&self, runner: &dyn CommandRunner, id: &str) -> SubmitOutcome {
        self.attempt(runner, id).outcome
    }

    fn resubmit(&self, runner: &dyn CommandRunner, id: &str) -> Result<SubmitAttempt, crate::error::AppError> {
        resubmit_remote(&self.db, runner, &self.data_dir, id)
    }

    fn job(&self, id: &str) -> crate::models::job::Job {
        get_job_conn(&self.db.lock().unwrap(), id).unwrap()
    }

    fn stamped(&self) -> bool {
        get_profile_conn(&self.db.lock().unwrap(), PROFILE).unwrap().verified_at.is_some()
    }

    fn sql(&self, sql: &str) {
        self.db.lock().unwrap().execute_batch(sql).unwrap();
    }

    /// The connection test's job, by hand: stamp the profile again.
    fn restamp(&self) {
        self.db
            .lock()
            .unwrap()
            .execute("UPDATE server_profiles SET verified_at = datetime('now'), core_count = ?1", params![cores()])
            .unwrap();
    }
}

fn cores() -> u32 {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as u32
}

/// The number of CPUs in the slot mask the submit tests use.
fn mask_cpus() -> u32 {
    let m = mask();
    match m.split_once('-') {
        Some((a, b)) => b.parse::<u32>().unwrap() - a.parse::<u32>().unwrap() + 1,
        None => 1,
    }
}

fn remote_job(lab: &Lab, id: &str) -> PathBuf {
    lab.root.join("jobs").join(id)
}

#[test]
fn a_submit_installs_uploads_and_enqueues_against_the_real_scripts() {
    let _serial = serial();
    let lab = Lab::new();
    fs::remove_file(&lab.wrapper).unwrap();
    assert!(!lab.root.join("tsp").exists(), "a root the app never used: no tsp/");
    let laptop = Laptop::new(&lab);
    laptop.draft("e2e1");
    let runner = Loop::new(&lab);

    let attempt = laptop.attempt(&runner, "e2e1");
    assert_eq!(attempt.outcome, SubmitOutcome::Enqueued { tsp_id: 0 });
    assert_eq!(*runner.calls.borrow(), ["prepare", "install", "prepare", "rsync", "submit"]);

    // The database: queued, with the recorded coordinates; the user's input untouched.
    let job = laptop.job("e2e1");
    let sock = slot_socket(&lab, 0);
    let remote = remote_job(&lab, "e2e1");
    assert_eq!(job.status, JobStatus::Queued);
    assert_eq!(
        coordinates(&job).unwrap(),
        Some(RemoteCoordinates { host: HOST.into(), job_dir: Lab::str(&remote).into(), socket: Lab::str(&sock).into() })
    );
    assert_eq!((job.error_message.as_deref(), job.input_content.as_str()), (None, "! HF\n"));
    // %pal: none in the input → the mask's CPU count, announced (o 14.2).
    assert_eq!((attempt.pal.input_nprocs, attempt.pal.nprocs, attempt.pal.rewritten), (None, mask_cpus(), true));
    assert!(attempt.pal.notice().unwrap().contains(&format!("aligned to {}", mask_cpus())));
    // The server: the wrapper by its sha, tsp/, the upload byte for byte, the claim and .enqueued.
    assert_eq!(sha256_hex(&fs::read_to_string(&lab.wrapper).unwrap()), sha256_hex(WRAPPER));
    assert!(lab.root.join("tsp").is_dir());
    let local = laptop.data_dir.join("jobs").join("e2e1");
    assert_eq!(fs::read_to_string(local.join("input.inp")).unwrap(), format!("! HF\n%pal nprocs {} end\n", mask_cpus()));
    for name in ["input.inp", "product.xyz"] {
        assert_eq!(fs::read(remote.join(name)).unwrap(), fs::read(local.join(name)).unwrap(), "{name}");
    }
    assert!(remote.join(".submitting").is_symlink());
    assert_eq!(fs::read_to_string(remote.join(".enqueued")).unwrap(), format!("socket={}\nid=0\n", Lab::str(&sock)));
    assert!(temps_in(&lab.root.join("bin")).is_empty(), "no install temp left");

    // The label call hands it to the classifier; the 5.2 collector sees it queued.
    assert_eq!(label_remote(&runner, &job).unwrap().label, Label::Classifier);
    assert_eq!(lab.classify(&remote, &[sock]), Outcome::Queued);
    // And it cannot be withdrawn: it is in the server's queue (remote cancel is unit 5.4).
    let err = withdraw_remote(&laptop.db, &runner, "e2e1").unwrap_err();
    assert!(err.to_string().contains("unit 5.4"), "{err}");
    assert!(!remote.join(".cancelled").exists());
}

#[test]
fn a_failed_enqueue_is_submit_interrupted_and_withdrawn_to_cancelled_by_the_classifier() {
    let _serial = serial();
    let lab = Lab::new();
    let laptop = Laptop::new(&lab);
    laptop.draft("e2e2");
    let sock = slot_socket(&lab, 0);
    fs::write(format!("{}.enqueue-fail", Lab::str(&sock)), "").unwrap();
    let runner = Loop::new(&lab);

    assert!(matches!(laptop.submit(&runner, "e2e2"), SubmitOutcome::FailedAfterClaim { .. }));
    let job = laptop.job("e2e2");
    assert_eq!(job.status, JobStatus::Queued, "the claim stays: the server decides");
    assert_eq!(label_remote(&runner, &job).unwrap().label, Label::SubmitInterrupted);
    assert!(laptop.resubmit(&runner, "e2e2").is_err(), "submit interrupted is never retried");

    runner.calls.borrow_mut().clear();
    let report = withdraw_remote(&laptop.db, &runner, "e2e2").unwrap();
    assert_eq!((report.label, &report.outcome, report.status), (Label::SubmitInterrupted, &Outcome::Cancelled, JobStatus::Cancelled));
    assert_eq!(*runner.calls.borrow(), ["label", "prepare", "mkjob", "run", "run"], "cancel and collect through the trampoline");
    assert_eq!(laptop.job("e2e2").status, JobStatus::Cancelled);
    assert!(remote_job(&lab, "e2e2").join(".cancelled").exists(), "the server holds the reason");
}

#[test]
fn a_kill_user_processes_refusal_clears_the_stamp_then_retry_and_withdraw_work() {
    let _serial = serial();
    let lab = Lab::new();
    let laptop = Laptop::new(&lab);
    laptop.draft("e2e3");
    laptop.draft("e2e4");

    let kup = Loop::with_env(&lab, "STUB_KUP", "b true");
    let outcome = laptop.submit(&kup, "e2e3");
    assert!(matches!(&outcome, SubmitOutcome::KillUserProcesses { evidence } if evidence.stdout == "b true\n"), "{outcome:?}");
    assert!(!laptop.stamped(), "RefusedKup clears verified_at");
    let job = laptop.job("e2e3");
    assert_eq!(job.status, JobStatus::Queued);
    assert_eq!(label_remote(&kup, &job).unwrap().label, Label::NotOnServer, "uploaded, but nothing claimed");
    assert!(!remote_job(&lab, "e2e3").join(".submitting").exists());

    // An unverified profile is no run target: neither a new submit nor a retry runs.
    let runner = Loop::new(&lab);
    assert!(submit_remote(&laptop.db, &runner, &laptop.data_dir, "e2e4", PROFILE).is_err());
    assert!(laptop.resubmit(&runner, "e2e3").is_err());
    assert!(runner.calls.borrow().is_empty(), "no ssh for an unverified profile");

    // Verified again (the connection test's job), the retry goes through on the recorded dir.
    laptop.restamp();
    assert_eq!(laptop.resubmit(&runner, "e2e3").unwrap().outcome, SubmitOutcome::Enqueued { tsp_id: 0 });
    assert_eq!(*runner.calls.borrow(), ["label", "prepare", "rsync", "submit"], "the scripts were already there");
    assert_eq!(laptop.job("e2e3").error_message, None);

    // A job refused the same way and withdrawn instead, with the profile unverified again — a
    // withdraw has no `verified_at` gate (n 6a): the classifier says Cancelled.
    assert!(matches!(laptop.submit(&kup, "e2e4"), SubmitOutcome::KillUserProcesses { .. }));
    assert!(!laptop.stamped());
    let report = withdraw_remote(&laptop.db, &runner, "e2e4").unwrap();
    assert_eq!((report.label, report.status), (Label::NotOnServer, JobStatus::Cancelled));
}

/// o 14.2 end to end: `%pal nprocs 48` on the 4-CPU slot mask uploads `nprocs 4` and the server's
/// set-and-hash check accepts it (local dir = uploaded = hashed); a retry after a mask change uploads
/// the new N, derived again from the untouched `input_content`.
#[test]
fn the_uploaded_pal_is_capped_by_the_attempts_mask() {
    let _serial = serial();
    let lab = Lab::new();
    let laptop = Laptop::new(&lab);
    let input = "! HF\n%pal nprocs 48 end\n";
    laptop.draft_with("e2e6", input);
    laptop.draft_with("e2e7", input);
    let n = mask_cpus();

    let attempt = laptop.attempt(&Loop::new(&lab), "e2e6");
    assert_eq!(attempt.outcome, SubmitOutcome::Enqueued { tsp_id: 0 }, "the server's hash check binds the rewritten bytes");
    assert_eq!((attempt.pal.input_nprocs, attempt.pal.nprocs, attempt.pal.rewritten), (Some(48), n, true));
    assert_eq!(fs::read_to_string(remote_job(&lab, "e2e6").join("input.inp")).unwrap(), format!("! HF\n%pal nprocs {n} end\n"));

    // A refused first attempt, then the mask narrows to one CPU, then the retry.
    let kup = Loop::with_env(&lab, "STUB_KUP", "b true");
    assert_eq!(laptop.attempt(&kup, "e2e7").pal.nprocs, n);
    let first = mask().split('-').next().unwrap().to_string();
    laptop.sql(&format!("UPDATE server_profiles SET core_mask = '{first}'"));
    laptop.restamp();
    let retry = laptop.resubmit(&Loop::new(&lab), "e2e7").unwrap();
    assert_eq!(retry.outcome, SubmitOutcome::Enqueued { tsp_id: 1 });
    assert_eq!(retry.pal.nprocs, 1);
    assert_eq!(fs::read_to_string(remote_job(&lab, "e2e7").join("input.inp")).unwrap(), "! HF\n%pal nprocs 1 end\n");
    assert_eq!(laptop.job("e2e7").input_content, input, "the database keeps the user's original");
}

/// NEGATIVE CONTROL target (d), end to end: `<root>/bin` is a symlink to a directory that holds
/// the right scripts. The prepare call's facts refuse the upload at the prepare step, naming
/// `bin`, and nothing reaches the job dir. Drop `bin` from `check_prepare`'s list and this goes
/// red (the attempt gets past prepare and fails at the install instead).
#[test]
fn a_symlinked_bin_refuses_the_upload_at_the_prepare_step() {
    let _serial = serial();
    let lab = Lab::new();
    let real_bin = lab.root.join("real-bin");
    fs::rename(lab.root.join("bin"), &real_bin).unwrap();
    std::os::unix::fs::symlink(&real_bin, lab.root.join("bin")).unwrap();
    let laptop = Laptop::new(&lab);
    laptop.draft("e2e5");
    let runner = Loop::new(&lab);

    let outcome = laptop.submit(&runner, "e2e5");
    assert!(
        matches!(&outcome, SubmitOutcome::NotClaimed { step: SubmitStep::Prepare, reason } if reason.contains("bin")),
        "{outcome:?}"
    );
    assert_eq!(*runner.calls.borrow(), ["prepare"]);
    assert!(!remote_job(&lab, "e2e5").exists(), "nothing was uploaded");
    assert_eq!(laptop.job("e2e5").status, JobStatus::Queued);
    assert_eq!(label_remote(&runner, &laptop.job("e2e5")).unwrap().label, Label::NotOnServer);
}

// ---- the prepare, install, mkjob and run scripts on their own -------------------------------

fn prepare(lab: &Lab, root: &Path) -> Prepared {
    let sent = PrepareArgs::new(Lab::str(root), &format!("{}/jobs/j1", Lab::str(root))).unwrap();
    let out = run(lab, PREPARE, &sent.values(), &[]);
    let facts = parse_prepare_reply(&out.stdout, &sent).unwrap_or_else(|e| panic!("{e}: {:?}", String::from_utf8_lossy(&out.stdout)));
    check_prepare(&facts, &sent).unwrap()
}

fn install(lab: &Lab, sent: &InstallArgs) -> Result<[Installed; 3], String> {
    let out = run(lab, INSTALL, &sent.values(), &[]);
    parse_install_reply(&out.stdout, sent).map_err(|e| e.to_string())
}

fn temps_in(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.'))
        .collect()
}

#[test]
fn install_readies_a_fresh_root_and_replaces_a_wrong_script_by_rename() {
    let lab = Lab::new();
    let root = lab.root.join("fresh");
    let p = prepare(&lab, &root);
    assert!(!p.ready() && !p.bin_exists && !p.tsp_exists, "{p:?}");
    assert_eq!(p.missing(), ["wrapper", "cancel", "collect"]);

    let sent = InstallArgs::new(Lab::str(&root)).unwrap();
    use Installed::{Installed as New, Kept};
    assert_eq!(install(&lab, &sent), Ok([New, New, New]));
    assert!(prepare(&lab, &root).ready());
    assert_eq!(install(&lab, &sent), Ok([Kept, Kept, Kept]), "right scripts are left as they are");

    // Wrong bytes under the cancel script's name: replaced by rename, so a process holding the old
    // file keeps its own inode; the others are kept.
    let cancel = root.join("bin").join(format!("cancel-{}.sh", sent.scripts[1].0));
    fs::write(&cancel, "echo not the cancel script\n").unwrap();
    let held = fs::File::open(&cancel).unwrap();
    assert_eq!(prepare(&lab, &root).missing(), ["cancel"]);
    assert_eq!(install(&lab, &sent), Ok([Kept, New, Kept]));
    assert!(prepare(&lab, &root).ready());
    use std::os::unix::fs::MetadataExt;
    assert_ne!(held.metadata().unwrap().ino(), fs::metadata(&cancel).unwrap().ino(), "a new inode, not an overwrite");
    assert!(temps_in(&root.join("bin")).is_empty(), "no temp left");
}

/// NEGATIVE CONTROL of the install's own sha check: bytes that do not hash to the name are never
/// published under it, and no temp is left behind. (The Rust post-condition — the prepare call
/// after the install — is the second layer, `ssh_backend` test
/// `an_install_that_leaves_the_wrapper_wrong_uploads_nothing`.)
#[test]
fn install_never_publishes_bytes_that_do_not_hash_to_the_name() {
    let lab = Lab::new();
    let root = lab.root.join("fresh");
    let mut sent = InstallArgs::new(Lab::str(&root)).unwrap();
    sent.scripts[0].1.push('\n');
    let err = install(&lab, &sent).unwrap_err();
    assert!(err.contains("hash to"), "{err}");
    assert!(!root.join("bin").join(format!("wrapper-{}.sh", sent.scripts[0].0)).exists());
    assert!(temps_in(&root.join("bin")).is_empty(), "the temp is removed");
}

#[test]
fn install_refuses_a_symlinked_bin_or_tsp_and_writes_nothing_through_it() {
    for dir in ["bin", "tsp"] {
        let lab = Lab::new();
        let root = lab.root.join("fresh");
        let elsewhere = lab.root.join("elsewhere");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join(dir)).unwrap();
        let err = install(&lab, &InstallArgs::new(Lab::str(&root)).unwrap()).unwrap_err();
        assert!(err.contains(dir), "{dir}: {err}");
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0, "{dir}: nothing written through the symlink");
    }
}

#[test]
fn mkjob_makes_the_dir_and_refuses_a_symlinked_component_after_the_mkdir() {
    let lab = Lab::new();
    let mk_with = |script: &str, job: &str| {
        let sent = MkjobArgs::new(Lab::str(&lab.root), job).unwrap();
        parse_mkjob_reply(&run(&lab, script, &sent.values(), &[]).stdout, &sent)
    };
    let mk = |job: &str| mk_with(MKJOB, job);
    let job = remote_job(&lab, "w1");
    mk(Lab::str(&job)).unwrap();
    assert!(job.is_dir());
    mk(Lab::str(&job)).unwrap();
    // A job dir that is a symlink to a directory elsewhere: mkdir -p succeeds, the shape does not.
    let elsewhere = lab.root.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let linked = remote_job(&lab, "w2");
    std::os::unix::fs::symlink(&elsewhere, &linked).unwrap();
    assert!(mk(Lab::str(&linked)).is_err());
    // A sibling link inside jobs/ (jobs/w3 -> jobs/w1): the parent's realpath is still <root>/jobs,
    // so only the job dir's own realpath catches it — a withdraw of w3 must never publish
    // .cancelled into w1 (verifier MED-1).
    let sibling = remote_job(&lab, "w3");
    std::os::unix::fs::symlink(&job, &sibling).unwrap();
    assert!(mk(Lab::str(&sibling)).is_err(), "jobs/w3 -> jobs/w1 is refused");
    // NEGATIVE CONTROL (permanent): a mkjob that reports the job dir as given instead of its realpath
    // lets the sibling link through.
    let blind = mutate(MKJOB, "    real job \"$job\"\n", "    emit_text job \"$job\"\n");
    assert!(mk_with(&blind, Lab::str(&sibling)).is_ok(), "the mutant must miss it, or this case guards nothing");
}

/// The test-only variant of the trampoline: its allow-list gains `probe`, a script that echoes its
/// argv and its stdin. The shipped allow-list stays exactly {cancel, collect}.
const SHIPPED_ALLOW_LIST: &str = "declare -A BUDGET=([cancel]=20 [collect]=15)";

/// The probe: each argv value as a length-framed record, then what it read on stdin, exit 7.
const PROBE: &str = "for a in \"$@\"; do printf 'arg %s\\n%s\\n' \"${#a}\" \"$a\"; done\nin=$(cat)\nprintf 'stdin %s\\n' \"${#in}\"\nexit 7\n";

fn run_reply(lab: &Lab, script: &str, sent: &RunArgs) -> RunReply {
    let out = run(lab, script, &sent.values(), &[]);
    parse_run_reply(&out.stdout, sent).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stdout)))
}

fn raw(root: &Path, name: &str, sha: &str, args: &[&str]) -> RunArgs {
    RunArgs { root: Lab::str(root).into(), name: name.into(), sha: sha.into(), args: args.iter().map(|a| a.to_string()).collect() }
}

#[test]
fn the_shipped_trampoline_allows_exactly_cancel_and_collect() {
    assert_eq!(RUN.matches(SHIPPED_ALLOW_LIST).count(), 1);
    assert_eq!(RUN.matches("BUDGET=(").count(), 1, "no second allow-list assignment");
    assert_eq!(RUN.matches("BUDGET[").count(), 2, "BUDGET is only read: the name check and the budget");
}

/// o 14.1's controls on the real trampoline. NEGATIVE CONTROLS: each refusal is a guard of the
/// script; the `refuses` cases go red if its check is removed (shown once by mutation, log.md).
#[test]
fn the_trampoline_runs_only_an_allowed_installed_script_by_its_sha() {
    let lab = Lab::new();
    let root = &lab.root; // Lab::new installs wrapper, cancel and collect under <root>/bin by sha.
    let cancel_sha = sha256_hex(CANCEL);
    let refused = |reply: RunReply, case: &str| match reply {
        RunReply::Refused(why) => why,
        other => panic!("{case}: expected a refusal, got {other:?}"),
    };

    // The wrapper is outside the allow-list: it would start ORCA outside tsp.
    let why = refused(run_reply(&lab, RUN, &raw(root, "wrapper", &sha256_hex(WRAPPER), &["/x", "0", "/o"])), "wrapper");
    assert!(why.contains("not an allowed script"), "{why}");
    assert_eq!(fs::read_to_string(lab.ran_log()).unwrap_or_default(), "", "no ORCA ran");
    for (case, name) in [("a path", "../bin/cancel"), ("upper case", "Cancel"), ("empty", "")] {
        refused(run_reply(&lab, RUN, &raw(root, name, &cancel_sha, &[])), case);
    }
    for (case, sha) in [("short", "abc"), ("upper", &cancel_sha.to_uppercase()), ("a path", "../../etc/passwd")] {
        let why = refused(run_reply(&lab, RUN, &raw(root, "cancel", sha, &[])), case);
        assert!(why.contains("64 lowercase hex"), "{case}: {why}");
    }
    // Missing: a distinct "not installed".
    assert_eq!(run_reply(&lab, RUN, &raw(root, "collect", &"a".repeat(64), &[])), RunReply::NotInstalled);
    // Bytes that are not the ones the name carries.
    let fake_sha = "b".repeat(64);
    fs::write(root.join("bin").join(format!("cancel-{fake_sha}.sh")), "echo forged\n").unwrap();
    let why = refused(run_reply(&lab, RUN, &raw(root, "cancel", &fake_sha, &[])), "forged bytes");
    assert!(why.contains("sha256"), "{why}");
    // A symlinked script, and a symlinked bin/ holding the right bytes.
    let elsewhere = lab.root.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let target = elsewhere.join("cancel.sh");
    fs::write(&target, CANCEL).unwrap();
    let link = root.join("bin").join(format!("cancel-{cancel_sha}.sh"));
    fs::rename(&link, root.join("cancel.bak")).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    refused(run_reply(&lab, RUN, &raw(root, "cancel", &cancel_sha, &[])), "symlinked script");
    let other_root = lab.root.join("r2");
    fs::create_dir_all(&other_root).unwrap();
    fs::copy(root.join("cancel.bak"), elsewhere.join(format!("cancel-{cancel_sha}.sh"))).unwrap();
    std::os::unix::fs::symlink(&elsewhere, other_root.join("bin")).unwrap();
    let why = refused(run_reply(&lab, RUN, &raw(&other_root, "cancel", &cancel_sha, &[])), "symlinked bin");
    assert!(why.contains("symlink"), "{why}");
}

/// A script that reads stdin gets EOF; a non-zero exit is a complete reply carrying that rc; an
/// argument with `'`, `$`, a space and a newline arrives byte-identical. Observed with a
/// **test-only** allow-list entry (`probe`): the shipped trampoline is mutated in this test only.
#[test]
fn the_trampoline_passes_values_verbatim_closes_stdin_and_reports_the_rc() {
    let lab = Lab::new();
    let variant = mutate(RUN, SHIPPED_ALLOW_LIST, "declare -A BUDGET=([cancel]=20 [collect]=15 [probe]=5)");
    let sha = sha256_hex(PROBE);
    fs::write(lab.root.join("bin").join(format!("probe-{sha}.sh")), PROBE).unwrap();
    let hostile = "it's $HOME; a b\nnext `id` *";
    let reply = run_reply(&lab, &variant, &raw(&lab.root, "probe", &sha, &[hostile, "", "-x"]));
    let RunReply::Ran { rc, stdout, stderr } = reply else { panic!("{reply:?}") };
    assert_eq!(rc, 7, "the script's rc, in a complete reply");
    let want = format!("arg {}\n{hostile}\narg 0\n\narg 2\n-x\nstdin 0\n", hostile.len());
    assert_eq!(String::from_utf8(stdout).unwrap(), want, "every value byte-identical; stdin at EOF");
    assert_eq!(stderr, b"");
    // The shipped trampoline refuses the same call.
    let shipped = run_reply(&lab, RUN, &raw(&lab.root, "probe", &sha, &[hostile]));
    assert!(matches!(shipped, RunReply::Refused(_)), "{shipped:?}");
}

/// The real cancel and collect, through the trampoline, as a withdraw runs them.
#[test]
fn cancel_and_collect_run_through_the_trampoline() {
    let lab = Lab::new();
    let job = lab.job("t1");
    let root = Lab::str(&lab.root);
    let cancel = RunArgs::new(root, JobScript::Cancel, vec!["cancel".into(), Lab::str(&job).into(), root.into()]).unwrap();
    let RunReply::Ran { rc, stdout, .. } = run_reply(&lab, RUN, &cancel) else { panic!("cancel did not run") };
    assert_eq!(rc, 0, "{}", String::from_utf8_lossy(&stdout));
    assert!(job.join(".cancelled").exists());
    let sock = lab.root.join("slot.sock");
    let collect = RunArgs::new(root, JobScript::Collect, vec![Lab::str(&job).into(), Lab::str(&sock).into()]).unwrap();
    let RunReply::Ran { rc, stdout, .. } = run_reply(&lab, RUN, &collect) else { panic!("collect did not run") };
    assert_eq!(rc, 0);
    assert!(stdout.starts_with(b"orcastudio-snapshot 1\n"), "the stdout record is the snapshot, verbatim");
}

// ---- the poller against the real scripts (unit 5.3 B2) ---------------------------------------

/// What "the wrapper" leaves in the remote job dir of a clean run: `output.out` (its last line
/// unterminated), an empty `stderr.log`, `.exit_code` 0.
const FINISHED_OUTPUT: &str = "SCF ITERATIONS\nFINAL SINGLE POINT ENERGY      -76.026760\n\
                             ****ORCA TERMINATED NORMALLY****\nTOTAL RUN TIME: 0 days 0 hours 0 minutes 2 seconds 5 msec";

fn finish_on_server(lab: &Lab, id: &str) {
    let job = remote_job(lab, id);
    fs::write(job.join("output.out"), FINISHED_OUTPUT).unwrap();
    fs::write(job.join("stderr.log"), "").unwrap();
    fs::write(job.join(".exit_code"), "0\n").unwrap();
}

/// The poller's world: its memory, the live log, a recording sink, the guard set.
struct PollerWorld {
    in_flight: crate::in_flight::InFlight,
    memory: crate::poller::plan::PollerMemory,
    live: crate::poller::live_log::LiveLog,
    sink: crate::poller::RecordingSink,
}

impl PollerWorld {
    fn new() -> Self {
        PollerWorld { in_flight: Default::default(), memory: Default::default(), live: Default::default(), sink: Default::default() }
    }

    fn poller<'a>(&'a self, laptop: &'a Laptop, runner: &'a dyn CommandRunner) -> crate::poller::Poller<'a> {
        crate::poller::Poller {
            db: &laptop.db,
            runner,
            in_flight: &self.in_flight,
            memory: &self.memory,
            live: &self.live,
            sink: &self.sink,
            policy: crate::execution_backend::FetchPolicy::SMALL_ONLY,
        }
    }
}

/// The whole remote finish against the real scripts: the real `poll_log` streams the start of the
/// log into a watching view; the status step's real label call hands the job to the classifier, the
/// real collector (through the trampoline) says `Completed`, the real rsync brings the files down and
/// the real `list` script's hashes match the copy; `detect_completion` over the copy finalises the
/// row, and the view gets the rest of the log — its unterminated last line included — before the
/// terminal `job:status`.
#[test]
fn the_poller_streams_fetches_and_finalises_against_the_real_scripts() {
    use crate::poller::{log_step, Event, LogStep, StatusStep};
    let _serial = serial();
    let lab = Lab::new();
    let laptop = Laptop::new(&lab);
    laptop.draft("e2e8");
    assert_eq!(laptop.submit(&Loop::new(&lab), "e2e8"), SubmitOutcome::Enqueued { tsp_id: 0 });
    finish_on_server(&lab, "e2e8");
    let coords = coordinates(&laptop.job("e2e8")).unwrap().unwrap();

    let world = PollerWorld::new();
    let runner = Loop::new(&lab);
    world.live.open("e2e8", &world.sink);
    let step = log_step(&world.live, &runner, &world.sink, "e2e8", &coords);
    assert!(matches!(step, LogStep::Applied(crate::poller::live_log::Applied::Lines { lines: 3, .. })), "{step:?}");

    let step = world.poller(&laptop, &runner).status_step("e2e8").unwrap();
    assert!(
        matches!(step, StatusStep::Finalised { outcome: Outcome::Completed { late_cancel: false }, status: JobStatus::Completed, drain_error: None }),
        "{step:?}"
    );
    assert_eq!(*runner.calls.borrow(), ["poll_log", "label", "run", "rsync", "list"]);

    let job = laptop.job("e2e8");
    assert_eq!((job.status, job.error_message.as_deref(), job.energy, job.wall_time), (JobStatus::Completed, None, Some(-76.026760), Some(2.005)));
    let local = laptop.data_dir.join("jobs").join("e2e8");
    assert_eq!(fs::read(local.join("output.out")).unwrap(), fs::read(remote_job(&lab, "e2e8").join("output.out")).unwrap());
    assert!(local.join(".submitting").is_symlink(), "the claim came down as a symlink");
    let events = world.sink.take();
    let lines: Vec<String> = events.iter().flat_map(|e| match e { Event::Log(_, l) => l.clone(), _ => vec![] }).collect();
    assert_eq!(lines, FINISHED_OUTPUT.lines().map(String::from).collect::<Vec<_>>());
    assert_eq!(events.last(), Some(&Event::Status("e2e8".into(), JobStatus::Completed)));
}

/// The download post-condition against the real `list` script: a copy that differs from the
/// server's file after a successful rsync is a failed fetch — the row stays `queued`, nothing is
/// finalised, a strike is shown.
#[test]
fn a_copy_that_differs_from_the_real_listing_is_not_finalised() {
    use crate::poller::StatusStep;
    let _serial = serial();
    let lab = Lab::new();
    let laptop = Laptop::new(&lab);
    laptop.draft("e2e9");
    assert_eq!(laptop.submit(&Loop::new(&lab), "e2e9"), SubmitOutcome::Enqueued { tsp_id: 0 });
    finish_on_server(&lab, "e2e9");
    let local = laptop.data_dir.join("jobs").join("e2e9");
    let corrupt = || fs::write(local.join("output.out"), FINISHED_OUTPUT.replace("SCF", "SCX")).unwrap();
    let runner = Loop { after_rsync: Some(Box::new(corrupt)), ..Loop::new(&lab) };

    let world = PollerWorld::new();
    let step = world.poller(&laptop, &runner).status_step("e2e9").unwrap();
    let StatusStep::FetchFailed { strikes: 1, reason } = &step else { panic!("{step:?}") };
    assert!(reason.contains("differing [\"output.out\"]"), "{reason}");
    assert_eq!(*runner.calls.borrow(), ["label", "run", "rsync", "list"]);
    let job = laptop.job("e2e9");
    assert_eq!(job.status, JobStatus::Queued);
    assert!(job.error_message.unwrap().contains("attempt 1 of 3"));
}
