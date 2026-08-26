# Architecture Overview

## Three layers

```
┌──────────────────────────────────────────────────────────┐
│ FRONTEND — React 18 + TypeScript (strict), Vite          │
│  Screens: Jobs · Editor · Molecules · Results · Manual   │
│  Libs: Monaco (input editor), 3Dmol.js (structures,      │
│  cubes), recharts (convergence, spectra), Zustand (state)│
└───────────────▲──────────────────────────────────────────┘
                │ Tauri commands (invoke) + events (emit)
┌───────────────┴──────────────────────────────────────────┐
│ RUST CORE — src-tauri/                                   │
│  · Job state machine + SQLite (rusqlite)                 │
│  · ExecutionBackend trait → LocalBackend | SshBackend    │
│  · Process spawn, log tailing (notify), byte-offset      │
│    polling for remote logs                               │
│  · Sidecar lifecycle (spawn on start, kill on exit)      │
└───────────────▲──────────────────────────────────────────┘
                │ HTTP, localhost:8765
┌───────────────┴──────────────────────────────────────────┐
│ PYTHON SIDECAR — sidecar/ (FastAPI)                      │
│  · RDKit: SMILES → 3D (ETKDG)                            │
│  · ASE: geometry kernel (d/θ/φ, masks) + format convert  │
│  · Manual indexer: HTML docs → sections → SQLite FTS5    │
│    (planned, Phase 4)                                     │
└──────────────────────────────────────────────────────────┘
```

Result parsing is **not** in the sidecar. cclib was rejected (crashes on ORCA 6.1.0); the
authoritative tier is own **Rust** parsers over ORCA's structured artifacts (ADR-012), next to the
streaming convergence parser. See `wiki/modules/artifact-readers.md`.

Division of responsibility, one line each:
- **Frontend** renders and edits; it never touches the filesystem or processes directly.
- **Rust core** owns *where and how things run*, *what is stored*, and *parsing results* (ADR-012).
- **Sidecar** owns *in-process chemistry over file content* — structure generation (RDKit) and the
  geometry kernel + conversion (ASE); it never spawns a process (ADR-009) and does not parse results.

## Job lifecycle (the central state machine)

```
draft ─▶ queued ─▶ uploading ─▶ running ─▶ completed ─▶ syncing ─▶ parsed
            │           │           │           │
            │           │           ▼           ▼
            └───────────┴──▶ failed ◀── cancelled
```

- `queued` is the sequential-queue waiting state (LocalBackend, Phase 2): submitting moves a
  draft job here; a single worker slot pulls the oldest queued job when free (concurrency = 1).
  `cancelled` reaches a queued job directly (dropped before it starts) or a running one
  (process group killed).
- `uploading`/`syncing` are no-ops for LocalBackend (instant transitions).
- Completion is detected by **two** signals: `.exit_code` marker file exists AND
  `ORCA TERMINATED NORMALLY` appears in output. Marker without the string = crashed run.
- **Reconciliation on startup:** for every job in `running`/`uploading`/`syncing`,
  the responsible backend re-checks reality (markers, file presence) and advances or
  fails the state. This is what makes the app disconnect-proof and restart-proof.

## Data flow for one calculation

1. User builds input (form or Monaco) → Rust writes an isolated job dir:
   `<data>/jobs/<job_id>/input.inp`
2. Backend `submit()` — local spawn or rsync+ssh nohup (see ADR-003, ADR-005)
3. Log lines stream to frontend as Tauri events → live console + incremental convergence
   parsing (lightweight regex in Rust for `SCF ITERATIONS` / `Geometry convergence` blocks)
4. On completion → results synced locally → **Rust parses the structured artifacts directly**
   (`.property.txt`/`.hess`/`_trj.xyz` + `orca_2json` over `.gbw`; ADR-012, no sidecar, no cclib)
   → stored in SQLite `results` table → job advances to `parsed`
5. Results screen reads from SQLite; heavy artifacts (orbital cubes) are generated lazily on demand
   via `orca_plot` and cached in the job dir (never in the DB)

## Storage layout

- **SQLite** `orcastudio.db`: `projects`, `molecules`, `jobs`, `results`, `server_profiles`,
  `manual_sections` (+FTS5), `settings`. Schema versioned with simple integer migrations.
- **Filesystem** `<data>/jobs/<job_id>/`: input, output, xyz/hess/gbw, cubes. The DB stores
  paths + parsed data, never large blobs.

## Security / privacy posture

Local-first: no telemetry, no network calls except (a) user-configured SSH servers,
(b) optional Anthropic API "explain" feature with the user's own key. SSH credentials are
never stored by the app — it delegates entirely to the user's `~/.ssh` setup. For (b), the
same "give the secret to the OS" posture applies: the Anthropic key lives in the **system
keyring**, not in `orcastudio.db`, and the network call is made by **Rust** so the key never
enters the webview — see [ADR-015](adr-015-api-key-storage.md).

## Computational boundary — ground-state, single-reference thermal mechanisms

The tool's scientific scope is **ground-state, single-reference, thermal reaction mechanisms**: a
molecule sits in one electronic state on one adiabatic potential-energy surface, and a reaction is a
path over that surface between stationary points (minima + first-order saddles). **Every** current
guard, parser, and barrier definition assumes exactly this — the located-TS ΔE‡/ΔG‡ math (one TS, one
surface), the convergence/stationarity verdict, the RRHO thermochemistry, the imaginary-frequency
count (0 = minimum, 1 = TS), the method-comparability guard (one functional/basis/solvation scale).

The following are **out of scope** because they need a **different definition of a surface, a barrier,
or a stationary point** than that assumption — not merely a new keyword:

- **Excited-state chemistry** — reactions on an *excited* surface (photochemistry): a different surface
  per state, and "the barrier" is state-specific.
- **Multireference methods** (CASSCF / CASPT2 / NEVPT2 / DMRG) — where a single Slater determinant /
  KS reference is qualitatively wrong (bond-breaking, diradicals, near-degeneracies); the very notion
  of "the" reference the comparability guard rests on dissolves.
- **Spin-crossing / non-adiabatic** (MECP, ISC) — the reactive event is a **crossing between two
  surfaces of different spin**, not a saddle on one; there is no single-surface TS to locate, and the
  RRHO/imaginary-mode guards do not apply.

**The one realistic future bridge** is **TD-DFT UV-Vis** — *vertical* excitations computed **on a
ground-state geometry** (already noted in Phase 6). That reads excited-state *energies at a fixed
ground-state structure*; it does **not** optimize, locate a TS on, or run thermochemistry over an
excited surface, so it stays inside the ground-state-geometry assumption. It is a spectroscopy readout,
not excited-state *chemistry*. Anything beyond it (excited-state optimization, MECP, multireference) is
a separate tool, not a feature of this one.

Consequence for the UI (honest-or-absent): where a genre is genre-agnostic about *where a number came
from* — e.g. the future publication energy-level diagram (ROADMAP Phase 6 Stage 6.x) accepting
hand-entered ISC / excited-state levels — such values are **user-entered, labeled as such**, never
implied to be pipeline-computed.
