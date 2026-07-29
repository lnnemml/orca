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

/// The minimum sidecar `__version__` this build of the app expects. Bumped in
/// lockstep with `sidecar/app/__init__.py` whenever an endpoint is added or its
/// shape changes (see wiki/modules/sidecar.md). A running sidecar older than this
/// is reported as `stale` — the classic `npm run tauri dev` trap where the
/// frontend hot-reloads a new route but uvicorn (no `--reload` in release) is
/// still on old code, producing a bare `Not Found`. See wiki/debugging/005.
const EXPECTED_MIN_SIDECAR_VERSION: &str = "0.2.0";

/// Health state of the sidecar, surfaced to the frontend as a lowercase string.
#[derive(Clone, Copy, PartialEq)]
enum Health {
    Starting,
    Healthy,
    /// Responding, but older than `EXPECTED_MIN_SIDECAR_VERSION` — running stale
    /// code. Distinct from `Down`: the process is alive, it just needs a restart.
    Stale,
    Down,
}

impl Health {
    fn as_str(self) -> &'static str {
        match self {
            Health::Starting => "starting",
            Health::Healthy => "healthy",
            Health::Stale => "stale",
            Health::Down => "down",
        }
    }
}

/// Serializable status returned by the `get_sidecar_status` command.
#[derive(Clone, Serialize)]
pub struct SidecarStatus {
    /// One of `"healthy"`, `"starting"`, `"stale"`, `"down"`.
    pub status: String,
    /// Port the sidecar was launched on, if it has been started.
    pub port: Option<u16>,
    /// The sidecar's reported `__version__` once `/health` answered (`None` while
    /// starting / down / unparseable).
    pub version: Option<String>,
    /// The minimum version this app build expects — so the UI can name what to
    /// restart to.
    pub expected_version: String,
}

/// Compare two dot-separated versions **component-wise as numbers**, returning
/// true iff `actual >= expected`. String comparison is wrong here: `"0.10.0"`
/// sorts before `"0.9.0"` lexically but is the newer version. Non-numeric or
/// missing components count as 0, so a completely unparseable `actual` compares
/// as older (a version we can't read is treated as stale, not healthy). Pure.
fn version_at_least(actual: &str, expected: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim()
            .split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let a = parse(actual);
    let e = parse(expected);
    for i in 0..a.len().max(e.len()) {
        let av = a.get(i).copied().unwrap_or(0);
        let ev = e.get(i).copied().unwrap_or(0);
        if av != ev {
            return av > ev;
        }
    }
    true // all components equal
}

/// Pull the `version` string out of a `/health` JSON body, or `None` if the body
/// isn't the shape we expect (an old sidecar predating the field, or garbage).
fn parse_health_version(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("version")?
        .as_str()
        .map(str::to_string)
}

struct Inner {
    child: Option<Child>,
    port: Option<u16>,
    health: Health,
    /// The sidecar's reported version once `/health` answered.
    version: Option<String>,
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
                version: None,
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

        let mut cmd = Command::new(&python);
        cmd.arg("-m")
            .arg("uvicorn")
            .arg("app.main:app")
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string());

        // Dev only: hot-reload Python edits like the frontend HMR, so a change to
        // an endpoint is picked up without restarting the window (the trap this
        // whole unit exists to close). `--reload` spawns a WORKER child; putting
        // the whole thing in its own process group (below) lets `stop`/`Drop`
        // `killpg` the supervisor AND the worker — the debugging/004 pattern,
        // verified so no orphaned uvicorn keeps the port after exit.
        if cfg!(debug_assertions) {
            cmd.arg("--reload").arg("--reload-dir").arg("app");
        }

        // Own process group so `killpg` reaps the entire uvicorn tree on exit.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }

        let child = cmd
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
        inner.version = None;
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
            let _ = self.set_health(Health::Down, None);
            return;
        };

        let url = format!("http://127.0.0.1:{port}/health");
        for _ in 0..15 {
            if let Ok(resp) = ureq::get(&url).timeout(Duration::from_secs(2)).call() {
                if resp.status() == 200 {
                    // Responded — read the version and decide healthy vs stale.
                    let version = resp.into_string().ok().and_then(|b| parse_health_version(&b));
                    let health = match &version {
                        Some(v) if version_at_least(v, EXPECTED_MIN_SIDECAR_VERSION) => {
                            Health::Healthy
                        }
                        // Responding but older than we expect, or a version we
                        // couldn't read → stale (needs a restart, not dead).
                        _ => Health::Stale,
                    };
                    let _ = self.set_health(health, version);
                    return;
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        let _ = self.set_health(Health::Down, None);
    }

    fn set_health(&self, health: Health, version: Option<String>) -> Result<(), AppError> {
        let mut inner = self.lock()?;
        inner.health = health;
        if version.is_some() {
            inner.version = version;
        }
        Ok(())
    }

    pub fn get_status(&self) -> Result<SidecarStatus, AppError> {
        let inner = self.lock()?;
        Ok(SidecarStatus {
            status: inner.health.as_str().to_string(),
            port: inner.port,
            version: inner.version.clone(),
            expected_version: EXPECTED_MIN_SIDECAR_VERSION.to_string(),
        })
    }

    /// Kill the sidecar process (and its whole process group) if running.
    pub fn stop(&self) -> Result<(), AppError> {
        let mut inner = self.lock()?;
        if let Some(child) = inner.child.take() {
            kill_process_tree(child);
        }
        inner.health = Health::Down;
        inner.version = None;
        Ok(())
    }
}

/// Kill the uvicorn tree. With `--reload` uvicorn spawns a worker CHILD in the
/// same process group (verified: supervisor + resource_tracker + worker all
/// share the leader's pgid), so `child.kill()` alone would orphan the worker and
/// leave the port held — exactly the debugging/004 shape. We `process_group(0)`
/// the child (in `start`), so `killpg` here reaps the whole group: SIGTERM for a
/// graceful uvicorn shutdown, a short grace, then SIGKILL as the backstop.
fn kill_process_tree(mut child: Child) {
    #[cfg(unix)]
    {
        let pgid = child.id() as i32;
        // SAFETY: killpg is a thin syscall wrapper taking a pgid and signal.
        unsafe {
            libc::killpg(pgid, libc::SIGTERM);
        }
        std::thread::sleep(Duration::from_millis(300));
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
    let _ = child.kill(); // reap the parent handle / non-unix fallback
    let _ = child.wait();
}

impl Default for SidecarManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SidecarManager {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(child) = inner.child.take() {
                kill_process_tree(child);
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

#[cfg(test)]
mod tests {
    use super::{parse_health_version, version_at_least};

    #[test]
    fn version_compares_component_wise_not_lexically() {
        // The trap: lexical string order says "0.10.0" < "0.9.0" (because
        // '1' < '9'), i.e. it would call the NEWER version older.
        assert!("0.10.0" < "0.9.0"); // Rust &str byte comparison — the wrong answer
        // Component-wise numeric gets it right: 10 > 9.
        assert!(version_at_least("0.10.0", "0.9.0"));
        assert!(!version_at_least("0.9.0", "0.10.0"));
    }

    #[test]
    fn version_ordering_basics() {
        assert!(version_at_least("0.2.0", "0.2.0")); // equal
        assert!(version_at_least("0.2.1", "0.2.0"));
        assert!(version_at_least("1.0.0", "0.2.0"));
        assert!(!version_at_least("0.1.0", "0.2.0")); // the stale case
        assert!(!version_at_least("0.1.9", "0.2.0"));
        // differing component counts pad with zeros
        assert!(version_at_least("0.2", "0.2.0"));
        assert!(version_at_least("0.2.0.0", "0.2.0"));
    }

    #[test]
    fn unparseable_version_is_treated_as_stale() {
        // A version we can't read as numbers must NOT be reported as up-to-date —
        // its first component parses to 0, which is < the expected 0, wait: it is
        // < the expected minor, so it lands as stale.
        assert!(!version_at_least("", "0.2.0"));
        assert!(!version_at_least("garbage", "0.2.0"));
        assert!(!version_at_least("abc.def", "0.2.0")); // → [0,0] < [0,2,0]
    }

    #[test]
    fn parses_the_version_out_of_a_health_body() {
        assert_eq!(
            parse_health_version(r#"{"status":"ok","version":"0.2.0"}"#).as_deref(),
            Some("0.2.0"),
        );
        // Old body without the field, or garbage → None (caller treats as stale).
        assert_eq!(parse_health_version(r#"{"status":"ok"}"#), None);
        assert_eq!(parse_health_version("not json"), None);
    }
}
