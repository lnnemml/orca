//! `poll_log` over ssh (ADR-024 o item 7): the values sent, and the strict parser of the reply
//! into a [`LogChunk`] with its post-condition.
//!
//! The server reads `output.out` **size first** in one call —
//! `sz=$(stat -c %s f); head -c "$sz" f | tail -c +$((off+1)) | head -c CAP` — so the size header
//! matches the bytes that follow even while ORCA appends (measured, probe 5.3a). `tail -c +K` past
//! the end prints nothing with rc 0 (measured), so a shrunken log is told by the size alone:
//! `size < offset` ⇒ a `reset`. The rule is [`plan_log_read`], shared with the local backend.
//!
//! **Reply** (records, the 5.2 format read by [`Reader`]):
//!
//! ```text
//! orcastudio-log 1
//! argc 3
//! arg <len>      the job dir, then the offset, then the cap (decimal), each verbatim
//! size <n>|-     output.out's size, or `-` when it does not exist (yet)
//! bytes <len>    then exactly <len> raw bytes and a newline
//! end
//! ```
//!
//! **Post-condition (rule #9):** the echoed values are the ones sent; with a size,
//! `len(bytes) == min(cap, size − offset)` when `size ≥ offset`, and no bytes on a reset; with no
//! file, no bytes and the offset unchanged (an absent `output.out` is not an error).

use super::classify::is_valid_path;
use super::ssh::MAX_OUTPUT_BYTES;
use super::wire::{parse_decimal, Reader, WireError};
use crate::execution_backend::{plan_log_read, LogChunk, LogRead, POLL_LOG_MAX_BYTES};

/// The first line of every reply; the number is the format version.
pub const OUTPUT_HEADER: &str = "orcastudio-log 1";

/// Room left in the ssh runner's output cap for everything but the log bytes (the header, the
/// echoed values, the records). A job dir is a path, far below this.
const FRAMING_MARGIN: usize = 64 * 1024;

// The cap must leave the reply under the runner's output limit, or a full poll would be killed as
// `OutputTooLarge` instead of delivered (ADR-024 o item 7).
const _: () = assert!(POLL_LOG_MAX_BYTES as usize + FRAMING_MARGIN < MAX_OUTPUT_BYTES);

/// What one poll sends: the job's recorded remote dir (its `output.out` is read), the byte offset
/// and the cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PollLogArgs {
    pub job_dir: String,
    pub offset: u64,
    pub cap: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PollError {
    #[error("job dir {0:?} breaks the path rule")]
    JobDir(String),
    #[error(transparent)]
    Wire(#[from] WireError),
    #[error("sent {sent} values, the script received {received}")]
    ArgCount { sent: usize, received: usize },
    #[error("value {index} was sent as {sent:?} but received as {received:?}")]
    ArgMismatch { index: usize, sent: String, received: String },
    #[error("poll_log post-condition: {0}")]
    PostCondition(String),
}

impl PollLogArgs {
    /// A poll of `job_dir` from `offset`, at most [`POLL_LOG_MAX_BYTES`].
    pub fn new(job_dir: &str, offset: u64) -> Result<Self, PollError> {
        if !is_valid_path(job_dir) {
            return Err(PollError::JobDir(job_dir.to_string()));
        }
        Ok(PollLogArgs { job_dir: job_dir.to_string(), offset, cap: POLL_LOG_MAX_BYTES })
    }

    /// The NUL-list values, in the order the script reads them.
    pub fn values(&self) -> Vec<String> {
        vec![self.job_dir.clone(), self.offset.to_string(), self.cap.to_string()]
    }
}

/// Parse one reply strictly and check it against what was sent.
pub fn parse_poll_reply(output: &[u8], sent: &PollLogArgs) -> Result<LogChunk, PollError> {
    let mut r = Reader::new(output);
    let (name, arg) = r.line()?;
    if OUTPUT_HEADER.split_once(' ') != Some((name, arg.unwrap_or_default())) {
        return Err(r.malformed(format!("expected {OUTPUT_HEADER:?}")).into());
    }
    check_echo(&mut r, &sent.values())?;

    let size = match r.word("size")? {
        "-" => None,
        text => Some(parse_decimal(text).ok_or_else(|| r.malformed(format!("size {text:?}")))?),
    };
    let bytes = r.bytes("bytes")?.ok_or_else(|| r.malformed("bytes is required".into()))?;
    if r.line()? != ("end", None) {
        return Err(r.malformed("expected end".into()).into());
    }
    if !r.at_end() {
        return Err(r.malformed("bytes after end".into()).into());
    }

    let got = bytes.len() as u64;
    let Some(size) = size else {
        return if got == 0 {
            Ok(LogChunk::unchanged(sent.offset))
        } else {
            Err(PollError::PostCondition(format!("no output.out, yet {got} bytes")))
        };
    };
    match plan_log_read(size, sent.offset, sent.cap) {
        LogRead::Reset if got == 0 => Ok(LogChunk::reset()),
        LogRead::Reset => Err(PollError::PostCondition(format!(
            "size {size} < offset {}, yet {got} bytes",
            sent.offset
        ))),
        LogRead::Range { start, len } if got == len => Ok(LogChunk { offset: start + len, bytes, reset: false }),
        LogRead::Range { len, .. } => Err(PollError::PostCondition(format!(
            "size {size}, offset {}, cap {}: expected {len} bytes, got {got}",
            sent.offset, sent.cap
        ))),
    }
}

/// The transport's post-condition (ADR-024 n items 6d, 11): the script echoes every value it read
/// and each must be exactly what was sent. Shared by every script reply of 5.3.
pub(crate) fn check_echo(r: &mut Reader<'_>, sent: &[String]) -> Result<(), PollError> {
    let received = r.count("argc")?;
    if received != sent.len() {
        return Err(PollError::ArgCount { sent: sent.len(), received });
    }
    for (index, want) in sent.iter().enumerate() {
        let got = r.bytes("arg")?.ok_or_else(|| r.malformed("arg is required".into()))?;
        if got != want.as_bytes() {
            return Err(PollError::ArgMismatch {
                index,
                sent: want.clone(),
                received: String::from_utf8_lossy(&got).into_owned(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const JOB: &str = "/home/anton/.orcastudio/jobs/0b7c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3";

    fn args(offset: u64, cap: u64) -> PollLogArgs {
        PollLogArgs { job_dir: JOB.into(), offset, cap }
    }

    /// The reply bytes for `sent`, with `size` and `bytes` as the script would print them.
    fn reply(sent: &PollLogArgs, size: Option<u64>, bytes: &[u8]) -> Vec<u8> {
        let mut out = format!("{OUTPUT_HEADER}\nargc 3\n").into_bytes();
        for v in sent.values() {
            out.extend(format!("arg {}\n{v}\n", v.len()).bytes());
        }
        out.extend(match size {
            Some(n) => format!("size {n}\n"),
            None => "size -\n".into(),
        }.bytes());
        out.extend(format!("bytes {}\n", bytes.len()).bytes());
        out.extend_from_slice(bytes);
        out.extend(b"\nend\n");
        out
    }

    #[test]
    fn a_full_read_and_a_capped_read() {
        let sent = args(4, 1024);
        assert_eq!(
            parse_poll_reply(&reply(&sent, Some(10), b"EFGHIJ"), &sent),
            Ok(LogChunk { offset: 10, bytes: b"EFGHIJ".to_vec(), reset: false })
        );
        let sent = args(0, 3);
        assert_eq!(
            parse_poll_reply(&reply(&sent, Some(10), b"ABC"), &sent),
            Ok(LogChunk { offset: 3, bytes: b"ABC".to_vec(), reset: false })
        );
    }

    #[test]
    fn bytes_are_raw_and_may_end_inside_a_character() {
        let sent = args(0, 3);
        let chunk = parse_poll_reply(&reply(&sent, Some(4), b"ab\xC3"), &sent).unwrap();
        assert_eq!(chunk.bytes, b"ab\xC3", "no decoding at the transport");
    }

    #[test]
    fn no_growth_absent_file_and_shrunken_file() {
        let sent = args(10, 1024);
        assert_eq!(parse_poll_reply(&reply(&sent, Some(10), b""), &sent), Ok(LogChunk::unchanged(10)));
        assert_eq!(parse_poll_reply(&reply(&sent, None, b""), &sent), Ok(LogChunk::unchanged(10)));
        assert_eq!(parse_poll_reply(&reply(&sent, Some(9), b""), &sent), Ok(LogChunk::reset()));
    }

    /// NEGATIVE CONTROLS of the post-condition: a byte too many or too few, bytes on a reset, bytes
    /// with no file — each is refused, never delivered as a plausible chunk.
    #[test]
    fn a_reply_that_breaks_the_length_rule_is_refused() {
        let sent = args(4, 1024);
        for (size, bytes) in [
            (Some(10), &b"EFGHI"[..]),
            (Some(10), &b"EFGHIJK"[..]),
            (Some(3), &b"x"[..]),
            (None, &b"x"[..]),
        ] {
            assert!(
                matches!(parse_poll_reply(&reply(&sent, size, bytes), &sent), Err(PollError::PostCondition(_))),
                "size {size:?}, {} bytes",
                bytes.len()
            );
        }
        let sent = args(0, 3);
        assert!(matches!(
            parse_poll_reply(&reply(&sent, Some(10), b"ABCD"), &sent),
            Err(PollError::PostCondition(_))
        ));
    }

    #[test]
    fn an_echo_that_differs_from_what_was_sent_is_refused() {
        let sent = args(4, 1024);
        let other = args(5, 1024);
        assert!(matches!(
            parse_poll_reply(&reply(&other, Some(10), b"FGHIJ"), &sent),
            Err(PollError::ArgMismatch { index: 1, .. })
        ));
    }

    #[test]
    fn malformed_replies_are_refused() {
        let sent = args(0, 1024);
        let good = reply(&sent, Some(2), b"ab");
        let mut trailing = good.clone();
        trailing.extend(b"x");
        let bad_size = String::from_utf8(good.clone()).unwrap().replace("size 2", "size 02");
        let error = format!("{OUTPUT_HEADER}\nerror 4\nboom\n");
        for wire in [trailing, bad_size.into_bytes(), b"orcastudio-log 2\n".to_vec(), error.into_bytes()] {
            assert!(matches!(parse_poll_reply(&wire, &sent), Err(PollError::Wire(_))));
        }
    }

    #[test]
    fn args_carry_the_shared_cap_and_a_valid_dir() {
        let a = PollLogArgs::new(JOB, 7).unwrap();
        assert_eq!(a.values(), [JOB, "7", &POLL_LOG_MAX_BYTES.to_string()]);
        assert!(PollLogArgs::new("/r/jobs/../x", 0).is_err());
    }
}
