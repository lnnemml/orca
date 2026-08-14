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
    /** Kelvin. */
    temperature_k: number;
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
  /** Vibrational data from .hess, or null (SP/GOAT have no .hess). */
  frequencies: {
    elements: string[];
    frequencies_cm: number[];
    /** 0 = minimum, 1 = transition state, >1 = neither. */
    imaginary_count: number;
    zero_count: number;
    is_linear: boolean;
    ir_intensity_km_mol: number[];
    n_modes: number;
    normal_modes: number[];
    temperature_k: number | null;
    scale_factor: number | null;
    unknown_sections: string[];
  } | null;
  /** Optimization/scan trajectory from _trj.xyz (frames are opt cycles, not scan
   * points), or null for a single-point job. */
  trajectory: {
    n_frames: number;
    elements: string[];
    frames: { energy_eh: number | null; xyz_angstrom: [number, number, number][] }[];
  } | null;
  /** MO energies/occupancies from orca_2json (no coefficients), or null (xTB/GOAT). */
  orbitals: {
    energy_unit: string;
    orbitals: [number, number][];
    homo_lumo: { homo_eh: number; lumo_eh: number; gap_eh: number } | null;
  } | null;
  /** Relaxed-scan profile from .relaxscanact/.relaxscanscf.dat (Phase 4.5 B1),
   * or null when the job is not a scan. One row per scan POINT (not opt cycle);
   * `act` = composite/actual energy, `scf` = bare SCF (both, never conflated). */
  scan: ScanProfileJson | null;
  /** NEB-TS band + MEP + converged TS (Phase 4.5 E3a-1), or null for a non-NEB job.
   * The per-iteration band VIEWER is E3a-2; here the data is parsed and stored. */
  neb: NebResults | null;
  /** Mayer bond orders from output.out — the COMPUTED, authoritative bond order of
   * the final structure (contrast the viewer's geometric estimate from bond length).
   * null when the run printed no table (xTB / an SP that didn't print it — normal). */
  mayer_bond_orders: MayerBond[] | null;
  unknown_blocks: string[];
}

/** Mirrors `src-tauri/src/parse/mayer.rs::MayerBond`. Indices are 0-based, in the
 * `final_geometry` atom order. `order` is fractional (e.g. a partial TS bond ≈0.68). */
export interface MayerBond {
  i: number;
  j: number;
  order: number;
}

/** Mirrors `src-tauri/src/results.rs::NebResultsJson`. */
export interface NebResults {
  iterations: NebIteration[];
  /** The converged smooth minimum-energy path (relative energies, image 0 = 0). */
  mep: NebImage[];
  /** The final iteration's barrier (Eh) — the converged NEB-TS barrier estimate. */
  final_barrier_eh: number | null;
  /** The converged transition-state geometry (seeds OptTS in E3a-2). */
  ts_geometry: { elements: string[]; xyz_angstrom: [number, number, number][] };
  /** The converged TS energy (Eh) from `_NEB-TS_converged.xyz` — the saddle's energy,
   * used to label the "Saddle (TS)" view in the band viewer (N3). */
  ts_energy_eh: number | null;
}
export interface NebIteration {
  index: number;
  images: NebImage[];
  barrier_eh: number;
  /** 0-based climbing-image index (once climbing is active), else null. */
  climbing_image: number | null;
}
export interface NebImage {
  /** Arc-length distance along the band (Å). */
  distance_angstrom: number;
  /** Eh — ABSOLUTE in `iterations`, RELATIVE (image 0 = 0) in `mep`. */
  energy_eh: number;
}

/** Mirrors `src-tauri/src/results.rs::ScanProfileJson`. */
export interface ScanProfileJson {
  /** "B" (distance), "A" (angle), "D" (dihedral). */
  kind: string;
  /** Scanned atoms, 0-based. */
  atoms: number[];
  /** "Å" for a distance scan (cross-checked), "°" for angle/dihedral. */
  coordinate_unit: string;
  points: { coordinate: number; energy_act_eh: number; energy_scf_eh: number }[];
}

/** Mirrors `src-tauri/src/results.rs::ScanGeometry` — one relaxed-scan point
 * geometry (`input.NNN.xyz`), loaded lazily via `read_scan_geometries`. */
export interface ScanGeometry {
  elements: string[];
  xyz_angstrom: [number, number, number][];
}

/** Mirrors `src-tauri/src/results.rs::ScanSurface2d` — a 2D relaxed-surface scan for
 * the surface viewer (Stage 4b), loaded via `read_scan_surface` (file-gated, NOT
 * `results.scan`). `dat_text` is the raw `input.relaxscanact.dat` (the frontend
 * `parseScanSurface2d` shapes it); `geometries[NNN-1]` is the geometry of `.dat` row NNN. */
export interface ScanSurface2d {
  dat_text: string;
  geometries: ScanGeometry[];
}

/** Mirrors `src-tauri/src/results.rs::NebImageGeometry` — one converged-MEP band
 * image geometry (`input_MEP_trj.xyz` frame), loaded lazily via `read_neb_geometries`.
 * Distinct from {@link NebImage} (a band point: distance+energy, no geometry). */
export interface NebImageGeometry {
  /** 0-based MEP index (im0 = reactant end … imN = product end). */
  index: number;
  /** The frame's `E <Eh>` comment energy, or null. */
  energy_eh: number | null;
  elements: string[];
  xyz_angstrom: [number, number, number][];
}

/** Mirrors `src-tauri/src/cpu_presets.rs::CpuPresetInfo`. */
export interface CpuPresetInfo {
  id: string;
  label: string;
  mask: string;
  nprocs: number;
  description: string;
}

/** Mirrors `src-tauri/src/secrets.rs::KeySource` (ADR-015). The ONLY key info the
 * frontend receives — never the key itself. `last4` is for recognition only. The
 * fallback trigger is a distinct third state (`unavailable`), not "no key". */
export type KeySource =
  | { state: "stored-in-keyring"; last4: string }
  | { state: "absent" }
  | { state: "from-environment"; last4: string }
  | { state: "unavailable"; reason: string };

/** Mirrors `src-tauri/src/anthropic.rs::ModelInfo` — one model this key may use, from the LIVE
 * `/v1/models` (rule #10: options are measured, not hardcoded). No price: the endpoint doesn't
 * return it, and hardcoding one would be the recalled-constant anti-pattern (ADR-014 (1a)). */
export interface ModelInfo {
  id: string;
  display_name: string | null;
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
  /** Serialized operation log (ADR-017 / unit 2b), or null for pre-v11 / no-scene
   * jobs. Co-written with `scene_json`; "New iteration" restores it, cross-checked
   * against the snapshot (the log is rejected loudly on a mismatch). */
  scene_log_json: string | null;
  /** Grouping FK into `pathways` (schema v13, Phase 4.5 C1), or null for a standalone
   * job — the normal state for every job today. Set by `attach_job_to_pathway`, nulled
   * by `detach_job_from_pathway` / a reaction-or-pathway deletion (which never delete
   * the job — the jobs-survive invariant). The reaction UI maps a pathway to its job by
   * matching `Job.pathway_id === Pathway.id`. */
  pathway_id: string | null;
  /** Grouping FK into `groups` (schema v16, Phase 4.7.2, ADR-019), or null = ungrouped
   * (the root / "All jobs" view). Set by `move_job`; nulled when its group is deleted
   * (the command promotes jobs to the deleted group's parent — `ON DELETE SET NULL` is
   * the fallback). Orthogonal to `pathway_id` and the re-opt/reference links. */
  group_id: string | null;
}

/** Mirrors `src-tauri/src/models/group.rs::Group` (schema v16, Phase 4.7.2, ADR-019).
 * One node in the job-organization tree (UI metaphor: a "folder"). Adjacency list:
 * `parent_id` is the parent node, null = a root-level group. Groups are pure metadata —
 * a job keeps its isolated `job_dir` wherever it sits; moving a job is `move_job`
 * (`UPDATE jobs.group_id`), zero filesystem ops. */
export interface Group {
  id: string;
  name: string;
  parent_id: string | null;
  created_at: string;
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
  /** Role flag (schema v12): a user-saved reagent (shown in "My reagents"), not a
   * library molecule. `false` for existing molecules and `create_molecule` saves. */
  is_reagent: boolean;
}

/** Mirrors `src-tauri/src/models/reaction.rs::Reaction` (schema v13, Phase 4.5 C1).
 * The central object of the reaction workstation (ADR-007): a named transformation.
 * A reaction contains one or more pathways; jobs group under a pathway. */
export interface Reaction {
  id: string;
  name: string;
  description: string | null;
  created_at: string;
}

/** Mirrors `src-tauri/src/models/reaction.rs::Pathway`. One approach geometry under
 * a reaction. Lean by design (one source of truth): it does NOT store the scan
 * coordinate/method/profile — those live in the attached job, read by Stage C2.
 * A job carries a nullable `pathway_id` ONLY; its reaction is derived via `pathways`
 * (normalized — no `reaction_id` on jobs). Deleting a reaction/pathway never deletes
 * a job; it nulls the job's `pathway_id`, and the job is standalone again. */
export interface Pathway {
  id: string;
  reaction_id: string;
  label: string;
  created_at: string;
}

/** Mirrors `src-tauri/src/models/reaction.rs::ReferenceJob` (schema v14, Phase 4.5
 * C2b-2a, ADR-018). One reference job of a reaction's summed reactant reference for
 * ABSOLUTE barriers. `final_energy_eh` is read from the authoritative parsed results
 * on demand and is `null` when the job is unparsed / still running / failed — the
 * "honest-or-absent" signal the C2b-2b UI uses to flag which reference job is missing. */
export interface ReferenceJob {
  job_id: string;
  title: string;
  final_energy_eh: number | null;
  /** Parsed Gibbs free energy G (Eh), `null` unless the job ran Freq and its
   * thermochemistry parsed — the honest-or-absent signal for ΔG‡ (Stage E1b). */
  free_energy_g_eh: number | null;
}

/** Mirrors `src-tauri/src/models/reaction.rs::ReferenceEnergy`. A reaction's summed
 * reactant reference: provenance (`jobs`) AND the honest total (`energy_eh`). `energy_eh`
 * is the sum `Σ final_energy_eh` ONLY when the list is non-empty and every job is parsed;
 * otherwise `null` (incomplete or empty) — a partial sum is never presented (ADR-018:
 * a wrong E(ref) would silently poison every absolute barrier). Never cached — computed
 * on demand from live job energies (one source of truth). */
export interface ReferenceEnergy {
  jobs: ReferenceJob[];
  energy_eh: number | null;
  /** Σ Gibbs free energy G (Eh), on the SAME honest-or-absent discipline as `energy_eh`
   * but over `free_energy_g_eh`: non-null ONLY when every reference job ran Freq (has G);
   * a partial ΣG (some references Opt-only) is `null`, never summed (Stage E1b, ADR-018). */
  free_energy_g_eh: number | null;
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
