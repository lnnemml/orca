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
