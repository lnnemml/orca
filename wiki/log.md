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
