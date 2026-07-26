//! Minimal result extraction from ORCA output.
//!
//! Both functions operate on a **tail** of `output.out` (never the whole file —
//! domain rule #5). We pull only the two numbers the job list needs today; the
//! rich parse (geometries, frequencies, spectra) belongs to the Python sidecar
//! via cclib in a later phase.

use std::sync::LazyLock;

use regex::Regex;

/// `FINAL SINGLE POINT ENERGY       -76.418938719971`
static ENERGY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"FINAL SINGLE POINT ENERGY\s+(-?[\d.]+)").unwrap());

/// `TOTAL RUN TIME: 0 days 0 hours 0 minutes 35 seconds 423 msec`
static WALL_TIME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"TOTAL RUN TIME:\s+(\d+)\s+days?\s+(\d+)\s+hours?\s+(\d+)\s+minutes?\s+(\d+)\s+seconds?\s+(\d+)\s+msec",
    )
    .unwrap()
});

/// Final SCF energy in Hartree. Optimizations print this once per cycle, so we
/// take the **last** occurrence (the converged value).
pub fn extract_final_energy(output_tail: &str) -> Option<f64> {
    ENERGY_RE
        .captures_iter(output_tail)
        .last()?
        .get(1)?
        .as_str()
        .parse()
        .ok()
}

/// Total wall time in seconds, from ORCA's `TOTAL RUN TIME` banner.
pub fn extract_wall_time(output_tail: &str) -> Option<f64> {
    let c = WALL_TIME_RE.captures_iter(output_tail).last()?;
    let days: f64 = c.get(1)?.as_str().parse().ok()?;
    let hours: f64 = c.get(2)?.as_str().parse().ok()?;
    let minutes: f64 = c.get(3)?.as_str().parse().ok()?;
    let seconds: f64 = c.get(4)?.as_str().parse().ok()?;
    let msec: f64 = c.get(5)?.as_str().parse().ok()?;
    Some(days * 86_400.0 + hours * 3_600.0 + minutes * 60.0 + seconds + msec / 1_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A realistic slice of the end of an ORCA run (r²SCAN-3c water single point).
    const SP_TAIL: &str = "\
-------------------------   --------------------
FINAL SINGLE POINT ENERGY       -76.418938719971
-------------------------   --------------------

                             ****ORCA TERMINATED NORMALLY****
TOTAL RUN TIME: 0 days 0 hours 0 minutes 35 seconds 423 msec
";

    #[test]
    fn extracts_energy() {
        assert_eq!(extract_final_energy(SP_TAIL), Some(-76.418938719971));
    }

    #[test]
    fn extracts_last_energy_for_optimization() {
        let opt = "\
FINAL SINGLE POINT ENERGY       -79.700000000000
... geometry cycle ...
FINAL SINGLE POINT ENERGY       -79.718923456789
";
        // The converged (last) energy wins.
        assert_eq!(extract_final_energy(opt), Some(-79.718923456789));
    }

    #[test]
    fn extracts_wall_time_seconds() {
        // 0d 0h 0m 35s 423ms = 35.423 s
        assert_eq!(extract_wall_time(SP_TAIL), Some(35.423));
    }

    #[test]
    fn extracts_wall_time_with_larger_units() {
        let t = "TOTAL RUN TIME: 0 days 1 hours 2 minutes 3 seconds 500 msec";
        // 1*3600 + 2*60 + 3 + 0.5 = 3723.5
        assert_eq!(extract_wall_time(t), Some(3723.5));
    }

    #[test]
    fn returns_none_when_absent() {
        assert_eq!(extract_final_energy("no energy here"), None);
        assert_eq!(extract_wall_time("no timing here"), None);
    }
}
