//! The three static server-side scripts of ADR-024 Decision l, embedded at build time.
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
//!
//! Every per-job value is a positional argument; nothing is substituted into the script text.
//! Unit 5.3 uploads each one as `<root>/bin/<name>-<sha>.sh` (content-addressed, by temp file +
//! rename) using [`sha256_hex`], and the "ours" rule accepts any lowercase-hex sha.

use sha2::{Digest, Sha256};

pub const WRAPPER: &str = concat!(include_str!("scripts/head.sh"), include_str!("scripts/wrapper.sh"));
pub const CANCEL: &str = concat!(include_str!("scripts/head.sh"), include_str!("scripts/cancel.sh"));
pub const COLLECT: &str = concat!(include_str!("scripts/head.sh"), include_str!("scripts/collect.sh"));

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
        for script in [WRAPPER, CANCEL, COLLECT] {
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
}
