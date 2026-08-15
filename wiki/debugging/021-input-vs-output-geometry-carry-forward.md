# 021 — New iteration carried the parent's INPUT seed, not its converged OUTPUT

**Symptom.** A downstream single-point/Freq created via **New iteration** from a converged OptTS sat
on the wrong geometry, producing a wrong barrier/energy — **with no crash and no error**. The 3D
viewer of the parent showed the *correct* converged structure, so the results parser looked fine.

## Root cause (measured, deterministic)

`NewJobScreen` seeds a New iteration from the parent `Job` object already in memory:

- `content` ← `initialJob.input_content` (the stored `.inp` text — the `* xyz` **seed** block),
- scene ← `restoreSceneLog(initialJob.input_content, initialJob.scene_json, …)` (the **creation-time**
  snapshot).

Both are the **SEED** (the geometry the job was *created* with), **not** the converged output. For an
optimization the two differ; using the seed silently seats a downstream single-point on a
non-stationary geometry.

**Confirmed on the real DA OptTS job `661a60a5`** (`! r2SCAN-3c OptTS Freq`, 16 atoms), forming
**C0–C10**:

| Source | forming C0–C10 |
|---|---|
| `input.inp` `* xyz` (≡ DB `input_content`) ≡ `input_trj.xyz` **first** frame (SEED) | **2.3636 Å** |
| `input.xyz` ≡ `input_trj.xyz` **last** frame ≡ `results.final_geometry` (CONVERGED) | **2.2893 Å** |

New iteration carried **2.3636** (bit-identical to the seed). Max per-atom seed→converged
displacement 0.092 Å.

## Why it was silent — and why it bit only on TS single-points

- **Washed out by downstream relaxation.** A carried geometry fed into another `Opt`/connectivity
  relaxes into the correct basin — the seed error evaporates. Only a **single-point** (SP/Freq), which
  does *no* relaxation, freezes the wrong geometry into the energy.
- **Invisible on minima.** For a minimum the seed ≈ the converged output (start ≈ end), so the wrong
  source is indistinguishable from the right one.
- **Bites on a TS.** A TS seed is a *rough guess* far from the located saddle, so seed ≠ converged is
  large — and a TS single-point (for ΔE‡/ΔG‡) is exactly the no-relaxation case. So the corruption
  concentrated on the highest-value numbers: reaction barriers.

## The subtle part (not "input is bad")

The leak was **not** "reading input". `input.xyz` — the file ORCA overwrites with the optimized
geometry — is 2.2893, **correct**. The leak was the DB **`input_content`** text + **`scene_json`**
snapshot (both the creation-time seed). The fix reads **`results.final_geometry`** (via
`read_job_results`) — the exact source the viewer and `finalGeometryXyz` already use
(≡ `input.xyz` ≡ last `_trj.xyz` frame). Only New iteration leaked; Export geometry,
"Use best (DFT)", and every derived spawn (OptTS/NEB/connectivity/reopt/F3) already read
`results.final_geometry`.

## Fix (TS-only, `src/scene/carryForward.ts` + `NewJobScreen`)

- **Resolution** — `resolveCarryForwardGeometry(job, results)` returns the **converged**
  `results.final_geometry` for a converged optimization, or an **honest refusal** (never a silent
  seed): a scan/NEB (many geometries → pick a point/image), a **non-converged** optimization (last
  geometry not stationary — reuses `results.converged`, the convergence verdict from the
  convergence-status guard), or no parsed result. A single point carries its geometry as
  `single-point` (its final == its input — no bug).
- **Variant (a)** — a converged carry seeds a **fresh** scene from the output; the parent's edit log
  is **not** carried (it produced the seed, not the output — carrying it would be a label≠content
  lie). A green banner says "seeded from the converged output of …" so it reads as from-the-output,
  not seed-editing.
- **Guard (defense-in-depth)** — `geometryMatchesFinal(geometry, results)`: the carried geometry must
  **bit-match** the parsed final frame; the seed (2.364) does not → rejected, even under a future
  re-route. (Bit-exact, element order included; the `.engrad` final gradient ≈ 0 is an available
  *secondary* check but not load-bearing — it is not on every exported dir.)
- **Provenance** — a `# geometry: converged output of job <id>` header in the new job's
  `input_content` (visible, persisted, copied verbatim into an export) makes the swap impossible to
  ship silently. (A structured jobs-column + manifest field is a deferred follow-up — it needs a
  migration; the comment already achieves the non-silent + exported goal.)

## Non-regression

A New iteration of an Opt **minimum** (start ≈ converged) still carries the converged frame (the guard
passes — it bit-matches the final), so the happy-path is intact; connectivity/scan/derived flows are
untouched (they never used `input_content` for geometry).
