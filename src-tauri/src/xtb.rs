//! Standalone **xtb** pre-optimization (Phase 2.5.5) — GFN2-xTB relaxation of a
//! scene while holding the user's constraints, so the geometry handed to ORCA is
//! already sensible. This is NOT xtb-via-ORCA (`! XTB`, the GOAT path); it shells
//! out to the standalone `xtb` binary, which is why it lives in Rust:
//!
//!  - Rust owns process spawning, and with it the isolated-directory rule
//!    (domain rule #3) and the kill-the-whole-group discipline (`debugging/004`);
//!  - the binary path is a **setting**, and settings live in SQLite under Rust;
//!  - the sidecar is deliberately ignorant of the jobs dir and of settings.
//!
//! ## Index base — xtb `$constrain` is **1-based** (NOT the same as ORCA!)
//! Settled by a real xtb 6.6.1 run (2026-07-29), NOT memory. ORCA `%geom
//! Constraints` is **0-based**; xtb `$constrain` is **1-based**. OrcaStudio stores
//! constraints 0-based (ADR-008), so every index written here is `+1`. Getting
//! this wrong freezes the WRONG coordinate on an optimization that finishes
//! cleanly — the post-condition below is the runtime guard against exactly that.
//! See `wiki/orca/xtb.md` and `wiki/orca/gotchas.md`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::local_backend::terminate_job;

/// GFN2 harmonic constraint stiffness. On a realistic reaction-coordinate hold
/// (ibuprofen + BH₄⁻, C···B at 2.2 Å) this held the target to **0.011 Å**
/// (5b run). Higher stiffens the hold at the cost of distorting the local PES.
const FORCE_CONSTANT: f64 = 1.0;

/// Post-condition tolerances. **Distance is measured** (realistic hold 0.011 Å at
/// `FORCE_CONSTANT=1.0`); 0.1 Å is a 10× margin that still catches a gross
/// non-hold OR an index-base mistake (which would let the intended pair drift far
/// more than 0.1 Å). Angle/dihedral springs were not separately measured, so their
/// tolerances are deliberately generous; a `$fix` (Cartesian) is a hard constraint,
/// near-exact.
const TOL_DISTANCE_ANG: f64 = 0.1;
const TOL_ANGLE_DEG: f64 = 5.0;
const TOL_CARTESIAN_ANG: f64 = 0.01;

const DEFAULT_TIMEOUT_SECS: u64 = 300;

// ── Constraints (mirror the TS `Constraint`, atoms 0-based) ───────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Constraint {
    Distance {
        atoms: [usize; 2],
        #[serde(default)]
        value: Option<f64>,
    },
    Angle {
        atoms: [usize; 3],
        #[serde(default)]
        value: Option<f64>,
    },
    Dihedral {
        atoms: [usize; 4],
        #[serde(default)]
        value: Option<f64>,
    },
    Cartesian {
        atoms: [usize; 1],
    },
}

impl Constraint {
    fn atom_indices(&self) -> &[usize] {
        match self {
            Constraint::Distance { atoms, .. } => atoms,
            Constraint::Angle { atoms, .. } => atoms,
            Constraint::Dihedral { atoms, .. } => atoms,
            Constraint::Cartesian { atoms } => atoms,
        }
    }
}

// ── Small geometry (self-contained; consistent target + post-check) ───────────

type V3 = [f64; 3];
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}
pub fn distance(p: &[V3], i: usize, j: usize) -> f64 {
    norm(sub(p[i], p[j]))
}
/// Angle i–v–j in degrees, [0, 180].
pub fn angle(p: &[V3], i: usize, v: usize, j: usize) -> f64 {
    let a = sub(p[i], p[v]);
    let b = sub(p[j], p[v]);
    let c = (dot(a, b) / (norm(a) * norm(b))).clamp(-1.0, 1.0);
    c.acos().to_degrees()
}
/// Dihedral i–j–k–l in degrees, folded to [0, 360) (matches ASE / `measure.ts`).
pub fn dihedral(p: &[V3], i: usize, j: usize, k: usize, l: usize) -> f64 {
    let b0 = sub(p[j], p[i]);
    let b1 = sub(p[k], p[j]);
    let b2 = sub(p[l], p[k]);
    let n1 = cross(b0, b1);
    let n2 = cross(b1, b2);
    let m = cross(n1, unit(b1));
    let x = dot(n1, n2);
    let y = dot(m, n2);
    let mut deg = y.atan2(x).to_degrees();
    if deg < 0.0 {
        deg += 360.0;
    }
    deg
}
fn unit(a: V3) -> V3 {
    let n = norm(a);
    if n == 0.0 {
        a
    } else {
        [a[0] / n, a[1] / n, a[2] / n]
    }
}
/// Smallest absolute difference between two angles on a 360° circle.
fn angular_diff(a: f64, b: f64) -> f64 {
    let d = (a - b).abs() % 360.0;
    d.min(360.0 - d)
}

// ── xyz parse ─────────────────────────────────────────────────────────────────

/// Parse a standard xyz string → (elements, positions). Errors on a malformed
/// header or a row count that doesn't match the declared atom count.
pub fn parse_xyz(text: &str) -> Result<(Vec<String>, Vec<V3>), AppError> {
    let mut lines = text.lines();
    let count: usize = lines
        .next()
        .and_then(|l| l.trim().parse().ok())
        .ok_or_else(|| AppError::Backend("xtb: malformed xyz (no atom count)".into()))?;
    let _comment = lines.next();
    let mut elements = Vec::with_capacity(count);
    let mut positions = Vec::with_capacity(count);
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let el = it
            .next()
            .ok_or_else(|| AppError::Backend("xtb: malformed xyz row".into()))?;
        let coords: Vec<f64> = it.filter_map(|t| t.parse().ok()).collect();
        if coords.len() < 3 {
            return Err(AppError::Backend("xtb: malformed xyz coordinates".into()));
        }
        elements.push(el.to_string());
        positions.push([coords[0], coords[1], coords[2]]);
        if elements.len() == count {
            break;
        }
    }
    if elements.len() != count {
        return Err(AppError::Backend(format!(
            "xtb: xyz declared {count} atoms but held {}",
            elements.len()
        )));
    }
    Ok((elements, positions))
}

// ── xcontrol ($constrain / $fix) generation, 1-based ──────────────────────────

/// Build the xtb detailed-input (`$constrain` / `$fix`) for these constraints,
/// converting every atom index from OrcaStudio's 0-based space to xtb's
/// **1-based** space (`+1`). Distance/angle/dihedral use their resolved target
/// value (explicit, or the geometry's current value when the user froze "as-is");
/// a Cartesian constraint becomes a hard `$fix atoms:` entry.
pub fn build_xcontrol(constraints: &[Constraint], targets: &[Target]) -> String {
    let mut out = String::new();
    let mut fixed: Vec<usize> = Vec::new();
    let mut has_geom = false;
    let mut body = String::new();
    for (c, t) in constraints.iter().zip(targets) {
        match (c, t) {
            (Constraint::Distance { atoms, .. }, Target::Value(v)) => {
                has_geom = true;
                body.push_str(&format!(
                    "  distance: {}, {}, {:.6}\n",
                    atoms[0] + 1,
                    atoms[1] + 1,
                    v
                ));
            }
            (Constraint::Angle { atoms, .. }, Target::Value(v)) => {
                has_geom = true;
                body.push_str(&format!(
                    "  angle: {}, {}, {}, {:.4}\n",
                    atoms[0] + 1,
                    atoms[1] + 1,
                    atoms[2] + 1,
                    v
                ));
            }
            (Constraint::Dihedral { atoms, .. }, Target::Value(v)) => {
                has_geom = true;
                body.push_str(&format!(
                    "  dihedral: {}, {}, {}, {}, {:.4}\n",
                    atoms[0] + 1,
                    atoms[1] + 1,
                    atoms[2] + 1,
                    atoms[3] + 1,
                    v
                ));
            }
            (Constraint::Cartesian { atoms }, _) => fixed.push(atoms[0] + 1),
            _ => {}
        }
    }
    if has_geom {
        out.push_str("$constrain\n");
        out.push_str(&format!("  force constant={FORCE_CONSTANT}\n"));
        out.push_str(&body);
        out.push_str("$end\n");
    }
    if !fixed.is_empty() {
        let list: Vec<String> = fixed.iter().map(|i| i.to_string()).collect();
        out.push_str("$fix\n");
        out.push_str(&format!("  atoms: {}\n", list.join(",")));
        out.push_str("$end\n");
    }
    out
}

/// The resolved target for a constraint: a scalar value (distance/angle/dihedral)
/// or `Fixed` (a Cartesian `$fix`, checked as zero displacement).
#[derive(Debug, Clone, Copy)]
pub enum Target {
    Value(f64),
    Fixed,
}

/// Resolve each constraint's target from the input geometry: the explicit value
/// if the user gave one, else the coordinate's CURRENT value (freeze as-is).
/// Errors (with the offending index) if any atom index is out of range — the same
/// out-of-range that would otherwise reach xtb.
pub fn resolve_targets(constraints: &[Constraint], p: &[V3]) -> Result<Vec<Target>, AppError> {
    let n = p.len();
    let mut out = Vec::with_capacity(constraints.len());
    for c in constraints {
        for &i in c.atom_indices() {
            if i >= n {
                return Err(AppError::Backend(format!(
                    "xtb: constraint references atom #{i} but the geometry has {n} atoms (0–{})",
                    n - 1
                )));
            }
        }
        let t = match c {
            Constraint::Distance { atoms, value } => {
                Target::Value(value.unwrap_or_else(|| distance(p, atoms[0], atoms[1])))
            }
            Constraint::Angle { atoms, value } => {
                Target::Value(value.unwrap_or_else(|| angle(p, atoms[0], atoms[1], atoms[2])))
            }
            Constraint::Dihedral { atoms, value } => Target::Value(
                value.unwrap_or_else(|| dihedral(p, atoms[0], atoms[1], atoms[2], atoms[3])),
            ),
            Constraint::Cartesian { .. } => Target::Fixed,
        };
        out.push(t);
    }
    Ok(out)
}

// ── Post-conditions ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct HeldConstraint {
    pub kind: String,
    pub target: f64,
    pub final_value: f64,
    pub deviation: f64,
    pub unit: String,
}

/// Check every constraint is held within tolerance in the optimized geometry.
/// Returns per-constraint results on success; an `Err` (naming the constraint,
/// target, actual, deviation) if any drifted — that is a real failure (xtb could
/// not hold it, or the index base was wrong and the wrong pair was constrained),
/// never a silently-returned geometry.
pub fn check_held(
    constraints: &[Constraint],
    targets: &[Target],
    p0: &[V3],
    p1: &[V3],
) -> Result<Vec<HeldConstraint>, AppError> {
    let mut held = Vec::with_capacity(constraints.len());
    for (c, t) in constraints.iter().zip(targets) {
        let (kind, unit, target, final_value, dev, tol) = match (c, t) {
            (Constraint::Distance { atoms, .. }, Target::Value(v)) => {
                let f = distance(p1, atoms[0], atoms[1]);
                ("distance", "Å", *v, f, (f - v).abs(), TOL_DISTANCE_ANG)
            }
            (Constraint::Angle { atoms, .. }, Target::Value(v)) => {
                let f = angle(p1, atoms[0], atoms[1], atoms[2]);
                ("angle", "°", *v, f, (f - v).abs(), TOL_ANGLE_DEG)
            }
            (Constraint::Dihedral { atoms, .. }, Target::Value(v)) => {
                let f = dihedral(p1, atoms[0], atoms[1], atoms[2], atoms[3]);
                ("dihedral", "°", *v, f, angular_diff(f, *v), TOL_ANGLE_DEG)
            }
            (Constraint::Cartesian { atoms }, _) => {
                let d = distance_between(p0[atoms[0]], p1[atoms[0]]);
                ("cartesian", "Å", 0.0, d, d, TOL_CARTESIAN_ANG)
            }
            _ => continue,
        };
        if dev > tol {
            return Err(AppError::Backend(format!(
                "xtb did not hold a {kind} constraint: target {target:.4}{unit}, \
                 got {final_value:.4}{unit} (off by {dev:.4}{unit}, tolerance {tol}{unit}). \
                 The geometry fights the constraint, or the target is unreachable."
            )));
        }
        held.push(HeldConstraint {
            kind: kind.to_string(),
            target,
            final_value,
            deviation: dev,
            unit: unit.to_string(),
        });
    }
    Ok(held)
}

fn distance_between(a: V3, b: V3) -> f64 {
    norm(sub(a, b))
}

// ── Runner state + commands ───────────────────────────────────────────────────

struct XtbRun {
    pgid: i32,
    dir: PathBuf,
    cancelled: Arc<AtomicBool>,
}

/// Single-slot runner: xtb is a synchronous helper (seconds), so at most one runs
/// at a time. Holds the running pgid + dir so `xtb_cancel` can kill the group.
#[derive(Default)]
pub struct XtbRunner {
    running: Mutex<Option<XtbRun>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct XtbResult {
    /// Optimized geometry, standard xyz, SAME atom order as the input.
    pub xyz: String,
    pub wall_time_secs: f64,
    pub held: Vec<HeldConstraint>,
}

/// Resolve a possibly-bare binary name to an absolute path via `$PATH`, so xtb is
/// always invoked by full path (domain rule #7 spirit; a bare name works for xtb
/// since it has no MPI, but we prefer explicit). Returns the input unchanged if it
/// already looks like a path or nothing matches (Command then fails cleanly).
fn resolve_binary(configured: &str) -> String {
    if configured.contains('/') {
        return configured.to_string();
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            let candidate = Path::new(dir).join(configured);
            if candidate.is_file() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }
    configured.to_string()
}

fn xtb_path(db: &State<'_, DbState>) -> Result<String, AppError> {
    let conn = db.lock()?;
    let configured: String = conn
        .query_row("SELECT value FROM settings WHERE key = 'xtb_path'", [], |r| {
            r.get(0)
        })
        .unwrap_or_else(|_| "xtb".to_string());
    Ok(resolve_binary(&configured))
}

/// Settings "check" button: run `xtb --version` and return the parsed version
/// line. Errors if the binary can't be run.
#[tauri::command]
pub fn xtb_version(db: State<'_, DbState>) -> Result<String, AppError> {
    let path = xtb_path(&db)?;
    let out = Command::new(&path)
        .arg("--version")
        .output()
        .map_err(|e| AppError::Backend(format!("could not run xtb at '{path}': {e}")))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // e.g. "* xtb version 6.6.1 (unknown) compiled by ..."
    let version = text
        .lines()
        .find(|l| l.to_ascii_lowercase().contains("xtb version"))
        .and_then(|l| {
            l.split("version")
                .nth(1)
                .map(|s| s.split_whitespace().next().unwrap_or("").to_string())
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    Ok(format!("xtb {version}"))
}

/// Cancel the running xtb (if any): flag it and kill the process group + sweep by
/// cwd (`debugging/004`).
#[tauri::command]
pub fn xtb_cancel(runner: State<'_, XtbRunner>) {
    let guard = match runner.running.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if let Some(run) = guard.as_ref() {
        run.cancelled.store(true, Ordering::SeqCst);
        terminate_job(run.pgid, &run.dir);
    }
}

/// Pre-optimize a scene with GFN2-xTB, holding `constraints`. Prepares an isolated
/// dir (domain rule #3), writes `input.xyz` + xcontrol (constraints in xtb's
/// 1-based `$constrain`), runs xtb by full path, checks post-conditions (atom
/// count, element order, each constraint held within tolerance), cleans up, and
/// returns the optimized geometry. Killable via `xtb_cancel` / timeout.
#[tauri::command]
pub fn xtb_optimize(
    db: State<'_, DbState>,
    runner: State<'_, XtbRunner>,
    xyz: String,
    charge: i32,
    multiplicity: i32,
    constraints: Vec<Constraint>,
    timeout_secs: Option<u64>,
) -> Result<XtbResult, AppError> {
    if multiplicity < 1 {
        return Err(AppError::Backend("xtb: multiplicity must be ≥ 1".into()));
    }
    // Reject a concurrent run (single slot).
    {
        let guard = runner
            .running
            .lock()
            .map_err(|_| AppError::Internal("xtb runner mutex poisoned".into()))?;
        if guard.is_some() {
            return Err(AppError::Backend(
                "an xtb optimization is already running".into(),
            ));
        }
    }
    let path = xtb_path(&db)?;

    // Parse the input geometry once — for target resolution AND the element-order
    // post-condition.
    let (elements_in, positions_in) = parse_xyz(&xyz)?;
    let targets = resolve_targets(&constraints, &positions_in)?;

    // Isolated directory (domain rule #3).
    let data_dir = dirs::data_dir()
        .ok_or_else(|| AppError::Internal("no user data directory".into()))?
        .join("orcastudio");
    let dir = data_dir.join("xtb").join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir)?;

    // Run everything with cleanup guaranteed afterwards (rule #3).
    let cancelled = Arc::new(AtomicBool::new(false));
    let result = run_in_dir(
        &path,
        &dir,
        &runner,
        &cancelled,
        &xyz,
        charge,
        multiplicity,
        &constraints,
        &targets,
        &elements_in,
        &positions_in,
        timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
    );

    // Always: unregister + remove the scratch dir (litters like ORCA — rule #3).
    if let Ok(mut g) = runner.running.lock() {
        *g = None;
    }
    let _ = std::fs::remove_dir_all(&dir);

    result
}

#[allow(clippy::too_many_arguments)]
fn run_in_dir(
    path: &str,
    dir: &Path,
    runner: &State<'_, XtbRunner>,
    cancelled: &Arc<AtomicBool>,
    xyz: &str,
    charge: i32,
    multiplicity: i32,
    constraints: &[Constraint],
    targets: &[Target],
    elements_in: &[String],
    positions_in: &[V3],
    timeout_secs: u64,
) -> Result<XtbResult, AppError> {
    std::fs::write(dir.join("input.xyz"), xyz)?;
    std::fs::write(dir.join("xcontrol"), build_xcontrol(constraints, targets))?;
    let uhf = multiplicity - 1; // unpaired electrons = 2S = mult − 1

    let stdout = std::fs::File::create(dir.join("xtb.out"))?;
    let stderr = stdout.try_clone()?;
    let mut cmd = Command::new(path);
    cmd.current_dir(dir)
        .arg("input.xyz")
        .arg("--input")
        .arg("xcontrol")
        .arg("--opt")
        .arg("--gfn")
        .arg("2")
        .arg("--chrg")
        .arg(charge.to_string())
        .arg("--uhf")
        .arg(uhf.to_string())
        .env("OMP_NUM_THREADS", "4")
        .env("OMP_STACKSIZE", "1G")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // own group → cancel can killpg the tree (004)
    }

    let start = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::Backend(format!("could not spawn xtb at '{path}': {e}")))?;
    let pgid = child.id() as i32; // process_group(0) → pgid == child pid

    // Register so xtb_cancel can reach it.
    {
        let mut g = runner
            .running
            .lock()
            .map_err(|_| AppError::Internal("xtb runner mutex poisoned".into()))?;
        *g = Some(XtbRun {
            pgid,
            dir: dir.to_path_buf(),
            cancelled: cancelled.clone(),
        });
    }

    // Poll for exit / cancel / timeout.
    let deadline = start + Duration::from_secs(timeout_secs);
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                return Err(AppError::Backend(format!(
                    "xtb exited with an error.\n{}",
                    tail(&dir.join("xtb.out"), 20)
                )));
            }
            break;
        }
        if cancelled.load(Ordering::SeqCst) {
            terminate_job(pgid, dir);
            return Err(AppError::Backend("xtb optimization cancelled".into()));
        }
        if Instant::now() > deadline {
            terminate_job(pgid, dir);
            return Err(AppError::Backend(format!(
                "xtb timed out after {timeout_secs}s"
            )));
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // xtb prints "normal termination of xtb" on success.
    let out_text = std::fs::read_to_string(dir.join("xtb.out")).unwrap_or_default();
    if !out_text.contains("normal termination") {
        return Err(AppError::Backend(format!(
            "xtb did not terminate normally.\n{}",
            tail(&dir.join("xtb.out"), 20)
        )));
    }

    // Read the optimized geometry.
    let opt = std::fs::read_to_string(dir.join("xtbopt.xyz"))
        .map_err(|_| AppError::Backend("xtb produced no xtbopt.xyz".into()))?;
    let (elements_out, positions_out) = parse_xyz(&opt)?;

    // Post-conditions (in the command, not only in tests — the price of a missed
    // error is the wrong geometry handed to a multi-hour ORCA run).
    if elements_out.len() != elements_in.len() {
        return Err(AppError::Backend(format!(
            "xtb changed the atom count ({} → {})",
            elements_in.len(),
            elements_out.len()
        )));
    }
    for (i, (a, b)) in elements_in.iter().zip(&elements_out).enumerate() {
        if !a.eq_ignore_ascii_case(b) {
            return Err(AppError::Backend(format!(
                "xtb changed the element order at atom #{i} ({a} → {b})"
            )));
        }
    }
    let held = check_held(constraints, targets, positions_in, &positions_out)?;

    Ok(XtbResult {
        xyz: opt,
        wall_time_secs: start.elapsed().as_secs_f64(),
        held,
    })
}

/// Last `n` lines of a file (for an error message); empty if unreadable.
fn tail(path: &Path, n: usize) -> String {
    std::fs::read_to_string(path)
        .map(|s| {
            let lines: Vec<&str> = s.lines().collect();
            lines[lines.len().saturating_sub(n)..].join("\n")
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xyz_positions() -> Vec<V3> {
        // chloromethane order Cl, C, H, H, H (the index-base experiment geometry)
        vec![
            [0.0, 0.0, 1.778],
            [0.0, 0.0, 0.0],
            [1.026719, 0.0, -0.363],
            [-0.51336, -0.889165, -0.363],
            [-0.51336, 0.889165, -0.363],
        ]
    }

    #[test]
    fn distance_matches_hand_calc() {
        let p = xyz_positions();
        assert!((distance(&p, 0, 1) - 1.778).abs() < 1e-6); // Cl–C
    }

    #[test]
    fn xcontrol_is_one_based_and_holds_the_right_pair() {
        // Our 0-based (0,1) = Cl–C → xtb 1-based must be "1, 2".
        let cs = vec![Constraint::Distance {
            atoms: [0, 1],
            value: Some(1.234),
        }];
        let p = xyz_positions();
        let targets = resolve_targets(&cs, &p).unwrap();
        let xc = build_xcontrol(&cs, &targets);
        assert!(xc.contains("$constrain"));
        assert!(xc.contains("force constant=1"));
        assert!(xc.contains("distance: 1, 2, 1.234000"), "got:\n{xc}");
    }

    #[test]
    fn freeze_as_is_resolves_current_value() {
        let cs = vec![Constraint::Distance {
            atoms: [0, 1],
            value: None,
        }];
        let p = xyz_positions();
        let targets = resolve_targets(&cs, &p).unwrap();
        match targets[0] {
            Target::Value(v) => assert!((v - 1.778).abs() < 1e-6),
            _ => panic!("expected a value target"),
        }
    }

    #[test]
    fn angle_and_dihedral_one_based_indices() {
        let p = xyz_positions();
        let cs = vec![
            Constraint::Angle {
                atoms: [0, 1, 2],
                value: Some(109.0),
            },
            Constraint::Dihedral {
                atoms: [0, 1, 2, 3],
                value: Some(90.0),
            },
        ];
        let targets = resolve_targets(&cs, &p).unwrap();
        let xc = build_xcontrol(&cs, &targets);
        assert!(xc.contains("angle: 1, 2, 3, 109.0000"), "got:\n{xc}");
        assert!(xc.contains("dihedral: 1, 2, 3, 4, 90.0000"), "got:\n{xc}");
    }

    #[test]
    fn cartesian_becomes_a_fix_block() {
        let cs = vec![Constraint::Cartesian { atoms: [4] }];
        let p = xyz_positions();
        let targets = resolve_targets(&cs, &p).unwrap();
        let xc = build_xcontrol(&cs, &targets);
        assert!(xc.contains("$fix"), "got:\n{xc}");
        assert!(xc.contains("atoms: 5"), "got:\n{xc}"); // 0-based 4 → 1-based 5
    }

    #[test]
    fn out_of_range_index_is_rejected_before_xtb() {
        let cs = vec![Constraint::Distance {
            atoms: [0, 99],
            value: None,
        }];
        let p = xyz_positions();
        assert!(resolve_targets(&cs, &p).is_err());
    }

    #[test]
    fn check_held_flags_a_drifted_constraint() {
        let cs = vec![Constraint::Distance {
            atoms: [0, 1],
            value: Some(1.234),
        }];
        let p0 = xyz_positions();
        let targets = resolve_targets(&cs, &p0).unwrap();
        // A "result" where Cl–C relaxed back to 1.778 (xtb did NOT hold it).
        let p1 = p0.clone();
        let err = check_held(&cs, &targets, &p0, &p1).unwrap_err();
        assert!(format!("{err}").contains("did not hold"));
    }

    #[test]
    fn check_held_passes_within_tolerance() {
        let cs = vec![Constraint::Distance {
            atoms: [0, 1],
            value: Some(1.778),
        }];
        let p0 = xyz_positions();
        let targets = resolve_targets(&cs, &p0).unwrap();
        let held = check_held(&cs, &targets, &p0, &p0).unwrap();
        assert_eq!(held.len(), 1);
        assert!(held[0].deviation < 1e-9);
    }

    #[test]
    fn parse_xyz_roundtrips_count_and_elements() {
        let text = "3\ncomment\nO 0 0 0\nH 0 0 1\nH 0 1 0\n";
        let (el, p) = parse_xyz(text).unwrap();
        assert_eq!(el, vec!["O", "H", "H"]);
        assert_eq!(p.len(), 3);
    }
}
