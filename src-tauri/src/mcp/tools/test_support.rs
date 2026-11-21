//! Shared helpers for the read tools' tests: the unit-name walk (rule #11), the size check, and
//! row builders on the migrated scratch database.

use std::io::Write;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::{json, Value};

use super::{result_text, size_of_text, CLIENT_SPILL_CHARS, RESULT_CAP_BYTES};

/// The unit suffixes a numeric field may end in (rule #11), and the names of counts and indices.
const UNIT_SUFFIXES: [&str; 11] = ["_eh", "_ang", "_deg", "_cm1", "_km_mol", "_k", "_s", "_mib", "_kib", "_bytes", "_byte"];
const DIMENSIONLESS_SUFFIXES: [&str; 3] = ["_count", "_index", "_indices"];

/// Every object key whose value is a number, or an array of numbers, and whose name carries no unit
/// suffix and is not a count or an index — with its path. `extra` names further dimensionless keys
/// the caller allows (each test lists its own).
pub fn unitless_numeric_keys(v: &Value, extra: &[&str]) -> Vec<String> {
    /// A number, or an array whose leaves, at any depth of nesting, are all numbers or nulls, with at
    /// least one number (`[[0, 1, 2]]` is numeric).
    fn numeric(v: &Value) -> bool {
        fn leaves(v: &Value, any: &mut bool) -> bool {
            match v {
                Value::Number(_) => {
                    *any = true;
                    true
                }
                Value::Null => true,
                Value::Array(items) => items.iter().all(|i| leaves(i, any)),
                _ => false,
            }
        }
        let mut any = false;
        matches!(v, Value::Number(_) | Value::Array(_)) && leaves(v, &mut any) && any
    }
    fn walk(v: &Value, path: &str, extra: &[&str], out: &mut Vec<String>) {
        match v {
            Value::Object(map) => {
                for (k, child) in map {
                    let here = format!("{path}.{k}");
                    let named = UNIT_SUFFIXES.iter().chain(DIMENSIONLESS_SUFFIXES.iter()).any(|s| k.ends_with(s)) || extra.contains(&k.as_str());
                    if numeric(child) && !named {
                        out.push(here.clone());
                    }
                    walk(child, &here, extra, out);
                }
            }
            Value::Array(items) => {
                for (i, child) in items.iter().enumerate() {
                    walk(child, &format!("{path}[{i}]"), extra, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, "", extra, &mut out);
    out.sort();
    out
}

/// The serialised length of a result, as the caps count it (UTF-8 bytes).
pub fn size_of<T: Serialize>(value: &T) -> usize {
    size_of_text(&result_text(value).unwrap())
}

/// The size assertion every "largest result" test makes: at or under the cap, so under the
/// client's threshold.
pub fn assert_fits<T: Serialize>(value: &T, tag: &str) {
    let n = size_of(value);
    assert!(n <= RESULT_CAP_BYTES && n < CLIENT_SPILL_CHARS, "{tag}: the result is {n} bytes, over the {RESULT_CAP_BYTES} cap");
}

/// A 36-character id (a UUID's length), `stem` padded.
pub fn id36(stem: &str) -> String {
    format!("{stem:-<36}")
}

/// A job row with every field the projections read.
pub struct JobRow<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub input: &'a str,
    pub status: &'a str,
    pub origin: &'a str,
    pub group_id: Option<&'a str>,
    pub backend_id: Option<&'a str>,
    pub created_at: &'a str,
    pub energy: Option<f64>,
    pub error: Option<&'a str>,
    pub job_dir: Option<&'a str>,
}

impl<'a> JobRow<'a> {
    pub fn new(id: &'a str) -> Self {
        JobRow {
            id,
            title: "t",
            input: "! SP\n* xyz 0 1\nH 0 0 0\nH 0 0 0.74\n*\n",
            status: "parsed",
            origin: "user",
            group_id: None,
            backend_id: None,
            created_at: "2026-11-20 10:00:00",
            energy: None,
            error: None,
            job_dir: None,
        }
    }

    pub fn insert(&self, conn: &Connection) {
        conn.execute(
            "INSERT INTO jobs (id, title, input_content, status, origin, group_id, backend_id, created_at, energy, error_message, job_dir) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                self.id,
                self.title,
                self.input,
                self.status,
                self.origin,
                self.group_id,
                self.backend_id,
                self.created_at,
                self.energy,
                self.error,
                self.job_dir
            ],
        )
        .unwrap();
    }
}

/// Stored results for job `id` (the `data_json` of `results`).
pub fn store_results(conn: &Connection, id: &str, data: &Value) {
    conn.execute(
        "INSERT INTO results (job_id, data_json, parser_version) VALUES (?1, ?2, 6)",
        params![id, data.to_string()],
    )
    .unwrap();
}

/// A molecule row from xyz text.
pub fn molecule(conn: &Connection, id: &str, xyz: &str, charge: i64, multiplicity: i64) {
    conn.execute(
        "INSERT INTO molecules (id, name, xyz, charge, multiplicity) VALUES (?1, 'm', ?2, ?3, ?4)",
        params![id, xyz, charge, multiplicity],
    )
    .unwrap();
}

// ----- the Part A fixtures, shared with the end-to-end tests (`server/read_tools_tests.rs`) -----

/// A real input: the app's `%maxcore`, a `%pal` block and an inline `* xyz` block.
pub const WATER_INPUT: &str = "! B3LYP def2-SVP Opt\n%maxcore 2000\n%pal nprocs 4 end\n* xyz 0 1\n\
O 0.000000 0.000000 0.117790\nH 0.000000 0.755453 -0.471161\nH 0.000000 -0.755453 -0.471161\n*\n";

/// Every coordinate value of [`WATER_INPUT`] that a leak would carry.
pub const WATER_COORDS: [&str; 3] = ["0.117790", "0.755453", "0.471161"];

/// The largest `get_job` (`jobs_tests`): a built header of 1,500 named keys (about 45,000 bytes
/// before its cut), a 5,000-character title, a 50,000-character error and 100 long reasons.
pub fn largest_get_job(conn: &Connection, id: &str) {
    let keys: String = (0..1_500).map(|i| format!(" Key{i:04} 1\n")).collect();
    let input = format!("! B3LYP def2-SVP\n%scf\n{keys}end\n{}", &WATER_INPUT[WATER_INPUT.find("* xyz").unwrap()..]);
    let reasons: Vec<Value> = (0..100).map(|i| json!({"row": format!("P{i}"), "message": "r".repeat(5_000)})).collect();
    JobRow { input: &input, title: &"T".repeat(5_000), error: Some(&"e".repeat(50_000)), ..JobRow::new(id) }.insert(conn);
    conn.execute("UPDATE jobs SET origin = 'mcp', mcp_reasons = ?1 WHERE id = ?2", params![Value::Array(reasons).to_string(), id])
        .unwrap();
}

/// The largest `list_jobs` page (`jobs_tests`): 80 jobs with 36-character ids, group and profile
/// ids, 5,000-character titles and errors.
pub fn largest_list_jobs(conn: &Connection) {
    let (group, profile) = (id36("group"), id36("profile"));
    conn.execute("INSERT INTO groups (id, name) VALUES (?1, 'g')", [&group]).unwrap();
    conn
        .execute(
            "INSERT INTO server_profiles (id, name, host, remote_orca_path, remote_scratch_dir) VALUES (?1, 'p', 'uni', '/opt/orca/orca', '/s')",
            [&profile],
        )
        .unwrap();
    let (title, error) = ("T".repeat(5_000), "E".repeat(5_000));
    for i in 0..80 {
        let id = id36(&format!("job{i:02}"));
        JobRow {
            title: &title,
            error: Some(&error),
            status: "cancelled",
            group_id: Some(&group),
            backend_id: Some(&profile),
            energy: Some(-1234.567_890_123_456),
            ..JobRow::new(&id)
        }
        .insert(&conn);
    }
}

/// A results row in the shape `parse_and_store` writes (canonical units), with frequencies (two
/// imaginary), thermochemistry, a distance scan and a NEB run. `n` sizes the long lists.
pub fn results_json(n_scan: usize, n_mep: usize, n_imag: usize, n_warn: usize, scan_unit: &str) -> Value {
    let mut freqs: Vec<f64> = (0..n_imag).map(|i| -100.0 - i as f64).collect();
    freqs.extend([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1650.2, 3700.1, 3800.4]);
    let intensities: Vec<f64> = freqs.iter().map(|f| f.abs() / 10.0).collect();
    json!({
        "parser_version": 6,
        "final_energy_eh": -76.408_912_345,
        "charges": [],
        "final_geometry": {"elements": ["O", "H", "H"], "xyz_angstrom": [[0.0, 0.0, 0.1], [0.0, 0.75, 0.6], [0.0, -0.75, 0.6]]},
        "frequencies": {
            "elements": ["O", "H", "H"],
            "frequencies_cm": freqs,
            "imaginary_count": n_imag,
            "zero_count": 6,
            "is_linear": false,
            "ir_intensity_km_mol": intensities,
            "n_modes": 9,
            "normal_modes": [],
            "temperature_k": 298.15,
            "scale_factor": null,
            "unknown_sections": []
        },
        "thermochemistry": {
            "temperature_k": 298.15,
            "el_energy_eh": -76.4089,
            "zpe_eh": 0.0213,
            "inner_energy_u_eh": -76.3848,
            "enthalpy_h_eh": -76.3839,
            "t_times_s_eh": 0.0214,
            "free_energy_g_eh": -76.4053
        },
        "scan": {
            "kind": "B",
            "atoms": [0, 1],
            "coordinate_unit": scan_unit,
            "points": (0..n_scan).map(|i| json!({"coordinate": 1.0 + i as f64 * 0.01, "energy_act_eh": -76.4 + i as f64 * 1e-4, "energy_scf_eh": -76.3 + i as f64 * 1e-4})).collect::<Vec<_>>()
        },
        "neb": {
            "iterations": [{"index": 0, "images": [], "barrier_eh": 0.05, "climbing_image": null}, {"index": 1, "images": [], "barrier_eh": 0.04, "climbing_image": 4}],
            "mep": (0..n_mep).map(|i| json!({"distance_angstrom": i as f64 * 0.1, "energy_eh": (i as f64 * 0.3).sin() * 0.04})).collect::<Vec<_>>(),
            "final_barrier_eh": 0.04,
            "ts_geometry": {"elements": ["O", "H", "H"], "xyz_angstrom": [[0.0, 0.0, 0.1], [0.0, 0.75, 0.6], [0.0, -0.75, 0.6]]},
            "ts_energy_eh": -76.36
        },
        "unknown_blocks": [],
        "converged": true,
        "parse_warnings": (0..n_warn).map(|i| format!("warning {i}: {}", "w".repeat(5_000))).collect::<Vec<_>>()
    })
}

/// Molecule `id` with `n` atoms on a wide grid (coordinates up to −999 Å, the widest text).
pub fn grid_molecule(conn: &Connection, id: &str, n: usize) {
    let mut xyz = format!("{n}\ngrid\n");
    for i in 0..n {
        let f = -999.0 + i as f64 * 1.000_000_01;
        xyz.push_str(&format!("C {f:.8} {:.8} {:.8}\n", f + 0.5, f - 0.5));
    }
    molecule(conn, id, &xyz, 0, 1);
}

/// A job `id` whose `output.out`, under `root/jobs/<id>/`, holds `lines`; returns the file's path.
pub fn output_job(conn: &Connection, root: &Path, id: &str, lines: impl Iterator<Item = String>) -> PathBuf {
    let dir = root.join("jobs").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("output.out");
    let mut f = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
    f.flush().unwrap();
    JobRow { job_dir: dir.to_str(), ..JobRow::new(id) }.insert(conn);
    path
}

/// The largest tail's output (`output_tests`): 2,000 lines of 1,000 characters.
pub fn largest_tail_lines() -> impl Iterator<Item = String> {
    (0..2_000).map(|i| format!("{i:04}{}", "w".repeat(996)))
}

/// The largest search page's output (`output_tests`): 2,400 lines of about 1,000 characters, every
/// fourth a `HIT`.
pub fn largest_search_lines() -> impl Iterator<Item = String> {
    (0..2_400).map(|i| format!("{} {i:04} {}", if i % 4 == 0 { "HIT" } else { "ctx" }, "w".repeat(990)))
}

/// Index `sections` (file, title, breadcrumb, body) as the ingest would, then rebuild the FTS table.
pub fn index_manual(conn: &Connection, sections: &[(String, String, Vec<String>, String)]) {
    for (i, (file, title, crumbs, body)) in sections.iter().enumerate() {
        conn.execute(
            "INSERT INTO manual_sections (orca_version, file, level, title, breadcrumb, labels, anchor, anchor_source, body_md, line_start, line_end) \
             VALUES ('6.1', ?1, 2, ?2, ?3, '[]', ?4, 'undetermined', ?5, ?6, ?6)",
            params![file, title, serde_json::to_string(crumbs).unwrap(), format!("sec-{i}"), body, i as i64],
        )
        .unwrap();
    }
    conn.execute_batch("INSERT INTO manual_fts(manual_fts) VALUES('rebuild');").unwrap();
}

/// The largest `search_manual` (`manual_tests`): 60 sections matching `basis`, with
/// 5,000-character titles, files and breadcrumbs, and words of 3,000 characters in the body.
pub fn largest_manual(conn: &Connection) {
    let long = |c: &str| c.repeat(5_000);
    let body = format!("basis {} set {} basis", "b".repeat(3_000), "c".repeat(3_000));
    let sections: Vec<_> = (0..60)
        .map(|i| (format!("{i}{}", long("f")), format!("basis {i} {}", long("t")), vec![long("x"); 20], body.clone()))
        .collect();
    index_manual(conn, &sections);
}

/// The largest `list_run_targets` (`targets_tests`): 150 profiles with 200-character hosts and
/// 5,000-character names.
pub fn largest_profiles(conn: &Connection) {
    for i in 0..150 {
        conn.execute(
            "INSERT INTO server_profiles (id, name, host, remote_orca_path, remote_scratch_dir, availability_window) VALUES (?1, ?2, ?3, '/opt/orca/orca', '/s', '22:00-08:00')",
            params![id36(&format!("p{i}")), "N".repeat(5_000), format!("h{i}{}", "h".repeat(200))],
        )
        .unwrap();
    }
}

/// The largest `list_groups` page (`groups_tests`): 300 groups with 36-character ids and
/// 5,000-character names under one root.
pub fn largest_groups(conn: &Connection) {
    let parent = id36("root");
    // An explicit earlier time: `datetime('now')` has whole seconds, so the root could otherwise sort
    // after a child created in the same second (`created_at, id`).
    conn.execute("INSERT INTO groups (id, name, created_at) VALUES (?1, 'root', '2000-01-01 00:00:00')", [&parent]).unwrap();
    for i in 0..300 {
        conn.execute("INSERT INTO groups (id, name, parent_id) VALUES (?1, ?2, ?3)", params![id36(&format!("g{i}")), "N".repeat(5_000), parent])
            .unwrap();
    }
}
