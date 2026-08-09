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

## [2026-07-28] session | 2.5.2a: atom picking in the viewer + GOAT-aware convergence label

First unit of the geometry editor: click an atom → a selection panel names it. Plus a small
fix to the convergence panel's label on GOAT jobs. **No coordinate math, no sidecar, no
constraints — that's 2.5.2b.**

**Risk cleared FIRST (the whole point of the sequencing).** Whether 3Dmol's `setClickable`
even works in WebKitGTK was unknown: `debugging/002` forced the direct-canvas path
(OffscreenCanvas removed) and the *mouse-event* path had never been checked since. Re-ran the
002 MiniBrowser technique (`webkit2gtk-4.1/MiniBrowser`, standalone HTML, real
`node_modules/3dmol`, `OffscreenCanvas=undefined`) with a 5-atom molecule, `setClickable({},
true, cb)` pushing `atom.index`, and for each atom **projected via `modelToScreen` then
dispatched a real `mousedown` (canvas) + `mouseup` (`document.body`)** at that page coord —
3Dmol's genuine handler chain (`getX`→`pageX`, `closeEnoughForClick` tol 5,
`handleClickSelection` ray-cast). **Clicking atoms 0–4 at five distinct screen points
returned indices 0/1/2/3/4 exactly** — not always-0, not shifted. Picking is sound under the
direct-canvas path; UI built only after this. (Full numbers + what it doesn't cover:
`modules/visualization.md` Watchpoints.)

**Decisions (recorded):**
- **Fifth-click rule — NOT FIFO.** Clicking a new atom when 4 are already picked makes the
  selection `[that atom]`, not "drop oldest, append". FIFO would silently leave the user
  measuring atoms different from the ones highlighted — a wrong-atom measurement with no
  visible cause. A hard reset to the just-clicked atom is unambiguous. (`selection.ts`,
  `MAX_SELECTION=4`.)
- **Selection re-validates on `compositionSignature`, not a bespoke length check.** Moved
  `compositionSignature` up from `MoleculeViewer` into `scene.ts` as an exported pure
  function — one canonical "did composition change?", sibling of `xyzMatchesScene`. Both the
  viewer's re-zoom and `NewJobScreen`'s `validateSelection` key off it; a coordinate-only edit
  changes neither camera nor selection.
- **Highlight via translucent `addSphere`, not `setStyle`** — a style path would clobber the
  per-fragment index-range colours and need manual restore; a separate `[selection, scene]`
  effect does `removeAllShapes` + re-add + render, **no `zoomTo`/model reload**. The spheres
  also become 2.5.2b's measurement-label anchors.
- **Selection state lives in `NewJobScreen`, not the scene store** — the store stays a pure
  geometry wrapper (ADR-008 #10).
- **Global index labelled `(0-based)` in the UI** — the 0-/1-based `%geom` Constraints
  question is unresolved empirically; while open, the UI must state the base it shows.
- **GOAT convergence fix — `variant` prop, NOT overloading `status`.** `ConvergenceDashboard`
  gains `variant: "standard" | "goat"`; the existing unused `status` prop is left untouched
  (one prop, one meaning). `JobDetailScreen` passes `isGoatInput(job.input_content) ? "goat" :
  "standard"`. `isGoatInput` scans only `!` keyword lines for the `GOAT` token on word
  boundaries (comments/geometry ignored). For `"goat"`: relabelled head ("Conformer search ·
  inner optimisation, cycle N · m/5 criteria met"), a grey "one candidate of many — overall
  GOAT progress is not shown", and the **progress bar is hidden** (a full bar with minutes of
  search ahead was the lie); chips stay. Why: those cycles are one candidate's inner opt, not
  search progress (`wiki/orca/goat.md`).

**Clickability is off unless `onAtomPick` is passed** (`pickable = onAtomPick != null` gates the
only `setClickable`). Only `NewJobScreen` passes it — Molecules screen and the Job-detail
conformer panel are byte-identical (acceptance criterion, grep-proven, no jsdom to unit-test a
render).

**Verified.** `tsc` clean, `vite build` clean, `vitest` **149** (was 128 → +21: `selection.ts`
invariants, `isGoatInput`, `compositionSignature`), `cargo test` **55** (Rust untouched). The
in-window click checks (real fragment/local index per click, toggle-off, fifth-click reset,
remove-first-fragment cleanup, coordinate-edit leaves selection+camera, GOAT running label) need
the real Tauri window — not headless-drivable; author checklist stands. Next: 2.5.2b —
distance/angle/dihedral measurement off this pick list.

## [2026-07-29] decision | Selection survival rule — signature-based, no remap

A 2.5.2a review finding, fixed first in 2.5.2b. **Defect (reproduced on the built code):**
`validateSelection` checks only that a picked index is in range, so it survives an index
*shift*. Scene = Water(global 0,1,2) + BH₄⁻(3..7); pick global 3 = B (BH₄⁻ local 0).
`removeFragment("wat")` → 5 atoms; `validateSelection([3], after)` → `[3]` (not cleared), and
`describeAtom(after, 3)` now resolves to element "H", local 3 — a **different atom**. The pick
silently moved boron → hydrogen. In 2.5.2d that index goes into an ASE mask, so a silent shift
would mask the wrong atom. The old test `"cleans the selection after removeFragment of the FIRST
fragment"` passed for the **wrong reason** — it used index 8, which merely fell out of range, so
it never exercised the shift.

**Decision (architect):**
- `addFragment` always appends the new fragment LAST, so a pure append does **not** shift
  existing indices — a selection survives it.
- Any other change to `compositionSignature` (fragment removed, composition changed) **clears
  the selection entirely**.
- **No remap.** After a removal "the same atom" has no operational definition; a silent guess
  costs more than a lost click.

**Implementation.** `selection.ts::selectionSurvives(prevSig, nextSig)` — true iff
`next === prev` or `next` starts with `prev + "|"` (clean append; the trailing `"|"` forces a
whole-field match so `"a:3"` can't append-match `"a:30|b:2"`). Works on signature strings, never
sees the scene. `NewJobScreen` on a signature change: `!selectionSurvives → setSelection([])`,
else keep; `validateSelection` stays a second echelon (mainly `scene → null`). Old test replaced
(not added beside) with an **in-range index 3** case that range validation can't catch, plus a
`selectionSurvives` block driving the real before/after signatures.

## [2026-07-29] decision | Dihedral convention pinned to ASE source — [0, 360)

2.5.2b's `measure.ts` is the tool 2.5.2c's acceptance test uses to **re-derive** d/θ/φ after
applying them through ASE. If our convention diverged from ASE, that test would fail a correct
core or pass a wrong one — so the convention is fixed now from the **real ASE source** in the
sidecar venv (`ase/geometry/geometry.py`, `ase/atoms.py`; checked 2026-07-29), not from memory.

- **Angle vertex = the MIDDLE index.** `Atoms.get_angle(a1,a2,a3)` = angle between `a1-a2` and
  `a3-a2` (`atoms.py::get_angles`) — `a2` is the vertex. Our `(i, vertex, j)` maps positionally.
- **Dihedral range `[0, 360)`, NOT `(-180, 180]`.** `geometry.py::get_dihedrals` does
  `atan2 → [-π,π]` then `dihedrals[dihedrals < 0.] += 2*pi` **before** `np.degrees`. We replicate
  that fold verbatim. Verified numerically against ASE on the butane fixture (via the sidecar
  venv): anti = **179.998°**, gauche = **67.523°** — the gauche value lands on the **60 side**,
  not 300, which is exactly what the fold with `v0=a1-a0, v1=a2-a1, v2=a3-a2` produces.
  `measure.test.ts` locks 67.523° → **this is the tripwire that breaks in 2.5.2c** if ASE and
  `measure.ts` ever fold opposite ways. Reversal-invariant (`d(i,j,k,l)==d(l,k,j,i)`); a mirror
  sends `φ → 360 − φ`. Collinearity guarded by the normalised cross-product norm, not an angle.

## [2026-07-29] session | 2.5.2b: geometry measurements + selection survival rule

Second unit of the geometry editor: read d/θ/φ off the 2.5.2a pick list. **No coordinate change,
no sidecar, no ASE call, no constraints — that is 2.5.2c/d.**

**Done first — the review fixes (2.5.2a findings):**
1. **Selection survival rule** — see the decision entry above (`selectionSurvives`; old
   wrong-reason test replaced with an in-range index-3 case).
2. **`isGoatInput` trailing comment** — `! XTB Opt # GOAT next time` was true; now the line is
   cut at the first `#` before the `!`/`GOAT` check (+ test).
3. **Highlight effect leaked shapes on `scene → null`** — `removeAllShapes`/`removeAllLabels`
   now run BEFORE the `!scene` bail-out (halos + labels clear when the last fragment goes).

**Measurement core (`scene/measure.ts`, pure/node-tested):** `distance` / `angle` (middle pick =
vertex) / `dihedral` (chain i-j-k-l, ASE `[0,360)`), `measureSelection` (positional: 2/3/4 →
d/θ/φ; 0/1 or degenerate → none), `formatMeasurementValue`. Degenerate → **null, never NaN**;
`sameFragment` flags an inter-fragment distance (a future reaction coordinate). Conventions in
the decision entry. Tests are invariants, not self-constructor literals: water H–O–H 104.52° /
O–H 0.9572 Å, BH₄⁻ H–B–H 109.47°, butane anti 179.998° / gauche 67.523°, symmetries, mirror
`φ→360−φ`, and the load-bearing **rigid-motion** test (explicit proper rotation + translation of
the whole scene leaves all three unchanged to 1e-9).

**UI (display-only):** `AtomInspector` gains a readout line at ≥2 picks — `H···B  1.234 Å`,
`104.5°`, `dihedral 178.9°`, atom chain in click order, prominent `inter-fragment` badge; index
line now `local index N · global index N (both 0-based)`. `MoleculeViewer`'s highlight effect
draws dashed bond lines + a value label (`drawMeasurement`) — **not clickable**, so a label over
an atom can't intercept the pick (repeat-click toggle-off preserved).

**Verified.** `tsc` + `vite build` clean; `vitest` **178** (was 149 → +29: `measure.ts`,
`selectionSurvives`, `isGoatInput` trailing-comment); `cargo test` **55** (Rust untouched).
In-window checks need the real Tauri window: 2 atoms cross-fragment → distance with
`inter-fragment`; 3 atoms → angle whose vertex is the 2nd click (not the middle index); 4 atoms →
dihedral; repeat-click on a picked atom under a label still deselects; remove first fragment →
selection AND labels vanish. Next: 2.5.2c — apply d/θ/φ through the ASE geometry kernel.

## [2026-07-29] session | 2.5.2e-1: proportional selection halo + optional atom numbering

Viewer ergonomics after the 2.5.2 manual check: the selection halo was nearly invisible except
on hydrogen, and atom numbering was wanted. **No geometry, no sidecar, no constraints.**

**Root cause of the invisible halo — a constant radius, not "too small".** 3Dmol draws each atom
as `sphere:{scale:0.3}` = `vdwRadii[element] * 0.3` (`GLModel.getRadiusFromStyle`), so the drawn
radius is element-dependent: H 0.36 Å, O 0.456, N 0.465, **C 0.51**. The old halo was a *constant*
`HIGHLIGHT_RADIUS = 0.55`, so the shell outside the atom was 0.19 Å on H but **0.04 Å on C** —
seen only on hydrogen. Fix: `highlightRadius(element)` (pure, `src/viewer/highlight.ts`) =
`vdw*0.3 + 0.25` floored at 0.5 — a **constant 0.25 Å shell on top of the drawn radius**, so every
element shows the same visible thickness.

**vdW table = documented copy of 3Dmol's.** 3Dmol exports `GLModel.vdwRadii` (typed), but its
bundle needs `window`/`document` and won't load under the node test runner (no jsdom, by design),
so importing it would break the pure test. Transcribed the table verbatim (v2.5.5), covering
3Dmol's own set — H–Kr **plus Pd(46)/Pt(78)**, the cross-coupling metals ADR-007 names (we already
hit a coverage hole exactly there in 2.5.0). Off-table → 1.5 Å = 3Dmol's `defaultSphereRadius`
(never NaN/zero). **Drift guard:** pure `vdwTableDrift(reference)` lists mismatches;
`MoleculeViewer` calls it once in dev with the live `GLModel.vdwRadii` and warns — the active check
in the real webview, since the test can't reach 3dmol.

**Empirical screenshot decision (MiniBrowser, `debugging/002` technique, H·C·N·O in one frame).**
Before (constant 0.55, `#ffffff`, op 0.35): no visible halo on C/N/O, only on H. Solid magenta
`vdw*0.3+0.25`: reads on C/N/H but **washes out over CPK red oxygen** (hue clash) and is thin on
grey C. **Wireframe** magenta cage: crisp on all four including grey C and red O — chosen. Colour
`#ff2d95` (saturated, reads on `#0d0f13` and on the light bg e-2 adds; NOT `#ffffff` = CPK H), op
0.85. Per element after: H 0.61 / O 0.706 / N 0.715 / C 0.76 Å halo.

**Atom numbering.** New `MoleculeViewer` prop `showAtomNumbers` (default false → Molecules screen
and conformer panel unchanged); a `NewJobScreen` "Numbers" toggle. **Only the global 0-based index
in the 3D view** — the local index stays in `AtomInspector` where the fragment gives it context
(two numbers on an atom is exactly the confusion the single index space removes). **Selected atoms
are numbered always**, even with the toggle off. Halos + measurement lines/labels + number labels
all live in the **one** overlay effect (`[selection, scene, showAtomNumbers]`) — the single owner
of `removeAllShapes`/`removeAllLabels`; a second owner would erase the first. `showAtomNumbers` is
NOT in the model effect's deps, so toggling redraws labels only — no model reload, no `zoomTo`.

**Number labels don't intercept picks — verified empirically.** A MiniBrowser probe placed the
"1" label over atom index 1, armed `setClickable`, and dispatched a real click at that atom
(project via `modelToScreen`, `mousedown` on canvas + `mouseup` on body — the 2.5.2a event path).
Callback fired with `atom.index === 1` (window title `PICKED-1`): the label and halo did not
intercept the pick.

**Verified.** `tsc` + `vite build` clean; `vitest` **188** (was 178 → +10: `highlight.ts`
monotonicity, floor, Pd/Pt non-fallback, drift guard); `cargo test` **55** (Rust untouched).
In-window checks needing the real Tauri window: halo visible on an aromatic-ring carbon and a
carbonyl oxygen; Numbers toggles without a camera jump; numbering on a ~50-atom scene stays
responsive (perf input for e-2). Next: 2.5.2e-2 — viewer fullscreen / background presets / settings
persistence. Screenshots archived with the session (`/tmp/halo-*.png`, `/tmp/pick-through-label.png`).

## [2026-07-29] decision | Viewer background: presets, not a free colour picker

2.5.2e-2 offers background themes. **Fixed presets, not a free colour picker** — because the
safety property is a testable **contrast invariant**: `theme.test.ts` asserts every theme's halo,
label text, and measurement line clear WCAG 3:1 against their background (`contrastRatio`, pure).
A free picker would let a user choose a background that makes the halo invisible, and no test can
catch an arbitrary runtime colour. Presets keep the guarantee. Four: `dark` (the pre-2.5.2e-2
look, unchanged, still default), `black`, `light`, `white`. Overlay colours moved OUT of
`MoleculeViewer` into the theme (e-1 had hard-coded `NUMBER_BG = #0d0f13`, which is a dark
rectangle on a light background).

**Reported, not silently fixed — FRAGMENT_PALETTE fails contrast on the light themes.** The four
fragment colours are shared with `FragmentList`, so this unit only *measures* them. Against
`#eceff3` / `#ffffff`: teal 1.61/1.86, coral 2.33/2.69, gold 1.45/1.67, violet 2.36/2.72 — all
below 3:1 (gold worst; MiniBrowser screenshot confirms it washes out on white). Dark themes pass.
The failure set is pinned in `theme.test.ts` (suite stays green, regression trips it). **Palette
decision is the architect's** — left unchanged.

## [2026-07-29] decision | Esc priority — one handler, explicit branch

With fullscreen added, Esc has two possible meanings (exit fullscreen; clear selection). Rule:
**in fullscreen, Esc exits fullscreen and does nothing else; otherwise it clears the selection.
One Esc = one action.** Implemented as a SINGLE keydown handler whose first branch reads a
`fullscreenRef` — the precedence is a code branch, NOT a race between two separately-mounted
keydown effects (whose order would depend on mount order and silently flip under refactor). The
visible Exit button is the discoverable path; Esc is the shortcut.

## [2026-07-29] session | 2.5.2e-2: fullscreen viewer, background themes, measurement vertex marking

Viewer polish; no geometry, no sidecar. Three pieces.

**Themes (`viewer/theme.ts`, pure).** Four `ViewerTheme` presets; `MoleculeViewer` takes a `theme`
prop (default `dark` = the old look exactly, a no-op). Background via `setBackgroundColor` in a
`[theme]` effect (no model reload, no `zoomTo`); halo/label/measurement colours read from the
theme in the overlay effect. See the presets-not-picker decision for the contrast invariant and
the FRAGMENT_PALETTE light-theme failure (reported, palette untouched). Persisted under
`settings.viewer_theme` via the existing `get_settings`/`set_setting` (no schema migration).

**Fullscreen — the main risk was a REMOUNT.** A remount would `viewer.clear()` + re-`createViewer`
(context re-init is WebKitGTK's fragile spot, `debugging/002`) and reset the camera exactly when
the user enlarged to look closer. So fullscreen changes ONLY a CSS class on the viewer container
(`position: fixed; inset 0`); `MoleculeViewer` keeps its React tree position (same `scene ?`
branch), never remounts. **Concrete proofs:** (1) module-level `viewerCreateCount`, dev-logged on
every `createViewer` — a fullscreen toggle must not tick it; (2) MiniBrowser probe with a
`getView()` snapshot across a `position: fixed` class toggle → title
`RO-fired=2 cameraSame=true maxDelta=0.00e+0`: the `ResizeObserver` fires (so `viewer.resize()`
runs, no explicit call) and the camera view matrix is bit-identical. Esc rule in its own decision.

**Measurement vertex marking (for 2.5.2d).** All halos are identical, so the angle vertex / dihedral
axis wasn't visible. `drawMeasurement` now marks it geometrically — **angle:** a solid arc at the
vertex (`drawAngleArc`, slerped segments, radius ≤ shorter arm); **dihedral:** the j–k axis as a
thick `addCylinder`, outer bonds thin dashed. **No second number on the atom** — the "one number =
global index" rule from e-1 holds; click order is shown by geometry, not digits.

**e-1 leftover answered.** Numbering ~50 atoms (MiniBrowser): labels build ~94 ms once, per-frame
re-render ~1 ms → rotation stays smooth, all 50 legible. Label `fontSize` is screen-space
(constant px), so it stays readable in fullscreen — no dynamic sizing needed.

**Verified.** `tsc` + `vite build` clean; `vitest` **199** (was 188 → +11: `theme.ts` WCAG
contrast maths, per-theme overlay ≥3:1, pinned palette failures); `cargo test` **55** (Rust
untouched). MiniBrowser screenshots archived (`/tmp/themes-stacked.png`, `theme-white-solo.png`,
`camera-proof.png`, `perf-proof.png`). In-window checks needing the real Tauri window: theme
survives an app restart; fullscreen enter/exit from the real window keeps the racurs; light/white
readability at full size. Next: 2.5.2d — apply a measured d/θ/φ back through the ASE kernel.

## [2026-07-29] decision | Fragment palette becomes a theme property (hue vs lightness)

2.5.2e-2 reported that `FRAGMENT_PALETTE` fails 3:1 on the light themes and left the call to the
architect. Decision: **the global `FRAGMENT_PALETTE` constant is NOT changed** — it is shared with
`FragmentList`'s swatches, which sit on the app's dark side panel and read fine there. Instead the
palette becomes a **per-theme property** (`ViewerTheme.fragmentPalette`): dark/black keep the four
current colours; light/white use the same four **hues at lower lightness**
(`#0f766e #e11d48 #a16207 #7c3aed`), each ≥3:1 against the light backgrounds.

**Named compromise:** on a light theme, the sidebar swatch (bright, global palette) and the viewer
fragment (darker, theme palette) are the *same hue at different brightness*. Identity rides on hue,
legibility on lightness. The alternative — one palette everywhere — means an **invisible fragment**
on the light themes, which is strictly worse than a brightness mismatch. `theme.test.ts` locks the
hue tie (light variant within ±15° of its dark counterpart) so "same colour, darker" can't drift
into "different colour".

## [2026-07-29] session | 2.5.2e-3a: light-theme legibility — select, CPK overrides, per-theme palette

The light themes were unusable — two defects, both spotted on a real-window screenshot. No layout,
no fullscreen (that's e-3b), no geometry.

**1. `<select>` regression (fixed first).** The debugging/003 WebKitGTK `-webkit-appearance: none`
fix lived on the `.select` **class**; e-2's `.viewer-theme-select` was a new class and silently
missed it, so WebKitGTK drew the native GTK widget with dark text. Moved the rule to the **element
selector** `select` (`.select` kept as alias) — every `<select>` is now covered by default. All
five selects in `src/` grepped and confirmed under the rule; `.viewer-theme-select` reduced to
cosmetic tweaks (no `background` shorthand, which would wipe the chevron). debugging/003 amended:
the lesson is "the fix lives on the element selector", not "don't forget the class" — because it
was forgotten. MiniBrowser-verified (fixed = light text + chevron; broken reference = dark native).

**2. CPK colours vs the theme background — the invariant was too narrow.** The e-2 contrast test
checked `FRAGMENT_PALETTE` (fragments 1+), but fragment 0 is drawn in **CPK element colours**, and
CPK hydrogen is **white** — on white every hydrogen vanished (the BH₄⁻ screenshot). The test was
honest, it measured the wrong thing. Widened, not rewritten: `ViewerTheme.elementColorOverrides`,
empty on dark (untouched), covering the **13** elements that fail 3:1 vs `#eceff3` on light/white
(`H He B C N F Si P S Cl Fe Ba Au`) — each the same hue, darker; H/C become greys. Applied via
3Dmol `colorscheme: {prop:"elem", map: {...elementColors.defaultColors, ...overrides}}` (default CPK
= `elementColors.rasmol`, confirmed in the bundle). CPK table transcribed into `theme.ts`
(`CPK_ELEMENT_COLORS`, a documented dup — the 3dmol bundle can't load in node), guarded by
`cpkColorDrift` in dev. MiniBrowser: BH₄⁻ on white → 4 H grey, B dark-green, all legible.

**3. Fragment palette as a theme property** — see the decision above (global constant untouched;
light/white get darker same-hue variants; `FragmentList` stays on the global palette).

**4. Round fragment swatches** (`border-radius: 50%`, `FragmentList` + `AtomInspector`) — a hollow
square swatch beside the real "Numbers" checkbox read as an unchecked checkbox.

**Contrast numbers (report).** CPK elements failing 3:1 on light `#eceff3` / white `#ffffff` (13,
identical sets): H 1.15/1.00, He 1.33/1.54, B 1.19/1.37, C 1.45/1.67, N 2.41/2.78, F 1.94/2.24,
Si 1.94/2.24, P 1.71/1.97, S 1.34/1.55, Cl 1.19/1.37, Fe 1.71/1.97, Ba 1.71/1.97, Au 1.94/2.24 —
all overridden to ≥3.2 vs `#eceff3`. Fragment palette light/white (vs `#eceff3`/`#fff`): teal
4.75/5.47, rose 4.07/4.70, gold 4.27/4.92, violet 4.94/5.70; hueΔ vs dark 2.9/4.5/7.8/7.0 (≤15°).

**Dark themes unchanged — proof beyond "looks the same":** dark/black ship empty overrides, so
`cpkBaseStyle` returns the literal old `baseStyle()` object, and `fragmentPalette ===
FRAGMENT_PALETTE`; `theme.test.ts` asserts both.

**Verified.** `tsc` + `vite build` clean; `vitest` **210** (was 199 → +11: CPK exact-cover /
no-redundant / ≥3:1, per-theme palette + hue ±15°, `cpkColorDrift`, `hueOf`); `cargo test` **55**
(Rust untouched). Screenshots archived (`/tmp/bh4-white.png`, `select-proof.png`, `frag-light.png`).
In-window checks needing the real Tauri window: BH₄⁻ + 3-fragment on light/white, dark before/after.
Next: 2.5.2e-3b — layout / side-rail.

## [2026-07-29] decision | Distinctness invariant for annotation colours (halo, measurement)

A review finding (verified on the 3Dmol bundle): `rasmol` (28 elements, 3Dmol's default scheme) has
NO Pd/Pt/Rh/Ru, so 3Dmol paints them `defaultColor` #ff1493 — `hueDistance` **1.05** from the old
halo #ff2d95, i.e. every ADR-007 metal atom looked permanently selected. The one-way `cpkColorDrift`
couldn't catch this: it iterated OUR keys, so "3Dmol has an element we lack" was invisible by
construction.

**Decision:** app annotation colours (selection halo, measurement line) are governed by a
**distinctness invariant**, not only contrast. `theme.test.ts` asserts, per theme, that halo and
measurement sit **≥30° in hue** (`hueDistance`) from every element colour (CPK *with* the theme's
overrides), from `defaultColor` #ff1493, and from every fragment-palette colour. Same doctrine as
contrast: the test states the requirement; the constant satisfies it.

Consequence: the halo could not stay pink. The search showed the **only** hue band clear of the
whole CPK/palette wheel is **chartreuse (74–90°)** — CPK occupies red 0, gold 35–48, green 120,
teal/cyan/blue metals 172–207, blue 240, purple 255–277, pink 328–351. Rejected candidates: cyan/teal
(≈180 — the metals Ru/Rh/Pd live there), orange/amber (≈40 — F/Si/Au/P/Fe/S), any pink/magenta (the
defaultColor and coral/rose palette), blue (Na 240). Chosen: halo **#adee2b/#5e8b04** (dark/light),
measurement **#b1eb70/#519504** — both ≥3:1 on all four backgrounds. This is the one e-3b change to
the dark theme; its background/CPK/palette are untouched. The drift guards are now two-directional
(`{changed, missing}`), closing the blind spot in both the colour and the vdW table.

## [2026-07-29] session | 2.5.2e-3b: element-colour distinctness + fullscreen workbench rail

Two parts. No geometry, no sidecar.

**A — colour distinctness.** See the decision above. Verified against the bundle, not memory:
3Dmol's default CPK is `elementColors.rasmol`; the ADR-007 metals are absent → `defaultColor`
#ff1493. Added Pd/Pt/Rh/Ru/Ir/Os to `CPK_ELEMENT_COLORS` from 3Dmol's own `Jmol` table (source named
per value); Pt (near-white) also gets a light-theme override. Halo/measurement moved to chartreuse
under the new `hueDistance` ≥30° invariant. `cpkColorDrift` and `vdwTableDrift` are now
two-directional; a `missing` test feeds a reference an extra element and asserts it surfaces. Two
3Dmol facts pinned in `visualization.md`: (1) a `{prop:'elem',map}` colorscheme is honoured by the
"discrete property mapping" branch of `getColorFromStyle` — the earlier `scheme[atom[scheme.prop]]`
branch is always false for that object shape; (2) 3Dmol's XYZ parser canonicalises the symbol as
`elem[0].toUpperCase()+substring(1).toLowerCase()` (= our `normalizeElement`), so map keys match and
the uppercase `rasmol` aliases (HE/LI/NA) are PDB-only and irrelevant. `orca/gotchas.md` gained the
element-keyed-table rule (this is the THIRD such hole: H–Kr atomic numbers in 2.5.0, vdW in e-1,
colours now — always at the ADR-007 metals). MiniBrowser: a Pd complex renders Pd dark-teal with a
chartreuse halo on a selected neighbour — no collision.

**B — fullscreen workbench rail.** The fixed class moved from `.viewer-panel` to `.viewer-column`:
the column now holds the viewer AND a `.viewer-rail` (AtomInspector + FragmentList) in ONE DOM
structure. Normal = column (viewer over rail, visually unchanged — the acceptance criterion).
Fullscreen = fixed row (viewer stretches, rail 320px right, collapsible via a toolbar button, not
persisted). The rail is a **single shared instance** — never a duplicate AtomInspector (that would
fork selection state); it's a section list built so 2.5.2c/d ADD a section.
**Main risk was a silent remount reintroduced by the layout rebuild** — re-proven FRESH, not
inherited: a MiniBrowser probe drove the exact new operation (flex container → `position:fixed;
flex-direction:row` + sibling rail) and checked **canvas DOM node identity** (a remount replaces the
node, tearing down the WebGL context) alongside `getView`: `sameCanvas=true cameraSame=true RO=2
maxD=0`. In-app witness stays `viewerCreateCount`.

**Verified.** `tsc` + `vite build` clean; `vitest` **219** (was 210 → +9: distinctness invariant per
theme, metals present, two-directional `changed`/`missing` for both tables; Part B is layout/CSS, no
unit tests); `cargo test` **55** (Rust untouched). Screenshots archived (`/tmp/pd-distinct.png`,
`/tmp/workbench-proof.png`). New page `wiki/architecture/prior-art.md` (author's practical experience,
marked as such — the d/θ/φ primitive exists elsewhere, but nowhere is the mask scene-derived nor the
index unbroken to the ORCA launch). In-window checks (rail in fullscreen dark/light, collapse,
normal-mode before/after) in the author checklist. Next: 2.5.2c — the ASE geometry kernel.

## [2026-07-29] session | 2.5.2c: ASE geometry kernel in the sidecar

The geometry core: `POST /geometry/set-internal` (`sidecar/app/geometry.py`) sets a
distance/angle/dihedral to a target by moving a masked subgroup, on ASE. Python + pytest only; no
frontend (that's 2.5.2d).

**This is a silent-error surface** — a wrong result doesn't crash, it returns coordinates ORCA
computes *other* chemistry from. So two tripping guards, not one test set:

1. **Convention tripwire — FIRED and PASSED.** The question we set two sessions ago: does ASE's
   [0,360) dihedral match what `measure.ts` pinned? Carried the SAME butane coordinates into pytest:
   ASE `get_dihedral(0,1,2,3)` = **179.998** (anti) / **67.523** (gauche) to 3 dp — exact. No
   divergence between the ASE call and `measure.ts`; the convention fixed in 2.5.2b is sound against
   ASE 3.29.0.
2. **Post-conditions INSIDE the endpoint**, not only in tests: atom count unchanged, element
   sequence unchanged positionally, `measured` within tol of target (1e-6 Å / 1e-4°, dihedral
   compared circularly) → else **500 with a diagnostic**. The count/order invariant (ADR-008) now
   crosses the process boundary, so it's checked where the coordinates are produced.

**ASE signatures — checked against the installed venv (3.29.0), not memory.** `set_distance` /
`set_angle` / `set_dihedral` each take both `mask=` (boolean array) and `indices=` (list of atom
indices; overrides mask). Chose **`indices=`** — the request already carries a list. Read the actual
bodies: `set_distance` needs **`fix=0`** with an a0-excluding mask (its loop only applies the a1-side
term to masked atoms; the a0-side term is dropped, so fix must put 100% on a1 or the distance is
silently wrong). `set_angle` rotates about the vertex a2; `set_dihedral` about the a2–a3 axis and
warns a4 must be in the mask. Full mapping in `wiki/modules/sidecar.md`.

**Reference-atom rule (validation, 422).** Last atom of the chain IN the mask, all preceding NOT —
the operational form of "reference atoms from the substrate side" (2026-07-28 decision). Enforced in
the endpoint with a message that names the cause, so a mask with a reference atom is refused, not
silently applied (which would let op 2 undo op 1).

**Sequential acceptance test (2026-07-28 decision — "sequential" is no longer an assumption).**
Carbonyl (H2C=O) + hydride, mask = the reagent. Three SEPARATE endpoint calls, each input = previous
output: set_distance(C,H⁻) → set_angle(O,C,H⁻) → set_dihedral(Ha,O,C,H⁻). Recomputed from the FINAL
coordinates vs the TARGETS: **d 1.5 → 1.50000000, θ 107.0 → 107.000000, φ 90.0 → 90.000000**;
substrate internal pairwise geometry unchanged to **1e-9**. The Bürgi-Dunitz one-pass convergence
holds against real ASE.

**Verified.** `pytest` **25** (was 11 → +14: tripwire, each op, sequential acceptance, idempotence,
rigid-motion invariance, six validation cases). `vitest` 219 and `cargo test` 55 untouched (nothing
in their zones changed). ASE **3.29.0**. Live `uvicorn` + `curl`: a real set-distance call (hydride
moved, substrate frozen, `max_static_displacement 0.0`) and a reference-atom-in-mask → 422 with the
explanation. Next: 2.5.2d — the frontend wires this endpoint (fetch to 127.0.0.1:{port}), preview,
edit mode.

## [2026-07-29] decision | Edit mode: the mask is VISIBLE before Apply

The design decision of 2.5.2d. GaussView/Chemcraft get this right and it's easy to lose because we
have an "obvious" answer (the reagent fragment). Before Apply the user must SEE which atoms will
move — the moving fragment glows in the viewer whenever `planEdit` is `ready`. The default (the
fragment of the last-clicked atom) is *shown*, not silent. Implementation: `NewJobScreen` passes
`plan.mask` to `MoleculeViewer`; the mask is a **solid translucent glow**, distinct in FORM from the
selection halo (a wireframe cage). **Colour distinctness has a hard limit here:** no hue is ≥30°
from every element/palette/default colour AND from the chartreuse halo — the only element-safe band
IS the halo's (`theme.test.ts` proves it per theme). So the mask reuses the halo hue and is set
apart by form, not hue — a reported exception, not a faked test.

## [2026-07-29] decision | Edit mode scope: inter-fragment only; intra-fragment refused HERE

2.5.2d does ONLY inter-fragment edits, where the mask is a WHOLE fragment (the reagent-vs-substrate
case). Editing an internal coordinate of one molecule (rotating the substrate's own torsion) needs a
bond-graph split with ring detection to decide which atoms move — a separate unit (**2.5.3**). An
intra-fragment selection is **explicitly rejected in `planEdit` with the reason** (not silently
applied to the whole fragment, which would translate the entire molecule instead of a part). The
rejection mirrors the sidecar's reference-atom rule client-side, so the user learns it from the UI,
not from a 422 after clicking Apply; the server check stays the boundary guard.

## [2026-07-29] session | 2.5.2d: edit mode — set distance/angle/dihedral from the viewer

Stitches 2.5.2a (pick) + 2.5.2b (measure) + 2.5.2c (sidecar kernel) into a working editor. Two
decisions above (visible mask; inter-fragment scope). Also fixed a fact error: `sidecar.md:94`
claimed `indices=` overrides `mask=` "in all three" — in ASE 3.29.0 the precedence DIFFERS
(`set_distance`: non-empty `mask` overwrites `indices`; `set_angle`/`set_dihedral`: `indices`
overwrites `mask`). We pass only `indices=` (mask left `None`), so our calls are unaffected; wording
corrected.

**Pure planner (`edit-plan.ts`).** `planEdit(scene, selection)` → `ready` (op/indices/mask/current/
unit/movingFragmentId) or `unavailable(reason)`. Math from `measureSelection` (not duplicated); mask
= `fragmentAtomIndices` of the last-clicked atom's fragment; intra-fragment refused via the mirrored
reference-atom rule. Two more pure, tested helpers: `applyResponseToScene` (slice the moving
fragment's rows by range → `replaceFragmentAtoms`) and `applyResponseIssue` (our-side boundary check
before mutating: static atoms unmoved `< 1e-6`, count matches).

**Preview touches ONLY the viewer.** `EditPanel` POSTs to `/geometry/set-internal` and hands the
result up as a `previewScene` the viewer renders (`scene={previewScene ?? scene}`). The store Scene
and Monaco are untouched until Apply — **proof it doesn't touch them:** `onPreview` is wired to
`setPreviewScene` (local `useState`), while the only writer to the store is `setScene`, called
solely in `applyEdit`; the Scene→Monaco inject effect depends on the *store* scene selector, which a
preview never sets. Apply → `applyResponseIssue` → `applyResponseToScene` → `setScene` (normal
injection) → one-step Undo notice (restores the pre-edit scene).

**Verified.** `tsc` + `vite build` clean; `vitest` **234** (was 219 → +15: planner incl. click-order
& intra-fragment, slice, boundary check; theme mask-hue-necessity per theme). `pytest` 25 and
`cargo test` 55 untouched (their zones unchanged). Live `uvicorn` + `curl` with the EXACT request
`EditPanel` builds (water+BH₄⁻, `op=distance`, `mask=[3,4,5,6,7]`, `value=1.8`): water frozen, BH₄⁻
moved, `measured 1.8`, `max_static_displacement 0.0`. In-window checks needing the real Tauri window
(the full carbonyl+BH₄⁻ d→θ→φ sequence by hand, re-measuring after each apply; intra-fragment
refusal; Undo; camera doesn't jump on Apply) in the author checklist. Next: 2.5.3 — bond-graph mask
split for intra-fragment edits.

## [2026-07-29] session | 2.5.2d-1: detect a stale sidecar (version handshake) + human errors

Diagnostic unit. 2.5.2d's code is correct; the trap is that the app can silently talk to an OLD
sidecar and report it as the word **"Not Found"**. **Reproduced, not hypothesised:** with the app
running, a route the running build lacks returns `HTTP 404 {"detail":"Not Found"}` — because
`npm run tauri dev` hot-reloads only the frontend while `SidecarManager::start` launched uvicorn once
at startup without `--reload`, so Python keeps serving the code loaded when the window opened. Full
page: `wiki/debugging/005`.

**Version handshake.** `app/__init__.py` → `0.2.0`; rule recorded in `sidecar.md`: bump minor on every
endpoint add/change. Rust holds `EXPECTED_MIN_SIDECAR_VERSION`, parses `/health`'s `version`, and
compares **component-wise as numbers** (`version_at_least`) — string compare lies (`"0.10.0" <
"0.9.0"`). Older/unparseable → new `Health::Stale` (alive but old, distinct from `Down`); the status
bar shows **"Sidecar STALE — restart the app"** as a warning band (running vs expected version), not a
tooltip. `SidecarStatus` gained `version` + `expected_version`. 4 Rust tests (incl. `0.10.0` vs
`0.9.0`, unparseable → stale, `parse_health_version`).

**Human errors, one wrapper.** `src/sidecar-client.ts` (`postSidecar` + pure, tested
`describeSidecarError`) now backs all three callers (EditPanel, import-file, smiles): 404 → "older
build, restart" (names the route); 422 → `detail` verbatim; 5xx → `detail` prominently; network →
"isn't running". No caller emits a bare `Not Found`.

**`--reload` in dev — added, orphan-free.** Debug builds launch uvicorn with `--reload --reload-dir
app`. `--reload` spawns a worker child; `start` puts the sidecar in its own process group and
`stop`/`Drop` `killpg` the tree (the `debugging/004` discipline). **Verified live:** the tree is
supervisor + resource_tracker + worker sharing one pgid; `killpg` reaped all three, `pgrep uvicorn`
empty, port released. Benefit verified too: `touch app/geometry.py` → `WatchFiles ... Reloading`, new
worker, `/health` still serving.

**Spec-error fix (my mistake).** The ≥30°-hue distinctness rule is for overlays marking DIFFERENT
atoms (halo, measurement). The halo and the edit mask coexist on the SAME atom by construction (the
last-clicked atom is always in the mask), so they're distinguished by FORM (cage vs fill) + lightness,
and only background contrast is required of them — the mask reuses the halo hue as the RULE, not an
exception. `theme.test.ts` and `visualization.md` rewritten to split coexisting vs different-atom
overlays.

**Verified.** `tsc` + `vite build` clean; `vitest` **240** (was 234 → +6: `describeSidecarError`);
`cargo test` **59** (was 55 → +4: version compare / parse); `pytest` **26** (was 25 → +1: the health
test now asserts against `__version__` so a bump doesn't break it, plus a dotted-numeric check).
In-window checks (status bar reads STALE not healthy against an old sidecar; Preview shows the
older-build message) need the real Tauri window — author checklist. Next: 2.5.3 — bond-graph mask
split.

## [2026-07-29] decision | Click order is a DEFAULT, not a rule (edit orientation)

The spec error behind the 2.5.2d defect. 2.5.2d made "the LAST-clicked atom's fragment moves" a
hard rule, so the reagent-attack-angle edit `B#33(BH₄⁻)→C#12(ibuprofen)→O#14(ibuprofen)` was refused
as "same fragment" — the last atom's fragment (ibuprofen) held the reference C#12. But the SAME angle
read the other way (`O#14–C#12–B#33`) moves BH₄⁻ with both references static. **Decision:** click
order is only the DEFAULT tiebreak, never a solvability rule. `planEdit` tries both chain
orientations; angle/dihedral are invariant under reversal (`angle(i,v,j)==angle(j,v,i)`,
`dihedral(i,j,k,l)==dihedral(l,k,j,i)`, distance symmetric — verified in ASE 3.29.0 and
`measure.test` §f), so reversal changes only which end moves, not the value. When only one
orientation is valid, take it; when both are, keep click order as default and expose the other as an
"alternative" the user can switch to. **What caused the spec error:** conflating "the mask is a whole
fragment" (true) with "the mover is fixed by click order" (false); the direction of a chain is a UI
artefact, not a chemical fact, and must not gate whether the task is solvable.

## [2026-07-29] session | 2.5.2d-2: edit mode considers both chain orientations

Fix for a defect found on a real scenario (ibuprofen + BH₄⁻, author screenshot). See the decision
above for the root cause.

**planEdit (both orientations).** Candidate A = chain as clicked (mover last); candidate B = reversed
(mover first). Each checked against the reference-atom rule. Only-A / only-B / both (→ A default +
`alternative` = B) / neither. `EditPlan.ready` gained `reversed` and `alternative`; `swapToAlternative`
(pure) flips them for the UI. `current` is orientation-invariant (computed once).

**Two refusals instead of one.** All atoms in one fragment → the genuine intra-fragment bond-graph
reason (2.5.3). Atoms across fragments but no orientation valid (dihedral axis straddling fragments, or
an angle whose two ends share a fragment) → `immovablePivotReason`, which **names the offending atom
indices** (the references that would move with an endpoint either way). The old code lied "same
fragment" for both.

**UI.** The panel names the rotation pivot (`vertex C #12` / `axis C#12–O#14`), shows a calm "chain
read in reverse so the reagent moves" line when `reversed`, and offers "Move <other> instead" when an
alternative exists (finishing the redefinable mask half-built in 2.5.2d). Orientation choice lives in
`NewJobScreen` (`preferAlternative`), resets on selection/scene change, and drives both the mask glow
and the sidecar call.

**Verified.** `tsc` + `vite build` clean; `vitest` **245** (was 240 → +5: both-orientation planner
incl. the exact screenshot `[33,12,14]` → `reversed:true` moving BH₄⁻, the reversed-order value
match, `swapToAlternative` mirror, the two refusals naming `#0`/`#3`, and `measure.test` §f). `pytest`
26 and `cargo test` 59 untouched. **Manual acceptance driven to the end through the LIVE sidecar** (the
indices `planEdit` reverses to, reagent-first order): distance C–B → 2.2, angle O–C–B → 107, dihedral
Ha–O–C–B → 90; re-measured after EACH Apply — d = 2.200000, angle = 107.000000, dih = 90.000000,
substrate internal deviation **0.00e+00**. This closes 2.5.2d. Next: 2.5.3 — bond-graph mask split for
intra-fragment edits.

## [2026-07-29] session | 2.5.3a: bond-graph mask split in the sidecar (rotatable side of a bond)

Unblocks intra-fragment edits — rotating a molecule's own torsion (side-chain conformation, OH
orientation, aryl-ring flip), which the editor could not do (planEdit refused, pointing here).
Sidecar only (Python + pytest); the frontend wiring is 2.5.3b.

**New endpoint `POST /geometry/rotatable-mask`** (`app/geometry.py`, same router/style as
set-internal). `{ xyz, cut:[i,j], moving, scale }` → `{ mask, static_count, cut_length }`. Perceive
bonds → graph → remove the cut edge → the component containing `moving` is the mask.

**Main risk = perception is a guess.** Bonds come from `ase.neighborlist` (checked against ASE
3.29.0): `natural_cutoffs(atoms, mult=scale)` (per-atom covalent radius × scale) +
`neighbor_list("ij", …)` (bond iff `d < cutoffs[i]+cutoffs[j]`). The editor itself makes geometries
where the guess fails (stretched bond vanishes; ~2.2 Å reaction distance spuriously bonds), so: the
multiplier is an **explicit param**, perception is **tested against valence**, and the endpoint
**refuses rather than guesses**.

**Multiplier — measured, not assumed.** ASE's default `mult=1.0` is too tight (butane → 0 bonds!).
Counts vs known valence: `mult` in [1.1, 1.3] all give butane 13 / benzene 12 / **BH₄⁻ 4** (charged
trap, same multiplier as neutrals) / water 2. Chose **1.2** — mid-plateau, tolerant of slightly
stretched real bonds, still below the threshold that would bond a 2.2 Å reaction distance (C···B →
1.92 Å). Test (a) is the gate.

**Refusals (422), each with what to do:** not bonded (names distance + threshold); **ring** (removing
the bond doesn't split the graph — `moving`'s side still reaches both cut atoms → "the bond is in a
cycle, pick a non-cyclic bond"; this IS the cycle detection, operational, no SSSR); `moving` on
neither side; >2 components before the cut (odd perception, names the count).

**Which bond to cut** (recorded for 2.5.3b): distance(i,j)→cut(i,j),move j; angle(i,v,j)→cut(v,j),
move j; dihedral(i,j,k,l)→cut(j,k),move l. For a dihedral, the axis atom k lands in the mover's
component but sits ON the axis — the endpoint **drops any cut atom that isn't the mover**, so the
reference atoms fall outside the mask automatically (exactly what set-internal's reference rule
needs).

**Acceptance (intra analogue of 2.5.2c).** butane dihedral anti → 60° via this mask through
set-internal: target **60.000000**; static side unmoved; **rigidity** — every pairwise distance
within each side unchanged (moving-side max dev **4.7e-11**, static-side **8.5e-11**) → a rigid
rotation, not a deformation; count/order preserved. Ibuprofen (from `/smiles-to-3d`): cut Cα–COOH →
mask = the carboxyl group **{C,O,O,H} = 4 atoms**, static_count 29.

**Verified.** `pytest` **34** (was 26 → +8). `vitest` 245 and `cargo test` 59 untouched (their zones
unchanged). `__version__` → **0.3.0** (new API — the handshake rule; else the version check loses
meaning on its second use). Live `uvicorn` + `curl`: butane cut(1,2)/move 3 → mask [3,9,10,11,12,13],
static_count 8, cut_length 1.527 Å; benzene ring bond → 422 cycle message. Next: 2.5.3b — planEdit
calls this endpoint so an intra-fragment selection becomes an edit instead of a refusal.

## [2026-07-29] decision | Perception sees the whole scene — `within` restricts the bond graph

Found on review of 2.5.3a: `/geometry/rotatable-mask` gets the xyz of the **entire scene**, not one
fragment. For a **metal–ligand** scene that's a trap. At `mult=1.2` the Pd–N threshold is **2.520 Å**
and Pd–C **2.580 Å**, but a real dative Pd–N bond is **2.05–2.15 Å** — well under threshold. So
perception **fuses the metal centre and its ligands into one molecule**, and an intra-substrate
torsion split would swallow the coordinated reagent into the mask.

**Decision: add `within: list[int] | None`.** When given, perception considers only bonds with BOTH
ends in `within`; the component universe is `within`; the mask ⊆ `within`. **Indices stay GLOBAL** —
`within` is a filter, not a re-basing (the 2.5.0 one-index-space rule holds). `cut`/`moving` must be
inside `within` (else 422). The frontend passes the editing fragment's atoms, so a split can never
reach across fragments. Pinned with a **computed** contact (Pd at 2.10 Å from a substrate C, below
the 2.580 Å threshold — from covalent radii, not a guess) + a paired trap test (mask swallows Pd
without `within`, clean with it). Request shape changed → `__version__` **0.3.0 → 0.4.0**.

## [2026-07-29] session | 2.5.3b: intra-fragment edits reach the UI

Wires 2.5.3a's endpoint into the editor: an intra-fragment torsion (a side-chain conformation) is now
an **edit**, not a refusal. Plus the two review items above (`within`) and a default fix.

**`planEdit` gains a third result `needs-split`** (`scene/edit-plan.ts`) — `{ op, indices, current,
unit, cut, moving, within }`. When ALL chain atoms are in one fragment, the planner describes WHAT to
ask (the sidecar's cut rule: distance→(i,j), angle→(v,j), dihedral→(j,k); moving = last chain atom;
within = the fragment's atoms) but stays **pure & synchronous** — it doesn't fetch.

**The UI resolves the mask** (`NewJobScreen`, not the panel — it already owns preview/undo/mask). A
**race-guarded** effect (keyed on op+indices; cleanup sets `cancelled`) POSTs cut/moving/within to
`/geometry/rotatable-mask`; **a response for a stale selection is dropped** so it never lands on the
new pick. The resolved mask drives BOTH the glow AND `set-internal` — **one source, not two**.
`EditPanel` takes it as `splitMask`/`splitResolving`/`splitError`, shows *"Internal edit · rotating
about C#i–C#j"* + *"finding the rotatable atoms…"* while waiting, and surfaces a 422 (ring bond) in
the sidecar's own words. The moved subset replaces its fragment's rows in place (unmoved rest
unchanged), so `applyResponseToScene` is reused as-is.

**Default fix: the SMALLER fragment moves** (was: last-clicked). When both orientations are valid,
`planEdit` picks fewer atoms (ties → click order); "Move X instead" still flips it. ibuprofen(33) +
BH₄⁻(5) in ANY click order → BH₄⁻ moves by default — almost always what the chemist means.

**Verified.** `tsc` + `vite build` clean; `vitest` **248** (edit-plan: needs-split per op, single-
fragment scene, smaller-fragment default independent of order); `pytest` **38** (the `within` tests,
incl. the computed-contact trap); `cargo test` **59** untouched.

## [2026-07-29] ingest | ORCA `%geom Constraints` is 0-based — settled by a real run

The first-session "Question C" is closed. Not by memory (`gotchas.md` forbids trusting memory about
ORCA), but by an ORCA 6.1.0 run designed so the two interpretations produce *visibly different
geometry*: **chloromethane**, atom order `Cl, C, H, H, H` (so a one-index shift changes the bond
*type*: C–Cl≈1.78 vs C–H≈1.09), constraint `{B 1 2 1.234 C}` with an **explicit value** distinct from
both. After the opt the **C–H** bond (atoms 1,2 = C,H under 0-based) sat at exactly 1.234 Å and C–Cl
relaxed to 1.80 Å; ORCA's own redundant-internal table printed `B(H 2, C 1)` — carbon = atom **1**,
chlorine = atom **0**. Unambiguously **0-based**. Bonus: an out-of-range `{C 5 C}` on the 5-atom
molecule **segfaulted** (ORCA does no bounds check) → range-check indices before writing a constraint.
Full input, output lines, and the in-range control run in `wiki/orca/constraints.md`. ORCA dirs removed
(rule 3).

## [2026-07-29] decision | Input text is the source of truth for constraints

Constraints live in the **ORCA input text**, exactly like the `!` keyword line and the geometry block —
`constraints.ts` (2.5.4a) parses/injects, and the 2.5.4b UI panel will be a **view over the text**, not
a parallel store. Why: a second home for constraints would drift from the input the same way a parallel
Scene would drift from the coordinate block if `xyzMatchesScene` didn't force the comparison. The
invariant `parse(inject(x, cs)) === cs` is a test. No separate DB column — the input *is* the storage.

## [2026-07-29] session | 2.5.4a: ORCA constraint block (generate/parse), index base settled

**Index base settled first** (the ingest entry above) — the whole point of the question, done with a
real run before any code.

**Pure `constraints.ts`** — `Constraint` (B/A/D/C, optional value), `ORCA_INDEX_BASE=0` +
`toOrcaIndex`/`fromOrcaIndex` (identity, but routed through the constant so the code states the fact),
`constraintsBlock` / `parseConstraintsBlock` / `injectConstraints`. Injection replaces or inserts the
`Constraints` sub-block **without disturbing sibling `%geom` settings** (maxiter survives) via a
depth-counting token scan that tells the inner `Constraints … end` from the outer `%geom … end`; parse
strips `#` comments (a commented-out block never parses live) and returns `null` on a malformed line
(never silently drops). The module's emitted separate-line form was itself run through ORCA 6.1.0 to
confirm it's valid input.

**Review hole from 2.5.3b, fixed (item 0).** Butane `angle(3,1,2)` → `needs-split`, cut (1,2), moving 2;
`/geometry/rotatable-mask` returned a mask **containing reference atom 3** (it's on the rotatable side),
and `set-internal` 422'd at Apply. The reference-atom rule was checked in `planEdit` but not re-run
after the split mask arrived. Fix: `maskRoleViolation` is now **one pure function on both paths**
(`orientationFor` inter-fragment; `NewJobScreen` post-split); a violation refuses with an explanation in
**selection terms** (`explainSplitViolation`: "atom C#3 lies on the moving side of the C#1–C#2 bond …"),
not the sidecar's inter-fragment wording. Butane test locks it.

**Verified.** `tsc` clean; `vitest` **248 → 275** (+21 constraints round-trip/inject/comment-safety,
+6 the split-mask rule incl. the butane repro); `pytest` **38** and `cargo test` **59** untouched (this
was frontend-only). Two real ORCA 6.1.0 runs (index base + emitted-format validity), dirs cleaned.

## [2026-07-29] decision | The one place the app blocks a run on input CONTENT: an out-of-range constraint index

OrcaStudio never blocks a run because it dislikes the input — except here. A constraint index outside
`[0, atomCount)` makes ORCA read past the atom array and **segfault with no diagnostic** (documented
2.5.4a). So `constraintIndexIssues` gates Create AND Create & Run (both — the job's input is immutable
once created, so a "draft" with a bad index is an un-runnable landmine). This is deliberately narrow:
the justification is that the cost of *not* blocking is a crash with no error message, not merely a
wrong result. Every other input mistake (bad parity, odd chemistry) is a warning, never a block.

## [2026-07-29] session | 2.5.4b: constraint panel as a view over the input text + two guards

The constraint UI. Non-negotiable shape (2.5.4a decision): **a view over the input text, not a parallel
store.** `ConstraintPanel.tsx` reads `parseConstraintsBlock(content)` and nothing else — a block
hand-edited in Monaco shows up with zero extra wiring (a test asserts different text → different list).
Each row shows the atoms in our terms (`describeAtom`: "C#12 (Ibuprofen) ··· B#33 (BH₄⁻)"), the set
value and the current measured value (`measureSelection`), and a `×` that deletes by re-injecting the
remaining list.

**"Constrain selection"** (`AtomInspector`, 2/3/4 atoms, kind by length) adds a constraint WITHOUT a
value (freeze as-is — the TS-guess case), optional value field beside it. It calls back into
`NewJobScreen`, which parses the text, appends (dedupe via `sameConstraint`), and writes through
`injectConstraints` — **one data path**, the panel re-reads the text. No parallel state, no shortcut.

**ЗАХИСТ 1 (range).** `constraintIndexIssues(cs, atomCount)` (pure) — see the decision entry above; the
panel marks bad rows red, `NewJobScreen` blocks both create buttons with a message naming the constraint
and indices and stating ORCA segfaults rather than erroring.

**ЗАХИСТ 2 (composition).** Uses the EXISTING `compositionSignature` (no second notion): when it moves
while constraints exist, a warning above the panel lists what each constraint names NOW (element +
fragment) so verification is by eye. Catches the in-range-but-wrong case a range check can't. We do NOT
rewrite the text or remap indices — "the same atom after a removal" has no operational definition, the
same call as `selectionSurvives` (2.5.2a). Warning clears on Dismiss or when no constraints remain.

**Verified.** `tsc` + `vite build` clean; `vitest` **275 → 286** (+11: range guard incl. the 38→33-atom
case, `constraintFromSelection`, panel round-trip, delete-preserves-block+%geom, manual-edit-reflected,
dedupe); `pytest` **38** / `cargo test` **59** untouched (frontend-only). No ORCA run — the segfault is
already documented; the whole point of this unit is never to reach it. Also fixed a stale comment in
`AtomInspector` that still said the index base was unsettled.

## [2026-07-29] decision | xTB pre-optimization lives in Rust, not the sidecar (deviates from ROADMAP)

ROADMAP said "sidecar endpoint `/xtb-optimize`". We build it in **Rust** instead. Why: (a) Rust owns
process spawning, and with it the isolated-directory rule (#3) and the kill-the-whole-group discipline
(`debugging/004`) — xtb can leave children/threads, same class of problem; (b) the binary path is a
**setting**, and settings live in SQLite under Rust; (c) the sidecar is deliberately ignorant of the
jobs dir and of settings — it understands the *chemistry of files*, it does not *run binaries*. The
sidecar stays that boundary; process orchestration stays in Rust next to the ORCA backend it mirrors.

## [2026-07-29] ingest | xtb `$constrain` is 1-based — and that is NOT ORCA's 0-based

Settled by a real xtb 6.6.1 run (not memory), same design as the ORCA experiment: chloromethane
(`Cl,C,H,H,H`), `distance: 1, 2, 1.234`. xtb echoed *"constraining bond 1 2 … actual value 1.7780 Å"* —
the initial value of pair `1,2` is the **Cl–C** distance (1.778), so `1,2` = atoms Cl(1),C(2): **1-based**.
ORCA's `%geom Constraints` is **0-based** (2.5.4a). OrcaStudio stores 0-based (ADR-008) → writes ORCA
as-is, xtb `+1`. **The two bases now sit side by side in `gotchas.md`** so they're never confused.
Second finding: xtb holds constraints by a **harmonic spring** (`force constant`), not rigidly — on the
realistic run (ibuprofen+BH₄⁻, C···B at 2.2 Å) the target held to **0.011 Å** at force constant 1.0;
the artificial extreme (compressing C–Cl 0.54 Å) deviated up to 0.12 Å. The post-condition tolerance
(0.1 Å distance) is sized from the realistic number — full data in `wiki/orca/xtb.md`. Both experiment
dirs removed (rule #3).

## [2026-07-29] session | 2.5.5: xTB pre-optimization (Rust) + constraint block made non-destructive

**Item 0 first — the 2.5.4b data-loss bug.** The panel rewrote the whole `%geom Constraints` block on
every add/delete, destroying (1) a `#` comment inside the block and (2) valid constraints sitting beside
an unknown token like `{X 9 9 C}` (parse→null→panel empty→rewrite from `[]`). Fix:
`inspectConstraintsBlock` → **`absent | parsed | unrecognised`**; **we rewrite only what we fully
recognised.** A comment-inside or an unparseable token → `unrecognised`; the panel is read-only, add +
delete + xtb are disabled, `injectConstraints` is never called. Numbers preserved as typed (`90.0`) via
`valueText`. Two regression tests reproduce both scenarios.

**xtb_optimize (Rust, `src-tauri/src/xtb.rs`).** Prepares an isolated dir (#3), writes `input.xyz` +
xcontrol (`$constrain`/`$fix`, every index **+1** for xtb's 1-based base), runs xtb by full path in its
own process group, reads `xtbopt.xyz`. **Post-conditions in the command** (not only tests): atom count,
element order, and **each constraint held within tolerance** (0.1 Å distance — measured; 5° angle; 0.01 Å
`$fix`) — the held-check also catches an index-base mistake (the intended pair would drift). Dir removed
after; `xtb_cancel`/timeout → `terminate_job` (killpg + cwd sweep, the ORCA primitives made `pub(crate)`,
one copy). `xtb_path` setting + `xtb_version` Check button in Settings. Frontend: rail button with
running/cancel state, `replaceAllAtoms` applies the result, Undo via the existing `applyEdit`/preEditScene.

**Experiments (both with numbers).** (a) index base — above. (b) realistic — ibuprofen(33)+BH₄⁻(5),
C(#12)···B(#33) held at 2.2 Å (`distance: 13, 34, 2.2`): held to **0.011 Å**, ibuprofen RMSD 0.67 Å (rest
relaxed), atom order preserved, **1.5 s**. The emitted xcontrol format was itself round-tripped through
xtb 6.6.1.

**Verified.** `tsc` + `vite build` clean; `vitest` **286 → 296** (+10: item-0 regressions + `valueText`
+ `replaceAllAtoms`); `cargo test` **59 → 68** (+10 `xtb::tests`: 1-based xcontrol per op, cartesian→
`$fix`, freeze-as-is, out-of-range rejected, held-check flags/passes, xyz parse — `2 ignored` pre-existing);
`pytest` **38** untouched (this unit added nothing to the sidecar — the whole point of the Rust decision).
xtb dirs cleaned. **Phase 2.5 (Scene / geometry editor / reaction-coordinate control) is complete.**

## [2026-07-29] session | 2.5.5-fix: run xtb OFF the main thread (window froze, cancel undeliverable)

**Found by reading the code, not by tests** — the defect is invisible to `cargo test` and shows on the
first click. `xtb_optimize` was a *synchronous* `#[tauri::command]`, so it ran the whole ~1.5 s xtb on
the **main GTK/WebKitGTK thread**: (1) the window froze for the entire run; (2) `xtb_cancel` (a separate
command) could not be delivered while the main thread was busy — the Cancel button was unreachable by
construction, and a UI-side timeout too. The project's own ORCA path already does the right thing
(`submit_job` returns at once; `drive_job` runs in `std::thread::spawn` and emits events).

**Fix — the existing pattern, not a new one.** `xtb_optimize` is now a **starter**: validate
synchronously (multiplicity, parse, resolve targets → out-of-range rejects immediately), **reserve the
single slot** (concurrent run rejected), spawn the worker thread, return. The thread runs `run_in_dir`
(spawn xtb in its own group, poll `try_wait` + the `cancelled` flag every 50 ms, post-conditions),
then **unconditionally** removes the isolated dir + frees the slot right after `run_in_dir` (outside any
`?`, so it holds on success / error / cancel / timeout — the guarantee most easily lost in the move to a
thread), then emits **`xtb:done`** / **`xtb:error`** (the job-log event convention). The slot now holds
**only the cancel flag** — `xtb_cancel` **just sets it and returns** (it runs on the main thread;
`terminate_job` sleeps up to ~12 s, so the worker thread — not the cancel command — does the killpg +
cwd sweep off the UI thread when it sees the flag). So neither the run nor the cancel blocks the window.

**Guarantees preserved:** single slot (reserved in the command before it returns), unconditional dir
cleanup + slot release, process-group kill (`terminate_job`). **Stale-result race** (a result arriving
after the user changed the scene): solved the SAME way as the 2.5.3b split-mask fetch — the frontend
captures the launch scene and applies the result **only to that exact reference** (`xtbResultApplies`,
now a named + tested pure guard); a changed scene → discarded, never clobbered. The event listener also
carries the 2.5.3b `cancelled`-flag cleanup.

**Verification — the honest state.** `cargo test` **68** (unchanged, no regression); `tsc` + `vite
build` clean; `vitest` **296 → 299** (+3 `xtbResultApplies`). The window is now structurally identical
to the proven-responsive `drive_job` path. **The physical acceptance test — rotate the molecule during
a run, hit Cancel mid-run, start a second run during the first — was NOT performed by me:** it needs
mouse interaction in the WebKitGTK window (no scripted-input tool here, and "feels responsive" is a
human perceptual check). Per the task, this manual run in the real window **remains the obligatory
acceptance step for the author.** I did not and do not report responsiveness as verified-by-me.

## [2026-07-29] session | 2.5.5-fix-2: diagnose the 300 s xtb hang — keep evidence, tail, live progress

A no-constraint pre-optimization (dexketoprofen, C16H14O3, 33 atoms) ran 300 s and timed out, and the
app made the cause **un-knowable**: `remove_dir_all` was unconditional, so `xtb.out` — the only record
of where xtb spent its time — was deleted exactly when needed. This unit builds the diagnostics FIRST,
then diagnoses; the invocation is unchanged (the fix is the author's call on this report).

**Diagnosis (xtb 6.6.1, terminal, measured; full table in `wiki/orca/xtb.md`).** The app's exact
command hangs at 99 % CPU with **0 optimization cycles** — a STARTUP hang, not convergence. It hangs
regardless of molecule (ibuprofen too), `OMP_*`, or opt level (`loose`/`crude`). The ONE variant that
converges (0.3 s, 16 cycles) is the one **without `--input xcontrol`**. **Root cause: `build_xcontrol`
returns an empty string for zero constraints, and the command still writes that empty `xcontrol` and
passes `--input xcontrol` — an empty `--input` file hangs xtb before cycle 1.** Every working 2.5.5 run
had a non-empty xcontrol (constraints); this was the first real no-constraint run.

**Rule #3 refined (my 2.5.5 spec error).** Rule #3 is about clearing ORCA-style scratch *litter* on
success — NOT about discarding *evidence* on failure (ORCA jobs keep their dir; that's why they're
debuggable). Cleanup is now split (`keep_dir_for_diagnostics`, pure + tested): **remove on
success/cancel, KEEP on any other failure.** The kept path rides the `xtb:error` payload and shows in
the UI as copyable text; the message carries the last ~20 lines of `xtb.out` via the shared
`read_tail_lines` (bounded tail, rule #5 — reused, not a second reader; the old `tail()` had read the
whole file). **Live progress:** the poll loop tails `xtb.out` ~1×/s and emits `xtb:progress { cycle }`;
the panel shows the cycle + a ticking clock, so a pre-cycle hang is visible immediately. **Open issue
(noted, not fixed):** kept dirs accumulate under `<data>/xtb/` — a reaper is deferred.

**Verified.** `cargo test` **68 → 70** (`keep_dir_for_diagnostics`: failure keeps / success + cancel
remove; `last_cycle` parse); `vitest` **299 → 301** (`formatXtbProgress`); `tsc` + `vite build` clean;
0 rust warnings. Diagnostic dirs cleaned. The invocation fix awaits the author's decision on the table.

## [2026-07-29] session | 2.5.5-fix-3: pass --input only when xcontrol has content; prune old diagnostic dirs

The fix for the 2.5.5-fix-2 diagnosis (an empty `xcontrol` passed via `--input` hangs xtb 6.6.1 before
cycle 1). Written up as `wiki/debugging/006-xtb-empty-input-hang.md`.

**One source, not an extra `if`.** The bug was three decisions drifting apart — *what content*,
*whether to write the file*, *whether to pass the flag* — and the content was empty while the file and
flag didn't know. `build_xcontrol` now returns **`Option<String>`** (`None` = nothing to write);
`None` → no file AND no `--input`, `Some` → both. Both reads come from the one `Option`. The argv is a
pure **`xtb_args(has_xcontrol, charge, uhf)`**, and `argv_includes_input_only_with_an_xcontrol` asserts
`--input` present with constraints, absent without — **the test that would have caught this in
milliseconds instead of five minutes**.

**Diagnostic-dir accumulation — closed.** `dirs_to_prune(entries, keep)` (pure + tested) returns all but
the `keep` most-recently-modified; `prune_diagnostic_dirs` scans `<data>/xtb/` at startup (off-thread)
and removes the rest, keeping `KEEP_DIAGNOSTIC_DIRS = 5`. **Newest-kept**, so a just-failed run's dir is
never pruned by the next launch (asserted). No setting.

**Verified with real xtb 6.6.1 on dexketoprofen (C16H14O3, 33 atoms), the exact argv the fix builds:**
- **no constraints** (fixed argv, NO `--input`, no `xcontrol` file) → **1 s, 16 cycles, CONVERGED**
  (was a 300 s timeout);
- **with a constraint** (`--input xcontrol`) → **<1 s, 17 cycles, CONVERGED, constraint applied** — so
  `--input` is not lost with the fix (the likely regression, checked explicitly).

`cargo test` **70 → 73** (`xtb_args` both states, `build_xcontrol` `None`/`Some`, `dirs_to_prune` keeps
newest / drops oldest); `vitest` **301** unchanged (no frontend change); `tsc` + `vite build` clean;
0 rust warnings. Verification dirs cleaned. **Phase 2.5 xtb pre-optimization is now working end to end.**
xtb 6.6.1's empty-`--input` hang recorded in `gotchas.md` + `xtb.md` as a named foreign-binary quirk
(upstream-issue candidate; not filed from here).

## [2026-07-29] lint | Wiki health-check after phase 2.5

Second full lint (first was after 2.5.0). Scope: the whole 2.5 arc — 2.5.2a–d, 2.5.3, 2.5.4,
2.5.5 + seven fixes, two empirically-closed index-base questions, one ROADMAP deviation (xtb in
Rust). **Checked:** the two index bases (ORCA 0-based / xtb 1-based) across pages **and** code;
the mid-phase decision reversals (visible+redefinable mask, distinctness invariant, reference-atom
rule checked twice, non-destructive constraint block); the three cross-cutting rules (order/count
invariant, no index remap, preview touches neither Scene nor input text); every file in
`src/scene/`, `src/viewer/`, `sidecar/app/`, `src-tauri/src/` against the module pages; ROADMAP
markers; orphans + links + page count. Reported first, fixed after author sign-off.

**Verified consistent — no change needed.** The two index bases sit side by side in `gotchas.md`
and match the code (`constraints.ts` `ORCA_INDEX_BASE=0` identity; `xtb.rs` writes `+1`). All four
mid-phase reversals describe the FINAL state in the authoritative module pages
(`scene.md`/`visualization.md`). The three cross-cutting rules are worded the same everywhere (the
no-remap rule for constraints explicitly cites the `selectionSurvives` call). ADR-008's freest-axis
amendment closed the 2.5.0 lint finding. `sidecar.md` does not claim xtb as an endpoint. No broken
links; no orphans; page count was correct (then 36).

**Fixed — mechanical (applied before the report):** `sidecar.md` version `0.3.0`→`0.4.0` (it
contradicted its own body + `app/__init__.py`); removed a duplicate `/parse` planned-endpoint line.

**Fixed — contradiction (b):** `frontend.md:357` still called the `%geom` index base "still open
empirically" (settled 0-based since 2.5.4a; the code comment was already fixed) → supersession
marker added, annotate-not-delete.

**Fixed — stale claims (c):** three module Status headers advanced to the end of phase 2.5
(`scene.md` was worst — "2.5.2 underway… Next: 2.5.2c"; `frontend.md` "Phase 2.5.0 complete";
`tauri-core.md` "Phase 2 complete"); `sidecar.md`'s "(Now 0.2.0)" reworded as a dated statement;
`frontend.md:486`'s intermediate "last-clicked default" got a supersession marker → smaller-fragment
default (2.5.3b).

**Fixed — gaps (g):** `scene.md` file list gained `ConstraintPanel.tsx` and `xtb-progress.ts`
(`formatXtbProgress`), both present in code but undocumented.

**Author-decided items applied:** ROADMAP `[ ]` "Fragment library … place at specified distance/
angle" left unchecked but annotated (library + `placeFragment` exist; guided one-step placement
does not), the "Phase 2.5 COMPLETE" line now names the deferral, and a new **Phase 2.6 backlog**
section carries it (+ the never-built constraint toggle). New **ADR-009** (running external
binaries belongs to Rust, not the sidecar — complements ADR-002/003, xtb precedent; ADR-002/003
NOT edited); the ROADMAP Open-Babel fallback now carries the pybel-library-vs-obabel-binary caveat.
New Ukrainian study note **`chemistry/burgi-dunitz.md`** (the ~107° attack angle, structure
correlation, NaBH₄ stereoselectivity, and the d/θ/φ→constraint→xtb bridge) — the chemistry gap for
the whole geometry-editor phase; uncertain points flagged inline, not smoothed over.

**Structural finding (the important one).** The five stale claims were not carelessness — they are
the tail of ~42 accumulated `As built (<unit>)` sections that turned the module pages into a second
chronicle competing with `log.md`. A page that records history drifts from the code by
construction. Codified a schema rule in `CLAUDE.md` (module pages are present-tense, current-state;
no new `As built` sections; history lives in `log.md`; a changed decision names the final rule +
one-line log pointer). **Consolidating the existing `As built` sections is deliberately deferred to
its own unit** — point fixes and a full rewrite of eight pages were kept out of this commit.

index.md page count 36→38 (+adr-009, +burgi-dunitz). No ADR rewritten; `resources/manual/`
untouched.

## [2026-07-29] session | Consolidate module pages to present-tense current state

The deferred unit from the phase-2.5 lint: collapse the ~42 accumulated `As built (<unit>)` sections
across `wiki/modules/*` under the schema rule (present-tense, current-state; history in `log.md`).
**No code touched — `wiki/` only.** Worked one section at a time: extract its facts, check each
against the page's current description / `log.md` / ADR / `gotchas.md`, and **move any fact found
nowhere else into the present-tense body before removing the chronological wrapper** — a lost fact
costs more than a spare paragraph.

**Collapsed, by page.** `parser.md` 2; `execution-backends.md` 2 + a "Correction" chronology folded;
`sidecar.md` 4 + the versioning section reframed; `tauri-core.md` 10; `visualization.md` a Phase-2.1
wrapper with ~15 dated subsections; `frontend.md` ~22 dated/`As built` sections. `scene.md` had no
`As built` sections (a function-contract page already in present tense) — only the one chronological
"Consolidation — closed in 2.5.0d" section was reframed to a present-tense boundary note.
`manual-index.md` untouched (not started). Each page now: 1–2-sentence Status, responsibilities +
boundaries, files/interfaces with one-line contracts, present-tense invariants, quirks, and a
`[YYYY-MM-DD]` log pointer where a decision changed.

**Facts that lived only in an `As built` section and were carried into the present body** (verified
by grep after the rewrite — the full set is large; the load-bearing ones): the offline-Monaco worker
path `editor/editor.worker.js?worker` + the exports-map double-map; the 8 templates and the
`%maxcore` no-`end` rule; `functionalHasBuiltInDispersion` / `builtInDispersion`; the input-builder
canonical keyword order + RI aux-basis pairing; `useLayoutEffect` store init, `didSubmit`,
listeners-first, `xtbResultApplies`, `valueText`, `inspectConstraintsBlock`, `preferAlternative`,
`snapshotRejected`, `keepScene`, `resetToken`/`navRef`, `formatXtbProgress`, the "above the loaded
window" viewer-truncation note, `627.509`, the `58vh` accordion cap; (Rust) `MAX_VIEWER_LINES`
300 000, `read_job_ensemble` + `MAX_ENSEMBLE_BYTES`, `KEEP_DIAGNOSTIC_DIRS`/`dirs_to_prune`,
`check_held`/`xtb_args`/`build_xcontrol` Option, `RESULT_TAIL_BYTES` 64 KB, the 431 KB/~3 ms search
measurement, the `errors` 0×/12 and `imaginary mode` preset findings, `version_at_least` +
`EXPECTED_MIN 0.2.0`, `pgid: 0` placeholder; (sidecar) `_COVALENT_SCALE_DEFAULT 1.2`, the Pd–N 2.520
/ Pd–C 2.580 / dative 2.05–2.15 Å trap, `proteindatabank`/`_ASE_FORMAT`, `fix=0`, the [0,360) fold
with 179.998/67.523, `maxIters=500`, `rdkit==2026.3.4`; (execution) `OMPI_MCA_hwloc_base_binding_policy=none`,
`align_pal_nprocs`, `sweep_job_processes`/`terminate_job` 10 s+2 s, `RunningJob{job_id,pgid,cancelled}`;
(viewer) `highlightRadius = vdw*0.3+0.25` floor 0.5, `MASK_OPACITY 0.22`, `AXIS_RADIUS 0.05`, the
chartreuse halo `#adee2b/#5e8b04`, the Jmol metal colours, the 13-element CPK contrast table, the two
`getColorFromStyle` colorscheme facts, `viewerCreateCount`; (parser) the SCF-gate against Freq
eigenvector rows, `GEOMETRY OPTIMIZATION CYCLE N`, the non-hardcoded criterion count.

**Dropped as chronicle (now only in `log.md`):** per-unit test counts (`vitest 296`, `cargo 68`…),
the repeated "GUI not headless-drivable, same limitation as prior phases" note, and the intermediate
halo hex `#ff2d95` (superseded by chartreuse — the "old pink halo, hueDistance 1.05" rationale is
kept, the literal is not). The frontend `State` section's aspirational `jobsStore`/`editorStore`/
`settingsStore` were relabelled **planned** (only `useSceneStore` exists) — a latent false
present-tense claim fixed in passing.

**Left as-is, deliberately:** `scene.md`'s inline `(2.5.xy)` provenance tags on function contracts
(they are provenance, not chronology, and mass-editing risks error — the schema rule allows a
one-line pointer); `manual-index.md`. Cross-refs checked: no page references a removed
`As built (…)` section; the two lint supersession markers (frontend.md:357/486) were removed with the
intermediate text they annotated. No broken links; page count unchanged (38).

## [2026-07-30] decision | Adopt the editor identity model (ADR-010), defer the renderer (ADR-011), reorder the roadmap

Planning/ingest unit against the author's design proposal
`architecture/proposals/editor-architecture-2026-07-30.md` (committed at `67c763c`). **No code
touched — `wiki/`, `CLAUDE.md`, `ROADMAP.md` only. No refactor started.** The proposal is an
*input*, not a decision; it was **not edited**. The author's review verdict governs: part
accepted, part deferred, three points corrected.

**ADR-010 (accepted) — editor identity & state model.** `AtomId` is opaque stable identity, not
an array position; branded `OrcaIndex`/`AseIndex` (no bare integer crosses a runtime boundary);
atom order matters in exactly one place — the input generator, which also hands back the inverse
table; `emit_input`/`parse_output` paired with an `IndexMap` (parse impossible without the emit's
map — a type invariant); state = fold over a typed op-log, with an ephemeral drag layer (60 fps
not logged, one op on mouseup); authority split by kind of data (text owns chemistry, Scene owns
geometry); product derived from reactant by ops, so atom mapping exists by construction. ADR-010
**refines ADR-008** (per-layer index discipline → locality in one typed module), does not
contradict it; ADR-008 not edited.

**Three corrections made during review** (each recorded with its reason so it is not
relitigated): **(i)** the sidecar returns POSITIONAL arrays — cclib/RDKit/ASE know nothing of
`AtomId` — so Rust builds the `IndexMap` at the boundary; without this ADR-010 would conflict
with ADR-002 and the Phase 3 cclib plan. **(ii)** bond perception has exactly ONE
implementation (today in the sidecar: `natural_cutoffs`, 1.2 multiplier, `within`, ring detection
via non-disconnecting cut); if it moves to core the sidecar LOSES its copy in the same change —
no "just in case" duplicate (the duplicated vdW/CPK-table history). **(iii)** the proposal's
"never show ORCA indices" rule is REJECTED — the app is a learning instrument; hiding ORCA's
language is harmful. Rule in force: never show a *bare* index without naming its space (as the
current `local index 3 · global index 3 (both 0-based)`).

**ADR-011 (proposed / deferred) — editor graphics stack.** Target: wgpu → WASM →
WebGL2/WebGPU in the webview, impostor spheres/cylinders, GPU picking returning `AtomId`.
Deferred because no phase-2.5 defect came from 3Dmol's index space (it is aligned — the model is
rebuilt from our merged xyz every render); the expensive authority-claimants are removed by
ADR-010 without pixels. Gated on a spike with **verifiable** exit criteria: wgpu-triangle under
`webkit2gtk-4.1` via the MiniBrowser bench (`debugging/002` technique); WebGL2 path works; WASM
bundle size measured as a number; GPU picking returns the correct id under the same webview.
Until then 3Dmol is a **dumb renderer** fed geometry + an `AtomId → viewer index` table, never a
source of truth.

**Two new CLAUDE.md domain rules (#9, #10)** — normative, not decisions: #9 every process
boundary has a post-condition that checks the result in OUR terms (recomputed `measured`,
`max_static_displacement`, atom count+order invariant), never trusting a foreign "success"; #10
no fact about a third-party program is accepted from memory/docs, only from a logged run (the two
opposite ORCA/xtb index bases, the empty `--input` hang, `mask` overriding `indices`). ADR-010
records these as the *empirical complement* to its type invariants — every phase-2.5 defect was
caught by a probe or post-condition, not a type.

**ROADMAP reorder:** Phase 3 (results) → Phase 4 (manual) → **Phase 4.2 — Geometry editor
completion** → Phase 4.5 (reaction modeling). The former `## Phase 2.6 — Geometry-editor backlog`
section was renamed to `Phase 4.2` and moved after Phase 4, filled with ADR-010's staging (stage
1 identity core; stage 2 op-log + 3Dmol-as-dumb-renderer + read-only Monaco xyz with a
paste-xyz-as-fragment replacement path; stage 3 drag/rotation/vdW/undo/ring-torsions as ops), the
two carried 2.5 items kept. **Label collision resolved:** the "Phase 2.6"/"Phase 2.7" tags inside
Phase 2's `[x]` items are *chronicle* (POST /convert; output_search.rs) and were left untouched
with one clarifying note; the two *references* to the section inside Phase 2.5 ("carried to the
Phase 2.6 backlog below" / "carried to Phase 2.6") were redirected to `Phase 4.2` with "below"
dropped. Phase 4.5 gained a Phase-4.2 dependency line and a UseSym open question (verify by a real
run whether ORCA reorders output atoms — a direct ADR-008/010 risk). Phase 3 gained the per-atom
seam item (one explicit mapping fn at the boundary; identity today, only-it-changes after ADR-010)
and an unfixed-stereocenter flag on SMILES import (RDKit picks an enantiomer arbitrarily — a
silent compound substitution, so it does not wait for 4.2).

**index.md:** +ADR-010, +ADR-011, +the proposal page (an index orphan since `67c763c` — the
"0 orphans" lint invariant was already broken). Page count re-derived from the tree: **41**
content pages (was a stale 38; the proposal was one of the missing three, ADR-010/011 the other
two). Structural-update date → 2026-07-30. **Next:** none of this is built — implementation of
Phase 4.2 stage 1 is the first code unit, and it starts only when the roadmap says so.

## [2026-07-30] ingest | Measure ORCA 6.1.0 parse sources (cclib vs .hess/_trj/property/orca_2json)

Probe unit 3.1 — measure, don't decide. **No decision, no ADR; `parser.md`, ROADMAP Phase 3,
ADR-002 untouched** (they state the current plan; if the probe overturns it, they change with a
future ADR-012, not now). The recommendation was delivered separately in the report, not written
into the wiki. Probe script committed: `sidecar/probes/parse_sources.py` (re-runnable on the next
ORCA version). cclib installed into the sidecar venv for the probe (it was deliberately not a
dependency yet — `requirements.txt`: "cclib / ase are added in later phases").

**Environment:** cclib **1.8.1** (measured LATEST on PyPI), Python 3.12.3, ORCA **6.1.0**, on four
real job dirs (SP `09de617c`, Opt+Freq min `d7992449`, Opt+Freq saddle `99e805f5`, GOAT `04aeca22`).

**Headline (unverified-assumption caught):** cclib 1.8.1 **crashes on every ORCA 6.1.0 output** —
`IndexError` in `orcaparser.py:2799 _append_scfvalues_scftargets` (SP/Opt+Freq/saddle),
`AssertionError` (GOAT). Our docs listed "cclib parses ORCA 6.1.0" as plan; it was never true for
the released cclib. Before dying (3–21 ms in), the parser harvests only `atomnos`, `atomcoords[0]`
(**initial** geometry, `len 1`), `natom`, `scfenergies` (`len 1`, first cycle), `metadata` — and
**never reaches** vibfreqs/charges/moenergies/homos/thermochemistry/etenergies.

**Atom-order seam:** cclib `atomnos`+`atomcoords[0]` vs the launching `input.inp` xyz — **exact
match on all four** (count + element order + coord Δ = 0.0). Caveat recorded: this is the *initial*
geometry (cclib crashed before the optimized one); final-order invariance rests on constant natom
in `_trj`/`.property.txt` and `C1` point group.

**Imaginary/near-zero (measured direct from artifacts, since cclib can't):** 6 trans/rot modes are
printed **exactly `0.0`** (`.hess`, `.out`, `.property.txt &FREQ`) — no residue to threshold; the
saddle's imaginary mode keeps its sign, `-33.6608873883281419` in `.hess` / `-33.66 cm**-1
***imaginary mode***` in `.out`, ORCA emitting the marker itself.

**Rule #5:** largest `output.out` = 647 818 B (GOAT); cclib import floor ≈ 83 MiB peak RSS;
full-parse peak **unmeasurable** (no parse completes).

**Structured sources inventoried:** `.hess` (14 sections incl. signed `$vibrational_frequencies`,
`$normal_modes`, `$ir_spectrum`); `_trj.xyz` (5/8/18 frames); `.property.txt` (energies, `$Geometry`,
Mulliken/Loewdin/Mayer `&AtomicCharges`, `$SCF_Dipole_Moment`, `$Hessian`, `$THERMOCHEMISTRY_Energies`
with zpe/H/S/G; GOAT has only Geometry+Single_Point_Data); `/opt/orca/orca_2json input.gbw`
(needs `LD_LIBRARY_PATH=/opt/orca`, rc 0 → JSON with `Molecule.Atoms` final Angs coords +
`MolecularOrbitals` 68 MOs with `OrbitalEnergy`/`Occupancy` in Eh + Charge/Mult/PointGroup; **no**
freq/thermo). Full per-quantity source table lives in `wiki/orca/parse-sources.md`.

**Gaps left (named, not faked):** no scan (`%geom Scan`) job → scan parsing unmeasured; no TD-DFT
job → etenergies/etoscs unmeasured; cclib full-parse RSS unmeasurable; cclib *dev/git* build (not
on PyPI) not tested. **Wiki:** +`orca/parse-sources.md`; `output-files.md` `.property.txt` row and
Imaginary-frequencies section replaced with measured facts; index count → 42.

## [2026-07-30] decision | Own structured-artifact parsing (ADR-012) after verifying artifact atom order

Two-part unit behind a gate. **No parsers written, no `results` schema, no UI, no scan run.**
Only the probe was extended (`sidecar/probes/parse_sources.py`) and the decision + docs synced.

**Part A — the gate (fixes a defect from unit 3.1).** Unit 3.1 checked atom order for cclib
only; cclib turned out dead and the seam moved to structured artifacts, but the order check did
not follow — `parse-sources.md` asserted "the structured artifacts carry the same atom count in
the same order" with evidence only for *count*. Part A extended the probe to compare the ELEMENT
sequence of **every** artifact against the launching `input.inp`, per frame and per block:
`.hess $atoms`, `orca_2json Atoms`, every `_trj.xyz`/`.xyz` frame (5/8/18), every `.property.txt
$Geometry` block (1/5/8/18). **GATE VERDICT: PASS** on all four jobs — no reorder anywhere.
Measured units (not assumed): `.hess $atoms` = Bohr (coord ratio 1.8886 ethane / 1.6579 saddle
vs input; `&Units "Bohr"` literal in `.property.txt`; `orca_2json` = Angs, final geometry so a
coord diff there is final-vs-initial, not reorder); `normal_modes` = 3N×3N, `vibfreqs` = 3N.

**Seam, named precisely** (and an earlier error corrected): the atomic-charge arrays are **not**
bare positional — Mulliken/Loewdin/Mayer each carry a co-located `&ATNO` array whose order was
verified == input (unit 3.1 had wrongly called them bare, because a `sed` started at
`&AtomicCharges` and skipped the `&ATNO` two lines above). The **only** bare positional
atom-ordered array is `$SCF_Nuc_Gradient &grad` (3N flattened), whose order is assumed from the
co-located `$Geometry` block — assumption named, not hidden. `&FREQ` is mode-indexed, not
atom-indexed. GOAT quirk measured: xTB tags elements `C(1)` in `$Geometry` (fragment suffix) and
its gbw is not readable by `orca_2json` (no JSON) — neither is a reorder.

**Part B — ADR-012 (ACCEPTED).** Authoritative tier = our own parsers over structured artifacts,
one quantity one home: `.property.txt` (energies/geometry/charges/dipole/thermo), `.hess`
(signed freqs/normal modes/IR), `_trj.xyz`/`.xyz` (trajectory/final geom), `orca_2json` over
`.gbw` (MO energies+occupations, HOMO/LUMO). `output.out` **not** authoritative (rule #5 holds by
construction). **cclib not adopted, not added to requirements.txt.** Durable reason, not the
symptom: (a) crash reproduces on all four outputs at a generic SCF-table site (env-independent);
(b) measured — cclib's `orcaparser.py` knows only version markers `Orca 2.6`/`ORCA4.0`/`ORCA 5.0`,
**no ORCA 6.x**, so 6.x is outside its handled matrix even absent the crash; (c) 1.8.1 is latest
on PyPI and predates 6.1. **Reopening needs BOTH:** our probe runs clean on our outputs AND ORCA
6.x appears in cclib's supported matrix — a cclib release alone reopens nothing. Second
consequence: the tier moves to **Rust** (`.property.txt`/`.hess` are library-free text parsing;
`orca_2json` is a binary spawn → ADR-009), dropping the results HTTP round-trip and one runtime
boundary. ADR-002 **narrowed** (sidecar no longer owns result parsing; cclib declined) — not
edited, per the ADR-008/010 precedent. ADR-010 amendment (i) **survives, narrowed**: positional
arrays still cross the sidecar boundary via RDKit/ASE, so Rust still builds the `IndexMap` there;
only the positional producer behind the boundary changed (cclib → artifacts). Rule-#10 caveat
kept explicit: `.property.txt`/`.hess`/`orca_2json` format stability across ORCA versions is
**unmeasured**; mitigation = probe re-run on every upgrade + real-artifact fixtures with the
parsers.

**Docs synced:** +`adr-012`; `parse-sources.md` unverified "same order" claim replaced with the
Part-A per-artifact table + array-labelling table + ADR-012 pointer; `output-files.md`
`output.out` row + per-quantity artifact map rewritten; `parser.md` Tier 2 retargeted cclib →
"Rust over structured artifacts (ADR-012), not started" and the "cclib gaps → Python parsers"
note removed; ROADMAP Phase 3 first item → four Rust artifact readers, seam item updated with the
measured order sources; index count → 43. **Gaps still open:** scan (`%geom Scan`) and TD-DFT —
no such job exists, need a real run (next unit). cclib left installed in the venv from the probe;
**not** pinned in requirements.

## [2026-07-30] ingest | Scan artifacts + units of every numeric array (unit 3.3, measured)

Measurement unit before writing parsers. **No parsers, no `results` schema, no UI.** Extended
`sidecar/probes/parse_sources.py` with `probe_artifact_order` (already there) re-run on a scan +
new `probe_units`.

**Part A — real relaxed scan (closes the Phase-4.5 "no scan" gap).** Ran one 6-point relaxed C–C
scan of ethane from the **terminal** (not the app — scan generation is Phase 4.5; no SQLite row),
`! r2SCAN-3c Opt TightSCF` + `%geom Scan B 0 1 = 1.4,2.4,6 end end`, 0-based indices range-checked
first. Gotcha measured: without `! Opt` ORCA runs a single point and silently ignores `Scan` (1
energy, 1 geometry) — the relaxed scan needs Opt. Dir:
`~/.local/share/orcastudio/probe-scans/scan-ethane-cc`. Scan-only files (verbatim):
`.relaxscanact.dat`/`.relaxscanscf.dat` (**structured**, 2 cols coordinate+energy, 6 rows; act =
composite, scf = bare SCF), `.allxyz` (6 geoms, comment carries `Step N E <energy>`, not the
coordinate), `.NNN.xyz`/`.NNN.gbw` (per-point). **Per-point energy+coordinate live in the
structured `.dat` files**; `.out RELAXED SURFACE SCAN RESULTS` is the text mirror;
`.property.txt` has 26 `$Geometry` (per-opt-cycle, NOT 6 points) and `_trj.xyz` 26 frames — not
the scan source. Order gate on the scan dir: **PASS** (all 26 trj frames + 26 `$Geometry` blocks
keep input order — best available silent-reorder test).

**Part B — units of every array, each by one method (1) literal / (2) cross-check ratio / (3)
determiner; UNDETERMINED where none applies.** Literals: `$Geometry`/`.hess $atoms` = **Bohr**
(`&Units "Bohr"`), `&FREQ` = **cm⁻¹** (`&Units "cm^-1"`), dipole = **a.u.** (`&Units "a.u."`),
`orca_2json` = **Å** (`CoordinateUnits`) / MO **Eh** (`EnergyUnit`). Cross-checks (ratio):
`.hess $atoms` Bohr (1.8886 ethane / 1.6579 saddle vs input), `.xyz`/`_trj` = Å (1.0 vs json),
`$vibrational_frequencies` cm⁻¹ (1.0 vs `.out`), `$normal_modes` **dimensionless unit-norm**
(Σ²=1.0; `orca_pltvib` scales ×2.0 for display, measured via path (3)), IR intensity **km/mol**
(`.hess $ir_spectrum` col2 = `.out` Int, header names units), gradient **Eh/Bohr** (`.out`
literal), scan coordinate Å + energy Eh (1.0). Thermochemistry all **Eh**; **`entropyS` = T·S in
Eh, NOT S** (`entropyS == enthalpyH − freeEnergyG`, exact). UNDETERMINED: `$hessian` absolute
unit (`.hess`==`.property.txt`, ratio 1.0, but no literal → determiner: reconstruct freqs from
Hessian+masses), `$dipole_derivatives` (determiner: cross-check vs IR intensities). The
load-bearing finding: the authoritative tier spans **two length systems** (Bohr vs Å).

Gaps: TD-DFT (`etenergies`/`etoscs`) still unmeasured — no such job (Phase 6).

## [2026-07-30] decision | Units norm — CLAUDE.md rule #11 + ADR-012 amendment (unit 3.3)

**No decision text rewritten.** New **domain rule #11** (units are a norm, like #9/#10, not a
choice): no physical quantity crosses a parser boundary as a bare number; each reader converts to
canonical units **once at the boundary** — length **Å**, energy **Eh**, frequency **cm⁻¹**, IR
**km/mol** (all measured); units come only from a literal / a cross-check ratio / a determiner,
never convention; **post-condition (rule #9 in our terms):** a reader whose artifact carries
geometry we already know recomputes it after conversion and asserts max Δ < 1e-4 Å — a missed
Bohr→Å conversion then fails loudly (≈1.889×), not silently animating wrong physics (the Phase-3
IR-peak click). The `$SCF_Nuc_Gradient &grad` order-from-`$Geometry` seam is named in the rule so
it survives to parser-writing time. Reason recorded: two unit systems in the authoritative tier
are measured, and a stray 1.889 on a mode displacement does not crash.

**ADR-012 gets an AMENDMENT** (ADR-008 precedent — decision text untouched): records the measured
two-unit-system finding + the full units table pointer, names rule #11 as the governing norm,
and marks the scan gap **closed**. ADR-002/009/010 and the proposal untouched; cclib still not in
requirements. **Wiki:** `parse-sources.md` +scan inventory +units table, scan gap closed;
`output-files.md` +scan rows +units note; ROADMAP Phase 4.5 scan parser sourced to `.dat`, Phase
3 "+no-MO-is-normal" item; CLAUDE.md +rule #11; ADR-012 +amendment. Page count unchanged (43).

## [2026-07-30] session | First artifact reader in Rust: .property.txt (canonical units by type, rule #11)

Unit 3.4 — first code that reads numbers from an artifact (ADR-012 authoritative tier). Written as
the **template** for the other three readers. `src-tauri/src/parse/`: `mod.rs` (+`ParseError`,
`#[from]` into `AppError`), `units.rs` (`Angstrom`), `elements.rs` (symbol↔Z, `C(1)`-suffix strip),
`property.rs` + `property/tests.rs`. Not wired to the pipeline / `results` schema (later units);
`mod parse` is `#[allow(dead_code)]`.

**Two-layer parser** (the template's spine): a generic `$Block…$End` / `&prop [&Type,&Dim,&Units]`
tokenizer that knows the grammar not the blocks and keeps every block (unknown ones surface via
`unknown_block_names()`, rule #10), then typed accessors that convert to canonical units at the
boundary.

**Units held by TYPE, not a comment (the unit's whole point, rule #11).** `Angstrom` has a private
field in its own module; `property` is a *sibling*, so `Angstrom(x)` does not compile there — the
reader must pick `from_bohr` (convert) or `from_angstrom` (already Å). The measured trap: `.property.txt`
`$Geometry` is Bohr, the app is Å; a forgotten ×0.529 renders a plausible 1.889×-too-large molecule.
Test `missed_bohr_conversion_fails_loudly` builds a geometry from Bohr magnitudes typed as Å and
asserts the geometry post-condition rejects it (~1.03 Å Δ). Without that test rule #11 is a slogan.

**Post-conditions are errors, not warnings:** geometry (first `$Geometry` after conversion vs a
known-Å reference, max Δ < 1e-4), `&ATNO` order == geometry order, lengths (charges=N, grad=3N,
FREQ=3N) checked against N, never trusted from `&Dim`.

**Three measured traps encoded in structure, not prose:** (a) `entropyS` field named
`t_times_s_eh` — measured `entropyS == enthalpyH − freeEnergyG`, i.e. T·S in Eh, unreadable as S;
(b) scan `$Geometry` blocks are opt cycles not scan points (test locks 26 for the 6-point scan);
(c) `$SCF_Nuc_Gradient &grad` bare positional, bound to its `$Geometry` via `geometry_index` in the
type. Also measured: Mayer's charge field is `&QA` (not `&AtomicCharges`); absent blocks → `None`
(GOAT parses: only Geometry + Single_Point_Data), a reader crashing on GOAT is a bug.

**Tests:** 13, against real SP / Opt+Freq(ethane) / GOAT / relaxed-scan `.property.txt` fixtures in
`src-tauri/tests/fixtures/` (author's own runs — rule #7 is about not redistributing ORCA, not our
outputs; stated in the commit). `elEnergy == -79.7918513760713` (cross-check with `.out`) passes.
Full suite: 86 passed, 0 failed, clean build (no warnings).

**Task 0 (did not wait):** corrected `$normal_modes` label in `parse-sources.md` + the probe script
— Σ²=1.0 proves normalization, not Cartesian-ness; Cartesian-vs-mass-weighted is now UNDETERMINED
with the determiner named (a gate for the `.hess` reader, not run here).

**Wiki:** +`modules/artifact-readers.md` (the template, present tense); `parser.md` Tier 2
"under way" (first reader built, 3 not started — precise); ROADMAP Phase 3 `.property.txt` → `[x]`
+ an entropyS-labelling card item; index count → 44. No SQLite `results` schema, no UI, no three
other readers — all out of scope per the unit. ADR-002/009/010/012 untouched; cclib not in requirements.

## [2026-07-30] session | Thin vertical slice: parsed results end to end (typestate → results table → completion hook → card)

Unit 3.5 — the first slice that puts per-atom data in permanent storage, so the invariant is
stricter than "store the numbers": no per-atom array is stored without the element sequence it was
already verified against.

**Task 1 — typestate (gate).** The reader's post-conditions used to sit *beside* the path (three
methods a caller could skip). Now `PropertyFile::parse` returns an **unverified** handle with **no
value accessors** — only `unknown_block_names()` and `verify(reference)`. `verify` runs all three
post-conditions and returns `Verified`, the only type with `charges()`/`geometries()`/… So
`parse(text).charges()` does not compile. The caller supplies the reference (each job's own
`input.inp`); the reader never reads `input.inp` (no hidden cross-module dep). Removed the blanket
`#[allow(dead_code)]` on `mod parse`; cleaned the real unused that surfaced (`RawProp.ptype`/`dim` +
their regexes, an unused `first_geometry` delegate, `geometry_block_count`) rather than silencing.

**Task 2 — `results` table (schema v5).** Narrow typed columns (final energy, dipole magnitude,
thermochemistry, `parser_version`, `parsed_at`) for the card + sorting; one `data_json` column with
the full structure. Per-atom arrays live in JSON **with their element order** (charges next to
`elements`/`atomic_numbers`, gradient next to its `$Geometry` `order_elements`) — **no
position-keyed atom table** (ADR-010, longest horizon). Idempotent upsert on `job_id`. Units in
column names (rule #11); `t_times_s_eh` is **T·S in Eh, not entropy S**.

**Task 3 — completion hook.** `local_backend::parse_results_after_completion` (live finish +
reconcile): read `.property.txt`, verify against the reference from `input_content`, store, read
back, advance to `parsed`. Two failure modes kept distinct: calc failed → `failed`; calc fine but
parse failed → stays `completed`, reason recorded (OUR problem); no `.property.txt` → "nothing to
parse", not a failure. **Storage post-condition (rule #9):** read the row back and assert per-atom
counts + element order survived serialization. New `JobStatus::Parsed` (post-`completed`);
`read_job_results` command.

**Task 4 — minimal card.** `ResultsCard.tsx`: final energy, dipole, three charge schemes
(atom→value, keyed by stored element order), thermochemistry with **T·S labelled as T·S**, not
entropy. Absent sections hidden — GOAT/SP render without crashing (measured: GOAT has no
charges/dipole/thermo). Unknown blocks shown as a small line, not an error.

**Proven on real data:** `real_optfreq_job_parses_stores_and_reads_back` (`#[ignore]`, ran green) —
a real ethane Opt+Freq job dir → parsed → stored → read back: E = −79.79185137607134 Eh, 8 Mulliken
charges (C ≈ −0.412, H ≈ +0.137), `t_times_s == H − G`. Tests: Rust 93 + 1 ignored real-data;
frontend 301; `tsc` clean; no warnings. Migration v4→v5 covered by
`migrate_v4_to_v5_adds_results_and_preserves_jobs`.

Out of scope (untouched): the other three readers, `results`-schema for spectra/orbitals, full
results screen, trajectory/orbitals/spectra/export, the normal-mode determiner. ADR-002/009/010/012
and the proposal untouched; cclib still not in requirements. **Wiki:** `artifact-readers.md`
+typestate section; `tauri-core.md` +v5/`results`/`parsed`/hook/per-atom rule; ROADMAP Phase 3
`.property.txt` wired + card `[~]`; index unchanged (44 — no new page).

## [2026-07-30] ingest | Gate: $normal_modes are Cartesian, not mass-weighted (determiner, per-atom numbers)

Unit-3.6 gate before writing the `.hess` reader. Σ² = 1.0 (unit 3.3) only proved *normalization* —
mass-weighted eigenvectors are unit-norm too — so it needed a determiner run (rule #10). Ran
`orca_pltvib m.hess 9` (mode 9 = 997 cm⁻¹, non-degenerate, C and H both move) on ethane
`d7992449`; took the first block's displacement columns (Å) ÷ the raw `$normal_modes` column, per
atom. **Per-atom ratio = 2.0000 for all 8 atoms** (both C at 12.011 u and H at 1.008 u) → **H/C =
1.0000, not √(12/1) ≈ 3.4519**. A single scalar (pltvib's animation amplitude) independent of mass
⇒ **Cartesian**, not mass-weighted. Consequence: the `.hess` reader consumes normal modes as-is
(Cartesian displacements), **no ÷√m**, no atomic-mass table. `parse-sources.md` updated: the
`$normal_modes` row is no longer UNDETERMINED — method (3) with the numbers. (`$hessian` /
`$dipole_derivatives` stay UNDETERMINED — not read.)

Also measured while grounding the reader (recorded in `parse-sources.md` / the reader): `.hess
$atoms` is the Freq geometry **rigidly reframed** (COM/Eckart) — a uniform 1.041 Å shift vs the
input frame on the asymmetric saddle, 0 on symmetric ethane; interatomic distances match to 4e-8 Å.
So the `.hess` geometry post-condition is **distance-based**, not coordinate-based (a missed Bohr→Å
still fails at 6.6 Å; the reframe passes).

## [2026-07-30] session | Second artifact reader: .hess (frequencies, IR, normal modes) + storage + card

Unit 3.6 Part B/C, after the gate above. `src-tauri/src/parse/hess.rs` — the **second** reader,
holding the `.property.txt` template: two layers, typestate (`parse → verify(reference) →
Verified`, accessors only on `Verified`), unknown sections surfaced (rule #10), post-conditions as
errors, units by type. Two deliberate deviations, both recorded: the **grammar** differs
(`$section`-with-its-own-shape vs `$Block`/`&prop`) — the tokenizer is written to it; the
**geometry post-condition is distance-based** (the reframe, above), and the caller supplies the
**optimized** geometry (property final `$Geometry`), not `input.inp`.

Measured facts in structure, not comments: signed frequencies with an explicit `imaginary_count`
(0 = minimum, 1 = TS, >1 = neither); exact-`== 0.0` trans/rot filter (no threshold), 5 (linear) vs
6 (non-linear) distinguished not failed; Cartesian modes (gate) with no ÷√m; `$ir_spectrum`
intensity = column 2 (km/mol), tested against a column swap; `$hessian`/`$dipole_derivatives`
recognized-but-not-read (UNDETERMINED units, never shown). Post-conditions: element order,
distance-invariant geometry, lengths (freqs 3N, modes 3N×3N, IR 3N), zero-count ∈ {5,6}. 9 tests
against real ethane-minimum + saddle fixtures (saddle carries the −33.66 cm⁻¹ imaginary), plus a
synthetic linear diatomic proving 5 zeros is legal and a `missed_bohr_conversion_fails_loudly`.

**Storage + card (Part C):** `results` schema **v6** (guarded ALTER adds `imaginary_count` — the
narrow sort/warning column; the full frequencies/IR/normal-mode matrix goes in `data_json` WITH the
`$atoms` element order, matrix not per-atom rows). `parser_version` → 2. `results.rs` reads the
optional `.hess` in the completion hook (absent for SP/GOAT = normal). Card gains a vibrational
table with IR intensities and a **prominent minimum/TS/neither verdict** (teaching, not alarming).
Two pinned items closed: (1) the thermo card now also shows a **derived S in J/(mol·K)**, labelled
"derived: T·S / T", with a named `EH_TO_J_PER_MOL` constant and `temperature_k` added to
`Thermochemistry` (K by the `.out` literal); (2) the `NoArtifact`-conflation forward-note is
recorded in `tauri-core.md` for Phase 5. Real e2e (`#[ignore]`, green): real ethane Opt+Freq →
24 frequencies, 0 imaginary, stored + read back. Tests: Rust 100 + 3 ignored, frontend 301, tsc
clean, no warnings. Out of scope (untouched): mode animation, IR Lorentzian plot, the other two
readers, ADR-002/009/010/012, proposal; cclib still not in requirements. Wiki: `parse-sources.md`
(gate), `artifact-readers.md` (second reader), `tauri-core.md` (v6 + NoArtifact note), ROADMAP
Phase 3.

## [2026-07-30] ingest | Gate: orca_2json scaling (rule #5) — measured, streamable

Unit-3.7 gate before writing the `orca_2json` reader. The JSON is dominated by
`MolecularOrbitals.MOs[].MOCoefficients` (an n×n matrix we don't need), measured on two gbw:
ethane 198 KB json / nMO 68 / MOCoeff **52.5%**; saddle (def2-TZVP) **3.5 MB** / nMO 314 / MOCoeff
**62%**. Extrapolation (arithmetic, `nBF ≈ 31·heavy + 5·H` fit to the saddle, json ∝ nBF²): a
50-atom def2-TZVP → ~38 MB, a 60-atom → ~52 MB, ~60% of it coefficients we discard — **tens of MB**,
a real rule-#5 hazard (not hundreds). Flags from `orca_2json -h` (not memory): only format
(`-json/-bson/-ubjson/-msgpack`) + `-property*`; **no flag omits coefficients**. **Resolution
(gate PASS):** `serde_json::from_reader` (already a dep) into a struct that omits `MOCoefficients`
— serde skips it as `IgnoredAny`, never allocated; peak memory is the two small per-MO arrays.
Reading the file whole into a `Value` would be the `.out` mistake in JSON clothing.

Also measured for the `_trj.xyz` reader: the comment line is **identical across job types**
(`Coordinates from ORCA-job input E <energy>` on Opt/GOAT/scan `_trj` and on `.xyz`) → the frame
energy is parseable to `Option<f64>`. `.allxyz` differs (`… Relaxed Surface Scan Step N …`,
`>`-separated) — out of scope, not fed to the reader.

## [2026-07-30] session | Last two artifact readers: _trj.xyz + orca_2json — the ADR-012 tier is complete

Unit 3.7 Part B/C/Г. Two readers on the template; the ADR-012 authoritative parse tier is now
**complete** (all four), and Phase 3 past here is pure visualization.

`parse/xyz.rs` (`_trj.xyz`/`.xyz`) — multi-frame xmol, typestate, Å via `from_angstrom` (the
identity case, still guarded by a `missed_conversion` test). Comment energy measured, uniform.
Post-conditions: natom constant, element order per **frame** == reference, ≥ 1 frame, first-frame
geometry. Frames are opt cycles, never scan points (26 for a 6-point scan). 8 tests.

`orca_2json` split in two (kept apart): **spawn** `crate::orca_json` (ADR-009 — Rust owns it;
ORCA path from **settings**, rule #7, not hard-coded; `LD_LIBRARY_PATH` + `.gbw` extension; lazy +
cached in the job dir, rule #3; xTB gbw → `Ok(None)`), and the **reader** `parse/mo.rs`
(streamed `from_reader` omitting `MOCoefficients` → rule #5; distance-invariant geometry check;
reference = final geometry; HOMO/LUMO from the occupancy boundary). 5 tests, incl. a 198 KB
gbw-json fixture that DOES contain coefficients (exercises the skip) + a stored-json assertion that
coefficients never persist.

Storage v7 (`homo_lumo_gap_eh` narrow column, guarded ALTER as v6); `parser_version` → 3.
`data_json` gains the trajectory (frames = opt cycles, element order once + per-frame Å coords +
comment energy) and MO energies/occupancies — **never** coefficients (rule #5; a test proves it).
Trajectory-size decision (reported): longest measured 26×8; a 30-cycle×50-atom opt ≈ tens of KB —
kept inline, frames→file-path switch noted if ever needed. Card: HOMO/LUMO gap in **Eh + eV**
(named `EH_TO_EV`), a trajectory frame-count row; absent sections hidden.

Real full-pipeline e2e (`#[ignore]`, green, on a temp COPY so orca_2json's json never pollutes user
data): all four readers → E=−79.79185 Eh, 24 freqs (0 imaginary), 5 trajectory frames, HOMO/LUMO
gap 0.4173 Eh, and `MOCoefficients` absent from the stored JSON. Tests: Rust 114 + 3 ignored,
frontend 301, tsc clean, no warnings. ADR-002/009/010/012 + proposal untouched; cclib still not in
requirements. Out of scope: trajectory playback, isosurfaces, mode animation, IR plot, export,
the Kabsch-rotation determiner (next unit's gate). Wiki: `parse-sources.md` (gate + comment line),
`artifact-readers.md` (readers 3–4 + the spawn/parse boundary), `tauri-core.md` (v7 + no-coeff
rule), ROADMAP Phase 3 (parse tier complete).

## [2026-07-30] session | Unit 3.8: trajectory playback + broadened IR spectrum (frame state in the app, not the viewer)

First **purely visualization** unit of Phase 3. Both parts stand on already-parsed `data_json`
(unit 3.7); no artifact is re-read. No Rust changes — frontend only.

**The load-bearing decision — frame ownership.** This is the first time the viewer receives a
*sequence* of geometries. 3Dmol has its own frame apparatus (`addModelsAsFrames`/`setFrame`/
`animate`; `setCoordinates` loads a whole `T×N×3` to drive it) — **not used.** The current frame
number is **application state** in `TrajectoryPlayer` (React), the play timer is a `useEffect`
`setInterval` there, and the viewer is a **dumb renderer** fed **one frame's** xyz (ADR-011). Reason
beyond tidiness: Phase 4.2 swaps the renderer once the ADR-011 spike passes — nothing may migrate
into 3Dmol's state; and a timer buried in the viewer couldn't sync with the cycle label, the energy
readout, or the highlighted point on the E(cycle) chart. Same call as `selection` (view-local UI
state, not the store).

**Part A — playback (`src/trajectory/`).** `frame.ts` (pure, tested): `frameToXyz` (throws on a
count mismatch — no silent render), `frameLabel` (**honest: optimization CYCLES, never "scan step"**
— measured, a 6-point scan has 26 frames), `frameEnergyText`/`frameDeltaKcal` (comment energy Eh +
ΔE-from-cycle-1 kcal/mol), `energySeries`, `elementsAgree`. `TrajectoryPlayer.tsx`: transport +
slider + speed (0.5–4× = 2–20 fps, app-layer timer), the **E(cycle) chart** (click a point → jump,
current cycle a vertical marker — the learning core, watch it descend). **Identity check at the UI
boundary:** `elementsAgree(trajectory.elements, final_geometry.elements)` before drawing — mismatch →
error, not a wrong animation (the readers' element-order discipline one layer out). Empty states: 1
frame → static, no controls; SP (no trajectory) → section hidden. `MoleculeViewer` gained one opt-in
prop `preserveCameraOnUpdate`: a same-atom-count `xyzData` change redraws **without** `zoomTo` (camera
holds through playback); count change still zooms; default false → Molecules/preview unchanged. We
rebuild the single-frame model per tick (the only in-place path is 3Dmol's forbidden frame apparatus);
`frameToXyz` is ~3.5 µs/8 atoms, ~13 µs/50 (measured, Node) — playback is **timer-bound**, not
rebuild-bound. Real in-webview `addModel` fps not headlessly measured (standing GUI-drive limit).

**Part B — IR spectrum (`src/spectrum/`).** `ir.ts` (pure, tested): `classifyModes` splits the stick
list **by measured fact, not a threshold** — exact-zero (`=== 0`) trans/rot excluded, negative
(imaginary) excluded by **sign** but returned separately, `cm > 0` broadened; `lorentzian` is
**area-normalized** (∫ = 1, so ∫ peak = km/mol intensity — a test locks it) with `g = FWHM/2` written
out; `autoGrid` names range+step explicitly. `IrSpectrumPanel.tsx`: verdict banner (min/TS/neither),
**imaginary modes listed separately as a transition-state diagnosis** (not dropped, not broadened),
Lorentzian curve (recharts `ComposedChart`) with a **FWHM slider** + printed grid (plot choices, not
molecule properties), the frequency table, and **peak ↔ row** two-way selection. **No mode animation**
— unit 3.9, behind the Kabsch gate; no static-preview stopgap.

**orca_mapspc cross-check (rules #9/#10).** Probe `sidecar/probes/ir_mapspc_xcheck.py` (one-off, not
app code). Flags from its own `-h` (`IR -l0 -w<FWHM> -x0<min> -x1<max> -n<npts>`, attached values;
its `-h` prints "Peak FWHM" → `-w` IS the FWHM). **Max shape deviation = 14.0%**, cause **measured**:
orca broadens col1 (a.u.) peak-height-normalized and writes `1000 − absorption`, we broadcast col2
(km/mol = 5053.6·col1) area-normalized; the real residual is **wing truncation** (orca is exactly 0
at 3172/3401 pts, cutting tails beyond ~1.9·FWHM) while we keep full wings so area = intensity. Peak
cores + FWHM agree. Reported, **not fudged** — our curve isn't bent to a tool whose normalization and
windowing differ by design. Recorded in `parse-sources.md`.

**Shared:** extracted `useContainerWidth` to `src/charts/` (the WebKitGTK 0×0 workaround, one owner);
`ConvergenceDashboard` now imports it (dedup).

**Verified.** `tsc` clean; **vitest 322 passed** (19 files; +21: `ir.test.ts` — sign/zero split, area
= intensity, FWHM half-max, superposition, grid; `frame.test.ts` — honest label, ΔE, identity,
count-mismatch throw); `vite build` clean (pre-existing bundle-size warning only). In-GUI legs (the
player animating, peak/row click, the FWHM slider in the real webview) need the Tauri window — not
headless-drivable, same limitation as every prior unit; the risky logic is all pure-tested and the
recharts/3Dmol paths reuse proven patterns. ADR-002/009/010/011/012 + the proposal untouched.

Next (unit 3.9): normal-mode animation — the Kabsch-alignment determiner gate, then click-a-peak →
watch the atoms move.

## [2026-07-30] session | Unit 3.9: three defects from the first real molecule (dexketoprofen, 33 atoms)

The author's first real run — dexketoprofen, `! r2SCAN-3c CPCM(ethanol) Opt Freq TightSCF`, 21m29s —
exposed three defects, two serious. Kabsch + mode animation deliberately deferred: half of Phase 3
was physically unreachable from the UI, which mattered more. **The unifying insight: both serious
defects are Phase-1 decisions Phase 3 outgrew** — neither was wrong then, both invisible on ethane,
both appeared on the first real molecule. Fixed at the cause, not the symptom. Full writeup:
`wiki/debugging/007`.

**Defect 1 — half the results screen unreachable.** `.screen.detail` had `overflow: hidden` with a
`flex: 1` log console filling the viewport (correct in Phase 1, when the console was the only content).
Phase 3 added the results card above it, so the IR spectrum / imaginary list / verdict were rendered
and clipped. Fix: the SAME recipe already applied to `.screen.new-job` — scroll as a normal column
(`overflow-y: auto`), console + Browse viewer get a fixed `60vh` height, not `flex: 1`. **One layout,
not a status-conditional split.** Audited every other `overflow: hidden`: all on fixed-size boxes
(Monaco wrappers, 3D canvas, toggle, progress bar) — `.screen.detail` was the only diseased one.

**Defect 2 — blank header energy.** Measured: the last `FINAL SINGLE POINT ENERGY` sits **164 186 B**
from EOF (99 modes + IR + thermo between it and the end), past `RESULT_TAIL_BYTES` (64 KB) — so the
tail regex left `jobs.energy` NULL while `results.final_energy_eh` held −843.690396. NOT a constant
bump (moving target; contradicts ADR-012). Fix, three parts: (a) after parse, `jobs.energy` is
overwritten from the authoritative `results` tier (`stored_final_energy` → `set_job_energy_conn`); the
regex stays a **live estimate during a run**. (b) Migration **v7→v8** backfills old jobs from `results`
(one-time data correction, guarded on `jobs.energy` existing). (c) **The post-condition that would have
caught it (rule #9):** cycle energies now have two independent sources — the streaming `.out` parser
(`convergence.rs`) and `_trj.xyz` frame comments (unit 3.7) — and `cycle_energy_cross_check` compares
counts (`n_traj ∈ {n_opt, n_opt+1}`, the trailing converged frame — measured) and values (< 1e-6 Eh)
after every run. Divergence → recorded `ParseFailed`, not silence. **Gated to non-GOAT** (its
trajectory is conformers, not one opt's cycles — 17 inner-opt blocks vs 18 frames, measured, would
false-fail). Agreement is bit-for-bit on dexketoprofen / ethane / saddle.

**Defect 3 — dipole in a.u. only.** The card gives the gap in Eh+eV and entropy in J/(mol·K), but left
the dipole in a.u. Now shows debye too (1.7621 a.u. → 4.48 D), `AU_TO_DEBYE` a named CODATA constant
beside `EH_TO_EV`/`EH_TO_J_PER_MOL`. **Display only** — the DB still stores a.u. (the measured artifact
unit); debye is derived, like S from T·S.

**Real-data fixtures + tests (dexketoprofen, the largest real system we have — catches what ethane
can't):** `src-tauri/tests/fixtures/dexketoprofen_output_tail.out` (200 KB real tail) locks the
defect-2 window gap (`final_energy_sits_past_the_estimate_window…`: 64 KB tail → None, full read →
−843.690395750533). `src/spectrum/__fixtures__/dexketoprofen-freqs.json` (real 99 freqs+IR) locks the
low-mode regression: 6 exact zeros excluded, the four genuine low modes **21.36 / 31.94 / 36.84 / 49.15
cm⁻¹ survive** (a naive "<50 cm⁻¹" threshold would eat them, invisibly on ethane whose lowest is 318).
`cycle_energy_cross_check` unit-tested with the real 16 opt / 17 traj energies (match, +1 frame, planted
divergence caught, count mismatch caught, empty skipped); `input_is_goat` tested. A `#[ignore]`
real-dir test parses the whole 33-atom job and asserts the cross-check PASSES and the authoritative
energy is recovered.

**Verified.** `cargo test` **121 + 4 ignored** (added: output-tail defect-2 regression, 5 cross-check/
GOAT unit tests, migration v7→v8 backfill; the 2 ignored real-data tests — dexketoprofen + ethane —
pass, confirming the cross-check doesn't false-fail on real runs). `tsc` clean, `vitest` **324** (+2
real-data low-mode tests), `vite build` clean, no Rust warnings. In-GUI legs (scrolling the screen,
the debye/energy values in the real webview) need the Tauri window — standing limitation; the logic is
unit-tested and the CSS mirrors the proven `.screen.new-job` fix. ADR-002/009/010/011/012 + proposal
untouched.

Next: Kabsch-alignment gate → normal-mode animation (click an IR peak → watch the atoms move).

## [2026-07-31] session | Unit 3.10: honest IR presentation after the first chemist review

The IR panel worked but *looked* right in ways that weren't true. First real look by the chemist
found three defects and two places where a display choice was implicit instead of labelled. **The
physics was not touched** — `ir.ts` is cross-checked against `orca_mapspc` and the numbers hold
(tallest peak 40.3 = 2·632.8/(π·10)). The whole unit is about the seam rule #11 draws on screen:
every element of the plot is either a measured quantity or an explicitly labelled construction
choice — never a third thing that reads as physics but isn't.

**Bug 1 — markers → sticks.** On-curve Scatter markers replaced by **sticks**: a vertical line per
mode at its wavenumber, height = IR intensity in **km/mol** (the physically honest object — the
spectrum IS a set of lines). The broadened curve (km/mol·cm⁻¹, area-normalized density) sits on top.
**Two labelled Y axes** (left curve km/mol·cm⁻¹, right sticks km/mol) — chosen over one axis because
they are genuinely different quantities and a single axis would need an arbitrary FWHM/lineshape
conversion factor (a made-up parameter, rule #11). Sticks are drawn SVG (`IrSticks`, recharts v3
`useXAxisScale` + `usePlotArea`), positioned from the plot area + the explicit km/mol max so they do
not depend on the data-less right axis registering an internal scale.

**Bug 2 — the two-series tooltip.** recharts merged the curve Line and the markers Scatter and took
the label from one x and a value from another (`115 cm⁻¹` header next to `9.350` = the O–H peak
height at 3714). Fixed **structurally**: sticks are no longer a chart series, so the Tooltip has only
the Line to read — nothing to merge. A custom `content` derives everything from the one hovered
wavenumber via `irTooltipModel(label, curveValueThere, modes)`: the curve value at that x plus the
**nearest mode** labelled *as nearest* with its Δ. `irPresentation.ts` is a new pure, node-tested
module (physics stays in `ir.ts`); `irPresentation.test.ts` reproduces the 115-vs-3714 scenario and
locks the one-x property.

**Bug 3 — Y axis units.** Both axes now carry units (left km/mol·cm⁻¹, right km/mol) — rule #11 on
the display.

**Choice — inverted view.** A peaks-up / peaks-down toggle. Inversion **reverses both Y axes** (data
unchanged — the honest inversion), x stays increasing, and it is labelled a *conventional depiction,
explicitly NOT transmittance*: %T needs the Beer–Lambert law (path length, concentration) a
calculation does not contain, so no `%T` axis and no invented parameters.

**Choice — frequency scaling.** Harmonic frequencies run high (measured: C–H 3025–3193 where a
chemist expects 2900–3000). Scaling is a **display slider** (default 1.00), NOT a method-specific
number baked into code (we have no measured/cited one) and NOT `$frequency_scale_factor` — that field
is **measured 1.0** = "ORCA applied none"; building "scaled frequencies" on it would show the same
numbers twice. When scale ≠ 1 the table shows raw **and** scaled columns (scaled marked *derived*)
and the curve/sticks move to the scaled positions. If a future run prints ≠ 1.0, that is a measured
fact and may seed the slider.

**`$actual_temperature` audit.** Measured **0.0** on dexketoprofen though thermochemistry was at
298.15 K — it is NOT the calculation temperature. Confirmed it is never used as one: the card's
entropy uses `ThermoJson::temperature_k` (`.property.txt`). Strengthened the `hess.rs` accessor doc
and the `results.rs` population-site comment; the field stays only to surface the raw value (a rename
to avoid the trap is noted, deferred — it would change the stored `data_json` key).

**Teaching page.** `chemistry/ir-spectrum.md` gained "чому обчислений спектр не схожий на
експериментальний" on **measured** dexketoprofen numbers: intensity(km/mol) vs %T (C=O acid 1752.7 =
632.8 km/mol vs all 13 C–H = 125, strongest 24.9 — C–H are objectively weak, they only *look*
prominent in %T); two resolved C=O (1752.7 acid / 1670.1 ketone, lowered by ring conjugation); the
harmonic overestimate + why no factor is coded; **O–H 3714 = free monomer** (real acid dimerizes →
band to ~3000, smeared — the biggest discrepancy, and not in C–H); low modes 21–49 cm⁻¹ → why entropy
is the least reliable number in the card.

**Verified.** `tsc` clean; **vitest 334 passed** (20 files; +10: `irPresentation.test.ts` — scaled
no-op/transform, nearest-mode, single-source tooltip incl. the 115-vs-3714 bug); `vite build` clean
(pre-existing bundle-size warning only). Rust changes are comments only (`cargo` untouched
semantically). In-GUI legs (sticks + two axes rendering, the tooltip, the scale/inversion controls in
the real webview) need the Tauri window — standing headless limitation; the tooltip/scale/nearest
logic is all pure-tested and the recharts v3 hooks are used per their documented custom-shape purpose.
ADR-002/009/010/011/012 + the proposal untouched.

Next (unchanged): Kabsch-alignment gate → normal-mode animation (click an IR peak → watch the atoms move).

## [2026-07-31] session | Unit 3.11: keep the IR x-grid fixed so the display scale actually moves the peaks

A one-defect follow-up to unit 3.10. The display-scale slider looked functional but changed nothing
visible: **the grid was scaled together with the data**. Measured on two panel states (O–H, the
highest mode, 3714.4 cm⁻¹): scale 0.945 → axis max 3591.25, scale 1.040 → axis max 3944 — in both,
axis max = scaled-max + ~81 cm⁻¹. So the range was derived from the ALREADY-scaled frequencies;
multiplying data and ruler by one number gives a self-similar picture — peaks frozen in pixels, only
the tick labels changing (900/1800/2700 → 1000/2000/3000). That defeats the parameter's only purpose:
you scale to compare peak positions against experiment, which needs a **stationary** ruler.

**Fix (`fixedGrid` in `irPresentation.ts`, pure).** The grid bounds now come from the **raw** modes +
the slider's full range, never the current scale: it hands `autoGrid` two synthetic extremes — lowest
raw mode × `MIN_SCALE`, highest × `MAX_SCALE` — so the frame covers every position a peak can reach
across the whole slider and stays constant while the slider moves. Reusing `autoGrid` keeps the exact
pad/0-clamp/step and leaves `ir.ts` untouched (its broadening math is the orca_mapspc-verified part).
The step is FWHM-only, so it was already scale-stable and stays so. The panel's `grid` useMemo now
depends on `active` + `fwhm`, **not** `scale`; the curve is still `spectrum(scaledModes(...), grid)`,
so peaks land at their scaled wavenumbers inside a fixed axis.

**Checked the rest for the same disease.** Everything that *should* move with scale still does and is
computed in the drawn (scaled) space: sticks (`IrSticks modes={scaledActive}`), the curve
(`spectrum(scaledActive, …)`), the selected-mode dashed marker (`raw × scale`), and the tooltip's
nearest-mode (`irTooltipModel(label, curveThere, scaledActive)` — label and modes both in scaled
space, so it compares like with like, not scaled-vs-raw). Only the grid was wrong.

**Test (pure, `irPresentation.test.ts`).** The invariant is now locked, not eyeballed: at two scales
the grid bounds are identical while a given mode's fractional position on the axis differs; every peak
stays inside the frame across the whole slider range; the step is range/scale-independent; and a
reproduction of the old `autoGrid(scaledModes(...))` path shows its max tracked the scale (the bug),
whereas `fixedGrid`'s does not.

**Verified.** `tsc` clean; **vitest 339 passed** (20 files; +5 fixedGrid tests); `vite build` clean
(pre-existing bundle-size warning only). No Rust changes. In-GUI leg (watching the peaks slide against
a fixed axis as the slider moves) needs the Tauri window — standing headless limitation; the invariant
is pure-tested. `wiki/modules/results-ui.md` updated (why the grid is raw-derived). ADR-002/009/010/
011/012 + the proposal untouched.

Next (unchanged): Kabsch-alignment gate → normal-mode animation (click an IR peak → watch the atoms move).

## [2026-07-31] ingest | Unit-3.12 GATE: the .hess $atoms frame is a PURE TRANSLATION (no rotation)

Determiner run before any animation code (`sidecar/probes/hess_frame_kabsch.py`, terminal, not app
code). Kabsch superposition between `.hess $atoms` (Bohr→Å) and the reference geometry the reader
already accepts (`.property.txt` final `$Geometry`, Bohr→Å), in **index order, no correspondence
search**. R maps `.hess → reference`.

| job | max&#124;R−I&#124; | det R | RMSD (Å) | raw per-atom &#124;hess−ref&#124; | &#124;t&#124; (Å) |
|---|---|---|---|---|---|
| ethane-min (8) | 2.05e-13 | +1 | 5.7e-13 | 0.000000 (same frame) | 0.000 |
| saddle (19) | 2.87e-14 | +1 | 4.2e-13 | 1.098986 (uniform) | 1.099 |
| dexketoprofen (33) | 1.22e-14 | +1 | 4.4e-13 | 0.149018 (uniform) | 0.149 |

**Verdict: PURE TRANSLATION on all three** — `max|R−I| ≤ 3e-13` (machine precision), `det R=+1`, RMSD
~1e-13. Independent tell: the raw per-atom shift `|hess−ref|` is **identical for every atom** (min ==
max == mean) — a rigid translation's signature. The **33-atom asymmetric dexketoprofen** is the
decisive witness: any real rotation there is unambiguous, and it is 1e-14. The reader's distance-based
post-condition (rotation-invariant) could never have said this; the gate did. **Consequence:**
`$normal_modes` are added to the reference geometry **as-is** — no mode rotation owed at any boundary
(had R≠I, the reader would owe one, visible in the type like `÷√m`). Narrows the earlier "centre of
mass / Eckart frame" wording to **centre-of-mass translation, no Eckart rotation** on these jobs.
Recorded with matrices in `parse-sources.md`; `hess.rs` / `artifact-readers.md` comments corrected.

**Amplitude calibration (same probe):** at the `orca_pltvib` multiplier A=2.0 the median mode keeps a
min interatomic distance ≈0.95 Å, but the sharpest localized C–H stretches overshoot to 0.02–0.07 Å
(ethane 7/18, saddle 14/51, dexket 16/93 modes < 0.5 Å). So 2.0 is a good default for bends but
overshoots stretches → amplitude is a slider with a **collapse guard** at 0.5 Å.

## [2026-07-31] session | Unit 3.12 Part B: normal-mode animation (click a peak → watch the atoms move)

Gate passed (ingest above), so Part B was written. **No Rust reader change** — the gate proved no mode
rotation is owed (comment-only corrections to `hess.rs`).

**Pure module `src/spectrum/mode.ts`** (node-tested, mirrors `trajectory/frame.ts`): `modeDisplacements`
extracts mode k as the **column** of the row-major 3N×3N matrix (a row would be a different thing — the
seam, locked by a known-matrix test); `modeFrameCoords` = `x_eq + A·sin(2π·phase)·v` (phase 0 =
equilibrium **exactly**, sin 0 = 0); `modeFrameXyz` reuses the trajectory formatter (one path to the
dumb renderer); `minInteratomicDistance` / `modeMinDistanceOverPeriod` are the collapse guard (samples
the whole period, not just the sin=±1 extremes — the per-pair distance is convex in sin).

**`ModeAnimator.tsx` — ownership is the trajectory's, verbatim (ADR-011):** phase (int 0…39),
amplitude, play timer and speed are **application state**; the viewer gets ONE frame, no timer, no
3Dmol `animate`/`setFrame`. The timer loops the period forever (vs the trajectory's play-once).
Identity check at the UI boundary (`elementsAgree(f.elements, geometry.elements)`) before drawing —
mismatch → error, not the wrong atoms. Auto-plays on select; selecting a new mode restarts at phase 0.

**Amplitude is a display choice** (the mode is normalized — no absolute amplitude), default **2.0** =
measured `orca_pltvib` multiplier, labelled as such; a slider like FWHM / display-scale. **Collapse
guard (rule #9):** when the current amplitude drives atoms < 0.5 Å (`MIN_SAFE_DISTANCE_ANGSTROM`,
measured floor), a warning tells the user to reduce it instead of drawing mush.

**Spectrum ↔ mode:** clicking a peak/row (already wired) or now an **imaginary-mode chip** selects a
mode and animates it; the shared `selected` index is also the `$normal_modes` column. **Imaginary
modes are animatable — the teaching payoff:** for a TS the imaginary mode traces the **reaction
coordinate** (downhill both ways), labelled as such (dexketoprofen has none → the saddle fixture is
the imaginary case). Panel gained a `geometry` prop (the final geometry = the equilibrium the modes
animate around); `ResultsCard` passes `results.final_geometry`. Empty states: no `.hess` → panel
absent; frequencies but no `$normal_modes`/bad shape → no animator, the table stands.

**Teaching page** `wiki/chemistry/normal-modes.md` (Ukrainian): what a normal mode is, why amplitude is
arbitrary, why animating an imaginary mode shows the reaction coordinate, why modes are taken as-is.

**Verified.** `tsc` clean; **vitest 351 passed** (21 files; +12 `mode.test.ts` — column-not-row
extraction, phase-0 = equilibrium, ±A extremes, count-mismatch throws, collapse guard flags/passes);
`vite build` clean (pre-existing bundle warning only). Rust untouched semantically (comment-only). The
gate + amplitude calibration are reproducible via `sidecar/probes/hess_frame_kabsch.py` (numpy only).
In-GUI leg (the atoms actually moving in the Tauri webview) needs the window — standing headless
limit; the geometry math, the phase-0 invariant, the column seam and the guard are all pure-tested.
ADR-002/009/010/011/012 + the proposal untouched.

Next: Phase 3 remainder — orbital/density isosurfaces (`orca_plot` → `.cube`), then export.

## [2026-07-31] session | Unit 3.13: chemically-honest mode animation — amplitude = max atomic displacement, frozen bond topology

The first real look at the 3.12 animation found it chemically false in TWO independent ways (measured
on dexketoprofen mode #84 = 1752.7 cm⁻¹, the C=O acid stretch). Fixed both; `probes/mode_amplitude.py`
grounds every number (rule #10).

**Bug 1 — amplitude normalized against the wrong norm.** The mode is unit-normalized over all **3N**
components (measured Σ|v|²=1), so a bare `A·v` gives a localized mode's busiest atom a huge move and a
delocalized mode's a crumb — the "fine for bends, collapsing for stretches" the 3.12 calibration already
saw. That diagnosis was right; the 3.12 answer (a slider + collapse guard) treated the symptom.
**Cause fixed:** `A` now means the **maximum atomic displacement in Å** — normalize by `max_j|v_j|`, the
largest **atomic tri-vector norm** (NOT the largest component — measured ratio 1.07–1.41, up to √3, a
silent error). Verified: max atomic move == A to 1e-9 for the localized C=O #84 AND the delocalized low
mode (#6). Default **0.18 Å** (measured: largest round value keeping the C=O #84 bond's closest approach
≥ 0.9 Å — 0.25→0.808, 0.20→0.889, 0.18→0.921). The `orca_pltvib` 2.0 is dropped as the default — it is a
*norm* multiplier, a different quantity; kept in the wiki so the measured fact isn't lost. The collapse
guard stays as the last line, not the main mechanism.

**Physical amplitude in the label.** Verified the `.hess $atoms` 2nd column IS masses (C 12.0110, H
1.0080, O 15.9990 — rule #10), so the frontend derives mass from the element symbol (a standard-weight
table equal to those) without touching the reader/stored data. Label now shows the mode's real
zero-point amplitude `A0 = √(ħ/2μω)`, μ = 1/Σ(|v_i|²/m_i). Measured C=O #84: μ ≈ 3.12 amu, **A0 ≈ 0.055
Å** — reported vs the reviewer's ≈0.04 estimate: the mode mixes in light-H motion, lowering μ and raising
A0 (discrepancy reported, not fudged). So the label reads "real ≈ 0.055 Å, drawn 0.18 for visibility".

**Bug 2 — bond topology re-perceived every frame.** 3Dmol perceives bonds from each frame's distances,
so an animated stretch made bonds flicker (over-compressed blink, over-stretched detach — the O floating
off in the screenshot). A vibration is the SAME molecule; its graph is a function of the **equilibrium**
only. Fix: new `MoleculeViewer.bondTopologyReference` (the equilibrium xyz) — bonds are perceived **once**
from it (3Dmol's own perception, the sole one — NOT a second implementation, ADR-010: `readFrozenBonds`
reads 3Dmol's result back) and reused every frame (`assignBonds:false` on the frame, then the frozen
pairs applied by array position, which is what `drawBondSticks` indexes by). The **app decides** the
topology (picks the equilibrium reference); the viewer draws (ADR-011). Even at default A a plain 1.75 Å
cutoff keeps the bonded set identical across the period (measured separation 0.10 Å #84 / 0.24 Å low) —
freeze is belt-and-suspenders.

**Trajectory: same disease, LEFT alone (reported, not fixed).** `TrajectoryPlayer` also re-perceives per
frame — but along an opt/reaction path bonds genuinely form/break, so freezing would hide real chemistry.
Different question, different answer; it passes no `bondTopologyReference`.

**Tests (`mode.test.ts`, real fixture `dexketoprofen-modes.json` — 99 modes + geometry + masses).**
max atomic move == A for localized #84 AND delocalized #6 (the normalization proof); atomic-norm-not-
component (diagonal √3 test); phase 0 = equilibrium; #84 min interatomic ≥ 0.9 Å at default A; topology
(1.75 Å cutoff) identical at phases 0/0.25/0.75 for both modes; μ≈3.12 / A0≈0.055 for #84; the element→
mass table equals the fixture's `.hess` mass column; null on unknown element / imaginary freq; collapse
guard still flags/passes.

**Not touched:** artifact readers, stored data, spectrum math (untouched); frame/timer ownership stays
in the app; trajectory topology; ADR-002/009/010/011/012 + the proposal.

**Verified.** `tsc` clean; **vitest 356 passed** (21 files; mode.test 12→17); `vite build` clean
(pre-existing bundle warning only). No Rust changes. Gate/calibration reproducible via
`probes/mode_amplitude.py` (numpy only). In-GUI legs (the actual motion + non-flickering bonds in the
Tauri webview) need the window — standing headless limit; the amplitude math, the max==A invariant, the
topology-stability property and the physical amplitude are all pure-tested.

Next: Phase 3 remainder — orbital/density isosurfaces (`orca_plot` → `.cube`), then export.

## [2026-07-31] session | Unit 3.14: draw bonds again — freeze topology by updating coordinates, not rebuilding

A regression from `2e54e49` (unit 3.13): after freezing bond topology the mode animation drew **atoms
but no bonds at all**. Atoms in place, oscillation plausible (amplitude fix intact), zero sticks.

**Root cause — from the 3Dmol bundle, not memory (rule #10).** 3.13 froze topology by parsing each
frame `assignBonds:false` and setting the equilibrium bonds by hand. The bonds reached the live atoms
(verified: `selectedAtoms()` returns real atoms, not copies; written symmetrically with `bondOrder`).
But `GLModel.drawBondSticks` draws each bond only from the lower index: gate **`atom.index <
atom2.index`**. `assignBonds:false` never runs `assignBonds`, which is what assigns `atom.index` (the
parser sets only `serial`). So `atom.index` was `undefined` on every frame, `undefined < undefined` is
false, and **every cylinder was dropped** — a perfect bond list, nothing drawn. Spheres don't use that
gate, hence "atoms, no bonds". Full writeup: `wiki/debugging/008`.

**Fix — the path where the problem can't exist.** Build the model **once** from the equilibrium
reference (a normal parse → 3Dmol perceives bonds AND assigns `index`, so the gate holds), then each
frame **update only the coordinates** (`applyCoordsToAtoms` over `selectedAtoms({})`) + `setStyle`
(nulls the cached `molObj` → `render` rebuilds sticks at the moved atoms with the same bonds/indices).
No model rebuild, no `assignBonds:false`, no manual bonds — the whole class is gone. Topology frozen by
construction; app decides it (picks the reference), viewer draws (ADR-011); the sole perception is
3Dmol's normal one (ADR-010). This also corrects the unit-3.8 belief that in-place coordinate updates
need 3Dmol's `setFrame`/`animate` apparatus — they don't; frame ownership stays in `ModeAnimator`.
Logic in the pure `src/viewer/frozenTopology.ts`.

**Trajectory untouched** (checked): it passes no `bondTopologyReference` → the normal rebuild-per-frame
path, re-perceiving bonds (correct — bonds change along a path). Camera still preserved (zoom only on
first build of the frozen model). Amplitude / default 0.18 Å / physical-amplitude label untouched.

**The lesson — the test checked the INPUT, not the OUTPUT.** 3.13 shipped green: its test asserted our
*bonded set* (our coords + a cutoff) was stable across phases — true even when nothing reached the
screen. A blank render is invisible to an input-side test. 3.14's test targets the output:
`drawableBondCount` mirrors 3Dmol's `atom.index < atom2.index` stick gate — **>0** for a normal parse,
**0** when `index` is unset (the regression reproduced), constant across coordinate updates. 3Dmol needs
WebGL (no jsdom → the rendered pixels still can't be checked headless; boundary named), so a **DEV
assertion in the viewer** warns in the real webview if a built frozen model has 0 drawable bonds — the
check 3.13 lacked, now on the real object.

**Verified.** `tsc` clean; **vitest 363 passed** (22 files; +7 `frozenTopology.test.ts` incl. the
regression reproduction); `vite build` clean (pre-existing bundle warning only). No Rust changes. In-GUI
leg (bonds actually rendering as atoms move in the Tauri webview) needs the window — standing headless
limit; the coordinate-preservation and draw-gate invariants are pure-tested and the DEV warning guards
the real object. ADR-002/009/010/011/012 + the proposal untouched.

Next: Phase 3 remainder — orbital/density isosurfaces (`orca_plot` → `.cube`), then export.

## [2026-07-31] ingest | Unit-3.15 GATE: orca_plot batch, cube sizes, WebKitGTK isosurface — all measured, PASS

Gate before any UI (`/tmp` runs + MiniBrowser, not app code). Full page: `wiki/orca/orca-plot.md`.

1. **Non-interactive orca_plot.** Its usage advertises `orca_plot gbw-file plot-inputfile`, but that
   batch file's field order is undocumented and unsatisfiable from a run — after `PlotType/Format/MO-OP`
   it demands a "state density"/"infile" field; every attempt exited rc=64 FATAL, no cube (an early
   "success" was a STALE cube — corrected). **What works: drive the interactive menu over stdin**
   (`printf "2\n{mo}\n4\n{grid}\n11\n12\n" | orca_plot input.gbw -i`) — deterministic, produces
   `input.mo{N}a.cube`. Invocation per `orca_json.rs` (ADR-009): path from `dirname(settings.orca_path)`,
   LD_LIBRARY_PATH, cwd=job dir. Menu numbers pinned to ORCA 6.1.0.

2. **Sizes/times (HOMO of dexketoprofen, 33 atoms).** 40³ 0.87 MB/0.06s · 60³ 2.9 MB/0.16s · **80³ 6.9
   MB/0.36s** · 100³ 13.5 MB/0.67s. ASCII, ~13.75 bytes/point, size = (N+1)³·13.75 — scales with grid,
   independent of atom count at fixed N. Extrapolation to ~60 atoms at equal resolution (N≈120): ~24 MB /
   ~3s (arithmetic in the page). Rule #5's 80–100 default verified — 80³ = 6.9 MB, not "hundreds of MB".
   3Dmol needs the whole cube text (VolumeData parses a full string — stated), so it is read whole once;
   the app caps the read at 32 MB and refuses larger.

3. **WebKitGTK isosurface — the real unknown, PASSED.** MiniBrowser probe (debugging/002 technique) in the
   identical webkit2gtk-4.1 engine: `OffscreenCanvas=undefined` fix + addModel(cube) + two addVolumetricData
   (+/− lobes) + render → window title `ISO_OK` (no exception) and a screenshot shows the HOMO's blue
   (+phase)/red (−phase) lobes on the molecule. The volumetric path works in the real engine. Author still
   confirms in the actual Tauri app.

**Verdict: PASS on all three.** Part B (lazy cached cube generation + orbital picker + isovalue slider)
proceeds. No ADR touched.

## [2026-07-31] session | Unit 3.15: orbital isosurfaces from orca_plot cubes (+ three-column freq table)

The last big Phase-3 visualization. Gate first (ingest above / `wiki/orca/orca-plot.md`), then Part B.

**Gate (measured, PASS on all three).** (1) `orca_plot`'s advertised `gbw plot-inputfile` batch mode was
**unusable** — after PlotType/Format/MO-OP it demands an undocumented "state density" field; every attempt
exited FATAL with no cube (an early "success" was a STALE cube). Non-interactive route that works: **drive
its interactive menu over stdin** (`2\n{mo}\n4\n{grid}\n11\n12\n`), producing `input.mo{N}a.cube`. (2)
Cube size/time by grid (HOMO, dexketoprofen): 40³ 0.87 MB/0.06s · 60³ 2.9/0.16 · **80³ 6.9/0.36** · 100³
13.5/0.67; ASCII ~13.75 B/point, size = (N+1)³·13.75 (independent of atom count at fixed N); ~60-atom @120³
≈ 24 MB/~3s. Rule #5's 80³ default verified. (3) The real unknown — WebKitGTK rendering a 3Dmol isosurface —
**PASSES**: MiniBrowser probe (debugging/002 technique) with the EXACT app API path (`new VolumeData` +
`addIsosurface(+/-)` + `removeShape`+re-add on isovalue change) reached `ISO_OK` and a screenshot shows the
HOMO's blue(+phase)/red(−phase) lobes. Author still confirms in the actual Tauri app.

**Rust (`orca_plot.rs`, mirrors `orca_json.rs`, ADR-009).** `ensure_mo_cube(orca_path, job_dir, mo, grid)`:
lazy + cached in the job dir under a **grid-keyed** name `orbital.mo{N}.g{G}.cube` (so grids of one MO
coexist), regenerated only when missing/older than the gbw. Path from `dirname(settings.orca_path)` (rule
#7), LD_LIBRARY_PATH, cwd=job dir (rule #3); stdin written then dropped (EOF) so it can't hang. Command
`read_orbital_cube` reads the cube capped at **32 MB** (3Dmol needs the whole text — stated, not hidden)
and returns the text or `None` (xTB/GOAT gbw → normal). **Cubes never touch the DB.** Real integration test
(`#[ignore]`) generates + caches + confirms a second call is a cache hit; 3 unit tests for the menu script /
cache name / no-gbw path.

**Frontend.** `orbitals/orbitalList.ts` (pure, tested): HOMO = highest occupied, LUMO = first virtual,
default = HOMO. `OrbitalPanel.tsx`: picker (MO#, Eh+eV, occupancy, HOMO/LUMO tagged) + **isovalue slider**
(display choice, default 0.05) + a +/− phase legend; `invoke("read_orbital_cube", …)` on select (grid fixed
80³). `MoleculeViewer` gained an `orbitalCube`/`orbitalIsoValue` path: molecule built once from the cube's
atoms (model effect), ± isosurfaces in a **dedicated effect** that parses the cube into a `VolumeData` once
(cached by text) and on an isovalue change `removeShape`s exactly its two surfaces + re-adds — no re-parse;
the scene-editor overlay effect is guarded to leave those shapes alone. **State app-owned (ADR-011)**; one
scene, one mode (no animation in the orbital view). Absence hides the section.

**Task 0:** `FrequencyTable` flows the 93 modes into **three** columns (`.ir-table-columns`, wraps on a
narrow window); selection/peak↔row/scaled logic unchanged.

**Teaching:** `chemistry/orbitals.md` (Ukrainian) — MO/isosurface; the two colours are ψ **phase, not
charge**; isovalue as a viewing choice; why HOMO/LUMO are the frontier orbitals.

**Verified.** `tsc` clean; **vitest 372 passed** (23 files; +9 `orbitalList.test.ts`); `vite build` clean
(pre-existing bundle warning); `cargo test` **124 passed** (+3 orca_plot unit; +1 `#[ignore]` real cube-gen
test passes on demand). The isosurface renders in the real webkit2gtk-4.1 engine (MiniBrowser screenshot);
the React wiring (invoke/effects) is not headless-drivable — standing Tauri-GUI limit. ADR-002/009/010/011/
012 + the proposal untouched.

Next: Phase 3's last item — export (xyz / CSV / PNG).

## [2026-07-31] ingest | Unit-3.16 GATE: WebKitGTK PNG export — both paths PASS (measured)

Gate before the PNG-export UI (MiniBrowser probes, 002 technique; full page `wiki/debugging/009`).
- **Charts (recharts SVG → 2D-canvas → PNG):** serialize `<svg>` → `data:image/svg+xml` Image →
  drawImage onto a 2D canvas → `toDataURL("image/png")`. **`SVG_OK 6237`** — real PNG, no exception,
  no taint. (App caveat, not a WebKit limit: `var(--…)` in recharts colours must be resolved to
  computed values in the serialized SVG — handled in `export/png.ts`.)
- **3D scene (3Dmol WebGL → `pngURI()` readback):** **`PNG_OK 17388`** — real PNG, no exception. The
  GL readback works under webkit2gtk-4.1 with the 002 direct-canvas fix. (A first "stuck" run was a
  probe escaping bug, not WebKit.)
**Verdict: both PASS** → all three PNG exports (spectrum, energy-per-cycle, 3D snapshot) are in scope;
nothing dropped or faked. Author confirms in the real app. Data export (xyz/CSV) isn't gated.

## [2026-07-31] session | Unit 3.16: export (geometry/data/plots) + core-orbital marking + a lines representation — Phase 3 closes

The last Phase-3 unit. Gate first (ingest above), then the exports; two orbital-review follow-ons ride along.

**Task 0 — core-orbital marking (DERIVED).** The author built MO 0 → blank; the render only wakes at
MO 19 because C₁₆H₁₄O₃ has exactly 19 core orbitals (3 O-1s + 16 C-1s), the boundary sitting there.
`orbitalList.ts` marks core from a **per-element table** (H/He→0, Li–Ne→1, Na–Ar→5; anything else →
no mark — NOT "1s per heavy atom", which holds only for the 2nd period) **cross-checked** against the
biggest low-energy gap: the count is placed only if the table's number equals the gap position, else no
mark + the mismatch reported. Named DERIVED, like T·S and the display scale. Measured: expected 19, the
−10.03→−1.08 Eh gap falls after 19 → agree → MOs 0–18 tagged core.

**Task 1 — representation toggle.** A core 1s hides inside the atom's drawn sphere (occlusion), so
`MoleculeViewer` got a `representation` prop (**stick / lines**, two only) + a shared `RepresentationToggle`;
app-owned (ADR-011), honoured on the orbital, mode-animation and single-xyz paths.

**GATE — WebKitGTK PNG (both paths PASS, measured).** MiniBrowser probes (`wiki/debugging/009`): recharts
SVG→2D-canvas→`toDataURL` (`SVG_OK 6237`) and 3Dmol WebGL `pngURI()` readback (`PNG_OK 17388`). Nothing
dropped. (A first "stuck" 3D run was a probe escaping bug, not WebKit; and `pkill -f MiniBrowser` kills the
probe shell itself — kill by PID.)

**Export.** New `tauri-plugin-dialog` (native save dialog) + Rust `write_export_text` / `write_export_bytes`
commands that **refuse any path under the app data dir** (rule #3; the default is elsewhere too). Content is
built from the already-parsed `results` — **no re-parse** (ADR-012). `export/exporters.ts` (pure, tested):
`finalGeometryXyz` (Å, stored order, comment = job+energy, post-condition lines == atoms+2 or throw);
`frequenciesCsv` (active modes; a *derived* `scaled ×N` column only when the panel's scale ≠ 1); `chargesCsv`;
`orbitalsCsv` (Eh+eV); `thermochemistryCsv` — **units in every header**, full stored precision, `entropyS`
exported as **T·S in Eh** (never "entropy") with a separate *derived* S in J/(mol·K). PNG: `svgToPngBytes`
(serialize the `<svg>`, resolve `var(--…)` to computed colours, white bg, fixed 2× canvas) for the spectrum
and energy charts; `MoleculeViewer.toPngBytes()` (imperative handle → `pngURI()`) for the 3D scene. Buttons:
data bar in `ResultsCard`; freq CSV + spectrum PNG in `IrSpectrumPanel` (owns the scale + chart); energy PNG
in `TrajectoryPlayer`; 3D snapshot in `OrbitalPanel`. Absent data → disabled button, never an empty file.

**Verified.** `tsc` clean; **vitest 388** (24 files; +5 core-orbital, +11 exporters); `vite build` clean;
`cargo test` **126** (+2 export-guard: a path inside the data dir is refused, outside is allowed). The write
path and the PNG rasterization are not headless-drivable (dialog + canvas), so the pure builders + the guard
+ the measured WebKit gate carry it; author confirms the dialogs in the real app. No ADR touched (dialog
plugin added; ADR-009 already puts file I/O in Rust). **Phase 3 complete.**

Next: Phase 4 — the geometry/reaction editor (ADR-010/011 land).

## [2026-07-31] lint | Post-Phase-3 wiki lint — parse tier moved to Rust, cclib removed, roadmap realigned

First lint since ~unit 3.1 (16 units: ADR-012 killed cclib, the authoritative parse tier moved from
the sidecar to Rust, four artifact readers landed, rules #9/#10/#11 were added, orbitals passed a
gate, the plan shifted twice). **Code/tests/probes untouched — wiki + ROADMAP + CLAUDE.md only.**

**Found: 15 items across 5 categories.**

- **Stale — cclib survivors (7):** `CLAUDE.md` (repo layout + tech-stack said "cclib parsing"),
  `overview.md` (diagram "cclib: full output parsing", "sidecar owns what things mean chemically",
  data-flow "Rust calls sidecar /parse → cclib JSON"), `parser.md` (status + Tier-2 "first of four
  built, rest not started, not wired" + "sidecar/cclib tier not yet built"), `sidecar.md` (status,
  responsibilities, deps, the planned `POST /parse → cclib JSON` endpoint). **Fixed** — all now say
  the tier is Rust over structured artifacts (ADR-012), cclib rejected; `/parse` marked REJECTED, not
  built. cclib mentions in ADR-002/012, `log.md`, `parse-sources.md`, `output-files.md` are
  historical/measured/decision — **left as chronicle**.
- **Stale — Phase-3 "not started" claims (4):** `visualization.md` (status "trajectories/orbitals/
  spectra not started", "Orbitals (planned)", "Spectra — mode animation deferred") and `frontend.md`
  ("Screens (planned): Results — Phase 3"). **Fixed** — all done (units 3.8–3.16), pointing to
  results-ui.md; the viewer's Phase-3 props documented.
- **ROADMAP drift (2):** summary card `[~]` → `[x]` (feature-complete); the per-atom-seam `[ ]`
  reworded (parsing IS done with named order assumptions; the typed `AtomId`/`IndexMap` is Phase-4.2
  Stage 1, where it is already tracked) and "Phase 3 complete" clarified vs the one remaining
  cross-cutting `[ ]` (stereocenter-flag on import). Phase 0–2/4.x statuses spot-checked — accurate.
- **CLAUDE.md rule wording (1):** rule #5 read as violated (we read `.property.txt`/`.hess` whole).
  **Reworded** to distinguish the unbounded `output.out` (never whole) from the small, size-capped
  structured artifacts (read whole, justified) — no code change, the practice was already right.

**Checked clean (no action):** no orphan pages; **page count 52 is factually correct** (54 files −
index.md − log.md). **No measurement divergences** — `.hess` reframe (pure translation), Bohr/Å,
`entropyS`=T·S, imaginary modes, index bases are consistent across parse-sources (canonical) /
output-files / orca-plot / artifact-readers; only duplication-with-references, which is fine.
Language convention holds (chemistry/ Ukrainian, rest English).

**Planning added (not lint):** ROADMAP Phase 6 UV-Vis (TD-DFT) and NMR fleshed out with real
dependencies — UV-Vis needs an excited-state-source probe (gap already flagged) + a real TD-DFT run,
Gaussian broadening, nm-vs-eV axis, an assumed band width, and a hybrid/range-separated functional
(r²SCAN-3c unfit for excited states); NMR needs σ→δ (a second same-method reference calc) + Boltzmann
averaging (GOAT exists) + equivalent-nuclei averaging, so it waits until Reaction is a first-class
multi-job object (Phase 4.5) — the same aggregation need as ΔΔG‡.

**Surfaced, not self-decided:** none blocking. The per-atom-seam item is duplicated between Phase 3
(now a note) and Phase 4.2 Stage 1 (the actual typed-seam work) — left in both, not deleted.

## [2026-07-31] decision | ADR-013 manual indexing ownership + FTS5 build-gate test + two stale wiki lines

A pre-Phase-4 unit whose whole risk is silent: no crashing code, just wiki lines that lie and one
architectural decision that, unrecorded, would be re-made in Phase 4 (maybe differently). Neither
class is caught by a test — hence a unit of its own, before Phase 4.

**ADR-013 (accepted, narrows ADR-006 — ADR-006 NOT edited, the ADR-012-narrows-ADR-002 precedent).**
Fixes the *who*/*how* of manual indexing (the *what* — local FTS5 index — is ADR-006, unchanged):
(1) **only Rust writes `orcastudio.db`** and does the indexing — the sidecar's own invariant is
"stateless, all persistence is Rust-owned SQLite", so a sidecar indexer would break the sidecar's own
boundary; plus a second argument, the sidecar links **system** libsqlite3 while Rust links its **own**
amalgamation (3.46.0, `-DSQLITE_ENABLE_FTS5` unconditional — measured), and two SQLite builds writing
one file give two answers to "is FTS5 here?". (2) **The app never fetches the manual over the
network** — fetch is an out-of-band author script writing `resources/manual/` (RAW, immutable),
preserving `overview.md`'s no-extra-network posture; not a Tauri command, not a sidecar endpoint.
(3) **Sectioning in Rust** (ADR-012's text-to-structure-without-a-chem-library rule), over **Markdown
with ATX headings**, not HTML — measured: the ORCA 6.1 manual is Sphinx+MyST with `html_copy_source`,
Markdown sources at `_sources/*.md.txt`, so no HTML parser in any language. Named review condition: if
the Phase-4.1 gate shows real files need a true MyST parser, (3) reopens; (1)/(2) do not depend on it.
Separate finding: `keywords.json` is **seeded** from the manual's dozens of native Keywords sections +
genindex and curated on top — narrowing ADR-006's "curated by hand".

**FTS5 build-gate test** (`db.rs::fts5_is_available_with_ranking_and_snippet`, a test — NOT a
migration/table). On an in-memory conn: `CREATE VIRTUAL TABLE ... USING fts5` (porter unicode61),
MATCH + `snippet()`, porter stemming, and `ORDER BY bm25(...)` **without DESC** puts the most relevant
first (bm25 returns NEGATIVE scores, less = more relevant — counter-intuitive, so it's asserted, not
commented). Purpose is the future rusqlite upgrade: if the build ever drops FTS5, it fails here, not
in Phase 4. Measured baseline holds: libsqlite3-sys 0.30.1 `build_bundled` = SQLite 3.46.0 with FTS5
unconditional.

**Two stale wiki lines** the 2026-07-31 lint missed (it checked pages, not the one-line catalog
descriptions): `index.md`'s `artifact-readers.md` blurb still said "`.property.txt` built, 3 others
not started" while the page says all four complete — rewritten (and all other index descriptions
spot-checked against their pages; only this one drifted). `modules/tauri-core.md` said "migrations
(v1–v5)" while `SCHEMA_VERSION = 8`, and its migration list stopped at v7 — added v8 (the unit-3.9
`jobs.energy` backfill, previously documented only in `debugging/007` + `results-ui.md`) and fixed the
range; other migrations untouched.

**Consequences applied:** `modules/sidecar.md` — `/manual/build-index` + `/manual/search` marked
**REJECTED (ADR-013)** (same style as `/parse`); sidecar not involved in Phase 4. `modules/
manual-index.md` — pipeline rewritten to Rust. `resources/manual/README.md` — "sidecar indexing
pipeline" line corrected to author fetch-script + Rust indexing. `ROADMAP.md` Phase 4 — added an
ADR-013 pointer note; **statuses unchanged**. No DB migration (SCHEMA_VERSION stays 8), no new
dependency (Cargo.toml/requirements.txt untouched), ADR-002/004/006/012 untouched.

**Verified:** `cargo test` green (+1: the FTS5 test); `tsc`/`vitest` untouched (out of scope). Repo
grep for "not started"/"cclib" in `wiki/`: remaining hits are historical/chronicle — `log.md` (past
entries), `manual-index.md`'s own "Status: not started" (Phase 4 hasn't started — correct), ADR-012's
retarget note, and cclib in the ADRs/parse-sources/output-files as the rejected-decision record.

Next: Phase 4 — the manual indexer (4.1 gate on the real `_sources/*.md.txt`).

## [2026-07-31] ingest | Part-A GATE: measured ORCA 6.1 manual source format (Sphinx/MyST toctree)

`scripts/fetch-manual.py --manifest --sample 6` — an out-of-band author script (ADR-013 (2): not a
Tauri command, not a sidecar endpoint; stdlib only, no new dependency). Builds the file list by a
**deterministic toctree walk**, never a crawl (URLs come only from manifest paths; body links are
never followed). Measured facts (full page: `wiki/orca/manual-sources.md`):

- **Manifest (140 paths):** 11 container `index*` pages (all 200) + 126 leaf `.md.txt` + 3 generated
  no-source, named — `bibliography`, `genindex`, `html_versions`. No silent skips.
- **The `//` trap is real, not hypothetical:** the root `index.md.txt` writes one toctree entry as
  `contents/structurereactivity//index_structurereactivity`; `normpath` collapses it — without that,
  the entire **Structure and Reactivity** branch (Opt/Scan/TS/IRC/NEB/GOAT — the research program)
  would vanish silently. It was the only path the walk had to normalize.
- **toctree shapes handled:** backtick AND colon fences (varying `:::`/`::::` lengths); `:`-option
  lines skipped; root `{eval-rst}` `.. toctree::` read (holds genindex/bibliography); `Title <path>`
  and bare entries.
- **ATX sectioning:** sample 44 headings, `#`=7/`##`=20/`###`=17, deepest `###` in the sample (corpus
  goes deeper — Part-B full sweep). Corpus ≈ **2.8 MiB** Markdown (126 leaves × ~23.5 KB mean).
- **Keywords markup is HETEROGENEOUS (the ADR-013 seed paragraph's ground truth):** RI's `## Keywords`
  is a labeled `:::{table}` MyST directive over a GFM pipe table (col1=keyword, col2=desc); the `%cpcm`
  "Complete Keyword List" is NOT a table but an annotated ` ```orca ` code block (`name value #
  desc`). Seeding `keywords.json` survives — the data IS structured — but needs **two extractors**
  (table + `%block` code-block), curated on top. Refines, does not break, ADR-013's keywords para;
  concerns the seeder, not sectioning, so it does not touch the (3) review condition.
- **Anchors:** `(label)=` → `#slug` where slug = lowercase + non-alnum-runs→`-`; verified **46/46**
  labels against real HTML ids, across `sec:`/`tab:`/`fig:`/`table:`. And **`objects.inv` exists**
  (46 257 B) — the authoritative label→anchor map, so slugify need not be trusted as a guess (NOT
  parsed this unit, per scope — flagged for the sectioner/keyword units).
- **ADR-013 (3) MyST-parser review NOT triggered by the sample:** body `{eval-rst}` = 0 (root = 1,
  expected). ATX-only sectioning holds so far; definitive over all 126 is Part B.
- **Network hygiene:** descriptive UA + contact link, ~0.7 s pause, 5xx/timeout backoff (≤3), hard cap
  250 requests. Run used **24/250**. `resources/manual/` untouched — nothing written to disk, nothing
  committed (Part A measures in memory).

Part-A commit (this): `scripts/fetch-manual.py` (--manifest/--sample) + `wiki/orca/manual-sources.md`
+ index/manual-index/ROADMAP wiring. **STOP at the gate** — Part B (`--all` full fetch,
`manifest.json`, in-our-terms post-conditions, git-clean license check) awaits author approval.

## [2026-07-31] session | Part B: full ORCA 6.1 manual fetch (--all + manifest.json + post-conditions)

Extended `scripts/fetch-manual.py` with `--all` (author-approved after the Part-A gate): full fetch of
all 126 leaves into `resources/manual/6.1/<path>.md.txt`, a `resources/manual/manifest.json`
(path/URL/status/size/sha256/ETag/Last-Modified/fetch-time + ORCA version + run date), idempotency,
and in-our-terms post-conditions (rule #9). Still stdlib only, no new dependency; `db.rs`/sectioner/
`keywords.json`/DB schema untouched (out of scope).

**Full-corpus measurements (Part B re-measured over all 126, not the sample):**
- **ATX:** 2055 headings — `#`=593, `##`=657, `###`=606, `####`=193, `#####`=6; **deepest `#####`
  (level 5)**. TOC implied `####` exists — confirmed (193) and one deeper (e.g. `orca_2json`). No
  discrepancy; deep subsections ARE ATX, so ATX-only sectioning reaches them.
- **body `{eval-rst}` = 0 across all 126** (root has 1, expected). This is the number that **closes**
  ADR-013 (3): ATX-only sectioning is sufficient, definitively — not "so far".
- **Exact corpus: 4 084 799 B = 3.90 MiB** over 126 leaves (mean ≈ 32.4 KB). The Part-A ~2.8 MiB was
  an under-estimate (the three tiny `preface/*` sample leaves pulled the mean below corpus).
- **All 1448 labels ASCII** → `predict_anchor`'s `[^a-z0-9]+`→`-` is lossless here.

**Idempotency — a measured correction (rule #10).** The manifest showed the server sends
**Last-Modified on all 126 but NO ETag (0/126)**, so `If-None-Match` could never 304. Added an
`If-Modified-Since` fallback; verified a second `--all` reports **downloaded=0, reused=126**. `--force`
refetches. (Memory/HTTP-lore said "use ETag"; the run said this server has none — fixed per the run.)

**Post-conditions PASS:** every stored file is text (not `<!DOCTYPE`/`<html>`), non-empty, and OK-count
== 200-leaf-count (126 == 126); mismatch would exit non-zero with a named list. Requests: 137/250, 0
failures. **License:** `resources/manual/*` (Markdown + manifest.json) gitignored except README —
`git check-ignore` confirms both; `git status` clean under the dir; **nothing committed** (ADR-006).

**Wiki:** `orca/manual-sources.md` updated to full-corpus numbers (the 2.8 MiB estimate labelled and
replaced by exact 3.90 MiB; ATX distribution; eval-rst=0; ASCII), anchor map reframed as a **rule-#9
post-condition** (`objects.inv` authoritative × `predict_anchor` independent check, both asserted;
objects.inv still NOT parsed — intent only), `manifest.json`/idempotency/license sections added.
**ADR-013 amendment appended** (decision text untouched, ADR-012-precedent): keyword markup
heterogeneous → two extractors, (b) `%block` code-block the richer source; (3) closed.

Next: Phase 4.1 sectioner (Rust) over the fetched Markdown — ATX heading tree; and the label→anchor
map as objects.inv × predict_anchor.

## [2026-07-31] ingest | Fence-aware ATX recount — 464 phantom headings were ORCA '#' comments

Defect (self-inflicted, silent): `analyze_atx` and `classify_keywords_markup` in
`scripts/fetch-manual.py` matched `^#{1,6}\s` on EVERY line, not tracking ` ``` ` fences — while
`parse_toctrees` in the SAME file tracked fences correctly. One file, two implementations of one job,
one naive. ORCA input comments with `#`, the manual is full of ` ```orca ` examples, so comment lines
(`# ...`, `#### separator`) were counted as headings. Symptom that gave it away: `#`=593 over 126
pages ≈ 4.7 H1/page, where a Sphinx page has one.

**Fix — one rule, one home.** New `iter_prose_lines(text)` yields only lines OUTSIDE fenced blocks
(backtick code fences AND `:::` directives, any length, nesting via longer-outer/shorter-inner — the
same close rule `parse_toctrees` uses, widened to code fences it doesn't need to see). `analyze_atx`
and `classify_keywords_markup` now both route through it. `parse_toctrees`/`count_eval_rst` untouched
(never buggy — so **body eval-rst = 0 still stands**, (3) stays closed). Regression test `--selftest`
(in-code fixture, no network): two real headings + a ` ```orca ` block with `# comment` / `####
separator` / inline `#` → asserts exactly 2 headings, none from the block. PASS. New `--analyze-only`
recounts the on-disk corpus with **no network** (rule: don't re-download).

**Recount (offline, all 126 leaves):**
- **Corrected ATX:** `#`=129, `##`=654, `###`=604, `####`=193, `#####`=6 (1586 total); deepest still
  `#####` (5). Old naive: `#`=593 … (2055 total). **464 false level-1** removed (469 all levels) — all
  ORCA `#` comments. `####`/`#####` were unpolluted (real deep sections). Matches the ~440–470 estimate.
- **H1 per page:** 3/126 leaves have TWO H1 (`numericalintegration`, `preface/foreword`, `magnx`) —
  real double-top-heading pages (verified), so a sectioner must not assume one H1/file. Reported, not
  explained.
- **Keyword markup reclassified over ALL 126 (79 headings):** ` ```orca ` code block **33**, pipe table
  **27**, prose **21**, `{list-table}` **1**. So it is **three** structured forms + a prose tail, not
  two — the sample's "two extractors" was an undercount. `{list-table}` is a third structured markup;
  21 prose sections are curation targets, not extractor input. Seed-then-curate still holds (majority
  seedable). Corpus size unchanged (3.90 MiB) — the bug was heading-counting only.

**Direct spec recorded for the next unit** (`manual-sources.md`): the sectioner MUST support ≥5 ATX
levels AND MUST ignore `#` inside fenced blocks (~460 phantom sections otherwise). Wiki updated:
`orca/manual-sources.md` (corrected distribution + named cause + 3-form keyword table + sectioner-
requirements spec), ADR-013 amendment corrected (append, decision text untouched). No app code / db.rs
touched; no objects.inv parse; no deps; `resources/manual/` not committed.

## [2026-07-31] lint | manual-sources.md: drop stale two-form keyword paragraph; add H1 post-condition

The fence-aware recount rewrote the keyword section to three structured forms + a prose tail, but a
**previous-version paragraph survived directly under it** ("Some sections … read as prose … table
extractor + %block code-block extractor … not one uniform table format") — two forms where the block
above says three. A knowledge page describes the present; the "at first it looked like two" history is
already in the two ADR-013 amendments + log.md. **Deleted.** Its guess "likely mix prose with one of
the two forms below their heading" also **contradicted the measurement**: `_keyword_forms` adds `prose`
only `if not forms`, so a prose-classified section has no in-body structured form — noted inline
instead of guessing.

Added two measured lines: (1) **why there are two fence scanners and it isn't debt** —
`parse_toctrees` is an allow-list (acts only inside `{toctree}`/`{eval-rst}`, so a bare ` ```orca `
cannot inject a manifest entry), `iter_prose_lines` is a deny-list (must see every fence); shared close
rule, separate on purpose. (2) **H1 identity as a checked post-condition** — 129 = 126 leaves + 3 real
double-top pages (numericalintegration 2.9+2.10, foreword 6.1.1+6.1.0, magnx 5.27+5.28); `--analyze-only`
now asserts H1 ≥ 126 and exits non-zero otherwise, since H1 < 126 is the one way an unclosed fence
could make `iter_prose_lines` silently swallow a file (measured: 0 zero-H1 leaves). No app code / db.rs;
no deps; resources/manual/ not committed.

## [2026-07-31] session | Unit 4.2: manual sectioner + objects.inv anchor map (Rust), line-conservation gate

Built `src-tauri/src/manual/` (mirrors `src/parse/`): `sections.rs`, `objects_inv.rs`, `mod.rs`,
`tests.rs`. Rust per ADR-013 (3). **No DB touched** — no migration, table, FTS5, or Tauri command;
schema/storage are unit 4.3. One dependency: `flate2` (already transitive in Cargo.lock 1.1.9 —
declaration only), for the zlib in `objects.inv`.

**Section definition (fixed explicitly):** one ATX heading, body to the NEXT heading of ANY level,
bodies **not nested** (a `#`'s body is just its preamble to the first `##`) — required for FTS and it
is what makes line conservation checkable. Headings found **only outside fenced blocks** (Rust port of
the deny-list `prose_mask`; the Python `iter_prose_lines` is now removed). **Label binding:** the
`(name)=` lines directly above a heading (blanks skipped) bind to it; `anchor` = slug of the closest.

**Three post-conditions IN CODE (rule #9):** (a) line conservation inside `sectionize` — sections +
preamble tile the file exactly once, else `SectionError` naming the lost/overlapping indices; (b)
`predict_anchor` vs `objects.inv` fragment per label; (c) label→section-file binding vs the `objects.inv`
uri. (b)/(c) are `objects_inv::verify_against_inventory` (library code, reused by the gate). The
`#[ignore]` corpus gate (`cargo test manual_corpus -- --ignored`) runs all three over the 126 leaves.

**Gate report (measured, inputs for 4.3's schema):** 1586 sections (#=129 ##=654 ###=604 ####=193
#####=6 — identical to the fence-aware ATX recount, so Rust and the Python count agree); sections/file
min 1 / median 7 / max 162; deepest breadcrumb 4. Body median 1330 B / p95 9074 B / max 48 245 B;
27 empty-body (1.7 %). Labels: 1069 with, 517 without; **140 unlabelled sections collide on title-slug
within a file** (title slug is not a unique key — flagged for 4.3). objects.inv 1671 entries (1450
std:label); of 1069 heading labels **944 found, 0 anchor mismatches, 0 binding mismatches**; the 125
not found are **all `sec:`** (genuine section labels Sphinx didn't register, not over-capture). **Line
conservation 126/126 PASS.** Bytes prose 57.3 % / fenced 42.7 % (raw-vs-cleaned indexing is a 4.3
choice). `objects.inv` fetched once via `scripts/fetch-manual.py --objects-inv` (46 257 B, gitignored).

**Atomic rule migration (ADR-010 (ii)):** removed from `scripts/fetch-manual.py` — `iter_prose_lines`,
`analyze_atx`, `classify_keywords_markup`, `_keyword_forms`, `_naive_atx`, `analyze_disk`, `selftest`,
`predict_anchor`, `find_labels`, `count_eval_rst`, `html_url`, and modes `--analyze-only`/`--selftest`;
plus the `report()`/`report_all()` content-analysis sections that consumed them. The script keeps ONLY
fetch / manifest / toctree (`parse_toctrees`, the allow-list, untouched) / `objects.inv` (+ new
`--objects-inv`). No second Python home for the heading/keyword/anchor rule — it moved atomically. The
numbers those functions produced stay in `manual-sources.md` as measured, now recomputed by the Rust gate.

**Verified:** `cargo test` 138 passed / 6 ignored / 0 warnings; corpus gate green; `tsc`/`vitest`
untouched (no TS). No db.rs / command / UI. `resources/manual/*` (incl. objects.inv) gitignored, not
committed. Wiki: +modules/manual-sections.md, index.md, manual-sources.md (gate numbers + Rust-migration
note), manual-index.md (pipeline: sectioner built, storage 4.3).

Next: unit 4.3 — the `manual_sections` schema + FTS5 storage over these sections.

## [2026-07-31] ingest | Sphinx lowercases label names — case-fold the objects.inv key (125 "not found" → 1)

Defect in `objects_inv::verify_against_inventory`: the inventory map was keyed by the raw,
case-sensitive `e.name`, so the 4.2 gate reported 125/1069 heading labels "not found". Arithmetic ruled
out missing data (corpus 1448 `(…)=` targets vs inventory 1450 `std:label` — equal cardinality), which
pointed at key spelling.

**Measured (rule #10, not assumed):** Sphinx's std domain **lowercases** label names before writing
`objects.inv`. Of the 125, **124 matched after lowercasing** — the manual is full of acronyms and
camelCase command labels (`BohrToAngs`, `closeFile`, `makeReferenceFromDir`). The **1 remainder**,
`sec:spectroscopyproperties.nocv.theory`, is absent from the inventory entirely (not a substring) — a
genuine unregistered section label, not case.

**Fix:** one `normalize_label` (lowercase) called on BOTH sides — the map build and every lookup — not
two scattered `.to_lowercase()`. Result: **not_found 125 → 1**, and labels actually cross-checked
**944 → 1068**, still **0 anchor + 0 binding mismatches** (so the anchor rule and binding rule now hold
1068/1068, a far stronger check than 944/944). `entries_not_ours` 727 → 603.

**Made the old reading impossible (rule #9):** the gate now prints the denominator for (b) and (c) —
`0 mismatch(es) out of 1068 checked; 1 unchecked` — so a post-condition can never report PASS while
silently having checked nothing. Added a unit test (`label_lookup_folds_case`) locking the fold.

`cargo test` 139 passed / 6 ignored / 0 warnings; corpus gate green. No db.rs, no schema/FTS5, no new
dependency; sectioner untouched beyond this. Wiki: `manual-sections.md` + `manual-sources.md` updated
(944→1068, 125→1, the Sphinx-lowercase fact with before/after).

## [2026-07-31] decision | Manual FTS column = raw body_md (external-content), chosen by the retrieval gate

The 4.2 gate left one column open: 42.7 % of corpus bytes are inside fenced blocks, so should the FTS
index (A) the raw `body_md` or (B) a cleaned projection (strip MyST/LaTeX, keep ` ```orca ` blocks)?
Decided **by number**, not taste. `retrieval_gate` built both over the real corpus and measured 17
pre-registered queries (two are ROADMAP acceptance criteria): **A hit@5 15/17 (88 %), B 16/17 (94 %)**;
hit@1 9/17 for both. B's only edge is GOAT (A ranks the Compound-scripting `goat` commands above the
GOAT page) — a 1-query, within-noise difference.

**Chosen: A (raw `body_md`).** Rationale: (1) the tie-break rule — within noise, take the simpler; (2)
A is what makes the FTS **external-content** (`content='manual_sections'`), so the 4 MB body is not
duplicated; B would need a second stored column. The projection code (`projection.rs`) stays for the
gate but is NOT in the ingest path. Honest caveats recorded: the `imaginary frequency` "miss" actually
returned a relevant section not in the pre-registered targets (goalpost not moved); hit@1 ~53 % leaves
real work for the future exact-keyword layer (`keywords.json` / hover, 4.4+).

## [2026-07-31] session | Unit 4.3: manual_sections schema (v9) + FTS5 index + search + retrieval gate

First tables the manual owns in `orcastudio.db` (ADR-013). Migration **v8→v9** (`SCHEMA_VERSION` 8→9,
`create_manual_tables`): `manual_sections` (synthetic `id` PK — neither `anchor` nor `(file,
title_slug)` is unique, 140 slug collisions; **nullable `anchor`** + `anchor_source`; JSON
`breadcrumb`/`labels`; `(orca_version, file)` index), external-content `manual_fts` over
`title/breadcrumb/body_md` (no body duplication), and `manual_provenance` (base_url, collected_at,
corpus_hash, sectioner_version, counts — the `parser_version` role for a diffable refresh). Migration
test `migrate_v8_to_v9_...` asserts data preservation + a working external-content FTS.

**Three anchor populations (rule #11):** of 1586 sections, **1068** have a verified anchor (closest
label in `objects.inv`, matching file + slug); **517** are unlabelled (uncheckable — `objects.inv`
carries only explicit labels); the rest undetermined. `anchor` is NULL for all unverified — Sphinx
auto-generates unlabelled ids with traversal-state suffixes we cannot recompute, and ~140 collide on
the title slug within a file. A guessed anchor points at a nonexistent fragment and reads as "the
manual moved"; NULL is honest and the link lands on the page. (The lone named gap:
`sec:spectroscopyproperties.nocv.theory`, not in the inventory.)

**Ingest (`manual/index.rs`, `build_manual_index` command, no UI):** sectionise → resolve anchors →
write, **idempotent** (replace the version's rows; re-ingest gave 1586 rows, no dup). Content-preserving
post-conditions run **inside the transaction** (rule #9) so a lossy ingest rolls back: row count ==
sections; **every `body_md` reads back byte-for-byte** (subsumes byte-sum — catches silent truncation);
byte total matches; FTS rows == table rows; NULL-anchor count == section_count − verified. Measured:
1586 sections, 1068 verified, 518 NULL, 4 025 114 body bytes.

**Search (`search_manual` command):** `Vec<ManualHit { id, file, breadcrumb, title, anchor, snippet,
rank }>`, FTS5 `snippet()`, `ORDER BY bm25` ASC (title-weighted 10/5/1). Empty query → empty result
(the `output_search` contract). The MATCH builder `to_fts_match` is the ONE shared with the gate, so the
gate predicts production. End-to-end on the real index: both ROADMAP queries (RIJCOSX, CPCM-for-water)
land their target page in top-5.

**Column choice by the retrieval gate:** raw `body_md` (external-content) — see the `decision` entry
above. Gates (all `#[ignore]`, like `manual_corpus`): `retrieval_gate` (A/B measurement),
`manual_ingest` (real build_index + post-conditions + idempotency).

**Verified:** `cargo test` 147 passed / 8 ignored / 0 warnings; migration test green; gates green;
`tsc`/`vitest` untouched (no TS). No UI, no `keywords.json`, no HTML id scraping (the nullable
anchor + anchor_source keep the schema out of that unit's way). No new dependency. Wiki: manual-index.md
(schema/ingest/search/column choice/3 anchor populations), tauri-core.md (v9), manual-sources.md
(retrieval gate), ROADMAP (indexing [x], search [~]).

Next: 4.4 — the manual panel UI + Monaco hover provider (and separately the `keywords.json` seeder).

## [2026-08-01] decision | hit@1 constrains the hover layer — keywords.json, not FTS (ADR-013 amendment)

Small ingest, no code: move a **consequence** to where it acts. The 4.3 retrieval gate's second number
— **hit@1 = 9/17 (~53 %)** alongside hit@5 = 15/17 (88 %), raw `body_md` — lived only as a side remark
in `manual-sources.md`. It is constructive: the right section is almost always *in* the result set but
about half the time is **not first**.

**Rule made explicit:** tolerable imprecision depends on how many results the surface shows. The search
**panel** shows five candidates → hit@5 88 % works (the chemist recognises the target). The Monaco
**hover** shows **one**, confidently, with no sign of a guess → at hit@1 ~53 % it would show the wrong
section on ~half of hovers. **Therefore the hover provider is NOT fed by FTS**; its source is
`keywords.json` (keyword→section mapped explicitly). FTS stays for the panel. If a keyword is absent
from the map, hover stays silent / says "not in the map" — it does not fall back to FTS and does not
guess (same posture as the nullable anchor in 4.3: UNDETERMINED beats plausible-but-wrong, rule #11).

Placed where it works: `modules/manual-index.md` (hit@1 next to hit@5, both exact fractions; new
"Two surfaces, two precision bars" subsection); **ADR-013** amendment (2026-08-01 — hit@1 is the
measured justification for the two-tier structure ADR-006 assumed without a number; decision text
untouched, same precedent as the two prior amendments); ROADMAP Phase 4 (hover item now names its
dependency on `keywords.json` and the no-FTS-fallback rule in prose; markers unchanged). Not done, by
instruction: no `keywords.json` built, no hover written, no code, no re-measure, ADR-006 untouched.

## [2026-08-01] decision | ADR-014 AI integration boundary + agentic-AI landscape page

Documentation unit, no code. The agentic-AI field made ADR-007's speculative AI ladder concrete: by
mid-2026 several systems drive real QC engines from natural language (verified, URL+date, in the new
`architecture/ai-landscape.md`) — **El Agente drives ORCA 6.0.1** (Aspuru-Guzik, arXiv:2505.02484,
Matter Jul 2025), **ChemGraph** drives ORCA/NWChem/Psi4/xTB via ASE (Argonne, arXiv:2506.06363,
Apache-2.0), **Aitomia** drives Gaussian/ORCA/PySCF/xtb over MLatom in the cloud (Dral, arXiv:2505.08195),
and **Bunsen** (Schrödinger, early access 2026-07-27) orchestrates its validated physics stack on a
GCP+NVIDIA cluster for drug discovery. "AI over ORCA" already exists — so the boundary of the agent's
authority had to be fixed **before** Phase 4's "Explain with Claude" ships, not re-decided ad hoc in
Phase 6.

**ADR-014 (accepted, narrows ADR-007 §"AI integration" — ADR-007 not edited, same precedent as
012→002 / 013→006).** Five decisions: **(1)** AI never inside the numerical pipeline — it reads what
the deterministic tier (ADR-012 readers, ASE kernel, `ir.ts`) emits, never emits a number that lands
in a plot/table/coordinate (rule #11 in its worst form); **(1a)** geometric constants — including
**scan windows / step / point counts** (`%geom Scan … = 1.5, 3.0, 12`) — are **retrieved from curated
data or derived from measured geometry, never recalled**; "typically 1.5 to 3.0 Å" is forbidden. This
admits ADR-007's L2 only in retrieval form. **(2)** Three authority tiers T1 explain (read-only) /
T2 draft (text the author reads before Run) / T3 orchestrate (MCP over the command layer, gated on
Phase 4.5), mapped onto ADR-007's L1–L4 on an orthogonal axis (L = what AI does, T = what it may
touch). **(3)** Methodology is an executable guard, not a system prompt — safe with a mediocre model
(rule #10); named honestly as **not yet implemented** (the three guards land with the Phase 4.5 scan
generator). **(4)** Commands are an API: every new compute-spending command needs a **pollable** path,
not only a Tauri event — named hole: `xtb_optimize` (event-only `xtb:done`); a debt, not fixed here.
**(5)** The model is a rented asset; no in-house orchestration model trained. **What does not change:**
phase order 4 → 4.2 → 4.5 → 5 stays — a competitor release is not grounds to reorder.

Touched (all `.md`, no code): new `architecture/adr-014-*.md`, new `architecture/ai-landscape.md`;
`prior-art.md` (+1 sibling-link line); `ROADMAP.md` Phase 6 (+MCP T3 item, +ADR-014 link on
AI-assisted reaction setup); `modules/tauri-core.md` (pollable-path rule for new commands, decision
(4)); `index.md` (+2 Architecture entries, page count 55→59, last-structural-update).
Next: still Phase 4.4 (manual panel UI + Monaco hover + keywords.json seeder) — this unit did not
change the phase order.

## [2026-08-01] ingest | El Agente line (Quntur / Estructural) — honest delta after verification

Chase-down of `ai-landscape.md` "To verify" item 1, which turned out to be substantive and changed
the Delta. No code; ADR-014 not edited (its decisions stand). Web-verified (URL+date, rule #10):

- **El Agente Quntur** — Pérez-Sánchez … Aspuru-Guzik, [arXiv:2602.04850](https://arxiv.org/abs/2602.04850)
  (v1 2026-02-04). **Instantiated in ORCA; supports the full range of ORCA 6.0 calculations; reasons
  over software documentation and scientific literature** to plan/execute/adapt/analyze.
- **El Agente Estructural** — Choi … Aspuru-Guzik/Bernales, [arXiv:2602.04849](https://arxiv.org/abs/2602.04849)
  (v1 2026-02-04). **Multimodal NL-driven geometry generation/manipulation** with control over
  atom/functional-group replacement, connectivity, and **stereochemistry**, using **VLMs**;
  integrated into Quntur.
- **El Agente Forjador** — [arXiv:2604.14609](https://arxiv.org/abs/2604.14609) (2026-04) — task-driven
  agent generation for quantum *simulation*/dynamics; **not our domain**, checked and set aside.

**Why it changed the page:** the old delta ("OrcaStudio has geometric control, they don't") no longer
holds — Estructural is exactly NL geometry manipulation incl. stereochemistry, and Quntur's
documentation-grounding overlaps our planned Phase-4 manual layer. Rewrote the El Agente record as a
**line** (Q 2025 → Quntur 2026 → Estructural 2026, each with the standard field schema; Estructural a
separate entry) and replaced the Delta with a **divergence of bets, not of features**: Estructural
lets the **model** decide geometry (NL+VLM); OrcaStudio bets on **direct human manipulation** checked
by post-conditions (rule #9) over an **unbroken index space** ([ADR-008]) where the AI **cannot emit a
coordinate** ([ADR-014] (1a)). Framed as exactly the critique ADR-014 already makes — now with an
addressee in the literature — not as a stronger/weaker position. "What this does NOT change" (d)
reworked accordingly (dropped "learning layer"/"geometry editing" as differentiators — both now exist
upstream); "What it confirms" gains the Estructural caveat (the emit-boundary is clean for
energies/gradients but geometry construction is where the field now lets a model generate numbers, an
unsettled bet). "To verify" trimmed to the still-open fields (El Agente line interface/compute/access;
Aitomia source; Bunsen interface/Jaguar). Updated: `ai-landscape.md`, `index.md` (description line;
page count unchanged — edits only). Next: still Phase 4.4.

## [2026-08-01] ingest | Keyword-seed measurement (unit 4.4): stable key, app coverage, precision proxy

Measured the seeding inputs BEFORE generating `keywords.json`, over the real sectioner
(`keyword_seed_measure` gate, `src-tauri/src/manual/tests.rs`, `#[ignore]`). The hazard this unit
guards is a map that points at the wrong section yet looks complete — a confident wrong hover
(ADR-013's hit@1 = 9/17). So the gate measures correctness, not volume.

**A1 — stable key.** `manual_sections.id` is reassigned per ingest, so the curated file cannot key on
it. `(file, breadcrumb, title)` is **NOT unique**: exactly 1 collision — `modelchemistries/mreom` has
two identical `## Perturbative MR-EOM-CCPT` H2 siblings (lines 374, 1572), same parent → same triple.
**Decision:** key = `(file, breadcrumb, title)` + an optional `nth` ordinal only where the triple is
ambiguous; `line_start` NOT written to the file; loader post-condition (rule #9) — every key resolves
to exactly one section, 0 or ≥2 errors naming the key, never pick-first.

**A2 — sources.** 79 "Keyword"-titled sections: 847 distinct ```orca tokens + 747 distinct table
tokens (union 1471). "List of Input Blocks" flat-table = the richest single `%`-block source (64
names, has pal/geom/maxcore). "Simple Keyword Lines" is an INDEX of 25 topic groups, NOT a keyword
list (hypothesis corrected). `:::{flat-table}` is a 4th structured form (36 files) beyond the 4.1
three. Corpus-wide pool: 4247 distinct tokens.

**A3 — app coverage (the number that matters): 42/46.** Keywords the app emits (read from
`input-builder/`, `templates/`, `scene/constraints.ts`) vs the pool. 4 missing, two causes:
`TightSCF`/`VeryTightSCF` exist only in prose (curation, not a corpus gap); `M06-L`/`M06-2X` are
spelled `M06L`/`M062X` in the manual → `aliases[]`, NOT hyphen-normalization (dashes are significant:
def2-SVP, NEB-TS, B3LYP-D4). Seeder lesson: the functional table puts the input token in column 2.

**A4 — precision proxy: 7574/7574 = 100 %** literal occurrence in the home section — the extractor
invents nothing. (Home mapping only; the `{numref}`-target precision is the seeding unit's job.)

Numbers recorded in `wiki/orca/manual-sources.md` ("Keyword-seed measurement (unit 4.4)"). Part A is
docs-only; the measurement gate stays in the tree for the seeding unit (Part B), which will build
`keywords.json` (schema: keyword, type, section key, aliases[], summary, provenance) and the loader
post-condition. No code committed here, no schema/ingest/sectioner/FTS touched, no prose parsed, no
genindex/HTML.

## [2026-08-01] session | Unit 4.4 Part B: seed keywords.json from the manual structured pool

Generated `src/manual/keywords.json` — the keyword→section map the Monaco hover will read (ADR-013:
Rust owns manual text-to-structure, a `#[ignore]` generator emits it; frontend consumes). Seed scope
(user decision): **broad** (whole structured pool: annotated ```orca + pipe/list/flat tables +
keyword-titled sections), **home mappings only** — `{numref}`-target records deferred (A4's 100 %
precision was for home only). New gates in `manual/tests.rs`: `keyword_seed_measure` (A1–A4),
`keyword_seed_ambiguity` (+ 30 % exit), `generate_keywords_json`.

**Stable key** = `(file, breadcrumb, title, nth)` — not the synthetic id (reassigned per ingest). The
triple collides once (mreom's two `## Perturbative MR-EOM-CCPT` H2 siblings), so `nth` disambiguates
only where needed; `line_start` NOT written (diff churn). Loader post-condition (rule #9): every key
resolves to exactly one section, 0/≥2 errors naming the key, never pick-first.

**New measure — ambiguity (mirror of the A1 collision): 14.2 %** (370 of 2606 home-seed tokens map to
≥2 sections: MaxIter 17, %method 16, PrintLevel 15…). Under the 30 % exit bar, so generation
proceeded; ambiguous keywords carry **`targets[]`, not a guessed single section** — hover must not
pick first (disambiguation is the next unit, on a number).

**Coverage post-condition (hard): 46/46** app-emitted keywords resolve or the generator panics naming
misses. Four needed curation: `M06-L`/`M06-2X` via `aliases[]` (the manual writes `M06L`/`M062X`; no
hyphen normalization — dashes matter in def2-SVP/NEB-TS/B3LYP-D4); `TightSCF`/`VeryTightSCF` as curated
prose entries (scf › Convergence Tolerances). Extractor reads the functional table's 2nd column and
strips `{cite}` role backticks; appendix (change log/glossary) excluded (its `## GOAT` change entries
aren't docs).

**Output:** 2608 records, ~1.05 MB (2447 block-option / 121 simple / 40 block; 390 ambiguous),
deterministic sort. Size noted as a fact — the block-option bulk is curation material, not finished
entries. No DB schema/migration (bundled file, map lookup). `cargo test` green (147 passed, 11
ignored). Wiki: new `modules/manual-keywords.md`, `index.md` (60 pages), `ROADMAP` (item → [~]).
Left: hand summaries, the {numref} deferred layer (60 %-blocks), targets[] disambiguation.

## [2026-08-01] decision | Qualify block-options + normalize sections in keywords.json (schema v2)

Schema fix while keywords.json still has no consumer. Both first-cut problems — size and
"ambiguity" — had one cause: a block-option record stored the BARE option name though the owning
block was known at extraction, and every target carried a full breadcrumb copy.

**Normalization.** 3173 target objects → 317 distinct `sections` (10× dup); records reference the
array by int. **1.00 → 0.56 MB** (dedup alone ~0.25 MB; qualification trades part back for
correctness). Added `schema_version: 2` at root (the record shape changed — `targets: [{…}]` →
`[<int>]` — an old reader would take ints for objects; same role as `parser_version` / DB
`SCHEMA_VERSION`).

**Qualification — a block-option is a QUALIFIED NAME, the qualifier is identity, not metadata.**
Key = `(block, option)`. `MaxIter` becomes 11 records (`%scf MaxIter`, `%casscf MaxIter`, …) — the
exact AtomId lesson (position confused for atom identity; here option name confused for option
identity; both surface as a surplus of candidates, not a crash). `block` derived by UNION of two
independent signals with provenance `owner_source`: **text** (single literal `%block` in the home
section — text, not inference; priority) → **structural** (unique `%block` of file / unique deepest
ancestor) → **null** (a value, like anchor_source 'undetermined'). Union coverage 74.7 %, null 25.3 %.

**The load-bearing measure — agreement.** Where both signals resolve (936 targets) they agree
**98.5 %** (14 disagreements). This retroactively VALIDATES the structural 62 % already in the wiki —
the same `objects.inv` × `predict_anchor` construction (two derivations that must agree), held again.
So structural is not demoted. Dropped the "several %-tokens" third rule by number (rescues 145/726 =
4.7 % on an uncheckable heuristic). Cross-reference sections ("List of related keywords" / "See
also", measured 2: nocv, mcd) → owner null BY RULE, not by accidental tie.

**Consumer contract fixed here (not reinvented in the hover unit):** 25.3 % of block-options have
`block: null` — unreachable by qualified lookup. The hover does NOT fall back to unqualified
bare-name search on a qualified miss; unqualified lookup is a separate, deliberate path answering
"documented in N places" (a list), not one section. Same posture as "hover does not fall back to FTS"
(ADR-013 amendment) — unwritten, it gets replayed. `targets[]` reflects reality (`%casscf MaxIter`
truly in CASSCF and DMRG); we do NOT target zero null or zero ambiguity.

Post-conditions (rule #9): 46/46 app coverage or panic; **zero dangling int refs** (asserted);
byte-deterministic re-run. Output: 2836 records, 317 sections, owner_source text 1204 / structural
802 / null 669. New measures in `manual/tests.rs`: `owner_signal_measure`, `owner_union_measure`.
`cargo test` green (147 passed, 13 ignored). Wiki: `manual-keywords.md` rewritten, `manual-sources.md`
Part C before/after, `index.md` line. No DB/FTS/sectioner touched, no {numref} layer, no new deps.

## [2026-08-01] session | Unit 4.4: manual search panel + SectionView (loss-free render)

First REAL consumer of the manual index (ADR-013). Backend: `get_manual_section(id) -> ManualSection`
(full body, not a snippet; missing id = NotFound, never empty) and `manual_index_status() ->
Option<ManualStatus>` (drives a Build-index state, not a mis-readable empty list). UI: `ManualScreen`
(debounced `search_manual`, results as breadcrumb › title + highlighted snippet, click →
`get_manual_section`) hosting a **standalone `SectionView`** — kept separate on purpose so the next
unit's Monaco hover can open a section in a drawer without pulling the author out of the editor (global
drawer NOT built here).

**Render rule (display analogue of line-conservation, rule #9): what is not recognized is shown AS IS,
never dropped.** Section bodies are MyST (prose, LaTeX, `:::{directives}`, tables; 42.7 % of bytes in
fences). A naive markdown renderer eats what it can't parse WITHOUT error. So `render.ts` recognizes
exactly one structure — ` ``` ` fences → monospace, indentation preserved — and emits everything else
verbatim; no MyST rendering, no new dependency (react-markdown/marked would be a dep that doesn't
understand MyST — not taken). **Preservation post-condition (`render.test.ts`):** every non-whitespace
char of `body_md` survives into the rendered text (`reactText` mirrors DOM `textContent`, no jsdom),
asserted char-for-char over hazard samples AND checked over the whole real corpus (126 leaves, 0 loss).

**Snippet markers moved off `[`/`]` → PUA `U+E000`/`U+E001`.** Measured: `[`/`]` occur **1905/1903**
in the 4 MB corpus (every `[link]`/role), PUA pair **0** — so `<mark>` highlighting can't fire on
literal brackets (task caught by number, not by eye).

**Real-window verification (WebKitGTK, the ethos: critical bugs surface in the actual window, not
vitest).** Launched `npm run tauri dev`, screenshotted: (1) app renders, Manual tab present; (2) the
Build-index empty state; (3) after building (1586 sections) a search for RIJCOSX — results with
correct highlights (RIJCOSX marked, literal `[`/`:` in bib text NOT falsely marked), and a section
whose body is a MyST `{bibliography}` directive **rendered verbatim in a monospace fence, delimiters
and indentation intact** — the exact silent-loss case, prevented. Temporary auto-drive + default-screen
patches used to reach the state (no xdotool for input injection) and **reverted** before commit.

Debt named, not fixed: `manual_root()` = `CARGO_MANIFEST_DIR`, so indexing works only from a source
run; bundled-app corpus path is later. Verify: cargo 148, tsc clean, vitest 397 (incl. preservation).
Wiki: frontend.md (Manual panel + SectionView + why), manual-index.md (get/status commands, markers,
manual_root debt), ROADMAP (panel [x]). No schema/FTS/sectioner/generator/keywords.json changes; no
hover, no {numref}, no drawer, no deps.

## [2026-08-01] ingest | ORCA block nesting + word boundaries measured for the hover qualifier (4.4 Part A)

The Monarch tokenizer is stateless (one `root`, no @push/@pop), so the block qualifier the hover needs
isn't in Monaco's state — derived by a PURE function `enclosingBlock(text, line)` (`src/editor/`,
tested without Monaco, 10 trap tests). The hazard: a scanner wrong about the enclosing block makes the
hover show the WRONG section confidently (a qualified lookup finds a record either way). So it returns
**null on any ambiguity**, never a guess.

Measured over the manual's own **1477 ` ```orca ` blocks** (a real validation corpus, rule #10):
- **Opener forms:** 682 multi-line block, 27 single-line block, **46 no-`end` directive (6.1 %)**
  (`%maxcore 3000`, `%moinp "…"`, `%base "…"`). A naive "every `%name` opens" scanner ends with a
  dirty stack on **42/1477 (2.8 %)** — all no-`end` directives. So the scanner opens a block ONLY when
  a matching `end` follows (forward scan); built that way it never runs away (**0/1477** open stacks),
  and truncated fragments are treated as directives too (conservative).
- **Nesting is real:** `%geom … Constraints … end end` — bare-word sub-blocks (no `%`), the case
  `scene/constraints.ts::locateGeom` already handles. The scanner pops the enclosing `%block` early on
  a sub-block `end` → conservative null after the sub-block, correct `%geom` inside it.
- **Word boundaries:** Monaco's default `wordPattern` (reconstructed from node_modules, not memory)
  splits `def2-SVP`→[def2,SVP], `NEB-TS`, `M06-2X`, `%maxcore`→[maxcore], `def2/J`→[def2,J]; only
  `RIJCOSX` survives. `def2` handed instead of `def2-SVP` is a miss indistinguishable from "not in the
  map" — the hover unit MUST set a `wordPattern` keeping `-_./%` in words (exact regex in the wiki).
- **Coverage on OUR text (Phase-1 templates):** all simple `!` keywords resolve; `%pal`/`%geom` ✓,
  but **`%maxcore` is ABSENT** from `keywords.json` (a no-`end` directive from the `{numref}`-deferred
  layer) → hover stays silent (correct per contract), a named curation hole.
- **Render piggyback:** **110/1558 sections (7.1 %)** carry a pipe-table outside a ` ``` ` fence (26
  Keyword-titled) → they render as misaligned proportional prose in `SectionView`. Substantial, so
  Part B routes a `^\s*\|` line into the same monospace `<pre>` as a fence (same linear check, no
  parser, preservation test unchanged).

Facts in new `wiki/orca/input-syntax.md`. Verify: vitest green (enclosing-block 10 traps), tsc clean,
cargo untouched. Part B (hover provider + drawer + wordPattern + pipe-table `<pre>`) NOT started.
No keywords.json/schema/sectioner/Monarch-state changes, no deps.

## [2026-08-01] session | Unit 4.4 Part B: manual hover provider (qualified lookup, silence on miss) + section drawer

The editor's consumer of keywords.json — and the first real test of its whole design (a block-option
is a qualified name). Six pieces:

**Coverage gate rewritten in CONSUMER form (the flaw the unit opened on).** The old gate matched by
bare STRING (`norm_kw`, drops `%`), so `%maxcore` counted covered because `maxcore` matched a `MAXCORE`
block-option in %xtb/%cis/%mdci — string, not entity; "46/46" was partly empty. Now type-aware
(`!`→simple, `%`→block, `opt`→block-option): **44/46 resolve**, two named gaps the old gate hid —
`%maxcore` ({numref} layer) and `CPCM` (emitted simple `CPCM(solvent)`, only `%cpcm` seeded). Both stay
silent (correct); curation targets, file untouched. A second **TS consumer gate** (coverage.test.ts)
tokenises the templates with the real wordPattern and asserts type+block-aware resolution (MaxIter→%scf,
not the other 15).

**Bridge keywords.json→DB** (`resolve_descriptor`): descriptor `(file,breadcrumb,title,nth)` → row.
Post-condition promised in 4.4, now checkable and **verified: 317/317 descriptors → exactly one row,
injective** (gate `keywords_bridge`). Version check: keywords.json.orca_version must equal the built
index (stale map reported, not resolved).

**wordPattern** (task 3): the language config keeps `-_./%` in words so `def2-SVP`/`%maxcore`/`def2/J`
come whole (Monaco default split them). 6-token test.

**Hover provider** (`orca-hover.ts`): 3 cases via `hoverContext` (+`enclosingBlock`, +same-line
`%pal nprocs`), type/block-aware lookup, `aliases[]`. **Contract enforced: a miss → no hover at all
(silence)**, never a bare-name or FTS fall-back. Body: keyword/type/owning-block(+owner_source), Open
command-link; several targets → "documented in N places", not a picked first. Empty summary doesn't
suppress.

**Drawer** (`ManualDrawer.tsx`): a fixed side overlay opened by the Open command; resolves via
`resolve_manual_section` and renders the SAME `SectionView` as ManualScreen — author stays in the
editor. **Pipe-tables** (task 6): `render.ts` groups `^\s*\|` runs into a `<pre>` (110 sections, 7.1 %,
were misaligned prose); font choice only, preservation test unchanged and green.

**Window verification (WebKitGTK).** App runs, editor + hover registered, no crash. Drawer verified
end-to-end in the real window: a descriptor resolved through the bridge and rendered RIJCOSX in the
side drawer with the editor still visible (loss-free render: `{cite}` roles and `[…](sec:…)` shown
as-is). Honest limit: the hover POPUP itself needs a real mouseover and this environment has no input
injection (no xdotool), so the hover's LOGIC (resolveHover/buildHoverMarkdown: RIJCOSX simple/multi,
MaxIter→%scf, %maxcore→silence, def2-SVP whole) is covered by unit tests, and its OUTPUT (the drawer)
by the window. Temp auto-open patch reverted before commit.

Verify: cargo 148 (+ ignored gates keywords_bridge/generate green), tsc clean, vitest 420. No
{numref} layer, no Monarch state, no schema/sectioner change, file unchanged (only the gate), no deps.
Left: `%maxcore`/`CPCM`/`Constraints`-in-%geom curation; the {numref} layer.

## [2026-08-01] ingest | coverage inventory made explicit (builder+domain+workflow); gaps classified by closer

The coverage gate improved twice by the number falling (46/46 → 44/46) as the FORM of the question got
honest (type, not string). Third, deepest flaw: the POPULATION. The 46 were what input-builder emits;
the hover fires on what the author TYPES — domain guards (`! XTB GOAT`, `TightOpt` before `Freq`) and
the reaction chain (NEB-TS → OptTS → IRC). Different sets.

**One home.** The expectation set is now `src/manual/keyword-inventory.json`, read by BOTH gates (Rust
`generate_keywords_json` + TS `coverage.test.ts`) — the two implicit lists collapsed to one. Each entry
names its **source** (builder / template / domain=ADR-014 / workflow=ADR-007), so it is reviewable
word-by-word; populated from named sources, NOT memory — `MORead`/`PrintBasis` deliberately absent (no
named source: named, not longer). Type-inference hint for seeding kept SEPARATE (input-builder `!`
tokens) so the inventory can't reclassify the seed — keywords.json byte-identical, unchanged.

**Honest number: 45 of 53 resolve** (type+block-aware; Rust and TS agree). Hard post-condition = the
45 non-gap words; a `gap` word is a declared, classified hole — reported, never a panic. **8 gaps by
closer: (a) {numref} — 1** (`%maxcore`); **(b) curated prose — 3** (`IRC`, `ScanTS`, `NEB-CI`, run-types
in prose only, 0 backtick); **(c) second/right form — 4** (`CPCM`/`XTB`/`TightOpt` need the simple form
of a concept that exists as `%block`/block-option; `Constraints` seeded under `%method` but emitted
under `%geom`); **(d) not in corpus — 0**.

**{numref} priced for the first time: it closes 1 of 8 named gaps** (`%maxcore`). The other 7 are
curation (3) + second-form (4). So the block index is NOT the high-value next step for the words the
project is built around (the reaction chain, the domain guards) — that number is the basis for
sequencing {numref} later, not next. Domain weight recorded: `! XTB GOAT` is mandatory before any
pathway (GOAT resolves, XTB does not); NEB-TS/OptTS exist, IRC absent — the map fails exactly on the
thesis chain.

Gate design: HARD post-condition (must resolve) vs INVENTORY of expectation (want to cover) are
separated — a red/incomplete coverage number is a REPORT with the gap list, not a panic. Verify: cargo
148 (+ ignored gates green), tsc clean, vitest 420, keywords.json unchanged. Numbers in
`orca/manual-sources.md` Part D. NOT done (by instruction): no keywords.json edits, no {numref} layer,
no hover/drawer/schema/sectioner/generator-seed change, no deps. Next units fill the map by category.

## [2026-08-01] ingest | the structural owner is wrong OFF the intersection — scale is hundreds, not units

Measurement only (no fix), before any (c) curation. The 98.5 % owner-agreement was measured on the
INTERSECTION (936 targets, both signals resolve). It never covered the 855 targets where the structural
proxy resolved ALONE — and that is exactly where the error sits. TightOpt (%method) and PrintBasis
(%output) are not "missing simple forms": they are simple `!` keywords MISqualified as block-options,
the structural proxy inferring a `%block` owner the section text never named (a `%block` higher up the
breadcrumb).

Measured over the 802 structural block-option records (814 targets): **537 targets (66.8 %) name an
owner absent from the section body** (529 records); **515 (64 %) sit in a body with no `%`-token at
all** (the `!`-line-in-```orca signal is weak, 4.5 % — absence of the owner is the discriminator). By
title (rule derived from real corpus headings): **475 targets / 473 records** in sections whose heading
is about `!`-line keywords — 384 `… Basis Sets` (basis names ARE simple keywords), 54 `… Optimization
Keywords`, 21 `Convergence Tolerances`, 16 `Simple Input Keywords`. Manual 10: **7 misqualified** (5
basis names → %basis; 2 geom keywords → %method not %geom, the Constraints class); 3 genuine (%mm,
%md/Minimize).

Two failure modes: (i) simple keywords qualified as block-options; (ii) right-kind-wrong-block. Scale
~500 records — hundreds, so NOT a curate-a-few fix. Verdict: owner derivation needs a THIRD signal (a
section-title / body-text veto — reject a structural owner the body never names), a generator change
with its own gate = a separate unit. Nothing changed here.

Wiki: numbers in `orca/manual-sources.md` Part E; the 98.5 % statement corrected in manual-keywords.md
and manual-sources.md to say it holds ONLY on the intersection, not as a validation of the structural
proxy outside it. No code, no keywords.json, no generator/owner-rule change (that is the next unit).

## [2026-08-01] ingest | ROOT of the mis-typing found (else→block-option dumpster); merging won't fix it (1a=0)

Measurement only, before Part B. Confirmed the ROOT in the generator's `type_of`: `simple` is granted
only to OUR builder words or title-home matches; the `else` branch defaults everything unknown to
`block-option` with NO manual signal — it collects the unknown rather than classifying it. The
structural proxy then owns each such record from a breadcrumb ancestor. So Part E's two symptoms are
ONE defect: **the TYPE was inferred from our application, not from the manual** — the third instance of
this pattern (%maxcore measured in our notation; 46/46 held because the inventory was ours; here the
type comes from our app).

Overlap measure (`structural_overlap_measure`, real sectioner + exact descriptor match). Of the **522**
structural block-option targets whose owner is absent from the section body: **1a already type=simple
elsewhere = 0** (merge impossible — the dumpster made no simple records); **1b already a confirmed
block-option elsewhere = 14**; **1c true orphans = 508 (500 distinct words)**. Sum 522. Corpus-wide `!`
signal weak too: only **44/500 (8.8 %)** orphan words appear on a `!` line anywhere in a ```orca block.
Orphans are overwhelmingly basis-set tables (Jensen 56, Correlation-consistent 53, AuxC 47, …).

Consequence, by number not guess: merging is out (1a=0). An owner **veto** alone leaves a
`block-option`/`block:null`, still the WRONG TYPE for a basis name (a `!`-hover wants `simple` and
misses silently). With no simple record to merge and a weak `!` signal, the words can't be confidently
retyped simple either — so the shape is a third type value **`scope:"undetermined"`** (a value like
anchor=NULL, not a false second type) for the ~508. Part B builds veto (owner) + undetermined (type),
two independent signals. Nothing changed here.

Wiki: numbers + root in `orca/manual-sources.md` Part F. No code/keywords.json/generator/lookup/hover
change; keywords.json byte-identical; plain cargo test + vitest green; the measure is an `#[ignore]`
gate. Part B (the fix) not started.

## [2026-08-01] session | fix: keyword type from the manual, not from what our builder emits

Root fixed, not the symptom. `type_of`'s `else` branch was a dumpster — a token neither `%`-prefixed
nor an OUR-builder/title word defaulted to `block-option`, then the structural proxy owned it from a
breadcrumb ancestor. Two independent signals, one commit:

**B1 owner veto:** a structural owner is accepted only when the section body NAMES it (correlate two
sources, like objects.inv × predict_anchor). The 522 unconfirmed structural targets lose their owner;
291 remain (confirmed), 1224 text. Block-option/null-owner records → 0.

**B2 type from the manual, with correction:** `type_of` no longer reads `app_simple`. Seed: `%`→block,
title-is-keyword→simple, else **`undetermined`** (block-option requires a positive, veto-confirmed
owner). 1183 `undetermined` records where the dumpster forced block-option/null. A basis table is now
homogeneous in the seed (`def2-QZVPP` and `ma-def2-SVP` both undetermined — the builder accident
removed). The `app_simple` KNOWLEDGE is not deleted but moved to `provenance:"curated"`: of 42 builder
simples, 5 stay seeded (title), 22 flipped from undetermined, 11 added beside a block-option, 2 alias;
0 hard words lost. The third type value lives in `type` (one field per decision), not a separate
`scope` — two fields would re-split one hover branch into two sources of truth.

**B3 hover contract (one line):** an `undetermined` record answers neither a qualified block lookup nor
a simple one — invisible to the hover (silence), reachable only by the panel's unqualified path.

**B4 gate:** hard post-condition (non-gap words) holds; **46 of 53 resolve, split 9 SEED / 37 CURATION**
so the channels stay visible (up from 45; CPCM moved gap→curated-covered). File 576→515 KB;
regeneration byte-identical; descriptors 317/317; 0 dangling. `cargo test` + `tsc` + `vitest` green.

**Pattern named (wiki):** a check that measures US instead of the subject — %maxcore in our notation,
46/46 from our builder's output, `app_simple` inside `type_of`. Three cases, ONE pattern: channel-
mixing, not the presence of a second channel. The rule is not "our knowledge is forbidden" but "it must
not masquerade as a domain measurement — it has its own attributed channel." Named to be caught a
fourth time before it ships.

## [2026-08-01] ingest | demand on the ! line, from the author's own inputs (A vs B∪C)

Before curating the 7 gaps: measure DEMAND, because the inventory of 46 is our builder's vocabulary —
the pattern's third face. `demand_measure` reads `!`-line tokens from three groups, never merged: (A)
the author's real jobs in the user DB (`~/.local/share/orcastudio/orcastudio.db`, outside the repo);
(B) test fixtures; (C) Phase-1 templates. Tokenised by the REAL `wordPattern` from orca-language.ts.

Sizes: **(A) 16 jobs, 15 distinct** `!`-tokens; (B) 6; (C) 9; **(B∪C) 11**. `A \ (B∪C) =
{cpcm, def2-tzvp, dmso, ethanol}` — and `dmso`/`ethanol` are solvent VALUES (`CPCM(water)` →
`[CPCM, water]`), `cpcm`/`def2-tzvp` builder words absent from our fixtures. So **A barely diverges
from B∪C: the demand IS the builder's vocabulary** — the author generates inputs WITH the input-builder,
so the "measure of 46" was already, by accident, a measure of demand. No independent signal (hand-typed
%-block options) exists yet — the pattern seen once more, from the demand side.

**(A) `!`-coverage: 12/15 resolve** (2 seed, 10 curation). 3 misses: `dmso`/`ethanol` (solvent values,
correctly silent) and **`XTB`** — a real gap `c` HIT BY REAL DEMAND (`! XTB GOAT`, the ibuprofen run +
a domain guard; GOAT resolves, XTB does not). Echoes 46/53 (same builder set).

**Reverse — seed value: 1 of 1363 block-option keywords touched by demand** (`nprocs`). The 1515
block-option records (~53 % of the map), the part the manual structures and the seed captured well,
serve essentially NO current demand — the author types `!` lines, not `%`-block bodies. Where seed
effort paid off (structured capture) is not where demand is (the `!` line, in flat-tables/prose the
seed can't read).

Conclusion: next curation is not "the 7 gaps" broadly but **XTB** (the one real-demand miss) + a
**solvent** class; the block-option half needs none for demand; re-measure once %-blocks are hand-edited.
Also reconciled the B2 arithmetic: 5+22+11+2=40; the missing 2 of 42 are `TightSCF`/`VeryTightSCF`,
covered via PROSE_CURATED (a fourth path, not a discrepancy). Verify: `demand_measure` #[ignore] gate;
plain cargo test 148/17-ignored, vitest green; keywords.json byte-identical. Numbers in
`orca/manual-sources.md` Parts G–H. Nothing changed: no keywords.json/generator/curation/hover edits.

## [2026-08-01] session | argument rule + curated ! keyword (XTB), measured against the manual's own ! vocabulary

Task 0 first — the DENOMINATOR: the manual's own `!` vocabulary from its ```orca blocks (the one source
that is NOT us). **424 distinct `!`-tokens, 417 lines; the map resolves 42 (10 %)** (13 seed, 29
curation); 293 absent, 89 undetermined-only. Our demand (A, 15 tokens) covers 3.1 %. Top absent are real
methods/bases (MOReAd 27, DLPNO-CCSD 20, NEB 15, NoIter 13, ALPB 10, RHF 10, `/C` aux bases). **Verdict:
the manual's `!` vocabulary is ~28× wider than our 15; the curated layer is OPEN — a plan, not seven
entries. Phase 4's keyword coverage is not "done"** (hover functionally works — RIJCOSX explains,
`keywords.json`[~] not [x]).

**Argument rule (task 1):** a token inside `(…)` after a keyword is an ARGUMENT — `keyword-lookup.ts::
isArgumentToken`, a positive pure line-check in `hoverContext`. Forms measured in the corpus (general,
not a whitelist): CPCM/SMD/ALPB/DDCOSMO/CPCMX solvents, SV(P) basis, DLPNO-CCSD(T) triples, PAL8(n).
Regression proof: `water` IS in the map (%frag), yet a hover on `CPCM(water)`'s `water` is silent —
silence from the RULE, not absence. The 4.9 "silence for dmso/ethanol" was a hole in the map, already
half-broken (water seeded under %frag), now closed by the rule.

**XTB curated (task 2):** the one gap real demand hit (`! XTB GOAT`). Added a `simple`/`curated` record
→ `semiempirical › Extended Tight-Binding: GFN0-xTB, …` (found, not guessed); its `%xtb` block-option
untouched (both forms legit). Section ordering changed so a curated-only section APPENDS after the
stable seed sections → keywords.json diff = **one record + one appended section**, fully reviewable
(not a 1400-line renumber cascade); regeneration byte-identical; descriptors 318/318.

**Task 3:** the other 6 gaps (IRC/ScanTS/NEB-CI b, TightOpt/Constraints c, %maxcore a) stay DECLARED,
NOT curated one-by-one — the 10 % denominator says the layer needs a plan and none of the six is
demand-hit. Inventory 47/53 (9 seed, 38 curation; CPCM+XTB moved gap→curated).

**Pattern, 4th case (task 4):** the demand corpus looked independent but was us (A ≈ B∪C, the author
builds with the input-builder) — caught by measuring against the manual's own `!` vocabulary. Also
recorded plainly: the 1515 block-option records (~53 % of the map) serve 1 real request of 1363; durably
valuable regardless are qualified addressing, the owner veto, consumer-form gates, the argument rule,
and the pattern discipline itself. Verify: cargo 148/18-ignored (+bridge/generate green), tsc, vitest
423 green; keywords.json diff reviewable + byte-identical re-gen. Numbers: `manual-sources.md` Part I.

## [2026-08-01] session | fix: hover clipped on the top `!` line (fixedOverflowWidgets) + wiring test

**Symptom (architect, real window):** hovering `! r2SCAN-3c Opt Freq TightSCF` shows no hover;
highlighting, drawer, and all pure-hover unit tests are green. The presumed cause (registration
order / language id) was handed over — but the brief was measure, don't guess.

**Measured, not guessed.** No mouse/console access in WebKitGTK, so facts were written into visible
DOM banners from a temporary `onMount`. Four probes, each killing a candidate: model language =
`orca-inp` (matches); `getWordAtPosition` on the `!` line = `r2SCAN-3c` (full token); `resolveHover`
on the live model = MATCH, `buildHoverMarkdown` = 673 chars no throw; a side-effect inside the
**registered** `provideHover` fired (provider IS invoked). Yet `.monaco-hover` sat at **y≈255, above
the editor's top edge** → clipped by the editor's `overflow:hidden` guard. The presumed cause was
wrong on both counts; the real defect was widget rendering.

**Fix.** `fixedOverflowWidgets: true` (`src/editor/editor-options.ts`, split out as a type-only
module so a test can pin it) makes overflow widgets render body-level and escape the clip — real
manual popup verified un-clipped in-window. Plus registration hardening in `registerOrcaHover`:
hover provider **first** (mandatory), open-in-drawer command **second** in `try/catch` (optional,
must not vanish the hover), `registered=true` only after the mandatory registration succeeds.

**Test that would have caught it:** `orca-hover-wiring.test.ts` (fake monaco, no jsdom) — provider
registered for the same language id `<Editor>` uses, survives the command throwing, and
`fixedOverflowWidgets` pinned. **Lesson:** the pure functions were unit-tested; the wiring — order,
options, rendering — was not, and that is exactly where it broke. Page: `debugging/010`.
Verify: tsc clean, vitest 426 green (30 files). No Rust/sidecar touched.

## [2026-08-01] session | Manual: full-page rendering (a section indexes, a page shows) + manual_root() debt closed

**Why.** Author feedback after real use: sections are too atomized — a search result does not show
*why* a keyword sits where it does. Not a sectioner defect; the section serves two tasks with opposite
optima (search wants fine granularity for bm25, reading wants context). Median body 1330 B and 27
empty navigational sections were the symptoms. Split the surfaces: **search stays section-grained; the
result opens the whole page** and scrolls to the found section. The sectioner still supplies search
granularity — it just stops being the display screen.

**Task 1 — `manual_root()` debt closed.** Was `CARGO_MANIFEST_DIR/../resources/manual` (source-only),
which now also blocks page reads. Resolved honestly: **source/dev run** → the repo tree (the
compile-time path, absent on a bundled app elsewhere = the discriminator); **bundled run** →
`<data_dir>/orcastudio/manual` (same `dirs::data_dir()` base as the DB). **Not** an app-resource dir —
the manual is never bundled/redistributed (domain rule #7). Neither resolves → an explicit error
**naming where it looked**.

**Task 2 — `get_manual_page(file)`** → `ManualPage { file, orca_version, text, sections:[{id, level,
title, anchor, line_start, line_end}] }`. Source is the **file on disk**, NOT the stored sections: the
preamble is coverage-checked but never stored, and heading lines are not byte-reproducible from
`title`+`level`. **Post-condition (rule #9):** file line count == `max(line_end)+1`, and each
section's `line_start` line has exactly `level` `#` and contains its `title`; a mismatch is an explicit
"page on disk does not match the index; rebuild" error, never a silent wrong page. Chose **no per-file
hash / no migration** — the two cheap checks cover the realistic drift (version refresh, partial
reload); `corpus_hash` attests the *indexed* state (computed from sections), so it cannot be the
freshness check. `verify_page_matches_index` unit-tested three ways + a temp-corpus round-trip.

**Task 3 — one display component.** `SectionView` folded into **`PageView`** (`src/manual/PageView.tsx`),
shared by `ManualScreen` and `ManualDrawer` — no second render path (the pattern had collapsed four
times). It splits the file into line-owned segments (preamble + each section `[line_start,line_end]`),
so a section's DOM node spans exactly its bounds → highlight = `.target` on that node; scrolls the
target into view. In-page ToC (`<details>`, headings) for big pages. Body rendering still through the
unchanged `renderManualBody`→`parseManualBody`; preservation test stays green (moved import to
`PageView`).

**Task 4 — measured.** `page_size_measure` (ignored): PAGE bytes median **18 773** / p95 **119 545** /
max **214 493** (≈209 KB) vs SECTION bytes median **1330** / p95 9074 / max 48 245 — the display unit
grew ~14× at the median. **The max page is ~209 KB, not the 48 KB assumed** (that was the max
*section*). Sections/page median 7, max 162; **4 pages >50 sections** (ToC mandatory). `parseManualBody`
on the 209 KB page ≈ **0.48 ms** (V8) — render-prep is sub-ms.

**Honesty note (rule #10).** The interactive WebKitGTK check (scroll landed on the section, highlight
matches the bounds, the 209 KB page renders un-clipped) I could **not** perform: this environment has
no input-injection tool (`xdotool` missing) and the browser tooling speaks CDP to Chrome, not
WebKit's protocol, so I cannot drive or observe the Tauri webview. Not claiming a check I didn't run —
it is the author's to do (Manual tab → search e.g. `CASSCF` → click a result → confirm scroll +
highlight + no clipping). Everything else is verified: cargo 152 pass, tsc clean, vitest 426 green,
preservation test still char-for-char.

**Not touched (next units):** the hover provider (its replacement by selection is the next unit),
Explain-with-Claude, the sectioner, `keywords.json`, FTS, the search schema. No migration, no new deps.

## [2026-08-04] ingest | measure MyST constructs in the manual corpus (the render plan by number)

**Gate (unit 4.11, before touching the renderer).** `cargo test myst_constructs_measure --
--ignored --nocapture` (`src-tauri/src/manual/tests.rs`) walks all 126 leaves (3 984 406 chars)
with the sectioner's own fence model and censuses every render construct by **occurrences AND
characters**. Numbers recorded in `wiki/orca/manual-sources.md` ("MyST construct census").

**Findings.** Math is dollar-only: `$$…$$` 874/4.27 %, `$…$` 4746/1.64 %; `\(…\)`, `{math}` role
and `{math}` directive are **0** — KaTeX needs two delimiters, zero tail. Inline code `` `…` `` is
the biggest transform, **11.77 %** (9280 occ), and role-args are stripped from that count.
Cross-refs come in two nav syntaxes (`{ref}`/`{numref}` roles + `[..](sec:/tab:)` links, ≈1710),
with a citation/equation/formatting tail that stays verbatim. Of 32 directive names, **13.6 % of
corpus is under a directive fence but almost all of it is VISIBLE content** (tables, admonitions,
figures); the genuinely-invisible metadata is only `{index}` (321 occ, 0.31 %), `{tabularcolumns}`
(76) and `{raw} latex` (176) — ≈0.3–0.6 %.

**The reading.** Two–three constructs carry each category; the rest is a tail. Load-bearing
correction to the screenshot model: the noise is NOT the whole `{directive}` class — so category (3)
must be a **named** whitelist (proposed: `{index}` + `{tabularcolumns}`; `{raw} latex` deferred to
the author), never "looks like a directive → hide", which would eat 13 % of visible content.

**Next (Задачі 2–4, on "ок"):** KaTeX (first visual dep, bundle size before/after), inline code →
`<code>`, cross-refs → anchor-map links, the three categories split in the preservation test, and a
before/after render-time measurement on the largest page.

## [2026-08-04] decision | manual render — three categories, KaTeX (first visual dep), a THREE-name hide-whitelist

The 4.11 census (previous entry) refuted the screenshot model, so the render is built on the
reformulated invariant: **every source char is in EXACTLY ONE of three categories** — (1) recognized &
transformed (math→KaTeX, `` `code` ``→`<code>`, resolvable cross-refs→`<a>`), (2) unrecognized→verbatim
(the preservation test, unweakened, split off so it stays a pure char check), (3) recognized &
deliberately hidden — a **named** whitelist. Decisions recorded:

- **KaTeX, not MathJax** — the project's FIRST visual dependency (precedent). Reasons: fully offline
  (fonts bundled, 0 network `url()` in built CSS — verified), synchronous, covers Sphinx's LaTeX subset.
  `throwOnError:false` so an unknown macro shows its source (category 2), never breaks the page.
  Corpus math is dollar-only (`\(…\)` and `{math}` are 0), so exactly two delimiters are supported.
- **The hide-whitelist is EXACTLY three, by name:** `{index}`, `{tabularcolumns}`, and **`({raw},
  latex)`** — the last keyed on the PAIR, not the name, because "arg always latex" is measured on THIS
  corpus, not a MyST property (a 6.2 refresh could add `{raw} html`, which stays visible — pinned by a
  test). Name compare is one case-insensitive function from day one (admonitions arrive as
  `{Note}`/`{note}`/`{NOTE}`).
- **Cross-refs resolve via the EXISTING anchor map** (`predict_anchor` reused, no second normalization),
  through a new read-only `resolve_manual_anchors`; an unresolved target stays verbatim, never a dead
  click — the same posture as a NULL anchor and hover silence.
- **`{literalinclude}` is its own class** — a visible absence marker, because the external `.inp` was
  never fetched; verbatim would show a path where the manual gave an input example.

## [2026-08-04] session | feat: render math (KaTeX), inline code and cross-references; hide three named metadata directives

**Done.** `src/manual/render.ts` rewritten: `parseManualBody` now recognizes directive fences (```` ``` ````
and `:::`) as `directive` blocks; `tokenizeInline` splits prose into code/math/xref/text; pure
`isHiddenDirective`/`isMissingInclude`/`xrefLabels`. `PageView` renders category 1 (KaTeX via
`renderToString`+`dangerouslySetInnerHTML`, `<code>`, resolved `<a>`), hides the three named
directives, flags `{literalinclude}`, and batch-resolves cross-refs on page load; `onNavigate` threaded
through ManualScreen + ManualDrawer (same-page scroll internal, cross-page loads the file). Rust: new
read-only `resolve_manual_anchors` command + `index::resolve_anchors` (reuses `predict_anchor`). KaTeX
CSS imported once in `main.tsx`.

**Measured.** Bundle before/after (`npm run build`): main JS +303 KB raw / +82 KB gz, CSS +30 KB,
`dist/` +1.41 MB (59 bundled fonts), 0 CDN refs. Cross-ref resolution 1364/1722 (79.2 %;
`xref_resolution_measure`). KaTeX render (Node V8, honest proxy): worst page mdci 364 formulas 21.5 ms,
CASSCF 153 formulas 6.6 ms. **NOT measured:** WebKitGTK DOM-insert + paint in a real window — left to
the author's interactive check before deciding lazy math / collapsed sections; the sync render is under
a frame at the median, ~21 ms worst, so unlikely to be the bottleneck (a prediction, not a paint time).

**Verification.** `render.test.ts` split three ways (category 2 preservation unweakened; a transform
test per construct; whitelist-hides + `{raw} html`-visible + `{note}`-not-hidden + `{literalinclude}`
marker). tsc, vitest (444), cargo (manual unit tests) all green.

**Not touched (next units):** the editor hover (its replacement by selection is next), the sectioner,
`keywords.json`, the schema, FTS. Only KaTeX added; "unrecognized → verbatim" stays the base — this unit
adds named exceptions only. Deferred by number: images (`{figure}`/`{subfigure}` 175×) and the
`{numref}`-to-non-section tail; lazy math pending the author's paint measurement.

## [2026-08-04] session | test: restore the CORPUS preservation check for the render path (4.12)

**Why.** 4.11 split the preservation test three ways but left category 2 on **8 synthetic
samples only** — no test read `resources/manual/` at all, dropping render coverage from ~4 M real
chars to eight fixtures, silently (all three descriptions look correct). Archaeology: a TS *render*
corpus check never actually existed (no `readFileSync`/`readdirSync` in any `.test.ts`, ever); the
"0 loss on 126 leaves" wording (from 4.4, cd4e824) conflated it with the Rust **storage** byte-for-byte
post-condition (`index.rs`). So this is the first real corpus-level check of the render path.

**Done.** `src/manual/render.corpus.test.ts` reads all 126 leaves and checks, per file, the
**sum over categories** (not "rendered == source", which a transform breaks): **(S)** parser+tokenizer
partition every source char into `cat1 ⊎ cat2 ⊎ cat3 ⊎ cat5` (== source, char multiset); **(R)** the
actual React render emits exactly the declared-visible chars (`cat2` + code contents + resolved-xref
texts + `{literalinclude}` markers; math/hidden → nothing). Every xref resolved by a stub → the
strictest, DB-free path. Failure names the file + char diff. Corpus gitignored → SKIP with an explicit
message on a machine without it (`ORCA_MANUAL_ROOT` overrides). VERBATIM samples kept as fast units.

**Verified.** Passes over all 126 leaves (1.1 s). Non-vacuous: a negative control (a render that drops
inline-code content) unbalances (R) — the gate bites. Corrected the stale frontend.md wording.

## [2026-08-04] lint | Post-Phase-4 wiki lint — number-provenance pass; two measurement anti-patterns named

First full lint since Post-Phase-3 (14 Phase-4 units). **New dimension: FALSE facts, not just stale** —
triggered by "render preservation: 0 loss on 126 leaves", which lived in frontend.md four units, read as
a measured fact, though no render corpus test ever existed (it conflated with the Rust `body_md` storage
check). A contradiction-only lint cannot catch a number that was never true. **Wiki + CLAUDE.md only —
no `.ts/.rs/.py` touched; cargo (152) / tsc / vitest (445) green.**

**Reviewed ~70 numeric/`measured`/`verified`/`PASS` claims (Phase-4 pages exhaustively, older spot-checked).
Found: 3 without committed provenance (b), 6 contradicting what the gate measures (c), + count/wording.**

- **(c1) `manual-keywords.md` contradicted itself** — first half pre–Part-G (schema example with
  `owner_source:null` block-options; owner section calling the veto "a separate unit"; "25.3 % block:null";
  size `0.56 MB / 2836 / 317`), later half post–Part-G (47/53, `undetermined`, "the owner veto"). Exactly
  the layered-module-page drift CLAUDE.md forbids. **Rewritten to the present rule** against the actual
  `keywords.json`: type ∈ {simple, block, block-option, **undetermined** (1183)}; owner veto is the rule,
  **0** null-owner block-options; `owner_source` text 1224 / structural 291; 2857 records / 318 sections /
  ~515 KB; 318 descriptors.
- **(c2/c3) `index.md`** manual-keywords description + footer were pre–Part-G (46/46, 0.56 MB, 98.5 % un-
  scoped, no `undetermined`). Rewritten to 47/53 / 515 KB / owner-veto / type-from-manual.
- **(c4/c5)** stale `317/2836/0.56 MB` → `318/2857/515 KB`; dedup 3173→317 (+1 curated = 318).
- **(c6) `SectionView` as a live display path** in `input-syntax.md` and `manual-keywords.md` → `PageView`
  (the one component; `SectionView` is gone). The input-syntax pipe-table paragraph also described a DONE
  fix as future — corrected.
- **(b) no committed provenance, labelled honestly (not deleted):** KaTeX render `21.5/6.6 ms` = an **ad-hoc
  Node proxy** (V8 string-gen), NOT a gate and NOT the perf answer (WebKitGTK paint, author's manual check);
  KaTeX bundle deltas = "before" recoverable by rebuilding at `d9a6492^` (provenance laborious, not absent).
- **Health:** page count 61 → **60** (my own report's 59 was itself a false count — `grep -vE index.md`
  also excluded `manual-index.md`; noted in-session); 0 orphans, 0 dead links, log tail = 5, language ok.

**Two anti-patterns named (own `## Part F` collection, cross-linked):** *Pattern 1 — a check that measures
US not the subject* (%maxcore / 46-46 / app_simple-in-`type_of` / demand-A≈B∪C), the fourth (Part H) now
collected with the first three. *Pattern 2 — the adjacent-measurement trap* (98.5 % intersection-only /
"0 loss" storage-not-render / KaTeX V8-not-paint): a real number cited for the adjacent question; cure is
scope, not provenance — a proxy gate only launders it. **`CLAUDE.md` "Coding conventions"** gained the
negative-control rule: a gate whose ability to fail is undemonstrated is green for an unknown reason
(`d9a6492` precedent).

**Consolidated future unit:** the absent-content gap — `{literalinclude}` 255 + `{figure}`/`{subfigure}`
175 = **430**, one cause (fetch took only `_sources/*.md.txt`), one cure — collected in manual-sources.md
with a **dependent forecast**: if `_images/` are ingested, `{numref}` resolution (today 32.8 %, 342 unre-
solved) must rise measurably — the future unit's built-in post-condition. The 342 are kept SEPARATE (mixed
targets), not merged into 430.

**Surfaced, not self-decided:** ROADMAP line 377 ("full-page rendering [x]") does not mention 4.11/4.12
(math/KaTeX/cross-refs/corpus gate) — minor, left for the author. A perf GATE for KaTeX paint was
explicitly declined (a proxy gate would manufacture a fresh false number — Pattern 2); the real number is
the author's, in a window.

## [2026-08-04] session | feat: selection-triggered manual lookup (replaces hover), by a measured normalization

**Why (real-use feedback).** The Monaco hover fired with a delay, popped up unbidden during edits, and
its markdown `command:` "Open" link silently failed to open the drawer (masked by a try/catch). Changed
the TRIGGER, not patched it: help by **selection**, not hover. Also removes a class of `wordPattern`
patches — ORCA keywords are often not one token (`NEB-TS`, `def2/J`, `%geom Constraints`); a selection
lets the author name the boundary. And a selection is deliberate → help as a request, not an interruption.

**Task-0 gate (commit A, `c01450e`).** Measured the normalization over 1475 ` ```orca ` blocks + the
2528-key map (`selection-lookup.corpus.test.ts`): 0 space keys, 0 paren keys; **16/121 simple keys are a
substring of a longer simple key** (`opt`⊂`optts`…) → a mid-token cut answers about the neighbour, and
the type qualifier is powerless (both simple), so the **boundary guard IS the rule**. Decisive number:
**false-reject = 0/2877** (a guard, not a muffler). Two premises refuted by the same measurement: `tight`
IS in the map (silence there is the type qualifier, not absence); `sv(p)` is NOT a key (`sv` is) → `SV(P)`
→`SV` is correct. Whole-first-then-strip: 0 loss.

**Done (commit B).** `resolveSelection` (pure: multi-line/space → malformed; boundary guard `[\w%/.-]`,
parens excluded; whole-first exact then strip `(…)` retry; position qualifier unchanged) → hit / malformed
/ miss. Panel (`selection-panel.ts`): a Monaco content widget over the settled selection (debounced),
`allowEditorOverflow` + `fixedOverflowWidgets` (the debugging/010 path); a **hit** shows keyword+type+Open,
a **malformed** selection shows a **format hint** ("select one keyword whole"), a qualified **miss** shows
nothing (silence) — three outcomes the hover collapsed into one. Open calls the `manual-open.ts` channel
directly (not a markdown command). Hover REMOVED (`orca-hover.ts` + its two tests gone); the pure lookup
(`hoverContext`/`resolveHover`/`enclosingBlock`/`isArgumentToken`) and the `setManualOpenHandler` channel
kept. Reserved layout slot for a future *Explain with Claude* — no stub button.

**Verified.** tsc 0, vitest 461 (incl. the pure resolver tests — Task 3 arg rule: `CPCM(water)`→CPCM,
`water`-inside→silence — and the wiring test — Task 4, fake editor, no jsdom: selection→resolve→panel,
Open→channel with the descriptor). cargo untouched (no Rust). **NOT done: the interactive real-window
check** (`r2SCAN-3c`/`TightSCF`/`%pal`/`nprocs`/`CPCM(water)`/`Tight`/xyz) — headless, left to the author.

**Not touched:** PageView, sectioner, keywords.json, schema, FTS; no deps; keywords.json not extended.

## [2026-08-04] session | fix: manual fences scroll instead of clipping; resizable manual drawer (two independent UI defects)

Two INDEPENDENT problems, kept apart (a wider drawer does not fix clipping — it clips again at any width
where the editor sits beside it).

**Problem 1 — clipping (a defect, not tightness).** ` ```orca ` fences clipped right with no scrollbar
though `.manual-fence` has `white-space:pre` + `overflow-x:auto`. Root cause = the flex/grid **automatic
minimum size** (`min-width:auto` = min-content) on the CONTAINER item: it grows to the fence's content
width, so the fence is never bounded and its `overflow-x` never engages. **One defect, two layouts** —
`.manual-drawer-body` (column-FLEX item) and `.manual-view-col` (the `1fr` GRID track of `ManualScreen`).
Fix: `min-width:0` on both (the horizontal twin of the `min-height:0` already present). NOT line-wrapping
— ORCA indentation is significant. Recorded as `wiki/debugging/011` (one cause, two manifestations) with
the rule: a scroll container that won't scroll → look UP the flex/grid chain, not at it.

**Problem 2 — resize.** `ManualDrawer` left edge is now draggable (`.manual-drawer-resize`, `col-resize`),
clamped **[320 px, 85 % viewport]**. The app's second continuous interaction (after fragment-drag): it
moves ONLY the width via **direct DOM** (`asideRef.style.width` on `pointermove`) and commits + persists
on `pointerup` — **no setState during the drag**, so a 209 KB `PageView` is not re-rendered per mouse
move. Width persists in **`localStorage`** — a NAMED, deliberate exception to ADR-004 (SQLite persistence):
a pure UI preference read **synchronously** on first render avoids the default-width flash a SQLite read
would cause (the `viewer_theme` pref accepts that flash; a resizing drawer should not). Pure width
math/clamp in `drawer-width.ts` (unit-tested, 6 cases). No new dependency.

**Verified.** tsc 0, vitest 467 (the render **preservation** tests untouched — this is CSS + geometry,
not render; 28 render tests unchanged). **NOT done — the interactive real-window check** (headless here),
which is the main one for a layout fix: (a) widest fence scrolls in a MINIMAL-width drawer; (b) same on
`ManualScreen`; (c) drag smooth on a 209 KB page; (d) width survives restart. The `ManualScreen` grid
clip is a certain bug (grid `1fr` + `min-width:auto`); the drawer is the same trap engines commonly show.
Left to the author to confirm in the window — not claimed.

**Not touched:** PageView, render.ts, the selection panel, lookup, keywords.json, schema.

## [2026-08-04] ingest | measure what the rendered manual HIDES — source vs published HTML (4.15 gate)

**Why.** The author saw `(sec:…solvationmodels)=` rendered before a section heading — a MyST anchor
label, INVISIBLE in the real manual (1448 in the corpus, 4.1). We showed them because the 4.12 census
listed construct TYPES (directives/math/code/xrefs) and a `(name)=` label is none — the inventory was OUR
list, not the subject's answer. Pattern 1, 5th instance; found not by a gate but by the author's EYE in
the window.

**Gate (`scripts/fetch-manual.py --html-sample N`).** Author-run, out-of-band (ADR-013 (2) intact). Fetches
a diverse 12-page sample's published HTML (Furo `<article id="furo-main-content">`, stdlib `html.parser`,
no dep) + source, and reports which SOURCE constructs are absent from the rendered text. Decisive only for
SYNTHETIC payloads (label/key/spec never coincide with prose); natural-word payloads (index/code) marked
NOT decisive — Pattern 2 named (scope < claim). Findings: `(name)=` INVISIBLE (2/186, hand-checked 0/60 on
RI+mdci; corpus 1438) → category 3; `{cite}` keys invisible but the citation VISIBLE → category 1;
`{tabularcolumns}` invisible (confirms whitelist); reverse check — none of the 3 existing whitelist items
is actually visible (no defect). Numbers + method in `wiki/orca/manual-sources.md`.

**Consolidated gap grew to five components, one root (we took sources, not the render):** 430 (input
examples + images) · 342 unresolved `{numref}` · `bibliography` (generated page → `{cite}` keeps keys) ·
`{eq}` 146 (generated numbers → stays category 2). Kept as separate numbers, not merged.

**Next (Task 1–4, commit B):** `(name)=` → category 3 (boundary = whole prose line, tested both ways);
`{cite}` → category 1 (`[keys]`); corpus gate grows (cat3 += label, cat1 += cite) + a negative control.

## [2026-08-04] session | feat: hide MyST anchor labels; render {cite} compactly (whitelist → four, Task 1–4)

**Task 1 — `(name)=` → category 3.** `render.ts` gains a `label` block (`isAnchorLabelLine`): a WHOLE
trimmed prose line `^\([^()]+\)=$` — the sectioner's own `parse_label` rule (rule #9). PageView renders it
to nothing. Boundary tested BOTH ways: a `(x)= …` mid-line (67× corpus) and a `(x)=` inside a ` ```orca `
fence (10×, kept as code — the safe side; those ride an over-extended fence past a non-standard
`` ``` --> `` close) are NOT hidden. 1438 labels hidden.

**Task 2 — `{cite}` → category 1.** The tokenizer already identified the role (excluded from inline code);
only the ACTION changed: `{cite}`/`{cite:t}`keys`` → a `cite` token → `[keys]`. Recorded as a DECISION,
not a workaround: the KEY is better than `[n]` (stable + searchable; `[n]` re-flows on reprint), so a
future "add bibliography → numbers" would be a regression.

**Task 3 — the gap grew to five components** (manual-sources.md): 430 + 342 + bibliography (generated →
keeps keys) + `{eq}` 146 (generated numbers → stays category 2), one root (we took sources, not the
render). `{eq}` left category 2 (no compact form).

**Task 4 — corpus gate + negative control.** `render.corpus.test.ts` grows: cat3 += label source, cat1 +=
cite source, visible += `[keys]`; the (S)/(R) sum still balances on **all 126 pages**. A committed
negative control proves the new branches bite (a render eating cite keys, or showing a hidden label,
unbalances the sum).

**Verified.** tsc 0, vitest 474, cargo 152 — green; corpus gate green on 126. **NOT done:** the interactive
window check (that `(sec:…)=` is gone and citations read `[barone1998]`) — headless, left to the author.
Not touched: schema, sectioner, keywords.json, the selection panel; no deps.

## [2026-08-05] ingest | measure keyring availability on this host (ADR-015 gate)

Rule #10 gate before choosing an API-key store: does a Secret Service keyring backend actually work
here (Linux Mint / Cinnamon / gnome-keyring)? Probed in a throwaway `/tmp/keyring-probe` — `src-tauri/Cargo.toml`
untouched. Recorded in [`architecture/keyring-availability.md`](architecture/keyring-availability.md).

**Works.** `keyring` 4.1.6, default Linux backend `zbus-secret-service` (pure Rust; `secret-service 5.1.0`
→ `zbus 5.18`, **no libsecret/libdbus C dep**). Happy path under service `orcastudio`:
set → get (byte-equal, rule #9) → delete → get = `NoEntry`. All clean, no panic.

**The naive absence test lied — caught on the artifact, not postfactum.** `env -u DBUS_SESSION_BUS_ADDRESS`
returns `NoEntry`, *not* absence: zbus still finds the bus via `$XDG_RUNTIME_DIR/bus`. A naive test would
have sent the code down the *key-absent* branch and the fallback would never fire — invisible here (always
has Secret Service), visible only on a foreign machine that lacks it. Same class as "the measurement measured
the adjacent thing". Genuine absence (bogus bus address) → **`Error::NoDefaultStore` at `Entry::new`**, a clean
`Err`. **`NoDefaultStore` ≠ `NoEntry`** is the load-bearing distinction: "keyring empty, ask user" vs "no
keyring, fall back to env and say so".

**Options weighed** (real crates.io resolve): `keyring` 4.1.6 chosen (thin wrapper over the OS store, ADR-005
spirit) over the community `tauri-plugin-keyring` 0.1.0 / `-keyring-store` 0.2.0 / `-keychain` 2.0.2
(pre-1.0 indirection) and the official `tauri-plugin-stronghold` 2.3.1 (**rejected**: encrypted file still
rides the backup). **Not tested:** a locked keyring (would disrupt the user's real credentials / hang on a GUI
prompt) — structural expectation `NoStorageAccess`/`PlatformFailure`, an `Err` not a panic. **Gate: PASSED.**

## [2026-08-05] decision | ADR-015 — Anthropic API key in the system keyring; key never crosses into the webview

[ADR-015](architecture/adr-015-api-key-storage.md), accepted. **Narrows ADR-014** (T1 becomes a construction,
not a described property; ADR-014 gets an amendment, not an edit), **precedent ADR-005** (give the secret to
the OS). Three decisions, each against an *invisible* loss:

1. **The key never enters the webview** — Rust makes the Anthropic call; the frontend sends the selection and
   gets text back, never the key. Consequence of ADR-009 (external side-effects = Rust) + ADR-013/overview
   posture (network egress is enumerated), not separate caution. A key in renderer scope is one `console.log`
   away — we placed exactly such logs twice in two weeks (debugging/010).
2. **Storage = system keyring** (`keyring` 4.1.6), not `settings` in `orcastudio.db` — the DB is a file that
   gets *copied* (backup, second machine, archive to a colleague) and a plaintext key leaves with every copy.
   Honest boundary carried in the ADR: keyring protects **on copy and at rest**, **not** against code as the
   same user in an unlocked session. The measurement forced a **four-state** model (`stored-in-keyring` /
   `absent` / `from-environment` / `unavailable`) because the fallback's trigger (`NoDefaultStore`) is a *third*
   state a two-state model can't express. Env fallback is **explicit in the UI, never a silent plaintext write**.
   Rejected `tauri-plugin-stronghold` (encrypted file still rides the backup).
3. **The wire payload is explicit and minimal** — word + surrounding line + section text; **not** the whole
   input, **not** the coordinates (geometry = unpublished research). Expansion is a separate, consented change.

**Next (not this unit):** the code — `keyring` dep, `set/delete/api_key_status/verify_api_key` commands (none
returns the key), the single Rust egress module, and the Settings field (Check disabled in `absent`/`unavailable`).
`ureq` already present covers the HTTP need — no new HTTP dep. ROADMAP: "Explain with Claude" leaves *Optional*.

## [2026-08-05] session | feat: Anthropic API key in the system keyring (ADR-015)

Implemented storage + the minimal verify call for [ADR-015](architecture/adr-015-api-key-storage.md).
No Explain button yet (next unit).

- **`secrets.rs`** — `KeySource` four-state resolver (`stored-in-keyring` / `absent` /
  `from-environment` / `unavailable`) + `store`/`read`/`delete` over the `keyring` crate 4.1.6.
  The env var is read **only** when the keyring is unusable (`NoDefaultStore`/`NoStorageAccess`/
  `PlatformFailure`), never when it's merely empty; `set` fails loudly if the keyring is
  unavailable — **no silent plaintext-in-DB fallback**. Commands `api_key_status`/`set_api_key`/
  `delete_api_key` — **none returns the key**; the frontend gets only the source state (+ `last4`).
- **`anthropic.rs`** — the single egress. `verify_api_key` is `async` + `spawn_blocking` (threading
  rule: a 15 s offline timeout must not freeze the GTK/WebKit main thread) and hits `GET /v1/models`
  (authenticates, spends **no** generation tokens — grounded via the claude-api skill; header
  `anthropic-version: 2023-06-01`, model const `claude-opus-4-8`). No logging of key or body.
- **No new HTTP dep** — the existing `ureq` 2.12.1 carries rustls TLS (verified in Cargo.lock).
  Only new dep is `keyring = "4.1.6"` (pure-Rust zbus-secret-service, no C `-dev` package).
- **Settings UI** — password field cleared right after Save; Save/Delete/Check; the key **source is
  shown explicitly** (keyring / env / unavailable-reason); **Check disabled** in `absent`/`unavailable`
  (a network call there would misreport "could not reach Anthropic" — different causes, different
  messages, the same posture as the three tildes in the selection panel).
- **Verification.** cargo 160 / tsc 0 / vitest 474 — green. Wiring tests: the key commands' return
  types are pinned (none is the key); `KeySource` serializes without the secret body — **negative
  control demonstrated to bite** (a `last4` that forgets to truncate leaks the key and turns the gate
  red, reverted green). **NOT checked:** the live `verify` network call (needs a real key — the
  window check is the author's), and a locked keyring (named-untested, ADR-015).
- ROADMAP: "Explain with Claude" is **no longer Optional**.

## [2026-08-05] decision | ADR-015 env-fallback review condition; CLAUDE.md rule for measurement-page placement

Two follow-ups that belonged with ADR-015, appended (no decision text rewritten).

- **[ADR-015](architecture/adr-015-api-key-storage.md) amendment — review condition for the env
  fallback**, in the genre of ADR-013 (3). Names the cost of the narrow rule: a user with
  `ANTHROPIC_API_KEY` set AND a working-but-empty keyring is shown `absent`, not `from-environment` —
  the app ignores a key they have. Harmless on the desktop; the **trigger to reopen is Phase 5 (SSH
  remote execution)**, where headless/remote contexts make env the only usable channel. Reopening
  means deciding whether env stays a fallback or becomes a config channel — which needs its own design
  (documented precedence, visible override indicator, stale-variable behaviour). Decisions (1) and (3)
  don't depend on it.
- **CLAUDE.md — new page-types row for measurement (rule-#10) pages.** Placement is by *what the
  measurement serves*, not "is it about the host": supports a **decision** → `wiki/architecture/`
  (keyring-availability.md under ADR-015); about **ORCA behaviour / how to run it** → `wiki/orca/`
  (performance.md, parse-sources.md, manual-sources.md, input-syntax.md). Both existing pages already
  sit correctly — nothing moved; the rule exists so a third such page doesn't drift to a third home.
  `index.md` needs no change (it carries no per-category placement prose).

## [2026-08-05] decision | model choice from the LIVE /v1/models; the default is review-conditioned

Recorded as amendments (decision text untouched): [ADR-015](architecture/adr-015-api-key-storage.md)
(model selection) + [ADR-014](architecture/adr-014-ai-integration-boundary.md) (T1 now implemented).

- **Options are measured, not documented (rule #10).** The Settings model picker is populated from
  `GET /v1/models` — the models THIS key can use — never a hardcoded menu. The picker can only ever offer
  a model this key can reach, because its source is the run, not **our idea of the model lineup** — and a
  wrong idea of the lineup is exactly what a hardcoded menu inherits. This session recorded the failure
  twice, both from the **architect** (not a gate, not Claude Code): `claude-opus-4-8` suspected as
  fabricated (it exists), and a model absent from a search-found release list declared not to exist (it
  exists too). Neither could reach the picker's options. (See Part F, Pattern 1, 6th instance —
  corrected 2026-08-05: an earlier draft of this entry claimed "Opus 4.6 does not exist"; it does. The
  argument now needs no lineup fact.)
- **The default lives in `settings`, not a const** — a price/sufficiency decision (Sonnet 4.6 is enough
  for explain), ADR-004's store, not a UI preference (it governs cost + what goes on the wire). The const
  is only the seed/fallback.
- **No price shown** — `/v1/models` doesn't return it; hardcoding one is the recalled-constant anti-
  pattern ADR-014 (1a). Omitted, not invented.
- **Review condition:** the default `claude-sonnet-4-6` is a superseded model (Sonnet 5 displaces it) —
  revisit at its deprecation or when the live list stops containing it. The live-list rule removes the
  stale-menu failure for the *options*; this names it for the single *default* string it can't remove.

## [2026-08-05] session | feat: explain a selection with Claude (T1: three fields, live model list, no editor writes)

The first AI feature — ADR-014 T1, layer 1 of 3. Selection + resolved section → one answer in the drawer.

- **Grounding is STRUCTURAL, not a prompt.** No Explain button without an open, resolved section
  (`canExplain` = usable key AND section, tested), so the model has nothing to answer from memory — the
  "plausible number without provenance" regress is removed by the absence of a path. The system prompt
  ("ground only in the section") is the belt to that suspenders.
- **`explain_selection(word, line, section)` — exactly three `&str`.** No file/geometry parameter; the
  bound is the command's type. `build_explain_prompt`'s `fn(&str,&str,&str)->String` signature is pinned
  by a wiring test (sibling of the secrets return-type pins). Model read from `settings`, not the caller.
- **Tier-zero.** The explain path exposes no editor-mutating call; the answer renders in a bordered,
  labelled drawer band ("Explained by Claude — not ORCA manual text") — the reader sees the source↔
  interpretation border. A wiring test asserts `explain()` hands the drawer only `{word, line, descriptor}`
  and never mutates the (spied) editor.
- **Error causes distinct (TASK 5):** 401 key / 404 model-not-available / other API / transport offline —
  different messages (`status_error`, tested). No logging of key or body. Reuses `ureq` (no new dep, no
  new feature — `serde_json` over string bodies).
- **Model picker** from the live `/v1/models` (see the decision entry above), stored in `settings`.
- **ПРИЧІП:** `{eq}`label` → `[label]` (the {cite} fix, one construct later; sample + corpus negative
  control added). `{table}` caption **flagged, NOT done** — it is the first directive whose opener must
  split into hidden syntax + visible caption, a new axis unlike an inline role; left to its own unit (the
  author's "stop if harder than {cite}" instruction).
- **Verification.** cargo 160 / tsc 0 / vitest 481 — green; corpus preservation gate green on **126**
  leaves with the {eq} change; negative controls bite. **NOT checked:** the live Anthropic call (needs a
  real key — the window check is the author's), so `verify`/`list_models`/`explain` are exercised by unit
  tests over the pure parse/prompt/error-map helpers, not an end-to-end network round-trip.

## [2026-08-05] lint | correct a false model-lineup fact; Pattern 1 gains a 6th (architect-sourced) instance

The unit-4.16 model-selection amendment argued the live-list picker "cannot offer a non-existent model
(there is no Opus 4.6 — 4.6 is Sonnet)." **That is false — Opus 4.6 exists** (it is in the claude.ai
model list). The error was the architect's: a release list found by search held the *notable* releases,
not all, and "absent from the list" was read as "absent from reality."

Fix — reformulate the argument so it needs **no fact about the model lineup at all**: the picker can only
offer a model this key can reach, because its source is the run (`GET /v1/models`), not our idea of the
lineup — and an idea of the lineup is precisely what a hardcoded menu would inherit. The illustration is
now a **recorded event, not a claim about a third-party product**: the architect erred about the lineup
twice this session (suspected `claude-opus-4-8` as fabricated — it exists; declared Opus 4.6 nonexistent —
it exists), and **neither wrong belief could reach the list**, because the list is not from him. The
construction survived the architect's error; a hardcoded list would not.

Recorded in [`orca/manual-sources.md`](orca/manual-sources.md) Part F as **Pattern 1, 6th instance**, with
its source named the **ARCHITECT** (not a gate, not Claude Code) — the pattern has no owner, and that is
part of the lesson. Touched: adr-015 (model-selection amendment), tauri-core.md, frontend.md, and the
prior log entry's false clause; the default's review condition (Sonnet 4.6 → Sonnet 5) is **unchanged** —
it does not depend on this error.

## [2026-08-05] ingest | probe: does `! UseSym` reorder atoms in ORCA 6.1.0 output artifacts (measured)

Phase 4.2 unit 1a, Part A. The gate for the Stage-1 IndexMap design (ADR-016): does ORCA reorder
atoms in its output when symmetry is active? The ORCA manual describes `! UseSym` as reorient +
center + symmetrize and says **nothing** about atom ordering — so it was measured, not assumed
(rule #10).

Probe [`sidecar/probes/usesym_atom_order.py`](../sidecar/probes/usesym_atom_order.py) — three real
`/opt/orca/orca` runs (serial, one isolated dir each): formaldehyde `H C H O` SP (→ C2v), methanol
SP with interleaved methyl-H order (→ Cs), water `H O H` Opt+Freq r2SCAN-3c (→ C2v). Detector
compares **rigid-motion-invariant fingerprints** (per atom: sorted distance vector) so a legal
reorientation/symmetrization is not mistaken for a reorder, and names **equivalence classes** so a
permutation of symmetry-equivalent atoms is reported unobservable rather than "no reorder".

**Verdict (within scope): NO observable reorder in any artifact** — `.out` echo, `.property.txt`
`$Geometry`, `_trj.xyz` (all frames), final `.xyz`, `.hess $atoms`. ORCA's own "Symmetry-perfected
Cartesians" table lists atoms in exact input order with a 0-based `Index` (consistent with the 0-based
`%geom` base). Symmetrization drift ≈ 0 (inputs built symmetric); `.hess`-vs-final-frame fingerprint
1e-6 Bohr (print precision, not motion). **Negative controls demonstrably bite** (CLAUDE.md convention):
element-swap → element-check red; in-plane↔mirror methyl-H swap (same element, inequivalent) → fingerprint
red where element-check is blind; equivalent-mirror-H swap → correctly unobservable (no crying wolf).

Consequence for ADR-016 / ADR-010: for a `UseSym` job the IndexMap is the **identity**, carried as a
post-condition (the probe's check re-run on real output) rather than a mandatory permuted map — but only
within the measured scope. Claim bounded to these 3 molecules, SP+Opt/Freq, these artifacts, ORCA 6.1.0;
D-groups / cubic groups / `PointGroup "..."` / large systems / intra-equivalence-class order NOT measured.
Recorded: [`orca/usesym-atom-order.md`](orca/usesym-atom-order.md). Part B (ADR-016 + amendments) pending
the author's go-ahead. Run artifacts gitignored (`sidecar/probes/_*_runs/`); the wiki quotes them verbatim.

## [2026-08-05] decision | ADR-016 emit_input ownership (Rust core); ADR-010(i) + ADR-014 amendments; Stage 1 units 1a–1e

Phase 4.2 unit 1a, Part B — ingest of the decisions the UseSym probe gated.

**ADR-016 (new) — `emit_input` ownership: the Rust core.** ADR-010 made `emit_input`/`parse_output`
a type-level pair, but they live in different languages — emit is TS (`injectSceneIntoInput`,
`constraints.ts`, `input-builder/`), parse is Rust (ADR-012 readers, immovable: they stream the
unbounded log and size-cap the artifacts). A type invariant across a language boundary is a
convention with a compiler on one side. Since `parse_output` cannot move, **the order-bearing
`emit_input` moves to Rust** — a new `orcastudio-core` crate in the workspace (proposal §6.3;
separate now because Stage 2 compiles it to WASM). **Narrow move:** only the coordinate block +
`%geom Constraints` emit (the two paths whose output depends on atom order) cross in Stage 1; the
Scene store, editor UI, method/basis form, and geometry↔sidecar seam stay TS to Stage 2/3 — stated
as a decision, not left as residue. Five bare-integer seams enumerated; ~68-site scope recorded as a
rough sizing number, not a precise count. **Probe consequence:** the `parse_output` map is the
**identity in the measured scope**, and — the point ADR-016 makes precise — the **post-condition**
(element-seq + fingerprint on real output) is not a defensive extra but the mechanism that makes
identity a safe assumption *outside* the probe (unprobed point groups, future ORCA versions): the
map is verified against the artifact, never trusted. Display-vs-authoritative emit tension named and
deferred to 1e on purpose.

**ADR-010 amendment (correction (i) refined).** History not rewritten. Correction (i) said "Rust
builds the IndexMap"; the exact principle is "the map's builder is the **owner of the emitted
order**." For the ORCA-input seam that owner is now Rust (ADR-016); for the geometry↔sidecar seam it
is **TS** — TS fetches `/geometry` directly, Rust never touches it (verified: `grep -rn "/geometry"
src-tauri/src/` empty) — and stays TS until the Scene moves in Stage 2. The sidecar stays positional
(unaffected).

**ADR-014 amendment (charge/multiplicity) — PENDING AUTHOR REVIEW.** Charge and spin multiplicity
fall between decision (1) (not coordinates) and (1a) (not curated geometric constants), yet T2 may
draft an `.inp` that carries `* xyz <charge> <mult>`, and a wrong multiplicity runs cleanly and
returns meaningless physics — the charge/spin analogue of the missed Bohr→Å conversion. Proposed
rule: the AI never emits charge/multiplicity silently; they come from the Scene (fragment-charge sum,
ADR-008 #8 / 2.5.0b) or an explicit user value; the guard is `checkElectronParity` (`parity.ts`), a
tool refusal not a prompt line — same shape as decision (3). Formulated for the author's review, not
yet ratified.

**ROADMAP.** Stage 1 rewritten into units 1a–1e (1a checked done: probe + ingest; 1b AtomId in TS
Scene; 1c orcastudio-core crate + golden vs TS emit; 1d parse pairing + round-trip property test; 1e
wiring, mints map at create_job, resolves the display-vs-authoritative tension). Phase-4.5 UseSym
open question rewritten from "settle before any symmetry work" to "PARTLY SETTLED" — points at the
measured page, keeps the block, names what is still open (D/cubic groups, `PointGroup "..."`, large
systems, intra-equivalence-class order) and the re-run-the-probe-per-system rule. "Explain with
Claude" already carries the "Layer 1 of 3" mark (capital L — a lowercase grep missed it); no change.

Touched: adr-016 (new), adr-010 (amendment), adr-014 (amendment, pending), ROADMAP.md, index.md.
No code in this unit (ADR-016 lands across 1c–1e).

## [2026-08-05] decision | ratify the ADR-014 charge/multiplicity amendment (two scoping fixes)

Phase 4.2 unit 1b, task 0. The author accepted the unit-1a charge/multiplicity amendment to ADR-014
with two wording fixes; PENDING AUTHOR REVIEW removed, status → accepted.

(a) **Bound the parity guard's claim.** `checkElectronParity` catches only parity-**impossible**
states; a parity-**consistent** wrong multiplicity (a triplet for a closed-shell singlet) passes
through it. For that class the protection is the **provenance rule** (the draft writes no field —
Scene or user does), with parity as an arithmetic backstop. "Holds on the model's worst day" belongs
to the provenance rule as a tool-shape, not to the parity check.

(b) **Name the asymmetry with its reason.** The human path *warns, does not block* (a person builds
incrementally and may legally pass through a temporarily odd state — scene.md "Why the UI warns, not
blocks"); the AI-draft path *refuses* (no incremental excuse — one-shot artifact must be born valid).
Cross-referenced both ways (adr-014 ↔ scene.md) so a future lint reads it as a stated asymmetry, not
a page contradiction.

Touched: adr-014 (amendment ratified), scene.md (back-reference). No code.

## [2026-08-05] session | feat(scene): AtomId — stable atom identity in the TS Scene model (unit 1b)

Phase 4.2 Stage 1 unit 1b. Adds `AtomId` to the TS Scene ahead of the Rust core move (ADR-016 lands
that in 1c–1e). **No Rust touched; the ~68 positional-index sites were NOT rebranded** (that is 1c–1e
/ Stage 2 territory — rebranding without changing seam ownership is cosmetic churn with regression
risk and no new invariant). 1b adds identity to the *model* only.

**Types (`ids.ts` new, `types.ts`).** Branded `AtomId = number & {unique symbol}` (erases to a plain
int at runtime → JSON writes a bare integer). Split `RawAtom` (`{element,x,y,z}`, what parsing
returns) vs `SceneAtom` (raw + `id`, only inside a Scene); `RawFragment` (detached) vs its in-Scene
subtype `SceneFragment` — so anything accepting `RawFragment` also accepts `SceneFragment`. This
subtype relation removed the "provisional id" smell entirely: library/placement/detached fragments
are `RawFragment` and simply have no ids; ids are minted only on Scene entry.

**Allocation is pure.** Counter on the Scene (`nextAtomId`), never a module global; `stampFreshIds`
mints and advances. Fresh scene → `0..n-1`; `addFragment` mints from the counter for joining atoms;
`removeFragment` never rolls back (ids never reused; uniqueness is per-Scene, so `undoReset` is
correct). `placeFragment`/`translateFragment` made generic over Raw/Scene, preserving ids on a rigid
move.

**The id-transfer rule (the risk this unit was about).** `replaceFragmentAtoms`/`replaceAllAtoms`
take `RawAtom[]` (ASE/xtb/GOAT/parsed — the type has no `id`, so nothing to mis-transfer) and carry
the OLD atoms' ids **positionally** (`carryIds`), correct because count+element-order is already
enforced. Re-minting on replace would silently void identity on every edit; nothing crashes. Negative
control: `id` identical (`===`) before/after `replaceAllAtoms` — **shown red** by swapping
`carryIds → stampFreshIds` (`[1000,1001,…] ≠ [0,1,…,7]`), then reverted.

**scene_json v2 + v1 migration.** `serializeScene` → v2 (per-atom id + `nextAtomId`). `deserializeScene`
validates v2 (ids unique, all `< nextAtomId`) and **migrates v1 in place** (ids `0..N-1` scene-wide,
`nextAtomId=N`) — never rejects it. Rejecting v1 would make `restoreScene` mark every existing
multi-fragment job `snapshotRejected` and silently collapse it to one fragment on open. Tested with a
**real** v1 string emitted by the pre-1b code (`__fixtures__/scene-v1.json`, copied verbatim — not
synthesized): through `restoreScene` the 2-fragment layout survives, `snapshotRejected=false`.
Negative control: valid v1 not `null` — **shown red** by disabling migration, then reverted. **No SQL
migration** (version lives inside the JSON; `jobs.scene_json` stays a plain TEXT column).

**Monaco collapse = identity boundary (named, not a bug).** `collapseToSingleFragment` mints FRESH
ids — identity continuity across arbitrarily hand-edited text is undefined; holds until Stage 2 makes
the xyz block a read-only projection. Recorded in scene.md.

**Task 0 (same session, separate commit `f56fc8a`):** ratified the ADR-014 charge/multiplicity
amendment with two scoping fixes (parity guard catches only parity-impossible states; human-warns vs
AI-refuses asymmetry, cross-referenced adr-014 ↔ scene.md).

Verify: tsc 0; vitest **489 passed** (+8 new: 5 AtomId allocation/preservation, 3 v1 migration);
cargo unchanged (160, no Rust touched). The app window was **not** run — 1b is a pure TS-model change
with no screen or UI-flow change; acceptance is "nothing changed", carried by the full green suite +
the new tests. Test-only `scene-test-util.ts` added so tests mint ids as production does without
making `id` optional. Touched: ids.ts (new), types.ts, scene.ts, store.ts, ensemble.ts,
fragment-library.ts, placement.ts, JobDetailScreen.tsx, NewJobScreen.tsx, + test files; scene.md,
ROADMAP.md.

## [2026-08-05] ingest | probe: JS toFixed(8) vs Rust {:.8} coordinate-formatter parity (bit-pattern corpus)

Phase 4.2 unit 1c, Part A — the gate before the orcastudio-core crate. ADR-016's golden test asserts
byte-identity of the Rust coordinate emit with the TS `mergeToAtomLines` (`toFixed(8).padStart(14)`);
byte-identity is safe only if JS `toFixed(8)` (round-half-up per ECMA-262) and Rust `{:.8}`
(round-half-to-even, flt2dec) agree. Different algorithms → measured, not assumed (rule #10).

Method (load-bearing): `scripts/float-parity-corpus.mjs` transfers every double to
`scripts/float_parity_reader.rs` as its exact **u64 bit pattern** (`writeDoubleLE` → hex →
`f64::from_bits`), NEVER a decimal string — a decimal string re-parses with its own rounding and the
corpus would measure a parser round-trip, not the two formatters. Decimal strings on each line are the
JS output being compared against, not a transport of the value.

Corpus (fixed seed, reproducible): signed zero; tiny round-to-zero values; 2000 near-half points at
the 8th decimal; classic decimal-tie traps (1.005, 8.575, …); padStart(14) width boundaries (13/14/15
chars); 5000 typical chem coords; + 1,000,000 random in [-1000,1000]. **1,012,786** distinct doubles.

Result: **0 rounding divergences.** The round-half-up vs half-to-even difference did not surface — for
8 decimals a binary double almost never lands exactly on a decimal half. The **only** divergence is
signed zero: `0x8000000000000000` (−0.0) → JS `"0.00000000"` vs Rust `"-0.00000000"` (Rust keeps the
sign; toFixed drops it). Deterministic, one case.

Verdict: golden byte-identity is viable; the −0.0 case needs one rule. **STOP per the Part-A protocol
(>0 divergences) — the rule is the architect's call:** (A) Rust normalizes −0.0 → +0.0 in the
coordinate formatter (recommended — minimal, keeps strict byte-identity) or (B) weaken the golden to
token-numeric equality. Recorded: `architecture/float-formatting-parity.md`. Part B (the crate) waits
on the rule decision. Corpus gitignored (regenerated from seed); generator + reader committed.

## [2026-08-05] ingest | unit 1c Part A2 — correct the formatter probe (real ties = odd/512); fmt_coord rule; constraint-value front

Architect review of Part A: bit-pattern method right, −0.0 real, STOP right — but "0 rounding
divergences" was **false in scope**. The tie stress used `(k+0.5)·1e-8`, which are not representable
binary halves; the TRUE 8th-decimal ties are `x = odd/512` (`x·10⁸+½ ∈ ℤ ⇔ x = odd/2⁹`). At those, JS
`toFixed` rounds half-**away**, Rust `{:.8}` half-to-**even** → diverge when the 8th digit is even.
Reproduced on this toolchain (rustc 1.97.1): 1/512 JS `0.00195313` vs Rust `0.00195312`; 3/512 agree;
−1e-12 agree; −0.0 diverges.

**Front 1 (coordinates) — SOLVED.** Corpus rebuilt with the odd/512 tie class (both signs, integer
offsets, near padStart boundaries), 1,008,832 doubles. Bare `{:>14.8}` diverges **2025** (1
sign-of-zero + 2024 tie) — the negative control, shown red. `fmt_coord` (3-part rule: signed-zero
`if x==0.0{0.0}else{x}`; tie detect `y=|x|·512` exact, odd-integer ⇒ away-from-zero
`m=floor(|x|·1e8)+1`; else `{:.8}`; then padStart(14)) is **0** divergences, byte-identical incl.
negative ties. This is the emit spec for Part B; the corpus comparison becomes a permanent `#[ignore]`
gate, adversarial values baked into golden fixtures.

**Front 2 (constraint value) — OPEN, STOP.** `constraintsBlock` renders values via `String(v)`
(shortest round-trip), not toFixed. Measured `String(v)` vs Rust `format!("{}")` over 505,972
constraint-plausible doubles: **14 divergences**, all the same class — a value whose shortest
round-trip needs **17 sig digits** with the 17th ambiguous; V8 dtoa picks the lower, Rust flt2dec the
higher (e.g. `-200.30410766601562` vs `…63`). Arises only for raw full-precision doubles (a measured
bond length); a user value is preserved by `valueText` and a canonical short number is unique. Named
boundary: JS `String` goes exponential for `|v|≥1e21` and `<1e-6`, Rust `{}` never — unreachable for
the constraint range but stated. Per protocol (>0 → STOP), `fmt_value` not baked; recommended rule
(B1): the core Constraint value is `valueText` or a short number, "freeze at measured" omits the value
(ORCA idiom), so a 17-digit double never reaches the formatter — matches the TS path. Architect's call.

Recorded: `architecture/float-formatting-parity.md` (both fronts, corrected). Corpora gitignored.

## [2026-08-05] decision | unit 1c fmt_value rule — canonicality judged by each emitter's own render; loud 17-digit guard

Architect's decision closing Front 2 (constraint value). Not zero divergences, not V8 emulation, not
a weakened golden: `fmt_value(v) = format!("{}", v)`, and the value model mirrors TS so **canonicality
is judged by each emitter's OWN render**. Core Constraint: `None` → freeze (no number);
`value: Option<f64>` + `value_text: Option<String>`; EMIT `value_text ?? fmt_value(value)`; PARSE
`value_text = Some(tok) ⇔ tok != fmt_value(parse(tok))`. Because Rust judges by its own `fmt_value`
(as TS by `String(v)`), anything Rust can't reproduce canonically is preserved verbatim — the
"17-digit double never reaches the formatter" becomes a guarantee by construction, not an assumption.
Backstop: a programmatic value (`value_text=None`) with ≥17 significant digits → `CoreError` (named,
loud), threshold measured (all 14 divergences 17-digit, 0 at ≤16). Opposite-judgment/same-bytes case
(`1e-7`: TS goes exponential and sets valueText, Rust stays fixed and does not) documented + golden-pinned.

## [2026-08-05] session | feat(core): orcastudio-core crate — AtomId/OrcaIndex/AseIndex/IndexMap, v2 scene, emit_input (unit 1c Part B)

Phase 4.2 Stage 1 unit 1c Part B. New Rust crate `orcastudio-core` in a cargo workspace (root
Cargo.toml, members src-tauri + orcastudio-core), std-only + serde (WASM-ready, ADR-016). **Dead code
beyond its own tests until 1d/1e** — expected.

- `ids.rs`: `AtomId`/`OrcaIndex`/`AseIndex` newtypes (mixing does not compile — two `compile_fail`
  doctests prove it); `IndexMap<T>` built from ONE source (`from_emit_order`), forward+reverse
  consistent by construction.
- `scene.rs`: `deserialize_scene` for v2 with TS validation semantics; **v1 → loud named
  `UnsupportedSceneVersion`, NOT migrated** — migration has one home (TS at DB read), create_job
  always emits fresh v2, so v1 in Rust is a caller bug.
- `emit.rs`: `emit_coordinate_block → (text, IndexMap<OrcaIndex>)` and `emit_constraints_block`,
  byte-identical to `injectSceneIntoInput`'s block / `constraintsBlock`. `fmt_coord` (odd/512 ties +
  signed zero, from Part A2) and `fmt_value` + `value_text_for` + the 17-digit guard (Front 2 rule).
  Own Constraint type WITH value_text; xtb.rs Constraint left untouched with a TODO(1e).
- Golden fixtures written once from the real TS emitters (throwaway vitest generator, then deleted),
  committed; `tests/golden.rs` asserts byte-identity + IndexMap↔rows coupling + measured-value
  round-trip + the exponent-boundary opposite-judgment case. `tests/parity_gate.rs` = the permanent
  `#[ignore]` fmt_coord corpus gate (0/1,008,832 by hand).

Negative controls, all shown red then reverted: (a) corrupt one golden byte → coordinate golden
FAILED; (b) build the IndexMap from a reversed order → both coupling tests FAILED (order and map are
one source); (d) disable `value_text_for` → the measured-value round-trip FAILED (why the parser must
set value_text).

Verify: `cargo build --workspace` + `cargo test --workspace` green (src-tauri 160 unchanged + core 12
unit + 6 golden + 2 doctests + 1 ignored gate); tsc 0; vitest 489 (TS untouched). **Named build risk:**
the workspace target dir moves to ./target; `npm run tauri dev`/`build` (desktop window + bundling) is
for the author to confirm — verified headless as far as cargo reaches. Added: modules/orcastudio-core.md;
ROADMAP 1c → [x] + the Stage-2 selection-on-AtomId dividend.

## [2026-08-06] session | feat(parse): readers take the IndexMap — verified against the artifact, identity for legacy (unit 1d)

Phase 4.2 Stage 1 unit **1d** — paired the ADR-012 artifact readers with the ADR-016 identity core.

**Docs first (unit 1c tail, Task 0).** Recorded the corpus-construction lesson as a Pattern-2
corollary on `architecture/float-formatting-parity.md` (with a cross-ref from `orca/manual-sources.md`
Part F): the `toFixed(8)` parity corpus was first seeded from the *folklore* tie (`1.005`,
`(k+0.5)·1e-8`) — non-representable decimals that can never be a round-half tie, an adversarial-looking
corpus that tested nothing. The real ties (`x = odd/512`) come from the failure-class arithmetic
(`x·10⁸+½ ∈ ℤ`). Lesson: build the stress inputs from the failure condition, not from the canonical
cautionary example. (Separate `docs:` commit.)

**The pairing.** `src-tauri` now depends on `orcastudio-core`. The readers (`property`/`hess`/`xyz`/`mo`)
take the job's `IndexMap<OrcaIndex>` in `verify()`; the former element-order post-condition is rephrased
as the **map post-condition** (`parse::check_map_order`): the artifact's element sequence must equal the
order the map asserts — position `p` holds `map.to_atom(OrcaIndex(p))`, whose element the reference fixes
**independently of the map** (its `ids`↔`z` table). Exactly **one function per reader** changed (the seam
ADR-016/Phase-3 promised); accessors, result structs, and stored JSON are byte-for-byte unchanged — the
`real_optfreq` fixture numbers are identical. For the identity map (every job in 1d) it reduces to the
pre-1d `artifact_z == reference.z` check.

**The honest claim (the architect's first review point).** The ADR-010 `emit_input`/`parse_output` pair
is type-level only **in-process** (orcastudio-core on both sides). The map is minted at `create_job`,
**serialized into SQLite**, and re-read at parse time — serialization **erases the type provenance**, so
across persistence the invariant degrades to *a required argument cross-checked against the artifact* (a
post-condition, rule #9), NOT a type guarantee. Stated verbatim on `check_map_order` and in
`modules/artifact-readers.md` — the over-reach ADR-010's empirical addendum warns against, avoided by
name.

**Persistence.** Schema **v10**: nullable `jobs.index_map_json` (guarded ALTER). Every row is NULL in 1d
→ `results::job_index_map` derives an **identity map** from the input coordinate block, cross-checked in
`verify()` (never postulated — the input on disk can be hand-edited after the run). An unreadable
coordinate block is a loud, named parse failure; a *present* map value is refused (minting is 1e's job).
Migration test `migrate_v9_to_v10_adds_index_map_json_and_preserves_jobs`.

**Round-trip (typed, in-process half).** `orcastudio-core/tests/roundtrip.rs`: 2000 seeded scenes,
`set(AtomId) → emit → parse → set(AtomId)` = identity, map a bijection. Deterministic splitmix64 (no
`rand`); `emit::parse_coordinate_rows` is the crate-local inverse.

**Negative controls — demonstrably bite (the second review point).** `parse::map_order_controls` +
`property::tests`: (a) a permuted map (C↔H, non-equivalent) → `OrderMismatch`; (b) a wrong-count map →
`LengthMismatch`; (c) `check_order_ignoring_map`, a map-ignoring twin, goes **green** on the same
permuted input that (a) rejects — proving the map/artifact cross-check is what holds the permutation red,
not chance. Verified by temporarily gutting `check_map_order`: exactly those 5 control tests went red,
everything else stayed green; reverted.

**Verification.** `cargo test --workspace` green (166 src-tauri + core + roundtrip); `tsc --noEmit`
clean; vitest 489/489. `create_job`, TS, and `xtb.rs` untouched (1e's scope). Next: **1e** — mint the map
at `create_job`, brand the xtb serde boundary, resolve the display-vs-authoritative-emit tension.

## [2026-08-06] session | feat(jobs)+refactor(xtb): unit 1e — mint the map at create_job, brand the xtb boundary, close Stage 1

Phase 4.2 Stage 1 unit **1e** (the wiring unit). Two feature commits + this docs/lint commit.

**Minting (feat commit).** `orcastudio_core::mint_index_map(&Scene, input_content)` parses the
coordinate block of the **submitted text** and verifies it corresponds to the scene (element
sequence exact + float-tolerant coords — the `xyzMatchesScene` standard, `normalize_element` the Rust
twin). Mints from that verified correspondence — **never from the scene alone**: ORCA runs the text,
so a scene/text drift SKIPS (`{"skipped":"<reason>"}`), it does not silently encode a lying map (the
1d failure class, moved to mint time). `create_job` stores `results::StoredIndexMap`
(`{"minted":[atomids]}` | `{"skipped":…}`) in `jobs.index_map_json`; the job is never blocked (input
validity is ORCA's, the map is ours). Single mint site → a clone/"new iteration" mints its own map.
`results::resolve_job_mapping` uses a minted map with a **scene-sourced AtomId anchor** (from
`scene_json`, *independent of the stored map* — so a corrupted stored map is caught, not cancelled),
else the derived identity map (1d). Only the anchor switches to scene for minted jobs; element/coord
truth stays artifact/text-sourced.

**xtb branding (refactor commit).** The `$constrain` 0→1 base flip was a bare `+ 1` at ~9 writer
sites (a missed/doubled flip freezes the wrong coordinate on a clean run — rule #9). Branded the
serde boundary: `SceneIndex` (0-based, `#[serde(transparent)]`, **no `Display`**) → `XtbIndex`
(1-based) via one `to_xtb()`; `.zero_based()` is the named accessor for our own geometry. Not unified
with `orcastudio_core::emit::Constraint` (different emit) — named `TODO(1e-followup)`.

**Negative controls — all four demonstrably bite (reverted after):** (a) reordered/drifted/wrong-count
text → skip with a named reason (core `mint::tests` + `create_skips_the_map_when_text_reorders_the_scene`);
(b) a synthetic **permuted stored map** → parse verify refuses via the artifact cross-check, while the
correct minted map verifies (`minted_map_is_load_bearing…`) — breaking the scene-independence of the
anchor makes it green, proving the anchor holds it; (c) gutting the mint correspondence turns the (a)
skips into scene-only mints, tests go red; (d) writing a bare `SceneIndex` into a line is a **compile
error** (`doesn't implement Display`) + a source-grep test forbids a bare `atoms[N] + 1` in the writer.

**Verification.** `cargo test --workspace` green (203); `tsc --noEmit` clean; vitest 489/489. Sync
(Scene→Monaco) untouched; no submit-time text canonicalisation; no runtime display-vs-authoritative
byte-check (deliberate). Real-window run (New Job → Create & Run → Results on a 2-fragment scene) is
for the author — headless cannot exercise it.

## [2026-08-06] decision | ADR-016 amendment — display/authoritative tension resolved; two minting paths

Resolved the tension ADR-016 deferred to 1e (amendment section, history not rewritten): (1) the
display emit stays TypeScript (sync untouched, no IPC on the debounce); (2) the authoritative act is
at `create_job` — mint from the **submitted text verified against the scene**, never the scene alone;
(3) **no** submit-time text canonicalisation (text is truth, 2.5.4b) and **no** runtime
display-vs-authoritative byte-check (it cannot tell a legal edit from a drift — noise, not a guard;
byte-identity lives in the golden/corpus gates), recorded so it is not "finished" into existence
later; (4) two production minting paths named — **authored-by-app** (Phase 4.5 + gates) and
**authored-by-text** (1e: verify text↔scene). Also amended **ADR-010**: its "type-level invariant"
claim for the `emit`/`parse` pair is refined — type-level in-process, a required artifact-cross-checked
post-condition across SQLite persistence.

## [2026-08-06] lint | Stage-1 pages (ADR-010/016, orcastudio-core, artifact-readers, tauri-core, scene)

Mini-lint on Stage-1 close. Findings + fixes: `orcastudio-core.md` "dead code until 1d/1e" and
"unit 1e still owes" → updated to "Stage 1 complete" + `mint.rs` entry; `artifact-readers.md`
"NULL for every row until 1e mints it" → the minted-vs-derived story with the scene-anchor
independence; `tauri-core.md` v10 "every row is NULL" → the minted/skipped envelope; `ADR-010` line
30–32 unqualified "type-level invariant" → pointer to the 1d–1e / ADR-016 refinement; `index.md`
core + readers lines refreshed. No broken cross-references found; the ROADMAP marks 1e `[x]` and
**Stage 1 COMPLETE**.

## [2026-08-06] ingest | Phase 4.2 Stage 2 rewritten into units 2a–2d; ephemeral layer → Stage 3

ROADMAP ingest for the operation log. **Stage 2** ("operation log + ephemeral layer") rewritten
into units **2a–2d**: 2a pure log types + ingest (`[x]` this session); 2b the store on the log
(deep undo/redo, `scene_log_json` persist, "New iteration" restores the log); 2c1 the dumb renderer
(`AtomId → viewer index` table, picking → `AtomId`); 2c2 the pipeline (selection/measure/edit-plan/
constraints) onto `AtomId` + the **`selectionSurvives` dividend** (a conscious behaviour change —
preserve a selection across a fragment removal, now that identity gives "the same atom" an
operational definition); 2d the Monaco xyz **read-only projection** + paste-as-fragment (manual
coordinate editing is *replaced, not removed*). The **ephemeral drag layer** moved out of Stage 2's
heading into the first unit of **Stage 3** (the rigid-body drag) — it is needed only for the drag
(ADR-010). *Open (blocked on the architect's verbatim wording): the Phase 3 tail item
"multi-fragment frontier-orbital labeling" — to be pasted in with its honesty caveat about weights
without an overlap matrix.*

## [2026-08-06] decision | ADR-017 — the operation log (design)

Four decisions, each with rationale. **(1) Each entry materializes its resultant snapshot** — the
snapshot is the source of truth, the `Op` is provenance (a lab-journal line), not a recompile
recipe. The load-bearing argument, stood verbatim in the ADR *and* the `oplog.ts` header: a
replay-from-ops log would make history a **function of the installed ASE version** (geometry ops run
through ASE in the sidecar), so a dependency bump would silently rewrite old geometries — a
scientific instrument's history must not change retroactively. **(2) No WASM in Stage 2** — the op
schema is shared by serde-JSON + goldens, apply-orchestration is TS; Phase 4.5 replays materialized
snapshots. Openly corrects the unit-1c ground "the crate is separate *because* Stage 2 is WASM" →
the crate's separateness stands on std-only/MSRV, not WASM. **(3)** New optional `jobs.scene_log_json`
column (migration is 2b); `jobs.scene_json` stays the v2 snapshot, core contract untouched.
**(4)** Undo/redo is a pointer; append truncates the redo tail; **no length cap yet** — sizes
**measured** first (38-atom scene: ~2.9 KB/snapshot, ~3.5 KB/entry, ~345 KiB/100-op session), cap
deferred with numbers. Op vocabulary = one variant per Scene mutator (checklist in the ADR so 2b
finds no hole).

## [2026-08-06] session | feat(scene): operation log — pure types, pointer semantics, provenance (unit 2a)

`src/scene/oplog.ts` (pure — no store/viewer/Monaco/DB/Rust): the tagged-union `Op` (one variant per
Scene mutator + `collapse-from-text`/`restore-snapshot`; geometry ops AtomId-native), `describe(op)`
(a human lab-journal line per variant), `LogEntry {op, scene}` with the snapshot **deep-frozen**,
`SceneLog {entries, pointer}`, `append` (truncates the redo tail), `undo`/`redo`/`current`,
`logInvariant` (`-1 ≤ pointer < len`, `-1` = empty scene), and log-format-**v1** serialization
(scenes embedded as v2 via the existing `serializeScene`, versioned independently; `deserializeLog`
never throws). `oplog.test.ts` (13 tests): pointer invariants, tail-truncation, undo/redo round-trip
**identity** (same frozen snapshot `===`), `describe` per variant, serialize round-trip + rejection.
**Negative controls demonstrated red then restored:** (a) breaking tail-truncation reddens "redo
after append impossible" (len 3 ≠ 2); (b) neutering the deep-freeze reddens the immutability gate.
Sizes measured (numbers in ADR-017). Full vitest green (502 + 13), `tsc` 0, `cargo test --workspace`
green + unchanged (no Rust touched). Wiki: +ADR-017, scene.md op-log section, index.md, ROADMAP
(Stage 2 units, ephemeral→Stage 3). **Next: 2b — the store on the log.**

## [2026-08-06] ingest | Phase 3 tail — multi-fragment frontier-orbital labeling (verbatim)

Closes the item the 2a ingest flagged `Open (blocked on the architect's verbatim wording)`. Added
the Phase 3 `[ ]` **multi-fragment frontier-orbital labeling (teaching moment)** verbatim: per-fragment
HOMO/LUMO localization from MO-coefficient weights in the cached `orca_2json`, with the **honesty
caveat** (rule #11 spirit) that coefficient² weight without the overlap matrix is approximate
(Mulliken-like sans S) — label the method, or use Loewdin per-MO populations if a determiner run
shows ORCA prints them. Useful for Phase 4.5 donor/acceptor identification; explicitly not to be
gold-plated before then. Phase 3 closing note updated: two carried-forward `[ ]` items now, not one.

## [2026-08-06] session | feat(scene): the store folds over the operation log — deep undo/redo, dispatch-only (unit 2b)

The Zustand store (`store.ts`) now **folds over the log**: `scene` is DERIVED (`scene ===
current(log)`, always), there is **no `setScene`** — the only doors are `commit(op, resultScene)`
and `installLog(log)` (+ pointer moves `undo`/`redo`/`jumpTo`). Convenience mutators funnel through
`commit`; `seedScene(scene, source)` is a thin `installLog` of a seeded log. The mutator-bypasses-
the-log defect is impossible by construction — **control (a)** asserts `scene === current(log)`
after every action and reddens when one action writes `scene` without the log. `oplog.ts` gained
`goto(pointer)` and `SnapshotSource` = `new-iteration | text-adopt | library`. `restore.ts`
gained **`restoreSceneLog`**: the persisted log is honoured only if its current snapshot equals the
co-written `scene_json`; a mismatch **rejects the log (named reason) and honours the snapshot**
(**control (b)** — the map-minting contract, unit 1e, stays authoritative). Consumers moved to the
new API: `NewJobScreen` (sync → `seedScene`/`collapseFromText`; restore → `restoreSceneLog` +
`installLog`; edit apply → `commit`; deep undo replaces one-step `preEditScene`), `EditPanel`
(hands up the typed `Op`), `JobDetailScreen` (conformer via), plus a read-only **`HistoryPanel`**
(`describe()` list, click = pointer jump, Undo/Redo + Ctrl/Cmd+Z). The **collapse↔undo loop is
dead** — collapse is a logged op, undo re-injects, no second collapse (**control (c)**, a sync
integration test). Three `scene: null` consumers defined + tested. All three negative controls
demonstrated red then restored. vitest 511 green, `tsc` 0. Persistence lands in the next commit.

## [2026-08-06] session | feat(jobs): persist scene_log_json; New iteration restores the history (unit 2b)

Schema **v11** — guarded `ALTER TABLE jobs ADD COLUMN scene_log_json TEXT` (nullable, additive,
like v6/v7/v10). `create_job` writes `scene_log_json` **co-written with `scene_json` in the same
INSERT** — atomic by construction, so a restore can cross-check the two (the log↔snapshot check of
`restoreSceneLog`, this session's control (b)). The `Job` struct + `Job::COLUMNS` + `from_row` carry
it as the 13th column, so `get_job`/`list_jobs` return it to the UI for New iteration; the TS `Job`
type mirrors it. A GOAT search job writes NULL (no editing history → seeds fresh on iterate). Tests:
`create_persists_and_reloads_scene_log_json` (round-trip + NULL for a bare job) and
`migrate_v10_to_v11_adds_scene_log_json_and_preserves_jobs` (preservation). The 1e map-minting
contract is **untouched** — minting still reads `scene_json`, as before. `cargo test --workspace`
green. Wiki: `tauri-core.md` (v11 migration + cross-check), ADR-017 amendment, `scene.md`. **Stage 2
units 2a+2b done; next: 2c (dumb renderer + AtomId pipeline).**

## [2026-08-06] session | feat(ui): editor workspace — viewer-first right dock, fullscreen as workspace (unit 2b-ux)

Pure-layout unit: the New Job editor becomes **viewer-first**. The geometry panels moved from a stack
**below** the viewer into a **right dock** (`src/scene/EditorDock.tsx`) — a thin always-visible icon
rail that expands per section: Selection & Measure · Edit · Fragments · Constraints · History ·
Actions (use-order), each toggling independently (session-only state; fresh screen starts with
Fragments open so Add Fragment is discoverable). The **Add-Fragment palette** (reagents/import/SMILES/
library) moved out of the top accordion into the Fragments section, so it's reachable **inside
fullscreen** — the same one dock is used in both modes (fullscreen = workspace). Removed the
fullscreen-only rail-collapse (closing sections is the clean canvas now). **Resize:** reuses the one
existing mechanism — `MoleculeViewer`'s `ResizeObserver` fires `viewer.resize()` on the container box
change (dock toggle flips the split ratio + changes dock width → viewer box changes), same path as
the split-panel resize and fullscreen; **no per-toggle resize call**. **Zero model changes** —
`git diff` touches only `NewJobScreen.tsx`, `app.css`, `+EditorDock.tsx`;
`scene`/`oplog`/`store`/`constraints` untouched (verified). tsc 0; vitest 511 green (no drop); cargo
unchanged (no Rust). Wiki: +`modules/editor-ui.md` (viewer-first principle, the one resize
mechanism, where future panels go — 2c2 index-space labels, Phase 4.5 reaction setup), index.md,
ROADMAP 2b-ux `[x]`. **Window verification (checklist) is the author's — WebKitGTK layout can't be
tested headless.** Next: 2c (dumb renderer + AtomId pipeline).

## [2026-08-06] session | fix(xtb): completion post-condition anchored on results, not the binary's last words (2b-hotfix)

The xTB pre-optimize completion gate was wrong in **both** directions (measured, xtb 6.6.1
`builduser@buildhost`). **False negative:** the gate `tail(xtb.out,30).contains("normal termination")`
missed the marker — it is stderr-only and sits **41 lines from the end**, buried under the post-opt
Wiberg bond-order table — so a run that finished with a **correct** geometry was rejected (the
author's dexketoprofen+BH₄ case). **Latent false positive:** a `--cycles 2` run prints
`*** FAILED TO CONVERGE GEOMETRY OPTIMIZATION IN 2 ITERATIONS ***`, yet exits **0**, writes
`.xtboptok` + a **non-optimized** `xtbopt.xyz`, AND prints `normal termination` — so the same gate
would silently apply a non-optimized geometry to a multi-hour ORCA run. **Fix:** `classify_completion`
(pure, tested) anchors on **results in our terms** — an optimized geometry present + parseable and
**no `FAILED TO CONVERGE`** (scanned over the whole size-capped log, because that line can sit
hundreds of lines from the end). `normal termination` and the exit code become **named diagnostics,
never gates**; the exit-code gate in the poll loop is removed (capture-only). Non-convergence → a
named error quoting the FAILED line + N iterations; artifacts **kept** (rule #3) so the user can
inspect — the geometry is not applied. **Real fixtures** (`tests/fixtures/xtb_{success_dexketoprofen_bh4,
fail_cycles2}.out`), classifier tested on both. **Negative controls:** (a) the clean fixture through
the OLD gate reproduces the false negative (documents why it was replaced); (b) removing the FAILED
scan makes the classifier accept the non-converged fixture — shown red, then restored. GOAT (`! XTB
GOAT` via ORCA) does **not** share this gate — untouched. Spawn/killpg/isolation (`debugging/004`)
untouched; cancel/timeout stay gates. `cargo test --workspace` green (+6 xtb tests, 182 lib);
frontend unchanged. Wiki: `orca/xtb.md` (Pattern-2 correction — completion-signals table),
`+debugging/012`, `tauri-core.md`, index.md.

## [2026-08-06] session | feat(viewer): 3Dmol becomes a dumb renderer — AtomId↔viewer table (unit 2c1)
3Dmol is now fed geometry **and** an `AtomId↔viewer-index` table built by the SAME function, and
returns picks as `AtomId` (ADR-010 I1 / ADR-011). **The table cannot drift from the geometry:**
`buildViewerFeed(scene)` (`src/scene/scene.ts`) returns `{ xyz, table }` from one pass over
`allAtoms(scene)` — the model `addModel` draws and the table the pick handler resolves through are one
object, not two states kept in sync. `MoleculeViewer` sets `viewerTableRef` in the same effect run
that builds the model and **clears it on every non-scene path** (xyz/orbital/animation) + unmount, so
a stale table can never resolve a pick. **Picking → `AtomId`:** `onAtomPick(pick: AtomPick)` with
`AtomPick = { atomId, viewerIndex }`; `viewerIndex` is the raw `atom.index`, **viewer space,
diagnostics only** — never an app id. Post-condition (rule #9): an unresolvable click emits **nothing**
rather than a guessed id. **Overlay fed through the table:** halo/mask resolve *global index → AtomId →
atom*; the number a label shows is `table.viewerIndexOf(id)` (the same positional value, but sourced
from the table, not a loop-counter coincidence). Value unchanged — renaming the index *space* in the UI
is 2c2. **2c1→2c2 seam:** consumers (`selection`/`measure`/`edit-plan`/`constraints`) stay positional
behind ONE named adapter in `NewJobScreen.onAtomPick` (`buildViewerAtomTable(scene).viewerIndexOf`,
`TODO(2c2)`). **Reads-from-3Dmol audit (ADR-011):** `3dmol` imported in one file; only two reads —
`pngURI()` (output from what was drawn, sanctioned) and the app-owned frozen-topology animation model
(`selectedAtoms({})`, unit 3.14) — both named in `editor-ui.md`; `getModel()` in `selection-panel.ts`
is Monaco (false positive). **Tests:** `viewer-atom-table.test.ts` — bijection + order-consistency,
byte-identical to `mergeToXyz`, same-physical-atom-after-remove; **negative controls** (a) a stale
table (not rebuilt with the geometry) resolves a drawn slot to a now-removed atom, (b) a reversed table
on **asymmetric** fragments (3+2) returns a **cross-fragment** atom — both demonstrated bites (the live
reversal also drove the positive bijection/identity tests red: `expected 5 to be 0`, `expected 1 to be
3`; restored). `tsc`/vitest (517) green; no Rust touched. Also **docs-rider (Part F, Pattern 1 #7):**
the xtb-tail absence claim — `normal termination` declared absent on `tail -35` + case-sensitive grep
(a **presence-only** tool) without a full-file search; the line was at 1492/1533, and a build-teardown
theory was built on the false absence. Rule: **an absence claim needs a whole-file, case-insensitive
search or it is not measured.** Cross-ref `debugging/012`. Wiki: `editor-ui.md` (feed contract, seam,
audit), `visualization.md` (feed source, picking, overlay), `adr-011`, `manual-sources.md` Part F,
ROADMAP 2c1 → [x]. **Next:** 2c2 — pipeline onto `AtomId`, the `selectionSurvives` removal dividend.

## [2026-08-06] session | feat(scene): selection/measure/edit-plan/constraints key on AtomId; drop the 2c1 seam (unit 2c2)
The geometry editor's selection/measurement layer moved from positional global indices to stable
**`AtomId`s** (ADR-010). **Resolver primitive:** `globalIndexOfAtom` / `atomIdAtIndex` in `scene.ts` —
the bijection AtomId↔global-index over `allAtoms` order, independent of the viewer's `ViewerAtomTable`
(that names indices for 3Dmol; these for the core). **On AtomId now:** `toggleAtom(AtomId[], AtomId)`;
`measureSelection(scene, AtomId[])` (resolves → the unchanged index math); `planEdit(scene, AtomId[])`
(resolves ONCE on entry); `constraintFromSelection(scene, AtomId[], value?)` (resolves at build time);
`describeAtomById`; the viewer `selection` prop + halo/numbers. **Stays positional at its emit seam
(ADR-010 correction i — order matters in exactly one place):** the **ASE mask** (`EditPlan.indices`/
`mask`/`cut`/`within`) and the **`%geom` constraint** (a `Constraint`'s atoms are ORCA 0-based
indices, frozen into the text). `AtomId → index` conversion happens at **exactly two seams**, both via
`globalIndexOfAtom`. **The dividend:** `selectionSurvives` + `validateSelection` are **removed**;
`filterSelection` keeps every id still in the scene, so removing an UNRELATED fragment leaves the
selection intact (the 2.5.2b bug — a kept index silently re-pointing boron→hydrogen — is now
**structurally impossible**). A conscious behaviour change (old clearing was correct for the positional
space). **Journal (Variant A):** `describe(op)` stays pure/AtomId-native; new `describeInScene(op,
entry.scene)` renders `set-internal` atoms by the global index they held in that snapshot (count+order
preserved ⇒ always resolves; no `[removed]` case). **UI labels the index space per panel** (ADR-010
correction iii extended to the whole UI): AtomInspector "global index M", ConstraintPanel "ORCA
0-based index", the 3D view the viewer index. The **2c1→2c2 adapter is deleted** (`grep TEMPORARY
SEAM` / `selectionSurvives` / `validateSelection` → empty, full-file). **Tests:** every 2c2 assertion
is built on the canonical **divergent fixture** `borohydrideAfterWaterRemoved()` (water+BH₄⁻ then water
removed → AtomId 3 = boron at global 0), because on a fresh scene id==index would be green under broken
code too; `idsFor(scene, ...)` helper resolves indices→ids so the existing math tests read unchanged.
**Negative controls, each demonstrated red then reverted:** (a) filterSelection clear-all → "unrelated
removal keeps selection" red (`[] vs [3,4]`); (b) measure by stale index → boron read as H (`3 vs 0`);
(c) constraint lays raw AtomId → `[3,4] vs [0,1]`; (d) journal joins raw ids → `Set angle 3-4-5 vs
0-1-2`. `tsc` 0 errors; vitest 518 green; no Rust/sidecar touched. Wiki: `scene.md` (selection/measure/
edit-plan/constraints/oplog + Index-space resolver + the removed guards), `editor-ui.md` (whole-UI
space labelling + seam gone), `visualization.md` (AtomId selection + halo direct-resolve),
`adr-017` (describeInScene amendment), ROADMAP 2c2 → [x]. **Next:** 2d — the Monaco xyz block becomes a
read-only projection (coordinate hand-editing moves to "paste xyz → import a fragment").

## [2026-08-06] session | feat(editor): xyz block is a read-only Scene projection; import-as-fragment + replace-input; close Stage 2 (unit 2d)
The `* xyz … *` coordinate block in Monaco became a **generated read-only projection of the Scene**
(Variant C; ADR-010 authority split: input text owns chemistry `!`/`%`, the Scene owns geometry).
**Mechanism (review point 1): a guard+revert at the React content layer, not a Monaco read-only
range** — the Monaco→Scene debounce effect in `NewJobScreen`, on a diverged/deleted block with a
scene present, calls `setContent((c) => injectSceneIntoInput(c, current))` to restore the projection
(keeping the `!`/`%` edits), and Scene is never touched. Chosen over a Monaco range because it reuses
the ONE existing locator (`sceneFromOrcaInput`/`injectSceneIntoInput` — no second `* xyz` finder,
invariant 3), the editor stays a plain controlled component (the controlled `value` makes the revert
authoritative, no editor-instance race), and it is the minimal wiring the unit called for. **Where the
Monaco→Scene collapse vanished (review point 2):** the effect's old `diverged → collapseFromText(...)`
branch is gone, replaced by the revert; grep-verified full-file that `collapseFromText` has **no
occurrences in `src/`** and `resetNotice`/`dismissResetNotice` are gone. The **`collapse-from-text`
op type stays** — union member, `OP_TYPES`, `describe`, and the `deserialize` case — marked **legacy**
(«kept for deserialization, never emitted»), so pre-2d `scene_log_json` still opens (persist not
broken); the store's `collapseFromText` mutator + the `resetNotice` machinery are **removed** (dead
once the collapse path is gone), which is what makes "never emitted" literally true.

**Two doors preserve coordinate hand-editing** (ROADMAP requires the capability survive): **Import xyz
as fragment** (Fragments dock — a textarea → `sceneFromXyz` → `addFragmentToScene`; soft failure on
unparseable xyz) and **Replace input** (a button by *Save to Library* → confirm → *Unlock editor*
[suppresses the revert for one round] → paste → *Adopt input* = `seedScene(text-adopt)`, a fresh log;
**the block re-locks after adoption — review point 3: a one-shot escape, not a permanent unlock**).
Whole-buffer replacers that already existed (template pick, builder Generate) route through the SAME
conscious re-adopt (`adoptWholeInput` → `seedScene(text-adopt)`) so they aren't caught by the revert
(this was a real trap: without it, picking a template while a scene existed would revert the template's
block to the old scene).

**Gates.** Pure (vitest): **(c1)** import xyz builds an `add-fragment` op adding exactly N atoms with
count+order preserved — broke it (reversed the parser's atom order) → red, reverted; **(c2)** a
Replace re-seed installs a FRESH `text-adopt` log with no lineage leak — broke it (`seedScene` appends
to the live log instead of `emptyLog()`) → red, reverted; plus a read-only-projection decision test
(revert on divergence / keep on a keyword edit / seed on an empty scene) — the pure core of m1/m2.
`tsc` clean, **vitest 516 green**, `vite build` clean. **Manual gates m1–m4 verified LIVE** in the
running app (the WebKitGTK dev server at :1420, driven via Chrome — the real React↔Monaco↔Scene effect
wiring, not jsdom): **m1** a coordinate hand-edit reverts to the projection and the scene stays
2-fragment (BH₄⁻ + H₂O), notice shown; **m2** `! HF TightOpt` is accepted and persists, block + scene
untouched; **m3** Paste xyz adds an HF fragment to 3D + sidebar, journal shows "Add fragment Pasted
xyz"; **m4** Replace input → confirm → unlock → paste N₂ → Adopt → fresh 1-fragment scene, journal
reset to one `text-adopt` entry, and a coordinate edit on the new block reverts (block read-only
again). **m5** (clone a legacy job carrying a `collapse-from-text` log) needs the Tauri/SQLite backend
(unreachable from Chrome); 2d only *kept* the deserialize/describe path — grep-verified + the
`oplog.test.ts` legacy describe case + `restore.test.ts` all green — so it is unit-covered; the
end-to-end clone is left for the author's real backend.

**Also closed the 2c2 wiki debt** (the 2c2 close ritual missed it): `frontend.md` lines describing
`selectionSurvives`/`validateSelection` as the live pick-list mechanism now name **`filterSelection`**
(per-atom, id-based, the preservation dividend). **Wiki:** `scene.md` (Scene↔Monaco sync rewritten —
read-only projection, collapse branch gone, the two doors, the jsdom-vs-manual split; the identity
boundary marked removed; op-vocabulary `collapse-from-text` → legacy), `frontend.md` (InputEditor
read-only block + Paste xyz source + Replace input door + 2c2 debt fix), `editor-ui.md` (status → 2d;
the authority-split / read-only-projection principle; Replace input as the named escape), ADR-017
(unit-2d amendment: `collapse-from-text` is legacy), `app.css` (`.paste-xyz` textarea), ROADMAP (2d
→ [x], **Stage 2 marked COMPLETE**, Stage 3 next). **Next: Stage 3 — operations over the core (rigid
drag + the ephemeral layer; the first item is the Scene→Rust/WASM move).**

## [2026-08-06] session | feat(editor): rigid-body fragment drag — ephemeral layer, one op on release (Phase 4.2 Stage 3, unit 3.1)
The first CONTINUOUS interaction in the app, and the first Stage-3 op. In **Move mode** (a toggle in the
Edit dock section) a mouse-drag starting on an atom grabs that atom's whole fragment and moves it
rigidly **in the plane of the screen at 60fps** — a **viewer-only ephemeral overlay**; the Scene, text
and log are untouched until mouseup, when exactly ONE `translate-fragment` op commits the TOTAL delta
(ADR-010: 60fps motion not logged, one Undo).

**CORRECTION (append-only, per the schema — the prior entry is not edited):** the 2d session entry
ended "Next: Stage 3 … the first item is the Scene→Rust/WASM move." That was a **misread**. Stage 3 =
**operations over the core** (rigid drag first); **Scene→Rust/WASM is ADR-011, deferred behind a
spike, and is NOT what Stage 3 means**. ROADMAP had the same drift in its Stage-1 recap line
("Stage 3 (Scene→Rust/WASM)") — fixed this unit.

**PROBE FIRST (rule #10), the single real unknown — screen→world unproject.** Before any wiring I
measured, in the real running viewer, whether 3Dmol 2.5.5 gives a reliable pixel→world delta in the
screen plane. **PASS:** `viewer.screenOffsetToModel(dxPx, dyPx, modelz)` returns a world **delta** (the
same call 3Dmol uses for its own pan); `viewer.modelToScreen` returns **page** pixels (hit-test vs
`event.pageX/Y`). Round-trip accuracy: the default `modelz` (scene-centre depth) tracks to ~0.13 px
frontal but **leads the cursor +6% at a rotated camera** (perspective depth effect — direction always
exact, only the magnitude of which depth-plane sticks to the cursor). Passing the **grabbed atom's
depth** — `new Vector3(atom).applyMatrix4(viewer.modelGroup.matrixWorld).z`, which is literally the
first line of `modelToScreen`'s own chain (so it reuses the documented projection input, NOT invented
camera math — a wrong first guess blew up to −350%) — makes tracking **pixel-exact (0 px)** across 3
atoms × 4 deltas at frontal AND 45°y+30°x. Recorded in `wiki/debugging/013`. Because the probe passed,
I proceeded; had it failed I would have stopped for the axis-constraint alternative.

**Where the ephemeral state lives (NOT in the store).** The store gained a committable
`translateFragment(id, dx, dy, dz)` → `translateFragmentInScene` (new pure scene mutator) →
`translate-fragment` op → `commit` (the one door, like every mutator). Everything else is **viewer-local**:
`MoleculeViewer`'s drag effect holds the pointer session in closures/refs; the frozen-topology
coordinate-update path (unit 3.14) moves the grabbed fragment's live model atoms per frame
(`applySceneStyle` — the styling extracted from the model effect so a drag frame keeps the per-fragment
palette — nulls cached geometry so sticks redraw; no `addModel`, no re-perception, no `zoomTo`). `commit`
is called **exactly once, in `pointerup`** — never in `pointermove`. The pure accumulate/commit logic is
`src/viewer/fragment-drag.ts` (`makeDragController`), split out so a simulated (begin, move×N, end)
sequence is unit-tested without jsdom (the `syncMonacoToScene` pattern from 2d).

**Camera suppression (review point 3).** 3Dmol binds `mousedown` on its canvas (`glDOM`). The Move
effect binds `mousedown` on the **container in the CAPTURE phase**, so an atom-grab runs first and
`stopPropagation()`s → 3Dmol's canvas mousedown never fires and its rotate (gated on state that mousedown
sets) is suppressed for the drag. Empty-space drag isn't stopped → 3Dmol rotates; a click (< 3 px)
re-emits the pick through `viewerTableRef` (we intercepted 3Dmol's own click). `mousemove`/`mouseup`
are on `window` for the drag's life; a mid-drag Move-off / unmount cancels via a tracked cleanup ref.

**Gates.** Pure (vitest): **(c1)** `translateFragment` commits ONE op, shifts every mover atom by the
SAME delta, internal pairwise distances + count/order/AtomId invariant, other fragments untouched — broke
it (shift only atom 0) → red, reverted; **(c2)** a simulated (down, move×3, up) yields exactly ONE log
entry with the SUMMED delta — broke it (commit per move) → log grows → red, reverted; **(c3)** the Scene
snapshot is `===` its pre-drag value during the drag (before up) — the same per-move-commit break reddens
it too. `tsc` clean, **vitest 521 green**, `vite build` clean. **Manual gates m1–m4 verified LIVE** (the
WebKitGTK dev server at :1420 via Chrome — the real 3Dmol + mouse + React wiring, not jsdom): **m1** in
Move mode, dragging the H₂O fragment moved all 3 of its atoms by the SAME delta (4.284, −3.122, 0) Å in
the screen plane while BH₄⁻ stayed put and the camera did NOT rotate; **m2** release logged ONE history
entry "Move H₂O by (4.284, −3.122, 0) Å", one Undo restored it, the Monaco block updated once (on
release); **m3** with Move off, a viewer drag rotated the camera (both fragments together, block
unchanged — a view change) and a click picked (halo appeared); **m4** a drag frame (setStyle+render) cost
**~1.3 ms at 38 atoms** (BH₄ + H₂O + 5×CH₃OH), ~12× under the 16.7 ms/60fps budget, essentially flat vs 8
atoms (fixed per-frame overhead dominates) — no stutter; the definitive WebKitGTK-GPU number is worth a
confirming glance in the real window but the headroom makes a regression very unlikely, and the
frozen-topology precedent agrees.

**Wiki (same commit):** `debugging/013` (the probe — API + accuracy + camera binding), `visualization.md`
(Move-mode/ephemeral-drag section: camera suppression, frozen-topology reuse, Scene-untouched
post-condition), `editor-ui.md` (status → 3.1; Move mode as a coarse-placement affordance — precision is
the editor/constraints, division of labour), `scene.md` (store `translateFragment` mutator + the pure
`translateFragmentInScene`), `index.md` (+debugging/013, page count 71), ROADMAP (rigid-drag → [x],
"Undo deeper than one step" → [x] (done in 2b), Stage-1 recap line de-drifted, Stage 3 begun). **Not
touched (ADR-011 deferred):** no Scene→Rust/WASM, no renderer move; no vdW-clash warning (next Stage-3
unit); no axis-constraint drag; the pure scene/oplog core, Rust, sidecar all untouched. **Next: Stage 3
continues — vdW-overlap detection after a move, then fragment rotation about the approach axis.**

## [2026-08-06] session | feat(editor): inter-fragment vdW clash warning — Bondi/Alvarez radii, constraint pairs excluded, tunable heuristic threshold (Phase 4.2 Stage 3, unit 3.2)
The second Stage-3 unit — a **post-move** steric check (not a continuous interaction, far less risky
than the drag). After any geometry change, atoms of **different fragments** closer than `k·(rᵢ+rⱼ)` of
their van der Waals sum are flagged as a **WARNING, never a block** (the drag is coarse; a close contact
at setup is expected and refined in the editor).

**Cited radii, one physical table (`src/scene/vdw-radii.ts`), UNDETERMINED not guessed (rules #10/#11).**
Bondi 1964 (main group), Mantina 2009 (main-group gaps — **B 1.92 Å**, the BH₄⁻ centre), Alvarez 2013
(transition metals — **Pd 2.10 / Pt 2.13**, the cross-coupling metals Bondi lacks). Each radius is
source-attributed (`VDW_SOURCE`, a test asserts every radius is cited and vice-versa). An element in
**none** of the sources → `vdwRadius` returns `undefined` = **UNDETERMINED**: the clash check skips that
pair and surfaces it, never radius 0 or a guess. **Deliberately SEPARATE from `viewer/highlight.ts`'s
`VDW_RADII`** (which mirrors 3Dmol's radii for visual halo sizing, 1.5 Å fallback, drift-guarded): the
two have different masters (literature vs 3Dmol) and different missing-element semantics (UNDETERMINED
vs fallback), documented so a future lint doesn't merge them.

**Pure `detectClashes(scene, k, activeConstraints): ClashReport` (`src/scene/clash.ts`).** Inter-fragment
pairs only (a rigid fragment can't self-clash; testing intra would flag its own bonds); reuses
`measure.ts` `distance` (no second distance impl); **excludes pairs carrying an active distance
constraint** — an intentional forming bond — read via the SAME `constraints.ts` `fromOrcaIndex` (no
second constraint reader); UNDETERMINED pairs reported apart from clashes. **Where the state lives (NOT
in Scene):** clash state is DERIVED in `NewJobScreen` (`useMemo` over `scene`/`k`/`content`, stable ref
so the viewer overlay only redraws when the clash set changes); `k` is app-owned session state
(grep-confirmed: `clashK` absent from `src/scene/`). The clashing atoms get a distinct **magenta danger
glow** in `MoleculeViewer` (`clashHighlight`, `CLASH_COLOR #ff2d95`) — apart from the chartreuse
selection halo (wireframe) and edit mask (translucent) in BOTH hue and form (no CPK element is magenta;
deliberately not the halo colour — that collision bit once with Pd/Pt). UNDETERMINED elements get a
**quiet, separate** notice; `k` is a **labeled heuristic slider** ("vdW overlap threshold — heuristic,
not a physical cutoff", default 0.65) in the Edit dock section, like the IR FWHM slider.

**Gates.** Pure (vitest, each demonstrated red then reverted): **(c1)** a real inter-fragment clash is
flagged for the RIGHT pair — swap `<`/`>` → red; **(c2)** well-separated fragments are clean and
INTRA/own-bonds are never flagged — remove the inter-fragment guard → a fragment's C–C bond screams →
red; **(c3)** an UNDETERMINED element (W) skips the pair + flags it, never radius 0 — `?? 0` on a missing
radius → false clash → red; **(c4, the mission gate)** a distance-constrained pair is NOT a clash even at
1.2 Å — ignore constraints → the intentional contact screams → red; **(c5)** monotone in k (proves k is
used, not hardcoded). Plus a `vdw-radii.test.ts` (cited-coverage + B/Pd/Pt present + W/Nd UNDETERMINED).
`tsc` clean, **vitest 530 green**, `vite build` implied clean.

**Manual gates m1–m5 verified LIVE** (WebKitGTK dev server at :1420 via Chrome — real 3Dmol + React):
**m1** dragging BH₄ into H₂O raised "12 steric clashes" + a magenta glow on the overlapping atoms
(clearly distinct from the chartreuse halo), Run NOT blocked; **m2** pulling them apart cleared the
banner and the glow (derived, auto-updates); **m3** raising k 0.65→0.90 on a grazing geometry (unchanged
coords) raised the count 0→3 — k really applied, label reads as a heuristic; **m4 — THE MISSION VERDICT:**
formaldehyde + BH₄⁻ dragged to a textbook Bürgi–Dunitz distance (**C···B = 2.83 Å**) shows **0 clashes at
default k=0.65** — NO false alarm on the legitimate reactive approach even WITHOUT a constraint (raw
threshold `k·(r_C+r_B)` = 2.35 Å < 2.83); then a distance constraint on the forming pair (written
`%geom Constraints {B 4 1 C}`) at k=0.90 dropped the flagged count **6→5** — the intentional pair excluded
while genuine peripheral clashes remain; **m5** a tungsten atom (no cited radius) → a **quiet separate
notice** "Couldn't steric-check W: no cited van der Waals radius, so pairs touching it were skipped (not
guessed)", NOT a clash, NO crash. (Positioning via remote-control mouse was fiddly on rotated/overlapping
views — the drag itself is fine, m1 proved it; a fresh frontal view made the BD placement reliable.)

**Wiki (same commit):** `scene.md` (`clash.ts` + `vdw-radii.ts` files + a dedicated section: four
decisions, the two-tables-separate rationale, derived-state + app-owned k), `chemistry/vdw-steric.md`
(NEW, Ukrainian — vdW radii, why the threshold is a heuristic, why a reactive contact isn't a clash,
cross-ref burgi-dunitz), `editor-ui.md` (status → 3.2; clash-warn affordance, labeled k-slider, distinct
glow), `visualization.md` (the danger-glow overlay + prop list), `index.md` (+chemistry/vdw-steric,
count 72), ROADMAP (vdW-overlap → [x]), `app.css` (clash banner/notice/slider). **Not touched:** no
sidecar computation (ratified TS), no bond-perception change, no intra-fragment, no axial rotation (next
unit), no Run/Apply block. **Next: Stage 3 — rotation of a fragment about its approach axis (an `Op`
over the mask).**

## [2026-08-06] session | feat(editor): rigid fragment rotation about a picked approach axis (Phase 4.2 Stage 3, unit 3.3)
The third Stage-3 unit — a **rigid whole-fragment rotation** about the reaction's approach axis. Two
picked atoms ARE the axis: **P** (first pick, the pivot on the rotating fragment) and **Q** (second,
the direction, typically the substrate contact atom). A **numeric** angle drives it (reproducible; the
journal reads "Rotate BH₄⁻ 30° about O→C" — spin-drag deferred), with a live viewer-only preview and
one op on Apply.

**Architectural place — TS pure, a sibling of (NOT routed through) the sidecar.** A rigid transform
changes no internal coordinate, so there is nothing for ASE to compute: `rotateFragment(fragment,
axisDir, angleRad, pivot)` is Rodrigues' formula in `scene.ts`, parallel to `translateFragment`;
`rotateFragmentInScene(scene, fragmentId, [P,Q], angleRad)` is the scene mutator, `rotationAxis(scene,
P, Q)` the shared degeneracy test (`null` on P≡Q / absent → the mutator no-ops same-ref AND the UI
disables Apply, so they never disagree). This is a hard boundary from the intra-fragment set-internal
edit (bond-graph split + coordinate solve — the sidecar's job, emits `replace-fragment-atoms`), now
recorded as its own section in `scene.md`. The rotation lives in the Edit section as a **sibling of
`EditPanel`** (`RotatePanel.tsx`): pick + value + preview + apply, but pure TS.

**The op stores the two axis ATOMS, not the derived vector (ADR-017 amendment).** Unlike
`translate-fragment` (a raw delta), the approach axis IS two atoms by definition (ADR-007), so
`rotate-fragment {fragmentId, name, axisAtoms:[P,Q], angleRad}` keeps the journal legible in the
reaction's terms. The resolve is safe under decision 1: the op applies to its **own** snapshot, where
P,Q are present by construction. `describeInScene` was extended (Variant A) to render P→Q by **global
index** (`about 0→1`) while `describe` stays AtomId-native (`about 3→4`).

**Preview = the frozen-topology coordinate-update path, driven by a prop (NOT the previewScene
model-rebuild).** The set-internal preview passes a whole `previewScene` and the model effect rebuilds
(removeAllModels/addModel) — fine for an on-demand button, but a **live slider** would re-perceive
bonds every tick and **flicker an inter-fragment stick** in/out exactly in the reactive-approach setup
this feature is for. So rotation reuses the SAME path the Move-mode drag (3.1) and mode animation use:
a new `ephemeralScene?: Scene|null` prop → a viewer effect sets the live model atoms' coords +
`applySceneStyle` (sticks redraw, no `addModel`, no re-perception, no `zoomTo`), restoring committed
coords on `null`. The axis is drawn (`axisHighlight?: [AtomId,AtomId]`) as an extended cylinder through
P→Q; **both endpoints are fixed points** of the rotation (P is the pivot, Q lies on the line), so
drawing it — and the P/Q selection halos — at committed coords stays correct all through the preview.
Rotate-state (angle/ephemeral/axis) is **app-owned** (angle in `RotatePanel`, ephemeral+axis in
`NewJobScreen`), never in the Scene (grep-confirmed).

**Gates.** Pure (vitest, `rotate.test.ts` + additions to `store.test.ts`/`oplog.test.ts`): **c1** RIGID
(internal pairwise distances invariant, pivot P fixed, other fragments untouched, ids/order invariant);
**c2** RODRIGUES (closed-form CCW rot(π/2), identity at 0/2π, round-trip rot(θ)∘rot(−θ), point-on-axis
fixed, non-unit-axis normalized, zero-axis throws); **c3** ONE op on Apply (a pure preview recompute
leaves the store `===`; Apply appends exactly one with the FINAL angle); **c4** axis P→Q / pivot P (P and
on-axis M fixed, off-axis atoms actually moved); **c5** DEGENERATE (`rotationAxis` null on P≡Q/absent,
mutator same-ref no-op, store no-op). **Bites demonstrated live and reverted:** a non-rigid break
(`rx*1.5`) reddened c1/c2/c4; a Rodrigues **sign flip** (`+s`→`−s`) passed round-trip/identity/rigidity
(all sign-invariant) but reddened the **closed-form CCW** assertion — proving that exact-value check
earns its place. `tsc` clean, **vitest 540 green** (was 530), `vite build` clean. Grep gates: rotation
math is pure TS in `scene.ts` (no sidecar/fetch in `RotatePanel`), `rotate-fragment` emitted only in
`store.rotateFragment` (one door), rotate-state absent from `src/scene/`.

**Not yet done — manual gates m1–m5 (pick→turn→Apply live in the Tauri/WebKitGTK window).** The pure
logic + build are green; the live interaction gates are **pending** and flagged in ROADMAP — offered to
the author to drive via the dev server, or for the author to run.

**Wiki (same commit):** `scene.md` (the three `scene.ts` functions + `rotate-fragment` op + store
mutator + a new "rigid transforms are TS; internal edits are the sidecar" split section + `RotatePanel`
in Files), `visualization.md` (a new section — `ephemeralScene` frozen-topology preview, why not a
model rebuild, `axisHighlight` axis cylinder + why committed coords stay correct), `editor-ui.md`
(status → 3.3; the "Rotate about axis" affordance), ADR-017 (unit-3.3 amendment + table row), ROADMAP
(rotation → [x], m1–m5 flagged). **Not touched:** no sidecar computation (ratified TS), no 2.5.3
torsion / rotatable-mask, no spin-drag, no Move-drag change; Rust/sidecar untouched. **Next: Stage 3 —
ring torsions (the last unit of Stage 3).**

## [2026-08-06] session | feat(editor): rotate-axis overlay toggle — axis or distance, never both (Phase 4.2 Stage 3, unit 3.3b)
A small polish of 3.3 from the real window. **The problem:** while the Rotate panel holds an axis
`[P, Q]`, the viewer drew BOTH the extended axis cylinder (3.3) AND the measurement distance line on the
**same two atoms** — two overlapping greenish objects of different length that read as "the line is
wrong". **A2 (ratified):** while the panel has an axis, draw **exactly ONE** overlay for that pair, chosen
by a toggle; in axis mode the Å number stays (on the axis midpoint) so length always reads, but no second
line.

**The decision is a pure function, tested apart from the jsdom-less viewer.** `viewer/rotate-overlay.ts`:
`chooseRotateOverlay(hasAxis, overlay) → {axis, measure}` — no axis → `{false, true}` (the measure tool
outside Rotate is **untouched**, both modes identical); axis + "axis" → `{true, false}` (cylinder + label,
no measure line); axis + "distance" → `{false, true}` (measure line + label, no cylinder). The
post-condition callers lean on: **`axis && measure` is never both true**. The viewer reads the plan and
gates the two draws; in axis mode it adds the Å label at `midpoint(P,Q)`.

**The Å number is the measure distance — one source, no second computation.** `rotationAxisValueLabel`
reuses `measureSelection`/`formatMeasurementValue`, the SAME calls `drawMeasurement` makes; both render it
through a new shared `drawValueLabel` (extracted so the distance line and the axis midpoint show the value
in one style). So the number reads identically whichever overlay is up (grep-shown).

**State is app-owned, resets with the axis.** `rotateOverlay: "axis" | "distance"` lives in `NewJobScreen`
(next to `rotateAxis`/`clashK` — NOT in the Scene; grep-confirmed absent from `src/scene/`), default
"axis", reset to "axis" whenever the axis pair changes/clears (a `[rotateAxis]` effect) and on Cancel.
`RotatePanel` renders a small segmented **Axis ⇄ Distance** toggle that only flips this render choice — it
does not touch `rotateAxis`/`selection`/the rotation op (3.3 math untouched).

**Gates.** Pure (`viewer/rotate-overlay.test.ts`): **c1** default is "axis" + flip is its own inverse;
**c2** the decision — outside Rotate measure-as-is in either mode, with an axis exactly one overlay, and
**never both** (broke it to `{axis:true, measure:true}` → the invariant test reddened, reverted). `tsc`
clean, **vitest 545 green** (was 540, +5), `vite build` clean. **Manual gates m1–m5 (WebKitGTK) pending
live verification** — flagged in ROADMAP.

**Wiki (same commit):** `visualization.md` (the `rotateOverlay` bullet — one overlay at a time, the pure
decision, Å from measure), `editor-ui.md` (the toggle under the Rotate affordance), ROADMAP (3.3b note),
this entry. **Not touched:** the 3.3 rotation math/op, the ephemeral preview path, the measurement tool
outside Rotate; Rust/sidecar untouched. **Next: Stage 3 — ring torsions (the last unit of Stage 3).**

## [2026-08-06] session | fix(scene): restore preserves fragment boundaries — no silent re-adopt to a single "Molecule" fragment (debugging/014)
A **fragment-merge bugfix**, not a feature. A substrate+reagent scene (2 fragments) silently collapsed
into **one fragment named "Molecule"**, breaking everything that keys on the layout at once: rigid
rotate (3.3), move (3.1), inter-fragment vdW clash (3.2), per-fragment charge/constraints.

**The clue.** "Molecule" is the *default* name `sceneFromAtomLines` gives a fragment parsed from a
coordinate block with no name — so something **re-adopted the merged xyz from text as one fragment**.
History confirmed the op: "Adopt geometry from input text".

**Measured, not assumed (rule #10).** The prompt's lead hypothesis was **restore/New-iteration
re-adopting from text** — **measured FALSE**: a persist→restore round-trip of a 2-fragment scene keeps
both (a scratch test showed `restoreScene`/`restoreSceneLog` honour `scene_json`, `xyzMatchesScene`
true, `snapshotRejected: false`). Restore was never the bug. The culprit was found by a **live
WebKitGTK repro** (`localhost:1420` in Chrome): H₂O + BH₄⁻ (viewer: CPK water + **teal** BH₄, "2
fragments · 8 atoms") → click **Input Builder → Generate Input** → BH₄ recoloured **teal→CPK** and the
DOM read **"1 fragment · 8 atoms"**, name **"Molecule"**. The action is **Generate Input**, not restore.

**Root cause.** `handleGenerate`/`pickTemplate` → `adoptWholeInput` did an **unconditional**
`seedScene(sceneFromOrcaInput(newContent), "text-adopt")`. But "Generate Input" rewrites only the
`!`/`%` keyword lines over the **same** coordinates, and `sceneFromOrcaInput` parses a block into ONE
"Molecule" fragment — so it merged 2→1 for nothing. (The Input Builder is itself fragment-blind — it
parses `currentContent` into one fragment, which is why it showed "Σ of 1 fragment".)

**Fix.** A pure guard, reusing the SAME `xyzMatchesScene` primitive that guards the Monaco↔Scene sync:
`adoptPreservesScene(current, newContent)` → **true (keep the Scene)** iff a scene exists and the new
content's geometry matches it; different/absent geometry → **false (real re-adopt)**. `adoptWholeInput`
guards on it — Generate/same-geometry preserves the multi-fragment scene; Replace-input/new-molecule
still re-adopts (its confirmed reset unchanged). Mirrors the "block matches → keep" branch: a text
change that doesn't change geometry never disturbs the Scene.

**Regression guard** (`adopt.test.ts`) — at the ADOPT seam (not the restore round-trip the hypothesis
suggested): `adoptPreservesScene` true on same-geometry / false on different-geometry/null/no-block;
store-level: seed 2 fragments → guarded adopt keeps **2** (`["Dexketoprofen","BH4"]`), while the
**negative control** (blind `text-adopt`) collapses to **1 "Molecule"** — the measured bug. Forcing the
guard to `false` (baseline) reddens both. `tsc` clean, **vitest 548 green** (+3), `vite build` clean;
scratch repro removed.

**Manual gate (WebKitGTK, live).** **m1 confirmed:** after the fix, "Generate Input" leaves **"2
fragments · 8 atoms"**, names **["H₂O","BH₄⁻"]**, BH₄ still teal — layout survives. m2/m3
(rotate/clash/per-fragment) follow from the restored 2-fragment scene + their own unit tests.

**Wiki (same commit):** `debugging/014` (the "Molecule" clue, the measured culprit = Generate Input not
restore, root cause, fix, what it silently broke), `scene.md` (the adopt-preserve rule + the
`adoptPreservesScene` contract; restore's persist-is-truth clarified as measured-correct), `index.md`
(+debugging/014), this entry. **Not touched:** `restore.ts` (measured correct), the rotate/clash/move
ops, Rust/sidecar. **Next: Stage 3 — ring torsions (the last unit of Stage 3).**

## [2026-08-06] session | fix(editor): rotation axis gets a distinct accent colour so the Axis/Distance toggle is visibly perceptible (unit 3.3b-fix)
The 3.3b Axis⇄Distance toggle changed nothing visible. The **mandatory manual gate** (which is exactly
what exposed this) turned up **two** measured causes, not one.

**Cause 1 — colour.** The axis cylinder borrowed `theme.haloColor` (chartreuse), indistinguishable from
the green measurement line, so "axis" and "distance" looked identical. **Fix:** a dedicated
`theme.axisColor` — a saturated **azure** (`#3b82f6` dark, `#1d4ed8` light). Chosen deliberately to avoid
every known overlay collision: the chartreuse selection halo / green measurement line (~85°), the magenta
clash glow (~330°), the off-table Pd/Pt pink (`#ff1493`), and each fragment-palette hue
(teal/coral/gold/violet). The blue sits in the gap between the teal and violet fragment hues, so the max
reachable gap to the nearer of them is ~42°; `theme.test.ts` therefore locks a **whole-family gap (>90°)
from the greens it was confused with** and a **>30° gap** from everything else, plus 3:1 contrast on each
background. Negative control: setting the axis back to the green halo reddens the green-family assertion.

**Cause 2 — a render loop that the colour fix alone would NOT have solved (also a manual-gate finding).**
Selecting a pair spammed **"Maximum update depth exceeded"** (92/min in the console) and the toggle
**snapped straight back to Axis**. Root cause (measured, not the review's "toggle logic is correct"):
`RotatePanel` computed `axis = rotationAxis(scene, p, q)` — a fresh object every render — and fed it into
the effect's deps, so `onAxis([p,q])` (`setRotateAxis`) fired every render → re-render → loop; and because
`NewJobScreen`'s `[rotateAxis]` reset snaps the overlay to "axis" whenever `rotateAxis`'s identity changes,
the toggle could never leave Axis. This shipped latent in **unit 3.3** (whose manual gates were deferred) —
the deferral is what let it through. **Fix:** `useMemo` the axis (keyed on scene/P/Q) and **split** the
one effect into two — `onAxis` keyed only on the pair (fires on a pair change, not an angle tick),
`onEphemeral` keyed on the angle + memoized axis.

**Gates.** Pure: `theme.test.ts` — axis contrast in the overlay loop + a distinctness block (whole-family
off the greens, clearly off clash/pink/fragments), demonstrated-biting. `tsc` clean, **vitest 552 green**
(+4), `vite build` clean. **Manual (WebKitGTK, live — the decisive gate):** built H₂O+BH₄⁻, picked B#3 →
O#0. **m1** default Axis → **azure** cylinder (not green); **m2** flip Distance → **green** measurement
line, cylinder gone — a **clearly visible** change; **m3** flip Axis → azure cylinder back, Å identical in
both; console **clean** (0 "Maximum update depth", was 92/min before the loop fix). Toggle switches and is
perceptible.

**Wiki (same commit):** `theme.ts` (the `axisColor` field, documented), `visualization.md` (the azure
accent + why, and the render-loop fix — memoize + split effects), ROADMAP (3.3b → DONE, both causes),
this entry. **Not touched:** the toggle's pure decision (`chooseRotateOverlay` unchanged), the measure
render, the rotate math/op, the merge fix. **Next: Stage 3 — ring torsions (the last unit of Stage 3).**

## [2026-08-06] lint | Phase 4.2 arc lint (Stage 2→3) — status/drift reconciled, module pages verified against code
A full health-check after the Phase 4.2 Stage-2→3 arc (op log → rigid drag → clash → rotation → overlay
toggle → fragment-merge fix), plus the ring-torsion cut. Ran every CLAUDE.md lint check.

**Unambiguous fixes applied (6 stale status/drift claims):**
- ROADMAP: Stage 3 said "**begun**" and "**Next: Stage 3 — operations over the core**" while Stage 3 is
  now COMPLETE → the recap says complete, the Stage-2 trailer points forward correctly, and the Stage-3
  header gained a ✅ COMPLETE marker. (The "operations over the core, NOT Scene→Rust/WASM" wording from
  the 3.1 fix is correct and was kept.)
- `editor-ui.md` status: `unit 3.3` → `unit 3.3b (Stage 3 — COMPLETE)`.
- `scene.md` and `frontend.md` status leads: `Phase 2.5 complete` → `current through Phase 4.2 Stage 3`
  with a one-line arc summary (op-log fold, 3.1 drag, 3.2 clash, 3.3/3.3b rotation, `adoptPreservesScene`).
  The bodies were already current; only the lead label under-sold the state.

**Verified CLEAN (no change):**
- **Orphans/dangling: none.** 73/73 wiki pages linked from `index.md`; the two folder READMEs catalogued.
- **Module pages vs code: no false-about-current-code claims** (two Explore audits + spot checks). Every
  cited symbol exists — `translateFragment`/`rotateFragment`/`rotationAxis`/`adoptPreservesScene`/
  `detectClashes`, the `ephemeralScene`/`axisHighlight`/`rotateOverlay` props, the azure `axisColor`.
  `filterSelection` is correctly the CURRENT selection prune; `selectionSurvives`/`validateSelection`
  are correctly marked REMOVED (not presented as current). `collapse-from-text` correctly LEGACY.
- log.md 2d-misread correction present; "Undo deeper than one step" is [x] (2b); debugging/013+014 in the
  index bullet list; **ADR-011 coherent** (deferred behind a spike; "3Dmol stays a dumb renderer" affirmed
  — ADRs are not rewritten).

**Escalated to the architect (no page-vs-page contradiction needed a "which is right" choice — none found):**
- **ROADMAP L601** honestly still says unit 3.3's manual rotation-APPLY gates **m1–m5 are pending**. That
  is ACCURATE — the 3.3b-fix live session verified the axis/overlay/toggle + the render-loop, but never
  applied a rotation live. Stage 3 is marked COMPLETE on code + pure gates; the 3.3 manual gates genuinely
  remain un-run. Left as-is (honest); flagged for a decision on whether to run them.
- Minor: ADR-011 (2026-07-30) argues 3Dmol's index space is safe "because the model is rebuilt on every
  render"; the frozen-topology ephemeral path (3.1/3.14) updates coords in place on some frames instead,
  but that does not change the AtomId↔index alignment, so the argument still holds. Not rewritten (ADR).

**Correctly-historical, deliberately kept** (they explain evolution, they don't lie about the present):
the `selectionSurvives→filterSelection` removal note, `collapse-from-text` legacy, the "2c1 adapter gone"
mentions, and log.md's append-only "Next: ring torsions" trailers (history of what was planned then).

## [2026-08-07] session | feat(editor): guided fragment placement — add a reagent at d/θ/φ in one flow (Phase 4.2 tail-1)

**Unit 4.2-tail-1 — guided fragment placement (carried from Phase 2.5).** The roadmap tail:
today a reagent is added coarsely (`placeFragment`, ≥3.5 Å bbox gap) and then positioned in a
SEPARATE edit-mode action — two steps. Architect's decision (B): full d/θ/φ, each optional, in
ONE guided flow. Built by **composing existing ops, inventing no geometry.**

**What landed.**
- `src/scene/guided-placement.ts` (pure): `planGuidedPlacement` (reagent atom + 1–3 substrate
  anchors + target d/θ/φ → a `GuidedStep[]`, one per GIVEN coordinate), `guidedStepOp`, and the
  DI driver `runGuidedPlacement`. Each step is resolved through **`planEdit`** (reusing the mask /
  reference-atom rule / both-orientation search), forcing the orientation whose mover IS the
  reagent (`swapToAlternative` when planEdit defaulted to the smaller fragment). **No new d/θ/φ
  math.** Z-matrix nesting (d → θ → φ, mover R last in each chain, each edit rotating about an axis
  through anchor 1) so each coordinate preserves the earlier — the sidecar's existing
  `test_sequential_burgi_dunitz_acceptance` is the numeric proof.
- `src/scene/GuidedPlacementPanel.tsx` (React, Fragments section): splits the shared pick list by
  fragment membership (one reagent atom + substrate anchors), d (required) / θ / φ (optional, a
  field disabled with a reason until enough anchors are picked — empty ≠ 0), Preview (view-only) /
  Apply. Reuses the now-exported `callSetInternal` from `EditPanel` (one set-internal client, no
  second copy).
- `EditPanel.tsx`: exported `callSetInternal` + `SidecarResponse`, generalized the param to
  `{op,indices,mask}`.
- `NewJobScreen.tsx`: **app-owned** `guidedReagent` / `guidedMode` (NOT in the Scene — `store.ts`
  untouched). "Guided placement" toggle → clicking a reagent adds it roughly AND opens the panel on
  the just-added fragment; `applyGuided` commits one `replace-fragment-atoms` (`set-internal`) op
  per coordinate, in order — so **Undo unwinds d/θ/φ one at a time** (the `add-fragment` op is
  already committed at rough add). Guided reagent leaving the scene (undo past the add) closes the flow.

**Gates.** tsc 0; full vitest **560 passed** incl. `guided-placement.test.ts` c1–c4, each with a
**proven-biting** negative control (verified red-then-green: c1 null-vs-0 → 3 red; c2 wrong mask →
1 red; c3 bundled op → 2 red). pytest set-internal/geometry endpoint **14 passed, unchanged** (incl.
the sequential Bürgi-Dunitz acceptance). `vite build` clean. Grep gates: guided state absent from
`src/scene/store.ts` and from `src/scene/` (`guidedReagent`/`guidedMode` only in `src/screens`);
`guided-placement.ts` carries no geometry math and drives everything through `planEdit` /
`applyResponseToScene` / `applyResponseIssue`.

**Manual gate m1–m5 (real Tauri/WebKitGTK window) — PENDING author verification.** This is the
mandatory UI-unit gate and it is NOT yet run: the native WebKitGTK window + 3Dmol atom-picking
cannot be driven from this session (the browser tooling targets Chrome, not the Tauri webview). The
code + all automated gates are green; the live verdict — m1 the mission case (BH₄⁻ from the library
→ guided → pick B + carbonyl C, d=2.5, θ=107° → Preview → Apply → reagent sits on the geometry,
measured d/θ match, substrate unmoved), m2 step-by-step Undo, m3 d-only + φ, m4 degenerate pick
disables the field, m5 Monaco block updates on Apply — is left for the author to run in the window.

**Next:** run the manual gate m1–m5; then the constraint on/off toggle tail. Do NOT add a constraint
automatically on placement (separate action, next tail) — untouched here.

## [2026-08-07] session | feat(scene): user-extensible reagent catalog (molecules-table reuse) + seed cations — closes Phase 4.2

**Unit 4.2-tail-2 — extensible reagent catalog + seed cations.** Architect's decisions: (1) reuse
the `molecules` table (charge already there) + a role flag; (2) charge MANDATORY at save (never a
silent 0 — ADR-014); (3) curated (built-in, reference contract) vs user (user provenance, no
reference) distinguished in the palette. **This closes Phase 4.2.**

**What landed.**
- **Seed cations** (`fragment-library.ts`): Na⁺/Li⁺/K⁺ (+1), Mg²⁺ (+2) — monatomic, closed-shell
  (singlet, no radical hint), empty `reference` like H⁻/Cl⁻, "Monatomic cation" provenance.
- **DB schema v12** (`db.rs`): guarded `ALTER TABLE molecules ADD COLUMN is_reagent INTEGER NOT NULL
  DEFAULT 0` (guarded on the table existing, like v10/v11, so the migration fixtures skip cleanly).
  `Molecule` model + `COLUMNS`/`from_row` carry `is_reagent` (9th column). New commands
  `create_reagent(name, xyz, charge)` (charge a plain `i32`, never defaulted) + `list_reagents`
  (`WHERE is_reagent = 1`); `list_molecules` now filters `WHERE is_reagent = 0` (existing rows are
  all role 0 → molecule library + screen unchanged). Registered in `lib.rs`.
- **Frontend** (`reagent-catalog.ts` + `NewJobScreen`): `userReagentToFragment` (a saved reagent →
  a scene fragment, charge carried into the total like a built-in — `source:"library"`, never
  `"fragment-library"`) + `fragmentToXyz`. Palette shows **Built-in** vs **My reagents** as distinct
  groups (solid vs dashed chips); a "+ Save" dialog captures a picked fragment or pasted xyz with a
  **required** integer charge (Save disabled until valid). Guided mode (tail-1) now works for user
  reagents too. `Molecule.is_reagent` added to the TS type.

**Gates.** tsc 0; vitest **574 pass** incl. `fragment-library.test.ts` (c1 cations + scene total)
and `reagent-catalog.test.ts` (c2 charge flow, c4 curated↔user) — bites verified red-then-green
(Na⁺→0 reddens c1; charge→0 reddens c2). cargo **184 pass** incl. `reagent_role_separates_from_the_
molecule_library` + `reagent_persists_across_reopen` (c3 — role + charge survive a DB reopen; the
migration is idempotent). `vite build` clean.

**Manual gate m1–m5 (real Tauri/WebKitGTK window) — PENDING author verification** (as tail-1: the
native window + palette clicks can't be driven from this session). m1 Na⁺ add → total +1; m2 Save
requires charge → appears in My reagents; m3 add user reagent → total correct; m4 restart persists;
m5 curated vs user visually distinct.

## [2026-08-07] decision | Curated↔user reagent distinction; constraint on/off toggle CUT

- **Curated↔user reagent distinction is a rule, not a display nicety.** A built-in reagent carries a
  verified `reference` internal-coordinate contract; a user reagent does not. The palette keeps them
  in separate groups and the two never merge by type (`Molecule` has no `reference`; `LibraryFragment`
  does). Recorded in `wiki/modules/scene.md` "Extensible reagent catalog" (no separate ADR — the rule
  is small and lives with the module it governs). Guarded by `reagent-catalog.test.ts` (c4).
- **Constraint "toggle on/off" — CUT.** Delete + re-add already covers it (the 2.5.4b note). A
  persistent enabled/disabled state would introduce a second source of truth over the constraint
  text — the exact drift `constraints.ts` exists to prevent — for marginal ergonomics. Struck from
  the Phase 4.2 backlog (same reasoning as the ring-cut refusal). ROADMAP updated.

## [2026-08-07] milestone | Phase 4.2 — Geometry editor completion — COMPLETE

Stages 1–3 (identity core, operation log, operations over the core) + tails 1 (guided fragment
placement) and 2 (extensible reagent catalog). The "Done when" is met: editor state is a fold over a
typed operation log, 3Dmol is a dumb renderer fed an `AtomId` table, fragments drag/rotate rigidly
with step-by-step Undo, and no bare integer crosses an app-owned boundary. **Next: Phase 4.5 —
Reaction modeling.** Recorded an EARLY Phase 4.5 item: *microsolvation (explicit solvent shell)* —
install CREST, probe `--qcg` with xtb 6.6.1 (rule #10) BEFORE designing; builds on placement + xtb +
GOAT; caveats: shell conformer sampling + quasi-RRHO.

## [2026-08-07] session | feat(editor): bond display control — cations excluded by default + manual hide/show, per-AtomId-pair, display-only

**Unit bond-display-control.** 3Dmol draws bonds by distance; an s-block cation (Na⁺/K⁺/Mg²⁺…) that
*coordinates* an O/N/π gets a spurious covalent stick (surfaced by the tail-2 cation catalog).
Architect's decision: (B) auto-exclude cations from viewer bond perception (default, root cause) +
(A) manual hide/show any bond (escape hatch). Both **DISPLAY-ONLY**, app-owned, keyed by AtomId pair.

**What landed.**
- `src/viewer/bond-display.ts` (pure, 3Dmol-free like `frozenTopology`): `CATION_ELEMENTS` (alkali +
  alkaline-earth — justified: s-block metals coordinate ionically; deliberately NOT H/N/transition
  metals, whose bonds are real), `isCationBond`, `bondKey` (normalized AtomId pair), `shouldDrawBond`,
  and `filterDrawnBonds` (splices `bonds`/`bondOrder` on the live 3Dmol atom array — the frozenTopology
  technique).
- `MoleculeViewer`: `hiddenBonds`/`showCationBonds` props; `applyBondFilter` runs right after the ONLY
  two `addModel` perceptions (scene path → resolve via the feed's `ViewerAtomTable`; mode-animation
  build → element-only). Filters the perception 3Dmol already did — **no second pass**. The ephemeral
  drag/rotate paths reuse the filtered model without re-perceiving, so a hide survives an animation.
- `NewJobScreen`: app-owned `hiddenBonds: Set<BondKey>` + `showCationBonds` (NOT in the Scene); a
  "Show cation coordinate bonds" checkbox + a "Hide/Show bond between selection" button (2 atoms
  selected) + a "N hidden · show all" reset, in the Edit section.

**Why not in ORCA/sidecar (invariant 1).** An ORCA input is coordinates + charge — there is **no** bond
list (`wiki/orca/parse-sources.md`); the sidecar's mask perception has its own `within`. This unit is
purely the viewer's — geometry (Scene/Monaco xyz/total charge/generated `.inp`) is byte-identical
whatever is hidden.

**Gates.** tsc 0; vitest **583 pass** incl. `bond-display.test.ts` c1–c4 — bites verified red-then-green
(drop Na from the list → c1 red; key on viewer index instead of AtomId → c2 red). c2 is the key test
(AtomId pair survives a 2c2-style index shift; a positional key hides the wrong bond). `vite build`
clean. Grep: bond-display state absent from `src/scene/`; the filter operates on `model.selectedAtoms`
after `addModel` (no second `addModel` for bonds).

**Manual gate m1–m5 (real Tauri/WebKitGTK window) — PENDING author verification** (native 3Dmol
picking can't be driven from this session). m1 Na⁺ near aromatic-H/carbonyl-O → no fake bond, rest of
the molecule intact; m2 hide/show a real disputed bond; m3 hide survives drag/rotate (AtomId, not
position); m4 display-only live (Monaco/charge/`.inp` unchanged); m5 stick/line + mode animation work
with the filter.

**ROADMAP.** Bond display control `[x]` (viewer polish, post-4.2). Recorded EARLY Phase 4.5 items:
transition-state methods (OptTS / NEB / NEB-CI / IRC) pulled from Phase 6 with a probe-then-design note
(rule #10) — they are the point of reaction modeling, not a power feature. (Microsolvation CREST-probe
item was recorded in tail-2.)

## [2026-08-07] session | fix(editor): rotate-overlay defaults to distance; reconcile Phase 4.5 roadmap

**A — rotate-overlay default: axis → distance.** On picking two atoms in the Rotate tool the
researcher first wants the separation (the reaction coordinate), and the green measurement line reads
unambiguously; the axis cylinder is one toggle away when actually rotating. Changed
`DEFAULT_ROTATE_OVERLAY` (`rotate-overlay.ts`) to `"distance"`; the constant propagates to
`NewJobScreen` init + `[rotateAxis]` reset and to `RotatePanel`'s Cancel (which now references the
constant instead of a hardcoded `"axis"`). Toggle logic (`flipRotateOverlay`/`chooseRotateOverlay`),
the azure axis colour, the "exactly one overlay per pair" invariant, and the Å label are unchanged.
`rotate-overlay.test.ts` default assertion updated to `"distance"`; flip-symmetry + one-overlay tests
untouched. vitest green.

**B — ROADMAP Phase 4.5 reconciled into a clean ordered block.** Dependency line now names both
completed prerequisites (Phase 4.2 + Phase 3 parsing). Items regrouped: **Early** (new tools, probe
first — rule #10) = Microsolvation (install CREST → probe `crest --qcg` with xtb 6.6.1, domain rule #2
compatibility; caveats: shell conformer sampling + quasi-RRHO) and **Transition-state methods** moved
from Phase 6 (OptTS / NEB-TS / NEB-CI / IRC — the core of mechanism work, each probe-then-design);
**Core** (existing native-scan pipeline, ordered) = conformer→reaction-center, data model, reaction
setup UI (reuses tail-1 guided placement), scan input gen, scan output parser, energy-profile viz,
comparative pathway view, and TS refinement — the last now framed as the *application* of the early
OptTS/IRC methods (no duplicate implementation).

tsc 0; vitest green. wiki: editor-ui.md (rotate default now Distance). No manual gate beyond the one
short rotate step (pick two atoms → Distance shown by default; toggle → Axis).

## [2026-08-07] decision | Phase 4.5 staged (A–F); Reaction/Pathway data model deferred to Stage C

Phase 4.5 restructured from a flat item list into six ordered stages, on the project's standing
principle (probe before build · least risk first · every stage ends in a usable result · the
Reaction object appears where grouping/comparison needs it, not as infrastructure ahead).

- **A** scan input gen (A1 emit — pure+Rust golden+real ORCA; A2 panel+manual gate) · **B** scan
  reader + single profile · **C** Reaction/Pathway data model + comparative ΔΔE‡ (= mission
  done-when) · **D** conformer→reaction-center DFT-rigor layer · **E** TS methods (OptTS/IRC/NEB,
  each probe-first) · **F** CREST microsolvation (probe-first).
- **Spine A–C is de-risked already** — unit 3.3 measured the relaxed-scan artifacts
  (`.relaxscanact.dat`/`.relaxscanscf.dat`, coord Å + energy Eh; `! Opt` required or silent SP).
  Unmeasured work (TS, CREST) carries its own probe at the head of its stage (rule #10).
- **Decision — data model deferred to Stage C** (was an early core item). A single relaxed scan is
  standalone-usable (ADR-007 blesses standalone jobs); tables that serve only grouping/comparison
  land with grouping/comparison. Same "earn its place" rule that cut ring-torsions + constraint-toggle.
  Retrofit is minimal (Pathway = additive metadata + nullable FK; the scan job is unchanged).
- **Architectural guard recorded for Stage A** (surfaced in review): `Scan` and `Constraints` are
  both `%geom` sub-blocks — scan injection must **compose into the one `%geom`**, never emit a
  second (ORCA would silently take one). Scan emit rides the same order-bearing 0-based
  AtomId→OrcaIndex path as constraints (ADR-016), byte-identical Rust/TS pair.
- Two standing gates preserved: symmetry re-probe per system before `! UseSym`; unfixed-stereocenter
  flag on SMILES import (land before Stage C — si/re needs defined stereochemistry).

Next: Stage A1 (scan-coordinate emit). ADR-007 / ADR-010 / ADR-016 unchanged.

## [2026-08-07] session | Phase 4.5 Stage A1 — scan-coordinate emit (%geom Scan), byte-identical Rust/TS, composes with constraints

Stage A1 of the reaction-modeling scan spine: generate a `%geom Scan` relaxed-scan block from a
`ScanCoordinate`, pure + Rust golden + a real ORCA run. No UI (that is A2).

- **New `src/scene/scan.ts`** — `ScanCoordinate` (kind B/A/D, 0-based atoms in the SAME index space
  as `Constraint`, `start`/`end` carrying the `startText`/`endText` value_text-analogue, `npoints`
  ≥ 2), `scanBlock` (emit), `injectScan`, `parseScanBlock`/`inspectScanBlock` (read-back for A2),
  `scanOptIssue` (the `! Opt` guard). **New `orcastudio-core/src/emit.rs::emit_scan_block`** — the
  second order-bearing `%geom` emit, **byte-identical** to `scanBlock`, pinned by a golden pair
  (Rust `scan_block_golden_ethane` + the TS vitest assert the same literal
  `%geom\n  Scan\n    B 0 1 = 1.4, 2.4, 6\n  end\nend`). Reuses `to_orca_index`, `fmt_value`, and
  the 17-digit non-canonical-value guard.
- **Composition (the unit's central property) — one `%geom`, never two.** `Scan` and `Constraints`
  are both `%geom` sub-blocks. Lifted the depth-tracking `%geom` locator out of `constraints.ts`
  into **new `src/scene/geomBlock.ts`** (`scanTokens`/`locateGeom`/`leadingIndent`), now tracking
  BOTH sub-blocks (a constraints-only locator would mis-read a `Scan` block's `end` as closing
  `%geom`). `injectConstraints` and `injectScan` share it; `injectScan` inserts/replaces/removes
  only the `Scan` sub-block, as a sibling of any `Constraints`. Negative control **C-two-geom** in
  `scan.test.ts` shows a deliberately-parallel injector produces two `%geom` (bites), the real one
  exactly one holding both blocks.
- **Four negative controls, red-then-green** (`scan.test.ts`): C-two-geom (two-%geom), C-index-base
  (0-based app index vs a 1-based mis-emit), C-byte-parity (canonical form vs a formatting drift),
  C-opt-guard (scan without `! Opt` fires; a commented-out `Opt` does not count).
- **Real ORCA loop closed (rule #10).** The `scan.ts` emit generated an ethane C–C scan (indices
  0,1; 1.4→2.4 Å; 6 pts — mirrors unit 3.3), run via `/opt/orca/orca` full-path in an isolated dir
  with `! r2SCAN-3c Opt TightSCF`: `input.relaxscanact.dat`/`.relaxscanscf.dat` each **6 rows**
  (coordinate Å 1.4…2.4 + energy Eh), `ORCA TERMINATED NORMALLY`. Our **generator** now provably
  produces the artifacts unit 3.3 measured from a hand-written input.
- Verify: `cargo test` (10 emit tests incl. 4 scan), `tsc` 0, `vitest` 606 green (64 in
  scan+constraints), `vite build` clean. New `wiki/orca/scan.md`; module pages + index updated.

Next: Stage A2 (Scan panel + define-coordinate-from-selection, the manual gate). ADR-016 unchanged.

## [2026-08-07] session | Phase 4.5 Stage A2 — Scan panel + Scan-from-selection + Run-guard (opt family measured); author manual gate PENDING

Stage A2 builds the UI over the A1 scan core: a Scan panel (view over the input text), "Scan this
coordinate" from a selection, and the `! Opt` Run-guard given teeth — plus a rule-#10 probe that
broadens the guard's opt-keyword set to the measured truth.

- **Task-1 probe (rule #10) — which opt keywords trigger a relaxed scan.** Ran real ORCA 6.1 ethane
  C–C scans (`! r2SCAN-3c <kw> TightSCF` + the same `%geom Scan`): **TightOpt, VeryTightOpt, LooseOpt
  all produce a 6-row `.relaxscanact.dat`** (relaxed scan), like `Opt`/`OptTS` (A1). So
  `hasOptKeyword`'s set is now the measured `{opt, optts, tightopt, verytightopt, looseopt}` — NOT
  widened from docs; a keyword enters only after a run confirms it. Table recorded in
  `wiki/orca/scan.md`. This closes the "must not false-block a valid `! TightOpt`" risk with a
  measurement, not an assumption.
- **`ScanPanel.tsx`** — a **view over the input text**, mirror of `ConstraintPanel`: source =
  `inspectScanBlock(content)`, every edit = `injectScan(content, …)`; NO React state that *is* the
  scan (the number fields hold only a transient keystroke draft). Renders absent / parsed (editable
  start/end/npoints + remove) / unrecognised, surfaces `scanOptIssue` inline. Sits in the editor dock
  next to Constraints (new `scan` dock section).
- **Scan-from-selection** — `AtomInspector` gains "Scan this {distance/angle/dihedral}" (mirrors
  "Constrain selection"): a 2/3/4-atom pick → new `scanFromSelection(scene, AtomId[], range)` in
  `scan.ts`, resolving `AtomId → 0-based global index` at build time (survives a fragment index
  shift). Default range = current measured value → +1 Å / +30° / +60°, N = 10 (editable in the panel).
- **Run-guard wired** — `scanBlockMessage = scanOptIssue(content)` feeds the SAME create/run gate as
  the constraint range-check: a scan with no measured opt keyword blocks Create & Run with the
  diagnostic (immutable input that would silently single-point is a landmine, like an out-of-range
  constraint). Banner mirrors `constraintBlockMessage`.
- **Three negative controls green** (`scan.test.ts`, +17 tests): C-view-over-text (every panel edit
  is a pure `injectScan` transform; `inspectScanBlock` is the whole truth), C-tightopt-block (each
  measured opt keyword NOT false-blocked; a no-opt scan bites), C-atomid-pick (boron id 3 → current
  global 0 after water removed — emits `[0,1]`, not the ids). No Rust change.
- Verify: `tsc` 0, `vitest` 620 green (37 in scan.test.ts), `vite build` clean, `cargo test`
  unaffected.

**AUTHOR MANUAL GATE — PENDING (unit stays open until this passes; Claude Code cannot drive the
3Dmol window).** In the real Tauri/WebKitGTK window (`npm run tauri dev`):
- g1: select two atoms → "Scan this distance" → a `%geom Scan B …` appears in Monaco + the Scan
  panel; editing start/end/npoints rewrites only that block (any Constraints intact — one `%geom`).
- g2: remove `Opt` from the `!` line → Run blocked with the scan diagnostic; add `Opt` → Run enabled.
- g3: `! TightOpt` (measured) → Run NOT blocked.
- g4: an unrecognised / multi-coordinate `Scan` block → panel shows hands-off, never clobbers it.

Next: record the g1–g4 result, flip ROADMAP A2 `[~]`→`[x]` + Stage A complete, then Stage B (scan
output parser `relaxscan.rs` + energy profile). ADR-016 unchanged.

## [2026-08-07] session | Phase 4.5 Stage B1 — relaxed-scan reader (.relaxscanact/.relaxscanscf.dat) + geometry cross-check

The fifth artifact reader: the relaxed-scan **profile**. Rust-only, no manual gate — verified by
`cargo` + the real ethane C–C scan artifacts (committed as fixtures).

- **New `src-tauri/src/parse/relaxscan.rs`** on the `property.rs` template (two layers, typestate
  `parse → verify → Verified`, post-conditions-as-errors). Reads the two `.dat` files as N rows of
  `coordinate energy` (per **scan point**, NOT the 26 per-cycle rows of `.property.txt`/`_trj.xyz`).
- **The load-bearing post-condition — the coordinate's Å is confirmed per-read (rule #11).** A bare
  2-col `.dat` has no unit literal, and a Bohr coordinate would draw a plausible-but-wrong profile,
  not crash. So for a `B` scan `verify` recomputes the scanned distance from each `input.NNN.xyz`
  (Å, via a new `XyzFile::pair_distance_angstrom` witness — reuses the xyz reader, no re-implemented
  parsing) and asserts it equals col1 within 1e-3 Å. A Bohr coordinate fails ≈1.889× (`GeometryMismatch`).
  `A`/`D` parse the same but their cross-check is deferred (coordinate is degrees).
- **act vs scf both stored, labelled, never conflated** — `act` = composite (gCP+D4), `scf` = bare
  SCF; measured to differ. Cross-file post-condition: same N + identical coordinate column. Plus N≥2,
  energies finite, coordinate strictly monotone.
- **Wired into `results.rs`** — `ParsedResults.scan: Option<ScanProfileJson>`, populated when
  `.relaxscanact.dat` is present, `None` otherwise (absent-is-normal, like `ensure_gbw_json`). Rides
  in `data_json`; `parser_version` 3→4 (no narrow column, no migration — like the trajectory). The
  scanned-atom spec is a minimal regex parse of the input's `%geom Scan` line (`parse_scan_spec`,
  requires the `=` to skip a `{B..}` constraint), done in `results.rs` and passed in — the reader
  never reads `input.inp`.
- **Real-ORCA parse (checkpoint 3).** The committed fixtures (`tests/fixtures/scan-ethane-cc/`, copied
  from the probe dir `~/.local/share/orcastudio/probe-scans/scan-ethane-cc`) ARE the real ORCA 6.1
  output. Parse: **6 points**, coordinate 1.4→2.4 Å monotone, act≠scf (row 0: act −79.78236865, scf
  −79.78571668), geometry cross-check green vs `input.001…006.xyz`.
- **Three negative controls red-then-green** (`relaxscan/tests.rs`): C-bohr-coordinate (col1 ×1.889 →
  GeometryMismatch >1), C-act-scf-conflated (scf:=act would collapse the >1e-6 gap), C-per-cycle-source
  (26 rows can't pass the 6-point-file cross-check → Io on missing `input.007.xyz`).
- Verify: `cargo test` 195 pass (11 relaxscan). No frontend change (tsc/vitest/build unaffected).

Next: Stage B2 (energy-profile React view — recharts + click-a-point→load `.NNN.xyz`, max marked as
approximate TS). ADR-012 template held (fifth reader, no bend).

## [2026-08-07] session | Phase 4.5 Stage B2 — scan energy-profile view (ΔE vs coordinate, click→geometry, approximate-TS); author manual gate PENDING

The first time a scan is *visible*. Reuses the trajectory disciplines; reads B1's `ParsedResults.scan`
and re-parses nothing (ADR-012).

- **New Rust command `read_scan_geometries(job_id)`** (`results.rs` + `commands/jobs.rs`, registered
  in `lib.rs`) — loads each `input.NNN.xyz` in point order via a new `XyzFile::first_frame` witness
  (a scan point geometry is display data whose element order is checked at the UI boundary, not
  authoritative output — it does NOT go through the reference-based geometry post-condition, which
  fails by design for a relaxed point). Writes nothing to the job dir (rule #3); reads point files
  whole (rule #5). `None` for a non-scan job. Two cargo tests green on the real fixture (6 geometries,
  C–C 1.4→2.4 Å, right file per index; None for non-scan).
- **New `src/scan/scanProfile.ts`** (pure, node-tested): `profileSeries` (ΔE kcal/mol vs coordinate,
  reference point exactly 0), `maxIndex` (approximate-TS point), `pointGeometryXyz` (element-order
  identity at the boundary, reusing `elementsAgree`/`frameToXyz`), `pointReadout`.
- **New `src/scan/ScanProfilePanel.tsx`** (wired into `ResultsCard` under `results.scan`): the
  selected point index is **React state** (viewer never owns it, ADR-011) → feeds one geometry to
  `MoleculeViewer`; recharts with `useContainerWidth` (no `ResponsiveContainer`); click a point → set
  index. Labelled display choices: `act`(default)/`scf`, reference first/minimum. The maximum is
  marked **"approximate TS (scan maximum)"** — a ΔE‡ estimate on a relaxed surface, never the TS /
  ΔG‡ (ADR-007), with a forward pointer to OptTS (Stage E). `<2`-point and non-scan states handled.
- **Three negative controls red-then-green** (`scanProfile.test.ts`): C-relative-energy (ref point
  ==0; raw Eh would be ≈−79.78, not 0), C-app-owned-index (index i → geometry i's xyz; a 3Dmol-owned
  frame would ignore i), C-element-order (mismatched sequence → loud refusal, no render).
- Verify: `tsc` 0, `vitest` 628 green (8 scanProfile), `cargo test` 197 (2 new command), `vite build`
  clean.

**AUTHOR MANUAL GATE — PENDING (batches with A2 g1–g4; Claude Code cannot drive the 3Dmol window).**
In the real Tauri/WebKitGTK window on a completed relaxed-scan job:
- h1: the profile renders (coordinate Å × ΔE kcal/mol), points marked, the maximum labelled
  "approximate TS (scan maximum)".
- h2: click a point → that geometry loads in the viewer; the readout shows its coordinate + ΔE.
- h3: toggle act↔scf and reference first↔minimum → the curve + the zero move, labels update.
- h4: a non-scan job (Opt/SP) shows no scan panel; a 1-point degenerate case doesn't crash.

Next: record h1–h4 (with A2 g1–g4), flip ROADMAP A2+B2 `[~]`→`[x]` + Stage A/B complete, then Stage C
(Reaction object + comparative ΔΔE‡ — mission done-when). ADR-011/012 held (app-owned frame, no
re-parse).

## [2026-08-07] session | fix(parse): scan jobs parse profile-only (Phase 4.5 B1 fix — unblocks B2 gate)

**Bug (caught by the B2 manual gate).** The full `parse_and_store` on a completed relaxed scan
failed with `geometry post-condition failed: max Δ 0.056013 Å exceeds 1e-4 (a missed Bohr→Å
conversion looks like ≈1.889×)`. The calculation succeeded; our parse failed → job stuck at
`completed` with an error, so the B2 profile panel had nothing to render.

**Measured first (rule #10).** The Bohr hint is a red herring: 0.056 Å = the C–C compression
(input 1.512 Å → scan point 1's constrained 1.400 Å = 0.056/carbon), not a 1.889× units miss. Root
cause = `property.rs`'s geometry post-condition compares the **first `$Geometry`** to the **input**
(`input_ref`), but a scan `.property.txt` is **multi-point**: 26 `$Geometry` blocks (opt cycles
across 6 points), first = scan point 1's constrained geometry, charges only at some cycles, one
dipole at the end, no thermochemistry. Premise "first structure == input" is structurally false.
`_trj.xyz` first frame is likewise scan point 1 (a second latent `input_ref` failure); hess/mo
anchor on a non-existent single final structure. Fixture max Δ = 0.0635 Å (its input C–C = 1.527).

**Fix.** `parse_and_store` branches on `input.relaxscanact.dat` (B1's detection) to a new
`parse_and_store_scan` → **profile-only**: parse the B1 profile (its coordinate cross-check is the
live units guard), build the record via `ParsedResults::from_scan_profile` (header energy = last
point's `act`; final geometry = last point's `input.NNN.xyz`, Å), skip property/`_trj`/hess/mo. The
Opt/SP/Freq path is untouched; **no tolerance loosened, Bohr guard not skipped** — it moved to where
its premise holds.

**Fixtures consolidated.** `property_scan_ethane.property.txt` → `scan-ethane-cc/input.property.txt`
and `xyz_trj_scan.xyz` → `scan-ethane-cc/input_trj.xyz` (git mv), refs in property/xyz tests
updated — `scan-ethane-cc/` is now a complete scan job dir (single source of truth).

**Tests (RED→GREEN, guard preserved, gap closed).**
- `scan_job_parses_profile_only_full_pipeline` — full `parse_and_store` on the real `scan-ethane-cc/`
  dir: RED before (the 0.056-class `GeometryMismatch`), GREEN after (profile stored, 6 points, energy
  = last `act` −79.69075938, final geom C–C ≈ 2.4, no mis-attributed charges/thermo/trj). **Closes
  the test gap** — B1/B2 tested `.dat`/point-`.xyz` in isolation, never the full pipeline on a scan.
- `single_structure_property_check_bites_on_a_scan_artifact` — the routed-around guard still fires
  (`GeometryMismatch`, max Δ 0.0635, compression-scale ≪ a 1.889× Bohr blow-up): tolerance intact.
- `non_scan_dir_still_runs_the_single_structure_readers` — a non-scan dir still goes through
  property (produces charges); scan branch did not leak. Bohr-guard biting on the non-scan path is
  `property::tests::missed_bohr_conversion_fails_loudly`; the scan units guard is
  `relaxscan::tests::c_bohr_coordinate_fails_the_cross_check_loudly` — both still green.

**Verify.** `cargo test` 200 lib green (21 ignored real-data), `tsc` 0, `vitest` scan 8 green. Wiki:
+debugging/015, parse-sources.md + results-ui.md + ROADMAP.md B1 notes, index.md.

**Next.** The scan now parses → the author re-runs the B2 manual gate h1–h4 (batched with A2 g1–g4);
Stage B closes when it passes.

## [2026-08-07] session | fix(results): chart-click selects a point under recharts v3 (scan + trajectory) — B2 fix

**Bug (B2 manual gate h2).** Clicking a scan point in the energy-profile chart did nothing — the
readout stayed "point 1 / 6" and the molecule stayed point 1; hover tooltips worked.

**Localized + measured (rule #10).** Readout unchanged on click → the bug is the click→`setSelected`
layer, not the viewer feed / `read_scan_geometries` / profile math (all verified). recharts
**3.10.1** (`package.json`); installed types show **`TooltipIndex = string | null`** — v3 delivers
`activeTooltipIndex` as a **string**, so the inline `typeof i === "number"` guard was always false →
**every click silently dropped**. Inline handlers are never unit-tested (jsdom can't fire a real
recharts click) → pure tests stayed green. **Cross-cutting:** `TrajectoryPlayer` used the identical
pattern → its click-to-jump (unit 3.8) had been silently broken since the v3 upgrade too. IR/table
onClicks use the datum's own `m.index` (not `activeTooltipIndex`) — grep-confirmed unaffected.

**Fix — the class, extracted + tested.** New `src/charts/clickIndex.ts` `resolveClickedIndex(state,
series, getX?)`: number/string `activeTooltipIndex` → array position; else `activeLabel` (x value)
matched via `getX` (`coordinate`/`cycle`) within 1e-6; `null` otherwise; never throws. Both
`ScanProfilePanel` and `TrajectoryPlayer` route through it and map `series[pos].index` to their
setter; each DEV-warns on an unresolved click. Redundant on-dot select uses the **function form** of
`activeDot` (a `<circle onClick>`) — measured in the recharts source (`ActivePoints.js` +
`adaptEventHandlers`): the **object** form's onClick receives the activeDot props object, **not** the
datum, so it cannot select; the function form gets `ActiveDotProps` (`index`/`payload`). This
corrects the plan's "object form delivers the datum" premise.

**Tests (RED→GREEN, closes the wiring gap).** `clickIndex.test.ts` — C-v3-string-index (`"5"` → 5,
with an inline old-guard assertion proving the number-only logic returns null on the string),
C-number-index (v2 shape), C-label-fallback (x match + non-match → null + no-getX → null), C-garbage
(`{}`/`"x"`/null/out-of-range/empty series → null, no throw).

**Verify.** `tsc` 0; `vitest` 632 (48 files, +4 resolver); `vite build` clean. Grep: no
`activeTooltipIndex` handler left on the old pattern (only the two new resolver-routed activeDot
paths). Wiki: +debugging/016, results-ui.md (trajectory + scan click model → shared resolver),
index.md.

**Manual gate (author, real window) — PENDING, unit stays open until it passes:**
- h2 (scan): click point 6 → readout "point 6 / 6 · 2.400 Å" AND the molecule shows two separated
  CH₃ groups (no C–C bond at 2.4 Å); click point 1 → compact bonded ethane returns.
- trajectory regression: open an Opt job, click the E(cycle) chart → the frame jumps to that cycle.

**Next.** Author runs h2 + the trajectory regression (batched with the still-pending A2 g1–g4 / B2
h1,h3,h4); Stage B closes when they pass.

## [2026-08-07] session | Phase 4.5 Stage C1 — reaction/pathway data model (migration v13, no manual gate)

**Built.** The first slice of the reaction data model (ADR-007), pure Rust + SQLite, fully
cargo-verifiable — no UI, no manual gate.

- **Migration v13** (`SCHEMA_VERSION` 12→13, guarded + idempotent like every prior arm):
  `reactions(id, name, description?, created_at)`, `pathways(id, reaction_id→reactions, label,
  created_at)`, and a nullable `ALTER TABLE jobs ADD COLUMN pathway_id TEXT REFERENCES pathways(id)`
  (guarded on `jobs` existing; ADD-COLUMN-with-REFERENCES is legal because the default is NULL).
- **Types + commands** (`models/reaction.rs` `Reaction`/`Pathway`; `commands/reactions.rs` thin
  wrappers over `*_conn`): `create_reaction`/`list_reactions`/`rename_reaction`/`delete_reaction`,
  `create_pathway`/`list_pathways`/`delete_pathway`, `attach_job_to_pathway`/`detach_job_from_pathway`.
  Registered in `lib.rs`. Frontend `Reaction`/`Pathway` types in `src/types.ts` (no component — C2).

**Decisions (ratified, deviating from ADR-007's sketch — amendment written this session).**
1. **Normalized: `jobs.pathway_id` ONLY, no `jobs.reaction_id`.** Reaction is derived by joining
   `pathways`. Two columns that can disagree is the trap the project refuses; one source of truth
   over the one-join saving.
2. **A pathway is lean** (`{ id, reaction_id, label }`) — no coordinate/method/profile and no
   `reaction_centers` here. Those live in the attached job (Stages A/B); C2 reads them there.
   `reaction_centers` is deferred to the reaction-center editor.
3. **Jobs-survive invariant (load-bearing):** `delete_reaction`/`delete_pathway` null the attached
   jobs' `pathway_id` and remove only grouping rows — **never a job**. Referential integrity is
   enforced in the commands (this DB leaves SQLite FK enforcement off; `REFERENCES` is docs).

**Negative controls (cargo, RED→GREEN).** `migrate_v12_to_v13_adds_reaction_tables_and_preserves_data`
(reactions/pathways exist, `jobs.pathway_id` NULL for existing rows, prior job+molecule data intact);
`delete_reaction_keeps_jobs` (**bite-verified**: a naive DELETE-instead-of-NULL cascade turns it red —
"the job MUST survive" panics); `referential_integrity_is_enforced` (missing reaction/job/pathway →
`NotFound`, no orphan row); `standalone_job_unaffected_by_reaction_model` (a `pathway_id = NULL` job
is untouched while a whole reaction is built and torn down around it).

**Verify.** `cargo test` 207 passed (0 failed; +7 new: 1 migration, 6 command); `tsc` 0; `vite build`
clean. No manual gate (schema + Rust). Wiki: amended `adr-007` (Amendment section — ratified
normalized schema), `modules/tauri-core.md` (v13 entry + commands + migration-test list),
`ROADMAP.md`, `index.md`.

**Housekeeping flushed (was deferred to this touch).** Stage A2 + Stage B2 flipped `[x]` and **Stage A
+ Stage B marked complete** — the author's manual gates (A2 g1–g4, B2 h1–h4) **PASSED** in the real
Tauri window. The two B-stage fixes that landed against the B gate before it passed are recorded:
**B1 fix** — scan jobs parse profile-only (`debugging/015`, the single-structure `.property.txt`
post-condition does not apply to a multi-point scan); **B2 fix** — recharts-v3 chart-click via a
shared, tested `resolveClickedIndex` (`debugging/016`, v3 delivers `activeTooltipIndex` as a string).

**Next.** C2 — promote the Stage-A/B scan setup into a `pathways` row (`attach_job_to_pathway`),
overlay Pathway A vs B, highlight **ΔΔE‡**. Reads coordinate/method/profile from the attached job
(one source of truth). Manual gate returns at C2.

## [2026-08-07] session | Phase 4.5 Stage C2a — reaction/pathway management UI (code complete; manual gate PENDING)

**Built.** The management UI over the C1 reaction/pathway commands — a new **"Reactions"** tab
(`src/screens/ReactionsScreen.tsx`, wired in `App.tsx`). Thin over C1: it only calls
`create_reaction`/`list_reactions`/`rename_reaction`/`delete_reaction`, `create_pathway`/
`list_pathways`/`delete_pathway`, `attach_job_to_pathway`/`detach_job_from_pathway`, plus `list_jobs`
+ `read_job_results`. No energy/coordinate/ΔΔE‡ logic (that is C2b).

- **List + create** a reaction (name required, description optional). **Detail**: its pathways, each
  showing label + the attached job's title/status. **Attach a scan job as a pathway**: a label field
  + a picker over unattached completed/parsed jobs → `create_pathway` then `attach_job_to_pathway`.
- **Mark/warn scans**: the picker marks each candidate `✓ scan` / `(not a scan)` via `isScanJob`
  (results carry a scan profile); a non-scan pick shows an advisory warning but is **allowed** (C1's
  attach is permissive; the comparability guard is C2b).
- **Jobs-survive, made visible**: detach / delete-pathway / delete-reaction each carry a Tauri-dialog
  `confirm` whose copy says the scan job stays in the Jobs list (only the grouping is removed); a
  pathway's job title is a link that **opens the still-standalone job**.

**Extracted logic + controls** (`src/reactions/pathway.ts`, `pathway.test.ts`): `isScanJob`,
`isValidPathwayLabel`/`normalizePathwayLabel`. **C-scan-detection** (true w/ scan profile, false
without / null / zero-points — bite-demonstrated RED with `return true`) and **C-empty-label**
(empty/whitespace rejected).

**Decisions.**
1. **Exposed `jobs.pathway_id` on the `Job` model** (14th column of `Job::COLUMNS`/`from_row`;
   `pathway_id` on the TS `Job`). C1 deliberately left it off ("C2 exposes it when it needs to"); C2a
   needs it to map pathway→job (`Job.pathway_id === Pathway.id`) so the mapping **survives reload**
   and stays one-source-of-truth (the job carries the FK, not the pathway). Additive — `cargo test`
   still 207 (jobs tests use `init_db`, which now has the column). This is the one Rust touch; the
   prompt's "no Rust change" assumption didn't account for the reload-safe mapping need.
2. **Tauri-dialog `confirm`, not `window.confirm`/`prompt`.** Native dialogs are unreliable under
   WebKitGTK — a silently-false `confirm` would make delete a no-op (and fail the gate). Use
   `@tauri-apps/plugin-dialog` `confirm` (already a dep, initialized in `lib.rs`, used in
   `export/save.ts`). The plugin has no text prompt, so **rename is an inline edit** (input +
   Save/Cancel), not a prompt.

**Verify.** `tsc` 0; `vitest` 638 (49 files, +6 for the two controls); `vite build` clean; `cargo
test` 207 (the additive `Job` column). Wiki: +`modules/reactions-ui.md`, `tauri-core.md` (v13 note:
`Job` now carries `pathway_id`), `ROADMAP.md` (C2a `[~]`), `index.md`.

**Manual gate (author, real window) — PENDING; the unit stays open until it passes.**
- m1: create "Ketone + BH₄ (si vs re)" → appears, opens.
- m2: attach two completed scan jobs as pathways "si face"/"re face" → both listed with job titles;
  a non-scan job in the picker is marked/warned.
- m3: detach one pathway's job → un-groups; the scan job is **still in Jobs**, openable with its
  profile intact.
- m4: delete the reaction → gone from the list; **both scan jobs still exist in Jobs**.

**Next.** Author runs m1–m4. Then C2b — promote reads coordinate/method/profile from the attached job,
overlays Pathway A vs B with **ΔΔE‡**, and adds the comparability guards C2a leaves advisory.

## [2026-08-07] decision | Reaction energy reference model ratified (three barriers; hybrid summed optional reference)

**Ratified in the C2b design conversation.** Two wiki artifacts, no code (the table + overlay land in
C2b).

**The science** ([`chemistry/reaction-barriers.md`](chemistry/reaction-barriers.md), Ukrainian
teaching note). One relaxed scan gives **three different barriers**, each off a different reference:
1. **ΔΔE‡ (si/re selectivity)** = E(max_si) − E(max_re) — the shared reactant reference **cancels**, so
   the number is **reference-independent**. The mission screening value (scan maxima, not saddles, not ΔG‡).
2. **Intrinsic barrier** = E(max) − E(scan minimum) — a scan started far enough (Nu···C ≈ 2.5 Å+)
   captures the **pre-reaction complex** as its minimum, so this is **free** from the scan.
3. **Barrier vs separated reactants** = E(max) − [E(substrate) + E(reagent)], each reactant optimized
   **separately in its own job** (BH₄⁻ with its −1 charge). The reference comparable to **solution
   kinetics / Arrhenius**. Complex-formation energy = ref3 − ref2, **not small** for the ion-dipole pair,
   so barriers 2 and 3 differ — always name which is shown. Experimental caveats named (association
   entropy, standard-state, ΔG‡≠ΔE‡ → full comparison needs OptTS+Freq+thermochemistry, Stage E+).
   Method commonality (same method/basis/dispersion/solvation, SMD-not-ALPB for ions) = the C2b guard.

**The data-model decision** ([ADR-018](architecture/adr-018-reaction-energy-reference.md), cross-refed
from ADR-007). A reaction's reactant reference = an **optional, summed list of references to
optimized-reactant jobs** whose final energies SUM to E(ref). One job → pre-reaction complex; two+ →
separated reactants; semantics user-labelled, the app sums+labels. **Not a single `reference_job_id`**
(can't express two separated reactants) and **not a cached scalar** (`reference_energy_eh` drifts from
its source — the two-sources-of-truth trap). One source of truth: each energy read from its job's
parsed result at read time. **Representation:** a lean `reaction_reference_jobs(reaction_id, job_id)`
join table, **migration v14 at C2b**, same normalization + jobs-survive rule as C1 (a reference row is
grouping metadata, never deletes a job); a JSON job-id list on `reactions` is noted and rejected for the
same reason `jobs.pathway_id` is a column. **ΔΔE‡ is reference-free**, so C2b ships ΔΔE‡ + intrinsic
barriers with the reference **optional** — the stereoselectivity deliverable does not depend on the
reference machinery.

**Housekeeping.** `index.md` (+chemistry/reaction-barriers.md, +adr-018; page count 77→79), `ROADMAP.md`
(Stage C2b now points at the ratified summed-optional reference + the three-barrier methodology). This
commit is **documentation only** — `reaction_reference_jobs` + the ΔΔE‡/overlay are C2b.

## [2026-08-07] session | Phase 4.5 Stage C2a CLOSED — manual gate m1–m4 PASSED + attach-form overflow fix

**Manual gate PASSED** (author, real Tauri window). m1 create "Ketone + BH₄ (si vs re)" → appears,
opens. m2 attach two completed scan jobs as "si face"/"re face" → both listed with job titles; a
non-scan job in the picker marked/warned. m3 detach one pathway's job → un-groups; **the scan job
still in the Jobs list**, openable with its profile intact. m4 delete the reaction → gone; **both
scan jobs still in Jobs** (jobs-survive, visible).

**Overflow fix** (`AttachPathwayForm`, `ReactionsScreen.tsx`): the **Attach** button spilled past the
card edge. Added `flexWrap: "wrap"` to the attach-form row and `minWidth: 0` to the `flex: 1` job-select
field (the classic flex-item min-content overflow: a `flex:1` child won't shrink below its intrinsic
width without `minWidth: 0`, pushing siblings out). Attach now stays inside the card; wraps to a second
line when narrow. Author confirmed by eye.

**Verify.** `tsc` 0, `vitest` 638, `vite build` clean (no cargo change). Wiki: `ROADMAP.md` (C2a
`[~]`→`[x]`, **Stage C2a complete**), `modules/reactions-ui.md` (gate PASSED).

**Next.** C2b — promote + comparative ΔΔE‡ overlay; `reaction_reference_jobs` (migration v14) per
[ADR-018](architecture/adr-018-reaction-energy-reference.md); reads coordinate/method/profile from the
attached job; comparability guards that C2a's attach leaves advisory.

## [2026-08-07] session | Phase 4.5 Stage C2b-1 — comparative overlay + ΔΔE‡ (reference-free) + method-comparability guard (code complete; manual gate PENDING)

**Built.** The mission "done-when" overlay: a **Compare view** in the reaction detail (shown when ≥ 2
pathways carry a scan profile; < 2 → a clear empty state). `CompareView.tsx` overlays the pathways'
B1 scan profiles on one recharts chart on a **shared zero** (global minimum), one colour + legend
label each, explicit width (no `ResponsiveContainer`), each max marked. Reads nothing new — reuses
`results.scan` (ADR-012), fetched once into a `resultsById` map reused by the attach picker too.

**The numbers are pure + unit-tested** (`src/reactions/compare.ts`, `compare.test.ts` — 12 tests):
- `intrinsicBarrierKcal` = (E(max) − E(min))·627.509 per pathway (self-contained, no reference).
- `deltaDeltaEKcal` = E(max_A) − E(max_B), **reference-free** (ADR-018 — the shared reactant reference
  cancels).
- `pathwaysComparable` — parses the `!`-line **method signature** (identity keywords only; drops
  run-type/SCF-conv/print/PAL; + SMD from `%cpcm smd true`) and the **scan coordinate** (kind+atoms+unit);
  returns the **specific reason** on a mismatch. The UI shows the curves but **replaces ΔΔE‡ with the
  reason** — never a faked number.

**Negative controls (RED→GREEN, both load-bearing ones bite-demonstrated):**
- **C-symmetry-zero** — ΔΔE‡ ≈ 0 on identical/mirror profiles (enantiomeric si/re → 0 by symmetry; a
  sign/reference bug → non-zero). Sign-flip bite turns the "correctly signed" test red.
- **C-guard-refuses** — method/coordinate mismatch → a `reason` (ok when matched); a compute-anyway
  guard turns the three refuse tests red.
- **C-intrinsic** — the (max−min)·627.509 factor on a known profile.

**Honest + reference-free** (carried from B2): maxima are *approximate TS (scan maximum)* / a *screening*
ΔE‡ estimate, never ΔG‡; a note says absolute (vs separated reactants) barriers need a reactant reference
(C2b-2). No reactant reference required or built here (deliberately — ΔΔE‡ needs none).

**Verify.** `tsc` 0; `vitest` 650 (50 files, +12); `vite build` clean; `cargo test` 207 (no Rust
change). Wiki: `modules/reactions-ui.md` (+compare view section), `ROADMAP.md` (C2b split into **C2b-1
`[~]`** + C2b-2), `index.md`.

**Manual gate (author, real window) — PENDING; the unit stays open until it passes.**
- c1: two scan pathways → both profiles overlaid, legend-labelled, shared zero; intrinsic barriers listed; maxima approximate-TS.
- c2: ΔΔE‡ shown when method+coordinate match; **on two identical/mirror scans ΔΔE‡ ≈ 0** (symmetry sanity).
- c3: two scans with different methods → curves shown, **ΔΔE‡ replaced by the reason**.
- c4: < 2 scan pathways → the clear empty state, no crash.

**Next.** Author runs c1–c4 → the Phase 4.5 mission "done-when" is met. Then C2b-2 — `reaction_reference_jobs`
(migration v14) + absolute barriers on the overlay (reference summed, optional; jobs-survive like C1).

## [2026-08-07] session | fix(results): GOAT jobs render the conformer ensemble, not a single-structure trajectory (regression, debugging/017)

**Symptom (author, real butanone GOAT).** A new GOAT job showed the standard Results dashboard ("17
optimization cycles" trajectory) + status `parsed`, instead of the **Conformers (N)** ensemble the old
ibuprofen GOAT job (`completed`) showed.

**Measured first (rule #10) — corrected the stated hypothesis.** *Both* GOAT jobs have a `property.txt`
(the "present vs absent" guess was wrong). The real trigger: the new job has a `results` row at the
current `parser_version 4` — the current `parse_and_store` **ran the single-structure readers on the
GOAT `property.txt` and they SUCCEEDED** (17 `$Geometry` cycles, first ≈ input) → status `parsed`. The
old job predates that (no results row, stayed `completed`). The ensemble panel's guard keyed on exactly
`status === "completed"`, so a `parsed` GOAT hid its ensemble. `finalensemble.xyz` present for both →
display bug, not a missing-ensemble run.

**Fix (GOAT is a special job type, like a scan — mirrors debugging/015).**
1. `results.rs::parse_and_store` **routes a GOAT input past the single-structure readers**
   (`input_is_goat` → `ParseOutcome::NoArtifact`), so the job stays `completed` and no results row is
   stored; the ensemble is read separately. (`NoArtifact` doc generalized to "no single-structure
   artifact / special job type read elsewhere".)
2. `JobDetailScreen` reads the ensemble on any **terminal success** (`isTerminalSuccessStatus` =
   `completed` **or** `parsed`) — so the already-`parsed` butanone job still shows its ensemble.
3. `showsSingleStructureResults = !isGoatInput` **suppresses `ResultsCard`** for a GOAT job (its
   "N optimization cycles" trajectory is misleading); a GOAT job with no readable ensemble shows a plain
   note. Non-GOAT untouched.

**Controls (RED→GREEN).** C-goat-not-parsed (Rust `goat_dir_is_routed_past_the_single_structure_readers`
— **bite-verified**: removing the branch → outcome `Parsed`, the exact regression). C-goat-parsed-shows-
ensemble (`isTerminalSuccessStatus("parsed")` true — the narrow guard returned false; `showsSingleStructureResults(goat)`
false). C-nongoat-unaffected (`showsSingleStructureResults(opt)` true; the existing Opt→`Parsed` and
scan-routing tests stay green).

**Verify.** `cargo test` 208 (+1); `tsc` 0; `vitest` 655 (+5); `vite build` clean. Wiki:
+`debugging/017`, `orca/goat.md`, `modules/results-ui.md`, `index.md`.

**Manual gate (author, real window) — PENDING.** Open the butanone GOAT job → the **Conformers (N)**
panel shows (ΔE kcal/mol, "Use this conformer"), the "17 optimization cycles" trajectory is gone; a
normal Opt job → still the Results dashboard; a scan job → still the profile. Note: the existing butanone
job's DB row stays `parsed` (not mutated) — the frontend fix renders it correctly regardless; only
FUTURE GOAT jobs stay `completed`.

## [2026-08-08] session | Phase 4.5 Stage C2b-2a — reaction reference jobs (summed reactant reference, migration v14, honest-or-absent)

The data half of C2b-2 (ADR-018): the summed reactant reference for **absolute** barriers. Pure
Rust + SQLite + cargo, **no manual gate**.

**Migration v14** (`SCHEMA_VERSION` 13 → 14, guarded + idempotent). New join table
`reaction_reference_jobs(reaction_id TEXT REFERENCES reactions(id), job_id TEXT REFERENCES jobs(id),
created_at, PRIMARY KEY (reaction_id, job_id))` — a reaction has 0+ references to optimized-reactant
jobs whose parsed final energies SUM to `E(ref)`. Preservation test
`migrate_v13_to_v14_adds_reference_jobs_and_preserves_data` (v13 data intact, idempotent).

**Commands** (`commands/reactions.rs`, `ReferenceJob`/`ReferenceEnergy` in `models/reaction.rs`,
registered in `lib.rs`): `add_reference_job` (NotFound if either id absent; idempotent on the PK via
`INSERT OR IGNORE`), `remove_reference_job`, `list_reference_jobs → Vec<ReferenceJob{ job_id, title,
final_energy_eh: Option<f64> }>`, `reaction_reference_energy → { jobs, energy_eh }`.

**Honest-or-absent (load-bearing, ADR-018):** `E(ref) = Σ final_energy_eh`, read on demand from the
authoritative `results` tier — **never cached** on `reactions`. `energy_eh = Some(Σ)` **only if the
list is non-empty AND every reference job is parsed**; any unparsed job → incomplete → `None`, with
`jobs` still listing all of them (missing one's energy `None`) so the C2b-2b UI can name it. A partial
sum is never returned. Expressed totally via `Option<f64>: Sum<Option<f64>>` (no `unwrap`).
**Jobs-survive:** `delete_reaction` also drops the reference rows (child-first); `remove_reference_job`
drops the grouping row only — neither ever deletes the job. Four cargo controls
(`reference_energy_incomplete_is_none_not_partial`, `reference_energy_sums_when_all_parsed`,
`delete_reaction_keeps_reference_jobs`, `add_reference_job_integrity_and_idempotent`); the
incomplete-not-summed (partial `filter_map` sum) and delete-keeps-jobs (naive job cascade) controls
are **bite-verified**. 213 Rust tests green.

**Decided/measured (rule #10):** jobs are **not deletable today** (no `delete_job` command) — noted as
the future integrity point (a delete must also drop `reaction_reference_jobs` rows by `job_id`).
**Correction:** the bundled SQLite is compiled with `SQLITE_DEFAULT_FOREIGN_KEYS=1` (measured:
`PRAGMA foreign_keys` = 1, `pragma_compile_options` lists `DEFAULT_FOREIGN_KEYS`), so `REFERENCES`
clauses are **actively enforced** — the older codebase comment "this DB leaves FK enforcement off" is
false; delete ordering is load-bearing (child before parent, else error 787). Corrected in
`db.rs`, `commands/reactions.rs`, and `modules/tauri-core.md`.

**Next:** C2b-2b — the reference-management UI + absolute barrier E(max) − Σ E(ref job) in the compare
overlay (manual gate).

## [2026-08-08] session | Phase 4.5 Stage C2b-2b — reactant-reference UI + absolute barriers vs separated reactants (code complete; manual gate r1–r4 PENDING)

The UI half of C2b-2 (ADR-018), closing the ΔΔE‡ story: manage the reactant reference (C2b-2a
commands) and show the **absolute barrier vs separated reactants** = E(max) − Σ E(reactant jobs)
alongside the intrinsic barriers. Additive — ΔΔE‡ + intrinsic (C2b-1) unchanged; the reference is
optional (no reference → exactly C2b-1).

**Pure logic** (`src/reactions/compare.ts`, unit-tested — the chart carries no correctness weight):
`absoluteBarrierKcal(maxEh, refEh)` = (max − ref)·627.509; `referenceComparable(refInputs,
pathwaySig)` reuses `methodSignature` to refuse a reference-vs-pathway method mismatch;
`absoluteBarrierCell(maxEh, refEh|null, refInputs, pathwaySig, refJobCount)` centralizes the
**honest-or-absent** decision (`{ kcal } | { reason }`) so the UI cannot treat a `null` (incomplete)
reference as `0`. Controls in `compare.test.ts`: **C-absolute-barrier**, **C-ref-method-mismatch**
(bite-verified — a compute-anyway guard turns 3 tests red), **C-incomplete-no-number** (bite-verified —
treating null as 0 turns it red). 28 reactions tests, 665 vitest total, tsc + vite build clean;
cargo unaffected (213).

**UI** (`CompareView.tsx` + `ReactionsScreen.tsx`): `ReferenceJobsSection` reads
`reaction_reference_energy`, lists each reference job's `final_energy_eh` (or "no parsed energy") + the
summed E(ref) — or "**incomplete — Missing: job X**" when `energy_eh` is null; add/remove via
`add_reference_job`/`remove_reference_job` (picker marks scan vs optimized; candidates not filtered on
`pathway_id`). The overlay gains a **"separated reactants" zero** (enabled only when E(ref) complete +
all pathways method-match the reference; re-zeros curves on E(ref)) and an **absolute-barrier column**
(number only where complete + method-matching, else the reason). Honest labelling: absolute = screening
ΔE‡ vs separated reactants, **not ΔG‡** (Stage E). Semantics user-labelled (1 job = complex; 2+ =
separated reactants).

**Manual gate (author, real window) — PENDING (r1–r4):** r1 two optimized reactant references → summed
E(ref) + absolute barriers; r2 remove/unparsed → "incomplete", no number; r3 different-method reference
→ refused with reason, ΔΔE‡ still shows; r4 no reference → exactly C2b-1. The unit stays open until it
passes.

**Next** (ratified reorder): CREST probe → Stage D (conformer→reaction-center rigor) → Stage E/F (ΔG‡).

## [2026-08-08] session | CREST 3.0.2 QCG microsolvation probe (Phase 4.5 Stage F probe — measure-only, xtb 6.6.1 linkage verified)

Measure-only probe (rule #10) — no production code (nothing under src/ or src-tauri/). New page
`wiki/orca/crest.md`; the only deliverables are the runs + the recorded measurements.

**Rule #2 linkage — CONFIRMED.** CREST 3.0.2 (`/opt/crest/crest`, GNU static, commit af7eb99) QCG
shells out to the external `/usr/bin/xtb` **6.6.1** (quoted: `* xtb version 6.6.1 …` + `program call
: xtb solute --gfn2 --sp` from the kept `-keepdir` tmp). `-alpb <solvent>` reaches the cluster
optimization. Nuance: `normal termination of xtb` is NOT in the kept dirs (CREST keeps xtb **stdout**;
the banner is on **stderr**) — prove xtb ran via the version banner + program-call + `.xtboptok`, and
CREST's own `CREST terminated normally.`

**Per-rung (all `-T 4 -keepdir`, isolated dirs, rule #3):**
- **Rung 0** benzoic acid + 3 H₂O `-grow -alpb water -nofix` → terminated normally, **10.15 s**, 24-atom cluster. Clean baseline.
- **Rung 1** BH₄⁻ + 3 MeOH `-grow -alpb methanol -chrg -1 -fixsolute` → terminated normally, **8.68 s**, 23-atom cluster.
- **Rung 2** same `-ensemble` → grow OK, **ensemble SEGFAULTS** (reproducible, CREGEN `newcregen_`); `-enslvl gfn2` → MTD non-convergence (exit 1). Ensemble unusable for this ionic system.

**Two blocking findings for the ionic (mission) case:**
1. **QCG grows/optimizes the anion cluster as NEUTRAL (rule #9 footgun).** `-chrg -1` reaches only
   the solute monomer preopt (`.CHRG=-1`); the grow-phase docking (`charge of molecule A : 0.0`) and
   cluster opt (no `.CHRG`, `total charge 0.0`) run neutral. "Terminated normally, wrong charge."
2. **`-ensemble` crashes** on the ionic system (both GFN-FF default and `-enslvl gfn2`).

**ALPB/GBSA only — no SMD** at this level (that is the later ORCA refinement; SMD-over-ALPB for ions).

**E-vs-F implication:** **do Stage E (gas-phase ΔG‡) before Stage F.** QCG is cheap + correctly linked
but not production-trustworthy for the anion until the charge-on-cluster + ensemble-crash issues are
resolved (not our bug to fix). Pragmatic F path when built: use QCG's neutral grown shell as a
**geometry seed only**, re-optimize the cluster **in ORCA at the correct charge with SMD**. ROADMAP
Stage F: the install+probe item marked `[x]` (probe, not the feature); the design/build item stays open.

**Next:** Stage E (OptTS + Freq + thermochemistry → true ΔG‡), per the sharpened reorder (E before F).
Also still pending: the C2b-1 (c1–c4) and C2b-2b (r1–r4) author manual gates.

## [2026-08-08] session | Phase 4.5 Stage D unit D1 — Boltzmann populations over the GOAT ensemble (xTB-level)

**Scope (one unit).** Add Boltzmann populations to the conformer-ensemble panel: a pure function
plus two display columns. Nothing else — no DFT re-opt, no orchestration, no migration, no new job,
no data-model change. Those are D2 (next).

**`boltzmannWeights(conformers, tempK = 298.15)`** in `src/scene/ensemble.ts` (TS-only; NOT a
Rust/TS byte-identical pair — this is derived display analysis, not emit-to-input-file text, so no
`orcastudio-core` mirror, stated in a comment). `w_i = exp(−ΔE_i/RT) / Σ exp(−ΔE_j/RT)` over the
FINITE-energy conformers, `R = 1.987204259e-3` kcal/(mol·K), reusing `deltaEKcal` so every exp
argument is ≤ 0 (the relative-to-min form IS the overflow guard). **NaN contract** mirrors
`deltaEKcal` (rule #9, honest-or-absent): a NaN-energy conformer gets weight NaN and is EXCLUDED from
the normalization sum, so the finite weights still sum to 1. Empty → `[]`; `tempK ≤ 0` throws.

**Derived, never stored** (same one-source-of-truth rule the C2b absolute-barrier work settled):
populations + cumulative computed at render in `JobDetailScreen` (`useMemo`), no migration, no column,
nothing persisted. Beside ΔE the panel now shows **Population** (percent) and **Cumulative** (Σ,
running total down the energy-sorted list — this is what D2's k-selection reads), NaN weight → "—",
and an honest **xTB/GFN2-level, 298.15 K** label. No temperature UI yet (the `tempK` param exists for
D2/later); no scope creep.

**Tests** (`ensemble.test.ts`, real butane fixture, +7 → 33 file / 671 suite pass; `tsc` clean):
sum-to-1 over finite conformers, monotonicity (min largest), butane sanity (anti leads but < 90 % —
several conformers populated, the teaching point), NaN-weight-excluded-still-sums-to-1, and
T-flattening (min's weight strictly decreases as T rises).

**Manual gate (real data, through the actual panel functions).** No `sqlite3` on the box and the
desktop GUI can't be click-driven headless, so verified the exact `deltaEKcal`/`boltzmannWeights`
computation the panel calls against a **full real on-disk ensemble** (job
`04aeca22…`/`input.finalensemble.xyz`, 29 conformers) via a throwaway test: populations sum to
1.000000000; cumulative climbs 15.1 % → 100.0 %, reaching 100 % at #19 with the high-energy tail
adding ~0 %; four near-degenerate conformers at ~15 % each dominate then a tail — chemically sane, and
exactly the cumulative-threshold signal D2 needs. A live click-through of the running app was NOT
performed (offered to the author).

**Wiki:** `chemistry/conformers.md` (+«Больцман-заселеність» subsection, Ukrainian — R·T scale,
xTB-level caveat, cumulative → k-selection tie to D2); `modules/frontend.md` (ensemble-panel line,
present tense + pointer here); `index.md` conformers line.

**Next: D2** — DFT re-opt fan-out over the top-k conformers (k chosen by the cumulative threshold),
which will re-rank and re-weight; xTB- vs DFT-level populations must never be conflated.

## [2026-08-08] session | Phase 4.5 Stage D unit D2a — DFT conformer re-opt fan-out (CREATE side) + migration v15 + SMD determiner run

**Scope (one unit, create side only).** Fan out DFT re-opt children over a GOAT ensemble's
lowest-k conformers: migration v15, a pure charge-safe child-input builder, the trigger UI, and
k queued job creations tagged back to the source. OUT (D2b, next): reading child energies back,
re-rank/re-weight, any xTB-vs-DFT comparison UI. Nothing reads the linkage FKs yet.

**A1 — migration v15** (`db.rs`): two nullable `jobs` FKs — `source_ensemble_job_id TEXT` +
`source_conformer_index INTEGER`. NO new table (the fan-out set is DERIVED by GROUP BY in D2b,
Fork 1). Guarded ALTER; jobs-survive (deleting the source nulls the link, never cascades). Test
`migrate_v14_to_v15_adds_source_fks_and_preserves_jobs` (pre-existing job intact + NULL, a child
carries both FKs, second migrate no-op). **cargo test green — the container DID have a Rust
toolchain (215 lib tests pass), contrary to the prompt's assumption.**

**A2/A3 — pure builder** `buildReoptInput(sourceInputText, conformer, opts)` in new
`src/scene/reopt.ts`. Reuses `sceneFromOrcaInput` (extract source c/m — the existing parser, NO
new charge regex), `sceneFromAtomLines` (conformer scene), `buildOrcaInput` (assembly). TS-only,
no `orcastudio-core` mirror (a proposal emit, not an order-bearing golden pair — reasoned in a
comment). **Charge-footgun post-condition (rule #9):** re-parses the EMITTED child and asserts
(c,m) == source AND atoms == conformer (count + element order), else throws. 10 byte-level tests:
`-1 1` anion / `0 2` radical / `+2` cation propagation, throw-on-no-`* xyz`-block, default
`r2SCAN-3c Opt Freq`, Freq-iff-opts, custom method, SMD-iff-solvation, count+order preserved
(incl. a real 14-atom butane conformer).

**SMD determiner run (rule #10, blocking Part B — settled by a real run, not memory).** Author
flagged that "reuse build-input's SMD emit" (keyword `SMD(x)`) and the gate's `%cpcm smd true`
block are different syntaxes. Ran 3 tiny water SPs (ORCA 6.1.0, full path, isolated dirs): the
keyword form `! … SMD(methanol)` IS **real SMD** — "utilizes the SMD solvation module", SMD-CDS
Gcds term present, FSPE `-76.430993230852` **bit-identical** to the explicit `%cpcm smd true /
SMDsolvent "methanol" end` block; the `CPCM(methanol)` control has NO CDS and a different FSPE.
**Conclusion: the input-builder's keyword `SMD(x)` emit is correct real SMD — no builder bug.**
Pinned in NEW `wiki/orca/solvation.md`. Part B wired with the verified keyword form.

**B1/B2/B3 — wiring.** JobDetail ensemble panel gains "Re-optimize top-k at DFT": k (+ the D1
cumulative-% at k, Fork 3), method (default r2SCAN-3c), Opt+Freq(default)/Opt-only mode toggle,
SMD toggle+solvent, "creates k jobs" note. On trigger: build+charge-check ALL k inputs FIRST (a
mismatch aborts the fan-out having created nothing), then `create_reopt_job` (queued, tagged with
the two FKs) + `submit_job` each. New Rust command `create_reopt_job` reuses `create_job_conn`
then stamps the FKs; create-boundary post-condition = source ensemble job must exist (clean
NotFound). Mode lives in the child input (D2b auto-detects). All k share one mode.

**Manual gate (real data, headless — no GUI click-through).** (1) Ran a real **BH₄⁻ GOAT**
(`* xyz -1 1`, `! XTB GOAT`, full path, isolated dir — TERMINATED NORMALLY); its ensemble
(1 conformer, tetrahedral) built through `buildReoptInput` with SMD(methanol)+Opt+Freq → child
carries **`* xyz -1 1`** (NOT `0 1`), `! r2SCAN-3c SMD(methanol) Opt Freq`, the 5-atom geometry —
**the footgun proven on real anion data.** (2) Real 33-atom/29-conformer ensemble, k=3 → **3
distinct children**, each `! r2SCAN-3c Opt Freq` / `* xyz 0 1` / 33 atoms with distinct
per-conformer coordinates (multi-child loop). The DB-insert + queue path is covered by the Rust
`create_reopt_job` unit test, not a live GUI click.

**Verification:** `tsc` clean; vitest 681 pass (reopt +10 → 51 files); cargo 215 lib pass (v15 +
reopt create tests). **Next: D2b** — aggregate the children's DFT energies (GROUP BY
source_ensemble_job_id), re-rank + re-weight vs the xTB populations, and the comparison UI;
xTB-level and DFT-level populations must never be conflated.

## [2026-08-08] session | Phase 4.5 Stage D unit D2b — DFT re-opt aggregate + xTB-vs-DFT re-ranking (READ side)

**Scope (one unit, read/aggregate side).** Read a GOAT job's DFT re-opt children back, re-rank +
re-weight them vs the xTB populations, and show the comparison on the same job's detail. OUT (D3):
no job creation, no migration, no "use best conformer" wiring, no reaction_centers.

**Rust — `read_conformer_reoptimization(source_job_id)`** (`commands/jobs.rs`). The set is DERIVED:
`WHERE source_ensemble_job_id = ?1 ORDER BY source_conformer_index` LEFT JOIN `results` (a child
with no parsed results still appears — never dropped, rule #9). Per child: status + DFT electronic
energy (`final_energy_eh`) + Gibbs G (`free_energy_g_eh`, None unless Freq) + `imaginary_count` +
`freq_requested` (from the input) + `element_mismatch` (its `* xyz` composition vs the source
ensemble's — the post-condition). NO weighting in Rust. `mode_inconsistent` flag when children
disagree on Freq. Two tests: grouping + G-present/absent/no-results children; element-mismatch +
mixed-mode flags.

**TS — `aggregateReopt(raw, ensemble)`** (`src/scene/reopt-aggregate.ts`). REUSES `boltzmannWeights`
/`deltaEKcal` (one Boltzmann impl). Mode auto-detect: ΔG iff every child ran Freq, ΔE iff none,
else mixed (weighted on electronic E, warned). **Honest-or-absent:** not-terminal / composition-
mismatch / saddle (imaginary>0) / no-usable-energy → EXCLUDED + listed with a reason, never a fake
weight. xTB and DFT pops computed over the SAME included subset (comparable, never combined);
within-subset ranks → `rankChanged` per conformer. 8 tests: ΔG weighting, ΔE fallback +
not-minimum-validated, failed/imaginary/running exclusions, no-G-in-ΔG-mode exclusion, composition
mismatch, rank-change detection, mixed-mode.

**UI** on the GOAT JobDetail (below D1 panel + D2a trigger, recomputed on open): per-conformer
`xTB #/ΔE/pop | DFT #/ΔG-or-ΔE/pop`, DFT column labelled by detected mode (never ΔG on a set
missing a G), reordered rows highlighted, excluded children listed with reasons, "n of k complete
— provisional, not for decisions" banner until all terminal, "not frequency-validated" caveat in
ΔE-mode. Nothing stored.

**Manual gate — REAL DFT data (headless; could not click the running app).** D2a only *built* the
children (didn't run the slow 33-atom DFT jobs), so no completed children existed. Ran the pipeline's
real functions end-to-end instead: r2SCAN-3c Opt+Freq (gas phase) on the 3 real butane fixture
conformers via `/opt/orca/orca` (isolated dirs), parsed real Gibbs G, fed through the actual
`aggregateReopt`. Result — a **dramatic real reorder** (all 0 imaginary, clean minima):
- conf #3: xTB rank 3 (+2.57 kcal/mol, 1.0 % pop) → **DFT rank 1** (43.6 % pop)
- conf #1: xTB rank 1 (72.5 % pop, the xTB "winner") → **DFT rank 2** (43.6 %)
- conf #2: xTB rank 2 → DFT rank 3 (+0.73 kcal/mol ΔG, 12.7 %)

`mode=dG, provisional=false, reordered=true, included=3/3`, pops sum to 100 %. Physically conf #3
relaxed into the anti basin under DFT opt and tied conf #1 (both anti) — xTB had mis-ranked its
energy. Exactly the teaching point: trusting the xTB winner could pick the wrong conformer for a
reaction center. Pinned in `chemistry/conformers.md` (real table).

**Verification:** tsc clean; vitest 689 pass (+8 aggregate → 52 files); cargo 217 lib pass (+2 D2b).
**Next: D3** — "use the best (DFT) conformer" into a reaction center + C2b-2b reference convergence.

## [2026-08-08] session | Phase 4.5 Stage D unit D3 — carry the DFT winner downstream + scope-correct the re-rank caption (Stage D CLOSED)

**Scope (one unit, closes Stage D).** (1) let the user carry the DFT-re-optimised winner downstream
(not the raw xTB frame); (2) the caption honesty scope-fix. OUT: no reaction_reference_jobs/C2b-2b,
no migration, no reaction_centers.

**Task 1 — caption scope-fix (Pattern 2, honesty).** `reordered = rows.some(rankChanged)` is too weak
to claim "the xTB minimum is not the DFT minimum" — false when the minimum holds and only the tail
reorders. Split into two tie-aware flags in `aggregateReopt`, using `CO_MINIMUM_TIE_KCAL` (0.05
kcal/mol, display-only — weighting uses exact energies): **`minimumChanged`** (no xTB-best is among
the DFT co-minima → the strong "wrong conformer" case) and **`dismissedRoseToTop`** (a DFT co-minimum
that wasn't an xTB best → a dismissed conformer rose to the top). Caption picks exactly one:
minimumChanged → "xTB minimum is NOT the DFT minimum"; else dismissedRoseToTop → "minimum held, but a
lower-ranked conformer is a DFT co-minimum"; else "re-ranked below the top". 4 tests: minimum-held-
tail-reordered (ibuprofen shape), minimum-changed, tie-within-tolerance, just-outside-tolerance.

**Task 2 — carry the DFT geometry downstream.** New `useDftConformer` in JobDetail: reads the child
job's parsed **`results.final_geometry`** (Phase 3, the DFT-optimised structure — NOT the xTB frame),
builds a Conformer, and applies it via the SAME 2.5.1b path (`planConformerApply` → replace/new/refuse).
That path's composition refusal (atom count + element order) IS the rule-#9 post-condition — asserted,
not assumed. Op-log records `level: "DFT"` + method (`FragmentGeometryVia.conformer` gained optional
`level`/`method`), so the carried level is never ambiguous. Each D2b row gets a "Use (DFT)" button
("Use best (DFT)" on DFT rank-1); only completed clean children (the `included` rows) are offered.
`method` added to the Rust `ReoptChild` (first `!`-line token) → threaded to the row. Refactored the
shared apply logic into `applyConformer` (used by both `useConformer` and `useDftConformer`). 3 tests:
DFT-geometry composition post-condition (same order accepted; reordered/short refused).

**Manual gate — REAL data (headless; could not click the app).** (a) Ran `aggregateReopt` on the real
butane DFT G values (D2b run): `minimumChanged=false, dismissedRoseToTop=true` — the strong "minimum
is not the minimum" caption NO LONGER fires (the D2b over-statement is fixed); conf #3 (xTB rank 3)
rose to DFT co-minimum with conf #1 (anti held). (b) Ran r2SCAN-3c Opt on the anti conformer: optimised
C1 `1.95200, 0.07815, 0.24081` ≠ xTB frame C1 `1.93939, 0.07737, 0.24304` (Δx ≈ 0.013 Å) — so "Use
best (DFT)" carries a genuinely different, DFT-level geometry.

**Verification:** tsc clean; vitest 696 pass (+7: 4 caption + 3 DFT-geom post-condition → 52 files);
cargo 217 lib pass (method assertion added). **Stage D CLOSED.**

**ROADMAP stub recorded (not lost):** new "Phase 4.7 — Job organization & lifecycle (planned)" with
three `[ ]` bullets (ADR-019 job groups = a tree in SQLite, not a filesystem hierarchy; job deletion
DB-only vs DB+files with FK links NULLed not cascaded + running-guard; job grouping + filter/search).
**Next: ADR-019 + its decomposition (a separate session).**

## [2026-08-08] decision | ADR-019 job organization — groups as a SQLite tree (not disk), jobs-survive, Phase 4.7 decomposed

Ingested a design decision settled in a Claude web session (documentation-only — no schema, no
migration, no code). Context: the flat job list stops scaling once a researcher runs many reaction
studies; the app needs **unlimited nested grouping** ("reduction of ibuprofen by LiAlH4" as a folder
with sub-folders) plus the ability to **delete a job**.

**ADR-019** (`wiki/architecture/adr-019-job-organization.md`) records:
- **LOAD-BEARING INVARIANT (stated first):** job groups are a **TREE OF METADATA IN SQLITE, NEVER a
  filesystem hierarchy**. A job keeps its isolated `job_dir` (rule #3) wherever it sits; moving a job
  is `UPDATE jobs.group_id` — **zero filesystem ops**; the dir path never follows the logical group.
  Preserves rule #3, crash-reconciliation, killpg-by-cwd. Cites **ADR-017** as the sibling precedent
  (logical model decoupled from disk/backend).
- **Schema (proposed, NOT applied):** `groups(id, name, parent_id NULL REFERENCES groups, created_at)`
  adjacency list (parent_id NULL = root); `jobs.group_id … ON DELETE SET NULL` (one group per job — a
  tree, not tags).
- **Adjacency-list over closure-table / materialized-path** at hundreds-scale (rejected-alts recorded;
  materialized-path's rename-rewrites-descendants is the same disk-coupling smell we reject).
- **Jobs-survive:** group-delete orphans jobs to root via `SET NULL` (FK enforcement ON,
  `SQLITE_DEFAULT_FOREIGN_KEYS=1` — load-bearing) and **PROMOTES** sub-groups + jobs to the deleted
  group's parent. So `parent_id` is **NOT `ON DELETE CASCADE`** — promotion is re-parent-then-remove in
  the command (post-condition: count conserved, no dangling FK, no `job_dir` touched).
- **Generic tree, jobs-only FK now** (molecules/reactions `group_id` deferred, zero churn);
  `group_id` **orthogonal** to the pipeline FKs (source_ensemble/reference/pathway — a job can be in a
  group AND a re-opt child AND a reference). Term = "group", UI metaphor = "folder".
- **Boundary:** deleting an actual JOB (DB-only vs DB+files, running-guard killpg+cwd-sweep) is a
  SEPARATE 4.7 unit, named not designed here.

**ROADMAP Phase 4.7** expanded from the stub into four ordered units referencing ADR-019: 4.7.1 job
deletion (independent, can land first) · 4.7.2 groups schema + model (data + tests before UI) · 4.7.3
group nav UI (tree sidebar, assign-on-create, move-leaves-job_dir-untouched gate) · 4.7.4 filter/search
(plain LIKE, not FTS5). All `[ ]`. **Next: implement 4.7 units** (start 4.7.1 or 4.7.2).

## [2026-08-09] session | Phase 4.7.1 — delete a job (DB+files, guarded, FK cleanup, terminal-states-only)

Implemented job deletion — the first Phase 4.7 unit (ADR-019's "delete an actual JOB" boundary,
independent of grouping). **No schema change** (SCHEMA_VERSION stays 15 — every FK already exists).

**DB core** (`commands/jobs.rs::delete_job_conn(&Connection, id) -> Option<String>`): not-found
first (mirrors `delete_reaction_conn`); **terminal-states-only** guard — `Running`/`Queued` → `Err`
("cancel it first"), **no killpg in the delete path** (the ratified simplification: terminating a
live run is `cancel()`'s job). FK cleanup in **one `unchecked_transaction`, in this order**:
(1) `UPDATE jobs SET source_ensemble_job_id = NULL, source_conformer_index = NULL WHERE
source_ensemble_job_id = ?` (a DFT re-opt child becomes standalone); (2) `DELETE FROM
reaction_reference_jobs WHERE job_id = ?` (the reaction survives, loses one reference);
(3) `DELETE FROM jobs` — **cascades** to `results` (`ON DELETE CASCADE`; never deleted by hand).
`pathway_id` points OUT, vanishes with the row. Returns the `job_dir` for the caller to remove after
commit. **Jobs-survive both ways** (like v13 `delete_reaction`, v14 `remove_reference_job`).

**Guarded fs removal** (`local_backend.rs`): `remove_job_dir(app, job_dir)` computes the jobs root
as `runner.data_dir/jobs` and removes the dir **only if** `path_is_within(root, dir)` — canonicalized
root-or-descendant; non-existent path or symlink escape → refuse. Best-effort (`eprintln` on error,
never fails — the row is already gone). **Exactly one `remove_dir_all` in the diff, reached only
through the guard.** Command `delete_job(app, id)` (registered in `lib.rs` next to `cancel_job`):
locks the DB, calls `delete_job_conn`, drops the lock, then removes the dir if `Some`.

**UI** (`JobsScreen.tsx`): a **Delete** button (`.btn-danger`, new CSS) on **terminal-state rows
only** (draft/completed/parsed/failed/cancelled) beside Run/Open; running/queued keep Cancel alone.
`confirm` (`@tauri-apps/plugin-dialog`, warning kind, "removes the job AND its files") →
`invoke("delete_job")` → reload; `stopPropagation` on both action buttons. Errors → existing banner.

**Two ratified decisions** (from the continuation spec): DB+files guarded; terminal-states-only.

**Verification.** `cargo test` 231 passed (was 217, +14) + the second binary 24; `tsc` clean;
`vitest` 696 passed (frontend count unchanged). **Negative control bit red** —
`raw_delete_without_cleanup_hits_fk_constraint` asserts a bare `DELETE FROM jobs` (with a re-opt
child / a reference row still pointing) errors on the RESTRICT FK, proving `SQLITE_DEFAULT_FOREIGN_KEYS=1`
enforcement is ON and the cleanup is required. **Manual gate on real data (rule #9, throwaway test
run then removed):** built a GOAT source + re-opt child + results row + `reaction_reference_jobs`
row, real dirs at the source's `job_dir` and a sibling's under `data_dir/jobs/`; `delete_job_conn` +
the guarded removal → (a) child `source_ensemble_job_id`/`source_conformer_index` NULL, (b) 0
reference rows, (c) 0 results (cascade fired), (d) source dir gone AND sibling untouched, (e)
reaction + child still exist. The live Delete-button + confirm render is Anton's eyeball gate.

**db.rs comment fix (in-scope landmine):** the v13 doc-comment still claimed "this DB leaves SQLite
FK enforcement off" — **false**, contradicting the measured v14 note (enforcement ON, `PRAGMA
foreign_keys` reads 1). Corrected to state enforcement is ON and integrity is enforced both by SQLite
and the commands.

**Next: Phase 4.7.2** — groups schema + model (migration `groups` table + `jobs.group_id`, Rust CRUD
with delete-as-promotion).

## [2026-08-09] session | Phase 4.7.2 — job groups data layer (migration v16 + Rust CRUD; cycle-guard + promotion-on-delete)

Built the groups **data layer** (ADR-019 Decisions 0–5) — schema + Rust CRUD, **no UI** (the tree
sidebar is 4.7.3). Jobs-only FK for now (molecules/reactions `group_id` deferred — Decision 4).

**Migration v16** (`db.rs`, SCHEMA_VERSION 15 → 16, one additive arm): `groups(id, name, parent_id
TEXT REFERENCES groups(id), created_at)` — **adjacency list**, `parent_id` **NOT `ON DELETE CASCADE`**
(a cascade would destroy a subtree — the opposite of promotion). Guarded `ALTER TABLE jobs ADD COLUMN
group_id TEXT REFERENCES groups(id) ON DELETE SET NULL` (one group per job — a tree, not tags).
Migration test `migrate_v15_to_v16_adds_groups_table_and_group_id_and_preserves_jobs`.

**Model** `models/group.rs` — `Group { id, name, parent_id: Option<String>, created_at }` + `COLUMNS`
+ `from_row` (mirrors `Reaction`). Registered in `models/mod.rs`.

**CRUD** `commands/groups.rs` (`_conn` helpers + thin `#[tauri::command]` wrappers; registered in
`commands/mod.rs` + `lib.rs`): `create_group(name, parent_id?)` (parent must exist → NotFound else),
`list_groups`, `rename_group`, `move_group(id, new_parent_id?)`, `move_job(job_id, group_id?)`,
`delete_group(id)`.

**Two load-bearing rules** (FK enforcement ON, `SQLITE_DEFAULT_FOREIGN_KEYS=1`):
- **Cycle guard on `move_group`** (RISK 1): refuses a self-parent or a descendant-parent via a
  **bounded** walk up the new parent's chain (bounded by `COUNT(*) FROM groups`, so a pre-existing
  corrupt cycle can't hang the check either). Error → `AppError::Backend`.
- **Promotion-to-parent on `delete_group`** (RISK 2, ADR-019 Decision 3): in one transaction —
  `UPDATE groups SET parent_id=<parent> WHERE parent_id=?id`, then `UPDATE jobs SET group_id=<parent>
  WHERE group_id=?id` (**explicit** — NOT the `ON DELETE SET NULL` drop-to-root), then `DELETE FROM
  groups WHERE id=?id`. A deleted folder's contents rise one level; nothing under it is destroyed,
  no job is ever deleted. Post-condition (rule #9): no dangling `group_id`, count conserved, no fs.

**Pure metadata (Decision 0):** the whole module makes **zero filesystem calls** (production code) —
moving a job is `UPDATE jobs.group_id`, the `job_dir` never moves, so rule #3 / crash-reconciliation /
killpg-by-cwd are all preserved.

**Verification.** `cargo test` **243** (was 231, +12: 11 group tests + 1 migration test); `tsc` clean;
`vitest` **696** (frontend UNCHANGED — no .tsx touched). **Both negative controls bit red:**
`move_group_refuses_cycle` (self- + descendant-parent both error, parents unchanged) and
`raw_delete_without_reparent_hits_restrict_fk` (a bare `DELETE FROM groups` with a child trips the
RESTRICT FK — proves enforcement is ON and the promotion re-parent is required). Headline
`delete_group_promotes_children_to_parent_not_root` proves a job lands at the PARENT, not root (the
SET NULL trap); `delete_group_conserves_count_under_parent` for conservation + no-dangling post-cond.

**Adjacent stale-fact fixes (in the same commit):** the tauri-core.md "jobs are not deletable today"
note is now false (delete_job landed in 4.7.1) — corrected; and v15/v16 added to the migrations
catalog (the list had jumped v14 → gap).

**Next: Phase 4.7.3** — the group navigation UI (tree sidebar, assign-on-create, move-leaves-job_dir).

## [2026-08-09] session | Phase 4.7.3 — group navigation sidebar (tree, deep filter, move-to picker, assign-on-create)

Built the group navigation **UI** over the Jobs view (ADR-019 Decision 3 + the group-aware-create
consequence). Composes the 4.7.2 commands — the only Rust change is surfacing `Job.group_id`.

**Pure logic** `src/groups/tree.ts` (vitest, no React/invoke): `buildGroupTree` (adjacency list →
nested, deterministic order, **orphan-as-root** defense), `descendantGroupIds` (**cycle-safe**),
`filterJobsByGroup` (three modes; group mode is **DEEP** — the group OR any descendant),
`moveTargetsFor` (all groups EXCEPT self + descendants, so the Move-to picker can never offer a
cycle). 12 new tests. **Filter and Move-to exclusion both derive from the same `descendantGroupIds`**
— one subtree walk, reused.

**Single source of truth:** one `GroupSelection` (`all | ungrouped | group{id}`) **lifted to `App`**
(React-only, not persisted). It drives THREE things consistently: (1) the deep jobs filter, (2)
assign-on-create (`App` derives `activeGroupId` → `NewJobScreen`; after `create_job`,
`move_job(id, activeGroupId)` for both the normal path and the GOAT quick-action), (3) the Move-to
exclusion.

**Sidebar** `GroupSidebar.tsx`: "All jobs" + "Ungrouped" roots, then the tree with expand/collapse.
Per-group inline actions (hover-revealed, **no DnD**): new subgroup / top-level new group (inline
name input → `create_group`), rename (inline input → `rename_group`), Move to… (native `<select>` of
`moveTargetsFor` + "(root)" → `move_group`; auto-themed by the element-level WebKitGTK select fix,
`debugging/003`), delete (`confirm` "sub-groups and jobs move up to the parent — no job is deleted" →
`delete_group`; selection falls back to "All jobs" if the deleted group was selected). Every mutation
reloads groups + jobs.

**Two-pane Jobs view** `JobsScreen.tsx`: owns groups + jobs lists; renders
`filterJobsByGroup(jobs, selection, groups)` with a per-selection header + empty-state; each row gains
a **"Move…"** `<select>` (all groups + "(ungrouped)" → `move_job`; a job has no cycle concern). A
guard effect resets to "All jobs" if the selected group vanished. The sidebar re-implements NO cycle
guard beyond hiding self+descendants — the backend `move_group` is the source of truth.

**Rust (the only change):** `models/job.rs` — `group_id` appended to `Job::COLUMNS` (row 14),
`from_row`, struct field. No new command, no migration, no schema change (v16 column + all group
commands already exist). `types.ts` gained `Job.group_id` + the `Group` interface. Group sidebar CSS
in `app.css`.

**Verification.** `cargo test` **243** (unchanged — the `Job.group_id` touch strengthened
`create_lists_job_as_draft` to assert `group_id` round-trips NULL); `tsc` clean; `vitest` **708**
(was 696, +12 tree/filter tests). Checkpoints confirmed: one lifted selection drives all three (no
duplicated subtree walk); no drag-and-drop / no new command / SCHEMA_VERSION unchanged (only Rust
diff is `models/job.rs` + a jobs.rs test assertion); Move-to excludes self+descendants and the filter
is deep. **Live WebKitGTK render + interactions are Anton's eyeball gate** (this is a render unit; no
headless click-through fabricated) — eyeball: (1) create group+subgroup → tree nests; (2) create a
job with a group active → it appears under that group; (3) select the parent → the subgroup's jobs
show too (deep); (4) Move a job via the picker; (5) Move a group — the picker omits its own
descendants; (6) delete a non-root group → children+jobs promote to the parent, none lost; (7) "All
jobs" vs "Ungrouped".

**Next: Phase 4.7.4** — filter/search over the job list (plain LIKE/column filter, complementary to
the tree).

## [2026-08-09] session | Phase 4.7.4 — filter/search over the job list (composed on the group filter) — Phase 4.7 CLOSED

Added a frontend search/status filter over the Jobs view, **composed on top of** the 4.7.3 group
filter. Frontend-only — **no Rust, no new command, no migration, no SQL/FTS5** (verified: zero
src-tauri changes in the diff).

**Pure logic** `src/groups/search.ts` (vitest, no React/invoke): `parseMethodLine(input)` = the first
`!` keyword line stripped of its `!` and trimmed (client-side method token, `""` if none), and
`filterJobsBySearch(jobs, query, statuses)` = case-insensitive **substring** over title OR method,
ANDed with the status set; **empty query + empty statuses ⇒ identity**. 11 new tests, incl. the
**composition test (main risk):** `filterJobsBySearch(filterJobsByGroup(jobs, {group:R}, groups), q,
…)` never returns a job outside R's subtree — search narrows WITHIN the group, never re-widens.

**Composition order (load-bearing):** the rendered rows are
`filterJobsBySearch(filterJobsByGroup(jobs, selection, groups), query, statuses)` — group filter
FIRST, search SECOND. Both pure over `Job[]`.

**UI** (`JobsScreen.tsx`): a search box (title-or-method placeholder) + seven status **chips**
(draft…cancelled toggles) + a Clear affordance, **local** state (not lifted to `App`, not persisted).
Three-way empty-state: "No jobs yet" (`jobs` empty) / "No jobs in this group" (`groupJobs` empty) /
**"No jobs match this filter"** (search over-narrowed). Chip + filter-bar CSS in `app.css` (buttons,
not `<select>` — no WebKitGTK fix needed).

**Verification.** `tsc` clean; `vitest` **719** (was 708, +11 search tests); `cargo` **243
UNCHANGED** (explicitly — a Rust count change would mean scope leaked; there was none). Checkpoints:
rendered rows are the single composed `filterJobsBySearch(filterJobsByGroup(...))` expression; zero
src-tauri/ in the diff (substring only, no FTS5/command/SQL); empty query+status is the identity
(test-asserted). **Live search box + chips + empty state are Anton's eyeball gate** (render unit; no
headless click-through) — eyeball: (1) title fragment narrows; (2) method fragment ("r2SCAN") matches
via the `!` line; (3) a status chip filters to that status; (4) group + query → results stay within
the group's subtree; (5) over-narrow → "No jobs match this filter"; (6) Clear → back to the
group-filtered list.

**Phase 4.7 CLOSED** — all four units done (4.7.1 delete · 4.7.2 groups data layer · 4.7.3 nav UI ·
4.7.4 search). The Jobs view is a two-pane tree of folders with delete-with-promotion, assign-on-
create, and a composed search filter; grouping never touches disk (rule #3). ROADMAP Phase 4.7 marked
COMPLETE.

**Next:** Phase 5 (remote execution over SSH) per the roadmap, or whatever Anton directs.

## [2026-08-09] lint | Full wiki lint — 4 findings applied (ADR-007 FK-off correction + 3 stale/index)

Full health-check across all of `wiki/**` (88 pages), with extra depth on the Phase 4.5
reaction-modeling corpus (Stages A–D). Measure-only report first, then this apply pass — a
docs-only commit (no source, no schema, no tests).

**Dimensions checked and CLEAN:**
- **Contradictions — the Phase 4.5 measured-fact corpus is internally consistent.** Index base
  (ORCA **0-based** / xtb **1-based**) stated consistently across ~15 pages; SMD keyword-form ==
  `%cpcm` block bit-identical (`solvation.md`); GOAT preserves atom count+order (`goat.md`); `.hess`
  normal modes **Cartesian** + `$atoms` frame pure-translation/Kabsch (`parse-sources.md` ⟷
  `artifact-readers.md`); scan needs `! Opt` (`scan.md`); Boltzmann at **GFN2-xTB** level, honestly
  labelled (`conformers.md` ⟷ `conformer-reoptimization.md`). No conflicts.
- **cclib** — every live mention correctly "rejected/crashes on 6.1" (ADR-012 etc.).
- **Module-vs-code** — `tauri-core.md` current (`SCHEMA_VERSION=16`, `delete_job`, all six group
  commands, `Group` model); "**five** readers" matches `src-tauri/src/parse/`; guided placement built
  and documented as built. Spot-checked, no drift.
- **Orphans/xrefs** — every `index.md` link resolves; no orphans; **no broken in-page `.md` links
  across all 88 pages**; no dangling ADR refs; no stale Phase-2.6 links.
- **ROADMAP** — no drift (Stage D + Phase 4.7 correctly COMPLETE; the reactions-ui c1–c4 / r1–r4
  manual gates read "PENDING" consistently in `reactions-ui.md`, `log.md`, and ROADMAP — a genuine
  open state, not drift).
- **Language** — chemistry/* Ukrainian, architecture/modules/orca English; the author's raw design
  proposal (Ukrainian) is exempt. **log.md** — all prefixes well-formed, chronological, tail-5 parses.

**Four fixes applied (quote-and-correct in place; no decision rewritten, no history edited):**
- **F1 (HIGH) — `architecture/adr-007-reaction-modeling.md`:** the integrity paragraph's aside
  claimed `PRAGMA foreign_keys` off and `results ON DELETE CASCADE` is "documentation" — measured
  **false**. Added a correction note (mirrors `modules/tauri-core.md`:151): FK enforcement is **ON**
  (`SQLITE_DEFAULT_FOREIGN_KEYS=1`, `PRAGMA` reads 1), the cascade **fires** (Phase 4.7.1 test
  `delete_job_removes_it_and_cascades_results`), command checks are belt-and-suspenders. The one live
  survivor the 2026-08-08 tauri-core sweep missed; ADR decision untouched.
- **F2 (LOW) — `CLAUDE.md`:** trimmed the completed tail clause "Consolidating the existing `As
  built` sections is tracked as its own unit" (no module page has `As built` sections anymore). Rule
  + rationale kept.
- **F3 (MEDIUM) — `architecture/adr-016-emit-input-ownership.md`:** the five-seams list item 3
  (`%geom Scan`) said "not yet emitted (only in the manual index today)" — scan emit shipped in Stage
  A1. Re-annotated "emitted since Stage A1 (`src/scene/scan.ts` + `geomBlock.ts`, Rust golden)".
- **F4 (LOW) — `index.md` footer:** page count 84 → **86** (all catalogable pages = `wiki/**.md`
  minus `index.md`/`log.md`; the two Phase-4.7 pages are the delta), date → 2026-08-09, prepended
  `+groups.md` / `+groups-ui.md` to the structural-update narrative. Body links untouched.

**Deliberately NOT changed:** the "FK off" / "not deletable today" survivors elsewhere in this
`log.md` are **append-only correct history** (true when written) — left as-is per the log's
append-only rule. No existing entry edited.

## [2026-08-09] session | Phase 4.5 C2b — two overlay defects the manual gate surfaced (intrinsic reactant-side min + per-pathway barriers at ≥1)

The C2b-2b r1–r4 manual gate, run on a REAL single-pathway exothermic SN2 (Menshutkin — methylamine +
ethyl iodide, DMF/SMD), surfaced two hidden-assumption bugs in the comparative-overlay code — both
invisible to the prior endothermic/symmetric fixtures. Fixed (frontend-only; no Rust, no schema).

**Defect 1 — intrinsic barrier used the GLOBAL scan minimum.** `intrinsicBarrierKcal = E(max) −
minEnergyEh`, and `minEnergyEh` is the min over ALL points. For an exothermic reaction scanned past
the barrier into a lower product (Menshutkin product ≈ 22 kcal/mol below the reactant complex), the
global min IS the product → the formula gave the **reverse** barrier (~30 kcal/mol), not the forward
intrinsic (~7.87). The single-job view already showed the correct forward value; the overlay
disagreed with it. **Fix (`src/reactions/compare.ts`):** new `argMaxIndex` + `reactantSideMinEh`
(min over `points[0..argmax]` inclusive — the pre-barrier branch, scan reactant→product convention,
assumption named in the docstring per rule #9); `intrinsicBarrierKcal` now uses it. `minEnergyEh`
kept unchanged (the chart shared-zero still needs it — out of scope). Headline vitest: the `SI`
fixture was itself the bug witness (global min = product), so its expectation moved from the reverse
0.09 Eh to the forward 0.05 Eh; plus endothermic-no-op and max-at-first-point-→0 fixtures.

**Defect 2 — per-pathway barriers gated behind ≥2 pathways.** `ReactionsScreen` rendered the entire
`CompareView` (per-pathway intrinsic + absolute barrier table AND the chart) only at
`comparePathways.length >= 2`. But intrinsic and absolute barriers are PER-PATHWAY (need 1); only
ΔΔE‡ (a difference of two maxima) needs 2. So for a single-pathway reaction (SN2 has no si/re face)
the absolute barrier — the whole point of C2b-2b — was unreachable. **Fix:** gate → `>= 1`;
`CompareView` shows a single-curve chart + the per-pathway barriers at 1, and a **note** ("Attach a
second pathway … to compute ΔΔE‡") replaces the ΔΔE‡ number at 1 — never a NaN. The ≥2 path renders
exactly as before (pure superset). The ΔΔE‡/absolute MATH and the chart shared-zero are untouched.

**Verification.** tsc clean; vitest **721** (was 719, +2 net intrinsic tests). **No cargo change**
(frontend-only — a Rust delta would mean scope leaked; there was none). Live single-pathway overlay
render is Anton's eyeball gate (r1–r4 stays open). **C2b-2b NOT marked done** — the gate re-runs on
top of this fix.

Wiki: `modules/reactions-ui.md` (overlay renders per-pathway barriers at ≥1; ΔΔE‡ at ≥2; intrinsic
from the reactant-side min), `chemistry/reaction-barriers.md` (intrinsic = E(max) − reactant-side
min, with the DMF-SMD Menshutkin witness), ROADMAP C2b-2b (gate-driven fix note; still `[~]`).

## [2026-08-09] session | Phase 4.5 C2b — 3rd gate-surfaced defect: comparability NON_METHOD missing LooseOpt/NormalOpt

The r1 manual gate (real scan using `! … LooseOpt` for floppy-complex convergence) exposed a third
overlay defect: the comparability guard's `NON_METHOD` drop-set was missing the geometry-opt
convergence presets `LOOSEOPT`/`NORMALOPT` (while `TIGHTOPT`/`VERYTIGHTOPT` were already present). So a
`! r²SCAN-3c SMD(DMF) … LooseOpt` scan compared against `! … Opt` reference jobs produced **different**
method signatures → spurious "reference method differs" → the absolute barrier was refused even though
scan and reference are the SAME electronic-structure method.

**Fix (pure, `src/reactions/compare.ts`):** added `"LOOSEOPT"` and `"NORMALOPT"` to `NON_METHOD`,
completing the {Loose,Normal,Tight,VeryTight}Opt family (convergence tightness ≠ method). One-line set
change + comment.

**Bite test (`compare.test.ts`):** a LooseOpt scan vs Opt/TightOpt references →
`referenceComparable` **ok** (+ the signatures are equal across the whole opt-preset family); and the
**guard-against-over-broadening** — a `B3LYP def2-SVP D4 SMD(DMF) Opt` reference vs an `r²SCAN-3c
SMD(DMF) LooseOpt` pathway still **refuses** (r3 depends on this). Presets neutralized, functional
difference not.

**Verification.** tsc clean; vitest **723** (was 721, +2). No cargo change (pure frontend logic). Anton
re-checks r1 live — the absolute barrier now shows a NUMBER for the LooseOpt scan vs the Opt refs.
C2b-2b stays open (r1–r4). Wiki: `modules/reactions-ui.md` (NON_METHOD covers the full Opt family +
the screening-level caveat: a LooseOpt max is slightly less converged than a tighter reference), ROADMAP
C2b note.

## [2026-08-09] session | Phase 4.5 C2b — 4th gate-surfaced defect: no stoichiometry guard → confident-garbage absolute barrier

The r2 manual gate exposed the most serious C2b defect: the absolute barrier E(max) − Σ E(ref) had NO
composition/charge guard, so with the wrong reference set it computed a CONFIDENT GARBAGE number
instead of an honest refusal. On the real SN2 (methylamine + ethyl iodide, DMF/SMD), removing one of
the two reactant references left a single EtI reference (8 atoms); the app subtracted E(EtI) from the
15-atom E(max) and displayed **−60127.66 kcal/mol** (≈ −E(methylamine)). This violated honest-or-absent
(ADR-018): a mismatched reference must be refused with a reason, never turned into a number.

**Fix (pure, `src/reactions/compare.ts`):** new `referenceStoichiometryOk(complexInput,
referenceInputs)` — **reuses `sceneFromOrcaInput`** (the app's one `* xyz` block reader, NOT a
hand-rolled regex) to read the complex's and each reference's element multiset + charge, sums the
references, and refuses unless Σ(reference atoms) = complex atoms AND Σ(reference charge) = complex
charge. Specific Hill-formula reason on mismatch ("reactant atoms (C2H5I) do not sum to the reacting
complex (C3H10IN) …") + a charge-note branch. Threaded into `absoluteBarrierCell` as a new last param
`complexInput`, called AFTER the count/null/method guards, BEFORE the number. `CompareView` passes each
pathway's own `p.input` (the scan complex's input_content) — the only render change.

**Both valid shapes still pass:** two references summing to the complex (8 + 7 = 15 atoms) OR a single
reference that IS the whole complex (15 atoms); only a reference whose atoms/charge ≠ the complex's is
refused. Charge imbalance (a −1 reference vs a 0 complex) refuses too — guards the future ionic BH₄⁻
case.

**Verification.** tsc clean; vitest **729** (was 723, +6: the guard's own tests + the threaded
`absoluteBarrierCell` cases). No cargo change (pure frontend). The r2 headline test asserts the
EtI-only reference is REFUSED (no −60127 number). Anton re-runs r2 live: removing a reference now
shows "reference incomplete — reactant atoms … do not sum …" (a reason), not −60127; putting it back
returns the number. C2b-2b stays open (r1–r4). Wiki: `modules/reactions-ui.md` (the three-refusal
guard chain: complete → method → stoichiometry), `chemistry/reaction-barriers.md` (mass+charge balance
requirement + the −60127 SN2 witness), ROADMAP C2b note.

## [2026-08-09] milestone | Phase 4.5 C2b-2b closed — r1–r4 absolute-barrier gate passed (Menshutkin SN2)

The C2b-2b manual gate (r1–r4) **PASSED live** — author, real Tauri window — on a real
single-pathway exothermic SN2: **Menshutkin, methylamine + ethyl iodide, DMF/SMD**. The absolute
barrier vs separated reactants now renders honestly, so C2b-2b is closed (`[x]`), and with it the
parent C2b-2 (data half C2b-2a + UI half C2b-2b both done).

**The gate earned its keep — it surfaced four hidden-assumption defects, ALL invisible to the prior
endothermic/symmetric unit fixtures**, each fixed before r1–r4 passed:
- **(i)** intrinsic barrier used the GLOBAL scan min → the **reverse** barrier on an exothermic scan
  (global min = product); fixed to the reactant-side min — `9ea3ace`.
- **(ii)** per-pathway barriers were gated behind ≥ 2 pathways → a single-pathway absolute barrier
  (the whole point of C2b-2b) was unreachable; decoupled to render at ≥ 1, ΔΔE‡ at ≥ 2 — `9ea3ace`.
- **(iii)** `NON_METHOD` missed `LooseOpt`/`NormalOpt` → the comparability guard spuriously refused
  the absolute barrier when the scan used LooseOpt; Opt-preset family completed — `2ed1473`.
- **(iv)** no stoichiometry guard → a composition-mismatched reference (EtI alone, 8 atoms, vs the
  15-atom complex) produced a **confident garbage −60127 kcal/mol** instead of a refusal; composition
  + charge balance guard added — `39b39e7`.

**The manual-gate value, stated plainly:** defect (iv) — the r2 step — caught a **confident WRONG
number** (−60127 kcal/mol), not a crash. Any green CI would have passed it; only a human reading a
real result against physical intuition caught it. This is exactly why C2b units carry a live gate.

**Screening result** (approximate-TS ΔE‡ on the relaxed surface — **not** ΔG‡; a located TS + Freq
is Stage E): intrinsic ≈ **+7.87 kcal/mol**, absolute vs separated reactants ≈ **+6.23 kcal/mol**.

**Still pending:** C2b-1's **c1–c4** (ΔΔE‡) gate — it needs a **two-pathway (si/re) stereochemical**
case, which this single-pathway SN2 does not provide. It stays open until that case is run.
