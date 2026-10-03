mod anthropic;
mod commands;
// The connection test's transport, output parser and verdict (unit 5.1 Part B, Part A: pure and
// tested). Its production caller — the Tauri command that runs `ssh <host> bash -s` — is the
// wiring half of 5.1 Part B, so until then it is exercised only by its own tests. Scoped allow so
// the build stays warning-clean without a faked caller; removed when that command lands.
#[allow(dead_code)]
mod connection_test;
mod convergence;
mod cpu_presets;
mod crest;
mod db;
mod error;
mod execution_backend;
mod local_backend;
mod manual;
mod models;
mod orca_json;
mod orca_plot;
mod output_search;
mod parse;
// Unit 5.2 Part A lands the pure remote-job classifier (ADR-024 l) with its tests; its callers
// (the snapshot collector in 5.2 Part B, SshBackend reconcile in 5.3/5.4) do not exist yet.
// Scoped allow until 5.3/5.4 wires it, so the build stays warning-clean without a faked caller.
#[allow(dead_code)]
mod remote;
mod result_extraction;
mod results;
mod secrets;
mod sidecar;
mod xtb;

use std::path::Path;
use std::sync::{Arc, Mutex};

use tauri::Manager;

use commands::settings::DbState;
use sidecar::SidecarManager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // --- Storage: single SQLite file under the user data dir (ADR-004). ---
            let data_dir = dirs::data_dir()
                .ok_or("could not determine user data directory")?
                .join("orcastudio");
            let conn = db::init_db(&data_dir)?;
            // Reconcile jobs left `running` by a previous crash/close before the
            // connection is handed to managed state.
            local_backend::reconcile_on_startup(&conn);
            app.manage(DbState(Mutex::new(conn)));

            // --- LocalBackend: job-directory root + single execution slot. ---
            app.manage(local_backend::JobRunner::new(data_dir.clone()));

            // --- xtb pre-optimizer: its own single-slot runner (2.5.5). ---
            app.manage(xtb::XtbRunner::default());
            // --- CREST QCG grow: its own single-slot runner (Stage F F1b). ---
            app.manage(crest::CrestRunner::default());
            // Prune old kept diagnostic dirs (keep the newest few) off-thread so
            // setup returns promptly (2.5.5-fix-3).
            let prune_dir = data_dir.clone();
            std::thread::spawn(move || xtb::prune_diagnostic_dirs(&prune_dir));

            // Resume the sequential queue: pick up any jobs left `queued` across a
            // restart. Off-thread so setup returns promptly.
            let queue_handle = app.handle().clone();
            std::thread::spawn(move || local_backend::try_start_next(&queue_handle));

            // --- Sidecar: spawn uvicorn, then health-poll on a background thread. ---
            // In dev the sidecar lives at <project root>/sidecar (sibling of src-tauri).
            let sidecar_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .ok_or("could not locate project root from CARGO_MANIFEST_DIR")?
                .join("sidecar");
            let log_path = data_dir.join("sidecar.log");

            let sidecar = Arc::new(SidecarManager::new());
            sidecar.start(&sidecar_dir, &log_path)?;
            app.manage(sidecar.clone());

            let health_handle = sidecar.clone();
            std::thread::spawn(move || health_handle.health_check());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::settings::get_settings,
            commands::settings::set_setting,
            commands::jobs::create_job,
            commands::jobs::create_reopt_job,
            commands::jobs::create_optts_job,
            commands::jobs::create_neb_job,
            commands::jobs::read_conformer_reoptimization,
            commands::jobs::list_jobs,
            commands::jobs::get_job,
            commands::jobs::rename_job,
            commands::jobs::update_job_status,
            commands::jobs::submit_job,
            commands::jobs::cancel_job,
            commands::jobs::delete_job,
            commands::jobs::pause_queue,
            commands::jobs::resume_queue,
            commands::jobs::is_queue_paused,
            commands::jobs::read_job_output,
            commands::jobs::read_job_output_for_viewer,
            commands::jobs::read_job_ensemble,
            commands::jobs::read_job_convergence,
            commands::jobs::read_job_results,
            commands::jobs::reparse_job,
            commands::jobs::read_scan_geometries,
            commands::jobs::read_scan_surface,
            commands::jobs::read_neb_geometries,
            commands::jobs::read_orbital_cube,
            commands::export::write_export_text,
            commands::export::write_export_bytes,
            commands::export::export_group,
            commands::export::export_job,
            commands::export::export_selection,
            commands::jobs::open_job_folder,
            output_search::search_job_output,
            output_search::get_search_presets,
            cpu_presets::get_cpu_presets,
            commands::molecules::create_molecule,
            commands::molecules::list_molecules,
            commands::molecules::create_reagent,
            commands::molecules::list_reagents,
            commands::molecules::get_molecule,
            commands::molecules::update_molecule,
            commands::molecules::delete_molecule,
            commands::reactions::create_reaction,
            commands::reactions::list_reactions,
            commands::reactions::rename_reaction,
            commands::reactions::delete_reaction,
            commands::reactions::create_pathway,
            commands::reactions::list_pathways,
            commands::reactions::delete_pathway,
            commands::reactions::attach_job_to_pathway,
            commands::reactions::detach_job_from_pathway,
            commands::reactions::add_reference_job,
            commands::reactions::remove_reference_job,
            commands::reactions::list_reference_jobs,
            commands::reactions::reaction_reference_energy,
            commands::server_profiles::create_server_profile,
            commands::server_profiles::list_server_profiles,
            commands::server_profiles::update_server_profile,
            commands::server_profiles::delete_server_profile,
            commands::server_profiles::set_profile_verified,
            commands::server_profiles::clear_profile_verified,
            commands::groups::create_group,
            commands::groups::list_groups,
            commands::groups::rename_group,
            commands::groups::move_group,
            commands::groups::move_job,
            commands::groups::delete_group,
            commands::manual::build_manual_index,
            commands::manual::search_manual,
            commands::manual::get_manual_section,
            commands::manual::get_manual_page,
            commands::manual::manual_index_status,
            commands::manual::resolve_manual_section,
            commands::manual::resolve_manual_anchors,
            sidecar::get_sidecar_status,
            xtb::xtb_version,
            xtb::xtb_optimize,
            xtb::xtb_cancel,
            crest::crest_grow,
            crest::crest_cancel,
            secrets::api_key_status,
            secrets::set_api_key,
            secrets::delete_api_key,
            anthropic::verify_api_key,
            anthropic::list_anthropic_models,
            anthropic::explain_selection,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if let tauri::RunEvent::ExitRequested { .. } = event {
            // Kill any running ORCA tree first — its MPI ranks escape the
            // parent's process group, so if we just exit they outlive the app
            // burning CPU (mpirun can't forward our signal once we're gone, and
            // no reconciliation runs until the next launch). Synchronous by
            // design (see `terminate_on_exit`).
            local_backend::terminate_on_exit(app_handle);
            // Kill the sidecar on a clean exit so uvicorn doesn't outlive the app.
            if let Some(sidecar) = app_handle.try_state::<Arc<SidecarManager>>() {
                let _ = sidecar.stop();
            }
        }
    });
}
