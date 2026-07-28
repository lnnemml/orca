mod commands;
mod convergence;
mod cpu_presets;
mod db;
mod error;
mod local_backend;
mod models;
mod result_extraction;
mod sidecar;

use std::path::Path;
use std::sync::{Arc, Mutex};

use tauri::Manager;

use commands::settings::DbState;
use sidecar::SidecarManager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
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
            commands::jobs::read_job_convergence,
            commands::jobs::open_job_folder,
            cpu_presets::get_cpu_presets,
            commands::molecules::create_molecule,
            commands::molecules::list_molecules,
            commands::molecules::get_molecule,
            commands::molecules::update_molecule,
            commands::molecules::delete_molecule,
            sidecar::get_sidecar_status,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        // Kill the sidecar on a clean exit so uvicorn doesn't outlive the app.
        if let tauri::RunEvent::ExitRequested { .. } = event {
            if let Some(sidecar) = app_handle.try_state::<Arc<SidecarManager>>() {
                let _ = sidecar.stop();
            }
        }
    });
}
