# Module: 2D relaxed-surface scan — parse, contour viewer, node→OptTS (Stage 4b)

**What it is.** The result side of a two-coordinate relaxed-surface scan (the input side is Stage 4a,
`wiki/orca/scan.md`): parse ORCA's `input.relaxscanact.dat` into a grid, **visualise it as a filled
contour / heatmap (a 2D PES)**, let the researcher **click a grid node**, and hand that node's geometry
to the source-agnostic **OptTS** engine. Unblocks Diels-Alder TS-finding (a concerted reaction mapped as
a 2D surface). Relates to [scene](scene.md) (`parseScanCoordinates` routes 1D vs 2D), [results-ui](results-ui.md)
(the 1D `ScanProfilePanel` this parallels), [ADR-020](../architecture/adr-020-optts-refinement-source-agnostic.md)
(the OptTS engine it reuses), [groups-ui](groups-ui.md) (the 2b group picker that rides along).

**Status:** landed (Stage 4b, 2026-08-14). **The runtime plotly render is Anton's live m0 gate** (see below).

**Files:** `src/scan/scanSurface2d.ts` (+`.test.ts`, `.fixture.ts` — the pure parser), `src/scan/ContourPlot.tsx`
(the isolated plotly wrapper), `src/scan/ScanSurface2dPanel.tsx` (the panel), the thin Rust reader
`results.rs::read_scan_surface` + its `commands/jobs.rs` wrapper, the B1 stand-down in
`parse/relaxscan.rs`, routing in `screens/JobDetailScreen.tsx`.

## The pure parser (`scanSurface2d.ts`)

`parseScanSurface2d(datText) → { axis1, axis2, energies, nodeRow } | null`. The `.dat` is space-delimited
triples `c1 c2 E_act`, **row-major OUTER=coord1 / INNER=coord2** (measured — `wiki/orca/scan.md`). `axis1` /
`axis2` are the unique coord1 / coord2 in first-seen order; `energies[i1][i2]` = E (Eh); **`nodeRow(i1,i2)` =
the 1-based `.dat` row = point-file `NNN` = `geometries[row-1]`** (`i1*N2 + i2 + 1`). All-or-nothing: `null`
on a malformed row, a non-rectangular grid, or a layout that doesn't match row-major outer=coord1 —
**refusing to mis-plot rather than silently transpose**. Non-square (N₁≠N₂) is fine.

**The identity seam (the MAIN RISK).** A transpose or off-by-one in `nodeRow` would BOTH mirror the plotted
surface AND hand OptTS the WRONG node's geometry (the class the export unit defended). It is pinned by a
bite against the **real 10×10 Diels-Alder** fixture (`scanSurface2d.fixture.ts`, verbatim from job
`d6d2c3b0`): `nodeRow(0,0)=1` (reactant), `(9,9)=100` (product), `(0,9)=10` / `(9,0)=91` (the two stepwise
corners — asymmetric, so the test catches transposition even on a square grid). The measured global **max**
is a stepwise corner (`-17.78704`, rows 10 & 91), **not** an interior saddle — documented so the UI never
auto-picks the max.

## Why the geometry source is NOT `read_scan_geometries` (measured — rule #10)

`read_scan_geometries` is gated on `results.scan`, which is **`None` for a 2D scan**: the B1 1D reader is
2-column, and the real 10×10 job was `completed` with **no results row** (`error_message`: *"relaxscan:
malformed scan coordinate column: not strictly monotone at point 1 (3.446 then 3.446)"* — `c1` repeats
across the outer loop). So a file-gated sibling is used instead:

- **`read_scan_surface(job_dir) → Option<{ dat_text, geometries }>`** (`results.rs`) — gated on
  `input.relaxscanact.dat` **existing** (NOT `results.scan`); returns the `.dat` text + `input.NNN.xyz` for
  `N = 1..row_count` **in row order** (`geometries[NNN-1]` ↔ row `NNN`). `None` for no dir / no `.dat`
  (absent-is-normal, mirroring `read_scan_geometries`). Reads whole (small, rule #5, capped); writes nothing
  (rule #3). **Does NOT touch `read_scan_geometries`.** Cargo-tested for row order + absent→None.
- **The B1 stand-down** (`parse/relaxscan.rs`): a column discriminator — a **3-column** (2D) `.dat` → the
  1-coordinate reader **stands DOWN cleanly** (`Ok(None)`, NOT a `Malformed` error), so a successful 2D scan
  finishes without the scary monotone failure; a **2-column** (1D) `.dat` → the full monotone + Å cross-check
  guard runs UNCHANGED (the 1D guard is not weakened — bite + control both cargo-tested). It does NOT parse
  2D into `results.scan` (that shape is a 1D profile, not a grid — the surface is read separately here).

**Count assert (the identity gate in the UI):** the panel asserts `geometries.length === N₁×N₂` before
enabling click-to-refine. A partial run (fewer point files than rows) → the assert fails → "incomplete
scan", handoff **disabled** — never a wrong geometry.

## The viewer (`ScanSurface2dPanel.tsx` + `ContourPlot.tsx`)

Routed from `JobDetailScreen` when **`parseScanCoordinates(job.input_content)?.length === 2`** — a **branch**,
not a replacement: a 1D scan still renders the old `ScanProfilePanel` (inside `ResultsCard`), unchanged.
Gated on the INPUT (not results), because a 2D scan has no `results.scan`; the panel loads its own data via
`read_scan_surface`.

- **Contour + heatmap** of ΔE (kcal/mol vs the global minimum) over (coord2 = x, coord1 = y), with a colorbar
  and hover `(c1, c2, ΔE)`. The four corners are labelled by **bond length** ("reactant — both long",
  "product — both short", "stepwise — 1 long, 1 short"; long/short is per-axis, correct whichever way the
  scan steps) — **orientation facts, never a TS claim**.
- **The whole surface is clickable — a click SNAPS to the nearest grid node.** `ContourPlot`'s `onClick`
  reads the click's DATA-space `x`/`y` from `e.points[0]` and snaps: `i2 = nearestIndex(axis2, x)` (coord2 =
  X), `i1 = nearestIndex(axis1, y)` (coord1 = Y) — **not swapped**, or OptTS would get the transposed node
  (`src/scan/nearestIndex.ts`, `nearestIndex` = argmin `|v − target|`, clamps, correct for a descending
  axis; bite-tested). The node markers stay as the visual affordance, but the hit-test is no longer
  marker-only. *(The m2 bug: the old `onClick` gated on `e.points.find(curveNumber === 1)` — the 6px marker
  trace — so a click that wasn't pixel-exact on a node returned only the contour point and was silently
  dropped. Snapping makes the whole contour clickable.)*
- The snapped node → `nodeRow` → `geometries[row-1]` → the **verbatim `ScanProfilePanel` OptTS handoff**:
  `buildOptTSInput(source.input_content, seed)` (rebuilds a fresh OptTS input — the Scan block can't leak) →
  `create_optts_job` → the **2b `useGroupPicker`** (default = the scan's group, overridable) → `submit_job`
  → navigate. **No node is preselected** — the user reads the surface and clicks the col.

## The plotly dependency (and the WebKitGTK gate)

recharts (the app's chart library) has **no contour/heatmap trace**, so a 2D PES needs plotly. To keep it
WebKitGTK-safe and lighter, the dependency is **`plotly.js-cartesian-dist-min`** — the **SVG cartesian**
bundle (scatter/heatmap/contour), **NOT** the WebGL (`scattergl`) traces. All plotly usage is isolated in the
one **`ContourPlot.tsx`** wrapper, so if plotly misbehaves in WebKitGTK the swap to a d3-contour fallback is
contained (the panel keeps the same `<ContourPlot>` props). **De-risk order:** the runtime render (m0) is the
FIRST live gate — if plotly renders heavy/blank, swap `ContourPlot` before trusting the panel. The static
integration is de-risked (tsc + `vite build` pass; the main bundle grows ~1 MB gzip — acceptable for a local
desktop app, not a network-loaded page).

## Deliberately out of scope

Auto-picking a TS node (the global max is a stepwise corner); a saddle / minimum-energy-path finder (a
clearly-labelled follow-up); 3-D+ grids (the parser/builder already loop 1..N; the viewer is 2D); parsing
2D into `results.scan` (a 1D-profile shape — rejected). No change to `read_scan_geometries`,
`create_optts_job`, `buildOptTSInput`, or the 1D `ScanProfilePanel`.
