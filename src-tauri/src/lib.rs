mod commands;
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
            app.manage(DbState(Mutex::new(conn)));

            // --- LocalBackend: job-directory root + single execution slot. ---
            app.manage(local_backend::JobRunner::new(data_dir.clone()));

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
            commands::jobs::read_job_output,
            commands::jobs::open_job_folder,
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
