//! Spawn `orca_2json` to convert a job's `.gbw` into JSON — **process
//! orchestration, so Rust owns it** (ADR-009), kept separate from the pure
//! `parse::mo` reader that consumes the result.
//!
//! Generation is **lazy and cached** (overview.md: heavy artifacts on demand): the
//! JSON is written into the **job directory** — the one place we may write (rule
//! #3) — and regenerated only when it is missing or older than the `.gbw`. Absence
//! is normal: an xTB/GOAT `.gbw` yields no JSON (measured), which is `Ok(None)`,
//! not an error.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::AppError;

/// Ensure `input.json` exists in `job_dir` for its `input.gbw`, generating it with
/// `orca_2json` if needed. Returns the path, or `None` when there is no gbw / no
/// converter / the converter produced nothing (an xTB gbw — normal).
///
/// `orca_binary_path` is the user-configured ORCA path (a **setting**, not
/// hard-coded — rule #7); `orca_2json` and the shared libs live beside it.
pub fn ensure_gbw_json(orca_binary_path: &str, job_dir: &Path) -> Result<Option<PathBuf>, AppError> {
    let gbw = job_dir.join("input.gbw");
    if !gbw.exists() {
        return Ok(None);
    }
    let json = job_dir.join("input.json");

    // Lazy cache: reuse a JSON at least as new as the gbw.
    if is_fresh(&json, &gbw) {
        return Ok(Some(json));
    }

    let orca_dir = match Path::new(orca_binary_path).parent() {
        Some(d) => d,
        None => return Ok(None),
    };
    let converter = orca_dir.join("orca_2json");
    if !converter.exists() {
        return Ok(None);
    }

    // Measured requirements: the filename WITH the `.gbw` extension, and ORCA's
    // shared libs on LD_LIBRARY_PATH. cwd = job_dir so the output lands there.
    let output = Command::new(&converter)
        .arg("input.gbw")
        .current_dir(job_dir)
        .env("LD_LIBRARY_PATH", orca_dir)
        .output()
        .map_err(|e| AppError::Backend(format!("spawning orca_2json: {e}")))?;

    // A non-zero exit or a missing file is a normal state (e.g. an xTB gbw the
    // converter cannot read), not a panic.
    if output.status.success() && json.exists() {
        Ok(Some(json))
    } else {
        Ok(None)
    }
}

/// `json` exists and is not older than `gbw`.
fn is_fresh(json: &Path, gbw: &Path) -> bool {
    let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    match (mtime(json), mtime(gbw)) {
        (Some(j), Some(g)) => j >= g,
        _ => false,
    }
}
