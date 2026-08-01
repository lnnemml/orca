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

> **Note on labels:** the "Phase 2.6" / "Phase 2.7" tags inside the completed items below are
> *historical* — they mark the order work landed within Phase 2, not future phases. (The former
> `## Phase 2.6 — Geometry-editor backlog` **section** was renamed and moved to
> `## Phase 4.2 — Geometry editor completion`, after Phase 4.)

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
      (e.g. mol2) — but **per ADR-009** that means the Open Babel *library* (`pybel`) inside the
      sidecar; shelling out to the `obabel` *binary* would belong in Rust, not the sidecar.
      Export to other formats from the UI is Phase 3.
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
      — **Partly built:** the reagent library (`src/scene/fragment-library.ts`) and
        bounding-box placement (`placeFragment`, coarse ≥3.5 Å gap) exist and add a fragment;
        what is NOT built is adding a reagent *at a specified distance/angle/dihedral* in one
        step — today it is added coarsely, then positioned via the separate edit mode. Left
        `[ ]` until the two are one guided action; carried to `Phase 4.2 — Geometry editor completion`.
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

**Deferred out of 2.5 (carried to `Phase 4.2 — Geometry editor completion`):** *guided fragment placement* — adding a reagent
*at a specified distance/angle/dihedral* in one step. The pieces exist (reagent library +
bounding-box placement + edit mode); what is missing is the single add-at-geometry action. See
the `[ ]` "Fragment library" item above.

---

## Phase 3 — Results dashboard (≈ 3–4 weeks)

**Goal:** post-calculation analysis without any external tools.

- [x] **Authoritative parsers in Rust, over structured artifacts** — all four built (units 3.4–3.7). ([ADR-012](wiki/architecture/adr-012-output-parsing-ownership.md);
      **no cclib**, `output.out` not authoritative). One per source, results → SQLite per job:
  - [x] `.property.txt` reader → energies, geometry, atomic charges (Mulliken/Loewdin/Mayer),
        dipole, thermochemistry (ZPE/H/S/G) — **done** (unit 3.4, `src-tauri/src/parse/property.rs`,
        the template for the other three): two-layer tokenizer, canonical units held by the
        `Angstrom` type (Bohr→Å at the boundary, rule #11), order + geometry post-conditions,
        unknown blocks surfaced. **Wired end-to-end in unit 3.5**: typestate (`verify()` → `Verified`,
        accessors only after post-conditions pass), `results` table (v5, per-atom data with element
        order in JSON), completion hook → `parsed` state, `ResultsCard`. Real Opt+Freq proven
        (`real_optfreq_job_parses_stores_and_reads_back`).
  - [x] `.hess` reader → signed frequencies (3N), normal modes (3N×3N), IR intensities — **done**
        (unit 3.6, `src-tauri/src/parse/hess.rs`): same template, distance-based geometry
        post-condition (`.hess $atoms` is rigidly reframed — measured). Normal modes settled
        **Cartesian** by a determiner run (pltvib÷raw = 2.0 for every atom, H/C = 1.0 not √12), so
        no ÷√m. `imaginary_count` is an explicit field; exact-zero trans/rot (5 linear / 6 not).
        Wired via `results.rs` (v6, `imaginary_count` column) + shown in the card.
  - [x] `_trj.xyz` / `.xyz` reader → trajectory frames / final geometry — **done** (unit 3.7,
        `src-tauri/src/parse/xyz.rs`): multi-frame, comment energy (measured, uniform across job
        types), frames = opt cycles not scan points. Stored in `data_json` (v7).
  - [x] `orca_2json` over `.gbw` (Rust spawns the binary, ADR-009) → MO energies + occupations,
        HOMO/LUMO — **done** (unit 3.7): spawn (`orca_json.rs`, path from settings, lazy-cached)
        separate from the reader (`parse/mo.rs`, **streamed** — `MOCoefficients` skipped, never in
        memory or DB; rule #5 gate measured). `homo_lumo_gap_eh` narrow column (v7).
- [x] **The per-atom seam — parsing side done** (Part A of the parse-sources probe measured it).
      Every per-atom array is read with its order verified: `.property.txt` charges are
      element-labelled (`&ATNO`, order == input); `$Geometry` / `.hess $atoms` / `_trj` / `.xyz` /
      `orca_2json Atoms` all carry the element column, order == input; the one bare positional
      atom-ordered array (`$SCF_Nuc_Gradient &grad`) takes its order from the co-located `$Geometry`
      block — **that assumption is named, not hidden** (readers enforce it via post-conditions). The
      mapping is the **identity** today, so the dashboard is correct. *Not yet done:* extracting this
      into the typed `AtomId`/`IndexMap` — that is **Phase 4.2 Stage 1** (and if `! UseSym` reorders,
      the open Phase-4.5 probe), where **only that one function changes**, not the dashboard.
- [ ] **Unfixed-stereocenter flag on SMILES import.** RDKit's ETKDG picks an enantiomer
      arbitrarily for a SMILES with no stereo descriptors; for stereoselectivity work that is a
      *silent substitution of the compound*, so the import must mark undefined stereocenters
      rather than hide the choice. Small, and it does **not** wait for Phase 4.2 — it belongs
      wherever an imported structure is first shown.
- [x] **"No MO data" is a normal state, not an error.** — **done** (unit 3.7): `orca_json::ensure_gbw_json`
      returns `Ok(None)` for an xTB/GOAT `.gbw` (converter produces no JSON), `orbitals` is `None`,
      and the card simply omits the HOMO/LUMO row — no crash, no error.
- [x] Results screen per job: summary card. **Done (units 3.5–3.7)**: final energy, dipole, HOMO/LUMO
      gap (Eh + eV), trajectory frame count, three
      charge schemes (atom→value), thermochemistry (T·S labelled as T·S, plus a *derived* S in
      J/(mol·K)), and a **vibrational table with IR intensities + a prominent minimum/TS/neither
      verdict from `imaginary_count`** (the teaching moment). Absent sections hidden (SP/GOAT
      render without crashing). The summary card is now feature-complete for parsed data; richer
      views (trajectory playback, isosurfaces, spectra) are the visualization items below.
- [x] Optimization trajectory playback in 3Dmol.js (multiframe xyz) — **done** (unit 3.8, Part A;
      `src/trajectory/`). Transport + slider + speed + an energy-per-cycle chart (click to jump).
      **The current frame number and the play timer are application state (`TrajectoryPlayer`), NOT
      3Dmol's frame apparatus** — the viewer is fed one frame (ADR-011). Frames are labelled honestly
      as optimization **cycles** (not scan points, measured). Empty states: 1 frame → static, no
      controls; no trajectory (SP) → hidden. Element-order identity checked at the UI boundary. See
      `wiki/modules/results-ui.md`.
- [x] Orbital isosurfaces: wrap `orca_plot` → `.cube` → 3Dmol.js volumetric rendering; MO picker with
      energies and occupations — **done** (unit 3.15). Gated first (measured, `wiki/orca/orca-plot.md`):
      `orca_plot`'s advertised `plot-inputfile` batch mode was unusable (undocumented "state density"
      field → FATAL, no cube), so generation drives its interactive menu over **stdin**; cube sizes
      40³–100³ = 0.9–13.5 MB, sub-second (rule #5's 80³ default = 6.9 MB verified by number, read capped
      at 32 MB); and the real unknown — WebKitGTK rendering a 3Dmol **isosurface** — PASSES (MiniBrowser
      screenshot, debugging/002 technique). Generation is **lazy + cached** in the job dir keyed by
      MO+grid (`orca_plot.rs`, mirrors `orca_json.rs`, ADR-009), **never in the DB**. Picker marks
      HOMO/LUMO (default HOMO); **isovalue is a display-choice slider**; +/− colours are the ψ **phase,
      not charge** (labelled). State (orbital/isovalue/visibility) is app-owned (ADR-011);
      `MoleculeViewer` gained an `orbitalCube`/`orbitalIsoValue` path. **Density** cubes / the MO-coeff
      route are not done — only canonical MOs from the gbw. (Also: the frequency table now flows into
      three columns.)
- [x] IR spectrum: Lorentzian broadening of freq/intensity list, interactive recharts plot — **done**
      (unit 3.8, Part B; `src/spectrum/`). Area-normalized Lorentzian (∫ peak = km/mol intensity,
      tested), FWHM slider + explicit grid (plot choices, not molecule properties), trans/rot excluded
      by exact-zero and imaginary excluded by sign (both surfaced: imaginary shown separately as a
      transition-state diagnosis). Peak ↔ frequency-row click both ways. Cross-checked against
      `orca_mapspc` (14.0%, cause = its wing truncation — reported, not fudged; `parse-sources.md`).
      **`click a peak → animate that normal mode` is now `[x]` (unit 3.12).** Its Kabsch determiner
      cleared the last `.hess` uncertainty: the `$atoms` frame is a **pure translation** of the
      reference geometry (`max|R−I| ≤ 3e-13` on ethane / saddle / dexketoprofen — the asymmetric 33-atom
      case is the decisive witness), so `$normal_modes` are added **as-is**, no mode rotation. Animation
      is `x_eq + A·sin(2π·phase)·v`; phase/amplitude/timer are **application state** (`ModeAnimator`,
      ADR-011), the viewer gets one frame. Amplitude is a display choice (default 2.0 = measured
      `orca_pltvib` multiplier) with a collapse guard (<0.5 Å → warn, measured). Imaginary modes are
      animatable and labelled as the **reaction coordinate**. Pure math + gate in `src/spectrum/mode.ts`
      (+ `probes/hess_frame_kabsch.py`); teaching page `chemistry/normal-modes.md`. (Unit 3.9 turned out to be a defect-fix unit:
      the results screen was made reachable and the header energy re-sourced from the parsed artifact —
      see `wiki/debugging/007`; Kabsch + mode animation did not start.) **Unit 3.10** (first chemist
      review) made the presentation honest: on-curve markers replaced by **sticks** (km/mol, right
      axis) under the broadened curve (km/mol·cm⁻¹, left axis) — two labelled axes, not one; the
      two-series tooltip that mixed a label from one x with a value from another replaced by a
      **single-source** custom tooltip (`irPresentation.ts`, tested); a **display scale factor** slider
      (default 1.00, no method-specific number baked in — `$frequency_scale_factor` measured 1.0 = none)
      and an **inverted view** toggle (labelled a *conventional depiction, not transmittance* — no
      Beer–Lambert, no invented %T). `$actual_temperature` (measured 0.0) confirmed never used as a
      temperature. Teaching page `chemistry/ir-spectrum.md` gained "why the computed spectrum differs
      from experiment" (measured dexketoprofen numbers).
- [x] Imaginary-frequency detection surfaced prominently (saddle point vs minimum — teaching
      moment) — **done** (unit 3.6): `imaginary_count` explicit field + the card's verdict banner
      (0 = minimum, 1 = transition state, >1 = neither → re-optimize).
- [x] Export: xyz of final geometry, CSV of parsed data, PNG of plots — **done** (unit 3.16).
      Built from already-parsed `results` (no re-parse, ADR-012), saved via the native dialog to a
      user-chosen location — **never the job dir** (rule #3, enforced in Rust). xyz (Å, post-condition
      lines == atoms + 2); CSV per set (frequencies with a derived scaled column when scale ≠ 1; charges;
      MOs; thermochemistry — units in every header, `entropyS` exported as **T·S**, derived S separate).
      **PNG gated first** (both paths measured under webkit2gtk-4.1 — `wiki/debugging/009`): charts via
      recharts-SVG→canvas→PNG (`SVG_OK 6237`), 3D scene via 3Dmol `pngURI()` (`PNG_OK 17388`) — both PASS,
      nothing dropped. Absent data → the button is disabled, never an empty file. Also this unit: **core
      orbitals marked** (derived per-element table + energy-gap cross-check, not "1s per heavy atom"), and
      a **ball-and-stick / lines** representation toggle (to see a core 1s that hides inside the sphere).

**Phase 3 (the results dashboard) is complete** — for an Opt+Freq job the author can watch the
trajectory, spin the HOMO isosurface, click IR peaks to see the vibrations, and export geometry /
data / plots — all inside OrcaStudio. The one remaining `[ ]` above (the **unfixed-stereocenter flag
on SMILES import**) is a small cross-cutting import-time TODO, not a dashboard feature — it belongs
wherever an imported structure is first shown and is carried forward independently.

---

## Phase 4 — ORCA manual integration (≈ 1–2 weeks)

**Goal:** the app teaches ORCA while you use it. Key differentiator.

> **Ownership settled before the phase:** [ADR-013](wiki/architecture/adr-013-manual-indexing-ownership.md)
> — indexing runs in **Rust** (not the sidecar), over the raw **Markdown** `_sources/*.md.txt`
> (ATX headings, no HTML parser), writing the Rust-owned SQLite; the app never fetches the manual
> over the network (an out-of-band per-version author script does); `keywords.json` is seeded from
> the manual's own Keywords/genindex and curated on top. Narrows ADR-006. (Statuses below unchanged.)

- [x] One-off indexing pipeline (Rust, not sidecar — ADR-013): raw Markdown `_sources/*.md.txt`
      (ATX headings, no HTML parser) → sections → SQLite FTS5 (stored locally; never redistributed).
      Fetch is an out-of-band per-version author script. **Done (units 4.1–4.3):** fetch
      (`scripts/fetch-manual.py`), sectioner + `objects.inv` anchor map (`src/manual/`, line
      conservation), and the v9 schema (`manual_sections` + external-content `manual_fts` +
      provenance) with `build_manual_index` ingest (byte-for-byte read-back post-conditions). 1586
      sections, 1068 verified anchors.
- [x] Manual panel: full-text search with keyword highlighting, section rendering. Backend (4.3):
      `search_manual` → `bm25`-ranked hits; **UI (4.4):** `ManualScreen` (debounced search, results as
      breadcrumb › title + highlighted snippet) + `get_manual_section` → a standalone `SectionView`
      (drawer-ready for the hover unit). Render is **minimal + loss-free** — fences as monospace, all
      else verbatim; a preservation test asserts every non-whitespace char of `body_md` survives.
      Snippet markers moved off `[`/`]` (1905/1903 in the corpus) to PUA codepoints (0). Verified in
      the real WebKitGTK window. See [manual-index.md](wiki/modules/manual-index.md).
- [~] `keywords.json`: input keywords (`!` line + `%` blocks) → manual sections; **seeded** from the
      manual's own native "Keywords" sections + genindex, then curated on top (ADR-013 narrows
      ADR-006's "by hand"); grow it organically, starting with everything the template library uses.
      **Seed done (4.4 B):** `src/manual/keywords.json` seeded from the structured pool (home only);
      stable key `(file, breadcrumb, title, nth)`; ambiguous keywords carry `targets[]`. **Coverage now
      gated against an explicit, named inventory** (`keyword-inventory.json`, builder+domain+workflow;
      one home, both gates): **45 of 53 resolve** — the honest number after fixing the *population*, not
      just the form. The 8 gaps are classified by closer (a `{numref}` ×1 · b curated ×3 · c
      second/right form ×4 · d none) — see the table in
      [manual-keywords.md](wiki/modules/manual-keywords.md). **Left (by closer):** curation for
      `IRC`/`ScanTS`/`NEB-CI` + `CPCM`/`XTB`/`TightOpt`/`Constraints`-in-%geom; the `{numref}` layer
      (closes only `%maxcore`, so low priority); hand summaries.
- [x] Monaco hover provider: hover a keyword → keyword/type/owning-block + Open → a **side drawer**
      showing the section (reusing `SectionView`, author stays in the editor). Fed by `keywords.json`,
      **NOT** FTS; qualified type/block-aware lookup (`enclosingBlock` + `wordPattern`), `aliases[]`
      consulted. **On a miss the hover does not appear at all** (silence), never a bare-name/FTS
      fall-back — hovering `%maxcore` shows nothing (rule #11: silence beats plausible-but-wrong).
      keywords.json→DB bridge (`resolve_manual_section`) verified: 317/317 descriptors → exactly one
      row. Coverage gate rewritten in consumer form (44/46 type-aware; `%maxcore`/`CPCM` named gaps the
      old string gate hid). See [manual-keywords.md](wiki/modules/manual-keywords.md).
- [ ] (Optional) "Explain with Claude": keyword + current input context + manual excerpt →
      Anthropic API (user's own key in settings) → plain-language explanation

**Done when:** hovering `RIJCOSX` in the editor explains what it is, and the manual panel
answers "how do I set up CPCM for water" in one search.

---

## Phase 4.2 — Geometry editor completion

**Goal:** finish the geometry editor on the identity and state model of
[ADR-010](wiki/architecture/adr-010-editor-identity-state.md) — not as a list of ergonomic
gaps, but as a small set of typed operations over one authoritative core. The renderer stays
3Dmol (a dumb renderer) until the [ADR-011](wiki/architecture/adr-011-editor-graphics-stack.md)
spike passes; this phase touches no pixels of its own except where 3Dmol is fed a new table.
Estimate deferred until stage 1 is scoped.

**Stage 1 — identity core** (the largest part of the win; touches no pixels)

- [ ] `AtomId` (opaque, stable), branded `OrcaIndex` / `AseIndex` (mixing does not compile),
      and `IndexMap` as the only conversion — extracted so atom identity is one thing across the
      codebase. Per ADR-010 correction (i), the sidecar stays positional and Rust builds the
      `IndexMap` at the boundary (the same seam Phase 3 already exercises).
- [ ] `emit_input` / `parse_output` paired: `parse_output` cannot be called without the mapping
      produced by the matching `emit_input` (type-level invariant). Property test: round-trip
      `set(AtomId) → emit → parse → set(AtomId)` is identity.

**Stage 2 — operation log + ephemeral layer**

- [ ] State becomes a fold over a log of typed operations; undo/redo fall out of the log.
- [ ] 3Dmol becomes a **dumb renderer**: it is handed geometry + an `AtomId → viewer index`
      table and is never a source of truth (ADR-010 / ADR-011).
- [ ] The xyz block in Monaco becomes a **generated read-only projection** of the Scene.
      **Cost to preserve:** today the author edits coordinates directly in Monaco — making the
      block read-only removes that path, so it must be *replaced, not deleted*, by a
      "paste xyz → import as a fragment" action. The capability moves; it does not disappear.

**Stage 3 — operations over the core** (each item is an `Op`, not new state)

- [ ] Rigid-body drag of a fragment. **Risk — the first *continuous* interaction in the app:**
      during the drag only the viewer moves (ephemeral layer, 60 fps, not logged); the Scene and
      the input text update **once on release**, as a single step with a single Undo.
      Post-condition: pairwise distances *within the mask* are unchanged by the drag (rigid-body
      move, verified in our terms — domain rule #9).
- [ ] Rotation of a fragment about its approach axis (an `Op` over the mask).
- [ ] vdW-overlap detection after a move (warn on clashes the coarse placement can produce).
- [ ] Undo deeper than one step (falls out of the operation log; today edit mode is one-step).
- [ ] Ring torsions (rotate a torsion whose bond is inside a ring) — an `Op` over a
      graph-derived mask, building on the 2.5.3 bond-graph split.

**Carried from Phase 2.5** (unchanged, still open)

- [ ] Guided fragment placement: add a reagent *at a specified distance/angle/dihedral* in one
      step (today: coarse bounding-box add, then position via edit mode). Reuses
      `placeFragment` + the edit-mode `set-internal` path — the gap is the unified UI action.
- [ ] Constraint "toggle on/off" (currently delete + re-add covers it — see the 2.5.4b note).

**Done when:** the editor's state is a fold over a typed operation log, 3Dmol is a dumb renderer
fed an `AtomId` table, a fragment can be dragged rigidly with one Undo, and no bare integer
crosses a boundary the app owns.

---

## Phase 4.5 — Reaction modeling (≈ 3–5 evenings)

**Goal:** OrcaStudio becomes a reaction mechanism workstation. The researcher defines a
reaction, explores pathways via native ORCA scans, and compares electronic energy
barriers — the full computational experiment lifecycle. See ADR-007.

**Depends on Phase 4.2** — the reaction-center and scan setup UIs build on the completed
geometry editor (typed operations, one authoritative core); product-from-reactant derivation is
ADR-010's `ReactionPath` (`fold(reactant, transform)`, atom mapping by construction).

> **Open question — settle before any symmetry work.** ORCA may reorder atoms in its output when
> symmetry is active. Before relying on symmetry (`! UseSym` or point-group detection) anywhere
> in this phase, run a real `! UseSym` job and **check whether the output atom order matches the
> input order** (domain rule #10 — verify by a run, not from docs). If it reorders, that is a
> direct risk to [ADR-008](wiki/architecture/adr-008-scene-fragment-model.md) (one index space,
> merged-xyz order) and to ADR-010's `IndexMap`, and must be handled at the `parse_output`
> boundary before scans are trusted.

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
- [ ] Scan output parser: per-point energies + scanned-coordinate values from the **structured**
      `.relaxscanact.dat` / `.relaxscanscf.dat` (2 cols `coordinate energy`, one row per point;
      `act` = composite/actual, `scf` = bare SCF) — **measured** in unit 3.3 ([parse-sources.md](wiki/orca/parse-sources.md)).
      `.out` `RELAXED SURFACE SCAN RESULTS` is the text mirror; `.property.txt`/`_trj.xyz` are
      per-opt-cycle, **not** per-point. Coordinate in Å, energy in Eh (both cross-checked).
      Note for the scan *generator* (above): a relaxed scan needs `! Opt` — without it ORCA runs
      a single point and silently ignores the `Scan` block (measured).
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

- [ ] **UV-Vis (TD-DFT)** — *early Phase 6.* The broadening machinery already exists (shared with the
      IR spectrum), but UV-Vis is **not** a copy: broaden with a **Gaussian** (not Lorentzian); the
      x-axis is usually **nm** (a non-uniform reciprocal of a uniform-in-eV grid — plot in eV, label
      in nm, or resample); and **oscillator strength → absorbance needs an assumed band width** (yet
      another labelled display choice, like FWHM). A state table (energy, fosc, dominant transitions)
      accompanies the plot.
      - **Prerequisites (both measured, rule #10):** (1) a **probe for where excited states live** —
        `.property.txt` vs only the `.out` text — this gap is **already flagged open** (parse-sources
        "Gaps": no TD-DFT job exists yet); and (2) a **real TD-DFT run** to read from.
      - **Methodological caveat (record it in the UI):** **r²SCAN-3c is not suitable for excited
        states** — semilocal/composite functionals systematically underestimate charge-transfer
        states. A **hybrid or range-separated** functional (e.g. ωB97X-D4, CAM-B3LYP) is required;
        the app should steer the user there, not silently accept the ground-state method.
- [ ] **NMR (shielding → chemical shift)** — *after Phase 4.5, and here is why.* ORCA computes an
      **absolute shielding σ**, but a spectrum needs the **shift δ = σ_reference − σ_sample**, i.e. a
      **second calculation** (the reference, e.g. TMS) **with the exact same method/basis** — so a
      single job is never enough. On top of that: **Boltzmann averaging over conformers** (the GOAT
      primitive already exists, 2.5.1) and **averaging chemically-equivalent nuclei**. This is the
      **same structural need as ΔΔG‡** — aggregating **several jobs into one result** — which is why
      NMR waits until **Reaction is a first-class object** (Phase 4.5's data model), not before: the
      multi-job aggregation must exist first.
- [ ] NEB: path setup UI + energy profile visualization per iteration
- [ ] Job comparison view: N jobs side by side (energies, geometries overlay, spectra)
- [ ] Batch/parametric runs: same molecule × list of functionals or basis sets
- [ ] SLURM backend (third `ExecutionBackend` implementation)
- [ ] Markdown notes attached to jobs/projects → the app becomes a lab journal
- [ ] Multi-step mechanism support: Mechanism = ordered sequence of Reactions (catalytic
      cycles like Sonogashira: oxidative addition → transmetalation → reductive elimination)
- [ ] AI-assisted reaction setup: describe reaction → AI proposes reaction center, approach
      geometry, pathways (Anthropic API, user's key). **Authority boundary fixed by
      [ADR-014](wiki/architecture/adr-014-ai-integration-boundary.md)** (T2 draft: text the author
      reads before Run; geometric constants retrieved, never recalled).
- [ ] MCP server over the Tauri command layer (T3 of
      [ADR-014](wiki/architecture/adr-014-ai-integration-boundary.md)) — **depends on Phase 4.5**
      (nothing to orchestrate while the central object is the Job, not the Reaction)
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
