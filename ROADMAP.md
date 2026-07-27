# OrcaStudio — Roadmap

Status markers: `[ ]` not started · `[~]` in progress · `[x]` done.
Keep this file current (see CLAUDE.md → session workflow).

**Guiding principle:** every phase ends with something the author actually uses for real
calculations. No phase is "infrastructure only".

---

## Phase 0 — Foundation (≈ 2–3 days)

**Goal:** working environment + repo skeleton + wiki discipline from day one.

- [x] Install ORCA 6.x (FAccTs academic registration), matching OpenMPI version — ORCA 6.1.0 at `/opt/orca`, system OpenMPI 4.1.6
- [x] Verify from terminal: water single-point + geometry opt with `%pal nprocs 4`, full-path invocation — `water_optfreq` (r²SCAN-3c Opt+Freq) converged, minimum confirmed, `ORCA TERMINATED NORMALLY`
- [x] Record the verified install path, versions, and any install pain in `wiki/orca/orca-basics.md`
- [x] Scaffold Tauri 2 + React + TS (Vite), runs with `npm run tauri dev` — window verified (React 19 per current template)
- [x] Scaffold Python sidecar (FastAPI, `/health` endpoint), spawned by Tauri on startup, killed on exit — dynamic port, venv python, `sidecar.log`
- [x] SQLite opened/migrated by Rust core (single `orcastudio.db` in user data dir) — migration v1 `settings` table, seeded `orca_path`
- [x] Git repo initialized; first `log.md` entry written

**Done when:** `npm run tauri dev` opens a window that shows sidecar status = healthy and
ORCA path = configured; a water optimization ran successfully from the terminal.

---

## Phase 1 — MVP: run a calculation from the GUI (≈ 1–2 weeks)

**Goal:** the terminal is no longer needed for a basic ORCA run. This is the moment the
project mission becomes real.

- [x] Job model + state machine in SQLite: `draft → running → completed | failed`
      (the `parsed` state is deferred until result extraction lands — Phase 1 later step)
- [x] Monaco editor for `.inp` with ORCA syntax highlighting (custom Monarch grammar, basic)
- [x] Template library (hardcoded to start): SP, Opt, Freq, Opt+Freq on r²SCAN-3c / B3LYP D4 def2-SVP
- [x] `LocalBackend`: isolated job dir, full-path spawn, stdout → `output.out`, `.exit_code` marker
- [x] Live log view: Rust tails `output.out`, streams lines to frontend via Tauri events
- [x] Completion detection (marker + `ORCA TERMINATED NORMALLY`), status surfaced in UI
- [x] Minimal result extraction: final SCF energy, wall time (regex in Rust is fine for now)
- [x] Job list screen: history of all runs with status, energy, open-folder button

**Done when:** author creates a job from a template, edits it, hits Run, watches the log live,
and sees the final energy in the job list — without touching a terminal.

---

## Phase 2 — Molecules & input UX (≈ 2–3 weeks)

**Goal:** working with structures and building inputs becomes visual.

- [~] 3Dmol.js viewer component: load xyz, ball-and-stick — **done** (Phase 2.1: `MoleculeViewer`
      + live xyz preview on New Job, WebKitGTK WebGL fix). Licorice toggle + atom picking → Phase 3.
- [~] Import: xyz file, SMILES → 3D via sidecar (RDKit ETKDG + MMFF cleanup) — **done**
      (Phase 2.2: `.xyz` file import + `/smiles-to-3d` endpoint, both inject coords into the
      editor). MMFF conformer only; conformer choice / protonation states later.
- [~] Molecule library in SQLite (name, formula, xyz, tags) — **done** (Phase 2.3: `molecules`
      table + CRUD commands, Molecules screen with add/list/detail/use/delete, "Save to Library"
      + "Use" ↔ New Job integration). NOT yet linked to jobs (`molecule_id` FK deferred to
      Phase 4.5, reaction modeling).
- [~] Input builder form: method/functional, basis, RI approximations, dispersion,
      solvation (CPCM/SMD + solvent), job type, charge/multiplicity, `%pal`/`%maxcore`
      → generates `.inp`, still hand-editable in Monaco (form ↔ text one-way is fine) — **done**
      (Phase 2.4: `src/input-builder/`, composite vs functional modes, auto RI aux basis, live
      `!`-line preview). Double-hybrids (AuxC) deferred; form→text one-way as planned.
- [ ] Live convergence dashboard for Opt jobs: energy per cycle, gradient norm vs criteria
      (parse incrementally from the streamed log)
- [ ] Sidecar endpoint: xyz ↔ common format conversions (Open Babel)
- [ ] Sequential job queue: `queued` status, worker loop picks next job when current
      completes (concurrency 1). Replaces the current "another job is running" error
      with transparent queuing. Small scope: SQLite status + loop in LocalBackend.

**Done when:** author pastes a SMILES, gets a 3D structure, configures an optimization in the
form, runs it, and watches the energy curve descend in real time.

---

## Phase 2.5 — Geometry editor (≈ 3–4 weeks)

**Goal:** precise geometric control over molecular structures — the foundation for
reaction mechanism research. See ADR-007.

- [ ] Atom picking in 3Dmol.js: click → highlight, show atom info (element, index, coordinates)
- [ ] Measurement mode: pick 2/3/4 atoms → display distance / angle / dihedral angle
- [ ] Geometry kernel in sidecar (ASE-based): `atoms.set_distance()`, `set_angle()`,
      `set_dihedral()` with mask arrays — recalculate Cartesian coordinates; move
      single atom or rigid fragment. No custom trigonometry.
- [ ] Edit mode in viewer: pick atoms → enter target value → preview → apply →
      coordinates update in editor
- [ ] Fragment library: common reagents (BH₄⁻, H₂O, common ligands), place at position
      with specified distance/angle
- [ ] Constraint manager: list of active constraints, toggle on/off, export to
      ORCA `%geom Constraints ... end` block in the input
- [ ] xTB integration: sidecar endpoint `/xtb-optimize` (xyz + constraints → optimized xyz),
      xTB binary path in Settings (standalone `xtb`, not through ORCA)

**Done when:** author builds a TS guess by placing BH₄⁻ at a Bürgi-Dunitz angle relative
to a carbonyl carbon, constrains the approach distance, runs xTB pre-optimization, and
gets a physically reasonable starting geometry — all inside OrcaStudio.

---

## Phase 3 — Results dashboard (≈ 3–4 weeks)

**Goal:** post-calculation analysis without any external tools.

- [ ] Sidecar: full cclib parse endpoint → structured JSON (energies, orbitals, frequencies,
      intensities, charges, dipole, TD-DFT states); stored in SQLite per job
- [ ] Results screen per job: summary card (energy, HOMO/LUMO gap, dipole, imaginary freq warning)
- [ ] Optimization trajectory playback in 3Dmol.js (multiframe xyz)
- [ ] Orbital/density isosurfaces: wrap `orca_plot` (batch mode) → `.cube` → 3Dmol.js volumetric
      rendering; MO picker with energies and occupations
- [ ] IR spectrum: Lorentzian broadening of freq/intensity list, interactive recharts plot;
      click a peak → animate that normal mode in the viewer
- [ ] Imaginary-frequency detection surfaced prominently (saddle point vs minimum — teaching moment)
- [ ] Export: xyz of final geometry, CSV of parsed data, PNG of plots

**Done when:** for an Opt+Freq job the author can watch the trajectory, spin the HOMO isosurface,
and click IR peaks to see the vibrations — all inside OrcaStudio.

---

## Phase 4 — ORCA manual integration (≈ 1–2 weeks)

**Goal:** the app teaches ORCA while you use it. Key differentiator.

- [ ] One-off indexing script (sidecar): ORCA HTML docs → markdown sections → SQLite FTS5
      (stored locally in user data dir; never redistributed)
- [ ] Manual panel: full-text search with keyword highlighting, section rendering
- [ ] `keywords.json`: curated map of input keywords (`!` line + `%` blocks) → manual sections;
      grow it organically, starting with everything the template library uses
- [ ] Monaco hover provider: hover a keyword in the editor → short description → click opens
      the full manual section in the panel
- [ ] (Optional) "Explain with Claude": keyword + current input context + manual excerpt →
      Anthropic API (user's own key in settings) → plain-language explanation

**Done when:** hovering `RIJCOSX` in the editor explains what it is, and the manual panel
answers "how do I set up CPCM for water" in one search.

---

## Phase 4.5 — Reaction modeling (≈ 3–5 evenings)

**Goal:** OrcaStudio becomes a reaction mechanism workstation. The researcher defines a
reaction, explores pathways via native ORCA scans, and compares electronic energy
barriers — the full computational experiment lifecycle. See ADR-007.

- [ ] Conformer ensemble step: `! XTB GOAT` on the substrate → Boltzmann-weighted
      ensemble → re-optimise lowest 3–4 at DFT → build reaction centers on those.
      Mandatory before any pathway (see ADR-007).
- [ ] Data model: `reactions`, `reaction_centers`, `pathways` tables; nullable FKs from
      `jobs` (`reaction_id`, `pathway_id`)
- [ ] Reaction setup UI: define substrate + reagent, pick reaction center atoms,
      set approach geometry (distance, angle, dihedral)
- [ ] Scan input generation: from ReactionCenter → ORCA `%geom Scan B a1 a2 = start, end, npoints end end`
      (one job per pathway, native relaxed scan — NOT N separate jobs)
- [ ] Scan output parser: extract per-point energies and coordinate values from ORCA output
      (table format in scan output section)
- [ ] Energy profile visualization: reaction coordinate vs energy (recharts)
- [ ] Comparative pathway view: overlay Pathway A vs Pathway B energy profiles;
      ΔΔE‡ (electronic energy barrier difference) highlighted
- [ ] TS refinement (late step): scan maximum geometry → OptTS → Freq → verify one
      imaginary frequency → ΔG‡ with thermochemistry (publication-quality result)

**Done when:** author defines two stereofacial attacks on a ketone (si vs re face),
runs two native ORCA scans, and sees two energy profiles side by side with ΔΔE‡ —
a computational screening of stereoselectivity.

---

## Phase 5 — Remote execution over SSH (≈ 2–3 weeks)

**Goal:** heavy jobs run on a server; the laptop stays free. The app becomes disconnect-proof.
**This is a requirement, not a convenience** — a full mechanism study is 300–800 jobs
and the dev laptop is a development machine, not a compute node. See
`wiki/orca/performance.md`.

- [ ] Formalize `ExecutionBackend` trait (submit / stream_log / status / fetch_results / cancel);
      refactor `LocalBackend` onto it (the seam exists since Phase 1 — this makes it explicit)
- [ ] Server profiles in settings: host alias (reuses `~/.ssh/config`), remote ORCA path,
      remote scratch dir; connection test button
- [ ] `SshBackend` via system `ssh`/`rsync`: rsync job dir up → `nohup` runner script with
      `.pid` + `.exit_code` markers → byte-offset polling of output → selective rsync down
      (output/xyz/hess always; gbw opt-in)
- [ ] Job state machine extended: `uploading → running → syncing`; reconciliation on app start
      (check markers for every job that was `running`)
- [ ] Remote `orca_plot` option: generate cubes server-side, download only `.cube`
- [ ] Job pause/cancel (the sequential queue itself lands in Phase 2)

**Done when:** author submits a job to the server, closes the laptop, reopens it hours later,
and OrcaStudio picks the job up, syncs results, and parses them — no terminal, no lost state.

---

## Phase 6 — Power features (ongoing, pick by research needs)

- [ ] TD-DFT UV-Vis: simulated spectrum with oscillator strengths, state table
- [ ] NEB: path setup UI + energy profile visualization per iteration
- [ ] Job comparison view: N jobs side by side (energies, geometries overlay, spectra)
- [ ] Batch/parametric runs: same molecule × list of functionals or basis sets
- [ ] SLURM backend (third `ExecutionBackend` implementation)
- [ ] Markdown notes attached to jobs/projects → the app becomes a lab journal
- [ ] Multi-step mechanism support: Mechanism = ordered sequence of Reactions (catalytic
      cycles like Sonogashira: oxidative addition → transmetalation → reductive elimination)
- [ ] AI-assisted reaction setup: describe reaction → AI proposes reaction center, approach
      geometry, pathways (Anthropic API, user's key)
- [ ] TS refinement automation: scan maximum → OptTS → Freq → ΔG‡ pipeline
      (if not completed in Phase 4.5)

---

## Explicit non-goals (for now)

- Windows/macOS support (Linux first; Tauri keeps the door open)
- Multi-user / collaboration features
- Bundling ORCA or its manual (licensing)
- Full-featured molecular drawing from scratch (symmetry-aware bond drawing, ring
  perception, etc.) — structure creation uses import + fragment placement + geometric
  manipulation (Phase 2.5), not free-hand sketching
