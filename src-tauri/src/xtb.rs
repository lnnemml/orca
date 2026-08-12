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
//! constraints 0-based (ADR-008). Getting the flip wrong freezes the WRONG coordinate
//! on an optimization that finishes cleanly — so unit 1e **brands the serde boundary**
//! (`SceneIndex` 0-based in, `XtbIndex` 1-based out): the `+1` is the ONE typed
//! conversion [`SceneIndex::to_xtb`], and a 0-based index has no `Display`, so it
//! cannot reach a line without it. The `check_held` post-condition below remains the
//! runtime guard. See `wiki/orca/xtb.md` and `wiki/orca/gotchas.md`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

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

/// The MEASURED non-convergence marker (xtb 6.6.1, `builduser@buildhost`, real
/// `--cycles 2` run — `wiki/orca/xtb.md`, `debugging/012`). The full line is
/// `*** FAILED TO CONVERGE GEOMETRY OPTIMIZATION IN N ITERATIONS ***`; this stable
/// substring is what the completion post-condition scans for. **Copied from a real
/// run, not invented.** On non-convergence xtb 6.6.1 still writes `xtbopt.xyz` +
/// `.xtboptok` AND prints `normal termination` AND exits 0 — so this line is the
/// ONLY reliable non-convergence signal, and it must be checked first.
const FAILED_TO_CONVERGE: &str = "FAILED TO CONVERGE GEOMETRY OPTIMIZATION";

/// Measured (same run): xtb prints this to **stderr** near the end. It is NOT a
/// gate — it can sit ~40 lines deep under the post-opt bond-order analysis (a small
/// tail misses it → the false negative this hotfix fixes) AND it is printed even on
/// a non-converged run. Kept only as a **named diagnostic**.
const NORMAL_TERMINATION: &str = "normal termination of xtb";

/// Size cap for reading `xtb.out` whole to scan for {@link FAILED_TO_CONVERGE}
/// (which can sit hundreds of lines from the end — the full property analysis runs
/// even after non-convergence, so a bounded tail can't catch it). A GFN2 pre-opt
/// log is small (measured 42–91 KB); the cap (rule #5) refuses a pathological file
/// rather than stream an unbounded one.
const XTB_OUT_CAP_BYTES: u64 = 16 * 1024 * 1024;

// ── Branded index bases (unit 1e) ─────────────────────────────────────────────
//
// The xtb serde boundary is branded so the 0→1 base flip is ONE typed conversion,
// not a bare `+ 1` scattered across the writer (where a missed or doubled flip
// freezes the wrong coordinate on a clean-finishing optimization — domain rule #9).

/// A 0-based atom index as it arrives from the app (scene / ORCA space, ADR-008).
/// `#[serde(transparent)]`, so the wire `[0,1]` deserializes straight into these.
/// It has **no `Display`** on purpose: a 0-based index therefore cannot be written
/// into a `$constrain` / `$fix` line without first going through [`SceneIndex::to_xtb`]
/// — that is the compile-time half of control (d).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct SceneIndex(usize);

impl SceneIndex {
    #[cfg(test)]
    pub fn new(i: usize) -> Self {
        SceneIndex(i)
    }
    /// The 0-based value, for indexing OUR OWN geometry arrays. Named (not `Deref`)
    /// so a raw geometry index is always a deliberate `.zero_based()`, never implicit.
    pub fn zero_based(self) -> usize {
        self.0
    }
    /// The SINGLE 0→1 base flip in the whole module — the only place `+ 1` touches an
    /// atom index. Returns the 1-based [`XtbIndex`] that xtb `$constrain` demands
    /// (measured: `wiki/orca/xtb.md`).
    pub fn to_xtb(self) -> XtbIndex {
        XtbIndex(self.0 + 1)
    }
}

/// A 1-based xtb `$constrain` / `$fix` atom index. Constructible ONLY via
/// [`SceneIndex::to_xtb`], and the ONLY index type with `Display`, so every index
/// written into an xcontrol line provably passed the one typed conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XtbIndex(usize);

impl std::fmt::Display for XtbIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── Constraints (mirror the TS `Constraint`, atoms 0-based `SceneIndex`) ───────
//
// TODO(1e-followup): NOT unified with `orcastudio_core::emit::Constraint` (the ORCA
// `%geom` 0-based constraint with `value_text`). Reason: this type drives a DIFFERENT
// emit — xtb `$constrain`/`$fix`, 1-based, with a separate `Target` resolution and no
// `value_text` — so folding the two would drag a rewrite of the xtb-input emit, which
// the unit-1e brief explicitly scopes out. Unify only if/when the xtb emit is revisited.

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Constraint {
    Distance {
        atoms: [SceneIndex; 2],
        #[serde(default)]
        value: Option<f64>,
    },
    Angle {
        atoms: [SceneIndex; 3],
        #[serde(default)]
        value: Option<f64>,
    },
    Dihedral {
        atoms: [SceneIndex; 4],
        #[serde(default)]
        value: Option<f64>,
    },
    Cartesian {
        atoms: [SceneIndex; 1],
    },
}

impl Constraint {
    fn atom_indices(&self) -> &[SceneIndex] {
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
///
/// Returns **`None` when there is nothing to write** (no constraints). This is the
/// SINGLE source of truth for the empty case: `None` → the file is NOT created AND
/// `--input` is NOT passed. An empty `xcontrol` passed via `--input` hangs xtb
/// 6.6.1 before the first cycle (`wiki/debugging/006`) — the 2.5.5-fix-2 bug came
/// from three decisions (content / write file / pass flag) drifting apart; making
/// them read one value closes it.
pub fn build_xcontrol(constraints: &[Constraint], targets: &[Target]) -> Option<String> {
    let mut out = String::new();
    let mut fixed: Vec<XtbIndex> = Vec::new();
    let mut has_geom = false;
    let mut body = String::new();
    // Every index below is written via `.to_xtb()` (the one typed 0→1 flip); there is
    // no bare `+ 1` on an atom index in this writer.
    for (c, t) in constraints.iter().zip(targets) {
        match (c, t) {
            (Constraint::Distance { atoms, .. }, Target::Value(v)) => {
                has_geom = true;
                body.push_str(&format!(
                    "  distance: {}, {}, {:.6}\n",
                    atoms[0].to_xtb(),
                    atoms[1].to_xtb(),
                    v
                ));
            }
            (Constraint::Angle { atoms, .. }, Target::Value(v)) => {
                has_geom = true;
                body.push_str(&format!(
                    "  angle: {}, {}, {}, {:.4}\n",
                    atoms[0].to_xtb(),
                    atoms[1].to_xtb(),
                    atoms[2].to_xtb(),
                    v
                ));
            }
            (Constraint::Dihedral { atoms, .. }, Target::Value(v)) => {
                has_geom = true;
                body.push_str(&format!(
                    "  dihedral: {}, {}, {}, {}, {:.4}\n",
                    atoms[0].to_xtb(),
                    atoms[1].to_xtb(),
                    atoms[2].to_xtb(),
                    atoms[3].to_xtb(),
                    v
                ));
            }
            (Constraint::Cartesian { atoms }, _) => fixed.push(atoms[0].to_xtb()),
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
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Build xtb's argv (after the binary path). **`--input xcontrol` is included ONLY
/// when there is an xcontrol to pass** — the SAME `has_xcontrol` that decides
/// whether the file is written (see `build_xcontrol`). Pure, so a test proves the
/// flag is present with constraints and absent without — the test that would have
/// caught the 2.5.5-fix-2 hang without a five-minute wait.
pub fn xtb_args(has_xcontrol: bool, charge: i32, uhf: i32) -> Vec<String> {
    let mut a = vec!["input.xyz".to_string()];
    if has_xcontrol {
        a.push("--input".to_string());
        a.push("xcontrol".to_string());
    }
    for s in ["--opt", "--gfn", "2", "--chrg"] {
        a.push(s.to_string());
    }
    a.push(charge.to_string());
    a.push("--uhf".to_string());
    a.push(uhf.to_string());
    a
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
            let i = i.zero_based();
            if i >= n {
                return Err(AppError::Backend(format!(
                    "xtb: constraint references atom #{i} but the geometry has {n} atoms (0–{})",
                    n - 1
                )));
            }
        }
        // `.zero_based()` indexes OUR geometry (0-based); the 0→1 xtb flip happens only
        // in the writer via `.to_xtb()`.
        let t = match c {
            Constraint::Distance { atoms, value } => Target::Value(
                value.unwrap_or_else(|| distance(p, atoms[0].zero_based(), atoms[1].zero_based())),
            ),
            Constraint::Angle { atoms, value } => Target::Value(value.unwrap_or_else(|| {
                angle(p, atoms[0].zero_based(), atoms[1].zero_based(), atoms[2].zero_based())
            })),
            Constraint::Dihedral { atoms, value } => Target::Value(value.unwrap_or_else(|| {
                dihedral(
                    p,
                    atoms[0].zero_based(),
                    atoms[1].zero_based(),
                    atoms[2].zero_based(),
                    atoms[3].zero_based(),
                )
            })),
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
                let f = distance(p1, atoms[0].zero_based(), atoms[1].zero_based());
                ("distance", "Å", *v, f, (f - v).abs(), TOL_DISTANCE_ANG)
            }
            (Constraint::Angle { atoms, .. }, Target::Value(v)) => {
                let f = angle(p1, atoms[0].zero_based(), atoms[1].zero_based(), atoms[2].zero_based());
                ("angle", "°", *v, f, (f - v).abs(), TOL_ANGLE_DEG)
            }
            (Constraint::Dihedral { atoms, .. }, Target::Value(v)) => {
                let f = dihedral(
                    p1,
                    atoms[0].zero_based(),
                    atoms[1].zero_based(),
                    atoms[2].zero_based(),
                    atoms[3].zero_based(),
                );
                ("dihedral", "°", *v, f, angular_diff(f, *v), TOL_ANGLE_DEG)
            }
            (Constraint::Cartesian { atoms }, _) => {
                let a = atoms[0].zero_based();
                let d = distance_between(p0[a], p1[a]);
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

// ── Completion post-condition — anchored on RESULTS, not the binary's last words ─
//
// The 2b-hotfix. The old gate ("`normal termination` present in a 30-line tail")
// was wrong in BOTH directions, measured on xtb 6.6.1 (`builduser@buildhost`):
//  - FALSE NEGATIVE on a clean run — `normal termination` (stderr) sits ~41 lines
//    from the end, buried under the post-opt bond-order table, so the small tail
//    misses it and a perfectly good geometry is rejected;
//  - FALSE POSITIVE latent on non-convergence — `--cycles 2` prints
//    `*** FAILED TO CONVERGE GEOMETRY OPTIMIZATION IN N ITERATIONS ***`, yet exits
//    0, writes `.xtboptok`, AND writes a (non-optimized!) `xtbopt.xyz`, AND prints
//    `normal termination`. The old gate would silently accept that geometry.
// So the exit code and `normal termination` do NOT gate (measured: both lie either
// way) — they are named diagnostics. Success is defined in OUR terms: an optimized
// geometry present + parseable, and NO `FAILED TO CONVERGE` line. See
// `wiki/orca/xtb.md` (Pattern-2 correction) and `debugging/012`.

/// The outcome of an xtb run, classified from the log + whether the optimized
/// geometry is present and parseable. **Pure** — the same classifier the tests run
/// on the three real fixtures (success / non-convergence / no-geometry).
#[derive(Debug, PartialEq, Eq)]
pub enum XtbCompletion {
    /// Optimized geometry present and no non-convergence marker.
    Ok,
    /// xtb wrote a geometry but reported it did NOT converge (the quoted line +
    /// the iteration count). The geometry is NOT optimized — must not be applied.
    NonConvergence { line: String, iterations: Option<u32> },
    /// No readable optimized geometry (xtb died before writing / wrote garbage).
    NoGeometry,
}

/// Classify an xtb run from `out_text` (the whole size-capped `xtb.out`) and
/// whether `xtbopt.xyz` exists and parsed. **Non-convergence is checked FIRST**:
/// xtb writes `xtbopt.xyz` even when it fails to converge, so a present geometry is
/// NOT success on its own.
pub fn classify_completion(out_text: &str, geometry_ok: bool) -> XtbCompletion {
    if let Some(line) = out_text.lines().find(|l| l.contains(FAILED_TO_CONVERGE)) {
        return XtbCompletion::NonConvergence {
            line: line.trim().to_string(),
            iterations: parse_failed_iterations(line),
        };
    }
    if !geometry_ok {
        return XtbCompletion::NoGeometry;
    }
    XtbCompletion::Ok
}

/// Pull `N` out of `*** FAILED TO CONVERGE GEOMETRY OPTIMIZATION IN N ITERATIONS ***`.
fn parse_failed_iterations(line: &str) -> Option<u32> {
    line.split(" IN ").nth(1)?.split_whitespace().next()?.parse().ok()
}

/// Read `xtb.out` whole for the FAILED scan, size-capped (rule #5): the marker can
/// be hundreds of lines from the end, so a tail won't do — but a pre-opt log is
/// small, and a pathological one is refused rather than streamed.
fn read_out_capped(path: &Path) -> Result<String, AppError> {
    let len = std::fs::metadata(path)
        .map_err(|_| AppError::Backend("xtb produced no xtb.out".into()))?
        .len();
    if len > XTB_OUT_CAP_BYTES {
        return Err(AppError::Backend(format!(
            "xtb.out is {len} bytes (over the {} MB scan cap) — refusing to load it whole",
            XTB_OUT_CAP_BYTES / 1024 / 1024
        )));
    }
    std::fs::read_to_string(path)
        .map_err(|e| AppError::Backend(format!("could not read xtb.out: {e}")))
}

/// The named diagnostics that are NOT gates (measured to lie both ways): the exit
/// code and whether `normal termination` was seen. Appended to a failure message so
/// the reason is legible without gating on either.
fn completion_diagnostics(out_text: &str, exit: std::process::ExitStatus) -> String {
    let code = exit
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "killed by signal".into());
    format!(
        "[xtb diagnostics, not gates] exit code: {code}; \"{NORMAL_TERMINATION}\" seen: {} \
         (measured: both are unreliable in either direction — debugging/012)",
        out_text.contains(NORMAL_TERMINATION)
    )
}

// ── Runner state + commands ───────────────────────────────────────────────────

/// The reserved slot for the one in-flight run. Just the cancel flag: `Some` means
/// busy, and the worker thread (which holds the pgid + dir locally) does the actual
/// killpg/sweep off the UI thread when it sees the flag. `xtb_cancel` only flips it.
struct XtbRun {
    cancelled: Arc<AtomicBool>,
}

/// Single-slot runner: xtb is a helper (seconds), so at most one runs at a time.
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

/// Payload of the `xtb:error` event (the run failed / was cancelled / timed out).
/// `dir` is the kept diagnostic directory on a genuine failure (not a cancel), for
/// the UI to show as copyable text.
#[derive(Clone, Serialize)]
struct XtbErrorPayload {
    message: String,
    dir: Option<String>,
}

/// Payload of the `xtb:progress` event — the current optimization cycle, so the UI
/// shows live movement (and "very long" becomes visible immediately, not after
/// five minutes of silence).
#[derive(Clone, Serialize)]
struct XtbProgressPayload {
    cycle: u32,
}

/// Resolve a possibly-bare binary name to an absolute path via `$PATH`, so xtb is
/// always invoked by full path (domain rule #7 spirit; a bare name works for xtb
/// since it has no MPI, but we prefer explicit). Returns the input unchanged if it
/// already looks like a path or nothing matches (Command then fails cleanly).
pub(crate) fn resolve_binary(configured: &str) -> String {
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

/// Cancel the running xtb (if any). **Only sets the flag** — it must NOT block: it
/// runs on the main thread, and `terminate_job` sleeps up to ~12 s (SIGTERM grace
/// + SIGKILL). The worker thread's poll loop checks `cancelled` every 50 ms and
/// does the actual killpg + cwd sweep (`debugging/004`) on ITS thread, then cleans
/// up. So the button returns instantly and the kill happens off the UI thread.
#[tauri::command]
pub fn xtb_cancel(runner: State<'_, XtbRunner>) {
    if let Ok(guard) = runner.running.lock() {
        if let Some(run) = guard.as_ref() {
            run.cancelled.store(true, Ordering::SeqCst);
        }
    }
}

/// Start a GFN2-xTB pre-optimization holding `constraints`, then RETURN. This is a
/// **starter**, mirroring `submit_job` → `drive_job`: it validates the input,
/// reserves the single slot, and moves the actual work (spawn xtb, poll,
/// post-conditions, dir cleanup) into a `std::thread::spawn`. A long operation must
/// NEVER run inside a synchronous command — the command executes on the main
/// GTK/WebKit thread, so the window would freeze for the whole run AND `xtb_cancel`
/// (a separate command) could not be delivered. The result / errors arrive on the
/// frontend as **events** (`xtb:done` / `xtb:error`), the same mechanism job logs
/// use. See `wiki/modules/tauri-core.md`.
#[tauri::command]
pub fn xtb_optimize(
    app: AppHandle,
    db: State<'_, DbState>,
    runner: State<'_, XtbRunner>,
    xyz: String,
    charge: i32,
    multiplicity: i32,
    constraints: Vec<Constraint>,
    timeout_secs: Option<u64>,
) -> Result<(), AppError> {
    // Validate synchronously so the user gets immediate feedback (bad multiplicity,
    // out-of-range index) instead of an event a moment later.
    if multiplicity < 1 {
        return Err(AppError::Backend("xtb: multiplicity must be ≥ 1".into()));
    }
    let path = xtb_path(&db)?;
    let (elements_in, positions_in) = parse_xyz(&xyz)?;
    let targets = resolve_targets(&constraints, &positions_in)?; // out-of-range → immediate err

    // Reserve the single slot (reject a concurrent run) BEFORE returning, so a
    // second click between here and the thread spawning can't start a second run.
    let cancelled = Arc::new(AtomicBool::new(false));
    let data_dir = dirs::data_dir()
        .ok_or_else(|| AppError::Internal("no user data directory".into()))?
        .join("orcastudio");
    let dir = data_dir.join("xtb").join(uuid::Uuid::new_v4().to_string());
    {
        let mut g = runner
            .running
            .lock()
            .map_err(|_| AppError::Internal("xtb runner mutex poisoned".into()))?;
        if g.is_some() {
            return Err(AppError::Backend(
                "an xtb optimization is already running".into(),
            ));
        }
        *g = Some(XtbRun {
            cancelled: cancelled.clone(),
        });
    }

    // Off the main thread — the whole point of the 2.5.5-fix.
    let timeout = timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
    std::thread::spawn(move || {
        let result = run_in_dir(
            &app,
            &dir,
            &path,
            &cancelled,
            &xyz,
            charge,
            multiplicity,
            &constraints,
            &targets,
            &elements_in,
            &positions_in,
            timeout,
        );

        // Cleanup policy (2.5.5-fix-2): rule #3 is about clearing ORCA-style
        // scratch litter on SUCCESS, not about throwing away evidence on FAILURE.
        //  - success        → remove the dir (as before);
        //  - user cancel     → remove (not a diagnostic case);
        //  - any other error → KEEP the dir; xtb.out is the only thing that shows
        //    WHERE xtb spent its time, and it's needed exactly when a run fails.
        // Freeing the slot is unconditional either way.
        let was_cancelled = cancelled.load(Ordering::SeqCst);
        let keep = keep_dir_for_diagnostics(result.is_ok(), was_cancelled);
        if !keep {
            let _ = std::fs::remove_dir_all(&dir);
        }
        if let Some(runner) = app.try_state::<XtbRunner>() {
            if let Ok(mut g) = runner.running.lock() {
                *g = None;
            }
        }

        match result {
            Ok(res) => {
                let _ = app.emit("xtb:done", res);
            }
            Err(e) => {
                // Attach the last ~20 lines of xtb.out (bounded tail, rule #5) and,
                // when kept, the dir path — surfaced in the UI as copyable text.
                let mut message = e.to_string();
                let log_tail = tail(&dir.join("xtb.out"), 20);
                if !log_tail.trim().is_empty() {
                    message = format!("{message}\n\n— last lines of xtb.out —\n{log_tail}");
                }
                let dir_str = keep.then(|| dir.display().to_string());
                if let Some(ref d) = dir_str {
                    message = format!("{message}\n\nDiagnostic files kept at:\n{d}");
                }
                let _ = app.emit("xtb:error", XtbErrorPayload { message, dir: dir_str });
            }
        }
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_in_dir(
    app: &AppHandle,
    dir: &Path,
    path: &str,
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
    // Create the isolated dir here so the thread's cleanup-after is unconditional
    // even if we bail before spawning (a missing dir → remove_dir_all is a no-op).
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("input.xyz"), xyz)?;
    // One value drives both: write the file iff there's content, and pass --input
    // iff we wrote it. No empty --input (the hang).
    let xcontrol = build_xcontrol(constraints, targets);
    if let Some(ref content) = xcontrol {
        std::fs::write(dir.join("xcontrol"), content)?;
    }
    let uhf = multiplicity - 1; // unpaired electrons = 2S = mult − 1

    // A cancel that landed during setup: bail before spawning.
    if cancelled.load(Ordering::SeqCst) {
        return Err(AppError::Backend("xtb optimization cancelled".into()));
    }

    let stdout = std::fs::File::create(dir.join("xtb.out"))?;
    let stderr = stdout.try_clone()?;
    let mut cmd = Command::new(path);
    cmd.current_dir(dir)
        .args(xtb_args(xcontrol.is_some(), charge, uhf))
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
    let pgid = child.id() as i32; // process_group(0) → pgid == child pid; kept local
                                  // (the poll loop below does the killpg + cwd sweep)

    // Poll for exit / cancel / timeout. Between checks, stream progress: read the
    // bounded tail of xtb.out (~once a second) and emit the current opt cycle, so
    // the panel shows live movement instead of five minutes of silence.
    let deadline = start + Duration::from_secs(timeout_secs);
    let mut last_progress = Instant::now();
    let mut last_cycle_seen: Option<u32> = None;
    let exit_status: std::process::ExitStatus = loop {
        if let Some(status) = child.try_wait()? {
            // The exit code is captured for DIAGNOSTICS, never gated on: measured,
            // xtb 6.6.1 exits 0 on a non-converged optimization (`--cycles 2`) AND
            // can exit non-zero on a clean run whose teardown trips an FP-exception
            // signal. Completion is decided on RESULTS below (classify_completion).
            break status;
        }
        if cancelled.load(Ordering::SeqCst) {
            terminate_job(pgid, dir);
            return Err(AppError::Backend("xtb optimization cancelled".into()));
        }
        if Instant::now() > deadline {
            terminate_job(pgid, dir);
            return Err(AppError::Backend(format!("xtb timed out after {timeout_secs}s")));
        }
        if last_progress.elapsed() >= Duration::from_millis(1000) {
            last_progress = Instant::now();
            let lines = crate::local_backend::read_tail_lines(&dir.join("xtb.out"), 40)
                .unwrap_or_default();
            if let Some(cycle) = last_cycle(&lines) {
                if Some(cycle) != last_cycle_seen {
                    last_cycle_seen = Some(cycle);
                    let _ = app.emit("xtb:progress", XtbProgressPayload { cycle });
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    // Completion post-condition, anchored on RESULTS (the 2b-hotfix — see the block
    // above `classify_completion`). Scan the WHOLE (size-capped) log for the
    // measured non-convergence marker, and require a present + parseable optimized
    // geometry. The exit code and "normal termination" are diagnostics, not gates.
    let out_text = read_out_capped(&dir.join("xtb.out"))?;
    let opt = std::fs::read_to_string(dir.join("xtbopt.xyz")).ok();
    let parsed = opt.as_deref().and_then(|t| parse_xyz(t).ok());
    match classify_completion(&out_text, parsed.is_some()) {
        XtbCompletion::Ok => {}
        XtbCompletion::NonConvergence { line, iterations } => {
            let iters = iterations
                .map(|n| format!(" in {n} iterations"))
                .unwrap_or_default();
            // Artifacts are KEPT (a genuine error → keep_dir_for_diagnostics), so the
            // user can inspect the non-optimized geometry; it is NOT applied.
            return Err(AppError::Backend(format!(
                "xtb failed to converge the geometry optimization{iters}: \"{line}\". \
                 The written geometry is NOT optimized, so it was not applied. {}",
                completion_diagnostics(&out_text, exit_status)
            )));
        }
        XtbCompletion::NoGeometry => {
            return Err(AppError::Backend(format!(
                "xtb produced no readable optimized geometry (xtbopt.xyz). {}",
                completion_diagnostics(&out_text, exit_status)
            )));
        }
    }
    // `Ok` ⇒ geometry present and parsed.
    let opt = opt.expect("classify_completion Ok ⇒ xtbopt.xyz present");
    let (elements_out, positions_out) = parsed.expect("classify_completion Ok ⇒ geometry parsed");

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

/// Last `n` lines of a file (for an error message / progress). Reads only a
/// bounded **tail** from the end (domain rule #5) by reusing the ORCA log tailer
/// `local_backend::read_tail_lines` — one copy, not a second file reader. Empty if
/// unreadable.
fn tail(path: &Path, n: usize) -> String {
    crate::local_backend::read_tail_lines(path, n)
        .unwrap_or_default()
        .join("\n")
}

/// Whether to KEEP the scratch dir for diagnostics after a run. Rule #3 is about
/// clearing ORCA-style scratch litter on SUCCESS, not throwing away the evidence
/// on FAILURE — so keep it only on a genuine failure (an error that is NOT a user
/// cancel; a cancel is not a diagnostic case). Success and cancel remove it.
fn keep_dir_for_diagnostics(succeeded: bool, cancelled: bool) -> bool {
    !succeeded && !cancelled
}

/// How many kept diagnostic dirs to retain under `<data>/xtb/`. The rest are pruned
/// at startup — closing the 2.5.5-fix-2 "kept dirs accumulate" issue without a
/// setting (a small fixed window is enough to debug the last few failures).
const KEEP_DIAGNOSTIC_DIRS: usize = 5;

/// Choose which diagnostic dirs to prune: everything EXCEPT the `keep`
/// most-recently-modified. Pure + testable. The newest are kept, so a run that
/// just failed (its dir is the newest) is never pruned by the next startup.
fn dirs_to_prune(mut entries: Vec<(PathBuf, SystemTime)>, keep: usize) -> Vec<PathBuf> {
    entries.sort_by(|a, b| b.1.cmp(&a.1)); // newest first
    entries.into_iter().skip(keep).map(|(p, _)| p).collect()
}

/// At startup, remove all but the `KEEP_DIAGNOSTIC_DIRS` newest dirs under
/// `<data>/xtb/`. Best-effort: an unreadable root / entry is skipped, never fatal.
pub fn prune_diagnostic_dirs(data_dir: &Path) {
    let root = data_dir.join("xtb");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    let mut dirs: Vec<(PathBuf, SystemTime)> = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        if !path.is_dir() {
            continue;
        }
        let mtime = e
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        dirs.push((path, mtime));
    }
    for p in dirs_to_prune(dirs, KEEP_DIAGNOSTIC_DIRS) {
        let _ = std::fs::remove_dir_all(&p);
    }
}

/// The highest optimization cycle number visible in the tail lines, if any. xtb
/// prints `.......... CYCLE    N ..........` per geometry step.
fn last_cycle(lines: &[String]) -> Option<u32> {
    lines.iter().rev().find_map(|l| {
        let after = l.split("CYCLE").nth(1)?;
        after.split_whitespace().next()?.parse::<u32>().ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0-based scene indices for a constraint's `atoms` array (test ergonomics — the
    /// production path deserializes these through `#[serde(transparent)]`).
    fn si<const N: usize>(a: [usize; N]) -> [SceneIndex; N] {
        a.map(SceneIndex::new)
    }

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
            atoms: si([0, 1]),
            value: Some(1.234),
        }];
        let p = xyz_positions();
        let targets = resolve_targets(&cs, &p).unwrap();
        let xc = build_xcontrol(&cs, &targets).unwrap();
        assert!(xc.contains("$constrain"));
        assert!(xc.contains("force constant=1"));
        assert!(xc.contains("distance: 1, 2, 1.234000"), "got:\n{xc}");
    }

    #[test]
    fn freeze_as_is_resolves_current_value() {
        let cs = vec![Constraint::Distance {
            atoms: si([0, 1]),
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
                atoms: si([0, 1, 2]),
                value: Some(109.0),
            },
            Constraint::Dihedral {
                atoms: si([0, 1, 2, 3]),
                value: Some(90.0),
            },
        ];
        let targets = resolve_targets(&cs, &p).unwrap();
        let xc = build_xcontrol(&cs, &targets).unwrap();
        assert!(xc.contains("angle: 1, 2, 3, 109.0000"), "got:\n{xc}");
        assert!(xc.contains("dihedral: 1, 2, 3, 4, 90.0000"), "got:\n{xc}");
    }

    #[test]
    fn cartesian_becomes_a_fix_block() {
        let cs = vec![Constraint::Cartesian { atoms: si([4]) }];
        let p = xyz_positions();
        let targets = resolve_targets(&cs, &p).unwrap();
        let xc = build_xcontrol(&cs, &targets).unwrap();
        assert!(xc.contains("$fix"), "got:\n{xc}");
        assert!(xc.contains("atoms: 5"), "got:\n{xc}"); // 0-based 4 → 1-based 5
    }

    #[test]
    fn out_of_range_index_is_rejected_before_xtb() {
        let cs = vec![Constraint::Distance {
            atoms: si([0, 99]),
            value: None,
        }];
        let p = xyz_positions();
        assert!(resolve_targets(&cs, &p).is_err());
    }

    #[test]
    fn check_held_flags_a_drifted_constraint() {
        let cs = vec![Constraint::Distance {
            atoms: si([0, 1]),
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
            atoms: si([0, 1]),
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

    // Negative control (d): the 0→1 flip goes through exactly ONE typed conversion.
    // The compile-time half — a bare `usize` / `SceneIndex` cannot be `Display`ed into
    // a line — is enforced by `XtbIndex` being the only index type with `Display`
    // (SceneIndex has none). This source-grep is the observable complement: the writer
    // must carry no bare index flip, and the flip must live only in
    // `to_xtb`. A future edit that reintroduces a bare index flip in the writer fails here.
    #[test]
    fn the_base_flip_is_one_typed_conversion_not_a_bare_plus_one() {
        let src = include_str!("xtb.rs");
        // Needles are built at runtime so THIS test's own source does not self-match.
        let plus = "+";
        let flip = format!("self.0 {plus} 1"); // the one flip, inside to_xtb
        assert_eq!(src.matches(&flip).count(), 1, "the flip must live in to_xtb only");
        let bare = format!("] {plus} 1"); // the pre-1e bare-index-flip pattern
        assert!(!src.contains(&bare), "no bare index flip may remain in the writer");
        assert!(src.contains(".to_xtb()"), "the writer flips via SceneIndex::to_xtb");
    }

    #[test]
    fn to_xtb_is_one_based_and_zero_based_indexes_our_geometry() {
        let s = SceneIndex::new(4);
        assert_eq!(s.zero_based(), 4); // indexes our 0-based geometry
        assert_eq!(s.to_xtb().to_string(), "5"); // 0-based 4 → 1-based 5 for xtb
    }

    #[test]
    fn keep_dir_only_on_a_genuine_failure() {
        // A failure that is NOT a cancel (e.g. a timeout) keeps the dir for
        // diagnostics — the guarantee most easily lost on a refactor.
        assert!(keep_dir_for_diagnostics(false, false));
        // Success removes it.
        assert!(!keep_dir_for_diagnostics(true, false));
        // A user cancel removes it (not a diagnostic case), even though the run
        // ended in error.
        assert!(!keep_dir_for_diagnostics(false, true));
    }

    #[test]
    fn argv_includes_input_only_with_an_xcontrol() {
        // WITHOUT constraints → no `--input` (an empty --input file hangs xtb).
        let no_xc = xtb_args(false, 0, 0);
        assert!(!no_xc.iter().any(|a| a == "--input"), "argv: {no_xc:?}");
        assert!(no_xc.contains(&"--opt".to_string()));
        // WITH constraints → `--input xcontrol` present (guards the regression).
        let with_xc = xtb_args(true, -1, 1);
        let i = with_xc.iter().position(|a| a == "--input").expect("has --input");
        assert_eq!(with_xc[i + 1], "xcontrol");
        assert!(with_xc.windows(2).any(|w| w == ["--chrg", "-1"]));
        assert!(with_xc.windows(2).any(|w| w == ["--uhf", "1"]));
    }

    #[test]
    fn build_xcontrol_is_none_without_constraints() {
        assert!(build_xcontrol(&[], &[]).is_none());
        let cs = vec![Constraint::Distance {
            atoms: si([0, 1]),
            value: Some(1.5),
        }];
        let p = xyz_positions();
        let targets = resolve_targets(&cs, &p).unwrap();
        assert!(build_xcontrol(&cs, &targets).is_some());
    }

    #[test]
    fn prune_keeps_the_newest_and_drops_the_oldest() {
        use std::time::Duration;
        // 6 dirs, times 1..=6; keep 5 → only the oldest (time 1) is pruned.
        let entries: Vec<(PathBuf, SystemTime)> = (1..=6)
            .map(|t| {
                (
                    PathBuf::from(format!("/x/{t}")),
                    SystemTime::UNIX_EPOCH + Duration::from_secs(t),
                )
            })
            .collect();
        let pruned = dirs_to_prune(entries, 5);
        assert_eq!(pruned, vec![PathBuf::from("/x/1")]);
        // The newest (a just-failed run's dir) is never pruned.
        assert!(!pruned.contains(&PathBuf::from("/x/6")));
    }

    #[test]
    fn last_cycle_reads_the_highest_xtb_cycle_marker() {
        let lines: Vec<String> = [
            ".............................. CYCLE    1 ..............................",
            "  ... energy ...",
            ".............................. CYCLE    2 ..............................",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(last_cycle(&lines), Some(2));
        assert_eq!(last_cycle(&["no cycles here".to_string()]), None);
    }

    // ── Completion post-condition — on the THREE real fixtures (2b-hotfix) ────────
    // Real xtb 6.6.1 (`builduser@buildhost`) logs, not synthesized:
    //  - SUCCESS: the author's dexketoprofen+BH₄ pre-opt (geometry good, but
    //    `normal termination` sits 41 lines from the end — the false-negative case);
    //  - FAIL: the same input re-run with `--cycles 2` (non-convergence; xtb still
    //    wrote xtbopt.xyz + .xtboptok and printed `normal termination`, exit 0).

    const SUCCESS_OUT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/xtb_success_dexketoprofen_bh4.out"
    ));
    const FAIL_OUT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/xtb_fail_cycles2.out"
    ));

    /// The OLD gate, reproduced: `normal termination` present in a 30-line tail.
    fn old_gate_passes(out: &str) -> bool {
        let lines: Vec<&str> = out.lines().collect();
        let start = lines.len().saturating_sub(30);
        lines[start..].join("\n").contains("normal termination")
    }

    #[test]
    fn classify_success_on_the_real_clean_run() {
        // Geometry present + no FAILED marker → Ok, even though `normal termination`
        // is buried past a 30-line tail and the exit code is not consulted.
        assert_eq!(classify_completion(SUCCESS_OUT, true), XtbCompletion::Ok);
        // Sanity on the fixture itself.
        assert!(SUCCESS_OUT.contains(NORMAL_TERMINATION));
        assert!(!SUCCESS_OUT.contains(FAILED_TO_CONVERGE));
    }

    #[test]
    fn classify_non_convergence_on_the_real_cycles2_run() {
        // xtb WROTE a geometry (geometry_ok = true) but FAILED to converge → the
        // classifier rejects it and quotes the measured line + iteration count.
        match classify_completion(FAIL_OUT, true) {
            XtbCompletion::NonConvergence { line, iterations } => {
                assert!(line.contains(FAILED_TO_CONVERGE), "quoted: {line}");
                assert_eq!(iterations, Some(2));
            }
            other => panic!("expected NonConvergence, got {other:?}"),
        }
    }

    #[test]
    fn classify_no_geometry_when_xtbopt_absent() {
        // No FAILED marker but no parseable geometry → NoGeometry (xtb died before
        // writing / wrote garbage).
        assert_eq!(classify_completion(SUCCESS_OUT, false), XtbCompletion::NoGeometry);
    }

    #[test]
    fn parse_failed_iterations_reads_n() {
        assert_eq!(
            parse_failed_iterations("*** FAILED TO CONVERGE GEOMETRY OPTIMIZATION IN 2 ITERATIONS ***"),
            Some(2)
        );
        assert_eq!(parse_failed_iterations("no number here"), None);
    }

    // NEGATIVE CONTROL (a): the author's real successful run through the OLD gate
    // reproduces the FALSE NEGATIVE — documenting exactly why the gate was replaced.
    // (`normal termination` is 41 lines from the end, so a 30-line tail misses it.)
    #[test]
    fn old_gate_false_negatives_on_the_real_clean_run() {
        assert!(
            !old_gate_passes(SUCCESS_OUT),
            "the old 30-line-tail gate MISSES `normal termination` (it's buried) → \
             a good geometry was rejected"
        );
        // The new post-condition gets it right on the very same log.
        assert_eq!(classify_completion(SUCCESS_OUT, true), XtbCompletion::Ok);
    }

    // NEGATIVE CONTROL (b): a post-condition WITHOUT the FAILED scan would accept the
    // non-converged geometry — green for the wrong reason. This asserts the real
    // classifier catches it; removing the FAILED check turns this test red.
    #[test]
    fn a_gate_without_the_failed_scan_would_accept_the_non_converged_run() {
        // The "broken" post-condition = geometry-present only (no FAILED scan).
        let broken_accepts = /* xtbopt.xyz present */ true;
        assert!(
            broken_accepts,
            "a geometry-only gate would ACCEPT the non-converged run (the latent false positive)"
        );
        // The real classifier REJECTS it — this is the line that reddens if the
        // non-convergence check is ever removed.
        assert!(
            matches!(classify_completion(FAIL_OUT, true), XtbCompletion::NonConvergence { .. }),
            "the real post-condition must reject a non-converged geometry"
        );
    }
}
