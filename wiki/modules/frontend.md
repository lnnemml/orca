# Module: Frontend (src/)

**Status:** Phase 0 scaffold done — Vite + React + TS (strict), single status dashboard.

## As built (Phase 0)
- Scaffolded via `create-tauri-app` (react-ts template). Note: the current template ships
  **React 19**, not React 18 as ADR-001 states — see the 2026-07-26 log entry; ADR-001 to be
  updated/superseded.
- `src/App.tsx` is a **System Status** dashboard (inline styles, no UI library yet):
  - Sidecar row: colored dot (green/amber/red) + label, via `invoke('get_sidecar_status')`,
    polled every 5s (no Tauri events yet).
  - ORCA path row: editable field + Save button via `invoke('get_settings')` /
    `invoke('set_setting', {key, value})`; shows Configured / Unsaved-change state.
- Zustand not yet introduced (single component, no shared state to justify it).

## Responsibilities
Rendering, editing, user interaction. No filesystem/process access — everything through
Tauri commands and events.

## Screens (planned)
- **Jobs** — list + statuses, "Run on:" backend selector, live console per job
- **Editor** — Monaco with ORCA grammar, template picker, hover help (Phase 4)
- **Molecules** — library, 3Dmol.js viewer, SMILES/xyz import
- **Results** — summary card, trajectory player, MO viewer, spectra
- **Manual** — FTS search panel (Phase 4)
- **Settings** — ORCA path, server profiles, API key

## State
Zustand stores: `jobsStore` (job list + live statuses via Tauri events),
`editorStore`, `settingsStore`. Server state (SQLite reads) via simple invoke wrappers;
don't over-engineer with react-query until pain appears.

## Conventions / quirks
- 3Dmol.js instances must be explicitly destroyed on unmount (WebGL context leaks).
- Log console: virtualized list (large outputs), append via event batching (100ms flush).
