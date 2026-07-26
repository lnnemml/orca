//! Shared TypeScript types mirroring the Rust IPC surface.

export type JobStatus = "draft" | "running" | "completed" | "failed";

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
}

export type SidecarState = "healthy" | "starting" | "down";

export interface SidecarStatus {
  status: SidecarState;
  port: number | null;
}
