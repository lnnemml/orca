# Module: Results UI (`src/trajectory/`, `src/spectrum/`, `src/charts/`)

**Status:** the post-calculation **visualization** tier of Phase 3 — optimization **trajectory
playback**, the broadened **IR spectrum**, and **normal-mode animation** (click a peak → watch the
atoms move; unit 3.12, after its Kabsch determiner cleared the `.hess` frame). All stand entirely on
already-parsed, already-stored data (`data_json`, unit 3.7); no artifact is re-read.

## Where the current frame lives (the load-bearing decision)

This is the first place the viewer receives a **sequence** of geometries rather than one. 3Dmol has
its own frame apparatus (`addModelsAsFrames` / `setFrame` / `animate`, and `setCoordinates` loads a
whole `T×N×3` trajectory to drive it). **We do not use any of it.**

- **The current frame number is application state** — React state in
  `TrajectoryPlayer`, never 3Dmol's. The viewer is a **dumb renderer** (ADR-011): it is handed
  **one frame's** xyz and draws it, exactly as it draws any single structure.
- **The play timer lives in `TrajectoryPlayer`** (a `setInterval` in a `useEffect`), so the frame
  number stays in lock-step with the cycle label, the energy readout, and the highlighted point on
  the E(cycle) chart. A timer buried in the viewer could be synchronised with none of them.
- **Why it matters beyond tidiness:** Phase 4.2 swaps the renderer for `orcastudio-render` once the
  [ADR-011](../architecture/adr-011-editor-graphics-stack.md) spike passes. Anything that had
  migrated into 3Dmol's frame/animation state would have to be dug back out. Nothing does.
- This is the **same ownership call the geometry editor makes for `selection`** — view-local,
  ephemeral UI state is held by the screen/component, not by a store and not by the renderer (see
  [scene.md](scene.md) → Atom selection). The frame number is component state in `TrajectoryPlayer`,
  not the scene store.

## `src/trajectory/` — playback

- **`frame.ts`** (pure, node-tested, React-free): `frameToXyz(elements, frame)` builds the one
  frame's standard xyz (and **throws** on an element/coordinate count mismatch — the UI-boundary
  echo of the readers' count invariant, never a silent render); `frameLabel` (an **honest** label —
  `cycle N / total`, never "scan step": frames are optimization cycles, measured — a 6-point scan
  has 26); `frameEnergyText` / `frameDeltaKcal` (the comment energy in Eh and ΔE-from-cycle-1 in
  kcal/mol — the "how far it fell" number); `energySeries` (chart data, only frames that carry an
  energy); `elementsAgree` (the identity check below).
- **`TrajectoryPlayer.tsx`**: owns `frame` / `playing` / `fps`; transport (first / prev / play-pause
  / next / last), a slider, and a speed select (0.5×–4× = 2–20 fps — a UI choice, the app-layer timer
  ticks at that rate). It renders **one** `MoleculeViewer` fed `frameToXyz(current)` with
  `preserveCameraOnUpdate`, the cycle/energy/ΔE readout, and the **E(cycle) chart** (recharts,
  click a point to jump; the current cycle is a vertical `ReferenceLine`). The energy chart is the
  learning core — you watch the curve descend and can step to any cycle.

### Identity check at the UI boundary
Before anything is drawn, `elementsAgree(trajectory.elements, referenceElements)` compares the
trajectory's atom order against the order the summary card is drawn in (`final_geometry.elements`).
A mismatch renders an **error**, not a silently-wrong animation — the same discipline the readers
enforce on their artifacts (ADR-010/012 seam), applied one layer out.

### Empty states
- **1 frame** → nothing to play: the static geometry is shown with **no controls**.
- **no trajectory** (a single-point job — `results.trajectory` is `null`) → the section is **not
  rendered** at all (`ResultsCard`).

### The viewer's coordinate-update path (`MoleculeViewer` `preserveCameraOnUpdate`)
A new opt-in prop: when set, an `xyzData` change that keeps the **same atom count** redraws the
frame **without** re-`zoomTo` (the camera stays put through playback); a count change still zooms.
Default false → the Molecules/preview path is byte-for-byte unchanged. **We rebuild the single-frame
model each tick** (`removeAllModels`/`addModel`) rather than mutate coordinates in place, because the
only in-place path 3Dmol offers is its trajectory/`animate` apparatus, which would move frame
ownership into the viewer (ADR-011). At the sizes that occur this is not a bottleneck: the app-side
per-frame work (`frameToXyz`) is ~3.5 µs for 8 atoms / ~13 µs for 50 (measured, Node) — negligible
against a 50 ms (20 fps) tick, so playback is **timer-bound**, not rebuild-bound. The real in-webview
`addModel`/`render` time was not headlessly measured (the standing Tauri-GUI-drive limitation), but
for ≤ 50 atoms it is far under the tick.

## `src/spectrum/` — IR spectrum

- **`ir.ts`** (pure, node-tested): the whole spectrum math.
  - **`classifyModes`** splits the stored stick list by **measured fact, not a threshold**:
    **exact-zero** (`=== 0`) translation/rotation modes are counted and excluded; **negative**
    (imaginary) modes are excluded from the curve **by sign** but kept and returned separately; the
    rest (`cm > 0`) are the real vibrations that get broadened. A tiny positive stays a vibration; a
    tiny negative is imaginary — the sign decides, never the magnitude.
  - **`lorentzian`** is **area-normalized** (∫ = 1), written so it reads: half-width `g = FWHM/2`,
    peak `2/(π·FWHM)`, half-max at `x₀ ± FWHM/2`. So `intensity · L` integrates to the mode's km/mol
    intensity — **the area under a peak is physically meaningful** (a test locks `∫ ≈ intensity`).
  - **`autoGrid`** names the x-axis explicitly: range = mode span padded by 8·FWHM, clamped at 0;
    step = FWHM/8 floored at 0.5 cm⁻¹ — not "by eye".
- **`irPresentation.ts`** (pure, node-tested): the **presentation** layer on top of `ir.ts`'s
  physics — everything here is a *drawing* choice, never a molecular property. `scaledModes` applies
  the display scale factor (identity at 1.0) by transforming the mode list fed to `ir.ts`, so the
  physics module is untouched and curve + sticks share one x-axis; `nearestMode` / `irTooltipModel`
  build the single-source tooltip state (see below). Kept separate from `ir.ts` so the "physics" and
  the "how we draw it" layers don't blur.
- **`IrSpectrumPanel.tsx`**: the verdict banner (minimum / TS / neither, from `imaginary_count` — the
  teaching moment), the **imaginary modes listed separately** as a transition-state **diagnosis**
  (not dropped, not broadened), the chart, the plot-choice controls, and the frequency table.

  **Sticks vs curve — two honest representations, two labelled axes (unit 3.10).** The spectrum *is*
  a set of lines, so the primary object is **sticks**: a vertical line per mode at its wavenumber,
  height = its IR intensity in **km/mol** (right axis, drawn by the `IrSticks` component using v3's
  `useXAxisScale` + `usePlotArea` — positioned from the plot area and the explicit km/mol max, so it
  does not depend on the data-less right axis registering an internal scale). The **broadened curve**
  (km/mol·cm⁻¹, area-normalized — a *density*) sits on top on the **left** axis. They are genuinely
  different quantities (integrated intensity vs density), so they get **two labelled axes** — a single
  axis would need an arbitrary FWHM/lineshape-dependent conversion factor, a made-up parameter (rule
  #11). The earlier on-curve Scatter markers are **gone** (they were neither sticks nor curve, and
  they were the second series that broke the tooltip).

  **Single-source tooltip (unit 3.10 bug fix).** The old tooltip merged two recharts series and took
  the label from one x and a value from another (header `115 cm⁻¹` beside a `9.350` that was the O–H
  peak height at 3714). Fixed structurally: sticks are **not a chart series** (they are drawn SVG), so
  the Tooltip has only the curve Line to read — nothing to merge. A custom `content` derives
  everything from the one hovered wavenumber via `irTooltipModel(label, curveValueThere, modes)`: the
  curve value at that x, and the **nearest mode** labelled *as nearest* with its Δ, never as "the value
  here". `irPresentation.test.ts` locks the one-x property (reproduces the 115-vs-3714 scenario).

  **The x-grid is built from the RAW frequencies + the slider's full range, never the current scale
  (`fixedGrid`, unit 3.11).** The scale slider exists to slide the peaks against a *stationary* ruler
  and compare with experiment. Deriving the grid from the already-scaled modes (the original code)
  multiplied both the data and the axis by the same factor — a self-similar picture where the peaks
  never moved in pixels, only the tick labels changed, so the parameter was useless. `fixedGrid` hands
  `autoGrid` two synthetic extremes — lowest raw mode × min-scale, highest × max-scale — so the frame
  covers every reachable peak position and stays put while the slider moves (invariant, tested: move
  the scale → axis bounds unchanged, peak position changed). The step is FWHM-only, so it too is
  scale-stable. Everything that *should* move with scale still does — the sticks, the curve, the
  selected-mode marker, and the tooltip's nearest-mode (all computed from `scaledModes`, i.e. in the
  same drawn space as the axis).

  **Three labelled plot choices**, each a UI control, none a molecular property: the **FWHM** slider;
  the **display scale factor** slider (default **1.00**, range 0.9–1.1 — NOT baked in per method, NOT
  read from the artifact's `$frequency_scale_factor`; when ≠ 1 the table shows raw **and** scaled
  columns, the scaled marked *derived*, the curve/sticks move to the scaled positions **against the
  fixed grid above**); and the
  **inverted view** toggle (peaks up / peaks down). The inverted view **reverses both Y axes** (data
  unchanged — the honest inversion), the x-axis stays increasing, and it is labelled a *conventional
  depiction, explicitly NOT transmittance* — %T needs the Beer–Lambert law (path length,
  concentration) a calculation does not contain, so no `%T` axis and no invented parameters appear.

  **Axis units on screen (rule #11 on the display):** left `km/mol·cm⁻¹` (broadened density), right
  `km/mol` (stick intensity), x `cm⁻¹`. The artifact's own `$frequency_scale_factor` is surfaced with
  its value and an explanation that 1.0 means ORCA applied none — distinct from the display scale.

  **Peak ↔ row ↔ animation:** clicking a stick selects its frequency-table row (and draws a dashed
  marker at the scaled wavenumber); clicking a row selects the stick; clicking an imaginary-mode chip
  selects it too — one shared `selected` mode index (the original index into `frequencies_cm`), which
  is also the **column** of `$normal_modes`. Selecting a mode reveals the animation (below). Note:
  `$actual_temperature` (measured 0.0) is **never** used as a temperature here or anywhere; the card's
  entropy uses `ThermoJson::temperature_k` (see [`orca/parse-sources.md`](../orca/parse-sources.md)).

### Normal-mode animation (unit 3.12) — `src/spectrum/mode.ts` + `ModeAnimator.tsx`
Gated behind the **unit-3.12 Kabsch determiner** (`probes/hess_frame_kabsch.py`): the `.hess $atoms`
frame is a **pure translation** of the reference geometry on all three jobs (`max|R−I| ≤ 3e-13`, incl.
asymmetric dexketoprofen), so `$normal_modes` are added to the reference geometry **as-is** — no mode
rotation. Had the gate found a rotation, the animation would have been smooth, symmetric and wrong;
this is why it is a gate, not an assumption.

- **`mode.ts`** (pure, node-tested): `modeDisplacements` extracts mode `k` as the **column** of the
  row-major 3N×3N matrix (a row would be a different thing — the seam, locked by a test);
  `modeFrameCoords` is `x_eq + A·sin(2π·phase)·v` (phase 0 = equilibrium exactly); `modeFrameXyz`
  reuses the trajectory formatter (one code path to the dumb renderer); `modeMinDistanceOverPeriod`
  is the collapse guard.
- **Ownership is the trajectory's, verbatim (ADR-011).** The **phase, amplitude, play timer and
  speed are application state** in `ModeAnimator`; the viewer is handed **one frame's** geometry, with
  no timer, no frame list, and no 3Dmol `animate`/`setFrame`. The timer loops the period forever
  (unlike the trajectory's play-once). Same call as the trajectory frame number and the editor
  `selection` — view-local state held by the component, not the store or the renderer.
- **Amplitude is a display choice**, defaulting to the measured `orca_pltvib` multiplier **2.0**
  (labelled as such — the mode is normalized and has no absolute amplitude), a slider like the FWHM and
  the display scale. A **collapse guard** (rule #9) warns when the current amplitude drives atoms closer
  than **0.5 Å** (`MIN_SAFE_DISTANCE_ANGSTROM`) — measured: 2.0 suits bends (median min ≈0.95 Å) but
  overshoots localized C–H stretches (0.02–0.07 Å), so the guard tells the user to reduce it rather than
  drawing mush.
- **Identity check at the UI boundary**, like the trajectory: `elementsAgree(f.elements,
  geometry.elements)` before animating — a mismatch renders an error, not the wrong atoms moving.
- **Imaginary modes are animatable, and it is the teaching payoff:** for a transition state the imaginary
  mode traces the **reaction coordinate** (the motion downhill in both directions), labelled as such in
  the animator — never filtered out as noise.
- **Empty states:** no `.hess` (SP/GOAT) → the whole panel is absent; frequencies but no `$normal_modes`
  (or a bad shape) → no animator, the frequency table still stands.

### Cross-checked against ORCA's own tool
The broadening was verified against `/opt/orca/orca_mapspc` (domain rules #9/#10) — flags taken from
its `-h`, result recorded in [`orca/parse-sources.md`](../orca/parse-sources.md). The core lineshape
and FWHM match; the documented difference (orca truncates the Lorentzian wings, we keep them so the
area = intensity property holds) is reported as a number, not fudged away.

## `src/charts/useContainerWidth.ts`
The one owner of the **WebKitGTK 0×0 `ResponsiveContainer`** workaround (`debugging/002`/`003` class):
a `ResizeObserver` that feeds an explicit pixel width to every chart. Extracted from
`ConvergenceDashboard` (which now imports it) so the convergence, trajectory-energy, and IR charts
share one copy.

## The Job-detail screen: one layout, and where the header energy comes from (unit 3.9)

**One scrolling layout, not two.** The Job-detail screen (`JobDetailScreen`) scrolls as a normal
column (`.screen.detail { overflow-y: auto }`); the live log console and the Browse-mode Monaco viewer
get a **fixed** height (`60vh`), not `flex: 1`. Before unit 3.9 the screen was `overflow: hidden` with
a flex-filling console — correct in Phase 1 when the console was the only content, but it clipped the
Phase-3 results card (trajectory, IR spectrum, verdict) that now sits above it, so half of Phase 3 was
rendered and unreachable. The fix is deliberately **not** a status-conditional layout (running →
full-height console, parsed → scroll): that is two layouts, each only ever tested by hand. See
[debugging/007](../debugging/007-phase1-decisions-phase3-outgrew.md).

**The header energy is authoritative, not an estimate.** The energy in the job header/list
(`jobs.energy`) is filled from `results.final_energy_eh` (parsed `.property.txt`, ADR-012) once a job
reaches `parsed` — **not** from the `output.out` tail regex, which misses the final energy on a large
molecule (measured: 164 KB past the 64 KB window on the 33-atom dexketoprofen Freq). The regex stays a
**live estimate during a run**; the parsed value replaces it. Old jobs are backfilled once by
migration v7→v8. A post-condition (`results::cycle_energy_cross_check`) compares the two independent
optimization-cycle energy sources (`.out` convergence vs `_trj.xyz` frames) after every non-GOAT run,
so a silent drift is a recorded diagnostic. Details in
[debugging/007](../debugging/007-phase1-decisions-phase3-outgrew.md).

## See also
- [ADR-011](../architecture/adr-011-editor-graphics-stack.md) — 3Dmol is a dumb renderer until the
  spike passes; the reason the frame number may not live in the viewer.
- [visualization.md](visualization.md) — the `MoleculeViewer` this feeds (and its
  `preserveCameraOnUpdate` note).
- [artifact-readers.md](artifact-readers.md) / [`orca/parse-sources.md`](../orca/parse-sources.md) —
  the parsed `data_json` this stands on, and the `orca_mapspc` cross-check.
