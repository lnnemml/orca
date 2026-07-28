# Module: Frontend (src/)

**Status:** Phase 2 **complete** — step 2.6 extends file import to pdb/cif/mol/sdf/gen (shared
`import-file.ts` → sidecar `/convert`); step 2.5 adds the live convergence dashboard on Job detail
(energy + criteria per cycle, recharts); step 2.4 the ORCA input builder form (dropdowns → `.inp`,
still editable in Monaco); step 2.3 the molecule library (Molecules screen + Save/Use ↔ New Job);
step 2.2 .xyz import + SMILES→3D on New Job; step 2.1 the 3Dmol.js molecule preview. Phase 1
complete (step 4): output backfill, energy/time in the job list, Open Folder; step 3 (live log
console) and step 2 (editor + templates).

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

## As built (Phase 2.3) — molecule library + Molecules screen
A molecule becomes a persistent object: save it, browse it, and reuse it in a future job.
New screen + New Job integration; still `useState` + `invoke` only (no Zustand).

- **Shared xyz helpers (`viewer/xyz-format.ts`, new):** `xyzToAtomLines` (moved out of
  `NewJobScreen`), `atomLinesToXyz` (build standard xyz from ORCA coord lines), and
  `parseChargeMult` (read `* xyz charge mult` header, default `0/1`). Reused by both screens.
- **`screens/MoleculesScreen.tsx` (new):** header + "Add Molecule" toggle; `list_molecules` on
  mount into a `.jobs-table` (Name / Formula / Charge / Tags / Created / Use+Delete). Row click
  toggles a detail panel below the table: a `.viewer-panel` `MoleculeViewer` of the stored xyz +
  an info line (formula · charge · mult · tags). Empty state points users at the New Job import.
  **Add form** (inline `AddMoleculeForm`): Name + Charge/Multiplicity number inputs; the same
  Import .xyz / SMILES→Generate 3D row as New Job (`get_sidecar_status` → `fetch /smiles-to-3d`);
  Tags input; a live `MoleculeViewer` preview; Save → `create_molecule`. Delete → `delete_molecule`
  then reload; Use → `onUseMolecule(mol)` bubbles to `App`.
- **`App.tsx`:** new "Molecules" tab between Jobs and Settings; `Screen` union gains
  `{kind:"molecules"}` and `new-job` gains an optional `initialMolecule`. `onUseMolecule` switches
  to New Job carrying the molecule.
- **`NewJobScreen.tsx`:** accepts `initialMolecule?` — on mount injects its xyz (with the
  molecule's charge/mult) into the editor and seeds the title. New **"Save to Library"** button in
  the import row: `extractXyzFromInput` + `parseChargeMult` → `create_molecule` (formula carried
  from the last SMILES generation / the used molecule, empty for a bare `.xyz`), then a
  "Saved to library" success banner. Local `xyzToAtomLines` replaced by the shared import.
- **Types (`types.ts`):** `Molecule` mirrors the Rust struct.
- **CSS:** reuses `.jobs-table` / `.card` / `.viewer-panel`; added `.molecule-form` /
  `.molecule-detail` spacing and a `flex:none` override so the detail/form viewer panels honour
  their explicit inline height (they aren't inside the New Job flex split).
- **Verified** in Chromium (`vite dev`, real sidecar on :8765, `window.__TAURI_INTERNALS__.invoke`
  stubbed with an in-memory molecule store — same GUI-can't-be-driven-headlessly limitation as
  prior phases): Molecules empty state → Add → SMILES `CCO` → Generate 3D renders ethanol → Save →
  row appears; row click → detail viewer; Use → New Job with `* xyz 0 1` block + preview; Save to
  Library → banner + second row (formula `C2H6O` carried through); Delete removes it. `tsc` +
  `vite build` clean, no console errors. Rust CRUD is `cargo test`-covered (24 green).

## As built (Phase 2.4) — ORCA input builder form
A form-constructor that turns method/basis/solvation dropdowns into a valid `.inp`, so the user
doesn't have to memorise ORCA syntax. New `src/input-builder/` module; still `useState` only.

- **`orca-options.ts`** (data only, no React): keyword catalogs — `JOB_TYPES`,
  `COMPOSITE_METHODS`, `FUNCTIONAL_GROUPS` (optgroup'd by rung), `BASIS_SETS`, `DISPERSION`,
  `RI_METHODS`, `SOLVATION_MODELS`, `SOLVENTS` (curated 20), `SCF_CONV`. `OrcaOption =
  {keyword, label, description?}`; empty `keyword` = "no keyword". Keywords checked vs ORCA 6.1.
- **`build-input.ts`** (pure, no React — unit-tested): `buildKeywordLine(state)` and
  `buildOrcaInput(state, atomBlock)`. Encodes the domain rules (see `orca/input-format.md`):
  composite methods emit ONLY their name (no basis/dispersion/RI); RI auto-adds the aux basis
  (`def2/J` for RIJCOSX/RI-J, `def2/JK` for RI-JK) when a `def2` basis is used; canonical order
  `method basis auxbasis RI dispersion solvation jobtype scfconv`; `%maxcore` no `end`, `%pal`
  with `end`; solvation `MODEL(solvent)`; the passed atom rows are preserved verbatim (form's
  charge/mult drive the header), placeholder comment when none. `DEFAULT_BUILDER_STATE`:
  r2SCAN-3c composite, Opt+Freq, TightSCF, gas phase, 0/1, nprocs 4, maxcore 2000.
- **`build-input.test.ts`** (vitest, 9 tests): composite self-sufficiency, RIJCOSX→def2/J,
  RI-JK→def2/JK, CPCM(water), gas-phase omits solvation, `%maxcore`/`%pal` end rules, verbatim
  coordinate preservation, form charge/mult in the header. **vitest added** (`npm test` =
  `vitest run`); pure-function tests, default node env, no config file needed.
- **`InputBuilderForm.tsx`**: the collapsible form. Seeds charge/mult from the current geometry
  via `parseChargeMult` (reads only the `* xyz c m` header — the `!` line is never parsed back;
  form→text stays one-way per ROADMAP). Composite-vs-functional radio toggle disables the
  basis/dispersion/RI selects in composite mode; solvent select disabled in gas phase. **Live
  preview** of the `!` line (`buildKeywordLine`) under the form — the learning element: the user
  sees the syntax their choices produce before Generate. "Generate Input" extracts the editor's
  atoms (`extractXyzFromInput` + `xyzToAtomLines`) and calls `onGenerate(buildOrcaInput(...))`.
- **`NewJobScreen`**: collapsible `.input-builder` panel (▸/▾, default collapsed) above the
  template picker; `onGenerate` → `setContent`, so the viewer refreshes via the existing debounce.
- **CSS:** `.input-builder`/`.builder-toggle`/`.builder-body`/`.builder-row`/`.radio-row`/
  `.builder-preview` (dark monospace `!`-line box) + a `.select` cursor rule; reuses `.field`/
  `.input`/`.label`.
- **Verified** in Chromium (`vite dev`, real sidecar, invoke stubbed): builder default preview
  `! r2SCAN-3c Opt Freq TightSCF`; Functional mode →
  `! B3LYP def2-TZVP def2/J RIJCOSX D4 Opt Freq TightSCF` (auto def2/J); CPCM →
  `…D4 CPCM(water) Opt…` and the solvent select flips from disabled→enabled; SMILES `CCO` →
  Generate 3D → **Generate Input** yields a full `.inp` with the new `!` line, `%pal…end` /
  `%maxcore` (no end), and the 9 ethanol atoms preserved verbatim (read back via the stubbed
  `create_job`). `tsc` + `npm test` (9) + `vite build` clean, no console errors. The real
  ORCA-run leg of the checklist needs the Tauri backend + an ORCA binary — not drivable
  headlessly (same limitation as prior phases); the generator's exact output is byte-covered by
  vitest.

## As built (Phase 2.5) — New Job UI fixes (WebKitGTK contrast, accordion, scroll)
Three issues found by manual testing in the real Tauri window (not visible in Chromium). CSS +
a collapse wrapper only — no Input Builder / `build-input.ts` / `orca-options.ts` logic changed.

- **`<select>` dark-on-dark (WebKitGTK).** Native GTK `<select>` widgets ignore the inherited
  `.input` color, so dropdown values rendered near-black on `--panel`. Fix in `styles/app.css`:
  `.select` now sets `-webkit-appearance:none` + explicit `color`/`background-color` + a custom
  inline-SVG chevron (native arrow is gone under `appearance:none`); `.select:disabled` and
  `.input[type=number]` get explicit dark colours too. The `option` popup is a native GTK menu —
  `.select option` styling is best-effort. Full write-up + MiniBrowser verification in
  `debugging/003-webkitgtk-select-styling.md`.
- **Couldn't scroll to the editor.** `.screen.new-job` was `overflow: hidden` with the editor on
  `flex: 1`, so an expanded builder/templates squeezed Monaco out with no way to reach it. Now
  `.screen.new-job` is `overflow-y: auto` (scrolls as a normal column) and `.editor-viewer-split`
  is `flex: none; height: 420px` (fixed usable height instead of competing for flex space).
- **Templates always on screen → accordion.** Templates is now a collapsible `.input-builder`
  section like the Input Builder (same toggle/caret markup; its body `.template-groups` gets a
  `border-top` + padding via `.input-builder > .template-groups`). `NewJobScreen` replaced the
  `builderOpen` boolean with one `openSection: "builder" | "templates" | null` (default `null` —
  both closed, editor + viewer visible on open). The two sections are **mutually exclusive**
  (opening one closes the other); picking a template or hitting **Generate Input** collapses the
  accordion (`setOpenSection(null)`) since the user now wants the editor. Builder's `onGenerate`
  is wrapped by `handleGenerate` to collapse after generating.
- **Verified:** `tsc` + `npm test` (10) + `vite build` clean. The `<select>`/number-input contrast
  fix confirmed in `webkit2gtk-4.1 MiniBrowser` (Tauri's engine, where the bug reproduces) via a
  probe HTML + `gnome-screenshot`: values render light-on-dark, disabled select muted-dark, chevron
  present, number inputs readable. The open-popup `option` styling and the live accordion/scroll
  interactions need the real Tauri window (GUI not drivable headlessly — same limitation as prior
  phases); the accordion is plain React state and the layout is pure CSS.

## As built (Phase 2.5) — live convergence dashboard
A learning instrument on Job detail: watch energy per cycle and the convergence criteria live,
instead of reading the text log. New `src/convergence/` module; still `useState` + `invoke` only.

- **`types.ts`** — mirrors `src-tauri/src/convergence.rs`: `ScfPoint` / `Criterion` / `OptPoint`,
  the `ConvergenceEvent` union (discriminated on `kind: "scf" | "opt"`), and `ConvergencePayload`.
- **`ConvergenceDashboard.tsx`** — props `events` + `status`. Three sections, each rendered only
  when it has data:
  - **A. Progress indicator** — real state, not a fake %: for an Opt, `Optimization cycle N ·
    M/T criteria met` + a bar + a chip per criterion (✓/✗); for a single point, `SCF iteration K`;
    for Freq, nothing (no opt data — a Freq run's SCF still shows as SCF iterations, which is fine).
  - **B. Energy per cycle** (recharts `LineChart`, ≥2 points) — Y auto-domain, ticks 6 dp
    (differences are ~1e-5 Eh on a total of hundreds); tooltip adds ΔE-from-previous in kcal/mol
    (×627.509) for legibility.
  - **C. Criteria vs tolerance** (recharts, **log Y**) — a line per gradient/step criterion with a
    dashed `ReferenceLine` at its tolerance (same colour); `Energy change` excluded (signed,
    different scale — it lives in the progress indicator). Values are magnitudes, safe for log.
- **WebKitGTK / recharts** — recharts' `ResponsiveContainer` measures **0×0** in WebKitGTK (same
  class of bug as 3Dmol's OffscreenCanvas and the `<select>` styling — the webview mismeasures).
  Mitigated proactively: a `useContainerWidth` ResizeObserver hook measures the panel and passes an
  **explicit pixel `width`** (+ fixed `height`) to each `LineChart`. No `ResponsiveContainer`. SVG
  rendering itself is fine (lower risk than the WebGL/native-widget cases).
- **`JobDetailScreen` integration** — a `job:convergence` listener is attached **before** submit
  (listeners-first, same rule as `job:log`/`job:status` — Phase 1.3), and `read_job_convergence`
  backfills after listeners attach, seeding only if live events haven't arrived (same order/guard
  as the log backfill → no dedup needed). The dashboard sits in a collapsible accordion (the
  `.input-builder` toggle pattern) **above** the log console: expanded by default while the job is
  active (or auto-running), collapsed once finished.
- **CSS** — `.convergence-dashboard` + `.conv-*` classes in `app.css`; the detail accordion body is
  `max-height: 58vh; overflow-y: auto` so a tall dashboard never crushes the log console below it.
- **Verified** — `tsc` + `npm test` (10) + `vite build` clean. The Rust parser (the risky part) is
  `cargo test`-covered (7 unit tests over a real-output fixture) and validated against the two real
  full outputs on the dev machine (`real_full_outputs_parse_sanely`, ignored): 4 and 7 opt cycles
  parsed, **zero** Freq-eigenvector false positives. The live in-GUI rendering (charts updating
  mid-run, backfill on reopen) needs the real Tauri window — not headless-drivable, same limitation
  as every prior phase; recharts output is SVG and the data path is fully typed + tested.

## As built (Phase 2.6) — multi-format structure import
The "Import .xyz" button on **both** New Job and Molecules is now **"Import file"** and accepts
`.xyz,.pdb,.cif,.mol,.sdf,.gen`. Logic extracted to a shared `src/viewer/import-file.ts`
(`importStructureFile(file)` + `IMPORT_ACCEPT`) so the two screens don't duplicate it:
- `.xyz` → parsed locally (`xyzToAtomLines`), no round-trip.
- other formats → `from_format` from the extension, `fetch` the sidecar `/convert`
  (`to_format: "xyz"`, port via `get_sidecar_status`), then the same `xyzToAtomLines` path.
- throws `Error(message)` on any failure; each screen shows it in its existing banner. On error
  the editor/draft is left untouched (the coordinate block is only replaced on success).

Returns ORCA coordinate lines; New Job injects them with `injectXyzIntoInput(…, 0, 1)`, Molecules
builds a standard xyz via `atomLinesToXyz`. No new screen. Export to other formats is Phase 3.

## Status-bar queue control (fix)
The bottom status bar's Queue indicator + Pause/Resume button (`App.tsx` footer):
- **The Pause/Resume button always renders** (previously only when `running > 0 || queued > 0`).
  You could not pause *ahead* of stacking jobs — the primary "queue work and walk away" flow — and
  worse, the `paused` state became invisible once the queue drained, so the next job silently
  didn't start.
- **Adaptive label** (`queueLabel`): activity → `1 running, 2 waiting`; empty + not paused →
  `idle`; empty + paused → `paused`. The paused suffix/state shows even with an empty queue.
- **`.queue-paused`** class (`var(--warn)`, `app.css`) highlights the indicator whenever paused so
  the state can't be missed. Button `title`s explain the semantics ("the running job finishes, but
  no queued job starts").
- **Optimistic toggle:** `togglePause` flips `queue.paused` locally right after the successful
  `invoke`, so the label changes immediately instead of after the `refreshQueue` round-trip
  (which still runs in `finally` to reconcile counts).
- **Reason on the job row:** `App` passes `queuePaused` to `JobsScreen`; a `queued` job's badge
  reads **`queued (paused)`** with a "Queue is paused" tooltip while paused — so a job that isn't
  moving shows why. Backend (`pause_queue`/`resume_queue`/`is_queue_paused`) untouched.

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
