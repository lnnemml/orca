//! CREST/QCG microsolvation — the **grow parse + completion** core (Phase 4.5 Stage F,
//! unit F1a). Pure data: classify a CREST run's completion and read the grown
//! microsolvated cluster (geometry + seed energy + intended charge) from a real `grow/`
//! dir. This is the SEED reader — the cluster is a **geometry seed** for a later ORCA
//! re-opt (F2), **NEVER a solvated result**. F1b adds the **ephemeral runner** below
//! (spawn QCG grow off-thread, parse, emit `crest:done`/`crest:error`); the persistent job +
//! form is F1c. **K3: nothing here persists** — the runner is a helper (seconds, like
//! `XtbRunner`); the persisted artifact is the F2 ORCA re-opt job, not this grow.
//!
//! Mirrors the external-tool discipline of `crate::xtb` (completion classified from the
//! log, not trusted from an exit code) — but the sentinels are CREST's, measured, not
//! xtb's. See `wiki/orca/crest.md` (the QCG probe of record) and
//! `wiki/modules/crest-microsolvation.md`.
//!
//! ## Two measured facts this module encodes (rule #10, do not re-derive per run)
//! 1. **Completion sentinel is `CREST terminated normally.`** in `crest.out` — NOT
//!    `normal termination of xtb` (that is xtb *stderr*, which CREST does not keep;
//!    `wiki/orca/xtb.md`). Ok also requires the grown `cluster.xyz` present.
//! 2. **The cluster's `energy:` comment is an xtb-ALPB energy of the GROWN cluster**, and
//!    QCG grows an ion's cluster **NEUTRAL** (four evidences, `crest.md`). So for a nonzero
//!    intended charge the seed energy is the WRONG species' energy — hence `seed_energy_eh`
//!    (never "solvated"/"final"), and `intended_charge` is surfaced from `crest.out` (the
//!    solute's charge) so a later step can warn on a wrong-charge seed. This unit computes
//!    **no** solvation energy and derives **no** charge from the cluster geometry.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::local_backend::terminate_job;
use crate::parse::xyz::XyzFile;
use crate::results::FinalGeometry;

/// CREST's own success line (stdout, `crest.out`). NOT `normal termination of xtb`.
const CREST_SENTINEL: &str = "CREST terminated normally.";

/// The outcome of a CREST QCG-grow run, classified from `crest.out` + whether the grown
/// `cluster.xyz` is present. **Pure** — the classifier the tests run on the real fixtures.
#[derive(Debug, PartialEq, Eq)]
pub enum CrestCompletion {
    /// `CREST terminated normally.` seen AND the grown cluster geometry is present.
    Ok,
    /// The sentinel is present but the grown `cluster.xyz` is not — CREST reported success
    /// yet produced no cluster (nothing to seed from). Refuse, don't fabricate.
    NoCluster,
    /// The sentinel is absent — CREST did not terminate normally (crash / kill / MTD-fail).
    Failed,
}

/// Classify a CREST run from `crest_out` (the whole `crest.out`) and whether the grown
/// `cluster.xyz` exists. Ok requires the **CREST** sentinel AND the cluster — the sentinel
/// alone is not success (a "terminated normally, no cluster" is a real, refused state).
pub fn classify_crest_completion(crest_out: &str, cluster_xyz_present: bool) -> CrestCompletion {
    if !crest_out.contains(CREST_SENTINEL) {
        return CrestCompletion::Failed;
    }
    if !cluster_xyz_present {
        return CrestCompletion::NoCluster;
    }
    CrestCompletion::Ok
}

/// Parse the energy (Eh) out of a CREST cluster comment line of the form
/// `energy: <Eh> gnorm: <val> xtb: 6.6.1 (unknown)`. **CREST-specific** — the token is
/// `energy:` (with the colon). Deliberately does NOT match ORCA's `E <Eh>` comment form
/// (`XyzFile::first_frame_energy` owns that): two formats, one parser each, never
/// cross-matched. `None` when there is no `energy:` token or the value does not parse.
pub fn parse_crest_energy_comment(comment: &str) -> Option<f64> {
    let toks: Vec<&str> = comment.split_whitespace().collect();
    let pos = toks.iter().position(|&t| t == "energy:")?;
    toks.get(pos + 1)?.parse().ok()
}

/// The intended (solute) molecular charge from `crest.out`'s `Molecular charge : <n>`
/// line — the charge CREST was *told*, which for the grow phase is applied only to the
/// solute monomer preopt (QCG grows the cluster NEUTRAL regardless; `crest.md`). `None`
/// when the line is absent. This is the solute's charge, NEVER derived from the cluster.
fn parse_intended_charge(crest_out: &str) -> Option<i32> {
    let line = crest_out.lines().find(|l| l.contains("Molecular charge"))?;
    line.rsplit(':').next()?.trim().parse().ok()
}

/// The grown microsolvated cluster — a **geometry SEED** for a later ORCA re-opt, not a
/// solvated result. `seed_energy_eh` is named to make that self-documenting: it is the
/// xtb-ALPB energy of the grown cluster (of the NEUTRAL species for an ion), never a
/// solvated/final energy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrestGrowResult {
    /// The grown cluster geometry (solute + n solvent), Å.
    pub cluster: FinalGeometry,
    /// The cluster's `energy:` comment energy (Eh) — xtb-level SEED energy, or `None`.
    pub seed_energy_eh: Option<f64>,
    /// The intended (solute) charge from `crest.out` — nonzero ⇒ the grown cluster is the
    /// NEUTRAL species (a wrong-charge seed a later step must warn on). `None` if absent.
    pub intended_charge: Option<i32>,
    /// Atom count of the grown cluster (a quick cross-check for the caller).
    pub n_atoms: usize,
}

/// Read the grown cluster from a CREST `grow/` dir: geometry + seed energy from
/// `cluster_optimized.xyz`, intended charge from `crest_out`. `Ok(None)` — honest absence —
/// when `cluster_optimized.xyz` is absent (a grow that produced no optimized cluster).
/// Reads the small file whole via the shared `xyz` reader (rule #5); coordinates are Å.
/// **Computes no solvation energy and no charge from the geometry** (F1a scope).
pub fn parse_crest_grow(
    grow_dir: &Path,
    crest_out: &str,
) -> Result<Option<CrestGrowResult>, AppError> {
    let path = grow_dir.join("cluster_optimized.xyz");
    if !path.exists() {
        return Ok(None); // no optimized cluster → nothing to seed from (honest absence)
    }
    let xyz = XyzFile::from_path(&path)?;
    let (elements, xyz_angstrom) = xyz.first_frame().ok_or_else(|| {
        AppError::Backend(format!("CREST cluster geometry ({}) is empty", path.display()))
    })?;
    let n_atoms = elements.len();
    // The seed energy is the cluster comment's `energy:` value (CREST form) — NOT the ORCA
    // `E` form. Absent/unparseable comment → None (the geometry seed still stands).
    let seed_energy_eh = xyz.first_frame_comment().and_then(parse_crest_energy_comment);
    let intended_charge = parse_intended_charge(crest_out);
    Ok(Some(CrestGrowResult {
        cluster: FinalGeometry { elements, xyz_angstrom },
        seed_energy_eh,
        intended_charge,
        n_atoms,
    }))
}

// ── The grow invocation (F1b) — the arg vector + the growth-table parse ─────────────

/// Everything the runner needs to build a QCG-grow invocation. `charge`/`uhf` are the
/// SOLUTE's (the caller's); `solvent_name` is the ALPB solvent (e.g. "water"), which is
/// ALSO the solvent-monomer identity written to `solvent.xyz`. `fix_solute` = `-fixsolute`
/// (rigid solutes; auto for water) vs `-nofix` (needed for water).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrestGrowOpts {
    pub solvent_name: String,
    pub nsolv: u32,
    pub charge: i32,
    pub uhf: i32,
    pub fix_solute: bool,
    pub threads: u32,
}

/// Build the CREST arg vector that FOLLOWS `crest solute.xyz` (the runner writes the fixed
/// `solute.xyz` / `solvent.xyz`). Stable order:
/// `-qcg solvent.xyz -grow -nsolv <n> -alpb <solvent> [-chrg <c>] [-uhf <u>]
///  (-fixsolute|-nofix) -T <threads>`.
///
/// **Two invariants (measured, `wiki/orca/crest.md`):** ALWAYS `-grow`, **NEVER
/// `-ensemble`** (reproducibly segfaults on the ionic system), and **no `-keepdir`** in
/// production (probe-only). `-chrg`/`-uhf` are emitted ONLY when nonzero — matching the two
/// probed invocations (the neutral rung omits `-chrg`, the anion passes `-chrg -1`).
pub fn build_crest_args(opts: &CrestGrowOpts) -> Vec<String> {
    let mut args = vec![
        "-qcg".into(),
        "solvent.xyz".into(),
        "-grow".into(),
        "-nsolv".into(),
        opts.nsolv.to_string(),
        "-alpb".into(),
        opts.solvent_name.clone(),
    ];
    if opts.charge != 0 {
        args.push("-chrg".into());
        args.push(opts.charge.to_string());
    }
    if opts.uhf != 0 {
        args.push("-uhf".into());
        args.push(opts.uhf.to_string());
    }
    args.push(if opts.fix_solute { "-fixsolute".into() } else { "-nofix".into() });
    args.push("-T".into());
    args.push(opts.threads.to_string());
    args
}

/// One row of `grow/qcg_energy.dat`: the cluster size (# solvent added), its total energy
/// (Eh), and the ΔEtot column. **Display-only** growth energetics (the runner surfaces it
/// alongside the seed); it is NOT the seed energy and NOT a solvated result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QcgGrowthPoint {
    pub size: usize,
    pub energy_eh: f64,
    pub delta: f64,
}

/// Parse `grow/qcg_energy.dat`'s whitespace rows `<size> <E(Eh)> <ΔEtot>` in file order.
/// A line that does not yield three parseable columns (blank / header / garbage) is skipped
/// — this is a display table, not an authoritative artifact, so a ragged line is dropped,
/// never fatal.
pub fn parse_qcg_energy(dat: &str) -> Vec<QcgGrowthPoint> {
    dat.lines()
        .filter_map(|line| {
            let toks: Vec<&str> = line.split_whitespace().collect();
            if toks.len() < 3 {
                return None;
            }
            Some(QcgGrowthPoint {
                size: toks[0].parse().ok()?,
                energy_eh: toks[1].parse().ok()?,
                delta: toks[2].parse().ok()?,
            })
        })
        .collect()
}

// ── The ephemeral runner (F1b) — MIRRORS `crate::xtb::XtbRunner` ─────────────────────
//
// K3: no persistence, no jobs row — a CREST grow is a helper (seconds), like an xtb
// pre-opt. The grown cluster is returned as an EVENT (`crest:done`); the persisted artifact
// is the F2 ORCA re-opt. The runner runs OFF the main thread (the whole 2.5.5 lesson — a
// synchronous long command freezes the GTK/WebKit window AND blocks `crest_cancel`).

/// A generous ceiling — `-grow` is ~10 s (measured), but a pathological run must not hang
/// the single slot forever. NOT the ensemble path (never run — it segfaults, `crest.md`).
const CREST_TIMEOUT_SECS: u64 = 600;

/// The reserved slot for the one in-flight grow. Just the cancel flag: the worker thread
/// holds the pgid + dir locally and does the killpg/sweep when it sees the flag.
struct CrestRun {
    cancelled: Arc<AtomicBool>,
}

/// Single-slot runner: CREST grow is a helper, at most one at a time (mirrors `XtbRunner`).
#[derive(Default)]
pub struct CrestRunner {
    running: Mutex<Option<CrestRun>>,
}

/// The `crest:done` payload — the F1a seed result + the display growth table. `result`
/// carries `seed_energy_eh` (xtb-level, NEVER relabelled solvated) + `intended_charge`.
#[derive(Debug, Clone, Serialize)]
pub struct CrestGrowDone {
    pub result: CrestGrowResult,
    pub growth: Vec<QcgGrowthPoint>,
}

/// Payload of `crest:error` (the grow failed / was cancelled / timed out). `dir` is the kept
/// diagnostic directory on a genuine failure (not a cancel).
#[derive(Clone, Serialize)]
struct CrestErrorPayload {
    message: String,
    dir: Option<String>,
}

/// The configured CREST binary — full path, never bundled (rule #7). Default
/// `/opt/crest/crest` (the probed install). Reuses `xtb`'s `$PATH` resolver so a bare name
/// still resolves. A user setting `crest_path`, mirroring `xtb_path`.
fn crest_path(db: &State<'_, DbState>) -> Result<String, AppError> {
    let conn = db.lock()?;
    let configured: String = conn
        .query_row("SELECT value FROM settings WHERE key = 'crest_path'", [], |r| r.get(0))
        .unwrap_or_else(|_| "/opt/crest/crest".to_string());
    Ok(crate::xtb::resolve_binary(&configured))
}

/// Cancel the running CREST grow (if any). **Only sets the flag** (must not block the main
/// thread) — the worker's poll loop does the killpg + cwd sweep. Mirrors `xtb_cancel`.
#[tauri::command]
pub fn crest_cancel(runner: State<'_, CrestRunner>) {
    if let Ok(guard) = runner.running.lock() {
        if let Some(run) = guard.as_ref() {
            run.cancelled.store(true, Ordering::SeqCst);
        }
    }
}

/// Start a CREST QCG **grow** (solute + n solvent → a microsolvated cluster), then RETURN.
/// A **starter** mirroring `xtb_optimize`: validate synchronously, reserve the single slot,
/// then move the spawn/poll/parse/cleanup into a `std::thread::spawn`. The cluster arrives
/// on the frontend as a `crest:done` event (or `crest:error`). See `wiki/modules/tauri-core.md`.
#[tauri::command]
pub fn crest_grow(
    app: AppHandle,
    db: State<'_, DbState>,
    runner: State<'_, CrestRunner>,
    solute_xyz: String,
    solvent_xyz: String,
    opts: CrestGrowOpts,
) -> Result<(), AppError> {
    // Validate synchronously so the user gets immediate feedback, not an event a moment later.
    if opts.nsolv < 1 {
        return Err(AppError::Backend("CREST: nsolv must be ≥ 1".into()));
    }
    if opts.uhf < 0 {
        return Err(AppError::Backend("CREST: uhf (unpaired electrons) must be ≥ 0".into()));
    }
    if opts.solvent_name.trim().is_empty() {
        return Err(AppError::Backend("CREST: an ALPB solvent name is required".into()));
    }
    let path = crest_path(&db)?;

    // Reserve the single slot BEFORE returning, so a second click can't start a second run.
    let cancelled = Arc::new(AtomicBool::new(false));
    let data_dir = dirs::data_dir()
        .ok_or_else(|| AppError::Internal("no user data directory".into()))?
        .join("orcastudio");
    let dir = data_dir.join("crest").join(uuid::Uuid::new_v4().to_string());
    {
        let mut g = runner
            .running
            .lock()
            .map_err(|_| AppError::Internal("crest runner mutex poisoned".into()))?;
        if g.is_some() {
            return Err(AppError::Backend("a CREST grow is already running".into()));
        }
        *g = Some(CrestRun { cancelled: cancelled.clone() });
    }

    // Off the main thread — the whole point of the 2.5.5-fix.
    std::thread::spawn(move || {
        let result = run_crest_in_dir(&dir, &path, &cancelled, &solute_xyz, &solvent_xyz, &opts);

        // Cleanup policy (mirrors xtb): SUCCESS → remove the dir (AFTER parsing, which
        // happened inside run_crest_in_dir); CANCEL → remove (not a diagnostic case); any
        // other FAILURE → KEEP the dir (crest.out is the only evidence of where it failed).
        // Freeing the slot is unconditional.
        let was_cancelled = cancelled.load(Ordering::SeqCst);
        let keep = result.is_err() && !was_cancelled;
        if !keep {
            let _ = std::fs::remove_dir_all(&dir);
        }
        if let Some(runner) = app.try_state::<CrestRunner>() {
            if let Ok(mut g) = runner.running.lock() {
                *g = None;
            }
        }

        match result {
            Ok(done) => {
                let _ = app.emit("crest:done", done);
            }
            Err(e) => {
                // Attach the last ~20 lines of crest.out (bounded tail, rule #5) and, when
                // kept, the dir path — surfaced in the UI as copyable text.
                let mut message = e.to_string();
                let log_tail = crate::local_backend::read_tail_lines(&dir.join("crest.out"), 20)
                    .unwrap_or_default()
                    .join("\n");
                if !log_tail.trim().is_empty() {
                    message = format!("{message}\n\n— last lines of crest.out —\n{log_tail}");
                }
                let dir_str = keep.then(|| dir.display().to_string());
                if let Some(ref d) = dir_str {
                    message = format!("{message}\n\nDiagnostic files kept at:\n{d}");
                }
                let _ = app.emit("crest:error", CrestErrorPayload { message, dir: dir_str });
            }
        }
    });
    Ok(())
}

/// The actual work (off-thread): isolated dir → write `solute.xyz`/`solvent.xyz` → spawn
/// `crest solute.xyz <build_crest_args>` with cwd = the dir, stdout+stderr → `crest.out` →
/// poll for exit/cancel/timeout → classify from crest.out + `grow/cluster.xyz` presence →
/// **parse the grown cluster BEFORE returning** (the caller removes the dir on success only
/// after this returns, so the cluster is read first). Never trusts the exit code (mirrors
/// xtb): completion is classified from the log + the artifact.
fn run_crest_in_dir(
    dir: &Path,
    path: &str,
    cancelled: &Arc<AtomicBool>,
    solute_xyz: &str,
    solvent_xyz: &str,
    opts: &CrestGrowOpts,
) -> Result<CrestGrowDone, AppError> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("solute.xyz"), solute_xyz)?;
    std::fs::write(dir.join("solvent.xyz"), solvent_xyz)?;

    // A cancel that landed during setup: bail before spawning.
    if cancelled.load(Ordering::SeqCst) {
        return Err(AppError::Backend("CREST grow cancelled".into()));
    }

    let stdout = std::fs::File::create(dir.join("crest.out"))?;
    let stderr = stdout.try_clone()?;
    let mut cmd = Command::new(path);
    cmd.current_dir(dir)
        .arg("solute.xyz")
        .args(build_crest_args(opts))
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // own group → cancel can killpg the tree (debugging/004)
    }

    let start = Instant::now();
    let mut child = cmd
        .spawn()
        .map_err(|e| AppError::Backend(format!("could not spawn crest at '{path}': {e}")))?;
    let pgid = child.id() as i32; // process_group(0) → pgid == child pid; kept local

    // Poll for exit / cancel / timeout. CREST's exit code IS reliable here (`crest.md`), but
    // completion is still decided on the RESULTS below (sentinel + cluster), never the code.
    let deadline = start + Duration::from_secs(CREST_TIMEOUT_SECS);
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if cancelled.load(Ordering::SeqCst) {
            terminate_job(pgid, dir);
            return Err(AppError::Backend("CREST grow cancelled".into()));
        }
        if Instant::now() > deadline {
            terminate_job(pgid, dir);
            return Err(AppError::Backend(format!(
                "CREST grow timed out after {CREST_TIMEOUT_SECS}s"
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // Completion classified from crest.out + the grown cluster's presence (F1a) — the
    // sentinel `CREST terminated normally.` AND `grow/cluster.xyz`, never the exit code.
    let crest_out = std::fs::read_to_string(dir.join("crest.out")).unwrap_or_default();
    let grow_dir = dir.join("grow");
    let cluster_present = grow_dir.join("cluster.xyz").is_file();
    match classify_crest_completion(&crest_out, cluster_present) {
        CrestCompletion::Ok => {}
        CrestCompletion::NoCluster => {
            return Err(AppError::Backend(
                "CREST terminated normally but produced no grow/cluster.xyz — nothing to seed from"
                    .into(),
            ));
        }
        CrestCompletion::Failed => {
            return Err(AppError::Backend(
                "CREST did not terminate normally (no `CREST terminated normally.` in crest.out)"
                    .into(),
            ));
        }
    }

    // Parse the grown cluster + the growth table BEFORE the caller's cleanup (success removes
    // the dir only after this returns). The result carries the xtb-level SEED energy +
    // intended charge — never relabelled as a solvated result.
    let result = parse_crest_grow(&grow_dir, &crest_out)?.ok_or_else(|| {
        AppError::Backend(
            "CREST grow: cluster_optimized.xyz missing after a normal termination".into(),
        )
    })?;
    let growth = std::fs::read_to_string(grow_dir.join("qcg_energy.dat"))
        .map(|d| parse_qcg_energy(&d))
        .unwrap_or_default();
    Ok(CrestGrowDone { result, growth })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn crest_completion_ok_needs_sentinel_and_cluster() {
        let out_ok = "…\n   CREST terminated normally.\n";
        assert_eq!(classify_crest_completion(out_ok, true), CrestCompletion::Ok);
        // Sentinel but no cluster → NoCluster (terminated normally, nothing to seed from).
        assert_eq!(classify_crest_completion(out_ok, false), CrestCompletion::NoCluster);
        // BITE: the xtb STDERR string is NOT the CREST sentinel — a gate on it would be wrong
        // (crest.md: xtb's `normal termination` is stderr, which CREST does not keep).
        assert_eq!(
            classify_crest_completion("normal termination of xtb\n", true),
            CrestCompletion::Failed
        );
        assert_eq!(classify_crest_completion("", true), CrestCompletion::Failed);
    }

    #[test]
    fn crest_energy_comment_parses_energy_colon_form() {
        let c = " energy: -41.452349356509 gnorm: 0.000785273392 xtb: 6.6.1 (unknown)";
        assert!((parse_crest_energy_comment(c).unwrap() - (-41.452349356509)).abs() < 1e-9);
        // BITE: the ORCA `E <val>` comment form must NOT match the CREST parser (one home
        // each) — else the two conventions cross-contaminate.
        assert!(parse_crest_energy_comment("Coordinates from ORCA-job input E -76.4").is_none());
        assert!(parse_crest_energy_comment("gnorm: 0.1 xtb: 6.6.1").is_none());
    }

    #[test]
    fn parse_neutral_grow_rung0() {
        let dir = fixture("crest_grow_neutral");
        let out = std::fs::read_to_string(dir.join("crest.out")).unwrap();
        let r = parse_crest_grow(&dir, &out).unwrap().expect("neutral grow parses");
        // benzoic acid (15) + 3×H₂O (9) = 24 atoms; the seed energy is the cluster comment's.
        assert_eq!(r.n_atoms, 24);
        assert_eq!(r.cluster.elements.len(), 24);
        assert_eq!(r.cluster.xyz_angstrom.len(), 24);
        assert!((r.seed_energy_eh.unwrap() - (-41.452349)).abs() < 1e-4, "{:?}", r.seed_energy_eh);
        // Charge-clean case: intended charge 0, surfaced from crest.out.
        assert_eq!(r.intended_charge, Some(0));
    }

    #[test]
    fn parse_anion_grow_rung1() {
        let dir = fixture("crest_grow_anion");
        let out = std::fs::read_to_string(dir.join("crest.out")).unwrap();
        let r = parse_crest_grow(&dir, &out).unwrap().expect("anion grow parses");
        // BH₄⁻ (5) + 3×CH₃OH (18) = 23 atoms.
        assert_eq!(r.n_atoms, 23);
        assert!((r.seed_energy_eh.unwrap() - (-27.915061)).abs() < 1e-4, "{:?}", r.seed_energy_eh);
        // BITE: the nonzero INTENDED charge (−1) is surfaced from crest.out (the solute's) —
        // it drives the F1b wrong-charge-seed warning. The parser derives NO charge from the
        // cluster geometry, and the seed energy is NOT treated as a solvated result.
        assert_eq!(r.intended_charge, Some(-1));
    }

    #[test]
    fn parse_crest_grow_none_when_no_cluster() {
        // Honest absence: a real dir with no `cluster_optimized.xyz` → Ok(None), never a
        // fabricated geometry.
        let empty = std::env::temp_dir();
        assert!(parse_crest_grow(&empty, "CREST terminated normally.\n").unwrap().is_none());
    }

    fn opts(charge: i32, uhf: i32, fix_solute: bool) -> CrestGrowOpts {
        CrestGrowOpts {
            solvent_name: "water".into(),
            nsolv: 3,
            charge,
            uhf,
            fix_solute,
            threads: 4,
        }
    }

    #[test]
    fn crest_args_grow_never_ensemble() {
        // BITE: -grow present, -ensemble NEVER (the measured segfault mode, crest.md).
        let args = build_crest_args(&opts(0, 0, false));
        assert!(args.iter().any(|a| a == "-grow"));
        assert!(!args.iter().any(|a| a == "-ensemble"));
    }

    #[test]
    fn crest_args_chrg_only_when_nonzero() {
        // BITE: matches the two probed invocations — neutral omits -chrg, the anion passes it.
        let neutral = build_crest_args(&opts(0, 0, false));
        assert!(!neutral.iter().any(|a| a == "-chrg"));
        let anion = build_crest_args(&opts(-1, 0, false));
        let i = anion.iter().position(|a| a == "-chrg").expect("-chrg present for the anion");
        assert_eq!(anion[i + 1], "-1");
    }

    #[test]
    fn crest_args_water_nofix_else_fixsolute() {
        // BITE: -nofix (needed for water) vs -fixsolute (rigid solutes) — exactly one.
        let nofix = build_crest_args(&opts(0, 0, false));
        assert!(nofix.iter().any(|a| a == "-nofix"));
        assert!(!nofix.iter().any(|a| a == "-fixsolute"));
        let fixed = build_crest_args(&opts(0, 0, true));
        assert!(fixed.iter().any(|a| a == "-fixsolute"));
        assert!(!fixed.iter().any(|a| a == "-nofix"));
    }

    #[test]
    fn crest_args_carry_qcg_alpb_nsolv() {
        // BITE: the QCG/solvent/count flags are present; -keepdir is ABSENT (production clean).
        let args = build_crest_args(&opts(0, 0, false));
        let has_pair = |k: &str, v: &str| {
            args.iter().position(|a| a == k).is_some_and(|i| args.get(i + 1).map(String::as_str) == Some(v))
        };
        assert!(has_pair("-qcg", "solvent.xyz"));
        assert!(has_pair("-alpb", "water"));
        assert!(has_pair("-nsolv", "3"));
        assert!(!args.iter().any(|a| a == "-keepdir"));
    }

    #[test]
    fn parse_qcg_energy_rung0() {
        // BITE: the real neutral fixture — 4 growth points, sizes 0..3, total energies falling.
        let dat = std::fs::read_to_string(fixture("crest_grow_neutral").join("qcg_energy.dat")).unwrap();
        let pts = parse_qcg_energy(&dat);
        assert_eq!(pts.len(), 4);
        assert_eq!(pts.iter().map(|p| p.size).collect::<Vec<_>>(), [0, 1, 2, 3]);
        let e = |i: usize| pts[i].energy_eh;
        assert!((e(0) - (-26.173)).abs() < 1e-2, "{}", e(0));
        assert!((e(1) - (-31.248)).abs() < 1e-2, "{}", e(1));
        assert!((e(2) - (-36.329)).abs() < 1e-2, "{}", e(2));
        assert!((e(3) - (-41.406)).abs() < 1e-2, "{}", e(3));
    }
}
