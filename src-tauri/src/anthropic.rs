//! The single outbound network path to Anthropic (ADR-015 (1) + (3), the first realized
//! T1 tier of ADR-014).
//!
//! One egress point. The key never crosses into the webview: Rust reads it from the keyring
//! (`secrets::read_key`) and makes the call. Nothing here logs the key or a request/response
//! body — not even in debug. Model + API version are pinned in constants; the JSON is built
//! and parsed with `serde_json` (already a dependency) over `ureq`'s string bodies, so no new
//! dependency and no new feature is pulled in.
//!
//! Three calls:
//!   - `verify` / `list_models` — `GET /v1/models`: authenticates the key AND enumerates the
//!     models THIS key may use (rule #10: the option list is a measurement, not a doc lookup).
//!   - `explain_selection` — `POST /v1/messages`: T1 explain. The payload is exactly the three
//!     ADR-015 (3) fields (word + line + section); the command has NO parameter for the input
//!     file or coordinates — the bound is the command's type, and `build_explain_prompt`'s
//!     signature pins it (tested).

use rusqlite::OptionalExtension;
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;
use tauri::State;

use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::secrets;

/// API version header. Pinned per ADR-015 (`wiki/architecture/adr-015-api-key-storage.md`).
const API_VERSION: &str = "2023-06-01";

/// Default explain model — a **price/sufficiency** choice, so it lives in `settings` (ADR-004),
/// not here; this const is only the seed/fallback when the setting is unset. Sonnet 4.6 is
/// sufficient for explain (not chosen for novelty). **Review condition** (ADR-015 amendment):
/// Sonnet 4.6 is superseded by Sonnet 5 — revisit this default at Sonnet 4.6 deprecation or when
/// the live `/v1/models` list stops containing it. The live list (not this const) drives the UI's
/// options, so no *hardcoded* menu can go stale — only this one default string can.
const DEFAULT_MODEL: &str = "claude-sonnet-4-6";
/// The `settings` row holding the user's chosen model.
const MODEL_SETTING: &str = "anthropic_model";

/// `GET /v1/models` — authenticates without spending generation tokens and returns the models
/// available to this key. `limit=100` covers the whole Claude line in one page.
const MODELS_URL: &str = "https://api.anthropic.com/v1/models?limit=100";
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";

/// Explain is a short task; cap the reply.
const EXPLAIN_MAX_TOKENS: u32 = 1024;

/// The grounding contract, enforced as a system prompt: answer ONLY from the section the user
/// supplies — the structural half is that there is no button without an open section, so the
/// model has nothing to recall from; this is the belt to that suspenders (ADR-014 (1a)).
const EXPLAIN_SYSTEM: &str = "You explain a single ORCA keyword to a chemist who is learning \
quantum chemistry. Ground your answer ONLY in the manual section text the user provides — do \
not add facts, numbers, defaults, or values from memory. If the section does not cover \
something, say it is not covered rather than filling it in. Answer in two or three plain \
sentences.";

/// Offline is a normal mode in this project, not a failure — bound the wait so a dead network
/// gives a clear error instead of hanging.
const TIMEOUT: Duration = Duration::from_secs(20);

/// One model the key may use. `display_name` is optional — parsed defensively (rule #10: the
/// shape is whatever the real endpoint returns, not what a doc claims). **Pricing is not
/// surfaced: `/v1/models` does not return it, and hardcoding a price would be exactly the
/// recalled-constant anti-pattern ADR-014 (1a) forbids** — so price is omitted, not invented.
#[derive(Debug, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: Option<String>,
}

// ---- Pure helpers (unit-tested; the invariants live here) -------------------

/// The user-message content for an explain call — **exactly the three ADR-015 (3) fields**.
/// Its signature is the type-level boundary: three `&str`, no path, no coordinates. The wiring
/// test pins it; adding a geometry parameter would change this signature and fail to compile.
pub fn build_explain_prompt(word: &str, line: &str, section: &str) -> String {
    format!(
        "Keyword: {word}\n\nUsed on this input line:\n{line}\n\nManual section for it:\n{section}\n\n\
Explain what `{word}` does here, grounded only in the section above."
    )
}

/// Map an HTTP status to a cause-specific error (TASK 5: different causes, different messages).
/// `401` = the key; `404` = the model this key can't reach; anything else = a generic API error.
fn status_error(code: u16, model: &str) -> AppError {
    match code {
        401 => AppError::Backend(
            "API key rejected by Anthropic (401 — check the key in Settings).".into(),
        ),
        404 => AppError::Backend(format!(
            "model \"{model}\" is not available to this key (404 — pick another in Settings)."
        )),
        other => AppError::Backend(format!("Anthropic API error (HTTP {other}).")),
    }
}

/// Turn a `ureq` transport failure into the offline message (never leaks the URL or key).
fn transport_error(t: &ureq::Transport) -> AppError {
    AppError::Backend(format!("could not reach Anthropic (offline?): {}", t.kind()))
}

/// The first `text` block of a Messages response, or an error if none.
fn extract_text(body: &Value) -> Result<String, AppError> {
    let text = body
        .get("content")
        .and_then(Value::as_array)
        .and_then(|blocks| {
            blocks.iter().find_map(|b| {
                (b.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| b.get("text").and_then(Value::as_str))
                    .flatten()
            })
        });
    match text {
        Some(t) if !t.trim().is_empty() => Ok(t.to_string()),
        _ => Err(AppError::Backend("Anthropic returned no explanation text.".into())),
    }
}

/// Parse `data[]` of a `/v1/models` body into `ModelInfo`s (defensive: skips entries without an id).
fn parse_models(body: &Value) -> Vec<ModelInfo> {
    body.get("data")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("id").and_then(Value::as_str)?.to_string();
                    let display_name =
                        m.get("display_name").and_then(Value::as_str).map(String::from);
                    Some(ModelInfo { id, display_name })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn setting_or(conn: &rusqlite::Connection, key: &str, default: &str) -> Result<String, AppError> {
    let v: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
        .optional()?;
    Ok(v.unwrap_or_else(|| default.to_string()))
}

// ---- Network (blocking; always run OFF the UI thread via the async commands) ----

/// Models available to the stored/env key. Errors map by cause (401 → key, transport → offline).
pub fn list_models() -> Result<Vec<ModelInfo>, AppError> {
    let key = secrets::read_key()?;
    let body: Value = match ureq::get(MODELS_URL)
        .set("x-api-key", &key)
        .set("anthropic-version", API_VERSION)
        .timeout(TIMEOUT)
        .call()
    {
        Ok(r) => {
            let s = r
                .into_string()
                .map_err(|e| AppError::Backend(format!("could not read Anthropic response: {e}")))?;
            serde_json::from_str(&s)
                .map_err(|e| AppError::Backend(format!("could not parse model list: {e}")))?
        }
        Err(ureq::Error::Status(code, _)) => return Err(status_error(code, "")),
        Err(ureq::Error::Transport(t)) => return Err(transport_error(&t)),
    };
    Ok(parse_models(&body))
}

/// Minimal real proof the key works, for the "Check" button — reuses the model list.
pub fn verify_key() -> Result<String, AppError> {
    let models = list_models()?;
    Ok(format!("API key is valid ({} models available).", models.len()))
}

fn call_messages(model: &str, user_content: &str) -> Result<String, AppError> {
    let key = secrets::read_key()?;
    let body = serde_json::json!({
        "model": model,
        "max_tokens": EXPLAIN_MAX_TOKENS,
        "system": EXPLAIN_SYSTEM,
        "messages": [{ "role": "user", "content": user_content }],
    })
    .to_string();
    match ureq::post(MESSAGES_URL)
        .set("x-api-key", &key)
        .set("anthropic-version", API_VERSION)
        .set("content-type", "application/json")
        .timeout(TIMEOUT)
        .send_string(&body)
    {
        Ok(r) => {
            let s = r
                .into_string()
                .map_err(|e| AppError::Backend(format!("could not read Anthropic response: {e}")))?;
            let v: Value = serde_json::from_str(&s)
                .map_err(|e| AppError::Backend(format!("could not parse Anthropic response: {e}")))?;
            extract_text(&v)
        }
        Err(ureq::Error::Status(code, _)) => Err(status_error(code, model)),
        Err(ureq::Error::Transport(t)) => Err(transport_error(&t)),
    }
}

// ---- Tauri commands (all async → the blocking call runs OFF the GTK/WebKit main thread) ----

/// "Check" button.
#[tauri::command]
pub async fn verify_api_key() -> Result<String, AppError> {
    match tauri::async_runtime::spawn_blocking(verify_key).await {
        Ok(result) => result,
        Err(e) => Err(AppError::Backend(format!("verify task failed: {e}"))),
    }
}

/// The models this key may use — the live option list for the Settings model picker.
#[tauri::command]
pub async fn list_anthropic_models() -> Result<Vec<ModelInfo>, AppError> {
    match tauri::async_runtime::spawn_blocking(list_models).await {
        Ok(result) => result,
        Err(e) => Err(AppError::Backend(format!("model list task failed: {e}"))),
    }
}

/// T1 explain (ADR-014). Exactly three data fields — word + line + section. There is **no**
/// parameter for the input file or the geometry (coordinates are unpublished research; the bound
/// is this type). The model is read from `settings`, not passed in, so the wire choice is the
/// user's saved decision. Returns advice text — it writes **nothing** to the editor (ADR-014
/// tier-zero: there is no insert path in this flow).
#[tauri::command]
pub async fn explain_selection(
    db: State<'_, DbState>,
    word: String,
    line: String,
    section: String,
) -> Result<String, AppError> {
    // Quick sync read of the chosen model; the guard is dropped before the network await.
    let model = {
        let conn = db.lock()?;
        setting_or(&conn, MODEL_SETTING, DEFAULT_MODEL)?
    };
    let prompt = build_explain_prompt(&word, &line, &section);
    match tauri::async_runtime::spawn_blocking(move || call_messages(&model, &prompt)).await {
        Ok(result) => result,
        Err(e) => Err(AppError::Backend(format!("explain task failed: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wiring test (ADR-015 (3) / TASK 1): the explain payload builder takes **exactly three
    /// `&str`** — word, line, section. No path, no coordinates. If someone added a geometry
    /// parameter, this binding would not compile — the type IS the boundary. (Sibling of the
    /// secrets return-type pins.)
    #[test]
    fn explain_prompt_has_exactly_three_text_fields_no_geometry() {
        let _sig: fn(&str, &str, &str) -> String = build_explain_prompt;
        // And the built prompt carries the three fields, nothing more is required to build it.
        let p = build_explain_prompt("RIJCOSX", "! RIJCOSX", "RIJCOSX enables the RI-J ...");
        assert!(p.contains("RIJCOSX"));
        assert!(p.contains("! RIJCOSX"));
        assert!(p.contains("RIJCOSX enables the RI-J"));
    }

    /// TASK 5: error causes are distinct — a single "failed" would hide the cause. 401 names the
    /// key, 404 names the model, other codes are a generic API error.
    #[test]
    fn status_errors_distinguish_key_model_and_generic() {
        assert!(status_error(401, "claude-sonnet-4-6").to_string().contains("key"));
        let m404 = status_error(404, "claude-opus-4-8").to_string();
        assert!(m404.contains("claude-opus-4-8") && m404.contains("not available"));
        assert!(status_error(500, "x").to_string().contains("HTTP 500"));
    }

    #[test]
    fn parse_models_reads_id_and_optional_display_name() {
        let body = serde_json::json!({
            "data": [
                { "type": "model", "id": "claude-sonnet-4-6", "display_name": "Claude Sonnet 4.6" },
                { "type": "model", "id": "claude-opus-4-8" },
                { "type": "model" } // no id → skipped, not a panic
            ]
        });
        let models = parse_models(&body);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "claude-sonnet-4-6");
        assert_eq!(models[0].display_name.as_deref(), Some("Claude Sonnet 4.6"));
        assert_eq!(models[1].display_name, None);
    }

    #[test]
    fn extract_text_takes_the_first_text_block_or_errors() {
        let ok = serde_json::json!({ "content": [{ "type": "text", "text": "It enables RI-J." }] });
        assert_eq!(extract_text(&ok).unwrap(), "It enables RI-J.");
        let empty = serde_json::json!({ "content": [] });
        assert!(extract_text(&empty).is_err());
    }
}
