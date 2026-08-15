# ORCA `OptTS` — transition-state optimization from a guess (rule #10)

Recorded from a **real run**, not the manual: the Menshutkin SN2 (methylamine + ethyl iodide,
DMF/SMD) TS, refined from the relaxed-scan maximum on **2026-08-09** (Phase 4.5 Stage E1a). Every
claim here is from that invocation; where the manual and the run would disagree, the run wins (domain
rule #10).

## What `OptTS` is

`! OptTS` runs a **saddle-point** optimization: instead of walking downhill to a minimum, it walks up
along one Hessian eigenvector (the mode with a negative eigenvalue) and downhill along all others,
converging on a **first-order saddle** — a transition state (exactly one imaginary frequency). It
needs a **good starting geometry** (a TS *guess*) and **curvature information** (a Hessian) to know
which mode to maximize along. It is the refinement step after a TS guess is in hand — from a relaxed
scan (today), a NEB climbing image (Stage E3), or a hand guess.

## The recipe that worked (measured)

```
! r2SCAN-3c OptTS Freq SMD(DMF) TightSCF
%geom Calc_Hess true end
* xyz 0 1
  <seed = the approximate-TS scan point, N···C 2.418 Å>
*
```

- **`Calc_Hess true` ALONE sufficed** — a single exact Hessian computed at the start. **No
  `Recalc_Hess`** was needed on this clean 1-D coordinate. (`Recalc_Hess N` recomputes the Hessian
  every N steps; it is the fallback for a harder surface, not needed here.)
- **`Freq` in the same job** is what makes the result *honest*: it confirms the located stationary
  point is a TS (exactly one imaginary mode), not a minimum or a higher-order saddle. An OptTS
  without Freq is an unverified claim — always pair them.
- **`TightSCF`** as for any barrier work (energy differences need converged SCF).
- Method + solvation **inherited from the source job**, verbatim — the TS must be on the SAME surface
  as the scan that produced its guess, or the barrier is not comparable
  (`chemistry/reaction-barriers.md`; the same discipline `compare.ts`'s comparability guard enforces).

## The result (measured)

- **Converged, `HURRAY`, `ORCA TERMINATED NORMALLY`** in **3 min 8 s**, 15 atoms, on the dev machine.
- **Exactly ONE imaginary frequency: −385.31 cm⁻¹** — agreeing between `output.out` and `input.hess`
  (the two sources cross-checked). That mode IS the SN2 reaction coordinate: **N···C forming (2.353
  Å) while C···I breaks (2.592 Å)**, backside angle **161°**. A textbook concerted-backside SN2
  saddle.
- The verdict "**transition state**" is earned by the one-imaginary count, not asserted.

## Why the relaxed-scan maximum was an EXCELLENT seed here — and when it will NOT be

- On this reaction the scan maximum was **≈ the saddle**: from seed to converged TS the N···C
  distance moved only **0.065 Å** (2.418 → 2.353), and the seed's computed Hessian already carried
  **one imaginary mode**. A clean, essentially **1-D** reaction coordinate (one bond forms as one
  breaks, collinearly) is exactly the case where "click the scan maximum → OptTS" lands immediately.
- **The contrast that motivates NEB (Stage E3):** a **concerted** reaction with **no single clean
  coordinate** — e.g. BH₄⁻ hydride transfer, where a C–H bond forms *while* B–H and O–B bonds break
  in a coupled way — has a scan maximum that is **NOT** the barrier (scanning one distance drags the
  system along the wrong path). There, NEB/NEB-CI finds the true saddle, and **OptTS then refines
  THAT climbing image** — through the *same* engine (`buildOptTSInput`), only a different entry point
  supplies the guess. This is why the refine engine is **source-agnostic** (ADR-020).
- **When the seed is poor, the tool now says so.** A Diels-Alder OptTS seeded from an **XTB** scan
  point exhausted its cycle budget without converging — and because ORCA still printed `TERMINATED
  NORMALLY`, it read as a clean COMPLETED (with a confusing `.hess` mismatch, itself a *consequence*
  of the moved geometry). The **convergence-status guard** (`orca/convergence-status.md`) surfaces
  this honestly: a non-converged OptTS reads "did not converge (max cycles)", frequencies are
  suppressed, and the `.hess` mismatch is no longer a parse error. The guard makes the status honest;
  *why* the XTB seed is too poor for the r2SCAN-3c saddle (seed quality / method) is the separate
  chemistry question it unblocks.

## Cost

r2SCAN-3c OptTS+Freq with `Calc_Hess true`, 15 atoms, DMF/SMD: **≈ 3 min** wall on the dev machine.
The up-front exact Hessian dominates a small-molecule TS; it is the price of knowing which mode to
climb.

## Emit pattern — REUSE, no golden pair (Fork 2)

The OptTS input carries **no atom-index-bearing directive** (unlike a `%geom Scan`/`Constraints`
block, whose 0-based indices must survive byte-for-byte and so earn an `orcastudio-core` Rust golden
pair). `Calc_Hess true` is a fixed flag; method/solvation/charge are inherited text. So
`buildOptTSInput` **REUSES `buildOrcaInput`** (like `buildReoptInput`) and owes **no** cross-language
byte-identical mirror — settled empirically, the Fork-2 branch of the emit-ownership decision
(`adr-016`). The source's opt keyword (e.g. the scan's `LooseOpt`) and its `Scan`/`Constraints`
`%geom` block must **not** leak into the OptTS input: `buildOrcaInput` builds the `!` line and body
**fresh** from `BuilderState` + the seed geometry, so nothing carries over except what we choose
(method + solvation + charge).

### Boundary of the method+solvation inheritance claim (Pattern-2)

Method + solvation are inherited by reading the source's **`!` keyword line** (`methodSolvationKeywords`
in `src/reactions/compare.ts` — the same `!`-line reader the comparability guard uses, so the two
cannot drift). This covers the **inline keyword form** `SMD(<solvent>)` — which is exactly what the app
emits (`orca/solvation.md`) and what is tested. It does **NOT** cover a source that states solvation as
a **`%cpcm … smd true … end` block** with a separate `SMDsolvent "<name>"`: such a source would re-emit
`SMD` **without the solvent name**. No source the app writes today uses the block form, so this is a
non-issue now — but it is a **named limit**, not a silent assumption: when a NEB / hand-authored source
(Stage E3) can carry a `%cpcm` block, `buildOptTSInput` must be extended to read it (or the entry point
must pass `options.solvation` explicitly). Recorded here so the gap is visible before that caller exists.

### Method override — the `methodState` seam (default = inherit)

Inheriting the source method is the DEFAULT, but a refine can now run at a **chosen** level:
`OptTSOptions.methodState?: MethodSlice` (the [`<MethodPicker>`](../modules/method-picker.md) family
model). This exists to fix the **XTB-scan → XTB-OptTS** trap: a scan run at semi-empirical GFN2-xTB
inherited an XTB OptTS, which is not publication-grade. Now the researcher can refine the XTB Diels-
Alder seed at DFT.

- **`methodState` present** → the child `state` is `{...DEFAULT_BUILDER_STATE, ...methodState}` and the
  `!` line is built through `buildOrcaInput`'s **family logic**. This is the crux: a DFT override
  carries functional + basis + the **paired** RI aux (`def2/J`) + dispersion, because the pairing
  lives in the `dft` branch. It is **NOT** flattened into the composite string — a flatten would drop
  the aux and emit a mismatched `!` line (the MAIN RISK; a `dft_override_pairs_ri_aux` bite asserts
  `def2/J` is present, red on a flatten impl).
- **Solvation still inherits from the source** under an override (comparability), carried via
  `splitSolvation("SMD(DMF)") → {solvationModel, solvent}` so `buildKeywordLine` applies the per-family
  rule (emitted for dft/composite/wf, suppressed for xtb). This is a solvation-**token** split, not a
  reverse-parse of the method family.
- **`methodState` absent** → the ORIGINAL composite-string path, **byte-identical** to before the seam
  existed (an `inherit_default_is_byte_identical` snapshot bite pins it three ways: `{}`,
  `{methodState: undefined}`, no-arg). Charge/mult inheritance + the rule-#9 post-condition run after
  the fork, so they are unchanged on either path.

**"Inherit" = pass nothing.** The UI default at the refine sites is "Inherit from source", which sends
`{}` — NOT a slice reconstructed to "equal" the source (that would be a back-door reverse-parse of the
`!` line and could drift from this byte-identical path). When the source is XTB, the picker shows an
inline note nudging a DFT level (`sourceMethodIsXtb`, a UI hint only — never touches the emit).

## See also

- `chemistry/reaction-barriers.md` — ΔE‡ (relaxed-scan estimate) vs ΔG‡ (a located TS + Freq +
  thermochemistry); OptTS+Freq is the first half of the ΔG‡ path.
- `architecture/adr-007-*` §"ΔE‡ vs ΔG‡" — the scan maximum is an *estimate*; the located TS is the
  real saddle.
- `architecture/adr-020-optts-refinement-source-agnostic.md` — why the refine engine takes a generic
  seed from any source (scan max now, NEB image in E3).
- `orca/scan.md` — the relaxed scan that produces the guess; `orca/solvation.md` — that `SMD(<solvent>)`
  is real SMD (the solvation inherited here).
