//! The SMILES seam's real implementation (ADR-027 decision 4, unit M1b Part B): Rust's first call
//! into the Python sidecar. `POST http://127.0.0.1:<port>/smiles-to-3d` with `{"smiles": …}`
//! (`sidecar/app/smiles.py`, `SmilesToXyzRequest`); the reply is `SmilesToXyzResponse`
//! (`xyz`, `formula`, `charge`, `multiplicity`, `num_atoms`).
//!
//! **The reply is checked in our terms before it becomes a geometry (rule #9).** A sidecar that
//! answers 200 with nonsense is an `Err`, never a structure. [`check_reply`] requires:
//! - the body to be JSON with `xyz` (text), and `charge`, `multiplicity` and `num_atoms` as JSON
//!   integers (a `1.0` or a string is refused by the types);
//! - `num_atoms > 0`;
//! - `multiplicity == 1`, the sidecar's own contract ("always 1 (singlet) for now") and what P1's
//!   message tells Anton ("the SMILES sidecar, which always gives multiplicity 1"). Any other value
//!   would make that message false, so it is refused rather than passed on;
//! - the `xyz` text to be exactly one xyz frame: a count line equal to `num_atoms`, a comment line,
//!   then `num_atoms` rows of exactly four fields, and nothing after but blank lines;
//! - every element to be one `z_of` knows, after case normalisation (C6);
//! - every coordinate to be finite. Rust's `f64` parser accepts `NaN` and `inf`, so the check is
//!   explicit.
//!
//! The coordinates are Å, the xyz format's unit (`wiki/orca/parse-sources.md`), and are passed on
//! as text. The resolver reads them with the app's xyz reader and checks the count again
//! (`resolve::resolve_smiles`).
//!
//! **Blocking.** The call blocks for up to [`SMILES_TIMEOUT`]. Its caller (M3's `create_job`) runs
//! it off the GTK main thread, and the resolver never holds the database lock across it. No Tauri
//! command calls it yet.

use std::time::Duration;

use serde::Deserialize;

use super::classify::normalise_element;
use super::resolve::{SmilesStructure, SmilesTo3d};
use crate::parse::elements::z_of;
use crate::sidecar::SidecarStatus;

/// The bound on connecting to the sidecar (it is on this machine).
pub const SMILES_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// The bound on the whole request. ETKDG embedding plus MMFF of a drug-sized molecule takes well
/// under a second; 30 s leaves room for a large or awkward one without letting a hung sidecar hold
/// a `create_job` call.
pub const SMILES_TIMEOUT: Duration = Duration::from_secs(30);

/// The sidecar's reply, as far as the resolver reads it. `formula` is not read; an unknown field is
/// ignored, so a later sidecar may add fields.
#[derive(Debug, Deserialize)]
struct Reply {
    xyz: String,
    charge: i64,
    multiplicity: i64,
    num_atoms: usize,
}

/// The sidecar's `/smiles-to-3d` over HTTP. Built from the sidecar's status
/// ([`SidecarSmiles::from_status`]), so a sidecar that was never started or is down is an error at
/// the call, with no request made.
#[derive(Debug, Clone)]
pub struct SidecarSmiles {
    port: Option<u16>,
    /// The sidecar's health (`healthy`, `starting`, `stale`, `down`).
    health: String,
    timeout: Duration,
}

impl SidecarSmiles {
    /// From `SidecarManager::get_status()`.
    pub fn from_status(status: &SidecarStatus) -> SidecarSmiles {
        SidecarSmiles { port: status.port, health: status.status.clone(), timeout: SMILES_TIMEOUT }
    }

    /// A sidecar known to listen on `port` (the tests' canned server).
    #[cfg(test)]
    pub fn at_port(port: u16, timeout: Duration) -> SidecarSmiles {
        SidecarSmiles { port: Some(port), health: "healthy".into(), timeout }
    }

    fn url(&self) -> Result<String, String> {
        let port = self.port.ok_or("the SMILES sidecar was not started, so its port is unknown")?;
        if self.health == "down" {
            return Err("the SMILES sidecar is not running (its health check failed)".into());
        }
        Ok(format!("http://127.0.0.1:{port}/smiles-to-3d"))
    }
}

impl SmilesTo3d for SidecarSmiles {
    fn smiles_to_3d(&self, smiles: &str) -> Result<SmilesStructure, String> {
        let url = self.url()?;
        let body = serde_json::json!({ "smiles": smiles }).to_string();
        let agent = ureq::AgentBuilder::new().timeout_connect(SMILES_CONNECT_TIMEOUT).timeout(self.timeout).build();
        let reply = match agent.post(&url).set("content-type", "application/json").send_string(&body) {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                let detail = r.into_string().unwrap_or_default();
                return Err(format!("the SMILES sidecar answered HTTP {code}: {}", detail.trim()));
            }
            Err(ureq::Error::Transport(t)) => {
                return Err(format!("the SMILES sidecar could not be reached or did not answer in time: {t}"))
            }
        };
        if reply.status() != 200 {
            return Err(format!("the SMILES sidecar answered HTTP {} instead of 200", reply.status()));
        }
        let text = reply.into_string().map_err(|e| format!("the SMILES sidecar's reply could not be read: {e}"))?;
        check_reply(&text)
    }
}

/// The post-condition on a `/smiles-to-3d` reply body (rule #9; the list in the module doc).
pub(crate) fn check_reply(body: &str) -> Result<SmilesStructure, String> {
    let bad = |why: String| Err(format!("the SMILES sidecar's reply is not a structure: {why}"));
    let r: Reply = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return bad(format!("{e}")),
    };
    if r.num_atoms == 0 {
        return bad("it has no atoms".into());
    }
    if r.multiplicity != 1 {
        return bad(format!("multiplicity {} (the sidecar always gives 1)", r.multiplicity));
    }
    let lines: Vec<&str> = r.xyz.lines().collect();
    let count = lines.first().map(|l| l.trim()).unwrap_or("");
    if count.parse::<usize>().ok() != Some(r.num_atoms) {
        return bad(format!("the xyz count line `{count}` is not num_atoms = {}", r.num_atoms));
    }
    let end = r.num_atoms.saturating_add(2);
    let Some(rows) = lines.get(2..end) else {
        return bad(format!("the xyz has fewer than {} atom rows", r.num_atoms));
    };
    for (i, row) in rows.iter().enumerate() {
        let fields: Vec<&str> = row.split_whitespace().collect();
        let [element, x, y, z] = fields.as_slice() else {
            return bad(format!("atom row {i} `{row}` is not `element x y z`"));
        };
        if z_of(&normalise_element(element)).is_none() {
            return bad(format!("atom {i} has an unknown element `{element}`"));
        }
        for c in [x, y, z] {
            match c.parse::<f64>() {
                Ok(v) if v.is_finite() => {}
                _ => return bad(format!("atom {i} has a coordinate `{c}` that is not a finite number")),
            }
        }
    }
    if let Some(extra) = lines.get(end..).and_then(|rest| rest.iter().find(|l| !l.trim().is_empty())) {
        return bad(format!("text after the last atom row: `{extra}`"));
    }
    Ok(SmilesStructure { xyz: r.xyz, charge: r.charge, multiplicity: r.multiplicity, num_atoms: r.num_atoms })
}
