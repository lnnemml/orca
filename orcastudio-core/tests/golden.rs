//! Golden tests: the Rust emit must be BYTE-IDENTICAL to the TS emit (ADR-016).
//! Fixtures are committed (written once from the real TS emitters — see the deleted
//! `gen-emit-goldens.gen.test.ts`), so this needs no Node. They carry the
//! adversarial coordinates unit 1c Part A2 surfaced: odd/512 round-half ties (which
//! survive JSON; `-0.0` does not — `JSON.stringify(-0)==="0"` — so signed zero is a
//! `fmt_coord` unit test, not a scene fixture), 1- and 2-char elements, two
//! fragments, and a non-zero total charge.

use orcastudio_core::emit::{
    emit_constraints_block, emit_coordinate_block, fmt_value, value_text_for, Constraint,
};
use orcastudio_core::ids::OrcaIndex;
use orcastudio_core::scene::deserialize_scene;

const SCENE_V2: &str = include_str!("fixtures/scene-v2.json");
const EXPECTED_BLOCK: &str = include_str!("fixtures/expected-block.txt");
const CONSTRAINTS_JSON: &str = include_str!("fixtures/constraints.json");
const EXPECTED_CONSTRAINTS: &str = include_str!("fixtures/expected-constraints.txt");

#[test]
fn coordinate_block_is_byte_identical_to_ts() {
    let scene = deserialize_scene(SCENE_V2).unwrap();
    let (block, _map) = emit_coordinate_block(&scene);
    assert_eq!(block, EXPECTED_BLOCK);
}

#[test]
fn constraints_block_is_byte_identical_to_ts() {
    let cs: Vec<Constraint> = serde_json::from_str(CONSTRAINTS_JSON).unwrap();
    assert_eq!(emit_constraints_block(&cs).unwrap(), EXPECTED_CONSTRAINTS);
}

/// The IndexMap and the emitted rows come from ONE source (the atom order), so the
/// element on row `i` must be the element of the atom the map sends `OrcaIndex(i)`
/// to. If the map were built from a different order than the rows, this fails.
#[test]
fn index_map_tracks_the_emitted_rows() {
    let scene = deserialize_scene(SCENE_V2).unwrap();
    let (block, map) = emit_coordinate_block(&scene);
    // rows: skip the header line, drop the trailing "*"
    let rows: Vec<&str> = block.lines().skip(1).filter(|l| *l != "*").collect();
    let by_id: std::collections::BTreeMap<_, _> = scene
        .fragments
        .iter()
        .flat_map(|f| f.atoms.iter())
        .map(|a| (a.id, a.element.as_str()))
        .collect();
    for (i, row) in rows.iter().enumerate() {
        let row_element = row.trim().split_whitespace().next().unwrap();
        let atom = map.to_atom(orca_index_at(i)).unwrap();
        assert_eq!(row_element, by_id[&atom], "row {i} element vs IndexMap atom");
    }
}

// small helper: OrcaIndex has no public position ctor exposed via a const fn here,
// so use the trait through IndexMap's own construction path indirectly.
fn orca_index_at(i: usize) -> OrcaIndex {
    use orcastudio_core::ids::SpaceIndex;
    OrcaIndex::from_position(i)
}

/// Negative control for coupling (Part B item 6b): swapping two atoms of DIFFERENT
/// elements changes BOTH the emitted text AND the IndexMap, together — because they
/// share one source of order. A change to only one would be the bug this catches.
#[test]
fn swapping_two_atoms_moves_text_and_map_together() {
    let scene = deserialize_scene(SCENE_V2).unwrap();
    let (block_a, map_a) = emit_coordinate_block(&scene);

    // Swap the first two atoms (O and H — different elements) inside fragment A.
    let mut swapped = scene.clone();
    swapped.fragments[0].atoms.swap(0, 1);
    let (block_b, map_b) = emit_coordinate_block(&swapped);

    assert_ne!(block_a, block_b, "text must change when atom order changes");
    // OrcaIndex(0) now resolves to a different AtomId — the map moved with the text.
    assert_ne!(
        map_a.to_atom(orca_index_at(0)),
        map_b.to_atom(orca_index_at(0)),
        "IndexMap must move with the emitted order (one source)"
    );
}

/// Killer case (a): a measured value V8 renders as "…62", Rust's canonical is "…63".
/// The value-model absorbs it: the parser (via `value_text_for`, judged by Rust's
/// OWN render) preserves the token, and emit reproduces it verbatim — byte-identical
/// to the TS text. Negative control: with `value_text = None` the core REFUSES it
/// (17-digit guard), which is exactly why the parser must set value_text.
#[test]
fn measured_value_round_trips_via_value_text() {
    for tok in ["-200.30410766601562", "106.62368774414062"] {
        let v: f64 = tok.parse().unwrap();
        let vtext = value_text_for(tok, v);
        assert_eq!(vtext.as_deref(), Some(tok), "Rust must preserve {tok}");
        let c = Constraint::Distance { atoms: [3, 7], value: Some(v), value_text: vtext };
        let block = emit_constraints_block(&[c]).unwrap();
        assert!(block.contains(&format!(" {tok} ")), "emit must reproduce {tok}");
    }
}

/// Boundary case (b): "0.0000001". TS `String` switches to exponential ("1e-7") so
/// TS SETS valueText; Rust `format!("{}")` never does, so Rust does NOT — OPPOSITE
/// canonicality judgments, SAME emitted bytes. (Documented on the parity page so a
/// future reader does not "fix" one side.)
#[test]
fn exponent_boundary_opposite_judgments_same_bytes() {
    let tok = "0.0000001";
    let v: f64 = tok.parse().unwrap();
    assert_eq!(fmt_value(v), "0.0000001"); // Rust stays fixed-point
    assert_eq!(value_text_for(tok, v), None); // Rust judges it canonical (TS would not)
    let c = Constraint::Distance { atoms: [0, 1], value: Some(v), value_text: None };
    assert!(emit_constraints_block(&[c]).unwrap().contains(" 0.0000001 "));
}
