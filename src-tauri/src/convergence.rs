//! Incremental convergence parser for the live dashboard.
//!
//! Fed the **same** stdout stream `local_backend::drive_job` already tails, one
//! line at a time — the output file is never re-read while a job runs (domain
//! rule #5). Two things are extracted for the learning-instrument dashboard:
//!
//!   * **SCF iterations** — energy + ΔE per iteration, for the SCF progress line
//!     and (indirectly) how many cycles a step took.
//!   * **Geometry optimization steps** — per-cycle energy + the convergence
//!     criteria (gradient/step vs their tolerances) that drive the plots.
//!
//! ORCA's SCF table shape differs between algorithms (DIIS / SOSCF / TRAH) and
//! between versions, so the row parse is **tolerant**: a line is an SCF
//! iteration iff it starts with an integer, its second token is a negative
//! decimal (the energy), and it has ≥3 numeric fields. Anything else is silently
//! skipped. Crucially, SCF-row parsing only runs **inside** an SCF table (gated
//! by the `Iteration ... Energy (Eh)` column header): Freq output prints
//! normal-mode eigenvector rows that match the row shape exactly but are not SCF
//! data — the gate is what keeps them out.

use serde::Serialize;

use crate::result_extraction::extract_final_energy;

/// One SCF iteration.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ScfPoint {
    /// Optimization cycle this SCF belongs to; `0` for a single point.
    pub cycle: u32,
    pub iter: u32,
    pub energy: f64,
    pub delta_e: f64,
}

/// One geometry-convergence criterion (a row of the `|Geometry convergence|`
/// table): its current value, ORCA's tolerance for it, and whether it's met.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Criterion {
    pub name: String,
    pub value: f64,
    pub tolerance: f64,
    pub converged: bool,
}

/// One geometry optimization step: its cycle, the cycle's final energy (if seen
/// yet), and the convergence criteria emitted at the end of the cycle.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct OptPoint {
    pub cycle: u32,
    pub energy: Option<f64>,
    pub criteria: Vec<Criterion>,
}

/// A datapoint completed by a fed line. Internally tagged so the frontend can
/// discriminate on `kind` (`scf` / `opt`) with the payload flattened alongside.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ConvergenceEvent {
    Scf(ScfPoint),
    Opt(OptPoint),
}

/// Incremental line-by-line parser. One instance per running job.
#[derive(Default)]
pub struct ConvergenceParser {
    /// Current geometry optimization cycle (0 until the first cycle marker →
    /// single points stay at 0).
    current_cycle: u32,
    /// Inside an SCF iteration table (between the column header and the table's
    /// end). Gates the tolerant row parse so Freq eigenvector rows are ignored.
    in_scf: bool,
    /// Last `FINAL SINGLE POINT ENERGY` seen since the current cycle began — the
    /// energy attached to this cycle's `OptPoint`.
    pending_energy: Option<f64>,
    /// Inside a `|Geometry convergence|` block, accumulating its criteria.
    in_geom_conv: bool,
    criteria: Vec<Criterion>,
}

impl ConvergenceParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one line; return any datapoint it completes.
    pub fn feed(&mut self, line: &str) -> Option<ConvergenceEvent> {
        // A new optimization cycle: bump the counter, reset per-cycle state.
        if line.contains("GEOMETRY OPTIMIZATION CYCLE") {
            if let Some(n) = parse_cycle_number(line) {
                self.current_cycle = n;
            }
            self.pending_energy = None;
            self.in_scf = false;
            self.in_geom_conv = false;
            self.criteria.clear();
            return None;
        }

        // Per-cycle final energy (reuse the result-extraction regex, domain rule
        // #5-friendly — one line at a time).
        if line.contains("FINAL SINGLE POINT ENERGY") {
            if let Some(e) = extract_final_energy(line) {
                self.pending_energy = Some(e);
            }
            return None;
        }

        // Geometry convergence block: start collecting criteria.
        if line.contains("|Geometry convergence|") {
            self.in_geom_conv = true;
            self.criteria.clear();
            return None;
        }

        if self.in_geom_conv {
            // A dashed rule *after* ≥1 criterion closes the block and emits.
            if !self.criteria.is_empty() && is_rule_line(line) {
                self.in_geom_conv = false;
                let point = OptPoint {
                    cycle: self.current_cycle,
                    energy: self.pending_energy,
                    criteria: std::mem::take(&mut self.criteria),
                };
                return Some(ConvergenceEvent::Opt(point));
            }
            if let Some(c) = parse_criterion(line) {
                self.criteria.push(c);
            }
            // Header / leading rule / blank lines fall through as no-ops.
            return None;
        }

        // Entering an SCF iteration table. ORCA 6 prints an `Iteration ...
        // Energy (Eh)` column header per DIIS/SOSCF sub-table; older builds
        // print `SCF ITERATIONS`. Either opens the gate.
        if is_scf_header(line) {
            self.in_scf = true;
            return None;
        }

        if self.in_scf {
            // The table's end (convergence banner or the TOTAL SCF ENERGY
            // section) closes the gate.
            if is_scf_end(line) {
                self.in_scf = false;
                return None;
            }
            if let Some(p) = parse_scf_iteration(line, self.current_cycle) {
                return Some(ConvergenceEvent::Scf(p));
            }
        }

        None
    }
}

/// The integer after `CYCLE` in a `GEOMETRY OPTIMIZATION CYCLE N` marker.
fn parse_cycle_number(line: &str) -> Option<u32> {
    let mut toks = line.split_whitespace();
    while let Some(t) = toks.next() {
        if t == "CYCLE" {
            return toks.next()?.parse().ok();
        }
    }
    None
}

/// The `Iteration ... Energy (Eh)` column header (any SCF algorithm), or the
/// legacy `SCF ITERATIONS` banner.
fn is_scf_header(line: &str) -> bool {
    (line.contains("Iteration") && line.contains("Energy (Eh)")) || line.contains("SCF ITERATIONS")
}

/// A line that marks the end of the SCF iteration table.
fn is_scf_end(line: &str) -> bool {
    line.contains("SCF CONVERGED")
        || line.contains("SCF NOT CONVERGED")
        || line.contains("TOTAL SCF ENERGY")
}

/// A pure dashed rule line (`------...`), used to bound the convergence block.
fn is_rule_line(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3 && t.chars().all(|c| c == '-')
}

/// Tolerantly parse an SCF iteration row: `<int iter> <-energy> <ΔE> ...` with
/// ≥3 numeric fields. Returns `None` for anything that doesn't fit (skipped).
fn parse_scf_iteration(line: &str, cycle: u32) -> Option<ScfPoint> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    if toks.len() < 3 {
        return None;
    }
    // First token: a pure integer iteration number (no dot → rejects `1.` etc.).
    let iter: u32 = toks[0].parse().ok()?;
    // Second token: the energy — a negative decimal.
    if !(toks[1].starts_with('-') && toks[1].contains('.')) {
        return None;
    }
    let energy: f64 = toks[1].parse().ok()?;
    if energy >= 0.0 {
        return None;
    }
    // Third token: ΔE (may be positive, negative, or scientific).
    let delta_e: f64 = toks[2].parse().ok()?;
    Some(ScfPoint {
        cycle,
        iter,
        energy,
        delta_e,
    })
}

/// Parse one `|Geometry convergence|` criterion row: a (possibly two-word) name,
/// then value, tolerance, and a trailing `YES`/`NO`. Returns `None` for the
/// header, rule, and summary (`Max(Bonds) ...`) lines.
fn parse_criterion(line: &str) -> Option<Criterion> {
    let toks: Vec<&str> = line.split_whitespace().collect();
    if toks.len() < 4 {
        return None;
    }
    // Converged flag is the last token.
    let converged = match toks.last()?.to_ascii_uppercase().as_str() {
        "YES" => true,
        "NO" => false,
        _ => return None,
    };
    // The name is everything up to the first numeric token (the value).
    let value_idx = toks.iter().position(|t| t.parse::<f64>().is_ok())?;
    if value_idx == 0 {
        return None; // no name
    }
    let name = toks[..value_idx].join(" ");
    let value: f64 = toks.get(value_idx)?.parse().ok()?;
    let tolerance: f64 = toks.get(value_idx + 1)?.parse().ok()?;
    Some(Criterion {
        name,
        value,
        tolerance,
        converged,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXCERPT: &str = include_str!("../tests/fixtures/opt_output_excerpt.txt");

    /// Run the whole fixture through a fresh parser and collect every event.
    fn parse_all(text: &str) -> Vec<ConvergenceEvent> {
        let mut p = ConvergenceParser::new();
        text.lines().filter_map(|l| p.feed(l)).collect()
    }

    fn scf_points(events: &[ConvergenceEvent]) -> Vec<&ScfPoint> {
        events
            .iter()
            .filter_map(|e| match e {
                ConvergenceEvent::Scf(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    fn opt_points(events: &[ConvergenceEvent]) -> Vec<&OptPoint> {
        events
            .iter()
            .filter_map(|e| match e {
                ConvergenceEvent::Opt(o) => Some(o),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn parses_scf_iterations() {
        let events = parse_all(EXCERPT);
        let scf = scf_points(&events);
        // Cycle 1: 8 iterations (DIIS 1-5 + SOSCF 6-8); cycle 2: 4.
        assert_eq!(scf.len(), 12);
        assert_eq!(scf.first().unwrap().energy, -79.6871232228694026);
        assert_eq!(scf.last().unwrap().energy, -79.7949277548867855);
        // ΔE of the very first iteration is 0.00e+00.
        assert_eq!(scf.first().unwrap().delta_e, 0.0);
    }

    #[test]
    fn parses_geometry_convergence() {
        let events = parse_all(EXCERPT);
        let opt = opt_points(&events);
        // The second cycle's block carries all five criteria (an energy delta
        // exists once there's a previous cycle).
        let c2 = opt[1];
        assert_eq!(c2.criteria.len(), 5);

        let energy_change = &c2.criteria[0];
        assert_eq!(energy_change.name, "Energy change");
        assert_eq!(energy_change.value, -0.0000431163);
        assert_eq!(energy_change.tolerance, 0.0000050000);
        assert!(!energy_change.converged); // NO

        // RMS step is the one criterion that flipped to YES this cycle.
        let rms_step = c2.criteria.iter().find(|c| c.name == "RMS step").unwrap();
        assert!(rms_step.converged);
        let max_step = c2.criteria.iter().find(|c| c.name == "MAX step").unwrap();
        assert!(!max_step.converged);
    }

    #[test]
    fn does_not_hardcode_five_criteria() {
        // Cycle 1 legitimately has only four (no energy change yet).
        let events = parse_all(EXCERPT);
        let opt = opt_points(&events);
        assert_eq!(opt[0].criteria.len(), 4);
        assert!(opt[0].criteria.iter().all(|c| c.name != "Energy change"));
    }

    #[test]
    fn tracks_optimization_cycles() {
        let events = parse_all(EXCERPT);
        let scf = scf_points(&events);
        // The first eight SCF points are cycle 1, the last four cycle 2.
        assert!(scf[..8].iter().all(|s| s.cycle == 1));
        assert!(scf[8..].iter().all(|s| s.cycle == 2));

        let opt = opt_points(&events);
        assert_eq!(opt[0].cycle, 1);
        assert_eq!(opt[1].cycle, 2);
        // Each cycle's energy is its FINAL SINGLE POINT ENERGY.
        assert_eq!(opt[0].energy, Some(-79.791800280837));
        assert_eq!(opt[1].energy, Some(-79.791843397136));
    }

    #[test]
    fn ignores_garbage_lines() {
        let mut p = ConvergenceParser::new();
        let junk = [
            "",
            "   ",
            "some random prose about orbitals",
            "  NO LB      ZA    FRAG     MASS         X           Y           Z",
            "   0 C     6.0000    0    12.011    0.000000    0.000000    1.451310",
            "     1. B(C   1,C   0)                1.5360  0.003944 -0.0057    1.5303",
            "Max(Bonds)      0.0057      Max(Angles)    0.03",
            "-------------------------------------------------",
        ];
        for line in junk {
            assert_eq!(p.feed(line), None, "garbage produced an event: {line:?}");
        }
    }

    #[test]
    fn single_point_has_no_opt_points() {
        // A single point: an SCF table, no cycle markers, no convergence block.
        let sp = "\
----------------------------------------D-I-I-S--------------------------------------------
Iteration    Energy (Eh)           Delta-E    RMSDP     MaxDP     DIISErr   Damp  Time(sec)
-------------------------------------------------------------------------------------------
    1    -76.3200000000000000     0.00e+00  1.00e-01  2.00e-01  3.00e-01  0.700   0.0
    2    -76.4180000000000000    -9.80e-02  1.00e-02  2.00e-02  3.00e-02  0.000   0.0
                 **** Energy Check signals convergence ****
               *           SCF CONVERGED AFTER   2 CYCLES          *
TOTAL SCF ENERGY
FINAL SINGLE POINT ENERGY       -76.418000000000
                             ****ORCA TERMINATED NORMALLY****
";
        let events = parse_all(sp);
        // Only SCF points, all at cycle 0 (no optimization).
        assert_eq!(scf_points(&events).len(), 2);
        assert!(opt_points(&events).is_empty());
        assert!(scf_points(&events).iter().all(|s| s.cycle == 0));
    }

    /// Sanity-check against the two real full outputs on the dev machine (an
    /// Opt+Freq and a larger Opt). Ignored — the files live outside the repo.
    #[test]
    #[ignore = "reads real outputs from ~/.local/share"]
    fn real_full_outputs_parse_sanely() {
        for path in [
            "/home/laptop/.local/share/orcastudio/jobs/d7992449-10e3-47c9-9a16-8e22d60b955d/output.out",
            "/home/laptop/.local/share/orcastudio/jobs/99e805f5-1892-4ebb-9cb8-181cf7fc5fee/output.out",
        ] {
            if !std::path::Path::new(path).exists() {
                continue;
            }
            let text = std::fs::read_to_string(path).unwrap();
            let events = parse_all(&text);
            let scf = scf_points(&events);
            let opt = opt_points(&events);
            eprintln!("{path}: {} scf, {} opt", scf.len(), opt.len());
            // Opt cycles produced criteria blocks.
            assert!(!opt.is_empty(), "expected optimization steps in {path}");
            // The Freq eigenvector matrix (int, negative decimal, ≥3 numbers)
            // must NOT leak in as SCF points: real SCF energies for these runs
            // are around -80 / -402 Eh, never near 0.
            assert!(
                scf.iter().all(|s| s.energy < -1.0),
                "a near-zero 'SCF' energy leaked from the Freq eigenvector matrix in {path}"
            );
        }
    }

    #[test]
    fn freq_eigenvector_rows_are_not_scf() {
        // Normal-mode eigenvector rows match the SCF row *shape* (int, negative
        // decimal, ≥3 numbers) but appear outside any SCF table → must be
        // ignored. This is the gate's whole reason for existing.
        let mut p = ConvergenceParser::new();
        let rows = [
            "       0      -0.000014   0.048084   0.000002   0.000000   0.145824  -0.000007",
            "       3      -0.000014   0.048084   0.000001   0.000000  -0.145824   0.000007",
        ];
        for line in rows {
            assert_eq!(p.feed(line), None);
        }
    }
}
