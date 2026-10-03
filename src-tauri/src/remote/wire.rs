//! The collector's wire format, parsed strictly into a [`Snapshot`] (ADR-024 l).
//!
//! `collect.sh` (see [`super::scripts::COLLECT`]) prints one snapshot as a sequence of records in
//! a **fixed order**. A record is one ASCII line `<name>` or `<name> <arg>`; a **byte record**
//! has a decimal length (no leading zeros) or `-` (absent) as its arg, and a present one is
//! followed by exactly that many raw bytes and a `\n`. Raw bytes may hold NULs (a cmdline) or
//! newlines, which is why every value is length-prefixed instead of line-delimited.
//!
//! ```text
//! orcastudio-snapshot 1
//! boot_id <len>              /proc/sys/kernel/random/boot_id (required)
//! started <len>|-            .started, first read
//! proc collected|skipped     collected iff .started parses and is from this boot
//!   wrapper_stat <len>|-     /proc/<pid>/stat              ┐
//!   wrapper_cmdline <len>|-  /proc/<pid>/cmdline           │ only when
//!   sid_stat <len>|-         /proc/<sid>/stat              │ collected
//!   members <n>              ps -s <sid>, then n times:    │
//!     member <pid>                                         │
//!     cwd <len>|-            /proc/<pid>/cwd (raw target)  ┘
//! sockets <n>                then n times:
//!   socket <len>             the socket path
//!   socket_error <len>       the socket could not be determined (an unparsable .enqueued), or
//!   net_unix <len>           /proc/net/unix header + the lines listing this path, then one of
//!     nodaemon               not listed: tsp was not run
//!     rows <n>               listed: n × `row <len>`, the tsp -l lines mentioning the job dir
//!     tsp_error <len>        listed, but tsp -l failed
//! exit_code <len>|-
//! cancelled yes|no
//! tail <len>|-               the last 5 KiB of output.out
//! started <len>|-            .started, read again last
//! end
//! ```
//!
//! An `error <len>` record may replace any record line: the collector hit a read error other than
//! ENOENT/ESRCH and stopped (it also exits 3). That is [`WireError::Collector`] — the snapshot is
//! unusable, never "absent". Unknown, missing, duplicate or out-of-order records, a bad length,
//! a missing `\n` after the bytes and anything after `end` are [`WireError::Malformed`].
//!
//! The parser also re-checks what the collector decided (rule #9) and reports a disagreement as
//! [`WireError::Inconsistent`]: whether `/proc` was collected must match Rust's own parse of
//! `.started` and `boot_id`; `nodaemon` vs `rows`/`tsp_error` must match Rust's
//! [`unix_socket_listed`] over the emitted `/proc/net/unix` evidence; the slot sockets must come
//! first, in order, with at most one extra (the `.enqueued` socket); every row must mention the
//! job dir; the tail is at most `TAIL_BYTES`.

use super::markers::{parse_started, BootId};
use super::procfs::unix_socket_listed;
use super::snapshot::{Attempt, JobIdentity, SessionMember, Snapshot, SocketFact, SocketState};
use crate::local_backend::TAIL_BYTES;

/// The first line of every snapshot; the number is the format version.
pub const HEADER: &str = "orcastudio-snapshot 1";

/// No single record is larger than this (a stat line, a cwd, a 5 KiB tail, a tsp row…).
const MAX_RECORD: usize = 1 << 20;
/// No count (members, sockets, rows) is larger than this.
const MAX_COUNT: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    /// The collector stopped on a read error it must not read as "absent".
    #[error("the collector reported an error: {0}")]
    Collector(String),
    #[error("malformed record stream at byte {offset}: {what}")]
    Malformed { offset: usize, what: String },
    #[error("inconsistent snapshot: {0}")]
    Inconsistent(String),
}

/// Parse one collector output. `slot_sockets` are the profile's slot sockets, in the order they
/// were passed to `collect.sh`.
pub fn parse_snapshot(
    wire: &[u8],
    identity: JobIdentity,
    attempt: Attempt,
    slot_sockets: &[String],
) -> Result<Snapshot, WireError> {
    let mut r = Reader { buf: wire, pos: 0 };
    let header = r.line()?;
    if HEADER.split_once(' ') != Some((header.0, header.1.unwrap_or_default())) {
        return Err(r.malformed(format!("expected {HEADER:?}")));
    }

    let boot = r.bytes("boot_id")?.ok_or_else(|| r.malformed("boot_id is required".into()))?;
    let current_boot_id =
        String::from_utf8(boot).map_err(|_| WireError::Inconsistent("boot_id is not UTF-8".into()))?;
    let started_first = r.bytes("started")?;

    let collected = match r.word("proc")? {
        "collected" => true,
        "skipped" => false,
        other => return Err(r.malformed(format!("proc {other:?}"))),
    };
    check_proc_scope(collected, started_first.as_deref(), &current_boot_id)?;

    let (mut wrapper_stat, mut wrapper_cmdline, mut sid_stat) = (None, Vec::new(), None);
    let mut session_members = Vec::new();
    if collected {
        wrapper_stat = r.bytes("wrapper_stat")?;
        wrapper_cmdline = r.bytes("wrapper_cmdline")?.unwrap_or_default();
        sid_stat = r.bytes("sid_stat")?;
        for _ in 0..r.count("members")? {
            let pid_text = r.word("member")?;
            let pid = parse_decimal(pid_text)
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n > 0)
                .ok_or_else(|| r.malformed(format!("member pid {pid_text:?}")))?;
            let cwd = r.bytes("cwd")?;
            session_members.push(SessionMember { pid, cwd });
        }
    }

    let mut sockets = Vec::new();
    for _ in 0..r.count("sockets")? {
        sockets.push(socket_fact(&mut r, &identity.job_dir)?);
    }
    check_socket_list(&sockets, slot_sockets, &identity.job_dir)?;

    let exit_code = r.bytes("exit_code")?;
    let cancelled = match r.word("cancelled")? {
        "yes" => true,
        "no" => false,
        other => return Err(r.malformed(format!("cancelled {other:?}"))),
    };
    let output_tail = r.bytes("tail")?.unwrap_or_default();
    if output_tail.len() as u64 > TAIL_BYTES {
        return Err(WireError::Inconsistent(format!(
            "tail is {} bytes, more than {TAIL_BYTES}",
            output_tail.len()
        )));
    }
    let started_last = r.bytes("started")?;
    let end = r.line()?;
    if end != ("end", None) {
        return Err(r.malformed("expected end".into()));
    }
    if r.pos != r.buf.len() {
        return Err(r.malformed("bytes after end".into()));
    }

    Ok(Snapshot {
        identity,
        attempt,
        current_boot_id,
        started_first,
        wrapper_stat,
        wrapper_cmdline,
        sid_stat,
        session_members,
        sockets,
        exit_code,
        cancelled,
        output_tail,
        started_last,
    })
}

/// The collector reads `/proc` only for a `.started` that parses and is from this boot. Rust
/// re-derives that from the same bytes; if they disagree the snapshot is unusable — reading a
/// skipped `/proc` as "no process" would make a running job `Lost`. A corrupt `.started` (row 1)
/// or a bad `boot_id` (a `SnapshotError` in `classify`) does not use `/proc`, so either is fine.
fn check_proc_scope(collected: bool, started: Option<&[u8]>, boot: &str) -> Result<(), WireError> {
    let expected = match (started.map(parse_started), BootId::parse(boot)) {
        (None, _) => Some(false),
        (Some(Ok(s)), Ok(current)) => Some(s.boot_id == current),
        _ => None,
    };
    match expected {
        Some(want) if want != collected => Err(WireError::Inconsistent(format!(
            "the collector {} /proc, but .started says it should{} have",
            if collected { "read" } else { "skipped" },
            if want { "" } else { " not" }
        ))),
        _ => Ok(()),
    }
}

fn socket_fact(r: &mut Reader<'_>, job_dir: &str) -> Result<SocketFact, WireError> {
    let path = r.bytes("socket")?.ok_or_else(|| r.malformed("socket path is required".into()))?;
    let socket_path =
        String::from_utf8(path).map_err(|_| WireError::Inconsistent("socket path is not UTF-8".into()))?;
    let (name, arg) = r.line()?;
    let state = match name {
        "socket_error" => SocketState::Error(r.text_after(arg)?),
        "net_unix" => {
            let evidence = r.text_after(arg)?;
            let listed = unix_socket_listed(&evidence, &socket_path)
                .map_err(|e| WireError::Inconsistent(e.to_string()))?;
            let (name, arg) = r.line()?;
            let state = match (name, arg) {
                ("nodaemon", None) => SocketState::NoDaemon,
                ("rows", Some(n)) => {
                    let n = r.parse_count(n)?;
                    let mut rows = Vec::with_capacity(n);
                    for _ in 0..n {
                        let row = r.bytes("row")?.ok_or_else(|| r.malformed("row is required".into()))?;
                        let row = String::from_utf8(row)
                            .map_err(|_| WireError::Inconsistent("tsp row is not UTF-8".into()))?;
                        if !row.contains(job_dir) {
                            return Err(WireError::Inconsistent(format!("row does not mention the job dir: {row:?}")));
                        }
                        rows.push(row);
                    }
                    SocketState::Rows(rows)
                }
                ("tsp_error", arg) => SocketState::Error(r.text_after(arg)?),
                _ => return Err(r.malformed(format!("expected a socket state, got {name:?}"))),
            };
            // `tsp` may run only on a listed socket; a listed socket must have been queried.
            if listed == matches!(state, SocketState::NoDaemon) {
                return Err(WireError::Inconsistent(format!(
                    "{socket_path}: /proc/net/unix says listed={listed}, the collector says {state:?}"
                )));
            }
            state
        }
        _ => return Err(r.malformed(format!("expected socket_error or net_unix, got {name:?}"))),
    };
    Ok(SocketFact { socket_path, state })
}

/// The slot sockets first, in order, then at most one extra: the `.enqueued` socket (which may
/// have left the profile), or the `.enqueued` marker's own path for an unparsable marker.
fn check_socket_list(facts: &[SocketFact], slots: &[String], job_dir: &str) -> Result<(), WireError> {
    let paths: Vec<&str> = facts.iter().map(|f| f.socket_path.as_str()).collect();
    let slot_paths: Vec<&str> = slots.iter().map(String::as_str).collect();
    if paths.len() < slot_paths.len() || paths[..slot_paths.len()] != slot_paths[..] {
        return Err(WireError::Inconsistent(format!("socket facts {paths:?} do not start with the slots {slot_paths:?}")));
    }
    match facts.get(slot_paths.len()..) {
        Some([]) => Ok(()),
        Some([extra]) => {
            let bad_marker = extra.socket_path == format!("{job_dir}/.enqueued");
            if slot_paths.contains(&extra.socket_path.as_str()) {
                Err(WireError::Inconsistent(format!("socket {} listed twice", extra.socket_path)))
            } else if bad_marker && !matches!(extra.state, SocketState::Error(_)) {
                Err(WireError::Inconsistent("the .enqueued marker path is not a socket".into()))
            } else {
                Ok(())
            }
        }
        _ => Err(WireError::Inconsistent(format!("more than one extra socket fact: {paths:?}"))),
    }
}

/// The strict record reader shared by the collector's snapshot and the connection test's output
/// (`crate::connection_test`): record lines, counts and length-prefixed byte records.
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    /// Whether every byte has been consumed.
    pub(crate) fn at_end(&self) -> bool {
        self.pos == self.buf.len()
    }

    pub(crate) fn malformed(&self, what: String) -> WireError {
        WireError::Malformed { offset: self.pos, what }
    }

    /// The next record line as `(name, arg)`. An `error` record ends the parse with
    /// [`WireError::Collector`].
    pub(crate) fn line(&mut self) -> Result<(&'a str, Option<&'a str>), WireError> {
        let rest = &self.buf[self.pos..];
        let len = rest
            .iter()
            .position(|b| *b == b'\n')
            .ok_or_else(|| self.malformed("unterminated record line".into()))?;
        let line = std::str::from_utf8(&rest[..len])
            .ok()
            .filter(|l| l.bytes().all(|b| b.is_ascii_graphic() || b == b' '))
            .ok_or_else(|| self.malformed("record line is not printable ASCII".into()))?;
        self.pos += len + 1;
        let (name, arg) = match line.split_once(' ') {
            Some((name, arg)) => (name, Some(arg)),
            None => (line, None),
        };
        if name.is_empty() || arg.is_some_and(|a| a.is_empty() || a.contains(' ')) {
            return Err(self.malformed(format!("bad record line {line:?}")));
        }
        if name == "error" {
            let message = self.text_after(arg)?;
            return Err(WireError::Collector(message));
        }
        Ok((name, arg))
    }

    pub(crate) fn expect(&mut self, want: &str) -> Result<Option<&'a str>, WireError> {
        let (name, arg) = self.line()?;
        if name != want {
            return Err(self.malformed(format!("expected {want:?}, got {name:?}")));
        }
        Ok(arg)
    }

    /// `<name> <word>`.
    pub(crate) fn word(&mut self, want: &str) -> Result<&'a str, WireError> {
        self.expect(want)?
            .ok_or_else(|| self.malformed(format!("{want} needs an argument")))
    }

    /// `<name> <n>`, a bounded count.
    pub(crate) fn count(&mut self, want: &str) -> Result<usize, WireError> {
        let arg = self.word(want)?;
        self.parse_count(arg)
    }

    fn parse_count(&self, arg: &str) -> Result<usize, WireError> {
        parse_decimal(arg)
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n <= MAX_COUNT)
            .ok_or_else(|| self.malformed(format!("bad count {arg:?}")))
    }

    /// A byte record: `Some(bytes)`, or `None` for `<name> -`.
    pub(crate) fn bytes(&mut self, want: &str) -> Result<Option<Vec<u8>>, WireError> {
        match self.expect(want)? {
            Some("-") => Ok(None),
            arg => self.payload(arg).map(Some),
        }
    }

    /// The payload of a byte record that cannot be absent, as UTF-8 text.
    fn text_after(&mut self, arg: Option<&str>) -> Result<String, WireError> {
        let bytes = self.payload(arg)?;
        String::from_utf8(bytes).map_err(|_| self.malformed("text record is not UTF-8".into()))
    }

    fn payload(&mut self, arg: Option<&str>) -> Result<Vec<u8>, WireError> {
        let arg = arg.ok_or_else(|| self.malformed("missing length".into()))?;
        let len = parse_decimal(arg)
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n <= MAX_RECORD)
            .ok_or_else(|| self.malformed(format!("bad length {arg:?}")))?;
        let end = self.pos + len;
        if end >= self.buf.len() || self.buf[end] != b'\n' {
            return Err(self.malformed(format!("{len} bytes and a newline expected")));
        }
        let bytes = self.buf[self.pos..end].to_vec();
        self.pos = end + 1;
        Ok(bytes)
    }
}

/// ASCII digits, no sign, no leading zero (except `0` itself).
fn parse_decimal(text: &str) -> Option<u64> {
    let canonical = !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    if canonical {
        text.parse().ok()
    } else {
        None
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const JOB: &str = "/home/anton/.orcastudio/jobs/j1";
    const ROOT: &str = "/home/anton/.orcastudio";
    const SLOT: &str = "/home/anton/.orcastudio/tsp/slot0.sock";
    const BOOT: &str = "0f6e3c1a-5b7d-4c2e-9a8b-1d2e3f4a5b6c";

    /// Builds wire bytes record by record, the way collect.sh prints them.
    #[derive(Default)]
    struct Wire(Vec<u8>);

    impl Wire {
        fn line(mut self, line: &str) -> Self {
            self.0.extend_from_slice(line.as_bytes());
            self.0.push(b'\n');
            self
        }
        fn rec(mut self, name: &str, bytes: &[u8]) -> Self {
            self.0.extend_from_slice(format!("{name} {}\n", bytes.len()).as_bytes());
            self.0.extend_from_slice(bytes);
            self.0.push(b'\n');
            self
        }
    }

    fn started(boot: &str) -> String {
        format!("pid=4242\npgid=4242\nsid=4242\nboot_id={boot}\nstarttime=777\nstarted_at=1\n")
    }

    fn net_unix(listed: bool) -> String {
        let mut s = "Num       RefCount Protocol Flags    Type St Inode Path\n".to_string();
        if listed {
            s.push_str(&format!("0000000000000000: 00000002 00000000 00010000 0001 01 2960949 {SLOT}\n"));
        }
        s
    }

    /// A complete, valid snapshot of a running job: `.started` this boot, /proc collected, one
    /// slot socket with a daemon and one row.
    fn running() -> Wire {
        let stat = "4242 (bash) S 1 4242 4242 0 -1 4194304 95 0 0 0 0 0 0 0 20 0 1 0 777 1 1";
        Wire::default()
            .line(HEADER)
            .rec("boot_id", format!("{BOOT}\n").as_bytes())
            .rec("started", started(BOOT).as_bytes())
            .line("proc collected")
            .rec("wrapper_stat", stat.as_bytes())
            .rec("wrapper_cmdline", format!("bash\0{ROOT}/bin/wrapper-ab.sh\0{JOB}\00\0/opt/orca\0").as_bytes())
            .rec("sid_stat", stat.as_bytes())
            .line("members 2")
            .line("member 4242")
            .rec("cwd", JOB.as_bytes())
            .line("member 4243")
            .line("cwd -")
            .line("sockets 1")
            .rec("socket", SLOT.as_bytes())
            .rec("net_unix", net_unix(true).as_bytes())
            .line("rows 1")
            .rec("row", format!("3    running    /tmp/ts-out.x   bash {ROOT}/bin/wrapper-ab.sh {JOB} 0 /opt/orca").as_bytes())
    }

    fn finish(w: Wire) -> Vec<u8> {
        finish_with(w, &started(BOOT))
    }

    /// The records after the sockets, with `last_started` as the bracketing re-read.
    fn finish_with(w: Wire, last_started: &str) -> Vec<u8> {
        w.line("exit_code -")
            .line("cancelled no")
            .rec("tail", b"line\n")
            .rec("started", last_started.as_bytes())
            .line("end")
            .0
    }

    fn parse(wire: &[u8]) -> Result<Snapshot, WireError> {
        parse_snapshot(
            wire,
            JobIdentity { job_dir: JOB.into(), root: ROOT.into() },
            Attempt::First,
            &[SLOT.to_string()],
        )
    }

    #[test]
    fn a_full_snapshot_round_trips() {
        let snap = parse(&finish(running())).unwrap();
        assert_eq!(snap.current_boot_id, format!("{BOOT}\n"));
        assert_eq!(snap.started_first, Some(started(BOOT).into_bytes()));
        assert_eq!(snap.started_last, snap.started_first);
        assert!(snap.wrapper_cmdline.contains(&0), "NUL bytes survive");
        assert_eq!(
            snap.session_members,
            vec![
                SessionMember { pid: 4242, cwd: Some(JOB.as_bytes().to_vec()) },
                SessionMember { pid: 4243, cwd: None },
            ]
        );
        assert!(matches!(&snap.sockets[0].state, SocketState::Rows(rows) if rows.len() == 1));
        assert_eq!(snap.exit_code, None);
        assert!(!snap.cancelled);
        assert_eq!(snap.output_tail, b"line\n");
    }

    #[test]
    fn an_error_record_anywhere_fails_the_snapshot() {
        let mut wire = running().0;
        let msg = "read x: Permission denied";
        wire.extend_from_slice(format!("error {}\n{msg}\n", msg.len()).as_bytes());
        assert_eq!(parse(&wire), Err(WireError::Collector("read x: Permission denied".into())));
        // Even in place of the header.
        assert!(matches!(parse(b"error 3\nboo\n"), Err(WireError::Collector(_))));
    }

    #[test]
    fn malformed_wires_are_rejected() {
        let good = finish(running());
        let text = String::from_utf8_lossy(&good).into_owned();
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty", Vec::new()),
            ("trailing garbage", [good.clone(), b"x".to_vec()].concat()),
            ("trailing newline", [good.clone(), b"\n".to_vec()].concat()),
            ("truncated", good[..good.len() - 4].to_vec()),
            ("unknown record", text.replacen("cancelled no", "canceled no", 1).into_bytes()),
            ("bad cancelled word", text.replacen("cancelled no", "cancelled maybe", 1).into_bytes()),
            ("length too long", text.replacen("tail 5", "tail 6", 1).into_bytes()),
            ("length too short", text.replacen("tail 5", "tail 4", 1).into_bytes()),
            ("leading zero length", text.replacen("tail 5", "tail 05", 1).into_bytes()),
            ("negative length", text.replacen("tail 5", "tail -5", 1).into_bytes()),
            ("missing record", text.replacen("exit_code -\n", "", 1).into_bytes()),
            ("duplicate record", text.replacen("exit_code -\n", "exit_code -\nexit_code -\n", 1).into_bytes()),
            ("out of order", text.replacen("exit_code -\ncancelled no\n", "cancelled no\nexit_code -\n", 1).into_bytes()),
            ("wrong header", text.replacen(HEADER, "orcastudio-snapshot 2", 1).into_bytes()),
            ("member count too high", text.replacen("members 2", "members 3", 1).into_bytes()),
            ("member pid 0", text.replacen("member 4243", "member 0", 1).into_bytes()),
            ("no end", text.replacen("end\n", "", 1).into_bytes()),
            ("double space", text.replacen("cancelled no", "cancelled  no", 1).into_bytes()),
        ];
        for (name, wire) in cases {
            let result = parse(&wire);
            assert!(
                matches!(result, Err(WireError::Malformed { .. })),
                "{name}: expected Malformed, got {result:?}"
            );
        }
    }

    #[test]
    fn proc_scope_must_match_rusts_own_reading_of_started() {
        // .started is from this boot, yet the collector skipped /proc: a running job would look
        // Lost.
        let skipped = Wire::default()
            .line(HEADER)
            .rec("boot_id", format!("{BOOT}\n").as_bytes())
            .rec("started", started(BOOT).as_bytes())
            .line("proc skipped")
            .line("sockets 1")
            .rec("socket", SLOT.as_bytes())
            .rec("net_unix", net_unix(false).as_bytes())
            .line("nodaemon");
        assert!(matches!(parse(&finish(skipped)), Err(WireError::Inconsistent(_))));

        // A .started from another boot: skipping is right.
        let other = "9b1d0e2f-3a4c-4d5e-8f60-718293a4b5c6";
        let stale = Wire::default()
            .line(HEADER)
            .rec("boot_id", format!("{BOOT}\n").as_bytes())
            .rec("started", started(other).as_bytes())
            .line("proc skipped")
            .line("sockets 1")
            .rec("socket", SLOT.as_bytes())
            .rec("net_unix", net_unix(false).as_bytes())
            .line("nodaemon");
        assert!(parse(&finish_with(stale, &started(other))).is_ok());

        // The reverse: /proc read for a .started from another boot — the collector's boot gate
        // failed, and its /proc facts describe some other process.
        let stat = "4242 (bash) S 1 4242 4242 0 -1 4194304 95 0 0 0 0 0 0 0 20 0 1 0 777 1 1";
        let wrong = Wire::default()
            .line(HEADER)
            .rec("boot_id", format!("{BOOT}\n").as_bytes())
            .rec("started", started(other).as_bytes())
            .line("proc collected")
            .rec("wrapper_stat", stat.as_bytes())
            .line("wrapper_cmdline -")
            .rec("sid_stat", stat.as_bytes())
            .line("members 0")
            .line("sockets 1")
            .rec("socket", SLOT.as_bytes())
            .rec("net_unix", net_unix(false).as_bytes())
            .line("nodaemon");
        assert!(matches!(parse(&finish_with(wrong, &started(other))), Err(WireError::Inconsistent(_))));
    }

    #[test]
    fn nodaemon_must_agree_with_the_net_unix_evidence() {
        // The collector says no daemon, but its own evidence lists the socket.
        let w = Wire::default()
            .line(HEADER)
            .rec("boot_id", format!("{BOOT}\n").as_bytes())
            .line("started -")
            .line("proc skipped")
            .line("sockets 1")
            .rec("socket", SLOT.as_bytes())
            .rec("net_unix", net_unix(true).as_bytes())
            .line("nodaemon");
        assert!(matches!(parse(&finish(w)), Err(WireError::Inconsistent(_))));
        // And the reverse: tsp rows for a socket nothing listens on.
        let w = Wire::default()
            .line(HEADER)
            .rec("boot_id", format!("{BOOT}\n").as_bytes())
            .line("started -")
            .line("proc skipped")
            .line("sockets 1")
            .rec("socket", SLOT.as_bytes())
            .rec("net_unix", net_unix(false).as_bytes())
            .line("rows 0");
        assert!(matches!(parse(&finish(w)), Err(WireError::Inconsistent(_))));
    }

    #[test]
    fn slot_sockets_come_first_and_only_one_extra() {
        let fact = |w: Wire, path: &str| {
            w.rec("socket", path.as_bytes())
                .rec("net_unix", net_unix(false).as_bytes())
                .line("nodaemon")
        };
        let base = || {
            Wire::default()
                .line(HEADER)
                .rec("boot_id", format!("{BOOT}\n").as_bytes())
                .line("started -")
                .line("proc skipped")
        };
        // Missing slot socket.
        let w = fact(base().line("sockets 1"), "/home/anton/.orcastudio/tsp/other.sock");
        assert!(matches!(parse(&finish(w)), Err(WireError::Inconsistent(_))));
        // Slot + one extra (the .enqueued socket): fine.
        let w = fact(fact(base().line("sockets 2"), SLOT), "/home/anton/.orcastudio/tsp/old.sock");
        assert!(parse(&finish(w)).is_ok());
        // Slot twice.
        let w = fact(fact(base().line("sockets 2"), SLOT), SLOT);
        assert!(matches!(parse(&finish(w)), Err(WireError::Inconsistent(_))));
        // An unparsable .enqueued is an Error fact.
        let w = base()
            .line("sockets 2")
            .rec("socket", SLOT.as_bytes())
            .rec("net_unix", net_unix(false).as_bytes())
            .line("nodaemon")
            .rec("socket", format!("{JOB}/.enqueued").as_bytes())
            .rec("socket_error", b"unparsable .enqueued");
        let snap = parse(&finish(w)).unwrap();
        assert!(matches!(snap.sockets[1].state, SocketState::Error(_)));
    }

    #[test]
    fn rows_must_mention_the_job_dir_and_tail_is_bounded() {
        let w = Wire::default()
            .line(HEADER)
            .rec("boot_id", format!("{BOOT}\n").as_bytes())
            .line("started -")
            .line("proc skipped")
            .line("sockets 1")
            .rec("socket", SLOT.as_bytes())
            .rec("net_unix", net_unix(true).as_bytes())
            .line("rows 1")
            .rec("row", b"4 queued (file) bash /x/bin/wrapper-ab.sh /other/job 0 /opt/orca");
        assert!(matches!(parse(&finish(w)), Err(WireError::Inconsistent(_))));

        let big = vec![b'x'; TAIL_BYTES as usize + 1];
        let w = Wire::default()
            .line(HEADER)
            .rec("boot_id", format!("{BOOT}\n").as_bytes())
            .line("started -")
            .line("proc skipped")
            .line("sockets 1")
            .rec("socket", SLOT.as_bytes())
            .rec("net_unix", net_unix(false).as_bytes())
            .line("nodaemon")
            .line("exit_code -")
            .line("cancelled no")
            .rec("tail", &big)
            .line("started -")
            .line("end");
        assert!(matches!(parse(&w.0), Err(WireError::Inconsistent(_))));
    }
}
