//! `emit_input` — the **order-bearing** emit that ADR-016 moves to Rust. Two
//! blocks, each **byte-identical** to its TypeScript source:
//!
//!   * [`emit_coordinate_block`] == `injectSceneIntoInput`'s `* xyz … *` block
//!     (`src/scene/scene.ts`: header + `mergeToAtomLines` rows), and hands back the
//!     `IndexMap<OrcaIndex>` the corresponding `parse_output` will read with;
//!   * [`emit_constraints_block`] == `constraintsBlock` (`src/scene/constraints.ts`).
//!
//! Splicing a block into existing input text does NOT move here — it carries no
//! atom order — so it stays in TS (ADR-016).
//!
//! The two number formatters were measured against their JS counterparts
//! (`wiki/architecture/float-formatting-parity.md`, unit 1c Part A/A2):
//!   * coordinates: [`fmt_coord`] reproduces `toFixed(8).padStart(14)` (incl. the
//!     odd/512 round-half ties and signed zero);
//!   * constraint values: [`fmt_value`] = `format!("{}")` reproduces `String(v)` for
//!     canonical / `value_text` values; a raw 17-significant-digit programmatic value
//!     cannot round-trip byte-identically and is refused loudly (see the guard).

use crate::ids::{IndexMap, OrcaIndex};
use crate::scene::Scene;
use crate::CoreError;

// ── coordinate formatter (fmt_coord) — see float-formatting-parity.md Front 1 ─

/// Format one coordinate byte-identically to JS `n.toFixed(8).padStart(14)`.
/// Three parts (measured): (1) signed zero `-0.0 → +0.0`; (2) exact 8th-decimal
/// ties `x = odd/512`, where JS rounds half-AWAY but Rust `{:.8}` rounds half-to-EVEN,
/// rendered away-from-zero; (3) else `{:.8}`. Then `padStart(14)`.
pub fn fmt_coord(x: f64) -> String {
    let x = if x == 0.0 { 0.0 } else { x }; // (1) -0.0 == 0.0, so this maps only ±0.0
    let ax = x.abs();
    let y = ax * 512.0; // exact: scaling by a power of two never rounds
    let is_tie = y < 9_007_199_254_740_992.0 /* 2^53 */ && y.fract() == 0.0 && (y as u64) % 2 == 1;
    let core = if is_tie {
        // away from zero: m = floor(|x|*1e8) + 1 (|x|*1e8 is an exact half-integer < 2^53)
        let m = (ax * 1e8).floor() as u64 + 1;
        let sign = if x.is_sign_negative() { "-" } else { "" };
        format!("{}{}.{:08}", sign, m / 100_000_000, m % 100_000_000)
    } else {
        format!("{:.8}", x)
    };
    format!("{:>14}", core) // padStart(14) — right-align, min width 14, no truncation
}

/// One canonical coordinate row: element padded to 2 (`padEnd(2)`) then the three
/// formatted coordinates (each already left-padded to 14). No separators — matches
/// `atomRow` in `scene.ts`.
fn atom_row(element: &str, x: f64, y: f64, z: f64) -> String {
    format!("{:<2}{}{}{}", element, fmt_coord(x), fmt_coord(y), fmt_coord(z))
}

/// Emit the `* xyz <totalCharge> <multiplicity> … *` coordinate block AND the
/// `IndexMap<OrcaIndex>` for the order it just wrote (ADR-010: same module owns
/// outgoing order + incoming map). Byte-identical to the block
/// `injectSceneIntoInput` inserts. Rows are in fragment order, then in-fragment
/// order — the same order `atom_order` / the `IndexMap` use.
pub fn emit_coordinate_block(scene: &Scene) -> (String, IndexMap<OrcaIndex>) {
    let rows: Vec<String> = scene
        .fragments
        .iter()
        .flat_map(|f| f.atoms.iter().map(|a| atom_row(&a.element, a.x, a.y, a.z)))
        .collect();
    let text = format!(
        "* xyz {} {}\n{}\n*",
        scene.total_charge(),
        scene.multiplicity,
        rows.join("\n")
    );
    let map = IndexMap::<OrcaIndex>::from_emit_order(&scene.atom_order());
    (text, map)
}

// ── constraint value formatter (fmt_value) — parity.md Front 2 ────────────────

/// Format a constraint value byte-identically to JS `String(v)` for the canonical
/// case. Rust `format!("{}")` and V8 `String` agree on every value whose shortest
/// round-trip is unique; they diverge only on 17-significant-digit values where the
/// last digit is itself ambiguous (measured). Those never reach here under the
/// value-model invariant (they arrive as `value_text` from parsing, or are refused
/// by [`emit_constraints_block`]'s guard).
pub fn fmt_value(v: f64) -> String {
    format!("{}", v)
}

/// Canonicality decision, judged by **this** emitter's own render (the Rust analogue
/// of TS `if (String(value) !== token) valueText = token`). A parser (unit 1d) calls
/// this so that any token this core cannot reproduce canonically is preserved
/// verbatim — turning "a 17-digit double never reaches `fmt_value`" from an
/// assumption into a guarantee by construction. Exposed now so emit can round-trip.
pub fn value_text_for(token: &str, parsed: f64) -> Option<String> {
    if token != fmt_value(parsed) {
        Some(token.to_string())
    } else {
        None
    }
}

/// Significant digits of a shortest-repr string: digit chars minus leading and
/// trailing zeros. `"-200.30410766601563"` → 17; `"90"` → 1; `"0.00195313"` → 6.
fn significant_digits(s: &str) -> usize {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.trim_start_matches('0').trim_end_matches('0').len()
}

/// OrcaStudio's own 0-based atom indices, and ORCA's constraint index base, are
/// both 0 (`ORCA_INDEX_BASE`, settled by a real run — `wiki/orca/constraints.md`),
/// so the map is identity. Named as a discipline, not a magic pass-through.
const ORCA_INDEX_BASE: u32 = 0;
const APP_INDEX_BASE: u32 = 0;
fn to_orca_index(global: u32) -> u32 {
    global - APP_INDEX_BASE + ORCA_INDEX_BASE
}

/// A `%geom` constraint, atoms in OrcaStudio's 0-based global index space — mirrors
/// the TS `Constraint`, INCLUDING `value_text` (the user's exact numeric text,
/// preserved through a parse→emit round-trip).
///
/// TODO(1e): the app's other constraint type — `src-tauri/src/xtb.rs::Constraint`
/// (1-based `$constrain`, no `value_text`) — is deliberately NOT unified with this
/// one in 1c. Unifying them and branding the xtb serde boundary is unit 1e's job.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Constraint {
    Distance {
        atoms: [u32; 2],
        #[serde(default)]
        value: Option<f64>,
        #[serde(default, rename = "valueText")]
        value_text: Option<String>,
    },
    Angle {
        atoms: [u32; 3],
        #[serde(default)]
        value: Option<f64>,
        #[serde(default, rename = "valueText")]
        value_text: Option<String>,
    },
    Dihedral {
        atoms: [u32; 4],
        #[serde(default)]
        value: Option<f64>,
        #[serde(default, rename = "valueText")]
        value_text: Option<String>,
    },
    Cartesian {
        atoms: [u32; 1],
    },
}

impl Constraint {
    fn type_letter(&self) -> char {
        match self {
            Constraint::Distance { .. } => 'B',
            Constraint::Angle { .. } => 'A',
            Constraint::Dihedral { .. } => 'D',
            Constraint::Cartesian { .. } => 'C',
        }
    }
    fn atom_indices(&self) -> &[u32] {
        match self {
            Constraint::Distance { atoms, .. } => atoms,
            Constraint::Angle { atoms, .. } => atoms,
            Constraint::Dihedral { atoms, .. } => atoms,
            Constraint::Cartesian { atoms } => atoms,
        }
    }
    fn value_parts(&self) -> (Option<f64>, Option<&String>) {
        match self {
            Constraint::Distance { value, value_text, .. }
            | Constraint::Angle { value, value_text, .. }
            | Constraint::Dihedral { value, value_text, .. } => (*value, value_text.as_ref()),
            Constraint::Cartesian { .. } => (None, None),
        }
    }
}

/// One `{ … }` line, byte-identical to `constraintLine` in `constraints.ts`.
/// `value_text ?? fmt_value(value)` — but a programmatic (non-`value_text`) value
/// with ≥17 significant digits is refused (see the module note / `CoreError`).
fn constraint_line(c: &Constraint) -> Result<String, CoreError> {
    let idx = c
        .atom_indices()
        .iter()
        .map(|&g| to_orca_index(g).to_string())
        .collect::<Vec<_>>()
        .join(" ");
    if let Constraint::Cartesian { .. } = c {
        return Ok(format!("{{C {idx} C}}"));
    }
    let (value, value_text) = c.value_parts();
    let vt: Option<String> = match value_text {
        Some(t) => Some(t.clone()),
        None => match value {
            Some(v) => {
                let s = fmt_value(v);
                let digits = significant_digits(&s);
                if digits >= 17 {
                    return Err(CoreError::NonCanonicalConstraintValue { value: s, digits });
                }
                Some(s)
            }
            None => None,
        },
    };
    let value_str = vt.map(|s| format!(" {s}")).unwrap_or_default();
    Ok(format!("{{{} {idx}{value_str} C}}", c.type_letter()))
}

/// The standalone `%geom Constraints … end end` block, byte-identical to
/// `constraintsBlock`. No trailing newline (callers add separators).
pub fn emit_constraints_block(cs: &[Constraint]) -> Result<String, CoreError> {
    let lines = cs
        .iter()
        .map(|c| constraint_line(c).map(|l| format!("    {l}")))
        .collect::<Result<Vec<_>, _>>()?
        .join("\n");
    Ok(format!("%geom\n  Constraints\n{lines}\n  end\nend"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_coord_signed_zero_and_ties() {
        assert_eq!(fmt_coord(-0.0), "    0.00000000"); // JS drops the sign
        assert_eq!(fmt_coord(0.0), "    0.00000000");
        assert_eq!(fmt_coord(1.0 / 512.0), "    0.00195313"); // away-from-zero tie
        assert_eq!(fmt_coord(-(1.0 / 512.0)), "   -0.00195313");
        assert_eq!(fmt_coord(-1e-12), "   -0.00000000"); // NOT zero → keeps sign
        assert_eq!(fmt_coord(1.234), "    1.23400000");
    }

    #[test]
    fn value_text_is_judged_by_rust_own_render() {
        // A measured value V8 shows as "…62"; Rust's canonical is "…63", so Rust
        // MUST preserve the token as value_text to reproduce the TS text.
        let tok = "-200.30410766601562";
        let parsed: f64 = tok.parse().unwrap();
        assert_eq!(fmt_value(parsed), "-200.30410766601563"); // Rust ≠ token
        assert_eq!(value_text_for(tok, parsed), Some(tok.to_string()));
        // A canonical value needs no value_text.
        assert_eq!(value_text_for("90", 90.0), None);
    }

    #[test]
    fn constraint_line_forms() {
        let d = Constraint::Distance { atoms: [0, 1], value: Some(1.234), value_text: None };
        assert_eq!(constraint_line(&d).unwrap(), "{B 0 1 1.234 C}");
        let freeze = Constraint::Distance { atoms: [0, 1], value: None, value_text: None };
        assert_eq!(constraint_line(&freeze).unwrap(), "{B 0 1 C}");
        let cart = Constraint::Cartesian { atoms: [4] };
        assert_eq!(constraint_line(&cart).unwrap(), "{C 4 C}");
        let vt = Constraint::Angle {
            atoms: [0, 1, 2],
            value: Some(90.0),
            value_text: Some("90.0".to_string()),
        };
        assert_eq!(constraint_line(&vt).unwrap(), "{A 0 1 2 90.0 C}");
    }

    #[test]
    fn guard_refuses_a_programmatic_17_digit_value() {
        let bad = Constraint::Distance {
            atoms: [0, 1],
            value: Some(-200.30410766601562), // parses to a 17-digit-canonical double
            value_text: None,                 // programmatic, no preserved text
        };
        assert!(matches!(
            constraint_line(&bad),
            Err(CoreError::NonCanonicalConstraintValue { digits: 17, .. })
        ));
    }

    #[test]
    fn block_shape() {
        let cs = vec![
            Constraint::Distance { atoms: [0, 1], value: Some(1.234), value_text: None },
            Constraint::Cartesian { atoms: [4] },
        ];
        assert_eq!(
            emit_constraints_block(&cs).unwrap(),
            "%geom\n  Constraints\n    {B 0 1 1.234 C}\n    {C 4 C}\n  end\nend"
        );
    }
}
