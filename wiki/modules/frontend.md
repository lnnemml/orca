# Module: Frontend (src/)

**Status:** Phase 2 **in progress** — step 2.2 adds .xyz import + SMILES→3D on New Job; step 2.1
added the 3Dmol.js molecule preview. Phase 1 complete (step 4): output backfill, energy/time in
the job list, Open Folder; on step 3 (live log console) and step 2 (editor + templates).

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

## As built (Phase 1 step 3) — run + live log console
- **`App.tsx`** screen state became a union so it can hold a selected job:
  `{kind:"job-detail", jobId, autoRun}` alongside the three tabs (Jobs stays highlighted while
  drilled in).
- **`NewJobScreen`** now has two buttons: "Create Job" (draft → Jobs) and "Create & Run"
  (create → open detail with `autoRun`). It does NOT submit itself — the detail screen submits
  after attaching listeners (see below).
- **`JobsScreen`** rows are clickable (→ detail); an actions column shows Run (draft),
  "Running…" (running), or Open. `onOpenDetail(jobId, autoRun)` from the parent.
- **`JobDetailScreen`** (new): loads the job, `listen`s to `job:log` (append, capped at 5000
  lines, auto-scroll) and `job:status` (update badge; reload full record on terminal state to
  surface `error_message`/`completed_at`). Terminal-style `<pre className="log-console">`.
  - **Ordering rule:** it attaches the log/status listeners FIRST, then (if `autoRun`) calls
    `submit_job` — so no early output lines are missed. A `didSubmit` ref guards against React
    StrictMode's dev double-mount firing two submits (the backend slot-mutex is the real guard).
## As built (Phase 1 step 4) — backfill + results + open folder
- **`format.ts`** (new): shared `formatEnergy` (6 dp, `—` when null), `formatWallTime`
  (`35.4s` / `2m 15s` / `1h 5m`), `formatTimestamp`. Used by Jobs list + Job detail.
- **`JobDetailScreen` backfill:** after attaching listeners, for a non-draft job it calls
  `read_job_output(id)` and seeds the console from `output.out` — so opening a finished job shows
  the full log instead of "Waiting…". Guarded with `setLines(prev => prev.length ? prev : existing)`
  so live events that already arrived win. For a *running* job a small duplicate window is
  possible (backfill + overlapping live lines) — accepted for Phase 1. Also shows an
  `energy … Eh · time …` line once results exist, and an **Open Folder** button
  (`open_job_folder`) when `job_dir` is set.
- **`JobsScreen` columns:** added Energy (Eh) and Time, right-aligned, via the shared
  formatters. Row click and the per-row Run/Open buttons open the detail screen (Open opens the
  in-app detail, not a file manager — the file manager is the detail's Open Folder button).

## As built (Phase 2.1) — molecule preview on New Job
New `src/viewer/` module (details + design decisions in `modules/visualization.md`):
- **`MoleculeViewer.tsx`** — 3Dmol.js ball-and-stick viewer; props `xyzData` + optional `style`.
  Fills its parent, `ResizeObserver` → `viewer.resize()`, `viewer.clear()` on unmount to release
  the WebGL context.
- **`parse-xyz-from-input.ts`** — `extractXyzFromInput` pulls the `* xyz … *` block from the
  editor content into standard xyz; `null` for `xyzfile` / no coordinates.
- **`3dmol-setup.ts`** — side-effect module that neutralises `OffscreenCanvas` so 3Dmol renders
  in the WebKitGTK webview (see `debugging/002`). Same side-effect-import pattern as
  `editor/monaco-setup.ts`.
- **`NewJobScreen`** — added a split panel right of the editor (`.editor-viewer-split`). Editor
  content is parsed on a **500 ms debounce** (`useEffect` + `setTimeout`) into `previewXyz`; shows
  the molecule or a muted "No coordinates in input" placeholder. `useState` only — no Zustand yet.
- **CSS (`styles/app.css`)** — `.editor-viewer-split` / `.viewer-panel` / `.molecule-viewer` /
  `.viewer-empty`; the panel reuses the panel border + `var(--radius)` and the `#0d0f13` console
  background. `.molecule-viewer` is `position: relative` (3Dmol appends an absolute canvas).
- **Verified**: `tsc` + `vite build` clean; Chromium (`vite dev`) — picking a template renders
  H₂, clearing the editor falls back to "No coordinates", no console errors; H₂ render confirmed
  in `webkit2gtk-4.1 MiniBrowser` (Tauri's engine) with the OffscreenCanvas fix; the real Tauri
  window renders the split layout (molecule-in-Tauri-GUI verified via the identical-engine
  MiniBrowser, since the GUI can't be driven headlessly — same limitation as Phase 0/1).
- **Bundle note**: `3dmol` adds ~4 MB to the JS bundle (on top of Monaco). Acceptable for a local
  desktop app; a future optimisation is code-splitting the viewer.

## As built (Phase 2.2) — molecule import (.xyz + SMILES → 3D)
Two ways to load a molecule into the editor; both feed the Phase 2.1 preview automatically (edit
→ 500 ms debounce → `extractXyzFromInput` → viewer). Added to `NewJobScreen`, no new screen.
- **`src/viewer/inject-xyz-into-input.ts`** — `injectXyzIntoInput(content, atomLines, charge,
  mult)`: finds the existing `* xyz|xyzfile … *` block (same marker `extractXyzFromInput` uses)
  and **replaces** it, or **appends** a fresh block (blank-line-separated) when none exists.
  Everything outside the block (`!` line, `%` blocks) is preserved.
- **Import row** (between title and template picker, one line — `[Import .xyz] or [SMILES] [Generate
  3D]`): a hidden `<input type="file" accept=".xyz,.XYZ">` triggered by a button (no
  `tauri-plugin-dialog` — the HTML input suffices); `FileReader.readAsText` → local
  `xyzToAtomLines` (validates first line = positive atom count, skips count+comment, keeps
  `element x y z` rows) → `injectXyzIntoInput(…, 0, 1)`; sets the title from the filename if empty.
  The input's `value` is reset after each pick so the same file can be re-imported.
- **SMILES flow:** `get_sidecar_status` (invoke) → `fetch http://127.0.0.1:{port}/smiles-to-3d` →
  `xyzToAtomLines(resp.xyz)` + `resp.charge` → `injectXyzIntoInput`. On non-OK response the
  `detail` field is surfaced in the error banner; `Generating…` disables the button; title seeded
  from `resp.formula` if empty. Enter in the field also triggers it. Still `useState` only — no
  Zustand.
- **CSS:** `.import-row` (flex, one line), `.import-smiles` (`flex:1`, monospace), `.import-or`
  (muted).
- **Verified** in Chromium (`vite dev`): `.xyz` import (methane) appends a block and renders CH₄;
  `CCO` → ethanol (9 atoms), replacing the prior block; `xxx` → "Invalid SMILES" banner, editor
  unchanged; `[O-]` → `* xyz -1 1`. `tsc`/`vite build` clean, no console errors. The SMILES happy
  path was exercised against the real sidecar by stubbing only `window.__TAURI_INTERNALS__.invoke`
  to return the running port (plain-browser `invoke` is otherwise unavailable — same GUI-driving
  limitation as earlier phases); the endpoint itself is `pytest` + `curl` covered.

## Resolved from step 3
The earlier "no backfill of `output.out`" gap is closed by `read_job_output` + the detail
backfill above.

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
