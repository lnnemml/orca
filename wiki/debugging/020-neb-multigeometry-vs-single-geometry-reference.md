# 020 — a NEB-TS job failed to parse: 2.45 Å GeometryMismatch (multi-geometry vs the single-geometry reference)

**Status:** solved (Phase 4.5 Stage E3a-1 completion). **Area:** `src-tauri/src/results.rs`
(parse orchestration), `src-tauri/src/parse/neb.rs`. **Kind:** a wrong-shaped reference model — a
correct calculation rejected by a post-condition whose premise does not hold for this job type.

## Symptom

A NEB-TS job that ran correctly (`ORCA TERMINATED NORMALLY`, the converged TS = the known
Menshutkin saddle) **failed to parse** with a banner like:

```
geometry post-condition failed: max interatomic-distance Δ 2.45 Å exceeds tolerance
```

`.neb` was never populated — the band reader (`neb.rs`, landed in E3a-1) never ran. The energy,
the geometry, nothing reached the Results screen. The job showed as a parse failure despite a
perfect run.

## Root cause

A NEB-TS job is **multi-geometry**, and the standard parse path assumes **single-geometry**.

- `results.rs::parse_and_store` builds `input_ref` from the job input's `* xyz` block and runs
  `PropertyFile::verify(&input_ref)` — a post-condition that recomputes the property.txt's **first**
  `$Geometry` and asserts it matches the input within tolerance. Its premise: *the input geometry ==
  the property-final/first geometry* — true for an SP or an Opt (you optimize the geometry you gave).
- For a NEB job that premise is **false three ways**: the input `* xyz` is the **REACTANT**; the
  property.txt holds the **BAND** (measured: 22 `$Geometry` blocks on this run — the interpolated
  images across iterations, not one structure); and the `.gbw`/`.xyz`/`_NEB-TS_converged.xyz` hold the
  **TS**. So `verify` compares the reactant to a band image and fires a **~2.45 Å `GeometryMismatch`**
  — `r ≈ 1` (a real *different structure*, not a unit error, not `.gbw` staleness) — and aborts
  **before** `neb.rs` runs.

This is the exact shape of `debugging/015` (a scan's first `$Geometry` ≠ the input) and `017` (GOAT's
cycles ≠ a single result): a **special multi-structure job type** does not fit the single-geometry
reference, and forcing it through produces a misleading geometry-mismatch abort.

## Fix — route a NEB job to its own band+TS parse (mirrors the scan / GOAT branches)

`parse_and_store` already routes a **scan** (detected by `input.relaxscanact.dat`) to
`parse_and_store_scan` and a **GOAT** job (detected by the `! … GOAT` keyword) to `NoArtifact`,
**both before** the fatal `input_ref` verify. NEB is the third such type:

- **Detect** with `input_has_neb(input_content)` — mirrors `input_is_goat` exactly (whole-token,
  case-insensitive, split on non-alphanumeric, so `NEB-TS` yields the `NEB` token; not a regex).
- **Branch** to `parse_and_store_neb` right after the GOAT branch, before `input_ref` is built.
- `parse_and_store_neb` parses the band via `neb.rs` and builds the result with
  `ParsedResults::from_neb`: `final_geometry` = the **converged TS**, `final_energy_eh` = the
  converged-TS comment energy. It uses the reactant `* xyz` **only** for the element-ORDER check
  (`neb.rs` asserts converged-TS order == reactant order — that IS a NEB precondition) and
  **never** runs the reactant-referenced geometry match. Single-structure quantities
  (charges/dipole/thermo/freq/traj/orbitals) are absent — same discipline as `from_scan_profile`.

The level is exact: the **order** guard (mandatory for NEB) is kept; only the **geometry-match**
guard (whose premise is false here) is dropped. No tolerance was loosened — the guard moved to
where its premise holds (rule #11 discipline, as in `debugging/015`).

## Bite (negative controls, `results/tests.rs`)

- `neb_job_parses_via_the_band_route_not_the_reactant_reference` — the full pipeline on the real
  probe fixtures now returns `Parsed`; `.neb` has 24 iterations, `final_geometry` N···C ≈ 2.353 (the
  TS), `final_energy` ≈ −472.7549. **Before** the route this was `ParseFailed`.
- `the_reactant_reference_would_fail_on_a_neb_property_file` — proves the pre-state: running
  `PropertyFile::verify(&reactant_ref)` on the NEB `input.property.txt` returns a `GeometryMismatch`
  with `max_delta > 1.0` (why the route is needed).
- `input_has_neb_detects_the_keyword` — accepts NEB/NEB-TS/NEB-CI, rejects Opt/scan.

## Standard path unchanged

The branch is gated on `input_has_neb`, so a NEB job never reaches the single-structure readers, and
a non-NEB job never takes the NEB route — the SP/Opt/scan/GOAT parse paths are behaviorally
unchanged (all their tests stay green).

## Related

- `wiki/orca/neb.md` — the NEB-TS run + the three band artifacts.
- `wiki/debugging/015-scan-property-post-condition.md` — the same class for a scan (first
  `$Geometry` ≠ the input).
- `wiki/debugging/017-goat-parsed-hid-ensemble.md` — the same class for GOAT (cycles ≠ a result).
- `wiki/debugging/019-orca2json-plain-opt-gbw-staleness.md` — the neighbouring geometry-check
  refinement (a same-unit difference that is NOT a unit error).
