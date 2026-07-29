//! Shared TypeScript types mirroring the Rust IPC surface.

export type JobStatus =
  | "draft"
  | "queued"
  | "running"
  | "completed"
  | "failed"
  | "cancelled";

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
