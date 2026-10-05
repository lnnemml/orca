//! Tests that run the **real** stdin-fed scripts of unit 5.3 (ADR-024 o items 3, 6, 7) through
//! `bash -s` and read their output with the **Rust parsers** — never against a hand-written
//! expected string, so a script that drifts from its parser fails here.
//!
//! The harness is the 5.2 [`Lab`] (`script_tests.rs`): a stub `tsp` (it also enqueues, starts a
//! stub daemon, and records any call that inherited fd 9), a stub `busctl`, a stub ORCA. `HOME` is
//! the lab root, so each lab has its own account lock.
//!
//! **The slot check scans this machine's real processes** (own uid), so the tests that run the
//! submit or create pinned processes or `tsp/slot<N>.sock` listeners hold [`SERIAL`], and the slot
//! mask avoids every CPU the 5.2 tests pin (0, 0-3, 0,2,4-7, 12-23): `8-11` on a host with ≥ 12
//! CPUs.
//!
//! Each guard has a **negative control** that runs the same check on a mutated copy of the script
//! (exactly one occurrence replaced) and requires it to fail.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard};

use super::classify::Outcome;
use super::poll::{parse_poll_reply, PollError, PollLogArgs};
use super::script_tests::{
    is_dead, new_session, stat_of, tracked, tsp_listing, wait_for, Lab,
};
use super::scripts::{stdin_with_values, LABEL, LIST, POLL_LOG, STDIN_SCRIPTS, SUBMIT, WRAPPER};
use super::submit::{
    label, parse_label_reply, parse_submit_reply, Label, LabelArgs, LabelFacts, Markers, SubmitArgs,
    SubmitReply,
};
use super::sync::{compare_download, download_selects, list_dir, parse_list_reply, upload_expected, ListArgs};
use super::wire::WireError;
use crate::execution_backend::{FetchPolicy, LogChunk};

static SERIAL: Mutex<()> = Mutex::new(());

/// The machine-wide serialisation of the tests whose scan sees, or whose processes are seen by,
/// another submit test. A panic in one test must not fail the rest.
fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// The slot's CPUs: 8-11, clear of every mask the 5.2 tests pin.
fn slot_cpus() -> (usize, usize) {
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    if n >= 12 {
        (8, 11)
    } else {
        assert!(n >= 2, "the slot-check tests need at least 2 CPUs");
        (n - 1, n - 1)
    }
}

fn mask() -> String {
    match slot_cpus() {
        (a, b) if a == b => a.to_string(),
        (a, b) => format!("{a}-{b}"),
    }
}

/// Run `script` as `bash -s` with `values` after it, as the ssh call will (n item 11).
fn run(lab: &Lab, script: &str, values: &[String], env: &[(&str, &str)]) -> Output {
    let stdin = stdin_with_values(script, values).unwrap();
    let mut child = Command::new("bash")
        .arg("-s")
        .env("PATH", lab.path_env())
        .env("HOME", &lab.root)
        .envs(env.iter().copied())
        .current_dir(&lab.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&stdin).unwrap();
    child.wait_with_output().unwrap()
}

/// `script` with exactly one occurrence of `from` replaced: a negative control's mutant. Panics
/// if `from` is not there exactly once, so a control can never pass by mutating nothing.
fn mutate(script: &str, from: &str, to: &str) -> String {
    assert_eq!(script.matches(from).count(), 1, "the mutation target {from:?} must occur once");
    script.replacen(from, to, 1)
}

fn text(out: &Output) -> String {
    format!("stdout {:?}\nstderr {}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

#[test]
fn stdin_scripts_pass_bash_n() {
    for script in STDIN_SCRIPTS {
        let mut child = Command::new("bash").arg("-n").stdin(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
}

// ---- submit ----------------------------------------------------------------------------------

/// The lab's slot socket, `<root>/tsp/slot<n>.sock` (the layout the slot check recognises).
fn slot_socket(lab: &Lab, n: u32) -> PathBuf {
    fs::create_dir_all(lab.root.join("tsp")).unwrap();
    lab.root.join("tsp").join(format!("slot{n}.sock"))
}

fn lock_path(lab: &Lab) -> PathBuf {
    lab.root.join(".orcastudio-submit.lock")
}

/// A job dir as the upload leaves it (an input, a geometry, a file in a subdirectory) and the
/// submit values for it on `socket`.
fn prepared(lab: &Lab, name: &str, socket: &Path) -> (PathBuf, SubmitArgs) {
    let job = lab.job(name);
    fs::write(job.join("input.xyz"), "1\n\nH 0 0 0\n").unwrap();
    fs::create_dir_all(job.join("aux")).unwrap();
    fs::write(job.join("aux").join("product.xyz"), "1\n\nH 0 0 1\n").unwrap();
    let args = SubmitArgs::new(
        Lab::str(&lab.root),
        name,
        Lab::str(socket),
        &mask(),
        Lab::str(&lab.stub_dir.join("orca")),
        upload_expected(&job).unwrap(),
    )
    .unwrap();
    assert_eq!(
        format!("{}/bin/wrapper-{}.sh", Lab::str(&lab.root), args.wrapper_sha),
        Lab::str(&lab.wrapper),
        "the script's wrapper path, built from the sha, is the uploaded one"
    );
    (job, args)
}

fn submit_with(lab: &Lab, script: &str, args: &SubmitArgs, env: &[(&str, &str)]) -> SubmitReply {
    let out = run(lab, script, &args.values().unwrap(), env);
    parse_submit_reply(&out.stdout, args).unwrap_or_else(|e| panic!("{e}\n{}", text(&out)))
}

fn submit(lab: &Lab, args: &SubmitArgs) -> SubmitReply {
    submit_with(lab, SUBMIT, args, &[])
}

/// The enqueue lines of the stub tsp's log (every call that is not `-l`/`-r`).
fn enqueues(lab: &Lab) -> Vec<String> {
    lab.tsp_log().lines().filter(|l| l.contains(" bash ")).map(String::from).collect()
}

fn fd9_log(lab: &Lab) -> String {
    fs::read_to_string(lab.stub_dir.join("tsp.fd9.log")).unwrap_or_default()
}

/// `flock -n` on the account lock: true if nobody holds it.
fn lock_is_free(lab: &Lab) -> bool {
    Command::new("flock").arg("-n").arg(lock_path(lab)).arg("true").status().unwrap().success()
}

fn refused(reply: &SubmitReply) -> &str {
    match reply {
        SubmitReply::Refused(why) => why,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn submit_enqueues_claims_and_publishes_enqueued() {
    let _serial = serial();
    let lab = Lab::new();
    let sock = slot_socket(&lab, 0);
    let (job, args) = prepared(&lab, "happy", &sock);

    assert_eq!(submit(&lab, &args), SubmitReply::Enqueued(0));

    let job_s = Lab::str(&job);
    assert_eq!(fs::read_link(job.join(".submitting")).unwrap(), Path::new("x"), "the claim is a symlink to x");
    assert_eq!(fs::read_to_string(job.join(".enqueued")).unwrap(), format!("socket={}\nid=0\n", Lab::str(&sock)));
    assert!(job.join(".tsp-out").is_dir());
    assert_eq!(
        enqueues(&lab),
        [format!("{} bash {} {job_s} {} {}", Lab::str(&sock), Lab::str(&lab.wrapper), args.core_mask, args.orca_path)],
        "tsp runs `bash <wrapper> <job> <mask> <orca>`, argv verbatim"
    );
    assert_eq!(
        fs::read_to_string(format!("{}.tmpdir", Lab::str(&sock))).unwrap(),
        format!("{job_s}/.tsp-out\n"),
        "TMPDIR is the job's .tsp-out (m item 1)"
    );
    assert!(lock_is_free(&lab), "the lock is released when the script ends");
    assert_eq!(fd9_log(&lab), "", "no tsp call inherited the lock fd");

    // The 5.2 collector and classifier see the new row: Queued.
    assert_eq!(lab.classify(&job, std::slice::from_ref(&sock)), Outcome::Queued);
}

#[test]
fn a_second_submit_of_the_same_job_is_refused_and_enqueues_nothing() {
    let _serial = serial();
    let lab = Lab::new();
    let sock = slot_socket(&lab, 0);
    let (job, args) = prepared(&lab, "twice", &sock);
    assert_eq!(submit(&lab, &args), SubmitReply::Enqueued(0));
    let why = submit(&lab, &args);
    assert!(refused(&why).starts_with("marker: "), "{why:?}");
    assert_eq!(enqueues(&lab).len(), 1, "exactly one enqueue");
    assert_eq!(fs::read_to_string(job.join(".enqueued")).unwrap(), format!("socket={}\nid=0\n", Lab::str(&sock)));
}

/// o item 3.3 negative control: two submits of one job started together — the lock serialises
/// them, and the second sees the first one's claim or row.
#[test]
fn two_concurrent_submits_of_one_job_enqueue_it_once() {
    let _serial = serial();
    let lab = Lab::new();
    let sock = slot_socket(&lab, 0);
    let (_job, args) = prepared(&lab, "concurrent", &sock);
    let values = args.values().unwrap();
    let (a, b) = std::thread::scope(|s| {
        let a = s.spawn(|| run(&lab, SUBMIT, &values, &[]));
        let b = s.spawn(|| run(&lab, SUBMIT, &values, &[]));
        (a.join().unwrap(), b.join().unwrap())
    });
    let mut replies = [parse_submit_reply(&a.stdout, &args).unwrap(), parse_submit_reply(&b.stdout, &args).unwrap()];
    replies.sort_by_key(|r| matches!(r, SubmitReply::Refused(_)));
    assert!(matches!(replies[0], SubmitReply::Enqueued(_)), "{replies:?}");
    assert!(refused(&replies[1]).starts_with("marker: "), "{replies:?}");
    assert_eq!(enqueues(&lab).len(), 1);
}

/// The no-clobber claim, checked through a race the lock cannot stop: a `.submitting` that
/// appears after the marker check (another writer than a submit — a fault-injected `mkdir` makes
/// it at step 6). `Ok` iff the submit refused at the claim and enqueued nothing.
fn claim_race(script: &str) -> Result<(), String> {
    let _serial = serial();
    let lab = Lab::new();
    let sock = slot_socket(&lab, 0);
    let (job, args) = prepared(&lab, "race", &sock);
    let fault = lab.root.join("fault");
    fs::create_dir_all(&fault).unwrap();
    fs::write(
        fault.join("mkdir"),
        "#!/bin/bash\nln -s y \"$FAULT_CLAIM\" 2>/dev/null\nPATH=${PATH#*:} exec mkdir \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(fault.join("mkdir"), fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", Lab::str(&fault), lab.path_env());
    let claim = job.join(".submitting");
    let reply = submit_with(&lab, script, &args, &[("PATH", &path), ("FAULT_CLAIM", Lab::str(&claim))]);
    let target = fs::read_link(&claim).map_err(|e| e.to_string())?;
    match reply {
        SubmitReply::Refused(why) if why.starts_with("claim: ") && enqueues(&lab).is_empty() && target == Path::new("y") => {
            Ok(())
        }
        other => Err(format!("{other:?}; enqueues {:?}; .submitting -> {target:?}", enqueues(&lab))),
    }
}

#[test]
fn the_claim_is_no_clobber() {
    claim_race(SUBMIT).unwrap();
}

/// NEGATIVE CONTROL (b): a claim that overwrites (`ln -sfT`) lets the second claimer enqueue.
#[test]
fn an_overwriting_claim_turns_the_claim_race_red() {
    let broken = mutate(SUBMIT, "ln -sT x \"$job/.submitting\"", "ln -sfT x \"$job/.submitting\"");
    let red = claim_race(&broken).unwrap_err();
    eprintln!("control (b) red: {red}");
}

/// The lock fd never reaches tsp or its daemon (o item 3.3.1, probe 5.3c). Two submits on a fresh
/// socket: the first starts the stub daemon (which inherits the client's fds, as tsp's does), the
/// second runs `tsp -l` on the now-live slot. `Ok` iff no tsp call saw fd 9 and the lock is free
/// while the daemon still runs.
fn lock_fd_stays_out_of_tsp(script: &str) -> Result<(), String> {
    let _serial = serial();
    let lab = Lab::new();
    let sock = slot_socket(&lab, 0);
    let (_a, first) = prepared(&lab, "fd-a", &sock);
    let (_b, second) = prepared(&lab, "fd-b", &sock);
    let r1 = submit_with(&lab, script, &first, &[]);
    let r2 = submit_with(&lab, script, &second, &[]);
    let daemons = fs::read_to_string(lab.stub_dir.join("daemon.pids")).unwrap_or_default();
    let daemon: u32 = daemons.lines().next().ok_or("no daemon was started")?.parse().unwrap();
    let alive = stat_of(daemon).is_some_and(|s| !s.is_zombie());
    let free = lock_is_free(&lab);
    let fd9 = fd9_log(&lab);
    if r1 == SubmitReply::Enqueued(0) && r2 == SubmitReply::Enqueued(1) && alive && free && fd9.is_empty() {
        Ok(())
    } else {
        Err(format!("replies {r1:?} {r2:?}; daemon alive {alive}; lock free {free}; tsp calls with fd 9: {fd9:?}"))
    }
}

#[test]
fn the_lock_fd_never_reaches_tsp_or_its_daemon() {
    lock_fd_stays_out_of_tsp(SUBMIT).unwrap();
}

/// NEGATIVE CONTROL (a): without `9>&-` on the enqueue, the daemon tsp starts keeps the account
/// lock after the script ends; without it on `tsp -l`, that call sees fd 9.
#[test]
fn a_tsp_call_without_closing_the_lock_fd_turns_the_check_red() {
    let enqueue = mutate(SUBMIT, "2>\"$T/enqueue.err\" 9>&-", "2>\"$T/enqueue.err\"");
    let red = lock_fd_stays_out_of_tsp(&enqueue).unwrap_err();
    assert!(red.contains("lock free false"), "{red}");
    eprintln!("control (a) enqueue red: {red}");
    let list = mutate(SUBMIT, "2>\"$T/slot_rows.err\" 9>&-", "2>\"$T/slot_rows.err\"");
    let red = lock_fd_stays_out_of_tsp(&list).unwrap_err();
    assert!(red.contains("-l"), "{red}");
    eprintln!("control (a) tsp -l red: {red}");
}

#[test]
fn a_busy_lock_is_refused_naming_its_holder_and_nothing_is_enqueued() {
    let mut lab = Lab::new();
    let sock = slot_socket(&lab, 0);
    let (job, args) = prepared(&lab, "busy", &sock);
    let ready = lab.root.join("holder.ready");
    let mut holder = Command::new("perl");
    holder
        .arg("-MFcntl=:flock")
        .arg("-e")
        .arg("open my $l, '>', $ARGV[0] or die; flock($l, LOCK_EX) or die; open my $r, '>', $ARGV[1] or die; close $r; sleep 40")
        .arg(lock_path(&lab))
        .arg(&ready)
        .stdin(Stdio::null());
    new_session(&mut holder);
    let pid = lab.spawn(holder);
    wait_for("the holder took the lock", || ready.exists());

    let started = std::time::Instant::now();
    let reply = submit(&lab, &args);
    let why = refused(&reply);
    assert!(started.elapsed() >= std::time::Duration::from_secs(19), "flock -w 20 waited");
    assert!(why.starts_with("lock busy: "), "{why}");
    assert!(why.starts_with("lock busy: lock file open in: "), "{why}");
    assert!(why.contains(&format!("{pid} perl -MFcntl=:flock")), "the opener is listed: {why}");
    assert!(enqueues(&lab).is_empty());
    assert!(!job.join(".submitting").exists() && !job.join(".tsp-out").exists());
}

/// The upload post-condition (o item 3.3.4). `Ok` iff each broken upload is refused, naming the
/// file, with nothing claimed or enqueued.
fn upload_check(script: &str) -> Result<(), String> {
    let _serial = serial();
    let lab = Lab::new();
    let sock = slot_socket(&lab, 0);
    let cases: [(&str, &dyn Fn(&Path), &str); 4] = [
        ("one-byte", &|j: &Path| fs::write(j.join("input.xyz"), "1\n\nH 0 0 1\n").unwrap(), "differing [input.xyz]"),
        ("deep-byte", &|j: &Path| fs::write(j.join("aux/product.xyz"), "x").unwrap(), "differing [aux/product.xyz]"),
        ("missing", &|j: &Path| fs::remove_file(j.join("input.xyz")).unwrap(), "missing [input.xyz]"),
        ("extra", &|j: &Path| fs::write(j.join(".input.inp.Ab12Cd"), "").unwrap(), "extra [.input.inp.Ab12Cd]"),
    ];
    for (name, damage, want) in cases {
        let (job, args) = prepared(&lab, name, &sock);
        damage(&job);
        let reply = submit_with(&lab, script, &args, &[]);
        let ok = matches!(&reply, SubmitReply::Refused(why) if why.starts_with("upload: ") && why.contains(want));
        if !ok || job.join(".submitting").exists() || !enqueues(&lab).is_empty() {
            return Err(format!("{name}: {reply:?}"));
        }
    }
    Ok(())
}

#[test]
fn a_broken_upload_is_refused_naming_the_file() {
    upload_check(SUBMIT).unwrap();
}

/// NEGATIVE CONTROL (d): without the sha256 comparison a corrupted byte is enqueued.
#[test]
fn skipping_the_sha_check_turns_the_upload_check_red() {
    let broken = mutate(SUBMIT, "[[ ${hashed[$name]-} == \"${expected[$name]}\" ]] || differing+=(\"$name\")", "true");
    let red = upload_check(&broken).unwrap_err();
    assert!(red.starts_with("one-byte: Enqueued"), "{red}");
    eprintln!("control (d) red: {red}");
}

#[test]
fn submit_refuses_before_the_claim_for_each_failed_precondition() {
    let _serial = serial();
    let mut lab = Lab::new();
    let sock = slot_socket(&lab, 0);

    let (job, args) = prepared(&lab, "kup", &sock);

    // A job dir reached through a symlink (o item 1, probe 5.3c).
    let real = lab.job("real");
    std::os::unix::fs::symlink(&real, lab.root.join("jobs").join("link")).unwrap();
    let link_args = SubmitArgs { job_dir: format!("{}/jobs/link", Lab::str(&lab.root)), ..args.clone() };
    let why = submit(&lab, &link_args);
    assert!(refused(&why).starts_with("realpath: "), "{why:?}");
    assert!(!real.join(".submitting").exists());

    // The job dir must be <root>/jobs/<id>, checked by the script itself too.
    let outside = SubmitArgs { job_dir: format!("{}/elsewhere/x", Lab::str(&lab.root)), ..args.clone() };
    assert!(refused(&submit(&lab, &outside)).starts_with("values: "));

    // Every marker, in any form, refuses (a dangling .started symlink too).
    for marker in [".started", ".enqueued", ".exit_code", ".cancelled", ".submitting"] {
        std::os::unix::fs::symlink("nowhere", job.join(marker)).unwrap();
        let why = submit(&lab, &args);
        assert_eq!(refused(&why), format!("marker: {}/{marker} exists", Lab::str(&job)));
        fs::remove_file(job.join(marker)).unwrap();
    }

    // A row on the live slot that already holds the job dir.
    let row = format!("4    queued     (file)                                       bash {} {} {} /o", Lab::str(&lab.wrapper), Lab::str(&job), mask());
    lab_listen_with_rows(&mut lab, &sock, &[row]);
    let why = submit(&lab, &args);
    assert!(refused(&why).starts_with("row: "), "{why:?}");

    assert!(enqueues(&lab).is_empty(), "no refusal enqueued anything");
    assert!(!job.join(".submitting").exists() && !job.join(".tsp-out").exists());
}

/// A live "daemon" on `socket` (a lab listener) whose `tsp -l` prints `rows`.
fn lab_listen_with_rows(lab: &mut Lab, socket: &Path, rows: &[String]) {
    fs::write(format!("{}.rows", Lab::str(socket)), tsp_listing(rows)).unwrap();
    if !listed(socket) {
        lab.listen(socket);
    }
}

fn listed(socket: &Path) -> bool {
    fs::read_to_string("/proc/net/unix").unwrap().lines().any(|l| l.split_whitespace().nth(7) == Some(Lab::str(socket)))
}

#[test]
fn a_failure_after_the_claim_leaves_the_claim_and_labels_submit_interrupted() {
    let _serial = serial();
    let lab = Lab::new();
    let sock = slot_socket(&lab, 0);
    let (job, args) = prepared(&lab, "after-claim", &sock);
    fs::write(format!("{}.enqueue-fail", Lab::str(&sock)), "").unwrap();
    let reply = submit(&lab, &args);
    assert!(matches!(&reply, SubmitReply::FailedAfterClaim(why) if why.starts_with("enqueue: tsp exited 1")), "{reply:?}");
    assert!(fs::symlink_metadata(job.join(".submitting")).is_ok(), "the claim stays");
    assert!(!job.join(".enqueued").exists());
    assert_eq!(label_of(&lab, &job, &sock).1, Label::SubmitInterrupted);
}

/// o item 13.3: every busctl result but rc 0 + exactly the 8 bytes `b false\n` is `refused-kup`,
/// with the evidence; nothing is claimed. `Ok` iff every case gives the variant with its rc and
/// stdout, no claim and no enqueue.
fn kup_check(script: &str) -> Result<(), String> {
    let _serial = serial();
    let lab = Lab::new();
    let sock = slot_socket(&lab, 0);
    let (job, args) = prepared(&lab, "kup", &sock);
    let cases: [(&str, &[(&str, &str)], u8, &str); 5] = [
        ("b true", &[("STUB_KUP", "b true")], 0, "b true\n"),
        ("rc 1", &[("STUB_KUP", "b false"), ("STUB_KUP_RC", "1")], 1, "b false\n"),
        ("garbage", &[("STUB_KUP", "garbage")], 0, "garbage\n"),
        ("timeout", &[("STUB_KUP_SLEEP", "6")], 124, ""),
        ("nul", &[("STUB_KUP_NUL", "1")], 0, "b false\n\0junk"),
    ];
    for (name, env, rc, stdout) in cases {
        match submit_with(&lab, script, &args, env) {
            SubmitReply::RefusedKup(e) if (e.rc, e.stdout.as_str()) == (rc, stdout) => {}
            other => return Err(format!("{name}: {other:?}")),
        }
        if fs::symlink_metadata(job.join(".submitting")).is_ok() || !enqueues(&lab).is_empty() {
            return Err(format!("{name}: claimed or enqueued"));
        }
    }
    Ok(())
}

#[test]
fn kill_user_processes_failures_are_refused_kup_with_their_evidence() {
    kup_check(SUBMIT).unwrap();
}

/// NEGATIVE CONTROL (13.3): the old free-text signal — a plain `refused "KillUserProcesses: …"` —
/// is not the variant.
#[test]
fn a_plain_kup_refusal_turns_the_kup_check_red() {
    let broken = mutate(SUBMIT, "refuse_kup \"$rc\" \"$T/kup\" \"$T/kup.err\"", "refuse \"KillUserProcesses: rc $rc\"");
    let red = kup_check(&broken).unwrap_err();
    assert!(red.starts_with("b true: Refused(\"KillUserProcesses: rc 0\")"), "{red}");
    eprintln!("control (13.3) red: {red}");
}

/// NEGATIVE CONTROL (LOW-3): without the size check, `b false\n` followed by a NUL and more bytes
/// passes (`read -d ''` stops at the NUL) and the job is enqueued.
#[test]
fn dropping_the_kup_size_check_turns_the_kup_check_red() {
    let broken = mutate(SUBMIT, " || [[ $(stat -c %s -- \"$T/kup\" </dev/null) != 8 ]]", "");
    let red = kup_check(&broken).unwrap_err();
    assert!(red.starts_with("nul: Enqueued"), "{red}");
    eprintln!("control (LOW-3) red: {red}");
}

/// o item 13.1: the wrapper is `<root>/bin/wrapper-<sha>.sh`, a regular file equal to its realpath
/// whose sha256 is `<sha>`; a 6th value that is not 64 lowercase hex refuses at the value check.
/// `Ok` iff every case is a plain `refused` with the expected step, nothing claimed or enqueued.
fn wrapper_check(script: &str) -> Result<(), String> {
    let _serial = serial();
    let lab = Lab::new();
    let sock = slot_socket(&lab, 0);
    let bin = lab.root.join("bin");
    let realbin = lab.root.join("realbin");
    let elsewhere = lab.root.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let cases: [(&str, &dyn Fn(), &dyn Fn(), &dyn Fn(&mut SubmitArgs), &str); 6] = [
        ("bytes", &|| fs::write(&lab.wrapper, format!("{WRAPPER}# changed\n")).unwrap(), &|| fs::write(&lab.wrapper, WRAPPER).unwrap(), &|_| {}, "wrapper: "),
        ("symlinked wrapper", &|| {
            fs::write(elsewhere.join("w.sh"), WRAPPER).unwrap();
            fs::remove_file(&lab.wrapper).unwrap();
            std::os::unix::fs::symlink(elsewhere.join("w.sh"), &lab.wrapper).unwrap();
        }, &|| {
            fs::remove_file(&lab.wrapper).unwrap();
            fs::write(&lab.wrapper, WRAPPER).unwrap();
        }, &|_| {}, "wrapper: "),
        ("symlinked bin", &|| {
            fs::rename(&bin, &realbin).unwrap();
            std::os::unix::fs::symlink(&realbin, &bin).unwrap();
        }, &|| {
            fs::remove_file(&bin).unwrap();
            fs::rename(&realbin, &bin).unwrap();
        }, &|_| {}, "wrapper: "),
        ("absent", &|| fs::remove_file(&lab.wrapper).unwrap(), &|| fs::write(&lab.wrapper, WRAPPER).unwrap(), &|_| {}, "wrapper: "),
        ("a path", &|| {}, &|| {}, &|a| a.wrapper_sha = Lab::str(&lab.wrapper).to_string(), "values: "),
        ("uppercase", &|| {}, &|| {}, &|a| a.wrapper_sha = a.wrapper_sha.to_uppercase(), "values: "),
    ];
    for (name, damage, restore, edit, want) in cases {
        let (job, mut args) = prepared(&lab, &name.replace(' ', "-"), &sock);
        edit(&mut args);
        damage();
        let reply = submit_with(&lab, script, &args, &[]);
        restore();
        let ok = matches!(&reply, SubmitReply::Refused(why) if why.starts_with(want));
        if !ok || fs::symlink_metadata(job.join(".submitting")).is_ok() || !enqueues(&lab).is_empty() {
            return Err(format!("{name}: {reply:?}"));
        }
    }
    Ok(())
}

#[test]
fn the_wrapper_is_checked_by_its_sha_and_its_realpath() {
    wrapper_check(SUBMIT).unwrap();
}

/// NEGATIVE CONTROL (13.1): without the realpath clause (the `-f`/`! -L` test kept), a wrapper
/// with the right bytes behind a symlinked `<root>/bin` is enqueued.
#[test]
fn dropping_the_wrapper_realpath_check_turns_the_wrapper_check_red() {
    let broken = mutate(
        SUBMIT,
        "(( rc == 0 )) && [[ $line == \"$wrapper\" ]] || refuse \"wrapper: $wrapper is reached through a symlink (realpath: $line)\"",
        "true",
    );
    let red = wrapper_check(&broken).unwrap_err();
    assert!(red.starts_with("symlinked bin: Enqueued"), "{red}");
    eprintln!("control (13.1 realpath) red: {red}");
}

// ---- the slot check (o item 9) ---------------------------------------------------------------

/// A running wrapper of `job` on the slot mask, as tsp would run it: own session, sleeping stub
/// ORCA pinned to the mask with the job dir as cwd. Returns the wrapper PID once ORCA runs.
fn running_on_mask(lab: &mut Lab, job: &Path) -> u32 {
    let mut cmd = Command::new("bash");
    cmd.arg(&lab.wrapper)
        .arg(job)
        .arg(mask())
        .arg(lab.stub_dir.join("orca"))
        .env("STUB_RAN_LOG", lab.ran_log())
        .env("STUB_ORCA_SLEEP", "20")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    new_session(&mut cmd);
    let pid = lab.spawn(cmd);
    lab.tracked_pid(&job.join("orca.pid"));
    lab.tracked_pid(&job.join("sleep.pid"));
    pid
}

/// No process of session `sid` is a zombie.
fn no_zombie_in_session(sid: u32) -> bool {
    fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()))
        .filter_map(stat_of)
        .all(|st| st.session != sid || !st.is_zombie())
}

fn running_row(lab: &Lab, job: &Path) -> String {
    format!(
        "3    running    {}/.tsp-out/ts-out.AbCdEf                    bash {} {} {} {}",
        Lab::str(job),
        Lab::str(&lab.wrapper),
        Lab::str(job),
        mask(),
        Lab::str(&lab.stub_dir.join("orca"))
    )
}

#[test]
fn the_slot_check_accounts_for_this_slots_running_job_and_blocks_an_unaccounted_one() {
    let _serial = serial();
    let mut lab = Lab::new();
    let sock = slot_socket(&lab, 0);
    let running = lab.job("running");
    let wrapper = running_on_mask(&mut lab, &running);
    // A pinned zombie of the running job is never accounted for (its cwd is gone) and would make
    // this submit "slot busy" until reaped (ADR-024 o item 13, "Not changed"): wait it out.
    wait_for("no zombie in the running job's session", || no_zombie_in_session(wrapper));

    // The running job's row on this slot's daemon: it is accounted for; the new job queues behind.
    let row = running_row(&lab, &running);
    lab_listen_with_rows(&mut lab, &sock, &[row]);
    let (_job, args) = prepared(&lab, "behind", &sock);
    assert_eq!(submit(&lab, &args), SubmitReply::Enqueued(0));

    // The same process with its row gone: the wrapper and its pinned ORCA block; no claim.
    fs::write(format!("{}.rows", Lab::str(&sock)), tsp_listing(&[])).unwrap();
    let (job, args) = prepared(&lab, "blocked", &sock);
    let reply = submit(&lab, &args);
    let why = refused(&reply);
    assert!(why.starts_with("slot busy: "), "{why}");
    assert!(why.contains(&format!("pid {wrapper} (wrapper, cores {}", mask())), "{why}");
    assert!(why.contains("(pinned, cores"), "the pinned ORCA blocks too: {why}");
    assert!(fs::symlink_metadata(job.join(".submitting")).is_err(), "a slot refusal writes no claim");
    assert_eq!(enqueues(&lab).len(), 1);
}

#[test]
fn with_no_daemon_on_the_slot_every_live_session_on_the_mask_blocks() {
    let _serial = serial();
    let mut lab = Lab::new();
    let sock = slot_socket(&lab, 0);
    let running = lab.job("survivor");
    running_on_mask(&mut lab, &running);
    // A row file exists, but nothing listens: NoDaemon, so nothing is accounted for.
    fs::write(format!("{}.rows", Lab::str(&sock)), tsp_listing(&[running_row(&lab, &running)])).unwrap();
    let (_job, args) = prepared(&lab, "after-restart", &sock);
    assert!(refused(&submit(&lab, &args)).starts_with("slot busy: "));
}

#[test]
fn the_slot_check_sees_queued_rows_of_other_own_sockets_and_pinned_strays() {
    let _serial = serial();
    let mut lab = Lab::new();
    let sock = slot_socket(&lab, 0);
    let other = slot_socket(&lab, 1);
    let queued_job = format!("{}/jobs/elsewhere", Lab::str(&lab.root));
    let queued = format!("7    queued     (file)                                       bash {} {queued_job} {} /o", Lab::str(&lab.wrapper), mask());
    lab_listen_with_rows(&mut lab, &other, &[queued]);
    let (_job, args) = prepared(&lab, "queued-elsewhere", &sock);
    let why = submit(&lab, &args);
    assert!(refused(&why).contains(&format!("queued row 7 on {}", Lab::str(&other))), "{why:?}");

    // A failed `tsp -l` on a qualifying socket refuses (Error), never reads as "no rows".
    fs::write(format!("{}.fail", Lab::str(&other)), "").unwrap();
    let why = submit(&lab, &args);
    assert!(refused(&why).starts_with("slot check: error tsp -l on"), "{why:?}");
    fs::remove_file(format!("{}.fail", Lab::str(&other))).unwrap();
    fs::write(format!("{}.rows", Lab::str(&other)), tsp_listing(&[])).unwrap();

    // A pinned process outside any root, on a CPU of the mask, blocks; one off the mask does not.
    let elsewhere = lab.root.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let pinned = |lab: &mut Lab, cpu: usize| {
        let mut cmd = Command::new("taskset");
        cmd.arg("-c").arg(cpu.to_string()).arg("sleep").arg("20").current_dir(&elsewhere).stdin(Stdio::null());
        let pid = lab.spawn(cmd);
        wait_for("pinned", || {
            fs::read_to_string(format!("/proc/{pid}/cmdline")).is_ok_and(|c| c.starts_with("sleep"))
        });
        tracked(pid)
    };
    let off = pinned(&mut lab, 0);
    assert_eq!(submit(&lab, &args), SubmitReply::Enqueued(0), "cores off the mask never block");
    let (_job, args) = prepared(&lab, "stray", &sock);
    let on = pinned(&mut lab, slot_cpus().0);
    let why = submit(&lab, &args);
    assert!(refused(&why).contains(&format!("pid {} (pinned, cores {}", on.pid, slot_cpus().0)), "{why:?}");
    assert!(!is_dead(off) && !is_dead(on), "the scan only reads");
}

// ---- label -----------------------------------------------------------------------------------

fn label_of(lab: &Lab, job: &Path, socket: &Path) -> (LabelFacts, Label) {
    let args = LabelArgs::new(Lab::str(job), Lab::str(socket)).unwrap();
    let out = run(lab, LABEL, &args.values(), &[]);
    let facts = parse_label_reply(&out.stdout, &args).unwrap_or_else(|e| panic!("{e}\n{}", text(&out)));
    (facts, label(&facts))
}

#[test]
fn the_label_call_reports_its_facts_and_label_in_the_adr_order() {
    let mut lab = Lab::new();
    // Not the slot layout: the submit tests' scans must not see these listeners.
    let live = lab.root.join("live.sock");
    let dead = lab.root.join("dead.sock");
    lab.listen(&live);
    lab.stale_socket(&dead);
    let row_for = |job: &Path| format!("4    queued     (file)                                       bash {} {} 0 /o", Lab::str(&lab.wrapper), Lab::str(job));
    let none = Markers::default();

    // 1. No dir: not on the server, whatever the socket says.
    let gone = lab.root.join("jobs").join("gone");
    let (facts, l) = label_of(&lab, &gone, &live);
    assert_eq!((facts, l), (LabelFacts::default(), Label::NotOnServer));

    // 2. Each marker, or a row → the classifier's.
    for (marker, want) in [
        (".started", Markers { started: true, ..none }),
        (".exit_code", Markers { exit_code: true, ..none }),
        (".cancelled", Markers { cancelled: true, ..none }),
        (".enqueued", Markers { enqueued: true, ..none }),
    ] {
        let job = lab.job(&format!("m{marker}"));
        fs::write(job.join(marker), "").unwrap();
        let (facts, l) = label_of(&lab, &job, &dead);
        assert_eq!(facts, LabelFacts { dir_exists: true, markers: want, ..LabelFacts::default() }, "{marker}");
        assert_eq!(l, Label::Classifier, "{marker}");
    }
    let job = lab.job("row");
    fs::write(format!("{}.rows", Lab::str(&live)), tsp_listing(&[row_for(&job)])).unwrap();
    let (facts, l) = label_of(&lab, &job, &live);
    assert!(facts.row_holds_job && !facts.socket_error);
    assert_eq!(l, Label::Classifier);

    // A row of /jobs/row0 does not hold /jobs/row (whole token), and an empty dir is not on the
    // server.
    let short = lab.job("ro");
    let (facts, l) = label_of(&lab, &short, &live);
    assert!(!facts.row_holds_job);
    assert_eq!(l, Label::NotOnServer);

    // 2 before 4: .submitting plus a marker or a row is the classifier's.
    let job = lab.job("sub-enq");
    std::os::unix::fs::symlink("x", job.join(".submitting")).unwrap();
    fs::write(job.join(".enqueued"), "").unwrap();
    assert_eq!(label_of(&lab, &job, &dead).1, Label::Classifier);
    let job = lab.job("sub-row");
    std::os::unix::fs::symlink("x", job.join(".submitting")).unwrap();
    fs::write(format!("{}.rows", Lab::str(&live)), tsp_listing(&[row_for(&job)])).unwrap();
    assert_eq!(label_of(&lab, &job, &live).1, Label::Classifier);

    // 3 before 4: a socket Error with .submitting is the classifier's, never "submit interrupted".
    fs::write(format!("{}.fail", Lab::str(&live)), "").unwrap();
    let job = lab.job("sub-error");
    std::os::unix::fs::symlink("x", job.join(".submitting")).unwrap();
    let (facts, l) = label_of(&lab, &job, &live);
    assert!(facts.socket_error && facts.markers.submitting);
    assert_eq!(l, Label::Classifier);
    fs::remove_file(format!("{}.fail", Lab::str(&live))).unwrap();

    // 4. .submitting alone (a dangling symlink): submit interrupted.
    let job = lab.job("sub-only");
    std::os::unix::fs::symlink("x", job.join(".submitting")).unwrap();
    lab.clear_tsp_log();
    let (facts, l) = label_of(&lab, &job, &dead);
    assert_eq!(facts.markers, Markers { submitting: true, ..none });
    assert_eq!(l, Label::SubmitInterrupted);
    assert_eq!(lab.tsp_log(), "", "no tsp call on a dead socket (probe 5.2b)");

    // 5. An empty dir: not on the server.
    let job = lab.job("empty");
    for f in fs::read_dir(&job).unwrap() {
        fs::remove_file(f.unwrap().path()).unwrap();
    }
    assert_eq!(label_of(&lab, &job, &dead).1, Label::NotOnServer);
}

#[test]
fn the_label_call_reports_a_read_error_never_absence() {
    let lab = Lab::new();
    let job = lab.job("locked");
    let sock = lab.root.join("s.sock");
    fs::set_permissions(&job, fs::Permissions::from_mode(0o000)).unwrap();
    let args = LabelArgs::new(Lab::str(&job), Lab::str(&sock)).unwrap();
    let out = run(&lab, LABEL, &args.values(), &[]);
    fs::set_permissions(&job, fs::Permissions::from_mode(0o755)).unwrap();
    let result = parse_label_reply(&out.stdout, &args);
    assert!(
        matches!(&result, Err(super::submit::SubmitError::Reply(PollError::Wire(WireError::Collector(m)))) if m.contains("Permission denied")),
        "{result:?}"
    );
}

/// A hung `tsp -l` on the recorded socket (the stub sleeps 6 s) is bounded by the label call's
/// `timeout -k 1 3` and reported as a socket Error, so a `.submitting` job goes to the classifier,
/// never to "submit interrupted". `Ok` iff so, within 5 s.
fn label_tsp_timeout(script: &str) -> Result<(), String> {
    let mut lab = Lab::new();
    let live = lab.root.join("hung.sock");
    lab.listen(&live);
    fs::write(format!("{}.rows", Lab::str(&live)), tsp_listing(&[])).unwrap();
    fs::write(format!("{}.sleep", Lab::str(&live)), "6").unwrap();
    let job = lab.job("hung");
    std::os::unix::fs::symlink("x", job.join(".submitting")).unwrap();
    let args = LabelArgs::new(Lab::str(&job), Lab::str(&live)).unwrap();
    let started = std::time::Instant::now();
    let out = run(&lab, script, &args.values(), &[]);
    let took = started.elapsed();
    let facts = parse_label_reply(&out.stdout, &args).map_err(|e| e.to_string())?;
    if facts.socket_error && label(&facts) == Label::Classifier && took < std::time::Duration::from_secs(5) {
        Ok(())
    } else {
        Err(format!("after {took:?}: {facts:?} → {:?}", label(&facts)))
    }
}

#[test]
fn a_hung_tsp_in_the_label_call_is_a_socket_error() {
    label_tsp_timeout(LABEL).unwrap();
}

/// NEGATIVE CONTROL (LOW-4): without the timeout the call waits out the hung client (6 s, the
/// stub's own bound — the test cannot hang) and reports "submit interrupted" from an empty listing.
#[test]
fn a_label_call_without_the_tsp_timeout_turns_red() {
    let broken = mutate(LABEL, "TS_SOCKET=$sock timeout -k 1 3 tsp -l", "TS_SOCKET=$sock tsp -l");
    let red = label_tsp_timeout(&broken).unwrap_err();
    eprintln!("control (LOW-4) red: {red}");
}

// ---- poll_log --------------------------------------------------------------------------------

/// Raw bytes, a NUL and an invalid UTF-8 byte included: the transport never decodes.
const LOG: &[u8] = b"line 1\n\x00\xff Ang\xc3\x85 \xe2\x86\x92\nORCA TERMINATED NORMALLY\n";

fn poll_with(lab: &Lab, script: &str, job: &Path, offset: u64, cap: u64) -> Result<LogChunk, PollError> {
    let args = PollLogArgs { job_dir: Lab::str(job).into(), offset, cap };
    let out = run(lab, script, &args.values(), &[]);
    parse_poll_reply(&out.stdout, &args)
}

/// Every case of o item 7 through the real script and the parser. `Ok` iff each chunk is right.
fn poll_round_trip(script: &str) -> Result<(), String> {
    let lab = Lab::new();
    let job = lab.job("poll");
    let size = LOG.len() as u64;
    let check = |what: &str, got: Result<LogChunk, PollError>, want: LogChunk| {
        if got.as_ref() == Ok(&want) {
            Ok(())
        } else {
            Err(format!("{what}: {got:?}"))
        }
    };
    check("absent", poll_with(&lab, script, &job, 5, 64), LogChunk::unchanged(5))?;
    check("no job dir", poll_with(&lab, script, &lab.root.join("jobs/none"), 0, 64), LogChunk::unchanged(0))?;
    fs::write(job.join("output.out"), LOG).unwrap();
    check("full", poll_with(&lab, script, &job, 0, 1 << 20), LogChunk { offset: size, bytes: LOG.to_vec(), reset: false })?;
    check("capped", poll_with(&lab, script, &job, 2, 3), LogChunk { offset: 5, bytes: LOG[2..5].to_vec(), reset: false })?;
    check("no growth", poll_with(&lab, script, &job, size, 64), LogChunk::unchanged(size))?;
    check("shrunken", poll_with(&lab, script, &job, size + 7, 64), LogChunk::reset())?;
    // Reassembled from 7-byte chunks, byte for byte.
    let (mut offset, mut rebuilt) = (0, Vec::new());
    while offset < size {
        let chunk = poll_with(&lab, script, &job, offset, 7).map_err(|e| format!("chunk at {offset}: {e}"))?;
        rebuilt.extend(chunk.bytes);
        offset = chunk.offset;
    }
    if rebuilt != LOG {
        return Err(format!("reassembled {rebuilt:?}"));
    }
    Ok(())
}

#[test]
fn poll_log_round_trips_every_case() {
    poll_round_trip(POLL_LOG).unwrap();
}

/// NEGATIVE CONTROL (c): one byte too many in the framing after the bytes is refused.
#[test]
fn a_corrupt_poll_frame_turns_the_round_trip_red() {
    let broken = mutate(POLL_LOG, "printf '\\nend\\n'", "printf 'x\\nend\\n'");
    let red = poll_round_trip(&broken).unwrap_err();
    eprintln!("control (c) red: {red}");
}

#[test]
fn an_unreadable_log_is_an_error_record() {
    let lab = Lab::new();
    let job = lab.job("unreadable");
    fs::write(job.join("output.out"), LOG).unwrap();
    fs::set_permissions(job.join("output.out"), fs::Permissions::from_mode(0o000)).unwrap();
    let result = poll_with(&lab, POLL_LOG, &job, 0, 64);
    assert!(matches!(&result, Err(PollError::Wire(WireError::Collector(m))) if m.contains("Permission denied")), "{result:?}");
}

// ---- the server listing (o item 6) -----------------------------------------------------------

/// A finished job dir: artifacts, a large and a scratch file, the dangling `.submitting` claim, a
/// selected symlink to another file, `.tsp-out/`, and directories the download never enters.
fn finished_job(lab: &Lab) -> PathBuf {
    let job = lab.job("listing");
    for (name, content) in [
        ("output.out", "out\n"),
        ("input.xyz", "1\n\nH 0 0 0\n"),
        ("input.hess", "$hessian\n"),
        ("input.property.txt", "props"),
        (".exit_code", "0\n"),
        (".started", "pid=1\n"),
        ("stderr.log", ""),
        ("input.gbw", "large"),
        ("input.tmp", "scratch"),
        (".input.gbw.lMlWCT", "rsync temp"),
    ] {
        fs::write(job.join(name), content).unwrap();
    }
    std::os::unix::fs::symlink("x", job.join(".submitting")).unwrap();
    std::os::unix::fs::symlink("input.xyz", job.join("input_trj.xyz")).unwrap();
    fs::create_dir_all(job.join(".tsp-out")).unwrap();
    fs::write(job.join(".tsp-out").join("ts-out.AbC"), "tsp").unwrap();
    fs::create_dir_all(job.join(".tmp").join("pmix")).unwrap();
    fs::create_dir_all(job.join("sub")).unwrap();
    fs::write(job.join("sub").join("deep.xyz"), "x").unwrap();
    job
}

/// The listing through the real script equals Rust's own listing of the same dir under the same
/// filter (`.tsp-out/` left out, as the comparator does). `Ok` iff equal for both policies.
fn listing_round_trip(script: &str) -> Result<(), String> {
    let lab = Lab::new();
    let job = finished_job(&lab);
    for policy in [FetchPolicy::SMALL_ONLY, FetchPolicy::WITH_GBW] {
        let args = ListArgs::new(Lab::str(&job), policy).unwrap();
        let out = run(&lab, script, &args.values(), &[]);
        let server = parse_list_reply(&out.stdout, &args).map_err(|e| format!("{policy:?}: {e}\n{}", text(&out)))?;
        let local: Vec<_> = list_dir(&job, &|p| download_selects(p, policy))
            .unwrap()
            .into_iter()
            .filter(|f| !f.name.starts_with(".tsp-out/"))
            .collect();
        if server != local {
            return Err(format!("{policy:?}: server {server:?}\nlocal {local:?}"));
        }
        compare_download(&local, &server).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[test]
fn the_server_listing_matches_the_local_listing_of_the_same_dir() {
    listing_round_trip(LIST).unwrap();
}

/// NEGATIVE CONTROL (e): a listing that follows symlinks (`%Y`) misreports `.submitting` (dangling)
/// and `input_trj.xyz` (a link to a file).
#[test]
fn a_listing_that_follows_symlinks_turns_the_round_trip_red() {
    let broken = mutate(LIST, "-printf '%y %f\\0'", "-printf '%Y %f\\0'");
    let red = listing_round_trip(&broken).unwrap_err();
    eprintln!("control (e) red: {red}");
}
