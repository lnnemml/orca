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

## Phase 2 — Molecules & input UX (≈ 2–3 weeks) — ✅ COMPLETE

**Goal:** working with structures and building inputs becomes visual.

- [x] 3Dmol.js viewer component: load xyz, ball-and-stick — **done** (Phase 2.1: `MoleculeViewer`
      + live xyz preview on New Job, WebKitGTK WebGL fix). Licorice toggle + atom picking → Phase 3.
- [x] Import: xyz file, SMILES → 3D via sidecar (RDKit ETKDG + MMFF cleanup) — **done**
      (Phase 2.2: `.xyz` file import + `/smiles-to-3d` endpoint, both inject coords into the
      editor). MMFF conformer only; conformer choice / protonation states later.
- [x] Molecule library in SQLite (name, formula, xyz, tags) — **done** (Phase 2.3: `molecules`
      table + CRUD commands, Molecules screen with add/list/detail/use/delete, "Save to Library"
      + "Use" ↔ New Job integration). NOT yet linked to jobs (`molecule_id` FK deferred to
      Phase 4.5, reaction modeling).
- [x] Input builder form: method/functional, basis, RI approximations, dispersion,
      solvation (CPCM/SMD + solvent), job type, charge/multiplicity, `%pal`/`%maxcore`
      → generates `.inp`, still hand-editable in Monaco (form ↔ text one-way is fine) — **done**
      (Phase 2.4: `src/input-builder/`, composite vs functional modes, auto RI aux basis, live
      `!`-line preview). Double-hybrids (AuxC) deferred; form→text one-way as planned.
- [x] Live convergence dashboard for Opt jobs: energy per cycle, gradient norm vs criteria
      (parse incrementally from the streamed log) — **done** (Phase 2 step 5). Incremental
      Rust parser (`src-tauri/src/convergence.rs`) feeds off the same stdout stream as the
      log, emits batched `job:convergence` events; `read_job_convergence` backfills on open.
      Frontend `src/convergence/` dashboard: progress indicator (N/M criteria met), energy-
      per-cycle chart (ΔE in kcal/mol tooltip), criteria-vs-tolerance chart (log Y). recharts;
      explicit chart width (WebKitGTK ResponsiveContainer 0×0 mitigation). Not in SQLite —
      parsed on demand.
- [x] Sidecar endpoint: xyz ↔ common format conversions (**ASE**, not Open Babel) — **done**
      (Phase 2.6: `POST /convert` + `GET /formats` in `sidecar/app/convert.py`). ASE chosen over
      Open Babel: already a sidecar dep (ADR-007 geometry kernel), pure-Python wheel (no system
      binary), covers xyz/pdb/cif/mol/sdf/gen/turbomole/vasp/... Whitelisted read/write formats
      (security: ASE's full registry includes code-executing calc-package readers). Frontend:
      the New Job + Molecules "Import file" button now accepts `.pdb/.cif/.mol/.sdf/.gen` too —
      `.xyz` parsed locally, the rest converted to xyz via the sidecar (shared
      `src/viewer/import-file.ts`). Open Babel remains the fallback only for formats ASE lacks
      (e.g. mol2). Export to other formats from the UI is Phase 3.
- [x] Sequential job queue: `queued` status, worker loop picks next job when current
      completes (concurrency 1) — **done**. `submit` enqueues instead of erroring on a busy
      slot; the queue lives in SQLite (`status='queued'`, oldest first), pulled by
      `try_start_next` after each finish. Pause/resume (queue-only). Plus, beyond scope:
      **CPU core pinning presets** (taskset + `%pal` alignment, domain rule #8),
      **job cancellation** (killpg the ORCA process group **+ sweep escaped MPI ranks by cwd**),
      and **startup reconciliation** of jobs left `running` by a crash. See
      `wiki/modules/execution-backends.md`.
- [x] Output search (addition beyond the original plan) — **done** (Phase 2.7:
      `src-tauri/src/output_search.rs`, `commands::jobs::read_job_output_for_viewer`,
      `src/screens/OutputSearchPanel.tsx` + `OutputViewer.tsx`). Streaming line-by-line search over
      `output.out` (never loads it whole, domain rule #5), a 500-match cap that still reports the
      true total, hit column ranges, and **chemistry-aware presets** (Warnings / Errors /
      SCF-not-converged / Imaginary modes / Final energies / Geometry convergence / Timings / Basis)
      verified against real ORCA 6.1 output. **In-file navigation:** search reveals hits in a
      read-only **Monaco viewer** (Browse mode, loaded lazily, absolute line numbers, decorations)
      with prev/next + `i / N` counter and F3/Enter keybindings; the live streaming console stays on
      `<pre>`. (Supersedes the initial atomised-excerpt list — jump-to-line is now done, not
      deferred.)

**Done when:** author pastes a SMILES, gets a 3D structure, configures an optimization in the
form, runs it, and watches the energy curve descend in real time.

---

## Phase 2.5 — Geometry editor (≈ 3–4 weeks)

**Goal:** precise geometric control over molecular structures — the foundation for
reaction mechanism research. See ADR-007.

**2.5.0 — Scene / fragment foundation** (≈ 4 sessions, prerequisite for
everything below — see ADR-008)

- [x] a. Scene/SceneFragment model: types + pure functions (merge, index
      mapping, immutable updates, serialization, float-tolerant comparison)
- [x] b. Scene ↔ input builder: total charge from fragments, coordinate
      injection, electron-parity validation
- [x] c. MoleculeViewer: multi-fragment rendering (one model, index-range
      styling, per-fragment colours)
- d. Store + Add Fragment UI + persistence — split into three (scope; see ADR-008
      note in the log):
  - [x] d-1. Zustand scene store + Scene↔Monaco two-way sync + New Job on the
        store; parser consolidation closed
  - d-2. Add Fragment: pure foundation, then UI
    - [x] a. Curated reagent library + bounding-box placement (pure, no UI)
    - [x] b. Add-Fragment panel + fragment sidebar (shared palette)
  - [x] d-3. `jobs.scene_json` persistence (schema v4) + "New iteration" action
        that restores it (`restoreScene`: snapshot annotates, input decides)

**✅ 2.5.0 — Scene / fragment foundation is COMPLETE.** Multi-fragment scenes can be
built, viewed, persisted, and iterated on. Next in Phase 2.5 is **2.5.1** (conformer
search / GOAT), then the geometry editor (**2.5.2**), which builds on `locateAtom` /
`fragmentAtomIndices`.

Deferred (needs Phase 3 output parsing): **"continue from the result"** — iterate
from the *optimised* geometry instead of the starting one. The fragment snapshot
already supports it (atom count/order invariant after Opt), see the ADR-008
amendment.

**2.5.1 — Conformer search (GOAT)** (ADR-007's mandatory first step, pulled up from
Phase 4.5 — SMILES fragments arrive as an arbitrary ETKDG conformer, so every scene
may stand on the wrong one)

- [x] a. GOAT primitive, pure (`src/scene/ensemble.ts`): `parseEnsemble` (against a
      real ORCA 6.1.0 run — see `wiki/orca/goat.md`), `conformerMatchesFragment`,
      `goatInputForFragment`. **Verified: GOAT preserves atom count + order**, so a
      conformer drops back into a fragment via `replaceFragmentAtoms` with no mapping.
- [x] b. Run GOAT from the app on a fragment → parse the ensemble → let the user pick
      a conformer → substitute it back into the scene. "Find conformers" per fragment,
      an ensemble panel (ΔE kcal/mol) on Job detail, "Use this conformer" (replace in
      place / new scene / refuse). Verified end-to-end on a real butane run.

**✅ 2.5.1 — Conformer search (GOAT) is COMPLETE.** Boltzmann weighting + DFT
re-optimisation of the lowest 3–4 stay in Phase 4.5 (the scientific pipeline);
2.5.1 delivered the primitive + the run/pick UI. **Next: 2.5.2 — geometry editor**
(the d/θ/φ sequential-apply acceptance test is already recorded — see the log).

**2.5.2 — Geometry editor** (builds on the foundation)

- [x] Atom picking in 3Dmol.js: click → highlight, show atom info (element, index, coordinates)
      — uses `locateAtom(globalIndex)` to report "atom N of <fragment>" **(2.5.2a — pick path
      WebKitGTK-verified; `selection.ts` + atom panel; also the GOAT convergence-label fix)**
- [x] Measurement mode: pick 2/3/4 atoms → display distance / angle / dihedral angle
      — fragment boundaries make inter- vs intra-fragment distances distinguishable
      **(2.5.2b — `measure.ts`, ASE conventions pinned to source; readout + viewer
      labels; also the `selectionSurvives` review fix)**
- [x] Geometry kernel in sidecar (ASE-based): `atoms.set_distance()`, `set_angle()`,
      `set_dihedral()` with mask arrays — recalculate Cartesian coordinates; move
      single atom or rigid fragment. No custom trigonometry.
      — `fragmentAtomIndices()` **is** the mask argument
      **(2.5.2c — `POST /geometry/set-internal`; ASE 3.29 `indices=`; reference-atom rule +
      in-endpoint post-conditions; sequential d/θ/φ acceptance test passes)**
- [x] Edit mode in viewer: pick atoms → enter target value → preview → apply →
      coordinates update in editor
      — pick substrate + reagent atom ⇒ mask = reagent fragment, so the reagent moves
      **(2.5.2d — `edit-plan.ts` + `EditPanel`; visible mask glow; preview view-only;
      Apply → replaceFragmentAtoms; one-step Undo. INTER-fragment only.)**
- [x] **2.5.3** — bond-graph mask split (intra-fragment edits, ring detection):
      rotate a torsion of one molecule's own substrate by splitting the bond graph
      at the picked bond and detecting rings (so a ring bond is refused, not mis-split)
      — the mask becomes a graph-derived subset of ONE fragment, not the whole fragment
      - [x] **2.5.3a** — sidecar `POST /geometry/rotatable-mask`: perception (explicit
        multiplier 1.2, valence-tested), graph split, ring/unbonded/off-cut refusals,
        which-bond-to-cut rule; acceptance = rigid intra-dihedral through set-internal
      - [x] **2.5.3b** — planEdit → `needs-split`; the UI resolves the mask (race-guarded)
        and drives glow + set-internal from it; `within` restricts perception to one
        fragment (metal–ligand trap); smaller fragment moves by default
- [ ] Fragment library: common reagents (BH₄⁻, H₂O, common ligands), place at position
      with specified distance/angle
      — placement = set_distance/angle/dihedral with the mask on the newly added fragment
- [x] Constraint manager: list of active constraints, delete, export to
      ORCA `%geom Constraints ... end` block in the input
      — constraints reference cross-fragment atom pairs
      - [x] **2.5.4a** — pure `constraints.ts` (generate/parse/inject); **index base
            0-based settled by a real ORCA 6.1.0 run** (`wiki/orca/constraints.md`);
            input text is the source of truth. Also closed a 2.5.3b hole: the
            reference-atom rule re-runs on the resolved split mask.
      - [x] **2.5.4b** — the panel as a view over the input text (`ConstraintPanel`),
            "Constrain selection" in the Atom section, and two guards: range-check
            blocks Run (ORCA segfaults on a bad index), composition-change warning
            (no auto-remap). "Toggle on/off" not built — delete + re-add covers it.
- [x] **2.5.5** xTB pre-optimization — **in RUST, not the sidecar** (logged decision:
      Rust owns process spawn + isolation + settings). `xtb_optimize` command: merged xyz
      in, optimised xyz out; fragment boundaries preserved (order + count invariant via
      `replaceAllAtoms`); isolated dir + killpg/sweep (`debugging/004`); post-conditions in
      the command. **xtb `$constrain` is 1-based — settled by a real xtb 6.6.1 run** (≠ ORCA's
      0-based; `wiki/orca/xtb.md`). xtb path + Check button in Settings. Also fixed a 2.5.4b
      data-loss bug: the constraint panel now rewrites only a fully-recognised block.

**Phase 2.5 — COMPLETE.** The scene/fragment model (2.5.0), conformer search (2.5.1), the
geometry editor with d/θ/φ set from the viewer and bond-graph torsion splits (2.5.2–2.5.3),
the constraint manager over the input text with range + composition guards (2.5.4), and xTB
pre-optimization (2.5.5) together make OrcaStudio a reaction-geometry workstation: build a
substrate + reagent, place the reagent with geometric control, constrain the approach
coordinate, and get a physically reasonable GFN2 starting geometry — all inside the app, all
restorable on a job clone. **Done-when met:** the author can build a TS guess (BH₄⁻ at a
Bürgi-Dunitz approach to a carbonyl), constrain the distance, xTB pre-optimize, and iterate
the angle on a cloned job without rebuilding the scene by hand.

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

- [ ] Conformer ensemble → reaction-center pipeline: **Boltzmann weighting** of the
      GOAT ensemble + **re-optimise the lowest 3–4 at DFT** → build reaction centers on
      those. Mandatory before any pathway (see ADR-007). *(The GOAT primitive itself —
      run + ensemble parse — was done in 2.5.1; this is the scientific layer on top.)*
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
