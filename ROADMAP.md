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
- [x] Fragment library: common reagents (BH₄⁻, H₂O, common ligands), place at position
      with specified distance/angle
      — placement = set_distance/angle/dihedral with the mask on the newly added fragment
      — **Done (Phase 4.2 tail-1):** the reagent library (`src/scene/fragment-library.ts`) +
        bounding-box `placeFragment` add a fragment, and the **guided placement panel** then drives
        it to a target d/θ/φ in ONE flow (`src/scene/guided-placement.ts` + `GuidedPlacementPanel`),
        reusing the `set-internal` edit path — the add-at-geometry action that was missing.
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

**Deferred out of 2.5, now DONE in `Phase 4.2 tail-1`:** *guided fragment placement* — adding a
reagent *at a specified distance/angle/dihedral* in one flow. The pieces (reagent library +
bounding-box placement + the `set-internal` edit path) are now composed into a single guided
add-at-geometry action (`src/scene/guided-placement.ts` + `GuidedPlacementPanel`). See the
now-`[x]` "Fragment library" item above.

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
- [ ] **Multi-fragment frontier-orbital labeling (teaching moment).** For a scene with ≥2
      fragments, the results card labels the gap as the SYSTEM gap and shows per-fragment
      localization of HOMO/LUMO (e.g. "HOMO 99.7% on Butylamine · LUMO 93.1% on Acetaldehyde"),
      derived from MO coefficient weights in the cached orca_2json. HONESTY CAVEAT (rule #11
      spirit): coefficient-squared weight without the overlap matrix is approximate
      (Mulliken-like sans S) — label the method, or use Loewdin per-MO populations if a
      determiner run shows ORCA prints them. Genuinely useful for Phase 4.5 (identifying
      donor/acceptor orbitals when building approach geometries) — do not gold-plate before then.
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
data / plots — all inside OrcaStudio. Two `[ ]` items above are carried forward independently, neither
a core dashboard feature: the **unfixed-stereocenter flag on SMILES import** (a small cross-cutting
import-time TODO — it belongs wherever an imported structure is first shown), and **multi-fragment
frontier-orbital labeling** (a teaching moment that becomes genuinely useful for Phase 4.5 donor/
acceptor identification — explicitly *not* to be gold-plated before then).

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
- [x] Manual panel: full-text search with keyword highlighting, **full-page rendering**. Backend (4.3):
      `search_manual` → `bm25`-ranked hits; **UI (4.4):** `ManualScreen` (debounced search, results as
      breadcrumb › title + highlighted snippet). **A section indexes, a page shows** — a result opens the
      whole page via `get_manual_page` and scrolls to / highlights the found section (`PageView`, the one
      display component, shared with the hover drawer; `SectionView` folded in). Render is **minimal +
      loss-free** — fences as monospace, all else verbatim; a preservation test asserts every
      non-whitespace char of `body_md` survives. Page reads the **file on disk** with a post-condition
      that disk matches the index (line count + heading identity), so a stale corpus fails loudly.
      `manual_root()` debt closed (source + bundled). Snippet markers moved off `[`/`]` (1905/1903 in the
      corpus) to PUA codepoints (0). See [manual-index.md](wiki/modules/manual-index.md).
- [~] `keywords.json`: input keywords (`!` line + `%` blocks) → manual sections; **seeded** from the
      manual's own native "Keywords" sections + genindex, then curated on top (ADR-013 narrows
      ADR-006's "by hand"); grow it organically, starting with everything the template library uses.
      **Seed done (4.4 B):** `src/manual/keywords.json` seeded from the structured pool (home only);
      stable key `(file, breadcrumb, title, nth)`; ambiguous keywords carry `targets[]`. **Coverage
      gated against an explicit, named inventory** (`keyword-inventory.json`, builder+domain+workflow;
      one home, both gates). **Type now from the manual, not our builder** (`type_of` dropped
      `app_simple`; no-owner tokens are `undetermined`, a value, not a dumpster block-option; the
      builder's knowledge moved to a `provenance:"curated"` channel). **47 of 53 inventory words
      resolve — 9 via SEED, 38 via CURATION**. Argument rule added (`CPCM(water)` → `water` gets no
      hover; a positive line-check). **BUT the inventory is our builder's vocabulary; measured against
      the manual's OWN `!` vocabulary the map covers only 10 % (42 of 424 tokens).** So the **curated
      layer is OPEN — a plan, not a list** (curation is demand-driven: only `XTB` was hit by a real
      run). Remaining declared gaps: `%maxcore` (a); `IRC`/`ScanTS`/`NEB-CI` (b); `TightOpt`/
      `Constraints`-in-%geom (c). Table + denominator in
      [manual-keywords.md](wiki/modules/manual-keywords.md) and `orca/manual-sources.md` Part I.
- [x] Monaco hover provider: hover a keyword → keyword/type/owning-block + Open → a **side drawer**
      showing the section's page (reusing `PageView`, author stays in the editor). Fed by `keywords.json`,
      **NOT** FTS; qualified type/block-aware lookup (`enclosingBlock` + `wordPattern`), `aliases[]`
      consulted. **On a miss the hover does not appear at all** (silence), never a bare-name/FTS
      fall-back — hovering `%maxcore` shows nothing (rule #11: silence beats plausible-but-wrong).
      keywords.json→DB bridge (`resolve_manual_section`) verified: 317/317 descriptors → exactly one
      row. Coverage gate rewritten in consumer form (44/46 type-aware; `%maxcore`/`CPCM` named gaps the
      old string gate hid). See [manual-keywords.md](wiki/modules/manual-keywords.md).
- [x] "Explain with Claude": select a keyword → its line + resolved manual section →
      Anthropic API (user's own key) → plain-language explanation in the drawer. **Layer 1 of 3**
      (ADR-014 T1 amendment): selection + section → one answer; no chat, no history, no input box.
      Storage + live model list in [ADR-015](wiki/architecture/adr-015-api-key-storage.md); the key
      never enters the webview (Rust makes the call); wire payload bounded to word+line+section by the
      command's type; the explanation writes nothing to the editor (tier-zero). Layers 2 (follow-up in
      the same grounded context) and 3 (chat with `search_manual` as a tool) are later units.

**Done when:** hovering `RIJCOSX` in the editor explains what it is, and the manual panel
answers "how do I set up CPCM for water" in one search.

---

## Phase 4.2 — Geometry editor completion ✅ COMPLETE

**Goal:** finish the geometry editor on the identity and state model of
[ADR-010](wiki/architecture/adr-010-editor-identity-state.md) — not as a list of ergonomic
gaps, but as a small set of typed operations over one authoritative core. The renderer stays
3Dmol (a dumb renderer) until the [ADR-011](wiki/architecture/adr-011-editor-graphics-stack.md)
spike passes; this phase touches no pixels of its own except where 3Dmol is fed a new table.
Estimate deferred until stage 1 is scoped.

**Stage 1 — identity core** (the largest part of the win; touches no pixels). Broken into units
1a–1e once [ADR-016](wiki/architecture/adr-016-emit-input-ownership.md) fixed that `emit_input`
(the order-bearing part) moves to a Rust `orcastudio-core` crate so the ADR-010 `emit`/`parse` pair
is same-language and compiler-enforced. **Only the order-bearing emit moves** — the Scene store,
editor UI, method/basis form, and the geometry↔sidecar seam stay TS until Stage 2/3.

- [x] **1a — UseSym probe + ingest.** Measured whether `! UseSym` reorders atoms in ORCA 6.1.0
      output artifacts (gate for the whole IndexMap design): **no observable reorder** in the
      measured scope (`wiki/orca/usesym-atom-order.md`) → the `parse_output` map is the **identity**,
      carried as a **post-condition** (element-seq + fingerprint on real output), not trusted.
      Ingested ADR-016 (emit ownership), ADR-010 correction-(i) refinement, and the ADR-014
      charge/multiplicity amendment (pending review).
- [x] **1b — `AtomId` in the TS Scene.** Branded `AtomId` (`ids.ts`), `RawAtom`/`SceneAtom` +
      `RawFragment`/`SceneFragment` split (identity minted only at the Scene boundary), pure
      allocation via `nextAtomId`, positional id-carry across every replace (`carryIds`, never
      re-minted — negative-control tested), scene JSON **v2** with **v1 migrated on read** (no SQL
      migration; real-fixture + collapse-guard tests). No Rust, no pipeline rebrand.
- [x] **1c — the `orcastudio-core` crate.** Workspace (`src-tauri` + `orcastudio-core`, std-only,
      WASM-ready). `AtomId` / `OrcaIndex` / `AseIndex` / `IndexMap` (mixing does not compile —
      `compile_fail` doctests); v2 deserialization (same validation as TS; **v1 → loud named Err**,
      migration stays in TS); `emit_input → (text, IndexMap)` for the coordinate block + `%geom
      Constraints`, **byte-identical** to the TS emit (golden vs committed TS-emitted fixtures). The
      byte-identity rests on two measured formatters — `fmt_coord` (odd/512 ties + signed zero) and
      `fmt_value` (17-digit ties absorbed by `value_text`) — see
      [float-formatting-parity.md](wiki/architecture/float-formatting-parity.md). Dead code until
      1d/1e wire it. Desktop-window build (`tauri dev`/`build`) left for the author (workspace target
      dir); `cargo build/test --workspace` verified.
- [x] **1d — parse pairing.** The ADR-012 readers (`property`/`hess`/`xyz`/`mo`) take the job's
      `IndexMap<OrcaIndex>` in the `verify()` path: the former element-order post-condition is
      rephrased as "artifact order == the order the map asserts" (`parse::check_map_order`), for the
      identity map the same check as before, so the dashboard numbers are unchanged. Schema **v10**
      adds nullable `jobs.index_map_json`; every row is NULL in 1d, so the parser derives an identity
      map from the input coordinate block (`results::job_index_map`), cross-checked against the
      artifact. Round-trip property test in core (`set(AtomId) → emit → parse → set(AtomId)` = identity,
      2000 seeded scenes). Negative controls demonstrably bite (permuted map / wrong count refuse;
      disabling the map check turns the permutation green). **Claim corrected:** the pair is a
      *type-level* invariant only *in-process* (same crate on both sides); across SQLite the provenance
      is serialized away, so the map is a **required, artifact-cross-checked argument** (post-condition,
      rule #9), NOT a type invariant — stated verbatim in code and on the module page.
- [x] **1e — wiring.** `create_job` mints the `IndexMap` from the **submitted text verified against
      the scene** (`orcastudio_core::mint_index_map`; never from the scene alone — a scene/text drift
      SKIPS, `{"skipped":…}`), stored in `jobs.index_map_json`. `results::resolve_job_mapping` uses a
      minted map with a **scene-sourced anchor** (independent of the stored map, so a corrupted stored
      map is caught), else the derived identity map (1d). The xtb constraint serde boundary is
      **branded** — `SceneIndex` (0-based, no `Display`) → `XtbIndex` (1-based) via one `to_xtb()`, so
      the `$constrain` `+1` flip is a single typed conversion (a bare index is a compile error). The
      display-vs-authoritative tension is resolved in the **ADR-016 amendment**: display emit stays TS,
      authoritative mint at `create_job`, and NO runtime byte-check (it cannot tell a legal edit from a
      drift). Negative controls (a)–(d) all demonstrably bite.

**Stage 1 is COMPLETE** — the identity core (ADR-016) is authored in Rust (`orcastudio-core`),
emit-input is byte-identical to the TS source, the parsers are paired with the `IndexMap` and
verify it against the artifact, the map is minted at `create_job` from verified text, and the xtb
index-base seam is branded. Stage 2 (operation log) is **complete**; Stage 3 (operations over the
core) is **complete** (rigid drag 3.1, vdW clash 3.2, rotation 3.3/3.3b; manual ring torsions cut —
GOAT owns ring conformations). (Scene→Rust/WASM is ADR-011, deferred behind a spike — it is
**not** what Stage 3 means; Stage 3 = operations over the core.)

**Stage 2 — operation log** ✅ **COMPLETE** (units 2a–2d, once [ADR-017](wiki/architecture/adr-017-operation-log.md)
fixed the log design: each entry **materializes** its resultant snapshot — provenance, not a
recompile recipe, so history can't be rewritten by an ASE bump. The ephemeral drag layer moved to
Stage 3, where it is actually needed.) **Stage 3 is now COMPLETE (see below).**

- [x] **2a — operation log: pure types + ingest.** `src/scene/oplog.ts` — pure, no store/viewer/
      Monaco/DB/Rust: the tagged-union `Op` (one variant per Scene mutator — checklist in ADR-017,
      so 2b finds no hole), `describe(op)` (a human lab-journal line per variant already at the type
      layer), `LogEntry {op, scene}` with the snapshot **deep-frozen** and materialized, `SceneLog
      {entries, pointer}`, `append` (truncates the redo tail), `undo`/`redo`/`current`, and log
      format **v1** serialization (scenes embedded as v2 via the existing `serializeScene`, versioned
      independently). Pointer invariant `-1 ≤ pointer < len` (`-1` = empty scene). Negative controls
      demonstrably bite: **(a)** breaking tail-truncation reddens "redo after append impossible";
      **(b)** neutering the deep-freeze reddens the immutability gate. Sizes **measured** (38-atom
      reaction scene: ~2.9 KB/snapshot, ~3.5 KB/entry, ~345 KiB per 100-op session) → **no length cap
      yet** (deferred with numbers, not chosen blind). Ingested ADR-017.
- [x] **2b — the store folds over the log.** `scene` is now **derived** (`scene === current(log)`,
      always), so there is **no `setScene`** — the only doors are `commit(op, resultScene)` and
      `installLog(log)`; the mutator-bypasses-the-log defect is *impossible by construction* (control
      (a) proves it red). Deep undo/redo/`jumpTo` (superseding the one-step `previous`/`undoReset`);
      a **read-only history panel** (`describe()` lines, click = pointer jump, Undo/Redo + Ctrl/Cmd+Z).
      `scene_log_json` persisted (schema **v11**, guarded ALTER) **co-written with `scene_json` in the
      one INSERT**; "New iteration" restores the log **cross-checked against the snapshot** — a
      diverged log is **rejected loudly, the snapshot wins** (the map-minting contract, unit 1e, is
      untouched; control (b)). The collapse↔undo loop is **dead** (collapse is a logged op; undo
      re-injects, no second collapse — control (c)). `jobs.scene_json` stays the v2 snapshot (ADR-017
      decision 3). Three `scene: null` consumers defined (sync / input builder / minting). Sizes: no
      cap (ADR-017 decision 4).
- [x] **2b-ux — the editor workspace: viewer-first right dock, fullscreen as workspace.** Pure
      layout (zero model changes — `scene`/`oplog`/`store`/`constraints` untouched): the 3Dmol canvas
      becomes the primary surface and the panels move from a stack **below** the viewer into a **right
      dock** — a thin always-visible icon rail that expands per section (Selection · Edit · Fragments ·
      Constraints · History · Actions, in use-order), each toggling independently (session state). The
      **same dock lives inside fullscreen** (the workspace mode), so Add Fragment / measure / History /
      constraints are all reachable without leaving it. Resize uses the **one existing mechanism** —
      `MoleculeViewer`'s `ResizeObserver` fires `viewer.resize()` on the container box change (dock
      toggle + fullscreen), same as the split-panel resize; no per-toggle call. *Why before 2c:*
      viewer-first is the frame the AtomId-picking work (2c) plugs into, and a usable workspace makes
      testing the rest of Stage 2 far easier. See `wiki/modules/editor-ui.md`.
- [x] **2c1 — 3Dmol becomes a dumb renderer.** It is handed geometry + an `AtomId → viewer index`
      table and is never a source of truth (ADR-010 / ADR-011); picking resolves through the table to
      an `AtomId`. `buildViewerFeed(scene)` returns geometry **and** table from one pass (they cannot
      drift); `onAtomPick` returns an `AtomId` (raw `atom.index` kept only as a diagnostic
      `viewerIndex`); reads-from-3Dmol audit clean (one import site, two sanctioned reads). Consumers
      stay positional behind one named `2c1→2c2` adapter. See `wiki/modules/editor-ui.md`.
- [x] **2c2 — the pipeline moves onto `AtomId` + the removal dividend.** `selection` / `measure` input
      / `planEdit` input / `constraintFromSelection` input / the viewer highlight key on `AtomId`.
      `selectionSurvives`/`validateSelection` are **removed**; `filterSelection` keeps every picked id
      still in the scene, so removing an *unrelated* fragment no longer clears the selection — the
      **conscious behaviour change** this unit exists for (the old clearing was correct for the
      positional space). Two things stay positional at their emit seams: the **ASE mask** (`EditPlan`)
      and the **`%geom` constraint** (`Constraint` atoms are ORCA indices); `AtomId → index` conversion
      happens at exactly those two seams via `globalIndexOfAtom`. History panel shows the global index
      of each record (`describeInScene`, `describe` stays pure). UI labels the index space per panel
      (global for the inspector, ORCA for constraints). Negative controls (a)–(d) demonstrated red. See
      `wiki/modules/scene.md`, `editor-ui.md`.
- [x] **2d — the Monaco xyz projection (Stage 2 closed).** The `* xyz … *` block in Monaco is a
      **generated read-only projection** of the Scene (ADR-010 authority split: text owns chemistry
      `!`/`%`, the Scene owns geometry). The Monaco→Scene effect **reverts** a block hand-edit (keeping
      keyword edits), so no path adopts hand-typed coordinates into geometry — the pre-2d
      `collapseFromText` store mutator + Monaco-collapse reaction are **removed**, and
      `collapse-from-text` survives only as a **legacy** op type for deserializing pre-2d logs
      (ADR-017 amendment). **Capability replaced, not deleted, by two doors:** *Paste xyz* (import as a
      fragment — the typical path) and *Replace input* (a one-shot conscious escape: unlock the buffer,
      paste a different calculation, Adopt it as a fresh `text-adopt` scene). Pure gates: **c1** (import
      builds an `add-fragment` preserving atom count+order), **c2** (Replace re-seed installs a fresh
      log, no lineage leak), plus the revert/keep/seed decision — each with a proven-biting negative
      control. Manual gates **m1–m4** verified live (WebKitGTK dev server): a coordinate edit reverts
      and the scene stays multi-fragment; a `!`/`%` edit passes through; Paste xyz adds a fragment
      (journal "Add fragment …"); Replace input adopts a fresh scene and re-locks the block. See
      `wiki/modules/scene.md`, `editor-ui.md`, `frontend.md`.

**Stage 3 — operations over the core** ✅ **COMPLETE** (each item is an `Op`, not new state)

- [x] **Rigid-body drag of a fragment (unit 3.1).** **The ephemeral layer landed here** (ADR-010):
      during the drag only the viewer moves (60 fps, **not** logged) via the frozen-topology
      coordinate-update path; the Scene and the input text update **once on release**, as a single
      `translate-fragment` op with a single Undo. Move mode is a toggle in the Edit dock section; the
      grab intercepts 3Dmol's canvas mousedown in the capture phase so the camera doesn't rotate for
      the drag (empty-space drag still rotates; a click still picks). Screen→world is 3Dmol's
      `screenOffsetToModel` at the grabbed atom's depth — measured **pixel-exact** (`debugging/013`, the
      mandatory probe). Post-condition (rule #9): every mover atom shifts by the same delta, internal
      pairwise distances + count/order/AtomId invariant, other fragments untouched. Pure gates **c1**
      (translate commit), **c2** (one op / summed delta), **c3** (Scene untouched mid-drag), each with a
      proven-biting negative control; manual gates **m1–m4** verified live (drag moves one fragment /
      camera held; one history entry + one Undo; rotate+pick outside Move mode; ~1.3 ms/frame at 38
      atoms). See `wiki/modules/visualization.md`, `editor-ui.md`, `debugging/013`.
- [x] **Rotation of a fragment about its approach axis (unit 3.3).** A rigid whole-fragment spin about
      the axis two picked atoms define — P (pivot, on the rotating fragment) and Q (direction, typically
      the substrate contact atom). **Pure TS, a sibling of — not routed through — the sidecar set-internal
      edit** (`rotateFragment`/`rotateFragmentInScene` in `scene.ts`, Rodrigues; rigid transform vs
      internal-coordinate solve). A **numeric** angle (reproducible; spin-drag deferred) drives a live
      **ephemeral preview** through the same frozen-topology coordinate-update path as the drag (viewer
      only, Scene untouched); **one** `rotate-fragment` op commits on Apply (the op stores the two axis
      ATOMS + angle, so the journal reads "Rotate BH₄⁻ 30° about O→C"), Cancel drops the preview with
      zero ops. Post-condition (rule #9): rigid (internal pairwise distances invariant), P and every
      on-axis point fixed, other fragments untouched, ids/order invariant. Pure gates **c1** (rigid),
      **c2** (Rodrigues — closed-form + round-trip + identity + on-axis), **c3** (one op on Apply; preview
      never commits), **c4** (axis P→Q / pivot P), **c5** (degenerate axis → no-op), each with a
      demonstrated-biting negative control. See `wiki/modules/scene.md`, `visualization.md`, `editor-ui.md`,
      ADR-017. **Manual gates m1–m5 pending live verification in the Tauri window.**
  - **Polish 3.3b — axis/distance overlay toggle.** The axis cylinder (3.3) and the measurement distance
    line drew on the same two atoms at once (two overlapping objects that read as one wrong line). Now a
    toggle shows **exactly one** overlay for the pair — Axis (cylinder + Å on the axis midpoint) or
    Distance (measurement line + label), never both; the Å number is the same in both (single source:
    `measure` distance). Pure decision `chooseRotateOverlay` (`viewer/rotate-overlay.ts`), gates **c1**
    (default/flip) + **c2** (never both — demonstrated-biting). App-owned `rotateOverlay`, reset with the
    axis; the measure tool outside Rotate is untouched. **3.3b-fix (mandatory live gate):** the toggle was
    imperceptible for TWO measured reasons — (1) the axis borrowed the green `haloColor`, indistinguishable
    from the green measurement line → gave it a distinct **azure `theme.axisColor`** (hue-distinctness +
    contrast locked in `theme.test.ts`); (2) a **render loop** (`rotationAxis`'s fresh object in an effect's
    deps → `setRotateAxis` every render → "Maximum update depth", and the `[rotateAxis]` reset snapped the
    toggle back to Axis) → memoized `axis` + split the panel effect. **m1–m3 confirmed live** (WebKitGTK):
    Axis → azure cylinder, Distance → green line (visible change), back to Axis → cylinder returns, Å
    identical; console clean (was 92 loop errors/min). **DONE.**
- [x] **vdW-overlap detection after a move (unit 3.2).** After any geometry change, **inter-fragment**
      atoms closer than `k·(rᵢ+rⱼ)` of their cited vdW sum (Bondi/Mantina/Alvarez — `scene/vdw-radii.ts`,
      **B** and **Pd/Pt** covered; uncovered → UNDETERMINED, skipped + surfaced, never guessed) are
      flagged as a **warning, not a block**: a banner + a distinct **magenta danger glow** (apart from
      the halo/mask). `k` is a **labeled heuristic slider** (default ≈0.65, app-owned, not in Scene).
      **Pairs with an active distance constraint are excluded** (intentional contacts — read via the
      existing `constraints.ts` parser), so a Bürgi–Dunitz reactive approach never false-alarms (mission
      gate m4, confirmed live: 0 clashes at C···B ≈ 2.8 Å under default k; a constraint on the forming
      pair drops the count while genuine peripheral clashes remain). Pure `detectClashes`; controls
      c1–c5 (found pair / no false positive / UNDETERMINED / constraint-exclusion / k-monotone) each
      demonstrated red; manual m1–m5 verified live. See `scene.md`, `editor-ui.md`, `chemistry/vdw-steric.md`.
- [x] **Undo deeper than one step** — done in unit 2b: undo/redo/`jumpTo` fold over the whole
      operation log (superseding the old one-step `previous`/`undoReset`), so every op (drag included)
      is a full history step.
- **Cut (2026-08-06):** manual ring torsions. Ring conformations (chair/boat/twist, ring flip) belong
      to **GOAT** (Find conformers, unit 2.5.1) — the correct tool, which samples ring puckering without
      deforming bonds. A manual ring-bond rotation would break the ring (an unphysical intermediate),
      contradicting the "no plausible-but-wrong physics" ethos, and would duplicate GOAT. The 2.5.3
      ring-bond **refusal stays** (it is correct) and now *points at GOAT* (`sidecar/app/geometry.py`).

**Carried from Phase 2.5** (unchanged, still open)

- [x] **Guided fragment placement (tail-1):** add a reagent *at a specified distance/angle/dihedral*
      in one flow — pick a reagent (rough `placeFragment` + `add-fragment` op, unchanged), then the
      **guided approach-geometry panel** (Fragments section): pick the reagent atom + 1–3 substrate
      anchors, enter d (required) / θ / φ (each optional — an empty field is a SKIP, not a 0),
      Preview → Apply. Reuses the edit-mode `set-internal` path VERBATIM: each given coordinate is one
      `planEdit`/`applyResponseToScene` step masking the reagent fragment, committed as its own
      `replace-fragment-atoms` op (Undo unwinds d/θ/φ one at a time). `src/scene/guided-placement.ts`
      (pure planner + DI driver) + `GuidedPlacementPanel.tsx`; c1–c4 negative controls proven-biting.
      Z-matrix nesting (d then θ then φ) so each edit preserves the earlier coordinate.
- [x] **User-extensible reagent catalog + seed cations (tail-2):** the curated `FRAGMENT_LIBRARY`
      gains monatomic cations (Na⁺/Li⁺/K⁺ +1, Mg²⁺ +2 — closed-shell, no internal geometry), and a
      user can **save their own reagents** — a `molecules` row with a role flag (`is_reagent`,
      schema **v12**) reusing the existing table (`charge` was already there). `create_reagent` /
      `list_reagents` commands (charge **mandatory** at save, never a silent 0 — ADR-014);
      `list_molecules` filters role 0 so the molecule library and its screen are unchanged. The
      palette shows **Built-in** (reference-contract, curated) and **My reagents** (user provenance,
      no reference) as visually distinct groups; a user reagent's charge flows into the scene total
      by the same path as a built-in. `src/scene/reagent-catalog.ts` + cargo/vitest c1–c4
      proven-biting. **This closes Phase 4.2.**
- ~~Constraint "toggle on/off"~~ — **cut (decision 2026-08-07).** Delete + re-add already covers it
      (the 2.5.4b note); a persistent enabled/disabled state would add a second source of truth over
      the constraint text (the very drift `constraints.ts` was built to avoid) for marginal
      ergonomics. Not worth the complication — the same reasoning as the manual ring-cut refusal.

**Done when:** the editor's state is a fold over a typed operation log, 3Dmol is a dumb renderer
fed an `AtomId` table, a fragment can be dragged rigidly with one Undo, and no bare integer
crosses a boundary the app owns. **✅ MET — Phase 4.2 is COMPLETE** (Stages 1–3 + tails 1–2).

- [x] **Bond display control (viewer polish, post-completion).** 3Dmol draws bonds by distance, so an
      s-block cation (Na⁺/K⁺/Mg²⁺…) coordinated to O/N/π gets a spurious covalent stick — **excluded by
      default** (a "show cation bonds" toggle reveals them) — plus a general **manual hide/show** of any
      bond, keyed by the **AtomId pair** (survives drag/rotate/re-perception). **DISPLAY-ONLY**: filters
      the perception 3Dmol already did (no second pass, like `frozenTopology`), changes no geometry —
      Scene / Monaco xyz / total charge / generated `.inp` are byte-identical. `src/viewer/bond-display.ts`
      + c1–c4 proven-biting. Builds on the tail-2 cation catalog (the elements that need it).

---

## Phase 4.5 — Reaction modeling (staged)

**Goal:** OrcaStudio becomes a reaction mechanism workstation. The researcher defines a
reaction, explores pathways via native ORCA scans, and compares electronic energy
barriers — the full computational experiment lifecycle. See ADR-007.

**Depends on Phase 4.2 (✅ COMPLETE) + Phase 3 result parsing (✅ COMPLETE)** — the
reaction-center / scan-setup UIs build on the finished geometry editor (typed operations,
one authoritative core); the profile/frequency artifacts this phase reads (relaxed-scan
`.dat`, `.hess`) are the Phase 3 authoritative-parsing tier; product-from-reactant derivation
is ADR-010's `ReactionPath` (`fold(reactant, transform)`, atom mapping by construction).

**Staging principle** (as everywhere in this project): probe/measure before build; least
risk first; every stage ends with something the author actually uses; the Reaction object
appears in the stage where grouping/comparison first needs it, not as infrastructure ahead.
The **scan spine (Stages A–C) is already de-risked** — a real relaxed scan was measured in
unit 3.3 (`.relaxscanact.dat`/`.relaxscanscf.dat`, 2 cols coordinate Å + energy Eh; a relaxed
scan needs `! Opt` or ORCA silently does a single point — `wiki/orca/parse-sources.md`). The
**unmeasured** work (TS methods, CREST) carries its own probe at the head of its stage.

### Standing gates (not stages — apply throughout)

- [ ] **Symmetry re-probe (per system, before trusting `! UseSym`).** Measured NO reorder for
      the common groups (`wiki/orca/usesym-atom-order.md`), but D-/cubic groups, explicit
      `%Symmetry PointGroup`, and large systems are unmeasured; permutations of equivalent atoms
      are unobservable in principle. Re-run the probe on a specific reaction system before relying
      on symmetry there (domain rule #10); the `parse_output` post-condition catches a reorder at
      the boundary (ADR-016) before scans are trusted.
- [ ] **Unfixed-stereocenter flag on SMILES import** (carried from Phase 3). RDKit's ETKDG picks
      an enantiomer arbitrarily for a SMILES with no stereo descriptors — a *silent substitution of
      the compound* for stereoselectivity work. Cheap prerequisite for si/re work; land it before
      Stage C.

---

### Stage A — Scan input generation from a picked coordinate (spine, part 1)

- [x] **A1 — scan-coordinate emit (pure + Rust golden + real ORCA; no manual gate).**
      `ScanCoordinate` (kind B/A/D, atoms [2|3|4], start/end as exact user text, npoints int),
      0-based app index space (same as `Constraint`). `emit_scan_block` in
      `orcastudio-core/src/emit.rs` (sibling of `emit_constraints_block`, same `to_orca_index`),
      TS mirror `scanBlock` in a new `src/scene/scan.ts` — **byte-identical Rust/TS pair**
      (ADR-016). `injectScan` **composes into the existing `%geom`** (Scan + Constraints as sibling
      sub-blocks under one `end…end`), never a second `%geom`. `parseScanBlock`/`inspectScanBlock`
      (read-back for A2). `! Opt`-requirement guard (else silent single-point — measured).
      Verified by a real app-generated ethane C–C scan (mirrors 3.3): `.relaxscanact.dat` has
      npoints rows + `ORCA TERMINATED NORMALLY`.
- [x] **A2 — Scan panel + define-coordinate-from-selection (UI → manual gate).** Pick 2/3/4 atoms
      via existing `selection`/`measure` → a Scan panel that is a **view over the input text**
      (mirror `ConstraintPanel`): start/end/npoints inputs → `injectScan`. Run-guard surfaced
      (no measured opt keyword → loud, blocks Create & Run, like the constraint range-check).
      **Code + tests complete** (`ScanPanel.tsx`, `scanFromSelection`, `hasOptKeyword` broadened to
      the measured set `{opt, optts, tightopt, verytightopt, looseopt}` — probe recorded in
      `wiki/orca/scan.md`; three negative controls green). **Manual gate (author) PASSED** — g1–g4
      in the real Tauri window.

**Stage A done when:** the author picks an approach coordinate, sets start/end/steps, generates a
relaxed-scan input, and runs it — no hand-editing of `%geom`. *(A1 ✅; A2 ✅ — author g1–g4 gate
passed. **Stage A complete.**)*

### Stage B — Scan output parser + single energy profile (spine, part 2)

- [x] **B1 — scan reader (Rust, over structured artifacts).** `src-tauri/src/parse/relaxscan.rs`
      over `.relaxscanact.dat`/`.relaxscanscf.dat` (2 cols coordinate Å + energy Eh, measured 3.3),
      artifact-reader template + post-conditions (act/scf same N + identical coordinate column, N≥2,
      energies finite, coordinate strictly monotone). **The coordinate's Å is confirmed per-read** by
      a geometry cross-check vs each `input.NNN.xyz` (rule #11; Bohr fails ≈1.889×) — the load-bearing
      post-condition. `act`/`scf` both stored, labelled, never conflated. Wired into `results.rs`
      (`ParsedResults.scan`, `data_json`, `parser_version` 3→4). Three negative controls green on real
      ethane fixtures. No manual gate (Rust reader).
      **B1 fix (scan jobs parse profile-only)** — the B2 manual gate caught a real bug: the full
      `parse_and_store` on a completed scan failed the single-structure `property.rs` geometry
      post-condition (first `$Geometry` = scan point 1, not the input — a false-Bohr `0.056 Å`). A
      multi-point scan `.property.txt` does not fit the single-structure readers, so `parse_and_store`
      now routes a scan (detected by `input.relaxscanact.dat`) to **profile-only**: parse the profile,
      skip property/`_trj`/hess/mo, header energy + final geometry from the profile's last point. No
      tolerance loosened; the units guard is the profile's coordinate cross-check. Full-pipeline test
      on the real `scan-ethane-cc/` dir (RED→GREEN) closes the test gap. See
      `wiki/debugging/015-scan-property-post-condition.md`.
- [x] **B2 — energy-profile view (React → manual gate).** Reaction coordinate (Å) vs **ΔE kcal/mol**
      (relative, labelled reference: point 1 / minimum; `act`/`scf` both, labelled). recharts +
      `useContainerWidth`; click a scan point → app-owned index → load `input.NNN.xyz` of that point
      into the viewer (one frame, ADR-011), element-order checked at the boundary. Maximum marked
      **"approximate TS (scan maximum)"** — a ΔE‡ estimate, never the TS / ΔG‡. New Rust command
      `read_scan_geometries` (reads point files, writes nothing). **Code + tests complete**
      (`ScanProfilePanel` + `scanProfile.ts`; three negative controls green; `read_scan_geometries`
      read-only; cargo tests green). Two fixes landed against the manual gate before it passed:
      **B1 fix** (scan jobs parse profile-only — `debugging/015`) and **B2 fix** (recharts-v3
      chart-click via a shared, tested `resolveClickedIndex` — `debugging/016`). **Manual gate
      (author) PASSED** — h1–h4 in the real window.

**Stage B done when:** run a scan → see the profile → click the maximum → see that geometry. The
single-pathway relaxed scan is fully usable end-to-end. *(B1 ✅; B2 ✅ — author h1–h4 gate passed.
**Stage B complete.**)*

### Stage C — Reaction as a first-class object + comparative ΔΔE‡ (data model earns its place here)

- [x] **C1 — data model (migration v13).** `reactions` + `pathways` tables + a **nullable
      `jobs.pathway_id` only** (migration v13, guarded like v10/v11). **Normalized deviation from
      ADR-007's both-FKs sketch:** a job carries `pathway_id` only; its reaction is derived by
      joining `pathways` — no `reaction_id` on jobs (one source of truth). `reaction_centers` is
      **not** built here (it belongs with the reaction-center editor, later); a pathway stays lean
      (`{ id, reaction_id, label }` — no coordinate/method/profile; C2 reads those from the job).
      **Jobs-survive invariant:** deleting a reaction/pathway nulls the attached jobs' `pathway_id`
      and never deletes a job (enforced in the Rust commands; app-level referential integrity, this
      DB leaves SQLite FK enforcement off). Nullable FK = standalone jobs (`pathway_id = NULL`, every
      job today) fully functional and unchanged. Commands: `create_reaction`/`list_reactions`/
      `rename_reaction`/`delete_reaction`, `create_pathway`/`list_pathways`/`delete_pathway`,
      `attach_job_to_pathway`/`detach_job_from_pathway`. Four cargo controls green (migration
      preservation; delete-keeps-jobs, bite-verified; referential integrity; standalone-unaffected).
      **No manual gate** (schema + Rust). See ADR-007 amendment + `modules/tauri-core.md`.
- [~] **C2a — reaction/pathway management UI.** `ReactionsScreen.tsx` (new "Reactions" tab): create a
      reaction, attach unattached completed **scan jobs** as labelled pathways, detach, rename, delete
      — a thin view over the C1 commands (no energy/coordinate/ΔΔE‡ logic). The picker marks/warns
      scan vs non-scan via `isScanJob` (attach stays permissive — C1). **Jobs-survive made visible:**
      every delete/detach only un-groups (Tauri-dialog `confirm` copy says so; a pathway's job title
      opens the still-standalone job). Exposes `jobs.pathway_id` on the `Job` model (14th column) so
      the UI maps pathway→job (one source of truth; survives reload). Extracted logic
      (`src/reactions/pathway.ts`) unit-tested: **C-scan-detection** (bite-verified) + **C-empty-label**.
      **Code + tests complete**: `tsc` 0, `vitest` 638 (+6), `vite build` clean, `cargo` 207 (Job
      column additive). **Manual gate (author) PENDING** — m1–m4 in the real window; C2a closes when it
      passes. See `modules/reactions-ui.md`.
- [ ] **C2b — promote + comparative view.** Overlay Pathway A vs B, **ΔΔE‡** (barrier difference)
      highlighted; reads coordinate/method/profile from the attached job (one source of truth); adds
      the comparability guards (same coordinate/method) that C2a's attach leaves advisory.
      **Manual gate.**
      *Ratified for this stage (2026-08-07, [ADR-018](wiki/architecture/adr-018-reaction-energy-reference.md)
      + [chemistry/reaction-barriers.md](wiki/chemistry/reaction-barriers.md)):* a relaxed scan yields
      **three barriers** — **ΔΔE‡** (si/re) is **reference-free** (the shared reactant reference cancels),
      **intrinsic** = E(max) − scan minimum (RC captured for free when the scan starts far enough), and
      **absolute vs separated reactants** = E(max) − Σ E(reactant jobs). So C2b **ships ΔΔE‡ + intrinsic
      barriers with the reactant reference OPTIONAL** (no reference → a note that absolute barriers need
      one). The reference is a **summed, optional list of reference-job references** — `reaction_reference_jobs`
      (reaction_id, job_id), **migration v14 when this data touch lands** — normalized and jobs-survive
      like C1 (not a single FK, not a cached scalar). The comparability guard extends to the reference
      jobs (shared method/basis/dispersion/solvation; SMD-not-ALPB for the ionic BH₄⁻). Full experimental
      ΔG‡ comparison (OptTS+Freq+thermochemistry, association entropy, standard state) is **Stage E+**.

**Stage C done when (= mission "done-when"):** author defines si vs re facial attack on a ketone,
runs two native scans, and sees two profiles side by side with ΔΔE‡ — a computational
stereoselectivity screen.

### Stage D — Conformer → reaction-center scientific-rigor layer

- [ ] Boltzmann-weight the GOAT ensemble (primitive from 2.5.1) + **re-optimise the lowest 3–4 at
      DFT** → build reaction centers on those (ADR-007: mandatory for valid science). Upgrades the
      inputs to Stages A–C so ΔΔE‡ is defensible, not merely mechanically produced. No new probe
      (GOAT measured); the DFT re-opt orchestration is the new code.

### Stage E — Transition-state methods (each probe-first, rule #10)

- [ ] **OptTS** (`! OptTS`) — probe (block, artifacts, cost) → then the scan-max → OptTS → Freq →
      one imaginary → **IRC** connectivity check → ΔG‡ pipeline (the *application* of these methods
      in the scan spine — the natural continuation of Stage B's "click the maximum").
- [ ] **IRC** (`! IRC`) — probe → the post-condition that a found TS connects the intended
      reactant/product.
- [ ] **NEB / NEB-TS / NEB-CI** (`! NEB-TS`) — probe → the alternative path-finder when there is no
      clean scan coordinate; needs a product geometry (ADR-010 `ReactionPath = fold(reactant,
      transform)`) + a per-iteration band viewer. Highest effort; may trail.

### Stage F — Microsolvation (explicit solvent shell), probe-first

- [ ] Install **CREST** (a separate binary that shells xtb — **domain rule #2:** its build must
      match the installed **xtb 6.6.1**) → **probe `crest --qcg`** (record artifacts, cost, what
      works) → then design. Builds on fragment placement + xtb + GOAT; the tail-2 cation catalog
      seeds the ion side. **Caveats to settle in the probe:** shell **conformer sampling** (floppy
      shell, many near-degenerate arrangements) and **quasi-RRHO** thermochemistry for the low
      frequencies a loose cluster introduces. Especially for the ionic/charged TS (Na⁺–BH₄–ketone;
      **SMD over ALPB for ions**). Lowest immediate mission priority — last, or in parallel on demand.

**Phase 4.5 done when:** author defines two stereofacial attacks on a ketone (si vs re), runs two
native ORCA scans, and sees two energy profiles side by side with ΔΔE‡ — a computational
screening of stereoselectivity (Stage C). Stages D–F deepen it toward publication-quality ΔG‡.

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
