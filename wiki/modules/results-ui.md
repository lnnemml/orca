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
  learning core — you watch the curve descend and can step to any cycle. **The click-to-jump goes
  through the shared `resolveClickedIndex` resolver** (see below) — it had been silently broken since
  the recharts→v3 upgrade (`debugging/016`) and was fixed here in the same change as the scan chart.

### Identity check at the UI boundary
Before anything is drawn, `elementsAgree(trajectory.elements, referenceElements)` compares the
trajectory's atom order against the order the summary card is drawn in (`final_geometry.elements`).
A mismatch renders an **error**, not a silently-wrong animation — the same discipline the readers
enforce on their artifacts (ADR-010/012 seam), applied one layer out.

### Empty states
- **1 frame** → nothing to play: the static geometry is shown with **no controls**.
- **no trajectory** (a single-point job — `results.trajectory` is `null`) → the section is **not
  rendered** at all (`ResultsCard`).

### Geometry measurement readout (distance / angle / dihedral, F1 + F1c)
Clicking atoms in the viewer builds a **pick list** (`pickAtom`; a click on a picked atom removes it,
a 5th pick drops the oldest — **cap 4**). The picks are passed to the viewer as **`xyzSelection={picked}`**
on **both** mounts, and the measurement now renders **ON the molecule** (F1c) — selection halos + the
distance line / angle arc / dihedral axis + the value label — exactly like the geometry editor, drawn by
the viewer's overlay effect off the **currently displayed frame's coords** via `measureByCoords`
([visualization.md](visualization.md) → overlay effect). Positional: **2 → distance** (3 dp Å),
**3 → angle** (middle pick = vertex, 1 dp °), **4 → dihedral** (chain, `[0,360)`, 1 dp °). Because the
overlay has `xyzData` in its deps, it **redraws per frame** — the halos + line + label follow the atoms
and the value updates as you scrub/play (a forming bond's distance changes live).

The **bottom text line** is now just the annotation the in-scene overlay can't show: the picked-atom
**chips** (`{elem}#{k}`), a **Clear** button, and — for a **2-atom** pick only — the **bond-order** line
(`resultsBondLabel` — Mayer authoritative / geometric estimate, `bondReadout.ts`), which has no in-scene
form. 0/1 picks show a quiet hint. (Editor parity: the editor shows both an in-scene overlay AND a text
panel; here the in-scene overlay is primary and the only text is the bond-order note.)

### The viewer's coordinate-update path (`MoleculeViewer` `preserveCameraOnUpdate`)
A new opt-in prop: when set, an `xyzData` change that keeps the **same atom count** redraws the
frame **without** re-`zoomTo` (the camera stays put through playback); a count change still zooms.
Default false → the Molecules/preview path is byte-for-byte unchanged. **The trajectory rebuilds the
single-frame model each tick** (`removeAllModels`/`addModel`) — on purpose, because along an
optimization/reaction path bonds genuinely change and should be re-perceived. (Unit 3.14 showed the
in-place alternative — mutate `x/y/z` + `setStyle` to rebuild geometry — does NOT need 3Dmol's
`setFrame`/`animate` apparatus, correcting an earlier note; the **mode animation** uses exactly that
in-place path to freeze topology, while the trajectory keeps rebuilding by design.) At the sizes that
occur neither is a bottleneck: the app-side
per-frame work (`frameToXyz`) is ~3.5 µs for 8 atoms / ~13 µs for 50 (measured, Node) — negligible
against a 50 ms (20 fps) tick, so playback is **timer-bound**, not rebuild-bound. The real in-webview
`addModel`/`render` time was not headlessly measured (the standing Tauri-GUI-drive limitation), but
for ≤ 50 atoms it is far under the tick.

## `src/scan/` — relaxed-scan energy profile (Phase 4.5 B2)

The first time a scan is **visible**. Reuses the trajectory's three disciplines verbatim; it reads
B1's `ParsedResults.scan` and **re-parses nothing** (ADR-012).

- **`scanProfile.ts`** (pure, node-tested, React-free): `profileSeries(points, energyChoice, refChoice)`
  → the chart data as **ΔE in kcal/mol** (a barrier is a *relative* quantity; raw Eh is unreadable),
  with the reference point exactly 0 by construction; `referenceEh` / `maxIndex` (the maximum of the
  shown series — the approximate-TS point); `pointGeometryXyz(geometry, referenceElements)` — the
  UI-boundary **element-order identity check** (`elementsAgree`, reused from `trajectory/frame.ts`)
  before a point renders, a loud refusal on mismatch, never a wrong molecule; `pointReadout`.
- **`ScanProfilePanel.tsx`**: owns the **selected point index** (React state — the viewer never owns
  it, ADR-011), the `act`/`scf` energy choice, and the `first`/`min` reference choice. Renders **one**
  `MoleculeViewer` fed the selected point's `input.NNN.xyz` geometry (fetched once via
  `read_scan_geometries`, `xyz.rs` `first_frame` witness). recharts with explicit `useContainerWidth`
  (no `ResponsiveContainer` — the WebKitGTK 0×0 class); click a point → set the app index.
- **Chart-click resolution is shared and tested — `src/charts/clickIndex.ts` (`resolveClickedIndex`).**
  Both this panel and `TrajectoryPlayer` route their recharts `onClick` through it: recharts **v3**
  delivers `activeTooltipIndex` as a **string** (`TooltipIndex = string | null`), so the old inline
  `typeof i === "number"` guard silently dropped every click (`debugging/016`). The resolver takes a
  number/string index, falls back to matching `activeLabel` (the x value) via a `getX` accessor
  (`coordinate` here, `cycle` for trajectory), returns the array position (caller maps to
  `series[pos].index`), and never throws. Each handler DEV-warns on an unresolved click; a redundant
  **function-form** `activeDot` `<circle onClick>` selects on a direct dot hit (the object form does
  not receive the datum — measured). Four pure controls in `clickIndex.test.ts` (v3 string, v2 number,
  label fallback, garbage) close the inline-glue test gap.
- **Honest labelling (the teaching-moment discipline, like the IR "conventional depiction" and the
  `imaginary_count` verdict).** x = scanned coordinate (Å for `B`, ° for `A`/`D`); y = ΔE kcal/mol
  against a **labelled** reference; both `act` (composite, default) and `scf` are offered and labelled.
  The maximum is marked and labelled **"approximate TS (scan maximum)"** — a ΔE‡ *estimate* on a
  relaxed surface (ADR-007 §"ΔE‡ vs ΔG‡"), never "the transition state" and never ΔG‡; a note points
  forward to OptTS (Stage E). A `< 2`-point scan is a clear empty state, not a crash; a non-scan job
  hides the panel (`results.scan` is null).
- **Per-point geometry export — WYSIWYG, and the Stage-E seam.** The panel **opens on the
  approximate-TS maximum** (`selected` initialises to `maxIndex(points, "act")`, not point 1 — an
  intended B2 behaviour change: the panel's Stage-E purpose is "the maximum → refine"). A
  `geometry .xyz` button in the readout row exports **the point the viewer is showing** — the same
  `geometries[clamped]` from `read_scan_geometries` the viewer is fed, **never `results.final_geometry`**
  (which is the *last* point — seeding OptTS from it was the bug: `debugging/018`). It reuses the one
  canonical `finalGeometryXyz` via `scanPointExportXyz` (no second xyz builder; the atoms+2
  post-condition, rule #9, is inherited). **Honest-or-absent:** enabled ONLY when the point actually
  renders (`viewerState && "xyz" in viewerState`, i.e. element order agrees) — disabled otherwise.
  The comment always carries `energy_act_eh` (the composite total, the plotted energy) regardless of
  the `scf` toggle, and the `(approx TS / scan maximum)` tag appears only when the selected point IS
  the maximum. **This selected-max extraction is the Stage-E seam** — the OptTS-refine child job (E1)
  reuses exactly this geometry as its seed. Correspondingly the **top ExportBar geometry button is
  relabelled `geometry .xyz (last point)` for a scan job** (`results.scan != null`) and its comment
  says "last scan point", so the last point is offered honestly and is not confused with the
  approx-TS selection; a non-scan job is unchanged (`geometry .xyz`, the optimized final geometry).
- **Refine with OptTS — the scan entry point into the source-agnostic refine engine (Stage E1a,
  ADR-020).** Next to the geometry-export button, a `Refine with OptTS (Stage E)` button is enabled
  **only on the approx-TS maximum AND when it renders** (`clamped === tsIndex && viewerState && "xyz"
  in viewerState`) — the scan maximum is the TS *guess* (a clean 1-D coordinate; `wiki/orca/optts.md`).
  On click it reads **this scan job's own input** (`get_job`, not reconstructed) as the method/
  solvation/charge context, seeds from the max point's geometry, calls `buildOptTSInput`
  (`src/scene/optts.ts` — the generic engine, NOT scan-specific), `create_optts_job(sourceJobId=jobId,
  …)`, `submit_job`, then navigates to the child via `onOpenJob` (prop-drilled App → JobDetailScreen →
  ResultsCard → panel). A `buildOptTSInput` post-condition failure (wrong charge / Scan leak) surfaces
  in a banner and **creates no job**. The engine is deliberately source-agnostic: E3's NEB entry point
  will call the SAME `buildOptTSInput` / `create_optts_job` with a climbing-image seed.
  A `<GroupSelect>` beside the button (via `useGroupPicker`) sets the child's destination group,
  **defaulting to this scan job's group** (`useJobGroupId(jobId)`, NOT the active sidebar group;
  `assignPicked(child.id)` before submit — unit 2b, [groups-ui.md](groups-ui.md#the-derived-spawn-picker-unit-2b)).
- **The point geometries are a witness read (`xyz.rs` `first_frame`), not authoritative output** — a
  relaxed-scan point is not the input geometry, so it does not go through the reference-based geometry
  post-condition (which fails by design); its identity is the UI-boundary `elementsAgree` check, the
  same one the trajectory does. The `read_scan_geometries` command **writes nothing** to the job dir
  (rule #3) and reads the small point files whole (rule #5).
- **A scan job parses profile-only (B1 fix).** The same "a scan point is not the input geometry"
  fact holds one level up, at `parse_and_store`: a scan `.property.txt` is **multi-point** (its first
  `$Geometry` is scan point 1, not the input), so it does not fit the single-structure readers. A
  completed scan is routed (by `input.relaxscanact.dat`) to **profile-only** — parse the profile,
  skip property/`_trj`/hess/mo; `final_energy_eh` and `final_geometry` come from the profile's **last
  point**. That is what makes `results.scan` non-null (this panel visible) and gives it
  `referenceElements`. See `wiki/debugging/015-scan-property-post-condition.md`.

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

### Normal-mode animation (unit 3.12; made chemically honest in 3.13) — `src/spectrum/mode.ts` + `ModeAnimator.tsx`
Gated behind the **unit-3.12 Kabsch determiner** (`probes/hess_frame_kabsch.py`): the `.hess $atoms`
frame is a **pure translation** of the reference geometry on all three jobs (`max|R−I| ≤ 3e-13`, incl.
asymmetric dexketoprofen), so `$normal_modes` are added to the reference geometry **as-is** — no mode
rotation. Had the gate found a rotation, the animation would have been smooth, symmetric and wrong;
this is why it is a gate, not an assumption.

- **`mode.ts`** (pure, node-tested): `modeDisplacements` extracts mode `k` as the **column** of the
  row-major 3N×3N matrix (a row would be a different thing — the seam, locked by a test);
  `modeFrameCoords` is `x_eq + A·sin(2π·phase)·v̂` (phase 0 = equilibrium exactly); `modeFrameXyz`
  reuses the trajectory formatter (one code path to the dumb renderer); `modeMinDistanceOverPeriod`
  is the collapse guard; `zeroPointAmplitudeAngstrom` is the physical amplitude for the label.
- **Ownership is the trajectory's, verbatim (ADR-011).** The **phase, amplitude, play timer and
  speed are application state** in `ModeAnimator`; the viewer is handed **one frame's** geometry, with
  no timer, no frame list, and no 3Dmol `animate`/`setFrame`. The timer loops the period forever
  (unlike the trajectory's play-once). Same call as the trajectory frame number and the editor
  `selection` — view-local state held by the component, not the store or the renderer.
- **Amplitude = the MAXIMUM ATOMIC DISPLACEMENT, in Å (unit 3.13).** The mode is unit-normalized over
  all **3N** components (measured `Σ|v|²=1`), so a bare `A·v` gives the busiest atom of a *localized*
  mode a huge move and of a *delocalized* mode a crumb — the earlier "fine for bends, collapsing for
  stretches" (the C=O #84 bond reached **0.63 Å**). The fix normalizes by `max_j|v_j|` — the largest
  **atomic tri-vector norm**, NOT the largest component (off by up to √3) — so the busiest atom always
  moves exactly `A` and every mode is comparably visible. Default **0.18 Å** (measured: the largest round
  value keeping the worst localized stretch's bonds ≥ 0.9 Å; real thermal amplitudes are ~0.04–0.07 Å,
  we exaggerate for visibility). The old `orca_pltvib` 2.0 is **not** the default — it is a *norm*
  multiplier, a different quantity (`parse-sources.md`). The label shows the mode's real zero-point
  amplitude `√(ħ/2μω)` (from verified masses — see below), naming the exaggeration. The **collapse
  guard** (`< 0.5 Å`) stays as the last line for a large hand-set A, no longer the main mechanism.
- **Masses are derived from the element symbol** via a standard-weight table **verified equal** to the
  `.hess $atoms` mass column (C 12.011 / H 1.008 / O 15.999, `probes/mode_amplitude.py`) — so the reader
  and stored data are untouched (rule #10). No mass → no physical amplitude shown (never guessed).
- **Bond topology is FROZEN at equilibrium — by building ONCE and updating coordinates (unit 3.13/3.14).**
  A vibration is the same molecule; its bond graph is a function of the **equilibrium** geometry only. But
  3Dmol perceives bonds from each frame's distances, so an animated stretch made bonds flicker. `MoleculeViewer`
  takes a `bondTopologyReference` (the equilibrium xyz): the model is built **once** from it — a normal
  parse, so 3Dmol perceives bonds *and* assigns `atom.index`, the sole perception (ADR-010) — and each frame
  then only **updates the atoms' coordinates** in place (`applyCoordsToAtoms` over `selectedAtoms({})`) +
  `setStyle` (which nulls the cached geometry so `render` rebuilds sticks at the new positions from the same
  bonds). Topology is frozen by construction; the **app decides** it by choosing the reference, the viewer
  draws (ADR-011). This corrects the unit-3.8 belief that in-place coordinate updates need 3Dmol's
  `setFrame`/`animate` apparatus — they do not, and frame ownership stays in the app.
  - **Why NOT the unit-3.13 first attempt** (`assignBonds:false` + hand-set bonds each frame): it drew
    **nothing**. `assignBonds:false` leaves `atom.index` unset and 3Dmol's stick gate is `atom.index <
    atom2.index`, so `undefined < undefined` dropped every cylinder — see [debugging/008](../debugging/008-frozen-bonds-drew-nothing.md).
  - **The test now checks the OUTPUT, not the input.** The 3.13 test asserted our *bonded set* was stable
    across phases — which passes even on a blank render (input, not output). `drawableBondCount`
    (`frozenTopology.ts`) mirrors 3Dmol's draw gate: **> 0** for a normal parse, **0** when `index` is unset
    (the regression, reproduced), constant across coordinate updates. 3Dmol needs WebGL (no jsdom), so a
    **DEV assertion in the viewer** warns in the real webview if a built frozen model has 0 drawable bonds.
- **The trajectory has the SAME per-frame perception — and is deliberately LEFT that way.** Along an
  optimization/reaction path bonds can genuinely form and break; freezing them would hide real chemistry.
  So `TrajectoryPlayer` passes no `bondTopologyReference`. Different question, different answer (reported,
  not fixed — this is the one place per-frame perception is correct).
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

### The frequency table flows into three columns (task, unit 3.15)
93 modes in one column is a scroll, so `FrequencyTable` flows them into **three** side-by-side
sub-tables (`FREQ_TABLE_COLUMNS`), the running `#` continuous down each column. Selection, the
peak↔row highlight, and the imaginary/scaled logic are unchanged — each row still calls the shared
`onSelect(m.index)`. On a narrow window the flex row (`.ir-table-columns`) **wraps**, it doesn't
overflow. Cosmetic only.

## `src/orbitals/` — orbital isosurfaces (unit 3.15)
The last Phase-3 visualization. The MO energies + occupancies were already parsed (`orca_2json`,
`data_json.orbitals`); this adds the orbitals as **volume**. Gated behind a measured three-part gate
([`orca/orca-plot.md`](../orca/orca-plot.md)): `orca_plot` batch invocation, cube sizes/times, and —
the real unknown — whether WebKitGTK renders a 3Dmol isosurface (it does; MiniBrowser screenshot).

- **`orbitalList.ts`** (pure, node-tested): `orbitalRows` marks exactly one **HOMO** (highest
  occupied) and one **LUMO** (first virtual) — the teaching pair — and `defaultOrbital` opens on the
  HOMO. Occupied = occupancy > 0.5 (measured MOs are 2.0/0.0, not a knife-edge).
- **`OrbitalPanel.tsx`**: the picker (MO number, energy Eh + eV, occupancy, HOMO/LUMO tagged) beside
  the viewer, an **isovalue slider**, and the +/− phase legend. On selecting an MO it `invoke`s
  `read_orbital_cube` (Rust generates the cube lazily via `orca_plot`, caches it in the job dir keyed
  by MO+grid, reads it once capped at 32 MB — the cube is a **disk artifact, never in the DB**).
- **Isovalue is a DISPLAY choice** (like the FWHM and the mode amplitude): a slider, default **0.05**
  named. The two surface colours are the wavefunction's two **phases (sign of ψ), NOT charge** — said
  in the label. Grid is fixed at the measured moderate **80³** (~6.9 MB, ~0.2 Å; rule #5 verified by
  number, `orca-plot.md`).
- **State ownership (ADR-011):** the selected orbital, the isovalue and visibility are app state in
  `OrbitalPanel`; the viewer is handed the cube text + isovalue and draws. `MoleculeViewer` grew an
  **`orbitalCube` / `orbitalIsoValue`** path: the molecule is built once from the cube's atoms (model
  effect), and the ± isosurfaces are drawn by a **dedicated effect** that parses the cube into a
  `VolumeData` **once** (cached by text) and, on an isovalue change, `removeShape`s exactly its two
  surfaces and re-adds them — no re-parse, and the scene-editor overlay effect is guarded to leave
  those shapes alone. **One scene, one mode:** the orbital viewer shows a static molecule + surface;
  it never also animates a normal mode (that lives in the IR panel).
- **Absence is normal:** an xTB/GOAT `.gbw` yields no JSON MOs (measured), so `results.orbitals` is
  absent and the whole section is not rendered; if `orca_plot` produces nothing, the viewer shows a
  note, never crashes.
- **Core-orbital marking is DERIVED, not read (unit 3.16).** `orbitalList.ts` marks the deep 1s-type
  **core** orbitals (the empty-looking, occluded ones — MO 0 confused the author) from a per-element
  table (H/He→0, Li–Ne→1, Na–Ar→5; anything else → no mark) **cross-checked** against the big
  core→valence energy gap: the count is placed only if the table's number equals the position of the
  largest low-energy gap, else no mark and the disagreement is reported. It is **NOT** "one 1s per
  heavy atom" (true only for the 2nd period), and the UI names it derived. Measured on dexketoprofen:
  19 core (16 C + 3 O), the −10→−1 Eh gap confirms it.
- **Representation toggle (unit 3.16, `RepresentationToggle`).** Ball-and-stick / **lines**, two
  representations only — lines exist to expose a core 1s isosurface that hides inside an atom's drawn
  sphere. App state in the panel (ADR-011); `MoleculeViewer` honours `representation` on the orbital,
  mode-animation and single-xyz paths (the scene editor is always ball-and-stick).

## Export (unit 3.16) — `src/export/`
Everything is built from the **already-parsed** `results` (no re-parse; ADR-012) and saved to a
**user-chosen** location via the native dialog (`@tauri-apps/plugin-dialog`), **never the job dir**
(rule #3 — the default is elsewhere AND the Rust write commands `write_export_text`/`write_export_bytes`
refuse any path under the app data dir). Default filename = `{job}-{what}.{ext}`.

- **`exporters.ts`** (pure, node-tested): `finalGeometryXyz` (Å, stored order, comment = job + energy,
  **post-condition** lines == atoms + 2 or throw); `frequenciesCsv` (active modes; a `scaled ×N
  (derived)` column only when the panel's display scale ≠ 1); `chargesCsv`; `orbitalsCsv` (Eh + eV);
  `thermochemistryCsv`. **Units are in every header** (rule #11 in the file), numbers keep full stored
  precision, and `entropyS` is exported as **T·S in Eh**, never "entropy" — with a separate, explicitly
  *derived* entropy-S row in J/(mol·K).
- **PNG** (gated — both paths measured to work under WebKitGTK, [debugging/009](../debugging/009-webkitgtk-png-export.md)):
  the spectrum and energy-per-cycle charts via `svgToPngBytes` (serialize the recharts `<svg>`, resolve
  `var(--…)` to computed colours, white bg, 2× canvas → PNG); the 3D scene via `MoleculeViewer`'s
  imperative `toPngBytes()` (3Dmol `pngURI()` readback). Fixed 2× resolution, named.
- **Where the buttons live:** data-export bar in `ResultsCard` (xyz/charges/MO/thermo — each present only
  when its data is); the frequency CSV + spectrum PNG in `IrSpectrumPanel` (it owns the display scale and
  the chart); the energy PNG in `TrajectoryPlayer`; the 3D snapshot in `OrbitalPanel`. Absent data → the
  button is disabled with a reason, never an empty file.

## `src/charts/useContainerWidth.ts`
The one owner of the **WebKitGTK 0×0 `ResponsiveContainer`** workaround (`debugging/002`/`003` class):
a `ResizeObserver` that feeds an explicit pixel width to every chart. Extracted from
`ConvergenceDashboard` (which now imports it) so the convergence, trajectory-energy, and IR charts
share one copy.

## The Job-detail screen: one layout, and where the header energy comes from (unit 3.9)

**The header title is inline-renamable (unit 3).** The `<h2>` job title hosts an `<InlineRename>`
(`jobs/InlineRename.tsx`, documented in [groups-ui.md](groups-ui.md#inline-job-rename-jobsinlinerenametsx-unit-3))
→ `rename_job(id, title)` then `setJob(updated)` (the header updates in place, no reload). The Rust
command trims + refuses an empty title and is **state-agnostic** (a running job renames fine). Rename
touches only `jobs.title` — not `input_content`, not the `job_dir`, and it is **not retroactive** to a
derived child that baked `— <old title>` into its own title at create time.

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

**A GOAT conformer search renders the ensemble, not the single-structure dashboard.** A GOAT job's
`.property.txt`/`_trj.xyz` are one candidate's optimization *cycles*, not a meaningful single result, so
`parse_and_store` routes GOAT past the single-structure readers and leaves the job **`completed`**
(`orca/goat.md`, `debugging/017`). In the UI: the ensemble is read on **any terminal success**
(`isTerminalSuccessStatus` = `completed` **or** `parsed` — a GOAT job that already reached `parsed` under
older logic still shows its ensemble), and `showsSingleStructureResults(input)` (`= !isGoatInput`)
**suppresses `ResultsCard`** for a GOAT job so its misleading "N optimization cycles" trajectory is never
shown; a GOAT job with no readable ensemble shows a plain note. Non-GOAT jobs are unaffected. Regression
+ fix: [debugging/017](../debugging/017-goat-parsed-hid-ensemble.md).

## See also
- [ADR-011](../architecture/adr-011-editor-graphics-stack.md) — 3Dmol is a dumb renderer until the
  spike passes; the reason the frame number may not live in the viewer.
- [visualization.md](visualization.md) — the `MoleculeViewer` this feeds (and its
  `preserveCameraOnUpdate` note).
- [artifact-readers.md](artifact-readers.md) / [`orca/parse-sources.md`](../orca/parse-sources.md) —
  the parsed `data_json` this stands on, and the `orca_mapspc` cross-check.
