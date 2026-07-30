//! Authoritative artifact readers (ADR-012): own Rust parsers over ORCA's
//! structured artifacts, replacing the cclib plan (cclib crashes on ORCA 6.1 —
//! `wiki/orca/parse-sources.md`). Four readers, this being the first and the
//! template:
//!
//!   * [`property`] — `.property.txt` (energies, geometry, charges, dipole,
//!     gradient, thermochemistry). **Built.**
//!   * `.hess` — signed frequencies, normal modes, IR. *Not started.*
//!   * `_trj.xyz` / `.xyz` — trajectory / final geometry. *Not started.*
//!   * `orca_2json` over `.gbw` — MO energies + occupations. *Not started.*
//!
//! Every reader is **two-layered** — a generic grammar tokenizer, then typed
//! accessors that convert to the app's **canonical units at the boundary**
//! (rule #11): lengths → Å ([`units::Angstrom`]), energies → Eh, frequencies →
//! cm⁻¹, IR → km/mol. Unknown grammar constructs stay *visible*, never silently
//! dropped (rule #10). See `wiki/modules/artifact-readers.md`.

pub mod elements;
pub mod property;
pub mod units;

/// Errors shared by the artifact readers. Converts into [`crate::error::AppError`]
/// so command-facing code keeps returning `Result<T, AppError>`.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("io reading {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },

    #[error("{artifact} too large: {bytes} bytes exceeds the {cap} byte cap")]
    TooLarge {
        artifact: &'static str,
        bytes: u64,
        cap: u64,
    },

    #[error("missing required field: {0}")]
    MissingField(String),

    /// The geometry post-condition (rule #11): a missed Bohr→Å conversion shows up
    /// here as ≈1.889×, far above the 1e-4 Å tolerance.
    #[error("geometry post-condition failed: max Δ {max_delta:.6} Å exceeds 1e-4 (a missed Bohr→Å conversion looks like ≈1.889×)")]
    GeometryMismatch { max_delta: f64 },

    #[error("atom order mismatch in {block}: element sequence differs from $Geometry at index {index}")]
    OrderMismatch { block: String, index: usize },

    #[error("length mismatch in {field}: expected {expected}, got {got}")]
    LengthMismatch {
        field: String,
        expected: usize,
        got: usize,
    },
}
