//! Strict parsers for the `/proc` facts in a remote-job snapshot (ADR-024 l): a process's
//! `stat` line, its `cmdline`, and `/proc/net/unix` (is a `tsp` socket path live?).
//!
//! Formats are the recorded ones (`wiki/architecture/task-spooler-uni-probe.md`, probes 5.2,
//! 5.2b, 5.2c). The probe page records `stat` and `/proc/net/unix` lines with `…` elisions; the
//! test fixtures keep every recorded field verbatim and fill the elided ones, saying so where
//! they do.

use super::FactError;

/// The fields of `/proc/<pid>/stat` the classifier uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcStat {
    /// Field 1.
    pub pid: u32,
    /// Field 3: `R`, `S`, `D`, `Z`, … A `Z` (zombie) is dead for every rule (probe 5.2c).
    pub state: char,
    /// Field 5.
    pub pgrp: u32,
    /// Field 6.
    pub session: u32,
    /// Field 22: start time in clock ticks since boot. Unchanged when the process turns zombie
    /// (probe 5.2c), so it identifies a process across its life.
    pub starttime: u64,
}

impl ProcStat {
    pub fn is_zombie(&self) -> bool {
        self.state == 'Z'
    }
}

/// Parse one raw `/proc/<pid>/stat` line (an optional trailing `\n` is allowed).
///
/// `comm` (field 2) may contain spaces and parentheses — measured with a script named
/// `w q) x.sh` — so the fields after it are taken from the text after the **last** `) `; field N
/// is then token N−2 (probe 5.2c).
pub fn parse_stat(raw: &[u8]) -> Result<ProcStat, FactError> {
    let err = |msg: String| FactError::Stat(msg);
    let line = raw.strip_suffix(b"\n").unwrap_or(raw);

    // Field 1 runs up to " (", the opening of comm.
    let open = find(line, b" (").ok_or_else(|| err("no ' (' after the pid".into()))?;
    let pid_text = ascii(&line[..open]).ok_or_else(|| err("pid is not ASCII".into()))?;
    let pid = parse_u32(pid_text, "pid").map_err(err)?;

    let close = rfind(line, b") ").ok_or_else(|| err("no ') ' after comm".into()))?;
    if close < open {
        return Err(err("comm is not closed".into()));
    }
    let rest = ascii(&line[close + 2..]).ok_or_else(|| err("fields after comm are not ASCII".into()))?;
    let tokens: Vec<&str> = rest.split(' ').collect();
    // Field 22 is token 20 (index 19).
    if tokens.len() < 20 {
        return Err(err(format!("only {} fields after comm, need at least 20", tokens.len())));
    }

    let state = match tokens[0].as_bytes() {
        [c] if c.is_ascii_alphabetic() => char::from(*c),
        _ => return Err(err(format!("state is not one letter: {:?}", tokens[0]))),
    };
    Ok(ProcStat {
        pid,
        state,
        pgrp: parse_u32(tokens[2], "pgrp").map_err(err)?,
        session: parse_u32(tokens[3], "session").map_err(err)?,
        starttime: parse_u64(tokens[19], "starttime").map_err(err)?,
    })
}

/// Split a raw `/proc/<pid>/cmdline` into argv. The kernel separates arguments with NUL and ends
/// the list with one; that final NUL is removed. An empty cmdline (a zombie's, probe 5.2c) is an
/// empty argv. Arguments stay raw bytes: comparing them is the caller's job.
pub fn split_cmdline(raw: &[u8]) -> Vec<&[u8]> {
    if raw.is_empty() {
        return Vec::new();
    }
    let body = raw.strip_suffix(b"\0").unwrap_or(raw);
    body.split(|b| *b == 0).collect()
}

/// The header line of `/proc/net/unix`, as read on the laptop (Linux 6.14, 2026-10-03).
const NET_UNIX_HEADER: [&str; 8] = ["Num", "RefCount", "Protocol", "Flags", "Type", "St", "Inode", "Path"];

/// Is `socket_path` listed in this raw `/proc/net/unix` text? A live `tsp` daemon's socket is
/// listed; a stale socket file (its daemon died) is not (probe 5.2b). Matching is by **exact**
/// path: a path that merely shares a prefix does not count.
///
/// The text must start with the known header and every other line must have the seven fixed
/// columns; otherwise this is an error, so a garbled read can never pass as "no daemon".
/// Lines without a path (unnamed sockets) are skipped. Our socket paths contain no spaces
/// (submit asserts `[A-Za-z0-9._/-]+`), so a path is the eighth column onward.
pub fn unix_socket_listed(proc_net_unix: &str, socket_path: &str) -> Result<bool, FactError> {
    let err = |msg: String| FactError::NetUnix(msg);
    let mut lines = proc_net_unix.lines();
    let header: Vec<&str> = lines
        .next()
        .ok_or_else(|| err("empty".into()))?
        .split_whitespace()
        .collect();
    if header != NET_UNIX_HEADER {
        return Err(err(format!("unexpected header {header:?}")));
    }

    let mut listed = false;
    for line in lines {
        let columns: Vec<&str> = line.split_whitespace().collect();
        if columns.len() < 7 || !columns[0].ends_with(':') {
            return Err(err(format!("malformed line {line:?}")));
        }
        if columns.len() > 7 && columns[7..].join(" ") == socket_path {
            listed = true;
        }
    }
    Ok(listed)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

fn ascii(bytes: &[u8]) -> Option<&str> {
    if bytes.is_ascii() {
        std::str::from_utf8(bytes).ok()
    } else {
        None
    }
}

fn parse_u64(text: &str, what: &str) -> Result<u64, String> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("{what} is not a decimal number: {text:?}"));
    }
    text.parse::<u64>().map_err(|e| format!("{what} {text:?}: {e}"))
}

fn parse_u32(text: &str, what: &str) -> Result<u32, String> {
    let n = parse_u64(text, what)?;
    u32::try_from(n).map_err(|_| format!("{what} {text} does not fit u32"))
}

/// Recorded fixtures, shared with the classifier's table tests.
#[cfg(test)]
pub(crate) mod fixtures {
    /// Fields 9–21 and 23–52 of a real `/proc/self/stat` read on the laptop (`cat`, Linux 6.14,
    /// 2026-10-03). Used only to fill the fields the probe page elides with `…`; the parser
    /// does not read them.
    const FILL_9_TO_21: &str = "4194304 95 0 0 0 0 0 0 0 20 0 1 0";
    const FILL_23_TO_52: &str = "11542528 493 18446744073709551615 103124286140416 103124286158001 \
140735875004016 0 0 0 0 0 0 0 0 0 17 10 0 0 0 0 0 103124286171792 103124286173288 103124949581824 \
140735875005862 140735875005882 140735875005882 140735875010539 0";

    /// Probe 5.2c, a script named `w q) x.sh`. Recorded:
    /// `66186 (w q) x.sh) R 66182 66186 66182 0 -1 … 1890507 …` — fields 1–8 and 22 verbatim.
    pub fn stat_w_q_x() -> String {
        format!("66186 (w q) x.sh) R 66182 66186 66182 0 -1 {FILL_9_TO_21} 1890507 {FILL_23_TO_52}\n")
    }

    /// Probe 5.2c, a live bash. Recorded: `66401 (bash) S … 1891976 13090816 991 …` — fields
    /// 1–3, 22–24 verbatim. Fields 4–8 are elided in the record; filled here with ppid 66400 and
    /// pgrp = session = 66401 (the wrapper's PID = PGID = SID shape, probe P2), tty 0, tpgid -1.
    pub fn stat_bash_live() -> String {
        stat_bash('S', "13090816 991")
    }

    /// Probe 5.2c, the same bash as a zombie. Recorded: `66401 (bash) Z … 1891976 0 0 …` —
    /// field 22 unchanged, vsize/rss 0. Elided fields filled as in [`stat_bash_live`].
    pub fn stat_bash_zombie() -> String {
        stat_bash('Z', "0 0")
    }

    fn stat_bash(state: char, fields_23_24: &str) -> String {
        let after_24 = FILL_23_TO_52.splitn(3, ' ').nth(2).unwrap_or_default();
        format!(
            "66401 (bash) {state} 66400 66401 66401 0 -1 {FILL_9_TO_21} 1891976 {fields_23_24} {after_24}\n"
        )
    }

    /// Probe P2, the tsp-launched wrapper's cmdline, verbatim (`|` in the record = NUL).
    pub const CMDLINE_P2: &[u8] =
        b"bash\0/home/anton/.orcastudio/probe-5.2/bin/wrapper.sh\0/home/anton/.orcastudio/probe-5.2/jobs/j1\00-3\0/opt/orca\0";

    /// Probe P2, the wrapper's `taskset`-exec'd child: `sleep|60|`.
    pub const CMDLINE_P2_CHILD: &[u8] = b"sleep\x0060\0";

    /// `/proc/net/unix` with the live-socket line of probe 5.2b. Recorded:
    /// `…: 00000002 00000000 00010000 0001 01 2960949 …/live.sock` — columns 2–7 verbatim. The
    /// `Num` column and the path's directory are elided in the record; filled with the laptop's
    /// `Num` shape and the probe's root `/home/anton/.orcastudio/probe-5.2b`. The header and the
    /// other two lines (an unnamed socket, a padded inode) are the laptop's own read, 2026-10-03.
    pub const NET_UNIX_LIVE: &str = "\
Num       RefCount Protocol Flags    Type St Inode Path
0000000000000000: 00000002 00000000 00010000 0001 01 2960949 /home/anton/.orcastudio/probe-5.2b/live.sock
0000000000000000: 00000003 00000000 00000000 0001 03  3938 /run/user/1000/bus
0000000000000000: 00000003 00000000 00000000 0001 03 21848
";

    pub const LIVE_SOCKET: &str = "/home/anton/.orcastudio/probe-5.2b/live.sock";
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    #[test]
    fn stat_takes_fields_after_the_last_paren() {
        let s = parse_stat(stat_w_q_x().as_bytes()).unwrap();
        // The values the probe's shell parse returned: state=R pgrp=66186 session=66182
        // starttime=1890507.
        assert_eq!(
            s,
            ProcStat { pid: 66186, state: 'R', pgrp: 66186, session: 66182, starttime: 1890507 }
        );
    }

    #[test]
    fn stat_zombie_keeps_its_starttime() {
        let live = parse_stat(stat_bash_live().as_bytes()).unwrap();
        let zombie = parse_stat(stat_bash_zombie().as_bytes()).unwrap();
        assert_eq!(live.state, 'S');
        assert!(!live.is_zombie());
        assert!(zombie.is_zombie());
        assert_eq!(live.starttime, 1891976);
        assert_eq!(zombie.starttime, live.starttime);
    }

    #[test]
    fn stat_rejects_garbage() {
        let good = stat_w_q_x();
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty", Vec::new()),
            ("no comm", b"66186 R 1 2 3".to_vec()),
            ("non-numeric pid", good.replacen("66186", "x", 1).into_bytes()),
            ("truncated before field 22", good[..60].as_bytes().to_vec()),
            ("two-letter state", good.replace(") R ", ") RR ").into_bytes()),
            ("non-numeric starttime", good.replace("1890507", "18905o7").into_bytes()),
            ("non-numeric session", good.replace("66186 66182 0", "66186 x 0").into_bytes()),
            ("double space", good.replace(" 1890507", "  1890507").into_bytes()),
        ];
        for (name, raw) in cases {
            assert!(parse_stat(&raw).is_err(), "{name} must not parse");
        }
    }

    #[test]
    fn cmdline_splits_on_nul() {
        let argv = split_cmdline(CMDLINE_P2);
        assert_eq!(
            argv,
            vec![
                &b"bash"[..],
                b"/home/anton/.orcastudio/probe-5.2/bin/wrapper.sh",
                b"/home/anton/.orcastudio/probe-5.2/jobs/j1",
                b"0-3",
                b"/opt/orca",
            ]
        );
        assert_eq!(split_cmdline(CMDLINE_P2_CHILD), vec![&b"sleep"[..], b"60"]);
        assert!(split_cmdline(b"").is_empty(), "a zombie's cmdline is empty");
    }

    #[test]
    fn net_unix_matches_the_exact_path_only() {
        assert!(unix_socket_listed(NET_UNIX_LIVE, LIVE_SOCKET).unwrap());
        // A stale socket is absent from /proc/net/unix (probe 5.2b).
        assert!(!unix_socket_listed(NET_UNIX_LIVE, "/home/anton/.orcastudio/probe-5.2b/stale.sock").unwrap());
        // Prefix and suffix relatives of the live path are not the live path.
        assert!(!unix_socket_listed(NET_UNIX_LIVE, "/home/anton/.orcastudio/probe-5.2b/live").unwrap());
        assert!(!unix_socket_listed(NET_UNIX_LIVE, "/home/anton/.orcastudio/probe-5.2b/live.sock2").unwrap());
    }

    #[test]
    fn net_unix_rejects_garbage() {
        assert!(unix_socket_listed("", LIVE_SOCKET).is_err());
        assert!(unix_socket_listed("garbage\n", LIVE_SOCKET).is_err());
        let truncated = NET_UNIX_LIVE.replace("00010000 0001 01 2960949 ", "");
        assert!(unix_socket_listed(&truncated, LIVE_SOCKET).is_err());
        // Header alone: a valid, empty table.
        let header_only = NET_UNIX_LIVE.lines().next().unwrap_or_default();
        assert!(!unix_socket_listed(header_only, LIVE_SOCKET).unwrap());
    }
}
