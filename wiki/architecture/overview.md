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
│  · cclib: full output parsing → structured JSON          │
│  · RDKit: SMILES → 3D (ETKDG), format conversions        │
│  · Manual indexer: HTML docs → sections → SQLite FTS5    │
└──────────────────────────────────────────────────────────┘
```

Division of responsibility, one line each:
- **Frontend** renders and edits; it never touches the filesystem or processes directly.
- **Rust core** owns *where and how things run* and *what is stored*.
- **Sidecar** owns *what things mean chemically*.

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
4. On completion → results synced locally → Rust calls sidecar `/parse` → cclib JSON
   → stored in SQLite `results` table
5. Results screen reads from SQLite; heavy artifacts (cubes) are generated lazily on demand
   via `orca_plot` and cached in the job dir

## Storage layout

- **SQLite** `orcastudio.db`: `projects`, `molecules`, `jobs`, `results`, `server_profiles`,
  `manual_sections` (+FTS5), `settings`. Schema versioned with simple integer migrations.
- **Filesystem** `<data>/jobs/<job_id>/`: input, output, xyz/hess/gbw, cubes. The DB stores
  paths + parsed data, never large blobs.

## Security / privacy posture

Local-first: no telemetry, no network calls except (a) user-configured SSH servers,
(b) optional Anthropic API "explain" feature with the user's own key. SSH credentials are
never stored by the app — it delegates entirely to the user's `~/.ssh` setup.
