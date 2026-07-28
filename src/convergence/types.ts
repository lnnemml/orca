//! Mirrors `src-tauri/src/convergence.rs` — the `job:convergence` event payload
//! and the internally-tagged `ConvergenceEvent` enum (`#[serde(tag = "kind")]`).

/** One SCF iteration. `cycle` is the opt cycle (0 for a single point). */
export interface ScfPoint {
  kind: "scf";
  cycle: number;
  iter: number;
  energy: number;
  delta_e: number;
}

/** One geometry-convergence criterion (value vs tolerance, met or not). */
export interface Criterion {
  name: string;
  value: number;
  tolerance: number;
  converged: boolean;
}

/** One geometry optimization step: its cycle energy + convergence criteria. */
export interface OptPoint {
  kind: "opt";
  cycle: number;
  energy: number | null;
  criteria: Criterion[];
}

export type ConvergenceEvent = ScfPoint | OptPoint;

/** Payload of the `job:convergence` Tauri event. */
export interface ConvergencePayload {
  job_id: string;
  events: ConvergenceEvent[];
}
