mod commands;
mod convergence;
mod cpu_presets;
mod db;
mod error;
mod local_backend;
mod manual;
mod models;
mod orca_json;
mod orca_plot;
mod output_search;
mod parse;
mod result_extraction;
mod results;
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
            commands::jobs::list_jobs,
            commands::jobs::get_job,
            commands::jobs::update_job_status,
            commands::jobs::submit_job,
            commands::jobs::cancel_job,
            commands::jobs::pause_queue,
            commands::jobs::resume_queue,
            commands::jobs::is_queue_paused,
            commands::jobs::read_job_output,
            commands::jobs::read_job_output_for_viewer,
            commands::jobs::read_job_ensemble,
            commands::jobs::read_job_convergence,
            commands::jobs::read_job_results,
            commands::jobs::read_orbital_cube,
            commands::export::write_export_text,
            commands::export::write_export_bytes,
            commands::jobs::open_job_folder,
            output_search::search_job_output,
            output_search::get_search_presets,
            cpu_presets::get_cpu_presets,
            commands::molecules::create_molecule,
            commands::molecules::list_molecules,
            commands::molecules::get_molecule,
            commands::molecules::update_molecule,
            commands::molecules::delete_molecule,
            sidecar::get_sidecar_status,
            xtb::xtb_version,
            xtb::xtb_optimize,
            xtb::xtb_cancel,
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
