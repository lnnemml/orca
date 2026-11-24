mod anthropic;
mod artifacts;
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
mod in_flight;
mod local_backend;
mod manual;
// The MCP orchestration server's core (ADR-027): the M1a classifier and input assembly, the M1b
// grant core, steps, resolver, preflight and the sidecar's SMILES call, and the M2a Part A transport
// cores (`mcp::server`: lock, socket, server, gate, lifecycle, relay). M1b Part B wires the grant
// commands (`commands::mcp`) and the `submit_job` guard, and `set_setting` uses `mcp::switch`; M2a
// Part B wires the transport (`mcp::server::wiring`: setup, the tick, `ExitRequested`, the relay) and
// `mcp_status`/`mcp_set_enabled`; M3a wires `create_job` (steps A/B/C, the resolver, `SidecarSmiles`).
// The scoped allow stays while a few items still have no non-test caller (the audit log's reader and
// `active_grant_conn` for M4's panel, test accessors), so the build stays
// warning-clean without a faked caller.
#[allow(dead_code)]
mod mcp;
// The memory and disk preflight (ADR-024 o26, unit 5.5): the pure estimator and host-fact parsers.
mod memory;
mod models;
mod orca_json;
mod orca_plot;
mod output_search;
mod parse;
mod poller;
// Unit 5.2 Part A lands the pure remote-job classifier (ADR-024 l) with its tests; its callers
// (the snapshot collector in 5.2 Part B, SshBackend reconcile in 5.3/5.4) do not exist yet.
// Scoped allow until 5.3/5.4 wires it, so the build stays warning-clean without a faked caller.
#[allow(dead_code)]
mod remote;
mod result_extraction;
mod results;
mod secrets;
mod sidecar;
mod ssh_backend;
#[cfg(test)]
mod job_delete_pin_tests;
#[cfg(test)]
mod text_guard_tests;
mod timing;
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

            // --- MCP (ADR-027 M2a, M2b): the lifecycle over the managed DbState and the data dir
            // (`memory_estimate`'s disk), and its tick thread, whose first step runs at once (a
            // switch left on serves from app start). A path refusal never fails setup: it becomes
            // the status reason. ---
            let mcp_lifecycle = mcp::server::wiring::app_lifecycle(app.handle(), &data_dir);
            app.manage(mcp_lifecycle.clone());
            mcp::server::wiring::start_ticks(mcp_lifecycle)?;

            // --- LocalBackend: job-directory root + single execution slot. ---
            app.manage(local_backend::JobRunner::new(data_dir.clone()));
            // --- Remote jobs: one operation in flight per job (ADR-024 o item 4); empty on launch. ---
            app.manage(in_flight::InFlight::default());
            // --- The remote poller (ADR-024 o item 4, o16): the live log and the per-launch memory,
            // both empty on launch. ---
            app.manage(poller::live_log::LiveLog::new());
            app.manage(poller::plan::PollerMemory::new());
            // --- The poller's per-job remote records, the view's structure (ADR-024 o19); empty on
            // launch, re-derived by the first status steps. ---
            app.manage(poller::remote_state::RemoteStates::new());

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

            // Resume monitoring every non-terminal remote job (ADR-024 o item 4). Last, once the
            // database and the poller's state are managed; it never blocks setup.
            poller::app::start(app.handle().clone())?;

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
            commands::jobs::submit_job,
            commands::jobs::cancel_job,
            commands::jobs::undo_cancel,
            commands::remote_jobs::retry_remote_submit,
            commands::remote_jobs::label_remote_job,
            commands::remote_jobs::retry_remote_fetch,
            commands::remote_jobs::watch_job_log,
            commands::remote_jobs::remote_job_state,
            commands::mcp::mcp_create_grant,
            commands::mcp::mcp_revoke_grant,
            commands::mcp::mcp_get_grant,
            commands::mcp::mcp_level_from_job,
            commands::mcp::mcp_status,
            commands::mcp::mcp_set_enabled,
            commands::mcp::mcp_claude_jobs_summary,
            commands::mcp::mcp_cancel_claude_jobs,
            commands::mcp::mcp_list_audit,
            commands::mcp::mcp_audit_row,
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
            commands::server_profiles::clear_profile_verified,
            commands::server_profiles::test_server_profile,
            commands::memory::memory_estimate,
            commands::memory::apply_maxcore,
            commands::memory::host_resources,
            commands::memory::local_fitted_maxcore,
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
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            // The exit flow (ADR-027 M2 amendment, item 6), off the main thread: the MCP wait, then
            // `terminate_on_exit` (an ORCA tree's MPI ranks escape its process group and would
            // outlive the app), then the sidecar stop, then the lock release and `app.exit(0)`,
            // whose own `ExitRequested` passes through. Nothing runs here but the hand-off.
            if mcp::server::wiring::exit_requested(app_handle) {
                api.prevent_exit();
            }
        }
    });
}

/// Whether the process was started as `orcastudio --mcp` (ADR-027 M2 amendment, item 3). `main.rs`
/// asks before [`run`], so the relay opens no window and initialises no GTK.
pub fn is_mcp_relay(first_arg: Option<&std::ffi::OsStr>) -> bool {
    mcp::server::wiring::is_relay_invocation(first_arg)
}

/// `orcastudio --mcp`: the stdio relay to this user's MCP socket. Returns the process exit code
/// (0 the client closed, 1 the server closed or a stream failed, 2 not reachable or the path refused).
/// Nothing but relayed bytes reaches stdout.
pub fn mcp_relay() -> i32 {
    mcp::server::wiring::relay_main()
}
