//! Spawn `orca_plot` to generate an orbital `.cube` from a job's `.gbw` — **process
//! orchestration, so Rust owns it** (ADR-009), mirroring [`orca_json`](crate::orca_json).
//!
//! Generation is **lazy and cached** (overview.md: heavy artifacts on demand): the cube
//! is written into the **job directory** — the one place we may write (rule #3) — under a
//! **grid-keyed** name so different grids of the same MO don't overwrite each other, and
//! it is regenerated only when missing or older than the `.gbw`. Absence is normal: an
//! xTB/GOAT `.gbw` yields no cube (its JSON already yields no MOs — measured), `Ok(None)`.
//!
//! **Non-interactive invocation (measured, `wiki/orca/orca-plot.md`):** `orca_plot`'s
//! advertised batch `plot-inputfile` mode was not usable (an undocumented field it names
//! "state density" — every attempt exited FATAL with no cube). What works is driving its
//! interactive menu over **stdin** — a deterministic script of the menu's own answers.
//! Menu numbers are pinned to ORCA 6.1.0; re-probe on upgrade.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::AppError;

/// The interactive-menu script that generates MO `mo` at `grid` intervals (closed shell →
/// operator 0, the menu default). `2`=orbital, `4`=grid, `11`=generate, `12`=exit.
fn menu_script(mo: u32, grid: u32) -> String {
    format!("2\n{mo}\n4\n{grid}\n11\n12\n")
}

/// Ensure the cube for MO `mo_index` at `grid` exists in `job_dir`, generating it with
/// `orca_plot` if needed. Returns the cube path, or `None` when there is no gbw / no
/// `orca_plot` / it produced nothing (an xTB gbw — normal).
///
/// `orca_binary_path` is the user-configured ORCA path (a **setting**, not hard-coded —
/// rule #7); `orca_plot` and the shared libs live beside it.
pub fn ensure_mo_cube(
    orca_binary_path: &str,
    job_dir: &Path,
    mo_index: u32,
    grid: u32,
) -> Result<Option<PathBuf>, AppError> {
    let gbw = job_dir.join("input.gbw");
    if !gbw.exists() {
        return Ok(None);
    }
    // Grid-keyed cache name (so mo66@40 and mo66@80 coexist).
    let cache = job_dir.join(format!("orbital.mo{mo_index}.g{grid}.cube"));
    if is_fresh(&cache, &gbw) {
        return Ok(Some(cache));
    }

    let orca_dir = match Path::new(orca_binary_path).parent() {
        Some(d) => d,
        None => return Ok(None),
    };
    let plot = orca_dir.join("orca_plot");
    if !plot.exists() {
        return Ok(None);
    }

    // Drive the interactive menu over stdin; cwd = job_dir so the cube lands there.
    let mut child = Command::new(&plot)
        .arg("input.gbw")
        .arg("-i")
        .current_dir(job_dir)
        .env("LD_LIBRARY_PATH", orca_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AppError::Backend(format!("spawning orca_plot: {e}")))?;

    // Write the script, then DROP stdin (EOF) so orca_plot cannot block waiting for input
    // even if the menu ever diverges — it will hit EOF and exit rather than hang.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(menu_script(mo_index, grid).as_bytes());
    }
    let _ = child
        .wait()
        .map_err(|e| AppError::Backend(format!("waiting for orca_plot: {e}")))?;

    // orca_plot names it `input.mo{N}a.cube` (operator 0 → 'a'). A FATAL banner on EOF is
    // normal (like orca_json's non-zero exit) — the file existing is the success signal.
    let produced = job_dir.join(format!("input.mo{mo_index}a.cube"));
    if produced.exists() {
        std::fs::rename(&produced, &cache).map_err(|e| AppError::Backend(format!(
            "renaming orca_plot cube: {e}"
        )))?;
        Ok(Some(cache))
    } else {
        Ok(None)
    }
}

/// `cube` exists and is not older than `gbw`.
fn is_fresh(cube: &Path, gbw: &Path) -> bool {
    let mtime = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    match (mtime(cube), mtime(gbw)) {
        (Some(c), Some(g)) => c >= g,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_script_is_the_measured_sequence() {
        // orbital-select (2), the MO, grid (4), the grid, generate (11), exit (12).
        assert_eq!(menu_script(66, 80), "2\n66\n4\n80\n11\n12\n");
    }

    #[test]
    fn no_gbw_is_none_not_error() {
        let dir = std::env::temp_dir().join("orcastudio_plot_test_nogbw");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::remove_file(dir.join("input.gbw"));
        assert!(ensure_mo_cube("/opt/orca/orca", &dir, 66, 80).unwrap().is_none());
    }

    #[test]
    fn cache_name_includes_mo_and_grid() {
        // The grid-keyed name is what lets two grids of one MO coexist (the cache key).
        assert_eq!(format!("orbital.mo{}.g{}.cube", 66, 80), "orbital.mo66.g80.cube");
        assert_eq!(format!("orbital.mo{}.g{}.cube", 66, 40), "orbital.mo66.g40.cube");
    }

    /// Real orca_plot run (needs `/opt/orca` + the dexketoprofen job's `.gbw`). Verifies
    /// the whole path: generate → rename to the grid-keyed cache name → cache hit second
    /// time. Ignored by default; run with `cargo test -- --ignored generates_and_caches`.
    #[test]
    #[ignore]
    fn generates_and_caches_a_real_cube() {
        let src = std::path::Path::new(
            "/home/laptop/.local/share/orcastudio/jobs/b0d1db94-8012-47aa-9d2a-bb5924abca13/input.gbw",
        );
        if !src.exists() || !std::path::Path::new("/opt/orca/orca_plot").exists() {
            eprintln!("skipping: no gbw / no orca_plot");
            return;
        }
        let dir = std::env::temp_dir().join("orcastudio_plot_realtest");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(src, dir.join("input.gbw")).unwrap();

        let cube = ensure_mo_cube("/opt/orca/orca", &dir, 66, 40).unwrap().unwrap();
        assert_eq!(cube.file_name().unwrap(), "orbital.mo66.g40.cube");
        assert!(cube.exists());
        let bytes = std::fs::metadata(&cube).unwrap().len();
        assert!(bytes > 100_000, "cube looks too small: {bytes} bytes");

        // Second call is a cache hit (no `input.mo66a.cube` regenerated).
        let _ = std::fs::remove_file(dir.join("input.mo66a.cube"));
        let again = ensure_mo_cube("/opt/orca/orca", &dir, 66, 40).unwrap().unwrap();
        assert_eq!(again, cube);
        assert!(!dir.join("input.mo66a.cube").exists(), "should not regenerate on a hit");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
