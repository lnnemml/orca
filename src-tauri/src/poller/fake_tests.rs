//! The fake server of the unit-5.4b poller tests (`lost/tests.rs`, `reenqueue/tests.rs`, the
//! `Indeterminate` count and the ready-on-not-installed step): every call a status step can make —
//! prepare, install, label, mkjob, `cancel.sh cancel` and `cancel.sh sweep` through the trampoline,
//! `collect.sh` through it, the `submit.sh reenqueue` call, the download and the listing — each
//! recorded, the database lock checked free on every one.

use std::sync::Mutex;
use std::time::Duration;

use super::tests::{download, listing, Remote};
use crate::remote::scripts::{INSTALL, LABEL, LIST, MKJOB, POLL_LOG, PREPARE, RUN, SUBMIT};
use crate::remote::ssh::{CommandRunner, TransportError, SSH_PROGRAM};
use crate::remote::sync::RSYNC_PROGRAM;
use crate::remote::wire::HEADER as SNAPSHOT_HEADER;
use crate::ssh_backend::tests::{
    expected_coords, install_ok, label_reply, mkjob_ok, prepare_ready, ran, rec, submit_reply, Reply, ENQUEUED, JOB, ROOT,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::poller) enum Call {
    Prepare,
    Install,
    Label,
    Mkjob,
    /// `cancel.sh cancel` through the trampoline.
    Cancel,
    /// `cancel.sh sweep` through the trampoline (o25.2).
    Sweep,
    /// `collect.sh` through the trampoline.
    Collect,
    /// `submit.sh` — in the re-enqueue form only (o25.4): a plain submit is a test error here.
    ReEnqueue,
    Download,
    List,
    PollLog,
}
pub(in crate::poller) use Call::*;

fn call_of(program: &str, stdin: &[u8]) -> (Call, Vec<String>) {
    if program == RSYNC_PROGRAM {
        return (Download, Vec::new());
    }
    assert_eq!(program, SSH_PROGRAM);
    for (call, script) in [
        (Prepare, PREPARE),
        (Install, INSTALL),
        (Label, LABEL),
        (Mkjob, MKJOB),
        (Collect, RUN),
        (ReEnqueue, SUBMIT),
        (List, LIST),
        (PollLog, POLL_LOG),
    ] {
        if let Some(rest) = stdin.strip_prefix(script.as_bytes()) {
            let mut values: Vec<String> = rest.split(|b| *b == 0).map(|v| String::from_utf8(v.to_vec()).unwrap()).collect();
            assert_eq!(values.pop().as_deref(), Some(""));
            let coords = expected_coords();
            if call == ReEnqueue {
                assert_eq!(values[0], "reenqueue", "the poller sends only the re-enqueue form of submit.sh");
                assert_eq!(values[1], coords.job_dir, "the recorded job dir");
                assert_eq!(values[3], coords.socket, "the recorded socket");
            }
            if call == Collect {
                return match values[1].as_str() {
                    "cancel" => {
                        assert_eq!(&values[4..], [coords.job_dir.as_str(), ROOT], "the recorded coordinates");
                        match values[3].as_str() {
                            "cancel" => (Cancel, values),
                            "sweep" => (Sweep, values),
                            other => panic!("cancel.sh is never run as {other:?}"),
                        }
                    }
                    "collect" => (Collect, values),
                    other => panic!("the trampoline never runs {other:?}"),
                };
            }
            return (call, values);
        }
    }
    panic!("a script the poller never sends");
}

/// Answers each call with `respond`, records it (and every re-enqueue's values), and checks the
/// database lock is free on every call.
pub(in crate::poller) struct Fake<'a> {
    r: &'a Remote,
    respond: Box<dyn Fn(Call, &[String]) -> Reply + 'a>,
    calls: Mutex<Vec<Call>>,
    reenqueues: Mutex<Vec<Vec<String>>>,
}

impl<'a> Fake<'a> {
    pub(in crate::poller) fn new(r: &'a Remote, respond: impl Fn(Call, &[String]) -> Reply + 'a) -> Self {
        Fake { r, respond: Box::new(respond), calls: Mutex::default(), reenqueues: Mutex::default() }
    }

    pub(in crate::poller) fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    pub(in crate::poller) fn count(&self, call: Call) -> usize {
        self.calls().iter().filter(|c| **c == call).count()
    }

    /// The values of every `submit.sh reenqueue` call so far.
    pub(in crate::poller) fn reenqueues(&self) -> Vec<Vec<String>> {
        self.reenqueues.lock().unwrap().clone()
    }
}

impl CommandRunner for Fake<'_> {
    fn run(&self, program: &str, _args: &[String], stdin: &[u8], _timeout: Duration) -> Reply {
        let (call, values) = call_of(program, stdin);
        assert!(self.r.w.db.0.try_lock().is_ok(), "the database lock is held across a {call:?} call");
        if call == ReEnqueue {
            self.reenqueues.lock().unwrap().push(values.clone());
        }
        self.calls.lock().unwrap().push(call);
        (self.respond)(call, &values)
    }
}

/// The default server: ready, the job enqueued (label → the classifier), `cancel.sh` and its sweep
/// rc 0, the re-enqueue enqueued as tsp id 5; the collects answer `collects` in turn (the last
/// repeats); download and listing over the server dir.
pub(in crate::poller) fn server<'a>(r: &'a Remote, collects: Vec<Vec<u8>>) -> impl Fn(Call, &[String]) -> Reply + 'a {
    let collects = Mutex::new(collects);
    move |call, values| match call {
        Prepare => prepare_ready(values),
        Install => install_ok(values),
        Label => label_reply(values, ENQUEUED),
        Mkjob => mkjob_ok(values),
        Cancel | Sweep => ran(values, 0, b"done\n", b""),
        Collect => {
            let mut c = collects.lock().unwrap();
            let snap = if c.len() > 1 { c.remove(0) } else { c[0].clone() };
            ran(values, 0, &snap, b"")
        }
        ReEnqueue => submit_reply(values, "enqueued 5\n"),
        Download => download(&r.server(), &r.local()),
        List => listing(values, &r.server()),
        PollLog => panic!("no log poll expected"),
    }
}

/// The transport's timeout: a call that got no answer.
pub(in crate::poller) fn timeout() -> Reply {
    Err(TransportError::Timeout { program: "ssh".into(), secs: 60 })
}

const BOOT: &str = "0f6e3c1a-5b7d-4c2e-9a8b-1d2e3f4a5b6c";

/// `.started` in this boot, the wrapper gone (no stat), `members` processes still in the job dir, no
/// `.exit_code`, no `.cancelled`: row 8 `Lost { orphans: [70001, …], stale_boot: false }`.
pub(in crate::poller) fn lost_in_this_boot(members: usize) -> Vec<u8> {
    let started = format!("pid=4242\npgid=4242\nsid=4242\nboot_id={BOOT}\nstarttime=777\nstarted_at=1\n");
    let coords = expected_coords();
    let session: String = (0..members).map(|i| format!("member {}\n{}", 70001 + i, rec("cwd", &coords.job_dir))).collect();
    format!(
        "{SNAPSHOT_HEADER}\n{}{}proc collected\nwrapper_stat -\nwrapper_cmdline -\nsid_stat -\nmembers {members}\n{session}\
         reenqueue_claim no\nsockets 1\n{}{}nodaemon\nexit_code -\ncancelled no\ntail -\n{}end\n",
        rec("boot_id", &format!("{BOOT}\n")),
        rec("started", &started),
        rec("socket", &coords.socket),
        rec("net_unix", "Num       RefCount Protocol Flags    Type St Inode Path\n"),
        rec("started", &started),
    )
    .into_bytes()
}

/// The orphan pids of [`lost_in_this_boot`]`(members)`.
pub(in crate::poller) fn orphans(members: usize) -> Vec<u32> {
    (0..members).map(|i| 70001 + i as u32).collect()
}

/// The job id of every fake (re-exported for the tests' brevity).
pub(in crate::poller) const ID: &str = JOB;
