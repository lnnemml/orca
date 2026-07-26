# Module: Frontend (src/)

**Status:** Phase 1 step 2 done — Monaco `.inp` editor + template library + job-creation
UI with tabbed navigation (New Job / Jobs / Settings). Builds on the Phase 0 scaffold.

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

## As built (Phase 1 step 2) — editor + templates + job UI
Layout: `App.tsx` is now a shell (topbar tabs + bottom status bar) that renders one of three
screens by local `useState` (no router yet). Styling moved out of inline styles into
`src/styles/app.css` — monochrome dark palette + one accent (`--accent`), monospace in
editor/inputs. Only the active screen is mounted (switching to Jobs remounts it → fresh
`list_jobs`); a side effect is that an in-progress New Job draft is discarded on tab switch
(acceptable for now).

- **Screens:**
  - `screens/NewJobScreen.tsx` — job title input, template picker (grid grouped by category),
    Monaco editor. "Create Job" → `invoke('create_job', { title, inputContent })` → on success
    navigates to Jobs. Picking a template fills the editor and, if title is empty, seeds it.
  - `screens/JobsScreen.tsx` — `invoke('list_jobs')` on mount; table of title / status badge /
    created_at, with a Refresh button. Empty + loading states. (Job detail is a later step.)
  - `screens/SettingsScreen.tsx` — the Phase 0 ORCA-path editor (`get_settings`/`set_setting`),
    relocated here off the main screen.
- **Bottom status bar** (`App.tsx`): sidecar dot (polled 5s via `get_sidecar_status`) + the
  configured ORCA path — always visible, replaces the old System Status dashboard as the home.
- **Editor (`editor/`):**
  - `orca-language.ts` — Monarch grammar for `orca-inp`: highlights the `!` directive line,
    `%block`/`end` keywords, `#` comments, `* xyz ... *` coordinate delimiters, numbers
    (int/float/scientific/signed), quoted strings. `ignoreCase` (ORCA is case-insensitive).
    Deliberately structural, not a full keyword list.
  - `InputEditor.tsx` — `@monaco-editor/react` wrapper (vs-dark, full height), registers the
    language once via `beforeMount`.
  - `monaco-setup.ts` — **critical for offline**: `@monaco-editor/react` defaults to loading
    Monaco from a CDN; we override with `loader.config({ monaco })` (bundled package) + Vite's
    base editor worker. Worker import path is `monaco-editor/editor/editor.worker.js?worker` —
    NOT `esm/vs/...`, because the package `exports` map rewrites `monaco-editor/*` → `esm/vs/*`
    (the `esm/vs/` prefix double-maps and fails to resolve). See `wiki/debugging/`.
- **Templates (`templates/orca-templates.ts`):** 8 hardcoded `OrcaTemplate`s across 4
  categories (SP / Opt / Freq / Opt+Freq × r²SCAN-3c and B3LYP-D4/def2-SVP). Each is a complete
  runnable `.inp` with `%pal nprocs 4 end`, `%maxcore 2000`, and an H2 placeholder geometry.
  **ORCA correctness note:** `%maxcore` is a simple directive, NOT a block — it takes no `end`
  (unlike `%pal`). The task spec said `%maxcore 2000 end`; we emit the correct `%maxcore 2000`.
- **Types (`types.ts`):** `Job` mirrors the Rust `Job` (lowercase `status`); `SidecarStatus`.
- **Verified** in Chrome against `vite dev`: UI renders, picker fills the editor with live ORCA
  highlighting, no console/worker errors (confirms offline Monaco works at runtime). Tauri
  `invoke` paths (`create_job` etc.) exercised via the already-tested Rust commands.
- **Bundle note:** importing the full `monaco-editor` pulls all built-in languages (~4 MB / ~1 MB
  gz). Fine for a local desktop app; a future optimization is importing only `editor.api`.

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
