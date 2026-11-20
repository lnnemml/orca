//! Tests for `get_job`'s built header (ADR-027 M2b; Anton's "build, don't filter") and its
//! tripwire. The attack matrix is the M2b verifier's (`v2/jobs_tests_attack.rs`), every input kept.

use super::header_view::{coordinate_row, reconstruct, Field};
use super::jobs::{get_job, list_jobs, tripwire, JobFilter, WITHHELD};
use super::test_support::JobRow;
use super::{result_text, ToolError};
use crate::mcp::test_db::TestDb;

/// The values a leak would carry: the verifier's coordinates and an atomic mass.
const SECRET: [&str; 4] = ["0.117790", "0.755453", "0.471161", "15.9949"];
const W: &str = "! B3LYP def2-SVP Opt\n";
const T: &str = "* xyz 0 1\nH 0 0 0\nH 0 0 0.74\n*\n";

fn secrets_in(text: &str) -> Vec<&'static str> {
    SECRET.iter().copied().filter(|s| text.contains(s)).collect()
}

/// `get_job` on `input` as it reaches the model: the view's JSON, or the refusal's text.
fn served(db: &TestDb, id: &str, input: &str) -> String {
    JobRow { input, ..JobRow::new(id) }.insert(&db.conn);
    match get_job(&db.conn, id) {
        Ok(v) => result_text(&v).unwrap(),
        Err(ToolError::Refused(why)) => why,
        Err(e) => panic!("{id}: get_job failed: {e}"),
    }
}

/// The verifier's attack matrix, every input (B1: the main block's rows quoted by an error; B2: a
/// `%coords` opener hidden after `end` or on a refused line; the second-job forms; the long row).
fn attack_matrix() -> Vec<(&'static str, String)> {
    let (w, t) = (W, T);
    vec![
        ("end_then_coords_mass", format!("{w}%pal nprocs 4 end %coords\n Coords\n  O 0.000000 0.000000 0.117790 M = 15.9949\n end\nend\n{t}")),
        ("end_then_coords_plain", format!("{w}%pal nprocs 4 end %coords\n Coords\n  O 0.000000 0.000000 0.117790\n end\nend\n{t}")),
        ("end_then_coords_comma", format!("{w}%pal nprocs 4 end %coords\n Coords\n  O 0.000000, 0.000000, 0.117790\n end\nend\n{t}")),
        ("lexfail_coords_mass", format!("{w}%coords # a # b\n Coords\n  O 0.000000 0.000000 0.117790 M = 15.9949\n end\nend\n{t}")),
        ("lexfail_coords_Z", format!("{w}%coords # a # b\n Coords\n  H 0.000000 0.755453 -0.471161 Z = 1.5\n end\nend\n{t}")),
        ("lexfail_coords_comma", format!("{w}%coords # a # b\n Coords\n  H 0.000000,0.755453,-0.471161\n end\nend\n{t}")),
        ("lexfail_pardef", format!("{w}%coords # a # b\n CTyp internal\n pardef\n  rCO = 0.117790;\n end\n Coords\n  C 0 0 0 0 0 0\n  O 1 0 0 {{rCO}} 0 0\n end\nend\n{t}")),
        ("coords_crlf_tabs", format!("{w}%coords\r\n\tCTyp\txyz\r\n\tCoords\r\n\t\tO\t0.000000\t0.000000\t0.117790\r\n\tend\r\nend\r\n{t}")),
        ("coords_mass", format!("{w}%coords\n Coords\n  O 0.000000 0.000000 0.117790 M = 15.9949\n end\nend\n{t}")),
        ("coords_Q", format!("{w}%coords\n Coords\n  Q 0.5 0.000000 0.755453 0.117790\n end\nend\n{t}")),
        ("newjob_xyz_mass", format!("{w}{t}$new_job\n! SP\n* xyz 0 1\nO 0.000000 0.000000 0.117790 M = 15.9949\n*\n")),
        ("newjob_int", format!("{w}{t}$new_job\n! SP\n* int 0 1\nO 0 0 0 0 0 0\nH 1 0 0 0.755453 0 0\n*\n")),
        ("newjob_gzmt", format!("{w}{t}$new_job\n! SP\n*gzmt 0 1\nO\nH 1 0.755453\n*\n")),
        ("primary_trailing_comment", format!("{w}* xyz 0 1\nO 0.000000 0.000000 0.117790 # oxygen\nH 0.000000 0.755453 -0.471161\n*\n")),
        ("primary_mass", format!("{w}* xyz 0 1\nO 0.000000 0.000000 0.117790 M = 15.9949\nH 0 0 1\n*\n")),
        ("primary_ghost", format!("{w}* xyz 0 1\nO 0.000000 0.000000 0.117790\nH : 0.000000 0.755453 -0.471161\n*\n")),
        ("primary_point_charge", format!("{w}* xyz 0 1\nO 0.000000 0.000000 0.117790\nQ 0.5 0.000000 0.755453 -0.471161\n*\n")),
        ("primary_fortran_d", format!("{w}* xyz 0 1\nO 0.000000 0.000000 0.117790d0\nH 0 0 1\n*\n")),
        ("primary_comma", format!("{w}* xyz 0 1\nO 0.000000, 0.000000, 0.117790\nH 0 0 1\n*\n")),
        ("primary_no_closer", format!("{w}* xyz 0 1\nO 0.000000 0.000000 0.117790\nH 0.000000 0.755453 -0.471161\n")),
        ("scan_eq_row", format!("{w}%geom Scan\n B 0 1 = 0.755453, 0.117790, 10\n end\nend\n{t}")),
        ("lexquote_coords_mass", format!("{w}%coords \"x\n Coords\n  O 0.000000 0.000000 0.117790 M = 15.9949\n end\nend\n{t}")),
        ("lexglued_coords_mass", format!("{w}%coords x\"y\"\n Coords\n  O 0.000000 0.000000 0.117790 M = 15.9949\n end\nend\n{t}")),
        ("end_then_coords_internal_eq", format!("{w}%scf MaxIter 100 end %coords\n CTyp internal\n pardef\n  rOH = 0.755453;\n end\n Coords\n  O 0 0 0 0 0 0\n  H 1 0 0 {{rOH}} 0 0 M = 2.014\n end\nend\n{t}")),
        ("coords_on_bang_line", format!("! SP %coords\n Coords\n  O 0.000000 0.000000 0.117790 M = 15.9949\n end\nend\n{t}")),
        ("primary_long_row", format!("{w}* xyz 0 1\nO 0.000000 0.000000 0.117790 {}\nH 0 0 1\n*\n", "x".repeat(50_000))),
        ("cm_malformed", format!("{w}* xyz 0 0.117790\nH 0 0 0\nH 0 0 0.74\n*\n")),
        ("maxcore_numbers", format!("{w}%maxcore 0.117790 0.755453 0.471161\n{t}")),
    ]
}

/// Every attack input: nothing of the geometry reaches the model, in the view or in a refusal, and
/// every result is small. NEGATIVE CONTROL (`ctl:build_bypass`): serve the raw header text instead
/// of the built one → red.
#[test]
fn no_attack_input_reaches_the_model() {
    let db = TestDb::new("tools-header-attack");
    for (id, input) in attack_matrix() {
        let out = served(&db, id, &input);
        assert_eq!(secrets_in(&out), Vec::<&str>::new(), "[ctl:build_bypass] {id} leaks: {out}");
        assert!(out.len() < 8_000, "{id}: {} bytes", out.len());
    }
}

/// Every error and reason in the view is a kind and a line, never the input's text. NEGATIVE
/// CONTROLS: the charge error's `Display` (`ctl:error_quotes_charge`), the `%maxcore` error with the
/// line (`ctl:error_quotes_maxcore`), or the row error with the row (`ctl:error_quotes_row`, the
/// round-2 behaviour) → red.
#[test]
fn errors_are_a_kind_and_a_line_never_source_text() {
    let db = TestDb::new("tools-header-errors");
    for (id, input) in attack_matrix().into_iter().filter(|(id, _)| id.starts_with("primary_") || id.starts_with("cm_") || id.starts_with("maxcore_")) {
        JobRow { input: &input, ..JobRow::new(id) }.insert(&db.conn);
        let v = get_job(&db.conn, id).unwrap();
        let tag = match id {
            id if id.starts_with("cm_") => "[ctl:error_quotes_charge]",
            id if id.starts_with("maxcore_") => "[ctl:error_quotes_maxcore]",
            _ => "[ctl:error_quotes_row]",
        };
        for why in [&v.header_withheld, &v.charge_mult.error].into_iter().flatten() {
            assert_eq!(secrets_in(why), Vec::<&str>::new(), "{tag} {id}: {why}");
            assert!(why.len() <= 200, "{tag} {id}: {why}");
        }
    }
    let v = get_job(&db.conn, "primary_mass").unwrap();
    assert_eq!(v.header, None);
    assert_eq!(v.header_withheld.as_deref(), Some("the coordinate block: line 3: a coordinate row is not an element and three numbers"));
    let cm = get_job(&db.conn, "cm_malformed").unwrap().charge_mult.error;
    assert_eq!(cm.as_deref(), Some("line 2: the `* xyz` line is not `* xyz <charge> <multiplicity>` with two integers"), "[ctl:error_quotes_charge]");
    let mc = get_job(&db.conn, "maxcore_numbers").unwrap().header_withheld;
    assert!(mc.as_deref().is_some_and(|m| m.contains("%maxcore line does not read")), "[ctl:error_quotes_maxcore] {mc:?}");
}

/// The header an app-built constrained Opt gives: keywords as parsed, `%maxcore` from the strict
/// reader, `%pal nprocs` from the display-only list, the constraints re-emitted, no main block.
#[test]
fn an_app_built_constrained_opt_header() {
    let input = "! B3LYP D3BJ def2-SVP Opt\n%maxcore 2000\n%pal nprocs 8 end\n%geom\n  Constraints\n    {B 0 1 1.234 C}\n    {A 0 1 2 C}\n    {D 0 1 2 3 90.0 C}\n  end\nend\n* xyz 0 1\nC 0.0 0.0 0.0\nC 1.5 0.0 0.0\nC 2.0 1.4 0.0\nC 3.5 1.4 0.0\n*\n";
    assert_eq!(
        reconstruct(input).unwrap(),
        "! B3LYP D3BJ def2-SVP Opt\n%maxcore 2000\n%pal\n  nprocs 8\nend\n%geom\n  Constraints\n    {B 0 1 1.234 C}\n    {A 0 1 2 C}\n    {D 0 1 2 3 90.0 C}\n  end\nend"
    );
}

/// The verifier's false positives are shown: constraint entries (with or without a target), and the
/// `%freq Temp` and `%elprop Origin` keys by name. NEGATIVE CONTROLS: a constraint with a value
/// withheld (`ctl:fp_constraint`), `%freq` or `%elprop` treated as geometry (`ctl:fp_freq`,
/// `ctl:fp_elprop`) → red.
#[test]
fn ordinary_app_headers_are_shown() {
    let db = TestDb::new("tools-header-fp");
    let cases = [
        ("fp_constraint_angle", format!("{W}%geom Constraints\n  {{A 0 1 2 C}}\n  {{B 0 1 C}}\n end\nend\n{T}"), vec!["{A 0 1 2 C}", "{B 0 1 C}"], "[ctl:fp_constraint]"),
        ("fp_constraint_dihedral", format!("{W}%geom Constraints\n  {{D 0 1 2 3 C}}\n end\nend\n{T}"), vec!["{D 0 1 2 3 C}"], "[ctl:fp_constraint]"),
        ("fp_constraint_value", format!("{W}%geom Constraints\n  {{B 0 1 1.234 C}}\n end\nend\n{T}"), vec!["{B 0 1 1.234 C}"], "[ctl:fp_constraint]"),
        ("constraint_value", format!("{W}%geom Constraints\n {{B 0 1 0.755453 C}}\n end\nend\n{T}"), vec!["{B 0 1 0.755453 C}"], "[ctl:fp_constraint]"),
        ("fp_freq_temps", format!("{W}%freq\n Temp 298.15 310.0 330.0 350.0\nend\n{T}"), vec!["%freq", "Temp  # values not shown"], "[ctl:fp_freq]"),
        ("fp_elprop_origin", format!("{W}%elprop\n Origin 0.0 0.0 0.0 0.0\nend\n{T}"), vec!["%elprop", "Origin  # values not shown"], "[ctl:fp_elprop]"),
        ("fp_scf", format!("{W}%scf\n MaxIter 200\n TolE 1e-8\nend\n{T}"), vec!["%scf", "MaxIter 200", "TolE  # values not shown"], "fp_scf"),
        ("fp_ts_active", format!("{W}%geom\n TS_Active_Atoms {{ 0 1 2 3 }} end\nend\n{T}"), vec!["TS_Active_Atoms  # values not shown"], "fp_ts_active"),
        ("fp_hybrid_hess", format!("{W}%geom\n Hybrid_Hess {{ 0 1 2 3 }} end\nend\n{T}"), vec!["Hybrid_Hess  # values not shown"], "fp_hybrid_hess"),
        ("fp_basis", format!("{W}%basis\n Basis \"def2-SVP\"\n NewGTO 1 \"def2-TZVP\" end\nend\n{T}"), vec!["%basis", "Basis  # values not shown", "NewGTO  # values not shown"], "fp_basis"),
        ("fp_fragments", format!("{W}%geom\n Fragments\n  1 {{0:3}} end\n  2 {{4:6}} end\n end\nend\n{T}"), vec!["%geom", "Fragments"], "fp_fragments"),
    ];
    for (id, input, shown, tag) in cases {
        JobRow { input: &input, ..JobRow::new(id) }.insert(&db.conn);
        let header = get_job(&db.conn, id).unwrap_or_else(|e| panic!("{tag} {id}: {e}")).header;
        let header = header.unwrap_or_else(|| panic!("{tag} {id}: the header is withheld"));
        for s in shown {
            assert!(header.contains(s), "{tag} {id}: `{s}` not shown in:\n{header}");
        }
        assert!(!header.contains("not shown") || !header.contains("geometry"), "{tag} {id}: marked as geometry:\n{header}");
    }
}

/// An allowlisted key's admitted value is shown; any value outside the allowlist is not. NEGATIVE
/// CONTROLS: drop the allowlisted value (`ctl:allowlisted_values`), show a non-allowlisted block's
/// values (`ctl:other_values`) → red.
#[test]
fn allowlisted_values_are_shown_and_others_are_not() {
    let input = format!("{W}%geom\n  Calc_Hess true\n  MaxIter 200\nend\n%cpcm\n  smd true\n  SMDsolvent \"water\"\nend\n%scf\n  TolE 0.117790 0.755453\nend\n%elprop Origin 0.117790 0.755453 0.471161 end\n{T}");
    let h = reconstruct(&input).unwrap();
    for s in ["Calc_Hess true", "MaxIter 200", "smd true", "SMDsolvent \"water\""] {
        assert!(h.contains(s), "[ctl:allowlisted_values] `{s}` not in:\n{h}");
    }
    assert_eq!(secrets_in(&h), Vec::<&str>::new(), "[ctl:other_values] {h}");
    assert!(h.contains("%scf\n  TolE  # values not shown\nend"), "[ctl:other_values] {h}");
    // An allowlisted key with a value outside its rule shows the key only.
    assert!(reconstruct(&format!("{W}%geom MaxIter 0.5 end\n{T}")).unwrap().contains("MaxIter  # value not shown"));
}

/// The geometry constructs are named markers with a line, never content.
#[test]
fn geometry_constructs_are_markers() {
    let h = reconstruct(&format!("{W}%coords\n Coords\n  O 0.0 0.0 0.117790\n end\nend\n{T}")).unwrap();
    assert!(h.contains("# a geometry block (%coords) at line 2, not shown"), "{h}");
    let h = reconstruct(&format!("{W}{T}$new_job\n! SP\n* xyz 0 1\nO 0 0 0.117790\n*\n")).unwrap();
    assert!(h.ends_with("# a second job at line 6, not shown") && !h.contains("! SP"), "{h}");
    let h = reconstruct(&format!("{W}%pal nprocs 4 end %coords\n Coords\nend\n{T}")).unwrap();
    assert!(h.contains("# line 2: outside the header grammar (text after a block's `end`), not shown"), "{h}");
    // A run of refused lines with one reason is one marker.
    assert!(h.contains("# lines 3–4: outside the header grammar (a line that is not"), "{h}");
}

/// The detector: a `*` line, or a label followed by three numbers **at any position** (ORCA's input
/// echo, measured in a real `output.out`; `geometry: O x y z`). SCF rows (numbers only) are not rows.
/// The `{` and `=` skips hold for the built header only. NEGATIVE CONTROLS: the first-token-only check
/// (`ctl:tripwire_any_position`), the element-only check (`ctl:tripwire_widened`), the `{` skip
/// (`ctl:skip_brace_all`) or the `=` skip (`ctl:skip_eq_all`) widened to every field → red.
#[test]
fn the_tripwire_detector_sees_rows_anywhere_and_skips_only_in_the_header() {
    // In text, `*` counts only as a geometry opener (Anton, round 5); in the header, any `*` line.
    for opener in ["* xyz 0 1", "*xyz 0 1", "*  XYZFILE 0 1 a.xyz", "* int 0 1", "*gzmt 0 1", "|  4> * xyz 0 1"] {
        assert!(coordinate_row(opener, Field::Text), "[ctl:opener_text] {opener}");
    }
    for banner in ["***ERROR***", "*****************", "***Turning on DIIS***", "* The job aborted *", "* xyzzy"] {
        assert!(!coordinate_row(banner, Field::Text), "[ctl:bare_star_text] {banner}");
        assert!(coordinate_row(banner, Field::Header), "{banner}");
    }
    for row in ["O 0.0 0.0 0.117790", "H 1 2 0 0.963217 104.471280 0", "O(1) 0.0 0.0 0.1", "H: 0.0 0.75 -0.47", "O : 5.75 6.49 5.41"] {
        assert!(coordinate_row(row, Field::Text), "[ctl:tripwire_widened] {row}");
    }
    for row in ["|  6> C     4.10220628   -0.56008055    0.58413357", "geometry: O 0.0 0.0 0.117790"] {
        assert!(coordinate_row(row, Field::Text) && coordinate_row(row, Field::Header), "[ctl:tripwire_any_position] {row}");
    }
    for line in ["    1    -978.8410613128917248     0.00e+00  8.38e-03  2.01e-01  2.38e-01  0.700   4.4", "MaxIter 200", "! B3LYP def2-SVP Opt"] {
        assert!(!coordinate_row(line, Field::Text), "{line}");
    }
    // The skips: header only.
    for line in ["{B 0 1 1.234 C}", "{D 0 1 2 3 C}", " B 0 1 = 1.0, 2.0, 10"] {
        assert!(!coordinate_row(line, Field::Header), "{line}");
    }
    assert!(coordinate_row("{ O 0.0 0.0 0.117790 }", Field::Text), "[ctl:skip_brace_all]");
    assert!(coordinate_row("O 0.0 0.0 0.117790 M = 15.9949", Field::Text), "[ctl:skip_eq_all]");
}

/// The backstop: a row that escaped every field guard (injected into the built header after the
/// guard) refuses the view. NEGATIVE CONTROL (`ctl:tripwire`): the backstop passes everything → red.
#[test]
fn the_backstop_refuses_a_row_that_escaped_the_guards() {
    let db = TestDb::new("tools-header-tripwire");
    JobRow::new("ok").insert(&db.conn);
    let mut view = get_job(&db.conn, "ok").unwrap();
    view.header = Some(format!("{}\nO 0.000000 0.000000 0.117790", view.header.unwrap_or_default()));
    assert!(matches!(tripwire(view), Err(ToolError::Refused(why)) if why.contains("`.header`")), "[ctl:tripwire]");
}

/// A realistic failed job: the stored error is ORCA's message plus the last 20 lines of
/// `output.out` (`local_backend::detect_completion`), with SCF rows and `***` banners. The error is
/// **served** (a banner is not a geometry opener, Anton round 5); the row-shaped title is withheld and
/// the rest of the view served. An error holding an echoed `|  4> * xyz 0 1` is withheld, in `get_job`
/// and in `list_jobs`. NEGATIVE CONTROLS: the guard passes the field through, so the whole view is
/// refused (`ctl:field_not_view`); the bare `*` rule back for text (`ctl:bare_star_text`); the opener
/// rule removed for text (`ctl:opener_text`) → red.
#[test]
fn a_failed_jobs_scf_tail_withholds_the_field_not_the_view() {
    let db = TestDb::new("tools-header-scf-tail");
    let tail = "ORCA did not terminate normally (exit code 1).\n\
       ----------------------------------------------------------------------------\n\
       ITER       Energy         Delta-E        Max-DP      RMS-DP      [F,P]     Damp\n\
         0   -976.4210935410   0.000000000000 0.13025648  0.00423498  0.4251271 0.7000\n\
         1   -978.8410613129  -2.419967771852 0.00838311  0.00201000  0.2380000 0.7000\n\
                                     ***Turning on DIIS***\n\
         2   -979.0101004955  -0.169039182606 0.00674000  0.00196000  0.0972000 0.0000\n\
       ****ERROR**** SCF NOT CONVERGED AFTER 125 CYCLES\n\
       *****************************************************\n\
       * The job aborted after the SCF failed to converge *\n\
       *****************************************************";
    JobRow { status: "failed", error: Some(tail), title: "O 0.0 0.0 0.117790", ..JobRow::new("scf") }.insert(&db.conn);
    let view = get_job(&db.conn, "scf").expect("[ctl:field_not_view] the view is served");
    assert_eq!(view.error.as_deref(), Some(tail), "[ctl:bare_star_text] the SCF tail is served");
    assert_eq!(view.title, WITHHELD, "[ctl:field_not_view]");
    assert_eq!(view.withheld_fields, ["title"], "[ctl:bare_star_text]");
    assert_eq!(view.header.as_deref(), Some("! SP"), "the rest is served");
    // `list_jobs` guards its error excerpt and title the same way (`ctl:list_excerpt_guard`).
    let page = list_jobs(&db.conn, &JobFilter::default(), None, None).unwrap();
    assert_eq!(page.jobs[0].withheld_fields, ["title"], "[ctl:bare_star_text]");
    assert!(page.jobs[0].error_excerpt.as_deref().is_some_and(|e| e.starts_with("ORCA did not terminate")), "[ctl:bare_star_text]");

    // An error whose tail ends in ORCA's echo of the input, cut right after the `* xyz` opener.
    let echo = "ORCA did not terminate normally (exit code 1).\n|  1> ! B3LYP def2-SVP Opt\n|  2> %maxcore 2000\n|  3> \n|  4> * xyz 0 1";
    JobRow { status: "failed", error: Some(echo), created_at: "2026-11-20 10:30:00", ..JobRow::new("echo") }.insert(&db.conn);
    let view = get_job(&db.conn, "echo").expect("served");
    assert_eq!((view.error.as_deref(), view.withheld_fields.as_slice()), (Some(WITHHELD), &["error".to_string()][..]), "[ctl:opener_text]");
    let page = list_jobs(&db.conn, &JobFilter::default(), None, None).unwrap();
    assert_eq!(page.jobs[0].id, "echo");
    assert_eq!(page.jobs[0].error_excerpt.as_deref(), Some(WITHHELD), "[ctl:opener_text] [ctl:list_excerpt_guard]");
    // An error with a row only past the excerpt's cut is still withheld: the guard reads it whole.
    let late = format!("{}\nC 0.0 0.0 0.117790", "x".repeat(1_000));
    JobRow { status: "failed", error: Some(&late), created_at: "2026-11-20 11:00:00", ..JobRow::new("late") }.insert(&db.conn);
    let page = list_jobs(&db.conn, &JobFilter::default(), None, None).unwrap();
    assert_eq!(page.jobs[0].error_excerpt.as_deref(), Some(WITHHELD), "[ctl:list_excerpt_guard] {}", page.jobs[0].id);
}

/// `!` tokens that look like numbers are hidden; method, basis and job type stay. NEGATIVE CONTROL
/// (`ctl:bang_numbers`): the tokens shown verbatim → red (the `M = …` form escapes the header's `=`
/// skip, and the plain form withholds the whole header instead of hiding the values).
#[test]
fn numeric_bang_tokens_are_hidden() {
    let db = TestDb::new("tools-header-bang");
    let plain = format!("! B3LYP D3 def2-SVP Opt O 0.000000 0.000000 0.117790\n{T}");
    let mass = format!("! SP O 0.000000 0.000000 0.117790 M = 15.9949\n{T}");
    JobRow { input: &plain, ..JobRow::new("plain") }.insert(&db.conn);
    JobRow { input: &mass, ..JobRow::new("mass") }.insert(&db.conn);
    let p = get_job(&db.conn, "plain").unwrap();
    assert_eq!(
        p.header.as_deref(),
        Some("! B3LYP D3 def2-SVP Opt O [value not shown] [value not shown] [value not shown]"),
        "[ctl:bang_numbers]"
    );
    let m = served(&db, "mass2", &mass);
    assert_eq!(secrets_in(&m), Vec::<&str>::new(), "[ctl:bang_numbers] {m}");
    assert!(get_job(&db.conn, "mass").unwrap().header.is_some_and(|h| h.contains("M = [value not shown]")), "[ctl:bang_numbers]");
}

/// The display-only list (separate from M1a's SUBMITTED allowlist) shows its integer values, and
/// hides any other value of the same keys. NEGATIVE CONTROLS: a display-list value dropped
/// (`ctl:display_dropped`), a non-integer value shown (`ctl:display_non_integer`) → red.
#[test]
fn the_display_list_shows_integers_only() {
    let h = reconstruct(&format!("{W}%pal nprocs 8 end\n%scf\n  MaxIter 300\nend\n%neb NImages 12 end\n%geom Recalc_Hess 5 end\n{T}")).unwrap();
    for shown in ["  nprocs 8", "  MaxIter 300", "  NImages 12", "  Recalc_Hess 5"] {
        assert!(h.contains(shown), "[ctl:display_dropped] `{shown}` not in:\n{h}");
    }
    let h = reconstruct(&format!("{W}%pal nprocs 0.117790 end\n%scf MaxIter 0.755453 0.471161 end\n%neb NImages \"x\" end\n{T}")).unwrap();
    assert_eq!(secrets_in(&h), Vec::<&str>::new(), "[ctl:display_non_integer] {h}");
    for hidden in ["  nprocs  # value not shown", "  MaxIter  # value not shown", "  NImages  # value not shown"] {
        assert!(h.contains(hidden), "[ctl:display_non_integer] `{hidden}` not in:\n{h}");
    }
    // M1a's SUBMITTED allowlist is unchanged: `%pal` is still P2/R3 for the classifier.
    let parsed = crate::mcp::header::parse_header("%pal nprocs 8 end");
    assert!(!crate::mcp::allowlist::allowlist_findings(&parsed).is_empty());
}

/// `TS_Mode {B 0 1} end` inside `%geom` is a known limitation of the shared grammar (`header.rs`
/// knows only `Constraints` and `Scan` as sub-blocks): its `end` closes `%geom` early. The built
/// header stays safe: the key is named without its value and the stray `end` is a marker.
#[test]
fn ts_mode_is_named_and_the_stray_end_is_a_marker() {
    let h = reconstruct(&format!("{W}%geom\n  TS_Mode {{B 0 1}} end\n  MaxIter 50\nend\n{T}")).unwrap();
    assert!(h.contains("  TS_Mode  # values not shown"), "{h}");
    assert!(h.contains("outside the header grammar"), "{h}");
}
