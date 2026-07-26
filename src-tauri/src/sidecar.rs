//! Python sidecar lifecycle: spawn uvicorn, poll `/health`, kill on exit.
//!
//! Phase 0 keeps this deliberately simple: one sidecar per app instance, a
//! blocking health poll on a background thread, and a `Drop` that kills the
//! child. The port is chosen dynamically so multiple app instances don't clash.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::State;

use crate::error::AppError;

/// Health state of the sidecar, surfaced to the frontend as a lowercase string.
#[derive(Clone, Copy, PartialEq)]
enum Health {
    Starting,
    Healthy,
    Down,
}

impl Health {
    fn as_str(self) -> &'static str {
        match self {
            Health::Starting => "starting",
            Health::Healthy => "healthy",
            Health::Down => "down",
        }
    }
}

/// Serializable status returned by the `get_sidecar_status` command.
#[derive(Clone, Serialize)]
pub struct SidecarStatus {
    /// One of `"healthy"`, `"starting"`, `"down"`.
    pub status: String,
    /// Port the sidecar was launched on, if it has been started.
    pub port: Option<u16>,
}

struct Inner {
    child: Option<Child>,
    port: Option<u16>,
    health: Health,
}

pub struct SidecarManager {
    inner: Mutex<Inner>,
}

impl SidecarManager {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                child: None,
                port: None,
                health: Health::Down,
            }),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Inner>, AppError> {
        self.inner
            .lock()
            .map_err(|_| AppError::Internal("sidecar mutex poisoned".into()))
    }

    /// Spawn the uvicorn process for `sidecar_dir`, redirecting output to
    /// `log_path`. Sets status to `starting`; call [`health_check`] to confirm
    /// it came up.
    pub fn start(&self, sidecar_dir: &Path, log_path: &Path) -> Result<(), AppError> {
        let port = free_port()?;
        let python = python_interpreter(sidecar_dir);

        let log = std::fs::File::create(log_path)?;
        let log_err = log.try_clone()?;

        let child = Command::new(&python)
            .arg("-m")
            .arg("uvicorn")
            .arg("app.main:app")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .current_dir(sidecar_dir)
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .spawn()
            .map_err(|e| {
                AppError::Sidecar(format!(
                    "failed to spawn sidecar via {}: {e}",
                    python.display()
                ))
            })?;

        let mut inner = self.lock()?;
        inner.child = Some(child);
        inner.port = Some(port);
        inner.health = Health::Starting;
        Ok(())
    }

    /// Poll `GET /health` every 2s, up to 15 times (~30s). Updates status to
    /// `healthy` on the first 200, or `down` if it never responds. Intended to
    /// run on a background thread so it doesn't block app startup.
    pub fn health_check(&self) {
        let port = match self.lock() {
            Ok(inner) => inner.port,
            Err(_) => None,
        };
        let Some(port) = port else {
            let _ = self.set_health(Health::Down);
            return;
        };

        let url = format!("http://127.0.0.1:{port}/health");
        for _ in 0..15 {
            if let Ok(resp) = ureq::get(&url).timeout(Duration::from_secs(2)).call() {
                if resp.status() == 200 {
                    let _ = self.set_health(Health::Healthy);
                    return;
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        let _ = self.set_health(Health::Down);
    }

    fn set_health(&self, health: Health) -> Result<(), AppError> {
        self.lock()?.health = health;
        Ok(())
    }

    pub fn get_status(&self) -> Result<SidecarStatus, AppError> {
        let inner = self.lock()?;
        Ok(SidecarStatus {
            status: inner.health.as_str().to_string(),
            port: inner.port,
        })
    }

    /// Kill the sidecar process if running.
    pub fn stop(&self) -> Result<(), AppError> {
        let mut inner = self.lock()?;
        if let Some(mut child) = inner.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        inner.health = Health::Down;
        Ok(())
    }
}

impl Default for SidecarManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SidecarManager {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(mut child) = inner.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

/// Reserve an ephemeral port by binding to `:0`, then release it for uvicorn to
/// claim. A small race window exists but is acceptable for local dev.
fn free_port() -> Result<u16, AppError> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    Ok(port)
}

/// Prefer the sidecar's virtualenv interpreter; fall back to system `python3`.
fn python_interpreter(sidecar_dir: &Path) -> PathBuf {
    let venv = sidecar_dir.join(".venv/bin/python");
    if venv.exists() {
        venv
    } else {
        eprintln!(
            "[sidecar] venv interpreter not found at {}; falling back to system python3",
            venv.display()
        );
        PathBuf::from("python3")
    }
}

/// Tauri command: current sidecar health, polled by the frontend.
#[tauri::command]
pub fn get_sidecar_status(
    sidecar: State<'_, std::sync::Arc<SidecarManager>>,
) -> Result<SidecarStatus, AppError> {
    sidecar.get_status()
}
