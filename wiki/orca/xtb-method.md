# ORCA's internal `! XTB` = GFN2-xTB (bundled `otool_xtb`)

**Measured 2026-08-12 on ORCA 6.1.0** (rule #10 — recorded from a real run, not docs).

This page is about the method you request **from inside an ORCA input** with the simple keyword
`! XTB`. It is a *different thing* from `wiki/orca/xtb.md`, which documents the **standalone**
`xtb` 6.6.1 binary OrcaStudio shells out to for pre-optimization. Do not conflate them.

## What `! XTB` is

- `! XTB` on ORCA 6.1 selects **GFN2-xTB**. The output states it plainly:
  *"utilizes the semiempirical GFN2-xTB method."*
- ORCA runs it via its **bundled** driver `/opt/orca/otool_xtb` (reports **v6.7.1**) — NOT the
  standalone `xtb` 6.6.1 of `xtb.md`. Two different binaries, two different versions.
- **Self-contained.** GFN2-xTB carries its own (minimal) basis and parameterization. A `! XTB`
  line takes the method keyword + a job type and **nothing else** — no basis, dispersion, RI,
  SCFConv, or solvation keyword. `! XTB def2-TZVP SMD(water) TightSCF` is invalid.

## Only GFN2 is verified

This unit ships GFN2-xTB as `keyword: "XTB"` **only**. GFN1-xTB, GFN0-xTB, and GFN-FF are
**unverified** on this ORCA build and are deliberately NOT offered by the builder. Add them only
after a real run confirms the keyword and behaviour (rule #10).

## Job types produce the same artifacts as DFT

Probed with two real NEB runs (HCN ⇌ HNC):

- `! XTB NEB-TS` produced the **same artifact set** as a DFT NEB:
  `input.NEB.log`, `input.final.interp`, `input_MEP_trj.xyz` (10 frames, each `E <Eh>`),
  `input_NEB-TS_converged.xyz`.
- File **formats are identical** to the DFT path — only the energy *scale* differs (xTB energies
  are ≈ −5.5 Eh, not comparable to DFT totals). So the existing NEB/opt readers apply unchanged;
  the method affects the numbers, not the parsing.

## Scope of this probe

- **Gas phase only.** xTB solvation (ALPB/GBSA) was NOT tested here and is a later follow-up —
  it is intentionally absent from the builder (the emit suppresses the solvation tail for xtb).
- Confirmed job types so far: `NEB-TS` (identical artifacts to DFT). `Opt`, `Freq`, `SP` follow
  the same self-contained `! XTB <jobtype>` shape by construction; run-confirm before relying on
  any that isn't yet exercised.

## How the builder encodes this

`build-input.ts`: `methodFamily === "xtb"` pushes `state.xtbMethod` (`"XTB"`) only, then the
solvation and SCFConv tail is guarded off (`state.methodFamily !== "xtb"`); the job type is still
emitted. See `wiki/orca/input-format.md` Rule 0. Bite test: `xtb_line_is_method_and_jobtype_only`
in `build-input.test.ts` sets basis + SMD + TightSCF + RI + D4 and asserts the line is exactly
`XTB Opt`.

## Related

- `wiki/orca/xtb.md` — the **standalone** xtb 6.6.1 pre-opt binary (1-based `$constrain`, etc.).
- `wiki/orca/goat.md` — `! XTB GOAT` conformer search (also drives the internal xTB).
- `wiki/orca/neb.md` — NEB-TS core; the artifact formats referenced above.
- `wiki/orca/input-format.md` — Rule 0 (method families), Rule 2 (aux-basis pairing).
