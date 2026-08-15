//! Group export — the PURE CORE (Part A of the group-export unit, ADR-021).
//!
//! **This module is a projection, not a mutation.** It turns an in-memory picture of a
//! job group (rows already read from SQLite by the caller) into a `ManifestV1` value and
//! the fs-safe directory names that value describes. It performs **NO filesystem I/O and
//! NO database access** — every function here is a pure function over its arguments, so it
//! is exhaustively unit-testable and deterministic. The wiring that actually reads the DB,
//! copies files, and writes `manifest.json` lives in `commands::export` (Part B).
//!
//! Why a projection and not a rename of the canonical dirs: the canonical `<UUID>/` job
//! directories and the SQLite rows are the source of truth and are NEVER touched (ADR-021,
//! domain rule #3). Export removes the *manual* rename step that historically swapped
//! `HCN-opt` ↔ `HNC-opt` by hand — so the two load-bearing invariants are identity ones:
//!
//! 1. **A title can never escape its leaf.** `slugify` maps a title containing `/`, `:`, a
//!    unicode arrow, or control chars to a flat ascii leaf with no path separator and no
//!    `..` — a `/` must never create a subdirectory.
//! 2. **The manifest encodes ONLY persisted structure.** Creation order (labelled as
//!    creation order, NOT logical order), the group-tree path, `pathway_id` membership, and
//!    the conformer-reopt FKs where present — nothing else. The fine OptTS/NEB/connectivity
//!    source DAG is deliberately not persisted (ADR-020) and is NOT asserted here; the
//!    `notes` field says so in the artifact itself. `computed_identity` is ALWAYS null in
//!    v1 (a bare formula stamp is blind to constitutional isomers HCN/HNC = CHN — false
//!    confidence; the real connectivity stamp is a follow-up).
//!
//! Honest-or-absent: in curated mode the files a job's dir contains but that the allowlist
//! does not select (`.gbw`, `.densities`, cubes, scratch `.tmp`) are RECORDED per job in
//! `files.omitted`, never dropped silently.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Inputs — plain in-memory rows the caller (Part B) hydrates from SQLite / a dir
// listing. Kept decoupled from the DB models on purpose: the pure core knows
// nothing about `rusqlite`, so it stays trivially testable.
// ---------------------------------------------------------------------------

/// The source group being exported (the manifest's `source`).
#[derive(Debug, Clone)]
pub struct GroupMeta {
    pub id: String,
    pub name: String,
}

/// One node of the group tree in the exported set — enough to reconstruct a job's
/// `group_path` by walking `parent_id` (ADR-019 adjacency list, mirrored in memory).
#[derive(Debug, Clone)]
pub struct GroupNode {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
}

/// One job to project. `present_files` is the dir listing the caller read from the
/// canonical `<UUID>/` dir (empty for a draft/never-run job whose `job_dir` is NULL —
/// such a job is recorded in the manifest but copies nothing). Everything else is a
/// straight read of the `jobs` row (ADR-019 columns).
#[derive(Debug, Clone)]
pub struct JobRow {
    pub id: String,
    pub title: String,
    /// Job kind (e.g. "opt", "neb", "scan"). `None` when not determinable — honest,
    /// serialized as null rather than guessed.
    pub job_type: Option<String>,
    pub status: String,
    pub created_at: String,
    /// `None` = draft/never-run: no canonical dir, nothing to copy. Read by Part B (the
    /// copy step) to locate the source dir; the pure core only projects, so it is unused here.
    #[allow(dead_code)]
    pub job_dir: Option<String>,
    pub group_id: Option<String>,
    pub pathway_id: Option<String>,
    pub source_ensemble_job_id: Option<String>,
    pub source_conformer_index: Option<i64>,
    /// Filenames present in the job's canonical dir (leaf names only, no path).
    pub present_files: Vec<String>,
}

/// A parsed-results row (`results` table), keyed by `job_id`. Absent for an unparsed
/// job — and then the manifest's `results` is null, never a fabricated `0.0`.
#[derive(Debug, Clone)]
pub struct ResultRow {
    pub job_id: String,
    pub energy_eh: Option<f64>,
    pub imaginary_count: Option<i64>,
}

// ---------------------------------------------------------------------------
// Manifest — the serialized artifact (ManifestV1). Derives Deserialize too so the
// Part B post-condition (rule #9) can re-read the written file and assert on it.
// No `skip_serializing_if` anywhere: a null field MUST appear as null in the JSON
// (the manifest shape shows pathway_id / results / computed_identity present-but-null).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CopyMode {
    /// Only the pinned scientific-artifact allowlist is copied; the rest is recorded
    /// in `files.omitted`.
    Curated,
    /// The whole canonical dir is copied verbatim.
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestSource {
    /// The source group's id/name. **Optional** so the single-job export can carry
    /// `null` for an ungrouped job (honest, never a fabricated group). A group export
    /// always fills `Some(...)`, which serializes to the same bare string as before — the
    /// manifest JSON of a group export is byte-identical.
    pub group_id: Option<String>,
    pub group_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestLineage {
    pub source_ensemble_job_id: Option<String>,
    pub source_conformer_index: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestResults {
    pub energy_eh: Option<f64>,
    pub imaginary_count: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestFiles {
    pub included: Vec<String>,
    pub omitted: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestJob {
    pub exported_dir: String,
    pub uuid: String,
    pub display_name: String,
    pub job_type: Option<String>,
    pub status: String,
    pub created_at: String,
    /// Group names root → leaf (creation-time tree path). Empty if ungrouped.
    pub group_path: Vec<String>,
    pub pathway_id: Option<String>,
    pub lineage: ManifestLineage,
    /// `None` when the job has no parsed-results row — never a fabricated zero.
    pub results: Option<ManifestResults>,
    pub files: ManifestFiles,
    /// ALWAYS null in v1 (see module docs) — the connectivity stamp is deferred.
    pub computed_identity: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestV1 {
    pub manifest_version: u32,
    pub exported_at: String,
    pub copy_mode: CopyMode,
    pub source: ManifestSource,
    pub jobs: Vec<ManifestJob>,
    /// Honesty preamble — states what the structure DOES and does NOT assert.
    pub notes: Vec<String>,
}

/// The single honesty note embedded in every manifest.
const HONESTY_NOTE: &str = "Order = job creation order; structure = group tree + pathway \
membership. The fine OptTS/NEB/connectivity source lineage is not persisted and is NOT \
asserted here.";

// ---------------------------------------------------------------------------
// Pure functions
// ---------------------------------------------------------------------------

/// Map a free-form job title to an fs-safe ascii leaf.
///
/// Only `[a-z0-9]` survive as themselves (lowercased); EVERY other byte — `/`, `:`, a
/// unicode arrow, control chars, `.`, whitespace — becomes a separator, and runs of
/// separators collapse to a single `-` with the ends trimmed. This is deliberately
/// aggressive: it guarantees no path separator, no `.`/`..` traversal, and no non-ascii
/// leaks into a directory name, regardless of the title. An empty result (a title with no
/// alphanumerics at all) falls back to `"job"` so the leaf is never empty.
pub fn slugify(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut pending_sep = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push('-');
            }
            pending_sep = false;
            out.push(ch.to_ascii_lowercase());
        } else {
            // Any non-alphanumeric (incl. '/', ':', '.', control, unicode) → separator.
            pending_sep = true;
        }
    }
    if out.is_empty() {
        "job".to_string()
    } else {
        out
    }
}

/// Zero-padded 1-based ordinal for a job at 0-based `index` within a group of
/// `group_size`, padded to the width of `group_size`'s decimal length so a lexical sort
/// of the resulting directory names equals creation order (`"01"`, `"02"`, … `"10"` — not
/// `"1"`, `"10"`, `"2"`).
pub fn numbered_prefix(index: usize, group_size: usize) -> String {
    let width = group_size.max(1).to_string().len();
    format!("{:0width$}", index + 1, width = width)
}

/// Whether `filename` (a leaf name, no path) is selected by the pinned curated allowlist.
///
/// Pinned from a real-machine probe (rule #10 — confirmed against actual COMPLETED, SCAN,
/// and NEB job dirs before pinning; see `wiki/modules/group-export.md`). The scientific
/// artifacts OrcaStudio's own parsers read, plus the run's input/geometry/completion
/// marker — never `.gbw`/`.densities`/`.tmp` scratch/cubes.
pub fn curated_match(filename: &str) -> bool {
    // Exact names.
    const EXACT: &[&str] = &["input.inp", "output.out", "input.xyz", ".exit_code"];
    if EXACT.contains(&filename) {
        return true;
    }
    // Suffix rules. `*_trj.xyz` subsumes the proposed `*_MEP_trj.xyz` and also catches
    // `input_MEP_ALL_trj.xyz` / `input_initial_path_trj.xyz` (all trajectory artifacts).
    // `*.property.txt` catches fragment outputs like `input_atom53.property.txt` too.
    // `*_converged.xyz` (not just `*_NEB-TS_converged.xyz`) enforces the rule "curated
    // NEVER discards a converged/final geometry": it also captures `input_NEB-CI_converged.xyz`
    // (the climbing-image converged TS) so a NEB-CI run's headline geometry is exported, not
    // relegated to `omitted`. Nothing scratch ends in `_converged.xyz`.
    const SUFFIX: &[&str] = &[
        ".property.txt",
        ".hess",
        "_trj.xyz",
        ".NEB.log",
        ".final.interp",
        "_converged.xyz",
    ];
    if SUFFIX.iter().any(|s| filename.ends_with(s)) {
        return true;
    }
    // `*.relaxscan*.dat` — a relaxed-surface-scan curve (`input.relaxscanact.dat`,
    // `input.relaxscanscf.dat`).
    if filename.contains(".relaxscan") && filename.ends_with(".dat") {
        return true;
    }
    // `input.[0-9]*.xyz` — the per-step scan geometries (`input.001.xyz` …). Requires a
    // DIGIT immediately after `input.`, so `input.xyz` (no digit) does NOT match here.
    if let Some(rest) = filename.strip_prefix("input.") {
        if rest.ends_with(".xyz") && rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    false
}

/// Partition a job's on-disk files into `{included, omitted}` for the chosen copy mode.
/// Curated: included = allowlist matches (copied), omitted = everything else present
/// (recorded, not copied — honest-or-absent). Full: everything included. Both lists are
/// sorted for a stable, diffable manifest.
fn partition_files(present_files: &[String], copy_mode: &CopyMode) -> ManifestFiles {
    let (mut included, mut omitted): (Vec<String>, Vec<String>) = match copy_mode {
        CopyMode::Full => (present_files.to_vec(), Vec::new()),
        CopyMode::Curated => present_files
            .iter()
            .cloned()
            .partition(|f| curated_match(f)),
    };
    included.sort();
    omitted.sort();
    ManifestFiles { included, omitted }
}

/// Build ONE `ManifestJob` entry from a job row (+ its optional result), the pre-computed
/// `exported_dir` and `group_path`. The single seam both `build_manifest` (group export) and
/// `build_single_job_manifest` (single-job export) construct a job entry through — so lineage,
/// results honest-null, and `computed_identity: None` are defined in exactly one place.
fn manifest_job_entry(
    job: &JobRow,
    result: Option<&ResultRow>,
    exported_dir: String,
    group_path: Vec<String>,
    copy_mode: &CopyMode,
) -> ManifestJob {
    ManifestJob {
        exported_dir,
        uuid: job.id.clone(),
        display_name: job.title.clone(),
        job_type: job.job_type.clone(),
        status: job.status.clone(),
        created_at: job.created_at.clone(),
        group_path,
        pathway_id: job.pathway_id.clone(),
        lineage: ManifestLineage {
            source_ensemble_job_id: job.source_ensemble_job_id.clone(),
            source_conformer_index: job.source_conformer_index,
        },
        results: result.map(|r| ManifestResults {
            energy_eh: r.energy_eh,
            imaginary_count: r.imaginary_count,
        }),
        files: partition_files(&job.present_files, copy_mode),
        computed_identity: None,
    }
}

/// Group names from the topmost ancestor down to the job's own group. Walks `parent_id`
/// over `by_id`, bounded by the node count so a corrupt cycle can't hang the projection.
fn group_path_for(group_id: Option<&str>, by_id: &HashMap<&str, &GroupNode>) -> Vec<String> {
    let mut chain: Vec<String> = Vec::new();
    let mut current = group_id;
    let mut steps = 0usize;
    while let Some(id) = current {
        let Some(node) = by_id.get(id) else { break };
        chain.push(node.name.clone());
        steps += 1;
        if steps > by_id.len() {
            break; // defensive: a pre-existing cycle among ancestors.
        }
        current = node.parent_id.as_deref();
    }
    chain.reverse();
    chain
}

/// Build a `ManifestV1` from the in-memory rows. Pure: no fs, no DB, no clock —
/// `exported_at` is passed in so the result is fully deterministic (and testable).
///
/// Jobs are ordered by `created_at` ascending, tie-broken by `id`, so the assigned
/// `exported_dir` ordinals are stable and the numbered prefix reflects creation order.
/// Lineage and pathway come ONLY from the persisted FKs; results come from the matching
/// `results` row (or null); `computed_identity` is always null.
pub fn build_manifest(
    group_meta: &GroupMeta,
    jobs: &[JobRow],
    results: &[ResultRow],
    group_tree: &[GroupNode],
    copy_mode: CopyMode,
    exported_at: String,
) -> ManifestV1 {
    let by_id: HashMap<&str, &GroupNode> =
        group_tree.iter().map(|n| (n.id.as_str(), n)).collect();
    let results_by_job: HashMap<&str, &ResultRow> =
        results.iter().map(|r| (r.job_id.as_str(), r)).collect();

    let mut ordered: Vec<&JobRow> = jobs.iter().collect();
    ordered.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    let group_size = ordered.len();

    let manifest_jobs = ordered
        .iter()
        .enumerate()
        .map(|(i, job)| {
            let exported_dir =
                format!("{}_{}", numbered_prefix(i, group_size), slugify(&job.title));
            manifest_job_entry(
                job,
                results_by_job.get(job.id.as_str()).copied(),
                exported_dir,
                group_path_for(job.group_id.as_deref(), &by_id),
                &copy_mode,
            )
        })
        .collect();

    ManifestV1 {
        manifest_version: 1,
        exported_at,
        copy_mode,
        source: ManifestSource {
            // A group export always names its root group — `Some(...)` serializes to the same
            // bare string as the pre-`Option` shape (byte-identical group manifest).
            group_id: Some(group_meta.id.clone()),
            group_name: Some(group_meta.name.clone()),
        },
        jobs: manifest_jobs,
        notes: vec![HONESTY_NOTE.to_string()],
    }
}

/// Build a `ManifestV1` for a **single** job — the single-job-export sibling of
/// [`build_manifest`] (same machinery, no group tree). Pure: no fs, no DB, no clock.
///
/// One `jobs` entry via the shared [`manifest_job_entry`]. Differences from the group form:
/// - `exported_dir = slugify(title)` with **no numeric prefix** (a lone job needs no ordinal);
/// - `source.group` is the job's group **if it has one, else null** — an ungrouped job carries
///   `ManifestSource { group_id: None, group_name: None }`, NEVER a fabricated group. The group
///   (id + name) is resolved by the caller and passed in (`None` = ungrouped);
/// - `group_path` is the single group's name when grouped, else empty.
pub fn build_single_job_manifest(
    job: &JobRow,
    result: Option<&ResultRow>,
    group: Option<&GroupMeta>,
    copy_mode: CopyMode,
    exported_at: String,
) -> ManifestV1 {
    let group_path = group.map(|g| vec![g.name.clone()]).unwrap_or_default();
    let entry = manifest_job_entry(job, result, slugify(&job.title), group_path, &copy_mode);
    ManifestV1 {
        manifest_version: 1,
        exported_at,
        copy_mode,
        source: ManifestSource {
            group_id: group.map(|g| g.id.clone()),
            group_name: group.map(|g| g.name.clone()),
        },
        jobs: vec![entry],
        notes: vec![HONESTY_NOTE.to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, title: &str, created_at: &str) -> JobRow {
        JobRow {
            id: id.to_string(),
            title: title.to_string(),
            job_type: None,
            status: "parsed".to_string(),
            created_at: created_at.to_string(),
            job_dir: Some(format!("/data/jobs/{id}")),
            group_id: None,
            pathway_id: None,
            source_ensemble_job_id: None,
            source_conformer_index: None,
            present_files: Vec::new(),
        }
    }

    fn group_meta() -> GroupMeta {
        GroupMeta {
            id: "g-root".to_string(),
            name: "HCN reduction".to_string(),
        }
    }

    // --- slugify --------------------------------------------------------------

    #[test]
    fn slugify_is_lowercase_ascii_and_collapses_whitespace() {
        assert_eq!(slugify("HCN Opt  Freq"), "hcn-opt-freq");
        assert_eq!(slugify("r2SCAN-3c"), "r2scan-3c");
    }

    /// BITE: a title used raw as a path leaf would escape. A `/`, a unicode arrow, and a
    /// `..` must all dissolve into separators — no path separator, no traversal, no
    /// leading `/` survives.
    #[test]
    fn slugify_cannot_escape_the_leaf() {
        let s = slugify("a/b→c");
        assert_eq!(s, "a-b-c");
        assert!(!s.contains('/'), "no path separator may survive");
        assert!(!s.contains(".."), "no parent traversal may survive");
        assert!(!s.starts_with('/'), "no absolute-path leak");

        // A pathological title made only of separators still yields a safe, non-empty leaf.
        let evil = slugify("../../etc/passwd");
        assert!(!evil.contains('/') && !evil.contains(".."), "got {evil:?}");
        assert_eq!(evil, "etc-passwd");

        // A title with NO alphanumerics falls back rather than producing an empty leaf.
        assert_eq!(slugify("/// : →"), "job");
    }

    // --- numbered_prefix ------------------------------------------------------

    #[test]
    fn numbered_prefix_pads_to_group_size_width() {
        // Single-digit group: width 1.
        assert_eq!(numbered_prefix(0, 2), "1");
        assert_eq!(numbered_prefix(1, 2), "2");
        // Ten jobs: width 2, 1-based, so lexical sort == creation order.
        assert_eq!(numbered_prefix(0, 10), "01");
        assert_eq!(numbered_prefix(9, 10), "10");
        // Hundred jobs: width 3.
        assert_eq!(numbered_prefix(0, 100), "001");
    }

    // --- curated_match --------------------------------------------------------

    #[test]
    fn curated_match_selects_artifacts_and_rejects_scratch() {
        // Selected: the scientific artifacts + input/geometry/marker.
        for f in [
            "input.inp",
            "output.out",
            "input.xyz",
            ".exit_code",
            "input.property.txt",
            "input_atom53.property.txt", // fragment output, via *.property.txt
            "input.hess",
            "input_trj.xyz",
            "input_MEP_trj.xyz",     // via *_trj.xyz
            "input_MEP_ALL_trj.xyz", // via *_trj.xyz
            "input.relaxscanact.dat",
            "input.relaxscanscf.dat",
            "input.001.xyz",
            "input.010.xyz",
            "input.NEB.log",
            "input.final.interp",
            "input_NEB-TS_converged.xyz",
            "input_NEB-CI_converged.xyz", // via *_converged.xyz — headline geometry, never omitted
        ] {
            assert!(curated_match(f), "{f} should be curated-included");
        }
        // Rejected: heavy/scratch/derivable — recorded in omitted, never copied.
        for f in [
            "input.gbw",
            "input.densities",
            "input.densitiesinfo",
            "orbital.mo7.g80.cube",
            "input.grid.tmp",
            "input.interp",  // only *.final.interp is pinned
            "input.allxyz", // not pinned
            "stderr.log",
        ] {
            assert!(!curated_match(f), "{f} should NOT be curated-included");
        }
    }

    /// `input.[0-9]*.xyz` must require the digit — `input.xyz` (no step number) is matched
    /// by the exact rule, NOT by the numbered-scan rule, and a would-be `input.foo.xyz`
    /// (no digit) is not curated.
    #[test]
    fn numbered_scan_rule_requires_a_digit() {
        assert!(curated_match("input.xyz")); // exact rule
        assert!(!curated_match("input.foo.xyz")); // no digit after `input.` → not a scan step
    }

    // --- build_manifest -------------------------------------------------------

    /// BITE (collision): two jobs both titled "Opt" must get two DISTINCT exported_dir —
    /// a constant prefix would collide and one would overwrite the other on disk.
    #[test]
    fn two_same_titled_jobs_get_distinct_dirs() {
        let jobs = vec![
            job("j1", "Opt", "2026-08-14T10:00:00"),
            job("j2", "Opt", "2026-08-14T11:00:00"),
        ];
        let m = build_manifest(&group_meta(), &jobs, &[], &[], CopyMode::Curated, "T".into());
        let dirs: Vec<&str> = m.jobs.iter().map(|j| j.exported_dir.as_str()).collect();
        assert_eq!(dirs, vec!["1_opt", "2_opt"]);
        assert_ne!(dirs[0], dirs[1], "same title must not collide");
    }

    /// BITE (silent omission): in curated mode a `.gbw` present on disk must appear in
    /// `files.omitted`, NOT `included` — and never simply vanish.
    #[test]
    fn curated_mode_records_omitted_gbw() {
        let mut j = job("j1", "Opt", "2026-08-14T10:00:00");
        j.present_files = vec![
            "input.property.txt".into(),
            "input.gbw".into(),
            "input.densities".into(),
        ];
        let m = build_manifest(&group_meta(), &[j], &[], &[], CopyMode::Curated, "T".into());
        let files = &m.jobs[0].files;
        assert!(files.included.contains(&"input.property.txt".to_string()));
        assert!(
            files.omitted.contains(&"input.gbw".to_string()),
            "gbw must be recorded as omitted, not dropped silently"
        );
        assert!(!files.included.contains(&"input.gbw".to_string()));
        // Full mode: the same file is included, omitted empty.
        let mut j2 = job("j1", "Opt", "2026-08-14T10:00:00");
        j2.present_files = vec!["input.gbw".into()];
        let full = build_manifest(&group_meta(), &[j2], &[], &[], CopyMode::Full, "T".into());
        assert!(full.jobs[0].files.included.contains(&"input.gbw".to_string()));
        assert!(full.jobs[0].files.omitted.is_empty());
    }

    /// build_manifest orders by created_at asc (tie-broken by id) and the prefix width
    /// matches the group size.
    #[test]
    fn build_manifest_orders_by_created_at_and_widths_prefix() {
        // Deliberately out of order; twelve jobs so width is 2.
        let mut jobs = Vec::new();
        for k in (0..12).rev() {
            let ts = format!("2026-08-14T{:02}:00:00", k);
            jobs.push(job(&format!("j{k}"), "step", &ts));
        }
        let m = build_manifest(&group_meta(), &jobs, &[], &[], CopyMode::Curated, "T".into());
        // First manifest job is the EARLIEST created_at, and prefixes are zero-padded to 2.
        assert_eq!(m.jobs[0].created_at, "2026-08-14T00:00:00");
        assert_eq!(m.jobs[0].exported_dir, "01_step");
        assert_eq!(m.jobs[11].exported_dir, "12_step");
        // Strictly ascending created_at across the projection.
        for w in m.jobs.windows(2) {
            assert!(w[0].created_at <= w[1].created_at);
        }
    }

    /// Honest-or-absent: a job with NO result row yields `results: None`, never a
    /// fabricated `0.0`. A job WITH a row carries the real values.
    #[test]
    fn missing_result_is_none_not_zero() {
        let jobs = vec![
            job("j1", "with-result", "2026-08-14T10:00:00"),
            job("j2", "no-result", "2026-08-14T11:00:00"),
        ];
        let results = vec![ResultRow {
            job_id: "j1".into(),
            energy_eh: Some(-93.42),
            imaginary_count: Some(0),
        }];
        let m = build_manifest(&group_meta(), &jobs, &results, &[], CopyMode::Curated, "T".into());
        let by_uuid: HashMap<&str, &ManifestJob> =
            m.jobs.iter().map(|j| (j.uuid.as_str(), j)).collect();
        let r = by_uuid["j1"].results.as_ref().expect("j1 has a result");
        assert_eq!(r.energy_eh, Some(-93.42));
        assert!(
            by_uuid["j2"].results.is_none(),
            "no result row → null results, NOT a fabricated 0.0"
        );
    }

    /// group_path is the persisted tree path (root → leaf); lineage/computed_identity come
    /// only from persisted FKs, and computed_identity is ALWAYS null in v1.
    #[test]
    fn manifest_encodes_only_persisted_structure() {
        let tree = vec![
            GroupNode { id: "g-root".into(), name: "HCN reduction".into(), parent_id: None },
            GroupNode { id: "g-sub".into(), name: "si-face".into(), parent_id: Some("g-root".into()) },
        ];
        let mut j = job("j1", "TS search", "2026-08-14T10:00:00");
        j.group_id = Some("g-sub".into());
        j.pathway_id = Some("pw-1".into());
        j.source_ensemble_job_id = Some("goat-7".into());
        j.source_conformer_index = Some(3);
        let m = build_manifest(&group_meta(), &[j], &[], &tree, CopyMode::Curated, "stamp".into());
        let mj = &m.jobs[0];
        assert_eq!(mj.group_path, vec!["HCN reduction", "si-face"]);
        assert_eq!(mj.pathway_id.as_deref(), Some("pw-1"));
        assert_eq!(mj.lineage.source_ensemble_job_id.as_deref(), Some("goat-7"));
        assert_eq!(mj.lineage.source_conformer_index, Some(3));
        assert!(mj.computed_identity.is_none(), "computed_identity is null in v1");
        assert_eq!(m.exported_at, "stamp");
        assert_eq!(m.manifest_version, 1);
        assert!(m.notes[0].contains("is NOT asserted here"));
    }

    // --- build_single_job_manifest (the single-job sibling) -------------------

    fn group_meta_named(id: &str, name: &str) -> GroupMeta {
        GroupMeta { id: id.to_string(), name: name.to_string() }
    }

    /// The single-job manifest: exactly one entry, `exported_dir = slug(title)` with NO
    /// numeric prefix, and the whole thing round-trips through JSON (the Part B post-condition
    /// re-reads it as a `ManifestV1`).
    #[test]
    fn single_job_manifest_round_trips() {
        let j = job("j1", "HCN Opt Freq", "2026-08-14T10:00:00");
        let g = group_meta_named("g-root", "HCN reduction");
        let m = build_single_job_manifest(&j, None, Some(&g), CopyMode::Curated, "stamp".into());

        assert_eq!(m.jobs.len(), 1, "a single-job manifest has exactly one entry");
        assert_eq!(m.jobs[0].uuid, "j1");
        assert_eq!(m.jobs[0].display_name, "HCN Opt Freq");
        // slug(title), NO numeric prefix (contrast the group form's "1_hcn-opt-freq").
        assert_eq!(m.jobs[0].exported_dir, "hcn-opt-freq");
        assert_eq!(m.source.group_name.as_deref(), Some("HCN reduction"));

        let json = serde_json::to_string_pretty(&m).unwrap();
        let back: ManifestV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(back.jobs.len(), 1);
        assert_eq!(back.jobs[0].uuid, "j1");
        assert_eq!(back.jobs[0].exported_dir, "hcn-opt-freq");
    }

    /// BITE (never fabricate a group): an UNGROUPED job → `source.group_id`/`group_name` are
    /// **null**, serialized as JSON null — not an invented placeholder group.
    #[test]
    fn ungrouped_job_source_group_is_null() {
        let j = job("j1", "scratch calc", "2026-08-14T10:00:00");
        let m = build_single_job_manifest(&j, None, None, CopyMode::Curated, "stamp".into());
        assert!(m.source.group_id.is_none(), "ungrouped → null group id, not a fabrication");
        assert!(m.source.group_name.is_none());
        assert!(m.jobs[0].group_path.is_empty(), "ungrouped → empty group path");

        let json = serde_json::to_string_pretty(&m).unwrap();
        assert!(json.contains("\"group_id\": null"), "null must be present in the JSON: {json}");
        assert!(json.contains("\"group_name\": null"));
    }

    /// BITE (silent omission — mirror the group-export honesty bite): curated mode records a
    /// present `.gbw` in `files.omitted`, never dropping it; Full includes it.
    #[test]
    fn single_job_curated_records_omitted() {
        let mut j = job("j1", "Opt", "2026-08-14T10:00:00");
        j.present_files = vec!["input.property.txt".into(), "input.gbw".into()];
        let m = build_single_job_manifest(&j, None, None, CopyMode::Curated, "T".into());
        let files = &m.jobs[0].files;
        assert!(files.included.contains(&"input.property.txt".to_string()));
        assert!(
            files.omitted.contains(&"input.gbw".to_string()),
            "curated: .gbw must be recorded as omitted, not dropped"
        );
        assert!(!files.included.contains(&"input.gbw".to_string()));

        let mut j2 = job("j1", "Opt", "2026-08-14T10:00:00");
        j2.present_files = vec!["input.gbw".into()];
        let full = build_single_job_manifest(&j2, None, None, CopyMode::Full, "T".into());
        assert!(full.jobs[0].files.included.contains(&"input.gbw".to_string()));
        assert!(full.jobs[0].files.omitted.is_empty());
    }

    /// The manifest round-trips through JSON and null fields serialize as JSON null
    /// (present, not omitted) — the Part B post-condition re-reads this file.
    #[test]
    fn manifest_serializes_nulls_present_and_roundtrips() {
        let jobs = vec![job("j1", "Opt", "2026-08-14T10:00:00")];
        let m = build_manifest(&group_meta(), &jobs, &[], &[], CopyMode::Curated, "T".into());
        let json = serde_json::to_string_pretty(&m).unwrap();
        assert!(json.contains("\"computed_identity\": null"));
        assert!(json.contains("\"results\": null"));
        assert!(json.contains("\"copy_mode\": \"curated\""));
        let back: ManifestV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(back.jobs.len(), 1);
        assert_eq!(back.jobs[0].uuid, "j1");
        assert_eq!(back.copy_mode, CopyMode::Curated);
    }
}
