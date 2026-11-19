//! The source job's charge and multiplicity (ADR-027 decision 4; clarification C1): a **strict**
//! Rust port of the locator in `sceneFromOrcaInput` (`src/scene/scene.ts`).
//!
//! Both scan for the first line starting with `*` whose keyword is `xyz` and read `<charge>
//! <multiplicity>` from it. They agree on every well-formed input (pinned by the shared cases file
//! `src-tauri/tests/fixtures/mcp/charge_mult_cases.json`, read by
//! `charge_mult_tests::well_formed_cases_match_the_ts_locator` and by
//! `src/scene/charge-mult-parity.test.ts`). They **deliberately diverge** on a malformed one: the
//! TS locator falls back to (0, 1) or returns null, which suits the human path that shows the
//! values before a run; this one returns an error, which the classifier turns into REFUSED (R4),
//! because nobody reads the values before a SUBMITTED job runs.
//!
//! Strict means: `#` comments are masked first; the line must be exactly `* xyz <int> <int>`;
//! `* xyzfile`, `* int`, `* gzmt`, a missing line, a non-integer, a third value or a value hidden
//! by a comment are all errors. So is any line it reads, up to and including the header, that holds
//! two or more `#` (the manual's second-`#` comment form is unmeasured; `header::UNMEASURED_COMMENT`).

use serde::Serialize;
use thiserror::Error;

use super::header::has_unmeasured_comment;

/// A charge and spin multiplicity, as integers (both dimensionless).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ChargeMult {
    pub charge: i64,
    pub multiplicity: i64,
}

/// Why the charge and multiplicity cannot be read.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
pub enum ChargeMultError {
    #[error("no `* xyz <charge> <multiplicity>` line")]
    NoXyzLine,
    #[error("line {line}: the geometry is `* {form}`, not an inline `* xyz` block")]
    NotInlineXyz { line: usize, form: String },
    #[error("line {line}: two or more `#` on one line, whose reading is unmeasured")]
    UnmeasuredComment { line: usize },
    #[error("line {line}: `{text}` is not `* xyz <charge> <multiplicity>` with two integers")]
    Malformed { line: usize, text: String },
}

/// A strict integer: an optional sign, then ASCII digits only (`1.0` and `1e0` are refused).
fn strict_int(s: &str) -> Option<i64> {
    let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// A line with its `#` comment removed.
fn mask_comment(line: &str) -> &str {
    line.find('#').map_or(line, |i| &line[..i])
}

/// The index (0-based line number) and value of the first `* xyz` header line of `input`.
pub(crate) fn locate_xyz_header(input: &str) -> Result<(usize, ChargeMult), ChargeMultError> {
    for (i, raw) in input.lines().enumerate() {
        // Every line the locator reads, up to and including the header: never past a second `#`.
        if has_unmeasured_comment(raw) {
            return Err(ChargeMultError::UnmeasuredComment { line: i + 1 });
        }
        let t = mask_comment(raw).trim();
        let Some(rest) = t.strip_prefix('*') else { continue };
        let rest = rest.trim();
        if rest.is_empty() {
            continue; // a closing `*`, as the TS locator skips it
        }
        let tokens: Vec<&str> = rest.split_whitespace().collect();
        let form = tokens[0].to_ascii_lowercase();
        if form != "xyz" {
            return Err(ChargeMultError::NotInlineXyz { line: i + 1, form });
        }
        return match tokens.as_slice() {
            [_, c, m] => match (strict_int(c), strict_int(m)) {
                (Some(charge), Some(multiplicity)) => Ok((i, ChargeMult { charge, multiplicity })),
                _ => Err(ChargeMultError::Malformed { line: i + 1, text: raw.trim().to_string() }),
            },
            _ => Err(ChargeMultError::Malformed { line: i + 1, text: raw.trim().to_string() }),
        };
    }
    Err(ChargeMultError::NoXyzLine)
}

/// The charge and multiplicity on a source input's `* xyz` line (C1).
pub fn locate_charge_mult(input: &str) -> Result<ChargeMult, ChargeMultError> {
    locate_xyz_header(input).map(|(_, cm)| cm)
}
