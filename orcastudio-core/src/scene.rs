//! Deserialization of the **v2** `scene_json` snapshot, with the SAME validation
//! semantics as the TS `deserializeScene` (unit 1b): ids unique, non-negative, and
//! `< nextAtomId`; any malformed shape is an `Err`, never a panic.
//!
//! **v1 is NOT migrated here — a decision, not a gap.** Migration has exactly one
//! implementation, in TypeScript, at the DB-read boundary (`deserializeScene`),
//! matching ADR-010 correction (ii)'s "one rule, one home". `create_job` (unit 1e)
//! always hands this core a **fresh** `serializeScene` output, i.e. v2 — so a v1
//! string reaching Rust is a **caller bug**, and it must be loud: this returns
//! `CoreError::UnsupportedSceneVersion` naming the version and where migration
//! lives, rather than silently doing a second migration that could drift from the TS one.

use serde::Deserialize;

use crate::ids::AtomId;
use crate::CoreError;

/// Valid fragment sources — mirrors the TS `FRAGMENT_SOURCES` list, so this core
/// rejects exactly what `validFragment` rejects.
const FRAGMENT_SOURCES: [&str; 5] = ["editor", "import", "smiles", "library", "fragment-library"];

/// A validated atom inside a scene: a stable id + element + Cartesian coords (Å).
#[derive(Debug, Clone, PartialEq)]
pub struct SceneAtom {
    pub id: AtomId,
    pub element: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneFragment {
    pub id: String,
    pub name: String,
    pub atoms: Vec<SceneAtom>,
    pub charge: i64,
    pub source: String,
    pub source_label: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub fragments: Vec<SceneFragment>,
    pub multiplicity: i64,
    pub next_atom_id: u32,
}

impl Scene {
    /// Σ of the per-fragment formal charges — the coordinate-block header charge
    /// (TS `totalCharge`).
    pub fn total_charge(&self) -> i64 {
        self.fragments.iter().map(|f| f.charge).sum()
    }

    /// Every atom's `AtomId` in emit order (fragment order, then in-fragment order)
    /// — the source the coordinate-block `IndexMap` is built from.
    pub fn atom_order(&self) -> Vec<AtomId> {
        self.fragments
            .iter()
            .flat_map(|f| f.atoms.iter().map(|a| a.id))
            .collect()
    }
}

// ── serde wire shapes (camelCase to match the TS JSON) ───────────────────────

#[derive(Deserialize)]
struct VersionProbe {
    version: i64,
}

#[derive(Deserialize)]
struct WireAtom {
    id: u32,
    element: String,
    x: f64,
    y: f64,
    z: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireFragment {
    id: String,
    name: String,
    atoms: Vec<WireAtom>,
    charge: i64,
    source: String,
    #[serde(default)]
    source_label: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireScene {
    fragments: Vec<WireFragment>,
    multiplicity: i64,
    next_atom_id: u32,
}

/// Parse a v2 `scene_json` string into a validated [`Scene`]. `Err` on a wrong
/// version, a malformed shape, or a broken id invariant — never a panic.
pub fn deserialize_scene(json: &str) -> Result<Scene, CoreError> {
    // Version first, so v1 gets the NAMED error (not a generic serde failure about
    // a missing atom id / nextAtomId).
    let probe: VersionProbe =
        serde_json::from_str(json).map_err(|e| CoreError::MalformedScene(e.to_string()))?;
    if probe.version != 2 {
        return Err(CoreError::UnsupportedSceneVersion { found: probe.version });
    }

    let wire: WireScene =
        serde_json::from_str(json).map_err(|e| CoreError::MalformedScene(e.to_string()))?;

    let mut seen: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    let mut fragments = Vec::with_capacity(wire.fragments.len());
    for wf in wire.fragments {
        if !FRAGMENT_SOURCES.contains(&wf.source.as_str()) {
            return Err(CoreError::MalformedScene(format!(
                "unknown fragment source {:?}",
                wf.source
            )));
        }
        let mut atoms = Vec::with_capacity(wf.atoms.len());
        for wa in wf.atoms {
            if wa.id >= wire.next_atom_id {
                return Err(CoreError::AtomIdOutOfRange {
                    id: wa.id,
                    next: wire.next_atom_id,
                });
            }
            if !seen.insert(wa.id) {
                return Err(CoreError::DuplicateAtomId(wa.id));
            }
            if !wa.x.is_finite() || !wa.y.is_finite() || !wa.z.is_finite() {
                return Err(CoreError::MalformedScene(format!(
                    "non-finite coordinate on atom id {}",
                    wa.id
                )));
            }
            atoms.push(SceneAtom {
                id: AtomId::new(wa.id),
                element: wa.element,
                x: wa.x,
                y: wa.y,
                z: wa.z,
            });
        }
        fragments.push(SceneFragment {
            id: wf.id,
            name: wf.name,
            atoms,
            charge: wf.charge,
            source: wf.source,
            source_label: wf.source_label,
        });
    }

    Ok(Scene {
        fragments,
        multiplicity: wire.multiplicity,
        next_atom_id: wire.next_atom_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const V2: &str = r#"{"version":2,"fragments":[{"id":"a","name":"W","atoms":[{"id":0,"element":"O","x":0.0,"y":0.0,"z":0.0},{"id":1,"element":"H","x":0.757,"y":0.0,"z":0.586}],"charge":0,"source":"editor"}],"multiplicity":1,"nextAtomId":2}"#;

    #[test]
    fn reads_a_valid_v2_scene() {
        let s = deserialize_scene(V2).unwrap();
        assert_eq!(s.fragments.len(), 1);
        assert_eq!(s.next_atom_id, 2);
        assert_eq!(s.atom_order(), vec![AtomId::new(0), AtomId::new(1)]);
        assert_eq!(s.total_charge(), 0);
    }

    #[test]
    fn v1_is_a_named_loud_error_not_a_migration() {
        let v1 = r#"{"version":1,"fragments":[],"multiplicity":1}"#;
        assert_eq!(
            deserialize_scene(v1),
            Err(CoreError::UnsupportedSceneVersion { found: 1 })
        );
    }

    #[test]
    fn rejects_id_at_or_above_next_atom_id() {
        let bad = r#"{"version":2,"fragments":[{"id":"a","name":"W","atoms":[{"id":5,"element":"O","x":0.0,"y":0.0,"z":0.0}],"charge":0,"source":"editor"}],"multiplicity":1,"nextAtomId":2}"#;
        assert_eq!(
            deserialize_scene(bad),
            Err(CoreError::AtomIdOutOfRange { id: 5, next: 2 })
        );
    }

    #[test]
    fn rejects_duplicate_ids() {
        let dup = r#"{"version":2,"fragments":[{"id":"a","name":"W","atoms":[{"id":0,"element":"O","x":0.0,"y":0.0,"z":0.0},{"id":0,"element":"H","x":1.0,"y":0.0,"z":0.0}],"charge":0,"source":"editor"}],"multiplicity":1,"nextAtomId":2}"#;
        assert_eq!(deserialize_scene(dup), Err(CoreError::DuplicateAtomId(0)));
    }

    #[test]
    fn malformed_json_is_err_not_panic() {
        assert!(matches!(
            deserialize_scene("{ not json"),
            Err(CoreError::MalformedScene(_))
        ));
    }
}
