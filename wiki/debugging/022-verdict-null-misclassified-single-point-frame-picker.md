# 022 — A null convergence verdict was misclassified "single-point", so New iteration silently seeded the INITIAL geometry

**Symptom.** New iteration from a **post-GOAT `! Opt`** (e.g. a GOAT ensemble winner re-optimized at
r2SCAN-3c) seeded the **initial** geometry (the seed the opt *started* from), not the optimized output
— **with no crash, no error, no refusal banner**. The very fix of [021](021-input-vs-output-geometry-carry-forward.md)
(carry the converged output) was **bypassed** for this class of job.

## Root cause (measured, deterministic)

[021](021-input-vs-output-geometry-carry-forward.md)'s `resolveCarryForwardGeometry` decides *what to
carry* from the **convergence verdict** `results.converged`:

- `true` → carry `results.final_geometry` (converged output);
- `false` → refuse (last geometry not stationary);
- `null` → treated as **`single-point`** → **early-return, keep the seed** (an SP's final == its input,
  so keeping the seed is correct *for a real SP*).

But `converged` is `null` for **two different** situations, and the classifier conflates them:

1. a **genuine single point** (no `Opt` — final == input, keeping the seed is right); and
2. an **optimization whose `OPTIMIZATION RUN DONE` / `THE OPTIMIZATION HAS CONVERGED` marker sits beyond
   the bounded output tail** — the convergence-status guard reads the (bounded) tail, finds no marker,
   and returns `null` (NotApplicable), **not** `true`. A post-GOAT r2SCAN Opt is exactly this: a long
   output whose convergence banner is past the 64 KB tail window.

For case 2 the job **is** an optimization with a full `_trj.xyz` trajectory and a genuine optimized last
frame — but the `null` verdict routed it down the single-point branch, so the override never ran and the
**initial** geometry (trajectory frame 0 / the seed) was kept. Silent, because it is the *same* silent
class 021 closed: an input-vs-output geometry swap that does not crash.

## The subtle part — the verdict is the wrong discriminator

The verdict answers "did it converge?" — a question about **stationarity**, useful for a *label*. It is
**not** a reliable answer to "is this an optimization?" (a `null` verdict can mean *unknown*, not
*no-optimization*). Classifying the carry *action* by the verdict is the defect: a robust classifier must
key on **the presence of an optimization trajectory**, which is verdict-independent.

## Fix (`src/scene/carryForward.ts` + `NewJobScreen`) — an explicit frame picker

Replace the verdict classification with an **explicit geometry-frame picker** over the parent's
optimization trajectory:

- **`iterationFrames(job, results)`** — an optimization with ≥ 1 `results.trajectory.frames` → the
  picker, **regardless of `results.converged`** (the trajectory presence is checked *before* the
  verdict). The **default is the LAST frame** (the optimized output) — never frame 0, never the
  `input_content` seed. Each frame's geometry comes **directly** from `results.trajectory.frames[i]`
  (elements stored once, per-frame Å coords), never reconstructed from input.
- **Verdict → label only** (honest-or-absent): last frame `true` → "final (converged)"; `false` →
  "last frame — did not converge (not stationary)" (**still selectable** — max control, informed, not
  refused); **`null` → "final frame (optimized output)"** — claims neither convergence nor
  non-convergence (a `null` verdict is *unknown*, not *failed*). Frame 0 → "initial geometry"; middle →
  "cycle N".
- **Refusals reuse 021's reasons** for scan / NEB / no-result (they keep their per-point/per-image
  handoff); a single point returns a distinct `no-trajectory` refusal.
- **Unconditional default seed in `NewJobScreen`** — when `iterationFrames` is `ok`, the default (last)
  frame is seeded **unconditionally**, killing the old "converged? *maybe* override" race that could
  leave the seed. The picker (`<select>` of frames + energies) lets the user re-pick; a re-pick reseeds
  from that real frame. Provenance: a `# geometry: frame <i> (<label>) of job <id>` header (stripped &
  replaced on re-pick, so it never stacks) + a banner.

## Two edges made honest (not silent)

- **`no-trajectory` must not swallow a real Opt.** A `no-trajectory` refusal → silent seed-keep **only**
  when the input is a genuine single point (`isSinglePoint(input)` — no Opt/OptTS/Scan/NEB/GOAT/IRC
  keyword). An input that **does** request an optimization but has **no parsed trajectory** (a parse
  glitch) is **warned** — "its trajectory could not be read; the geometry shown is the INPUT SEED, not
  the optimized output" — never a silent seed. Honest-or-absent at the edge.
- **The default is unconditional, not "maybe".** The 021 bug and this one share the two-effect race (a
  sync seed placeholder + an async override that might not fire). Here the async override **always** fires
  for any job with a trajectory — the default sits, the placeholder is only a brief local-read flicker,
  always replaced.

## Non-regression

A converged Opt **minimum** still defaults to its last (converged) frame; scan/NEB keep their refusal +
handoff; a genuine single point keeps its seed silently (final == input). The picker is **New-iteration
only** — the derived spawns (scan→OptTS, reopt, connectivity, F3) already read `results.final_geometry`
correctly and are untouched (extending the picker to them is the next unit).

## Tests (bites, `carryForward.test.ts`)

- `default_is_the_last_optimized_frame_not_the_seed` — `converged === null` still defaults to the last
  (optimized 2.2893) frame, not frame 0 / seed 2.3636 (the bug bite, real DA numbers).
- `non_converged_last_frame_is_labeled_not_stationary` — `false` → labeled, still selectable.
- `scan_or_neb_refuses_the_frame_picker` — scan/NEB/single-point refuse with the right `kind`.
- `every_frame_geometry_comes_from_the_trajectory` — no frame reconstructed from input.

## Related

- [021](021-input-vs-output-geometry-carry-forward.md) — the first half (carry the converged output);
  this closes the verdict-null hole it left.
- `wiki/modules/new-job-screen.md` — the frame picker + the unconditional default.
