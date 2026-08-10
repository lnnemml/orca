# Connectivity check — does a TS join the two basins I meant? (Stage E2)

A located first-order transition state has **exactly one imaginary frequency**, and its
normal mode **is the reaction coordinate** — the one direction in which the TS is a
maximum. Displacing the TS geometry a small distance **±δ** along that mode and relaxing
each displaced structure (a plain `Opt`) lands in the **two minima the saddle connects**.
Comparing those two minima answers the question a TS search cannot answer by itself: *does
this saddle connect the reactant and product I intended, or something else?*

This is a **poor-man's IRC**. The rigorous alternative is ORCA's `! IRC` (steepest-descent
along the mass-weighted coordinate); the displaced-endpoint method is cheaper, needs only
two ordinary optimizations, and covers the common case. `! IRC` is noted in ROADMAP Stage E
as a possible later, more rigorous mode.

## The method (as the app runs it)

1. **Read the imaginary mode** from the TS job's parsed `.hess`: the single negative
   frequency's column of `$normal_modes` (`modeDisplacements(...)`, already the animation
   vector — **not re-parsed**).
2. **Displace ±δ** — `x_TS + δ·v̂` and `x_TS − δ·v̂`, where the mode is normalized so the
   busiest atom moves exactly δ (`displaceAlongImaginaryMode`, reusing the validated
   animation math `modeFrameCoords` at sin = ±1). **The app generates BOTH geometries** —
   the manual forward/backward mix-up that wastes a run cannot happen.
3. **Two plain `Opt` children** — `buildReoptInput(tsInput, seed, {freq: false})` per
   endpoint (Fork B: plain Opt, **no Freq** — the endpoints are minima, we only need their
   geometry). Method, solvation, **and charge** are inherited from the TS input verbatim
   (comparability + the charge footgun; asserted back out of each emitted child). Created
   via the generic `create_optts_job` path, so both children join the TS's pathway.
4. **Verdict** once both relax — `connectivityVerdict(forward, backward, ts)`:
   `distinctBasins` ⟺ each endpoint moved off the TS **and** the two endpoints differ from
   each other. `reactionCoordinateChanges` lists the bonds that changed most (forward / TS
   / backward distances), so the chemist reads which endpoint is reactant vs product.

## δ = 0.5 Å is measured, not assumed

δ default **0.5 Å** (user-adjustable). Too small and an endpoint relaxes **back to the
saddle** (no split → `distinctBasins` false); large enough and each endpoint rolls
decisively into its basin. 0.5 Å split the validated case cleanly. δ scales the mode so
the **busiest atom** moves δ (not a raw multiplier — see `wiki/orca/parse-sources.md` unit
3.13 on why a norm multiplier is the wrong quantity).

## The metric — distance-matrix, not Kabsch RMSD

"Distinct basins" is judged by the **maximum change in any single interatomic distance**
(`maxInteratomicDistanceDelta`), NOT a Kabsch-aligned Cartesian RMSD. Reasons:

- **Rotation/translation invariant by construction** (it compares distances, never
  coordinates), so ORCA's per-job reframing of each endpoint is irrelevant — no alignment
  step, no hand-rolled SVD. This is the same discipline the Rust geometry post-conditions
  use (`parse/mo.rs`, `parse/hess.rs`).
- **A max, not a mean** — a reaction breaks/forms a few bonds; the max reads that change at
  full magnitude instead of diluting it across the many unchanged pairs, so the thresholds
  are **size-independent** (a whole-matrix RMS shrinks with molecule size, a Kabsch RMSD
  averages the signal away).

Thresholds (provisional, confirmed by the manual gate): an endpoint must differ from the TS
by ≥ **0.3 Å** in some interatomic distance to have "left the saddle"; the two endpoints
must differ from each other by ≥ **0.5 Å** to be two distinct basins. A δ-too-small run
fails the separation clause (both relaxed back to ≈ the TS); two endpoints that both roll to
the **same** basin also fail it (each far from the TS, but not from each other).

## Validated — MeNH₂ + EtI (Menshutkin SN2)

On the real located TS (r2SCAN-3c / SMD(dmf), charge 0 1):

| endpoint | seed N···C (Å) | relaxed to | N–C (Å) | C–I (Å) |
|---|---|---|---|---|
| forward (+δ) | ≈ 1.668 | **product** | ≈ 1.51 | ≈ 4.12 |
| backward (−δ) | ≈ 3.039 | **reactant** | ≈ 3.6 | ≈ 2.2 |

Both endpoints parse cleanly (the plain-`Opt` `.gbw`-staleness fix, `wiki/debugging/019`).
The forward endpoint forms the N–C bond and ejects iodide (product); the backward endpoint
restores the C–I bond and separates the amine (reactant). `distinctBasins` ✓.

## Honest-absent

The verdict appears **only when both children have parsed**. While a child is queued /
running / not yet parsed, the panel says `pending`; a failed child is reported as a failure,
never smoothed into a verdict. The two child ids are persisted in `localStorage` keyed by the
TS job (no schema change) so the verdict survives a reload.

## Related

- `wiki/chemistry/normal-modes.md` — why the imaginary mode IS the reaction coordinate.
- `wiki/orca/optts.md` — locating the TS whose mode this check displaces (Stage E1).
- `wiki/orca/parse-sources.md` — the mode vector (Cartesian, unit-normalized), δ amplitude
  calibration, the distance-matrix post-condition idiom.
- `wiki/modules/reactions-ui.md` — where the panel lives in the Results screen.
