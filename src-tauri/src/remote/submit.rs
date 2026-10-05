//! Submit, pure parts (ADR-024 o items 1 and 3): the remote job dir, the values of the one atomic
//! submit call and the strict parser of its reply, and the label rules of the read-only label
//! call.
//!
//! **Submit values** (NUL list on `bash -s` stdin, ADR-024 n item 11): the job dir, the root, the
//! slot socket, the slot's core mask, the ORCA path, then **two values per uploaded file** —
//! name, sha256 — so the server checks the upload itself (o item 3.3.4).
//!
//! **Submit reply** (records, the 5.2 format read by [`Reader`]):
//!
//! ```text
//! orcastudio-submit 1
//! argc <n>
//! arg <len>                      n times: every value, verbatim (o item 3.3, n item 6d)
//! refused <len>                  exactly one of: nothing claimed, then the reason's bytes
//! enqueued <id>                  the tsp id (decimal); `.enqueued` is published
//! failed-after-claim <len>       `.submitting` stays; the reason's bytes
//! end
//! ```
//!
//! A refusal wrote no claim, so the job stays "not on the server" and may be retried; after a
//! claim only the label call decides (o item 3.3.9).

use super::classify::is_valid_path;
use super::poll::{check_echo, PollError};
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
        ];
        values.extend(expected_values(&self.files)?);
        Ok(values)
    }
}

/// What the submit call did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmitReply {
    /// Nothing was claimed; the job is still "not on the server" and may be retried.
    Refused(String),
    /// Enqueued with this `tsp` id; `.enqueued` is published.
    Enqueued(u64),
    /// Claimed (`.submitting` exists) but not enqueued; only the label call decides now.
    FailedAfterClaim(String),
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
        ("refused", arg) => SubmitReply::Refused(r.text_after(arg)?),
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Markers {
    pub started: bool,
    pub exit_code: bool,
    pub cancelled: bool,
    pub enqueued: bool,
    pub submitting: bool,
}

/// The raw facts of one read-only label call, for a remote `Queued` job.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LabelFacts {
    pub dir_exists: bool,
    pub markers: Markers,
    /// The recorded socket has a `tsp -l` row holding the job dir.
    pub row_holds_job: bool,
    /// `tsp -l` on the recorded socket failed (`Error`).
    pub socket_error: bool,
}

/// What the poller does with a remote `Queued` job before any collect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    /// Nothing of the job is on the server: offer retry and withdraw.
    NotOnServer,
    /// Claimed, never enqueued (or the call is still running there): offer withdraw only.
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
    fn submit_values_are_five_then_two_per_file() {
        assert_eq!(
            sent().values().unwrap(),
            [
                format!("{ROOT}/jobs/{ID}").as_str(),
                ROOT,
                &slot_socket_path(ROOT, 0),
                "0-23",
                "/opt/orca/orca",
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
            parse_submit_reply(&reply(&s, "refused 9\nlock busy\n"), &s).unwrap(),
            SubmitReply::Refused("lock busy".into())
        );
        assert_eq!(
            parse_submit_reply(&reply(&s, "failed-after-claim 11\ntsp failed\n\n"), &s).unwrap(),
            SubmitReply::FailedAfterClaim("tsp failed\n".into())
        );
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
            Err(SubmitError::Reply(PollError::ArgMismatch { index: 6, .. }))
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
}
