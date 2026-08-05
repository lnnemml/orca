//! Anthropic API key storage in the system keyring (ADR-015).
//!
//! Two invariants this module exists to enforce:
//!
//! 1. **The key never crosses into the webview.** No command returns it. The
//!    frontend learns only the *source state* (`KeySource`) and, for recognition,
//!    the last 4 characters — never the key. Rust makes the network call itself
//!    (`anthropic.rs`).
//! 2. **No silent plaintext fallback.** The key lives in the OS keyring (Secret
//!    Service on Linux — measured working, `wiki/architecture/keyring-availability.md`).
//!    When the keyring is unusable we read `ANTHROPIC_API_KEY` and *say so*; we do
//!    NOT write the key to `orcastudio.db`.
//!
//! The four-state model is structural, not cosmetic: the fallback's trigger is a
//! *third* state (`NoDefaultStore` — "no keyring backend") that is distinct from
//! `NoEntry` ("keyring works, no key"). A two-state have/haven't model cannot
//! express why the fallback fired, so the UI could not tell the truth. See ADR-015 (2).

use keyring::{Entry, Error as KeyringError};
use serde::Serialize;

use crate::error::AppError;

/// Keyring service name — the string the probe verified on this host.
const SERVICE: &str = "orcastudio";
/// Account under the service. One key today; a named account leaves room for more.
const ACCOUNT: &str = "anthropic-api-key";
/// The environment variable consulted only when the keyring is unusable.
const ENV_VAR: &str = "ANTHROPIC_API_KEY";

/// Where a usable key would come from — the ONLY key-related information the
/// frontend ever receives. It never carries the key itself (invariant 1); `last4`
/// is for recognition only. Serialized internally-tagged, kebab-case:
/// `{"state":"stored-in-keyring","last4":"wxyz"}`.
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum KeySource {
    /// Keyring works and holds a key.
    StoredInKeyring { last4: String },
    /// Keyring works, no key stored → prompt the user to enter one.
    Absent,
    /// Keyring unusable, `ANTHROPIC_API_KEY` is set → using the env var (shown in UI).
    FromEnvironment { last4: String },
    /// Keyring unusable and no env var → name BOTH causes.
    Unavailable { reason: String },
}

fn entry() -> Result<Entry, KeyringError> {
    Entry::new(SERVICE, ACCOUNT)
}

/// Last 4 characters, for recognition. Char-safe (keys are ASCII, but no panic
/// on a short/odd value): a key shorter than 4 chars yields all of it.
fn last4(key: &str) -> String {
    let tail: Vec<char> = key.chars().rev().take(4).collect();
    tail.into_iter().rev().collect()
}

/// `ANTHROPIC_API_KEY`, treating empty/whitespace-only as unset.
fn env_key() -> Option<String> {
    std::env::var(ENV_VAR)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Resolve the key-source state WITHOUT returning the key. This is what the
/// frontend sees. The env var is consulted *only* when the keyring is unusable —
/// a working-but-empty keyring is `Absent`, not `FromEnvironment` (the env var is
/// a fallback for *unavailability*, not an override of an empty store — ADR-015 (2)).
pub fn key_source() -> KeySource {
    match entry().and_then(|e| e.get_password()) {
        Ok(key) => KeySource::StoredInKeyring { last4: last4(&key) },
        // Keyring works, nothing stored.
        Err(KeyringError::NoEntry) => KeySource::Absent,
        // Keyring unusable: NoDefaultStore (no backend) / NoStorageAccess /
        // PlatformFailure (e.g. locked). Fall back to the environment, visibly.
        Err(e) => match env_key() {
            Some(key) => KeySource::FromEnvironment { last4: last4(&key) },
            None => KeySource::Unavailable {
                reason: format!("system keyring unavailable ({e}) and {ENV_VAR} is not set"),
            },
        },
    }
}

/// Read the actual key for Rust's OWN use (the network module only). NEVER
/// exposed to the frontend — there is no command that calls this. Mirrors
/// `key_source`: the keyring value if present, else the env var when the keyring
/// is unusable.
pub fn read_key() -> Result<String, AppError> {
    match entry().and_then(|e| e.get_password()) {
        Ok(key) => Ok(key),
        Err(KeyringError::NoEntry) => Err(AppError::NotFound(
            "no Anthropic API key in the system keyring".into(),
        )),
        Err(e) => env_key().ok_or_else(|| {
            AppError::Backend(format!(
                "system keyring unavailable ({e}) and {ENV_VAR} is not set"
            ))
        }),
    }
}

/// Store the key in the keyring. Fails with a clear error if the keyring is
/// unavailable — there is deliberately NO silent fallback to plaintext-in-the-DB
/// (ADR-015 (2)); the caller/UI must surface the failure.
pub fn store_key(key: &str) -> Result<(), AppError> {
    let key = key.trim();
    if key.is_empty() {
        return Err(AppError::Backend("API key is empty".into()));
    }
    entry()
        .and_then(|e| e.set_password(key))
        .map_err(|e| AppError::Backend(format!("could not store key in the system keyring: {e}")))
}

/// Delete the key from the keyring. A missing key is not an error (idempotent).
pub fn delete_key() -> Result<(), AppError> {
    match entry().and_then(|e| e.delete_credential()) {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(e) => Err(AppError::Backend(format!(
            "could not delete key from the system keyring: {e}"
        ))),
    }
}

// ---- Tauri commands (thin wrappers; NONE returns the key) ------------------

/// The key's source state — the only key information the frontend may learn.
#[tauri::command]
pub fn api_key_status() -> KeySource {
    key_source()
}

/// Store a key in the system keyring.
#[tauri::command]
pub fn set_api_key(key: String) -> Result<(), AppError> {
    store_key(&key)
}

/// Remove the key from the system keyring.
#[tauri::command]
pub fn delete_api_key() -> Result<(), AppError> {
    delete_key()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last4_returns_only_the_last_four() {
        assert_eq!(last4("sk-ant-secret-material-WXYZ"), "WXYZ");
        // Short values don't panic and don't over-read.
        assert_eq!(last4("ab"), "ab");
        assert_eq!(last4(""), "");
    }

    /// Wiring test (ADR-015 invariant 1): the state the frontend receives carries
    /// the LAST 4 chars for recognition and NOTHING of the secret body. This bites:
    /// if `KeySource` were changed to carry the full key, the `!contains(secret)`
    /// assertion fails (the negative control).
    #[test]
    fn key_source_serializes_without_the_secret_body() {
        let secret = "sk-ant-secret-material-WXYZ";
        let source = KeySource::StoredInKeyring { last4: last4(secret) };
        let json = serde_json::to_string(&source).unwrap();

        assert!(json.contains("stored-in-keyring"), "{json}");
        assert!(json.contains("WXYZ"), "{json}");
        assert!(
            !json.contains("secret-material"),
            "the serialized state must not contain the key body: {json}"
        );
        assert!(!json.contains(secret), "{json}");
    }

    #[test]
    fn key_source_states_serialize_with_the_kebab_tag() {
        assert_eq!(
            serde_json::to_string(&KeySource::Absent).unwrap(),
            r#"{"state":"absent"}"#
        );
        let env = KeySource::FromEnvironment { last4: "wxyz".into() };
        assert_eq!(
            serde_json::to_string(&env).unwrap(),
            r#"{"state":"from-environment","last4":"wxyz"}"#
        );
        let un = KeySource::Unavailable { reason: "no keyring".into() };
        assert_eq!(
            serde_json::to_string(&un).unwrap(),
            r#"{"state":"unavailable","reason":"no keyring"}"#
        );
    }

    /// Wiring test (ADR-015 invariant 1): pin the RETURN TYPES of the key commands.
    /// None yields the key: status → `KeySource` (source only), set/delete →
    /// `Result<(), AppError>`. If someone added a `fn get_api_key() -> String`
    /// returning the key, it would not satisfy any of these bindings — the review
    /// (enumerated command list) plus these pins are the guard. `verify_api_key`
    /// (async) returns `Result<String, AppError>` — a human status line, asserted
    /// key-free where it is built (`anthropic.rs`).
    #[test]
    fn key_commands_return_only_source_or_unit_never_the_key() {
        let _status: fn() -> KeySource = api_key_status;
        let _set: fn(String) -> Result<(), AppError> = set_api_key;
        let _delete: fn() -> Result<(), AppError> = delete_api_key;
    }
}
