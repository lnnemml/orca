//! Shared TypeScript types mirroring the Rust IPC surface.

export type JobStatus =
  | "draft"
  | "queued"
  | "running"
  | "completed"
  /** Completed AND its .property.txt results were parsed/verified/stored (Phase 3).
   * A `completed` job with an `error_message` ran fine but its results would not
   * parse — distinct from `failed` (the calculation itself failed). */
  | "parsed"
  | "failed"
  | "cancelled";

/** Mirrors `src-tauri/src/results.rs::ParsedResults` (the stored JSON). Per-atom
 * arrays carry their own element order — never trust position alone. */
export interface AtomCharges {
  scheme: "mulliken" | "loewdin" | "mayer";
  elements: string[];
  atomic_numbers: number[];
  charges: number[];
}
export interface ParsedResults {
  parser_version: number;
  final_energy_eh: number | null;
  dipole: { magnitude_au: number; total_au: [number, number, number] } | null;
  charges: AtomCharges[];
  thermochemistry: {
    el_energy_eh: number;
    zpe_eh: number;
    inner_energy_u_eh: number;
    enthalpy_h_eh: number;
    /** T·S in Eh, NOT entropy S. */
    t_times_s_eh: number;
    free_energy_g_eh: number;
  } | null;
  final_geometry: { elements: string[]; xyz_angstrom: [number, number, number][] };
  gradient: { geometry_index: number; order_elements: string[]; grad_eh_per_bohr: number[] } | null;
  unknown_blocks: string[];
}

/** Mirrors `src-tauri/src/cpu_presets.rs::CpuPresetInfo`. */
export interface CpuPresetInfo {
  id: string;
  label: string;
  mask: string;
  nprocs: number;
  description: string;
}

/** Mirrors `src-tauri/src/models/job.rs::Job` (serde, lowercase status). */
export interface Job {
  id: string;
  title: string;
  input_content: string;
  status: JobStatus;
  job_dir: string | null;
  energy: number | null;
  wall_time: number | null;
  error_message: string | null;
  created_at: string;
  started_at: string | null;
  completed_at: string | null;
  /** Versioned SceneFragment snapshot (ADR-008 #5), or null for pre-v4 / no-scene
   * jobs. Annotates `input_content`; the text stays authoritative for geometry. */
  scene_json: string | null;
}

/** Mirrors `src-tauri/src/models/molecule.rs::Molecule`. */
export interface Molecule {
  id: string;
  name: string;
  formula: string;
  xyz: string;
  charge: number;
  multiplicity: number;
  tags: string;
  created_at: string;
}

export type SidecarState = "healthy" | "starting" | "stale" | "down";

export interface SidecarStatus {
  status: SidecarState;
  port: number | null;
  /** The sidecar's reported version once /health answered (2.5.2d-1). */
  version?: string | null;
  /** The minimum version this app build expects. */
  expected_version?: string;
}
