# Solvation emit — which SMD syntax ORCA 6.1.0 actually honors (rule #10)

Determiner run settled on **2026-08-08** (Phase 4.5 Stage D unit D2a, before wiring the
DFT re-opt fan-out's SMD toggle). The question: does the app's existing keyword-line
emit `! … SMD(<solvent>)` produce **real SMD** (Cramer–Truhlar, with the CDS
non-electrostatic term), or does ORCA silently fall back to plain CPCM? Settled by a run,
not memory — the two candidate syntaxes and a CPCM control, one tiny SP each on a single
water molecule (neutral, singlet — isolates "which model" from any charge concern),
`/opt/orca/orca` full path, isolated dirs (domain rules #1/#3). **ORCA Program Version
6.1.0.**

## Forms tried

| # | Form | Input |
|---|---|---|
| 1 | keyword (what `build-input.ts` emits today) | `! r2SCAN-3c SMD(methanol) SP` |
| 2 | `%cpcm` block | `! r2SCAN-3c SP` + `%cpcm smd true SMDsolvent "methanol" end` |
| 3 | control (CPCM, known-good keyword) | `! r2SCAN-3c CPCM(methanol) SP` |

## What ORCA reported

All three **TERMINATED NORMALLY**. The tell for *real* SMD is the SMD-CDS
(non-electrostatic / cavity-dispersion-solvent-structure) term — CPCM alone has no CDS.

**Form 1 — keyword `SMD(methanol)` → REAL SMD.**
```
Your calculation utilizes the SMD solvation module
CPCM SOLVATION MODEL        Solvent: ... METHANOL
SMD-CDS solvent descriptors:
SMD CDS free energy correction energy :   1.09176   Kcal/mol
SMD CDS (Gcds)     :   0.00173983076740 Eh
FINAL SINGLE POINT ENERGY       -76.430993230852
```

**Form 2 — `%cpcm smd true SMDsolvent "methanol"` → REAL SMD, bit-identical to form 1.**
```
Your calculation utilizes the SMD solvation module
SMD CDS (Gcds)     :   0.00173983076740 Eh
FINAL SINGLE POINT ENERGY       -76.430993230852   ← identical to form 1
```

**Form 3 — control `CPCM(methanol)` → CPCM only, NO CDS term.**
```
CPCM SOLVATION MODEL        Solvent: ... METHANOL
(no "utilizes the SMD solvation module" line; no SMD-CDS block)
FINAL SINGLE POINT ENERGY       -76.428359276858   ← differs by ~2.6 mEh (the missing CDS)
```

## Verified conclusion (ORCA 6.1.0)

- **The keyword form `! … SMD(<solvent>)` IS honored as real SMD** — it prints "utilizes the
  SMD solvation module", carries the SMD-CDS Gcds term, and gives an energy **bit-identical**
  to the explicit `%cpcm smd true / SMDsolvent` block (`-76.430993230852 Eh`). The two SMD
  syntaxes are equivalent; there is **no silent CPCM fallback** for the keyword form.
- The CPCM control lacks the CDS term and lands at a different energy (`-76.428359…`),
  confirming CDS is what SMD adds on top of the CPCM electrostatics.
- **Therefore the existing `input-builder` `SMD(x)` keyword emit is correct as-is — no builder
  bug, no builder fix needed.** The DFT re-opt fan-out (D2a) reuses that same keyword emit via
  `buildOrcaInput`, so its SMD toggle produces verified real SMD.

The verified emit for ORCA 6.1: **`! <method> SMD(<solvent>) …`** (keyword form). The `%cpcm`
block form is equally valid should a future need arise (e.g. custom SMD descriptors the keyword
form can't express), but is not required for the standard solvent case.

Artifacts: `/tmp/smd-probe/{f1-keyword,f2-block,f3-cpcm}/output.out` (this run; scratch, not
committed). Related: `wiki/orca/crest.md` (ALPB/GBSA at the xTB level — a *different* solvation
tier; SMD-over-ALPB for ions is the later ORCA refinement noted there).
