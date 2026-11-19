//! Tests for the input assembly and its re-parse post-condition (C9).

use orcastudio_core::emit::emit_coordinate_block;
use orcastudio_core::scene::Scene;

use super::assemble::{assemble, assemble_with, comment_safe_label, AssemblyError, SERVER_DEFAULT_MAXCORE_MIB};
use super::charge_mult::{locate_charge_mult, ChargeMult};
use super::classify::SourceAtom;
use crate::memory::read_maxcore;

const HEADER: &str = "! wB97X-3c CPCM(DMF) Opt Freq\n%geom MaxIter 300 end\n";
const NEUTRAL: ChargeMult = ChargeMult { charge: 0, multiplicity: 1 };

fn atom(element: &str, x: f64, y: f64, z: f64) -> SourceAtom {
    SourceAtom { element: element.into(), x_ang: x, y_ang: y, z_ang: z }
}

/// Water, three distinct rows so a reorder is visible.
fn water() -> Vec<SourceAtom> {
    vec![atom("O", 0.0, 0.0, 0.1173), atom("H", 0.0, 0.7572, -0.4692), atom("H", 0.0, -0.7572, -0.4692)]
}

/// The production emitter's block, its rows edited by `edit`.
fn emit_rows_edited(edit: fn(&mut Vec<String>)) -> impl Fn(&Scene) -> String {
    move |scene| {
        let block = emit_coordinate_block(scene).0;
        let mut lines: Vec<String> = block.lines().map(str::to_string).collect();
        let last = lines.len() - 1;
        let mut rows = lines[1..last].to_vec();
        edit(&mut rows);
        lines.splice(1..last, rows);
        lines.join("\n")
    }
}

fn post_condition_failed(r: Result<super::assemble::Assembled, AssemblyError>) -> bool {
    matches!(r, Err(AssemblyError::PostCondition(_)))
}

#[test]
fn assembles_header_maxcore_comment_and_block() {
    let a = assemble(HEADER, 3000, &water(), NEUTRAL, "job:opt-1").expect("assembles");
    let (block, _) = emit_coordinate_block(&Scene {
        fragments: vec![orcastudio_core::scene::SceneFragment {
            id: "x".into(),
            name: "x".into(),
            atoms: water()
                .iter()
                .enumerate()
                .map(|(i, a)| orcastudio_core::scene::SceneAtom {
                    id: orcastudio_core::ids::AtomId::new(i as u32),
                    element: a.element.clone(),
                    x: a.x_ang,
                    y: a.y_ang,
                    z: a.z_ang,
                })
                .collect(),
            charge: 0,
            source: "import".into(),
            source_label: None,
        }],
        multiplicity: 1,
        next_atom_id: 3,
    });
    assert_eq!(
        a.input,
        format!("! wB97X-3c CPCM(DMF) Opt Freq\n%maxcore 3000\n%geom MaxIter 300 end\n\n# charge/mult from job:opt-1\n{block}\n")
    );
    assert_eq!(read_maxcore(&a.input), Ok(Some(3000)));
    assert_eq!(locate_charge_mult(&a.input), Ok(NEUTRAL));
    assert_eq!((a.app_set.maxcore_mib, a.app_set.atom_count), (3000, 3));
    assert_eq!(a.app_set.charge_mult_from, "job:opt-1");
}

#[test]
fn charge_and_multiplicity_come_from_the_source() {
    let cm = ChargeMult { charge: -1, multiplicity: 2 };
    let a = assemble("! r2SCAN-3c\r\n", SERVER_DEFAULT_MAXCORE_MIB, &water(), cm, "job:x/scan:3").expect("assembles");
    assert_eq!(locate_charge_mult(&a.input), Ok(cm));
    assert!(!a.input.contains('\r'));
    assert_eq!(read_maxcore(&a.input), Ok(Some(2000)));
}

#[test]
fn element_case_is_normalised() {
    let mut atoms = water();
    atoms[0].element = "o".into();
    let a = assemble(HEADER, 3000, &atoms, NEUTRAL, "molecule:7").expect("assembles");
    assert!(a.input.contains("\nO "));
}

#[test]
fn the_post_condition_refuses_a_reordered_block() {
    let swap = emit_rows_edited(|rows| rows.swap(0, 2));
    assert!(post_condition_failed(assemble_with(HEADER, 3000, &water(), NEUTRAL, "job:1", swap)));
}

#[test]
fn the_post_condition_refuses_other_wrong_blocks() {
    let edits: [fn(&mut Vec<String>); 4] = [
        |rows| {
            rows.pop();
        },
        |rows| rows[1] = rows[1].replacen('H', "F", 1),
        |rows| rows[2] = "H   NaN   0.0   0.0".into(),
        |rows| rows[0] = format!("{} 0.0", rows[0]),
    ];
    for edit in edits {
        assert!(post_condition_failed(assemble_with(HEADER, 3000, &water(), NEUTRAL, "job:1", emit_rows_edited(edit))));
    }
    // A missed Å→Bohr scale (rule #11) is a coordinate mismatch, not a plausible geometry.
    let bohr = |scene: &Scene| {
        let mut s = scene.clone();
        for a in &mut s.fragments[0].atoms {
            (a.x, a.y, a.z) = (a.x * 1.889_726, a.y * 1.889_726, a.z * 1.889_726);
        }
        emit_coordinate_block(&s).0
    };
    assert!(post_condition_failed(assemble_with(HEADER, 3000, &water(), NEUTRAL, "job:1", bohr)));
    // A wrong header line, and a second block.
    let charged = |scene: &Scene| emit_coordinate_block(scene).0.replacen("* xyz 0 1", "* xyz 1 2", 1);
    assert!(post_condition_failed(assemble_with(HEADER, 3000, &water(), NEUTRAL, "job:1", charged)));
    let twice = |scene: &Scene| {
        let b = emit_coordinate_block(scene).0;
        format!("{b}\n{b}")
    };
    assert!(post_condition_failed(assemble_with(HEADER, 3000, &water(), NEUTRAL, "job:1", twice)));
}

#[test]
fn bad_inputs_are_refused_before_emitting() {
    assert_eq!(assemble("! SP\n%maxcore 9000\n", 3000, &water(), NEUTRAL, "j"), Err(AssemblyError::HeaderSetsMaxcore));
    assert_eq!(assemble(HEADER, 3000, &[], NEUTRAL, "j"), Err(AssemblyError::NoAtoms));
    assert!(matches!(assemble(HEADER, 0, &water(), NEUTRAL, "j"), Err(AssemblyError::Maxcore(_))));
    let mut atoms = water();
    atoms[1].element = "Qq".into();
    assert_eq!(assemble(HEADER, 3000, &atoms, NEUTRAL, "j"), Err(AssemblyError::UnknownElement("Qq".into())));
    let mut atoms = water();
    atoms[2].y_ang = f64::INFINITY;
    assert_eq!(assemble(HEADER, 3000, &atoms, NEUTRAL, "j"), Err(AssemblyError::NonFiniteCoordinate(2)));
}

#[test]
fn an_assembly_error_is_a_refused_internal_error() {
    assert_eq!(AssemblyError::NoAtoms.as_refusal().row(), "internal");
}

#[test]
fn a_smiles_label_cannot_put_a_second_hash_on_the_comment_line() {
    let a = assemble(HEADER, 3000, &water(), NEUTRAL, "smiles:C#N").expect("assembles");
    assert!(a.input.contains("\n# charge/mult from smiles:C?N\n"), "{}", a.input);
    assert!(a.input.lines().all(|l| l.matches('#').count() < 2));
    // The response still names the source as written.
    assert_eq!(a.app_set.charge_mult_from, "smiles:C#N");
    // A line break or a non-ASCII character cannot leave the comment either.
    assert_eq!(comment_safe_label("job:1\n* xyz 0 1"), "job:1?? xyz 0 1");
    assert_eq!(comment_safe_label("smiles:[NH4+]/C=C(\\Cl)@ ω"), "smiles:[NH4+]/C=C(?Cl)@ ?");
}

#[test]
fn the_post_condition_refuses_a_line_with_two_hashes() {
    // After the closing `*`: neither the locator nor the row reader reads this line, so only the
    // post-condition's own two-`#` check can refuse it.
    let trailing = |scene: &Scene| format!("{}\n# a # * xyz 1 2", emit_coordinate_block(scene).0);
    assert!(post_condition_failed(assemble_with(HEADER, 3000, &water(), NEUTRAL, "job:1", trailing)));
    // Before the header the locator refuses it too.
    let leading = |scene: &Scene| format!("# a # b\n{}", emit_coordinate_block(scene).0);
    assert!(post_condition_failed(assemble_with(HEADER, 3000, &water(), NEUTRAL, "job:1", leading)));
}
