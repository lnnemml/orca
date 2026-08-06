//! Minting the `IndexMap<OrcaIndex>` for a job at `create_job` (Phase 4.2 unit 1e,
//! ADR-016 amendment).
//!
//! **The map is minted from the SUBMITTED TEXT, verified against the scene — never
//! from the scene alone.** ORCA executes the *text*; if the scene and the text have
//! drifted (a sync race, any sync bug), a scene-only map would lie about the job and
//! per-atom data would be re-labelled silently — the exact 1d failure class, moved
//! to mint time. So [`mint_index_map`] parses the input's coordinate block and
//! checks it corresponds to the scene (element sequence exact, coordinates
//! float-tolerant — the same standard as the TS `xyzMatchesScene`); on any mismatch
//! or unsupported input form it returns `Err(reason)`, a **self-describing skip**,
//! and the caller records that reason rather than minting a map it cannot stand
//! behind. The job is NOT blocked (input validity is ORCA's business; the map is
//! ours).

use crate::ids::{IndexMap, OrcaIndex};
use crate::scene::Scene;

/// Coordinate agreement tolerance — the TS `xyzMatchesScene` default (`1e-6` Å).
const COORD_TOL: f64 = 1e-6;

/// Canonicalise an element symbol to `Xx` casing (`cl`/`CL` → `Cl`) — the Rust twin
/// of TS `normalizeElement`, so the mint correspondence and `xyzMatchesScene` accept
/// exactly the same pairs.
fn normalize_element(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase(),
    }
}

/// Extract the inline `* xyz <charge> <mult> … *` coordinate rows from an ORCA input,
/// or an `Err(reason)` naming why there is no inline block to mint from (an external
/// `* xyzfile`, a `%coords` block, or no block at all). Rows are `(element, [x,y,z])`
/// in file order.
fn coordinate_block_rows(input: &str) -> Result<Vec<(String, [f64; 3])>, String> {
    let mut inside = false;
    let mut rows = Vec::new();
    for line in input.lines() {
        let t = line.trim();
        let tl = t.to_lowercase();
        if !inside {
            if tl.starts_with("* xyzfile") || tl.starts_with("*xyzfile") {
                return Err(
                    "input uses `* xyzfile` (external geometry file), not an inline coordinate block"
                        .to_string(),
                );
            }
            if tl.starts_with("* xyz") || tl.starts_with("*xyz") {
                inside = true;
            }
            continue;
        }
        if t.starts_with('*') {
            break; // the closing `*`
        }
        let toks: Vec<&str> = t.split_whitespace().collect();
        if toks.len() >= 4 {
            if let (Ok(x), Ok(y), Ok(z)) =
                (toks[1].parse::<f64>(), toks[2].parse::<f64>(), toks[3].parse::<f64>())
            {
                rows.push((toks[0].to_string(), [x, y, z]));
            }
        }
    }
    if !inside {
        if input.to_lowercase().contains("%coords") {
            return Err(
                "input uses a `%coords` block, not a `* xyz` inline coordinate block".to_string()
            );
        }
        return Err("no inline `* xyz` coordinate block in the input".to_string());
    }
    Ok(rows)
}

/// Mint the job's `IndexMap<OrcaIndex>` from the **submitted text**, verified against
/// `scene`. `Ok(map)` when the text's coordinate block corresponds to the scene
/// (element sequence + float-tolerant coords, same standard as `xyzMatchesScene`);
/// `Err(reason)` — a self-describing skip — otherwise. The map keys the scene's
/// `AtomId`s by the text's row order (`OrcaIndex`), so a downstream `parse_output`
/// reads artifact rows back onto the right scene atoms.
///
/// It never mints from the scene alone: on any correspondence failure it skips, so a
/// scene/text drift is recorded, not silently encoded into a lying map.
pub fn mint_index_map(scene: &Scene, input_content: &str) -> Result<IndexMap<OrcaIndex>, String> {
    let rows = coordinate_block_rows(input_content)?;

    // Scene atoms in emit order (fragment order, then in-fragment) — the order
    // `injectSceneIntoInput` writes and `atom_order()` returns.
    let scene_atoms: Vec<(&str, [f64; 3])> = scene
        .fragments
        .iter()
        .flat_map(|f| f.atoms.iter().map(|a| (a.element.as_str(), [a.x, a.y, a.z])))
        .collect();

    if rows.len() != scene_atoms.len() {
        return Err(format!(
            "input coordinate block has {} atoms, the scene has {} — text and scene disagree",
            rows.len(),
            scene_atoms.len()
        ));
    }
    for (i, ((rel, rxyz), (sel, sxyz))) in rows.iter().zip(&scene_atoms).enumerate() {
        if normalize_element(rel) != normalize_element(sel) {
            return Err(format!(
                "atom {i}: element {rel:?} in the input text != {sel:?} in the scene"
            ));
        }
        for k in 0..3 {
            if (rxyz[k] - sxyz[k]).abs() > COORD_TOL {
                return Err(format!(
                    "atom {i}: coordinate {k} differs (text {} vs scene {}, tol {COORD_TOL})",
                    rxyz[k], sxyz[k]
                ));
            }
        }
    }

    // Verified: the text's row order equals the scene's emit order, so the map that
    // reads artifact rows back onto scene AtomIds IS the scene emit order.
    Ok(IndexMap::from_emit_order(&scene.atom_order()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{AtomId, OrcaIndex, SpaceIndex};
    use crate::scene::deserialize_scene;

    // Water, two fragments; AtomIds 0,1,2 are deliberately NOT emit-order-trivial in
    // the general case (here sequential, but the map carries them explicitly).
    const SCENE: &str = r#"{"version":2,"fragments":[
        {"id":"a","name":"O","atoms":[{"id":0,"element":"O","x":0.0,"y":0.0,"z":0.0}],"charge":0,"source":"editor"},
        {"id":"b","name":"H2","atoms":[
            {"id":1,"element":"H","x":0.757,"y":0.0,"z":0.586},
            {"id":2,"element":"H","x":-0.757,"y":0.0,"z":0.586}
        ],"charge":0,"source":"editor"}
    ],"multiplicity":1,"nextAtomId":3}"#;

    fn input(coords: &str) -> String {
        format!("! HF def2-SVP\n* xyz 0 1\n{coords}\n*\n")
    }

    fn scene() -> Scene {
        deserialize_scene(SCENE).unwrap()
    }

    #[test]
    fn mints_when_text_matches_scene() {
        let inp = input("O 0.0 0.0 0.0\nH 0.757 0.0 0.586\nH -0.757 0.0 0.586");
        let map = mint_index_map(&scene(), &inp).expect("text matches scene");
        assert_eq!(
            map.order(),
            &[AtomId::new(0), AtomId::new(1), AtomId::new(2)]
        );
        assert_eq!(map.to_atom(OrcaIndex::from_position(1)), Some(AtomId::new(1)));
    }

    #[test]
    fn element_casing_is_normalized_like_xyzmatchesscene() {
        let inp = input("o 0.0 0.0 0.0\nh 0.757 0.0 0.586\nH -0.757 0.0 0.586");
        assert!(mint_index_map(&scene(), &inp).is_ok(), "lowercase element still matches");
    }

    // Negative control (a): reordered rows vs the scene → SKIP with a named reason,
    // NOT a silent identity mint.
    #[test]
    fn reordered_rows_skip_with_a_named_reason() {
        // Swap the O row and a H row: element sequence now O↔H at position 0.
        let inp = input("H 0.757 0.0 0.586\nO 0.0 0.0 0.0\nH -0.757 0.0 0.586");
        let err = mint_index_map(&scene(), &inp).unwrap_err();
        assert!(err.contains("atom 0") && err.contains("element"), "{err}");
    }

    #[test]
    fn drifted_coordinate_skips() {
        let inp = input("O 0.0 0.0 0.0\nH 0.757 0.0 0.586\nH -0.757 0.0 0.999");
        let err = mint_index_map(&scene(), &inp).unwrap_err();
        assert!(err.contains("coordinate"), "{err}");
    }

    #[test]
    fn wrong_atom_count_skips() {
        let inp = input("O 0.0 0.0 0.0\nH 0.757 0.0 0.586");
        let err = mint_index_map(&scene(), &inp).unwrap_err();
        assert!(err.contains("2 atoms") && err.contains("3"), "{err}");
    }

    #[test]
    fn xyzfile_form_skips_with_named_reason() {
        let inp = "! HF def2-SVP\n* xyzfile 0 1 geom.xyz\n";
        let err = mint_index_map(&scene(), inp).unwrap_err();
        assert!(err.contains("xyzfile"), "{err}");
    }

    #[test]
    fn missing_block_skips_with_named_reason() {
        let inp = "! HF def2-SVP\n%pal nprocs 1 end\n";
        let err = mint_index_map(&scene(), inp).unwrap_err();
        assert!(err.contains("no inline"), "{err}");
    }

    // Negative control (c): with the correspondence check removed, the reordered
    // input (control a) would mint an identity map "for an unknown reason". This test
    // is the twin that proves the correspondence is what holds (a) a skip: it mints
    // scene-only and shows that map DISAGREES with the text order, which is exactly
    // what mint_index_map refuses to emit.
    #[test]
    fn without_correspondence_a_scene_only_mint_would_mislabel() {
        let inp = input("H 0.757 0.0 0.586\nO 0.0 0.0 0.0\nH -0.757 0.0 0.586");
        // The scene-only map (what mint would emit if it SKIPPED the check):
        let scene_only = IndexMap::<OrcaIndex>::from_emit_order(&scene().atom_order());
        // Text row 0 is H, but the scene-only map says row 0 → AtomId 0 (the O). That
        // disagreement is the mislabel; mint_index_map catches it and skips instead.
        assert_eq!(scene_only.to_atom(OrcaIndex::from_position(0)), Some(AtomId::new(0)));
        assert!(mint_index_map(&scene(), &inp).is_err(), "the real mint refuses to emit it");
    }
}
