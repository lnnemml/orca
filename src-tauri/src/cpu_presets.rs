//! Measured CPU-pinning presets for the LocalBackend (domain rule #8).
//!
//! ORCA is pinned to an explicit core set with `taskset`, and OpenMPI's own
//! binding is disabled so the two don't fight over placement. The masks below
//! are the outcome of a controlled benchmark — see `wiki/orca/performance.md`.

use serde::Serialize;

/// A named CPU-pinning configuration: a `taskset -c` core mask plus the matching
/// `%pal nprocs` rank count.
///
/// **These masks are specific to the dev machine — a 12th-gen i5-12500H, whose
/// hybrid topology is 4 hyperthreaded P-cores on logical CPUs 0–7 and 8 E-cores
/// on 8–15.** On any other CPU they are WRONG: `0,2,4,6,8-15` names one thread
/// per physical core *here*, but arbitrary CPUs on a different layout. Do not
/// treat them as portable. For a different machine use the `custom` preset and
/// set the mask by hand (`lscpu -e` shows the topology). Automatic topology
/// detection is deliberately out of scope — a separate task.
pub struct CpuPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub mask: &'static str,
    pub nprocs: u32,
    pub description: &'static str,
}

impl CpuPreset {
    /// Look up a built-in preset by its stable id.
    pub fn by_id(id: &str) -> Option<&'static CpuPreset> {
        CPU_PRESETS.iter().find(|p| p.id == id)
    }
}

/// The built-in presets, in display order. Measured on the dev machine
/// (i5-12500H, 4 P-cores + 8 E-cores); see `wiki/orca/performance.md`.
pub const CPU_PRESETS: &[CpuPreset] = &[
    CpuPreset {
        id: "interactive",
        label: "Interactive",
        mask: "8-15",
        nprocs: 8,
        description: "E-cores only. 7% slower than P-cores but runs 20 °C cooler \
                      and leaves every P-core free — the machine stays usable.",
    },
    CpuPreset {
        id: "max_throughput",
        label: "Max throughput",
        mask: "0,2,4,6,8-15",
        nprocs: 12,
        description: "All 12 physical cores. Fastest (28% ahead of P-cores only), \
                      but the machine runs hot and busy.",
    },
];

/// The id used when settings are missing or name an unknown preset.
pub const DEFAULT_PRESET_ID: &str = "interactive";

/// A serde-serializable copy of a [`CpuPreset`] for the UI.
#[derive(Serialize)]
pub struct CpuPresetInfo {
    pub id: String,
    pub label: String,
    pub mask: String,
    pub nprocs: u32,
    pub description: String,
}

/// Expose the built-in presets to the frontend (Settings screen).
#[tauri::command]
pub fn get_cpu_presets() -> Vec<CpuPresetInfo> {
    CPU_PRESETS
        .iter()
        .map(|p| CpuPresetInfo {
            id: p.id.to_string(),
            label: p.label.to_string(),
            mask: p.mask.to_string(),
            nprocs: p.nprocs,
            description: p.description.to_string(),
        })
        .collect()
}
