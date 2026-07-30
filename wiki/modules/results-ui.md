# Module: Results UI (`src/trajectory/`, `src/spectrum/`, `src/charts/`)

**Status:** the post-calculation **visualization** tier of Phase 3 (unit 3.8) — optimization
**trajectory playback** and the broadened **IR spectrum**. Both stand entirely on already-parsed,
already-stored data (`data_json`, unit 3.7); no artifact is re-read. Mode **animation** (clicking a
peak to see the atoms move) is **not** here — it is unit 3.9, behind the Kabsch-alignment gate.

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
- **`IrSpectrumPanel.tsx`**: the verdict banner (minimum / TS / neither, from `imaginary_count` — the
  teaching moment), the **imaginary modes listed separately** as a transition-state **diagnosis**
  (not dropped, not broadened), the Lorentzian curve (recharts `ComposedChart`) with a **FWHM
  slider** and the grid printed as numbers (both are plot choices, stated as such), and the frequency
  table. **Peak ↔ row:** clicking a peak marker selects its frequency-table row and draws a vertical
  marker; clicking a row selects the peak — one shared `selected` mode index (the original index into
  `frequencies_cm`). **No mode animation** — unit 3.9, behind the Kabsch gate.

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

## See also
- [ADR-011](../architecture/adr-011-editor-graphics-stack.md) — 3Dmol is a dumb renderer until the
  spike passes; the reason the frame number may not live in the viewer.
- [visualization.md](visualization.md) — the `MoleculeViewer` this feeds (and its
  `preserveCameraOnUpdate` note).
- [artifact-readers.md](artifact-readers.md) / [`orca/parse-sources.md`](../orca/parse-sources.md) —
  the parsed `data_json` this stands on, and the `orca_mapspc` cross-check.
