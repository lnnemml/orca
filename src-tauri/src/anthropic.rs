//! The single outbound network path to Anthropic (ADR-015 (1) + (3)).
//!
//! One egress point. The key never crosses into the webview: Rust reads it from
//! the keyring (`secrets::read_key`) and makes the call. Nothing here logs the
//! key or a request/response body — not even in debug. Model and API version are
//! pinned in constants, not scattered.
//!
//! This unit ships only the minimal `verify` call behind the Settings "Check"
//! button. The full Explain payload (word + surrounding line + section text —
//! ADR-015 (3)) is the next unit.

use std::time::Duration;

use crate::error::AppError;
use crate::secrets;

/// API version header. Pinned per ADR-015 (`wiki/architecture/adr-015-api-key-storage.md`).
const API_VERSION: &str = "2023-06-01";

/// Default model for the Explain unit that follows. Unused by `verify` (which
/// spends no generation tokens), kept here so the model id lives in ONE place.
#[allow(dead_code)]
const MODEL: &str = "claude-opus-4-8";

/// `GET /v1/models` authenticates the key WITHOUT spending generation tokens —
/// the cheapest real proof that a key works. `limit=1` keeps the body tiny.
const MODELS_URL: &str = "https://api.anthropic.com/v1/models?limit=1";

/// Offline is a normal mode in this project, not a failure — bound the wait so a
/// dead network gives a clear error instead of hanging the "Check" button.
const TIMEOUT: Duration = Duration::from_secs(15);

/// Minimal real call proving the stored/env key works. `200` ⇒ valid; `401` ⇒
/// the key was rejected; a transport error ⇒ offline / unreachable. Returns a
/// short human status line on success — never the key, never a response body.
pub fn verify_key() -> Result<String, AppError> {
    let key = secrets::read_key()?;
    match ureq::get(MODELS_URL)
        .set("x-api-key", &key)
        .set("anthropic-version", API_VERSION)
        .timeout(TIMEOUT)
        .call()
    {
        Ok(_) => Ok("API key is valid.".into()),
        Err(ureq::Error::Status(401, _)) => Err(AppError::Backend(
            "API key rejected by Anthropic (401 — check the key).".into(),
        )),
        Err(ureq::Error::Status(code, _)) => {
            Err(AppError::Backend(format!("Anthropic returned HTTP {code}.")))
        }
        // Offline / DNS / TLS — report the kind, never the URL or key.
        Err(ureq::Error::Transport(t)) => Err(AppError::Backend(format!(
            "could not reach Anthropic (offline?): {}",
            t.kind()
        ))),
    }
}

/// "Check" button command. Runs the blocking call OFF the GTK/WebKit main thread
/// (`tauri-core.md` threading rule — a 15 s offline timeout must not freeze the
/// window). The frontend gates this behind a usable key state, so it is only
/// ever invoked when a key exists.
#[tauri::command]
pub async fn verify_api_key() -> Result<String, AppError> {
    match tauri::async_runtime::spawn_blocking(verify_key).await {
        Ok(result) => result,
        Err(e) => Err(AppError::Backend(format!("verify task failed: {e}"))),
    }
}
