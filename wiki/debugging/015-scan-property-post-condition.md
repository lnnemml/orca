# debugging/015 — a completed relaxed scan fails the full results parse

**Date:** 2026-08-07 · **Area:** rust-core (parse pipeline)
**Symptom.** A completed relaxed-surface-scan job (`C2H6` C–C scan) fails `parse_and_store`
with:

```
geometry post-condition failed: max Δ 0.056013 Å exceeds 1e-4
(a missed Bohr→Å conversion looks like ≈1.889×)
```

The calculation itself succeeded (`ORCA TERMINATED NORMALLY`, six scan points written); it is
**our parse** that fails, so the job stays `completed` with an error banner and never reaches
`parsed`. This blocked the Phase 4.5 **B2 manual gate** (h1–h4): with the parse failing, the
scan profile is never stored, so the energy-profile panel has nothing to render.

## The Bohr hint is a red herring (measured, rule #10)

The error text mentions Bohr because the geometry post-condition (`property.rs::check_geometry`,
via `verify_geometry_atoms`) exists to catch a **missed Bohr→Å conversion**, which shows up as
≈1.889× — a **~1 Å** displacement on these coordinates. The reported Δ is **0.056 Å**, two orders
smaller. It is not a units error at all:

- input C–C distance = **1.512058 Å** (real job `ddac72c8`);
- scan point 1 constrains C–C to **1.400 Å**;
- each carbon therefore moves ≈**0.056029 Å** → matches the reported 0.056013 exactly.

(The `scan-ethane-cc` **fixture** is a *different* run with input C–C = 1.527 Å → its max Δ is
**0.0635 Å**, the H-atom z-compression; same story, same scale. Both are compression-scale, both
far below a 1.889× Bohr blow-up.)

## Root cause — a single-structure post-condition on a multi-point artifact

`property.rs::check_geometry` compares the **first `$Geometry`** against the **input xyz**
(`input_ref`) within 1e-4 Å (`results.rs`, the Bohr guard). Its premise is **"first structure ==
input"**. That premise is **structurally false for a relaxed scan**: the first `$Geometry` in a
scan's `.property.txt` is **scan point 1's constrained-optimized geometry** (C–C already pulled to
1.400 Å), not the input. `property.rs` — energies/geometry/**charges/dipole/thermo** — was built
for a **single structure**; a multi-point scan `.property.txt` does not fit it.

### Measured `.property.txt` structure (rule #10, before fixing)

On the real scan `.property.txt` (`scan-ethane-cc/input.property.txt`):

- **26 `$Geometry` blocks** — one per **optimization cycle across all six scan points**, NOT six
  scan points (already recorded by `property::tests::scan_geometry_blocks_are_per_cycle_not_scan_points`).
- The **first** `$Geometry` (`&GeometryIndex 1`) is scan point 1: C at z = ±1.322808 Bohr →
  C–C = **1.400 Å** (the constraint), ≠ the input's 1.512 Å.
- **charges** (`$SCF_*_Population_Analysis`) appear at only *some* cycles (the converged points),
  not one per structure; **dipole** (`$SCF_Dipole_Moment`) appears **once, at the very end**;
  there is **no `$THERMOCHEMISTRY_Energies`** block (no Freq in a scan). So reading it as one
  structure would **mis-attribute** per-point charges and a single dipole to the wrong geometry.

### `_trj.xyz` was a second latent failure

`xyz.rs` `_trj` verify is also `input_ref`-anchored (first frame == input). Measured: the scan
`_trj.xyz` first frame has C–C = **1.400 Å** (z = ±0.700), not the input — so it too would fail
the geometry post-condition. `hess.rs`/`mo.rs` anchor on the **final** geometry, and a scan has no
Freq/gbw-final structure to anchor them, so they never applied either.

## Fix — scan jobs parse **profile-only**

`results.rs`: when `input.relaxscanact.dat` is present (the same detection B1 already uses),
`parse_and_store` branches to `parse_and_store_scan`, which:

- parses the **profile** (B1 `relaxscan`) — whose own per-point geometry cross-check (`.dat`
  coordinate vs `input.NNN.xyz`) is the scan's **live units guard** (rule #11);
- builds the stored record via `ParsedResults::from_scan_profile`: `final_energy_eh` = the
  profile's **last point** (composite `act` energy), `final_geometry` = the **last point's**
  optimized structure (`input.NNN.xyz`, Å) so the viewer and the scan panel's element
  cross-check have a molecule; all single-structure quantities left empty (correct — a
  multi-point scan has none that belong to one structure);
- **skips** the single-structure readers (`property.rs`, the `_trj` `xyz.rs` verify, and the
  `property`-anchored `hess.rs`/`mo.rs`). A scan reaches `parsed` from the profile alone.

Non-scan jobs are **untouched** — the Opt/SP/Freq path still runs all four readers and still
catches a Bohr error. **No tolerance was loosened and the Bohr guard was not skipped**; the guard
simply moved to where its premise holds (B1's cross-check for a scan).

**Commit:** `fix(parse): scan jobs parse profile-only …` (Phase 4.5 B1 fix).

## Lesson — an isolated-artifact test can miss a full-pipeline failure

B1 tested `relaxscan` (the `.dat` reader) **in isolation** and B2 tested `read_scan_geometries`
against the point `.xyz` fixtures — neither ever ran the **full `parse_and_store` on a scan job
dir**, so the single-structure readers' collision with a multi-point artifact was invisible until
the manual gate ran a real scan. The gap is now closed:
`results::tests::scan_job_parses_profile_only_full_pipeline` runs the full pipeline on the real
`scan-ethane-cc/` dir (RED before this fix — the 0.056-class `GeometryMismatch`; GREEN after), and
`single_structure_property_check_bites_on_a_scan_artifact` demonstrates the routed-around guard
still bites (proving the tolerance was not loosened). See `wiki/orca/parse-sources.md` and
`wiki/modules/results-ui.md`.
