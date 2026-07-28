# GOAT — conformer search (`! XTB GOAT`)

Domain knowledge gained **by running it**, not from memory (gotchas.md rule).
Source run: n-butane (C₄H₁₀, 14 atoms), **ORCA 6.1.0**, 2026-07-28, isolated job
dir, full-path `/opt/orca/orca`. GOAT = *Global Optimizer for Atomistic Topology*;
it explores the conformational space from one starting geometry and writes an
energy-sorted ensemble. Chemistry background: `wiki/chemistry/conformers.md`.

## What appears in the job directory

A GOAT run litters the dir (~40 files for butane): per-candidate scratch
(`<name>.goat.<block>.<iter>.*`), intermediate ensembles
(`<name>.finalensemble.globaliter.<N>.xyz`), and the usual ORCA files
(`.gbw`, `.out`, `.property.txt`, …). **Only two output files matter** for us:

- **`<name>.finalensemble.xyz`** — the conformer ensemble (multi-frame xyz).
- **`<name>.globalminimum.xyz`** — the single lowest-energy conformer.

Completion is the usual `****ORCA TERMINATED NORMALLY****` (domain rule #6).

## Comment-line format (verified — do not assume)

**Ensemble** (`finalensemble.xyz`), per structure:

```
14
-13.6651277570 converged=true
C   1.939391853645...  0.077374061521...  0.243041820526...
... (14 atom rows, ~20-decimal coordinates)
```

- The comment is **`<energy> converged=<bool>`**. Energy is the **leading
  whitespace token**, in **Hartree (Eh)**. **No structure index** in the comment.
- **Global minimum** (`globalminimum.xyz`) uses a *different* comment: just the
  **energy, no `converged=` flag**. (Its geometry == the ensemble's first frame.)

`parseEnsemble` (`src/scene/ensemble.ts`) reads the leading token as the energy and
leaves it `NaN` if it isn't a finite number — it never invents one.

## Count, ordering, and the "below 3 kcal/mol" gotcha

- n-butane → **5 structures in `finalensemble.xyz`**, **sorted ascending by
  energy** (global minimum first): −13.66513, −13.66418, −13.66104, −13.66041,
  −13.65725 Eh.
- **The file count ≠ the summary count.** The output log says *"Conformers below
  3 kcal/mol: 4"* — but the file holds **5** (the 5th sits at ~4.9 kcal/mol, above
  the 3 kcal/mol window). So trust the file, not the log summary, for how many
  structures you actually get. (Butane has 3 physical conformers — anti + two
  degenerate gauche; GOAT's xTB-level ensemble lists 5 before dedup/Boltzmann.)

## ⚠️ Atom order is PRESERVED — verified on every structure

**On the n-butane run, the element order in every one of the 5 ensemble
structures is identical to the input** (`C C C C H H H H H H H H H H`). This is
the load-bearing fact: `replaceFragmentAtoms` (ADR-008 index-space invariant)
throws if composition changes, so order-preservation means a chosen conformer can
be dropped straight back into a fragment **without any atom mapping**. If a future
ORCA version reorders atoms, 2.5.1b's substitution breaks — re-verify on upgrade.

## Cost and `%pal` — GOAT reacts strongly (queue-relevant)

GOAT runs **many independent optimizations**, so it is embarrassingly parallel —
and it **honours `%pal`**, but *across candidates*, not within one:

| run | wall time (ORCA `TOTAL RUN TIME`) |
|---|---|
| no `%pal` (1 core) | **4 min 20 s** |
| `%pal nprocs 4 end` | **1 min 13 s** (~3.5×) |

The GOAT iteration table shows **`NProcs 1` per iteration even with `%pal 4`**, and
iterations **complete out of order** (0, 2, 3, 1…) — i.e. `%pal N` runs N candidate
optimizations concurrently, each single-threaded. **Implication for OrcaStudio:**
GOAT is *slow* (minutes even for butane; much longer for real substrates) and the
queue runs concurrency = 1 (domain rule #4), so a GOAT job **blocks the queue for
its whole duration**. Give GOAT jobs a `%pal` (near-linear speedup) and treat them
as long-running — the UI (2.5.1b) must not imply it's instant.

## Charge and multiplicity

The neutral butane run used the plain **`* xyz 0 1`** header — **no GOAT-specific
keyword for charge**. The `* xyz charge mult` header is ORCA's universal charge
mechanism and xTB/GOAT read it, so a charged fragment (e.g. BH₄⁻, `* xyz -1 1`)
uses the same header (mechanism is standard; a charged-fragment GOAT run would be a
belt-and-braces confirmation, not yet done). `goatInputForFragment` uses
`fragment.charge` (GOAT runs on **one fragment in isolation**, so the fragment's
own charge, not the scene's `totalCharge`) and multiplicity 1 — safe for the
closed-shell library fragments (all even electron count); an open-shell fragment
would need it set.

## Not done here (2.5.1b / Phase 4.5)

Running GOAT from the app, substituting a conformer back into the scene (2.5.1b);
Boltzmann weighting + DFT re-optimisation of the lowest 3–4 (Phase 4.5). This page
covers only the observed primitive: run → ensemble file → parse.

## Run through the app's input (2.5.1b, same machine)

Re-ran GOAT on butane using the **exact `goatInputForFragment` format** the app
produces (`! XTB GOAT` + `* xyz 0 1` + rows), written as `input.inp` with
`%pal nprocs 4 end` (as the backend's `align_pal_nprocs` inserts at submit). ORCA
6.1.0, **TERMINATED NORMALLY, 1 min 13 s** (matches the earlier `%pal 4` run). The
ensemble landed at **`input.finalensemble.xyz`** — the path `read_job_ensemble`
reads. `parseEnsemble` + `deltaEKcal` on it gave **4 conformers**, ΔE **0.00 /
0.596 / 2.567 / 2.607 kcal/mol** (the 0.596 anti→gauche gap is chemically sensible
and matches the unit fixture), **atom order preserved** in all four. So the whole
path — app input → GOAT → ensemble file → parse → ΔE — is verified on real data.
(This run gave 4 structures; the 2.5.1a run gave 5 — GOAT's retained-count varies
slightly; the parser and count-from-file handle either.)
