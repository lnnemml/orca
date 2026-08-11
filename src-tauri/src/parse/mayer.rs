//! Reader for ORCA's **Mayer bond-order** table — the seventh artifact reader
//! (ADR-012). Unlike the six structured-artifact readers, its source is the
//! **unbounded `output.out` log**, so it is **streamed** (domain rule #5): read
//! line-by-line, only ONE candidate block ever held in memory.
//!
//! ORCA prints, near the end of an SCF, a block:
//!
//! ```text
//!   Mayer bond orders larger than 0.100000
//! B(  0-C ,  1-N ) :   0.8996 B(  0-C ,  2-H ) :   0.9674 B(  0-C ,  3-H ) :   0.9527
//! B(  1-N ,  8-C ) :   0.1030 B(  8-C ,  9-I ) :   0.6767 ...
//! ```
//!
//! `B( i-El , j-El ) : order` — several entries per line, indices **0-based**. This
//! is the **computed, authoritative** bond order (contrast the editor's *geometric*
//! estimate from bond length). We keep the **LAST** block in the file: for an Opt/
//! OptTS that is the final (converged) structure; a run with no such block (xTB, an
//! SP that didn't print it) yields `None` — **absent is normal**, not an error.
//!
//! # Post-condition (rule #9)
//! Every parsed pair is checked against the structure's atom count: an index ≥
//! `natoms`, or a non-positive order, is a **loud** [`ParseError::Malformed`] — never
//! a silently-kept bad pair. The reader recomputes nothing else (it has no geometry),
//! so this bounds check is its entire in-our-terms guard.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::ParseError;

/// One computed Mayer bond order between two atoms (0-based indices, file order).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MayerBond {
    pub i: usize,
    pub j: usize,
    /// The Mayer bond order — a fractional, authoritative value (e.g. a partial TS
    /// bond ≈0.68), not a geometric estimate.
    pub order: f64,
}

/// The block header ORCA prints (the threshold value varies, so we match the stem).
const HEADER: &str = "Mayer bond orders larger than";

/// One `B( i-El , j-El ) : order` entry. Element symbols are matched but not
/// captured — the indices are the identity (0-based), the order is the value.
static ENTRY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"B\(\s*(\d+)-\s*[A-Za-z]+\s*,\s*(\d+)-\s*[A-Za-z]+\s*\)\s*:\s*(-?[0-9]*\.?[0-9]+)")
        .unwrap()
});

/// Parse the data lines of ONE Mayer block into bonds. Pure. `natoms` bounds-checks
/// the indices (rule #9): an index ≥ `natoms` or a non-positive order is a loud
/// error, not a silently-kept bad pair. Element symbols in the table are matched but
/// not used — the 0-based indices are the atom identity.
pub fn parse_mayer_lines(lines: &[&str], natoms: usize) -> Result<Vec<MayerBond>, ParseError> {
    let mut bonds = Vec::new();
    for line in lines {
        for cap in ENTRY_RE.captures_iter(line) {
            // The regex guarantees the three groups are numeric; a parse failure here
            // would be an internal contradiction, surfaced (never silently dropped).
            let parse_field = |g: usize| -> Result<f64, ParseError> {
                cap[g].parse::<f64>().map_err(|_| ParseError::Malformed {
                    field: "Mayer bond orders".into(),
                    detail: format!("un-parseable number in entry: {}", &cap[0]),
                })
            };
            let i = parse_field(1)? as usize;
            let j = parse_field(2)? as usize;
            let order = parse_field(3)?;
            if i >= natoms || j >= natoms {
                return Err(ParseError::Malformed {
                    field: "Mayer bond orders".into(),
                    detail: format!(
                        "atom index out of range: B({i}, {j}) but the structure has {natoms} atoms"
                    ),
                });
            }
            if !(order > 0.0) {
                return Err(ParseError::Malformed {
                    field: "Mayer bond orders".into(),
                    detail: format!("non-positive bond order {order} for B({i}, {j})"),
                });
            }
            bonds.push(MayerBond { i, j, order });
        }
    }
    Ok(bonds)
}

/// Stream `output.out` and return the **LAST** Mayer block's bonds (the final
/// structure), or `None` when the file has no Mayer table (xTB / an SP that didn't
/// print it — absent-is-normal). Rule #5: line-streamed, only one block buffered.
/// A file that cannot be opened is treated as absent (`Ok(None)`), matching the
/// other optional readers — a missing artifact is not a parse failure.
pub fn read_mayer(output_path: &Path, natoms: usize) -> Result<Option<Vec<MayerBond>>, ParseError> {
    let Ok(file) = File::open(output_path) else {
        return Ok(None);
    };

    // `cur` collects the block currently under a header; `last` holds the most recent
    // COMPLETED block. The last completed block wins (the final structure).
    let mut last: Option<Vec<String>> = None;
    let mut cur: Option<Vec<String>> = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if line.contains(HEADER) {
            // A new block starts — finalize any block still open, begin a fresh one.
            if let Some(block) = cur.take() {
                last = Some(block);
            }
            cur = Some(Vec::new());
            continue;
        }
        if let Some(buf) = cur.as_mut() {
            if line.trim_start().starts_with("B(") {
                buf.push(line);
            } else {
                // A non-entry line ends the block (blank line / the TIMINGS banner).
                last = cur.take();
            }
        }
    }
    if let Some(block) = cur.take() {
        last = Some(block);
    }

    match last {
        None => Ok(None),
        Some(block) => {
            let refs: Vec<&str> = block.iter().map(String::as_str).collect();
            Ok(Some(parse_mayer_lines(&refs, natoms)?))
        }
    }
}

#[cfg(test)]
mod tests;
