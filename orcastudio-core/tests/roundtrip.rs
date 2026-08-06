//! Round-trip property gate (Phase 4.2 unit 1d, Task 3): the **typed, in-process**
//! half of the ADR-010 `emit_input` / `parse_output` invariant.
//!
//! For many randomly-generated scenes: set atom data keyed by `AtomId` → emit the
//! coordinate block (which also hands back the `IndexMap<OrcaIndex>`) → parse that
//! emit's own rows back → re-key each row by `AtomId` through the map → the result
//! is **identity** with the scene's per-`AtomId` (element, coords). The map is also
//! a bijection covering every scene atom exactly once.
//!
//! This is the half the compiler CAN see: emit and parse are the same crate, so the
//! `AtomId ↔ OrcaIndex` provenance is typed, not serialized. The other half — a map
//! that crossed SQLite, provenance erased — is verified against the artifact in the
//! src-tauri readers (a post-condition, rule #9), NOT claimed as a type invariant.
//!
//! Deterministic on purpose: a seeded splitmix64 generator, no `rand`/`Math.random`
//! (which the workflow runtime forbids and which would make a failure irreproducible).
//! Coordinates are drawn on the `k/8` lattice — exactly representable doubles that
//! survive `fmt_coord`'s `toFixed(8)` and re-parse bit-identically — because THIS
//! test guards atom-identity plumbing, not float formatting (that is the parity
//! probe's job, `wiki/architecture/float-formatting-parity.md`).

use orcastudio_core::emit::{emit_coordinate_block, parse_coordinate_rows};
use orcastudio_core::ids::{AtomId, OrcaIndex, SpaceIndex};
use orcastudio_core::scene::{Scene, SceneAtom, SceneFragment};

/// A minimal seeded PRNG (splitmix64). Reproducible: the same seed always yields the
/// same scene, so a failing case is a fixed seed, printable and re-runnable.
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }
    /// A coordinate on the `k/8` lattice in [-64, 64): exactly representable.
    fn coord(&mut self) -> f64 {
        let k = self.below(1024) as i64 - 512; // -512..512
        k as f64 / 8.0
    }
}

const ELEMENTS: [&str; 6] = ["H", "C", "N", "O", "Cl", "Fe"];

/// Build a random valid scene with non-sequential `AtomId`s (so identity-of-position
/// is never accidentally trivial) split across 1–3 fragments.
fn random_scene(rng: &mut Rng) -> Scene {
    let n_atoms = 1 + rng.below(9) as usize; // 1..=9 atoms

    // Distinct, deliberately-scrambled AtomIds (not 0..n): draw unique ids from a
    // wider space so emit-order ≠ id-order in general.
    let mut ids: Vec<u32> = Vec::new();
    let mut next = 0u32;
    while ids.len() < n_atoms {
        next = next.wrapping_add(1 + rng.below(4) as u32); // strictly increasing, gappy
        ids.push(next);
    }
    // Scramble id→position so the emit order is not the id order.
    for i in (1..ids.len()).rev() {
        let j = rng.below((i + 1) as u64) as usize;
        ids.swap(i, j);
    }
    let next_atom_id = next + 1;

    let mut atoms: Vec<SceneAtom> = ids
        .into_iter()
        .map(|id| SceneAtom {
            id: AtomId::new(id),
            element: ELEMENTS[rng.below(ELEMENTS.len() as u64) as usize].to_string(),
            x: rng.coord(),
            y: rng.coord(),
            z: rng.coord(),
        })
        .collect();

    // Partition into 1–3 fragments, preserving order.
    let n_frags = 1 + rng.below(3.min(atoms.len() as u64)) as usize;
    let mut fragments = Vec::new();
    for f in 0..n_frags {
        let take = if f == n_frags - 1 {
            atoms.len()
        } else {
            1 + rng.below((atoms.len().saturating_sub(n_frags - f - 1)) as u64) as usize
        };
        let frag_atoms: Vec<SceneAtom> = atoms.drain(..take.min(atoms.len())).collect();
        fragments.push(SceneFragment {
            id: format!("frag{f}"),
            name: format!("F{f}"),
            atoms: frag_atoms,
            charge: rng.below(3) as i64 - 1, // -1, 0, +1
            source: "editor".to_string(),
            source_label: None,
        });
    }

    Scene {
        fragments,
        multiplicity: 1 + rng.below(3) as i64,
        next_atom_id,
    }
}

#[test]
fn coordinate_block_round_trips_by_atom_id() {
    let mut rng = Rng(0xC0FFEE_D15EA5E);
    for iter in 0..2000 {
        let scene = random_scene(&mut rng);

        // The scene's own per-AtomId truth (what we "set through AtomId").
        let want: std::collections::BTreeMap<AtomId, (String, [f64; 3])> = scene
            .fragments
            .iter()
            .flat_map(|f| f.atoms.iter())
            .map(|a| (a.id, (a.element.clone(), [a.x, a.y, a.z])))
            .collect();
        let n = want.len();

        // Emit → parse own emit back → re-key by AtomId THROUGH THE MAP.
        let (text, map) = emit_coordinate_block(&scene);
        let rows = parse_coordinate_rows(&text);
        assert_eq!(rows.len(), n, "iter {iter}: row count != atom count");
        assert_eq!(map.len(), n, "iter {iter}: map len != atom count");

        let mut recovered_ids = std::collections::BTreeSet::new();
        for (p, (element, xyz)) in rows.iter().enumerate() {
            let id = map
                .to_atom(OrcaIndex::from_position(p))
                .unwrap_or_else(|| panic!("iter {iter}: no AtomId at position {p}"));
            assert!(recovered_ids.insert(id), "iter {iter}: AtomId {id:?} twice");

            let (want_el, want_xyz) = &want[&id];
            assert_eq!(element, want_el, "iter {iter}: element on {id:?}");
            for k in 0..3 {
                assert!(
                    (xyz[k] - want_xyz[k]).abs() < 1e-9,
                    "iter {iter}: coord {k} on {id:?}: {} vs {}",
                    xyz[k],
                    want_xyz[k]
                );
            }
        }

        // Bijection: every scene AtomId was recovered exactly once, and the forward
        // direction agrees.
        for id in want.keys() {
            assert!(recovered_ids.contains(id), "iter {iter}: {id:?} not recovered");
            assert!(map.to_space(*id).is_some(), "iter {iter}: forward map missing {id:?}");
        }
    }
}
