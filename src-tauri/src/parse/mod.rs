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
pub mod hess;
pub mod mo;
pub mod property;
pub mod relaxscan;
pub mod units;
pub mod xyz;

use orcastudio_core::ids::{AtomId, IndexMap, OrcaIndex, SpaceIndex};

/// A reference geometry the **caller** supplies to a reader's `verify` — a geometry
/// we already know that the artifact should match (an `input.inp` start geometry, or
/// the optimized `.property.txt` final geometry). The reader never reads it from
/// disk itself (no hidden cross-module dependency). Shared by all four readers
/// (unit 1d unified `property` onto it too, so the map post-condition below is one
/// shape for every reader).
pub struct ReferenceGeometry {
    pub z: Vec<u8>,
    pub xyz_angstrom: Vec<[f64; 3]>,
    /// The `AtomId` at each emit position (`OrcaIndex`) — the atom-identity anchor
    /// [`check_map_order`] checks the `IndexMap` against, **independent of the map**.
    /// For a legacy job (every job in unit 1d) this is the DERIVED identity
    /// `[AtomId(0), AtomId(1), …]` built from the input coordinate block's atom
    /// count; unit 1e replaces it with the ids minted at `create_job`. Paired with
    /// `z` it is the `AtomId → element` table the post-condition consults.
    pub ids: Vec<AtomId>,
}

/// The map post-condition (unit 1d, rule #9), shared by all four readers — the ONE
/// per-atom function the ADR-016 seam changes. The artifact's element sequence
/// (`artifact_z`, in file / `OrcaIndex` order) must equal the sequence the
/// [`IndexMap`] asserts: position `p` holds `map.to_atom(OrcaIndex(p))`, whose
/// element is fixed **independently of the map** by the reference (its `ids`↔`z`
/// pairing, built alongside the map from the input coordinate block). For an
/// identity map this reduces to `artifact_z == reference.z` position-for-position —
/// exactly the element-order check Phase 3 shipped, so **green data is unchanged**
/// (identity ⇒ the same numbers on the dashboard). A permuted map makes the two
/// lookups disagree and this bites.
///
/// **Typed in-process / verified at the persistence boundary** — the distinction the
/// architect asked be stated verbatim, and the reason this is a post-condition and
/// not a claimed type invariant: within a single process the `emit_input` /
/// `parse_output` pair is *typed* (orcastudio-core sits on both sides, so the
/// compiler sees the `AtomId ↔ OrcaIndex` provenance). Across SQLite the map is
/// serialized and that provenance is **erased**, so at THIS boundary the invariant
/// honestly degrades from a type to *a required argument cross-checked against the
/// artifact*. Writing "type-level invariant" here would be exactly the over-reach
/// ADR-010's empirical addendum warns against — the map is the map an emit produced
/// only insofar as we re-derive and re-check it, never on the compiler's word.
pub fn check_map_order(
    artifact_z: &[u8],
    map: &IndexMap<OrcaIndex>,
    reference: &ReferenceGeometry,
    block: &str,
) -> Result<(), ParseError> {
    // Control (b): a map whose atom count disagrees with the artifact is refused,
    // named, before any per-atom work.
    if map.len() != artifact_z.len() {
        return Err(ParseError::LengthMismatch {
            field: format!("{block} vs IndexMap"),
            expected: map.len(),
            got: artifact_z.len(),
        });
    }
    if reference.ids.len() != reference.z.len() {
        return Err(ParseError::Malformed {
            field: "ReferenceGeometry".into(),
            detail: format!(
                "ids ({}) and z ({}) length disagree — the AtomId→element table is malformed",
                reference.ids.len(),
                reference.z.len()
            ),
        });
    }
    // AtomId → element, from the reference — the anchor that does NOT come from the
    // map, so permuting the map cannot cancel itself out against it.
    let elem_of: std::collections::BTreeMap<AtomId, u8> = reference
        .ids
        .iter()
        .copied()
        .zip(reference.z.iter().copied())
        .collect();
    for (p, &az) in artifact_z.iter().enumerate() {
        let id = map
            .to_atom(OrcaIndex::from_position(p))
            .ok_or(ParseError::OrderMismatch { block: block.to_string(), index: p })?;
        match elem_of.get(&id) {
            Some(&expected) if expected == az => {}
            // Either the map names an AtomId the reference does not know, or the
            // element the map+reference expect at p differs from the artifact.
            _ => return Err(ParseError::OrderMismatch { block: block.to_string(), index: p }),
        }
    }
    Ok(())
}

/// Negative-control twin of [`check_map_order`] that **ignores the map** and does
/// only the pre-1d positional element check (`artifact_z == reference.z`). Exists to
/// PROVE the map cross-check is load-bearing (CLAUDE.md: a gate whose ability to fail
/// is not demonstrated is green for an unknown reason): a permuted map that
/// `check_map_order` rejects passes THIS twin — so control (a)'s red is *caused by*
/// the map/artifact cross-check, not by chance. See the `map_order_controls` tests.
#[cfg(test)]
pub fn check_order_ignoring_map(
    artifact_z: &[u8],
    _map: &IndexMap<OrcaIndex>,
    reference: &ReferenceGeometry,
    block: &str,
) -> Result<(), ParseError> {
    if artifact_z != reference.z.as_slice() {
        let index = artifact_z
            .iter()
            .zip(&reference.z)
            .position(|(a, b)| a != b)
            .unwrap_or(artifact_z.len().min(reference.z.len()));
        return Err(ParseError::OrderMismatch { block: block.to_string(), index });
    }
    Ok(())
}

/// The `AtomId`s of a derived identity map, for a `ReferenceGeometry`'s `ids` field.
/// Paired with [`identity_map_for`] (`IndexMap::from_emit_order(&ids)`) this is the
/// legacy (unit 1d) path: `AtomId(i)` at `OrcaIndex i` for `n` atoms, the count taken
/// from the input coordinate block. Unit 1e replaces both with the minted ids/map.
pub fn derived_identity_ids(n: usize) -> Vec<AtomId> {
    (0..n as u32).map(AtomId::new).collect()
}

/// The `IndexMap` matching a reference's own `ids` — `IndexMap::from_emit_order(&ids)`.
/// The single source both the derived (unit 1d) path and tests use, so the map and the
/// reference it is checked against agree by construction on the green path.
pub fn identity_map_for(reference: &ReferenceGeometry) -> IndexMap<OrcaIndex> {
    IndexMap::from_emit_order(&reference.ids)
}

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

    /// A section/value that parsed structurally but is not what a valid artifact
    /// should contain (e.g. a translation/rotation zero-mode count that is neither
    /// 5 (linear) nor 6 (non-linear)).
    #[error("malformed {field}: {detail}")]
    Malformed { field: String, detail: String },
}

#[cfg(test)]
mod map_order_controls {
    //! Negative controls for the unit-1d map post-condition (CLAUDE.md: demonstrate
    //! the gate bites, then return to green). Ethane: elements C C H H H H H H.
    use super::*;

    fn ethane_reference() -> ReferenceGeometry {
        ReferenceGeometry {
            z: vec![6, 6, 1, 1, 1, 1, 1, 1],
            xyz_angstrom: vec![[0.0; 3]; 8],
            ids: derived_identity_ids(8),
        }
    }
    fn artifact_z() -> Vec<u8> {
        vec![6, 6, 1, 1, 1, 1, 1, 1]
    }
    /// A map that claims positions 0 and 2 hold each other's AtomId — a C↔H swap of
    /// two NON-equivalent atoms, the reorder that silently re-labels physics.
    fn permuted_map() -> IndexMap<OrcaIndex> {
        let mut order = derived_identity_ids(8);
        order.swap(0, 2);
        IndexMap::from_emit_order(&order)
    }

    #[test]
    fn a_permuted_map_is_refused() {
        let err = check_map_order(&artifact_z(), &permuted_map(), &ethane_reference(), "$test")
            .unwrap_err();
        assert!(matches!(err, ParseError::OrderMismatch { index: 0, .. }), "{err:?}");
    }

    #[test]
    fn b_wrong_atom_count_map_is_refused() {
        let short = IndexMap::from_emit_order(&derived_identity_ids(7)); // 7 vs the artifact's 8
        let err =
            check_map_order(&artifact_z(), &short, &ethane_reference(), "$test").unwrap_err();
        assert!(
            matches!(err, ParseError::LengthMismatch { expected: 7, got: 8, .. }),
            "{err:?}"
        );
    }

    #[test]
    fn c_without_the_map_check_the_permutation_goes_green() {
        // The decisive control: the SAME permuted map + normal reference + normal
        // artifact that (a) rejects passes the map-ignoring twin — so (a)'s red is
        // caused by the map/artifact cross-check, not by chance.
        let (z, r, m) = (artifact_z(), ethane_reference(), permuted_map());
        assert!(check_map_order(&z, &m, &r, "$test").is_err(), "(a) must be red");
        assert!(check_order_ignoring_map(&z, &m, &r, "$test").is_ok(), "(c) must be green");
    }

    #[test]
    fn identity_map_matches_the_pre_1d_positional_check() {
        let (z, r, m) = (artifact_z(), ethane_reference(), identity_map_for(&ethane_reference()));
        assert!(check_map_order(&z, &m, &r, "$test").is_ok());
        assert!(check_order_ignoring_map(&z, &m, &r, "$test").is_ok());
    }
}
