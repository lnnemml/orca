//! Submit, pure parts (ADR-024 o items 1 and 3): the remote job dir, the values of the one atomic
//! submit call and the strict parser of its reply, and the label rules of the read-only label
//! call.
//!
//! **Submit values** (NUL list on `bash -s` stdin, ADR-024 n item 11): the job dir, the root, the
//! slot socket, the slot's core mask, the ORCA path, the **wrapper's sha256** (64 lowercase hex,
//! of the embedded [`WRAPPER`]; the script builds `<root>/bin/wrapper-<sha>.sh` itself and checks
//! the file's bytes against it — the client never sends a wrapper path, o item 13.1), then **two
//! values per uploaded file** — name, sha256 — so the server checks the upload itself (o item
//! 3.3.4): 6 + 2n values. The script is [`super::scripts::SUBMIT`].
//!
//! **Submit reply** (records, the 5.2 format read by [`Reader`]):
//!
//! ```text
//! orcastudio-submit 1
//! argc <n>
//! arg <len>                      n times: every value, verbatim (o item 3.3, n item 6d)
//! refused <len>                  exactly one of — nothing claimed: any other refusal, its bytes
//! refused-kup <len>              nothing claimed: KillUserProcesses is not `b false`, then the
//!                                evidence (below)
//! blocked <n>                    nothing claimed: the slot check's n ≥ 1 blockers (below)
//! slot-check-error <len>         nothing claimed: the slot could not be checked, why
//! lock-busy <len>                nothing claimed: another submit holds the account lock; the
//!                                lock file's openers
//! upload-mismatch                nothing claimed: the uploaded set differs (below)
//! enqueued <id>                  the tsp id (decimal); `.enqueued` is published
//! failed-after-claim <len>       `.submitting` stays; the reason's bytes
//! end
//! ```
//!
//! A refusal wrote no claim, so the job stays "not on the server" and may be retried; after a
//! claim only the label call decides (o item 3.3.9). The refusals a caller acts on have their own
//! reply kinds (ADR-024 o23.7), parsed into a typed [`Refusal`]; a `refused` reason (`values:`,
//! `realpath:`, `wrapper:`, `marker:`, `row:`, `upload:` for a failed listing, `claim:` …) is for
//! people — **no caller decides on a reason's text**.
//!
//! **`blocked` records** (one per blocker; every path and list a byte record — a cwd can hold a
//! newline, o23.13.8):
//!
//! ```text
//! process <pid>                  a live own process on the mask:
//! kind wrapper|pinned              our wrapper (its argv mask) or a pinned process
//! cores <len>                      its cores (the wrapper's mask or its Cpus_allowed_list)
//! job <len> | job -                its job dir (the wrapper's argument or its cwd); - : the cwd
//!                                  was not readable
//! row <id>                       a queued row of another own slot daemon:
//! socket <len>                     that daemon's socket
//! job <len>                        the row's job dir
//! mask <len>                       the row's mask
//! ```
//!
//! **`upload-mismatch`**: `missing <n>`, `extra <n>`, `differing <n>`, each followed by n `name
//! <len>` records (step 4's comparison only).
//!
//! **`refused-kup` evidence** (o item 13.3; its bytes, themselves records):
//!
//! ```text
//! rc <n>                         busctl's exit status (124 = timed out)
//! stdout <len>                   its stdout, verbatim
//! stderr <len>                   its first stderr line
//! ```
//!
//! Only this outcome clears the profile's `verified_at` (n item 7). Evidence of rc 0 with stdout
//! exactly `b false\n` is a protocol error: that is the one passing result.
//!
//! **Label reply** ([`super::scripts::LABEL`], read-only, o item 3.4):
//!
//! ```text
//! orcastudio-label 1
//! argc 2
//! arg <len>                      × 2: the job dir, the recorded socket
//! dir yes|no                     no → end
//! started|exit_code|cancelled|enqueued|submitting yes|no   in this order
//! net_unix <len>                 /proc/net/unix header + the lines naming the socket, then
//!   nodaemon | rows <n> + n × row <len> | tsp_error <len>
//! end
//! ```

use super::classify::is_valid_path;
use super::poll::{check_echo, PollError};
use super::procfs::unix_socket_listed;
use super::scripts::{sha256_hex, WRAPPER};
use super::sync::{expected_values, FileDigest, SyncError};
use super::wire::{parse_decimal, Reader, WireError};
use super::MAX_SOCKET_PATH_BYTES;
use crate::models::server_profile::parse_core_mask;

/// The first line of every submit reply; the number is the format version.
pub const OUTPUT_HEADER: &str = "orcastudio-submit 1";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SubmitError {
    #[error("{what} {path:?} breaks the path rule")]
    Path { what: &'static str, path: String },
    #[error("job id {0:?} is not one path component")]
    JobId(String),
    #[error("socket path {0:?} is longer than {MAX_SOCKET_PATH_BYTES} bytes")]
    SocketTooLong(String),
    #[error("core mask {mask:?}: {why}")]
    CoreMask { mask: String, why: String },
    #[error("ORCA path {0:?} is not absolute")]
    OrcaPath(String),
    #[error("label reply: {0}")]
    Label(String),
    #[error("submit reply: {0}")]
    Kup(String),
    #[error(transparent)]
    Files(#[from] SyncError),
    #[error(transparent)]
    Reply(#[from] PollError),
}

impl From<WireError> for SubmitError {
    fn from(e: WireError) -> Self {
        SubmitError::Reply(PollError::Wire(e))
    }
}

/// The remote job dir: `<root>/jobs/<job_id>` (ADR-024 o item 1), checked by the one path rule
/// (l detail 4) before it is ever used. The job id must be a single component, so it cannot
/// reach outside `<root>/jobs`.
pub fn remote_job_dir(root: &str, job_id: &str) -> Result<String, SubmitError> {
    if !is_valid_path(root) {
        return Err(SubmitError::Path { what: "root", path: root.to_string() });
    }
    if job_id.is_empty() || job_id.contains('/') {
        return Err(SubmitError::JobId(job_id.to_string()));
    }
    let dir = format!("{root}/jobs/{job_id}");
    if !is_valid_path(&dir) {
        return Err(SubmitError::Path { what: "job dir", path: dir });
    }
    Ok(dir)
}

/// The values of one submit call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitArgs {
    pub job_dir: String,
    pub root: String,
    pub socket: String,
    pub core_mask: String,
    pub orca_path: String,
    /// The sha256 of the embedded [`WRAPPER`]; the server runs `<root>/bin/wrapper-<sha>.sh`.
    pub wrapper_sha: String,
    /// Name and sha256 of every uploaded file (`sync::upload_expected`).
    pub files: Vec<FileDigest>,
}

impl SubmitArgs {
    /// Check every value before anything leaves the laptop: the job dir derived from the root and
    /// the job id, the socket by the path rule and its length bound, the mask's syntax, an absolute
    /// ORCA path (rule #1), and files that each carry a sha256.
    pub fn new(
        root: &str,
        job_id: &str,
        socket: &str,
        core_mask: &str,
        orca_path: &str,
        files: Vec<FileDigest>,
    ) -> Result<Self, SubmitError> {
        let job_dir = remote_job_dir(root, job_id)?;
        if !is_valid_path(socket) {
            return Err(SubmitError::Path { what: "socket", path: socket.to_string() });
        }
        if socket.len() > MAX_SOCKET_PATH_BYTES {
            return Err(SubmitError::SocketTooLong(socket.to_string()));
        }
        parse_core_mask(core_mask)
            .map_err(|why| SubmitError::CoreMask { mask: core_mask.to_string(), why })?;
        if !orca_path.starts_with('/') {
            return Err(SubmitError::OrcaPath(orca_path.to_string()));
        }
        expected_values(&files)?;
        Ok(SubmitArgs {
            job_dir,
            root: root.to_string(),
            socket: socket.to_string(),
            core_mask: core_mask.to_string(),
            orca_path: orca_path.to_string(),
            wrapper_sha: sha256_hex(WRAPPER),
            files,
        })
    }

    /// The NUL-list values, in the order the script reads them.
    pub fn values(&self) -> Result<Vec<String>, SubmitError> {
        let mut values = vec![
            self.job_dir.clone(),
            self.root.clone(),
            self.socket.clone(),
            self.core_mask.clone(),
            self.orca_path.clone(),
            self.wrapper_sha.clone(),
        ];
        values.extend(expected_values(&self.files)?);
        Ok(values)
    }
}

/// What the submit call did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitReply {
    /// Nothing was claimed; the job is still "not on the server" and may be retried.
    Refused(Refusal),
    /// Nothing was claimed: `KillUserProcesses` is not `b false` (o item 13.3). The only outcome
    /// that clears the profile's `verified_at`.
    RefusedKup(KupEvidence),
    /// Enqueued with this `tsp` id; `.enqueued` is published.
    Enqueued(u64),
    /// Claimed (`.submitting` exists) but not enqueued; only the label call decides now.
    FailedAfterClaim(String),
}

/// Why the submit call claimed nothing (ADR-024 o23.7): the kinds a caller acts on, each its own
/// reply kind, and every other refusal as text for people. `{ "kind": "<snake_case>", "detail": … }`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum Refusal {
    /// Step 5: live work on the slot's mask that this slot's daemon does not account for.
    Blocked { blockers: Vec<Blocker> },
    /// The slot could not be checked: step 3's slot socket state, step 5's scan, or a bounded child
    /// before the claim that timed out (o23.13.2).
    SlotCheckFailed(String),
    /// Another submit holds the account lock; `openers` are the processes with the lock file open.
    LockBusy { openers: String },
    /// Step 4's comparison: the uploaded files are not the expected set (names lossily decoded).
    UploadMismatch { missing: Vec<String>, extra: Vec<String>, differing: Vec<String> },
    /// Any other refusal (values, realpath, wrapper, marker, row, a failed listing, claim, …), for people.
    Other(String),
}

impl std::fmt::Display for Refusal {
    /// The refusal for people, worded as the script's old reasons were.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Blocked { blockers } => {
                let held: Vec<String> = blockers.iter().map(Blocker::to_string).collect();
                write!(f, "slot busy: the core mask is held by {}", held.join("; "))
            }
            Refusal::SlotCheckFailed(why) => write!(f, "slot check: {why}"),
            Refusal::LockBusy { openers } => write!(f, "lock busy: lock file open in: {openers}"),
            Refusal::UploadMismatch { missing, extra, differing } => write!(
                f,
                "upload: missing [{}], extra [{}], differing [{}]",
                missing.join(" "),
                extra.iter().map(|n| format!("{n:?}")).collect::<Vec<_>>().join(" "),
                differing.join(" ")
            ),
            Refusal::Other(why) => f.write_str(why),
        }
    }
}

/// One blocker of the slot check (o item 9, o23.7). Paths and lists are lossily decoded.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Blocker {
    /// A live own process on the mask: our wrapper or a pinned process. `job_dir` is the wrapper's
    /// job dir argument or the process's cwd; `None`: the cwd was not readable.
    Process { pid: u32, role: ProcessRole, cores: String, job_dir: Option<String> },
    /// A queued row of another own slot daemon whose mask meets this one.
    QueuedRow { id: u64, socket: String, job_dir: String, mask: String },
}

/// What a blocking process is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessRole {
    Wrapper,
    Pinned,
}

impl Blocker {
    /// The blocker's job dir, if it names one.
    pub fn job_dir(&self) -> Option<&str> {
        match self {
            Blocker::Process { job_dir, .. } => job_dir.as_deref(),
            Blocker::QueuedRow { job_dir, .. } => Some(job_dir),
        }
    }

    /// How a blocker that is no local job is shown (o23.7, o23.13.8): "pid <pid> (cores <cores>)"
    /// or "tsp row <id> on <socket>".
    pub fn fallback(&self) -> String {
        match self {
            Blocker::Process { pid, cores, .. } => format!("pid {pid} (cores {cores})"),
            Blocker::QueuedRow { id, socket, .. } => format!("tsp row {id} on {socket}"),
        }
    }
}

impl std::fmt::Display for Blocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Blocker::Process { pid, role, cores, job_dir } => {
                let role = match role {
                    ProcessRole::Wrapper => "wrapper",
                    ProcessRole::Pinned => "pinned",
                };
                match job_dir {
                    Some(dir) => write!(f, "pid {pid} ({role}, cores {cores}, job dir {dir:?})"),
                    None => write!(f, "pid {pid} ({role}, cores {cores}, cwd not readable)"),
                }
            }
            Blocker::QueuedRow { id, socket, job_dir, mask } => {
                write!(f, "queued row {id} on {socket} (job dir {job_dir:?}, mask {mask})")
            }
        }
    }
}

/// A required byte record `<name> <len>`, lossily decoded.
fn text_record(r: &mut Reader<'_>, name: &str) -> Result<String, SubmitError> {
    let bytes = r.bytes(name)?.ok_or_else(|| r.malformed(format!("{name} is required")))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// `blocked <n>`'s n records (n ≥ 1).
fn parse_blockers(r: &mut Reader<'_>, n: Option<&str>) -> Result<Vec<Blocker>, SubmitError> {
    let n = n
        .and_then(parse_decimal)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| (1..=MAX_BLOCKERS).contains(n))
        .ok_or_else(|| r.malformed(format!("blocked {n:?}")))?;
    let mut blockers = Vec::with_capacity(n);
    for _ in 0..n {
        let blocker = match r.line()? {
            ("process", Some(pid)) => {
                let pid = parse_decimal(pid)
                    .and_then(|p| u32::try_from(p).ok())
                    .ok_or_else(|| r.malformed(format!("process {pid:?}")))?;
                let role = match r.word("kind")? {
                    "wrapper" => ProcessRole::Wrapper,
                    "pinned" => ProcessRole::Pinned,
                    other => return Err(r.malformed(format!("kind {other:?}")).into()),
                };
                let cores = text_record(r, "cores")?;
                let job_dir = r.bytes("job")?.map(|b| String::from_utf8_lossy(&b).into_owned());
                Blocker::Process { pid, role, cores, job_dir }
            }
            ("row", Some(id)) => {
                let id = parse_decimal(id).ok_or_else(|| r.malformed(format!("row {id:?}")))?;
                let socket = text_record(r, "socket")?;
                let job_dir = text_record(r, "job")?;
                let mask = text_record(r, "mask")?;
                Blocker::QueuedRow { id, socket, job_dir, mask }
            }
            (other, _) => return Err(r.malformed(format!("expected a blocker, got {other:?}")).into()),
        };
        blockers.push(blocker);
    }
    Ok(blockers)
}

/// `upload-mismatch`'s three name lists.
fn parse_mismatch(r: &mut Reader<'_>) -> Result<Refusal, SubmitError> {
    let mut lists: [Vec<String>; 3] = Default::default();
    for (list, name) in lists.iter_mut().zip(["missing", "extra", "differing"]) {
        for _ in 0..r.count(name)? {
            list.push(text_record(r, "name")?);
        }
    }
    let [missing, extra, differing] = lists;
    if missing.is_empty() && extra.is_empty() && differing.is_empty() {
        return Err(r.malformed("an upload mismatch with no name".into()).into());
    }
    Ok(Refusal::UploadMismatch { missing, extra, differing })
}

/// No `blocked` reply carries more blockers than this (a bound, not a measurement: the reply is
/// capped at 1 MiB anyway).
const MAX_BLOCKERS: usize = 10_000;

/// What the step-2 `busctl` did, as the script saw it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct KupEvidence {
    /// Its exit status (124: `timeout` ended it).
    pub rc: u8,
    /// Its stdout, verbatim (lossily decoded).
    pub stdout: String,
    /// Its first stderr line.
    pub stderr: String,
}

/// The one passing `busctl` output (n item 8).
const KUP_PASS: &str = "b false\n";

fn parse_kup_evidence(bytes: &[u8]) -> Result<KupEvidence, SubmitError> {
    let mut e = Reader::new(bytes);
    let rc = e.word("rc")?;
    let rc = parse_decimal(rc)
        .and_then(|n| u8::try_from(n).ok())
        .ok_or_else(|| e.malformed(format!("rc {rc:?}")))?;
    let stdout = e.bytes("stdout")?.ok_or_else(|| e.malformed("stdout is required".into()))?;
    let stderr = e.bytes("stderr")?.ok_or_else(|| e.malformed("stderr is required".into()))?;
    if !e.at_end() {
        return Err(e.malformed("bytes after the evidence".into()).into());
    }
    let evidence = KupEvidence {
        rc,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    };
    if evidence.rc == 0 && stdout == KUP_PASS.as_bytes() {
        return Err(SubmitError::Kup("refused-kup with passing evidence (rc 0, `b false`)".into()));
    }
    Ok(evidence)
}

/// Parse one submit reply strictly; the echoed values must be exactly the ones sent.
pub fn parse_submit_reply(output: &[u8], sent: &SubmitArgs) -> Result<SubmitReply, SubmitError> {
    let mut r = Reader::new(output);
    let (name, arg) = r.line()?;
    if OUTPUT_HEADER.split_once(' ') != Some((name, arg.unwrap_or_default())) {
        return Err(r.malformed(format!("expected {OUTPUT_HEADER:?}")).into());
    }
    check_echo(&mut r, &sent.values()?)?;
    let reply = match r.line()? {
        ("refused", arg) => SubmitReply::Refused(Refusal::Other(r.text_after(arg)?)),
        ("blocked", n) => SubmitReply::Refused(Refusal::Blocked { blockers: parse_blockers(&mut r, n)? }),
        ("slot-check-error", arg) => SubmitReply::Refused(Refusal::SlotCheckFailed(r.text_after(arg)?)),
        ("lock-busy", arg) => SubmitReply::Refused(Refusal::LockBusy {
            openers: String::from_utf8_lossy(&r.bytes_after(arg)?).into_owned(),
        }),
        ("upload-mismatch", None) => SubmitReply::Refused(parse_mismatch(&mut r)?),
        ("refused-kup", arg) => SubmitReply::RefusedKup(parse_kup_evidence(&r.bytes_after(arg)?)?),
        ("failed-after-claim", arg) => SubmitReply::FailedAfterClaim(r.text_after(arg)?),
        ("enqueued", Some(id)) => SubmitReply::Enqueued(
            parse_decimal(id).ok_or_else(|| r.malformed(format!("tsp id {id:?}")))?,
        ),
        (other, _) => return Err(r.malformed(format!("expected a submit outcome, got {other:?}")).into()),
    };
    if r.line()? != ("end", None) {
        return Err(r.malformed("expected end".into()).into());
    }
    if !r.at_end() {
        return Err(r.malformed("bytes after end".into()).into());
    }
    Ok(reply)
}

/// Which job markers the label call found (ADR-024 o item 3.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct Markers {
    pub started: bool,
    pub exit_code: bool,
    pub cancelled: bool,
    pub enqueued: bool,
    pub submitting: bool,
}

/// The raw facts of one read-only label call, for a remote `Queued` job.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct LabelFacts {
    pub dir_exists: bool,
    pub markers: Markers,
    /// The recorded socket has a `tsp -l` row holding the job dir.
    pub row_holds_job: bool,
    /// `tsp -l` on the recorded socket failed (`Error`).
    pub socket_error: bool,
}

/// What the poller does with a remote `Queued` job before any collect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Label {
    /// Nothing of the job is on the server: offer retry and cancel.
    NotOnServer,
    /// Claimed, never enqueued (or the call is still running there): offer cancel only.
    SubmitInterrupted,
    /// The server has started deciding: collect and `classify`.
    Classifier,
}

/// The label rules of ADR-024 o item 3.4, **checked in this order**: no dir → not on the server;
/// any of `.started`, `.exit_code`, `.cancelled`, `.enqueued`, or a row → the classifier; a socket
/// `Error` → the classifier (row 10 `Indeterminate`, never "not on the server"); `.submitting` →
/// submit interrupted; otherwise → not on the server.
pub fn label(facts: &LabelFacts) -> Label {
    let m = facts.markers;
    if !facts.dir_exists {
        return Label::NotOnServer;
    }
    if m.started || m.exit_code || m.cancelled || m.enqueued || facts.row_holds_job {
        return Label::Classifier;
    }
    if facts.socket_error {
        return Label::Classifier;
    }
    if m.submitting {
        return Label::SubmitInterrupted;
    }
    Label::NotOnServer
}

/// The first line of every label reply; the number is the format version.
pub const LABEL_HEADER: &str = "orcastudio-label 1";

/// What one label call sends: the job's recorded dir and socket (o item 1 — never the profile's
/// current values).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelArgs {
    pub job_dir: String,
    pub socket: String,
}

impl LabelArgs {
    pub fn new(job_dir: &str, socket: &str) -> Result<Self, SubmitError> {
        if !is_valid_path(job_dir) {
            return Err(SubmitError::Path { what: "job dir", path: job_dir.to_string() });
        }
        if !is_valid_path(socket) {
            return Err(SubmitError::Path { what: "socket", path: socket.to_string() });
        }
        Ok(LabelArgs { job_dir: job_dir.to_string(), socket: socket.to_string() })
    }

    /// The NUL-list values, in the order the script reads them.
    pub fn values(&self) -> Vec<String> {
        vec![self.job_dir.clone(), self.socket.clone()]
    }
}

/// Parse one label reply strictly into its facts. Rust re-derives what the script decided (rule
/// #9): `nodaemon` vs `rows`/`tsp_error` must agree with [`unix_socket_listed`] over the emitted
/// `/proc/net/unix` evidence, and a row holds the job only if the job dir is one of its **whole**
/// tokens (`/jobs/j1` never claims `/jobs/j10`'s row). An `error` record is an error, never facts.
pub fn parse_label_reply(output: &[u8], sent: &LabelArgs) -> Result<LabelFacts, SubmitError> {
    let mut r = Reader::new(output);
    let (name, arg) = r.line()?;
    if LABEL_HEADER.split_once(' ') != Some((name, arg.unwrap_or_default())) {
        return Err(r.malformed(format!("expected {LABEL_HEADER:?}")).into());
    }
    check_echo(&mut r, &sent.values())?;
    let mut facts = LabelFacts { dir_exists: yes_no(&mut r, "dir")?, ..LabelFacts::default() };
    if facts.dir_exists {
        facts.markers = Markers {
            started: yes_no(&mut r, "started")?,
            exit_code: yes_no(&mut r, "exit_code")?,
            cancelled: yes_no(&mut r, "cancelled")?,
            enqueued: yes_no(&mut r, "enqueued")?,
            submitting: yes_no(&mut r, "submitting")?,
        };
        let evidence = r.bytes("net_unix")?.ok_or_else(|| r.malformed("net_unix is required".into()))?;
        let evidence = String::from_utf8(evidence).map_err(|_| SubmitError::Label("net_unix is not UTF-8".into()))?;
        let listed = unix_socket_listed(&evidence, &sent.socket).map_err(|e| SubmitError::Label(e.to_string()))?;
        let (name, arg) = r.line()?;
        let queried = match (name, arg) {
            ("nodaemon", None) => false,
            ("tsp_error", arg) => {
                r.text_after(arg)?;
                facts.socket_error = true;
                true
            }
            ("rows", Some(n)) => {
                let n = parse_decimal(n).ok_or_else(|| r.malformed(format!("rows {n:?}")))?;
                for _ in 0..n {
                    let row = r.bytes("row")?.ok_or_else(|| r.malformed("row is required".into()))?;
                    let row = String::from_utf8(row).map_err(|_| SubmitError::Label("a tsp row is not UTF-8".into()))?;
                    if !row.contains(&sent.job_dir) {
                        return Err(SubmitError::Label(format!("row does not mention the job dir: {row:?}")));
                    }
                    facts.row_holds_job |= row.split_whitespace().any(|token| token == sent.job_dir);
                }
                true
            }
            _ => return Err(r.malformed(format!("expected a socket state, got {name:?}")).into()),
        };
        // tsp may run only on a listed socket, and a listed socket must have been queried.
        if listed != queried {
            return Err(SubmitError::Label(format!(
                "{}: /proc/net/unix says listed={listed}, the script {} tsp",
                sent.socket,
                if queried { "ran" } else { "did not run" }
            )));
        }
    }
    if r.line()? != ("end", None) {
        return Err(r.malformed("expected end".into()).into());
    }
    if !r.at_end() {
        return Err(r.malformed("bytes after end".into()).into());
    }
    Ok(facts)
}

/// `<name> yes|no`.
fn yes_no(r: &mut Reader<'_>, name: &str) -> Result<bool, SubmitError> {
    match r.word(name)? {
        "yes" => Ok(true),
        "no" => Ok(false),
        other => Err(r.malformed(format!("{name} {other:?}")).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::slot_socket_path;
    use crate::remote::sync::Digest;

    const ROOT: &str = "/home/anton/.orcastudio";
    const ID: &str = "0b7c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3";
    const SHA: &str = "4a5f0c6d2e1b3a7980f1e2d3c4b5a69788796a5b4c3d2e1f0a9b8c7d6e5f4a3b";

    #[test]
    fn remote_job_dir_is_root_jobs_id_and_checked() {
        assert_eq!(remote_job_dir(ROOT, ID).unwrap(), format!("{ROOT}/jobs/{ID}"));
        assert!(matches!(remote_job_dir(ROOT, "a/b"), Err(SubmitError::JobId(_))));
        assert!(matches!(remote_job_dir(ROOT, ""), Err(SubmitError::JobId(_))));
        assert!(matches!(remote_job_dir(ROOT, ".."), Err(SubmitError::Path { .. })));
        assert!(matches!(remote_job_dir(ROOT, "a b"), Err(SubmitError::Path { .. })));
        assert!(matches!(remote_job_dir("/r//x", ID), Err(SubmitError::Path { what: "root", .. })));
        assert!(matches!(remote_job_dir("relative", ID), Err(SubmitError::Path { what: "root", .. })));
    }

    fn files() -> Vec<FileDigest> {
        vec![FileDigest { name: "input.inp".into(), digest: Digest::Sha256(SHA.into()) }]
    }

    fn sent() -> SubmitArgs {
        SubmitArgs::new(ROOT, ID, &slot_socket_path(ROOT, 0), "0-23", "/opt/orca/orca", files()).unwrap()
    }

    #[test]
    fn submit_values_are_six_with_the_wrapper_sha_then_two_per_file() {
        assert_eq!(
            sent().values().unwrap(),
            [
                format!("{ROOT}/jobs/{ID}").as_str(),
                ROOT,
                &slot_socket_path(ROOT, 0),
                "0-23",
                "/opt/orca/orca",
                &sha256_hex(WRAPPER),
                "input.inp",
                SHA,
            ]
        );
    }

    #[test]
    fn submit_args_refuse_bad_values() {
        let sock = slot_socket_path(ROOT, 0);
        assert!(matches!(SubmitArgs::new(ROOT, ID, "/r/a b.sock", "0", "/o", files()), Err(SubmitError::Path { .. })));
        let long = format!("/{}/tsp/slot0.sock", "d".repeat(100));
        assert!(matches!(SubmitArgs::new(ROOT, ID, &long, "0", "/o", files()), Err(SubmitError::SocketTooLong(_))));
        assert!(matches!(SubmitArgs::new(ROOT, ID, &sock, "-p", "/o", files()), Err(SubmitError::CoreMask { .. })));
        assert!(matches!(SubmitArgs::new(ROOT, ID, &sock, "0", "orca", files()), Err(SubmitError::OrcaPath(_))));
        let link = vec![FileDigest { name: ".submitting".into(), digest: Digest::Symlink("x".into()) }];
        assert!(matches!(SubmitArgs::new(ROOT, ID, &sock, "0", "/o", link), Err(SubmitError::Files(_))));
    }

    /// The reply for `sent` with one outcome record (`outcome` is the record line plus, for a
    /// reason, its bytes).
    fn reply(sent: &SubmitArgs, outcome: &str) -> Vec<u8> {
        let values = sent.values().unwrap();
        let mut out = format!("{OUTPUT_HEADER}\nargc {}\n", values.len());
        for v in values {
            out.push_str(&format!("arg {}\n{v}\n", v.len()));
        }
        out.push_str(outcome);
        out.push_str("end\n");
        out.into_bytes()
    }

    #[test]
    fn the_three_outcomes_parse() {
        let s = sent();
        assert_eq!(parse_submit_reply(&reply(&s, "enqueued 0\n"), &s).unwrap(), SubmitReply::Enqueued(0));
        assert_eq!(parse_submit_reply(&reply(&s, "enqueued 17\n"), &s).unwrap(), SubmitReply::Enqueued(17));
        assert_eq!(
            parse_submit_reply(&reply(&s, "refused 9\nmarker: x\n"), &s).unwrap(),
            SubmitReply::Refused(Refusal::Other("marker: x".into()))
        );
        assert_eq!(
            parse_submit_reply(&reply(&s, "failed-after-claim 11\ntsp failed\n\n"), &s).unwrap(),
            SubmitReply::FailedAfterClaim("tsp failed\n".into())
        );
    }

    fn kup(rc: &str, stdout: &str, stderr: &str) -> String {
        let ev = format!("rc {rc}\nstdout {}\n{stdout}\nstderr {}\n{stderr}\n", stdout.len(), stderr.len());
        format!("refused-kup {}\n{ev}\n", ev.len())
    }

    #[test]
    fn refused_kup_parses_into_its_own_variant() {
        let s = sent();
        assert_eq!(
            parse_submit_reply(&reply(&s, &kup("0", "b true\n", "")), &s).unwrap(),
            SubmitReply::RefusedKup(KupEvidence { rc: 0, stdout: "b true\n".into(), stderr: String::new() })
        );
        assert_eq!(
            parse_submit_reply(&reply(&s, &kup("124", "", "")), &s).unwrap(),
            SubmitReply::RefusedKup(KupEvidence { rc: 124, stdout: String::new(), stderr: String::new() })
        );
        // A plain refusal whose text looks like the old free-text signal is just a refusal.
        let old = "KillUserProcesses: b true";
        assert_eq!(
            parse_submit_reply(&reply(&s, &format!("refused {}\n{old}\n", old.len())), &s).unwrap(),
            SubmitReply::Refused(Refusal::Other(old.into()))
        );
    }

    /// o23.7: each refusal a caller acts on is its own reply kind and parses into its typed
    /// `Refusal`, byte records carried verbatim — a cwd holding a newline and a space included.
    #[test]
    fn the_typed_refusals_parse_into_their_variants() {
        let s = sent();
        let odd = "/r/jobs/a b\nc";
        let blocked = format!(
            "blocked 3\nprocess 41\nkind wrapper\n{}{}process 42\nkind pinned\n{}job -\nrow 7\n{}{}{}",
            rec("cores", "8-11"),
            rec("job", odd),
            rec("cores", "9"),
            rec("socket", "/r/tsp/slot1.sock"),
            rec("job", "/r/jobs/q"),
            rec("mask", "8-15"),
        );
        assert_eq!(
            parse_submit_reply(&reply(&s, &blocked), &s).unwrap(),
            SubmitReply::Refused(Refusal::Blocked {
                blockers: vec![
                    Blocker::Process { pid: 41, role: ProcessRole::Wrapper, cores: "8-11".into(), job_dir: Some(odd.into()) },
                    Blocker::Process { pid: 42, role: ProcessRole::Pinned, cores: "9".into(), job_dir: None },
                    Blocker::QueuedRow { id: 7, socket: "/r/tsp/slot1.sock".into(), job_dir: "/r/jobs/q".into(), mask: "8-15".into() },
                ]
            })
        );
        assert_eq!(
            parse_submit_reply(&reply(&s, &rec("slot-check-error", "the scan timed out after 8 s")), &s).unwrap(),
            SubmitReply::Refused(Refusal::SlotCheckFailed("the scan timed out after 8 s".into()))
        );
        assert_eq!(
            parse_submit_reply(&reply(&s, &rec("lock-busy", "77 bash -s")), &s).unwrap(),
            SubmitReply::Refused(Refusal::LockBusy { openers: "77 bash -s".into() })
        );
        let mismatch = format!("upload-mismatch\nmissing 1\n{}extra 1\n{}differing 0\n", rec("name", "input.xyz"), rec("name", ".x\ny"));
        assert_eq!(
            parse_submit_reply(&reply(&s, &mismatch), &s).unwrap(),
            SubmitReply::Refused(Refusal::UploadMismatch { missing: vec!["input.xyz".into()], extra: vec![".x\ny".into()], differing: vec![] })
        );
    }

    /// NEGATIVE CONTROLS of the typed kinds: a blocked reply with no blocker, a count that is not
    /// the records', an unknown blocker kind or role, a missing required record, and an upload
    /// mismatch that names nothing — an error (an unreadable answer), never a refusal.
    #[test]
    fn a_broken_typed_refusal_is_an_error_never_a_refusal() {
        let s = sent();
        let process = format!("process 41\nkind wrapper\n{}job -\n", rec("cores", "8"));
        for outcome in [
            "blocked 0\n".to_string(),
            format!("blocked 2\n{process}"),
            format!("blocked 1\nprocess 41\nkind other\n{}job -\n", rec("cores", "8")),
            "blocked 1\nprocess 41\nkind wrapper\ncores -\njob -\n".to_string(),
            format!("blocked 1\nthread 41\n{}", rec("cores", "8")),
            "upload-mismatch\nmissing 0\nextra 0\ndiffering 0\n".to_string(),
            "upload-mismatch 3\n".to_string(),
            "lock-busy\n".to_string(),
        ] {
            assert!(parse_submit_reply(&reply(&s, &outcome), &s).is_err(), "{outcome:?}");
        }
    }

    /// The fallback a blocker is shown by when it is no local job (o23.7, o23.13.8).
    #[test]
    fn a_blocker_falls_back_to_its_pid_and_cores_or_its_row_and_socket() {
        let p = Blocker::Process { pid: 41, role: ProcessRole::Pinned, cores: "8-11".into(), job_dir: None };
        assert_eq!(p.fallback(), "pid 41 (cores 8-11)");
        let q = Blocker::QueuedRow { id: 7, socket: "/r/tsp/slot1.sock".into(), job_dir: "/r/jobs/q".into(), mask: "8".into() };
        assert_eq!(q.fallback(), "tsp row 7 on /r/tsp/slot1.sock");
        assert_eq!(q.job_dir(), Some("/r/jobs/q"));
        assert_eq!(
            serde_json::to_value(Refusal::Blocked { blockers: vec![p] }).unwrap(),
            serde_json::json!({ "kind": "blocked", "detail": { "blockers": [
                { "kind": "process", "pid": 41, "role": "pinned", "cores": "8-11", "job_dir": null }
            ] } })
        );
        assert_eq!(serde_json::to_value(Refusal::Other("x".into())).unwrap(), serde_json::json!({ "kind": "other", "detail": "x" }));
    }

    /// NEGATIVE CONTROL (rule #9): `refused-kup` with the passing evidence is a protocol error, and
    /// so is broken evidence.
    #[test]
    fn refused_kup_with_passing_or_broken_evidence_is_an_error() {
        let s = sent();
        for outcome in [
            kup("0", "b false\n", ""),
            kup("256", "b true\n", ""),
            kup("01", "b true\n", ""),
            "refused-kup 4\nrc 1\n".to_string(),
        ] {
            assert!(parse_submit_reply(&reply(&s, &outcome), &s).is_err(), "{outcome:?}");
        }
    }

    #[test]
    fn a_broken_submit_reply_is_an_error_never_an_outcome() {
        let s = sent();
        for outcome in ["enqueued 007\n", "enqueued\n", "enqueued x\n", "queued 1\n", "refused 99\nshort\n", ""] {
            assert!(parse_submit_reply(&reply(&s, outcome), &s).is_err(), "{outcome:?}");
        }
        let mut two = reply(&s, "enqueued 1\n");
        two.truncate(two.len() - 4);
        two.extend(b"enqueued 2\nend\n");
        assert!(parse_submit_reply(&two, &s).is_err(), "two outcomes");
        let error = format!("{OUTPUT_HEADER}\nerror 4\nboom\n");
        assert!(matches!(
            parse_submit_reply(error.as_bytes(), &s),
            Err(SubmitError::Reply(PollError::Wire(WireError::Collector(_))))
        ));
    }

    /// NEGATIVE CONTROL of the echo: a file sha that arrived different (a value swallowed or
    /// shifted on the way) is refused before the outcome is read.
    #[test]
    fn a_submit_echo_that_differs_is_refused() {
        let s = sent();
        let mut other = s.clone();
        other.files[0].digest = Digest::Sha256("0".repeat(64));
        assert!(matches!(
            parse_submit_reply(&reply(&other, "enqueued 1\n"), &s),
            Err(SubmitError::Reply(PollError::ArgMismatch { index: 7, .. }))
        ));
    }

    // --- labels ---

    fn dir(markers: Markers) -> LabelFacts {
        LabelFacts { dir_exists: true, markers, ..LabelFacts::default() }
    }

    const NONE: Markers = Markers { started: false, exit_code: false, cancelled: false, enqueued: false, submitting: false };
    const ALL: Markers = Markers { started: true, exit_code: true, cancelled: true, enqueued: true, submitting: true };

    /// One row per rule and per precedence boundary. NEGATIVE CONTROL (by hand): moving the
    /// `.submitting` rule above the marker/row rule makes the "submitting + …" rows red; moving the
    /// socket-Error rule below it makes "socket error + submitting" red.
    #[test]
    fn label_rules_in_their_order() {
        let submitting = Markers { submitting: true, ..NONE };
        let table: &[(&str, LabelFacts, Label)] = &[
            ("1: no dir, everything else set", LabelFacts { dir_exists: false, markers: ALL, row_holds_job: true, socket_error: true }, Label::NotOnServer),
            ("1: no dir, nothing", LabelFacts::default(), Label::NotOnServer),
            ("2: .started", dir(Markers { started: true, ..NONE }), Label::Classifier),
            ("2: .exit_code", dir(Markers { exit_code: true, ..NONE }), Label::Classifier),
            ("2: .cancelled", dir(Markers { cancelled: true, ..NONE }), Label::Classifier),
            ("2: .enqueued", dir(Markers { enqueued: true, ..NONE }), Label::Classifier),
            ("2: a row", LabelFacts { row_holds_job: true, ..dir(NONE) }, Label::Classifier),
            ("2 before 4: submitting + .enqueued", dir(Markers { enqueued: true, ..submitting }), Label::Classifier),
            ("2 before 4: submitting + .cancelled (a withdraw)", dir(Markers { cancelled: true, ..submitting }), Label::Classifier),
            ("2 before 4: submitting + a row", LabelFacts { row_holds_job: true, ..dir(submitting) }, Label::Classifier),
            ("3: socket error", LabelFacts { socket_error: true, ..dir(NONE) }, Label::Classifier),
            ("3 before 4: socket error + submitting", LabelFacts { socket_error: true, ..dir(submitting) }, Label::Classifier),
            ("4: .submitting", dir(submitting), Label::SubmitInterrupted),
            ("5: an empty dir", dir(NONE), Label::NotOnServer),
        ];
        for (name, facts, want) in table {
            assert_eq!(label(facts), *want, "{name}");
        }
    }

    // --- the label reply ---

    const JOB: &str = "/r/jobs/j1";
    const SOCK: &str = "/r/tsp/slot0.sock";

    fn label_args() -> LabelArgs {
        LabelArgs::new(JOB, SOCK).unwrap()
    }

    /// A label reply for `label_args()`: the echo, then `body` (record lines and byte records).
    fn label_wire(body: &str) -> Vec<u8> {
        format!("{LABEL_HEADER}\nargc 2\narg {}\n{JOB}\narg {}\n{SOCK}\n{body}end\n", JOB.len(), SOCK.len()).into_bytes()
    }

    fn rec(name: &str, text: &str) -> String {
        format!("{name} {}\n{text}\n", text.len())
    }

    fn net_unix(listed: bool) -> String {
        let mut s = "Num       RefCount Protocol Flags    Type St Inode Path\n".to_string();
        if listed {
            s.push_str(&format!("0000000000000000: 00000002 00000000 00010000 0001 01 2960949 {SOCK}\n"));
        }
        rec("net_unix", &s)
    }

    const MARKS: &str = "started no\nexit_code no\ncancelled no\nenqueued yes\nsubmitting yes\n";

    #[test]
    fn label_replies_parse_into_facts() {
        let a = label_args();
        assert_eq!(parse_label_reply(&label_wire("dir no\n"), &a).unwrap(), LabelFacts::default());
        let facts = parse_label_reply(&label_wire(&format!("dir yes\n{MARKS}{}nodaemon\n", net_unix(false))), &a).unwrap();
        assert_eq!(facts.markers, Markers { enqueued: true, submitting: true, ..NONE });
        assert!(facts.dir_exists && !facts.row_holds_job && !facts.socket_error);
        let row = format!("4 queued (file) bash /r/bin/wrapper-ab.sh {JOB} 0 /o");
        let body = format!("dir yes\n{MARKS}{}rows 1\n{}", net_unix(true), rec("row", &row));
        assert!(parse_label_reply(&label_wire(&body), &a).unwrap().row_holds_job);
        let body = format!("dir yes\n{MARKS}{}{}", net_unix(true), rec("tsp_error", "tsp -l failed"));
        assert!(parse_label_reply(&label_wire(&body), &a).unwrap().socket_error);
        // A row of /r/jobs/j10 mentions /r/jobs/j1 as a substring only: it does not hold it.
        let body = format!("dir yes\n{MARKS}{}rows 1\n{}", net_unix(true), rec("row", "4 queued (file) bash w /r/jobs/j10 0 /o"));
        assert!(!parse_label_reply(&label_wire(&body), &a).unwrap().row_holds_job);
    }

    /// NEGATIVE CONTROLS of the re-derivation: a script that ran tsp on an unlisted socket, skipped
    /// a listed one, sent a row that is not the job's, or anything malformed — an error, never facts.
    #[test]
    fn a_label_reply_that_contradicts_itself_is_refused() {
        let a = label_args();
        for body in [
            format!("dir yes\n{MARKS}{}rows 0\n", net_unix(false)),
            format!("dir yes\n{MARKS}{}nodaemon\n", net_unix(true)),
            format!("dir yes\n{MARKS}{}rows 1\n{}", net_unix(true), rec("row", "4 queued (file) other")),
            format!("dir yes\n{MARKS}{}", net_unix(false)),
            "dir maybe\n".to_string(),
            format!("dir no\n{MARKS}"),
            "dir yes\nstarted no\n".to_string(),
            rec("error", "stat: Permission denied"),
        ] {
            assert!(parse_label_reply(&label_wire(&body), &a).is_err(), "{body:?}");
        }
        assert!(LabelArgs::new("/r/jobs/../x", SOCK).is_err());
    }
}
