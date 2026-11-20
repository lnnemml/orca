//! Shared helpers for the read tools' tests: the unit-name walk (rule #11), the size check, and
//! row builders on the migrated scratch database.

use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;

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
