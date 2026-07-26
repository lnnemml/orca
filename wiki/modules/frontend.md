# Module: Frontend (src/)

**Status:** not started (Phase 0 scaffold pending)

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
