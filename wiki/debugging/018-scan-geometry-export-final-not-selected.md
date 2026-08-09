# 018 — Scan geometry export gave the final point, not the selected (approx-TS) one

**Phase 4.5, Stage B2 / Stage-E seam. Frontend-only.**

## Symptom

On the real Menshutkin scan (methylamine + ethyl iodide, DMF/SMD), the author selected the
approximate-TS point in the scan-profile panel — point **8/12, N···C ≈ 2.418 Å**, the scan maximum,
the whole point of the panel ("the maximum → refine with OptTS") — and clicked export to get a seed
geometry for an OptTS refine. The exported `.xyz` was **not** that point: it was the **last** scan
point (≈ 1.8 Å, N and C already bonded). Seeding OptTS from it would start the saddle search from the
wrong side of the barrier — a silently wrong scientific input, not a crash.

## Root cause

There was **only one** geometry-export path: the top `ExportBar` button, hardwired to
`results.final_geometry`. For a scan, `parse_and_store` routes to **profile-only** and builds
`final_geometry` from the **last** point's `input.{n:03}.xyz` (`results.rs`, n = last) — see
`debugging/015`. So "export geometry" always meant "export the last scan point," regardless of which
point the panel's viewer was showing. The panel had a per-point **viewer** (app-owned `selected`
index, fed from `read_scan_geometries`) but **no per-point export** — the selection the researcher
made never reached a file.

Compounding it, the panel opened on **point 1** (`useState(0)`), not the approx-TS maximum — so even
visually it did not land on the point the researcher wanted.

## Fix

Frontend-only; no Rust / command / schema change.

1. **`scanPointExportXyz`** (`src/scan/scanProfile.ts`) — a pure builder that **reuses the canonical
   `finalGeometryXyz`** (no second xyz formatter; the atoms+2 post-condition, rule #9, is inherited).
   It composes the point number + coordinate into the comment, tags `(approx TS / scan maximum)` only
   when the point is the maximum, and exports `energy_act_eh` (the plotted composite total).
2. **`ScanProfilePanel`** — a `geometry .xyz` button in the readout row exports
   `geometries[clamped]` (the **same** `read_scan_geometries` array the viewer is fed), **never**
   `results.final_geometry`. **Honest-or-absent:** enabled only when the selected point actually
   renders (`viewerState && "xyz" in viewerState`, element order agrees). The initial `selected` now
   defaults to `maxIndex(points, "act")` — the panel opens on the approx-TS point.
3. **`ResultsCard` ExportBar** — for a scan job the top button is relabelled
   `geometry .xyz (last point)` with a "last scan point" comment, so the last point is offered
   honestly and not confused with the selected approx-TS geometry. A non-scan job is unchanged.

## The seam this establishes

This selected-max geometry is the **Stage-E seam**: an OptTS-refine child job (E1) reuses exactly the
geometry exported here as its saddle-search seed — the scan panel's "click the maximum" continued into
"refine it."

## Guard against regression

`src/scan/scanProfile.test.ts` — `C-reuses-canonical` (body byte-identical to `finalGeometryXyz`),
`C-approx-ts-tag` (tag iff `isMax`, negative control), `C-atom-count-inherited` (length mismatch
throws — the inherited post-condition). Grep checkpoints on pull: `final_geometry` does **not** appear
in the panel's per-point path; the initial `selected` is `maxIndex(...)`, not a hardcoded `0`.
