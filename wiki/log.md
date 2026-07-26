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
