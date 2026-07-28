# Project Log

Append-only. Entry format: `## [YYYY-MM-DD] type | Title`
where `type ∈ {session, decision, ingest, lint, milestone}`. Newest entries at the bottom.

---

## [2026-07-26] milestone | Project scaffold created

Initial scaffold generated from architecture planning sessions (Claude web):

- Repository structure defined: Tauri/React frontend, Rust core, Python sidecar, wiki.
- Six founding ADRs written (stack, sidecar, execution backends, storage, SSH strategy,
  manual integration).
- Roadmap drafted: Phases 0–6, each ending in a usable increment.
- Wiki system initialized per the LLM-wiki pattern (CLAUDE.md = schema).

Key founding decisions to remember:
1. ExecutionBackend abstraction from day one — remote SSH execution is a first-class,
   optional path, not a bolt-on.
2. The app is a learning instrument, not just a launcher: live convergence plots and
   integrated manual are core features.
3. Mission test for every feature: "does it lower the barrier for a terminal-shy chemist?"

Next: Phase 0 — ORCA 6 installation + environment verification, then Tauri scaffold.

## [2026-07-26] session | Phase 0: ORCA verified

Environment confirmed working end-to-end:

- ORCA **6.1.0** at `/opt/orca/orca`, system **OpenMPI 4.1.6** (compatible with this build),
  host Laptop-main (Linux Mint).
- Verification run `water_optfreq` (r²SCAN-3c `Opt Freq TightSCF`, `%pal nprocs 4`,
  `%maxcore 2000`): opt converged in 4 cycles, final energy −76.418938719971 Eh,
  frequencies 1653.26 / 3813.32 / 3932.49 cm⁻¹ (all positive → true minimum),
  `ORCA TERMINATED NORMALLY`. Full-path + `%pal` parallelism verified.
- Learned: ORCA has no CLI flags — `orca --version` is treated as an input filename.
  Version comes from the run banner (`Program Version 6.1.0`). Recorded in gotchas.

Wiki updated: `orca/orca-basics.md` (verified environment table + test results),
`orca/gotchas.md` (no `--version`). ROADMAP Phase 0 install/verify/record marked done.

Next: scaffold Tauri 2 + React + TS, then Python sidecar `/health` + SQLite init.

## [2026-07-26] session | Phase 0: app scaffold (Tauri + sidecar + SQLite)

Built the Phase 0 skeleton — `npm run tauri dev` opens the OrcaStudio window showing
sidecar status and the configured ORCA path. Verified end-to-end on Laptop-main.

**Prereqs installed:** Rust 1.97.1 (rustup, minimal profile); apt libs
`libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev patchelf libayatana-appindicator3-dev`
(Ayatana substituted for the older libappindicator3-dev — maintained replacement).

**Frontend (src/):** `create-tauri-app` react-ts template moved into repo root; renamed to
OrcaStudio (identifier `com.orcastudio.app`). `App.tsx` = System Status dashboard: sidecar
dot (poll 5s) + editable ORCA path (Save). Builds clean under TS strict.

**Sidecar (sidecar/):** FastAPI `GET /health -> {status:"ok",version:"0.1.0"}` + localhost
CORS; venv; `pytest` green.

**Rust core (src-tauri/):** `db.rs` (init + migration v1 `settings` table, seeds
`orca_path=/opt/orca/orca`), `error.rs` (`AppError`, serialized as string in Phase 0),
`sidecar.rs` (`SidecarManager`: free-port pick, spawn uvicorn from venv, 15×2s health poll on
a bg thread, kill on `ExitRequested` + `Drop`), `commands/settings.rs`. `cargo test` green.

**Verified:** window launches; Rust spawns uvicorn (dynamic port, from `.venv`), health poll
hits `200`; DB seeded; changing `orca_path` survives restart (migration `INSERT OR IGNORE`
doesn't clobber). Note: GUI window couldn't be screenshotted headlessly (no xdotool to raise
it) — verified via process/log/DB/curl instead.

**Doc drift found:** template ships **React 19**, ADR-001 says React 18 → flag for an ADR
update. `AppError` serialized as plain string, not the `{code,message}` in tauri-core.md's
planned surface → revisit when structured error codes are needed.

Wiki updated: `modules/frontend.md`, `modules/tauri-core.md`, `modules/sidecar.md`
(all → Phase 0 scaffold done). ROADMAP Phase 0 fully checked.

Next (Phase 1): job model + state machine, Monaco editor, `LocalBackend`, live log tailing.

## [2026-07-26] session | Phase 1 step 1: job model + state machine in SQLite

Built the Rust data layer for jobs — no UI, no process spawn (those are later Phase 1 steps).

**Migration v2 (`db.rs`):** `migrate()` is now version-aware — ensures the v1 `settings`
table/seeds, reads stored `schema_version`, and steps forward (`< 2` → `CREATE TABLE jobs`,
persist `schema_version=2`). Backward-compatible: an existing v1 DB upgrades in place with
settings untouched (`orca_path` preserved). New test `migrate_v1_to_v2_preserves_settings`.

**`models/job.rs`:** `JobStatus` enum (`Draft|Running|Completed|Failed`, lowercase serde on
wire + in DB via `as_str`/`from_db`); `Job` struct mirroring the table 1:1 (`Serialize`);
`Job::from_row` + `Job::COLUMNS` (single source of truth for the select list). Remote
(uploading/syncing) and `parsed` states deliberately deferred.

**`commands/jobs.rs`:** `create_job` (UUID v4, inserts `draft`), `list_jobs`
(`created_at DESC`), `get_job` (`AppError::NotFound` if absent), `update_job_status` (stamps
`started_at` on `running`, `completed_at` on `completed`/`failed`; `NotFound` on unknown id).
Commands are thin lock-and-delegate wrappers over `*_conn(&Connection)` helpers → the
state-machine logic is unit-testable without a Tauri app. All 4 registered in `lib.rs`.

**`error.rs`:** added `NotFound(String)`. **Cargo.toml:** added `uuid` (v4).

**Verified:** `cargo test` — 7/7 green (5 new job tests: create→draft in list, running sets
`started_at`, completed sets `completed_at`, get/update missing → `NotFound`; plus the v1→v2
migration test and the original `init_db_seeds_defaults`). `cargo build` clean, no warnings.

Decision: `update_job_status` rejects unknown status strings via `JobStatus::from_db`
(returns `AppError::Internal` for now — no dedicated validation variant yet). Only the four
Phase 1 states exist; extend the enum + migration when remote/parsed states arrive.

Next: Monaco `.inp` editor + template library, then `LocalBackend` (job dirs, spawn, tailing).

## [2026-07-26] session | Phase 1 step 2: Monaco editor + templates + job-creation UI

Frontend for authoring jobs. `npm install @monaco-editor/react monaco-editor`.

**Editor (`src/editor/`):** `orca-language.ts` — Monarch grammar `orca-inp` highlighting the
structural bits (`!` line, `%block`/`end`, `#` comments, `* xyz ... *` delimiters, numbers,
strings; `ignoreCase`), deliberately not a full keyword list. `InputEditor.tsx` wraps
`@monaco-editor/react` (vs-dark, full height), registers the language on `beforeMount`.

**Offline Monaco (`monaco-setup.ts`):** `@monaco-editor/react` defaults to a CDN loader — fatal
for a desktop app. Pinned to the bundled package via `loader.config({ monaco })` +
`MonacoEnvironment.getWorker`. The worker import path is `monaco-editor/editor/editor.worker.js`
(NOT `esm/vs/...`): the package `exports` map rewrites `monaco-editor/*` → `esm/vs/*`, so the
prefixed path double-maps and Rollup can't resolve it. Non-trivial — logged in
`debugging/001-monaco-offline-worker-resolve.md`.

**Templates (`templates/orca-templates.ts`):** 8 hardcoded `OrcaTemplate`s (SP/Opt/Freq/Opt+Freq
× r²SCAN-3c and B3LYP-D4/def2-SVP), each a full runnable `.inp` with `%pal nprocs 4 end`,
`%maxcore 2000`, H2 placeholder geometry. **Domain-correctness deviation from the task spec:**
spec asked for `%maxcore 2000 end`, but `%maxcore` is a simple directive with NO `end` (unlike
the `%pal` block) — emitting `end` would be wrong ORCA, so templates use `%maxcore 2000`.

**UI (`App.tsx` + `screens/`):** rewrote the single dashboard into a tabbed shell — New Job /
Jobs / Settings (local `useState`, no router). `NewJobScreen` (title + template picker grid +
editor → `create_job`), `JobsScreen` (`list_jobs` → title/status-badge/created table),
`SettingsScreen` (the relocated ORCA-path editor). Bottom status bar shows sidecar dot + ORCA
path (the old System Status is no longer the home screen). Styling extracted to
`styles/app.css` — monochrome dark + one accent. Shared `types.ts` mirrors the Rust `Job`.

**Verified:** `tsc --noEmit` clean; `vite build` succeeds; loaded `vite dev` in Chrome — nav
works, picking a template fills the editor with live ORCA highlighting, zero console/worker
errors (confirms bundled Monaco loads offline at runtime). Full `create_job` write path relies
on the Rust commands already unit-tested in step 1. Note: full `tauri dev` GUI not screenshotted
headlessly (as in Phase 0); browser verification of the web frontend used instead.

**Bundle note:** full `monaco-editor` import pulls all built-in languages (~4 MB / ~1 MB gz).
Fine for a local app; future optimization = import only `editor.api`.

Next: `LocalBackend` — isolated job dir, full-path spawn, `output.out` capture + tailing.

## [2026-07-26] session | Phase 1 step 3: LocalBackend — ORCA spawn, live log, completion

The app runs its first real ORCA calculation. New `src-tauri/src/local_backend.rs` + the
`submit_job` command + `JobDetailScreen` with a live log console.

**Backend (`local_backend.rs`):** `submit(app, id)` → validate draft, read `orca_path`, reserve
the single slot, `prepare_job_dir` (`<data>/jobs/<id>/` + `input.inp`), `run_orca` (full
absolute path, `input.inp`, cwd = job dir, stdout piped, stderr → `stderr.log`), mark `running`,
then a background thread streams stdout: appends each line to `output.out` AND batches to the UI
via `job:log` (flush every 50 lines / 100 ms). On `wait()`: write `.exit_code`, then
`detect_completion` reads a ~5 KB tail → `completed` iff `ORCA TERMINATED NORMALLY` + exit 0,
else `failed` with a message from stderr/output tail; persisted via `finalize_job_conn`.
`JobRunner` managed state = job-dir root + `Mutex<Option<String>>` (running id) → concurrency 1.
All five domain rules honoured (full path / isolated dir / stream-not-slurp / marker+banner /
concurrency 1). New `AppError::Backend`. Emits `job:log` + `job:status`.

**Design choice:** no local `run.sh` wrapper — we pipe stdout in Rust and write `.exit_code`
ourselves (simpler + gives the live stream). SshBackend will still use a remote runner script;
the `.exit_code` marker convention stays shared. (Noted in execution-backends.md.)

**Frontend:** `App` screen state is now a union incl. `{kind:"job-detail", jobId, autoRun}`.
`NewJobScreen` gained "Create & Run" (creates draft, opens detail with autoRun). `JobsScreen`
rows clickable + Run/Open/"Running…" actions. New `JobDetailScreen`: attaches `job:log`/
`job:status` listeners FIRST, THEN submits (so no early lines lost); terminal-style `<pre>`
console with auto-scroll, 5000-line cap; reloads the record on terminal status for
`error_message`. A `didSubmit` ref neutralises StrictMode's dev double-submit (backend slot
mutex is the real guard).

**Verified:**
- `cargo test` — 13 green (added `prepare_job_dir`, `read_tail`, `last_lines`,
  `detect_completion`, `set_job_dir`, `finalize` tests). `cargo build` clean, no warnings.
- **Real ORCA end-to-end:** `#[ignore]`d test `real_orca_water_single_point_completes` runs an
  actual water single point (r²SCAN-3c) through `run_orca` + `detect_completion` against
  `/opt/orca/orca` → passes in ~0.8 s, output has `ORCA TERMINATED NORMALLY`, `.exit_code`
  written. Run with `cargo test -- --ignored`.
- Frontend: `tsc` clean, `vite build` ok, rendered in Chrome — New Job shows both buttons,
  Jobs table renders (invoke fails gracefully in a plain browser → banner, no crash), no
  uncaught console errors.
- **Not verified end-to-end:** the full in-app run flow (submit_job IPC → live `job:log` events
  → console updating in the real webview) — the Tauri GUI can't be driven headlessly here (same
  limitation as Phase 0). The risky part (real ORCA spawn/stream/detect) IS covered by the
  ignored test through the exact backend code; the IPC/event glue is standard Tauri.

**Known gaps (deferred, noted in wiki):** no startup reconciliation (a job left `running` after
a crash stays `running`); no cancel/kill; no result parsing (energy/wall_time); no `output.out`
backfill when opening an already-running job's detail.

Next: minimal result extraction (final SCF energy, wall time) + job list showing energy;
then startup reconciliation.

## [2026-07-26] milestone | Phase 1 step 4: results + backfill + job list — MVP closed

Last step of Phase 1. The MVP goal is met: create a job from a template, edit it, Run, watch
the log live, see the final energy in the job list — no terminal.

**Result extraction (`src-tauri/src/result_extraction.rs`, new; +`regex` dep):**
`extract_final_energy` (regex `FINAL SINGLE POINT ENERGY\s+(-?[\d.]+)`, last match = converged)
and `extract_wall_time` (parses ORCA's `TOTAL RUN TIME:` line → seconds). Regexes compiled once
via `LazyLock`. `drive_job` runs them on a 64 KB output tail when a job completes (64 KB, not the
5 KB completion tail, because Freq/Opt prints the final energy well before EOF) and stores them
via new `set_job_results_conn` **before** the terminal `job:status` event.

**Output backfill:** new `read_job_output(id, tail_lines?)` command → `read_tail_lines`
(reads ≤ 8 MB from the end, drops partial head line, caps 10 000 lines — never loads whole
files). `JobDetailScreen` calls it after attaching listeners for non-draft jobs, so opening a
finished job shows the full log (not "Waiting…"). `open_job_folder(id)` spawns the file manager
(`xdg-open`).

**Frontend polish:** `format.ts` (energy 6 dp, wall time `35.4s`/`2m 15s`/`1h 5m`, timestamp).
Jobs list gained Energy (Eh) + Time columns; Job detail shows energy/time line + Open Folder.

**Verified:**
- `cargo test` — 19 green + 1 ignored (added 5 extraction tests, `read_tail_lines` test).
  `cargo build` clean, no warnings. `tsc` + `vite build` clean.
- **Real ORCA e2e** (`real_orca_water_single_point_completes`, `--ignored`) extended: now also
  asserts `extract_final_energy` ≈ -76.419 Eh and `extract_wall_time` is `Some` against genuine
  ORCA output, and `read_tail_lines` returns the full log. Passes (~0.6 s).
- **Not verified in-GUI** (same headless limitation as Phase 0/1.3): the live in-app flow
  through the real webview (backfill rendering, energy appearing in the list after a Run, Open
  Folder launching the file manager). All backing commands are unit-/e2e-tested; IPC is standard.

**Phase 1 (MVP) closed.** Remaining known gaps carried into later phases: startup reconciliation
of interrupted `running` jobs; cancel/kill; backfill parsing of energy/time for jobs that
completed *before* this step existed (only new runs get results); authoritative cclib parse
(sidecar tier). Small accepted quirk: opening a *running* job can briefly duplicate log lines
(backfill vs live overlap).

Next (Phase 2): molecules & input UX — or first, startup reconciliation to harden the MVP.

## [2026-07-27] session | Phase 2 step 1: 3Dmol.js viewer component

First Phase 2 step — a live 3D molecule preview on New Job. `npm install 3dmol` (v2.5.5; ships
its own TS types, no custom `.d.ts` needed). New `src/viewer/`:

- **`MoleculeViewer.tsx`** — 3Dmol React component. Props `xyzData` + optional `style`. One
  `createViewer()` per mount (into a `useRef` div, `#0d0f13` background); a second effect
  re-renders on `xyzData` change (`removeAllModels` → `addModel(xyz,"xyz")` →
  `setStyle({},{stick:{},sphere:{scale:0.3}})` ball-and-stick → `zoomTo` → `render`). `ResizeObserver`
  → `viewer.resize()`. Cleanup on unmount: `viewer.clear()` + null refs (WebGL context release).
- **`parse-xyz-from-input.ts`** — `extractXyzFromInput` scans the `* xyz … *` block → standard
  xyz; `null` for the `* xyzfile … file.xyz` form and for no-coordinates.
- **`3dmol-setup.ts`** — side-effect module (imported by `MoleculeViewer`) neutralising
  `OffscreenCanvas`. See the WebGL bug below.
- **`NewJobScreen`** — split panel right of the editor; editor content parsed on a 500 ms debounce
  → molecule or muted "No coordinates in input". CSS: `.editor-viewer-split` (editor `flex:2`,
  viewer `flex:1 min-width 260px`) + panel styling in `app.css`. `useState` only, no Zustand.

**WebGL bug (the visualization.md watchpoint, hit and resolved — `debugging/002`):** 3Dmol worked
in Chromium but threw `TypeError: null is not an object (evaluating 'this._gl.clearDepth')` in the
WebKitGTK webview. Root cause: 3Dmol's `initGL` prefers an OffscreenCanvas+`webgl2` path;
WebKitGTK exposes `OffscreenCanvas` but returns `null` for `webgl2` on it → null GL context. Fix:
`window.OffscreenCanvas = undefined` before the first `createViewer` forces 3Dmol's working
direct-canvas branch (safe — we only show single molecules). **Reusable verification technique**
discovered: the Tauri GUI can't be driven headlessly (no xdotool), but its engine is
`libwebkit2gtk-4.1`, whose `MiniBrowser` (`/usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/MiniBrowser`)
runs a standalone probe HTML against the real `3dmol` build — reproduced the failure and confirmed
the H₂ fix in the identical engine via window-title + `gnome-screenshot`.

**Verified:** `tsc --noEmit` + `vite build` clean; Chromium (`vite dev`) — template → H₂ renders,
clear editor → "No coordinates", zero console errors; H₂ dumbbell rendered in webkit2gtk-4.1
MiniBrowser with the fix; the real Tauri window renders the split layout (sidecar healthy, ORCA
configured). Not directly seen: the molecule *inside the Tauri GUI* (needs coords, no input
automation) — covered by the identical-engine MiniBrowser render.

Decision: layout is split-view (editor beside preview), not viewer-below-editor — editing coords
next to a live 3D view is the natural pairing and preserves editor height. Bundle grew ~4 MB
(3dmol); acceptable for a local app, code-split later.

Next: extend to a Molecules screen / xyz import, and the JobDetailScreen result-geometry viewer
(deferred from this task); atom picking + trajectory playback are Phase 3.

## [2026-07-27] decision | ADR-007: from molecular modeling to reaction modeling

Strategic expansion of OrcaStudio's mission. Key decisions:

- **Reaction** (not Job) becomes the central intellectual object. A Reaction contains
  Pathways, each generating Jobs along a reaction coordinate sweep.
- New domain objects: Molecule, Fragment, ReactionCenter, Constraint, ReactionCoordinate,
  Pathway, Reaction.
- **Phase 2.5** (Geometry editor): atom picking, measurement, set distance/angle/dihedral,
  fragment library, constraint manager → `%geom`, xTB constrained pre-optimization.
- **Phase 4.5** (Reaction modeling): reaction/pathway data model, reaction coordinate editor,
  parametric sweep, batch job orchestration, comparative energy profiles.
- Phase 6 gains multi-step mechanisms (catalytic cycles) and AI-assisted reaction setup.
- Non-goal updated: not a free-hand molecular drawing tool; structure creation via import +
  fragment placement + geometric manipulation.

Motivated by author's research experience: stereoselectivity proof for NaBH₄ reduction
required precise control over Bürgi-Dunitz attack angle and stereofacial approach — 
impossible in existing tools (Avogadro, Chimera, GaussView).

All current Phase 2 work remains valid and unchanged — the new phases are additive.

## [2026-07-27] session | Phase 2 step 2: xyz import + SMILES → 3D

Two ways to load a molecule into the ORCA input, both feeding the Phase 2.1 viewer automatically.

**Sidecar — first chemistry endpoint (`app/smiles.py`, RDKit):** `POST /smiles-to-3d`
(`{smiles}` → `{xyz, formula, charge, multiplicity, num_atoms}`). Pipeline: `MolFromSmiles`
(→400 on invalid) → `AddHs` → `EmbedMolecule(ETKDGv3())` with a `useRandomCoords` retry (→422 if
still failing) → `MMFFOptimizeMolecule` (non-convergence tolerated) → `MolToXYZBlock` +
`CalcMolFormula` + `GetFormalCharge`; multiplicity hardcoded 1 for now. `pip install rdkit`
worked directly (modern wheel `rdkit==2026.3.4`; `rdkit-pypi` fallback NOT needed) — venv
recreated. Spec-vs-API fix: `useRandomCoords` is a property of the ETKDGv3 params object, not a
kwarg. `pytest` 5/5 green; `curl`-verified (`O`, `[O-]`→−1, invalid→400).

**Frontend:** `src/viewer/inject-xyz-into-input.ts` — `injectXyzIntoInput` replaces an existing
`* xyz|xyzfile … *` block or appends one, preserving the rest of the input. `NewJobScreen` gains
a one-line import row: hidden `<input type=file accept=.xyz>` (no tauri-plugin-dialog) →
`FileReader` → local `xyzToAtomLines` validation → inject (charge 0, mult 1, title from filename);
and a SMILES field + Generate 3D → `get_sidecar_status` → `fetch /smiles-to-3d` → inject with
RDKit's charge (title from formula). Error `detail` surfaced in the banner; `Generating…` disables
the button. Still `useState` only.

**Verified** in Chromium: methane `.xyz` appends + renders; `CCO` → ethanol replaces the block;
`xxx` → "Invalid SMILES" (no change); `[O-]` → `* xyz -1 1`. `tsc`/`vite build` clean, no console
errors. SMILES happy path tested against the live sidecar by stubbing only
`window.__TAURI_INTERNALS__.invoke` to hand back the running port (plain-browser `invoke` is
otherwise unavailable; the GUI still can't be driven headlessly — endpoint itself is pytest+curl
covered).

NOT done (deliberately, per task): Molecules screen, SQLite molecule library, Zustand — those are
Task 2.3. Multiplicity is always 1 (radicals/triplets later).

Next: molecule library + Molecules screen (Task 2.3), then the input builder form.

## [2026-07-27] decision | ADR-007 revision: native scan, ASE kernel, ΔE‡ precision

Four corrections to ADR-007 and ROADMAP based on domain review:

1. **Native ORCA scan over N-job orchestration.** Pathway sweep uses `%geom Scan`
   (one job per pathway), not N separate constrained jobs. Wavefunction + geometry
   chaining between scan points is critical for correct energy profiles.
2. **ΔE‡ not ΔG‡.** Scan maximum gives electronic energy barrier (ΔE‡). For
   publication-quality ΔG‡, need OptTS + Freq (TS refinement step). Phase 4.5
   targets ΔΔE‡ as screening; TS refinement is a late/Phase 6 step.
3. **ASE for geometry kernel.** `set_distance/angle/dihedral` with masks, not custom
   trigonometry. Fragment placement = one-vector attachment (Bürgi-Dunitz case).
4. **Job queue moved earlier.** Sequential queue (queued status + worker loop) added
   to Phase 2 — needed for daily use, not just reaction modeling. Removed from Phase 5.

Also: deduplicated Phase 6 (relaxed scans → 4.5, xTB → 2.5), added UFF fallback note
for exotic fragments, added numerical-control-not-drag-editing decision.

## [2026-07-27] session | Phase 2 step 3: molecule library in SQLite

A molecule becomes a persistent object: save it, browse it, reuse it in a future job.

**Rust core:** SQLite migration **v3** (`db.rs`, `SCHEMA_VERSION` → 3) adds a `molecules` table
(`id` UUID, `name`, `formula`, `xyz` full standard string, `charge`/`multiplicity` INTEGER,
`tags` comma-separated, `created_at`). `migrate()` is forward-only to `SCHEMA_VERSION`, so v1/v2
DBs jump straight to v3 (the old `migrate_v1_to_v2_preserves_settings` test now asserts the final
version is `SCHEMA_VERSION`). New `Molecule` model (`models/molecule.rs`, same `COLUMNS`/`from_row`
pattern as `Job`) and five CRUD commands (`commands/molecules.rs`): create/list/get/update/delete,
thin wrappers over `*_conn` helpers, `NotFound` on missing ids. **Molecules are NOT linked to
`jobs`** — no `molecule_id` FK; that association is Phase 4.5 (reaction modeling). New tests:
migration v2→v3 preserves jobs + molecule CRUD×4. `cargo test` 24 green.

**Frontend:** new **Molecules** tab/screen (`MoleculesScreen.tsx`) — list in a `.jobs-table`
(Name/Formula/Charge/Tags/Created + Use/Delete), row-click detail panel with a `MoleculeViewer`,
empty state, and an inline Add form (Name, Charge/Mult, Import .xyz or SMILES→Generate 3D via the
sidecar, Tags, live preview, Save → `create_molecule`). `App.tsx` gains the tab, a
`{kind:"molecules"}` screen, and `new-job`'s optional `initialMolecule`. **Use** sends a molecule
to New Job (injects xyz + charge/mult, seeds title); **Save to Library** on New Job persists the
editor's current coordinates (`extractXyzFromInput` + new `parseChargeMult`, formula carried from
the last SMILES gen). Shared xyz helpers extracted to `viewer/xyz-format.ts`
(`xyzToAtomLines`/`atomLinesToXyz`/`parseChargeMult`). Still `useState` + `invoke`, no Zustand.

**Verified** in Chromium (`vite dev`, real sidecar :8765, `invoke` stubbed with an in-memory
molecule store): empty state → Add → `CCO` → Generate 3D renders ethanol → Save → row appears;
detail viewer; Use → New Job with coords + preview; Save to Library → banner + second row (formula
`C2H6O` carried); Delete removes it. `tsc` + `vite build` clean, no console errors. GUI still can't
be driven headlessly (same limitation as prior phases); Rust CRUD is cargo-covered.

Next: input builder form, then the sequential job queue (moved into Phase 2 per the ADR-007 revision).

## [2026-07-27] session | Phase 2 step 4: ORCA input builder form

Dropdowns → valid `.inp`, still editable in Monaco. Replaces having to know ORCA syntax by heart.

**New `src/input-builder/` module:** `orca-options.ts` (data-only keyword catalogs — job types,
composite/3c methods, functionals grouped by rung, def2 bases, dispersion, RI, solvation models +
curated 20 solvents, SCF conv; all vs ORCA 6.1). `build-input.ts` — pure `buildKeywordLine` +
`buildOrcaInput(state, atomBlock)` encoding the domain rules: **composite methods emit only their
name** (no basis/dispersion/RI — disabled in the UI), **RI auto-pairs an aux basis** (`def2/J` for
RIJCOSX/RI-J, `def2/JK` for RI-JK on def2 bases), canonical keyword order, `%maxcore` no `end` /
`%pal` with `end`, `MODEL(solvent)` syntax, coordinates preserved verbatim (form charge/mult drive
the header). `InputBuilderForm.tsx` — collapsible panel on New Job with a composite-vs-functional
radio toggle, disabled solvent select in gas phase, and a **live `!`-line preview** (the learning
element). Seeds charge/mult from the current `* xyz c m` header via `parseChargeMult`; the `!` line
is never parsed back (form→text one-way, per ROADMAP).

**Tooling:** added **vitest** (`npm test` → `vitest run`). `build-input.test.ts` — 9 tests
(composite self-sufficiency, RIJCOSX→def2/J, RI-JK→def2/JK, CPCM(water), gas-phase omits solvation,
maxcore/pal end rules, verbatim coords, form charge/mult). CLAUDE.md's `npm test # frontend` line is
now real.

**Verified** in Chromium (real sidecar, invoke stubbed): default `! r2SCAN-3c Opt Freq TightSCF`;
Functional mode `! B3LYP def2-TZVP def2/J RIJCOSX D4 Opt Freq TightSCF`; CPCM → `…CPCM(water)…` +
solvent select enables; SMILES `CCO` → Generate 3D → **Generate Input** → full `.inp` with the new
`!` line, `%pal…end`/`%maxcore` (no end), and 9 ethanol atoms preserved verbatim (read back via the
stubbed `create_job`). `tsc` + 9 vitest + `vite build` clean, no console errors. The checklist's
real-ORCA-run leg needs the Tauri backend + ORCA binary (not drivable headlessly, same limitation as
prior phases); the generator's exact output is byte-covered by vitest.

NOT done (deliberate): reverse parse (input→form), double-hybrid AuxC logic, scan/NEB options
(Phase 4.5), the full 179-solvent list.

Next: live convergence dashboard for Opt jobs, then the sequential job queue.

## [2026-07-27] session | fix: don't double-count dispersion for built-in-D4/VV10 functionals
Some functionals bake the dispersion correction into the name — `wB97X-D4` (`-D4`) and `wB97M-V`
(VV10 `-V`). The builder was still appending the separate `dispersion` token, double-counting the
correction (`! wB97X-D4 def2-TZVP D4 …`).

**Fix:** `OrcaOption` gains `builtInDispersion?: boolean`; `wB97X-D4` and `wB97M-V` marked true.
New `functionalHasBuiltInDispersion(functional)` in `build-input.ts` looks the functional up in
`FUNCTIONAL_GROUPS`; `buildKeywordLine` skips `state.dispersion` when it returns true. Form disables
the Dispersion dropdown with the hint "Included in the functional" for such functionals.

`input-format.md` Rule 1 broadened: "Composite methods and dispersion-inclusive functionals are
self-contained." New vitest case: `wB97X-D4` + `D4` → `wB97X-D4` appears exactly once, no standalone
`D4` token. 10 vitest + `tsc` clean.

## [2026-07-27] session | UI fixes: WebKitGTK select contrast, accordion panels, scrollable New Job
Three New Job issues found by manual testing in the real Tauri window (invisible in Chromium).
CSS + a collapse wrapper only — no Input Builder / `build-input.ts` / `orca-options.ts` logic touched.

**1. `<select>` dark-on-dark (WebKitGTK).** Native GTK select widgets ignore the inherited `.input`
color → dropdown values rendered near-black on dark. `styles/app.css` `.select` now
`-webkit-appearance:none` + explicit color/background + inline-SVG chevron; `.select:disabled` and
`.input[type=number]` pinned dark too. `option` popup is a native menu → styling best-effort.
New `debugging/003-webkitgtk-select-styling.md` (symptom→cause→fix→lesson), the 2nd WebKitGTK gotcha
after 002. **Verified in `webkit2gtk-4.1 MiniBrowser`** (Tauri's engine) via probe HTML +
`gnome-screenshot`: values light-on-dark, disabled muted-dark, chevron present, numbers readable.

**2. Couldn't scroll to the editor.** `.screen.new-job` `overflow:hidden` + editor `flex:1` squeezed
Monaco out when panels expanded. Now `overflow-y:auto`; `.editor-viewer-split` `flex:none;height:420px`.

**3. Templates → accordion.** Templates is now a collapsible `.input-builder` section like the Input
Builder. `NewJobScreen`: `builderOpen` boolean → one `openSection: "builder"|"templates"|null`
(default null — both closed, editor+viewer visible). Sections mutually exclusive; picking a template
or Generate Input collapses the accordion.

`tsc` + `npm test` (10) + `vite build` clean. Live accordion/scroll + open-popup `option` styling
need the real Tauri window (GUI not headless-drivable, same as prior phases).

Next: live convergence dashboard for Opt jobs, then the sequential job queue.

## [2026-07-27] ingest | ORCA parallel scaling measured on dev machine

Controlled benchmark: 39-atom scaffold, 640 basis functions, RIJCOSX hybrid DFT SP,
30 runs (3 repeats × 10 core configurations), thermal cooldown between runs,
constant %maxcore, taskset pinning. All 30 runs converged in 12 SCF cycles —
identical work, valid comparison. Spread <1%.

**Two prior assumptions refuted:**
- "Avoid mixed P+E core sets" — FALSE. ALL12 (114.8 s) is fastest, 28% ahead of P4.
- "Hyperthreading hurts ORCA" — FALSE. HT8 (134.8 s) beats P4 (146.7 s) by 8%.

**Key finding:** E8 (E-cores only, 156.7 s) is just 7% slower than P4 but runs at
74 °C instead of 97 °C and leaves all P-cores free → best default for a machine
that is also used for development.

Presets adopted: Interactive = `taskset -c 8-15`, nprocs 8 (default);
Max throughput = `taskset -c 0,2,4,6,8-15`, nprocs 12.

Also recorded: memory ceiling on nprocs (15 GB RAM / 1 GB swap), benchmark
methodology (the first attempt was invalid — 8-atom ethane, 4.8 s runs measured
MPI startup, not scaling), and the E/P core equivalence factor of 0.53.

New: `wiki/orca/performance.md`. CLAUDE.md gains domain rule 8 (explicit core
pinning). ADR-007 gains a mandatory `! XTB GOAT` conformer-ensemble step before
pathway construction — single-conformer optimisation is not reproducible science.
Phase 5 restated as a requirement rather than a convenience.

## [2026-07-27] session | LocalBackend: CPU pinning, job queue, cancel

Made local execution fit for real work — three parts, one commit.

**1. CPU pinning (domain rule #8).** New `cpu_presets.rs` with measured presets
(`interactive` = `taskset -c 8-15`, 8 ranks, default; `max_throughput` =
`0,2,4,6,8-15`, 12 ranks) — masks are i5-12500H-specific, loudly documented, no
auto-detection. `resolve_cpu_config` reads `cpu_preset`/`cpu_mask`/`cpu_nprocs`
from settings (falls back to interactive). `run_orca` now takes an
`Option<mask>`: with one it spawns `taskset -c <mask> orca …` with
`OMPI_MCA_hwloc_base_binding_policy=none` (so taskset + OpenMPI don't fight);
missing `taskset` → a clear error. `align_pal_nprocs` rewrites `%pal nprocs N end`
to match the pinned core count (oversubscribing is 3× slower) and logs an info
line so it isn't silent. Settings screen gained a CPU section.

**2. Sequential queue — in SQLite, no worker thread.** The queue *is* the set of
`status='queued'` jobs; `try_start_next` pulls the oldest when the slot frees
(after enqueue / after each finish / on resume). `submit` now enqueues and
**never** errors on a busy slot. `JobRunner` holds one slot + an `AtomicBool`
pause flag. Pause is queue-only — the running job finishes; we deliberately do
NOT SIGSTOP it (frozen RAM + MPI ranks may not resume). New statuses `queued`
and `cancelled` (TEXT column, no migration).

**3. Cancel — killpg the process group.** Spawn with `process_group(0)` so ORCA
+ all MPI ranks share one pgid; `cancel` does SIGTERM → 5 s → SIGKILL via
`libc::killpg` (queued jobs just drop). A `cancelled` `AtomicBool` lets
`drive_job` record `cancelled` instead of `failed`. Added `libc` (Unix dep).

**Bonus: startup reconciliation.** `reconcile_on_startup` advances every job left
`running` by a crash (finalize if its dir shows a finished run, else `failed`);
`queued` jobs resume via a startup `try_start_next`. Closes the Phase 1 gap.

**Verified.** `cargo test` 32 green (new: 5× `align_pal_nprocs`, 3×
`resolve_cpu_config`); ignored real-ORCA e2e still passes. **Real ORCA, headless:**
benzene B3LYP/def2-SVP `%pal nprocs 4` under the exact rule-8 command line — all 5
ORCA processes pinned to cores 8–15, one shared PGID, `kill -TERM -<pgid>` reaped
the whole tree. `tsc` + 10 vitest + `vite build` clean. The full in-GUI legs
(3-job queue draining, Cancel button, htop core loading, Pause/Resume) still need
the real Tauri window — not headless-drivable, same limitation as prior phases;
the backend mechanics are unit- + real-ORCA-covered.

**Graceful stop** (stop-after-cycle marker file) was investigated but NOT
implemented: the ORCA 6.1 manual isn't indexed locally yet (Phase 4), so it
couldn't be confirmed. Only hard kill ships; recorded in `gotchas.md` to re-check
once the manual lands. Domain rule 4 (concurrency = 1) untouched.

Next: live convergence dashboard for Opt jobs.

## [2026-07-28] session | Phase 2 step 5: live convergence dashboard

The learning instrument from CLAUDE.md, realised: during a run the user watches an energy-per-
cycle plot and the convergence criteria update, instead of squinting at the text log.

**Rust — incremental parser (`src-tauri/src/convergence.rs`, new).** `ConvergenceParser::feed(line)
-> Option<ConvergenceEvent>` (`Scf(ScfPoint)` | `Opt(OptPoint)`, `#[serde(tag="kind")]`). Fed the
**same** stdout stream `drive_job` already tails — `output.out` is never re-read while running
(domain rule #5). `drive_job` batches events on the log cadence and emits `job:convergence`; new
`read_job_convergence` command replays the file line-by-line via `BufReader` for backfill (never
whole-file). **Not stored in SQLite** — parsed on demand.

**Format reality vs the task's idealised spec** (all confirmed against real ORCA 6.1 r²SCAN-3c
output, now in `orca/output-files.md`): there is **no `SCF ITERATIONS` banner** — ORCA 6 prints
per-algorithm headers (`D-I-I-S`, `S-O-S-C-F`) with a continuous iter counter; iter starts at 1;
ΔE is scientific and `0.00e+00` on iter 1. First opt cycle has **4** criteria (no `Energy change`
yet), later cycles 5 → count not hardcoded. **Key gotcha:** Freq normal-mode eigenvector rows have
the *identical* shape to SCF rows (`int  -0.000014  0.048084 …`) — pure line-shape parsing leaks
them in as bogus SCF points. Fix: gate SCF parsing to inside an `Iteration … Energy (Eh)` table
(closed by `SCF CONVERGED` / `TOTAL SCF ENERGY`). Per-cycle energy reuses
`result_extraction::extract_final_energy`.

**Frontend — `src/convergence/` + Job detail.** `ConvergenceDashboard.tsx`: (A) progress indicator
(`cycle N · M/T criteria met` + chips, or `SCF iteration K` for SP); (B) energy-per-cycle
`LineChart` (6-dp Y ticks, ΔE-in-kcal/mol tooltip); (C) criteria-vs-tolerance chart on a **log Y**
with dashed tolerance `ReferenceLine`s. `job:convergence` listener attached **before** submit
(listeners-first, Phase 1.3), `read_job_convergence` backfill in the same order/guard as the log.
Accordion above the console, expanded while active. `npm install recharts` (v3.10).

**WebKitGTK / recharts.** recharts' `ResponsiveContainer` measures 0×0 in WebKitGTK (same webview
mismeasurement class as 3Dmol/`<select>`) — mitigated proactively with a `useContainerWidth`
ResizeObserver passing explicit pixel `width` to each chart; no `ResponsiveContainer`. SVG render
is low-risk otherwise.

**Verified.** `cargo test` 39 + 2 ignored (7 new convergence unit tests over a real-C₂H₆ fixture).
Parser validated against the two **real full outputs** on the dev machine
(`real_full_outputs_parse_sanely`, ignored): 4 and 7 opt cycles, **zero** Freq-eigenvector false
positives (all SCF energies < −1 Eh). `tsc` + `npm test` (10) + `vite build` clean, no warnings.
The live in-GUI leg (charts updating mid-run, backfill on reopen) needs the real Tauri window —
not headless-drivable, same limitation as every prior phase; the risky parsing is real-data-tested
and the frontend data path is fully typed.

Next: sequential-queue polish already shipped; Phase 2.5 geometry editor (atom picking, measure,
set distance/angle/dihedral) is the next big item.

## [2026-07-28] milestone | Phase 2 complete: molecules, input builder, convergence, conversions

Last Phase 2 item — molecular format conversion in the sidecar — and with it Phase 2 closes.

**Library — ASE, not Open Babel** (ROADMAP said Open Babel; overridden). ASE is already a sidecar
dependency (ADR-007 geometry kernel for Phase 2.5), installs as a pure-Python wheel (no system
binary, no flaky `openbabel-wheel` build), and covers every format we need. Open Babel stays a
fallback only for formats ASE lacks (e.g. mol2). `ase>=3.23` added to `requirements.txt`
(`pip install -r` pulled `ase==3.29` + numpy/scipy/matplotlib; no venv recreate). Decision recorded
in `modules/sidecar.md`.

**Endpoint (`sidecar/app/convert.py`).** `POST /convert`
(`{content, from_format, to_format}` → `{content, num_atoms, formula}`): content → tempfile
(`suffix=.{from_format}`) → `ase.io.read(format=…, index=-1)` → `ase.io.write` to a second tempfile
→ text back; both tempfiles cleaned in `finally`. `GET /formats` → `{read, write}` for UI dropdowns.
**Security whitelist:** ASE's full registry includes calc-package readers that can execute code /
read arbitrary files on parse — only plain structure formats are accepted, checked **before** ASE
sees the input. `WRITE_FORMATS` narrower than `READ_FORMATS`. **Gotcha:** ASE's PDB format name is
`proteindatabank`, not `pdb` — public keys mapped via `_ASE_FORMAT`. Errors: unknown fmt → 400,
unparseable → 422, 0 atoms → 422.

**Frontend — extended import, no new screen.** "Import .xyz" → **"Import file"** on both New Job and
Molecules, `accept=".xyz,.pdb,.cif,.mol,.sdf,.gen"`. Shared `src/viewer/import-file.ts`
(`importStructureFile` + `IMPORT_ACCEPT`) removes the duplicated per-screen handler: `.xyz` parsed
locally, other formats converted to xyz via `/convert` (port from `get_sidecar_status`), then the
common `xyzToAtomLines` path. Errors surface in each screen's existing banner; the editor/draft is
only touched on success.

**Verified.** `pytest` 11 green (6 new convert tests: xyz→pdb, pdb→xyz round-trip ≤1e-3, mol2→400,
garbage→422, `/formats`, empty→422). Live `curl` against a running sidecar: xyz→pdb text, bad
format→400, garbage→422, `/formats` lists both maps. `tsc` + `npm test` (10) + `vite build` clean.
The GUI import legs (pick a `.pdb` on New Job → viewer shows the molecule; broken `.pdb` → clear
error, editor intact) need the real Tauri window — not headless-drivable, same limitation as every
prior phase; the pdb→xyz path the GUI uses is exactly `test_pdb_to_xyz` + curl-covered.

**Phase 2 closed.** All items done: 3Dmol viewer (2.1), xyz/SMILES import (2.2), molecule library
(2.3), input builder (2.4), convergence dashboard (2.5), format conversion (2.6). Plus beyond the
original scope: sequential queue, CPU pinning, cancel, startup reconciliation.

## [2026-07-28] session | fix: MPI ranks escape process group on cancel

A latent orphan bug in cancellation, found by actually inspecting `ps` instead of trusting the
old "killpg reaps the whole tree" claim.

**Root cause (verified on real ORCA, `%pal nprocs 4`).** `process_group(0)` does **not** put the
MPI ranks in ORCA's process group — `mpirun` `setpgid`s each rank into its own group
(`PGID == PID`) so terminal signals can't reach them. `ps` proof: only `orca` + `sh` + `mpirun`
share the leader's group; the 4 `orca_leanscf_mp` ranks each have their own. `killpg` therefore
reaches mpirun but not the ranks. Cancel *appeared* to work only because a SIGTERM'd mpirun reaps
its ranks cooperatively — but on the SIGKILL path (heavy job that ignores SIGTERM — the exact
Cancel case) mpirun dies before forwarding, stranding N ranks on N cores forever. Reproduced live
by SIGSTOP-ing mpirun then killpg-ing the group: ranks survived, had to be reaped by PID. Full
writeup + ps evidence in `debugging/004-mpi-ranks-escape-process-group.md`.

**Fix (`local_backend.rs`).** cwd is the reliable membership signal — every job process has
`cwd` = job dir (confirmed `readlink /proc/<rank>/cwd` == job dir).
- `sweep_job_processes(job_dir, sig)` — signal every `/proc/<pid>/cwd`-match (skip self, skip
  unreadable). The safety net behind `killpg`.
- `terminate_job(pgid, job_dir)` (replaces `terminate_pgid`): `killpg(SIGTERM)` → wait ≤10 s
  (was 5 — heavy jobs may still be flushing `.gbw`) → `sweep(SIGTERM)` **before** any SIGKILL →
  2 s → `killpg(SIGKILL)` + `sweep(SIGKILL)`; `eprintln!` if the final sweep hard-killed anything.
- **Non-blocking cancel:** `cancel` spawns `terminate_job` off-thread and returns at once (it can
  take ~12 s; the `cancelled` flag already drives finalization). Same for the startup-race path in
  `start_run`. Frontend Cancel button → disabled "Cancelling…" until the terminal `job:status`.
- **App exit:** new `terminate_on_exit` runs the same routine **synchronously** from the `lib.rs`
  `ExitRequested` handler (a spawned thread would die before the ranks). Previously nothing killed
  a running job on exit — the ranks outlived the app.

**Verified.** `cargo test` 42 + 2 ignored (3 new: `sweep_job_processes_matches_cwd`,
`sweep_ignores_other_dirs`, `sweep_never_kills_self` — real `sh`/`sleep` children, no leaks).
`tsc` + `npm test` (10) clean. **Real ORCA (headless):** confirmed ranks escape the group (ps),
ranks' cwd == job dir (readlink), killpg-alone orphans them after SIGSTOP-ing mpirun, and the
cwd sweep reaps them. The in-GUI Cancel-button "Cancelling…" state is standard React (not
headless-drivable, same limitation as prior phases). Doc corrections: `execution-backends.md`
(the false process-group claim + SshBackend note), `orca/gotchas.md`, `debugging/004`.

Next: Phase 2.5 geometry editor.

Next (Phase 2.5): geometry editor — atom picking in 3Dmol, measurement, set distance/angle/dihedral
via ASE, fragment library, constraint manager → `%geom`.

## [2026-07-28] session | fix: queue pause control always visible

Small but real UX bug in the status-bar queue control. The Pause/Resume button only rendered when
`running > 0 || queued > 0`, so (a) you couldn't pause *ahead* of stacking jobs — the main "queue
work and walk away" flow — and (b) once the queue drained, the `paused` state became invisible and
the next job silently didn't start. Frontend-only; backend queue logic untouched.

**`App.tsx`:** the Pause/Resume button now **always** renders. Adaptive `queueLabel` — activity →
`1 running, 2 waiting`; empty + not paused → `idle`; empty + paused → `paused` (shown even with an
empty queue). New `.queue-paused` class (`var(--warn)`) highlights the indicator whenever paused so
it can't be missed. Button `title`s explain the semantics. `togglePause` now flips `queue.paused`
optimistically right after the successful `invoke` (label updates immediately, not after the
`refreshQueue` round-trip; refresh still runs in `finally` to reconcile counts).

**`JobsScreen.tsx`:** takes a `queuePaused` prop from `App`; a `queued` job's badge reads
**`queued (paused)`** with a "Queue is paused" tooltip while paused — so a stalled job shows why.

**Verified:** `tsc` + `npm test` (10) + `vite build` clean. Traced all label states (idle / paused /
running+waiting / created-while-paused). The live-in-GUI legs (empty-queue pause highlight, badge
reason, resume-starts-immediately) need the real Tauri window — not headless-drivable, same
limitation as prior phases; the change is presentational React over the unchanged, already-verified
`pause_queue`/`resume_queue`/`is_queue_paused` backend.

## [2026-07-28] session | Phase 2 step 7: output search with ORCA presets

Search an ORCA output for words, in-app — the last Phase 2 item (an addition beyond the original
plan, per the author). Outputs reach tens of MB, so previously the only way to find something was
an external editor.

**Rust — streaming search (`output_search.rs`, new).** `search_output(path, opts)` reads the file
line by line via `BufReader` — **never whole** (domain rule #5): it holds only a 2-line ring buffer
of context, the ≤2 matches awaiting trailing context, and the capped result list. Each match
carries 2 lines of context either side; single pass (a match stays pending until its trailing
context arrives, then finalizes in line order; leftovers flush at EOF). `MAX_MATCHES = 500` caps
returned matches while `total` counts every hit (`truncated = total > matches.len()`). Matcher:
regex (`RegexBuilder.case_insensitive`; invalid → `AppError::Backend`) or literal `contains` with
the needle lowercased once up front. Empty query → empty result. Commands `search_job_output` +
`get_search_presets`, registered in `lib.rs`.

**ORCA presets — the point of the feature.** One-click chips for what a chemist greps for
(warnings, errors, SCF-not-converged, imaginary modes, energies, geom convergence, timings, basis).
Wording **verified against 12 real outputs**, and two corrections came out of that (both in
`orca/output-files.md`):
- **`errors` had to become case-SENSITIVE** (`ERROR|error termination|aborting|ABORTING`): a
  case-insensitive `error` matches the benign `Last DIIS Error`/`Startup error` on every SCF — 12+
  hits in a *successful* run. The case-sensitive query fires **0×** across all 12. Required adding
  a per-preset `case_sensitive` field (deviation from the task's struct — justified by the check).
- **`imaginary` = `imaginary mode`**, not bare `imaginary` (which hits `imaginary perturbations`, a
  CPHF count in every Freq run). Confirmed it matches ORCA's real `***imaginary mode***` marker on
  a saddle-point output (`6:  -33.66 cm**-1  ***imaginary mode***`).

**Frontend — `OutputSearchPanel.tsx`.** Collapsible accordion above the log console (same pattern
as the convergence dashboard), collapsed by default, shown for non-draft jobs. Search box (Enter),
`regex` + `Aa` toggles, preset chips that fill+flag+search in one click. Results: bounded scrolling
monospace list with line numbers, highlighted match line + `<span class="hl">` on the occurrence,
muted context lines; `500 of N` header when truncated; `No matches` when empty. **No
search-as-you-type** (would kill the UI on 50 MB). No console jump-to-line (deferred — the console
holds only the tail; noted in ROADMAP).

**Verified.** `cargo test` 51 + 2 ignored (9 new search tests). `tsc` + `npm test` (10) +
`vite build` clean. **Real outputs (headless):** ran every preset over the 12 real jobs — `errors`
0 false positives (the DIIS-Error trap), `imaginary` 1 real `***imaginary mode***` hit, `energies`
= cycles+1, and **431 KB / ~8600 lines searched in ~3 ms** (memory bounded). The in-GUI legs
(chips, highlight, scroll, live-job search) need the real Tauri window — not headless-drivable,
same limitation as prior phases; the search engine + presets are real-data-verified and the data
path is fully typed.

**Phase 2 fully closed** (2.1–2.7 + queue/pinning/cancel). Next: Phase 2.5 geometry editor.

## [2026-07-28] session | output search: in-file navigation instead of excerpts

Reworked output search. The prior design showed atomised 5-line excerpts — the author tried it and
it was awkward. Replaced with real in-file navigation: search reveals the first hit in an actual
file view, prev/next step between hits, a `3 / 12` counter shows position. My earlier "don't jump
into the console" call was wrong; the right path wasn't windowing the `<pre>`, it was a **separate
Monaco file viewer** (already a dependency: virtualized rendering, line numbers, `revealLineInCenter`,
decorations).

**Rust.** New `read_job_output_for_viewer` (`commands/jobs.rs`): streams `output.out` into a
`VecDeque` capped at `MAX_VIEWER_LINES = 300_000` (≈30 MB), keeping the **tail** and reporting
`first_line_no` so the viewer shows absolute line numbers even when truncated — never loads a
hundreds-of-MB file whole (domain rule #5). `output_search.rs`: `OutputMatch` gains `col_start`/
`col_end` (1-indexed char range, exclusive end — Monaco semantics) via a matcher that now returns
the first hit's position; `SearchOptions` gains `context_lines` (viewer passes `0` → drops ~2500
excerpt lines from the payload at 500 hits). New tests `reports_match_columns` +
`regex_match_columns_point_at_first_hit`; existing context tests pass `context_lines: 2`.

**Frontend — two output modes on Job detail.** Live = the existing `<pre>` console (kept for
streaming; appending to a `<pre>` beats a Monaco model, autoscroll already tuned). Browse =
`OutputViewer` (Monaco, read-only, plaintext, `wordWrap:off`, minimap, absolute `lineNumbers`).
`OutputViewer` (forwardRef + useImperativeHandle) exposes `revealFileLine`/`setHits` backed by
`createDecorationsCollection()` (`.hit-all` + current `.hit-current`/`.hit-current-line`); calls
before Monaco mounts are buffered and flushed on mount. `OutputSearchPanel` rewritten: box +
regex/Aa + preset chips kept, excerpt list gone; a search runs with `context_lines:0`, auto-switches
to Browse, and drives the viewer via effects keyed on the viewer handle (no mount-timing race).
Prev/Next cyclic, `i+1 / total` counter (`/ 500 of 637` when truncated) + current line; Enter =
search / next-if-unchanged, Shift+Enter = prev, F3/Shift+F3 in panel or viewer (Monaco `addCommand`
→ `navRef`). **Lazy:** `read_job_output_for_viewer` only on first Browse entry, not on opening a
job; running jobs get a `Reload` + `snapshot at HH:MM:SS` (Reload bumps a `resetToken` that clears
stale hits).

**Verified.** `cargo test` 53 + 2 ignored (11 output_search incl. the 2 new column tests). `tsc` +
`npm test` (10) + `vite build` clean. Column math unit-tested; streaming search real-data-verified
(431 KB / ~8600 lines in ~3 ms). The in-GUI legs (reveal/decorate, prev/next, mode toggle, Reload,
large-file scroll) need the real Tauri window — not headless-drivable, same limitation as prior
phases; the data path is fully typed and Monaco is the already-proven editor engine.

## [2026-07-28] decision | Scene/fragment model for multi-molecule geometry (ADR-008)

Ingested ADR-008 (accepted). A **Scene** = ordered list of **SceneFragment**s — OrcaStudio's own
abstraction; ORCA never sees it. On export fragments merge into one flat `* xyz totalCharge mult ... *`
block; fragment identity lives only in our state and (as a snapshot) in the DB. This unblocks Phase 2.5:
reaction setup needs substrate + reagent in one scene with known boundaries, but the whole Phase 2
geometry path is single-fragment (one xyz, one `* xyz *` block, import *replaces* geometry).

Key calls: `SceneFragment` not `Fragment` (React.Fragment collision); **one 3Dmol model** styled by
atom index range, not one model per fragment (single index space: pick = merged-xyz = ASE mask);
`atom.index` not `atom.serial`; canonical merged-xyz serializer; electron-parity validation from Σ Z − charge;
curated fixed-geometry fragment library with provenance (no runtime RDKit — MMFF lacks BH₄⁻ params);
Zustand store wrapping React-free pure functions; ORCA `(1)`/`(2)` annotation out of scope.

**Two positions changed during the discussion:**
- *Persistence:* in-memory-only → a **`scene_json` snapshot** (nullable TEXT, schema v4, versioned JSON).
  The Phase 2.5 TS-guess workflow is iterative (build → run → adjust angle → rerun); without a snapshot,
  cloning a job yields one flat fragment and the user re-splits by hand every iteration. NOT relational
  tables — Phase 4.5's Reaction/Pathway schema replaces it; migration = read JSON, expand into rows.
- *Reset rule:* "reset Scene to one fragment on any manual edit" → **coordinate-block-only, with float
  tolerance and Undo**. Compare parsed floats at tolerance 1e-6 (never string compare — formatting differs);
  reset only when the coordinate block actually changed, backed by an Undo notification (previous Scene in a ref).

**2.5.0a starts from:** pure core only — Scene/SceneFragment types + pure functions (merge, index mapping,
immutable updates, serialization, float-tolerant comparison) + tests, zero React. Then 2.5.0b (input builder +
electron parity) and 2.5.0c (multi-fragment viewer) in parallel; 2.5.0d adds the Zustand store + `scene_json`
migration. No code this session — ingest only.

## [2026-07-28] session | 2.5.0a: Scene/fragment pure core

Built the pure core of the Scene model (ADR-008) as a new React-free module `src/scene/`
(`types.ts`, `scene.ts`, `scene.test.ts`). No new npm deps (zustand waits for 2.5.0d; no uuid —
`crypto.randomUUID()` is built in). See [modules/scene.md](modules/scene.md) for the full contract.

**What's in it.** `Scene` = ordered `SceneFragment[]` + system `multiplicity`. Functions: canonical
merge (`mergeToAtomLines` / `mergeToXyz`), aggregates (`totalCharge`, `atomCount`, `electronCount`,
`atomicNumber` H–Kr), index space (`globalIndex`, `fragmentAtomIndices`, `locateAtom`,
`fragmentRanges`), immutable mutators (`add/remove/rename/setFragmentCharge/setMultiplicity`,
`replaceFragmentAtoms`), parsing (`parseAtomLines`, `sceneFromAtomLines`), snapshot
(`serializeScene` / `deserializeScene`, version 1), and the reset primitive `xyzMatchesScene`.

**Invariants chosen and why.**
- *Index-space invariant:* `replaceFragmentAtoms` throws on any atom-count or element-sequence
  change. Geometry ops move atoms, never alter composition — that's exactly what keeps every atom
  index stable across an ASE call / xTB round-trip (the whole reason for one flat merged xyz).
- *`makeFragmentId()` is the ONLY impure function* (`crypto.randomUUID()`); everything else is
  deterministic, so tests pass literal ids. `sceneFromAtomLines` takes an optional `opts.id` for the
  same reason (deterministic "editor" path without stubbing the RNG).
- *`xyzMatchesScene` is float comparison, never string* (ADR-008 decision 6): parse both sides,
  compare elements case-insensitively + coords within `tol=1e-6`. Verified true when the same numbers
  are formatted differently (`0.0` vs `0.00000000`) and at 1e-7 drift; false at 1e-3, on changed
  element, on changed count, and on null.
- *`deserializeScene` never throws on user/DB data* — validates shape + version, returns null
  otherwise. `electronCount` / `replaceFragmentAtoms` / `globalIndex` DO throw (programming errors).

**Nothing diverged from ADR-008.** The one small addition not spelled out in the ADR: `opts.id` on
`sceneFromAtomLines` (determinism). Known `xyz-format.ts` / `parse-xyz-from-input.ts` overlap left
untouched and flagged in `scene.ts` + the module page for 2.5.0b to consolidate.

**Verified.** `tsc --noEmit` clean, `vite build` clean, `vitest run` **42 tests** (was 10 → +32 in
the new `scene.test.ts`). Pure arithmetic — no real ORCA needed. Next: 2.5.0b (Scene ↔ input builder,
total charge from fragments, electron-parity validation) and 2.5.0c (multi-fragment viewer) in parallel.

## [2026-07-28] session | 2.5.0b: Scene-driven charge/multiplicity + electron parity

Wired the 2.5.0a pure core into ORCA-input generation and added electron-parity validation.

**What was built.**
- `buildOrcaInput(state, geometry)` now takes `Scene | string | null`. A Scene supplies canonical
  merged coordinates (`mergeToAtomLines`) and **overrides** the header charge (`totalCharge`) and
  multiplicity (`scene.multiplicity`); a string is the old verbatim atom-block path; null →
  placeholder. `build-input.ts` imports the pure scene functions (still React-free).
- `src/scene/parity.ts` — `checkElectronParity(scene): ParityIssue | null`. Even electrons ⇒ odd
  multiplicity, odd ⇒ even; mismatch returns electron count + offending multiplicity + nearest-first
  valid list (`[1,3,5]`/`[2,4,6]`) + an **explanatory** message (teaching, not "invalid"). Returns
  null for an empty scene and — swallowing `electronCount`'s throw — for elements outside H–Kr.
- `src/scene/scene.ts` gained `sceneFromOrcaInput(content, opts)` — extracts the `* xyz c m ... *`
  block into a single-fragment Scene (charge + multiplicity from the header; null for `* xyzfile`).
- `InputBuilderForm.tsx` is Scene-driven: derives a Scene from the buffer via `sceneFromOrcaInput`,
  Charge read-only = `totalCharge(scene)` with a "Σ of N fragments" caption, Multiplicity stays
  editable and is written into the Scene before generate, inline parity warning under the numeric
  row (never blocks Generate). CSS: `.builder-charge-note`, `.builder-parity` (uses `--warn`).

**Deviation 1 — buildOrcaInput signature is `Scene | string | null`, not the literal `Scene | null`
from the task.** Reason: two of the ten existing `build-input.test.ts` cases pass a raw atom-block
*string* as the second arg. A strict `Scene | null` would fail them at compile time, contradicting
the hard requirement "all 10 pass without changes". The union keeps the string branch (old behaviour
verbatim) and adds the Scene branch. All 10 tests pass **with the test file untouched** (`git diff`
on it is empty); nothing was edited.

**Deviation 2 — consolidation narrowed from ADR-008 (flagged so lint doesn't read it as drift).**
ADR-008 said 2.5.0b would fully consolidate the viewer parsers (`xyz-format.ts`,
`parse-xyz-from-input.ts`) into `src/scene/`. 2.5.0b migrated **only `InputBuilderForm.tsx`**;
`NewJobScreen.tsx` and `MoleculesScreen.tsx` still use the viewer helpers, which are untouched.
Reason: both screens are rewritten in 2.5.0d (Add Fragment UI + Zustand), so consolidating their
call sites now means rewriting them twice. `sceneFromOrcaInput` therefore duplicates a little of
`extractXyzFromInput`/`parseChargeMult` in the interim; **2.5.0d removes the viewer copies** — that is
where ADR-008's "full consolidation" lands. Recorded in `scene.ts`, `modules/scene.md`, ADR mapping.

**Design note — charge is now derived from the buffer.** With a Scene present, the form no longer
lets you type a charge; it reflects the sum of fragment charges (in 2.5.0b that is the single
fragment parsed from the `* xyz c m` header). To change charge in 2.5.0b you edit the header in
Monaco (the buffer owns the coordinate block per ADR-008 #6); the per-fragment charge editor is
2.5.0d. Also: generating from a Scene re-canonicalises coordinates to `toFixed(8)` (was verbatim) —
float-stable, intended by ADR-008 #4.

**Manual check (author, in the real Tauri window — not headless-drivable):**
1. Open a job whose input has a `* xyz 0 1` water block → open the Input Builder. The **Charge
   field is greyed/read-only** showing `0`, with a small "Σ of 1 fragment" caption beneath it;
   Multiplicity stays editable.
2. Set **Multiplicity to 2** → a yellow inline warning appears under the numeric row, reading roughly
   "This scene has 10 electrons (even), so its spin multiplicity must be odd — singlet (1),
   triplet (3), quintet (5). Multiplicity 2 (doublet) has the wrong parity …". Generate is **still
   enabled** — click it and confirm the `* xyz 0 2` header is written anyway (warning, not block).
3. Set Multiplicity back to 1 (or 3) → warning disappears. Clear the coordinate block entirely →
   Charge becomes an editable number field again (no Scene).

**Verified.** `tsc --noEmit` clean, `vite build` clean, `vitest run` **56 tests** (was 42 → +10
parity, +4 `sceneFromOrcaInput`). The 10 `build-input.test.ts` cases pass unchanged (file diff
empty). Next: 2.5.0c multi-fragment viewer, 2.5.0d store + `scene_json` + finish the consolidation.

## [2026-07-28] session | 2.5.0c: multi-fragment rendering in MoleculeViewer

Taught `MoleculeViewer` to draw a multi-fragment Scene. Viewer only — no new UI, state, or store.

**What was built.**
- `MoleculeViewer` props extended to `{ xyzData?: string; scene?: Scene; style? }`. `scene` wins
  when both are present; neither → empty viewer, no crash. Existing `xyzData` callers
  (`MoleculesScreen` ×2, `NewJobScreen`) are untouched and render byte-for-byte as before (legacy
  path keeps its always-`zoomTo`).
- **One model, index-range styling** (ADR-008 #2/#3): coords from `mergeToXyz(scene)`; base
  ball-and-stick CPK on all atoms; then fragments 1+ overridden with a flat palette colour on both
  stick and sphere via `setStyle({index:[…]}, …)`. Fragment 0 stays CPK (substrate must not
  recolour — hard requirement).
- `src/viewer/fragment-colors.ts` — `FRAGMENT_PALETTE` (teal/coral/gold/violet) + `fragmentColor(i)`
  (undefined for 0, cycling for 1+). Single source of truth, shared with the 2.5.0d sidebar.
- **`zoomTo` only on composition change**: a ref holds a signature of `id:size` per fragment (not
  coordinates); `zoomTo` fires only when it changes. Coordinate-only edits redraw without moving the
  camera — required for the 2.5.3 angle-tweak loop.

**Selector grounded in the types, not guessed.** `AtomSelectionSpec.index` is typed
`number | number[]` and `StickStyleSpec.color` / `SphereStyleSpec.color` are `ColorSpec`
(`number | string | Colored`) — so `{index:[…]}` + hex string colour are fully typed. No
`@ts-ignore` anywhere.

**WebKitGTK verification (mandatory — the headless-invisible failure mode).** vitest can't catch a
colour that silently doesn't apply, so I ran the `debugging/002` MiniBrowser technique
(`webkit2gtk-4.1/MiniBrowser`, the identical engine to Tauri's webview) on a standalone probe: water
(atoms 0–2) + BH₄⁻ (atoms 3–7) ~5 Å apart, the exact `createViewer/addModel/setStyle({index})/render`
calls, then it read each atom's `.style.stick.color` back. Result **ALL PASS**: (a) WebGL context
created (OffscreenCanvas neutralisation holds); (b) 8 atoms in one model, both fragments visible;
(c) the coloured index set was exactly `[3,4,5,6,7]` — **not** `[0,1,2,3,4]`, confirming
`fragmentRanges` end-exclusive has no off-by-one; (d) fragment 0 kept CPK. Screenshot showed water in
CPK (red O / white H) on the left and BH₄⁻ in teal on the right. So the `index` selector that all of
2.5.1 picking depends on is confirmed in the real engine, not just Chromium. Probe left in
`/tmp/frag-probe/` (throwaway).

**Manual check (author, real Tauri window — once 2.5.0d threads a Scene into the viewer):** a scene
with substrate + a reagent fragment shows the substrate in normal CPK element colours and each added
reagent in a distinct flat palette colour (teal, then coral, …); rotating/zooming works as before;
applying a coordinate change to a fragment (2.5.3) redraws **without** the camera jumping, while
adding/removing a fragment re-zooms to fit.

**Verified.** `tsc --noEmit` clean, `vite build` clean, `vitest run` 56 tests (unchanged — the
rendering path isn't headless-unit-testable; its inputs `mergeToXyz`/`fragmentRanges` are already
covered, and the engine behaviour was proven in MiniBrowser). Next: 2.5.0d — Zustand store +
`jobs.scene_json` + Add Fragment UI (and finish the viewer-parser consolidation).

## [2026-07-28] session | 2.5.0d-1: scene store as geometry source of truth on New Job

Made the Scene the source of truth for geometry on New Job — geometry flows through a Zustand store
synced two-way with the Monaco buffer, not through the raw `content` string. **Acceptance criterion
was "no visible behaviour change"**: Import / SMILES / Use / Save to Library / Generate / live
preview all behave as before; only the plumbing changed.

**Split 2.5.0d into d-1 / d-2 / d-3 — deliberate deviation from ADR-008 (reason: scope).** ADR-008
listed 2.5.0d as one item ("Add Fragment UI + Zustand + scene_json"). That's three separable
concerns and one session's worth is the store + sync + consolidation. So: **d-1** (this) store +
Scene↔Monaco sync + New Job on the store + close the parser consolidation; **d-2** the multi-fragment
Add-Fragment sidebar; **d-3** `jobs.scene_json` persistence (schema v4). ROADMAP updated to match.

**Store (`src/scene/store.ts`).** `zustand` added (first store in the app; ADR-008 #10 sanctioned it).
`useSceneStore` = `{ scene, previous, resetNotice }` + actions that are **thin wrappers over the pure
scene functions** — no geometry logic in the store. Only `collapseToSingleFragment` / `undoReset` /
`dismissResetNotice` are store-specific (undo bookkeeping the pure layer has no home for).

**Scene ↔ Monaco sync (ADR-008 #6).** Scene→content: `injectSceneIntoInput` writes the merged block,
guarded to skip when the text already matches (prevents echo, never reformats a manual edit).
content→Scene: 500 ms debounce, decision by `xyzMatchesScene` (**parsed floats, tol 1e-6, never
string compare** — the exact trap #6 warns about). Match → leave the scene (the silent common path:
editing the `!` line / `%pal` / comments). Diverge → text wins, `collapseToSingleFragment`. Block
gone → `setScene(null)`. No scene but a block appeared (template / Generate) → adopt. Reset notice +
Undo shows **only when >1 fragment merged** — a single-fragment collapse is a geometric no-op, so it
stays silent (else every hand-edit of a water coordinate would warn). Never fires in d-1's
single-fragment world; wired for d-2.

**Reference stability (the thing that stops the viewer blinking on every keystroke).** Selectors
return the stored object directly; no-op actions return state unchanged; store init is a
`useLayoutEffect` (the screen remounts on nav — a plain effect flashed the previous molecule).
`store.test.ts` asserts repeated `scene` reads are `===`, and mirrors the screen's sync decision
(match ⇒ no collapse ⇒ identity preserved; diverge ⇒ new reference).

**Consolidation closed (ADR-008 delivered).** With NewJobScreen on `sceneFromOrcaInput` /
`injectSceneIntoInput` (new, absorbed the old logic) + `sceneFromXyz`, the duplicate viewer parsers
were **deleted**: `parse-xyz-from-input.ts`, `inject-xyz-into-input.ts`, and `parseChargeMult`.
`viewer/xyz-format.ts` keeps only `xyzToAtomLines` / `atomLinesToXyz` (xyz-string formatters, not
ORCA-input parsers) — live consumers `import-file.ts` + `MoleculesScreen`. **MoleculesScreen was
deliberately NOT migrated to Scene**: it manages library molecules as stored xyz *strings* (the DB
format), not multi-fragment scenes, so a Scene there is churn with regression risk and removes no
duplication. Flagged, not overlooked.

**Three review clean-ups from 2.5.0b, done.**
1. `buildOrcaInput` signature is now `Scene | null` — the string branch (which existed only to keep
   two tests I'd frozen) is gone. Converted exactly those two tests to Scene; the other 8 untouched
   (`git diff` shows only 2 `it(` lines changed). Tests must not dictate the API — that was my error.
2. Atomic-number table extended **H–Kr → H–Rn (Z ≤ 86)**, so Pd(46)/Pt(78) organometallics (the
   cross-couplings ADR-007 names) count electrons instead of `checkElectronParity` silently
   returning null. Unknown-element message + a Pd parity test added.
3. Parity "nearest valid value" fixed — for even electrons + multiplicity 8 it said "1" (smallest),
   now says "7" (actually nearest); `suggested` stays smallest-first. Test added.

**Design notes (benign, flagged for the manual pass).** Save to Library now stores the **canonical**
xyz (`mergeToXyz`, `toFixed(8)`) instead of the verbatim editor text — same geometry, ADR-008 #4
format. Charge/mult on save still read from the live `* xyz` header (via `sceneFromOrcaInput`), so a
manual header edit is honoured exactly as before. Manual coordinate typing re-zooms the viewer (new
fragment id → composition change) — unchanged from the old always-`zoomTo` preview; programmatic
fragment moves (2.5.3, via `replaceFragmentAtoms`) will keep the camera.

**Manual checks (author, real Tauri window — sync behaviour isn't headless-drivable):**
1. Edit the `!` line or `%pal` → **no** reset notice, viewer does **not** blink/redraw.
2. Hand-edit a coordinate number → after ~0.5 s the viewer updates to the new geometry; on a
   single-fragment scene there's **no** notice (nothing to undo — it's the geometry you just typed).
3. Import a `.xyz` → SMILES `CCO` → Generate 3D → Use a library molecule: all three load and preview
   exactly as before; the title auto-fills when empty.
4. Save to Library then reopen the molecule → same geometry renders (stored canonical).
5. Navigate New Job → Jobs → New Job: the screen starts empty (no stale molecule flash).

**Verified.** `tsc --noEmit` clean, `vite build` clean, `vitest run` **76** (was 56 → +20: 13 store,
+ scene/parity additions). The 8 pre-existing `build-input.test.ts` cases untouched. Next: d-2
(Add-Fragment sidebar) then d-3 (`scene_json`).

## [2026-07-28] session | 2.5.0d-2a: curated fragment library + bounding-box placement

Built the pure foundation for adding fragments (ADR-008 #7 + #9) — no UI, no React, node-only tests.
d-2b (the Add-Fragment panel/sidebar) consumes this; d-2a intentionally has no consumer yet.

**Placement (`src/scene/placement.ts`).** `placeFragment(scene, fragment, gap=3.5)` separates the two
AABBs along the axis where the **scene is smallest** (ties → x), centring on the other two — so a
reagent approaches an x-elongated substrate from the side, not down the chain (the naive CoM+vector
lands mid-chain). Clearance is structural, not hopeful: fragment-min on the chosen axis = scene-max +
gap, so every cross pair differs by ≥ gap on that axis alone ⇒ distance ≥ gap. Empty scene → fragment
returned unmoved. Also added `translateFragment(fragment, dx,dy,dz)` to `scene.ts` (pure rigid shift;
placement uses it, 2.5.3 will too). Tests are **invariants, not golden numbers**: ≥ gap separation
(incl. a 2nd fragment clearing the 1st), intra-fragment distances preserved to 1e-9, x-elongated
substrate ⇒ off-x axis, empty scene identity, monatomic Cl⁻.

**Library (`src/scene/fragment-library.ts`).** Eight reagents: BH₄⁻(−1), H⁻(−1), OH⁻(−1), CN⁻(−1),
Cl⁻(−1), H₂O(0), NH₃(0), CH₃OH(0). `libraryFragmentToScene` → fresh id, deep-copied atoms,
`source:"fragment-library"`, `sourceLabel=key`. Kept the list at exactly the eight requested —
considered adding F⁻ but Cl⁻ already covers a halide nucleophile, so held off (avoid scope creep).

**Where each geometry came from (the honesty ledger):**
- Built from ideal symmetry + reference values (constructed in code, so symmetry is exact):
  BH₄⁻ (T_d cube-diagonal dirs × B–H 1.24 Å); H₂O (C2v bent, O–H 0.9572 Å / 104.52°); NH₃ (C3v
  pyramidal, N–H 1.012 Å / 106.67°); OH⁻ (0.964 Å), CN⁻ (1.16 Å) diatomics; H⁻, Cl⁻ monatomic.
  Reference bond lengths/angles are the standard experimental/spectroscopic values, cited in each
  `provenance`.
- **CH₃OH — run through ORCA**, because a 6-atom low-symmetry Z-matrix is too error-prone to
  hand-build and a wrong number here would converge silently. `! r2SCAN-3c Opt` in an isolated dir
  `/tmp/orca-meoh-d2a` (nproc-serial, full `/opt/orca/orca` path — domain rules 1 & 3), TERMINATED
  NORMALLY, E = −115.6947 Eh. Measured C–O 1.4303 Å, O–H 0.9597 Å, C–O–H 108.66° (all match
  literature methanol); hardcoded those coords, **cleaned the dir up** afterwards.
- **Nothing was dropped for uncertainty** — all eight geometries are either symmetry-exact or
  ORCA-verified.

**The invented-number guard.** Each entry declares `reference` internals; `fragment-library.test.ts`
recomputes every bond/angle *from the coordinates* (independent distance/dot-product math, tol
1e-3 Å / 0.1°) and fails on disagreement — a mistyped coordinate can't ship a wrong-but-converging
geometry. Also checks non-empty provenance, unique keys, charges (BH₄⁻=−1, H₂O=0…), BH₄⁻'s six
H–B–H all ≈109.47°, and that `libraryFragmentToScene` gives fresh ids + independent atoms.

**Wiki.** New Ukrainian chemistry note `chemistry/reagent-geometry.md` (hydride nucleophile; why BH₄⁻
is tetrahedral from VSEPR/cube-diagonals; why water is 104.5° not 109.5° — lone-pair compression
CH₄→NH₃→H₂O; why methanol had to be computed). `modules/scene.md` gained placement + library
sections; index + ROADMAP updated (d-2 split into d-2a done / d-2b pending).

**Verified.** `tsc --noEmit` clean, `vite build` clean, `vitest run` **97** (was 76 → +21: placement
8, library 13). ORCA temp dir removed.

## [2026-07-28] session | 2.5.0d-2b: Add-Fragment panel + fragment sidebar

The moment multi-fragment first becomes reachable to the user — everything under it (store d-1,
palette 2.5.0c, library + placement d-2a) was already in place; this is the glue.

**Regression guard result (the finest wire in the subsystem — recording it explicitly).** The risk:
add a fragment → scene goes 2-fragment → Scene→content injects → ~500 ms later content→Scene
re-parses and calls `xyzMatchesScene`; if ordering/formatting drift makes that FALSE, the scene
**silently collapses to one fragment half a second after the add**, no error. `add-fragment.test.ts`
drives the real inject → parse → `xyzMatchesScene` path a real add produces (water + BH₄⁻ → merged
into a `! r2SCAN-3c Opt` input → re-parsed) and asserts the comparison is **TRUE** → the effect
leaves the scene at 2 fragments. **PASS.** It's a pure-function simulation, not a rendered-component +
fake-timers test, because the suite has no jsdom (adding one is a new dep, out of scope) — and the
comparison is exactly where the bug would live, so the guard is faithful. Also checks total charge
shows through the merged header (water + BH₄⁻ = `* xyz -1 1`).

**UI.** `src/scene/FragmentList.tsx` — sidebar under the viewer: per-fragment row with the shared
`fragmentColor(index)` swatch (fragment 0 → neutral swatch + "CPK" label, honestly flagged),
inline-editable name (uncontrolled, commits on blur/Enter → `renameFragment`, no per-keystroke store
write), atom count + signed charge, remove button; totals line + a note that removing fragment 0
recolours the next (consequence of the CPK rule, surfaced as a label, rule unchanged). Add-Fragment
panel (`openSection: "add"`) with four sources — Reagents (library chips, provenance in `title`),
Import, SMILES, From-library (lazy `list_molecules`) — all through one helper `addFragmentToScene` =
`placeFragment(current, f)` → `addFragment`. **One road:** first molecule and later reagents share
the code (placeFragment on empty = identity), no "first replaces / rest add" split. Import/SMILES
moved off the top row into the panel; row now = Add Fragment + Save to Library. Store gained
`renameFragment`. Reset-notice banner (d-1, needs >1 fragment) now reachable with Undo/Dismiss.

**Two provenance fixes from the d-2a review (ran ORCA to name real sources, not memory/circular).**
Both r²SCAN-3c Opt in isolated dirs (`/opt/orca/orca`, cleaned up — domain rules 1 & 3):
- **BH₄⁻**: was `1.24 Å "cf. ADR-008"` (circular — ADR took it from the prompt). Optimised → **B–H
  1.2368 Å** (T_d), provenance now names the calc. Reference + builder length updated.
- **CN⁻**: was `1.16 Å` unsourced. Optimised → **C≡N 1.1743 Å**, provenance names the calc.
The library geometry tests still pass (constructor rebuilds T_d / the diatomic at the new lengths;
the recompute-from-coords cross-check confirms).

**Clarified what the library tests prove** (`modules/scene.md`, per review): for CH₃OH (hardcoded
coords) it's a genuine independent coords-vs-declaration cross-check; for the seven symmetry-built
fragments the coords are generated *from* the reference, so the test proves the **constructor**, not
that the reference number is physically right — the number's guarantee is provenance + review.

**Manual checks (author, real Tauri window — plain DOM, no WebGL, so no MiniBrowser needed):**
1. water → Add Fragment → BH₄⁻ → **wait ~1 s** → sidebar shows 2 fragments; viewer shows BH₄⁻ in
   teal, clearly separated from the water (not merged, and it did NOT collapse after the pause).
2. Charge in the input builder form reads −1 automatically (totalCharge through the merged header).
3. Parity warning behaves for the combined electron count.
4. Undo scenario: two molecules → hand-edit a coordinate in Monaco → "N fragments merged" banner →
   Undo → two fragments again with the original coordinates.
5. Remove BH₄⁻ from the sidebar → back to one fragment, charge 0.

**Verified.** `tsc --noEmit` clean, `vite build` clean, `vitest run` **105** (was 97 → +8). Two ORCA
temp dirs created and removed. Next: d-3 (`jobs.scene_json`, schema v4).

## [2026-07-28] session | 2.5.0d-3: persist scene snapshot + iterate on a job (2.5.0 closed)

Fragments now survive an iteration over a job — and it's hand-verifiable. Two halves in one unit
(`jobs.scene_json` + the action that reads it), for a reason worth recording.

**The finding that shaped scope: there was no clone.** ADR-008 #5 justified the snapshot column by
"cloning a job otherwise yields one flat fragment, re-split by hand every iteration." But **no
clone/duplicate action existed** — New Job is reachable only from its own tab and from Molecules
(`initialMolecule`). A column with no reader is dead weight. So the reader ships in the same task: a
**"New iteration"** button on job detail that seeds a New Job draft from a job's `input_content` +
`scene_json` (nothing from results/status/dir; iterating a running job is fine — only its input).
Fixed the ADR by **amendment** (dated, at the end — decision #5 stands, its *justification* was
wrong), not by editing #5. Naming: "New iteration", not "Duplicate" — chemist framing (next round of
the same TS-guess work).

**Restore rule is now code, reusing the sync primitive (not a second comparison).** `restoreScene`
(`src/scene/restore.ts`, pure): `input_content` is authoritative for geometry, `scene_json` only
annotates it. Four branches — no coord block → `{null,false}`; no snapshot → single fragment from
text, `false` (**pre-v4 job, not an anomaly**); malformed/wrong-version → text fragment, `true`;
valid snapshot → **`xyzMatchesScene(snapshot, input geometry)`** (the very primitive that guards the
Monaco sync): match returns the *snapshot* (multi-fragment layout preserved), mismatch returns the
text fragment, `true`. `snapshotRejected` deliberately splits the two single-fragment outcomes — a
discarded snapshot draws a UI note, a `NULL` snapshot is silent. Tests: all four branches + a matching
2-fragment snapshot (layout survives) + a right-count-but-shifted-coords snapshot (rejected).

**Rust (schema v4).** `SCHEMA_VERSION` → 4; additive `ALTER TABLE jobs ADD COLUMN scene_json TEXT`
gated on `version < 4`, same pattern as v2→v3. `Job` gained `scene_json: Option<String>` (COLUMNS +
`from_row` 11th col); `create_job(title, input_content, scene_json: Option<String>)` — **written once
at create, no update path** (input is immutable). `Result<T, AppError>`, no `.unwrap()` outside tests.
Tests: `migrate_v3_to_v4_preserves_jobs` (old job survives, column NULL) + `create_persists_and_
reloads_scene_json`. The v2→v3 test's `== 3` became `== SCHEMA_VERSION` (it was brittle; migrate always
runs fully forward — same fix the v1→v2 test already had).

**Real-DB migration check (mandatory).** Copied the live `~/.local/share/orcastudio/orcastudio.db`
and ran the actual `migrate()` on it via a throwaway `#[ignore]`d test (removed after): **BEFORE
schema_version=3, 13 jobs, no scene_json column → AFTER schema_version=4, 13 jobs (all preserved),
scene_json NULL on every one.** Temp copy + test removed.

**Write path (§4).** Only `NewJobScreen.create()` serialises: `sceneJson: scene ? serializeScene(scene)
: null`. One path.

**Deferred, noted (ROADMAP + ADR amendment):** "continue from the *result*" (iterate from the
optimised output geometry) needs cclib output parsing → Phase 3. The snapshot already supports it —
after Opt the atom count/order are invariant (`replaceFragmentAtoms`'s guarantee), so fragment
boundaries transfer onto the optimised coords with no guessing.

**Manual checks (author, real Tauri window):**
1. Build water + BH₄⁻ → Create Job → open it → **New iteration** → New Job opens with **two fragments**
   intact, same colours, charge −1, title "… (iteration)".
2. Same for a single-fragment job → New iteration → one fragment, **no note**, behaves as always.
3. A job created before this session (`scene_json` NULL) → New iteration → one fragment, **no note**.
4. New iteration, then hand-break a coordinate in the cloned input → the "snapshot dropped" note
   appears (rejected), and it starts from a single fragment.

**Verified.** `tsc` clean, `vite build` clean, `vitest` **112** (was 105 → +7), `cargo test` **55**
(was 53 → +2). Real DB migrated cleanly. **Phase 2.5.0 (Scene/fragment foundation) is complete** —
next is 2.5.1 (atom picking + measurement).

## [2026-07-28] lint | Wiki health-check after 2.5.0

First lint of the ADR-008 cycle (ADR-008 → 2.5.0d-3). **12 findings: 8 factually wrong, 3 incomplete,
2 stylistic (named, untouched), 1 needs-decision.** Fixed the first two categories; no code touched,
no ADR rewritten (amendments only), no structure reorganised.

**Fixed — factually wrong (deleted code described as present).** All in chronicle "As built (Phase
N)" sections; fixed with concise **supersession markers** (annotate, don't delete — the as-built
record stays, the reader gets a forward-pointer):
- `frontend.md` Phase 2.1 / 2.2 / 2.3 / 2.6 — `parse-xyz-from-input.ts`, `inject-xyz-into-input.ts`,
  `injectXyzIntoInput`, `extractXyzFromInput`, `parseChargeMult`, `previewXyz` all described live;
  deleted/removed in 2.5.0d-1 (2.6's `injectXyzIntoInput` line fixed inline).
- `visualization.md` — `parse-xyz-from-input.ts` in the file list (deleted); and the 2.5.0c claim
  "NewJobScreen passes only `xyzData`" (it passes `scene` since 2.5.0d-1; only Molecules uses xyzData).
- `tauri-core.md` — Phase 1 `create_job(title, input_content)` signature (gained `scene_json` in v4).
- `index.md` — "Page count: 26" → **29** (31 files − index − log; the metric had drifted; index
  lists all 29, no orphans).

**Fixed — incomplete (correct but behind):**
- `scene.md` fragment-library section → added the missing cross-link to
  `chemistry/reagent-geometry.md`.
- `frontend.md` + `visualization.md` Status lines → augmented to state 2.5.0 (scene/fragment) is
  complete / multi-fragment rendering added.

**Verified consistent — no action.** ADR-008 #4 (canonical format) matches code exactly
(`padEnd(2)` + `toFixed(8).padStart(14)`). All four deliberate deviations are recorded as decisions
with reasons, none silent (narrowed consolidation 2.5.0b; d→d-1/2/3; d-2→d-2a/2b; clone amendment).
ROADMAP: 2.5.0 correctly closed, no drift. No orphan content pages, no dead links.

**Stylistic (named, untouched):** `log.md` absent from `index.md` — intentional (append-only
chronicle meta-file, like `index.md` itself). `visualization.md:9` "`xyzData: string`" is
self-corrected 12 lines down by the 2.5.0c subsection.

**Needs author decision (not resolved unilaterally):** ADR-008 **#7** says place a fragment "along
the axis with the most free space / the freest axis"; the code + `scene.md` place "along the axis
where the scene is smallest (least extent)." These coincide for the elongated substrate #7 describes,
but can diverge for a disk-shaped one (smallest-extent = face-on, not the freest approach). Not a
contradiction for the documented case; the ADR prose is just looser. Option: leave as-is, or add a
one-line ADR amendment ("freest axis = smallest bounding-box extent"). Left for the author — I won't
edit #7 or amend without a call.

## [2026-07-28] session | 2.5.1a: GOAT ensemble parsing, verified against a real run

Pulled the GOAT conformer primitive up from Phase 4.5 (ADR-007 calls conformer search mandatory, but
SMILES fragments arrive as an arbitrary ETKDG conformer — every scene may stand on the wrong one).
**No UI.** Parser written **against a real run, not from memory** (gotchas rule).

**The run.** n-butane (14 atoms), **ORCA 6.1.0**, `! XTB GOAT`, isolated dir, full-path
`/opt/orca/orca`. TERMINATED NORMALLY. Observed (now in `wiki/orca/goat.md`):
- Output files that matter: **`<name>.finalensemble.xyz`** (ensemble) + **`<name>.globalminimum.xyz`**
  (~40 scratch files otherwise).
- Ensemble comment line: **`<energy_Eh> converged=true`** — energy is the leading token (Hartree),
  **no structure index**. globalminimum uses a *different* comment (energy only, no `converged=`).
- **5 structures** in the file, **sorted ascending by energy**. Gotcha: the log says "Conformers below
  3 kcal/mol: **4**" but the file has **5** (the 5th is ~4.9 kcal/mol) — trust the file, not the summary.
- **Cost / `%pal`:** 1 core **4m20s** → `%pal 4` **1m13s** (~3.5×). GOAT parallelises *across* candidate
  optimisations (NProcs=1 each, out-of-order completion), not within one. GOAT is slow and blocks the
  concurrency-1 queue — give it `%pal`, treat as long-running (matters for 2.5.1b).
- Charge via the plain `* xyz 0 1` header — no GOAT-specific keyword; the header is ORCA's universal
  charge mechanism, so charged fragments (BH₄⁻ → `* xyz -1 1`) use the same. Mult 1 safe for the
  closed-shell library fragments.

**CRITICAL CHECK — verified on the butane run: atom order is PRESERVED.** All 5 ensemble structures
have the identical element sequence to the input (`C C C C H H H H H H H H H H`). So a chosen conformer
drops back into a fragment via `replaceFragmentAtoms` **with no atom mapping** — 2.5.1b's substitution
is safe, no rewrite needed. (Re-verify on ORCA upgrade — noted in `goat.md`.)

**Code (`src/scene/ensemble.ts`, pure).** `parseEnsemble(text): Conformer[] | null` (energy = leading
comment token, `NaN` if unparsable — never invented; malformed/empty → `null`, never throws);
`conformerMatchesFragment` (the `replaceFragmentAtoms` composition check as a predicate, via the
now-**exported** `normalizeElement`); `goatInputForFragment` (uses `fragment.charge`, not scene
`totalCharge` — GOAT runs on one fragment in isolation). **Test oracle is a real 3-structure slice** of
the run in `src/scene/__fixtures__/butane.finalensemble.xyz` (loaded via Vite `?raw`) — declaration
must meet reality, same rule that saved the fragment library.

**Two decision records** (see the `decision` entry below for §5b): ADR-008 amendment pins *freest axis
≡ smallest bounding-box extent* (closes the lint finding); and the d/θ/φ sequential-application record.

**Wiki / ROADMAP.** New `orca/goat.md` + `chemistry/conformers.md` (Ukrainian: anti/gauche butane, why
one SMILES conformer is a random snapshot). ROADMAP: inserted **2.5.1 — Conformer search (GOAT)**
(a done, b pending) before the geometry editor, which becomes **2.5.2**; Phase 4.5's GOAT item narrowed
to Boltzmann + DFT re-opt with a pointer to 2.5.1. Fixed the stale "2.5.1 = picking" refs → 2.5.2, and
aligned the geometry-editor "2.5.3" refs → 2.5.2 (consequence of the renumber). index count 29 → 31.

**Verified.** `tsc` clean, `vite build` clean, `vitest` **122** (was 112 → +10 ensemble). `cargo` not
touched. Both ORCA job dirs removed (rule #3); the fixture kept. Next: 2.5.1b (run GOAT from the app +
conformer substitution).

## [2026-07-28] decision | Bürgi-Dunitz d/θ/φ apply sequentially (one pass, convergent)

For the geometry editor (2.5.2), placing a reagent by distance → angle → dihedral can be done in **one
sequential pass** — no iteration — **if the reference atoms are taken from the substrate side** and the
mask is the reagent fragment. This is safe by construction, not merely convenient: **each operation lies
in the symmetry group of the previous constraint**, so it cannot undo it.

- `set_distance(C, H, mask=reagent)` — translates the reagent along the C→H line. Sets |C–H|.
- `set_angle(O, C, H, mask=reagent)` — rotates the reagent about an axis **through C** (perpendicular to
  the O–C–H plane). A rotation about C leaves **|C–H| unchanged**. Sets ∠O–C–H.
- `set_dihedral(X, O, C, H, mask=reagent)` — rotates the reagent about the **O–C axis**. A rotation about
  O–C leaves **both |C–H| and ∠O–C–H unchanged**. Sets the dihedral.

Each later op's rotation axis passes through the atoms that define the earlier constraint, so the earlier
quantity is invariant under it. One pass converges; no relaxation loop.

**Mandatory acceptance test for 2.5.2 (without it "sequential" is just an assumption):** apply all three
in order to a fragment, then **recompute all three from the resulting coordinates** — |C–H|, ∠O–C–H, and
the dihedral must each equal their targets (tolerance ~1e-6). If any fails, the reference-atom convention
is wrong (e.g. an axis not passing through the constraint atoms) and must be fixed before edit mode ships.

## [2026-07-28] session | 2.5.1b: run GOAT on a fragment, apply a conformer (2.5.1 closed)

Conformer search is usable end to end. **2.5.1 is complete.**

**`scene_json` for a GOAT job — read this first (§1).** A GOAT job runs on ONE fragment, so its
`input_content` is that fragment alone. Its `scene_json` is a **single-fragment scene of that same
fragment** — the snapshot annotates its own single-fragment input, so `restoreScene`'s
`xyzMatchesScene` holds and it's honoured with no special branch, and `id`/`name`/`charge` survive a
restart. Putting the *whole* scene there would fail `xyzMatchesScene` against the single-fragment
input → silent rejection → a second implicit meaning in one column. An explicit test
(`ensemble.test.ts`) asserts the round-trip: `restoreScene(goatInputForFragment(f), serialize({[f]}))`
→ not rejected, one fragment.

**Flow.** "Find conformers" per fragment in `FragmentList` → creates + runs a job
(`goatInputForFragment`; `%pal` inserted by the backend's `align_pal_nprocs`, uniform with every job)
with the single-fragment `scene_json`, title `Conformer search — <name>`, honest cost caption. On a
completed job `JobDetailScreen` lazily calls the new **`read_job_ensemble`** Rust command
(`input.finalensemble.xyz` — the fixed input name gives a fixed ensemble name) → `parseEnsemble` →
panel of conformers with **ΔE in kcal/mol** (`deltaEKcal`; NaN → dash), the selected one in a
`MoleculeViewer`. Count from the file, not the log summary (goat.md gotcha). "Use this conformer" →
`planConformerApply` (pure, testable): **replace** the fragment in place if it's still in the store
scene (survives the nav — singleton) → New Job with a new `keepScene` flag that skips the mount reset;
else a **new** single-fragment scene; else **refuse** (banner, no throw) on composition mismatch.

**Real run (mandatory).** Ran GOAT on butane via the app's **exact `goatInputForFragment` format**
(`input.inp` + `%pal 4`). ORCA 6.1.0, **TERMINATED NORMALLY, 1m13s**; ensemble at
`input.finalensemble.xyz`. `parseEnsemble` + `deltaEKcal` → **4 conformers, ΔE 0.00 / 0.596 / 2.567 /
2.607 kcal/mol**, atom order preserved in all four. The 0.596 anti→gauche gap is chemically sensible
and matches the unit fixture — so app input → GOAT → ensemble file → parse → ΔE is verified on real
data, not just mocked. Job dir removed (rule #3). (4 structures this time vs 5 in 2.5.1a — GOAT's
retained count varies; count-from-file handles it.)

**Manual checks (author, real Tauri window — the click path isn't headless-drivable):**
1. Build a scene → a fragment's **Find conformers** → a `Conformer search — <name>` job queues + runs,
   with the cost caption visible beforehand.
2. On completion → conformer list with ΔE (kcal/mol), pick one → it renders in the panel viewer.
3. **Use this conformer** with the scene still live → the fragment updates **in place**, the rest of
   the scene (other fragments, colours) untouched, back on New Job.
4. Same after restarting the app (scene cleared) → the *new* branch: New Job with one fragment
   (name + charge from the snapshot, coords from the conformer).

**Verified.** `tsc` clean, `vite build` clean, `vitest` **128** (was 122 → +6), `cargo test` **55**.
Next: 2.5.2 — geometry editor (its d/θ/φ acceptance test is already on record).
