//! Application-wide error type.
//!
//! Every Tauri command returns `Result<T, AppError>`. `AppError` implements
//! `Serialize` so it can cross the IPC boundary to the frontend as a string.

use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("sidecar error: {0}")]
    Sidecar(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Internal invariant failure (e.g. a poisoned mutex). Kept separate from
    /// `Sidecar` so the source of the failure stays legible.
    #[error("internal error: {0}")]
    Internal(String),
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
