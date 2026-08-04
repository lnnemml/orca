# Module: Frontend (src/)

**Status:** Phase 2.5 complete — the frontend builds, views, persists and iterates multi-fragment
scenes and drives the whole geometry editor: a Zustand scene store (`src/scene/store.ts`) synced
two-way with Monaco, an Add-Fragment panel + `FragmentList` sidebar, atom picking + d/θ/φ
measurement, edit mode (set distance/angle/dihedral from the viewer, inter- and intra-fragment), a
constraint panel over the input text, and an xTB pre-optimize button. Phase 2 (jobs, editor,
templates, molecule library, input builder, convergence dashboard, output search, multi-format
import) is complete. **This page covers the editor/job frontend**; the Phase-3 **results** frontend
(summary card, trajectory/spectra/orbitals/export) is complete and documented in
[results-ui.md](results-ui.md).

## Responsibilities & boundaries

Rendering, editing, user interaction. **No filesystem or process access** — everything goes through
Tauri commands and events. Deep 3D/geometry mechanics live in `modules/visualization.md`
(`MoleculeViewer`) and `modules/scene.md` (the Scene model + pure functions); this page covers the
screens, the store wiring, and the UI-level quirks.

## App shell & navigation (`App.tsx`)

`App.tsx` is a shell: a topbar with tabs + a bottom status bar, rendering one screen at a time by a
local `Screen` union (**no router**). The union is `{kind:"new-job", initialMolecule?, initialJob?,
keepScene?}` | `{kind:"jobs"}` | `{kind:"molecules"}` | `{kind:"settings"}` |
`{kind:"job-detail", jobId, autoRun}` (Jobs stays highlighted while drilled into a detail). Only the
active screen is mounted — switching to Jobs remounts it (→ a fresh `list_jobs`); a side effect is
that an in-progress New Job draft is discarded on a tab switch (accepted).

- **Bottom status bar:** the sidecar dot (`get_sidecar_status`, polled every 5 s) + the configured
  ORCA path, always visible. It also hosts the **queue control** (below).
- **Styling:** `src/styles/app.css` — a monochrome dark palette + one accent (`--accent`), monospace
  in editor/inputs. Screens reuse `.jobs-table` / `.card` / `.viewer-panel` / `.field` / `.input` /
  `.label` / `.select`.

## Screens

- **`NewJobScreen`** — the geometry + input workbench (detailed under Scene store & geometry editor
  below). Title input; a collapsible **accordion** with mutually-exclusive sections
  (`openSection: "builder" | "templates" | "add" | null`, default `null` → editor + viewer visible).
  Two create buttons: **Create Job** (draft → Jobs) and **Create & Run** (create → open detail with
  `autoRun`). It does **not** submit itself — the detail screen submits after attaching listeners.
  Accepts `initialMolecule?` (inject its xyz + charge/mult, seed the title), `initialJob?` (iterate
  — see below), and `keepScene?` (skip the mount reset, used when applying a conformer). Picking a
  template or hitting **Generate Input** collapses the accordion (`setOpenSection(null)`).
- **`JobsScreen`** — `list_jobs` on mount into a `.jobs-table`: Title / Status badge / Energy (Eh) /
  Time / Created, a Refresh button, empty + loading states. Rows are clickable (→ detail); an
  actions column shows **Run** (draft) / **Running…** (running) / **Open**. Energy/Time via the
  shared formatters. Takes `queuePaused` from `App`: a `queued` job's badge reads **`queued
  (paused)`** with a "Queue is paused" tooltip while paused, so a stalled job shows why.
- **`JobDetailScreen`** — loads the job, then **attaches `job:log` / `job:status` /
  `job:convergence` listeners FIRST, then (if `autoRun`) calls `submit_job`** so no early lines are
  missed (a `didSubmit` ref neutralises React StrictMode's dev double-submit; the backend slot mutex
  is the real guard). `job:log` appends (capped 5000 lines, auto-scroll); `job:status` updates the
  badge and reloads the full record on a terminal state (to surface `error_message`/`completed_at`).
  Also hosts the convergence dashboard, the output console/search, the ensemble panel, **New
  iteration**, and Open Folder — all below.
- **`MoleculesScreen`** — a `.jobs-table` (Name / Formula / Charge / Tags / Created / Use+Delete);
  row-click toggles a detail panel (a `MoleculeViewer` of the stored xyz + an info line). An inline
  `AddMoleculeForm` (Name, Charge/Mult, the shared Import-file / SMILES→Generate 3D row, Tags, a live
  preview, Save → `create_molecule`). Delete → `delete_molecule` + reload; **Use** → `onUseMolecule`
  bubbles to `App`, which opens New Job carrying the molecule. **Deliberately not Scene-backed** — it
  manages library molecules as stored xyz *strings* (the DB format), so it keeps `atomLinesToXyz` +
  `MoleculeViewer xyzData`; migrating it to a Scene would be churn with regression risk and remove no
  duplication.
- **`SettingsScreen`** — the ORCA path editor (`get_settings` / `set_setting`), the CPU-preset
  section, and the xtb path field + **Check** button (`xtb_version`, same pattern as ORCA).

## Editor & templates (`src/editor/`, `src/templates/`)

- **`orca-language.ts`** — a Monarch grammar `orca-inp` highlighting the structural bits: the `!`
  directive line, `%block`/`end`, `#` comments, `* xyz … *` delimiters, numbers
  (int/float/scientific/signed), quoted strings; `ignoreCase` (ORCA is case-insensitive).
  Deliberately structural, not a full keyword list. `InputEditor.tsx` wraps `@monaco-editor/react`
  (vs-dark, full height), registering the language on `beforeMount`.
- **Hover wiring (`orca-hover.ts`, `editor-options.ts`).** `registerOrcaHover` registers the
  keyword→manual hover **provider first** (mandatory), then the "open in drawer" command in a
  `try/catch` (optional — its failure must not vanish the hover), and flips its `registered` guard
  only after the provider registration succeeds. Editor options are pinned in `editor-options.ts`
  (a type-only module, so a wiring test can import it without dragging `monaco-setup`); the
  load-bearing one is **`fixedOverflowWidgets: true`** — without it a hover on the top `!` line
  (line 1) renders inside the editor's `overflow:hidden` guard and is clipped to nothing
  (`debugging/010`). Covered by `orca-hover-wiring.test.ts` (fake monaco, no jsdom): provider
  registered for the same language id `<Editor>` uses, survives the command throwing, flag pinned.
- **`monaco-setup.ts` — critical for offline.** `@monaco-editor/react` defaults to a CDN loader
  (fatal for a desktop app); we pin the bundled package via `loader.config({ monaco })` + Vite's
  base editor worker. The worker import path is **`monaco-editor/editor/editor.worker.js?worker`**,
  **not** `esm/vs/...`: the package `exports` map rewrites `monaco-editor/*` → `esm/vs/*`, so the
  prefixed path double-maps and Rollup can't resolve it (`debugging/001`).
- **`templates/orca-templates.ts`** — 8 hardcoded `OrcaTemplate`s across 4 categories
  (SP / Opt / Freq / Opt+Freq × r²SCAN-3c and B3LYP-D4/def2-SVP), each a complete runnable `.inp`
  with `%pal nprocs 4 end`, `%maxcore 2000`, an H₂ placeholder. **ORCA-correctness:** `%maxcore` is a
  simple directive and takes **no `end`** (unlike the `%pal` block) — the task spec said `%maxcore
  2000 end`; we emit the correct `%maxcore 2000`.
- **Bundle note:** the full `monaco-editor` import pulls all built-in languages (~4 MB / ~1 MB gz),
  and `3dmol` adds ~4 MB. Acceptable for a local desktop app; code-split later (import only
  `editor.api`, split the viewer).

## Input builder form (`src/input-builder/`)

Dropdowns → a valid `.inp`, still hand-editable in Monaco (`form → text` one-way, per ROADMAP; the
`!` line is never parsed back).

- **`orca-options.ts`** (data only, no React) — keyword catalogs: `JOB_TYPES`, `COMPOSITE_METHODS`,
  `FUNCTIONAL_GROUPS` (optgroup'd by rung), `BASIS_SETS`, `DISPERSION`, `RI_METHODS`,
  `SOLVATION_MODELS`, `SOLVENTS` (curated 20), `SCF_CONV`. `OrcaOption = {keyword, label,
  description?, builtInDispersion?}`; an empty `keyword` = "no keyword". Keywords checked vs ORCA 6.1.
- **`build-input.ts`** (pure, unit-tested) — `buildKeywordLine(state)` and `buildOrcaInput(state,
  geometry)`. Encodes the domain rules (`orca/input-format.md`): composite methods emit **only** their
  name (no basis/dispersion/RI — disabled in the UI); RI auto-adds the aux basis (`def2/J` for
  RIJCOSX/RI-J, `def2/JK` for RI-JK) on a def2 basis; canonical order
  `method basis auxbasis RI dispersion solvation jobtype scfconv`; `%maxcore` no `end`, `%pal` with
  `end`; solvation `MODEL(solvent)`. **`functionalHasBuiltInDispersion`** looks a functional up in
  `FUNCTIONAL_GROUPS` and skips the standalone dispersion token for functionals that bake it in
  (`wB97X-D4` — `-D4`; `wB97M-V` — VV10 `-V`), so `! wB97X-D4 … D4` can't double-count; the form
  disables the Dispersion dropdown with "Included in the functional" for those.
  - `geometry` is a **`Scene | null`**: a Scene supplies canonical merged coordinates and
    **overrides** the header charge (`totalCharge`) and multiplicity (`scene.multiplicity`); null →
    placeholder. `DEFAULT_BUILDER_STATE`: r²SCAN-3c composite, Opt+Freq, TightSCF, gas phase, 0/1,
    nprocs 4, maxcore 2000.
- **`InputBuilderForm.tsx`** — collapsible, **Scene-driven**: it derives a (single-fragment) Scene
  from the current buffer via `sceneFromOrcaInput`. With a Scene present, **Charge is read-only**
  (`totalCharge(scene)`, a "Σ of N fragments" caption), **Multiplicity stays editable** (written into
  the Scene before generate), and an inline **electron-parity warning** (`checkElectronParity`)
  appears under the numeric row — informational, Generate still works. The composite-vs-functional
  radio disables the basis/dispersion/RI selects; the solvent select is disabled in gas phase. A
  **live `!`-line preview** (`buildKeywordLine`) is the learning element. Generate → `onGenerate(
  buildOrcaInput(state, scene))`.

## Scene store & the geometry-editor UI

Geometry flows through `useSceneStore` (Zustand, `src/scene/store.ts` — the app's first store, ADR-008
#10), **not** the raw `content` string. The store contract and the two-way Scene↔Monaco sync are in
`modules/scene.md`; here is how `NewJobScreen` and the rail wire to it.

- **`NewJobScreen`** selects `scene` / `resetNotice` (+ actions) from the store and runs the two
  ADR-008 #6 sync effects: Scene→content injection (`injectSceneIntoInput`, guarded against echo) and
  a 500 ms content→Scene debounce (`xyzMatchesScene`, float compare). Loading paths build fragments:
  Import → an `import` fragment; SMILES → a `smiles` fragment (`sceneFromXyz`, charge from RDKit); Use
  a library molecule → a `library` fragment (on mount); each goes through `placeFragment` +
  `addFragment` (one road — `placeFragment` on an empty scene is the identity, so the first molecule
  and later reagents share code). **Store init is a `useLayoutEffect`** (the screen remounts on every
  nav; a plain effect flashes the previous visit's molecule). `formula` stays a separate `useState`
  (RDKit metadata for the library record, orthogonal to Scene geometry). **Save to Library** stores
  the canonical `mergeToXyz(scene)` + charge/mult read from the live `* xyz` header.
- **Reset notice** — a `.banner.warn` with Undo (`undoReset`) / Dismiss (`dismissResetNotice`), shown
  **only** when a manual coordinate edit merged **>1** fragment (a single-fragment collapse is a
  geometric no-op, so it stays silent).
- **`FragmentList.tsx`** (sidebar, below the 3D view) — one row per fragment: the shared
  `fragmentColor(index)` swatch (fragment 0 → a neutral swatch + "CPK" label, honestly flagged),
  an inline-editable name (uncontrolled, commits on blur/Enter → `renameFragment`, no per-keystroke
  store write), atom count + signed charge, a remove button; a totals line, and a note that removing
  fragment 0 recolours the next. Per-fragment **"Find conformers"** (`onFindConformers`) — see GOAT.
- **Add-Fragment panel** (`openSection: "add"`) — four sources: **Reagents** (`FRAGMENT_LIBRARY`
  chips, `title` = provenance), **Import file**, **SMILES**, **From library** (`list_molecules`,
  lazy). All run `addFragmentToScene(f) = placeFragment(currentScene, f) → addFragment`.
- **`AtomInspector.tsx`** (viewer column / rail, under the viewer, above `FragmentList`) — for the
  **last** picked atom: `atom N of <fragment> (<element>)` via `describeAtom`, x/y/z to 4 dp, and
  `local index N · global index N (both 0-based)` (0-based is the settled `%geom` constraint base —
  `wiki/orca/constraints.md`). At ≥2 picks, a readout line: the atom chain in **click order** with
  the value — `H···B  1.234 Å`, `104.5°`, `dihedral 178.9°` (distance uses `···`, angle/dihedral use
  `–`), a prominent **`inter-fragment`** badge when the two atoms are in different fragments (the
  future reaction coordinate), the angle vertex being the **middle pick**. A click-ordered chip row
  with fragment-colour swatches; a `Clear` button; **Esc** clears too. Also hosts **"Constrain
  selection"** (below). **Selection state lives in `NewJobScreen`**, not the store (the store stays a
  pure geometry wrapper): picks go through `toggleAtom`, and on every `compositionSignature` change
  the pick list is re-run through `selectionSurvives` (unchanged or pure append → keep; else clear) —
  `validateSelection` is a defensive second echelon (see `modules/scene.md`).
- **Viewer toolbar** (an overlay in the viewer's top-right, so it travels into fullscreen): a
  **"Numbers"** checkbox (drives `showAtomNumbers`, local `showNumbers` state, shown only when a scene
  is present), a **Theme `<select>`** (Dark/Black/Light/White → `changeTheme` sets `themeId` and
  persists `set_setting("viewer_theme", id)`; loaded on mount from `get_settings`; passes
  `viewerTheme(themeId)` to `MoleculeViewer`), and an **Expand/Exit** button (toggles `fullscreen`,
  which only adds a fixed-position class — no remount; see `modules/visualization.md`).
- **Esc precedence** — ONE keydown handler with an explicit branch read from a `fullscreenRef`: in
  fullscreen Esc exits fullscreen and leaves the selection alone; otherwise it clears the selection.
  One Esc = one action, a code branch (not a race between two mounted keydown effects).
- **Fullscreen rail** — the `.viewer-column` (viewer panel + a `.viewer-rail` holding AtomInspector +
  FragmentList + EditPanel + ConstraintPanel) is one shared DOM structure; a **Hide/Panel** button
  collapses the rail in fullscreen (not persisted). Layout detail in `modules/visualization.md`.

### Edit mode (`EditPanel.tsx`)
In the Atom section of the rail; all decision logic is in the pure `scene/edit-plan.ts`
(`modules/scene.md`), the panel does only `fetch` + state.

- **The mask is VISIBLE before Apply** — whenever `planEdit` is `ready`, `NewJobScreen` passes
  `plan.mask` to `MoleculeViewer` and the moving fragment glows. The panel names the rotation pivot:
  `Set angle · moving BH₄⁻ · vertex C #12` / `… · axis C#12–O#14` (`pivotLabel`). A calm **"chain read
  in reverse so the reagent moves"** line when `plan.reversed`, and a **"Move X instead"** button when
  `plan.alternative` exists (flips `NewJobScreen`'s `preferAlternative` → `swapToAlternative` → the
  other fragment glows and moves; resets on selection/scene change).
- **Fields:** `ready` → the op, current value, a target `<input>` (pre-filled with the current
  value), the unit, Preview / Apply; `unavailable` → the reason as calm text, no buttons;
  `needs-split` → the wait state below.
- **Preview touches ONLY the viewer** — it POSTs to `/geometry/set-internal` and hands the result up
  as `previewScene`, which the viewer renders (`scene={previewScene ?? scene}`). The store Scene and
  Monaco are untouched until Apply, so a keystroke in the target field never runs the Scene↔Monaco
  sync + collapse. Preview resets on selection change, scene change, or Cancel.
- **Apply** validates on our side (`applyResponseIssue`: static atoms unmoved `< 1e-6`, count matches
  — else refuse with a message), builds the new Scene (`applyResponseToScene` → `replaceFragmentAtoms`),
  then `setScene` (the normal Scene→Monaco injection). A one-step **Undo** notice restores the
  pre-edit scene.
- **Intra-fragment (`needs-split`)** — the one place the panel isn't purely local. **`NewJobScreen`
  owns the resolution** (it already orchestrates preview/undo/mask): a **race-guarded** effect (keyed
  on op+indices, cleanup sets `cancelled`) POSTs `plan.cut`/`plan.moving`/`plan.within` to
  `/geometry/rotatable-mask`; **a stale response (selection changed mid-fetch) is dropped**. The one
  resolved mask drives BOTH the glow and `set-internal`. `EditPanel` takes it as
  `splitMask`/`splitResolving`/`splitError`, shows `Internal edit · rotating about C#12–C#14`
  (`bondLabel`) + "finding the rotatable atoms…" while waiting, and surfaces a **422** (ring bond,
  cut/moving outside `within`) in the sidecar's own words via `.edit-error-severe`.
- **Errors, human-readable:** sidecar not ready, 422 (the detail text), 500 (a real post-condition
  breach — `.edit-error-severe`). Direct sidecar `fetch` to `http://127.0.0.1:{port}` from
  `get_sidecar_status` (SMILES and convert go the same way; the shared `src/sidecar-client.ts`
  `postSidecar` + `describeSidecarError` translate 404/422/5xx/network so no caller shows a bare "Not
  Found").

### Constraint panel (`ConstraintPanel.tsx`)
A section in the rail; its **only source is `parseConstraintsBlock(content)`** — no local constraint
state, so a block hand-edited in Monaco shows up verbatim. Each row: the type badge (B/A/D/C), the
atoms in our terms (`describeAtom`, "C#12 (Ibuprofen) ··· B#33 (BH₄⁻)"), the set value **and** the
current measured value (`measureSelection` / `formatMeasurementValue` — a set-vs-now divergence is
information, not an error), and a `×` that deletes the row by re-`injectConstraints`-ing the rest.

- **"Constrain selection"** (in `AtomInspector`, active for a 2/3/4-atom pick, kind by length via
  `constraintFromSelection`) adds a constraint **without a value** (freeze as-is), with an optional
  value field. It calls `NewJobScreen`'s `constrainSelection`, which parses the text, appends (dedupe
  via `sameConstraint`), and writes through `injectConstraints` — **one data path**; the panel
  re-reads the text.
- **Range guard.** `constraintIndexIssues(cs, atomCount)` marks bad rows red (bad indices named) and
  `NewJobScreen` **blocks Create and Create & Run** on a non-empty result (`canCreate` false, and
  `create()` refuses even if called) — **the one place the app refuses to run on input content**,
  because ORCA does not range-check a constraint index and **segfaults** on a bad one, and the job's
  input is immutable once created. The message names the count, the valid range, and that ORCA
  segfaults.
- **Composition guard.** Using the existing `compositionSignature` (no second notion), a ref tracks
  the last signature; when it moves while the text has constraints, a warning above the panel lists
  what each constraint names **now** (element + fragment, or "out of range"). We do **not** rewrite
  or remap — "the same atom after a removal" has no operational definition (the same call as
  `selectionSurvives`). Clears on Dismiss or when no constraints remain.
- **Non-destructive rewrite.** The panel reads `inspectConstraintsBlock` → `absent` / `parsed` /
  `unrecognised`; on `unrecognised` (a `#` comment inside the block, or an unparseable token) it goes
  **read-only**, disables add + delete + "Constrain selection" + the xtb button, and never calls
  `injectConstraints` — so nothing is rewritten. Numbers are preserved as typed (`90.0`) via
  `valueText`.

### xTB pre-optimize button
In the rail, below the constraint panel. Relaxes the whole scene with GFN2-xTB while holding the
text's constraints, then replaces all coordinates.

- Constraints come from the text (the `parsed` cs passed straight to the Rust `xtb_optimize`); on an
  `unrecognised` block the button is **disabled with a reason**.
- **Running state + cancel:** the button shows "Pre-optimizing…" and a **Cancel** (`xtb_cancel` →
  killpg + sweep); `xtb-progress.ts` `formatXtbProgress` renders the `xtb:progress` cycle + a ticking
  clock, so a pre-cycle stall is visible at once.
- **Applying the result:** `replaceAllAtoms(scene, parseAtomLines(result.xyz))` (count + element
  order invariant), then `applyEdit` — so **Undo rides the exact same one-step mechanism as edit
  mode** (the "Edit applied · Undo" notice). A success line reports the wall time and the held
  deviations ("held distance ±0.011 Å"). A **stale-result race** (a result arriving after the scene
  changed) is guarded by `xtbResultApplies` — the launch scene is captured and the result applied
  only to that exact reference, else discarded.

### GOAT conformer search
- **"Find conformers"** (per-fragment, in `FragmentList`) creates + runs a normal job:
  `input_content = goatInputForFragment(fragment)` (`%pal` inserted by the backend's
  `align_pal_nprocs` at submit, uniform with every job), `scene_json` = a **single-fragment** scene of
  that fragment (`modules/scene.md`), title `Conformer search — <name>`. An honest cost caption sits
  under the list (GOAT is slow, holds the queue).
- **Ensemble panel** (`JobDetailScreen`) — on a *completed* GOAT job it lazily calls `read_job_ensemble`
  (`input.finalensemble.xyz`) + `parseEnsemble`; non-GOAT jobs show nothing. Lists conformers with
  **ΔE in kcal/mol** (`deltaEKcal`; `NaN` → a dash) + the absolute Eh secondary; the selected one
  renders in a memoised `MoleculeViewer`. Count comes from the *file*, not the log summary
  (`goat.md`).
- **"Use this conformer"** — `planConformerApply` decides: **replace** the fragment in place (it
  survives the New Job → detail nav — the store is a singleton) → New Job with `keepScene`; else a
  **new** single-fragment scene; else **refuse** (a banner, no throw) on a composition mismatch.

### Job iteration
**New iteration** (a `JobDetailScreen` button, shown whenever a job is loaded — cloning a running job
is fine, only its input is taken) → `onIterate(job)` → `App` sets `{kind:"new-job", initialJob:job}`.
`NewJobScreen` seeds the title `${job.title} (iteration)` and `content = job.input_content` (before
paint), and restores the scene in the `useLayoutEffect` via **`restoreScene`** (`modules/scene.md`).
A `snapshotRejected` state drives a `.banner.warn` **only** when a snapshot was present but discarded
(didn't match the input); a plain `scene_json = NULL` job shows **no** note. `create_job` is the one
write path: `NewJobScreen.create()` passes `sceneJson: scene ? serializeScene(scene) : null`.

## Convergence dashboard (`src/convergence/`)

A learning instrument on Job detail: watch energy per cycle and the criteria live instead of reading
the log. `types.ts` mirrors `src-tauri/src/convergence.rs` (`ScfPoint` / `Criterion` / `OptPoint`,
the `ConvergenceEvent` union on `kind: "scf" | "opt"`, `ConvergencePayload`).

- **`ConvergenceDashboard.tsx`** (props `events`, `status`, `variant`) — three sections, each shown
  only with data:
  - **A. Progress indicator** (real state, not a fake %): Opt → `Optimization cycle N · M/T criteria
    met` + a bar + a ✓/✗ chip per criterion; single point → `SCF iteration K`; Freq → nothing.
  - **B. Energy per cycle** (recharts `LineChart`, ≥2 points) — Y auto-domain, ticks 6 dp; tooltip
    adds ΔE-from-previous in kcal/mol (×627.509).
  - **C. Criteria vs tolerance** (recharts, **log Y**) — a line per gradient/step criterion with a
    dashed `ReferenceLine` at its tolerance; `Energy change` excluded (signed, different scale — it
    lives in the progress indicator).
  - **GOAT `variant: "goat"`** — the head reads `Conformer search · inner optimisation, cycle N · m/5
    criteria met` with a grey "one candidate of many — overall GOAT progress is not shown", and **the
    progress bar is hidden** (a full bar with minutes of search still ahead was misleading); the chips
    stay. `JobDetailScreen` passes `isGoatInput(job.input_content) ? "goat" : "standard"`. The unused
    `status` prop is left alone (one prop, one meaning).
- **WebKitGTK / recharts:** `ResponsiveContainer` measures **0×0** in WebKitGTK, so a
  `useContainerWidth` ResizeObserver passes an **explicit pixel `width`** (+ fixed height) to each
  `LineChart`; no `ResponsiveContainer`.
- **Integration:** the `job:convergence` listener is attached before submit (listeners-first);
  `read_job_convergence` backfills after listeners attach, seeding only if live events haven't arrived
  (same order/guard as the log). The dashboard sits in a collapsible accordion above the log console,
  expanded while the job is active, collapsed once finished; its body is `max-height: 58vh; overflow-y:
  auto` so it never crushes the log below.

## Output console, viewer & search

Two output modes on Job detail (toggled by a `Live log | Browse file` control; any successful search
with hits auto-switches to Browse):

- **Live** (default) — the streaming `<pre className="log-console">`. Appending 50 lines / 100 ms to a
  `<pre>` is cheaper than to a Monaco model, and its autoscroll is already tuned. **Backfill:** for a
  non-draft job, `read_job_output(id)` seeds the console after listeners attach, guarded
  `setLines(prev => prev.length ? prev : existing)` so live events win (a running job can briefly
  duplicate a few lines — accepted). Shows an `energy … Eh · time …` line once results exist, and an
  **Open Folder** button (`open_job_folder`) when `job_dir` is set.
- **Browse** — `OutputViewer.tsx` (`@monaco-editor/react`, offline via `editor/monaco-setup`):
  `plaintext` (no ORCA grammar), `readOnly`, `wordWrap:"off"` (ORCA output is columnar), minimap on.
  **Absolute line numbers when truncated:** `lineNumbers: n => n + firstLineNo - 1`.
  `forwardRef` + `useImperativeHandle` expose `revealFileLine(fileLineNo, colStart?, colEnd?)` and
  `setHits([])`; mapping `monacoLine = fileLineNo - firstLineNo + 1`, and `< 1` (a hit above the
  loaded tail window) returns `false` with the panel noting "above the loaded window". Decorations via
  `createDecorationsCollection()` (not the deprecated `deltaDecorations`): `.hit-all` for all hits,
  `.hit-current` (inline) + `.hit-current-line` (whole line) for the current one. Calls made before
  `onMount` are buffered and flushed on mount. **Lazy:** `read_job_output_for_viewer` runs only on the
  **first** Browse entry (not on opening a job); a running job's Browse shows a **Reload** button +
  `snapshot at HH:MM:SS`. Both modes stay mounted after the first Browse and switch via `display` so
  Monaco isn't rebuilt.
- **`OutputSearchPanel.tsx`** — a box, `regex` / `Aa` toggles, and preset chips (fill + flag + search
  in one click). A search runs with `context_lines: 0` (no excerpt payload), stores the hit list,
  triggers Browse, and drives the viewer via effects keyed on the viewer handle (no mount-timing
  race). `Prev`/`Next` step **cyclically**; a `i+1 / total` counter (`i+1 / 500 of 637` when
  truncated) + the current `line N`. **Keyboard:** `Enter` searches (or steps to the next hit if the
  query is unchanged), `Shift+Enter` previous, `F3`/`Shift+F3` next/prev whether focus is in the panel
  (container `onKeyDown`) or the viewer (Monaco `addCommand` → a `navRef`).
- **Coordination:** `JobDetailScreen` owns `mode`, the lazily-loaded `viewerContent`, the viewer
  handle (`viewerApi`, set via a callback `ref`), and the `navRef`; `resetToken` (bumped on Reload)
  clears stale hits since line numbers can shift.

## Multi-format import (`src/viewer/import-file.ts`)

The "Import file" button on **both** New Job and Molecules accepts `.xyz,.pdb,.cif,.mol,.sdf,.gen`.
Shared `importStructureFile(file)` + `IMPORT_ACCEPT`: `.xyz` is parsed locally (`xyzToAtomLines`,
no round-trip); other formats send `from_format` from the extension to the sidecar `/convert`
(`to_format: "xyz"`, port via `get_sidecar_status`), then the same `xyzToAtomLines` path. Throws
`Error(message)` on any failure, shown in each screen's existing banner; the editor/draft is only
touched on success. On New Job the result becomes a fragment added via the scene store; on Molecules
a standard xyz via `atomLinesToXyz`.

## Manual panel (`src/manual/`, `src/screens/ManualScreen.tsx`) — Phase 4.4

The first **real consumer** of the manual index (ADR-013 / [manual-index.md](manual-index.md)). A tab
in the shell; two columns — debounced FTS search on the left, the full **page** on the right.

- **A section indexes, a page shows.** Search is section-grained (bm25 needs it) but reading is not —
  from a lone section the author could not see *why* a keyword sat where it did. So a result opens the
  **whole page** and scrolls to the found section (see [manual-index.md](manual-index.md)). The section
  stays the search unit; it is no longer the display screen.
- **`ManualScreen`** — the host. On mount `manual_index_status`: **null → a "Build index" state**
  (a card + button running `build_manual_index`, then the `IngestReport` tallies), **not** an empty
  result list (which reads as "nothing found" — the failure this guards). With an index: a search box
  → `search_manual` (250 ms debounce; **empty query → empty list**, the command's contract, never an
  error), results as `breadcrumb › **title**` + a highlighted snippet (rank hidden). Click →
  `get_manual_page(hit.file)` → `PageView` with `targetSectionId = hit.id` (scroll + highlight).
- **`PageView` is the ONE display component** (`src/manual/PageView.tsx`), shared by `ManualScreen` and
  `ManualDrawer` — there is no second render path (the pattern was collapsing four times over). It takes
  a `ManualPage` + an optional `targetSectionId`. `SectionView` is **gone**, folded into `PageView`.
  - Splits `page.text` into line-owned segments — the preamble, then each section `[line_start,
    line_end]`. Sections tile the file by line (the sectioner's line-conservation), so **a section's DOM
    node spans exactly its indexed bounds**: highlighting the bounds is just adding `.target` to that
    node. Scrolls the target into view (`scrollIntoView`) on target/page change.
  - **In-page ToC**: the file's headings (`level`+`title`) as a collapsible list (`<details>`, open
    only on ≤20-section pages) — how the reader moves on a **209 KB / 162-section** page (measured max;
    4 pages carry >50 sections). No `<select>`, so the WebKitGTK select gotcha does not apply.
  - Body rendering still goes through `renderManualBody` → the pure, tested `parseManualBody`
    (`render.ts`) — the block rules are unchanged, and the preservation test (below) stays green after
    moving `renderManualBody` from `SectionView` into `PageView`.
- **Render rule — three categories, each source char in EXACTLY ONE (`src/manual/render.ts`,
  4.11), the display analogue of the sectioner's line-conservation (rule #9).** Section bodies are
  MyST. A naive Markdown renderer eats what it doesn't understand **without error** — the quietest
  way to lose text. So every char is one of:
  - **(1) recognized & TRANSFORMED**, each with its OWN test (a char-preservation test cannot cover a
    transform — it *changes* the text): **inline code** `` `…` `` → `<code>` (backticks gone, content
    kept — the largest surface, 11.77 % of corpus); **math** `$…$`/`$$…$$` → **KaTeX** (the corpus is
    dollar-only — `\(…\)` and `{math}` are **0** measured — so exactly two delimiters, no more);
    **cross-references** `{ref}`/`{numref}` roles + `[..](sec:/tab:)` links → a link, **but only when
    the anchor map resolves** — an unresolved target stays category 2 (verbatim text, never a dead
    click; same posture as a NULL anchor and hover silence).
  - **(2) UNRECOGNIZED → verbatim.** The preservation test lives HERE, **unweakened** — split off so
    it stays a pure char-for-char check over samples with NO category-1 construct (else a transform
    would force it green while silently dropping text). Code fences, pipe tables, prose, and every
    VISIBLE directive (`{note}`/`{table}`/…) are category 2.
  - **(3) recognized & DELIBERATELY HIDDEN — a NAMED whitelist, EXACTLY THREE**, each for a measured
    reason (the census refuted "hide anything directive-shaped": 13.6 % of corpus is under a directive
    fence but almost all is VISIBLE content — a blanket hide would eat it):
    - `{index}` (321×) — index markers, INVISIBLE in the real Sphinx render;
    - `{tabularcolumns}` (76×) — a LaTeX column spec, INVISIBLE in HTML;
    - **`({raw}, latex)` (176×) — the KEY IS THE PAIR, not the name `{raw}`.** `{raw}` is output aimed
      at a *different* builder; the `latex` variant is invisible in the HTML we reproduce. "The arg is
      always `latex`" is measured **on this corpus**, not a property of MyST — a 6.2 refresh could add
      `{raw} html`, and a name-only list would swallow it. A test pins `{raw} html` → **shown
      verbatim**; another pins that a directive outside the list (`{note}`) is **not** hidden.
    The name compare is **one function, case-insensitive from day one** (`isHiddenDirective`), because
    admonitions already arrive as `{Note}`/`{note}`/`{NOTE}` (a later second normalization would be the
    fourth turn of the `normalize_label` pattern).

  **Post-conditions (`render.test.ts`, split three ways):** category 2 preserves every non-whitespace
  char (`reactText` mirrors DOM `textContent` without jsdom); category 1 asserts each transform
  explicitly (`tokenizeInline` is pure, so tokens are checked directly); category 3 asserts the
  whitelist hides and nothing outside it does. **We still do NOT parse MyST** — no markdown library,
  no MyST parser; "unrecognized → verbatim" is the base, and this unit only adds **named** exceptions.
- **KaTeX — the FIRST visual dependency of the project (precedent).** Chosen over MathJax because it
  is **fully offline** (fonts bundled, **0 network `url()` in the built CSS** — verified; keeps the
  no-extra-network posture of `overview.md`), renders synchronously, and covers the LaTeX subset
  Sphinx emits. `throwOnError: false`, so an unknown macro renders its **source verbatim** (falls into
  category 2) instead of breaking the page — pinned by a test. Bundle cost, measured before/after
  `npm run build`: main JS **6 254 → 6 557 KB (+303 KB raw / +82 KB gz)**, CSS **187 → 217 KB
  (+30 KB)**, whole `dist/` **16.5 → 17.9 MB (+1.41 MB)** — the delta is the 59 bundled KaTeX font
  files (loaded by the browser only for glyphs actually used). CSS imported once in `main.tsx`, out of
  the test import chain.
- **Cross-reference resolution — via the EXISTING anchor map, no second normalization.** A label
  (`sec:…`/`tab:…`) resolves through the new **read-only** `resolve_manual_anchors` command, which
  slugifies with the sectioner's `predict_anchor` (rule #9 — the same transform that built the stored
  `anchor`, reused) and looks it up in `manual_sections.anchor`. PageView batches every label on a
  page into one call on load, then renders synchronously; a same-page target scrolls internally, a
  cross-page target calls the host's `onNavigate` (ManualScreen loads the file; ManualDrawer loads it
  in place). Measured (`xref_resolution_measure`): **1364 / 1722 (79.2 %) resolve** — `{ref}` 98.8 %
  and `[..](sec:/tab:)` links 98.5 %, while `{numref}` is only 32.8 % because it targets numbered
  tables/figures/equations, most of which are not section anchors; those **stay verbatim**, correctly.
- **`{literalinclude}` (255×) — a visible ABSENCE MARKER, not verbatim (category 5).** The directive
  references an external `.inp` the manifest fetch never took (`_sources/*.md.txt` only), so verbatim
  would show a **path where the manual gave an input example** — silent emptiness exactly where input
  examples are most valuable. `render.ts` `isMissingInclude` → “input example not loaded (`<path>`)”,
  turning 255 silent gaps into a measured, named hole. (`{figure}` 161 + `{subfigure}` 14 = **175** are
  the same class — external `_images/` we did not fetch — but are **left untouched this unit**; the
  number is recorded so “do we fetch images?” arrives with a figure, not a guess.)
- **Snippet highlighting** — `search_manual`'s `snippet()` wraps matches in **PUA codepoints
  `U+E000`/`U+E001`**, not `[`/`]`. Measured: `[`/`]` occur **1905 / 1903** times in the 4 MB corpus
  (every `[link](…)` / MyST role), the PUA pair **0** — so `<mark>` splitting on them can't paint a
  phantom highlight on literal brackets. `SNIP_OPEN`/`SNIP_CLOSE` are shared with Rust.
- **`manual_root()` debt closed** — the corpus path now resolves honestly for source *and* bundled runs
  (page display reads the corpus off disk, so it could no longer stay a source-only path). Detail in
  [manual-index.md](manual-index.md).
- **Pipe tables render monospace (4.4 Part B).** `render.ts` now groups a run of `^\s*\|` rows into a
  `<pre>` (like a fence) so table columns align — measured 110 sections (7.1 %) carried a pipe-table
  that was rendering as misaligned proportional prose. Same linear per-line check as the fence, **no
  table parsing** — a font choice only, so the preservation test stays green unchanged.

## Manual hover in the editor (`src/editor/orca-hover.ts`, `src/manual/ManualDrawer.tsx`) — 4.4 Part B

The editor's consumer of `keywords.json`. `InputEditor` registers a Monaco hover provider (+ the
`orca.openManualSection` command) on language registration. The provider gets the word (whole, via the
`wordPattern` — `def2-SVP`, `%maxcore` come intact), classifies its context
([keyword-lookup.ts](../../src/manual/keyword-lookup.ts): `!`-line → simple, `%name` → block, inside a
block → block-option of that block via `enclosingBlock`), and looks up **type- and block-aware** (a
wrong-type match is a miss), consulting `aliases[]`.

- **Contract (enforced, from manual-keywords.md):** a qualified **miss → the hover does not appear at
  all** (silence), never a bare-name or FTS fall-back — hovering `%maxcore` shows nothing, not "not
  found". Unqualified search is the panel's separate path.
- **Hover body:** keyword, type, owning block (+ `owner_source`), and an **Open** command-link with the
  target's breadcrumb › title; several targets → *"documented in N places"* (a list, not a picked
  first). An empty `summary` does not suppress it (seeded records have none).
- **`ManualDrawer`** is a fixed-position side overlay (does not disturb the editor layout, so the
  author stays in the editor). Clicking Open fires the command → the drawer resolves the descriptor via
  `resolve_manual_section` (the keywords.json→DB bridge, with a version check) to a section (file + id),
  then loads that section's **whole page** via `get_manual_page` and renders it in the **SAME `PageView`**
  as `ManualScreen` — one display component — scrolled to and highlighting the resolved section.

## Queue control (status bar)

- The **Pause/Resume button always renders** (you can pause *ahead* of stacking jobs, and the
  `paused` state stays visible once the queue drains). Adaptive `queueLabel`: activity → `1 running,
  2 waiting`; empty + not paused → `idle`; empty + paused → `paused`. A `.queue-paused` class
  (`var(--warn)`) highlights the indicator whenever paused. `togglePause` flips `queue.paused`
  optimistically right after the successful `invoke` (label updates immediately; `refreshQueue` still
  runs in `finally` to reconcile counts). The backend
  (`pause_queue`/`resume_queue`/`is_queue_paused`) is untouched.

## Quirks

- **Offline Monaco** — the worker path double-map (see Editor); `debugging/001`.
- **WebKitGTK `<select>`** — native GTK selects ignore the inherited `.input` colour and render
  dark-on-dark. The fix (`-webkit-appearance:none` + explicit colour/background + an inline-SVG
  chevron) lives on the **`select` element selector** (`.select` kept as an alias), so **every**
  `<select>` is covered by default (a new class silently missed the fix once — that's why it's on the
  element). `.select:disabled` and `.input[type=number]` are pinned dark too; the `option` popup is a
  native GTK menu, so its styling is best-effort. `debugging/003`.
- **recharts** `ResponsiveContainer` mismeasures 0×0 in WebKitGTK — pass explicit pixel widths.
- **StrictMode double-submit** — a `didSubmit` ref neutralises the dev double-mount; the backend slot
  mutex is the real guard.
- **Listeners-first** — attach event listeners before the action that produces events (submit,
  backfill), so no early output/status/convergence is lost.
- **`useLayoutEffect` for store init** — `NewJobScreen` remounts on every nav; a plain effect flashed
  the previous visit's molecule.
- **Round swatches** — `.fragment-swatch` is `border-radius: 50%` (one rule, `FragmentList` +
  `AtomInspector`); a hollow square swatch beside the "Numbers" checkbox read as an unchecked checkbox.

## Screens (later phases)

- **Results** (summary card, trajectory player, IR spectrum + mode animation, orbital isosurfaces,
  export) — **done** (Phase 3); the results-frontend detail lives in
  [results-ui.md](results-ui.md) (`src/screens/ResultsCard.tsx`, `src/spectrum/`, `src/trajectory/`,
  `src/orbitals/`, `src/export/`).
- **Manual** (FTS search panel) — **done** (Phase 4.4, below).
- **"Run on:" backend selector + server profiles** in Settings — Phase 5.

## State (current & planned)

Current: the scene store (`useSceneStore`, Zustand) for New Job geometry; everything else is local
`useState` + `invoke` wrappers over SQLite reads. Planned, as pain appears: a `jobsStore` (job list +
live statuses), `settingsStore`. Don't reach for react-query until it hurts.

## Conventions

TypeScript strict; functional components + hooks; no `any` without a comment. 3Dmol.js instances must
be explicitly destroyed on unmount (WebGL context leaks). Log console appends via event batching
(100 ms flush), capped at 5000 lines.
