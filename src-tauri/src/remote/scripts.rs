//! The static server-side scripts of ADR-024 Decisions l and n, embedded at build time.
//!
//! Each script is the shared head (`scripts/head.sh`: shebang, `set -u`, the strict `.started` /
//! `.enqueued` / stat parsers and the ENOENT-vs-error readers) followed by its own body. The
//! concatenation happens here, at compile time, so the three scripts ship the same parser code
//! byte for byte and the tests run exactly the bytes that are uploaded.
//!
//! - [`WRAPPER`] — `wrapper.sh <job_dir> <mask> <orca_path>`: the job's start sequence.
//! - [`CANCEL`] — `cancel.sh cancel|check <job_dir> <root>`: the cancel script; `check` prints
//!   its predicates without writing or signalling.
//! - [`COLLECT`] — `collect.sh <job_dir> [<socket>...]`: one raw-fact snapshot on stdout, parsed
//!   by [`super::wire::parse_snapshot`].
//! - [`CONNTEST`] — the profile connection test (ADR-024 n). Unlike the three job scripts it is not
//!   uploaded: it is fed to `bash -s` on stdin followed by its values as a NUL list
//!   ([`crate::connection_test::conntest_stdin`]), and its records are parsed by
//!   [`crate::connection_test::parse_output`]. Of the head it uses only `valid_path`.
//!
//! The stdin-fed calls of unit 5.3 (ADR-024 o) are fed the same way ([`stdin_with_values`]; n item 11:
//! the read loop is each script's last line):
//! - [`SUBMIT`] — the one atomic submit call; values [`super::submit::SubmitArgs::values`], reply
//!   [`super::submit::parse_submit_reply`].
//! - [`LABEL`] — the read-only label call; values [`super::submit::LabelArgs::values`], reply
//!   [`super::submit::parse_label_reply`].
//! - [`POLL_LOG`] — one chunk of `output.out`; values [`super::poll::PollLogArgs::values`], reply
//!   [`super::poll::parse_poll_reply`].
//! - [`LIST`] — the server's listing for the download post-condition; values
//!   [`super::sync::ListArgs::values`], reply [`super::sync::parse_list_reply`].
//! - [`PREPARE`] — the read-only pre-upload call of a submit (o items 3.2, 13.1): the shapes of
//!   the root's components and whether the wrapper already hashes right; values
//!   [`super::prepare::PrepareArgs::values`], reply [`super::prepare::parse_prepare_reply`].
//! - [`INSTALL`] — `mkdir -p <root>/bin <root>/tsp` and the upload of the wrapper, `cancel.sh` and
//!   `collect.sh` by unique temp name + rename; values [`super::prepare::InstallArgs::values`], reply
//!   [`super::prepare::parse_install_reply`].
//! - [`RUN`] — the trampoline: the one way an uploaded `cancel.sh`/`collect.sh` runs (o item 14.1);
//!   values [`super::run::RunArgs::values`], reply [`super::run::parse_run_reply`].
//! - [`MKJOB`] — a withdraw's `mkdir -p <job dir>` and the o-1 shapes after it; values
//!   [`super::run::MkjobArgs::values`], reply [`super::run::parse_mkjob_reply`].
//!
//! Every per-job value is a positional argument or a NUL-list value; nothing is substituted into
//! the script text. Unit 5.3 uploads the job scripts as `<root>/bin/<name>-<sha>.sh`
//! (content-addressed, by temp file + rename; [`upload_path`]) using [`sha256_hex`], and the
//! "ours" rule accepts any lowercase-hex sha.

use sha2::{Digest, Sha256};

pub const WRAPPER: &str = concat!(include_str!("scripts/head.sh"), include_str!("scripts/wrapper.sh"));
pub const CANCEL: &str = concat!(include_str!("scripts/head.sh"), include_str!("scripts/cancel.sh"));
pub const COLLECT: &str = concat!(include_str!("scripts/head.sh"), include_str!("scripts/collect.sh"));
pub const CONNTEST: &str = concat!(include_str!("scripts/head.sh"), include_str!("scripts/conntest.sh"));
pub const SUBMIT: &str = concat!(include_str!("scripts/head.sh"), include_str!("scripts/submit.sh"));
pub const LABEL: &str = concat!(include_str!("scripts/head.sh"), include_str!("scripts/label.sh"));
pub const POLL_LOG: &str = concat!(include_str!("scripts/head.sh"), include_str!("scripts/poll_log.sh"));
pub const LIST: &str = concat!(include_str!("scripts/head.sh"), include_str!("scripts/list.sh"));
pub const PREPARE: &str = concat!(include_str!("scripts/head.sh"), include_str!("scripts/prepare.sh"));
pub const INSTALL: &str = concat!(include_str!("scripts/head.sh"), include_str!("scripts/install.sh"));
pub const RUN: &str = concat!(include_str!("scripts/head.sh"), include_str!("scripts/run.sh"));
pub const MKJOB: &str = concat!(include_str!("scripts/head.sh"), include_str!("scripts/mkjob.sh"));

/// The scripts fed through `bash -s` stdin, followed by their values (ADR-024 n item 11).
pub const STDIN_SCRIPTS: [&str; 9] = [CONNTEST, SUBMIT, LABEL, POLL_LOG, LIST, PREPARE, INSTALL, RUN, MKJOB];

/// The last line of every stdin-fed script: the read loop that takes the NUL list. Anything after
/// it would be read as values (probe 5.1c), so it must be the last line, exactly.
pub const READ_LOOP: &str = "args=(); while IFS= read -r -d '' a; do args+=(\"$a\"); done; main \"${args[@]}\"; exit\n";

/// A value that holds a NUL byte cannot travel in a NUL-separated list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a value to send contains a NUL byte, which the NUL-separated list cannot carry")]
pub struct ValueHasNul;

/// The stdin of one `bash -s` call: the script, then each value followed by a NUL.
pub fn stdin_with_values<S: AsRef<str>>(script: &str, values: &[S]) -> Result<Vec<u8>, ValueHasNul> {
    let mut stdin = script.as_bytes().to_vec();
    for value in values.iter().map(AsRef::as_ref) {
        if value.contains('\0') {
            return Err(ValueHasNul);
        }
        stdin.extend_from_slice(value.as_bytes());
        stdin.push(0);
    }
    Ok(stdin)
}

/// Where a job script is uploaded: `<root>/bin/<name>-<sha256 of its bytes>.sh` (ADR-024 l).
pub fn upload_path(root: &str, name: &str, script: &str) -> String {
    format!("{root}/bin/{name}-{}.sh", sha256_hex(script))
}

/// Lowercase hex sha256 of a script's bytes — the `<sha>` of its upload name.
pub fn sha256_hex(script: &str) -> String {
    Sha256::digest(script.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_script_starts_with_the_shebang_and_shares_the_head() {
        let head = include_str!("scripts/head.sh");
        for script in [WRAPPER, CANCEL, COLLECT, CONNTEST, SUBMIT, LABEL, POLL_LOG, LIST, PREPARE, INSTALL, RUN, MKJOB] {
            assert!(script.starts_with("#!/bin/bash\n"));
            assert!(script.starts_with(head));
            // Exactly one shebang: the bodies must not carry their own.
            assert_eq!(script.matches("#!/bin/bash").count(), 1);
        }
    }

    #[test]
    fn sha256_is_lowercase_hex_of_the_bytes() {
        // sha256("") is the well-known e3b0c442… value.
        assert_eq!(
            sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let sha = sha256_hex(WRAPPER);
        assert_eq!(sha.len(), 64);
        assert!(sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    }

    /// n item 11: the read loop is the last line of every stdin-fed script, exactly once.
    #[test]
    fn every_stdin_script_ends_with_the_read_loop() {
        for script in STDIN_SCRIPTS {
            assert!(script.ends_with(READ_LOOP), "a stdin-fed script does not end with the read loop");
            assert_eq!(script.matches("while IFS= read -r -d '' a").count(), 1);
        }
    }

    #[test]
    fn stdin_is_the_script_then_nul_terminated_values() {
        let stdin = stdin_with_values("S\n", &["a b", "", "x\ny"]).unwrap();
        assert_eq!(stdin, b"S\na b\0\0x\ny\0");
        assert_eq!(stdin_with_values("S", &["a\0b"]), Err(ValueHasNul));
    }

    #[test]
    fn upload_path_is_content_addressed() {
        assert_eq!(
            upload_path("/r", "wrapper", WRAPPER),
            format!("/r/bin/wrapper-{}.sh", sha256_hex(WRAPPER))
        );
    }
}
