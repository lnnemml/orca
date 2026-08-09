//! Job commands: the CRUD + state-machine surface over the `jobs` table.
//!
//! The Tauri commands are thin wrappers that lock the shared connection and
//! delegate to the `*_conn` helpers, which take a `&Connection` directly so the
//! state-machine logic is unit-testable without a running Tauri app.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::{Manager, State};
use uuid::Uuid;

use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::models::job::{Job, JobStatus};

// --- Connection-level helpers (testable) ------------------------------------

/// Insert a fresh `draft` job and return it fully hydrated. `scene_json` is the
/// optional SceneFragment snapshot (ADR-008 #5), written once here at create
/// time and never updated — the job's input is immutable, so its snapshot is too.
///
/// The authoritative act of unit 1e (ADR-016 amendment): mint the job's
/// `IndexMap<OrcaIndex>` **from the submitted `input_content`, verified against the
/// scene**, and store it in `jobs.index_map_json`. On any text↔scene mismatch or an
/// input form we cannot map, a self-describing skip (`{"skipped": …}`) is stored
/// instead — the job is NOT blocked, and parse falls back to the derived identity map.
/// This is the single mint site, so a clone / "new iteration" (which also calls
/// `create_job`) mints its OWN map from its OWN text — never inherited.
fn create_job_conn(
    conn: &Connection,
    title: &str,
    input_content: &str,
    scene_json: Option<&str>,
    scene_log_json: Option<&str>,
) -> Result<Job, AppError> {
    let id = Uuid::new_v4().to_string();
    let index_map_json = serde_json::to_string(&crate::results::mint_stored_index_map(
        input_content,
        scene_json,
    ))
    .map_err(|e| AppError::Internal(format!("serialize index_map_json: {e}")))?;
    // scene_json (the map-minting contract, unit 1e) and scene_log_json (the
    // history, unit 2b) are written by the SAME INSERT — atomic by construction,
    // so a later restore can cross-check the two against each other.
    conn.execute(
        "INSERT INTO jobs (id, title, input_content, status, scene_json, index_map_json, scene_log_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            title,
            input_content,
            JobStatus::Draft.as_str(),
            scene_json,
            index_map_json,
            scene_log_json
        ],
    )?;
    get_job_conn(conn, &id)
}

/// Create a DFT re-opt CHILD job (Phase 4.5 Stage D unit D2a) and tag it back to
/// the GOAT ensemble job + conformer it came from (`jobs.source_ensemble_job_id` /
/// `source_conformer_index`, migration v15). Reuses [`create_job_conn`] for the
/// insert (so the child mints its OWN `index_map` from its OWN input, like any job)
/// then stamps the two linkage FKs. No scene snapshot: the child is a normal DFT
/// job whose input fully defines it (ADR-008 #5), and its "New iteration" restore
/// uses the input-parse branch — it does not need the ensemble's fragment identity.
///
/// The (charge, multiplicity) post-condition — the anion footgun — is enforced in
/// the TS `buildReoptInput` at the SINGLE construction point (it throws before this
/// command is ever called, so a wrong-charge child input never reaches the create
/// boundary). What this Rust boundary adds is referential integrity: the caller
/// (`create_reopt_job`) refuses when the source ensemble job is gone.
fn create_reopt_job_conn(
    conn: &Connection,
    title: &str,
    input_content: &str,
    source_ensemble_job_id: &str,
    source_conformer_index: i64,
) -> Result<Job, AppError> {
    let job = create_job_conn(conn, title, input_content, None, None)?;
    conn.execute(
        "UPDATE jobs SET source_ensemble_job_id = ?1, source_conformer_index = ?2 WHERE id = ?3",
        params![source_ensemble_job_id, source_conformer_index, job.id],
    )?;
    get_job_conn(conn, &job.id)
}

/// Create an OptTS-refinement CHILD job (Phase 4.5 Stage E1a, ADR-020) from a TS-guess
/// geometry. **SOURCE-AGNOSTIC:** `source_job_id` is ANY job — a relaxed scan (today's caller,
/// the scan maximum) or a NEB climbing image (Stage E3). The child is a normal `draft` (the
/// caller `submit_job`s it into the sequential queue), minting its OWN `index_map` from its OWN
/// `! OptTS …` input like any job.
///
/// **No lineage column** (unlike the re-opt fan-out): the TS↔source relationship is expressed by
/// the **shared pathway** + the **`! OptTS` role** derived from the child's own input — nothing
/// stored. If the source is attached to a pathway, the refined TS **joins that same pathway**
/// (reusing [`attach_job_to_pathway_conn`] — one attach mechanism, no second path), so the
/// reaction view groups the guess and its TS together.
///
/// The (charge, multiplicity) and no-Scan/opt-leak post-conditions are enforced in the TS
/// `buildOptTSInput` at the single construction point (it throws before this command is called),
/// so a wrong-charge or Scan-leaking input never reaches this boundary. What this Rust boundary
/// adds is referential integrity: it refuses (`NotFound`) when the source job is gone.
fn create_optts_job_conn(
    conn: &Connection,
    title: &str,
    input_content: &str,
    source_job_id: &str,
) -> Result<Job, AppError> {
    // The source must exist (a clean NotFound instead of a dangling reference).
    let source = get_job_conn(conn, source_job_id)?;
    let child = create_job_conn(conn, title, input_content, None, None)?;
    // The refined TS joins the SOURCE's pathway, if any — the same attach used everywhere.
    if let Some(pathway_id) = source.pathway_id {
        crate::commands::reactions::attach_job_to_pathway_conn(conn, &child.id, &pathway_id)?;
    }
    get_job_conn(conn, &child.id)
}

/// The element symbols of an ORCA input's inline `* xyz <c> <m> … *` block, in
/// order (`["C","C","H",…]`). Empty for an input with no inline block (e.g.
/// `* xyzfile …`). Used ONLY for the D2b element-list post-condition (rule #9):
/// a re-opt child must share the source ensemble's composition/order, or we refuse
/// to rank across mismatched atoms. Deliberately narrow — first whitespace token of
/// each row inside the block — not a coordinate parser.
fn element_symbols_from_input(input: &str) -> Vec<String> {
    let mut in_block = false;
    let mut elements = Vec::new();
    for line in input.lines() {
        let t = line.trim();
        if !in_block {
            // Opening marker: `* xyz …` (but not `* xyzfile …`, external geometry).
            if let Some(rest) = t.strip_prefix('*') {
                let kw = rest.trim().to_ascii_lowercase();
                if kw.starts_with("xyz") && !kw.starts_with("xyzfile") {
                    in_block = true;
                }
            }
            continue;
        }
        // Inside the block: a lone `*` closes it; otherwise the first token is the element.
        if t.starts_with('*') {
            break;
        }
        if let Some(sym) = t.split_whitespace().next() {
            elements.push(sym.to_string());
        }
    }
    elements
}

/// Whether an ORCA input requested a frequency calculation — a `Freq` (or `NumFreq`)
/// token on a `!` keyword line, case-insensitive, ignoring comments. Drives D2b's
/// mode detection (ΔG-mode iff every child requested Freq); NOT stored, derived from
/// the child's own input text (the "mode lives in the child input" decision, D2a).
fn input_requested_freq(input: &str) -> bool {
    input.lines().any(|line| {
        let code = line.split('#').next().unwrap_or("").trim();
        if !code.starts_with('!') {
            return false;
        }
        code.split_whitespace()
            .any(|tok| tok.eq_ignore_ascii_case("freq") || tok.eq_ignore_ascii_case("numfreq"))
    })
}

/// The method keyword of a re-opt child: the FIRST token on the `!` keyword line
/// (ignoring comments). `buildReoptInput` always emits `! <method> Opt [Freq] …`, so
/// the leading token is the composite/functional (`r2SCAN-3c`, …). Used only to label
/// the carried geometry's level in the UI (D3); `None` if there is no `!` line.
fn method_from_input(input: &str) -> Option<String> {
    input.lines().find_map(|line| {
        let code = line.split('#').next().unwrap_or("").trim();
        let rest = code.strip_prefix('!')?;
        rest.split_whitespace().next().map(str::to_string)
    })
}

/// One DFT re-opt child in a GOAT job's fan-out, as read back for the D2b aggregate.
/// Raw facts only — the TS side does the honest-or-absent weighting (one Boltzmann
/// implementation lives in `src/scene/ensemble.ts`, never a second one in Rust).
#[derive(Debug, Serialize)]
pub struct ReoptChild {
    /// 0-based conformer index in the source GOAT ensemble.
    pub source_conformer_index: i64,
    pub job_id: String,
    pub title: String,
    /// Job status string (`queued`/`running`/`completed`/`parsed`/`failed`/…).
    pub status: String,
    /// DFT electronic energy (Eh), `None` until the job's results are parsed.
    pub electronic_energy_eh: Option<f64>,
    /// Thermochemistry Gibbs free energy G (Eh), `None` unless Freq ran + parsed.
    pub gibbs_eh: Option<f64>,
    /// Imaginary-frequency count (Freq only). `Some(n>0)` ⇒ a saddle, not a minimum.
    pub imaginary_count: Option<i64>,
    /// This child's input requested `Freq` (mode detection is derived, not stored).
    pub freq_requested: bool,
    /// The method keyword (first `!`-line token), to label the carried geometry (D3).
    pub method: Option<String>,
    /// The child's `* xyz` element list differs from the source ensemble's — a
    /// composition mismatch (should be impossible by construction; surfaced loudly).
    pub element_mismatch: bool,
}

/// The DFT re-opt fan-out of ONE GOAT job, read back (Phase 4.5 Stage D unit D2b).
/// The set is DERIVED by `source_ensemble_job_id` (Fork 1, no table); the aggregate
/// is computed at read-time and never stored.
#[derive(Debug, Serialize)]
pub struct ReoptAggregate {
    pub source_job_id: String,
    pub children: Vec<ReoptChild>,
    /// Children whose input requested Freq (for TS mode detection: ΔG-mode iff all).
    pub freq_requested_count: usize,
    /// Some children requested Freq and some did not — a mixed set (D2a shouldn't
    /// produce this; the TS side refuses to pick a single mode when true).
    pub mode_inconsistent: bool,
}

/// Read a GOAT job's DFT re-opt children and their DFT energies (Phase 4.5 D2b).
/// Groups by `source_ensemble_job_id` (the derived set), LEFT JOINs `results` so a
/// child that hasn't parsed yet still appears (with `None` energies — never dropped,
/// rule #9 honest-or-absent). Returns raw per-child facts; the TS side re-ranks and
/// re-weights (reusing `boltzmannWeights`). Element-list post-condition: each child's
/// `* xyz` composition is compared to the source ensemble job's and a mismatch is
/// flagged (`element_mismatch`), never silently ranked across.
fn read_conformer_reoptimization_conn(
    conn: &Connection,
    source_job_id: &str,
) -> Result<ReoptAggregate, AppError> {
    // The source ensemble job's composition (its `* xyz` element order) is the
    // reference every child must match. NotFound if the source is gone.
    let source = get_job_conn(conn, source_job_id)?;
    let source_elements = element_symbols_from_input(&source.input_content);

    let mut stmt = conn.prepare(
        "SELECT j.id, j.title, j.status, j.source_conformer_index, j.input_content, \
                r.final_energy_eh, r.free_energy_g_eh, r.imaginary_count \
         FROM jobs j LEFT JOIN results r ON r.job_id = j.id \
         WHERE j.source_ensemble_job_id = ?1 \
         ORDER BY j.source_conformer_index",
    )?;
    let rows = stmt.query_map(params![source_job_id], |r| {
        let input_content: String = r.get(4)?;
        Ok((
            r.get::<_, i64>(3)?,             // source_conformer_index
            r.get::<_, String>(0)?,          // id
            r.get::<_, String>(1)?,          // title
            r.get::<_, String>(2)?,          // status
            r.get::<_, Option<f64>>(5)?,     // final_energy_eh
            r.get::<_, Option<f64>>(6)?,     // free_energy_g_eh
            r.get::<_, Option<i64>>(7)?,     // imaginary_count
            input_content,
        ))
    })?;

    let mut children = Vec::new();
    let mut freq_requested_count = 0usize;
    let mut any_freq = false;
    let mut any_no_freq = false;
    for row in rows {
        let (idx, job_id, title, status, e_elec, g, imag, input_content) = row?;
        let freq_requested = input_requested_freq(&input_content);
        if freq_requested {
            freq_requested_count += 1;
            any_freq = true;
        } else {
            any_no_freq = true;
        }
        let element_mismatch = element_symbols_from_input(&input_content) != source_elements;
        let method = method_from_input(&input_content);
        children.push(ReoptChild {
            source_conformer_index: idx,
            job_id,
            title,
            status,
            electronic_energy_eh: e_elec,
            gibbs_eh: g,
            imaginary_count: imag,
            freq_requested,
            method,
            element_mismatch,
        });
    }

    Ok(ReoptAggregate {
        source_job_id: source_job_id.to_string(),
        children,
        freq_requested_count,
        mode_inconsistent: any_freq && any_no_freq,
    })
}

/// All jobs, newest first.
fn list_jobs_conn(conn: &Connection) -> Result<Vec<Job>, AppError> {
    let sql = format!("SELECT {} FROM jobs ORDER BY created_at DESC", Job::COLUMNS);
    let mut stmt = conn.prepare(&sql)?;
    let jobs = stmt
        .query_map([], Job::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(jobs)
}

/// A single job by id, or [`AppError::NotFound`].
pub(crate) fn get_job_conn(conn: &Connection, id: &str) -> Result<Job, AppError> {
    let sql = format!("SELECT {} FROM jobs WHERE id = ?1", Job::COLUMNS);
    conn.query_row(&sql, params![id], Job::from_row)
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("job {id}")))
}

/// Delete a job from the database and return its isolated `job_dir` (or `None`)
/// for the caller to remove **after** the transaction commits (Phase 4.7.1).
///
/// Terminal-states-only: refuses a `Running`/`Queued` job ("cancel it first").
/// Terminating a live run is [`crate::local_backend::cancel`]'s job, never
/// delete's — there is no killpg in this path.
///
/// The FK cleanup runs in ONE transaction, in this exact order so any failure
/// rolls back whole. FK enforcement is **ON** in this build
/// (`SQLITE_DEFAULT_FOREIGN_KEYS=1`, measured — see the v14 note in `db.rs`), so
/// this cleanup is **required**, not hygiene: a raw `DELETE FROM jobs` would trip
/// a RESTRICT FK from a re-opt child or a reference row (the negative-control test
/// proves it).
///   1. NULL the re-opt link on any child that pointed at this job as its GOAT
///      source (`source_ensemble_job_id` / `source_conformer_index`) — the child
///      survives as a standalone job (jobs-survive).
///   2. DELETE this job's reaction-reference rows — the reaction survives, it just
///      loses this one reference (jobs-survive, the reaction side).
///   3. DELETE the job — this **cascades** to its `results` row
///      (`results.job_id … ON DELETE CASCADE`); never delete the results row by
///      hand, or the test can't tell the cascade fired.
/// `jobs.pathway_id` points OUT from this row and vanishes with it — no orphan,
/// no action.
pub(crate) fn delete_job_conn(conn: &Connection, id: &str) -> Result<Option<String>, AppError> {
    // Not-found first: nothing is touched if the id is absent (mirrors
    // delete_reaction_conn).
    let job = get_job_conn(conn, id)?;

    // Terminal-states-only guard: refuse a live job; cancel it first.
    if matches!(job.status, JobStatus::Running | JobStatus::Queued) {
        return Err(AppError::Backend(format!(
            "job is running or queued — cancel it first (status: {})",
            job.status.as_str()
        )));
    }

    // One transaction (`unchecked_transaction` borrows the shared `&Connection`
    // held behind the DbState mutex — the caller owns the only handle). A failure
    // at any step rolls the whole delete back.
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE jobs SET source_ensemble_job_id = NULL, source_conformer_index = NULL \
         WHERE source_ensemble_job_id = ?1",
        params![id],
    )?;
    tx.execute(
        "DELETE FROM reaction_reference_jobs WHERE job_id = ?1",
        params![id],
    )?;
    tx.execute("DELETE FROM jobs WHERE id = ?1", params![id])?;
    tx.commit()?;

    Ok(job.job_dir)
}

/// Record the isolated job directory for a job (set once at submit time).
pub(crate) fn set_job_dir_conn(conn: &Connection, id: &str, job_dir: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE jobs SET job_dir = ?1 WHERE id = ?2",
        params![job_dir, id],
    )?;
    Ok(())
}

/// Store extracted results (final SCF energy in Hartree, wall time in seconds).
/// Either may be `None` if extraction didn't find it.
pub(crate) fn set_job_results_conn(
    conn: &Connection,
    id: &str,
    energy: Option<f64>,
    wall_time: Option<f64>,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE jobs SET energy = ?1, wall_time = ?2 WHERE id = ?3",
        params![energy, wall_time, id],
    )?;
    Ok(())
}

/// Overwrite a job's energy from the **authoritative** parsed result (ADR-012),
/// leaving `wall_time` untouched. Called after a successful parse: the output.out
/// tail regex in [`set_job_results_conn`] is only a **live estimate** during a
/// run and misses the final energy on a large molecule (the last
/// `FINAL SINGLE POINT ENERGY` sits past the tail window — measured 164 KB on a
/// 33-atom Freq, unit 3.9 defect 2). `results.final_energy_eh`, read from
/// `.property.txt`, is the value the header/list must show.
pub(crate) fn set_job_energy_conn(conn: &Connection, id: &str, energy: f64) -> Result<(), AppError> {
    conn.execute(
        "UPDATE jobs SET energy = ?1 WHERE id = ?2",
        params![energy, id],
    )?;
    Ok(())
}

/// Terminal transition (`completed`/`failed`): set status, stamp `completed_at`,
/// and store an optional `error_message`. Used by the LocalBackend when a run
/// finishes.
pub(crate) fn finalize_job_conn(
    conn: &Connection,
    id: &str,
    status: JobStatus,
    error_message: Option<&str>,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE jobs SET status = ?1, completed_at = datetime('now'), error_message = ?2 \
         WHERE id = ?3",
        params![status.as_str(), error_message, id],
    )?;
    Ok(())
}

/// Record a **results parse** failure on a job that otherwise completed fine. The
/// status stays `completed` (the calculation ran; only our parse of it did not) —
/// this only surfaces the reason in the UI. Distinct from a `failed` calculation.
pub(crate) fn set_job_parse_error_conn(
    conn: &Connection,
    id: &str,
    message: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE jobs SET error_message = ?1 WHERE id = ?2",
        params![message, id],
    )?;
    Ok(())
}

/// Transition a job to `status`, stamping the matching timestamp:
/// `started_at` on entering `running`, `completed_at` on `completed`/`failed`.
pub(crate) fn update_job_status_conn(conn: &Connection, id: &str, status: &str) -> Result<(), AppError> {
    let status = JobStatus::from_db(status)?;
    let affected = match status {
        JobStatus::Running => conn.execute(
            "UPDATE jobs SET status = ?1, started_at = datetime('now') WHERE id = ?2",
            params![status.as_str(), id],
        )?,
        JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled => conn.execute(
            "UPDATE jobs SET status = ?1, completed_at = datetime('now') WHERE id = ?2",
            params![status.as_str(), id],
        )?,
        // `parsed` is post-`completed`; completed_at is already stamped, so only
        // the status advances.
        JobStatus::Parsed => conn.execute(
            "UPDATE jobs SET status = ?1 WHERE id = ?2",
            params![status.as_str(), id],
        )?,
        JobStatus::Draft | JobStatus::Queued => conn.execute(
            "UPDATE jobs SET status = ?1 WHERE id = ?2",
            params![status.as_str(), id],
        )?,
    };
    if affected == 0 {
        return Err(AppError::NotFound(format!("job {id}")));
    }
    Ok(())
}

// --- Tauri commands ---------------------------------------------------------

#[tauri::command]
pub fn create_job(
    db: State<'_, DbState>,
    title: String,
    input_content: String,
    scene_json: Option<String>,
    scene_log_json: Option<String>,
) -> Result<Job, AppError> {
    let conn = db.lock()?;
    create_job_conn(
        &conn,
        &title,
        &input_content,
        scene_json.as_deref(),
        scene_log_json.as_deref(),
    )
}

#[tauri::command]
pub fn list_jobs(db: State<'_, DbState>) -> Result<Vec<Job>, AppError> {
    let conn = db.lock()?;
    list_jobs_conn(&conn)
}

/// Create ONE DFT re-opt child of a GOAT conformer, tagged back to its source
/// ensemble job + conformer index (Phase 4.5 Stage D unit D2a). The child is
/// created as a `draft`; the caller then `submit_job`s it into the sequential
/// queue (concurrency 1). `input_content` is built + charge-checked by the TS
/// `buildReoptInput` before this call. Create-boundary post-condition (rule #9):
/// the source ensemble job must still exist — otherwise `NotFound`, and no child
/// is created (referential integrity; the jobs-survive rule works the other way —
/// deleting the source would null this link, never cascade).
#[tauri::command]
pub fn create_reopt_job(
    db: State<'_, DbState>,
    source_ensemble_job_id: String,
    source_conformer_index: i64,
    title: String,
    input_content: String,
) -> Result<Job, AppError> {
    let conn = db.lock()?;
    // The source must exist (a clean NotFound instead of a dangling FK).
    get_job_conn(&conn, &source_ensemble_job_id)?;
    create_reopt_job_conn(
        &conn,
        &title,
        &input_content,
        &source_ensemble_job_id,
        source_conformer_index,
    )
}

/// Create ONE OptTS-refinement child of a TS guess (Phase 4.5 Stage E1a, ADR-020). The child is
/// created as a `draft`; the caller then `submit_job`s it into the sequential queue.
/// `input_content` is built + charge/Scan-checked by the TS `buildOptTSInput` before this call.
/// **Source-agnostic:** `source_job_id` is any job (the scan is today's caller, NEB tomorrow).
/// Create-boundary post-condition (rule #9): the source job must still exist — otherwise
/// `NotFound`, and no child is created. If the source is on a pathway, the TS joins it.
#[tauri::command]
pub fn create_optts_job(
    db: State<'_, DbState>,
    source_job_id: String,
    title: String,
    input_content: String,
) -> Result<Job, AppError> {
    let conn = db.lock()?;
    create_optts_job_conn(&conn, &title, &input_content, &source_job_id)
}

/// Read a GOAT job's DFT re-opt fan-out for the D2b aggregate view: the derived set
/// of children (by `source_ensemble_job_id`), each with its status + parsed DFT
/// electronic energy / Gibbs G / imaginary count. Raw facts — the TS side re-ranks
/// and re-weights (reusing `boltzmannWeights`), applies honest-or-absent, and labels
/// ΔG vs ΔE. Read-time only; nothing is stored.
#[tauri::command]
pub fn read_conformer_reoptimization(
    db: State<'_, DbState>,
    source_job_id: String,
) -> Result<ReoptAggregate, AppError> {
    let conn = db.lock()?;
    read_conformer_reoptimization_conn(&conn, &source_job_id)
}

#[tauri::command]
pub fn get_job(db: State<'_, DbState>, id: String) -> Result<Job, AppError> {
    let conn = db.lock()?;
    get_job_conn(&conn, &id)
}

/// The parsed `.property.txt` results for a job (the full structure, incl. per-atom
/// arrays with their element order), or `None` if the job has none yet.
#[tauri::command]
pub fn read_job_results(
    db: State<'_, DbState>,
    id: String,
) -> Result<Option<crate::results::ParsedResults>, AppError> {
    let conn = db.lock()?;
    crate::results::read_job_results(&conn, &id)
}

/// The per-point geometries of a relaxed scan (`input.NNN.xyz`), in point order,
/// for the profile viewer's click-to-view (Phase 4.5 B2). `None` for a non-scan job.
/// Reads the point files whole (small, rule #5); writes nothing (rule #3).
#[tauri::command]
pub fn read_scan_geometries(
    db: State<'_, DbState>,
    id: String,
) -> Result<Option<Vec<crate::results::ScanGeometry>>, AppError> {
    let conn = db.lock()?;
    let job_dir = get_job_conn(&conn, &id)?.job_dir;
    crate::results::read_scan_geometries(&conn, &id, job_dir.as_deref())
}

#[tauri::command]
pub fn update_job_status(db: State<'_, DbState>, id: String, status: String) -> Result<(), AppError> {
    let conn = db.lock()?;
    update_job_status_conn(&conn, &id, &status)
}

/// Submit a draft job to the LocalBackend: prepare its dir, spawn ORCA, and
/// stream the log. Returns immediately — the run proceeds on a background
/// thread. See [`crate::local_backend`].
#[tauri::command]
pub fn submit_job(app: tauri::AppHandle, id: String) -> Result<(), AppError> {
    crate::local_backend::submit(&app, &id)
}

/// Cancel a running or queued job (see [`crate::local_backend::cancel`]).
#[tauri::command]
pub fn cancel_job(app: tauri::AppHandle, id: String) -> Result<(), AppError> {
    crate::local_backend::cancel(&app, &id)
}

/// Delete a job (Phase 4.7.1): its DB row (with FK cleanup; `results` cascade) and,
/// **guarded**, its isolated `job_dir`. Terminal-states-only — a running/queued job
/// is refused (cancel it first). The DB row is deleted first (under the lock); only
/// then, and only if a `job_dir` was recorded, is the directory removed — and that
/// removal is itself guarded to `data_dir/jobs/` (see
/// [`crate::local_backend::remove_job_dir`]).
#[tauri::command]
pub fn delete_job(app: tauri::AppHandle, id: String) -> Result<(), AppError> {
    let db = app.state::<DbState>();
    let job_dir = {
        let conn = db.lock()?;
        delete_job_conn(&conn, &id)?
    };
    if let Some(dir) = job_dir {
        crate::local_backend::remove_job_dir(&app, &dir);
    }
    Ok(())
}

/// Pause the sequential queue: the running job finishes, but no queued job
/// starts until [`resume_queue`].
#[tauri::command]
pub fn pause_queue(app: tauri::AppHandle) -> Result<(), AppError> {
    crate::local_backend::set_paused(&app, true);
    Ok(())
}

/// Resume the queue and immediately pull the next queued job if the slot is free.
#[tauri::command]
pub fn resume_queue(app: tauri::AppHandle) -> Result<(), AppError> {
    crate::local_backend::set_paused(&app, false);
    Ok(())
}

/// Whether the queue is currently paused.
#[tauri::command]
pub fn is_queue_paused(app: tauri::AppHandle) -> Result<bool, AppError> {
    Ok(crate::local_backend::is_paused(&app))
}

/// Max lines returned by [`read_job_output`] (also the default when `tail_lines`
/// is omitted). Bounds both the read and the payload for Phase 1.
const OUTPUT_LINE_CAP: usize = 10_000;

/// Read a job's `output.out` for the log console. Returns the last `tail_lines`
/// lines (default/cap [`OUTPUT_LINE_CAP`]). Returns an empty vec — not an error —
/// when the job has no directory yet or hasn't produced output.
#[tauri::command]
pub fn read_job_output(
    db: State<'_, DbState>,
    id: String,
    tail_lines: Option<usize>,
) -> Result<Vec<String>, AppError> {
    let job_dir = {
        let conn = db.lock()?;
        get_job_conn(&conn, &id)?.job_dir
    };
    let Some(job_dir) = job_dir else {
        return Ok(Vec::new());
    };
    let out_path = std::path::Path::new(&job_dir).join("output.out");
    if !out_path.exists() {
        return Ok(Vec::new());
    }
    let max_lines = tail_lines.map_or(OUTPUT_LINE_CAP, |n| n.min(OUTPUT_LINE_CAP));
    Ok(crate::local_backend::read_tail_lines(&out_path, max_lines)?)
}

/// Max size of a GOAT ensemble file we'll read whole. The ensemble is a
/// multi-frame xyz of ONE (small) fragment — kilobytes in practice — so reading
/// it fully is fine (unlike `output.out`, domain rule #5). Cap defensively so a
/// pathological file can't blow up the IPC payload.
const MAX_ENSEMBLE_BYTES: u64 = 8 * 1024 * 1024;

/// Read a GOAT job's conformer ensemble (`input.finalensemble.xyz`, the file
/// name GOAT derives from `input.inp` — see `wiki/orca/goat.md`). Returns an
/// empty string (not an error) when the job has no dir, hasn't produced the file,
/// or it isn't a GOAT run. Read lazily by `JobDetailScreen` on a completed job.
#[tauri::command]
pub fn read_job_ensemble(db: State<'_, DbState>, id: String) -> Result<String, AppError> {
    let job_dir = {
        let conn = db.lock()?;
        get_job_conn(&conn, &id)?.job_dir
    };
    let Some(job_dir) = job_dir else {
        return Ok(String::new());
    };
    let path = std::path::Path::new(&job_dir).join("input.finalensemble.xyz");
    if !path.exists() {
        return Ok(String::new());
    }
    if std::fs::metadata(&path)?.len() > MAX_ENSEMBLE_BYTES {
        return Err(AppError::Internal(format!(
            "ensemble file for job {id} is unexpectedly large (> {MAX_ENSEMBLE_BYTES} bytes)"
        )));
    }
    Ok(std::fs::read_to_string(&path)?)
}

/// Max lines handed to the Monaco output viewer. An ORCA output can reach
/// hundreds of MB; neither the IPC payload nor the editor model should carry
/// that. ~300k lines ≈ 30 MB — a comfortable ceiling for the viewer.
const MAX_VIEWER_LINES: usize = 300_000;

/// Full output for the Monaco-based viewer. Capped by line count: when capped we
/// keep the **tail** (that's where the interesting end of a run is) and report
/// `first_line_no` so the viewer can display absolute file line numbers and map
/// search hits correctly.
#[derive(Serialize)]
pub struct OutputContent {
    pub content: String,
    /// 1-indexed file line number of the first line in `content`.
    /// `> 1` exactly when `truncated`.
    pub first_line_no: usize,
    pub total_lines: usize,
    pub truncated: bool,
}

/// Read a job's `output.out` for the Monaco viewer, capped to the last
/// [`MAX_VIEWER_LINES`] lines. Streams the file line by line (never loads a
/// hundreds-of-MB file whole — domain rule #5), keeping only the tail window in
/// memory. Empty content (not an error) when there's no dir or output yet.
#[tauri::command]
pub fn read_job_output_for_viewer(
    db: State<'_, DbState>,
    id: String,
) -> Result<OutputContent, AppError> {
    let empty = OutputContent {
        content: String::new(),
        first_line_no: 1,
        total_lines: 0,
        truncated: false,
    };

    let job_dir = {
        let conn = db.lock()?;
        get_job_conn(&conn, &id)?.job_dir
    };
    let Some(job_dir) = job_dir else {
        return Ok(empty);
    };
    let out_path = std::path::Path::new(&job_dir).join("output.out");
    if !out_path.exists() {
        return Ok(empty);
    }

    let reader = BufReader::new(std::fs::File::open(&out_path)?);
    let mut kept: VecDeque<String> = VecDeque::new();
    let mut total_lines = 0usize;
    for line in reader.lines() {
        let line = line?;
        total_lines += 1;
        if kept.len() == MAX_VIEWER_LINES {
            kept.pop_front();
        }
        kept.push_back(line);
    }

    let first_line_no = total_lines.saturating_sub(kept.len()) + 1;
    let truncated = total_lines > kept.len();
    let content = kept.into_iter().collect::<Vec<_>>().join("\n");
    Ok(OutputContent {
        content,
        first_line_no,
        total_lines,
        truncated,
    })
}

/// Max orbital-cube size we hand to the webview. 3Dmol's `VolumeData` parses the WHOLE
/// cube string (an isosurface needs the whole grid — rule #5's stream/tail is impossible
/// here), so the cube IS loaded whole; this cap bounds the worst case. Measured
/// (`orca-plot.md`): 80³ = 6.9 MB, 100³ = 13.5 MB, a 60-atom @120³ ≈ 24 MB — all under
/// 32 MB. A larger request is refused with a "lower the grid" message, not read.
const CUBE_READ_CAP: u64 = 32 * 1024 * 1024;

/// Generate (lazily, cached) and read the `.cube` for one molecular orbital, for the
/// orbital-isosurface viewer (unit 3.15). Returns the cube text, or `None` when the job
/// has no `.gbw` / no `orca_plot` / it produced nothing (xTB/GOAT — a normal state, the
/// section is simply hidden). Errors only on an over-cap cube or an IO failure.
///
/// The cube is a disk artifact in the job dir (`orca_plot.rs`); it is **never** stored in
/// the DB — only its text crosses to the viewer, once, capped.
#[tauri::command]
pub fn read_orbital_cube(
    db: State<'_, DbState>,
    id: String,
    mo_index: u32,
    grid: u32,
) -> Result<Option<String>, AppError> {
    let (job_dir, orca_path) = {
        let conn = db.lock()?;
        let job_dir = get_job_conn(&conn, &id)?.job_dir;
        let orca_path = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'orca_path'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .filter(|p| !p.trim().is_empty());
        (job_dir, orca_path)
    };
    let (Some(job_dir), Some(orca_path)) = (job_dir, orca_path) else {
        return Ok(None);
    };

    let cube_path = crate::orca_plot::ensure_mo_cube(
        &orca_path,
        std::path::Path::new(&job_dir),
        mo_index,
        grid,
    )?;
    let Some(cube_path) = cube_path else {
        return Ok(None);
    };

    let bytes = std::fs::metadata(&cube_path)?.len();
    if bytes > CUBE_READ_CAP {
        return Err(AppError::Internal(format!(
            "orbital cube is {} MB (> {} MB cap) — lower the grid resolution",
            bytes / (1024 * 1024),
            CUBE_READ_CAP / (1024 * 1024),
        )));
    }
    Ok(Some(std::fs::read_to_string(&cube_path)?))
}

/// Backfill the convergence dashboard: replay a job's `output.out` through the
/// incremental parser and return every SCF / optimization datapoint. Returns an
/// empty vec — not an error — when the job has no directory or output yet.
/// Streams the file line by line (never loads it whole — domain rule #5).
#[tauri::command]
pub fn read_job_convergence(
    db: State<'_, DbState>,
    id: String,
) -> Result<Vec<crate::convergence::ConvergenceEvent>, AppError> {
    let job_dir = {
        let conn = db.lock()?;
        get_job_conn(&conn, &id)?.job_dir
    };
    let Some(job_dir) = job_dir else {
        return Ok(Vec::new());
    };
    let out_path = std::path::Path::new(&job_dir).join("output.out");
    if !out_path.exists() {
        return Ok(Vec::new());
    }
    Ok(crate::local_backend::read_convergence(&out_path)?)
}

/// Open a job's directory in the OS file manager.
#[tauri::command]
pub fn open_job_folder(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let job_dir = {
        let conn = db.lock()?;
        get_job_conn(&conn, &id)?.job_dir
    };
    let job_dir = job_dir.ok_or_else(|| AppError::Backend("job has no directory yet".into()))?;
    open_in_file_manager(&job_dir)
}

/// Spawn the platform file manager on `path` (Linux-first; also handles macOS /
/// Windows). Detached — we don't wait on the viewer.
fn open_in_file_manager(path: &str) -> Result<(), AppError> {
    #[cfg(target_os = "linux")]
    let program = "xdg-open";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "windows")]
    let program = "explorer";

    std::process::Command::new(program)
        .arg(path)
        .spawn()
        .map_err(|e| AppError::Backend(format!("failed to open '{path}': {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;

    /// A migrated (v2) database in a throwaway temp dir. A process-wide atomic
    /// counter keeps each test's directory unique even under parallel runs.
    fn test_db() -> (Connection, std::path::PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "orcastudio-jobs-test-{}-{}",
            std::process::id(),
            n
        ));
        std::fs::remove_dir_all(&dir).ok();
        let conn = init_db(&dir).expect("init_db should succeed");
        (conn, dir)
    }

    #[test]
    fn create_lists_job_as_draft() {
        let (conn, dir) = test_db();

        let job = create_job_conn(&conn, "water opt", "! r2SCAN-3c Opt", None, None).unwrap();
        assert_eq!(job.status, JobStatus::Draft);
        assert!(job.started_at.is_none());
        assert!(job.completed_at.is_none());

        let all = list_jobs_conn(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, job.id);
        assert_eq!(all[0].status, JobStatus::Draft);
        // No snapshot passed → NULL, not an empty string.
        assert_eq!(job.scene_json, None);
        // v16: a plain create is ungrouped — group_id round-trips as NULL through COLUMNS.
        assert_eq!(job.group_id, None);
        assert_eq!(all[0].group_id, None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_optts_child_is_draft_generic_source_and_joins_source_pathway() {
        let (conn, dir) = test_db();

        // A GENERIC source job — a relaxed scan today (LooseOpt + a %geom Scan block); the engine
        // does not care what produced it (source-agnostic, ADR-020).
        let scan_input = "! r2SCAN-3c LooseOpt SMD(DMF) TightSCF\n\
             %geom Scan B 0 1 = 3.0, 1.8, 12 end end\n* xyz 0 1\nN 0 0 0\nC 0 0 1.8\n*\n";
        let source = create_job_conn(&conn, "Menshutkin scan", scan_input, None, None).unwrap();

        // The OptTS child input the TS engine (`buildOptTSInput`) would have produced — no Scan,
        // no LooseOpt (the create boundary trusts the pure builder's post-conditions).
        let ts_input = "! r2SCAN-3c SMD(DMF) OptTS Freq TightSCF\n\
             %geom Calc_Hess true end\n* xyz 0 1\nN 0 0 0\nC 0 0 2.353\n*\n";

        // Source NOT on a pathway → the child is a plain draft, pathway_id NULL (the caller
        // submits it to reach `queued`). No lineage column is stamped.
        let child = create_optts_job_conn(&conn, "OptTS — Menshutkin", ts_input, &source.id).unwrap();
        assert_eq!(child.status, JobStatus::Draft);
        assert_eq!(child.pathway_id, None);

        // Put the SOURCE on a pathway and refine again → the refined TS joins that SAME pathway
        // (the reaction view groups guess + TS together), via the shared attach mechanism.
        conn.execute(
            "INSERT INTO reactions (id, name, description) VALUES ('rx1', 'Menshutkin', NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO pathways (id, reaction_id, label) VALUES ('pw1', 'rx1', 'sn2')",
            [],
        )
        .unwrap();
        conn.execute("UPDATE jobs SET pathway_id = 'pw1' WHERE id = ?1", params![source.id])
            .unwrap();

        let ts2 = create_optts_job_conn(&conn, "OptTS #2", ts_input, &source.id).unwrap();
        assert_eq!(
            ts2.pathway_id.as_deref(),
            Some("pw1"),
            "the refined TS joins the source's pathway"
        );

        // Generic-source referential integrity: a missing source is a clean NotFound, and creates
        // NOTHING (no dangling child).
        let before = list_jobs_conn(&conn).unwrap().len();
        let err = create_optts_job_conn(&conn, "orphan", ts_input, "no-such-job").unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)));
        assert_eq!(
            list_jobs_conn(&conn).unwrap().len(),
            before,
            "a NotFound source created nothing"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_reopt_groups_by_source_and_reads_energies() {
        let (conn, dir) = test_db();

        // Source GOAT job (butane: 4 C then 10 H).
        let source_xyz = "! XTB GOAT\n* xyz 0 1\nC 0 0 0\nC 1 0 0\nH 2 0 0\n*\n";
        let source = create_job_conn(&conn, "Conformer search — butane", source_xyz, None, None)
            .unwrap();
        // A DIFFERENT GOAT job whose children must NOT bleed into the aggregate.
        let other = create_job_conn(&conn, "other search", source_xyz, None, None).unwrap();

        // Child 0 — Opt+Freq, completed with G + 0 imaginary (a clean minimum).
        let child0 = create_reopt_job_conn(
            &conn,
            "re-opt #0",
            "! r2SCAN-3c Opt Freq\n* xyz 0 1\nC 0 0 0\nC 1 0 0\nH 2 0 0\n*\n",
            &source.id,
            0,
        )
        .unwrap();
        // Child 1 — Opt+Freq, completed but Freq FAILED (no G), 1 imaginary (a saddle).
        let child1 = create_reopt_job_conn(
            &conn,
            "re-opt #1",
            "! r2SCAN-3c Opt Freq\n* xyz 0 1\nC 0 0 0\nC 1 0 0\nH 2 0 0\n*\n",
            &source.id,
            1,
        )
        .unwrap();
        // Child 2 — still running: no results row at all.
        let _child2 = create_reopt_job_conn(
            &conn,
            "re-opt #2",
            "! r2SCAN-3c Opt Freq\n* xyz 0 1\nC 0 0 0\nC 1 0 0\nH 2 0 0\n*\n",
            &source.id,
            2,
        )
        .unwrap();
        // Child of the OTHER job — must be excluded by grouping.
        create_reopt_job_conn(&conn, "other child", "! r2SCAN-3c Opt Freq", &other.id, 0)
            .unwrap();

        // Parsed results: child0 has electronic + G; child1 has electronic only + imaginary.
        conn.execute(
            "INSERT INTO results (job_id, final_energy_eh, free_energy_g_eh, imaginary_count, data_json, parser_version) \
             VALUES (?1, -157.5, -157.4, 0, '{}', 4)",
            params![child0.id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO results (job_id, final_energy_eh, free_energy_g_eh, imaginary_count, data_json, parser_version) \
             VALUES (?1, -157.49, NULL, 1, '{}', 4)",
            params![child1.id],
        )
        .unwrap();

        let agg = read_conformer_reoptimization_conn(&conn, &source.id).unwrap();

        // Grouping: exactly the 3 children of `source`, ordered by conformer index.
        assert_eq!(agg.children.len(), 3);
        assert_eq!(
            agg.children.iter().map(|c| c.source_conformer_index).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );

        // Child 0 — electronic + G present, 0 imaginary, Freq requested.
        assert_eq!(agg.children[0].electronic_energy_eh, Some(-157.5));
        assert_eq!(agg.children[0].gibbs_eh, Some(-157.4));
        assert_eq!(agg.children[0].imaginary_count, Some(0));
        assert!(agg.children[0].freq_requested);
        assert_eq!(agg.children[0].method.as_deref(), Some("r2SCAN-3c"));
        assert!(!agg.children[0].element_mismatch);

        // Child 1 — electronic present, G ABSENT (Freq failed), 1 imaginary (saddle).
        assert_eq!(agg.children[1].electronic_energy_eh, Some(-157.49));
        assert_eq!(agg.children[1].gibbs_eh, None);
        assert_eq!(agg.children[1].imaginary_count, Some(1));

        // Child 2 — no results row → all energies None, but still LISTED (never dropped).
        assert_eq!(agg.children[2].electronic_energy_eh, None);
        assert_eq!(agg.children[2].gibbs_eh, None);
        assert_eq!(agg.children[2].imaginary_count, None);

        // All 3 requested Freq → ΔG-mode, consistent.
        assert_eq!(agg.freq_requested_count, 3);
        assert!(!agg.mode_inconsistent);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_reopt_flags_element_mismatch_and_mode_inconsistency() {
        let (conn, dir) = test_db();
        let source = create_job_conn(
            &conn,
            "src",
            "! XTB GOAT\n* xyz 0 1\nC 0 0 0\nC 1 0 0\nH 2 0 0\n*\n",
            None,
            None,
        )
        .unwrap();

        // A child with a DIFFERENT composition (N instead of C) — must be flagged.
        create_reopt_job_conn(
            &conn,
            "wrong-atoms",
            "! r2SCAN-3c Opt Freq\n* xyz 0 1\nN 0 0 0\nC 1 0 0\nH 2 0 0\n*\n",
            &source.id,
            0,
        )
        .unwrap();
        // An Opt-ONLY child (no Freq) alongside a Freq child → mixed mode.
        create_reopt_job_conn(
            &conn,
            "opt-only",
            "! r2SCAN-3c Opt\n* xyz 0 1\nC 0 0 0\nC 1 0 0\nH 2 0 0\n*\n",
            &source.id,
            1,
        )
        .unwrap();

        let agg = read_conformer_reoptimization_conn(&conn, &source.id).unwrap();
        assert!(agg.children[0].element_mismatch, "N≠C composition must be flagged");
        assert!(!agg.children[1].element_mismatch);
        assert!(agg.children[0].freq_requested);
        assert!(!agg.children[1].freq_requested);
        assert!(agg.mode_inconsistent, "one Freq + one Opt-only is a mixed set");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_reopt_job_tags_source_and_conformer() {
        let (conn, dir) = test_db();

        // A source GOAT ensemble job, then a re-opt child of its conformer #2.
        let source = create_job_conn(&conn, "Conformer search — butane", "! XTB GOAT", None, None)
            .unwrap();
        let child = create_reopt_job_conn(
            &conn,
            "re-opt #2 — Conformer search — butane",
            "! r2SCAN-3c Opt Freq\n* xyz 0 1\nC 0 0 0\n*\n",
            &source.id,
            2,
        )
        .unwrap();

        // The child is a fresh draft (the caller submits it into the queue).
        assert_eq!(child.status, JobStatus::Draft);
        assert_ne!(child.id, source.id);

        // The two linkage FKs are stored (read them directly — they're not on the
        // Job model yet, that's D2b's read side).
        let (src_job, src_idx): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT source_ensemble_job_id, source_conformer_index FROM jobs WHERE id = ?1",
                params![child.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(src_job.as_deref(), Some(source.id.as_str()));
        assert_eq!(src_idx, Some(2));

        // The source job is untouched — no back-link, still a standalone job.
        let (src_src, _): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT source_ensemble_job_id, source_conformer_index FROM jobs WHERE id = ?1",
                params![source.id],
                |r| Ok((r.get(0)?, r.get::<_, Option<i64>>(1)?)),
            )
            .unwrap();
        assert_eq!(src_src, None, "the source ensemble job is not itself a re-opt child");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_persists_and_reloads_scene_json() {
        let (conn, dir) = test_db();

        let snap = r#"{"version":1,"fragments":[],"multiplicity":1}"#;
        let job = create_job_conn(&conn, "with scene", "! HF", Some(snap), None).unwrap();
        assert_eq!(job.scene_json.as_deref(), Some(snap));

        // Survives a fresh hydration (get + list both read the new column).
        let reloaded = get_job_conn(&conn, &job.id).unwrap();
        assert_eq!(reloaded.scene_json.as_deref(), Some(snap));
        let listed = list_jobs_conn(&conn).unwrap();
        assert_eq!(listed[0].scene_json.as_deref(), Some(snap));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_persists_and_reloads_scene_log_json() {
        // Unit 2b: the operation log column (v11) is co-written with scene_json and
        // reads back on get + list. The store owns the log format; this only checks
        // the column round-trips the opaque string.
        let (conn, dir) = test_db();

        let snap = r#"{"version":2,"fragments":[],"multiplicity":1,"nextAtomId":0}"#;
        let log = r#"{"version":1,"pointer":-1,"entries":[]}"#;
        let job = create_job_conn(&conn, "with log", "! HF", Some(snap), Some(log)).unwrap();
        assert_eq!(job.scene_log_json.as_deref(), Some(log));

        let reloaded = get_job_conn(&conn, &job.id).unwrap();
        assert_eq!(reloaded.scene_log_json.as_deref(), Some(log));
        let listed = list_jobs_conn(&conn).unwrap();
        assert_eq!(listed[0].scene_log_json.as_deref(), Some(log));

        // A job with no log → NULL (not empty string) — the legacy restore branch.
        let bare = create_job_conn(&conn, "no log", "! HF", None, None).unwrap();
        assert_eq!(bare.scene_log_json, None);

        std::fs::remove_dir_all(&dir).ok();
    }

    // A v2 scene (HF) and an input whose coordinate block matches it in order.
    const HF_SCENE: &str = r#"{"version":2,"fragments":[{"id":"a","name":"HF","atoms":[
        {"id":0,"element":"F","x":0.0,"y":0.0,"z":0.0},
        {"id":1,"element":"H","x":0.0,"y":0.0,"z":0.92}
    ],"charge":0,"source":"editor"}],"multiplicity":1,"nextAtomId":2}"#;

    fn stored_index_map(conn: &Connection, id: &str) -> crate::results::StoredIndexMap {
        let raw: String = conn
            .query_row("SELECT index_map_json FROM jobs WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    #[test]
    fn create_mints_the_index_map_from_matching_text() {
        let (conn, dir) = test_db();
        let input = "! HF def2-SVP\n* xyz 0 1\nF 0.0 0.0 0.0\nH 0.0 0.0 0.92\n*\n";
        let job = create_job_conn(&conn, "hf", input, Some(HF_SCENE), None).unwrap();
        assert_eq!(
            stored_index_map(&conn, &job.id),
            crate::results::StoredIndexMap::Minted(vec![0, 1])
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_skips_the_map_when_text_reorders_the_scene() {
        // Negative control (a): the submitted text swaps F and H vs the scene → a
        // self-describing SKIP, NOT a silent identity mint (which would mislabel
        // per-atom data). The job is still created (input validity is ORCA's business).
        let (conn, dir) = test_db();
        let input = "! HF\n* xyz 0 1\nH 0.0 0.0 0.92\nF 0.0 0.0 0.0\n*\n"; // reordered
        let job = create_job_conn(&conn, "hf", input, Some(HF_SCENE), None).unwrap();
        assert_eq!(job.status, JobStatus::Draft); // not blocked
        match stored_index_map(&conn, &job.id) {
            crate::results::StoredIndexMap::Skipped(reason) => {
                assert!(reason.contains("element"), "reason: {reason}");
            }
            other => panic!("expected a self-describing skip, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_without_a_scene_skips_the_map() {
        let (conn, dir) = test_db();
        let job = create_job_conn(&conn, "raw", "! HF\n* xyz 0 1\nF 0 0 0\nH 0 0 1\n*\n", None, None).unwrap();
        assert!(matches!(
            stored_index_map(&conn, &job.id),
            crate::results::StoredIndexMap::Skipped(_)
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn running_sets_started_at() {
        let (conn, dir) = test_db();

        let job = create_job_conn(&conn, "j", "! HF", None, None).unwrap();
        update_job_status_conn(&conn, &job.id, "running").unwrap();

        let reloaded = get_job_conn(&conn, &job.id).unwrap();
        assert_eq!(reloaded.status, JobStatus::Running);
        assert!(reloaded.started_at.is_some());
        assert!(reloaded.completed_at.is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completed_sets_completed_at() {
        let (conn, dir) = test_db();

        let job = create_job_conn(&conn, "j", "! HF", None, None).unwrap();
        update_job_status_conn(&conn, &job.id, "completed").unwrap();

        let reloaded = get_job_conn(&conn, &job.id).unwrap();
        assert_eq!(reloaded.status, JobStatus::Completed);
        assert!(reloaded.completed_at.is_some());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn get_missing_job_is_not_found() {
        let (conn, dir) = test_db();

        let err = get_job_conn(&conn, "no-such-id").unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn update_missing_job_is_not_found() {
        let (conn, dir) = test_db();

        let err = update_job_status_conn(&conn, "no-such-id", "running").unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_job_dir_persists() {
        let (conn, dir) = test_db();

        let job = create_job_conn(&conn, "j", "! HF", None, None).unwrap();
        set_job_dir_conn(&conn, &job.id, "/data/jobs/abc").unwrap();

        let reloaded = get_job_conn(&conn, &job.id).unwrap();
        assert_eq!(reloaded.job_dir.as_deref(), Some("/data/jobs/abc"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finalize_sets_status_error_and_timestamp() {
        let (conn, dir) = test_db();

        let job = create_job_conn(&conn, "j", "! HF", None, None).unwrap();
        finalize_job_conn(&conn, &job.id, JobStatus::Failed, Some("boom")).unwrap();

        let reloaded = get_job_conn(&conn, &job.id).unwrap();
        assert_eq!(reloaded.status, JobStatus::Failed);
        assert_eq!(reloaded.error_message.as_deref(), Some("boom"));
        assert!(reloaded.completed_at.is_some());

        std::fs::remove_dir_all(&dir).ok();
    }

    // --- delete_job_conn (Phase 4.7.1): DB core + FK cleanup + terminal-states guard ---

    /// Insert a minimal reaction row (id, name) — grouping metadata for the
    /// reference-row test.
    fn insert_reaction(conn: &Connection, id: &str, name: &str) {
        conn.execute(
            "INSERT INTO reactions (id, name) VALUES (?1, ?2)",
            params![id, name],
        )
        .unwrap();
    }

    /// Insert a minimal parsed `results` row for `job_id` (proves ON DELETE CASCADE).
    fn insert_results(conn: &Connection, job_id: &str) {
        conn.execute(
            "INSERT INTO results (job_id, final_energy_eh, data_json, parser_version) \
             VALUES (?1, -76.4, '{}', 4)",
            params![job_id],
        )
        .unwrap();
    }

    #[test]
    fn delete_job_removes_it_and_cascades_results() {
        let (conn, dir) = test_db();

        let job = create_job_conn(&conn, "j", "! HF", None, None).unwrap();
        insert_results(&conn, &job.id);

        let returned_dir = delete_job_conn(&conn, &job.id).unwrap();
        assert!(returned_dir.is_none(), "no job_dir was set → None");

        // Job gone.
        assert!(matches!(
            get_job_conn(&conn, &job.id),
            Err(AppError::NotFound(_))
        ));
        // Results row removed AUTOMATICALLY by ON DELETE CASCADE (never by hand).
        let results_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM results WHERE job_id = ?1",
                params![job.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(results_count, 0, "results cascade should have fired");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_job_returns_its_job_dir() {
        let (conn, dir) = test_db();

        let job = create_job_conn(&conn, "j", "! HF", None, None).unwrap();
        set_job_dir_conn(&conn, &job.id, "/data/jobs/xyz").unwrap();

        let returned_dir = delete_job_conn(&conn, &job.id).unwrap();
        assert_eq!(returned_dir.as_deref(), Some("/data/jobs/xyz"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_job_nulls_reopt_children() {
        let (conn, dir) = test_db();

        let source = create_job_conn(&conn, "GOAT source", "! XTB GOAT", None, None).unwrap();
        let child = create_reopt_job_conn(&conn, "re-opt #0", "! r2SCAN-3c Opt", &source.id, 3)
            .unwrap();

        delete_job_conn(&conn, &source.id).unwrap();

        // Child survives as a standalone job with BOTH linkage FKs nulled.
        let reloaded = get_job_conn(&conn, &child.id).unwrap();
        assert_eq!(reloaded.id, child.id, "child still present");
        let (src, idx): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT source_ensemble_job_id, source_conformer_index FROM jobs WHERE id = ?1",
                params![child.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(src, None, "source_ensemble_job_id nulled");
        assert_eq!(idx, None, "source_conformer_index nulled");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_job_drops_reference_rows() {
        let (conn, dir) = test_db();

        insert_reaction(&conn, "rxn1", "reduction");
        let job = create_job_conn(&conn, "reference job", "! HF", None, None).unwrap();
        conn.execute(
            "INSERT INTO reaction_reference_jobs (reaction_id, job_id) VALUES (?1, ?2)",
            params!["rxn1", job.id],
        )
        .unwrap();

        delete_job_conn(&conn, &job.id).unwrap();

        // The reference row is gone; the reaction row survives (jobs-survive, reaction side).
        let ref_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM reaction_reference_jobs WHERE job_id = ?1",
                params![job.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ref_count, 0, "reference row dropped");
        let rxn_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM reactions WHERE id = 'rxn1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rxn_count, 1, "the reaction itself survives");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_job_absent_is_not_found() {
        let (conn, dir) = test_db();
        assert!(matches!(
            delete_job_conn(&conn, "no-such-job"),
            Err(AppError::NotFound(_))
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_job_refuses_running() {
        let (conn, dir) = test_db();
        let job = create_job_conn(&conn, "j", "! HF", None, None).unwrap();
        update_job_status_conn(&conn, &job.id, JobStatus::Running.as_str()).unwrap();

        assert!(matches!(
            delete_job_conn(&conn, &job.id),
            Err(AppError::Backend(_))
        ));
        // Still present — nothing was deleted.
        assert!(get_job_conn(&conn, &job.id).is_ok());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_job_refuses_queued() {
        let (conn, dir) = test_db();
        let job = create_job_conn(&conn, "j", "! HF", None, None).unwrap();
        update_job_status_conn(&conn, &job.id, JobStatus::Queued.as_str()).unwrap();

        assert!(matches!(
            delete_job_conn(&conn, &job.id),
            Err(AppError::Backend(_))
        ));
        assert!(get_job_conn(&conn, &job.id).is_ok());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// NEGATIVE CONTROL — proves FK enforcement is ON and the A1 cleanup is
    /// REQUIRED, not decoration. With a re-opt child pointing at a job, a RAW
    /// `DELETE FROM jobs` WITHOUT the pre-delete NULL trips the RESTRICT FK.
    #[test]
    fn raw_delete_without_cleanup_hits_fk_constraint() {
        let (conn, dir) = test_db();

        let source = create_job_conn(&conn, "GOAT source", "! XTB GOAT", None, None).unwrap();
        let _child = create_reopt_job_conn(&conn, "re-opt #0", "! r2SCAN-3c Opt", &source.id, 0)
            .unwrap();

        // No cleanup — the child's source_ensemble_job_id still RESTRICT-references source.
        let raw = conn.execute("DELETE FROM jobs WHERE id = ?1", params![source.id]);
        assert!(
            raw.is_err(),
            "raw delete must fail the FK constraint (enforcement is ON)"
        );
        // And with the reference-row FK too.
        insert_reaction(&conn, "rxn1", "r");
        let refjob = create_job_conn(&conn, "ref", "! HF", None, None).unwrap();
        conn.execute(
            "INSERT INTO reaction_reference_jobs (reaction_id, job_id) VALUES (?1, ?2)",
            params!["rxn1", refjob.id],
        )
        .unwrap();
        let raw2 = conn.execute("DELETE FROM jobs WHERE id = ?1", params![refjob.id]);
        assert!(
            raw2.is_err(),
            "raw delete of a referenced job must fail the FK constraint too"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
