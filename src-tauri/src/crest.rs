//! CREST/QCG microsolvation — the **grow parse + completion** core (Phase 4.5 Stage F,
//! unit F1a). Pure data: classify a CREST run's completion and read the grown
//! microsolvated cluster (geometry + seed energy + intended charge) from a real `grow/`
//! dir. This is the SEED reader — the cluster is a **geometry seed** for a later ORCA
//! re-opt (F2), **NEVER a solvated result**. The process runner (spawn/events) is F1b, the
//! persistent job + form is F1c; neither lives here.
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

// F1a is the parse+completion CORE; the callers — the process runner (F1b) and the
// persistent job + setup form (F1c) — are the next units. So these public items are not yet
// reached from elsewhere in the crate (only the tests exercise them). Allow dead-code until
// F1b wires the runner, rather than prematurely building a runner just to silence the lint.
#![allow(dead_code)]

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::AppError;
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
}
