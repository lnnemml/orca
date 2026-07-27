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
