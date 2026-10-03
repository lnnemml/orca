//! Strict parsers for the job-dir markers the wrapper writes (ADR-024 b, l) and for a host
//! `boot_id`.
//!
//! ## The `.started` format
//!
//! The wrapper writes `.started` (temp file + `rename`) as exactly six `key=value` lines, in any
//! order, each terminated by `\n`:
//!
//! ```text
//! pid=376681
//! pgid=376681
//! sid=376681
//! boot_id=0f6e3c1a-5b7d-4c2e-9a8b-1d2e3f4a5b6c
//! starttime=319395485
//! started_at=1759490000
//! ```
//!
//! - `pid`, `pgid`, `sid` — the wrapper's own ids (it is `PID = PGID = SID` under tsp, probe P2),
//!   positive integers;
//! - `boot_id` — `/proc/sys/kernel/random/boot_id` at start: a lowercase 8-4-4-4-12 hex UUID;
//! - `starttime` — field 22 of the wrapper's own `/proc/$$/stat` (clock ticks since boot),
//!   read with the builtin `read -r l </proc/$$/stat` (probe 5.2c);
//! - `started_at` — Unix seconds (UTC) at start. Informational only: the server clock is not
//!   NTP-synchronised (probe side findings), so no rule compares it with the laptop's clock.
//!
//! Every key is required, exactly once. An unknown key, a duplicate, a missing key, a
//! non-numeric value, a blank line, a `\r`, or spaces around `=` make the whole file a parse
//! error — which the classifier reads as precedence row 1 (`Failed`, "corrupt `.started`").

use super::FactError;

/// The parsed `.started` marker. See the module docs for the on-disk format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Started {
    pub pid: u32,
    pub pgid: u32,
    pub sid: u32,
    pub boot_id: BootId,
    /// `/proc/<pid>/stat` field 22 of the wrapper, in clock ticks since boot.
    pub starttime: u64,
    /// Unix seconds; informational (see the module docs).
    pub started_at: u64,
}

/// A host boot id: `/proc/sys/kernel/random/boot_id`, a fresh UUID every boot. Held only in its
/// validated lowercase form, so two boot ids compare equal iff they are the same boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootId(String);

impl BootId {
    /// Parse a boot id with at most one trailing `\n` (the kernel file ends with one).
    pub fn parse(raw: &str) -> Result<Self, FactError> {
        let text = raw.strip_suffix('\n').unwrap_or(raw);
        if is_lowercase_uuid(text) {
            Ok(BootId(text.to_string()))
        } else {
            Err(FactError::BootId(format!("not a lowercase UUID: {text:?}")))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`, lowercase hex.
fn is_lowercase_uuid(s: &str) -> bool {
    let groups: Vec<&str> = s.split('-').collect();
    let expected_lengths = [8, 4, 4, 4, 12];
    groups.len() == expected_lengths.len()
        && groups.iter().zip(expected_lengths).all(|(group, len)| {
            group.len() == len
                && group
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

const STARTED_KEYS: [&str; 6] = ["pid", "pgid", "sid", "boot_id", "starttime", "started_at"];

/// Parse the raw bytes of `.started` strictly (module docs). Any deviation is an error.
pub fn parse_started(raw: &[u8]) -> Result<Started, FactError> {
    let err = |msg: String| FactError::Started(msg);
    let text = std::str::from_utf8(raw).map_err(|_| err("not UTF-8".into()))?;
    if text.is_empty() {
        // A disk-full `rename` can publish an empty file (ADR-024 l, row 1).
        return Err(err("empty file".into()));
    }
    let body = text
        .strip_suffix('\n')
        .ok_or_else(|| err("last line is not newline-terminated".into()))?;

    let mut values: [Option<&str>; 6] = [None; 6];
    for line in body.split('\n') {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| err(format!("line without '=': {line:?}")))?;
        let slot = STARTED_KEYS
            .iter()
            .position(|k| *k == key)
            .ok_or_else(|| err(format!("unknown key {key:?}")))?;
        if values[slot].is_some() {
            return Err(err(format!("duplicate key {key:?}")));
        }
        values[slot] = Some(value);
    }
    let get = |i: usize| values[i].ok_or_else(|| err(format!("missing key {:?}", STARTED_KEYS[i])));

    Ok(Started {
        pid: parse_positive_id(get(0)?, "pid")?,
        pgid: parse_positive_id(get(1)?, "pgid")?,
        sid: parse_positive_id(get(2)?, "sid")?,
        // Lines are split on '\n', so the value never carries the newline `BootId::parse`
        // tolerates for the kernel file.
        boot_id: BootId::parse(get(3)?).map_err(|e| err(e.to_string()))?,
        starttime: parse_decimal(get(4)?, "starttime")?,
        started_at: parse_decimal(get(5)?, "started_at")?,
    })
}

/// A decimal `u64`: ASCII digits only — no sign, no spaces, no empty value.
fn parse_decimal(value: &str, key: &str) -> Result<u64, FactError> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(FactError::Started(format!("{key}={value:?} is not a decimal number")));
    }
    value
        .parse::<u64>()
        .map_err(|e| FactError::Started(format!("{key}={value:?}: {e}")))
}

/// A process / group / session id: a decimal that fits `u32` and is not 0.
fn parse_positive_id(value: &str, key: &str) -> Result<u32, FactError> {
    let n = parse_decimal(value, key)?;
    match u32::try_from(n) {
        Ok(id) if id > 0 => Ok(id),
        _ => Err(FactError::Started(format!("{key}={value} is not a valid process id"))),
    }
}

/// Parse the raw bytes of `.exit_code`: the wrapper's `$?`, a decimal 0–255 with an optional
/// single trailing `\n`. Leading zeros, signs, spaces, an empty file or anything else are an
/// error — which the classifier reads as row 6 (`Failed`, bad exit code).
pub fn parse_exit_code(raw: &[u8]) -> Result<u8, FactError> {
    let err = |msg: String| FactError::ExitCode(msg);
    let text = std::str::from_utf8(raw).map_err(|_| err("not UTF-8".into()))?;
    let digits = text.strip_suffix('\n').unwrap_or(text);
    if digits.is_empty() {
        return Err(err("empty".into()));
    }
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(err(format!("not a decimal exit code: {digits:?}")));
    }
    if digits.len() > 1 && digits.starts_with('0') {
        return Err(err(format!("leading zero: {digits:?}")));
    }
    digits
        .parse::<u8>()
        .map_err(|_| err(format!("out of range 0-255: {digits:?}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOT: &str = "0f6e3c1a-5b7d-4c2e-9a8b-1d2e3f4a5b6c";

    // pid/pgid/sid are the recorded wrapper PID from probe P2 (`ps` row 376681, PID = PGID =
    // SID); starttime is the value recorded under tsp on uni (probe 5.2c). The boot id and
    // started_at are synthetic: the probe pages do not record a boot id value.
    fn good() -> String {
        format!(
            "pid=376681\npgid=376681\nsid=376681\nboot_id={BOOT}\nstarttime=319395485\nstarted_at=1759490000\n"
        )
    }

    #[test]
    fn started_parses_all_six_fields() {
        let s = parse_started(good().as_bytes()).unwrap();
        assert_eq!(s.pid, 376681);
        assert_eq!(s.pgid, 376681);
        assert_eq!(s.sid, 376681);
        assert_eq!(s.boot_id.as_str(), BOOT);
        assert_eq!(s.starttime, 319395485);
        assert_eq!(s.started_at, 1759490000);
    }

    #[test]
    fn started_key_order_is_free() {
        let text = format!(
            "started_at=1\nstarttime=2\nboot_id={BOOT}\nsid=3\npgid=4\npid=5\n"
        );
        let s = parse_started(text.as_bytes()).unwrap();
        assert_eq!((s.pid, s.pgid, s.sid, s.starttime, s.started_at), (5, 4, 3, 2, 1));
    }

    #[test]
    fn started_rejects_garbage() {
        let g = good();
        let cases: Vec<(&str, String)> = vec![
            ("empty", String::new()),
            ("no trailing newline", g.trim_end().to_string()),
            ("missing key", g.replace("sid=376681\n", "")),
            ("unknown key", format!("{g}extra=1\n")),
            ("duplicate key", format!("{g}pid=376681\n")),
            ("non-numeric", g.replace("pid=376681", "pid=abc")),
            ("negative", g.replace("pid=376681", "pid=-1")),
            ("zero pid", g.replace("pid=376681", "pid=0")),
            ("empty value", g.replace("starttime=319395485", "starttime=")),
            ("space around =", g.replace("pid=376681", "pid = 376681")),
            ("blank line", g.replace("\nsid", "\n\nsid")),
            ("CRLF", g.replace('\n', "\r\n")),
            ("uppercase boot id", g.replace(BOOT, &BOOT.to_uppercase())),
            ("short boot id", g.replace(BOOT, "0f6e3c1a")),
            ("pid overflows u32", g.replace("pid=376681", "pid=4294967296")),
            ("half-written", g[..20].to_string()),
        ];
        for (name, text) in cases {
            assert!(
                parse_started(text.as_bytes()).is_err(),
                "{name} must not parse: {text:?}"
            );
        }
        assert!(parse_started(&[0xff, 0xfe]).is_err(), "non-UTF-8 must not parse");
    }

    #[test]
    fn boot_id_allows_one_trailing_newline_only() {
        assert_eq!(BootId::parse(&format!("{BOOT}\n")).unwrap().as_str(), BOOT);
        assert_eq!(BootId::parse(BOOT).unwrap().as_str(), BOOT);
        assert!(BootId::parse(&format!("{BOOT}\n\n")).is_err());
        assert!(BootId::parse("").is_err());
        assert!(BootId::parse(&format!(" {BOOT}")).is_err());
    }

    #[test]
    fn exit_code_parses_decimal_with_optional_newline() {
        assert_eq!(parse_exit_code(b"0\n").unwrap(), 0);
        assert_eq!(parse_exit_code(b"0").unwrap(), 0);
        assert_eq!(parse_exit_code(b"97\n").unwrap(), 97);
        assert_eq!(parse_exit_code(b"255").unwrap(), 255);
    }

    #[test]
    fn exit_code_rejects_garbage() {
        for raw in [
            &b""[..],
            b"\n",
            b"abc",
            b"-1",
            b"+0",
            b" 0",
            b"0 \n",
            b"00",
            b"01",
            b"256",
            b"0\n\n",
            b"0\r\n",
            b"\xff",
        ] {
            assert!(parse_exit_code(raw).is_err(), "{raw:?} must not parse");
        }
    }
}
